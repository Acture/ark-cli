//! Level solve exporting a 1-1 job whose retained roster keeps an automatic deployment-point
//! granter, and the independent validation of that job.
//!
//! The source guide retains 讯使 on her automatic first skill, which grants nine deployment points
//! at tick 510 and every 900 ticks after, and hands the solver one replaceable slot. Before
//! planning scheduled skill grants, the exported job could not be materialized at all: replay
//! credited that grant, planning modelled only natural regeneration, and the deploy-tick equality
//! the export requires rejected every proposal with `artifact_deployment_tick_mismatch`.

use std::collections::BTreeSet;

use ark_solver::data::{ArkDataSnapshot, BattlePrefabSnapshot};
use ark_solver::maa_job::MaaJobDifficulty;
use ark_solver::replay::ExpectedReplayReportStatus;
use ark_solver::workflow::{
	solve, validate_export, ExpectedDamageDpEntryAudit, ExpectedDamageExportValidationReport,
	ExpectedDamagePreparedJob, ExpectedDamagePreparedLevelReport, ExpectedDamageProbabilityBasis,
	ExportValidationRequest, SolveRequest, EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION,
	EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION,
};
use ark_solver::DeterministicDamageStatus;

mod support;
use support::local_battle_snapshots;

const SOURCE_GUIDE: &[u8] = include_bytes!("fixtures/maa_job_main_01_01_solve_source.json");
const SOURCE_GUIDE_ID: &str = "fixture:main-01-01-solve-source";
const ROSTER: &[u8] = include_bytes!("../examples/level_solve_rosters/bd7-exusiai-s2.json");
const LEVEL: &str = "main_01-01";

fn solve_main_01_01(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
) -> ExpectedDamagePreparedLevelReport {
	solve(SolveRequest {
		snapshot,
		prefabs,
		requested_level: LEVEL,
		guide_bytes: SOURCE_GUIDE,
		guide_source_id: SOURCE_GUIDE_ID,
		difficulty: MaaJobDifficulty::Normal,
		roster_json: Some(ROSTER),
		excluded_operators: &BTreeSet::new(),
		top_k: Some(1),
		additive_proposal_limit: 20,
		replay_worker_count: 2,
		guide_selection_budget: 10,
	})
	.expect("the 1-1 source guide and reviewed roster produce a complete retained-only gap")
}

fn dp_entry<'a>(job: &'a ExpectedDamagePreparedJob, name: &str) -> &'a ExpectedDamageDpEntryAudit {
	job.dp
		.entries
		.iter()
		.find(|entry| entry.operator_name == name)
		.unwrap_or_else(|| panic!("the exported DP ledger records {name}"))
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn level_solve_exports_a_one_one_job_whose_schedule_spends_the_couriers_grant() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let report: ExpectedDamagePreparedLevelReport = solve_main_01_01(&snapshot, &prefabs);

	// The deploy-tick equality is what used to reject every proposal on this roster.
	assert!(
		!report
			.job_materialization_failures
			.iter()
			.any(|failure| failure.reason_code == "artifact_deployment_tick_mismatch"),
		"{:?}",
		report.job_materialization_failures
	);
	let [job] = report.battlefield_verified_jobs.as_slice() else {
		panic!(
			"expected one exported job, found {:?} with failures {:?}",
			report.battlefield_verified_jobs.len(),
			report.job_materialization_failures
		);
	};
	assert_eq!(
		job.schema_version,
		EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION
	);
	assert_eq!(
		job.battlefield_status,
		ExpectedReplayReportStatus::ExpectedVictory
	);
	assert_eq!(
		job.probability_basis,
		ExpectedDamageProbabilityBasis::FullBattlefieldReplay
	);
	assert!(job.estimated_success_probability_millionths > 0);

	// 讯使 is retained and deploys at battle start for the stage's whole initial DP. Her skill's
	// first grant lands in tick 510, and the DP ledger records that the deployment executing in
	// that tick spent it: seventeen regenerated plus nine granted covers 蛇屠箱's nineteen.
	assert_eq!(dp_entry(job, "讯使").deploy_tick, 0);
	assert_eq!(dp_entry(job, "讯使").granted_dp, 0);
	let blocker: &ExpectedDamageDpEntryAudit = dp_entry(job, "蛇屠箱");
	assert_eq!(blocker.deploy_tick, 510);
	assert_eq!(blocker.regenerated_dp, 17);
	assert_eq!(blocker.granted_dp, 9);
	assert_eq!(blocker.dp_before_deployment, 26);
	assert_eq!(blocker.dp_after_deployment, 7);
	// Regeneration alone would have afforded her only at tick 570, and every later deployment
	// would have moved with her.
	assert!(job.dp.entries.iter().all(|entry| entry.deploy_tick != 570));
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_exported_one_one_job_passes_independent_export_validation() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let report: ExpectedDamagePreparedLevelReport = solve_main_01_01(&snapshot, &prefabs);
	let [job] = report.battlefield_verified_jobs.as_slice() else {
		panic!(
			"expected one exported job, found failures {:?}",
			report.job_materialization_failures
		);
	};
	let artifact_bytes: Vec<u8> = job.artifact.encode().expect("the artifact encodes");

	// Independent validation rebuilds the schedule from the exported strategy alone and requires
	// every replayed deployment tick to equal it; it reported strategy_deployment_dp_tick_mismatch
	// for a retained granter before planning scheduled skill grants.
	let validation: ExpectedDamageExportValidationReport =
		validate_export(ExportValidationRequest {
			snapshot: &snapshot,
			prefabs: &prefabs,
			requested_level: LEVEL,
			source_guide_bytes: SOURCE_GUIDE,
			source_guide_source_id: SOURCE_GUIDE_ID,
			maa_job_bytes: job.maa_job_json.as_bytes(),
			artifact_bytes: &artifact_bytes,
			difficulty: MaaJobDifficulty::Normal,
			guide_selection_budget: 10,
		})
		.expect("the exported 1-1 job validates independently");

	assert_eq!(
		validation.schema_version,
		EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION
	);
	assert_eq!(
		validation.damage_status,
		DeterministicDamageStatus::Feasible
	);
	assert_eq!(
		validation.battlefield_status,
		ExpectedReplayReportStatus::ExpectedVictory
	);
	assert_eq!(validation.artifact_id, job.artifact_id);
	assert_eq!(validation.strategy_sha256, job.strategy_sha256);
	// The independently rebuilt DP ledger is the solver's, grant for grant.
	assert_eq!(validation.dp, job.dp);
}
