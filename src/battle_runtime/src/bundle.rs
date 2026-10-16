//! The exact tooling bytes embedded in this executable; never read a build-machine checkout.
use anyhow::{ensure, Result};
use ark_unity::io;
use sha2::{Digest, Sha256};
use std::{
	fs,
	path::{Path, PathBuf},
};

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

pub(crate) use ark_battle_runtime_probe::FILES as PROBE_FILES;

pub(crate) fn materialize(state: &Path, kind: &str, files: &[(&str, &[u8])]) -> Result<PathBuf> {
	let mut digest: Sha256 = Sha256::new();
	for (name, bytes) in files {
		digest.update(name.as_bytes());
		digest.update(b"\0");
		digest.update(bytes);
		digest.update(b"\0");
	}
	let hash: String = format!("{:x}", digest.finalize());
	let root: PathBuf = io::owned(state, &state.join("embedded").join(kind).join(&hash[..24]))?;
	fs::create_dir_all(&root)?;
	for (name, bytes) in files {
		let path: PathBuf = io::owned(&root, &root.join(name))?;
		if path.exists() {
			ensure!(
				fs::read(&path)? == *bytes,
				"Embedded tooling changed: {}",
				path.display()
			);
		} else {
			fs::create_dir_all(path.parent().unwrap())?;
			fs::write(&path, bytes)?;
		}
	}
	Ok(root)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn embedded_tooling_roundtrips_and_refuses_modified_cached_sources() -> Result<()> {
		let state: PathBuf = std::env::temp_dir().join(format!(
			"ark-embedded-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)?
				.as_nanos()
		));
		fs::create_dir_all(&state)?;
		let root: PathBuf = materialize(&state, "probe", PROBE_FILES)?;
		assert_eq!(
			fs::read(root.join("Cargo.toml"))?,
			PROBE_FILES
				.iter()
				.find(|(name, _)| *name == "Cargo.toml")
				.unwrap()
				.1
		);
		assert!(root.join("src/battle-probe.rs").is_file());
		assert_eq!(materialize(&state, "probe", PROBE_FILES)?, root);
		fs::write(root.join("Cargo.toml"), b"changed")?;
		assert!(materialize(&state, "probe", PROBE_FILES).is_err());
		assert!(materialize(&state, "escape", &[("../outside", b"x")]).is_err());
		fs::remove_dir_all(io::owned(&std::env::temp_dir(), &state)?)?;
		Ok(())
	}
}
