use std::fs;

use std::path::PathBuf;

use anyhow::{ensure, Result};

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
	pub state: PathBuf,
	pub client: PathBuf,
}

impl RuntimeConfig {
	pub fn resolve(state: Option<PathBuf>, client: Option<PathBuf>) -> Result<Self> {
		let state: PathBuf = state
			.or_else(|| std::env::var_os("ARK_BATTLE_RUNTIME_STATE").map(PathBuf::from))
			.unwrap_or(std::env::current_dir()?.join("assets/battle-runtime"));
		let client: PathBuf = client
			.or_else(|| std::env::var_os("ARK_PC_CLIENT").map(PathBuf::from))
			.or_else(|| remembered(&state))
			.map_or_else(discover, Ok)?;
		ensure!(
			client.join("GameAssembly.dll").is_file(),
			"Client directory has no GameAssembly.dll: {}",
			client.display()
		);
		let client: PathBuf = ark_unity::io::external_path(&client.canonicalize()?);
		let state: PathBuf = ark_unity::io::resolved_destination(&state)?;
		ensure!(
			!state.starts_with(&client) && !client.starts_with(&state),
			"Owned state and the read-only client directory must not overlap"
		);
		fs::create_dir_all(&state)?;
		Ok(Self { state, client })
	}
}

/// The client directory `setup` recorded in this state.
fn remembered(state: &std::path::Path) -> Option<PathBuf> {
	let value: serde_json::Value = read_json(&state.join("client.json")).ok()?;
	value["client"].as_str().map(PathBuf::from)
}

/// The official client in a Hypergryph Launcher default location, when exactly one exists.
fn discover() -> Result<PathBuf> {
	let found: Vec<PathBuf> = (b'A'..=b'Z')
		.flat_map(|drive| {
			["", "Program Files/", "Program Files (x86)/"].map(move |prefix| {
				PathBuf::from(format!(
					"{}:/{prefix}Hypergryph Launcher/games/Arknights",
					drive as char
				))
			})
		})
		.filter(|path| path.join("GameAssembly.dll").is_file())
		.collect();
	ensure!(
		found.len() == 1,
		"Pass --client or set ARK_PC_CLIENT to the official PC client directory (found {} candidates)",
		found.len()
	);
	Ok(found[0].clone())
}

impl RuntimeConfig {
	/// Owned state alone, for maintainer tooling that never reads the official client.
	pub fn state_only(state: Option<PathBuf>) -> Result<Self> {
		let state: PathBuf = state
			.or_else(|| std::env::var_os("ARK_BATTLE_RUNTIME_STATE").map(PathBuf::from))
			.unwrap_or(std::env::current_dir()?.join("assets/battle-runtime"));
		fs::create_dir_all(&state)?;
		Ok(Self {
			state: ark_unity::io::resolved_destination(&state)?,
			client: PathBuf::new(),
		})
	}

	/// Remember the client so later commands need no --client.
	pub(crate) fn remember(&self) -> Result<()> {
		write_json(
			&self.state.join("client.json"),
			&serde_json::json!({"client": self.client}),
		)
	}
}

pub(crate) use ark_unity::io::{read_json, sha256_file, write_json};
