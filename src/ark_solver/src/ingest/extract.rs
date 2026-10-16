use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::Args;
use serde::Serialize;
use walkdir::WalkDir;

#[derive(Args, Debug, Clone)]
pub struct ExtractArgs {
	/// AssetBundle file or directory to unpack with ArkUnpacker.
	pub target: PathBuf,
	/// Root directory for standardized operator output.
	#[arg(short = 'o', long = "output", value_name = "DIR")]
	pub output: Option<PathBuf>,
	/// Override the operator id used for output layout.
	#[arg(long)]
	pub operator_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ManifestVariants {
	#[serde(skip_serializing_if = "Option::is_none")]
	front: Option<SpineVariant>,
	#[serde(skip_serializing_if = "Option::is_none")]
	back: Option<SpineVariant>,
	#[serde(skip_serializing_if = "Option::is_none")]
	build: Option<SpineVariant>,
	#[serde(skip_serializing_if = "Option::is_none")]
	portrait: Option<PortraitVariant>,
}

impl ManifestVariants {
	fn is_empty(&self) -> bool {
		self.front.is_none()
			&& self.back.is_none()
			&& self.build.is_none()
			&& self.portrait.is_none()
	}
}

#[derive(Debug, Clone, Serialize)]
struct SpineVariant {
	skel: String,
	atlas: String,
	png: String,
}

#[derive(Debug, Clone, Serialize)]
struct PortraitVariant {
	png: String,
}

#[derive(Debug, Serialize)]
struct Manifest {
	operator_id: String,
	source_bundle: String,
	extracted_at: String,
	variants: ManifestVariants,
}

struct RemovePathOnDrop {
	path: PathBuf,
	armed: bool,
}

impl RemovePathOnDrop {
	fn new(path: PathBuf) -> Self {
		Self { path, armed: true }
	}

	fn disarm(&mut self) {
		self.armed = false;
	}
}

impl Drop for RemovePathOnDrop {
	fn drop(&mut self) {
		if !self.armed || !self.path.exists() {
			return;
		}
		if self.path.is_dir() {
			let _ = fs::remove_dir_all(&self.path);
		} else {
			let _ = fs::remove_file(&self.path);
		}
	}
}

pub fn run(args: ExtractArgs) -> Result<()> {
	let input_path: PathBuf = canonicalize_input(&args.target)?;
	let output_root: PathBuf = match args.output.as_deref() {
		Some(output) => absolute_path(output)?,
		None => {
			Config::load()
				.context("load rogue config for extracted asset output")?
				.assets_dir
		}
	};
	let operator_id: String = resolve_operator_id(&input_path, args.operator_id.as_deref())?;
	let source_bundle: String = infer_source_bundle(&input_path, &operator_id);
	let (ark_root, python_path): (PathBuf, PathBuf) = super::tooling::unpacker()?;
	fs::create_dir_all(&output_root)
		.with_context(|| format!("create {}", output_root.display()))?;

	let stage_root: PathBuf = output_root.join(format!(
		".ark-unpacker-stage-{operator_id}-{}",
		unique_suffix()?
	));
	fs::create_dir_all(&stage_root).with_context(|| format!("create {}", stage_root.display()))?;
	let _stage_guard: RemovePathOnDrop = RemovePathOnDrop::new(stage_root.clone());

	run_ark_unpacker(&ark_root, &python_path, &input_path, &stage_root)?;

	let operators_root: PathBuf = output_root.join("operators");
	fs::create_dir_all(&operators_root)
		.with_context(|| format!("create {}", operators_root.display()))?;
	let operator_temp: PathBuf =
		operators_root.join(format!(".{operator_id}.partial-{}", unique_suffix()?));
	fs::create_dir_all(&operator_temp)
		.with_context(|| format!("create {}", operator_temp.display()))?;
	let mut operator_temp_guard: RemovePathOnDrop = RemovePathOnDrop::new(operator_temp.clone());

	let variants: ManifestVariants =
		collect_operator_assets(&stage_root, &operator_id, &operator_temp)?;
	if variants.is_empty() {
		bail!(
			"ArkUnpacker output for {} did not contain any expected operator assets",
			input_path.display()
		);
	}
	write_manifest(&operator_temp, &operator_id, &source_bundle, &variants)?;

	let operator_final: PathBuf = operators_root.join(&operator_id);
	remove_existing_path(&operator_final)?;
	fs::rename(&operator_temp, &operator_final).with_context(|| {
		format!(
			"move {} to {}",
			operator_temp.display(),
			operator_final.display()
		)
	})?;
	operator_temp_guard.disarm();

	eprintln!(
		"extracted {} -> {}",
		input_path.display(),
		operator_final.display()
	);
	Ok(())
}

fn canonicalize_input(path: &Path) -> Result<PathBuf> {
	path.canonicalize()
		.with_context(|| format!("canonicalize {}", path.display()))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
	if path.is_absolute() {
		return Ok(path.to_path_buf());
	}
	Ok(std::env::current_dir()
		.context("read current directory")?
		.join(path))
}

fn run_ark_unpacker(
	ark_root: &Path,
	python_path: &Path,
	input_path: &Path,
	output_path: &Path,
) -> Result<()> {
	let status = Command::new(python_path)
		.current_dir(ark_root)
		.arg("Main.py")
		.arg("-m")
		.arg("ab")
		.arg("-i")
		.arg(input_path)
		.arg("-o")
		.arg(output_path)
		.arg("--image")
		.arg("--text")
		.arg("--spine")
		.arg("-g")
		.arg("-l")
		.arg("3")
		.status()
		.with_context(|| format!("launch ArkUnpacker for {}", input_path.display()))?;
	if !status.success() {
		bail!("ArkUnpacker exited with status {status}");
	}
	Ok(())
}

fn resolve_operator_id(input_path: &Path, override_id: Option<&str>) -> Result<String> {
	if let Some(operator_id) = override_id {
		let trimmed: &str = operator_id.trim();
		if trimmed.is_empty() {
			bail!("--operator-id must not be empty");
		}
		return Ok(trimmed.to_string());
	}

	if input_path.is_file() {
		return operator_id_from_file_path(input_path)
			.with_context(|| format!("infer operator id from {}", input_path.display()));
	}

	let dir_name: Option<&str> = input_path.file_name().and_then(OsStr::to_str);
	if let Some(name) = dir_name {
		if name.starts_with("char_") {
			return Ok(name.to_string());
		}
	}

	let mut candidates: BTreeSet<String> = BTreeSet::new();
	for entry in WalkDir::new(input_path).max_depth(2) {
		let entry = entry.with_context(|| {
			format!("walk {} while inferring operator id", input_path.display())
		})?;
		if !entry.file_type().is_file() {
			continue;
		}
		let Some(stem) = operator_id_from_file_path(entry.path()).ok() else {
			continue;
		};
		if stem.starts_with("char_") {
			candidates.insert(stem);
		}
	}

	if candidates.len() == 1 {
		return Ok(candidates.into_iter().next().expect("single candidate"));
	}

	bail!(
		"could not infer operator id from {}. Pass --operator-id explicitly.",
		input_path.display()
	)
}

fn operator_id_from_file_path(path: &Path) -> Result<String> {
	let stem: &str = path
		.file_stem()
		.and_then(OsStr::to_str)
		.context("path has no UTF-8 file stem")?;
	Ok(stem.to_string())
}

fn infer_source_bundle(input_path: &Path, operator_id: &str) -> String {
	if let Some(logical_path) = logical_bundle_path(input_path) {
		return logical_path;
	}
	if let Some(file_name) = input_path.file_name().and_then(OsStr::to_str) {
		if file_name.ends_with(".ab") {
			if file_name.starts_with("char_") {
				return format!("chararts/{file_name}");
			}
			return file_name.to_string();
		}
	}
	format!("chararts/{operator_id}.ab")
}

fn logical_bundle_path(path: &Path) -> Option<String> {
	let mut parts: Vec<String> = Vec::new();
	let mut capture: bool = false;
	for component in path.components() {
		let part: &str = component.as_os_str().to_str()?;
		if capture {
			parts.push(part.to_string());
			continue;
		}
		if part == "chararts" {
			capture = true;
			parts.push(part.to_string());
		}
	}
	if capture && !parts.is_empty() {
		Some(parts.join("/"))
	} else {
		None
	}
}

fn collect_operator_assets(
	stage_root: &Path,
	operator_id: &str,
	output_dir: &Path,
) -> Result<ManifestVariants> {
	let mut variants: ManifestVariants = ManifestVariants::default();

	if copy_spine_variant(
		&stage_root.join("BattleFront").join(operator_id),
		operator_id,
		output_dir,
		"front",
	)? {
		variants.front = Some(SpineVariant {
			skel: "front.skel".to_string(),
			atlas: "front.atlas".to_string(),
			png: "front.png".to_string(),
		});
	}
	if copy_spine_variant(
		&stage_root.join("BattleBack").join(operator_id),
		operator_id,
		output_dir,
		"back",
	)? {
		variants.back = Some(SpineVariant {
			skel: "back.skel".to_string(),
			atlas: "back.atlas".to_string(),
			png: "back.png".to_string(),
		});
	}
	let build_stem: String = format!("build_{operator_id}");
	if copy_spine_variant(
		&stage_root.join("Building").join(&build_stem),
		&build_stem,
		output_dir,
		"build",
	)? {
		variants.build = Some(SpineVariant {
			skel: "build.skel".to_string(),
			atlas: "build.atlas".to_string(),
			png: "build.png".to_string(),
		});
	}

	let portrait_source: PathBuf = stage_root.join(format!("{operator_id}_1.png"));
	if portrait_source.is_file() {
		copy_file(&portrait_source, &output_dir.join("portrait.png"))?;
		variants.portrait = Some(PortraitVariant {
			png: "portrait.png".to_string(),
		});
	}

	Ok(variants)
}

fn copy_spine_variant(
	source_dir: &Path,
	source_stem: &str,
	output_dir: &Path,
	dest_prefix: &str,
) -> Result<bool> {
	let skel_source: PathBuf = source_dir.join(format!("{source_stem}.skel"));
	let atlas_source: PathBuf = source_dir.join(format!("{source_stem}.atlas"));
	let png_source: PathBuf = source_dir.join(format!("{source_stem}.png"));
	let any_present: bool = skel_source.is_file() || atlas_source.is_file() || png_source.is_file();
	let all_present: bool = skel_source.is_file() && atlas_source.is_file() && png_source.is_file();
	if any_present && !all_present {
		bail!(
			"incomplete {dest_prefix} variant in {}",
			source_dir.display()
		);
	}
	if !all_present {
		return Ok(false);
	}

	copy_file(
		&skel_source,
		&output_dir.join(format!("{dest_prefix}.skel")),
	)?;
	copy_file(
		&atlas_source,
		&output_dir.join(format!("{dest_prefix}.atlas")),
	)?;
	copy_file(&png_source, &output_dir.join(format!("{dest_prefix}.png")))?;
	Ok(true)
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
	if let Some(parent) = destination.parent() {
		fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
	}
	fs::copy(source, destination)
		.with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
	Ok(())
}

fn write_manifest(
	output_dir: &Path,
	operator_id: &str,
	source_bundle: &str,
	variants: &ManifestVariants,
) -> Result<()> {
	let manifest: Manifest = Manifest {
		operator_id: operator_id.to_string(),
		source_bundle: source_bundle.to_string(),
		extracted_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
		variants: variants.clone(),
	};
	let manifest_path: PathBuf = output_dir.join("manifest.json");
	let json: String =
		serde_json::to_string_pretty(&manifest).context("serialize extract manifest.json")?;
	fs::write(&manifest_path, format!("{json}\n"))
		.with_context(|| format!("write {}", manifest_path.display()))?;
	Ok(())
}

fn remove_existing_path(path: &Path) -> Result<()> {
	if !path.exists() {
		return Ok(());
	}
	let metadata =
		fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
	if metadata.file_type().is_dir() {
		fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
	} else {
		fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
	}
	Ok(())
}

fn unique_suffix() -> Result<u128> {
	Ok(SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.context("read system clock")?
		.as_millis())
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use super::{infer_source_bundle, logical_bundle_path, operator_id_from_file_path};

	#[test]
	fn operator_id_comes_from_file_stem() {
		let path: &Path = Path::new("char_002_amiya.ab");
		assert_eq!(operator_id_from_file_path(path).unwrap(), "char_002_amiya");
	}

	#[test]
	fn source_bundle_prefers_embedded_chararts_path() {
		let path: &Path = Path::new("/repo/cache/chararts/char_002_amiya.ab");
		assert_eq!(
			logical_bundle_path(path).unwrap(),
			"chararts/char_002_amiya.ab"
		);
	}

	#[test]
	fn source_bundle_defaults_to_chararts_for_operator_bundles() {
		let path: &Path = Path::new("/repo/client/probe/char_002_amiya.ab");
		assert_eq!(
			infer_source_bundle(path, "char_002_amiya"),
			"chararts/char_002_amiya.ab"
		);
	}
}
