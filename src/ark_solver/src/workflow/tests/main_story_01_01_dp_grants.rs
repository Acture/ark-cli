//! Planning's 1-1 deployment schedule against the deploy ticks the replay baseline pins.
//!
//! `tests/main_story_01_01_baseline.rs` replays both authored 1-1 rosters and
//! pins what actually happens: 讯使's automatic first skill grants nine deployment points at tick
//! 510 and every 900 ticks after, and the deployments that wait for DP land earlier because of it.
//! These tests drive the planning side of the same two jobs and require the same ticks, so a
//! schedule that goes back to modelling regeneration alone fails here rather than at export.

use std::{collections::BTreeMap, num::NonZeroU32};

use crate::data::{
	ArkDataSnapshot, BattlePrefabSnapshot, CompiledStage, StageDifficulty, StageIdentity, StageRef,
};
use crate::simulation::TickRate;

use super::{
	guide_deployment_domain_from_maa_job, guide_exact_action_overrides,
	resolve_no_module_max_build, validate_replayed_deployment_tick, LEVEL_SOLVE_TICKS_PER_SECOND,
};
use crate::maa_job::{parse_maa_job, MaaJob, MaaJobDifficulty, MaaJobSeed, MaaJobSourceLineage};
use crate::planning::internal::prepare_interleaved_export_deployment_schedule_with_cost_overrides;
use crate::planning::{
	prepare_guide_deployment_schedule_with_cost_overrides, BasePanelBuildSpec, CandidatePoolSpec,
	GuideDeploymentActionOverrides, GuideDeploymentDomain, PlannedDeploymentDpGrant,
	PreparedGuideDeploymentSchedule,
};
use crate::replay::{
	execute_expected_replay, materialize_expected_replay_input, ExactReplayBuildBinding,
	ExpectedReplayCompileInput, ExpectedReplayOutput,
};
use crate::workflow::prepare_guide_actions;

const AUTHORED_GUIDE: &str =
	include_str!("../../../tests/fixtures/maa_job_main_01_01_authored.json");
const REVIEWED_GUIDE: &str =
	include_str!("../../../tests/fixtures/maa_job_main_01_01_reviewed.json");
const STAGE_ID: &str = "main_01-01";

/// The authored roster, in the guide's own action order. 讯使 is the deployment-point granter.
const AUTHORED_ROSTER: [&str; 4] = [
	"char_198_blackd",
	"char_150_snakek",
	"char_204_platnm",
	"char_130_doberm",
];

/// The reviewed roster, which replaces the two operators whose semantics are still unreviewed.
const REVIEWED_ROSTER: [&str; 4] = [
	"char_198_blackd",
	"char_150_snakek",
	"char_1046_sbell2",
	"char_2025_shu",
];

/// 讯使's first skill at level ten: thirteen of thirty SP, one SP a second, instantaneous, nine DP.
const COURIER_GRANT: PlannedDeploymentDpGrant = PlannedDeploymentDpGrant::new(510, 900, 9);

struct LocalSnapshots {
	snapshot: ArkDataSnapshot,
	prefabs: BattlePrefabSnapshot,
}

fn local_snapshots() -> LocalSnapshots {
	let snapshot: ArkDataSnapshot =
		ArkDataSnapshot::load(&crate::test_support::configured_gamedata_dir())
			.expect("verified local gamedata");
	let prefabs: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(&crate::test_support::configured_battle_prefab_dir())
			.expect("verified local battle prefabs");
	prefabs
		.ensure_compatible_with(&snapshot)
		.expect("matching local revisions");
	LocalSnapshots { snapshot, prefabs }
}

fn tick_rate() -> TickRate {
	TickRate::new(NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("tick rate is nonzero"))
}

/// Builds the planning domain for one 1-1 guide around the named source-slot deployment.
///
/// The source slot needs no compiled coverage candidate, so it is also where an operator whose
/// base-panel attack planning does not yet model has to sit.
fn guide_domain(
	local: &LocalSnapshots,
	guide_json: &str,
	source_id: &str,
	roster: [&str; 4],
	source_action_index: usize,
) -> GuideDeploymentDomain {
	let guide: MaaJob = parse_maa_job(guide_json).expect("fixture guide parses");
	let digest: String = "ab".repeat(32);
	let lineage: MaaJobSourceLineage =
		MaaJobSourceLineage::new(source_id, &digest).expect("fixture lineage is valid");
	let seed = guide
		.executable_guide_seeds(lineage, MaaJobDifficulty::Normal, 10)
		.expect("fixture guide has executable seeds")
		.next()
		.expect("fixture guide has one selection");
	let stage: CompiledStage = local
		.snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(STAGE_ID.to_owned()),
			difficulty: StageDifficulty::Normal,
		})
		.expect("1-1 compiles from the pinned snapshot");
	let builds: Vec<BasePanelBuildSpec> = roster
		.iter()
		.map(|operator_id: &&str| {
			let (resolved, _) = resolve_no_module_max_build(&local.snapshot, operator_id, 1)
				.expect("roster entry has a maximal no-module build");
			BasePanelBuildSpec {
				operator_id: resolved.operator_id().to_owned(),
				elite: resolved.build().elite(),
				level: resolved.build().level(),
			}
		})
		.collect();
	guide_deployment_domain_from_maa_job(
		&seed,
		source_action_index,
		stage.map_dimensions(),
		CandidatePoolSpec::Roster { builds },
	)
	.expect("fixture guide converts to a planning domain")
}

fn prepared_schedule(
	local: &LocalSnapshots,
	domain: &GuideDeploymentDomain,
	overrides: &GuideDeploymentActionOverrides,
) -> PreparedGuideDeploymentSchedule {
	prepare_guide_deployment_schedule_with_cost_overrides(
		&local.snapshot,
		NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("tick rate is nonzero"),
		domain,
		overrides,
	)
	.expect("1-1 guide schedules from the pinned snapshot")
}

fn deploy_ticks(schedule: &PreparedGuideDeploymentSchedule) -> Vec<u64> {
	schedule
		.deployments
		.iter()
		.map(|deployment| deployment.deploy_tick())
		.collect()
}

fn granted_dp(schedule: &PreparedGuideDeploymentSchedule) -> Vec<u64> {
	schedule
		.deployments
		.iter()
		.map(|deployment| deployment.dp_schedule.granted_dp)
		.collect()
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_authored_one_one_schedule_equals_its_replayed_deploy_ticks() {
	let local: LocalSnapshots = local_snapshots();
	let domain: GuideDeploymentDomain = guide_domain(
		&local,
		AUTHORED_GUIDE,
		"fixture:main-01-01-authored",
		AUTHORED_ROSTER,
		3,
	);
	let overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(&local.snapshot, &local.prefabs, &domain, tick_rate())
			.expect("every authored action resolves an exact build");

	// Only 讯使 grants, and she is retained: 杜宾's deployment is the source slot, whose own
	// skill planning never schedules.
	assert_eq!(
		overrides.dp_grants_by_action,
		BTreeMap::from([(0, vec![COURIER_GRANT])])
	);

	let schedule: PreparedGuideDeploymentSchedule = prepared_schedule(&local, &domain, &overrides);

	// The replay baseline pins exactly these four ticks for this roster.
	assert_eq!(deploy_ticks(&schedule), [0, 510, 630, 1_020]);
	// The one grant that pays for anything is the first, which lifts 蛇屠箱's 17 regenerated DP
	// to 26 in tick 510. The two that follow her arrive after the pool already covers the cost.
	assert_eq!(granted_dp(&schedule), [0, 9, 0, 0]);
	assert_eq!(schedule.deployments[1].dp_schedule.regenerated_dp, 17);
	assert_eq!(schedule.deployments[1].dp_schedule.dp_before_deployment, 26);
	assert_eq!(schedule.deployments[1].dp_schedule.dp_after_deployment, 7);

	// Without the grant the same prefix is the regeneration-only schedule the replay produces
	// when 讯使 carries her manual second skill instead, which the baseline also pins.
	let passive_schedule: PreparedGuideDeploymentSchedule = prepared_schedule(
		&local,
		&domain,
		&GuideDeploymentActionOverrides {
			deployment_cost_by_action: overrides.deployment_cost_by_action.clone(),
			dp_grants_by_action: BTreeMap::new(),
		},
	);
	assert_eq!(deploy_ticks(&passive_schedule), [0, 570, 900, 1_290]);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_reviewed_one_one_schedule_equals_its_replayed_deploy_ticks() {
	let local: LocalSnapshots = local_snapshots();
	let domain: GuideDeploymentDomain = guide_domain(
		&local,
		REVIEWED_GUIDE,
		"fixture:main-01-01-reviewed",
		REVIEWED_ROSTER,
		2,
	);
	let overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(&local.snapshot, &local.prefabs, &domain, tick_rate())
			.expect("every reviewed action resolves an exact build");

	assert_eq!(
		overrides.dp_grants_by_action,
		BTreeMap::from([(0, vec![COURIER_GRANT])])
	);

	let schedule: PreparedGuideDeploymentSchedule = prepared_schedule(&local, &domain, &overrides);

	// The reviewed roster's two six-stars cost more, so the second grant pays as well: it arrives
	// in tick 1410 and takes 黍's 15 regenerated DP to 24 for a 20 DP deployment.
	assert_eq!(deploy_ticks(&schedule), [0, 510, 960, 1_410]);
	assert_eq!(granted_dp(&schedule), [0, 9, 0, 9]);
	assert_eq!(schedule.deployments[3].dp_schedule.regenerated_dp, 15);
	assert_eq!(schedule.deployments[3].dp_schedule.dp_before_deployment, 24);
	assert_eq!(schedule.deployments[3].dp_schedule.dp_after_deployment, 4);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn a_retained_manual_skill_schedules_no_grant() {
	let local: LocalSnapshots = local_snapshots();
	// 讯使's second skill is the same reviewed charge-cost owner, but it is manual: nothing in a
	// retained slot casts it, so planning must schedule nothing for it even though the template
	// claims a grant and the cadence exists.
	let manual_guide: String = AUTHORED_GUIDE.replace(
		"\"name\": \"讯使\",\n\t\t\t\"skill\": 1,\n\t\t\t\"skill_usage\": 0",
		"\"name\": \"讯使\",\n\t\t\t\"skill\": 2,\n\t\t\t\"skill_usage\": 0",
	);
	assert_ne!(manual_guide, AUTHORED_GUIDE);
	let domain: GuideDeploymentDomain = guide_domain(
		&local,
		&manual_guide,
		"fixture:main-01-01-authored-manual",
		AUTHORED_ROSTER,
		3,
	);
	let overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(&local.snapshot, &local.prefabs, &domain, tick_rate())
			.expect("every authored action resolves an exact build");

	assert!(overrides.dp_grants_by_action.is_empty());
	assert_eq!(
		deploy_ticks(&prepared_schedule(&local, &domain, &overrides)),
		[0, 570, 900, 1_290]
	);
}

/// 1-1's map height, which the MAA row convention is mirrored against.
const MAP_HEIGHT: u32 = 7;

fn exact_binding(
	local: &LocalSnapshots,
	source_name: &str,
	operator_id: &str,
	skill: u8,
) -> ExactReplayBuildBinding {
	let (resolved, _) = resolve_no_module_max_build(&local.snapshot, operator_id, skill)
		.expect("roster entry has a maximal no-module build");
	ExactReplayBuildBinding {
		source_name: source_name.to_owned(),
		build: resolved.build().clone(),
		skill_usage: 0,
		skill_times: None,
	}
}

/// Replays one 1-1 job exactly as the export validator does, and returns each action's tick.
fn replayed_action_ticks(
	local: &LocalSnapshots,
	guide_json: &str,
	bindings: &[ExactReplayBuildBinding],
	variant_id: &str,
) -> BTreeMap<usize, Option<u64>> {
	let job: MaaJob = parse_maa_job(guide_json).expect("fixture guide parses");
	let seed: MaaJobSeed = job
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal selection expands")
		.next()
		.expect("the fixture declares one executable selection")
		.materialize();
	let guide = prepare_guide_actions(&seed, bindings, MAP_HEIGHT).expect("guide actions prepare");
	let input = materialize_expected_replay_input(
		&local.snapshot,
		&local.prefabs,
		ExpectedReplayCompileInput {
			stage_name: seed.stage_name(),
			difficulty: StageDifficulty::Normal,
			bindings,
			guide: &guide,
			tick_rate: tick_rate(),
		},
	)
	.expect("1-1 materializes from the pinned snapshots");
	let output: ExpectedReplayOutput = execute_expected_replay(&input, [0_u8; 32], variant_id);
	output
		.report
		.actions
		.iter()
		.map(|action| (action.action_index, action.executed_tick))
		.collect()
}

/// Runs the export's deploy-tick equality over one schedule and one replay.
///
/// A rejection is reported as the action it names beside its code, so a test cannot pass on a
/// mismatch somewhere other than the one it is about.
fn export_tick_equality(
	schedule: &PreparedGuideDeploymentSchedule,
	replayed: &BTreeMap<usize, Option<u64>>,
) -> Result<(), (usize, String)> {
	for deployment in &schedule.deployments {
		let action_index: usize = deployment.action.action_index;
		validate_replayed_deployment_tick(
			&deployment.action.operator_name,
			action_index,
			deployment.deploy_tick(),
			replayed.get(&action_index).copied().flatten(),
		)
		.map_err(|error| (action_index, error.code().to_owned()))?;
	}
	Ok(())
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_export_deploy_tick_check_accepts_a_retained_automatic_granter() {
	// This is the check that used to reject every proposal whose retained roster kept 讯使: the
	// exported strategy's interleaved schedule against the ticks the full battlefield replay
	// actually executed at. It is exercised here on the reviewed 1-1 roster, which retains her.
	let local: LocalSnapshots = local_snapshots();
	let domain: GuideDeploymentDomain = guide_domain(
		&local,
		REVIEWED_GUIDE,
		"fixture:main-01-01-reviewed",
		REVIEWED_ROSTER,
		2,
	);
	let overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(&local.snapshot, &local.prefabs, &domain, tick_rate())
			.expect("every reviewed action resolves an exact build");
	let schedule: PreparedGuideDeploymentSchedule =
		prepare_interleaved_export_deployment_schedule_with_cost_overrides(
			&local.snapshot,
			NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("tick rate is nonzero"),
			&domain,
			&overrides,
		)
		.expect("the reviewed 1-1 job schedules as an exported strategy");
	let bindings: Vec<ExactReplayBuildBinding> = vec![
		exact_binding(&local, "讯使", "char_198_blackd", 1),
		exact_binding(&local, "蛇屠箱", "char_150_snakek", 1),
		exact_binding(&local, "圣聆初雪", "char_1046_sbell2", 1),
		exact_binding(&local, "黍", "char_2025_shu", 1),
	];
	let replayed: BTreeMap<usize, Option<u64>> = replayed_action_ticks(
		&local,
		REVIEWED_GUIDE,
		&bindings,
		"main-01-01-reviewed-export-ticks",
	);

	assert_eq!(export_tick_equality(&schedule, &replayed), Ok(()));

	// Without the grants the same comparison fails; scheduling the grants is what removes it.
	let passive: GuideDeploymentActionOverrides = GuideDeploymentActionOverrides {
		deployment_cost_by_action: overrides.deployment_cost_by_action.clone(),
		dp_grants_by_action: BTreeMap::new(),
	};
	let passive_schedule: PreparedGuideDeploymentSchedule =
		prepare_interleaved_export_deployment_schedule_with_cost_overrides(
			&local.snapshot,
			NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("tick rate is nonzero"),
			&domain,
			&passive,
		)
		.expect("the regeneration-only schedule still materializes");
	// 蛇屠箱's deployment at action 1 is the one the first grant paid for.
	assert_eq!(
		export_tick_equality(&passive_schedule, &replayed),
		Err((1, "strategy_deployment_dp_tick_mismatch".to_owned()))
	);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_export_deploy_tick_check_still_rejects_a_granter_that_does_not_grant() {
	// The equality check remains the guard for what planning cannot know. Here the schedule keeps
	// 讯使's automatic grant while the battlefield runs her manual second skill, which nothing
	// casts — the same divergence a granter retired or killed before its grant produces.
	let local: LocalSnapshots = local_snapshots();
	let domain: GuideDeploymentDomain = guide_domain(
		&local,
		REVIEWED_GUIDE,
		"fixture:main-01-01-reviewed",
		REVIEWED_ROSTER,
		2,
	);
	let overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(&local.snapshot, &local.prefabs, &domain, tick_rate())
			.expect("every reviewed action resolves an exact build");
	assert_eq!(
		overrides.dp_grants_by_action,
		BTreeMap::from([(0, vec![COURIER_GRANT])])
	);
	let schedule: PreparedGuideDeploymentSchedule =
		prepare_interleaved_export_deployment_schedule_with_cost_overrides(
			&local.snapshot,
			NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("tick rate is nonzero"),
			&domain,
			&overrides,
		)
		.expect("the reviewed 1-1 job schedules as an exported strategy");

	let mut bindings: Vec<ExactReplayBuildBinding> = vec![
		exact_binding(&local, "讯使", "char_198_blackd", 2),
		exact_binding(&local, "蛇屠箱", "char_150_snakek", 1),
		exact_binding(&local, "圣聆初雪", "char_1046_sbell2", 1),
		exact_binding(&local, "黍", "char_2025_shu", 1),
	];
	bindings[0].skill_usage = 0;
	let replayed: BTreeMap<usize, Option<u64>> = replayed_action_ticks(
		&local,
		REVIEWED_GUIDE,
		&bindings,
		"main-01-01-reviewed-ungranted-export-ticks",
	);

	assert_eq!(
		export_tick_equality(&schedule, &replayed),
		Err((1, "strategy_deployment_dp_tick_mismatch".to_owned()))
	);
}
