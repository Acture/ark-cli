use std::fs;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};
use chrono::Utc;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
	let bytes: Vec<u8> = fs::read(path).with_context(|| format!("Read {}", path.display()))?;
	let bytes: &[u8] = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
	serde_json::from_slice(bytes).with_context(|| format!("Decode {}", path.display()))
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
	let mut bytes: Vec<u8> = serde_json::to_vec_pretty(value)?;
	bytes.push(b'\n');
	fs::write(path, bytes).with_context(|| format!("Write {}", path.display()))
}

pub fn sha256_file(path: &Path) -> Result<String> {
	let mut stream: BufReader<fs::File> = BufReader::new(fs::File::open(path)?);
	let mut hash: Sha256 = Sha256::new();
	let mut buffer: [u8; 65536] = [0; 65536];
	loop {
		let count: usize = stream.read(&mut buffer)?;
		if count == 0 {
			break;
		}
		hash.update(&buffer[..count]);
	}
	Ok(format!("{:x}", hash.finalize()))
}

pub fn pin(path: &Path) -> Result<Value> {
	Ok(json!({"path":path,"sha256":sha256_file(path)?}))
}

pub fn verify_pin(value: &Value) -> Result<PathBuf> {
	let path: PathBuf = PathBuf::from(
		value["path"]
			.as_str()
			.context("Pinned input path missing")?,
	);
	ensure!(
		value["sha256"].as_str() == Some(&sha256_file(&path)?),
		"Pinned input changed: {}",
		path.display()
	);
	Ok(path)
}

/// Resolve existing ancestors before creating anything, including junctions on Windows.
pub fn external_path(path: &Path) -> PathBuf {
	#[cfg(windows)]
	{
		use std::os::windows::ffi::{OsStrExt, OsStringExt};
		let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
		if wide.starts_with(&[92, 92, 63, 92]) && wide.get(5) == Some(&58) {
			return PathBuf::from(std::ffi::OsString::from_wide(&wide[4..]));
		}
	}
	path.to_path_buf()
}

pub fn resolved_destination(path: &Path) -> Result<PathBuf> {
	let absolute: PathBuf = std::path::absolute(path)?;
	ensure!(
		!absolute.components().any(|c| c == Component::ParentDir),
		"Parent traversal rejected"
	);
	if absolute.exists() {
		return Ok(external_path(&absolute.canonicalize()?));
	}
	let parent: &Path = absolute.parent().context("Destination has no parent")?;
	Ok(resolved_destination(parent)?.join(absolute.file_name().context("Missing filename")?))
}

pub fn owned(root: &Path, path: &Path) -> Result<PathBuf> {
	let root: PathBuf = external_path(&root.canonicalize()?);
	let path: PathBuf = resolved_destination(path)?;
	ensure!(
		path.starts_with(&root) && path != root,
		"Path leaves owned state: {}",
		path.display()
	);
	Ok(path)
}

pub fn stage_dir(root: &Path, kind: &str) -> Result<PathBuf> {
	ensure!(
		!kind.is_empty() && kind.bytes().all(|x| x.is_ascii_alphanumeric() || x == b'-'),
		"Invalid stage"
	);
	let base: PathBuf = owned(root, &root.join(kind))?;
	fs::create_dir_all(&base)?;
	let path: PathBuf = base.join(Utc::now().format("%Y%m%dT%H%M%S%fZ").to_string());
	fs::create_dir(&path)?;
	Ok(path)
}

pub fn copy_tree(source: &Path, target: &Path) -> Result<()> {
	ensure!(
		!target.exists(),
		"Copy destination already exists: {}",
		target.display()
	);
	fs::create_dir_all(target)?;
	for entry in walkdir::WalkDir::new(source) {
		let entry = entry?;
		ensure!(
			!entry.file_type().is_symlink(),
			"Player tree contains a symbolic link"
		);
		let relative: &Path = entry.path().strip_prefix(source)?;
		let destination: PathBuf = target.join(relative);
		if entry.file_type().is_dir() {
			fs::create_dir_all(destination)?;
		} else {
			fs::copy(entry.path(), destination)?;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn destination_resolves_parents_before_creation() {
		let root: PathBuf = external_path(&std::env::temp_dir().canonicalize().unwrap());
		assert_eq!(
			resolved_destination(&root.join("ark-uncreated/a/b")).unwrap(),
			root.join("ark-uncreated/a/b")
		);
		assert!(owned(&root, &root).is_err());
		assert!(owned(&root, root.parent().unwrap()).is_err());
	}
}
