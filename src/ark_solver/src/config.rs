//! Workspace configuration loader.
//!
//! Reads a small TOML file describing where ingestion and local-data tests should look for
//! game assets and gamedata on disk. Resolution order:
//!
//! 1. `$ROGUE_CONFIG` env var — path to a `.toml` file.
//! 2. Nearest `rogue.toml` in the working directory or its ancestors.
//! 3. `rogue.toml` beside the executable.
//! 4. Built-in local paths rooted in the working directory.
//!
//! Relative paths inside the TOML are resolved against the directory
//! containing the config file itself. This means a single `rogue.toml`
//! at the workspace root keeps every binary pointing at the same dirs
//! regardless of where they're launched from.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

const DEFAULT_CONFIG: &str =
	"assets_dir = 'assets'\nclient_dir = 'client'\ngamedata_dir = 'client/_gamedata'\n";

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
	assets_dir: String,
	client_dir: String,
	gamedata_dir: String,
}

#[derive(Debug, Clone)]
pub struct Config {
	pub source: PathBuf,
	pub assets_dir: PathBuf,
	pub client_dir: PathBuf,
	pub gamedata_dir: PathBuf,
}

impl Config {
	/// Load the workspace config, applying the resolution order
	/// documented at the crate root.
	pub fn load() -> Result<Self> {
		let cwd: PathBuf = env::current_dir()?;
		let executable: PathBuf = env::current_exe()?;
		let custom: Option<PathBuf> = env::var_os("ROGUE_CONFIG").map(PathBuf::from);
		match resolve_config_path(&cwd, &executable, custom.as_deref())? {
			Some(source) => Self::from_path(&source),
			None => Self::from_text(&cwd.join("rogue.toml"), DEFAULT_CONFIG),
		}
	}

	/// Load a specific config file. Useful for tests.
	pub fn from_path(source: &Path) -> Result<Self> {
		let bytes = fs::read_to_string(source)
			.with_context(|| format!("read config at {}", source.display()))?;
		Self::from_text(source, &bytes)
	}

	fn from_text(source: &Path, bytes: &str) -> Result<Self> {
		let raw: RawConfig = toml::from_str(bytes)
			.with_context(|| format!("parse config at {}", source.display()))?;
		let base = source
			.parent()
			.ok_or_else(|| anyhow!("config path has no parent: {}", source.display()))?;
		Ok(Self {
			source: source.to_path_buf(),
			assets_dir: resolve_relative(base, &raw.assets_dir),
			client_dir: resolve_relative(base, &raw.client_dir),
			gamedata_dir: resolve_relative(base, &raw.gamedata_dir),
		})
	}

	pub fn hot_update_dir(&self) -> PathBuf {
		self.client_dir.join("hot_update")
	}

	pub fn battle_prefab_dir(&self) -> PathBuf {
		self.assets_dir.join("analysis/battle-prefabs")
	}
}

fn resolve_config_path(
	cwd: &Path,
	executable: &Path,
	custom: Option<&Path>,
) -> Result<Option<PathBuf>> {
	if let Some(custom) = custom {
		let path: PathBuf = if custom.is_absolute() {
			custom.to_path_buf()
		} else {
			cwd.join(custom)
		};
		if !path.is_file() {
			return Err(anyhow!(
				"ROGUE_CONFIG={} does not point at a readable file",
				path.display()
			));
		}
		return Ok(Some(path));
	}
	for parent in cwd.ancestors() {
		let candidate: PathBuf = parent.join("rogue.toml");
		if candidate.is_file() {
			return Ok(Some(candidate));
		}
	}
	Ok(executable
		.parent()
		.map(|parent| parent.join("rogue.toml"))
		.filter(|path| path.is_file()))
}

fn resolve_relative(base: &Path, raw: &str) -> PathBuf {
	let candidate = PathBuf::from(raw);
	if candidate.is_absolute() {
		candidate
	} else {
		base.join(candidate)
	}
}

#[cfg(test)]
mod tests {
	use super::{resolve_config_path, Config, DEFAULT_CONFIG};
	use std::path::Path;

	#[test]
	fn built_in_defaults_are_rooted_in_the_runtime_directory() {
		let root = std::env::temp_dir().join("ark-config-defaults");
		let config: Config = Config::from_text(&root.join("rogue.toml"), DEFAULT_CONFIG).unwrap();
		assert!(config.source.is_absolute());
		assert!(config.assets_dir.is_absolute());
		assert!(config.client_dir.is_absolute());
		assert!(config.gamedata_dir.is_absolute());
		assert_eq!(config.assets_dir, root.join("assets"));
		assert_eq!(config.gamedata_dir, root.join("client/_gamedata"));
	}

	#[test]
	fn explicit_missing_config_never_falls_back_to_the_checkout() {
		assert!(resolve_config_path(
			Path::new(env!("CARGO_MANIFEST_DIR")),
			Path::new("ark-cli"),
			Some(Path::new("missing-explicit-configuration.toml"))
		)
		.is_err());
	}

	#[test]
	fn config_discovery_prefers_explicit_then_working_tree_then_executable() {
		let root = std::env::temp_dir().join(format!(
			"ark-config-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		let cwd = root.join("work/nested");
		let executable = root.join("install/ark-cli");
		std::fs::create_dir_all(&cwd).unwrap();
		std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
		let adjacent = root.join("install/rogue.toml");
		let ancestor = root.join("work/rogue.toml");
		let explicit = cwd.join("custom.toml");
		std::fs::write(&adjacent, DEFAULT_CONFIG).unwrap();
		assert_eq!(
			resolve_config_path(&cwd, &executable, None).unwrap(),
			Some(adjacent)
		);
		std::fs::write(&ancestor, DEFAULT_CONFIG).unwrap();
		assert_eq!(
			resolve_config_path(&cwd, &executable, None).unwrap(),
			Some(ancestor)
		);
		std::fs::write(&explicit, DEFAULT_CONFIG).unwrap();
		assert_eq!(
			resolve_config_path(&cwd, &executable, Some(Path::new("custom.toml"))).unwrap(),
			Some(explicit)
		);
		std::fs::remove_dir_all(&root).unwrap();
	}
}
