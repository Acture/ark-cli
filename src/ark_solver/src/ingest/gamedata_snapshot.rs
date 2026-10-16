use std::{
	collections::BTreeSet,
	fs::{self, File},
	io::{Read, Write},
	path::{Path, PathBuf},
	time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const REPOSITORY: &str = "Kengxxiao/ArknightsGameData";
const RAW_BASE_URL: &str = "https://raw.githubusercontent.com/Kengxxiao/ArknightsGameData";
const GITHUB_API_BASE_URL: &str = "https://api.github.com/repos/Kengxxiao/ArknightsGameData";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(in crate::ingest) struct GameDataManifest {
	schema_version: u32,
	repository: String,
	resolved_revision: String,
	generated_at: String,
	entries: Vec<GameDataManifestEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(in crate::ingest) struct GameDataManifestEntry {
	pub(in crate::ingest) destination_path: String,
	pub(in crate::ingest) source_path: String,
	pub(in crate::ingest) byte_length: u64,
	pub(in crate::ingest) sha256: String,
}

impl GameDataManifest {
	pub(in crate::ingest) fn load_if_exists(path: &Path) -> Result<Option<Self>> {
		if !path.exists() {
			return Ok(None);
		}
		if !path.is_file() {
			bail!("manifest path exists but is not a file: {}", path.display());
		}
		let file: File = File::open(path)
			.with_context(|| format!("open gamedata manifest {}", path.display()))?;
		let manifest: Self = serde_json::from_reader(file)
			.with_context(|| format!("parse gamedata manifest {}", path.display()))?;
		Ok(Some(manifest))
	}

	pub(in crate::ingest) fn reusable_sha256(
		&self,
		resolved_revision: &str,
		destination_path: &str,
		source_path: &str,
		file_path: &Path,
	) -> Result<Option<String>> {
		if self.schema_version != 1
			|| self.repository != REPOSITORY
			|| self.resolved_revision != resolved_revision
		{
			return Ok(None);
		}
		let Some(entry) = self.entries.iter().find(|entry| {
			entry.destination_path == destination_path && entry.source_path == source_path
		}) else {
			return Ok(None);
		};
		let byte_length: u64 = file_path
			.metadata()
			.with_context(|| format!("stat {}", file_path.display()))?
			.len();
		if byte_length != entry.byte_length {
			return Ok(None);
		}
		let actual_sha256: String = sha256_file(file_path)?;
		if actual_sha256 != entry.sha256 {
			return Ok(None);
		}
		Ok(Some(actual_sha256))
	}

	pub(in crate::ingest) fn reusable_level_entries(
		&self,
		root: &Path,
		resolved_revision: &str,
	) -> Result<Vec<GameDataManifestEntry>> {
		if self.schema_version != 1
			|| self.repository != REPOSITORY
			|| self.resolved_revision != resolved_revision
		{
			return Ok(Vec::new());
		}
		let mut normalized_destinations: BTreeSet<String> = BTreeSet::new();
		let mut reusable: Vec<GameDataManifestEntry> = Vec::new();
		for entry in &self.entries {
			let normalized_destination: String = entry.destination_path.to_ascii_lowercase();
			let normalized_source: String = entry.source_path.to_ascii_lowercase();
			if !normalized_destination.starts_with("levels/")
				|| !normalized_destination.ends_with(".json")
				|| !normalized_source.starts_with("zh_cn/gamedata/levels/")
				|| !normalized_source.ends_with(".json")
				|| !is_safe_relative_posix_path(&entry.destination_path)
				|| !is_safe_relative_posix_path(&entry.source_path)
			{
				continue;
			}
			if !normalized_destinations.insert(normalized_destination) {
				bail!(
					"reusable manifest contains duplicate level destination {:?}",
					entry.destination_path
				);
			}
			let file_path: PathBuf = root.join(&entry.destination_path);
			if !is_regular_file_without_symlink(root, &entry.destination_path)? {
				continue;
			}
			let byte_length: u64 = file_path
				.metadata()
				.with_context(|| format!("stat {}", file_path.display()))?
				.len();
			if byte_length != entry.byte_length || sha256_file(&file_path)? != entry.sha256 {
				continue;
			}
			reusable.push(entry.clone());
		}
		Ok(reusable)
	}
}

fn is_safe_relative_posix_path(path: &str) -> bool {
	!path.is_empty()
		&& !path.starts_with('/')
		&& !path.contains('\\')
		&& path
			.split('/')
			.all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_regular_file_without_symlink(root: &Path, relative_path: &str) -> Result<bool> {
	let root_metadata: fs::Metadata = fs::symlink_metadata(root)
		.with_context(|| format!("stat output root {}", root.display()))?;
	if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
		return Ok(false);
	}
	let components: Vec<&str> = relative_path.split('/').collect();
	let mut current: PathBuf = root.to_path_buf();
	for (index, component) in components.iter().enumerate() {
		current.push(component);
		let metadata: fs::Metadata = match fs::symlink_metadata(&current) {
			Ok(metadata) => metadata,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
			Err(error) => return Err(error).with_context(|| format!("stat {}", current.display())),
		};
		let is_last: bool = index + 1 == components.len();
		if metadata.file_type().is_symlink()
			|| (is_last && !metadata.file_type().is_file())
			|| (!is_last && !metadata.file_type().is_dir())
		{
			return Ok(false);
		}
	}
	Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ingest) enum RevisionRequest {
	Immutable(String),
	Resolve(String),
}

pub(in crate::ingest) fn revision_request(revision: Option<&str>) -> Result<RevisionRequest> {
	let Some(revision) = revision else {
		return Ok(RevisionRequest::Resolve("master".to_string()));
	};
	let revision: &str = revision.trim();
	if revision.is_empty() {
		bail!("revision must not be empty");
	}
	if revision.eq_ignore_ascii_case("latest") {
		return Ok(RevisionRequest::Resolve("master".to_string()));
	}
	if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Ok(RevisionRequest::Immutable(revision.to_ascii_lowercase()));
	}
	Ok(RevisionRequest::Resolve(revision.to_string()))
}

pub(in crate::ingest) fn raw_asset_url(resolved_revision: &str, source_path: &str) -> String {
	format!("{RAW_BASE_URL}/{resolved_revision}/{source_path}")
}

pub(in crate::ingest) fn github_commit_url(revision: &str) -> Result<String> {
	let mut url: reqwest::Url =
		reqwest::Url::parse(GITHUB_API_BASE_URL).context("parse GitHub API base URL")?;
	url.path_segments_mut()
		.map_err(|_| anyhow::anyhow!("GitHub API base URL cannot accept path segments"))?
		.push("commits")
		.push(revision);
	Ok(url.to_string())
}

pub(in crate::ingest) fn github_tree_url(resolved_revision: &str) -> String {
	format!("{GITHUB_API_BASE_URL}/git/trees/{resolved_revision}?recursive=1")
}

pub(in crate::ingest) fn sha256_file(path: &Path) -> Result<String> {
	let mut file: File =
		File::open(path).with_context(|| format!("open {} for SHA-256", path.display()))?;
	let mut digest: Sha256 = Sha256::new();
	let mut buffer: [u8; 64 * 1024] = [0; 64 * 1024];
	loop {
		let bytes_read: usize = file
			.read(&mut buffer)
			.with_context(|| format!("read {} for SHA-256", path.display()))?;
		if bytes_read == 0 {
			break;
		}
		digest.update(&buffer[..bytes_read]);
	}
	Ok(format!("{:x}", digest.finalize()))
}

pub(in crate::ingest) fn manifest_path_string(path: &Path) -> String {
	path.to_string_lossy().replace('\\', "/")
}

pub(in crate::ingest) fn build_manifest(
	resolved_revision: &str,
	generated_at: &str,
	mut entries: Vec<GameDataManifestEntry>,
) -> GameDataManifest {
	entries.sort_by(|left, right| left.destination_path.cmp(&right.destination_path));
	GameDataManifest {
		schema_version: 1,
		repository: REPOSITORY.to_string(),
		resolved_revision: resolved_revision.to_string(),
		generated_at: generated_at.to_string(),
		entries,
	}
}

pub(in crate::ingest) fn write_manifest_atomically(
	path: &Path,
	manifest: &GameDataManifest,
) -> Result<()> {
	let mut bytes: Vec<u8> =
		serde_json::to_vec_pretty(manifest).context("serialize gamedata manifest")?;
	bytes.push(b'\n');
	let partial_path: PathBuf = partial_path(path)?;
	let mut cleanup: RemoveFileOnDrop = RemoveFileOnDrop::new(partial_path.clone());
	let mut file: File = File::create(&partial_path)
		.with_context(|| format!("create {}", partial_path.display()))?;
	file.write_all(&bytes)
		.with_context(|| format!("write {}", partial_path.display()))?;
	file.flush()
		.with_context(|| format!("flush {}", partial_path.display()))?;
	file.sync_all()
		.with_context(|| format!("sync {}", partial_path.display()))?;
	drop(file);
	fs::rename(&partial_path, path)
		.with_context(|| format!("move {} to {}", partial_path.display(), path.display()))?;
	cleanup.disarm();
	Ok(())
}

pub(in crate::ingest) fn load_reusable_manifest(path: &Path) -> Result<Option<GameDataManifest>> {
	if path.exists() {
		return GameDataManifest::load_if_exists(path);
	}
	GameDataManifest::load_if_exists(&previous_manifest_path(path)?)
}

pub(in crate::ingest) fn invalidate_current_manifest(path: &Path) -> Result<()> {
	if !path.exists() {
		return Ok(());
	}
	if !path.is_file() {
		bail!("manifest path exists but is not a file: {}", path.display());
	}
	let previous_path: PathBuf = previous_manifest_path(path)?;
	remove_file_if_present(&previous_path, "previous gamedata manifest")?;
	fs::rename(path, &previous_path).with_context(|| {
		format!(
			"invalidate current gamedata manifest {} -> {}",
			path.display(),
			previous_path.display()
		)
	})?;
	Ok(())
}

pub(in crate::ingest) fn publish_manifest(path: &Path, manifest: &GameDataManifest) -> Result<()> {
	if path.exists() {
		bail!(
			"refusing to publish over a current gamedata manifest: {}",
			path.display()
		);
	}
	let previous_path: PathBuf = previous_manifest_path(path)?;
	remove_file_if_present(&previous_path, "previous gamedata manifest")?;
	write_manifest_atomically(path, manifest)
}

fn previous_manifest_path(path: &Path) -> Result<PathBuf> {
	let parent: &Path = path.parent().unwrap_or_else(|| Path::new("."));
	let stem: &str = path
		.file_stem()
		.and_then(|stem| stem.to_str())
		.context("manifest file stem must be valid utf-8")?;
	let previous_name: String = match path.extension().and_then(|extension| extension.to_str()) {
		Some(extension) => format!("{stem}.previous.{extension}"),
		None => format!("{stem}.previous"),
	};
	Ok(parent.join(previous_name))
}

fn remove_file_if_present(path: &Path, label: &str) -> Result<()> {
	if !path.exists() {
		return Ok(());
	}
	if !path.is_file() {
		bail!("{label} path exists but is not a file: {}", path.display());
	}
	fs::remove_file(path).with_context(|| format!("remove {label} {}", path.display()))
}

fn partial_path(path: &Path) -> Result<PathBuf> {
	let parent: &Path = path.parent().unwrap_or_else(|| Path::new("."));
	let file_name: &str = path
		.file_name()
		.and_then(|file_name| file_name.to_str())
		.context("manifest file name must be valid utf-8")?;
	Ok(parent.join(format!(".{file_name}.partial-{}", unique_suffix()?)))
}

fn unique_suffix() -> Result<u128> {
	Ok(SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.context("read system clock")?
		.as_nanos())
}

struct RemoveFileOnDrop {
	path: PathBuf,
	armed: bool,
}

impl RemoveFileOnDrop {
	fn new(path: PathBuf) -> Self {
		Self { path, armed: true }
	}

	fn disarm(&mut self) {
		self.armed = false;
	}
}

impl Drop for RemoveFileOnDrop {
	fn drop(&mut self) {
		if self.armed {
			let _ = fs::remove_file(&self.path);
		}
	}
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
	use super::{
		build_manifest, github_commit_url, github_tree_url, invalidate_current_manifest,
		load_reusable_manifest, manifest_path_string, publish_manifest, raw_asset_url,
		revision_request, sha256_file, sha256_hex, write_manifest_atomically, GameDataManifest,
		GameDataManifestEntry, RevisionRequest,
	};
	use std::{
		fs,
		path::PathBuf,
		time::{SystemTime, UNIX_EPOCH},
	};

	const TEST_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

	#[test]
	fn immutable_revision_is_embedded_in_raw_and_tree_urls() {
		assert_eq!(
			raw_asset_url(TEST_REVISION, "zh_CN/gamedata/excel/stage_table.json"),
			format!(
				"https://raw.githubusercontent.com/Kengxxiao/ArknightsGameData/{TEST_REVISION}/zh_CN/gamedata/excel/stage_table.json"
			)
		);
		assert_eq!(
			github_tree_url(TEST_REVISION),
			format!(
				"https://api.github.com/repos/Kengxxiao/ArknightsGameData/git/trees/{TEST_REVISION}?recursive=1"
			)
		);
	}

	#[test]
	fn manifest_schema_is_version_one_and_entries_sort_by_destination() {
		let manifest: GameDataManifest = build_manifest(
			TEST_REVISION,
			"2026-07-22T00:00:00Z",
			vec![
				GameDataManifestEntry {
					destination_path: "levels/z.json".to_string(),
					source_path: "zh_CN/gamedata/levels/z.json".to_string(),
					byte_length: 1,
					sha256: "ff".repeat(32),
				},
				GameDataManifestEntry {
					destination_path: "character_table.json".to_string(),
					source_path: "zh_CN/gamedata/excel/character_table.json".to_string(),
					byte_length: 2,
					sha256: "00".repeat(32),
				},
			],
		);

		assert_eq!(manifest.schema_version, 1);
		assert_eq!(manifest.repository, "Kengxxiao/ArknightsGameData");
		assert_eq!(manifest.resolved_revision, TEST_REVISION);
		assert_eq!(manifest.generated_at, "2026-07-22T00:00:00Z");
		assert_eq!(
			manifest
				.entries
				.iter()
				.map(|entry| entry.destination_path.as_str())
				.collect::<Vec<_>>(),
			vec!["character_table.json", "levels/z.json"]
		);
	}

	#[test]
	fn reusable_level_entries_keep_only_intact_files_from_the_same_revision() {
		let root: PathBuf = std::env::temp_dir().join(format!(
			"rogue_reusable_levels_{}_{}",
			std::process::id(),
			SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		let levels: PathBuf = root.join("levels");
		fs::create_dir_all(&levels).unwrap();
		let intact_path: PathBuf = levels.join("intact.json");
		let tampered_path: PathBuf = levels.join("tampered.json");
		fs::write(&intact_path, b"intact").unwrap();
		fs::write(&tampered_path, b"before").unwrap();
		let manifest: GameDataManifest = build_manifest(
			TEST_REVISION,
			"2026-07-22T00:00:00Z",
			vec![
				GameDataManifestEntry {
					destination_path: "levels/intact.json".to_string(),
					source_path: "zh_CN/gamedata/levels/intact.json".to_string(),
					byte_length: 6,
					sha256: sha256_file(&intact_path).unwrap(),
				},
				GameDataManifestEntry {
					destination_path: "levels/tampered.json".to_string(),
					source_path: "zh_CN/gamedata/levels/tampered.json".to_string(),
					byte_length: 6,
					sha256: sha256_file(&tampered_path).unwrap(),
				},
				GameDataManifestEntry {
					destination_path: "stage_table.json".to_string(),
					source_path: "zh_CN/gamedata/excel/stage_table.json".to_string(),
					byte_length: 1,
					sha256: "00".repeat(32),
				},
			],
		);
		fs::write(&tampered_path, b"after!").unwrap();

		let reusable = manifest
			.reusable_level_entries(&root, TEST_REVISION)
			.unwrap();
		assert_eq!(reusable.len(), 1);
		assert_eq!(reusable[0].destination_path, "levels/intact.json");
		assert!(manifest
			.reusable_level_entries(&root, "ffffffffffffffffffffffffffffffffffffffff")
			.unwrap()
			.is_empty());

		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn sha256_output_matches_published_abc_vector() {
		assert_eq!(
			sha256_hex(b"abc"),
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
		);
	}

	#[test]
	fn omitted_and_latest_revision_resolve_master_once() {
		assert_eq!(
			revision_request(None).unwrap(),
			RevisionRequest::Resolve("master".to_string())
		);
		assert_eq!(
			revision_request(Some("latest")).unwrap(),
			RevisionRequest::Resolve("master".to_string())
		);
		assert_eq!(
			revision_request(Some("LATEST")).unwrap(),
			RevisionRequest::Resolve("master".to_string())
		);
	}

	#[test]
	fn explicit_sha_is_already_an_immutable_revision() {
		assert_eq!(
			revision_request(Some(TEST_REVISION)).unwrap(),
			RevisionRequest::Immutable(TEST_REVISION.to_string())
		);
	}

	#[test]
	fn github_commit_url_escapes_slashes_in_symbolic_revisions() {
		assert_eq!(
			github_commit_url("release/next").unwrap(),
			"https://api.github.com/repos/Kengxxiao/ArknightsGameData/commits/release%2Fnext"
		);
	}

	#[test]
	fn file_sha256_streams_the_same_published_vector() {
		let path: PathBuf =
			std::env::temp_dir().join(format!("rogue_sha256_test_{}", std::process::id()));
		fs::write(&path, b"abc").unwrap();
		assert_eq!(
			sha256_file(&path).unwrap(),
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
		);
		fs::remove_file(path).unwrap();
	}

	#[test]
	fn manifest_paths_use_forward_slashes() {
		assert_eq!(
			manifest_path_string(&PathBuf::from("levels/activities/example.json")),
			"levels/activities/example.json"
		);
	}

	#[test]
	fn atomic_manifest_write_replaces_the_complete_document() {
		let root: PathBuf =
			std::env::temp_dir().join(format!("rogue_manifest_test_{}", std::process::id()));
		fs::create_dir_all(&root).unwrap();
		let manifest_path: PathBuf = root.join("manifest.json");
		fs::write(&manifest_path, b"old manifest").unwrap();
		let expected: GameDataManifest =
			build_manifest(TEST_REVISION, "2026-07-22T00:00:00Z", Vec::new());

		write_manifest_atomically(&manifest_path, &expected).unwrap();

		let actual: GameDataManifest = GameDataManifest::load_if_exists(&manifest_path)
			.unwrap()
			.unwrap();
		assert_eq!(actual.schema_version, 1);
		assert_eq!(actual.resolved_revision, TEST_REVISION);
		let partials: Vec<PathBuf> = fs::read_dir(&root)
			.unwrap()
			.filter_map(|entry| entry.ok().map(|entry| entry.path()))
			.filter(|path| path != &manifest_path)
			.collect();
		assert!(partials.is_empty(), "left partial files: {partials:?}");
		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn manifest_reuse_requires_matching_revision_source_path_length_and_sha() {
		let path: PathBuf =
			std::env::temp_dir().join(format!("rogue_manifest_reuse_test_{}", std::process::id()));
		fs::write(&path, b"abc").unwrap();
		let digest: String = sha256_file(&path).unwrap();
		let manifest: GameDataManifest = build_manifest(
			TEST_REVISION,
			"2026-07-22T00:00:00Z",
			vec![GameDataManifestEntry {
				destination_path: "stage_table.json".to_string(),
				source_path: "zh_CN/gamedata/excel/stage_table.json".to_string(),
				byte_length: 3,
				sha256: digest.clone(),
			}],
		);

		assert_eq!(
			manifest
				.reusable_sha256(
					TEST_REVISION,
					"stage_table.json",
					"zh_CN/gamedata/excel/stage_table.json",
					&path,
				)
				.unwrap(),
			Some(digest)
		);
		assert_eq!(
			manifest
				.reusable_sha256(
					"ffffffffffffffffffffffffffffffffffffffff",
					"stage_table.json",
					"zh_CN/gamedata/excel/stage_table.json",
					&path,
				)
				.unwrap(),
			None
		);
		fs::write(&path, b"changed").unwrap();
		assert_eq!(
			manifest
				.reusable_sha256(
					TEST_REVISION,
					"stage_table.json",
					"zh_CN/gamedata/excel/stage_table.json",
					&path,
				)
				.unwrap(),
			None
		);
		fs::remove_file(path).unwrap();
	}

	#[test]
	fn failed_refresh_after_first_mutation_leaves_no_current_manifest() {
		let root: PathBuf = std::env::temp_dir().join(format!(
			"rogue_manifest_failure_test_{}",
			std::process::id()
		));
		fs::create_dir_all(&root).unwrap();
		let current_manifest_path: PathBuf = root.join("manifest.json");
		let previous_manifest_path: PathBuf = root.join("manifest.previous.json");
		let first_path: PathBuf = root.join("first.json");
		let second_path: PathBuf = root.join("second.json");
		fs::write(&first_path, b"old first").unwrap();
		fs::write(&second_path, b"old second").unwrap();
		let old_manifest: GameDataManifest = build_manifest(
			TEST_REVISION,
			"2026-07-22T00:00:00Z",
			vec![
				GameDataManifestEntry {
					destination_path: "first.json".to_string(),
					source_path: "source/first.json".to_string(),
					byte_length: 9,
					sha256: sha256_file(&first_path).unwrap(),
				},
				GameDataManifestEntry {
					destination_path: "second.json".to_string(),
					source_path: "source/second.json".to_string(),
					byte_length: 10,
					sha256: sha256_file(&second_path).unwrap(),
				},
			],
		);
		write_manifest_atomically(&current_manifest_path, &old_manifest).unwrap();

		let reusable_before: GameDataManifest = load_reusable_manifest(&current_manifest_path)
			.unwrap()
			.unwrap();
		assert!(reusable_before
			.reusable_sha256(
				TEST_REVISION,
				"first.json",
				"source/first.json",
				&first_path,
			)
			.unwrap()
			.is_some());

		invalidate_current_manifest(&current_manifest_path).unwrap();
		let injected_failure: anyhow::Result<()> = (|| {
			fs::write(&first_path, b"new first")?;
			anyhow::bail!("injected failure after first destination mutation")
		})();

		assert!(injected_failure.is_err());
		assert!(!current_manifest_path.exists());
		assert!(previous_manifest_path.is_file());
		let retry_manifest: GameDataManifest = load_reusable_manifest(&current_manifest_path)
			.unwrap()
			.unwrap();
		assert_eq!(
			retry_manifest
				.reusable_sha256(
					TEST_REVISION,
					"first.json",
					"source/first.json",
					&first_path,
				)
				.unwrap(),
			None
		);
		assert!(retry_manifest
			.reusable_sha256(
				TEST_REVISION,
				"second.json",
				"source/second.json",
				&second_path,
			)
			.unwrap()
			.is_some());
		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn successful_refresh_publishes_current_and_cleans_previous_manifest() {
		let root: PathBuf = std::env::temp_dir().join(format!(
			"rogue_manifest_success_test_{}",
			std::process::id()
		));
		fs::create_dir_all(&root).unwrap();
		let current_manifest_path: PathBuf = root.join("manifest.json");
		let previous_manifest_path: PathBuf = root.join("manifest.previous.json");
		let old_manifest: GameDataManifest =
			build_manifest(TEST_REVISION, "2026-07-22T00:00:00Z", Vec::new());
		write_manifest_atomically(&current_manifest_path, &old_manifest).unwrap();
		invalidate_current_manifest(&current_manifest_path).unwrap();
		let new_revision: &str = "ffffffffffffffffffffffffffffffffffffffff";
		let new_manifest: GameDataManifest =
			build_manifest(new_revision, "2026-07-22T01:00:00Z", Vec::new());

		publish_manifest(&current_manifest_path, &new_manifest).unwrap();

		assert!(current_manifest_path.is_file());
		assert!(!previous_manifest_path.exists());
		let current: GameDataManifest = GameDataManifest::load_if_exists(&current_manifest_path)
			.unwrap()
			.unwrap();
		assert_eq!(current.resolved_revision, new_revision);
		fs::remove_dir_all(root).unwrap();
	}
}
