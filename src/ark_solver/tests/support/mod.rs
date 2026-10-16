// Each integration-test binary imports a different subset of this shared local-data API.
#![allow(dead_code)]

use std::path::PathBuf;

use ark_solver::config::Config;
use ark_solver::data::{
	ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
};
use ark_solver::replay::ExactReplayBuildBinding;

pub fn configured_gamedata_dir() -> PathBuf {
	Config::load()
		.expect("load workspace rogue.toml for local-data test")
		.gamedata_dir
}

pub fn configured_battle_prefab_dir() -> PathBuf {
	Config::load()
		.expect("load workspace rogue.toml for local-data test")
		.battle_prefab_dir()
}

pub fn local_battle_snapshots() -> (ArkDataSnapshot, BattlePrefabSnapshot) {
	let gamedata_path: PathBuf = configured_gamedata_dir();
	let prefab_path: PathBuf = configured_battle_prefab_dir();
	let snapshot: ArkDataSnapshot =
		ArkDataSnapshot::load(&gamedata_path).expect("configured gamedata snapshot must verify");
	let prefabs: BattlePrefabSnapshot = BattlePrefabSnapshot::load(&prefab_path)
		.expect("configured battle-prefab snapshot must verify");
	prefabs
		.ensure_compatible_with(&snapshot)
		.expect("configured snapshots must target the same gamedata revision");
	(snapshot, prefabs)
}

/// Binds one source name to the exact no-module build the current adapter resolves for it.
pub fn maximal_no_module_binding(
	snapshot: &ArkDataSnapshot,
	source_name: &str,
	operator_id: &str,
	skill: u8,
) -> ExactReplayBuildBinding {
	let template: OperatorBuild = snapshot
		.operators()
		.maximal_builds(operator_id, skill)
		.expect("operator maximal builds should resolve")
		.into_iter()
		.next()
		.expect("operator should have one maximal build");
	ExactReplayBuildBinding {
		source_name: source_name.to_owned(),
		build: OperatorBuild::new(
			OperatorIdentity::Id(operator_id.to_owned()),
			template.elite(),
			template.level(),
			template.trust_percent(),
			template.skill(),
			template.skill_level(),
			template.potential(),
			OperatorModule::None,
			0,
		),
		skill_usage: 0,
		skill_times: None,
	}
}
