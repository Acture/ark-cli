use std::path::PathBuf;

use crate::config::Config;

pub(crate) fn configured_gamedata_dir() -> PathBuf {
	Config::load()
		.expect("load workspace rogue.toml for local-data test")
		.gamedata_dir
}

pub(crate) fn configured_battle_prefab_dir() -> PathBuf {
	Config::load()
		.expect("load workspace rogue.toml for local-data test")
		.battle_prefab_dir()
}
