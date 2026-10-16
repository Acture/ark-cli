use crate::{environment::RuntimeConfig, maa, squad};
use anyhow::{ensure, Context, Result};
use ark_unity::{build, io, player};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	path::{Path, PathBuf},
};

/// Original tables the probe exports with their probe output variable and cached label.
pub(crate) const IDENTITY_TABLES: [(&str, &str); 4] = [
	("ARK_RUNTIME_CHARACTER_OUTPUT", "characters"),
	("ARK_RUNTIME_STAGE_OUTPUT", "stages"),
	("ARK_RUNTIME_MODULE_OUTPUT", "modules"),
	("ARK_RUNTIME_FAVOR_OUTPUT", "favor"),
];

#[derive(Clone)]
pub(crate) struct Battle {
	pub actions: String,
	pub actions_source: Value,
	pub squad: Value,
	pub seed: u32,
	pub max_ticks: u32,
	/// Record every change of the operators' original skill and unit state per tick.
	pub trace: bool,
	pub presentation: Presentation,
	/// Original speed level (1 or 2) a presented battle starts at; a job's SpeedUp toggles it.
	pub speed: u32,
	/// Unity time scale of a presented battle (1, 2, 4, 8 or 16): game seconds per real second.
	pub time_scale: u32,
}

/// How a battle is driven.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presentation {
	/// Original manual ticks back to back, without Unity frames: fastest and repeatable, for
	/// search and results.
	#[default]
	Fast,
	/// The original loop drives the battle as in the client (FixedUpdate ticks, Update, UI,
	/// coroutines) in game seconds, hidden: checks behavior that depends on frames.
	Frames,
	/// As Frames, in a visible window.
	Watch,
}
#[derive(Clone)]
pub(crate) struct Request {
	pub probe: PathBuf,
	pub player_build: PathBuf,
	pub level: Option<PathBuf>,
	pub scene: Option<PathBuf>,
	pub services: Option<PathBuf>,
	pub battle: Option<Battle>,
	pub export_identities: bool,
	/// Load the client's original Lua code hotfixes after services, as the client does.
	pub lua_hotfix: bool,
	pub deadline: u64,
	pub notes: Value,
}
impl Request {
	pub fn control(probe: PathBuf, player_build: PathBuf) -> Self {
		Self {
			probe,
			player_build,
			level: None,
			scene: None,
			services: None,
			battle: None,
			export_identities: false,
			lua_hotfix: true,
			deadline: 30,
			notes: json!({}),
		}
	}
}

pub(crate) fn source_revision() -> Result<Value> {
	let mut sources: BTreeMap<String, String> = BTreeMap::new();
	for (name, bytes) in crate::bundle::SOURCE_FILES {
		sources.insert(
			format!("ark-battle-runtime/{name}"),
			format!("{:x}", Sha256::digest(bytes)),
		);
	}
	for (name, bytes) in crate::bundle::PROBE_FILES {
		sources.insert(
			format!("ark-battle-runtime-probe/{name}"),
			format!("{:x}", Sha256::digest(bytes)),
		);
	}
	let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&sources)?));
	Ok(
		json!({"source_sha256":hash,"files":sources,"executable_sha256":io::sha256_file(&std::env::current_exe()?)?}),
	)
}

pub(crate) fn inventory(config: &RuntimeConfig, record: bool) -> Result<Value> {
	let mut files: BTreeMap<String, Value> = BTreeMap::new();
	for (name, relative) in [
		("GameAssembly.dll", "GameAssembly.dll"),
		("baselib.dll", "baselib.dll"),
		("UnityPlayer.dll", "UnityPlayer.dll"),
		(
			"global-metadata.dat",
			"Arknights_Data/il2cpp_data/Metadata/global-metadata.dat",
		),
		(
			"ScriptingAssemblies.json",
			"Arknights_Data/ScriptingAssemblies.json",
		),
		(
			"RuntimeInitializeOnLoads.json",
			"Arknights_Data/RuntimeInitializeOnLoads.json",
		),
		("boot.config", "Arknights_Data/boot.config"),
		("globalgamemanagers", "Arknights_Data/globalgamemanagers"),
		(
			"globalgamemanagers.assets",
			"Arknights_Data/globalgamemanagers.assets",
		),
		(
			"sharedassets0.assets",
			"Arknights_Data/sharedassets0.assets",
		),
		(
			"sharedassets1.assets",
			"Arknights_Data/sharedassets1.assets",
		),
		("level1", "Arknights_Data/level1"),
		("xlua.dll", "Arknights_Data/Plugins/x86_64/xlua.dll"),
	] {
		let path = config.client.join(relative);
		files.insert(
			name.to_owned(),
			json!({"path":path,"sha256":io::sha256_file(&path)?,"size":fs::metadata(&path)?.len()}),
		);
	}
	let value = json!(files);
	let path = config.state.join("client-inventory.json");
	if record {
		io::write_json(
			&path,
			&json!({"schema":"ark-battle-client-inventory-v3","client":config.client,"files":value}),
		)?;
	} else {
		let pinned: Value = io::read_json(&path).context("Run `ark-cli battle setup` first")?;
		for (name, entry) in value.as_object().unwrap() {
			ensure!(
				pinned["files"][name]["sha256"] == entry["sha256"]
					&& pinned["files"][name]["size"] == entry["size"],
				"Official client changed since setup: {name}"
			);
		}
	}
	Ok(value)
}

fn check(
	level: &Option<Value>,
	scene: &Option<Value>,
	services: &Option<Value>,
	request: &Request,
) -> Result<()> {
	if let Some(level) = level {
		io::verify_pin(&level["level"])?;
		io::verify_pin(&level["verification_key"])?;
	}
	if let Some(scene) = scene {
		ensure!(scene["complete"] == true, "Incomplete scene closure");
		for entry in scene["bundles"]
			.as_array()
			.context("Scene bundles absent")?
		{
			io::verify_pin(entry)?;
			ensure!(
				entry["serialized_bytes_identical"] == true,
				"Unverified original scene payload"
			);
		}
	}
	if let Some(services) = services {
		ensure!(services["complete"] == true, "Incomplete services");
		for entry in services["files"]
			.as_array()
			.context("Service files absent")?
		{
			io::verify_pin(entry)?;
			let name = entry["name"].as_str().context("Service filename missing")?;
			ensure!(
				Path::new(name).components().count() == 1 && !name.contains(['/', '\\', ':']),
				"Invalid service filename"
			);
		}
		for table in services["tables"].as_array().context("Tables absent")? {
			let path = Path::new(
				table["configuration"]
					.as_str()
					.context("Configuration missing")?,
			);
			ensure!(
				table["configuration_sha256"] == io::sha256_file(path)?,
				"Table config changed"
			);
		}
		io::verify_pin(&services["table_list"])?;
		io::verify_pin(&services["assembly_source"])?;
		if let Some(scene) = &request.scene {
			ensure!(
				services["scene_inputs"]["sha256"] == io::sha256_file(scene)?,
				"Services belong to different scene"
			);
		}
		if let Some(level) = &request.level {
			ensure!(
				services["level_inputs"]["sha256"] == io::sha256_file(level)?,
				"Services belong to different level"
			);
		}
	}
	ensure!(
		request.battle.is_none() || (level.is_some() && scene.is_some() && services.is_some()),
		"Battle needs level, scene, services"
	);
	ensure!(
		!request.export_identities || services.is_some(),
		"Identity export needs services"
	);
	Ok(())
}

pub(crate) fn run(config: &RuntimeConfig, request: &Request) -> Result<(PathBuf, Value)> {
	let inventory = inventory(config, false)?;
	let player_dir = build::verified_player(&request.player_build)?;
	fn optional(path: &Option<PathBuf>) -> Result<Option<Value>> {
		path.as_ref().map(|p| io::read_json(p)).transpose()
	}
	let level = optional(&request.level)?;
	let scene = optional(&request.scene)?;
	let services = optional(&request.services)?;
	check(&level, &scene, &services, request)?;
	let output = io::stage_dir(&config.state, "runs")?;
	let copied = io::owned(
		&config.state,
		&config
			.state
			.join("players")
			.join(output.file_name().unwrap()),
	)?;
	let result = (|| -> Result<Value> {
		io::copy_tree(&player_dir, &copied)?;
		for (name, relative) in [
			("GameAssembly.dll", "GameAssembly.dll"),
			("baselib.dll", "baselib.dll"),
			(
				"global-metadata.dat",
				"ArkBattleRuntime_Data/il2cpp_data/Metadata/global-metadata.dat",
			),
		] {
			let target = copied.join(relative);
			fs::copy(inventory[name]["path"].as_str().unwrap(), &target)?;
			ensure!(
				io::sha256_file(&target)? == inventory[name]["sha256"],
				"Copied original runtime changed"
			);
		}
		let plugin = copied.join("ArkBattleRuntime_Data/Plugins/x86_64/ArkBattleProbe.dll");
		fs::create_dir_all(plugin.parent().unwrap())?;
		fs::copy(&request.probe, plugin)?;
		// The original xLua native library that the client's Lua hotfix layer P/Invokes.
		let lua = copied.join("ArkBattleRuntime_Data/Plugins/x86_64/xlua.dll");
		fs::copy(inventory["xlua.dll"]["path"].as_str().unwrap(), &lua)?;
		ensure!(
			io::sha256_file(&lua)? == inventory["xlua.dll"]["sha256"],
			"Copied original xLua library changed"
		);
		let assembly_path = copied.join("ArkBattleRuntime_Data/ScriptingAssemblies.json");
		let mut own: Value = io::read_json(&assembly_path)?;
		let mut additions: Vec<(Value, Value)> = Vec::new();
		if let Some(scene) = &scene {
			for item in scene["script_assemblies"]
				.as_array()
				.context("Scene assemblies missing")?
			{
				additions.push((item["name"].clone(), item["type"].clone()));
			}
		}
		if let Some(services) = &services {
			for entry in services["files"].as_array().unwrap() {
				let target = io::owned(
					&config.state,
					&copied
						.join("ArkBattleRuntime_Data")
						.join(entry["name"].as_str().unwrap()),
				)?;
				fs::copy(entry["path"].as_str().unwrap(), &target)?;
				ensure!(
					io::sha256_file(&target)? == entry["sha256"],
					"Copied service file changed"
				);
			}
			let original: Value = io::read_json(Path::new(
				services["assembly_source"]["path"].as_str().unwrap(),
			))?;
			let names = original["names"]
				.as_array()
				.context("Assembly names missing")?;
			let types = original["types"]
				.as_array()
				.context("Assembly types missing")?;
			ensure!(
				names.len() == types.len(),
				"Assembly manifest shape changed"
			);
			additions.extend(names.iter().cloned().zip(types.iter().cloned()));
		}
		for (name, kind) in additions {
			if !own["names"]
				.as_array()
				.context("Owned assemblies absent")?
				.contains(&name)
			{
				own["names"].as_array_mut().unwrap().push(name);
				own["types"]
					.as_array_mut()
					.context("Owned types missing")?
					.push(kind);
			}
		}
		io::write_json(&assembly_path, &own)?;
		let run = output.join("run");
		let mut env = BTreeMap::from([
			("ARK_RUNTIME_ENGINE_PROBE".to_owned(), "1".to_owned()),
			("ARK_RUNTIME_BATTLE_ACTIVATE".to_owned(), "1".to_owned()),
		]);
		let mut set = |key: &str, value: String| {
			env.insert(
				key.to_owned(),
				io::external_path(Path::new(&value))
					.to_string_lossy()
					.into_owned(),
			);
		};
		if let Some(level) = &level {
			set("ARK_RUNTIME_DECODE_LEVEL", "1".to_owned());
			set(
				"ARK_RUNTIME_LEVEL_INPUT",
				level["level"]["path"].as_str().unwrap().to_owned(),
			);
			set(
				"ARK_RUNTIME_VERIFICATION_KEY",
				level["verification_key"]["path"]
					.as_str()
					.unwrap()
					.to_owned(),
			);
			set(
				"ARK_RUNTIME_LEVEL_OUTPUT",
				run.join("decoded-level.json")
					.to_string_lossy()
					.into_owned(),
			);
		}
		if let Some(scene) = &scene {
			let bundles = scene["bundles"]
				.as_array()
				.context("Scene bundles missing")?;
			let ordered: Vec<&Value> = bundles
				.iter()
				.filter(|b| b["scene_bundle"] != true)
				.chain(bundles.iter().filter(|b| b["scene_bundle"] == true))
				.collect();
			let expected: Vec<Value> = ordered.iter().map(|b| b["path"].clone()).collect();
			ensure!(
				scene["load_order"] == json!(expected),
				"Scene load order differs from verified bundles"
			);
			let paths: Vec<String> = expected
				.iter()
				.map(|p| {
					p.as_str().context("Bundle path missing").map(|p| {
						io::external_path(Path::new(p))
							.to_string_lossy()
							.into_owned()
					})
				})
				.collect::<Result<_>>()?;
			let consumed = output.join("scene-load-order.txt");
			fs::write(&consumed, paths.join("\n") + "\n")?;
			set("ARK_RUNTIME_SCENE_BUNDLES", "1".to_owned());
			set(
				"ARK_RUNTIME_SCENE_LIST",
				consumed.to_string_lossy().into_owned(),
			);
			set("ARK_RUNTIME_SCENE_LOAD", "1".to_owned());
			set("ARK_RUNTIME_SCENE_NAME", "1".to_owned());
		}
		if let Some(services) = &services {
			set("ARK_RUNTIME_SERVICES", "1".to_owned());
			// Lua scripts route through the scene bundles, so only battle runs load hotfixes.
			if request.lua_hotfix && scene.is_some() {
				set("ARK_RUNTIME_LUA_HOTFIX", "1".to_owned());
			}
			set(
				"ARK_RUNTIME_SERVICE_TABLES",
				services["table_list"]["path"].as_str().unwrap().to_owned(),
			);
		}
		if request.export_identities {
			set("ARK_RUNTIME_EXPORT_IDENTITIES", "1".to_owned());
			set(
				"ARK_RUNTIME_CHARACTER_OUTPUT",
				run.join("original-characters.json")
					.to_string_lossy()
					.into_owned(),
			);
			for (variable, label) in IDENTITY_TABLES.iter().skip(1) {
				set(
					variable,
					run.join(format!("original-{label}.json"))
						.to_string_lossy()
						.into_owned(),
				);
			}
		}
		if let Some(battle) = &request.battle {
			fs::write(output.join("probe-actions.txt"), &battle.actions)?;
			fs::write(output.join("squad.jsonl"), squad::compile(&battle.squad)?)?;
			for (key, value) in [
				("ARK_RUNTIME_RUN_BATTLE", "1".to_owned()),
				(
					"ARK_RUNTIME_BATTLE_ACTIONS",
					output
						.join("probe-actions.txt")
						.to_string_lossy()
						.into_owned(),
				),
				("ARK_RUNTIME_CUSTOM_SQUAD", "1".to_owned()),
				(
					"ARK_RUNTIME_SQUAD_INPUT",
					output.join("squad.jsonl").to_string_lossy().into_owned(),
				),
				(
					"ARK_RUNTIME_SQUAD_OUTPUT",
					run.join("converted-squad.json")
						.to_string_lossy()
						.into_owned(),
				),
				("ARK_RUNTIME_BATTLE_SEED", battle.seed.to_string()),
				(
					"ARK_RUNTIME_BATTLE_LEVEL_ID",
					services.as_ref().unwrap()["level_id"]
						.as_str()
						.unwrap()
						.to_owned(),
				),
				(
					"ARK_RUNTIME_CAMERA_BUNDLE_PATH",
					services.as_ref().unwrap()["camera_bundle_path"]
						.as_str()
						.unwrap()
						.to_owned(),
				),
				("ARK_RUNTIME_BATTLE_UNTIL_FINISHED", "1".to_owned()),
				("ARK_RUNTIME_BATTLE_MAX_TICKS", battle.max_ticks.to_string()),
			] {
				set(key, value);
			}
			if battle.trace {
				set("ARK_RUNTIME_SKILL_TRACE", "1".to_owned());
			}
			if battle.presentation != Presentation::Fast {
				set("ARK_RUNTIME_PRESENTATION", "1".to_owned());
				set("ARK_RUNTIME_SPEED_LEVEL", battle.speed.to_string());
				set("ARK_RUNTIME_TIME_SCALE", battle.time_scale.to_string());
			}
		}
		let mut effective: Vec<Value> = Vec::new();
		for entry in walkdir::WalkDir::new(&copied).sort_by_file_name() {
			let entry = entry?;
			if entry.file_type().is_file() {
				let relative = entry
					.path()
					.strip_prefix(&copied)?
					.to_string_lossy()
					.replace('\\', "/");
				if !relative.starts_with("ArkBattleRuntime_BackUpThisFolder") {
					effective
						.push(json!({"path":relative,"sha256":io::sha256_file(entry.path())?}));
				}
			}
		}
		let completion = if request.battle.is_some() {
			"original_battle_natural_finish_complete"
		} else if scene.is_some() {
			"original_scene_framework_complete"
		} else if services.is_some() {
			"original_services_complete"
		} else {
			"battle_component_complete"
		};
		let consumed = execution_files(request, &level, &scene, &services, &output)?;
		let execution_inputs = hash_execution_files(&consumed)?;
		let presented: Option<&Battle> = request
			.battle
			.as_ref()
			.filter(|battle| battle.presentation != Presentation::Fast);
		// A presented battle runs in game seconds: 30 ticks per second at speed 1 and scale 1.
		let deadline: u64 = presented.map_or(request.deadline, |battle| {
			let rate: u64 = 30 * u64::from(battle.speed) * u64::from(battle.time_scale);
			(120 + u64::from(battle.max_ticks) / rate).min(900)
		});
		let visible: bool =
			presented.is_some_and(|battle| battle.presentation == Presentation::Watch);
		let mut result = player::run(&copied, &run, &env, completion, deadline, visible)?;
		ensure!(
			execution_inputs == hash_execution_files(&consumed)?,
			"An execution input changed while Player was running"
		);
		evaluate(&mut result, &run, request, &level, &scene, &services)?;
		result["effective_inputs"] = json!(effective);
		result["execution_inputs"] = execution_inputs;
		result["source_revision"] = source_revision()?;
		result["inputs"] = json!({"probe":request.probe,"probe_sha256":io::sha256_file(&request.probe)?,"player_build":request.player_build,"level":request.level,"scene":request.scene,"services":request.services,"notes":request.notes});
		if let Some(battle) = &request.battle {
			result["battle_inputs"] = json!({"actions_source":battle.actions_source,"squad":battle.squad,"seed":battle.seed,"max_ticks":battle.max_ticks,"actions_sha256":io::sha256_file(&output.join("probe-actions.txt"))?,"squad_sha256":io::sha256_file(&output.join("squad.jsonl"))?});
		}
		result["original_installation_modified"] = json!(false);
		io::write_json(&output.join("result.json"), &result)?;
		Ok(result)
	})();
	// Verify the resolved absolute target again immediately before recursive cleanup.
	if copied.exists() {
		let checked = io::owned(&config.state, &copied)?;
		fs::remove_dir_all(checked)?;
	}
	Ok((output, result?))
}

fn execution_files(
	request: &Request,
	level: &Option<Value>,
	scene: &Option<Value>,
	services: &Option<Value>,
	output: &Path,
) -> Result<BTreeMap<String, PathBuf>> {
	let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
	for (role, path) in [
		("player/build", Some(&request.player_build)),
		("level/manifest", request.level.as_ref()),
		("scene/manifest", request.scene.as_ref()),
		("services/manifest", request.services.as_ref()),
	] {
		if let Some(path) = path {
			files.insert(role.to_owned(), path.clone());
		}
	}
	if let Some(level) = level {
		for key in ["level", "verification_key"] {
			files.insert(
				format!("level/{key}"),
				PathBuf::from(
					level[key]["path"]
						.as_str()
						.context("Level payload path absent")?,
				),
			);
		}
	}
	if let Some(scene) = scene {
		for (index, bundle) in scene["bundles"]
			.as_array()
			.context("Scene bundles absent")?
			.iter()
			.enumerate()
		{
			files.insert(
				format!("scene/bundle/{index:03}"),
				PathBuf::from(
					bundle["path"]
						.as_str()
						.context("Scene bundle path absent")?,
				),
			);
		}
		files.insert(
			"scene/load-order".to_owned(),
			output.join("scene-load-order.txt"),
		);
	}
	if let Some(services) = services {
		for key in ["table_list", "assembly_source"] {
			files.insert(
				format!("services/{key}"),
				PathBuf::from(
					services[key]["path"]
						.as_str()
						.context("Service input path absent")?,
				),
			);
		}
		for (index, table) in services["tables"]
			.as_array()
			.context("Service tables absent")?
			.iter()
			.enumerate()
		{
			files.insert(
				format!("services/config/{index:03}"),
				PathBuf::from(
					table["configuration"]
						.as_str()
						.context("Service table configuration absent")?,
				),
			);
		}
	}
	if request.battle.is_some() {
		files.insert(
			"battle/actions".to_owned(),
			output.join("probe-actions.txt"),
		);
		files.insert("battle/squad".to_owned(), output.join("squad.jsonl"));
	}
	Ok(files)
}

fn hash_execution_files(files: &BTreeMap<String, PathBuf>) -> Result<Value> {
	let mut pins: Vec<Value> = Vec::new();
	for (role, path) in files {
		pins.push(json!({"role":role,"sha256":io::sha256_file(path)?}));
	}
	Ok(json!(pins))
}

fn evaluate(
	result: &mut Value,
	run: &Path,
	request: &Request,
	level: &Option<Value>,
	scene: &Option<Value>,
	services: &Option<Value>,
) -> Result<()> {
	let events = result["events"]
		.as_array()
		.context("Observer events missing")?
		.clone();
	let stages: BTreeSet<&str> = events.iter().filter_map(|e| e["stage"].as_str()).collect();
	let has = |required: &[&str]| required.iter().all(|s| stages.contains(s));
	let mut passed = result["passed"] == true && has(&["battle_component_complete"]);
	let activation = has(&[
		"battle_singleton_before_null",
		"battle_singleton_bound",
		"battle_awake_queue_initialized",
		"battle_activation_complete",
		"battle_singleton_destroy_cleared",
	]);
	result["controller_activation_verified"] = json!(activation);
	passed &= activation && !stages.contains("battle_controller_on_init_start");
	if level.is_some() {
		let path = run.join("decoded-level.json");
		let verified = path.is_file() && stages.contains("original_level_decode_complete");
		result["level_decode_verified"] = json!(verified);
		passed &= verified;
		if path.is_file() {
			let value: Value = io::read_json(&path)?;
			result["decoded_level"] = json!({"path":path,"sha256":io::sha256_file(&path)?,"wave_count":value["waves"].as_array().map_or(0,Vec::len)});
		}
	}
	if let Some(scene) = scene {
		let indices: Vec<u64> = events
			.iter()
			.filter(|e| e["stage"] == "original_scene_bundle_load_index")
			.filter_map(|e| e["value"].as_u64())
			.collect();
		let bundles = indices
			== (0..scene["bundles"].as_array().unwrap().len() as u64).collect::<Vec<_>>()
			&& stages.contains("original_scene_bundles_complete");
		let framework = has(&[
			"original_scene_load_complete",
			"original_scene_controller_bound",
			"original_scene_framework_references_bound",
			"original_scene_framework_complete",
		]);
		result["scene_bundles_verified"] = json!(bundles);
		result["scene_framework_verified"] = json!(framework);
		passed &= bundles && framework;
	}
	if services.is_some() {
		let verified = stages.contains("original_services_complete");
		result["services_verified"] = json!(verified);
		passed &= verified;
		let requested: bool = request.lua_hotfix && scene.is_some();
		let hotfixes = stages.contains("original_lua_hotfixes_registered");
		result["lua_hotfix"] = json!({"requested": requested, "registered": hotfixes});
		passed &= hotfixes == requested;
	}
	if request.export_identities {
		let mut identities = json!({});
		for (_, label) in IDENTITY_TABLES {
			let path = run.join(format!("original-{label}.json"));
			if path.is_file() {
				identities[label] = io::pin(&path)?;
			}
		}
		let verified = identities.as_object().unwrap().len() == IDENTITY_TABLES.len()
			&& has(&[
				"original_character_identity_exported",
				"original_stage_identity_exported",
				"original_module_table_exported",
				"original_favor_table_exported",
			]);
		result["identity_tables"] = identities;
		result["identity_tables_exported"] = json!(verified);
		passed &= verified;
	}
	if let Some(battle) = &request.battle {
		let resource = result["runtime_diagnostics"]["object_pool_failure_messages"] == 0
			&& result["runtime_diagnostics"]["resource_manager_null_reference_observed"] == false;
		let converted = run.join("converted-squad.json");
		let squad_verified = converted.is_file()
			&& stages.contains("original_battle_squad_exported")
			&& squad::verify(&battle.squad, &io::read_json(&converted)?);
		let init = has(&[
			"original_battle_load_game_complete",
			"original_battle_started",
		]);
		let maximum = |stage: &str| {
			events
				.iter()
				.filter(|e| e["stage"] == stage)
				.filter_map(|e| e["value"].as_u64())
				.max()
				.unwrap_or(0)
		};
		let verified = maximum("original_probe_verified_action_count");
		let auto = maximum("original_maa_auto_skill_count");
		let queued = events
			.iter()
			.filter(|e| e["stage"] == "original_probe_action_queued")
			.count() as u64;
		let actions = queued == verified + auto;
		let completed = maximum("original_maa_completed_action_count");
		let remaining = maa::command_count(&battle.actions) as i64 - completed as i64;
		let job = stages.contains("original_maa_job_complete") && remaining == 0;
		let outcome = outcome(result)?;
		// A natural win before later commands become due still establishes the job, provided
		// every issued operation was confirmed; the unexecuted remainder is reported separately.
		let won: bool = outcome["status"] == "original_win";
		let job_result: &str = if job {
			"complete"
		} else if won && actions {
			"won_early"
		} else {
			"incomplete"
		};
		passed &= resource
			&& squad_verified
			&& init && actions
			&& outcome["terminal_verified"] == true
			&& (job || won);
		for (key, value) in [
			("battle_resource_load_verified", json!(resource)),
			("squad_verified", json!(squad_verified)),
			("battle_initialization_verified", json!(init)),
			("verified_action_count", json!(verified)),
			("automatic_skill_count", json!(auto)),
			("battle_actions_verified", json!(actions)),
			("completed_action_count", json!(completed)),
			("remaining_action_count", json!(remaining)),
			("maa_job_complete", json!(job)),
			("job_result", json!(job_result)),
			("battle_outcome", outcome),
		] {
			result[key] = value;
		}
	}
	result["passed"] = json!(passed);
	Ok(())
}

pub(crate) fn outcome(record: &Value) -> Result<Value> {
	let events = record["events"]
		.as_array()
		.context("Missing observer events")?;
	let stages: BTreeSet<&str> = events.iter().filter_map(|e| e["stage"].as_str()).collect();
	let values: BTreeMap<&str, i64> = events
		.iter()
		.filter_map(|e| Some((e["stage"].as_str()?, e["value"].as_i64()?)))
		.collect();
	let get = |key: &str| values.get(key).copied();
	let finished = get("original_battle_is_finished");
	let playing = get("original_battle_is_playing");
	let game = get("original_battle_game_result");
	let life =
		get("original_battle_life_point").map(|x| if x >= 1 << 31 { x - (1 << 32) } else { x });
	let frame = get("original_battle_fixed_frames");
	let ticks = get("original_battle_requested_ticks");
	let rejection = events.iter().filter_map(|e| e["stage"].as_str()).find(|s| {
		[
			"deployment_card_not_found",
			"deployment_card_not_ready",
			"probe_action_character_not_deployed",
			"probe_action_character_has_no_skill",
			"probe_action_skill_not_ready",
			"probe_action_withdraw_not_ready",
		]
		.contains(s)
	});
	let normal = record["exit_code"] == 0 && record["forced_termination"] == false;
	let mut verified = false;
	let status = if rejection.is_some() && normal {
		"action_rejected"
	} else if stages.contains("engine_probe_failed") || record.get("error").is_some() {
		"runtime_error"
	} else if record["observation_deadline_reached"] == true {
		"wall_clock_deadline"
	} else if !normal {
		"runtime_error"
	} else if stages.contains("original_battle_natural_finish_complete") {
		verified = finished == Some(1)
			&& playing == Some(0)
			&& matches!(game, Some(1 | 2))
			&& life.is_some()
			&& frame.is_some_and(|f| f > 0)
			&& ticks.is_some()
			&& !stages.contains("original_battle_tick_budget_exhausted");
		if verified {
			if game == Some(1) {
				"original_win"
			} else {
				"original_loss"
			}
		} else {
			"inconsistent_terminal_state"
		}
	} else if stages.contains("original_battle_tick_budget_exhausted") {
		"tick_budget_exhausted"
	} else {
		"incomplete_observation"
	};
	Ok(
		json!({"status":status,"terminal_verified":verified,"action_rejection_reason":rejection,"is_finished":finished.map(|v|v!=0),"is_playing":playing.map(|v|v!=0),"original_result":game,"original_result_name":game.and_then(|g|match g {0=>Some("NOT_YET"),1=>Some("WIN"),2=>Some("LOSE"),_=>None}),"life_point":life,"fixed_frame":frame,"requested_ticks":ticks,"provenance":"original BattleController getters; pinned original GameResult enum","verification_scope":"original runtime observation, not an Ark Solver replay verdict"}),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn budget_and_exit_never_imply_victory() {
		let r = json!({"events":[{"stage":"original_battle_tick_budget_exhausted"}],"exit_code":0,"forced_termination":false});
		let o = outcome(&r).unwrap();
		assert_eq!(o["status"], "tick_budget_exhausted");
		assert_eq!(o["terminal_verified"], false);
	}
}
