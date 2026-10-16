//! Level solve on 1-1 whose retained-only battlefield is clean until the third wave.
//!
//! The source guide is the reviewed 1-1 roster with 圣聆初雪 as the one replaceable slot, deployed
//! last so a replacement's skill schedule can follow it. Without her, the retained 讯使, 蛇屠箱 and
//! 黍 defeat every enemy of the first two wave fragments and leak exactly three enemies of the
//! third, so any view of the battle that stops early sees no gap at all. The solver must still rank
//! on the complete causal battle, and only a complete battlefield replay may accept a proposal.
//! Every enemy record auto-decodes from its pinned battle prefab; nothing here is hand-modelled.
//!
//! This file replaces two tests deleted from `maa_job_bd7_fixture.rs` with the hand-written
//! BD-7 enemy catalog. No BD-7 number is ported; 1-1's own measured numbers are pinned.
//!
//! `bd7_expected_damage_solver_uses_the_full_battle_gap_instead_of_local_evidence` becomes
//! [`solver_ranks_on_the_full_battle_gap_not_local_evidence`]:
//!
//! - guide source id, content digest, difficulty, selection index, selected slots, source action
//!   and source operator: restored, with 1-1's three grouped slots plus the group-less source.
//! - planning context rebuilt from `report.full_battle_damage_gap`, one residual demand per
//!   wave-local HP group, terminal group per anchor summing to the stage's total HP, every demand
//!   `causal/`: restored.
//! - three wave timings: restored; 36 ledger enemies becomes 1-1's 33.
//! - effective cohorts reach fragments 1 and 2: restored.
//! - some candidate off BD-7's cell `(5, 3)`: that cell is BD-7's; the claim it guarded — the
//!   candidate slice is not collapsed onto one tile — is restored as "more than one cell".
//! - no damage-feasible proposal, one battlefield replay audit per replay candidate: restored.
//! - `AdditiveProposalLimitReached`, no job, no victory index and no job materialization failure:
//!   **inverted**, because 1-1 is
//!   solvable where BD-7 was not. 能天使 S2 closes the late gap, the second proposal fills top-k,
//!   and both replayed proposals are `ExpectedVictory` although neither is damage-feasible: the
//!   planner's score never accepted them, the complete replay did. The invariant the BD-7 test
//!   guarded — planning ranks, replay decides — is asserted directly instead of through an
//!   unsolvable stage. The other victory is refused at export as `activation_not_exportable`, so
//!   exactly one materialization failure is pinned where BD-7 had none.
//! - new: every residual demand's required damage equals its wave-local group's HP, so a demand
//!   set that keeps its 33 keys but zeroes groups without direct retained-only gap evidence fails.
//!
//! `bd7_workflow_owned_domain_cannot_hide_a_late_wave_damage_gap` becomes
//! [`a_clean_early_battlefield_cannot_hide_the_late_wave_gap`]:
//!
//! - BD-7 enemy instance 8's spawn 8787 and deadline 10422: not portable (catalog numbers); the
//!   claim is restored on 1-1's three leaked third-fragment enemies with their own ticks, and
//!   strengthened by requiring every first- and second-fragment enemy to be defeated.
//! - the late enemy's cohort is an effective target: restored for every ledger enemy.
//! - every residual demand is a space-time HP group: restored, plus a positive demand anchored on
//!   the late fragment.
//! - accepted proposals must close the late gap: restored as "every replay that is accepted kills
//!   the three enemies the retained plan leaked, and leaks nothing".
//! - no export while the late gap stays unsolved: **not restorable on this input**. No reviewed
//!   candidate at this revision both completes a replay and leaks in the third fragment. The
//!   reviewed skill-damage registry holds four selections and each was run against this source:
//!   能天使 S2 solves it, while 红云 S2, 阿米娅 S1 and 逻各斯 S3 clear it in all twenty replays with
//!   an unmodelled talent; any other selection fails with `no_supported_candidates`. A replay-refuted late-gap proposal needs a reviewed but
//!   insufficient candidate, which is still open.
//!
//! [`a_cleared_replay_with_an_omission_exports_nothing`] adds the export side of "replay decides":
//! 红云 S2 clears the stage in all twenty replays, and because her SP talent stays unmodelled each
//! replay is `Incomplete`, so nothing exports although every one of them ends in `Victory`.
//!
//! Not detectable here: which HP groups feed the retained ranking score. On this roster both
//! proposals are the same 能天使 placement whatever the score, and the score is not in the report,
//! so restricting it to groups with direct gap evidence leaves every output byte-identical; making that
//! input observable is still open.
//!
//! The roster files are single-operator; the four-operator reviewed roster makes this solve run for
//! more than half an hour.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use ark_solver::data::{
	ArkDataSnapshot, BattlePrefabSnapshot, GridPosition, StageDifficulty, StageIdentity, StageRef,
};
use ark_solver::maa_job::MaaJobDifficulty;
use ark_solver::mechanics::domain::FixedDamage;
use ark_solver::planning::{
	prepare_full_battle_unblocked_planning_context, ContributionConstraintKey,
	ExpectedDamageLevelSolveStatus, ExpectedDamageProposalDisposition, FullBattleEnemyDamageGap,
	FullBattleEnemyResolution, FullBattlePlanningContext, FullBattleWaveTiming, StageTimeAnchor,
};
use ark_solver::replay::{ExpectedReplayAuditReport, ExpectedReplayReportStatus};
use ark_solver::simulation::TickRate;
use ark_solver::workflow::{
	solve, ExpectedDamageBattlefieldReplayResult, ExpectedDamagePreparedLevelReport,
	ExpectedDamageProbabilityBasis, ExpectedDamageVerifiedSearchStatus, SolveRequest,
};
use sha2::{Digest, Sha256};

mod support;
use support::local_battle_snapshots;

const SOURCE_GUIDE: &[u8] = include_bytes!("fixtures/maa_job_main_01_01_late_gap_source.json");
const SOURCE_GUIDE_ID: &str = "fixture:main-01-01-late-gap-source";
const EXUSIAI_ROSTER: &[u8] = include_bytes!("../examples/level_solve_rosters/bd7-exusiai-s2.json");
/// 红云 S2 alone: a reviewed damage binding, and a talent no reviewed template claims.
const CLOUR_ROSTER: &[u8] =
	br#"{"schema_version": 1, "operators": [{"operator": "char_190_clour", "skill": 2}]}"#;
const LEVEL: &str = "main_01-01";
const TICKS_PER_SECOND: u32 = 30;
/// Fragment prefix of the third wave, the only one the retained-only battle leaks.
const LATE_FRAGMENT: &str = "wave:0/fragment:2/";
/// The retained-only battle's leaked instances, all in the third fragment.
const LATE_LEAKED_INSTANCES: [u32; 3] = [29, 30, 32];

fn solve_late_gap(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	roster: &[u8],
) -> ExpectedDamagePreparedLevelReport {
	solve(SolveRequest {
		snapshot,
		prefabs,
		requested_level: LEVEL,
		guide_bytes: SOURCE_GUIDE,
		guide_source_id: SOURCE_GUIDE_ID,
		difficulty: MaaJobDifficulty::Normal,
		roster_json: Some(roster),
		excluded_operators: &BTreeSet::new(),
		top_k: Some(1),
		additive_proposal_limit: 20,
		replay_worker_count: 2,
		guide_selection_budget: 10,
	})
	.expect("the 1-1 late-gap source guide produces a complete retained-only gap")
}

/// `(instance, enemy key, cohort, ordinal, spawn tick, failure deadline, required micros)`.
type LeakedEnemy<'a> = (u32, &'a str, &'a str, u32, Option<u64>, Option<u64>, u64);

fn is_early_fragment(cohort: &str) -> bool {
	cohort.starts_with("wave:0/fragment:0/") || cohort.starts_with("wave:0/fragment:1/")
}

/// Every attempted proposal's completed battlefield replay report, in attempt order.
fn completed_replays(
	report: &ExpectedDamagePreparedLevelReport,
) -> Vec<&ExpectedReplayAuditReport> {
	report
		.battlefield_replays
		.iter()
		.map(|replay| match &replay.result {
			ExpectedDamageBattlefieldReplayResult::Completed { report } => report.as_ref(),
			ExpectedDamageBattlefieldReplayResult::MaterializationFailed {
				reason_code,
				detail,
			} => {
				panic!(
					"proposal {} never reached replay: {reason_code}: {detail}",
					replay.proposal_index
				)
			}
		})
		.collect()
}

/// No proposal is accepted on its planning score: every one is a replay candidate, none is
/// damage-feasible, and each candidate is handed to a battlefield replay that completes.
fn assert_planning_only_ranks(report: &ExpectedDamagePreparedLevelReport) {
	assert!(report.solve.proposals.iter().all(|proposal| matches!(
		proposal.disposition,
		ExpectedDamageProposalDisposition::PlanningReplayCandidate { .. }
	)));
	assert!(report.solve.damage_feasible_proposal_indices.is_empty());
	let replayed: Vec<usize> = report
		.battlefield_replays
		.iter()
		.map(|replay| replay.proposal_index)
		.collect();
	assert_eq!(replayed, report.solve.replay_candidate_indices);
	assert_eq!(
		completed_replays(report).len(),
		report.solve.replay_candidate_indices.len()
	);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn solver_ranks_on_the_full_battle_gap_not_local_evidence() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let report: ExpectedDamagePreparedLevelReport =
		solve_late_gap(&snapshot, &prefabs, EXUSIAI_ROSTER);
	let expected_guide_digest: String = Sha256::digest(SOURCE_GUIDE)
		.iter()
		.map(|byte: &u8| format!("{byte:02x}"))
		.collect();

	assert_eq!(report.guide_source_id, SOURCE_GUIDE_ID);
	assert_eq!(report.guide_content_sha256, expected_guide_digest);
	assert_eq!(report.guide_difficulty, MaaJobDifficulty::Normal);
	assert_eq!(report.guide_selection_index, 0);
	assert_eq!(
		report.guide_selected_slots,
		BTreeMap::from([
			("先手".to_owned(), "讯使".to_owned()),
			("阻挡".to_owned(), "蛇屠箱".to_owned()),
			("近卫".to_owned(), "黍".to_owned()),
			("圣聆初雪".to_owned(), "圣聆初雪".to_owned()),
		])
	);
	// 圣聆初雪 is the guide's fourth and last deployment.
	assert_eq!(report.source_action_index, 3);
	assert_eq!(report.source_operator_name, "圣聆初雪");

	let stage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(report.stage_id.clone()),
			difficulty: StageDifficulty::Normal,
		})
		.expect("1-1 compiles");
	let planning: FullBattlePlanningContext = prepare_full_battle_unblocked_planning_context(
		&stage,
		TickRate::new(NonZeroU32::new(TICKS_PER_SECOND).unwrap()),
		&report.full_battle_damage_gap,
	)
	.expect("full-stage HP groups rebuild from the retained-only ledger");
	let total_stage_hp: FixedDamage = planning
		.model
		.enemies
		.iter()
		.try_fold(FixedDamage::ZERO, |total: FixedDamage, enemy| {
			total.checked_add(FixedDamage::from_whole(enemy.max_hp).unwrap())
		})
		.unwrap();
	let mut terminal_group_by_anchor: BTreeMap<StageTimeAnchor, (u64, FixedDamage)> =
		BTreeMap::new();
	for group in &planning.wave_local_hp_groups {
		let entry: &mut (u64, FixedDamage) = terminal_group_by_anchor
			.entry(group.id.anchor.clone())
			.or_insert((group.id.goal_deadline_offset_ticks, group.required_hp));
		if group.id.goal_deadline_offset_ticks > entry.0 {
			*entry = (group.id.goal_deadline_offset_ticks, group.required_hp);
		}
	}
	let terminal_group_hp: FixedDamage = terminal_group_by_anchor
		.values()
		.try_fold(FixedDamage::ZERO, |total: FixedDamage, (_, required_hp)| {
			total.checked_add(*required_hp)
		})
		.unwrap();
	assert_eq!(
		report.solve.residual_demands.len(),
		planning.wave_local_hp_groups.len(),
		"each event-relative cumulative HP group becomes one ranking demand"
	);
	// One group per distinct goal offset: 4 + 15 + 14 over the three fragments.
	assert_eq!(report.solve.residual_demands.len(), 33);
	// Each demand owes exactly its group's cumulative HP, early clean fragments included.
	let mut expected_demands: Vec<(String, u64)> = planning
		.wave_local_hp_groups
		.iter()
		.map(|group| {
			let StageTimeAnchor::WaveFragmentStart { wave, fragment } = group.id.anchor else {
				panic!("unexpected wave-local anchor {:?}", group.id.anchor);
			};
			(
				format!(
					"causal/wave:{wave}/fragment:{fragment}/goal_offset:{}",
					group.id.goal_deadline_offset_ticks
				),
				group.required_hp.scaled(),
			)
		})
		.collect();
	let mut actual_demands: Vec<(String, u64)> = report
		.solve
		.residual_demands
		.iter()
		.map(|demand| match &demand.constraint {
			ContributionConstraintKey::SpaceTimeHpGroup { group_id } => {
				(group_id.clone(), demand.required_damage_scaled)
			}
			other => panic!("unexpected ranking demand {other:?}"),
		})
		.collect();
	expected_demands.sort();
	actual_demands.sort();
	assert_eq!(actual_demands, expected_demands);
	// Each fragment's last cut owes its whole HP: 5040 + 17550 + 27050 = 49640.
	let terminal_groups: Vec<(u64, FixedDamage)> =
		terminal_group_by_anchor.values().copied().collect();
	assert_eq!(
		terminal_groups,
		vec![
			(1_015, FixedDamage::from_whole(5_040).unwrap()),
			(1_680, FixedDamage::from_whole(17_550).unwrap()),
			(1_952, FixedDamage::from_whole(27_050).unwrap()),
		]
	);
	assert_eq!(terminal_group_hp, total_stage_hp);
	assert_eq!(total_stage_hp, FixedDamage::from_whole(49_640).unwrap());
	assert!(report.solve.residual_demands.iter().all(|demand| matches!(
		&demand.constraint,
		ContributionConstraintKey::SpaceTimeHpGroup { group_id }
			if group_id.starts_with("causal/")
	)));

	// The retained-only ledger spans all three causally started waves: each starts a fixed delay
	// after the previous clears, 960 + 270 = 1230 and 2664 + 510 = 3174.
	assert_eq!(
		report.full_battle_damage_gap.wave_timings,
		vec![
			FullBattleWaveTiming {
				wave_id: 0,
				started_tick: Some(0),
				cleared_tick: Some(960),
			},
			FullBattleWaveTiming {
				wave_id: 1,
				started_tick: Some(1_230),
				cleared_tick: Some(2_664),
			},
			FullBattleWaveTiming {
				wave_id: 2,
				started_tick: Some(3_174),
				cleared_tick: Some(5_280),
			},
		]
	);
	assert_eq!(report.full_battle_damage_gap.enemies.len(), 33);
	// The stage's 16 spawn cohorts are all scoring targets, early and late alike.
	assert_eq!(report.effective_target_cohort_ids.len(), 16);
	assert!(report
		.effective_target_cohort_ids
		.iter()
		.any(|cohort| cohort.starts_with("wave:0/fragment:1/")));
	assert!(report
		.effective_target_cohort_ids
		.iter()
		.any(|cohort| cohort.starts_with(LATE_FRAGMENT)));
	let candidate_cells: BTreeSet<GridPosition> = report
		.materialized_candidates
		.iter()
		.map(|candidate| candidate.deployment_cell)
		.collect();
	assert!(
		candidate_cells.len() > 1,
		"the candidate slice collapsed onto {candidate_cells:?}"
	);

	// The second proposal fills top-k. Neither is damage-feasible under the incomplete planning
	// model, and both replay to `ExpectedVictory`: the verdict is the replay's, not the score's.
	assert_eq!(
		report.solve.status,
		ExpectedDamageLevelSolveStatus::ProposalVisitorStopped
	);
	assert_eq!(
		report.verified_search_status,
		ExpectedDamageVerifiedSearchStatus::TopKFilled
	);
	assert_eq!(report.solve.proposals.len(), 2);
	assert_planning_only_ranks(&report);
	assert_eq!(report.solve.replay_candidate_indices, vec![0, 1]);
	assert!(completed_replays(&report)
		.iter()
		.all(|replay| replay.status == ExpectedReplayReportStatus::ExpectedVictory));
	assert_eq!(
		report.battlefield_expected_victory_proposal_indices,
		vec![0, 1]
	);
	let [job] = report.battlefield_verified_jobs.as_slice() else {
		panic!(
			"expected one exported job, found failures {:?}",
			report.job_materialization_failures
		);
	};
	assert_eq!(
		job.battlefield_status,
		ExpectedReplayReportStatus::ExpectedVictory
	);
	assert_eq!(
		job.probability_basis,
		ExpectedDamageProbabilityBasis::FullBattlefieldReplay
	);
	// The other victory's skill activation has no exact MAA job encoding, so it is refused at
	// export rather than exported approximately.
	let failure_codes: Vec<(usize, &str)> = report
		.job_materialization_failures
		.iter()
		.map(|failure| (failure.proposal_index, failure.reason_code.as_str()))
		.collect();
	assert_eq!(failure_codes.len(), 1, "{failure_codes:?}");
	assert_eq!(failure_codes[0].1, "activation_not_exportable");
	assert_ne!(failure_codes[0].0, job.proposal_index);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn a_clean_early_battlefield_cannot_hide_the_late_wave_gap() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let report: ExpectedDamagePreparedLevelReport =
		solve_late_gap(&snapshot, &prefabs, EXUSIAI_ROSTER);
	let enemies: &[FullBattleEnemyDamageGap] = &report.full_battle_damage_gap.enemies;

	// Every first- and second-fragment enemy dies: a view that stops before the third fragment
	// sees no gap at all.
	let early: Vec<&FullBattleEnemyDamageGap> = enemies
		.iter()
		.filter(|enemy| is_early_fragment(enemy.planning_target.cohort.as_str()))
		.collect();
	assert_eq!(early.len(), 19, "instances #0..#18");
	assert!(early.iter().all(|enemy| {
		enemy.resolution == FullBattleEnemyResolution::Defeated && enemy.required_damage_micros == 0
	}));

	// Only three third-fragment enemies leak, each with the HP the retained plan still owed at
	// its goal. Two spawn together at 3774; the ranged one follows at 3984.
	let leaked: Vec<LeakedEnemy<'_>> = enemies
		.iter()
		.filter(|enemy| enemy.resolution == FullBattleEnemyResolution::Leaked)
		.map(|enemy| {
			(
				enemy.enemy_instance_id,
				enemy.enemy_key.as_str(),
				enemy.planning_target.cohort.as_str(),
				enemy.planning_target.ordinal,
				enemy.spawned_tick,
				enemy.failure_deadline_tick,
				enemy.required_damage_micros,
			)
		})
		.collect();
	assert_eq!(
		leaked,
		vec![
			(
				29,
				"enemy_1007_slime_2",
				"wave:0/fragment:2/action:3",
				2,
				Some(3_774),
				Some(4_159),
				1_050_000_000,
			),
			(
				30,
				"enemy_1030_wteeth",
				"wave:0/fragment:2/action:4",
				0,
				Some(3_774),
				Some(4_406),
				4_230_000_000,
			),
			(
				32,
				"enemy_1028_mocock",
				"wave:0/fragment:2/action:5",
				0,
				Some(3_984),
				Some(4_519),
				395_000_000,
			),
		]
	);
	// 1050 + 4230 + 395 HP.
	assert_eq!(
		report.full_battle_damage_gap.unresolved_damage_micros(),
		5_675_000_000
	);

	// The clean early cohorts stay scoring targets beside the leaking late ones.
	for enemy in enemies {
		assert!(
			report
				.effective_target_cohort_ids
				.iter()
				.any(|cohort| cohort == enemy.planning_target.cohort.as_str()),
			"cohort {} is not a scoring target",
			enemy.planning_target.cohort.as_str()
		);
	}
	assert!(report.solve.residual_demands.iter().all(|demand| matches!(
		demand.constraint,
		ContributionConstraintKey::SpaceTimeHpGroup { .. }
	)));
	let late_prefix: String = format!("causal/{}", LATE_FRAGMENT.trim_end_matches('/'));
	assert!(report.solve.residual_demands.iter().any(|demand| {
		demand.required_damage_scaled > 0
			&& matches!(
				&demand.constraint,
				ContributionConstraintKey::SpaceTimeHpGroup { group_id }
					if group_id.starts_with(&late_prefix)
			)
	}));

	// An accepted proposal is one whose complete replay kills the three enemies the retained plan
	// leaked, and leaks nothing.
	assert_planning_only_ranks(&report);
	let accepted: Vec<&ExpectedReplayAuditReport> = completed_replays(&report)
		.into_iter()
		.filter(|replay| replay.status == ExpectedReplayReportStatus::ExpectedVictory)
		.collect();
	assert!(!accepted.is_empty());
	for replay in accepted {
		assert!(replay
			.enemies
			.iter()
			.all(|enemy| enemy.leaked_tick.is_none()));
		for instance in LATE_LEAKED_INSTANCES {
			let enemy = replay
				.enemies
				.iter()
				.find(|enemy| enemy.instance_id == instance)
				.unwrap_or_else(|| panic!("the replay audits instance {instance}"));
			assert!(
				enemy.death_tick.is_some(),
				"instance {instance} survives: {enemy:?}"
			);
		}
	}
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn a_cleared_replay_with_an_omission_exports_nothing() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let report: ExpectedDamagePreparedLevelReport =
		solve_late_gap(&snapshot, &prefabs, CLOUR_ROSTER);

	// The ranking demands belong to the retained plan, not to the roster.
	assert_eq!(report.solve.residual_demands.len(), 33);
	assert_eq!(report.full_battle_damage_gap.leaked_enemy_count(), 3);
	// No proposal is accepted, so the additive budget runs out.
	assert_eq!(
		report.solve.status,
		ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached
	);
	assert_eq!(
		report.verified_search_status,
		ExpectedDamageVerifiedSearchStatus::ProposalBudgetExhausted
	);
	assert_eq!(report.solve.proposals.len(), 20);
	assert_planning_only_ranks(&report);
	let replays: Vec<&ExpectedReplayAuditReport> = completed_replays(&report);
	assert_eq!(replays.len(), 20);
	for replay in replays {
		// Every replay clears the stage, and each omits 红云's SP talent, which could only have
		// helped; an omission-bearing clear still certifies nothing.
		assert_eq!(replay.terminal.as_deref(), Some("Victory"));
		assert!(replay
			.enemies
			.iter()
			.all(|enemy| enemy.leaked_tick.is_none()));
		assert_eq!(replay.status, ExpectedReplayReportStatus::Incomplete);
		assert!(replay
			.unmodeled_omissions
			.iter()
			.any(|entry| entry.mechanic_id == "char_190_clour/effect/0/1"));
	}
	assert!(report
		.battlefield_expected_victory_proposal_indices
		.is_empty());
	assert!(report.battlefield_verified_jobs.is_empty());
	assert!(report.job_materialization_failures.is_empty());
}
