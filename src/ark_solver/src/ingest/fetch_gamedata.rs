use std::{
	collections::{BTreeMap, BTreeSet, HashMap},
	fs::{self, File},
	io::Write,
	num::NonZeroUsize,
	path::{Path, PathBuf},
	sync::Arc,
	thread,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::config::Config;
use crate::data::{resolve_available_stage_levels, AvailableStageLevel, StageIdentity};
use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::{Args, ValueEnum};
use rayon::prelude::*;
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde::{de::DeserializeOwned, Deserialize};

use crate::ingest::gamedata_snapshot::{
	build_manifest, github_commit_url, github_tree_url, invalidate_current_manifest,
	load_reusable_manifest, manifest_path_string, publish_manifest, raw_asset_url,
	revision_request, sha256_file, GameDataManifest, GameDataManifestEntry, RevisionRequest,
};

const USER_AGENT: &str = concat!(
	"ark-ingest/",
	env!("CARGO_PKG_VERSION"),
	" (+https://github.com/Acture/ark-cli)"
);
const GAMEDATA_PREFIX: &str = "zh_CN/gamedata/";
const LEVELS_PREFIX: &str = "zh_CN/gamedata/levels/";
const CONNECT_TIMEOUT_SECS: u64 = 30;
const TREE_TIMEOUT_SECS: u64 = 60;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
const MAX_ATTEMPTS: usize = 3;
const CONFIRMATION_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct DefaultTableSpec {
	name: &'static str,
	source_path: &'static str,
}

const DEFAULT_TABLES: [DefaultTableSpec; 5] = [
	DefaultTableSpec {
		name: "character_table.json",
		source_path: "zh_CN/gamedata/excel/character_table.json",
	},
	DefaultTableSpec {
		name: "enemy_database.json",
		source_path: "zh_CN/gamedata/levels/enemydata/enemy_database.json",
	},
	DefaultTableSpec {
		name: "enemy_handbook_table.json",
		source_path: "zh_CN/gamedata/excel/enemy_handbook_table.json",
	},
	DefaultTableSpec {
		name: "skill_table.json",
		source_path: "zh_CN/gamedata/excel/skill_table.json",
	},
	DefaultTableSpec {
		name: "stage_table.json",
		source_path: "zh_CN/gamedata/excel/stage_table.json",
	},
];

const ANALYSIS_TABLES: [DefaultTableSpec; 12] = [
	DefaultTableSpec {
		name: "character_table.json",
		source_path: "zh_CN/gamedata/excel/character_table.json",
	},
	DefaultTableSpec {
		name: "char_patch_table.json",
		source_path: "zh_CN/gamedata/excel/char_patch_table.json",
	},
	DefaultTableSpec {
		name: "skill_table.json",
		source_path: "zh_CN/gamedata/excel/skill_table.json",
	},
	DefaultTableSpec {
		name: "range_table.json",
		source_path: "zh_CN/gamedata/excel/range_table.json",
	},
	DefaultTableSpec {
		name: "favor_table.json",
		source_path: "zh_CN/gamedata/excel/favor_table.json",
	},
	DefaultTableSpec {
		name: "token_table.json",
		source_path: "zh_CN/gamedata/excel/token_table.json",
	},
	DefaultTableSpec {
		name: "uniequip_table.json",
		source_path: "zh_CN/gamedata/excel/uniequip_table.json",
	},
	DefaultTableSpec {
		name: "battle_equip_table.json",
		source_path: "zh_CN/gamedata/excel/battle_equip_table.json",
	},
	DefaultTableSpec {
		name: "gamedata_const.json",
		source_path: "zh_CN/gamedata/excel/gamedata_const.json",
	},
	DefaultTableSpec {
		name: "stage_table.json",
		source_path: "zh_CN/gamedata/excel/stage_table.json",
	},
	DefaultTableSpec {
		name: "enemy_database.json",
		source_path: "zh_CN/gamedata/levels/enemydata/enemy_database.json",
	},
	DefaultTableSpec {
		name: "enemy_handbook_table.json",
		source_path: "zh_CN/gamedata/excel/enemy_handbook_table.json",
	},
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum GameDataProfile {
	#[default]
	Default,
	Analysis,
}

impl GameDataProfile {
	fn includes_levels(self) -> bool {
		matches!(self, Self::Analysis)
	}
}

fn table_specs_for_profile(profile: GameDataProfile) -> &'static [DefaultTableSpec] {
	match profile {
		GameDataProfile::Default => &DEFAULT_TABLES,
		GameDataProfile::Analysis => &ANALYSIS_TABLES,
	}
}

#[derive(Args, Debug, Clone)]
pub struct FetchGameDataArgs {
	/// Output directory for the JSON tables.
	#[arg(long, value_name = "DIR")]
	pub to: Option<PathBuf>,
	/// Re-download files even when a matching snapshot manifest verifies them.
	#[arg(long)]
	pub force: bool,
	/// Git commit or ref to resolve once and pin for the complete snapshot.
	#[arg(long, value_name = "REVISION")]
	pub revision: Option<String>,
	/// Select thin runtime tables or analysis data; analysis includes stage level data.
	#[arg(long, value_enum, default_value_t)]
	pub profile: GameDataProfile,
	/// Also mirror every JSON file under `zh_CN/gamedata/levels/`.
	#[arg(long)]
	pub include_levels: bool,
	/// Limit analysis level mirroring to this stage code; repeat for multiple stages.
	#[arg(long = "stage", value_name = "CODE", conflicts_with = "include_levels")]
	pub stages: Vec<String>,
	/// Number of parallel download jobs.
	#[arg(long, default_value_t = NonZeroUsize::new(4).expect("non-zero default"))]
	pub jobs: NonZeroUsize,
	/// Confirm planned downloads larger than 50 MiB.
	#[arg(long)]
	pub yes: bool,
}

#[derive(Debug, Deserialize)]
struct GitTreeResponse {
	truncated: bool,
	tree: Vec<GitTreeEntry>,
}

#[derive(Debug, Deserialize)]
struct GitCommitResponse {
	sha: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitTreeEntry {
	path: String,
	#[serde(rename = "type")]
	kind: String,
	size: Option<u64>,
}

impl GitTreeEntry {
	fn is_blob(&self) -> bool {
		self.kind == "blob"
	}

	fn is_level_json(&self) -> bool {
		self.is_blob() && self.path.starts_with(LEVELS_PREFIX) && self.path.ends_with(".json")
	}

	fn required_size(&self) -> Result<u64> {
		self.size
			.with_context(|| format!("missing blob size for {}", self.path))
	}
}

#[derive(Debug, Clone)]
struct RemoteAsset {
	name: String,
	source_path: String,
	destination_relative_path: PathBuf,
	expected_bytes: u64,
	is_level: bool,
	prefetched_bytes: Option<Arc<[u8]>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedAction {
	Download,
	SkipExisting,
}

#[derive(Debug, Clone)]
struct PlannedAsset {
	index: usize,
	remote: RemoteAsset,
	destination: PathBuf,
	action: PlannedAction,
	existing_sha256: Option<String>,
}

#[derive(Debug)]
struct FetchResult {
	name: String,
	destination_path: String,
	source_path: String,
	bytes: u64,
	sha256: String,
	action: FetchAction,
	is_level: bool,
}

#[derive(Debug, Clone, Copy)]
enum FetchAction {
	Downloaded,
	Skipped,
}

impl FetchAction {
	fn label(self) -> &'static str {
		match self {
			Self::Downloaded => "download",
			Self::Skipped => "skip",
		}
	}
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

pub fn run(args: FetchGameDataArgs) -> Result<()> {
	validate_stage_filter(&args)?;
	let output_root: PathBuf = match args.to.as_deref() {
		Some(output) => absolute_path(output)?,
		None => {
			Config::load()
				.context("load rogue config for gamedata output")?
				.gamedata_dir
		}
	};
	fs::create_dir_all(&output_root)
		.with_context(|| format!("create {}", output_root.display()))?;
	let client: Client = build_client()?;
	let resolved_revision: String = resolve_revision(&client, args.revision.as_deref())?;
	let include_levels: bool = args.include_levels || args.profile.includes_levels();
	let tree_entries: Option<Vec<GitTreeEntry>> = if include_levels {
		Some(fetch_tree_entries(&client, &resolved_revision)?)
	} else {
		None
	};
	let tree_size_lookup: Option<HashMap<String, u64>> = tree_entries
		.as_ref()
		.map(|entries| build_tree_size_lookup(entries))
		.transpose()?;

	let mut assets: Vec<RemoteAsset> = profile_assets(args.profile, tree_size_lookup.as_ref())?;
	if !args.stages.is_empty() {
		let existing_manifest: Option<GameDataManifest> =
			load_reusable_manifest(&output_root.join("manifest.json"))?;
		let existing_manifest: Option<&GameDataManifest> = if args.force {
			None
		} else {
			existing_manifest.as_ref()
		};
		let root_plan: Vec<PlannedAsset> = build_plan(
			&output_root,
			assets.clone(),
			args.force,
			existing_manifest,
			&resolved_revision,
		)?;
		ensure_confirmation(&args, &root_plan)?;
		let stage_table_bytes: Arc<[u8]> =
			load_stage_table_for_selection(&client, &root_plan, &resolved_revision)?;
		let entries: &[GitTreeEntry] = tree_entries
			.as_deref()
			.context("--stage requires the GitHub tree response")?;
		let selected_levels: Vec<RemoteAsset> =
			level_assets_for_stages(entries, &stage_table_bytes, &args.stages)?;
		let stage_table_asset: &mut RemoteAsset = assets
			.iter_mut()
			.find(|asset| asset.name == "stage_table.json")
			.context("analysis profile is missing stage_table.json")?;
		if root_plan
			.iter()
			.find(|asset| asset.remote.name == "stage_table.json")
			.is_some_and(|asset| asset.action == PlannedAction::Download)
		{
			stage_table_asset.prefetched_bytes = Some(stage_table_bytes);
		}
		assets.extend(selected_levels);
	} else if let Some(entries) = tree_entries.as_ref() {
		assets.extend(level_assets_from_tree(entries)?);
	}

	let manifest_path: PathBuf = output_root.join("manifest.json");
	let reusable_manifest: Option<GameDataManifest> = load_reusable_manifest(&manifest_path)?;
	let existing_manifest: Option<&GameDataManifest> = if args.force {
		None
	} else {
		reusable_manifest.as_ref()
	};
	let planned: Vec<PlannedAsset> = build_plan(
		&output_root,
		assets,
		args.force,
		existing_manifest,
		&resolved_revision,
	)?;
	if include_levels {
		print_level_plan_summary(&output_root, &planned);
		ensure_confirmation(&args, &planned)?;
	}

	println!("resolved gamedata revision: {resolved_revision}");
	invalidate_current_manifest(&manifest_path)?;
	let results: Vec<FetchResult> = execute_plan(&client, &planned, args.jobs, &resolved_revision)?;
	let manifest_entries: Vec<GameDataManifestEntry> = manifest_entries_for_results(
		&results,
		reusable_manifest.as_ref(),
		&output_root,
		&resolved_revision,
		!args.stages.is_empty(),
	)?;
	let generated_at: String = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
	let manifest: GameDataManifest =
		build_manifest(&resolved_revision, &generated_at, manifest_entries);
	publish_manifest(&manifest_path, &manifest)?;
	print_results(&output_root, &results, include_levels);
	Ok(())
}

fn manifest_entries_for_results(
	results: &[FetchResult],
	reusable_manifest: Option<&GameDataManifest>,
	output_root: &Path,
	resolved_revision: &str,
	retain_reusable_levels: bool,
) -> Result<Vec<GameDataManifestEntry>> {
	let mut entries: Vec<GameDataManifestEntry> = results
		.iter()
		.map(|result| GameDataManifestEntry {
			destination_path: result.destination_path.clone(),
			source_path: result.source_path.clone(),
			byte_length: result.bytes,
			sha256: result.sha256.clone(),
		})
		.collect();
	if !retain_reusable_levels {
		return Ok(entries);
	}
	let replaced_destinations: BTreeSet<String> = entries
		.iter()
		.map(|entry| entry.destination_path.to_ascii_lowercase())
		.collect();
	if let Some(existing) = reusable_manifest {
		entries.extend(
			existing
				.reusable_level_entries(output_root, resolved_revision)?
				.into_iter()
				.filter(|entry| {
					!replaced_destinations.contains(&entry.destination_path.to_ascii_lowercase())
				}),
		);
	}
	Ok(entries)
}

fn validate_stage_filter(args: &FetchGameDataArgs) -> Result<()> {
	if !args.stages.is_empty() && args.profile != GameDataProfile::Analysis {
		bail!("--stage requires --profile analysis");
	}
	if !args.stages.is_empty() && args.include_levels {
		bail!("--stage conflicts with --include-levels");
	}
	if args.stages.iter().any(|stage| stage.trim().is_empty()) {
		bail!("--stage values must not be empty");
	}
	Ok(())
}

fn build_client() -> Result<Client> {
	Client::builder()
		.user_agent(USER_AGENT)
		.connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
		.build()
		.context("build reqwest client")
}

fn resolve_revision(client: &Client, revision: Option<&str>) -> Result<String> {
	match revision_request(revision)? {
		RevisionRequest::Immutable(revision) => Ok(revision),
		RevisionRequest::Resolve(reference) => {
			let url: String = github_commit_url(&reference)?;
			let response: GitCommitResponse =
				fetch_json(client, &url, Duration::from_secs(TREE_TIMEOUT_SECS))?;
			let resolved: String = response.sha.trim().to_ascii_lowercase();
			if resolved.len() != 40 || !resolved.bytes().all(|byte| byte.is_ascii_hexdigit()) {
				bail!("GitHub resolved {reference:?} to invalid commit SHA {resolved:?}");
			}
			Ok(resolved)
		}
	}
}

fn fetch_tree_entries(client: &Client, resolved_revision: &str) -> Result<Vec<GitTreeEntry>> {
	let url: String = github_tree_url(resolved_revision);
	let response: GitTreeResponse =
		fetch_json(client, &url, Duration::from_secs(TREE_TIMEOUT_SECS))?;
	if response.truncated {
		bail!("GitHub tree API response was truncated; refusing partial level enumeration");
	}
	Ok(response.tree)
}

fn build_tree_size_lookup(tree_entries: &[GitTreeEntry]) -> Result<HashMap<String, u64>> {
	let mut lookup: HashMap<String, u64> = HashMap::new();
	for entry in tree_entries {
		if !entry.is_blob() {
			continue;
		}
		lookup.insert(entry.path.clone(), entry.required_size()?);
	}
	Ok(lookup)
}

fn profile_assets(
	profile: GameDataProfile,
	size_lookup: Option<&HashMap<String, u64>>,
) -> Result<Vec<RemoteAsset>> {
	let specs: &[DefaultTableSpec] = table_specs_for_profile(profile);
	let mut assets: Vec<RemoteAsset> = Vec::with_capacity(specs.len());
	for spec in specs {
		let expected_bytes: u64 = match size_lookup {
			Some(lookup) => *lookup
				.get(spec.source_path)
				.with_context(|| format!("missing {} in GitHub tree response", spec.source_path))?,
			None => 0,
		};
		assets.push(RemoteAsset {
			name: spec.name.to_string(),
			source_path: spec.source_path.to_string(),
			destination_relative_path: PathBuf::from(spec.name),
			expected_bytes,
			is_level: false,
			prefetched_bytes: None,
		});
	}
	Ok(assets)
}

fn level_assets_from_tree(tree_entries: &[GitTreeEntry]) -> Result<Vec<RemoteAsset>> {
	let mut assets: Vec<RemoteAsset> = tree_entries
		.iter()
		.filter(|entry| entry.is_level_json())
		.map(remote_level_asset)
		.collect::<Result<Vec<_>>>()?;
	assets.sort_by(|left, right| left.name.cmp(&right.name));
	Ok(assets)
}

fn remote_level_asset(entry: &GitTreeEntry) -> Result<RemoteAsset> {
	let relative_path: &str = entry
		.path
		.strip_prefix(GAMEDATA_PREFIX)
		.context("level path should live under zh_CN/gamedata/")?;
	Ok(RemoteAsset {
		name: relative_path.to_string(),
		source_path: entry.path.clone(),
		destination_relative_path: PathBuf::from(relative_path),
		expected_bytes: entry.required_size()?,
		is_level: true,
		prefetched_bytes: None,
	})
}

fn load_stage_table_for_selection(
	client: &Client,
	root_plan: &[PlannedAsset],
	resolved_revision: &str,
) -> Result<Arc<[u8]>> {
	let stage_table: &PlannedAsset = root_plan
		.iter()
		.find(|asset| asset.remote.name == "stage_table.json")
		.context("analysis profile is missing stage_table.json")?;
	match stage_table.action {
		PlannedAction::SkipExisting => {
			let bytes: Vec<u8> = fs::read(&stage_table.destination)
				.with_context(|| format!("read {}", stage_table.destination.display()))?;
			Ok(Arc::from(bytes))
		}
		PlannedAction::Download => {
			let url: String = raw_asset_url(resolved_revision, &stage_table.remote.source_path);
			let response: Response =
				send_get_with_retries(client, &url, Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))?;
			let bytes = response
				.bytes()
				.with_context(|| format!("read response body from {url}"))?;
			Ok(Arc::from(bytes.as_ref()))
		}
	}
}

fn level_assets_for_stages(
	tree_entries: &[GitTreeEntry],
	stage_table_bytes: &[u8],
	requested_stages: &[String],
) -> Result<Vec<RemoteAsset>> {
	let mut selected_level_paths: BTreeSet<String> = BTreeSet::new();

	for requested in requested_stages {
		let requested: &str = requested.trim();
		if requested.is_empty() {
			bail!("--stage values must not be empty");
		}
		let levels: Vec<AvailableStageLevel> = resolve_available_stage_levels(
			stage_table_bytes,
			StageIdentity::Code(requested.to_owned()),
		)
		.with_context(|| format!("resolve --stage {requested:?}"))?;
		for level in levels {
			collect_stage_level_path(&level.level_id, &mut selected_level_paths, requested)?;
		}
	}

	let tree_levels: BTreeMap<String, &GitTreeEntry> = tree_entries
		.iter()
		.filter(|entry| entry.is_level_json())
		.map(|entry| (entry.path.to_ascii_lowercase(), entry))
		.collect();
	let mut assets: Vec<RemoteAsset> = Vec::with_capacity(selected_level_paths.len());
	for normalized_path in selected_level_paths {
		let entry: &GitTreeEntry =
			tree_levels
				.get(&normalized_path)
				.copied()
				.with_context(|| {
					format!("selected stage level is missing from GitHub tree: {normalized_path}")
				})?;
		assets.push(remote_level_asset(entry)?);
	}
	assets.sort_by(|left, right| left.name.cmp(&right.name));
	Ok(assets)
}

fn collect_stage_level_path(
	level_id: &str,
	selected_level_paths: &mut BTreeSet<String>,
	requested: &str,
) -> Result<()> {
	let level_id: &str = level_id.trim();
	if level_id.is_empty() {
		bail!("--stage {requested:?} resolved an empty levelId");
	}
	if level_id.starts_with('/')
		|| level_id.contains('\\')
		|| level_id
			.split('/')
			.any(|component| component.is_empty() || component == "." || component == "..")
	{
		bail!("--stage {requested:?} resolved unsafe levelId {level_id:?}");
	}
	selected_level_paths
		.insert(format!("{GAMEDATA_PREFIX}levels/{level_id}.json").to_ascii_lowercase());
	Ok(())
}

fn build_plan(
	output_root: &Path,
	assets: Vec<RemoteAsset>,
	force: bool,
	existing_manifest: Option<&GameDataManifest>,
	resolved_revision: &str,
) -> Result<Vec<PlannedAsset>> {
	let mut planned: Vec<PlannedAsset> = Vec::with_capacity(assets.len());
	for (index, remote) in assets.into_iter().enumerate() {
		let destination: PathBuf = output_root.join(&remote.destination_relative_path);
		if destination.exists() && !destination.is_file() {
			bail!(
				"destination exists but is not a file: {}",
				destination.display()
			);
		}
		let destination_path: String = manifest_path_string(&remote.destination_relative_path);
		let existing_sha256: Option<String> = if !force && destination.is_file() {
			match existing_manifest {
				Some(manifest) => manifest.reusable_sha256(
					resolved_revision,
					&destination_path,
					&remote.source_path,
					&destination,
				)?,
				None => None,
			}
		} else {
			None
		};
		let action: PlannedAction = if existing_sha256.is_some() {
			PlannedAction::SkipExisting
		} else {
			PlannedAction::Download
		};
		planned.push(PlannedAsset {
			index,
			remote,
			destination,
			action,
			existing_sha256,
		});
	}
	Ok(planned)
}

fn print_level_plan_summary(output_root: &Path, planned: &[PlannedAsset]) {
	let total_files: usize = planned.len();
	let level_files: usize = planned.iter().filter(|asset| asset.remote.is_level).count();
	let root_files: usize = total_files.saturating_sub(level_files);
	let planned_downloads: usize = planned_download_count(planned);
	let skipped_existing: usize = planned_skip_count(planned);
	let planned_bytes: u64 = planned_download_bytes(planned);

	println!(
		"gamedata plan: {} file(s) total ({} root tables + {} level JSON)",
		total_files, root_files, level_files,
	);
	println!(
		"will download {} file(s), skip {} existing, {} planned download",
		planned_downloads,
		skipped_existing,
		format_bytes(planned_bytes),
	);
	println!("destination: {}", output_root.display());
}

fn ensure_confirmation(args: &FetchGameDataArgs, planned: &[PlannedAsset]) -> Result<()> {
	let planned_bytes: u64 = planned_download_bytes(planned);
	if needs_confirmation(planned_bytes) && !args.yes {
		bail!(
            "planned download is {} across {} file(s); rerun with --yes to confirm downloads larger than {}",
            format_bytes(planned_bytes),
            planned_download_count(planned),
            format_bytes(CONFIRMATION_THRESHOLD_BYTES),
        );
	}
	Ok(())
}

fn execute_plan(
	client: &Client,
	planned: &[PlannedAsset],
	jobs: NonZeroUsize,
	resolved_revision: &str,
) -> Result<Vec<FetchResult>> {
	let pool = rayon::ThreadPoolBuilder::new()
		.num_threads(jobs.get())
		.build()
		.context("build rayon thread pool")?;
	let downloaded_results: Vec<(usize, FetchResult)> =
		pool.install(|| -> Result<Vec<(usize, FetchResult)>> {
			let results: Vec<Result<(usize, FetchResult)>> = planned
				.par_iter()
				.filter(|asset| asset.action == PlannedAction::Download)
				.map(|asset| {
					download_asset(client, asset, resolved_revision)
						.map(|result| (asset.index, result))
				})
				.collect();
			results.into_iter().collect()
		})?;
	let mut by_index: HashMap<usize, FetchResult> = downloaded_results.into_iter().collect();
	let mut results: Vec<FetchResult> = Vec::with_capacity(planned.len());
	for asset in planned {
		match asset.action {
			PlannedAction::Download => {
				results.push(by_index.remove(&asset.index).with_context(|| {
					format!("missing download result for {}", asset.remote.name)
				})?)
			}
			PlannedAction::SkipExisting => results.push(skip_result(asset)?),
		}
	}
	Ok(results)
}

fn download_asset(
	client: &Client,
	asset: &PlannedAsset,
	resolved_revision: &str,
) -> Result<FetchResult> {
	if let Some(parent) = asset.destination.parent() {
		fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
	}
	let partial_path: PathBuf = partial_path(&asset.destination)?;
	let mut cleanup: RemoveFileOnDrop = RemoveFileOnDrop::new(partial_path.clone());
	let mut file: File = File::create(&partial_path)
		.with_context(|| format!("create {}", partial_path.display()))?;
	let bytes: u64 = if let Some(prefetched) = asset.remote.prefetched_bytes.as_deref() {
		file.write_all(prefetched)
			.with_context(|| format!("write {}", partial_path.display()))?;
		prefetched
			.len()
			.try_into()
			.context("prefetched asset length does not fit u64")?
	} else {
		let url: String = raw_asset_url(resolved_revision, &asset.remote.source_path);
		let mut response: Response =
			send_get_with_retries(client, &url, Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))?;
		response
			.copy_to(&mut file)
			.with_context(|| format!("write {}", partial_path.display()))?
	};
	file.flush()
		.with_context(|| format!("flush {}", partial_path.display()))?;
	drop(file);
	if asset.destination.is_file() {
		fs::remove_file(&asset.destination)
			.with_context(|| format!("remove {}", asset.destination.display()))?;
	}
	fs::rename(&partial_path, &asset.destination).with_context(|| {
		format!(
			"move {} to {}",
			partial_path.display(),
			asset.destination.display()
		)
	})?;
	cleanup.disarm();
	let sha256: String = sha256_file(&asset.destination)?;
	Ok(FetchResult {
		name: asset.remote.name.clone(),
		destination_path: manifest_path_string(&asset.remote.destination_relative_path),
		source_path: asset.remote.source_path.clone(),
		bytes,
		sha256,
		action: FetchAction::Downloaded,
		is_level: asset.remote.is_level,
	})
}

fn skip_result(asset: &PlannedAsset) -> Result<FetchResult> {
	let bytes: u64 = asset
		.destination
		.metadata()
		.with_context(|| format!("stat {}", asset.destination.display()))?
		.len();
	let sha256: String = asset.existing_sha256.clone().with_context(|| {
		format!(
			"missing verified SHA-256 for skipped file {}",
			asset.destination.display()
		)
	})?;
	Ok(FetchResult {
		name: asset.remote.name.clone(),
		destination_path: manifest_path_string(&asset.remote.destination_relative_path),
		source_path: asset.remote.source_path.clone(),
		bytes,
		sha256,
		action: FetchAction::Skipped,
		is_level: asset.remote.is_level,
	})
}

fn print_results(output_root: &Path, results: &[FetchResult], include_levels: bool) {
	let total_bytes: u64 = results.iter().map(|result| result.bytes).sum();
	let downloaded: usize = results
		.iter()
		.filter(|result| matches!(result.action, FetchAction::Downloaded))
		.count();
	let skipped: usize = results
		.iter()
		.filter(|result| matches!(result.action, FetchAction::Skipped))
		.count();

	println!(
		"gamedata: {} file(s), {} total bytes -> {}",
		results.len(),
		format_bytes(total_bytes),
		output_root.display(),
	);
	println!(
		"downloaded {} file(s), skipped {} existing",
		downloaded, skipped
	);

	let root_results: Vec<&FetchResult> =
		results.iter().filter(|result| !result.is_level).collect();
	for result in &root_results {
		println!(
			"[{}] {}	{}",
			result.action.label(),
			result.name,
			format_bytes(result.bytes),
		);
	}

	if include_levels {
		let level_results: Vec<&FetchResult> =
			results.iter().filter(|result| result.is_level).collect();
		let level_bytes: u64 = level_results.iter().map(|result| result.bytes).sum();
		let downloaded_levels: usize = level_results
			.iter()
			.filter(|result| matches!(result.action, FetchAction::Downloaded))
			.count();
		let skipped_levels: usize = level_results
			.iter()
			.filter(|result| matches!(result.action, FetchAction::Skipped))
			.count();
		println!(
			"[levels] {}	{} downloaded / {} skipped",
			format_bytes(level_bytes),
			downloaded_levels,
			skipped_levels,
		);
	}
}

fn fetch_json<T>(client: &Client, url: &str, timeout: Duration) -> Result<T>
where
	T: DeserializeOwned,
{
	let response: Response = send_get_with_retries(client, url, timeout)?;
	response
		.json::<T>()
		.with_context(|| format!("parse JSON from {url}"))
}

fn resolve_github_token() -> Option<String> {
	if let Ok(token) = std::env::var("GITHUB_TOKEN") {
		if !token.is_empty() {
			return Some(token);
		}
	}
	// Fall back to the `gh` CLI's stored credential — most developers
	// have it set up via `gh auth login`, so no extra exports needed.
	let output = std::process::Command::new("gh")
		.args(["auth", "token"])
		.output()
		.ok()?;
	if !output.status.success() {
		return None;
	}
	let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
	if token.is_empty() {
		None
	} else {
		Some(token)
	}
}

fn send_get_with_retries(client: &Client, url: &str, timeout: Duration) -> Result<Response> {
	let auth_token: Option<String> = if url.starts_with("https://api.github.com/") {
		resolve_github_token()
	} else {
		None
	};
	for attempt in 1..=MAX_ATTEMPTS {
		let mut request = client.get(url).timeout(timeout);
		if let Some(token) = auth_token.as_deref() {
			request = request.header("Authorization", format!("Bearer {token}"));
		}
		match request.send() {
			Ok(response) => {
				let status: StatusCode = response.status();
				if status.is_success() {
					return Ok(response);
				}
				if status == StatusCode::NOT_FOUND {
					bail!("GET {url} returned 404");
				}
				if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
					&& attempt < MAX_ATTEMPTS
				{
					thread::sleep(backoff_delay(attempt));
					continue;
				}
				let body: String = response.text().unwrap_or_default();
				if body.trim().is_empty() {
					bail!("GET {url} returned {status}");
				}
				bail!("GET {url} returned {status}: {}", body.trim());
			}
			Err(error) => {
				if (error.is_timeout() || error.is_connect()) && attempt < MAX_ATTEMPTS {
					thread::sleep(backoff_delay(attempt));
					continue;
				}
				return Err(error).with_context(|| format!("GET {url}"));
			}
		}
	}
	bail!("GET {url} exhausted retries")
}

fn backoff_delay(attempt: usize) -> Duration {
	let factor: u64 = 1_u64 << (attempt.saturating_sub(1) as u32);
	Duration::from_millis(500 * factor)
}

fn planned_download_count(planned: &[PlannedAsset]) -> usize {
	planned
		.iter()
		.filter(|asset| asset.action == PlannedAction::Download)
		.count()
}

fn planned_skip_count(planned: &[PlannedAsset]) -> usize {
	planned
		.iter()
		.filter(|asset| asset.action == PlannedAction::SkipExisting)
		.count()
}

fn planned_download_bytes(planned: &[PlannedAsset]) -> u64 {
	planned
		.iter()
		.filter(|asset| asset.action == PlannedAction::Download)
		.map(|asset| asset.remote.expected_bytes)
		.sum()
}

fn needs_confirmation(bytes: u64) -> bool {
	bytes > CONFIRMATION_THRESHOLD_BYTES
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
	if path.is_absolute() {
		return Ok(path.to_path_buf());
	}
	Ok(std::env::current_dir()
		.context("read current directory")?
		.join(path))
}

fn partial_path(path: &Path) -> Result<PathBuf> {
	let parent: &Path = path.parent().unwrap_or_else(|| Path::new("."));
	let file_name: &str = path
		.file_name()
		.and_then(|file_name| file_name.to_str())
		.context("destination file name must be valid utf-8")?;
	Ok(parent.join(format!(".{file_name}.partial-{}", unique_suffix()?)))
}

fn unique_suffix() -> Result<u128> {
	Ok(SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.context("read system clock")?
		.as_millis())
}

fn format_bytes(bytes: u64) -> String {
	format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
	use super::{
		build_plan, level_assets_for_stages, level_assets_from_tree, manifest_entries_for_results,
		needs_confirmation, table_specs_for_profile, validate_stage_filter, FetchAction,
		FetchGameDataArgs, FetchResult, GameDataProfile, GitTreeEntry, PlannedAction, RemoteAsset,
		CONFIRMATION_THRESHOLD_BYTES, DEFAULT_TABLES,
	};
	use crate::ingest::gamedata_snapshot::{
		build_manifest, sha256_file, GameDataManifest, GameDataManifestEntry,
	};
	use serde_json::json;
	use std::{
		collections::BTreeSet,
		fs,
		num::NonZeroUsize,
		path::PathBuf,
		time::{SystemTime, UNIX_EPOCH},
	};

	const TEST_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

	#[test]
	fn analysis_profile_contains_every_required_table_and_raw_levels() {
		let actual: BTreeSet<&str> = table_specs_for_profile(GameDataProfile::Analysis)
			.iter()
			.map(|spec| spec.name)
			.collect();
		let expected: BTreeSet<&str> = [
			"battle_equip_table.json",
			"char_patch_table.json",
			"character_table.json",
			"enemy_database.json",
			"enemy_handbook_table.json",
			"favor_table.json",
			"gamedata_const.json",
			"range_table.json",
			"skill_table.json",
			"stage_table.json",
			"token_table.json",
			"uniequip_table.json",
		]
		.into_iter()
		.collect();

		assert_eq!(actual, expected);
		assert!(GameDataProfile::Analysis.includes_levels());
		assert!(!GameDataProfile::Default.includes_levels());
	}

	#[test]
	fn stage_table_is_part_of_the_default_set() {
		assert_eq!(DEFAULT_TABLES.len(), 5);
		assert_eq!(DEFAULT_TABLES[4].name, "stage_table.json");
		assert_eq!(
			DEFAULT_TABLES[4].source_path,
			"zh_CN/gamedata/excel/stage_table.json"
		);
	}

	#[test]
	fn level_tree_entries_are_rebased_under_levels_directory() {
		let tree_entries: Vec<GitTreeEntry> = vec![
			GitTreeEntry {
				path: "zh_CN/gamedata/levels/main_01-07.json".to_string(),
				kind: "blob".to_string(),
				size: Some(128),
			},
			GitTreeEntry {
				path: "zh_CN/gamedata/levels/enemydata/enemy_database.json".to_string(),
				kind: "blob".to_string(),
				size: Some(256),
			},
			GitTreeEntry {
				path: "zh_CN/gamedata/excel/stage_table.json".to_string(),
				kind: "blob".to_string(),
				size: Some(64),
			},
			GitTreeEntry {
				path: "zh_CN/gamedata/levels/readme.txt".to_string(),
				kind: "blob".to_string(),
				size: Some(32),
			},
		];

		let assets = level_assets_from_tree(&tree_entries).unwrap();
		assert_eq!(assets.len(), 2);
		assert_eq!(assets[0].name, "levels/enemydata/enemy_database.json");
		assert_eq!(
			assets[0].destination_relative_path,
			PathBuf::from("levels/enemydata/enemy_database.json")
		);
		assert_eq!(assets[0].expected_bytes, 256);
		assert_eq!(assets[1].name, "levels/main_01-07.json");
		assert_eq!(
			assets[1].destination_relative_path,
			PathBuf::from("levels/main_01-07.json")
		);
		assert_eq!(assets[1].expected_bytes, 128);
	}

	#[test]
	fn selected_stage_code_fetches_only_its_deduplicated_level_payload() {
		let tree_entries: Vec<GitTreeEntry> = vec![
			level_tree_entry(
				"zh_CN/gamedata/levels/activities/test/level_bd7.json",
				112_027,
			),
			level_tree_entry("zh_CN/gamedata/levels/activities/test/level_other.json", 64),
		];
		let stage_table: Vec<u8> = serde_json::to_vec(&json!({
			"stages": {
				"act_test_07": {
					"stageId": "act_test_07",
					"code": "BD-7",
					"levelId": "Activities/Test/Level_BD7",
					"difficulty": "NORMAL",
					"hardStagedId": "act_test_07#f#"
				},
				"act_test_07#f#": {
					"stageId": "act_test_07#f#",
					"code": "BD-7",
					"levelId": "Activities/Test/Level_BD7",
					"difficulty": "FOUR_STAR",
					"hardStagedId": null
				},
				"act_other_01": {
					"stageId": "act_other_01",
					"code": "OTHER-1",
					"levelId": "Activities/Test/Level_Other",
					"difficulty": "NORMAL",
					"hardStagedId": null
				}
			}
		}))
		.unwrap();

		let assets = level_assets_for_stages(
			&tree_entries,
			&stage_table,
			&["bd-7".to_string(), "BD-7".to_string()],
		)
		.unwrap();

		assert_eq!(assets.len(), 1);
		assert_eq!(
			assets[0].source_path,
			"zh_CN/gamedata/levels/activities/test/level_bd7.json"
		);
		assert_eq!(
			assets[0].destination_relative_path,
			PathBuf::from("levels/activities/test/level_bd7.json")
		);
		assert_eq!(assets[0].expected_bytes, 112_027);
	}

	#[test]
	fn selected_stage_keeps_distinct_normal_and_raid_payloads() {
		let tree_entries: Vec<GitTreeEntry> = vec![
			level_tree_entry("zh_CN/gamedata/levels/activities/test/normal.json", 100),
			level_tree_entry("zh_CN/gamedata/levels/activities/test/raid.json", 200),
		];
		let stage_table: Vec<u8> = serde_json::to_vec(&json!({
			"stages": {
				"normal": {
					"stageId": "normal",
					"code": "TEST-1",
					"levelId": "Activities/Test/Normal",
					"difficulty": "NORMAL",
					"hardStagedId": "normal#f#"
				},
				"normal#f#": {
					"stageId": "normal#f#",
					"code": "TEST-1",
					"levelId": "Activities/Test/Raid",
					"difficulty": "FOUR_STAR",
					"hardStagedId": null
				}
			}
		}))
		.unwrap();

		let assets =
			level_assets_for_stages(&tree_entries, &stage_table, &["TEST-1".to_string()]).unwrap();

		assert_eq!(assets.len(), 2);
		assert_eq!(assets[0].expected_bytes, 100);
		assert_eq!(assets[1].expected_bytes, 200);
	}

	#[test]
	fn selected_stage_fails_closed_for_unknown_code_and_missing_tree_payload() {
		let stage_table: Vec<u8> = serde_json::to_vec(&json!({
			"stages": {
				"act_test_07": {
					"stageId": "act_test_07",
					"code": "BD-7",
					"levelId": "Activities/Test/Level_BD7",
					"difficulty": "NORMAL",
					"hardStagedId": null
				}
			}
		}))
		.unwrap();

		let unknown =
			level_assets_for_stages(&[], &stage_table, &["MISSING-1".to_string()]).unwrap_err();
		assert!(format!("{unknown:#}").contains("stage was not found"));

		let missing =
			level_assets_for_stages(&[], &stage_table, &["BD-7".to_string()]).unwrap_err();
		assert!(missing
			.to_string()
			.contains("selected stage level is missing from GitHub tree"));
	}

	#[test]
	fn stage_filter_validates_profile_conflicts_and_blank_values() {
		let args = FetchGameDataArgs {
			to: Some(PathBuf::from("ignored")),
			force: false,
			revision: None,
			profile: GameDataProfile::Default,
			include_levels: false,
			stages: vec!["BD-7".to_string()],
			jobs: NonZeroUsize::new(1).unwrap(),
			yes: false,
		};

		let error = validate_stage_filter(&args).unwrap_err();
		assert_eq!(error.to_string(), "--stage requires --profile analysis");

		let conflicting = FetchGameDataArgs {
			profile: GameDataProfile::Analysis,
			include_levels: true,
			..args
		};
		let error = validate_stage_filter(&conflicting).unwrap_err();
		assert_eq!(error.to_string(), "--stage conflicts with --include-levels");

		let blank = FetchGameDataArgs {
			include_levels: false,
			stages: vec![" \t".to_string()],
			..conflicting
		};
		let error = validate_stage_filter(&blank).unwrap_err();
		assert_eq!(error.to_string(), "--stage values must not be empty");
	}

	#[test]
	fn confirmation_threshold_is_strictly_greater_than_fifty_mebibytes() {
		assert!(!needs_confirmation(CONFIRMATION_THRESHOLD_BYTES));
		assert!(needs_confirmation(CONFIRMATION_THRESHOLD_BYTES + 1));
	}

	#[test]
	fn targeted_manifest_retains_other_verified_levels_and_replaces_selected_entries() {
		let root: PathBuf = test_temp_dir("rogue_targeted_manifest");
		let levels: PathBuf = root.join("levels");
		fs::create_dir_all(&levels).unwrap();
		let first_path: PathBuf = levels.join("first.json");
		fs::write(&first_path, b"first").unwrap();
		let existing: GameDataManifest = build_manifest(
			TEST_REVISION,
			"2026-07-22T00:00:00Z",
			vec![GameDataManifestEntry {
				destination_path: "levels/first.json".to_string(),
				source_path: "zh_CN/gamedata/levels/first.json".to_string(),
				byte_length: 5,
				sha256: sha256_file(&first_path).unwrap(),
			}],
		);
		let second = fetch_result("levels/second.json", 6, "22");

		let mut entries =
			manifest_entries_for_results(&[second], Some(&existing), &root, TEST_REVISION, true)
				.unwrap();
		entries.sort_by(|left, right| left.destination_path.cmp(&right.destination_path));
		assert_eq!(entries.len(), 2);
		assert_eq!(entries[0].destination_path, "levels/first.json");
		assert_eq!(entries[1].destination_path, "levels/second.json");

		let replacement = fetch_result("levels/first.json", 7, "33");
		let entries = manifest_entries_for_results(
			&[replacement],
			Some(&existing),
			&root,
			TEST_REVISION,
			true,
		)
		.unwrap();
		assert_eq!(entries.len(), 1);
		assert_eq!(entries[0].byte_length, 7);
		assert_eq!(entries[0].sha256, "33".repeat(32));

		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn plan_skips_only_files_verified_by_a_matching_snapshot_manifest() {
		let root: PathBuf =
			std::env::temp_dir().join(format!("rogue_fetch_plan_test_{}", std::process::id()));
		fs::create_dir_all(&root).unwrap();
		let destination: PathBuf = root.join("stage_table.json");
		fs::write(&destination, b"abc").unwrap();
		let digest: String = sha256_file(&destination).unwrap();
		let manifest: GameDataManifest = build_manifest(
			TEST_REVISION,
			"2026-07-22T00:00:00Z",
			vec![GameDataManifestEntry {
				destination_path: "stage_table.json".to_string(),
				source_path: "zh_CN/gamedata/excel/stage_table.json".to_string(),
				byte_length: 3,
				sha256: digest,
			}],
		);
		let asset: RemoteAsset = RemoteAsset {
			name: "stage_table.json".to_string(),
			source_path: "zh_CN/gamedata/excel/stage_table.json".to_string(),
			destination_relative_path: PathBuf::from("stage_table.json"),
			expected_bytes: 3,
			is_level: false,
			prefetched_bytes: None,
		};

		let matching = build_plan(
			&root,
			vec![asset.clone()],
			false,
			Some(&manifest),
			TEST_REVISION,
		)
		.unwrap();
		assert_eq!(matching[0].action, PlannedAction::SkipExisting);
		assert!(matching[0].existing_sha256.is_some());

		let mismatched = build_plan(
			&root,
			vec![asset],
			false,
			Some(&manifest),
			"ffffffffffffffffffffffffffffffffffffffff",
		)
		.unwrap();
		assert_eq!(mismatched[0].action, PlannedAction::Download);
		assert!(mismatched[0].existing_sha256.is_none());
		fs::remove_dir_all(root).unwrap();
	}

	fn level_tree_entry(path: &str, size: u64) -> GitTreeEntry {
		GitTreeEntry {
			path: path.to_string(),
			kind: "blob".to_string(),
			size: Some(size),
		}
	}

	fn fetch_result(destination_path: &str, bytes: u64, sha_byte: &str) -> FetchResult {
		FetchResult {
			name: destination_path.to_string(),
			destination_path: destination_path.to_string(),
			source_path: format!("zh_CN/gamedata/{destination_path}"),
			bytes,
			sha256: sha_byte.repeat(32),
			action: FetchAction::Downloaded,
			is_level: true,
		}
	}

	fn test_temp_dir(prefix: &str) -> PathBuf {
		std::env::temp_dir().join(format!(
			"{prefix}_{}_{}",
			std::process::id(),
			SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		))
	}
}
