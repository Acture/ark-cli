use super::bundle::{self, Reader};
use super::serialized::{self, File, Object, Script};
use super::{bounded_join, pin, read_json, sha256, stage, text, write_json};
use anyhow::{ensure, Context, Result};
use md5::{Digest, Md5};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

// Guards against runaway dependency expansion, not game limits. Operators with many skins and
// effects (for example Wiš'adel with Logos on 1-7) need about 150 bundles.
const MAX_CLOSURE_BUNDLES: usize = 512;
const MAX_CLOSURE_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug)]
pub struct Catalog {
	pub assets: BTreeMap<String, String>,
	pub paths: BTreeMap<String, String>,
	pub version_id: String,
	pub(crate) state: PathBuf,
	pub(crate) client: PathBuf,
	pub(crate) data: PathBuf,
	bundles: Vec<String>,
	dependencies: Vec<Vec<usize>>,
	entries: BTreeMap<String, Value>,
	hot: PathBuf,
	base: PathBuf,
	provenance: Value,
}

impl Catalog {
	pub fn new(state: &Path, client: &Path) -> Result<Self> {
		let state: PathBuf = crate::io::resolved_destination(state)?;
		let client: PathBuf = crate::io::resolved_destination(client)?;
		ensure!(
			client.join("GameAssembly.dll").is_file(),
			"Original client lacks GameAssembly.dll"
		);
		ensure!(
			!state.starts_with(&client) && !client.starts_with(&state),
			"Owned state and original client must be separate directories"
		);
		fs::create_dir_all(&state)?;
		let data: PathBuf = client.join("Arknights_Data");
		let hot: PathBuf = data.join("PersistentData/Bundles");
		let base: PathBuf = data.join("StreamingAssets/AB/Windows");
		let list_root: &Path = if hot.join("hot_update_list.json").is_file() {
			&hot
		} else {
			&base
		};
		let list_path: PathBuf = list_root.join("hot_update_list.json");
		let listing: Value = read_json(&list_path)?;
		ensure!(
			text(&listing, "manifestVersion")? == "V077",
			"Unreviewed original manifest version"
		);
		let version_id: String = text(&listing, "versionId")?.to_owned();
		ensure!(
			version_id.len() <= 128
				&& version_id
					.bytes()
					.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'),
			"Unsafe manifest version identifier"
		);
		let mut entries: BTreeMap<String, Value> = BTreeMap::new();
		for entry in listing["abInfos"]
			.as_array()
			.context("Missing resource-list records")?
		{
			let name: String = text(entry, "name")?.to_owned();
			bounded_join(list_root, &name)?;
			ensure!(
				entries.insert(name, entry.clone()).is_none(),
				"Duplicate resource-list name"
			);
		}
		let manifest: PathBuf = bounded_join(list_root, text(&listing, "manifestName")?)?;
		let raw: Vec<u8> = fs::read(&manifest)?;
		let manifest_name: &str = manifest
			.file_name()
			.and_then(|s| s.to_str())
			.context("Invalid manifest filename")?;
		check_manifest(
			entries
				.get(manifest_name)
				.context("Manifest absent from resource list")?,
			&raw,
			manifest_name,
		)?;
		let fb: Flat<'_> = Flat { data: &raw };
		let root: BTreeMap<usize, usize> = fb.fields(
			128_usize
				.checked_add(fb.u32(128)? as usize)
				.context("Root offset overflow")?,
		)?;
		ensure!(
			root.keys().copied().collect::<Vec<usize>>() == [1, 2],
			"Original manifest root shape changed"
		);
		let mut bundles: Vec<String> = Vec::new();
		let mut dependencies: Vec<Vec<usize>> = Vec::new();
		for item in fb.tables(root[&1], 40_000)? {
			ensure!(
				item.contains_key(&0) && item.keys().all(|key| *key <= 3),
				"Original bundle metadata shape changed"
			);
			let name: String = fb.string(item[&0])?;
			bounded_join(list_root, &name)?;
			bundles.push(name);
			dependencies.push(if let Some(field) = item.get(&3) {
				fb.vector(*field, 40_000)?
					.into_iter()
					.map(|n| n as usize)
					.collect()
			} else {
				Vec::new()
			});
		}
		ensure!(
			bundles.iter().collect::<BTreeSet<_>>().len() == bundles.len(),
			"Duplicate manifest bundle name"
		);
		ensure!(
			dependencies
				.iter()
				.flatten()
				.all(|index| *index < bundles.len()),
			"Invalid manifest dependency index"
		);
		let mut assets: BTreeMap<String, String> = BTreeMap::new();
		let mut paths: BTreeMap<String, String> = BTreeMap::new();
		for item in fb.tables(root[&2], 400_000)? {
			ensure!(
				item.contains_key(&0) && item.keys().all(|key| *key <= 3),
				"Original asset metadata shape changed"
			);
			let name: String = fb.string(item[&0])?;
			let index: usize = if let Some(field) = item.get(&1) {
				fb.u32(*field)? as usize
			} else {
				0
			};
			ensure!(
				index < bundles.len() && !assets.contains_key(&name),
				"Invalid or duplicate original asset route"
			);
			assets.insert(name.clone(), bundles[index].clone());
			paths.insert(
				name,
				if let Some(field) = item.get(&3) {
					fb.string(*field)?
				} else {
					String::new()
				},
			);
		}
		let provenance: Value = json!({"list": list_path, "list_sha256": sha256(&fs::read(&list_path)?), "version_id": version_id, "manifest": manifest, "manifest_sha256": sha256(&raw), "bundle_count": bundles.len(), "asset_count": assets.len(), "resource_processor": "ark-unity-rust-v1"});
		Ok(Self {
			state,
			client,
			data,
			hot,
			base,
			entries,
			bundles,
			dependencies,
			assets,
			paths,
			version_id,
			provenance,
		})
	}

	pub fn provenance(&self) -> Value {
		self.provenance.clone()
	}
	pub fn installed(&self, bundle: &str) -> Result<bool> {
		Ok(self.candidate(bundle)?.is_some())
	}
	fn candidate(&self, name: &str) -> Result<Option<(&'static str, PathBuf)>> {
		for (origin, root) in [("hot", &self.hot), ("base", &self.base)] {
			let path: PathBuf = bounded_join(root, name)?;
			if path.is_file() {
				return Ok(Some((origin, path)));
			}
		}
		Ok(None)
	}
	pub fn file(&self, bundle: &str) -> Result<PathBuf> {
		let entry: &Value = self
			.entries
			.get(bundle)
			.with_context(|| format!("Bundle absent from current resource list: {bundle}"))?;
		let (_, path): (&str, PathBuf) = self
			.candidate(bundle)?
			.with_context(|| format!("Listed original bundle is not installed: {bundle}"))?;
		check_entry(entry, &fs::read(&path)?, bundle)?;
		Ok(path)
	}
	pub fn describe(&self, bundle: &str) -> Result<Value> {
		let path: PathBuf = self.file(bundle)?;
		Ok(
			json!({"bundle": bundle, "origin": self.candidate(bundle)?.context("Bundle disappeared")?.0, "path": path, "list_check": if text(&self.entries[bundle], "md5")?.len() == 32 { "md5" } else { "size_only_metadata_tag" }}),
		)
	}
	pub fn closure(&self, roots: &BTreeSet<String>) -> Result<BTreeSet<String>> {
		let indexes: BTreeMap<&str, usize> = self
			.bundles
			.iter()
			.enumerate()
			.map(|(i, name)| (name.as_str(), i))
			.collect();
		let mut result: BTreeSet<String> = roots.clone();
		let mut queue: Vec<String> = roots.iter().cloned().collect();
		while let Some(name) = queue.pop() {
			let index: usize = *indexes
				.get(name.as_str())
				.with_context(|| format!("Dependency root absent from manifest: {name}"))?;
			for index in &self.dependencies[index] {
				let name: &String = &self.bundles[*index];
				if result.insert(name.clone()) {
					queue.push(name.clone());
				}
			}
		}
		Ok(result)
	}

	pub fn prepare_level(&self, level_id: &str) -> Result<PathBuf> {
		let started: Instant = Instant::now();
		eprintln!("INFO Preparing original level {level_id}");
		let asset: String = format!("gamedata/levels/{level_id}");
		let bundle: &str = self
			.assets
			.get(&asset)
			.context("Original level absent from current catalog")?;
		let container: &str = self.paths.get(&asset).context("Missing level container")?;
		ensure!(
			container == format!("dyn/{asset}.bytes"),
			"Original level container identity changed"
		);
		let source: PathBuf = self.file(bundle)?;
		let (level_file, level_id): (File, i64) = container_object(&source, container)?;
		let obj: &Object = level_file.object(level_id)?;
		ensure!(
			level_file.class(obj) == 49,
			"Level is not an original TextAsset"
		);
		let (name, level_bytes): (String, Vec<u8>) = serialized::text_asset(&obj.raw)?;
		let options_source: PathBuf = self.data.join("sharedassets0.assets");
		let options_file: File = File::parse(&fs::read(&options_source)?)?;
		let options: &Object = options_file.object(27)?;
		ensure!(
			options_file.class(options) == 114,
			"GlobalOptions source class changed"
		);
		let (options_name, script, _): (String, (i32, i64), (i32, i64)) =
			serialized::mono_head(&options.raw)?;
		ensure!(
			options_name == "GlobalOptions" && script.0 == 1,
			"GlobalOptions identity or script file changed"
		);
		let mapper: PathBuf = self.data.join("globalgamemanagers.assets");
		let scripts: File = File::parse(&fs::read(&mapper)?)?;
		let identity: Script = serialized::mono_script(&scripts.object(script.1)?.raw)?;
		ensure!(
			(
				identity.namespace.as_str(),
				identity.class.as_str(),
				identity.assembly.as_str()
			) == ("Torappu", "GlobalOptions", "Torappu.Common.dll"),
			"GlobalOptions script identity changed"
		);
		let mut r: Reader<'_> = Reader::new(&options.raw, false);
		r.pos = 28;
		r.string()?;
		for _ in 0..3 {
			r.string()?;
		}
		let position: usize = r.pos;
		let file_id: i32 = r.i32()?;
		let key_id: i64 = r.i64()?;
		ensure!(file_id == 0, "Verification key reference is not local");
		let key: &Object = options_file.object(key_id)?;
		ensure!(
			options_file.class(key) == 49,
			"Verification key is not a TextAsset"
		);
		let (key_name, payload): (String, Vec<u8>) = serialized::text_asset(&key.raw)?;
		ensure!(
			!payload.is_empty() && payload.len() <= 8192,
			"Verification key exceeds bounded shape"
		);
		let output: PathBuf = stage(&self.state, "levels")?;
		let level_path: PathBuf = output.join("original-level.bytes");
		let key_path: PathBuf = output.join("original-verification-public-key.txt");
		fs::write(&level_path, &level_bytes)?;
		fs::write(&key_path, &payload)?;
		let mut level_entry: Value = pin(&level_path)?;
		let fields: &mut serde_json::Map<String, Value> =
			level_entry.as_object_mut().context("Invalid pin")?;
		fields.extend(json!({"name": name, "container": container, "source": self.describe(bundle)?, "path_id": obj.id}).as_object().context("Invalid record")?.clone());
		let mut key_entry: Value = pin(&key_path)?;
		key_entry.as_object_mut().context("Invalid pin")?.extend(json!({"source": pin(&options_source)?, "options_path_id": options.id, "script_mapper": pin(&mapper)?, "script_path_id": script.1, "field": "cryptoPubKey", "serialized_field_position": position, "text_asset_path_id": key_id, "text_asset_name": key_name}).as_object().context("Invalid record")?.clone());
		let result: PathBuf = output.join("inputs.json");
		write_json(
			&result,
			&json!({"schema": "ark-battle-level-inputs-v2", "level_id": asset.strip_prefix("gamedata/levels/").context("Invalid level asset")?, "level": level_entry, "verification_key": key_entry, "catalog": self.provenance(), "level_sha256": sha256(&level_bytes)}),
		)?;
		eprintln!(
			"INFO Original level prepared: {} bytes in {:.2}s",
			level_bytes.len(),
			started.elapsed().as_secs_f64()
		);
		Ok(result)
	}

	pub fn prepare_scene_services(
		&self,
		player: &Path,
		level_inputs: &Path,
		decoded_level: &Value,
		squad: &Value,
		extra_routes: &[String],
	) -> Result<(PathBuf, PathBuf, PathBuf)> {
		let level: Value = read_json(level_inputs)?;
		let plan: Value = self.resource_plan(
			text(&level, "level_id")?,
			decoded_level,
			squad,
			extra_routes,
		)?;
		let plan_path: PathBuf = stage(&self.state, "plans")?.join("plan.json");
		write_json(&plan_path, &plan)?;
		let scene_path: PathBuf = self.prepare_scene(&plan)?;
		let services_path: PathBuf =
			super::services::prepare(self, player, &scene_path, level_inputs)?;
		Ok((scene_path, services_path, plan_path))
	}

	pub fn resource_plan(
		&self,
		level_id: &str,
		decoded: &Value,
		squad: &Value,
		extra: &[String],
	) -> Result<Value> {
		let name: &str = level_id.rsplit('/').next().context("Empty level ID")?;
		let scene_key: String = format!("scenes/{level_id}/{name}");
		let scene: &String = self
			.assets
			.get(&scene_key)
			.context("Original catalog has no scene for level")?;
		let framework: [&str; 6] = [
			"battle/prefabs/[uc]skills.ab",
			"battle/prefabs/[uc]projectiles.ab",
			"battle/prefabs/effects/common.ab",
			"battle/prefabs/effects/tile.ab",
			"battle/prefabs/effects/screen.ab",
			"battle/prefabs/effects/lazerd.ab",
		];
		let mut selected: BTreeMap<String, String> = BTreeMap::new();
		for key in extra {
			selected.insert(
				key.clone(),
				self.assets
					.get(key)
					.with_context(|| format!("Missing explicit resource route {key}"))?
					.clone(),
			);
		}
		for (key, bundle) in &self.assets {
			// The client's Lua scripts, including code hotfixes, load through these routes.
			if framework.contains(&bundle.as_str()) || key.starts_with("gamedata/[uc]lua/") {
				selected.insert(key.clone(), bundle.clone());
			}
		}
		let operators: &[Value] = squad["operators"]
			.as_array()
			.context("Squad has no operators")?;
		let mut identifiers: BTreeSet<String> = BTreeSet::new();
		for operator in operators {
			identifiers.insert(text(operator, "character_id")?.to_owned());
		}
		for entry in decoded["enemyDbRefs"].as_array().into_iter().flatten() {
			identifiers.insert(text(entry, "id")?.to_owned());
		}
		for group in [
			"characterInsts",
			"tokenInsts",
			"characterCards",
			"tokenCards",
		] {
			for entry in decoded["predefines"][group]
				.as_array()
				.into_iter()
				.flatten()
			{
				identifiers.insert(text(&entry["inst"], "characterKey")?.to_owned());
			}
		}
		let prefabs: Vec<&String> = self
			.assets
			.keys()
			.filter(|key| key.starts_with("battle/prefabs/"))
			.collect();
		for identifier in identifiers {
			let mut matches: Vec<&String> = prefabs
				.iter()
				.copied()
				.filter(|key| key.rsplit('/').next() == Some(identifier.as_str()))
				.collect();
			if matches.is_empty() {
				if let Some((base, suffix)) = identifier.rsplit_once('_') {
					if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
						matches = prefabs
							.iter()
							.copied()
							.filter(|key| key.rsplit('/').next() == Some(base))
							.collect();
					}
				}
			}
			ensure!(
				!matches.is_empty(),
				"No original prefab candidate for {identifier}"
			);
			for key in matches {
				selected.insert(key.clone(), self.assets[key].clone());
			}
			let stem: &str = identifier
				.splitn(3, '_')
				.last()
				.context("Empty resource identifier")?;
			let stem: &str = stem
				.rsplit_once('_')
				.filter(|(_, s)| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
				.map_or(stem, |(base, _)| base);
			for key in &prefabs {
				if key.starts_with(&format!("battle/prefabs/effects/{stem}_")) {
					selected.insert((*key).clone(), self.assets[*key].clone());
				}
			}
		}
		for operator in operators {
			let skin: &str = operator
				.get("skin_id")
				.and_then(Value::as_str)
				.filter(|s| !s.is_empty())
				.unwrap_or("defaultskin");
			let key: String = format!(
				"battle/prefabs/skins/character/{}/{skin}",
				text(operator, "character_id")?
			);
			selected.insert(
				key.clone(),
				self.assets
					.get(&key)
					.with_context(|| format!("Missing original skin route {key}"))?
					.clone(),
			);
		}
		let closure: BTreeSet<String> = self.closure(&selected.values().cloned().collect())?;
		ensure!(
			closure.len() <= MAX_CLOSURE_BUNDLES,
			"Resource request exceeds bounded preparation"
		);
		Ok(
			json!({"schema": "ark-battle-resource-plan-v2", "level_id": level_id, "scene_bundle": scene, "bundles": closure, "selected_routes": selected, "extra_routes": extra.iter().collect::<BTreeSet<_>>(), "dynamic_resource_completeness_certified": false, "limitation": "String-selected prefabs may request additional resources; original runtime must verify them."}),
		)
	}

	fn cab_index(&self) -> Result<BTreeMap<String, Vec<String>>> {
		let output: PathBuf = crate::io::owned(&self.state, &self.state.join("cab-index"))?;
		fs::create_dir_all(&output)?;
		let path: PathBuf = crate::io::owned(
			&self.state,
			&output.join(format!("{}.json", self.version_id)),
		)?;
		if path.is_file() {
			let cache: Value = read_json(&path)?;
			if cache["list_sha256"] == self.provenance["list_sha256"] {
				return Ok(serde_json::from_value(cache["cabs"].clone())?);
			}
		}
		let mut cabs: BTreeMap<String, Vec<String>> = BTreeMap::new();
		let mut errors: Vec<Value> = Vec::new();
		for name in &self.bundles {
			if !(name.ends_with(".ab") || name.ends_with(".bin")) {
				continue;
			}
			if let Some((_, source)) = self.candidate(name)? {
				match bundle::names(&source) {
					Ok(nodes) => {
						for node in nodes {
							cabs.entry(node.to_lowercase())
								.or_default()
								.push(name.clone());
						}
					}
					Err(error) => {
						errors.push(json!({"bundle": name, "error": format!("{error:#}")}))
					}
				}
			}
		}
		write_json(
			&path,
			&json!({"list_sha256": self.provenance["list_sha256"], "version_id": self.version_id, "errors": errors, "cabs": cabs}),
		)?;
		Ok(cabs)
	}

	pub fn prepare_scene(&self, plan: &Value) -> Result<PathBuf> {
		eprintln!("INFO Preparing original scene and CAB closure");
		let cabs: BTreeMap<String, Vec<String>> = self.cab_index()?;
		let scene_bundle: &str = text(plan, "scene_bundle")?;
		ensure!(
			scene_bundle.starts_with("scenes/") && scene_bundle.ends_with(".ab"),
			"Scene must be an original scene bundle"
		);
		let output: PathBuf = stage(&self.state, "scenes")?;
		let started: Instant = Instant::now();
		let mut queue: VecDeque<String> = VecDeque::from([scene_bundle.to_owned()]);
		for name in plan["bundles"]
			.as_array()
			.context("Missing resource-plan bundles")?
		{
			queue.push_back(name.as_str().context("Invalid bundle name")?.to_owned());
		}
		let mut seen: BTreeSet<String> = BTreeSet::new();
		let mut total: u64 = 0;
		let mut records: Vec<Value> = Vec::new();
		let mut builtins: BTreeSet<String> = BTreeSet::new();
		let mut scripts: BTreeMap<(String, i64), Script> = BTreeMap::new();
		let mut behaviours: Vec<(String, String, i64, String, i64)> = Vec::new();
		let mut nulls: Vec<Value> = Vec::new();
		while let Some(name) = queue.pop_front() {
			if !seen.insert(name.clone()) {
				continue;
			}
			let source: PathBuf = self.file(&name)?;
			total = total
				.checked_add(fs::metadata(&source)?.len())
				.context("Scene source size overflow")?;
			ensure!(
				seen.len() <= MAX_CLOSURE_BUNDLES
					&& total <= MAX_CLOSURE_SOURCE_BYTES
					&& started.elapsed().as_secs() < 180,
				"Scene closure exceeds bounded preparation"
			);
			let entries: Vec<bundle::Entry> =
				bundle::load(&source).with_context(|| format!("Read bundle {name}"))?;
			let owners: BTreeSet<String> = entries
				.iter()
				.map(|entry| entry.name.to_lowercase())
				.collect();
			let mut externals: BTreeSet<String> = BTreeSet::new();
			for entry in &entries {
				if entry.flags & 4 == 0 {
					continue;
				}
				let file: File = File::parse(&entry.bytes)
					.with_context(|| format!("Read CAB {} in {name}", entry.name))?;
				let owner: String = entry.name.to_lowercase();
				for external in &file.externals {
					let cab: String = external_name(&external.path).to_lowercase();
					if owners.contains(&cab) {
						continue;
					}
					if ["unity default resources", "unity_builtin_extra"].contains(&cab.as_str()) {
						builtins.insert(external.path.clone());
						continue;
					}
					externals.insert(cab.clone());
					let choices: &Vec<String> = cabs.get(&cab).with_context(|| {
						format!("Missing external CAB {cab} required by {name}")
					})?;
					ensure!(
						choices.len() == 1,
						"Ambiguous external CAB {cab}: {choices:?}"
					);
					queue.push_back(choices[0].clone());
				}
				for obj in &file.objects {
					match file.class(obj) {
						115 => {
							scripts.insert(
								(owner.clone(), obj.id),
								serialized::mono_script(&obj.raw)?,
							);
						}
						114 => {
							let (_, script, _) = serialized::mono_head(&obj.raw)?;
							if script == (0, 0) {
								nulls.push(json!({"bundle": name, "path_id": obj.id}));
								continue;
							}
							let script_owner: String = if script.0 == 0 {
								owner.clone()
							} else {
								external_name(
									&file
										.externals
										.get(usize::try_from(script.0 - 1)?)
										.context("Invalid script external")?
										.path,
								)
								.to_lowercase()
							};
							behaviours.push((
								name.clone(),
								owner.clone(),
								obj.id,
								script_owner,
								script.1,
							));
						}
						_ => {}
					}
				}
			}
			let repacked: Vec<u8> = bundle::save(&entries)?;
			let target: PathBuf = output.join(format!("bundle-{:03}.ab", records.len()));
			fs::write(&target, &repacked)?;
			ensure!(
				bundle::load(&target)? == entries,
				"Repacking changed original serialized bytes: {name}"
			);
			let mut record: Value = self.describe(&name)?;
			record.as_object_mut().context("Invalid bundle record")?.extend(json!({"source_sha256": sha256(&fs::read(&source)?), "source_bytes": fs::metadata(&source)?.len(), "path": target, "sha256": sha256(&repacked), "bytes": repacked.len(), "cab_files": entries.iter().map(|entry| json!({"name": entry.name, "sha256": sha256(&entry.bytes), "bytes": entry.bytes.len()})).collect::<Vec<_>>(), "externals": externals, "serialized_bytes_identical": true, "scene_bundle": name == scene_bundle}).as_object().context("Invalid bundle record")?.clone());
			records.push(record);
		}
		let assembly_source: PathBuf = self.data.join("ScriptingAssemblies.json");
		let assembly: Value = read_json(&assembly_source)?;
		let names: &[Value] = assembly["names"]
			.as_array()
			.context("Missing assembly names")?;
		let types: &[Value] = assembly["types"]
			.as_array()
			.context("Missing assembly types")?;
		ensure!(
			names.len() == types.len(),
			"Assembly manifest lengths differ"
		);
		let assembly_types: BTreeMap<&str, &Value> = names
			.iter()
			.zip(types)
			.map(|(name, kind)| Ok((name.as_str().context("Invalid assembly name")?, kind)))
			.collect::<Result<_>>()?;
		let mut used: BTreeSet<String> = BTreeSet::new();
		let mut counts: BTreeMap<String, usize> = BTreeMap::new();
		let mut framework: Vec<Value> = Vec::new();
		for (name, owner, id, script_owner, script_id) in behaviours {
			let script: &Script = scripts
				.get(&(script_owner.clone(), script_id))
				.with_context(|| {
					format!("Missing original script identity {script_owner}:{script_id} for {name}:{id}")
				})?;
			ensure!(
				assembly_types.contains_key(script.assembly.as_str()),
				"Serialized script assembly absent from original manifest"
			);
			used.insert(script.assembly.clone());
			if name == scene_bundle {
				*counts.entry(script.class.clone()).or_default() += 1;
				if [
					"BattleController",
					"Map",
					"Scheduler",
					"BattleFactory",
					"BattleLauncher",
				]
				.contains(&script.class.as_str())
				{
					let entries: Vec<bundle::Entry> = bundle::load(&self.file(&name)?)?;
					let entry: &bundle::Entry = entries
						.iter()
						.find(|entry| entry.name.to_lowercase() == owner)
						.context("Framework CAB absent")?;
					let file: File = File::parse(&entry.bytes)?;
					framework.push(json!({"class": script.class, "path_id": id, "cab": entry.name, "fields": file.decode(file.object(id)?)?}));
				}
			}
		}
		let mut order: Vec<Value> = records
			.iter()
			.filter(|r| r["scene_bundle"] == false)
			.map(|r| r["path"].clone())
			.collect();
		order.extend(
			records
				.iter()
				.filter(|r| r["scene_bundle"] == true)
				.map(|r| r["path"].clone()),
		);
		fs::write(
			output.join("load-order.txt"),
			order
				.iter()
				.map(|p| p.as_str().context("Invalid load order"))
				.collect::<Result<Vec<_>>>()?
				.join("\n") + "\n",
		)?;
		let path: PathBuf = output.join("inputs.json");
		write_json(
			&path,
			&json!({"schema": "ark-battle-scene-inputs-v2", "catalog": self.provenance(), "scene_bundle": scene_bundle, "bundles": records, "unresolved": [], "builtins": builtins, "framework": framework, "missing_script_identifiers": [], "source_null_scripts": nulls, "complete": true, "scene_component_counts": counts, "script_assemblies": used.iter().map(|name| json!({"name": name, "type": assembly_types[name.as_str()]})).collect::<Vec<_>>(), "assembly_source": pin(&assembly_source)?, "source_bytes": total, "elapsed_seconds": started.elapsed().as_secs_f64(), "load_order": order}),
		)?;
		eprintln!(
			"INFO Original scene complete: {} bundles, {:.1} MiB in {:.2}s",
			records.len(),
			total as f64 / 1_048_576.0,
			started.elapsed().as_secs_f64()
		);
		Ok(path)
	}
}

pub(crate) fn external_name(path: &str) -> String {
	path.replace('\\', "/")
		.rsplit('/')
		.next()
		.unwrap_or("")
		.to_owned()
}

pub(crate) fn container_object(path: &Path, container: &str) -> Result<(File, i64)> {
	let mut found: Option<(File, i64)> = None;
	for entry in bundle::load(path)? {
		if entry.flags & 4 == 0 {
			continue;
		}
		let file: File = File::parse(&entry.bytes)?;
		let mut targets: Vec<i64> = Vec::new();
		for object in file.objects.iter().filter(|obj| file.class(obj) == 142) {
			for pair in file.decode(object)?["m_Container"]
				.as_array()
				.context("AssetBundle container is not an array")?
			{
				if pair[0].as_str() == Some(container) {
					let pointer: (i32, i64) = serialized::pointer(&pair[1]["asset"])?;
					ensure!(
						pointer.0 == 0 && file.object(pointer.1).is_ok(),
						"Container target is not local"
					);
					targets.push(pointer.1);
				}
			}
		}
		for id in targets {
			ensure!(found.is_none(), "Duplicate original container {container}");
			found = Some((file.clone(), id));
		}
	}
	found.with_context(|| format!("Original container absent: {container}"))
}

fn check_entry(entry: &Value, data: &[u8], name: &str) -> Result<()> {
	ensure!(
		entry["abSize"].as_u64() == Some(data.len() as u64),
		"Installed bundle size differs from current list: {name}"
	);
	let expected: &str = text(entry, "md5")?;
	if expected.len() == 32 {
		ensure!(
			format!("{:x}", Md5::digest(data)).eq_ignore_ascii_case(expected),
			"Installed bundle MD5 differs from current list: {name}"
		);
	} else {
		ensure!(
			entry["meta"].as_bool() == Some(true) || entry["meta"].as_u64() == Some(1),
			"Bundle has an unclaimed short checksum: {name}"
		);
	}
	Ok(())
}

fn check_manifest(entry: &Value, data: &[u8], name: &str) -> Result<()> {
	let expected: &str = text(entry, "md5")?;
	ensure!(
		expected.len() == 32 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
		"Original manifest requires a complete MD5 digest"
	);
	check_entry(entry, data, name)
}

struct Flat<'a> {
	data: &'a [u8],
}
impl Flat<'_> {
	fn bytes(&self, start: usize, size: usize) -> Result<&[u8]> {
		self.data
			.get(
				start
					..start
						.checked_add(size)
						.context("Manifest offset overflow")?,
			)
			.context("Manifest offset outside buffer")
	}
	fn u32(&self, pos: usize) -> Result<u32> {
		Ok(u32::from_le_bytes(self.bytes(pos, 4)?.try_into()?))
	}
	fn fields(&self, table: usize) -> Result<BTreeMap<usize, usize>> {
		ensure!(table >= 128, "Manifest table outside signed body");
		let distance: i32 = self.u32(table)? as i32;
		let vtable: usize = usize::try_from(
			(table as i64)
				.checked_sub(i64::from(distance))
				.context("Manifest vtable overflow")?,
		)?;
		ensure!(vtable >= 128, "Manifest vtable outside signed body");
		let raw: &[u8] = self.bytes(vtable, 4)?;
		let len: usize = u16::from_le_bytes(raw[..2].try_into()?) as usize;
		let size: usize = u16::from_le_bytes(raw[2..].try_into()?) as usize;
		ensure!(
			(4..=20).contains(&len) && len.is_multiple_of(2) && (4..=64).contains(&size),
			"Unexpected manifest vtable shape"
		);
		self.bytes(table, size)?;
		let mut fields: BTreeMap<usize, usize> = BTreeMap::new();
		for index in (4..len).step_by(2) {
			let offset: usize =
				u16::from_le_bytes(self.bytes(vtable + index, 2)?.try_into()?) as usize;
			if offset != 0 {
				ensure!(offset < size, "Manifest field outside table");
				fields.insert(index / 2 - 2, table + offset);
			}
		}
		Ok(fields)
	}
	fn target(&self, field: usize) -> Result<usize> {
		field
			.checked_add(self.u32(field)? as usize)
			.context("Manifest relative offset overflow")
	}
	fn vector(&self, field: usize, bound: usize) -> Result<Vec<u32>> {
		let start: usize = self.target(field)?;
		let count: usize = self.u32(start)? as usize;
		ensure!(count <= bound, "Manifest vector exceeds bound");
		self.bytes(
			start + 4,
			count.checked_mul(4).context("Manifest vector overflow")?,
		)?;
		(0..count)
			.map(|index| self.u32(start + 4 + index * 4))
			.collect()
	}
	fn tables(&self, field: usize, bound: usize) -> Result<Vec<BTreeMap<usize, usize>>> {
		let start: usize = self.target(field)?;
		self.vector(field, bound)?
			.iter()
			.enumerate()
			.map(|(index, offset)| {
				self.fields(
					(start + 4 + index * 4)
						.checked_add(*offset as usize)
						.context("Manifest table offset overflow")?,
				)
			})
			.collect()
	}
	fn string(&self, field: usize) -> Result<String> {
		let start: usize = self.target(field)?;
		let length: usize = self.u32(start)? as usize;
		ensure!(
			length <= 4096 && self.bytes(start + 4 + length, 1)? == [0],
			"Manifest string bound or terminator differs"
		);
		Ok(std::str::from_utf8(self.bytes(start + 4, length)?)?.to_owned())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn md5_rejects_stale_fallback_and_short_nonmetadata_checksums() {
		let good: Value = json!({"abSize": 3, "md5": format!("{:x}", Md5::digest(b"abc"))});
		assert!(check_entry(&good, b"abc", "bundle").is_ok());
		assert!(check_entry(&good, b"xyz", "bundle").is_err());
		assert!(check_entry(&json!({"abSize": 3, "md5": "tag"}), b"abc", "bundle").is_err());
		let metadata: Value = json!({"abSize": 3, "md5": "tag", "meta": 1});
		assert!(check_entry(&metadata, b"abc", "metadata bundle").is_ok());
		assert!(check_manifest(&metadata, b"abc", "manifest").is_err());
		assert!(check_manifest(&good, b"abc", "manifest").is_ok());
		assert!(check_manifest(
			&json!({"abSize": 3, "md5": "z".repeat(32)}),
			b"abc",
			"manifest"
		)
		.is_err());
	}
	#[test]
	fn flatbuffer_offsets_are_bounded_before_allocating() {
		let fb: Flat<'_> = Flat { data: &[0xff; 140] };
		assert!(fb.fields(128).is_err());
		assert!(fb.vector(128, 4).is_err());
		assert!(fb.string(128).is_err());
	}

	#[test]
	fn state_within_client_is_rejected_before_creating_any_directory() {
		let client: PathBuf = std::env::temp_dir().join(format!(
			"ark-client-boundary-{}-{}",
			std::process::id(),
			chrono::Utc::now().timestamp_nanos_opt().unwrap()
		));
		fs::create_dir(&client).unwrap();
		fs::write(client.join("GameAssembly.dll"), b"fixture").unwrap();
		let state: PathBuf = client.join("forbidden-state");
		assert!(Catalog::new(&state, &client)
			.unwrap_err()
			.to_string()
			.contains("separate directories"));
		assert!(!state.exists());
		fs::remove_file(client.join("GameAssembly.dll")).unwrap();
		fs::remove_dir(client).unwrap();
	}

	#[test]
	fn declared_bundle_closure_handles_cycles_and_rejects_unknown_roots() {
		let catalog: Catalog = Catalog {
			assets: BTreeMap::new(),
			paths: BTreeMap::new(),
			version_id: "test".into(),
			state: PathBuf::new(),
			client: PathBuf::new(),
			data: PathBuf::new(),
			bundles: vec![
				"scene.ab".into(),
				"dependency.ab".into(),
				"shared.ab".into(),
			],
			dependencies: vec![vec![1], vec![0, 2], vec![]],
			entries: BTreeMap::new(),
			hot: PathBuf::new(),
			base: PathBuf::new(),
			provenance: Value::Null,
		};
		let expected: BTreeSet<String> = ["scene.ab", "dependency.ab", "shared.ab"]
			.into_iter()
			.map(str::to_owned)
			.collect();
		assert_eq!(
			catalog
				.closure(&BTreeSet::from(["scene.ab".into()]))
				.unwrap(),
			expected
		);
		assert!(catalog
			.closure(&BTreeSet::from(["missing.ab".into()]))
			.is_err());
	}

	fn latest_fixture(parent: &Path, filename: &str) -> PathBuf {
		let mut fixtures: Vec<PathBuf> = fs::read_dir(parent)
			.unwrap()
			.map(|entry| entry.unwrap().path())
			.filter(|path| path.join(filename).is_file())
			.collect();
		fixtures.sort();
		fixtures
			.pop()
			.expect("No optional owned fixture is installed")
	}

	#[test]
	#[ignore = "Requires the optional locally installed official client and owned Player fixtures"]
	fn installed_client_resource_qualification() {
		let state: PathBuf = std::env::var_os("ARK_BATTLE_RUNTIME_STATE")
			.map(PathBuf::from)
			.unwrap_or_else(|| {
				Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/battle-runtime")
			})
			.canonicalize()
			.unwrap();
		let inventory: Value = read_json(&state.join("client-inventory.json")).unwrap();
		let client: PathBuf = std::env::var_os("ARK_PC_CLIENT")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from(text(&inventory, "client").unwrap()));
		let catalog: Catalog = Catalog::new(&state, &client).unwrap();
		let reference: PathBuf = std::env::var_os("ARK_RESOURCE_REFERENCE_PLAN")
			.map(PathBuf::from)
			.unwrap_or_else(|| latest_fixture(&state.join("plans"), "plan.json").join("plan.json"));
		let plan: Value = read_json(&reference).unwrap();
		let level: PathBuf = catalog
			.prepare_level(text(&plan, "level_id").unwrap())
			.unwrap();
		eprintln!("Rust level: {}", level.display());
		let scene: PathBuf = catalog.prepare_scene(&plan).unwrap();
		eprintln!("Rust scene: {}", scene.display());
		let player: PathBuf = std::env::var_os("ARK_RESOURCE_REFERENCE_PLAYER")
			.map(PathBuf::from)
			.unwrap_or_else(|| {
				latest_fixture(
					&state.join("unity-project/_PlayerOutput"),
					"ArkBattleRuntime.exe",
				)
			});
		let services: PathBuf =
			super::super::services::prepare(&catalog, &player, &scene, &level).unwrap();
		eprintln!("Rust services: {}", services.display());
		let record: Value = read_json(&services).unwrap();
		assert_eq!(record["complete"], true);
		assert!(!record["tables"].as_array().unwrap().is_empty());
		let descriptions: Value = read_json(Path::new(
			text(&record["type_descriptions"], "path").unwrap(),
		))
		.unwrap();
		let checked: u64 = descriptions
			.as_array()
			.unwrap()
			.iter()
			.map(|entry| entry["objects_checked"].as_u64().unwrap())
			.sum();
		let original: File =
			File::parse(&fs::read(catalog.data.join("sharedassets1.assets")).unwrap()).unwrap();
		assert_eq!(
			checked,
			original.objects.len() as u64,
			"Every original stripped object must roundtrip"
		);
		assert!(record["files"].as_array().unwrap().iter().all(|file| file
			["serialized_bytes_identical"]
			== true || file
			["owned_resource_map_extended"]
			== true || file["name"]
			.as_str()
			.unwrap()
			.ends_with(".resS")));
	}
}
