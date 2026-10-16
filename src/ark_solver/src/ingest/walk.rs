//! File / directory / zip traversal helpers.
//!
//! Yields `(source_label, bytes)` pairs for files whose extension marks them
//! as candidates for Spine inspection. We avoid reading huge unrelated files
//! into memory by filtering on extension first.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;
use zip::ZipArchive;

/// Extensions worth pulling into memory for Spine inspection.
///
/// `.ab` / `.bundle` are listed so that callers can detect Unity AssetBundles
/// and emit a clear "needs external extractor" message instead of silently
/// ignoring them.
const CANDIDATE_EXTS: &[&str] = &["skel", "bytes", "json", "atlas", "txt", "ab", "bundle"];

/// True if `name` ends with any extension we want to inspect.
///
/// Case-insensitive. `.skel.bytes` matches via the trailing `.bytes`.
pub fn is_candidate(name: &str) -> bool {
	let lower = name.to_ascii_lowercase();
	CANDIDATE_EXTS
		.iter()
		.any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Items yielded by `walk`. Each carries a human-readable label for the
/// source location (file path, or `archive!entry`) and the raw bytes.
pub struct Item {
	pub label: String,
	pub bytes: Vec<u8>,
}

/// Walk `root`, yielding candidate files. Pushes each into `sink` to keep the
/// API allocation-bounded — callers can short-circuit by returning early.
pub fn walk<F>(root: &Path, mut sink: F) -> Result<()>
where
	F: FnMut(Item) -> std::ops::ControlFlow<()>,
{
	let meta = fs::metadata(root).with_context(|| format!("stat {}", root.display()))?;

	if meta.is_file() {
		if is_zip_like(root) {
			return walk_zip(root, sink);
		}
		if is_candidate(&root.to_string_lossy()) {
			let bytes = fs::read(root).with_context(|| format!("read {}", root.display()))?;
			let _ = sink(Item {
				label: root.display().to_string(),
				bytes,
			});
		}
		return Ok(());
	}

	for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
		if !entry.file_type().is_file() {
			continue;
		}
		let path: &Path = entry.path();
		let name = path.to_string_lossy();
		if !is_candidate(&name) {
			continue;
		}
		match fs::read(path) {
			Ok(bytes) => {
				if let std::ops::ControlFlow::Break(()) = sink(Item {
					label: path.display().to_string(),
					bytes,
				}) {
					return Ok(());
				}
			}
			Err(err) => {
				eprintln!("skip {}: {err}", path.display());
			}
		}
	}
	Ok(())
}

fn is_zip_like(path: &Path) -> bool {
	matches!(
		path.extension().and_then(|s| s.to_str()),
		Some("apk") | Some("xapk") | Some("zip") | Some("jar"),
	)
}

fn walk_zip<F>(path: &Path, mut sink: F) -> Result<()>
where
	F: FnMut(Item) -> std::ops::ControlFlow<()>,
{
	let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
	let mut zip = ZipArchive::new(file).with_context(|| format!("read zip {}", path.display()))?;

	let names: Vec<String> = (0..zip.len())
		.filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
		.collect();

	let display_root = path.display().to_string();
	for name in names {
		if !is_candidate(&name) {
			continue;
		}
		let mut entry = match zip.by_name(&name) {
			Ok(e) => e,
			Err(err) => {
				eprintln!("skip {name}: {err}");
				continue;
			}
		};
		if entry.is_dir() {
			continue;
		}
		let mut bytes = Vec::with_capacity(entry.size() as usize);
		if let Err(err) = entry.read_to_end(&mut bytes) {
			eprintln!("skip {name}: {err}");
			continue;
		}
		if let std::ops::ControlFlow::Break(()) = sink(Item {
			label: format!("{display_root}!{name}"),
			bytes,
		}) {
			return Ok(());
		}
	}
	Ok(())
}

// Suppress "unused" warning when only one constructor path is exercised in
// tests; PathBuf import is real.
#[allow(dead_code)]
fn _path_buf_anchor(p: PathBuf) -> PathBuf {
	p
}
