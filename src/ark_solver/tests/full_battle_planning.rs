//! Full-battle unblocked planning context on main-story 1-1.
//!
//! Seven BD-7 integration tests were deleted together with the hand-written BD-7 enemy catalog,
//! whose enemies are now Unsupported. Their coverage is restored on 1-1, whose six enemy records
//! all decode from pinned battle prefabs. No BD-7 number is ported; every pinned number is 1-1's
//! own.
//!
//! | Deleted BD-7 test | Replacement |
//! | --- | --- |
//! | `full_battle_planning::bd7_full_battle_context_binds_all_fragments_and_enemies` | this file: `full_battle_context_binds_all_fragments_and_enemies` |
//! | `full_battle_gap::bd7_source_removal_produces_a_full_three_wave_gap_ledger` | `full_battle_gap.rs`: `source_removal_produces_a_full_three_wave_gap_ledger` |
//! | `maa_job_bd7_fixture::bd7_expected_damage_solver_uses_the_full_battle_gap_instead_of_local_evidence` | `main_story_01_01_late_gap_solve.rs`: `solver_ranks_on_the_full_battle_gap_not_local_evidence` |
//! | `maa_job_bd7_fixture::bd7_workflow_owned_domain_cannot_hide_a_late_wave_damage_gap` | `main_story_01_01_late_gap_solve.rs`: `a_clean_early_battlefield_cannot_hide_the_late_wave_gap` |
//! | `bd7_sbell_baseline::canonical_sbell_shu_guide_enters_full_bd7_replay_without_claiming_a_fake_win` | `main_story_01_01_operator_semantics.rs`: `reviewed_sbell_second_skill_keeps_its_mode_gap_and_shu_lowers_its_stored_attack` |
//! | `bd7_sbell_baseline::unbound_fastshot_replays_base_attacks_without_operator_admission_gate` | `main_story_01_01_operator_semantics.rs`: `unbound_fastshot_replays_base_attacks_without_operator_admission_gate` |
//! | `bd7_sbell_baseline::custom_potential_is_not_reported_twice_beside_its_selected_talent_candidate` | `main_story_01_01_operator_semantics.rs`: `custom_potential_is_not_reported_twice_beside_its_selected_talent_candidate` |
//!
//! Each replacement file's own module doc maps the deleted assertions one by one, including those
//! that are not portable. One claim is not restored: a solver proposal that completes replay and is
//! refuted by the late gap. No reviewed candidate at this revision is insufficient for 1-1's third
//! wave, so `main_story_01_01_late_gap_solve.rs` pins the accepting side and the export refusal of
//! an omission-bearing clear instead; the missing case remains open.
//!
//! Assertions of the deleted planning test, all **restored** here on the ledger that removes
//! 圣聆初雪 from the reviewed roster:
//!
//! - bound anchors are exactly `WaveFragmentStart { wave: 0, fragment: 0..=2 }`;
//! - three causal waves, fragments 1 and 2 anchored on the previous fragment's clear with no finite
//!   latest start (offsets 270 and 510, 1-1's own `preDelay`; BD-7's 360 / 1200 are not portable);
//! - the retained witness equals the ledger's own wave timing (now pinned for all three waves);
//! - every model enemy (33, BD-7 had 36) has nonempty occupancy, and the template matches it;
//! - the duty window ends at the later of the maximum goal deadline and the maximum gap deadline;
//! - full-HP groups are strictly nested by deadline, keyed by that deadline, one per distinct goal
//!   deadline, and the last covers every stage target with the total stage HP;
//! - the late enemy (BD-7 #8 at 8787, here #30 at 3774) binds into the model at its ledger spawn;
//! - joint scoring targets every cohort, and the final group includes a retained-defeated enemy.
//!
//! New here: the planner's optimistic causal start bound for fragment 2 differs from the retained
//! witness, and every fragment has wave-local HP groups, with the three leaked enemies in
//! fragment 2's.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use ark_solver::data::{CompiledStage, StageDifficulty, StageIdentity, StageRef};
use ark_solver::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSeed};
use ark_solver::mechanics::domain::{BattleTimeAnchor, FixedDamage, WaveId};
use ark_solver::mechanics::stage::SpawnInstanceId;
use ark_solver::planning::{
	prepare_full_battle_unblocked_planning_context, EventTickBounds, FullBattleDamageGapLedger,
	FullBattleEnemyDamageGap, FullBattleEnemyResolution, FullBattlePlanningContext,
	StageTimeAnchor,
};
use ark_solver::replay::{ExactReplayBuildBinding, ExpectedReplayInput};
use ark_solver::simulation::TickRate;
use ark_solver::workflow::{
	derive_full_battle_damage_gap_ledger, materialize_source_removed_expected_replay_input,
};

mod support;
use support::{local_battle_snapshots, maximal_no_module_binding};

const GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_reviewed.json");
const STAGE_ID: &str = "main_01-01";
/// The reviewed 1-1 roster, as source name and resolved operator id; every skill is S1.
const ROSTER: [(&str, &str); 4] = [
	("讯使", "char_198_blackd"),
	("蛇屠箱", "char_150_snakek"),
	("圣聆初雪", "char_1046_sbell2"),
	("黍", "char_2025_shu"),
];

fn fragment_anchor(fragment: u32) -> StageTimeAnchor {
	StageTimeAnchor::WaveFragmentStart { wave: 0, fragment }
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn full_battle_context_binds_all_fragments_and_enemies() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let seed: MaaJobSeed = parse_maa_job(GUIDE)
		.expect("reviewed 1-1 guide should parse")
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal guide selection should expand")
		.next()
		.expect("the reviewed roster selection should exist")
		.materialize();
	assert_eq!(seed.stage_name(), STAGE_ID);
	let stage: CompiledStage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(STAGE_ID.to_owned()),
			difficulty: StageDifficulty::Normal,
		})
		.expect("1-1 should compile");
	let tick_rate: TickRate = TickRate::new(NonZeroU32::new(30).expect("30 is nonzero"));
	let bindings: Vec<ExactReplayBuildBinding> = ROSTER
		.iter()
		.map(|(name, id)| maximal_no_module_binding(&snapshot, name, id, 1))
		.collect();
	let retained_only: ExpectedReplayInput = materialize_source_removed_expected_replay_input(
		&snapshot,
		&prefabs,
		&seed,
		&bindings,
		"圣聆初雪",
		tick_rate,
	)
	.expect("source removal should materialize");
	let ledger: FullBattleDamageGapLedger = derive_full_battle_damage_gap_ledger(&retained_only)
		.expect("retained-only replay should resolve every 1-1 enemy");
	let context: FullBattlePlanningContext =
		prepare_full_battle_unblocked_planning_context(&stage, tick_rate, &ledger)
			.expect("the full 1-1 ledger should bind the complete stage model");

	// 1-1 has one stage wave of three fragments, and the model binds each fragment start at the
	// retained replay's tick: 0, 960 + 270 = 1230 and 2664 + 510 = 3174.
	let fragment_anchors: BTreeSet<u32> = context
		.model
		.bound_anchors
		.iter()
		.map(|binding| match binding.anchor {
			StageTimeAnchor::WaveFragmentStart { wave: 0, fragment } => fragment,
			ref anchor => panic!("unexpected full-stage anchor {anchor:?}"),
		})
		.collect();
	assert_eq!(fragment_anchors, BTreeSet::from([0, 1, 2]));
	let bound_ticks: Vec<u64> = context.model.bound_anchors.iter().map(|b| b.tick).collect();
	assert_eq!(bound_ticks, [0, 1_230, 3_174]);

	// Fragment 0 opens the battle. Fragments 1 and 2 start `preDelay` after the previous fragment
	// clears: 9 s and 17 s at 30 ticks per second, with no finite latest start because blocking a
	// predecessor can delay the clear without bound.
	assert_eq!(context.causal_waves.len(), 3);
	assert_eq!(
		context.causal_waves[0].start_bounds,
		EventTickBounds {
			earliest: 0,
			latest: Some(0)
		}
	);
	for (index, previous, offset) in [(1_usize, 0_u32, 270_u64), (2, 1, 510)] {
		let wave = &context.causal_waves[index];
		assert_eq!(wave.stage_anchor, fragment_anchor(index as u32));
		assert_eq!(
			wave.start.earliest.anchor,
			BattleTimeAnchor::WaveCleared(WaveId::new(previous))
		);
		assert_eq!(wave.start.earliest.offset_ticks, offset);
		assert_eq!(wave.start_bounds.latest, None);
	}
	for (wave, timing) in context.causal_waves.iter().zip(&ledger.wave_timings) {
		assert_eq!(
			Some(wave.retained_witness.started_tick),
			timing.started_tick
		);
		assert_eq!(wave.retained_witness.cleared_tick, timing.cleared_tick);
	}
	// The optimistic earliest bound chains each predecessor's earliest start, its template
	// optimistic clear offset (measured: 810 ticks for fragment 0, 1200 for fragment 1) and the
	// `preDelay`: fragment 1 may start at 0 + 810 + 270 = 1080 and fragment 2 at
	// 1080 + 1200 + 510 = 2790, while the retained witness saw 1230 and 3174. The planner keeps
	// the two apart: a witness tick is never a later-wave constraint.
	assert_eq!(context.causal_waves[1].start_bounds.earliest, 1_080);
	assert_eq!(context.causal_waves[2].start_bounds.earliest, 2_790);
	assert_eq!(context.causal_waves[2].retained_witness.started_tick, 3_174);
	assert_ne!(
		context.causal_waves[2].start_bounds.earliest,
		context.causal_waves[2].retained_witness.started_tick
	);

	// 33 spawns across 16 cohorts, each with a nonempty unblocked route occupancy.
	assert_eq!(context.model.enemies.len(), 33);
	assert_eq!(context.model.cohorts.len(), 16);
	assert!(context
		.model
		.enemies
		.iter()
		.all(|enemy| !enemy.occupancy.is_empty()));
	let maximum_goal_deadline: u64 = context
		.model
		.enemies
		.iter()
		.map(|enemy| enemy.goal_deadline_tick)
		.max()
		.expect("1-1 has enemies");
	let maximum_gap_deadline: u64 = ledger
		.enemies
		.iter()
		.filter(|enemy| enemy.required_damage_micros > 0)
		.filter_map(|enemy| enemy.failure_deadline_tick)
		.max()
		.expect("source removal must leave at least one positive damage gap");
	// The last unblocked arrival (`wave:0/fragment:2/action:4` ordinal 1, the fragment's last
	// wteeth, 5126) outlasts the last retained leak (the mocock at 4519).
	assert_eq!(maximum_goal_deadline, 5_126);
	assert_eq!(maximum_gap_deadline, 4_519);
	assert_eq!(
		context.duty_window.end,
		maximum_goal_deadline.max(maximum_gap_deadline)
	);
	assert!(!context.hp_groups.is_empty());
	assert!(!context.wave_local_hp_groups.is_empty());
	assert_eq!(context.template.enemies.len(), context.model.enemies.len());
	assert!(context.hp_groups.windows(2).all(|pair| {
		pair[0].deadline_tick < pair[1].deadline_tick
			&& pair[0].members.is_subset(&pair[1].members)
			&& pair[0].required_hp < pair[1].required_hp
	}));
	assert!(context
		.hp_groups
		.iter()
		.all(|group| group.id.deadline_tick() == group.deadline_tick));

	// Late enemy #30 is ordinal 0 of `wave:0/fragment:2/action:4` (the fragment's second wteeth to
	// spawn; action 1's spawns earlier): 3174 + its 20 s `preDelay` = 3774, reaching the goal
	// unblocked at 4406, exactly where the retained replay leaked it.
	let replay_enemy_30: &FullBattleEnemyDamageGap = ledger
		.enemies
		.iter()
		.find(|enemy| enemy.enemy_instance_id == 30)
		.expect("ledger should retain enemy instance 30");
	let unblocked_enemy_30 = context
		.model
		.enemies
		.iter()
		.find(|enemy| enemy.instance_id == replay_enemy_30.planning_target)
		.expect("enemy instance 30 should bind into the unblocked model");
	assert_eq!(
		unblocked_enemy_30.instance_id.cohort.as_str(),
		"wave:0/fragment:2/action:4"
	);
	assert_eq!(unblocked_enemy_30.spawn_tick, 3_774);
	assert_eq!(replay_enemy_30.spawned_tick, Some(3_774));
	assert_eq!(unblocked_enemy_30.goal_deadline_tick, 4_406);
	assert_eq!(
		context.target_cohort_ids.len(),
		context.model.cohorts.len(),
		"joint scoring must cover the complete stage, not only cohorts with a direct gap"
	);

	let final_group = context
		.hp_groups
		.last()
		.expect("positive replay gaps must enable full-stage HP groups");
	let all_stage_targets: BTreeSet<SpawnInstanceId> = context
		.model
		.enemies
		.iter()
		.map(|enemy| enemy.instance_id.clone())
		.collect();
	let total_stage_hp: FixedDamage = context
		.model
		.enemies
		.iter()
		.try_fold(FixedDamage::ZERO, |total, enemy| {
			total.checked_add(FixedDamage::from_whole(enemy.max_hp)?)
		})
		.expect("total stage HP fits");
	let distinct_goal_deadlines: BTreeSet<u64> = context
		.model
		.enemies
		.iter()
		.map(|enemy| enemy.goal_deadline_tick)
		.collect();
	assert_eq!(context.hp_groups.len(), distinct_goal_deadlines.len());
	assert_eq!(final_group.deadline_tick, maximum_goal_deadline);
	assert_eq!(final_group.members, all_stage_targets);
	assert_eq!(final_group.members.len(), context.model.enemies.len());
	assert_eq!(final_group.required_hp, total_stage_hp);
	let defeated_target: SpawnInstanceId = ledger
		.enemies
		.iter()
		.find(|enemy| enemy.resolution == FullBattleEnemyResolution::Defeated)
		.map(|enemy| enemy.planning_target.clone())
		.expect("retained-only replay must defeat at least one enemy");
	assert!(
		final_group.members.contains(&defeated_target),
		"full-HP groups must include enemies defeated by the retained-only replay"
	);

	// Wave-local cuts exist under every fragment anchor, and all three leaks (#29, #30, #32) are
	// owed inside fragment 2's own cuts, independent of when that fragment starts.
	let wave_local_anchors: BTreeSet<StageTimeAnchor> = context
		.wave_local_hp_groups
		.iter()
		.map(|group| group.id.anchor.clone())
		.collect();
	assert_eq!(
		wave_local_anchors,
		(0..=2)
			.map(fragment_anchor)
			.collect::<BTreeSet<StageTimeAnchor>>()
	);
	let leaked_targets: Vec<&SpawnInstanceId> = ledger
		.enemies
		.iter()
		.filter(|enemy| enemy.resolution == FullBattleEnemyResolution::Leaked)
		.map(|enemy| &enemy.planning_target)
		.collect();
	assert_eq!(leaked_targets.len(), 3);
	assert!(leaked_targets.iter().all(|target| context
		.wave_local_hp_groups
		.iter()
		.any(|group| group.id.anchor == fragment_anchor(2) && group.members.contains(target))));

	// Each fragment's last wave-local cut owes that fragment's whole HP, and the three sum to the
	// stage's: 5040 + 17550 + 27050 = 49640. A late cut emptied of HP would leave the third wave
	// owing nothing while keeping its anchor and members.
	let terminal_wave_local_hp: Vec<(u32, u64, FixedDamage)> = (0..=2)
		.map(|fragment: u32| {
			let group = context
				.wave_local_hp_groups
				.iter()
				.filter(|group| group.id.anchor == fragment_anchor(fragment))
				.max_by_key(|group| group.id.goal_deadline_offset_ticks)
				.expect("every fragment has a wave-local cut");
			(
				fragment,
				group.id.goal_deadline_offset_ticks,
				group.required_hp,
			)
		})
		.collect();
	assert_eq!(
		terminal_wave_local_hp,
		vec![
			(0, 1_015, FixedDamage::from_whole(5_040).expect("fits")),
			(1, 1_680, FixedDamage::from_whole(17_550).expect("fits")),
			(2, 1_952, FixedDamage::from_whole(27_050).expect("fits")),
		]
	);
	let terminal_sum: FixedDamage = terminal_wave_local_hp
		.iter()
		.try_fold(FixedDamage::ZERO, |total, (_, _, required_hp)| {
			total.checked_add(*required_hp)
		})
		.expect("terminal HP fits");
	assert_eq!(terminal_sum, total_stage_hp);
}
