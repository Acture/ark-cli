//! Identify the Spine runtime version of skeleton files.
//!
//! Two parsers, both cheap and string-based:
//!
//! - **JSON Spine** (`.json`): a top-level object with `{"skeleton": {"spine":
//!   "4.2.33"}}`. We use `serde_json` to extract that single field. We avoid
//!   defining a typed schema for the entire skeleton — we only care about one
//!   string, and Spine skeleton JSON is large.
//! - **Binary Spine** (`.skel`, `.skel.bytes`): a length-prefixed hash string
//!   followed by a length-prefixed version string near the start of the file.
//!   Rather than fully implementing the binary format (which differs across
//!   Spine 3.8 / 4.0 / 4.1 / 4.2), we scan the first ~512 bytes for an ASCII
//!   `\d+\.\d+\.\d+` pattern with a leading major version of 3 or 4. This is
//!   conservative and good enough to choose a runtime stack.
//!
//! When we encounter a Unity AssetBundle (`.ab`, `.bundle`) we emit a
//! one-shot warning instead of attempting to parse it: Hypergryph wraps AB
//! contents in a non-standard block compression that we deliberately do not
//! handle in-tree.

use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use regex::bytes::Regex;
use serde::Deserialize;

use crate::ingest::walk::{self, Item};

#[derive(Debug)]
pub struct Hit {
	pub source: String,
	pub kind: &'static str,
	pub version: String,
}

#[derive(Deserialize)]
struct SpineJsonRoot {
	skeleton: Option<SpineJsonSkeleton>,
}

#[derive(Deserialize)]
struct SpineJsonSkeleton {
	spine: Option<String>,
}

fn version_from_json(bytes: &[u8]) -> Option<String> {
	let root: SpineJsonRoot = serde_json::from_slice(bytes).ok()?;
	root.skeleton?.spine
}

fn version_from_binary(bytes: &[u8]) -> Option<String> {
	static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
	let re = RE.get_or_init(|| Regex::new(r"(\d+\.\d+\.\d+(?:[\w\-]*)?)").expect("static regex"));
	let head = &bytes[..bytes.len().min(512)];
	let cap = re.captures(head)?;
	let candidate = std::str::from_utf8(cap.get(1)?.as_bytes()).ok()?;
	let major = candidate.split_once('.')?.0;
	if major == "3" || major == "4" {
		Some(candidate.to_string())
	} else {
		None
	}
}

/// Classify one item and yield zero or one Hit.
fn classify(item: &Item) -> Option<Hit> {
	let lower = item.label.to_ascii_lowercase();

	if lower.ends_with(".ab") || lower.ends_with(".bundle") {
		warn_unity_bundle_once();
		return None;
	}

	if lower.ends_with(".skel") || lower.ends_with(".skel.bytes") || lower.ends_with(".bytes") {
		return version_from_binary(&item.bytes).map(|v| Hit {
			source: item.label.clone(),
			kind: "skel(binary)",
			version: v,
		});
	}
	if lower.ends_with(".json") {
		return version_from_json(&item.bytes).map(|v| Hit {
			source: item.label.clone(),
			kind: "skel(json)",
			version: v,
		});
	}
	None
}

static UNITY_WARNED: AtomicBool = AtomicBool::new(false);
fn warn_unity_bundle_once() {
	if UNITY_WARNED
		.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
		.is_ok()
	{
		eprintln!(
			"note: Unity AssetBundle (.ab) files were found but are not parsed in-tree.\n\
			 Arknights uses a non-standard block compression. Use an external\n\
			 community tool (e.g. an AssetStudio fork that supports MRFZ) to\n\
			 extract .skel/.atlas/.png files first, then re-run this command\n\
			 on the extracted directory."
		);
	}
}

pub fn run(target: &Path, limit: usize) -> Result<()> {
	let mut hits: Vec<Hit> = Vec::new();
	walk::walk(target, |item| {
		if let Some(hit) = classify(&item) {
			hits.push(hit);
			if limit != 0 && hits.len() >= limit {
				return ControlFlow::Break(());
			}
		}
		ControlFlow::Continue(())
	})?;

	if hits.is_empty() {
		eprintln!(
			"\nNo Spine skeletons found.\n\
			 If you scanned a raw APK or AssetBundle directory, the relevant\n\
			 Spine files are inside .ab bundles which this tool does not parse.\n\
			 Extract them with an external tool first."
		);
		return Err(anyhow::anyhow!("No Spine skeletons found"));
	}

	let mut versions: BTreeMap<&'static str, usize> = BTreeMap::new();
	println!("Spine hits in {}:", target.display());
	println!("{:<10} {:<14} source", "version", "kind");
	for hit in &hits {
		let major: &'static str = match hit.version.split('.').next() {
			Some("3") => "3.x",
			Some("4") => "4.x",
			_ => "?",
		};
		*versions.entry(major).or_default() += 1;
		println!("{:<10} {:<14} {}", hit.version, hit.kind, hit.source);
	}

	println!("\nSummary:");
	for (major, count) in &versions {
		println!("  Spine {major}: {count} hit(s)");
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	#[test]
	fn empty_scan_returns_an_error_to_the_caller() {
		let path = std::env::temp_dir().join(format!(
			"ark-empty-spine-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		std::fs::create_dir(&path).unwrap();
		let result = super::run(&path, 0);
		std::fs::remove_dir(&path).unwrap();
		assert_eq!(result.unwrap_err().to_string(), "No Spine skeletons found");
	}
}
