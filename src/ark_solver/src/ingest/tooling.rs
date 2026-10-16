//! Resolve explicitly installed external ingestion tools without a build-machine path.
use anyhow::{ensure, Context, Result};
use std::path::PathBuf;

pub(super) fn unpacker() -> Result<(PathBuf, PathBuf)> {
	let configured: Option<PathBuf> = std::env::var_os("ARK_UNPACKER_ROOT").map(PathBuf::from);
	let cwd: PathBuf = std::env::current_dir()?;
	let root: PathBuf = if let Some(root) = configured {
		root
	} else {
		let mut candidates: Vec<PathBuf> = cwd
			.ancestors()
			.map(|path| path.join("vendor/ark-unpacker"))
			.collect();
		if let Some(parent) = std::env::current_exe()?.parent() {
			candidates.push(parent.join("tools/ark-unpacker"));
		}
		candidates
			.into_iter()
			.find(|path| path.join("Main.py").is_file())
			.context("Set ARK_UNPACKER_ROOT to an installed ArkUnpacker directory")?
	};
	ensure!(
		root.join("Main.py").is_file(),
		"ArkUnpacker missing at {}; set ARK_UNPACKER_ROOT",
		root.display()
	);
	let root: PathBuf = root.canonicalize()?;
	let python: PathBuf = std::env::var_os("ARK_UNPACKER_PYTHON")
		.map(PathBuf::from)
		.unwrap_or_else(|| {
			root.join(if cfg!(windows) {
				".venv/Scripts/python.exe"
			} else {
				".venv/bin/python"
			})
		});
	ensure!(
		python.is_file(),
		"ArkUnpacker Python missing at {}; prepare its environment or set ARK_UNPACKER_PYTHON",
		python.display()
	);
	Ok((root, python.canonicalize()?))
}
