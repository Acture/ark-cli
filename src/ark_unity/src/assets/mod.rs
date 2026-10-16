//! Read-only original resource resolution and byte-preserving owned Unity inputs.
//!
//! No Python interpreter or game-specific battle model participates in this layer.

mod bundle;
mod catalog;
mod serialized;
mod services;
mod tpk;
mod trees;

pub use catalog::Catalog;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const UNITY_VERSION: &str = "2021.3.39f1";

pub(crate) fn sha256(data: &[u8]) -> String {
	format!("{:x}", Sha256::digest(data))
}

pub(crate) fn pin(path: &Path) -> Result<Value> {
	Ok(json!({"path": fs::canonicalize(path)?, "sha256": sha256(&fs::read(path)?)}))
}

pub(crate) fn read_json(path: &Path) -> Result<Value> {
	let data: Vec<u8> = fs::read(path).with_context(|| format!("Read {}", path.display()))?;
	Ok(serde_json::from_slice(
		data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&data),
	)?)
}

pub(crate) fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
	let mut bytes: Vec<u8> = serde_json::to_vec_pretty(value)?;
	bytes.push(b'\n');
	fs::write(path, bytes).with_context(|| format!("Write {}", path.display()))
}

pub(crate) fn stage(state: &Path, name: &str) -> Result<PathBuf> {
	crate::io::stage_dir(state, name)
}

pub(crate) fn bounded_join(root: &Path, route: &str) -> Result<PathBuf> {
	ensure!(
		!route.is_empty() && !route.contains('\\') && !route.contains(':'),
		"Invalid resource route: {route}"
	);
	ensure!(
		Path::new(route)
			.components()
			.all(|part| matches!(part, Component::Normal(_))),
		"Resource route leaves archive: {route}"
	);
	let path: PathBuf = root.join(route);
	if path.exists() {
		ensure!(
			fs::canonicalize(&path)?.starts_with(fs::canonicalize(root)?),
			"Resource symlink leaves archive: {route}"
		);
	}
	Ok(path)
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
	value
		.get(key)
		.and_then(Value::as_str)
		.with_context(|| format!("Missing string {key}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn resource_routes_cannot_escape_archive() {
		for route in ["../other", "/absolute", "C:/client", "a/../../b", "a\\b"] {
			assert!(bounded_join(Path::new("."), route).is_err(), "{route}");
		}
		assert_eq!(
			bounded_join(Path::new("."), "scenes/test.ab").unwrap(),
			Path::new(".").join("scenes/test.ab")
		);
	}
}
