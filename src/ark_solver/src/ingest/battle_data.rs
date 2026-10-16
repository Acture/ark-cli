use std::{
	collections::{BTreeMap, BTreeSet},
	fs::{self, File},
	io::{Read, Seek, SeekFrom, Write},
	path::{absolute, Path, PathBuf},
	process::Command,
	time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::config::Config;
use crate::data::{
	compute_mechanics_revision, ArkDataSnapshot, BattlePrefabEntryKind, BattlePrefabManifestEntry,
	BattlePrefabSnapshot, BattlePrefabSource, BattlePrefabTooling,
};
use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::Args;
use regex::Regex;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fs::{renameat_with, RenameFlags, CWD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::ingest::gamedata_snapshot::sha256_file;

const MANIFEST_SCHEMA_VERSION: u32 = 2;
const APK_BUNDLE_ROOT: &str = "assets/AB/Android/battle/prefabs";
const APK_CHARPACK_ROOT: &str = "assets/AB/Android/charpack";
const APK_ENEMYPACK_ROOT: &str = "assets/AB/Android/battle";
const APK_ENEMYPACK_PREFIX: &str = "enm_pfb_";
const COMMON_BUNDLES: [&str; 7] = [
	"[uc]skills",
	"[uc]dynamicabilities",
	"[uc]tiles",
	"[uc]envsystems",
	"[uc]equips",
	"[uc]globalbuffs",
	"[uc]projectiles",
];

#[derive(Debug, Args)]
pub struct ExtractBattleDataArgs {
	/// User-owned Android APK containing the official battle AssetBundles.
	pub apk: PathBuf,
	/// Verified gamedata snapshot whose revision this prefab snapshot targets.
	#[arg(long, value_name = "PATH")]
	pub gamedata_snapshot: PathBuf,
	/// New snapshot directory to publish. The path must not already exist.
	#[arg(short, long, value_name = "PATH")]
	pub output: Option<PathBuf>,
	/// Include one charpack by full operator id or unique short key. Repeatable.
	#[arg(long = "operator", value_name = "KEY")]
	pub operators: Vec<String>,
	/// Also export every `enm_pfb_*` enemy battle-prefab bundle.
	///
	/// The APK batches enemy prefabs into packs rather than one file per record, and no pinned
	/// table says which pack holds which enemy, so the packs are all-or-nothing.
	#[arg(long)]
	pub enemy_packs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BundleSelection {
	logical_path: String,
	bundle_kind: BattlePrefabEntryKind,
	source_apk_path: String,
	export_path: String,
}

#[derive(Debug, Clone)]
struct ExtractedBundle {
	selection: BundleSelection,
	staged_path: PathBuf,
	source_sha256: String,
	source_byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BattleDataManifest {
	schema_version: u32,
	mechanics_revision: String,
	gamedata_revision: String,
	generated_at: String,
	source: BattlePrefabSource,
	tooling: BattlePrefabTooling,
	entries: Vec<BattlePrefabManifestEntry>,
}

#[derive(Debug, Serialize)]
struct ExportJob {
	logical_path: String,
	source_bundle: String,
	source_path: String,
	output_path: String,
	source_sha256: String,
}

#[derive(Debug, Deserialize)]
struct ExportToolReport {
	exporter_version: String,
	python_version: String,
	unitypy_version: String,
	ark_unpacker_version: String,
}

pub fn run(args: ExtractBattleDataArgs) -> Result<()> {
	validate_regular_file(&args.apk, "APK")?;
	let requested_output: PathBuf = match args.output.as_deref() {
		Some(output) => output.to_path_buf(),
		None => Config::load()
			.context("load rogue config for battle-prefab output")?
			.battle_prefab_dir(),
	};
	let output: PathBuf = prepare_output_path(&requested_output)?;
	let (ark_unpacker_root, python): (PathBuf, PathBuf) = super::tooling::unpacker()?;
	validate_resolved_regular_file(&python, "ArkUnpacker Python")?;

	eprintln!(
		"[1/5] Verifying gamedata snapshot {}...",
		args.gamedata_snapshot.display()
	);
	let verify_started: Instant = Instant::now();
	let gamedata: ArkDataSnapshot =
		ArkDataSnapshot::load(&args.gamedata_snapshot).with_context(|| {
			format!(
				"verify gamedata snapshot {}",
				args.gamedata_snapshot.display()
			)
		})?;
	let gamedata_revision: String = gamedata.resolved_revision().to_owned();
	eprintln!(
		"  revision {gamedata_revision} verified in {:.2?}",
		verify_started.elapsed()
	);
	eprintln!("[2/5] Indexing selected APK bundles...");
	let apk_hash_started: Instant = Instant::now();
	let (mut archive, source): (ZipArchive<File>, BattlePrefabSource) = open_apk(&args.apk)?;
	eprintln!(
		"  APK identity hashed in {:.2?}",
		apk_hash_started.elapsed()
	);
	let archive_names: BTreeSet<String> = archive_names(&mut archive)?;
	let selections: Vec<BundleSelection> =
		select_bundles(&archive_names, &args.operators, args.enemy_packs)?;
	eprintln!("  selected {} bundles", selections.len());

	let mut staging: StagingDirectory = StagingDirectory::new(&output)?;
	let source_root: PathBuf = staging.path().join(".source-bundles");
	fs::create_dir(&source_root)
		.with_context(|| format!("create bundle staging root {}", source_root.display()))?;

	eprintln!("[3/5] Extracting and hashing selected bundles...");
	let mut extracted: Vec<ExtractedBundle> = Vec::with_capacity(selections.len());
	let selection_count: usize = selections.len();
	let extraction_started: Instant = Instant::now();
	for (index, selection) in selections.into_iter().enumerate() {
		eprintln!(
			"  [{}/{}] {}",
			index + 1,
			selection_count,
			selection.logical_path
		);
		extracted.push(extract_bundle(&mut archive, &source_root, selection)?);
	}
	let mut apk_file: File = archive.into_inner();
	let source_after_extraction: BattlePrefabSource = hash_open_apk(&mut apk_file, &args.apk)?;
	if source_after_extraction != source {
		bail!("APK changed while selected bundles were being extracted");
	}
	drop(apk_file);
	eprintln!("  extracted in {:.2?}", extraction_started.elapsed());

	eprintln!("[4/5] Exporting rich TypeTree documents...");
	let exporter: PathBuf = staging.path().join(".export_unity_typetree.py");
	fs::write(
		&exporter,
		include_bytes!("../../tools/export_unity_typetree.py"),
	)?;
	let jobs_path: PathBuf = staging.path().join(".export-jobs.json");
	let report_path: PathBuf = staging.path().join(".export-report.json");
	let jobs: Vec<ExportJob> = extracted
		.iter()
		.map(|bundle| ExportJob {
			logical_path: bundle.selection.logical_path.clone(),
			source_bundle: bundle.selection.source_apk_path.clone(),
			source_path: bundle.staged_path.to_string_lossy().into_owned(),
			output_path: staging
				.path()
				.join(&bundle.selection.export_path)
				.to_string_lossy()
				.into_owned(),
			source_sha256: bundle.source_sha256.clone(),
		})
		.collect();
	write_json_file(&jobs_path, &jobs)?;
	let export_started: Instant = Instant::now();
	let status = Command::new(&python)
		.arg(&exporter)
		.arg("--jobs")
		.arg(&jobs_path)
		.arg("--ark-unpacker-root")
		.arg(&ark_unpacker_root)
		.arg("--report")
		.arg(&report_path)
		.status()
		.with_context(|| format!("run TypeTree exporter with {}", python.display()))?;
	if !status.success() {
		bail!("TypeTree exporter exited with {status}");
	}
	eprintln!("  exported in {:.2?}", export_started.elapsed());
	let report: ExportToolReport = read_json_file(&report_path)?;
	let tooling: BattlePrefabTooling = BattlePrefabTooling {
		ark_ingest_version: env!("CARGO_PKG_VERSION").to_owned(),
		exporter_version: report.exporter_version,
		python_version: report.python_version,
		unitypy_version: report.unitypy_version,
		ark_unpacker_version: report.ark_unpacker_version,
	};

	eprintln!("[5/5] Hashing and atomically publishing snapshot...");
	let mut entries: Vec<BattlePrefabManifestEntry> = extracted
		.into_iter()
		.map(|bundle| build_manifest_entry(staging.path(), bundle))
		.collect::<Result<Vec<_>>>()?;
	entries.sort_by(|left, right| {
		left.logical_path
			.as_bytes()
			.cmp(right.logical_path.as_bytes())
			.then_with(|| {
				left.export_path
					.as_bytes()
					.cmp(right.export_path.as_bytes())
			})
	});
	let mechanics_revision: String =
		compute_mechanics_revision(&gamedata_revision, &source, &tooling, &entries);
	let manifest: BattleDataManifest = BattleDataManifest {
		schema_version: MANIFEST_SCHEMA_VERSION,
		mechanics_revision: mechanics_revision.clone(),
		gamedata_revision,
		generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
		source,
		tooling,
		entries,
	};
	write_json_file(&staging.path().join("manifest.json"), &manifest)?;
	remove_regular_file(&jobs_path)?;
	remove_regular_file(&report_path)?;
	remove_regular_file(&exporter)?;
	remove_staged_sources(&source_root)?;
	let verified: BattlePrefabSnapshot = BattlePrefabSnapshot::load(staging.path())
		.with_context(|| format!("verify staged prefab snapshot {}", staging.path().display()))?;
	verified
		.ensure_compatible_with(&gamedata)
		.context("verify staged prefab/gamedata revision pairing")?;
	if verified.mechanics_revision() != mechanics_revision {
		bail!(
			"staged snapshot revision {} does not match manifest revision {mechanics_revision}",
			verified.mechanics_revision()
		);
	}
	staging.publish(&output)?;

	eprintln!("Battle prefab snapshot published.");
	eprintln!("  path:      {}", output.display());
	eprintln!("  revision:  {mechanics_revision}");
	Ok(())
}

fn open_apk(path: &Path) -> Result<(ZipArchive<File>, BattlePrefabSource)> {
	let mut file: File =
		File::open(path).with_context(|| format!("open APK {}", path.display()))?;
	let source: BattlePrefabSource = hash_open_apk(&mut file, path)?;
	let archive: ZipArchive<File> =
		ZipArchive::new(file).with_context(|| format!("read APK archive {}", path.display()))?;
	Ok((archive, source))
}

fn hash_open_apk(file: &mut File, path: &Path) -> Result<BattlePrefabSource> {
	file.seek(SeekFrom::Start(0))
		.with_context(|| format!("seek APK {}", path.display()))?;
	let mut digest = Sha256::new();
	let mut buffer: [u8; 1024 * 1024] = [0; 1024 * 1024];
	let mut byte_length: u64 = 0;
	loop {
		let read: usize = file
			.read(&mut buffer)
			.with_context(|| format!("hash APK {}", path.display()))?;
		if read == 0 {
			break;
		}
		digest.update(&buffer[..read]);
		byte_length = byte_length
			.checked_add(u64::try_from(read).expect("read length fits u64"))
			.context("APK byte length overflow")?;
	}
	if byte_length == 0 {
		bail!("APK is empty: {}", path.display());
	}
	Ok(BattlePrefabSource {
		apk_sha256: format!("{:x}", digest.finalize()),
		apk_byte_length: byte_length,
	})
}

fn archive_names(archive: &mut ZipArchive<File>) -> Result<BTreeSet<String>> {
	let mut names: BTreeSet<String> = BTreeSet::new();
	for index in 0..archive.len() {
		let entry = archive
			.by_index(index)
			.with_context(|| format!("read APK archive entry {index}"))?;
		if !names.insert(entry.name().to_owned()) {
			bail!("APK contains duplicate entry {:?}", entry.name());
		}
	}
	Ok(names)
}

fn select_bundles(
	archive_names: &BTreeSet<String>,
	operator_requests: &[String],
	include_enemy_packs: bool,
) -> Result<Vec<BundleSelection>> {
	let mut selected: BTreeMap<String, BundleSelection> = BTreeMap::new();
	for bundle in COMMON_BUNDLES {
		let source_apk_path: String = format!("{APK_BUNDLE_ROOT}/{bundle}.ab");
		if !archive_names.contains(&source_apk_path) {
			bail!("APK is missing required common bundle {source_apk_path:?}");
		}
		selected.insert(
			bundle.to_owned(),
			BundleSelection {
				logical_path: bundle.to_owned(),
				bundle_kind: BattlePrefabEntryKind::Common,
				source_apk_path,
				export_path: format!("typetree/common/{bundle}.json"),
			},
		);
	}

	let available_charpacks: BTreeMap<String, String> = archive_names
		.iter()
		.filter_map(|path| charpack_identity(path).map(|identity| (identity, path.clone())))
		.collect();
	let requested_charpacks: BTreeSet<String> = operator_requests
		.iter()
		.map(|request| resolve_operator_request(request, &available_charpacks))
		.collect::<Result<_>>()?;
	for identity in requested_charpacks {
		let source_apk_path: String =
			available_charpacks
				.get(&identity)
				.cloned()
				.with_context(|| {
					format!("resolved charpack {identity:?} disappeared from APK index")
				})?;
		selected.insert(
			identity.clone(),
			BundleSelection {
				logical_path: identity.clone(),
				bundle_kind: BattlePrefabEntryKind::Charpack,
				source_apk_path,
				export_path: format!("typetree/charpack/{identity}.json"),
			},
		);
	}
	if include_enemy_packs {
		let packs: Vec<String> = archive_names
			.iter()
			.filter(|path| enemy_pack_identity(path).is_some())
			.cloned()
			.collect();
		if packs.is_empty() {
			bail!("APK contains no {APK_ENEMYPACK_ROOT}/{APK_ENEMYPACK_PREFIX}*.ab bundle");
		}
		for source_apk_path in packs {
			let identity: String =
				enemy_pack_identity(&source_apk_path).expect("filtered paths are enemy packs");
			selected.insert(
				identity.clone(),
				BundleSelection {
					logical_path: identity.clone(),
					bundle_kind: BattlePrefabEntryKind::Enemypack,
					source_apk_path,
					export_path: format!("typetree/enemypack/{identity}.json"),
				},
			);
		}
	}

	let mut result: Vec<BundleSelection> = selected.into_values().collect();
	result.sort_by(|left, right| {
		left.logical_path
			.as_bytes()
			.cmp(right.logical_path.as_bytes())
	});
	Ok(result)
}

fn enemy_pack_identity(path: &str) -> Option<String> {
	let prefix: String = format!("{APK_ENEMYPACK_ROOT}/");
	let file_name: &str = path.strip_prefix(&prefix)?.strip_suffix(".ab")?;
	if file_name.contains('/') {
		return None;
	}
	let index: &str = file_name.strip_prefix(APK_ENEMYPACK_PREFIX)?;
	let is_index: bool = !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit());
	is_index.then(|| file_name.to_owned())
}

fn charpack_identity(path: &str) -> Option<String> {
	let prefix: String = format!("{APK_CHARPACK_ROOT}/");
	let file_name: &str = path.strip_prefix(&prefix)?.strip_suffix(".ab")?;
	if file_name.contains('/') || !is_full_operator_id(file_name) {
		return None;
	}
	Some(file_name.to_owned())
}

fn resolve_operator_request(request: &str, available: &BTreeMap<String, String>) -> Result<String> {
	if request.is_empty()
		|| !request
			.bytes()
			.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
	{
		bail!(
			"invalid operator key {request:?}; expected lowercase ASCII letters, digits, and underscore"
		);
	}
	if is_full_operator_id(request) {
		if available.contains_key(request) {
			return Ok(request.to_owned());
		}
		bail!("APK has no charpack for operator id {request:?}");
	}
	let suffix: String = format!("_{request}");
	let matches: Vec<&String> = available
		.keys()
		.filter(|identity| identity.ends_with(&suffix))
		.collect();
	match matches.as_slice() {
		[] => bail!("no APK charpack matched short operator key {request:?}"),
		[identity] => Ok((*identity).clone()),
		_ => bail!(
			"short operator key {request:?} matched multiple charpacks: {}; pass a full operator id",
			matches
				.iter()
				.map(|identity| identity.as_str())
				.collect::<Vec<_>>()
				.join(", ")
		),
	}
}

fn is_full_operator_id(value: &str) -> bool {
	Regex::new(r"^char_[0-9]+_[a-z0-9_]+$")
		.expect("operator id regex is valid")
		.is_match(value)
}

fn extract_bundle(
	archive: &mut ZipArchive<File>,
	source_root: &Path,
	selection: BundleSelection,
) -> Result<ExtractedBundle> {
	let staged_path: PathBuf = source_root.join(format!(
		"{:x}.ab",
		Sha256::digest(selection.logical_path.as_bytes())
	));
	let mut entry = archive
		.by_name(&selection.source_apk_path)
		.with_context(|| format!("read APK bundle {}", selection.source_apk_path))?;
	let expected_length: u64 = entry.size();
	let mut output: File = File::create(&staged_path)
		.with_context(|| format!("create staged bundle {}", staged_path.display()))?;
	let mut digest: Sha256 = Sha256::new();
	let mut byte_length: u64 = 0;
	let mut buffer: [u8; 64 * 1024] = [0; 64 * 1024];
	loop {
		let read: usize = entry
			.read(&mut buffer)
			.with_context(|| format!("read APK bundle {}", selection.source_apk_path))?;
		if read == 0 {
			break;
		}
		output
			.write_all(&buffer[..read])
			.with_context(|| format!("write staged bundle {}", staged_path.display()))?;
		digest.update(&buffer[..read]);
		byte_length += read as u64;
	}
	output
		.sync_all()
		.with_context(|| format!("sync staged bundle {}", staged_path.display()))?;
	if byte_length != expected_length {
		bail!(
			"APK bundle {} length changed while extracting: expected {expected_length}, read {byte_length}",
			selection.source_apk_path
		);
	}
	Ok(ExtractedBundle {
		selection,
		staged_path,
		source_sha256: format!("{:x}", digest.finalize()),
		source_byte_length: byte_length,
	})
}

fn build_manifest_entry(root: &Path, bundle: ExtractedBundle) -> Result<BattlePrefabManifestEntry> {
	let export: PathBuf = root.join(&bundle.selection.export_path);
	validate_regular_file(&export, "rich TypeTree export")?;
	Ok(BattlePrefabManifestEntry {
		logical_path: bundle.selection.logical_path,
		bundle_kind: bundle.selection.bundle_kind,
		source_apk_path: bundle.selection.source_apk_path,
		source_sha256: bundle.source_sha256,
		source_byte_length: bundle.source_byte_length,
		export_path: bundle.selection.export_path,
		export_sha256: sha256_file(&export)?,
		export_byte_length: export
			.metadata()
			.with_context(|| format!("stat rich TypeTree export {}", export.display()))?
			.len(),
	})
}

fn prepare_output_path(requested: &Path) -> Result<PathBuf> {
	let requested: PathBuf = absolute(requested)
		.with_context(|| format!("resolve output path {}", requested.display()))?;
	match fs::symlink_metadata(&requested) {
		Ok(_) => bail!("output path already exists: {}", requested.display()),
		Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
		Err(source) => {
			return Err(source)
				.with_context(|| format!("inspect output path {}", requested.display()));
		}
	}
	let parent: &Path = requested.parent().context("output path has no parent")?;
	fs::create_dir_all(parent)
		.with_context(|| format!("create output parent {}", parent.display()))?;
	let parent: PathBuf = parent
		.canonicalize()
		.with_context(|| format!("canonicalize output parent {}", parent.display()))?;
	let file_name = requested
		.file_name()
		.context("output path has no file name")?;
	Ok(parent.join(file_name))
}

fn validate_regular_file(path: &Path, label: &str) -> Result<()> {
	let metadata: fs::Metadata =
		fs::symlink_metadata(path).with_context(|| format!("stat {label} {}", path.display()))?;
	if !metadata.file_type().is_file() {
		bail!("{label} is not a regular file: {}", path.display());
	}
	Ok(())
}

fn validate_resolved_regular_file(path: &Path, label: &str) -> Result<()> {
	let metadata: fs::Metadata =
		fs::metadata(path).with_context(|| format!("stat resolved {label} {}", path.display()))?;
	if !metadata.is_file() {
		bail!(
			"{label} does not resolve to a regular file: {}",
			path.display()
		);
	}
	Ok(())
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<()> {
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)
			.with_context(|| format!("create JSON parent {}", parent.display()))?;
	}
	let mut bytes: Vec<u8> = serde_json::to_vec_pretty(value)
		.with_context(|| format!("serialize JSON {}", path.display()))?;
	bytes.push(b'\n');
	let mut file: File =
		File::create(path).with_context(|| format!("create JSON file {}", path.display()))?;
	file.write_all(&bytes)
		.with_context(|| format!("write JSON file {}", path.display()))?;
	file.sync_all()
		.with_context(|| format!("sync JSON file {}", path.display()))
}

fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
	let file: File = File::open(path).with_context(|| format!("open JSON {}", path.display()))?;
	serde_json::from_reader(file).with_context(|| format!("parse JSON {}", path.display()))
}

fn remove_regular_file(path: &Path) -> Result<()> {
	validate_regular_file(path, "temporary file")?;
	fs::remove_file(path).with_context(|| format!("remove temporary file {}", path.display()))
}

fn remove_staged_sources(path: &Path) -> Result<()> {
	let name: &str = path
		.file_name()
		.and_then(|name| name.to_str())
		.context("staged source directory name must be UTF-8")?;
	if name != ".source-bundles" {
		bail!("refusing to remove unexpected staged source directory {path:?}");
	}
	fs::remove_dir_all(path)
		.with_context(|| format!("remove staged source directory {}", path.display()))
}

struct StagingDirectory {
	path: PathBuf,
	armed: bool,
}

impl StagingDirectory {
	fn new(output: &Path) -> Result<Self> {
		let parent: &Path = output.parent().context("output path has no parent")?;
		let output_name: &str = output
			.file_name()
			.and_then(|name| name.to_str())
			.context("output directory name must be UTF-8")?;
		let suffix: u128 = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.context("read system clock")?
			.as_nanos();
		let path: PathBuf = parent.join(format!(
			".{output_name}.partial-{}-{suffix}",
			std::process::id()
		));
		fs::create_dir(&path)
			.with_context(|| format!("create snapshot staging directory {}", path.display()))?;
		Ok(Self { path, armed: true })
	}

	fn path(&self) -> &Path {
		&self.path
	}

	fn publish(&mut self, output: &Path) -> Result<()> {
		match atomic_rename_no_replace(&self.path, output) {
			Ok(()) => {
				self.armed = false;
				Ok(())
			}
			Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
				bail!(
					"output path appeared during extraction; refusing to overwrite it: {}",
					output.display()
				);
			}
			Err(source) => Err(source).with_context(|| {
				format!(
					"atomically publish snapshot {} -> {} without replacing the destination",
					self.path.display(),
					output.display()
				)
			}),
		}
	}
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn atomic_rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
	renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE)
		.map_err(std::io::Error::from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn atomic_rename_no_replace(_source: &Path, _destination: &Path) -> std::io::Result<()> {
	Err(std::io::Error::new(
		std::io::ErrorKind::Unsupported,
		"atomic no-replace snapshot publishing requires macOS or Linux",
	))
}

impl Drop for StagingDirectory {
	fn drop(&mut self) {
		if !self.armed {
			return;
		}
		let Some(name) = self.path.file_name().and_then(|name| name.to_str()) else {
			return;
		};
		if name.starts_with('.') && name.contains(".partial-") {
			let _ = fs::remove_dir_all(&self.path);
		}
	}
}

#[cfg(test)]
mod tests {
	#[cfg(unix)]
	use super::prepare_output_path;
	use super::{resolve_operator_request, select_bundles, StagingDirectory, COMMON_BUNDLES};
	use crate::data::{
		compute_mechanics_revision, BattlePrefabEntryKind, BattlePrefabManifestEntry,
		BattlePrefabSource, BattlePrefabTooling,
	};
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	use std::sync::{Arc, Barrier};
	use std::{
		collections::{BTreeMap, BTreeSet},
		fs,
		path::{Path, PathBuf},
		sync::atomic::{AtomicU64, Ordering},
	};

	static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(0);

	struct TestRoot(PathBuf);

	impl TestRoot {
		fn new() -> Self {
			let id: u64 = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
			let path: PathBuf = std::env::temp_dir().join(format!(
				"ark_ingest_battle_data_{}_{}",
				std::process::id(),
				id
			));
			fs::create_dir(&path).expect("create test root");
			Self(path)
		}

		fn path(&self) -> &Path {
			&self.0
		}
	}

	impl Drop for TestRoot {
		fn drop(&mut self) {
			let _result: std::io::Result<()> = fs::remove_dir_all(&self.0);
		}
	}

	fn synthetic_archive_names() -> BTreeSet<String> {
		let mut names: BTreeSet<String> = COMMON_BUNDLES
			.iter()
			.map(|bundle| format!("assets/AB/Android/battle/prefabs/{bundle}.ab"))
			.collect();
		names.insert("assets/AB/Android/charpack/char_1046_sbell2.ab".to_owned());
		names.insert("assets/AB/Android/charpack/char_2025_shu.ab".to_owned());
		names.insert("assets/AB/Android/battle/enm_pfb_0.ab".to_owned());
		names.insert("assets/AB/Android/battle/enm_pfb_12.ab".to_owned());
		// Neighbours that must not be mistaken for enemy packs.
		names.insert("assets/AB/Android/battle/panel_stage_info.ab".to_owned());
		names.insert("assets/AB/Android/battle/enm_pfb_extra.ab".to_owned());
		names
	}

	#[test]
	fn selection_always_includes_common_bundles_and_resolves_short_operator_keys() {
		let selected = select_bundles(
			&synthetic_archive_names(),
			&["shu".to_owned(), "sbell2".to_owned()],
			false,
		)
		.unwrap();

		assert_eq!(selected.len(), COMMON_BUNDLES.len() + 2);
		assert!(selected
			.iter()
			.any(|bundle| bundle.logical_path == "char_1046_sbell2"));
		assert!(selected
			.iter()
			.any(|bundle| bundle.export_path == "typetree/charpack/char_2025_shu.json"));
	}

	#[test]
	fn enemy_packs_are_opt_in_and_matched_only_by_their_indexed_name() {
		let names: BTreeSet<String> = synthetic_archive_names();
		let without = select_bundles(&names, &[], false).unwrap();
		assert!(without
			.iter()
			.all(|bundle| bundle.bundle_kind != BattlePrefabEntryKind::Enemypack));

		let with = select_bundles(&names, &[], true).unwrap();
		let packs: Vec<&str> = with
			.iter()
			.filter(|bundle| bundle.bundle_kind == BattlePrefabEntryKind::Enemypack)
			.map(|bundle| bundle.logical_path.as_str())
			.collect();
		assert_eq!(packs, ["enm_pfb_0", "enm_pfb_12"]);
		assert!(with
			.iter()
			.any(|bundle| bundle.export_path == "typetree/enemypack/enm_pfb_0.json"));
	}

	#[test]
	fn requesting_enemy_packs_from_an_apk_without_any_fails_closed() {
		let names: BTreeSet<String> = synthetic_archive_names()
			.into_iter()
			.filter(|path| !path.contains("enm_pfb_"))
			.collect();

		let error = select_bundles(&names, &[], true).unwrap_err();

		assert!(error
			.to_string()
			.contains("no assets/AB/Android/battle/enm_pfb_"));
	}

	#[test]
	fn ambiguous_short_operator_key_is_rejected() {
		let available: BTreeMap<String, String> = [
			("char_1_test".to_owned(), "first".to_owned()),
			("char_2_test".to_owned(), "second".to_owned()),
		]
		.into_iter()
		.collect();

		let error = resolve_operator_request("test", &available).unwrap_err();

		assert!(error.to_string().contains("matched multiple charpacks"));
	}

	#[test]
	fn mechanics_revision_is_full_deterministic_sha256_and_excludes_timestamp() {
		let source = BattlePrefabSource {
			apk_sha256: "a".repeat(64),
			apk_byte_length: 123,
		};
		let tooling = BattlePrefabTooling {
			ark_ingest_version: "0.0.1".to_owned(),
			exporter_version: "1".to_owned(),
			python_version: "3.12.0".to_owned(),
			unitypy_version: "1.25.0".to_owned(),
			ark_unpacker_version: "v5.0".to_owned(),
		};
		let entries = vec![BattlePrefabManifestEntry {
			logical_path: "[uc]skills".to_owned(),
			bundle_kind: BattlePrefabEntryKind::Common,
			source_apk_path: "assets/example.ab".to_owned(),
			source_sha256: "b".repeat(64),
			source_byte_length: 41,
			export_path: "typetree/common/[uc]skills.json".to_owned(),
			export_sha256: "c".repeat(64),
			export_byte_length: 99,
		}];

		let first = compute_mechanics_revision(
			"0123456789012345678901234567890123456789",
			&source,
			&tooling,
			&entries,
		);
		let second = compute_mechanics_revision(
			"0123456789012345678901234567890123456789",
			&source,
			&tooling,
			&entries,
		);

		assert_eq!(first, second);
		assert_eq!(first.len(), 64);
		assert!(first
			.bytes()
			.all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
	}

	#[cfg(any(target_os = "linux", target_os = "macos"))]
	#[test]
	fn publish_never_overwrites_an_existing_destination() {
		let root: TestRoot = TestRoot::new();
		let output: PathBuf = root.path().join("snapshot");
		let mut staging: StagingDirectory = StagingDirectory::new(&output).unwrap();
		fs::write(staging.path().join("manifest.json"), b"staged").unwrap();
		fs::write(&output, b"racing writer").unwrap();

		let error = staging.publish(&output).unwrap_err();

		assert!(error.to_string().contains("refusing to overwrite"));
		assert_eq!(fs::read(&output).unwrap(), b"racing writer");
		assert!(staging.path().is_dir());
	}

	#[cfg(any(target_os = "linux", target_os = "macos"))]
	#[test]
	fn publish_never_replaces_an_existing_empty_directory() {
		let root: TestRoot = TestRoot::new();
		let output: PathBuf = root.path().join("snapshot");
		let mut staging: StagingDirectory = StagingDirectory::new(&output).unwrap();
		fs::write(staging.path().join("manifest.json"), b"staged").unwrap();
		fs::create_dir(&output).unwrap();

		let error = staging.publish(&output).unwrap_err();

		assert!(error.to_string().contains("refusing to overwrite"));
		assert!(output.is_dir());
		assert!(fs::read_dir(&output).unwrap().next().is_none());
		assert_eq!(
			fs::read(staging.path().join("manifest.json")).unwrap(),
			b"staged"
		);
	}

	#[cfg(any(target_os = "linux", target_os = "macos"))]
	#[test]
	fn concurrent_publish_has_exactly_one_winner() {
		let root: TestRoot = TestRoot::new();
		let output: PathBuf = root.path().join("snapshot");
		let first: StagingDirectory = StagingDirectory::new(&output).unwrap();
		let second: StagingDirectory = StagingDirectory::new(&output).unwrap();
		fs::write(first.path().join("manifest.json"), b"first").unwrap();
		fs::write(second.path().join("manifest.json"), b"second").unwrap();
		let barrier: Arc<Barrier> = Arc::new(Barrier::new(2));

		let ((first, first_result), (second, second_result)) = std::thread::scope(|scope| {
			let first_barrier: Arc<Barrier> = Arc::clone(&barrier);
			let first_output: &Path = &output;
			let first_handle = scope.spawn(move || {
				let mut first: StagingDirectory = first;
				first_barrier.wait();
				let result = first.publish(first_output);
				(first, result)
			});
			let second_barrier: Arc<Barrier> = Arc::clone(&barrier);
			let second_output: &Path = &output;
			let second_handle = scope.spawn(move || {
				let mut second: StagingDirectory = second;
				second_barrier.wait();
				let result = second.publish(second_output);
				(second, result)
			});

			(
				first_handle.join().expect("first publisher did not panic"),
				second_handle
					.join()
					.expect("second publisher did not panic"),
			)
		});

		assert_ne!(first_result.is_ok(), second_result.is_ok());
		let published: Vec<u8> = fs::read(output.join("manifest.json")).unwrap();
		if first_result.is_ok() {
			assert_eq!(published, b"first");
			assert!(second.path().is_dir());
		} else {
			assert_eq!(published, b"second");
			assert!(first.path().is_dir());
		}
		let loser_error = first_result.err().or_else(|| second_result.err()).unwrap();
		assert!(loser_error.to_string().contains("refusing to overwrite"));
	}

	#[cfg(not(any(target_os = "linux", target_os = "macos")))]
	#[test]
	fn unsupported_publish_preserves_staging_and_destination() {
		let root: TestRoot = TestRoot::new();
		let output: PathBuf = root.path().join("snapshot");
		let mut staging: StagingDirectory = StagingDirectory::new(&output).unwrap();
		fs::write(staging.path().join("manifest.json"), b"staged").unwrap();
		let error = staging.publish(&output).unwrap_err();
		assert_eq!(
			error.downcast_ref::<std::io::Error>().unwrap().kind(),
			std::io::ErrorKind::Unsupported
		);
		assert!(!output.exists());
		fs::write(&output, b"existing destination").unwrap();
		assert!(staging.publish(&output).is_err());
		assert_eq!(fs::read(output).unwrap(), b"existing destination");
		assert_eq!(
			fs::read(staging.path().join("manifest.json")).unwrap(),
			b"staged"
		);
	}

	#[cfg(unix)]
	#[test]
	fn output_checks_and_publish_reject_dangling_symlinks() {
		use std::os::unix::fs::symlink;

		let root: TestRoot = TestRoot::new();
		let output: PathBuf = root.path().join("snapshot");
		let missing: PathBuf = root.path().join("missing");
		symlink(&missing, &output).unwrap();

		let early_error = prepare_output_path(&output).unwrap_err();
		assert!(early_error.to_string().contains("already exists"));

		let mut staging: StagingDirectory = StagingDirectory::new(&output).unwrap();
		let publish_error = staging.publish(&output).unwrap_err();
		assert!(publish_error.to_string().contains("refusing to overwrite"));
		assert!(fs::symlink_metadata(&output)
			.unwrap()
			.file_type()
			.is_symlink());
	}

	#[cfg(unix)]
	#[test]
	fn publish_atomically_moves_staging_into_an_absent_destination() {
		let root: TestRoot = TestRoot::new();
		let output: PathBuf = root.path().join("snapshot");
		let mut staging: StagingDirectory = StagingDirectory::new(&output).unwrap();
		fs::write(staging.path().join("manifest.json"), b"complete").unwrap();

		staging.publish(&output).unwrap();

		assert_eq!(fs::read(output.join("manifest.json")).unwrap(), b"complete");
	}
}
