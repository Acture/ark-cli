//! Full-battle damage-gap ledger coverage.
//!
//! `bd7_source_removal_produces_a_full_three_wave_gap_ledger` was deleted together with the
//! hand-written BD-7 enemy catalog it depended on.
//! `source_removal_produces_a_full_three_wave_gap_ledger` replaces it on main-story 1-1,
//! whose six enemy records all decode from pinned battle prefabs.
//! Its deleted assertions map as follows:
//!
//! - one retained build with the expected id: **restored** as the three retained reviewed builds
//!   left after removing 圣聆初雪.
//! - lineage and ledger both cover every stage enemy (36 on BD-7): **restored** as 33 on 1-1.
//! - three wave timings, each started and cleared: **restored**, and strengthened to pin every
//!   start and clear tick plus the causal rule linking them.
//! - `leaked_enemy_count() > 0` and `unresolved_damage_micros() > 0`: **restored** exactly — the
//!   leaked set is pinned and the unresolved total is the sum of its three gaps.
//! - late enemy #8 spawns at 8787, leaks, deadline 10422, positive gap: **restored** on 1-1's late
//!   enemy #30 (3774 / Leaked / 4406 / 4230 HP). The BD-7 numbers themselves are not portable:
//!   they measured a stage whose enemies were hand-modelled.
//! - count of positive gaps equals `leaked_enemy_count()`: **restored** verbatim.
//!
//! New here: the ledger is shown to be a *full-battle* view (fragments 0 and 1 are clean, so any
//! early-battlefield view sees no gap), and a second retained roster on the same stage yields a
//! different late-wave start under the same causal rule, so no static wave table can pass.
//!
//! `replay_materialization_rejects_duplicate_operator_identity` predates the catalog removal and
//! still runs on the sanitized BD-7 guide: it asserts only adapter identity rejection, which no
//! enemy catalog ever fed.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use ark_solver::data::{
	OperatorBuild, OperatorIdentity, OperatorModule, PotentialRank, StageDifficulty,
};
use ark_solver::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSeed};
use ark_solver::planning::{
	FullBattleDamageGapLedger, FullBattleEnemyDamageGap, FullBattleEnemyResolution,
};
use ark_solver::replay::{
	materialize_expected_replay_input, ExactReplayBuildBinding, ExpectedReplayAdapterError,
	ExpectedReplayCompileInput, ExpectedReplayInput,
};
use ark_solver::simulation::TickRate;
use ark_solver::workflow::{
	derive_full_battle_damage_gap_ledger, materialize_source_removed_expected_replay_input,
	prepare_guide_actions,
};

mod support;
use support::{local_battle_snapshots, maximal_no_module_binding};

const GUIDE: &str = include_str!("fixtures/maa_job_bd7_sanitized.json");
const MAIN_01_01_GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_reviewed.json");
/// The reviewed 1-1 roster, as source name and resolved operator id; every skill is S1.
const MAIN_01_01_ROSTER: [(&str, &str); 4] = [
	("讯使", "char_198_blackd"),
	("蛇屠箱", "char_150_snakek"),
	("圣聆初雪", "char_1046_sbell2"),
	("黍", "char_2025_shu"),
];
/// 1-1's one stage wave splits into three fragments whose `preDelay` is 0 s, 9 s and 17 s; at
/// 30 ticks per second a later fragment starts 270 or 510 ticks after the previous one clears.
const FRAGMENT_1_DELAY_TICKS: u64 = 9 * 30;
const FRAGMENT_2_DELAY_TICKS: u64 = 17 * 30;

fn binding_as(source_name: &str, operator_name: &str, skill: u8) -> ExactReplayBuildBinding {
	ExactReplayBuildBinding {
		source_name: source_name.to_owned(),
		build: OperatorBuild::new(
			OperatorIdentity::Name(operator_name.to_owned()),
			2,
			90,
			200,
			skill,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		),
		skill_usage: 0,
		skill_times: None,
	}
}

fn bd7_seed() -> MaaJobSeed {
	parse_maa_job(GUIDE)
		.expect("tracked BD-7 guide should parse")
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal guide selection should expand")
		.next()
		.expect("黍 + 圣聆初雪 selection should exist")
		.materialize()
}

fn main_01_01_seed() -> MaaJobSeed {
	parse_maa_job(MAIN_01_01_GUIDE)
		.expect("reviewed 1-1 guide should parse")
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal guide selection should expand")
		.next()
		.expect("the reviewed roster selection should exist")
		.materialize()
}

/// Removes one reviewed source slot from 1-1 and replays the retained roster to the full ledger.
fn main_01_01_source_removed(
	removed_source: &str,
) -> (ExpectedReplayInput, FullBattleDamageGapLedger) {
	let (snapshot, prefabs) = local_battle_snapshots();
	let bindings: Vec<ExactReplayBuildBinding> = MAIN_01_01_ROSTER
		.iter()
		.map(|(name, id)| maximal_no_module_binding(&snapshot, name, id, 1))
		.collect();
	let retained_only: ExpectedReplayInput = materialize_source_removed_expected_replay_input(
		&snapshot,
		&prefabs,
		&main_01_01_seed(),
		&bindings,
		removed_source,
		TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
	)
	.expect("source-slot removal should preserve a valid retained-only battle");
	let ledger: FullBattleDamageGapLedger = derive_full_battle_damage_gap_ledger(&retained_only)
		.expect("retained-only replay should resolve the full stage");
	(retained_only, ledger)
}

/// Every `(started, cleared)` pair, which the causal rule requires to be present.
fn wave_ticks(ledger: &FullBattleDamageGapLedger) -> Vec<(u64, u64)> {
	ledger
		.wave_timings
		.iter()
		.map(|wave| {
			(
				wave.started_tick.expect("every fragment starts"),
				wave.cleared_tick
					.expect("every fragment clears, by defeat or leak"),
			)
		})
		.collect()
}

fn assert_causal_fragment_starts(waves: &[(u64, u64)]) {
	assert_eq!(waves.len(), 3);
	assert_eq!(waves[0].0, 0, "fragment 0 opens the battle");
	assert_eq!(waves[1].0, waves[0].1 + FRAGMENT_1_DELAY_TICKS);
	assert_eq!(waves[2].0, waves[1].1 + FRAGMENT_2_DELAY_TICKS);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn source_removal_produces_a_full_three_wave_gap_ledger() {
	let (retained_only, ledger) = main_01_01_source_removed("圣聆初雪");
	// The three remaining reviewed builds, whatever order the adapter resolves them in.
	let retained_ids: BTreeSet<&str> = retained_only
		.resolved_builds
		.iter()
		.map(|build| build.operator_id())
		.collect();
	assert_eq!(
		retained_ids,
		BTreeSet::from(["char_198_blackd", "char_150_snakek", "char_2025_shu"])
	);
	// 1-1 spawns 33 enemies across its 16 spawn routes; lineage and ledger both cover all of them.
	assert_eq!(retained_only.enemy_lineage.len(), 33);
	assert_eq!(ledger.enemies.len(), 33);
	// The terminal names every enemy that reached a goal: exactly the three late leaks below.
	assert_eq!(
		ledger.terminal,
		"GoalLeak { enemies: [EnemyInstanceId(29), EnemyInstanceId(30), EnemyInstanceId(32)] }"
	);

	// Measured: fragment 0 clears at 960, so fragment 1 starts at 960 + 270 = 1230 and clears at
	// 2664; fragment 2 starts at 2664 + 510 = 3174 and clears at 5280.
	let waves: Vec<(u64, u64)> = wave_ticks(&ledger);
	assert_eq!(waves, [(0, 960), (1_230, 2_664), (3_174, 5_280)]);
	assert_causal_fragment_starts(&waves);

	// Without 圣聆初雪 the retained three still clear fragments 0 and 1 outright: an
	// early-battlefield view of this battle sees no gap at all.
	let early: Vec<&FullBattleEnemyDamageGap> = ledger
		.enemies
		.iter()
		.filter(|enemy| {
			let cohort: &str = enemy.planning_target.cohort.as_str();
			cohort.starts_with("wave:0/fragment:0/") || cohort.starts_with("wave:0/fragment:1/")
		})
		.collect();
	assert_eq!(
		early
			.iter()
			.map(|enemy| enemy.enemy_instance_id)
			.collect::<Vec<u32>>(),
		(0..=18).collect::<Vec<u32>>()
	);
	assert!(early.iter().all(
		|enemy| enemy.resolution == FullBattleEnemyResolution::Defeated
			&& enemy.required_damage_micros == 0
	));

	// Only three third-fragment enemies leak.
	let leaked: Vec<&FullBattleEnemyDamageGap> = ledger
		.enemies
		.iter()
		.filter(|enemy| enemy.resolution == FullBattleEnemyResolution::Leaked)
		.collect();
	assert_eq!(
		leaked
			.iter()
			.map(|enemy| enemy.enemy_instance_id)
			.collect::<BTreeSet<u32>>(),
		BTreeSet::from([29, 30, 32])
	);
	assert!(leaked.iter().all(|enemy| enemy
		.planning_target
		.cohort
		.as_str()
		.starts_with("wave:0/fragment:2/")));
	assert!(ledger
		.enemies
		.iter()
		.all(|enemy| enemy.resolution != FullBattleEnemyResolution::Unresolved));

	let late_enemy: &FullBattleEnemyDamageGap = ledger
		.enemies
		.iter()
		.find(|enemy| enemy.enemy_instance_id == 30)
		.expect("full ledger must retain enemy instance 30");
	assert_eq!(late_enemy.enemy_key, "enemy_1030_wteeth");
	assert_eq!(
		late_enemy.planning_target.cohort.as_str(),
		"wave:0/fragment:2/action:4"
	);
	assert_eq!(late_enemy.planning_target.ordinal, 0);
	// Spawns 600 ticks (20 s) into fragment 2: 3174 + 600 = 3774, so its timing follows the causal
	// fragment start rather than any static wave table.
	assert_eq!(late_enemy.spawned_tick, Some(3_774));
	assert_eq!(late_enemy.resolution, FullBattleEnemyResolution::Leaked);
	assert_eq!(late_enemy.failure_deadline_tick, Some(4_406));
	// Measured: it reaches the goal with 4230 HP still owed.
	assert_eq!(late_enemy.required_damage_micros, 4_230_000_000);

	assert_eq!(
		ledger
			.enemies
			.iter()
			.filter(|enemy| enemy.required_damage_micros > 0)
			.count(),
		ledger.leaked_enemy_count(),
		"the raw ledger remains per-enemy replay audit rather than planner constraints"
	);
	// #29 slime owes 1050 HP, #30 4230 HP and #32 mocock 395 HP.
	assert_eq!(
		ledger.unresolved_damage_micros(),
		1_050_000_000 + 4_230_000_000 + 395_000_000
	);

	// A different retained roster on the same stage leaks earlier (讯使's absence lets two
	// fragment-0 enemies through), clears later and so opens fragment 2 at 2790 + 510 = 3300. The
	// late-wave start is replayed, not copied from the source guide or a static table.
	let (_, without_blackd) = main_01_01_source_removed("讯使");
	let other_waves: Vec<(u64, u64)> = wave_ticks(&without_blackd);
	assert_eq!(other_waves, [(0, 1_015), (1_285, 2_790), (3_300, 5_647)]);
	assert_causal_fragment_starts(&other_waves);
	assert_ne!(other_waves[2].0, waves[2].0);
	assert_eq!(
		without_blackd
			.enemies
			.iter()
			.filter(|enemy| enemy.resolution == FullBattleEnemyResolution::Leaked)
			.map(|enemy| enemy.enemy_instance_id)
			.collect::<BTreeSet<u32>>(),
		BTreeSet::from([2, 3, 30, 31])
	);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn replay_materialization_rejects_duplicate_operator_identity() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let seed: MaaJobSeed = bd7_seed();
	let bindings = [binding_as("黍", "黍", 1), binding_as("圣聆初雪", "黍", 1)];
	let guide = prepare_guide_actions(&seed, &bindings, 8).expect("guide actions should prepare");
	let error = materialize_expected_replay_input(
		&snapshot,
		&prefabs,
		ExpectedReplayCompileInput {
			stage_name: seed.stage_name(),
			difficulty: StageDifficulty::Normal,
			bindings: &bindings,
			guide: &guide,
			tick_rate: TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
		},
	)
	.expect_err("one operator identity cannot occupy two guide slots");

	assert!(matches!(
		error,
		ExpectedReplayAdapterError::DuplicateOperatorIdentity { ref operator_id }
			if operator_id == "char_2025_shu"
	));
}
