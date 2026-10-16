use super::bundle::{self, Reader};
use super::catalog::container_object;
use super::serialized::{self, File, Node, Object, Script};
use super::trees::Trees;
use super::{pin, read_json, sha256, stage, text, write_json, Catalog};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const BUILTINS: [&str; 3] = [
	"globalgamemanagers.assets",
	"sharedassets0.assets",
	"sharedassets1.assets",
];
const TABLES: [&str; 23] = [
	"BattleMiscDB",
	"BuffTemplateDB",
	"BuffDB",
	"CharacterDB",
	"CharMetaDB",
	"CharMasterDB",
	"CharPatchDB",
	"EnemyDB",
	"EnemyHandBookDB",
	"SkillDB",
	"RangeDB",
	"DisplayMetaDB",
	"BattleUniEquipDB",
	"UniEquipDB",
	"GameDataConstsDB",
	"LevelScriptDB",
	"EPBreakBuffDB",
	"FavorDB",
	"SkinDB",
	"StageDB",
	"TokenDB",
	"SpecialOperatorDB",
	"CharWordDB",
];
const OPTIONS: [&str; 7] = [
	"GlobalOptions",
	"DBOptions",
	"ResourceOptions",
	"AudioOptions",
	"BasicUIOptions",
	"GradingOptions",
	"LuaOptions",
];
const SETTINGS: [(&str, i32); 5] = [
	("TagManager", 78),
	("Physics2DSettings", 19),
	("PhysicsManager", 55),
	("TimeManager", 5),
	("NavMeshProjectSettings", 126),
];
const CAMERA: &str = "battle/[pack]common.ab";

struct OwnedMap {
	gm: File,
	manager_id: i64,
	resources: Value,
	output: PathBuf,
	files: Vec<Value>,
}

impl OwnedMap {
	fn route(&mut self, name: &str, file: &str, path_id: i64) -> Result<()> {
		let pointer: Value = json!({"m_FileID": self.gm.external(file)?, "m_PathID": path_id});
		let name: String = name.to_lowercase();
		let container: &mut Vec<Value> = self.resources["m_Container"]
			.as_array_mut()
			.context("Owned Resources container missing")?;
		if container
			.iter()
			.any(|entry| entry[0].as_str() == Some(&name) && entry[1] == pointer)
		{
			return Ok(());
		}
		container.push(json!([name, pointer]));
		Ok(())
	}

	fn write_file(
		&mut self,
		source: &Path,
		name: &str,
		mut file: File,
		origin: Option<Value>,
	) -> Result<()> {
		let before: Vec<Object> = file.objects.clone();
		for external in &mut file.externals {
			if BUILTINS.contains(&external.path.as_str()) {
				external.path = format!("ark_original_{}", external.path);
			}
		}
		let target: PathBuf = self.output.join(name);
		let encoded: Vec<u8> = file.save()?;
		fs::write(&target, &encoded)?;
		let checked: File = File::parse(&fs::read(&target)?)?;
		ensure!(
			before == checked.objects,
			"Serialized object bytes changed in {name}"
		);
		let mut record: Value = json!({"source": source, "source_sha256": sha256(&fs::read(source)?), "path": target, "name": name, "sha256": sha256(&encoded), "serialized_bytes_identical": true});
		if let Some(origin) = origin {
			record["origin"] = origin;
		}
		self.files.push(record);
		Ok(())
	}
}

pub(crate) fn prepare(
	catalog: &Catalog,
	host: &Path,
	scene_inputs: &Path,
	level_inputs: &Path,
) -> Result<PathBuf> {
	let started: Instant = Instant::now();
	eprintln!("INFO Preparing original services and owned Resources map");
	let scene: Value = read_json(scene_inputs)?;
	let level: Value = read_json(level_inputs)?;
	ensure!(
		scene["complete"] == true
			&& scene["catalog"]["list_sha256"] == catalog.provenance()["list_sha256"]
			&& level["catalog"]["list_sha256"] == catalog.provenance()["list_sha256"],
		"Scene or level resource provenance differs"
	);
	let output: PathBuf = stage(&catalog.state, "services")?;
	let mut trees: Trees = Trees::new(catalog, &scene)?;
	let gm_source: PathBuf = host.join("ArkBattleRuntime_Data/globalgamemanagers");
	let mut gm: File = File::parse(&fs::read(&gm_source)?)?;
	trees.fill_builtins(&mut gm)?;
	let manager: &Object = gm.one_class(147)?;
	let manager_id: i64 = manager.id;
	let resources: Value = gm.decode(manager)?;
	let mut owned: OwnedMap = OwnedMap {
		gm,
		manager_id,
		resources,
		output: output.clone(),
		files: Vec::new(),
	};
	let mapper: File = File::parse(&fs::read(catalog.data.join("globalgamemanagers.assets"))?)?;
	let scripts: BTreeMap<i64, Script> = mapper
		.objects
		.iter()
		.filter(|obj| mapper.class(obj) == 115)
		.map(|obj| Ok((obj.id, serialized::mono_script(&obj.raw)?)))
		.collect::<Result<_>>()?;
	for id in scripts.keys() {
		owned.route(
			&format!("arkbootstrap/scripts/{id}"),
			"ark_original_globalgamemanagers.assets",
			*id,
		)?;
	}
	let mut configs: Vec<Value> = Vec::new();
	let mut tables: Vec<Value> = Vec::new();
	let mut prefabs: Vec<Value> = Vec::new();
	let mut fonts: Vec<Value> = Vec::new();
	let mut descriptions: Vec<Value> = Vec::new();
	for original in BUILTINS {
		let source: PathBuf = catalog.data.join(original);
		let mut file: File = File::parse(&fs::read(&source)?)?;
		if original == "sharedassets1.assets" {
			for mut entry in trees.restore(catalog, &mut file, &scripts)? {
				entry["file"] = json!(original);
				descriptions.push(entry);
			}
		}
		for obj in &file.objects {
			if file.class(obj) == 128 {
				let mut r: Reader<'_> = Reader::new(&obj.raw, false);
				let name: String = r.string()?;
				let key: String = format!("arkbootstrap/fonts/{name}");
				for alias in [&key, &format!("dyn/{key}")] {
					owned.route(alias, &format!("ark_original_{original}"), obj.id)?;
				}
				fonts.push(
					json!({"name": name, "resource": key, "file": original, "path_id": obj.id}),
				);
			}
			if file.class(obj) != 114 {
				continue;
			}
			let (_, script_pointer, game): (String, (i32, i64), (i32, i64)) =
				serialized::mono_head(&obj.raw)?;
			let Some(script) = scripts.get(&script_pointer.1) else {
				continue;
			};
			if script.class == "AVGController" {
				ensure!(game.0 == 0, "AVG controller prefab is not local");
				let key: String = format!("arkbootstrap/{}Prefab", script.class);
				owned.route(&key, &format!("ark_original_{original}"), game.1)?;
				prefabs.push(
					json!({"class": script.class, "resource": key, "path_id": game.1, "source": source}),
				);
				continue;
			}
			if !TABLES.contains(&script.class.as_str()) && !OPTIONS.contains(&script.class.as_str())
			{
				continue;
			}
			let key: String = format!("arkbootstrap/{}", script.class);
			owned.route(&key, &format!("ark_original_{original}"), obj.id)?;
			let item: Value = json!({"class": script.class, "namespace": script.namespace, "resource": key, "path_id": obj.id});
			if TABLES.contains(&script.class.as_str()) {
				tables.push(table_configuration(&output, &script.class, &obj.raw, item)?);
			} else {
				configs.push(item);
			}
		}
		owned.write_file(&source, &format!("ark_original_{original}"), file, None)?;
		let stream: PathBuf = catalog.data.join(format!("{original}.resS"));
		if stream.is_file() {
			let target: PathBuf = output.join(
				stream
					.file_name()
					.context("Resource stream has no filename")?,
			);
			fs::copy(&stream, &target)?;
			owned.files.push(json!({"source": stream, "source_sha256": sha256(&fs::read(&stream)?), "path": target, "name": target.file_name().and_then(|s| s.to_str()).context("Stream filename missing")?, "sha256": sha256(&fs::read(&target)?)}));
		}
	}
	route_xlua_resources(catalog, &mut owned, &mut trees)?;
	let avg: Value = avg_service(catalog, &scripts, &prefabs)?;
	for table in &mut tables {
		route_table(catalog, &mut owned, table)?;
		ensure!(
			started.elapsed().as_secs() < 120,
			"Service preparation exceeded bound"
		);
	}
	let level_entry: &Value = &level["level"];
	let level_bundle: &str = text(&level_entry["source"], "bundle")?;
	let level_source: PathBuf = catalog.file(level_bundle)?;
	let level_key: &str = text(level_entry, "container")?;
	let (level_file, pointer): (File, i64) = container_object(&level_source, level_key)?;
	ensure!(
		level_entry["path_id"].as_i64() == Some(pointer),
		"Pinned level container target changed"
	);
	let level_name: String = format!(
		"ark_level_{}.assets",
		&sha256(level_bundle.as_bytes())[..16]
	);
	owned.write_file(
		&level_source,
		&level_name,
		level_file,
		Some(catalog.describe(level_bundle)?),
	)?;
	for alias in [
		level_key.to_owned(),
		strip_extension(level_key),
		strip_extension(level_key.strip_prefix("dyn/").unwrap_or(level_key)),
	]
	.into_iter()
	.collect::<BTreeSet<_>>()
	{
		owned.route(&alias, &level_name, pointer)?;
	}
	let records: &[Value] = scene["bundles"]
		.as_array()
		.context("Missing scene bundles")?;
	let cameras: Vec<&Value> = records
		.iter()
		.filter(|entry| entry["bundle"] == CAMERA)
		.collect();
	ensure!(
		cameras.len() == 1,
		"Original camera bundle is outside scene closure"
	);
	let mut scene_routes: Vec<Value> = Vec::new();
	for record in records {
		for entry in bundle::load(Path::new(text(record, "path")?))? {
			if entry.flags & 4 == 0 {
				continue;
			}
			let file: File = File::parse(&entry.bytes)?;
			for obj in file.objects.iter().filter(|obj| file.class(obj) == 142) {
				let tree: Value = file.decode(obj)?;
				let cab: &str = &entry.name;
				for pair in tree["m_Container"]
					.as_array()
					.context("Scene bundle has no container")?
				{
					let key: &str = pair[0].as_str().context("Invalid scene container key")?;
					let info: &Value = &pair[1];
					let pointer: (i32, i64) = serialized::pointer(&info["asset"])?;
					if pointer.0 != 0 || pointer.1 == 0 {
						continue;
					}
					let archive: String = format!("archive:/{cab}/{cab}");
					for alias in [
						key.to_owned(),
						strip_extension(key),
						key.strip_prefix("dyn/").unwrap_or(key).to_owned(),
						strip_extension(key.strip_prefix("dyn/").unwrap_or(key)),
					]
					.into_iter()
					.collect::<BTreeSet<_>>()
					{
						owned.route(&alias, &archive, pointer.1)?;
					}
					let preload: &[Value] = tree["m_PreloadTable"]
						.as_array()
						.context("Scene bundle preload table missing")?;
					let start: usize = usize::try_from(
						info["preloadIndex"]
							.as_u64()
							.context("Invalid preload index")?,
					)?;
					let size: usize = usize::try_from(
						info["preloadSize"]
							.as_u64()
							.context("Invalid preload size")?,
					)?;
					let mut dependencies: Vec<Value> = Vec::new();
					for value in preload
						.get(start..start.checked_add(size).context("Preload end overflow")?)
						.context("Preload range outside table")?
					{
						dependencies.push(cab_pointer(&mut owned.gm, &file, cab, value)?);
					}
					let object: Value = cab_pointer(&mut owned.gm, &file, cab, &info["asset"])?;
					owned.resources["m_DependentAssets"]
						.as_array_mut()
						.context("Owned dependency map missing")?
						.push(json!({"m_Object": object, "m_Dependencies": dependencies}));
					scene_routes.push(json!({"path": key, "cab": cab, "path_id": pointer.1}));
				}
			}
		}
	}
	owned.resources["m_Container"]
		.as_array_mut()
		.context("Owned resource map missing")?
		.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
	owned.gm.replace(owned.manager_id, &owned.resources)?;
	let mut original_gm: File = File::parse(&fs::read(catalog.data.join("globalgamemanagers"))?)?;
	trees.fill_builtins(&mut original_gm)?;
	for (_, class) in SETTINGS {
		let original: &Object = original_gm.one_class(class)?;
		let target: i64 = owned.gm.one_class(class)?.id;
		// Both files are from the pinned engine. Validate the host's schema against
		// the exact original bytes, then copy those bytes without numeric conversion.
		let target_type: usize = owned.gm.object(target)?.type_index;
		let target_tree: &Node = owned.gm.types[target_type]
			.tree
			.as_ref()
			.context("Owned engine setting has no TypeTree")?;
		ensure!(
			serialized::encode(
				target_tree,
				&serialized::decode(target_tree, &original.raw)?
			)? == original.raw,
			"Original engine setting differs from owned same-version schema"
		);
		owned
			.gm
			.objects
			.iter_mut()
			.find(|obj| obj.id == target)
			.context("Owned setting missing")?
			.raw = original.raw.clone();
	}
	merge_scripts(&mut owned.gm, &original_gm)?;
	let gm_target: PathBuf = output.join("globalgamemanagers");
	fs::write(&gm_target, owned.gm.save()?)?;
	owned.files.push(json!({"source": gm_source, "source_sha256": sha256(&fs::read(&gm_source)?), "path": gm_target, "name": "globalgamemanagers", "sha256": sha256(&fs::read(&gm_target)?), "owned_resource_map_extended": true, "original_engine_settings": SETTINGS.iter().map(|(name, _)| *name).collect::<Vec<_>>()}));
	let table_list: PathBuf = output.join("tables.txt");
	let mut ordered: Vec<&Value> = tables.iter().collect();
	ordered.sort_by_key(|entry| entry["class"] != "BuffTemplateDB");
	fs::write(
		&table_list,
		ordered
			.iter()
			.map(|entry| {
				Ok(format!(
					"{}|{}",
					text(entry, "resource")?,
					text(entry, "configuration")?.replace('\\', "/")
				))
			})
			.collect::<Result<Vec<_>>>()?
			.join("\n")
			+ "\n",
	)?;
	let description_path: PathBuf = output.join("type-descriptions.json");
	write_json(&description_path, &descriptions)?;
	let result: PathBuf = output.join("inputs.json");
	write_json(
		&result,
		&json!({"schema": "ark-battle-service-inputs-v2", "complete": true, "catalog": catalog.provenance(), "files": owned.files, "configs": configs, "tables": tables, "prefabs": prefabs, "fonts": fonts, "avg_service_config": avg, "level_id": level["level_id"], "level_container": level_key, "camera_bundle_path": cameras[0]["path"], "scene_routes": scene_routes, "table_list": pin(&table_list)?, "scene_inputs": pin(scene_inputs)?, "level_inputs": pin(level_inputs)?, "assembly_source": pin(&catalog.data.join("ScriptingAssemblies.json"))?, "type_descriptions": pin(&description_path)?, "original_global_settings": pin(&catalog.data.join("globalgamemanagers"))?, "elapsed_seconds": started.elapsed().as_secs_f64()}),
	)?;
	eprintln!(
		"INFO Original services complete: {} tables, {} files in {:.2}s",
		tables.len(),
		owned.files.len(),
		started.elapsed().as_secs_f64()
	);
	Ok(result)
}

fn strip_extension(path: &str) -> String {
	path.rsplit_once('.')
		.map_or(path, |(stem, _)| stem)
		.to_owned()
}

fn cab_pointer(gm: &mut File, file: &File, cab: &str, value: &Value) -> Result<Value> {
	let pointer: (i32, i64) = serialized::pointer(value)?;
	let name: String = if pointer.0 == 0 {
		format!("archive:/{cab}/{cab}")
	} else {
		file.externals
			.get(usize::try_from(pointer.0 - 1)?)
			.context("Preload pointer file outside external table")?
			.path
			.clone()
	};
	Ok(json!({"m_FileID": gm.external(&name)?, "m_PathID": pointer.1}))
}

fn table_configuration(output: &Path, class: &str, raw: &[u8], mut item: Value) -> Result<Value> {
	ensure!(
		class
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b == b'_'),
		"Invalid table class filename"
	);
	let mut r: Reader<'_> = Reader::new(raw, false);
	r.pos = 28;
	r.string()?;
	let mut configs: Vec<Value> = Vec::new();
	let mut asset_path: String = String::new();
	let mut converter: u32 = 0;
	let mut load: u32 = 0;
	for _ in 0..2 {
		load = r.u32()?;
		asset_path = r.string()?;
		let pointer: (i32, i64) = (r.i32()?, r.i64()?);
		converter = r.u32()?;
		ensure!(pointer == (0, 0), "Unexpected embedded table asset pointer");
		configs.push(json!({"loadFromResource": load != 0, "assetPath": asset_path, "textAsset": null, "convertType": converter}));
	}
	let hot: u32 = r.u32()?;
	let login: u32 = r.u32()?;
	let bypass: u32 = r.u32()?;
	let asynchronous: u32 = r.u32()?;
	ensure!(
		r.u32()? == 0 && r.pos == raw.len(),
		"Unclaimed table configuration shape"
	);
	let path: PathBuf = output.join(format!("{class}.json"));
	write_json(
		&path,
		&json!({"_devConfig": configs[0], "_prodConfig": configs[1], "_requireConfig": {"hotupdate": hot != 0, "login": login != 0, "bypass": bypass != 0}, "_enableAsyncLoad": asynchronous != 0, "_samplePaths": []}),
	)?;
	item.as_object_mut().context("Invalid table record")?.extend(json!({"asset_path": asset_path, "converter": converter, "from_resource": load != 0, "configuration": path, "configuration_sha256": sha256(&fs::read(&path)?)}).as_object().context("Invalid table record")?.clone());
	Ok(item)
}

/// xLua's own Lua helpers (`xlua/*`) that the client's Lua hotfix layer requires through
/// `Resources.Load`. They live in the client's `resources.assets`, copied unchanged.
fn route_xlua_resources(catalog: &Catalog, owned: &mut OwnedMap, trees: &mut Trees) -> Result<()> {
	const RESOURCES: &str = "resources.assets";
	let mut original: File = File::parse(&fs::read(catalog.data.join("globalgamemanagers"))?)?;
	trees.fill_builtins(&mut original)?;
	let container: Value = original.decode(original.one_class(147)?)?;
	let mut routes: Vec<(String, i64)> = Vec::new();
	for entry in container["m_Container"]
		.as_array()
		.context("Original Resources container missing")?
	{
		let key: &str = entry[0].as_str().context("Resources key is not text")?;
		let file_id: i64 = entry[1]["m_FileID"].as_i64().context("Resources file id")?;
		if !key.starts_with("xlua/") || file_id <= 0 {
			continue;
		}
		let external: &str = &original.externals[file_id as usize - 1].path;
		ensure!(
			external == RESOURCES,
			"xLua resource is outside {RESOURCES}: {key}"
		);
		let path_id: i64 = entry[1]["m_PathID"].as_i64().context("Resources path id")?;
		routes.push((key.to_owned(), path_id));
	}
	ensure!(
		!routes.is_empty(),
		"Original xLua helper resources are missing"
	);
	let source: PathBuf = catalog.data.join(RESOURCES);
	let name: String = format!("ark_original_{RESOURCES}");
	owned.write_file(&source, &name, File::parse(&fs::read(&source)?)?, None)?;
	for (key, path_id) in routes {
		owned.route(&key, &name, path_id)?;
	}
	Ok(())
}

fn route_table(catalog: &Catalog, owned: &mut OwnedMap, table: &mut Value) -> Result<()> {
	let asset_path: &str = text(table, "asset_path")?;
	let candidates: Vec<&String> = catalog
		.assets
		.keys()
		.filter(|key| {
			key.as_str() == asset_path
				|| (key.starts_with(asset_path) && key.len() == asset_path.len() + 6)
		})
		.collect();
	ensure!(
		candidates.len() == 1,
		"Ambiguous original table route {asset_path}: {candidates:?}"
	);
	let key: &String = candidates[0];
	let bundle: &String = &catalog.assets[key];
	let source: PathBuf = catalog.file(bundle)?;
	let container: &String = &catalog.paths[key];
	let (file, target): (File, i64) = container_object(&source, container)?;
	let name: String = format!(
		"ark_data_{}.assets",
		Path::new(bundle)
			.file_stem()
			.and_then(|s| s.to_str())
			.context("Invalid table bundle stem")?
	);
	owned.write_file(&source, &name, file, Some(catalog.describe(bundle)?))?;
	for alias in [
		container.clone(),
		strip_extension(container),
		strip_extension(container.strip_prefix("dyn/").unwrap_or(container)),
		asset_path.to_owned(),
		format!("dyn/{asset_path}"),
	]
	.into_iter()
	.collect::<BTreeSet<_>>()
	{
		owned.route(&alias, &name, target)?;
	}
	table["original_container"] = json!(container);
	table["source"] = catalog.describe(bundle)?;
	Ok(())
}

fn avg_service(
	catalog: &Catalog,
	scripts: &BTreeMap<i64, Script>,
	prefabs: &[Value],
) -> Result<Value> {
	let source: PathBuf = catalog.data.join("level1");
	let file: File = File::parse(&fs::read(&source)?)?;
	let mut matches: Vec<&Object> = Vec::new();
	for obj in file.objects.iter().filter(|obj| file.class(obj) == 114) {
		let (_, pointer, _) = serialized::mono_head(&obj.raw)?;
		if scripts
			.get(&pointer.1)
			.is_some_and(|script| script.class == "AVG")
		{
			matches.push(obj);
		}
	}
	ensure!(
		matches.len() == 1,
		"Expected one original AVG scene service"
	);
	let obj: &Object = matches[0];
	let raw: &[u8] = &obj.raw;
	ensure!(
		raw.len() == 44 && raw[28..32] == [0, 0, 0, 0],
		"Unclaimed AVG scene configuration shape"
	);
	let mut r: Reader<'_> = Reader::new(raw, false);
	r.pos = 32;
	let pointer: (i32, i64) = (r.i32()?, r.i64()?);
	ensure!(
		file.externals
			.get(usize::try_from(pointer.0 - 1)?)
			.is_some_and(|entry| entry.path == "sharedassets1.assets"),
		"Original AVG service points to a different file"
	);
	let prefab: &Value = prefabs
		.iter()
		.find(|entry| entry["class"] == "AVGController")
		.context("Missing original AVG controller prefab")?;
	ensure!(
		prefab["path_id"].as_i64() == Some(pointer.1),
		"Original AVG controller target differs"
	);
	Ok(
		json!({"source": pin(&source)?, "path_id": obj.id, "serialized_sha256": sha256(raw), "fields": {"_prefab": {"file": "sharedassets1.assets", "path_id": pointer.1, "resource": "arkbootstrap/AVGControllerPrefab"}}}),
	)
}

fn find_node<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
	if node.name == name {
		Some(node)
	} else {
		node.children
			.iter()
			.find_map(|child| find_node(child, name))
	}
}

fn merge_scripts(gm: &mut File, original: &File) -> Result<()> {
	let original_tree: Value = original.decode(original.one_class(116)?)?;
	let mono: &Object = gm.one_class(116)?;
	let id: i64 = mono.id;
	let mut tree: Value = gm.decode(mono)?;
	let hash_node: Node = find_node(
		gm.types[mono.type_index]
			.tree
			.as_ref()
			.context("MonoManager schema absent")?,
		"m_ScriptHashes",
	)
	.and_then(|n| n.children.first())
	.and_then(|n| n.children.get(1))
	.and_then(|n| n.children.first())
	.context("MonoManager hash schema absent")?
	.clone();
	for value in original_tree["m_Scripts"]
		.as_array()
		.context("Original MonoManager script list absent")?
	{
		let (file_id, path_id): (i32, i64) = serialized::pointer(value)?;
		let file_id: i32 = if file_id == 0 {
			0
		} else {
			let name: &str = &original
				.externals
				.get(usize::try_from(file_id - 1)?)
				.context("Original script external absent")?
				.path;
			gm.external(&if BUILTINS.contains(&name) {
				format!("ark_original_{name}")
			} else {
				name.to_owned()
			})?
		};
		let pointer: Value = json!({"m_FileID": file_id, "m_PathID": path_id});
		let scripts: &mut Vec<Value> = tree["m_Scripts"]
			.as_array_mut()
			.context("Owned MonoManager script list absent")?;
		if !scripts.contains(&pointer) {
			scripts.push(pointer);
		}
	}
	let mut hashes: BTreeMap<(u64, u64), Value> = BTreeMap::new();
	for pair in tree["m_ScriptHashes"]
		.as_array()
		.context("Owned script hashes absent")?
		.iter()
		.chain(
			original_tree["m_ScriptHashes"]
				.as_array()
				.context("Original script hashes absent")?,
		) {
		let encoded: Vec<u8> = serialized::encode(&hash_node, &pair[0])?;
		ensure!(encoded.len() == 16, "MonoManager hash is not Hash128");
		hashes.insert(
			(
				u64::from_le_bytes(encoded[..8].try_into()?),
				u64::from_le_bytes(encoded[8..].try_into()?),
			),
			pair.clone(),
		);
	}
	tree["m_ScriptHashes"] = json!(hashes.into_values().collect::<Vec<_>>());
	gm.replace(id, &tree)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn aliases_strip_only_the_final_extension() {
		assert_eq!(
			strip_extension("dyn/gamedata/a.b.bytes"),
			"dyn/gamedata/a.b"
		);
		assert_eq!(strip_extension("battle/noextension"), "battle/noextension");
	}
}
