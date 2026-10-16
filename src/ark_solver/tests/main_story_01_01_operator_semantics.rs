//! Operator-semantics replay coverage on main story 1-1.
//!
//! Deleting the hand-written BD-7 enemy catalog also deleted the three full-replay tests of
//! `bd7_sbell_baseline.rs` that ran on BD-7. Those tests pinned operator-side behaviour, not enemy
//! behaviour, so they move here onto 1-1, whose six enemy records all decode from pinned battle
//! prefabs. The reviewed 1-1 roster (讯使, 蛇屠箱, 圣聆初雪, 黍) and its guide are the base; each
//! test changes exactly one roster entry. Enemy numbers are 1-1's own, never BD-7's.
//!
//! `canonical_sbell_shu_guide_enters_full_bd7_replay_without_claiming_a_fake_win` becomes
//! [`reviewed_sbell_second_skill_keeps_its_mode_gap_and_shu_lowers_its_stored_attack`]:
//!
//! - 黍's verified conditional-heal prefab lowering to the generic stored attack (three charges, a
//!   one-half HP threshold, a refund on target death, a `verified-prefab:` reached mechanic):
//!   restored.
//! - No unsupported mechanic and no compile issue: restored.
//! - Terminal `Victory` yet status `Incomplete`, because 圣聆初雪's second-skill mode gap blocks
//!   certification; the gap names snow-spread source ordering and no longer asks for a five-layer
//!   cell transition: restored.
//! - 圣聆初雪's second skill activating automatically at tick 2310: restored. The tick equals the
//!   deleted BD-7 pin because it does not depend on the stage: she deploys at 960 on both, and the
//!   skill charges from 0 SP at one per second to its cost of 45, 1350 ticks later. The test now
//!   derives it from her executed deployment instead of pinning it alone.
//! - The two BD-7 crossbowmen reporting `/normal-attack/range` omissions: not portable. That was
//!   BD-7 enemy data (`enemy_1403_gunsho`, `enemy_1404_msnip` with `rangeRadius -1.0`), not an
//!   operator property; replay now records the reach a ranged record never established for any
//!   stage, and `main_story_01_01_baseline.rs` pins that 1-1's one ranged record decodes.
//!
//! `unbound_fastshot_replays_base_attacks_without_operator_admission_gate` keeps its name:
//!
//! - Operator identity is no admission gate (no unsupported entry), 红云 executes her deployment,
//!   her SP talent `char_190_clour/effect/0/1` is an unmodelled omission, and no omission id
//!   mentions `catalog_shape`: restored. 红云's charpack is absent from the prefab snapshot at
//!   this revision, so today this also exercises admission without a pinned operator prefab; nothing
//!   here asserts that absence.
//! - Terminal `GoalLeak` with status `Incomplete`: not portable as a terminal. On 1-1 the base
//!   attacks clear the stage, so the terminal is `Victory` and only the clear branch — an omitted
//!   benefit withholds `ExpectedVictory` — is exercised here. The leak branch, an omitted benefit
//!   withholding `ExpectedDefeat`, is covered synthetically by
//!   `replay_expected.rs::unmodeled_omission_withholds_modeled_goal_leak_defeat`.
//! - 36 enemies: BD-7's count; 1-1 spawns 33, pinned instead.
//!
//! `custom_potential_is_not_reported_twice_beside_its_selected_talent_candidate` keeps its name
//! and is restored with 阿米娅 in 圣聆初雪's place: her talent candidate is reported, and her P6
//! `CUSTOM` potential is absorbed by it rather than reported again. An unabsorbed `CUSTOM` potential
//! surfaces as an unsupported `{operator}/potential/p{n}` entry, never as an unmodelled omission, so
//! the guard is the empty unsupported list; the deleted test's check that no unmodelled omission id
//! contains `/potential/` could not fail and is replaced by a check on the unsupported ids.
//!
//! Declared approximations (projectile flight, spawn scatter) may be exercised in any of these
//! replays; they are disclosed beside the verdict and never withhold it, so nothing
//! here asserts their absence.

use std::num::NonZeroU32;

use ark_solver::data::{ArkDataSnapshot, BattlePrefabSnapshot, PotentialRank, StageDifficulty};
use ark_solver::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSeed};
use ark_solver::mechanics::domain::{ExpectedValue, StoredAttackReservationInvalidationAction};
use ark_solver::replay::{
	execute_expected_replay, materialize_expected_replay_input, ExactReplayBuildBinding,
	ExpectedReplayCompileInput, ExpectedReplayInput, ExpectedReplayOutput,
	ExpectedReplayReportStatus, ReplaySkillActivationSource,
};
use ark_solver::simulation::TickRate;
use ark_solver::workflow::prepare_guide_actions;

mod support;
use support::{local_battle_snapshots, maximal_no_module_binding};

const REVIEWED_GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_reviewed.json");
/// The reviewed guide with only 圣聆初雪's `opers` skill changed from 1 to 2.
const SBELL_S2_GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_sbell_s2.json");
const STAGE_ID: &str = "main_01-01";
const MAP_HEIGHT: u32 = 7;
const SBELL_NAME: &str = "圣聆初雪";
const SBELL_OPERATOR_ID: &str = "char_1046_sbell2";
const SHU_OPERATOR_ID: &str = "char_2025_shu";
/// 1-1's 16 spawn routes carry 33 enemy instances across its three waves.
const STAGE_ENEMY_COUNT: usize = 33;

/// The reviewed roster, as source name and resolved operator id; every entry runs its first skill.
const REVIEWED_ROSTER: [(&str, &str); 4] = [
	("讯使", "char_198_blackd"),
	("蛇屠箱", "char_150_snakek"),
	(SBELL_NAME, SBELL_OPERATOR_ID),
	("黍", SHU_OPERATOR_ID),
];

/// The reviewed roster with 圣聆初雪 replaced by `(source name, operator id, skill)`.
fn bindings_with_sbell_slot(
	snapshot: &ArkDataSnapshot,
	slot: (&str, &str, u8),
) -> Vec<ExactReplayBuildBinding> {
	REVIEWED_ROSTER
		.iter()
		.map(|&(source_name, operator_id)| {
			if source_name == SBELL_NAME {
				maximal_no_module_binding(snapshot, slot.0, slot.1, slot.2)
			} else {
				maximal_no_module_binding(snapshot, source_name, operator_id, 1)
			}
		})
		.collect()
}

fn replay_guide(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	guide_json: &str,
	bindings: &[ExactReplayBuildBinding],
	seed: [u8; 32],
	variant_id: &str,
) -> (ExpectedReplayInput, ExpectedReplayOutput) {
	let job = parse_maa_job(guide_json).expect("1-1 guide variant should parse");
	let job_seed: MaaJobSeed = job
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal selection should expand")
		.next()
		.expect("the guide declares exactly one executable selection")
		.materialize();
	assert_eq!(job_seed.stage_name(), STAGE_ID);
	let guide = prepare_guide_actions(&job_seed, bindings, MAP_HEIGHT)
		.expect("1-1 guide actions should prepare");
	let input: ExpectedReplayInput = materialize_expected_replay_input(
		snapshot,
		prefabs,
		ExpectedReplayCompileInput {
			stage_name: job_seed.stage_name(),
			difficulty: StageDifficulty::Normal,
			bindings,
			guide: &guide,
			tick_rate: TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
		},
	)
	.expect("the pinned main-story stage should materialize");
	let output: ExpectedReplayOutput = execute_expected_replay(&input, seed, variant_id);
	(input, output)
}

fn unmodeled_ids(output: &ExpectedReplayOutput) -> Vec<&str> {
	output
		.report
		.unmodeled_omissions
		.iter()
		.map(|entry| entry.mechanic_id.as_str())
		.collect()
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn reviewed_sbell_second_skill_keeps_its_mode_gap_and_shu_lowers_its_stored_attack() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let bindings: Vec<ExactReplayBuildBinding> =
		bindings_with_sbell_slot(&snapshot, (SBELL_NAME, SBELL_OPERATOR_ID, 2));
	let (input, output) = replay_guide(
		&snapshot,
		&prefabs,
		SBELL_S2_GUIDE,
		&bindings,
		[0_u8; 32],
		"main-01-01-sbell-s2-no-module",
	);

	// 黍's first skill is the verified conditional-heal prefab, lowered through the generic stored
	// attack: three reserved charges, released below half HP, spent SP refunded when the reserved
	// target dies first.
	let shu = input
		.guide
		.builds
		.iter()
		.find(|build| build.operator_key == SHU_OPERATOR_ID)
		.expect("reviewed guide contains 黍");
	let replacement = shu
		.skill
		.as_ref()
		.and_then(|skill| skill.stored_attack_replacement.as_ref())
		.expect("verified conditional-heal prefab must lower through the generic stored attack");
	assert_eq!(replacement.max_charges, 3);
	assert_eq!(
		replacement.hp_ratio_threshold,
		ExpectedValue::from_ratio(1, 2).expect("valid ratio")
	);
	assert_eq!(
		replacement.invalidation.target_death,
		StoredAttackReservationInvalidationAction::RefundSpentSp
	);
	assert!(shu.reached_mechanics.iter().any(|mechanic| {
		mechanic
			.id
			.as_str()
			.ends_with("/template/conditional-heal-attack")
			&& mechanic.origin.starts_with("verified-prefab:")
	}));

	assert!(
		output.report.unsupported.is_empty(),
		"the reviewed roster on 1-1 must reach simulation: {:#?}",
		output.report.unsupported
	);
	assert!(
		output.report.compile_issues.is_empty(),
		"the reviewed roster on 1-1 must compile: {:#?}",
		output.report.compile_issues
	);
	// The roster clears 1-1, but the clear is only as sound as its inventory.
	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert_ne!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedVictory,
		"the remaining 圣聆初雪 mode gap must prevent victory certification"
	);
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
	let remaining_mode = output
		.report
		.unmodeled_omissions
		.iter()
		.find(|entry| {
			entry.mechanic_id == "char_1046_sbell2/skill/skchr_sbell2_2/remaining-mode-mechanics"
		})
		.unwrap_or_else(|| {
			panic!(
				"圣聆初雪's mode policy gap must stay visible: {:?}",
				unmodeled_ids(&output)
			)
		});
	assert!(remaining_mode
		.reasons
		.iter()
		.any(|reason| reason.contains("snow-spread source ordering")));
	assert!(!remaining_mode
		.reasons
		.iter()
		.any(|reason| reason.contains("still needs: a five-layer cell transition")));
	// Under the skill daemon her second skill fires on its own once charged: 0 initial SP, one per
	// second, a cost of 45, so 1350 ticks after her deployment at 960.
	let deployed_at: u64 = output
		.report
		.actions
		.iter()
		.find(|action| action.operator_id.as_deref() == Some(SBELL_OPERATOR_ID))
		.and_then(|action| action.executed_tick)
		.expect("圣聆初雪 deploys");
	assert_eq!(deployed_at, 960);
	let activation = output
		.report
		.skill_activations
		.iter()
		.find(|activation| activation.operator_id == SBELL_OPERATOR_ID)
		.expect("圣聆初雪's second skill should activate automatically after charging");
	assert_eq!(activation.source, ReplaySkillActivationSource::Automatic);
	assert_eq!(activation.tick, deployed_at + 45 * 30);
	assert_eq!(activation.tick, 2_310);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn unbound_fastshot_replays_base_attacks_without_operator_admission_gate() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let guide_json: String = REVIEWED_GUIDE.replace(SBELL_NAME, "红云");
	let bindings: Vec<ExactReplayBuildBinding> =
		bindings_with_sbell_slot(&snapshot, ("红云", "char_190_clour", 2));
	let (_, output) = replay_guide(
		&snapshot,
		&prefabs,
		&guide_json,
		&bindings,
		[1_u8; 32],
		"main-01-01-clour-no-module",
	);

	assert!(
		output.report.unsupported.is_empty(),
		"operator identity must not be a replay admission gate: {:#?}",
		output.report.unsupported
	);
	// 红云's base attacks clear 1-1, but her SP-regeneration talent is omitted; had the roster
	// leaked instead, that same omitted benefit could have held the leak off. Neither verdict
	// certifies.
	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
	assert_eq!(output.report.enemies.len(), STAGE_ENEMY_COUNT);
	assert!(output.report.actions.iter().any(|action| {
		action.operator_id.as_deref() == Some("char_190_clour")
			&& action.status.starts_with("executed")
	}));
	let unmodeled: Vec<&str> = unmodeled_ids(&output);
	assert!(
		unmodeled.contains(&"char_190_clour/effect/0/1"),
		"{unmodeled:?}"
	);
	assert!(
		unmodeled.iter().all(|id| !id.contains("catalog_shape")),
		"{unmodeled:?}"
	);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn custom_potential_is_not_reported_twice_beside_its_selected_talent_candidate() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let guide_json: String = REVIEWED_GUIDE.replace(SBELL_NAME, "阿米娅");
	let bindings: Vec<ExactReplayBuildBinding> =
		bindings_with_sbell_slot(&snapshot, ("阿米娅", "char_002_amiya", 2));
	let (_, output) = replay_guide(
		&snapshot,
		&prefabs,
		&guide_json,
		&bindings,
		[2_u8; 32],
		"main-01-01-amiya-no-module",
	);

	// The maximal build is P6, whose `CUSTOM` potential enhances the talent; without that rank the
	// absorption path would not be reached at all.
	assert_eq!(bindings[2].build.potential(), PotentialRank::P6);
	// 阿米娅's talent is reported once, as its selected candidate; the potential that customises
	// it is absorbed by that candidate instead of surfacing as an unsupported entry beside it.
	assert!(
		output
			.report
			.unsupported
			.iter()
			.all(|entry| !entry.mechanic_id.contains("/potential/")),
		"{:#?}",
		output.report.unsupported
	);
	assert!(
		output.report.unsupported.is_empty(),
		"{:#?}",
		output.report.unsupported
	);
	let unmodeled: Vec<&str> = unmodeled_ids(&output);
	assert!(
		unmodeled.contains(&"char_002_amiya/effect/0/1"),
		"{unmodeled:?}"
	);
}
