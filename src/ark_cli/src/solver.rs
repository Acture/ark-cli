use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ark_solver::data::{ArkDataSnapshot, BattlePrefabSnapshot};
use ark_solver::maa_job::MaaJobDifficulty;
use ark_solver::replay::ExpectedReplayReportStatus;
use ark_solver::workflow::{
	replay as replay_workflow, solve_with_observer, validate_export,
	ExpectedDamageExportValidationReport, ExpectedDamagePreparedLevelReport,
	ExpectedDamageProbabilityBasis, ExpectedDamageVerifiedSearchStatus, ExportValidationRequest,
	ReplayRequest, ReplayWorkflowOutput, SolveRequest,
};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

mod progress;

use progress::{CliSolveProgressObserver, SolveProgressHeartbeat};

type CliResult<T> = Result<T, Box<dyn Error>>;

const LEVEL_SOLVE_JOB_INDEX_SCHEMA_VERSION: u32 = 5;

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
	/// Solve one level from an exact source MAA job file.
	Level(LevelArgs),
	/// Preflight and, when fully supported, replay every exact variant of one MAA job file.
	Replay(ReplayArgs),
}

#[derive(Debug, Args)]
pub(crate) struct LevelArgs {
	#[command(subcommand)]
	command: LevelCommand,
}

#[derive(Debug, Subcommand)]
enum LevelCommand {
	/// Export battlefield-verified replacement jobs for one guide slot.
	Solve(LevelSolveArgs),
	/// Re-import and independently validate one battlefield-verified job.
	Validate(LevelValidateArgs),
}

#[derive(Debug, Args)]
struct LevelSolveArgs {
	/// Display stage code, for example `BD-7`.
	#[arg(value_name = "LEVEL")]
	level: String,

	/// Verified Ark data snapshot root.
	#[arg(long, value_name = "PATH")]
	snapshot: PathBuf,

	/// Verified battle-prefab snapshot extracted from the matching client build.
	#[arg(long, value_name = "PATH")]
	prefab_snapshot: PathBuf,

	/// MAA job file.
	#[arg(long, value_name = "PATH")]
	guide: PathBuf,

	/// Explicit MAA stage difficulty to solve.
	#[arg(long, value_enum)]
	difficulty: CliDifficulty,

	/// Explicit operator/skill roster JSON. Omit to use the built-in reviewed registry.
	#[arg(long, value_name = "PATH")]
	roster: Option<PathBuf>,

	/// Operator ID or exact name to exclude. May be repeated.
	#[arg(long, value_name = "OPERATOR")]
	exclude: Vec<String>,

	/// Override the requested number of expected-damage jobs returned.
	#[arg(long)]
	top_k: Option<usize>,

	/// Maximum additive MILP proposals to jointly check.
	#[arg(long, default_value_t = 100)]
	additive_proposal_limit: usize,

	/// CPU workers consuming the bounded full-replay/export queue.
	#[arg(long, default_value_t = 4)]
	replay_workers: usize,

	/// Write the report here instead of stdout.
	#[arg(long, value_name = "PATH")]
	output: Option<PathBuf>,

	/// Write each complete MAA job file, canonical artifact, and audit into this directory.
	#[arg(long, value_name = "PATH")]
	output_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct LevelValidateArgs {
	/// Display stage code, for example `BD-7`.
	#[arg(value_name = "LEVEL")]
	level: String,

	/// Verified Ark data snapshot root.
	#[arg(long, value_name = "PATH")]
	snapshot: PathBuf,

	/// Verified battle-prefab snapshot used to reproduce mechanic interpretation.
	#[arg(long, value_name = "PATH")]
	prefab_snapshot: PathBuf,

	/// Original MAA job file whose source-slot responsibility is being replaced.
	#[arg(long, value_name = "PATH")]
	source_guide: PathBuf,

	/// Exported MAA job file.
	#[arg(long, value_name = "PATH")]
	strategy: PathBuf,

	/// Canonical binary strategy artifact paired with the exported strategy.
	#[arg(long, value_name = "PATH")]
	artifact: PathBuf,

	/// Explicit MAA stage difficulty to validate.
	#[arg(long, value_enum)]
	difficulty: CliDifficulty,

	/// Maximum source-guide selections inspected while recovering the responsibility contract.
	#[arg(long, default_value_t = 1_024)]
	guide_selection_budget: usize,

	/// Write the validation report here instead of stdout.
	#[arg(long, value_name = "PATH")]
	output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ReplayArgs {
	/// Verified Ark data snapshot root.
	#[arg(long, value_name = "PATH")]
	snapshot: PathBuf,

	/// Verified battle-prefab snapshot extracted from the matching client build.
	#[arg(long, value_name = "PATH")]
	prefab_snapshot: PathBuf,

	/// MAA job file.
	#[arg(long, value_name = "PATH")]
	guide: PathBuf,

	/// Explicit MAA stage difficulty to replay.
	#[arg(long, value_enum)]
	difficulty: CliDifficulty,

	/// Directory for one binary artifact and JSON audit report per independent variant.
	#[arg(long, value_name = "PATH")]
	output_dir: PathBuf,

	/// Deterministic simulation ticks per second.
	#[arg(long, default_value_t = 30)]
	ticks_per_second: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CliDifficulty {
	Normal,
	Challenge,
}

impl From<CliDifficulty> for MaaJobDifficulty {
	fn from(value: CliDifficulty) -> Self {
		match value {
			CliDifficulty::Normal => Self::Normal,
			CliDifficulty::Challenge => Self::Challenge,
		}
	}
}

#[derive(Debug, Serialize)]
struct LevelSolveJobIndexEntry {
	proposal_index: usize,
	candidate_keys: Vec<String>,
	estimated_success_probability_millionths: u32,
	probability_basis: ExpectedDamageProbabilityBasis,
	strategy_sha256: String,
	artifact_id: String,
	battlefield_status: ExpectedReplayReportStatus,
	directory: String,
	strategy: String,
	artifact: String,
	audit: String,
}

#[derive(Debug, Serialize)]
struct LevelSolveJobIndex {
	schema_version: u32,
	data_revision: String,
	mechanics_revision: String,
	requested_level: String,
	entries: Vec<LevelSolveJobIndexEntry>,
}

pub(crate) fn run(command: Command) -> CliResult<()> {
	match command {
		Command::Level(LevelArgs {
			command: LevelCommand::Solve(args),
		}) => run_level_solve(args),
		Command::Level(LevelArgs {
			command: LevelCommand::Validate(args),
		}) => run_level_validate(args),
		Command::Replay(args) => run_replay(args),
	}
}

fn run_level_solve(args: LevelSolveArgs) -> CliResult<()> {
	let total_started: Instant = Instant::now();

	let guide_started: Instant = Instant::now();
	eprintln!("guide: reading {}", args.guide.display());
	let guide_bytes: Vec<u8> = fs::read(&args.guide)
		.map_err(|error| path_error("failed to read MAA job file", &args.guide, error))?;
	let guide_source_id: String = format!(
		"file:{}",
		args.guide
			.file_name()
			.and_then(|name| name.to_str())
			.unwrap_or("maa-job.json")
	);
	report_elapsed("guide", guide_started);

	let roster_bytes: Option<Vec<u8>> = args
		.roster
		.as_deref()
		.map(|path| {
			fs::read(path).map_err(|error| path_error("failed to read roster", path, error))
		})
		.transpose()?;

	let snapshot_started: Instant = Instant::now();
	eprintln!("snapshot: loading {}", args.snapshot.display());
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(&args.snapshot)
		.map_err(|error| path_error("failed to load and verify snapshot", &args.snapshot, error))?;
	report_elapsed("snapshot", snapshot_started);
	let prefabs: BattlePrefabSnapshot = load_prefab_snapshot(&args.prefab_snapshot, &snapshot)?;

	let excluded_operators: BTreeSet<String> = args.exclude.into_iter().collect();
	let solve_started: Instant = Instant::now();
	eprintln!(
		"solve: level {}; target {} battlefield-verified jobs, additive proposal limit {}, replay \
		 workers {}",
		args.level,
		args.top_k
			.map_or_else(|| "workflow default".to_owned(), |value| value.to_string()),
		args.additive_proposal_limit,
		args.replay_workers
	);
	let progress: Arc<CliSolveProgressObserver> =
		Arc::new(CliSolveProgressObserver::new(solve_started));
	let heartbeat: SolveProgressHeartbeat = progress.start_heartbeat();
	let prepared_report: Result<ExpectedDamagePreparedLevelReport, _> = solve_with_observer(
		SolveRequest {
			snapshot: &snapshot,
			prefabs: &prefabs,
			requested_level: &args.level,
			guide_bytes: &guide_bytes,
			guide_source_id: &guide_source_id,
			difficulty: args.difficulty.into(),
			roster_json: roster_bytes.as_deref(),
			excluded_operators: &excluded_operators,
			top_k: args.top_k,
			additive_proposal_limit: args.additive_proposal_limit,
			replay_worker_count: args.replay_workers,
			guide_selection_budget: 1_024,
		},
		progress.as_ref(),
	);
	drop(heartbeat);
	let report: ExpectedDamagePreparedLevelReport = prepared_report
		.map_err(|error| io::Error::other(format!("level solve failed: {error}")))?;
	report_elapsed("solve", solve_started);
	eprintln!(
		"solve: actual attempts: {} additive proposals (limit {}), {} battlefield replays via {} \
		 workers / queue {}; verified jobs: {}/{} requested ({} battlefield victories)",
		report.solve.proposals.len(),
		report.additive_proposal_limit,
		report.battlefield_replays.len(),
		report.replay_worker_count,
		report.replay_queue_capacity,
		report.battlefield_verified_jobs.len(),
		report.requested_top_k,
		report.battlefield_expected_victory_proposal_indices.len()
	);
	eprintln!(
		"solve: stopped: {} ({:?}); planner {:?}, {} full-battle gaps / {} damage micros",
		verified_search_stop_reason(report.verified_search_status),
		report.verified_search_status,
		report.solve.status,
		report.full_battle_damage_gap.leaked_enemy_count(),
		report.full_battle_damage_gap.unresolved_damage_micros()
	);
	for job in &report.battlefield_verified_jobs {
		eprintln!(
			"job proposal {}: estimated success {:.6}% ({:?} battlefield)",
			job.proposal_index,
			f64::from(job.estimated_success_probability_millionths) / 10_000.0,
			job.battlefield_status
		);
	}
	if let Some(output_dir) = args.output_dir.as_deref() {
		write_level_solve_jobs(output_dir, &report)?;
	}

	let mut json: Vec<u8> = serde_json::to_vec_pretty(&report).map_err(|error| {
		io::Error::other(format!("failed to serialize level-solve report: {error}"))
	})?;
	json.push(b'\n');
	write_report(args.output.as_deref(), &json)?;
	eprintln!("complete: {:.3}s", total_started.elapsed().as_secs_f64());
	Ok(())
}

fn run_level_validate(args: LevelValidateArgs) -> CliResult<()> {
	let total_started: Instant = Instant::now();

	eprintln!("source guide: reading {}", args.source_guide.display());
	let source_guide_bytes: Vec<u8> = fs::read(&args.source_guide).map_err(|error| {
		path_error(
			"failed to read source MAA job file",
			&args.source_guide,
			error,
		)
	})?;
	let source_guide_source_id: String = format!(
		"file:{}",
		args.source_guide
			.file_name()
			.and_then(|name| name.to_str())
			.unwrap_or("source-guide.json")
	);

	eprintln!("strategy: reading {}", args.strategy.display());
	let strategy_bytes: Vec<u8> = fs::read(&args.strategy).map_err(|error| {
		path_error(
			"failed to read exported MAA job file",
			&args.strategy,
			error,
		)
	})?;
	eprintln!("artifact: reading {}", args.artifact.display());
	let artifact_bytes: Vec<u8> = fs::read(&args.artifact)
		.map_err(|error| path_error("failed to read strategy artifact", &args.artifact, error))?;

	eprintln!("snapshot: loading {}", args.snapshot.display());
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(&args.snapshot)
		.map_err(|error| path_error("failed to load and verify snapshot", &args.snapshot, error))?;
	let prefabs: BattlePrefabSnapshot = load_prefab_snapshot(&args.prefab_snapshot, &snapshot)?;
	let report: ExpectedDamageExportValidationReport = validate_export(ExportValidationRequest {
		snapshot: &snapshot,
		prefabs: &prefabs,
		requested_level: &args.level,
		source_guide_bytes: &source_guide_bytes,
		source_guide_source_id: &source_guide_source_id,
		maa_job_bytes: &strategy_bytes,
		artifact_bytes: &artifact_bytes,
		difficulty: args.difficulty.into(),
		guide_selection_budget: args.guide_selection_budget,
	})
	.map_err(|error| {
		io::Error::other(format!("expected-damage export validation failed: {error}"))
	})?;
	let mut json: Vec<u8> = serde_json::to_vec_pretty(&report).map_err(|error| {
		io::Error::other(format!("failed to serialize validation report: {error}"))
	})?;
	json.push(b'\n');
	write_report(args.output.as_deref(), &json)?;
	eprintln!(
		"validate: {:?}, estimated success {:.6}%, battlefield {:?}",
		report.damage_status,
		f64::from(report.estimated_success_probability_millionths) / 10_000.0,
		report.battlefield_status
	);
	eprintln!("complete: {:.3}s", total_started.elapsed().as_secs_f64());
	Ok(())
}

fn write_level_solve_jobs(
	output_dir: &Path,
	report: &ExpectedDamagePreparedLevelReport,
) -> CliResult<()> {
	fs::create_dir_all(output_dir).map_err(|error| {
		path_error(
			"failed to create level-solve output directory",
			output_dir,
			error,
		)
	})?;
	let mut entries: Vec<LevelSolveJobIndexEntry> =
		Vec::with_capacity(report.battlefield_verified_jobs.len());
	for (job_index, job) in report.battlefield_verified_jobs.iter().enumerate() {
		let directory_name: String = format!("solution-{job_index:03}");
		let job_dir: PathBuf = output_dir.join(&directory_name);
		fs::create_dir_all(&job_dir).map_err(|error| {
			path_error(
				"failed to create level-solve job directory",
				&job_dir,
				error,
			)
		})?;
		let strategy_path: PathBuf = job_dir.join("strategy.maa.json");
		let artifact_path: PathBuf = job_dir.join("strategy.rgs");
		let audit_path: PathBuf = job_dir.join("audit.json");
		fs::write(&strategy_path, job.maa_job_json.as_bytes()).map_err(|error| {
			path_error(
				"failed to write exported MAA job file",
				&strategy_path,
				error,
			)
		})?;
		let artifact_bytes: Vec<u8> = job.artifact.encode().map_err(|error| {
			path_error(
				"failed to encode canonical strategy artifact",
				&artifact_path,
				error,
			)
		})?;
		fs::write(&artifact_path, artifact_bytes).map_err(|error| {
			path_error(
				"failed to write canonical strategy artifact",
				&artifact_path,
				error,
			)
		})?;
		write_pretty_json(&audit_path, job)?;
		entries.push(LevelSolveJobIndexEntry {
			proposal_index: job.proposal_index,
			candidate_keys: job.candidate_keys.clone(),
			estimated_success_probability_millionths: job.estimated_success_probability_millionths,
			probability_basis: job.probability_basis,
			strategy_sha256: job.strategy_sha256.clone(),
			artifact_id: job.artifact_id.clone(),
			battlefield_status: job.battlefield_status.clone(),
			directory: directory_name,
			strategy: "strategy.maa.json".to_owned(),
			artifact: "strategy.rgs".to_owned(),
			audit: "audit.json".to_owned(),
		});
	}
	let index: LevelSolveJobIndex = LevelSolveJobIndex {
		schema_version: LEVEL_SOLVE_JOB_INDEX_SCHEMA_VERSION,
		data_revision: report.data_revision.clone(),
		mechanics_revision: report.mechanics_revision.clone(),
		requested_level: report.requested_level.clone(),
		entries,
	};
	write_pretty_json(&output_dir.join("index.json"), &index)?;
	eprintln!(
		"output: wrote {} complete jobs to {}",
		index.entries.len(),
		output_dir.display()
	);
	Ok(())
}

fn run_replay(args: ReplayArgs) -> CliResult<()> {
	let total_started: Instant = Instant::now();

	let guide_started: Instant = Instant::now();
	eprintln!("guide: reading {}", args.guide.display());
	let guide_bytes: Vec<u8> = fs::read(&args.guide)
		.map_err(|error| path_error("failed to read MAA job file", &args.guide, error))?;
	report_elapsed("guide", guide_started);

	let snapshot_started: Instant = Instant::now();
	eprintln!("snapshot: loading {}", args.snapshot.display());
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(&args.snapshot)
		.map_err(|error| path_error("failed to load and verify snapshot", &args.snapshot, error))?;
	report_elapsed("snapshot", snapshot_started);
	let prefabs: BattlePrefabSnapshot = load_prefab_snapshot(&args.prefab_snapshot, &snapshot)?;
	let replay_started: Instant = Instant::now();
	eprintln!(
		"replay: running {:?} variants at {} ticks/second",
		args.difficulty, args.ticks_per_second
	);
	let output: ReplayWorkflowOutput = replay_workflow(ReplayRequest {
		snapshot: &snapshot,
		prefabs: &prefabs,
		guide_bytes: &guide_bytes,
		difficulty: args.difficulty.into(),
		ticks_per_second: args.ticks_per_second,
	})
	.map_err(|error| io::Error::other(format!("replay workflow failed: {error}")))?;
	report_elapsed("replay", replay_started);

	fs::create_dir_all(&args.output_dir).map_err(|error| {
		path_error(
			"failed to create replay output directory",
			&args.output_dir,
			error,
		)
	})?;
	for variant in &output.variants {
		let artifact_path: PathBuf = args.output_dir.join(format!("{}.rgs", variant.stem));
		let report_path: PathBuf = args.output_dir.join(format!("{}.json", variant.stem));
		fs::write(&artifact_path, &variant.artifact_bytes).map_err(|error| {
			path_error("failed to write replay artifact", &artifact_path, error)
		})?;
		write_pretty_json(&report_path, &variant.report)?;
	}
	write_pretty_json(&args.output_dir.join("index.json"), &output.index)?;
	eprintln!(
		"complete: wrote {} independent variants in {:.3}s",
		output.index.entries.len(),
		total_started.elapsed().as_secs_f64()
	);
	Ok(())
}

fn load_prefab_snapshot(
	path: &Path,
	gamedata: &ArkDataSnapshot,
) -> CliResult<BattlePrefabSnapshot> {
	let started: Instant = Instant::now();
	eprintln!("prefab snapshot: loading {}", path.display());
	let prefabs: BattlePrefabSnapshot = BattlePrefabSnapshot::load(path)
		.map_err(|error| path_error("failed to load battle-prefab snapshot", path, error))?;
	prefabs.ensure_compatible_with(gamedata).map_err(|error| {
		path_error(
			"battle-prefab snapshot does not match gamedata snapshot",
			path,
			error,
		)
	})?;
	eprintln!(
		"prefab snapshot: {} bundles, mechanics revision {}",
		prefabs.bundles().len(),
		prefabs.mechanics_revision()
	);
	report_elapsed("prefab snapshot", started);
	Ok(prefabs)
}

fn write_report(path: Option<&Path>, json: &[u8]) -> CliResult<()> {
	match path {
		Some(path) => {
			eprintln!("output: writing {}", path.display());
			fs::write(path, json)
				.map_err(|error| path_error("failed to write report", path, error))?;
		}
		None => {
			eprintln!("output: writing stdout");
			let stdout: io::Stdout = io::stdout();
			let mut lock: io::StdoutLock<'_> = stdout.lock();
			lock.write_all(json)
				.map_err(|error| io::Error::other(format!("failed to write report: {error}")))?;
			lock.flush()
				.map_err(|error| io::Error::other(format!("failed to flush report: {error}")))?;
		}
	}
	Ok(())
}

fn write_pretty_json(path: &Path, value: &impl Serialize) -> CliResult<()> {
	let mut json: Vec<u8> = serde_json::to_vec_pretty(value)
		.map_err(|error| path_error("failed to serialize JSON", path, error))?;
	json.push(b'\n');
	fs::write(path, json).map_err(|error| path_error("failed to write JSON", path, error))?;
	Ok(())
}

fn path_error(action: &str, path: &Path, error: impl std::fmt::Display) -> io::Error {
	io::Error::other(format!("{action} {}: {error}", path.display()))
}

fn report_elapsed(stage: &str, started: Instant) {
	eprintln!("{stage}: done in {:.3}s", started.elapsed().as_secs_f64());
}

const fn verified_search_stop_reason(status: ExpectedDamageVerifiedSearchStatus) -> &'static str {
	match status {
		ExpectedDamageVerifiedSearchStatus::TopKFilled => {
			"requested battlefield-verified job count was filled"
		}
		ExpectedDamageVerifiedSearchStatus::ProposalBudgetExhausted => {
			"additive proposal budget was exhausted before filling the requested jobs"
		}
		ExpectedDamageVerifiedSearchStatus::AdditiveInfeasible => {
			"the complete additive model proved that no feasible proposal exists"
		}
		ExpectedDamageVerifiedSearchStatus::AdditiveSearchExhausted => {
			"the feasible additive search space was exhausted before filling the requested jobs"
		}
		ExpectedDamageVerifiedSearchStatus::PlanningModelIncomplete => {
			"enumerated materialized proposals were exhausted, but the planning model is incomplete"
		}
		ExpectedDamageVerifiedSearchStatus::CandidateMaterializationIncomplete => {
			"candidate materialization ended before the legal candidate domain was exhausted"
		}
		ExpectedDamageVerifiedSearchStatus::ProposalStreamCancelled => {
			"proposal production was cancelled by the downstream replay coordinator"
		}
	}
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use clap::{error::ErrorKind, Parser};

	use super::{Command, LEVEL_SOLVE_JOB_INDEX_SCHEMA_VERSION};
	use crate::Cli;

	#[test]
	fn maa_job_index_schema_version_is_locked() {
		assert_eq!(LEVEL_SOLVE_JOB_INDEX_SCHEMA_VERSION, 5);
	}

	#[test]
	fn command_help_omits_the_removed_coverage_workflow() {
		let error: clap::Error = Cli::try_parse_from(["ark-cli", "--help"])
			.expect_err("help should stop argument parsing");
		assert_eq!(error.kind(), ErrorKind::DisplayHelp);
		let help: String = error.to_string();
		assert!(help.contains("level"));
		assert!(help.contains("replay"));
		assert!(!help.contains("coverage"));
	}

	#[test]
	fn removed_coverage_command_is_rejected() {
		let error: clap::Error = Cli::try_parse_from(["ark-cli", "coverage"])
			.expect_err("the public coverage workflow was removed");
		assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
		assert!(error.to_string().contains("coverage"));
	}

	#[test]
	fn replay_arguments_parse_with_tick_rate() {
		let cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"replay",
			"--snapshot",
			"snapshot",
			"--prefab-snapshot",
			"prefabs",
			"--guide",
			"guide.json",
			"--difficulty",
			"challenge",
			"--output-dir",
			"output",
			"--ticks-per-second",
			"60",
		])
		.unwrap();
		let crate::Command::Solver(Command::Replay(args)) = cli.command else {
			panic!("expected replay command");
		};
		assert_eq!(args.ticks_per_second, 60);
		assert_eq!(args.difficulty, super::CliDifficulty::Challenge);
		assert_eq!(args.prefab_snapshot, PathBuf::from("prefabs"));
		assert_eq!(args.output_dir, PathBuf::from("output"));
	}

	#[test]
	fn replay_rejects_an_implicit_difficulty() {
		let error: clap::Error = Cli::try_parse_from([
			"ark-cli",
			"replay",
			"--snapshot",
			"snapshot",
			"--prefab-snapshot",
			"prefabs",
			"--guide",
			"guide.json",
			"--output-dir",
			"output",
		])
		.expect_err("replay difficulty must be explicit");
		assert!(error.to_string().contains("--difficulty"));
	}

	#[test]
	fn level_solve_accepts_complete_job_output_directory() {
		let cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"level",
			"solve",
			"BD-7",
			"--snapshot",
			"snapshot",
			"--prefab-snapshot",
			"prefabs",
			"--guide",
			"guide.json",
			"--difficulty",
			"normal",
			"--output-dir",
			"jobs",
		])
		.unwrap();
		let crate::Command::Solver(Command::Level(level)) = cli.command else {
			panic!("expected level command");
		};
		let super::LevelCommand::Solve(args) = level.command else {
			panic!("expected level solve command");
		};
		assert_eq!(args.prefab_snapshot, PathBuf::from("prefabs"));
		assert_eq!(args.difficulty, super::CliDifficulty::Normal);
		assert_eq!(args.output_dir, Some(PathBuf::from("jobs")));
	}

	#[test]
	fn level_validate_accepts_complete_job_inputs() {
		let cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"level",
			"validate",
			"BD-7",
			"--snapshot",
			"snapshot",
			"--prefab-snapshot",
			"prefabs",
			"--source-guide",
			"source.json",
			"--strategy",
			"strategy.json",
			"--artifact",
			"strategy.rgs",
			"--difficulty",
			"challenge",
			"--output",
			"validation.json",
		])
		.unwrap();
		let crate::Command::Solver(Command::Level(level)) = cli.command else {
			panic!("expected level command");
		};
		let super::LevelCommand::Validate(args) = level.command else {
			panic!("expected level validate command");
		};
		assert_eq!(args.level, "BD-7");
		assert_eq!(args.prefab_snapshot, PathBuf::from("prefabs"));
		assert_eq!(args.artifact, PathBuf::from("strategy.rgs"));
		assert_eq!(args.difficulty, super::CliDifficulty::Challenge);
		assert_eq!(args.output, Some(PathBuf::from("validation.json")));
	}

	#[test]
	fn level_workflow_help_exposes_difficulty_and_omits_query() {
		for workflow in ["solve", "validate"] {
			let error: clap::Error = Cli::try_parse_from(["ark-cli", "level", workflow, "--help"])
				.expect_err("help should stop argument parsing");
			assert_eq!(error.kind(), ErrorKind::DisplayHelp);
			let help: String = error.to_string();
			assert!(help.contains("--difficulty"));
			assert!(!help.contains("--query"));
		}
	}

	#[test]
	fn level_solve_rejects_the_removed_query_argument() {
		let error: clap::Error = Cli::try_parse_from([
			"ark-cli",
			"level",
			"solve",
			"BD-7",
			"--snapshot",
			"snapshot",
			"--prefab-snapshot",
			"prefabs",
			"--guide",
			"guide.json",
			"--difficulty",
			"normal",
			"--query",
			"query.json",
		])
		.expect_err("the query file is no longer accepted");
		assert_eq!(error.kind(), ErrorKind::UnknownArgument);
		assert!(error.to_string().contains("--query"));
	}

	#[test]
	fn level_validate_rejects_the_removed_query_argument() {
		let error: clap::Error = Cli::try_parse_from([
			"ark-cli",
			"level",
			"validate",
			"BD-7",
			"--snapshot",
			"snapshot",
			"--prefab-snapshot",
			"prefabs",
			"--source-guide",
			"source.json",
			"--strategy",
			"strategy.json",
			"--artifact",
			"strategy.rgs",
			"--difficulty",
			"normal",
			"--query",
			"query.json",
		])
		.expect_err("the query file is no longer accepted");
		assert_eq!(error.kind(), ErrorKind::UnknownArgument);
		assert!(error.to_string().contains("--query"));
	}
}
