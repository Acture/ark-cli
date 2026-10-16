//! Ordinary main-story offline replay baseline for 1-1.
//!
//! 1-1 is the first ordinary main-story stage. None of its six enemy records describes behaviour of
//! its own in the pinned tables, its only non-spawn wave actions are presentation, and its 16 spawn
//! routes reconstruct into two walk paths that cross the melee tiles MAA `[4, 3]` and `[5, 3]`
//! before splitting to their two goals (three further routes are `E_NUM` stubs belonging to the
//! presentation actions, and three of the 16 pause five seconds at a checkpoint):
//!
//! ```text
//! left     (0,3) (1,3) (2,3) (3,3) (4,3) (5,3) (6,3) (7,3) (8,3 goal)
//! vertical (4,0) (4,1) (4,2) (4,3) (5,3) (5,4) (5,5) (5,6 goal)
//! ```
//!
//! The whole offline path therefore runs end to end here: snapshot loading, factual compilation,
//! MAA job decoding and complete battlefield replay. The positive case clears every spawn, the
//! control leaks, and the isolation probe reduces the inventory to the stage's own remaining gap.
//!
//! Every enemy record compiles from its pinned battle prefab through the Plain Enemy Prefab
//! Template, so the stage itself leaves nothing unmodelled: the isolation probe's inventory is
//! empty, and its leak is the certified defeat.
//!
//! Two rosters run the same stage. The **reviewed roster** is the acceptance gate: every mechanic
//! its four operators reach is decoded, so its clear is `ExpectedVictory` and the one-field control
//! that loses the stage is `ExpectedDefeat`. The original **authored roster** keeps 白金 and
//! 杜宾, whose talents and one skill still have no reviewed semantics; it is retained because it
//! pins what those omissions are and how 讯使's deployment-point grant and block-count talent
//! behave, but its clear and its leak stay `Incomplete` and certify nothing.
//!
//! Enemy movement here is the attribute scaled by the stage's own `options.moveMultiplier`, which
//! is 0.5 on every level document at this revision, so an enemy's tiles per second is half its
//! `moveSpeed`. Every arrival tick below is derived at that speed.

use std::num::NonZeroU32;

use ark_solver::data::{
	ArkDataSnapshot, BattlePrefabSnapshot, OperatorIdentity, OperatorModule, PotentialRank,
	StageDifficulty,
};
use ark_solver::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSeed};
use ark_solver::mechanics::compile::{
	built_in_battle_prefab_handler_registry, interpret_battle_prefab_asset,
	try_decode_charge_cost_skill_template,
};
use ark_solver::mechanics::domain::{AttackPattern, ChargeCostGrantCadence, MechanicBehavior};
use ark_solver::mechanics::prefab::BattlePrefabRootSection;
use ark_solver::replay::{
	execute_expected_replay, materialize_expected_replay_input, ExactReplayBuildBinding,
	ExpectedReplayCompileInput, ExpectedReplayInput, ExpectedReplayOutput,
	ExpectedReplayReportStatus, ReplayBlockerKind,
};
use ark_solver::simulation::{SimTick, TickRate};
use ark_solver::workflow::prepare_guide_actions;

mod support;
use support::{local_battle_snapshots, maximal_no_module_binding};

const GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_authored.json");
const REVIEWED_GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_reviewed.json");
const ISOLATION_GUIDE: &str = include_str!("fixtures/maa_job_main_01_01_isolation.json");
const STAGE_ID: &str = "main_01-01";
/// 1-1's `options.maxCost`; a grant landing on a full pool credits less than its amount.
const STAGE_MAX_DP: u32 = 99;
const MAP_HEIGHT: u32 = 7;

/// The authored roster, as source name and resolved operator id.
const ROSTER: [(&str, &str); 4] = [
	("讯使", "char_198_blackd"),
	("蛇屠箱", "char_150_snakek"),
	("白金", "char_204_platnm"),
	("杜宾", "char_130_doberm"),
];

/// The reviewed roster, as source name and resolved operator id. These are the only four operators
/// at this revision whose whole reached inventory — archetype normal attack, every talent and the
/// selected skill — is claimed by a reviewed template, so they are the entire pool the gate can
/// draw a winning roster from. 圣聆初雪 replaces 白金 on the same ranged tile and 黍 replaces 杜宾
/// on the same melee tile.
const REVIEWED_ROSTER: [(&str, &str); 4] = [
	("讯使", "char_198_blackd"),
	("蛇屠箱", "char_150_snakek"),
	("圣聆初雪", "char_1046_sbell2"),
	("黍", "char_2025_shu"),
];

/// Every reviewed-roster build the adapter must resolve, as
/// `(operator id, elite, level, trust percent, skill, skill level, potential)`. The gate names the
/// builds it certifies rather than inheriting whatever the snapshot happens to resolve.
const REVIEWED_BUILDS: [(&str, u8, u32, u8, u8, u8, PotentialRank); 4] = [
	("char_198_blackd", 2, 70, 200, 1, 10, PotentialRank::P6),
	("char_150_snakek", 2, 70, 200, 1, 10, PotentialRank::P6),
	("char_1046_sbell2", 2, 90, 200, 1, 10, PotentialRank::P6),
	("char_2025_shu", 2, 90, 200, 1, 10, PotentialRank::P6),
];

/// The stage's six enemy records, each compiled from its verified battle prefab.
const ENEMY_KEYS: [&str; 6] = [
	"enemy_1000_gopro",
	"enemy_1000_gopro_2",
	"enemy_1007_slime_2",
	"enemy_1027_mob",
	"enemy_1028_mocock",
	"enemy_1030_wteeth",
];

/// The stage's one ranged record, whose projectile lands the tick after it fires.
const RANGED_ENEMY_KEY: &str = "enemy_1028_mocock";

/// Operator-side mechanics that stay unmodelled because no reviewed semantics bind them.
///
/// These are not an ingestion gap. Every roster charpack is pinned and the talent blackboards are
/// read: 白金's talent root sits directly under its prefab root and is read too, and 杜宾's skill
/// resolves to a prefab — what is missing is a reviewed template for each talent effect and a
/// reviewed damage binding for that skill. 杜宾 authors his talent inside each skill mode, which the
/// talent lookup does not read. 黍 has no entry here because her archetype, talents and selected
/// skill are all reviewed, and 讯使 has none because the block-count toggle template binds her
/// talent and the instant deployment-point template her skill.
const UNREVIEWED_OPERATOR_SEMANTICS: [&str; 4] = [
	// 杜宾's instructor trait raises damage against a target it does not itself block, and its
	// talent buffs every deployed three-star operator; both need a predicate the model lacks.
	"char_130_doberm/effect/0/no-prefab",
	"char_130_doberm/effect/1/1",
	// A charged next-attack multiplier.
	"char_130_doberm/skill/skchr_doberm_1",
	// Attack scales with the delay since the previous attack.
	"char_204_platnm/effect/0/1",
];

fn roster_bindings(snapshot: &ArkDataSnapshot) -> Vec<ExactReplayBuildBinding> {
	ROSTER
		.iter()
		.map(|(source_name, operator_id)| {
			maximal_no_module_binding(snapshot, source_name, operator_id, 1)
		})
		.collect()
}

fn reviewed_roster_bindings(snapshot: &ArkDataSnapshot) -> Vec<ExactReplayBuildBinding> {
	REVIEWED_ROSTER
		.iter()
		.map(|(source_name, operator_id)| {
			maximal_no_module_binding(snapshot, source_name, operator_id, 1)
		})
		.collect()
}

/// The certified builds are exactly the ones named here, module-free, in roster order.
fn assert_the_certified_builds_resolved(bindings: &[ExactReplayBuildBinding]) {
	let resolved: Vec<(&str, u8, u32, u8, u8, u8, PotentialRank)> = bindings
		.iter()
		.map(|binding| {
			let OperatorIdentity::Id(operator_id) = binding.build.identity() else {
				panic!("{:?}", binding.build.identity());
			};
			assert_eq!(binding.build.module(), &OperatorModule::None);
			assert_eq!(binding.build.module_level(), 0);
			(
				operator_id.as_str(),
				binding.build.elite(),
				binding.build.level(),
				binding.build.trust_percent(),
				binding.build.skill(),
				binding.build.skill_level(),
				binding.build.potential(),
			)
		})
		.collect();
	assert_eq!(resolved, REVIEWED_BUILDS);
}

fn replay_guide(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	guide_json: &str,
	bindings: &[ExactReplayBuildBinding],
	variant_id: &str,
) -> (ExpectedReplayInput, ExpectedReplayOutput) {
	let job = parse_maa_job(guide_json).expect("authored baseline job should parse");
	let seed: MaaJobSeed = job
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal selection should expand")
		.next()
		.expect("the authored job declares exactly one executable selection")
		.materialize();
	assert_eq!(seed.stage_name(), STAGE_ID);
	let guide = prepare_guide_actions(&seed, bindings, MAP_HEIGHT)
		.expect("authored baseline actions should prepare");
	let input: ExpectedReplayInput = materialize_expected_replay_input(
		snapshot,
		prefabs,
		ExpectedReplayCompileInput {
			stage_name: seed.stage_name(),
			difficulty: StageDifficulty::Normal,
			bindings,
			guide: &guide,
			tick_rate: TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
		},
	)
	.expect("the pinned main-story stage should materialize");
	let output: ExpectedReplayOutput = execute_expected_replay(&input, [0_u8; 32], variant_id);
	(input, output)
}

/// The stage must reach a battlefield verdict: nothing unsupported, nothing failing to compile.
fn assert_the_stage_reaches_a_verdict(output: &ExpectedReplayOutput) {
	assert!(
		output.report.unsupported.is_empty(),
		"the pinned stage must reach simulation: {:#?}",
		output.report.unsupported
	);
	assert!(
		output.report.compile_issues.is_empty(),
		"the pinned stage must compile: {:#?}",
		output.report.compile_issues
	);
	assert!(output.report.conservative_omissions.is_empty());
}

/// The stable IDs of the declared approximations this battle actually exercised.
fn exercised_approximation_ids(output: &ExpectedReplayOutput) -> Vec<&str> {
	output
		.report
		.declared_approximations
		.iter()
		.filter(|entry| entry.exercised)
		.map(|entry| entry.mechanic_id.as_str())
		.collect()
}

/// Whether an omitted talent's reason says its prefab root was read and left unclaimed.
fn omits_a_read_prefab_root(output: &ExpectedReplayOutput, mechanic_id: &str) -> bool {
	let omission = output
		.report
		.unmodeled_omissions
		.iter()
		.find(|entry| entry.mechanic_id == mechanic_id)
		.unwrap_or_else(|| panic!("{mechanic_id} is omitted"));
	let [reason] = omission.reasons.as_slice() else {
		panic!("{:?}", omission.reasons);
	};
	reason.ends_with("its verified prefab root is read but no reviewed mechanic template claims it")
}

fn unmodeled_ids(output: &ExpectedReplayOutput) -> Vec<&str> {
	output
		.report
		.unmodeled_omissions
		.iter()
		.map(|entry| entry.mechanic_id.as_str())
		.collect()
}

/// Executed tick of every guide action, in action order.
fn executed_ticks(output: &ExpectedReplayOutput) -> Vec<Option<u64>> {
	output
		.report
		.actions
		.iter()
		.map(|action| action.executed_tick)
		.collect()
}

/// Every skill activation tick of one operator.
fn activation_ticks(output: &ExpectedReplayOutput, operator_id: &str) -> Vec<u64> {
	output
		.report
		.skill_activations
		.iter()
		.filter(|activation| activation.operator_id == operator_id)
		.map(|activation| activation.tick)
		.collect()
}

/// Every DP change other than natural regeneration, as `(tick, kind, before, amount, after)`.
fn non_regeneration_dp(output: &ExpectedReplayOutput) -> Vec<(u64, &str, u32, u32, u32)> {
	output
		.report
		.dp
		.as_ref()
		.expect("a replayed stage audits its DP")
		.entries
		.iter()
		.filter(|entry| entry.kind != "regenerated")
		.map(|entry| {
			(
				entry.tick,
				entry.kind.as_str(),
				entry.before,
				entry.amount,
				entry.after,
			)
		})
		.collect()
}

fn first_leak(output: &ExpectedReplayOutput) -> (&str, u64) {
	let leaked = output
		.report
		.enemies
		.iter()
		.filter(|enemy| enemy.leaked_tick.is_some())
		.min_by_key(|enemy| enemy.leaked_tick.expect("filtered leaked enemy"))
		.expect("a defeat control must leak at least one enemy");
	(
		leaked.enemy_key.as_str(),
		leaked.leaked_tick.expect("filtered leaked enemy"),
	)
}

/// Every enemy record is Supported from a verified prefab decode, and only the ranged record fires
/// a projectile.
fn assert_every_enemy_compiles_from_its_verified_prefab(input: &ExpectedReplayInput) {
	let keys: Vec<&str> = input
		.facts
		.enemies
		.iter()
		.map(|enemy| enemy.enemy_key.as_str())
		.collect();
	assert_eq!(keys, ENEMY_KEYS);
	for enemy in &input.facts.enemies {
		// The ranged record reaches a second mechanic: the flight its projectile does not execute.
		let ranged: bool = enemy.enemy_key == RANGED_ENEMY_KEY;
		let mechanic = enemy
			.reached_mechanics
			.iter()
			.find(|mechanic| mechanic.id.as_str() == format!("{}/prefab", enemy.enemy_key))
			.unwrap_or_else(|| panic!("{} reaches {:?}", enemy.enemy_key, enemy.reached_mechanics));
		assert_eq!(
			enemy.reached_mechanics.len(),
			usize::from(ranged) + 1,
			"{} reaches {:?}",
			enemy.enemy_key,
			enemy.reached_mechanics
		);
		let origin_prefix: String =
			format!("verified-prefab:enemy:{}/bundle:enm_pfb_", enemy.enemy_key);
		assert!(
			mechanic.origin.starts_with(&origin_prefix),
			"{}",
			mechanic.origin
		);
		assert_eq!(mechanic.behavior, MechanicBehavior::Damage);
		if ranged {
			let flight = enemy
				.reached_mechanics
				.iter()
				.find(|entry| {
					entry.id.as_str() == format!("{}/prefab/projectile-flight", enemy.enemy_key)
				})
				.unwrap_or_else(|| {
					panic!(
						"{} must inventory its unexecuted flight: {:?}",
						enemy.enemy_key, enemy.reached_mechanics
					)
				});
			assert!(
				matches!(
					flight.behavior,
					MechanicBehavior::DeclaredApproximation { .. }
				),
				"{:?}",
				flight.behavior
			);
		}
		let expected_pattern: AttackPattern = if ranged {
			AttackPattern::Projectile { flight_ticks: 1 }
		} else {
			AttackPattern::Single
		};
		assert_eq!(
			enemy.attack.pattern, expected_pattern,
			"{}",
			enemy.enemy_key
		);
	}
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_reviewed_roster_clears_every_spawn_with_nothing_omitted() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let bindings: Vec<ExactReplayBuildBinding> = reviewed_roster_bindings(&snapshot);
	assert_the_certified_builds_resolved(&bindings);
	let (input, output) = replay_guide(
		&snapshot,
		&prefabs,
		REVIEWED_GUIDE,
		&bindings,
		"main-01-01-reviewed-roster",
	);

	assert_eq!(output.report.stage_id, STAGE_ID);
	assert_eq!(output.report.path_count, 16);
	assert_eq!(output.report.wave_count, 3);
	assert_eq!(output.report.enemy_definition_count, 6);
	assert_eq!(output.report.enemy_spawn_count, 33);
	assert_eq!(output.report.exact_build_count, 4);
	assert_the_stage_reaches_a_verdict(&output);
	assert_every_enemy_compiles_from_its_verified_prefab(&input);

	// Nothing is left out on either side of the battlefield.
	assert!(
		unmodeled_ids(&output).is_empty(),
		"{:?}",
		unmodeled_ids(&output)
	);
	// The clear certifies, and the report now also discloses what the model simplified to reach it.
	// This battle runs every spawn, so it exercises both of the stage's declared approximations: the
	// ranged record's unexecuted projectile flight, and the spawn scatter the whole-cell path model
	// cannot hold. Neither is a hole in the model, so neither withholds the verdict — they travel
	// beside it, and they are what a later neutrality bound would have to start from.
	assert_eq!(
		exercised_approximation_ids(&output),
		[
			"enemy_1028_mocock/prefab/projectile-flight",
			"stage/route/11/spawn-scatter",
			"stage/route/12/spawn-scatter",
			"stage/route/13/spawn-scatter",
			"stage/route/14/spawn-scatter",
			"stage/route/15/spawn-scatter",
			"stage/route/16/spawn-scatter",
			"stage/route/18/spawn-scatter",
			"stage/route/6/spawn-scatter",
			"stage/route/7/spawn-scatter",
			"stage/route/8/spawn-scatter",
			"stage/route/9/spawn-scatter",
		]
	);
	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedVictory
	);
	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert!(output.report.simulation_issues.is_empty());
	assert_eq!(output.report.enemies.len(), 33);
	assert!(output
		.report
		.enemies
		.iter()
		.all(|enemy| enemy.leaked_tick.is_none() && enemy.remaining_hp_micros == 0));
	assert!(output
		.report
		.operators
		.iter()
		.all(|operator| operator.death_tick.is_none()));

	// 讯使 alone is affordable at battle start, and her skill's first deployment-point grant at tick
	// 510 pays for 蛇屠箱 in the same tick. The two six-stars then wait for regeneration: 圣聆初雪
	// at 960, and 黍 at 1410 in the tick the second grant arrives. The last three grants find
	// nothing left to pay for and are capped at the stage's 99 DP.
	assert_eq!(
		executed_ticks(&output),
		[
			Some(0),
			Some(510),
			Some(960),
			Some(1410),
			Some(1410),
			Some(1410)
		]
	);
	assert_eq!(
		activation_ticks(&output, "char_198_blackd"),
		[510, 1410, 2310, 3210, 4110, 5010]
	);
	assert_eq!(
		non_regeneration_dp(&output),
		[
			(0, "spent", 10, 10, 0),
			(510, "granted", 17, 9, 26),
			(510, "spent", 26, 19, 7),
			(960, "spent", 22, 22, 0),
			(1410, "granted", 15, 9, 24),
			(1410, "spent", 24, 20, 4),
			(2310, "granted", 34, 9, 43),
			(3210, "granted", 73, 9, 82),
			(4110, "granted", 99, 0, 99),
			(5010, "granted", 99, 0, 99),
		]
	);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn moving_the_reviewed_blocker_off_the_shared_tile_certifies_defeat() {
	let (snapshot, prefabs) = local_battle_snapshots();
	// The same one explicit change the authored control makes, on the reviewed roster: 讯使 moves
	// from [4, 3], which both walk paths cross, to [1, 3], which only the left path crosses.
	// Predicted effect, stated from the map and the DP schedule rather than from a run: the
	// vertical path is held by nobody until 蛇屠箱 can be afforded on [5, 3], and the first
	// vertical-path runner reaches the lower goal before she can cover it. The stage's own
	// `options.moveMultiplier` halves every enemy's speed, so the runner takes 205 rather than 103
	// ticks to cross, which is late enough that 讯使's first deployment-point grant does land and
	// 蛇屠箱 does reach the field — the defeat therefore has to survive the second blocker
	// arriving, not merely beat it.
	assert_eq!(REVIEWED_GUIDE.matches("\"location\": [4, 3]").count(), 1);
	let control: String = REVIEWED_GUIDE.replace("\"location\": [4, 3]", "\"location\": [1, 3]");
	let bindings: Vec<ExactReplayBuildBinding> = reviewed_roster_bindings(&snapshot);
	assert_the_certified_builds_resolved(&bindings);

	let (_, output) = replay_guide(
		&snapshot,
		&prefabs,
		&control,
		&bindings,
		"main-01-01-reviewed-unheld-vertical-path-control",
	);

	assert_the_stage_reaches_a_verdict(&output);
	assert!(
		unmodeled_ids(&output).is_empty(),
		"{:?}",
		unmodeled_ids(&output)
	);
	assert!(output.report.simulation_issues.is_empty());

	// Nothing is omitted either way, so the leak certifies the defeat rather than only recording it.
	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedDefeat
	);
	assert!(output
		.report
		.terminal
		.as_deref()
		.is_some_and(|terminal| terminal.starts_with("GoalLeak")));
	assert_eq!(first_leak(&output), ("enemy_1000_gopro", 565));

	// The divergence from the positive case's ordered events. The two runs share their first two
	// events exactly — 讯使 at tick 0, then her grant paying for 蛇屠箱 in tick 510 — and then
	// part: in the positive case 蛇屠箱 lands on [5, 3], which the vertical path crosses, while
	// here she lands there with 讯使 no longer holding [4, 3] ahead of her, and the runner that
	// entered before she arrived reaches the goal at 565. Every later deployment stays pending,
	// and the skill fires once rather than six times.
	assert_eq!(
		executed_ticks(&output),
		[Some(0), Some(510), None, None, None, None]
	);
	assert_eq!(activation_ticks(&output, "char_198_blackd"), [510]);
	assert_eq!(
		non_regeneration_dp(&output),
		[
			(0, "spent", 10, 10, 0),
			(510, "granted", 17, 9, 26),
			(510, "spent", 26, 19, 7),
		]
	);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_authored_baseline_clears_every_spawn_on_the_pinned_stage() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let bindings: Vec<ExactReplayBuildBinding> = roster_bindings(&snapshot);
	let (input, output) = replay_guide(
		&snapshot,
		&prefabs,
		GUIDE,
		&bindings,
		"main-01-01-authored-baseline",
	);

	assert_eq!(output.report.stage_id, STAGE_ID);
	assert_eq!(output.report.path_count, 16);
	assert_eq!(output.report.wave_count, 3);
	assert_eq!(output.report.enemy_definition_count, 6);
	assert_eq!(output.report.enemy_spawn_count, 33);
	assert_eq!(output.report.exact_build_count, 4);
	assert_the_stage_reaches_a_verdict(&output);

	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert_eq!(output.report.enemies.len(), 33);
	assert!(output
		.report
		.enemies
		.iter()
		.all(|enemy| enemy.leaked_tick.is_none() && enemy.remaining_hp_micros == 0));
	assert!(output
		.report
		.operators
		.iter()
		.all(|operator| operator.death_tick.is_none()));

	// 讯使 alone is affordable at battle start. Her skill starts at 13 of 30 SP and charges one SP a
	// second, so it first fires at tick 510 and every 900 ticks after, each time granting 9 DP.
	// The first grant lifts the 17 regenerated DP to 26 and 蛇屠箱 (19) deploys in the same tick,
	// sixty ticks before regeneration alone would afford it; 白金 and 杜宾 follow 270 ticks
	// earlier than they did without the grant. The later grants find nothing left to pay for and
	// are capped at the stage's 99 DP.
	assert_eq!(
		executed_ticks(&output),
		[
			Some(0),
			Some(510),
			Some(630),
			Some(1020),
			Some(1020),
			Some(1020)
		]
	);
	assert_eq!(
		activation_ticks(&output, "char_198_blackd"),
		[510, 1410, 2310, 3210, 4110]
	);
	assert_eq!(
		non_regeneration_dp(&output),
		[
			(0, "spent", 10, 10, 0),
			(510, "granted", 17, 9, 26),
			(510, "spent", 26, 19, 7),
			(630, "spent", 11, 11, 0),
			(1020, "spent", 13, 13, 0),
			(1410, "granted", 13, 9, 22),
			(2310, "granted", 52, 9, 61),
			(3210, "granted", 91, 8, 99),
			(4110, "granted", 99, 0, 99),
		]
	);

	// The battlefield verdict is reached, but it cannot be certified while operator semantics stay
	// unreviewed. Every enemy prefab already decodes, so reviewing them must flip this to
	// ExpectedVictory, and this assertion must then be rewritten around that verdict.
	assert_every_enemy_compiles_from_its_verified_prefab(&input);
	assert_eq!(unmodeled_ids(&output), UNREVIEWED_OPERATOR_SEMANTICS);
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
	// A talent root no reviewed template claims stays its talent's omission rather than refusing
	// the replay, and the omission says the root was read; 杜宾's per-mode talent is not.
	assert!(omits_a_read_prefab_root(
		&output,
		"char_204_platnm/effect/0/1"
	));
	assert!(!omits_a_read_prefab_root(
		&output,
		"char_130_doberm/effect/1/1"
	));
	// 讯使's talent is bound, but in this job she never holds two enemies at once.
	assert!(input.guide.builds[0]
		.reached_mechanics
		.iter()
		.any(|mechanic| mechanic.id.as_str() == COURIER_TALENT));
	assert!(two_block_spans(&output, 0).is_empty());
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn moving_the_only_immediate_blocker_off_the_shared_tile_loses_the_stage() {
	let (snapshot, prefabs) = local_battle_snapshots();
	// One explicit change. 讯使 is the only roster entry affordable at battle start; every other
	// deployment waits for DP. Moving 讯使 from [4, 3], which both walk paths cross, to [1, 3],
	// which only the left path crosses, leaves the vertical path unheld until 蛇屠箱 arrives.
	assert_eq!(GUIDE.matches("\"location\": [4, 3]").count(), 1);
	let control: String = GUIDE.replace("\"location\": [4, 3]", "\"location\": [1, 3]");
	let bindings: Vec<ExactReplayBuildBinding> = roster_bindings(&snapshot);

	let (_, output) = replay_guide(
		&snapshot,
		&prefabs,
		&control,
		&bindings,
		"main-01-01-unheld-vertical-path-control",
	);

	assert_the_stage_reaches_a_verdict(&output);
	assert!(output
		.report
		.terminal
		.as_deref()
		.is_some_and(|terminal| terminal.starts_with("GoalLeak")));
	// The first decisive event: a vertical-path runner reaches the lower goal unopposed. It takes
	// the same 565 ticks as in the reviewed control, because the runner and the tiles are the
	// same and only the roster differs.
	assert_eq!(first_leak(&output), ("enemy_1000_gopro", 565));
	// 讯使's grant lands at 510 and pays for 蛇屠箱 in the same tick, but she arrives on [5, 3]
	// behind the runner that is already past, so the leak still follows at 565 and every later
	// deployment stays pending.
	assert_eq!(
		executed_ticks(&output),
		[Some(0), Some(510), None, None, None, None]
	);
	assert_eq!(activation_ticks(&output, "char_198_blackd"), [510]);
	assert_eq!(
		non_regeneration_dp(&output),
		[
			(0, "spent", 10, 10, 0),
			(510, "granted", 17, 9, 26),
			(510, "spent", 26, 19, 7),
		]
	);

	// The leak is modelled but cannot certify a defeat. Every omission is an operator benefit —
	// damage and attack scaling — so the replay understates the roster, and nothing it
	// carries proves those benefits stay inactive before the leak. The omission-free
	// ExpectedDefeat regression is the stage-isolation probe.
	assert_eq!(unmodeled_ids(&output), UNREVIEWED_OPERATOR_SEMANTICS);
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_stage_isolation_replay_leaves_no_unmodeled_mechanic() {
	let (snapshot, prefabs) = local_battle_snapshots();
	// 黍's archetype, talents and selected skill are all reviewed, so every mechanic she reaches
	// decodes and whatever remains must come from the stage.
	let bindings: Vec<ExactReplayBuildBinding> = vec![maximal_no_module_binding(
		&snapshot,
		"黍",
		"char_2025_shu",
		1,
	)];
	let (input, output) = replay_guide(
		&snapshot,
		&prefabs,
		ISOLATION_GUIDE,
		&bindings,
		"main-01-01-stage-isolation",
	);

	assert_the_stage_reaches_a_verdict(&output);
	// With every operator-side mechanic decoded and every enemy prefab verified, nothing is omitted.
	assert_every_enemy_compiles_from_its_verified_prefab(&input);
	assert!(
		unmodeled_ids(&output).is_empty(),
		"{:?}",
		unmodeled_ids(&output)
	);

	// 黍 costs 22 DP and the stage starts at 10, so she reaches [4, 3] at tick 300 — but she
	// holds only the tile she stands on, and the first left-path runner is already past it. The
	// omission-free replay is therefore a completed defeat rather than a completed clear.
	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedDefeat
	);
	assert_eq!(executed_ticks(&output), [Some(300), Some(300), Some(300)]);
	assert_eq!(first_leak(&output), ("enemy_1000_gopro_2", 346));
}

/// The cadence planning schedules for one build's selected skill, decoded the way the solver does.
fn planned_grant_cadence(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	operator_id: &str,
	skill: u8,
) -> ChargeCostGrantCadence {
	let binding: ExactReplayBuildBinding =
		maximal_no_module_binding(snapshot, operator_id, operator_id, skill);
	let resolved = snapshot
		.operators()
		.resolve(&binding.build)
		.expect("reviewed build resolves");
	let selected_skill = resolved.selected_skill();
	let prefab_id: &str = selected_skill
		.prefab_id()
		.expect("a charge-cost skill names a common prefab");
	let graph = interpret_battle_prefab_asset(
		prefabs,
		"[uc]skills",
		&format!("dyn/battle/prefabs/[uc]skills/{prefab_id}.prefab"),
		BattlePrefabRootSection::MainAsset,
		prefab_id,
		&built_in_battle_prefab_handler_registry(),
	)
	.expect("the common skill prefab interprets");
	let cadences: Vec<ChargeCostGrantCadence> = try_decode_charge_cost_skill_template(
		&graph,
		selected_skill,
		snapshot,
		snapshot.resolved_revision(),
	)
	.expect("the reviewed charge-cost owner must not drift")
	.expect("the reviewed charge_cost buff claims the owner")
	.grant_cadences(
		selected_skill,
		TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
	)
	.expect("the reviewed owner has a cadence");
	let [cadence] = cadences.as_slice() else {
		panic!("the reviewed owner carries exactly one grant, found {cadences:?}");
	};
	*cadence
}

/// What a cadence predicts from a first grant at `first_tick`, through `last_tick`.
fn predicted_grant_ticks(
	cadence: ChargeCostGrantCadence,
	first_tick: u64,
	last_tick: u64,
) -> Vec<u64> {
	std::iter::successors(Some(first_tick), |tick: &u64| {
		tick.checked_add(cadence.period_ticks)
	})
	.take_while(|tick: &u64| *tick <= last_tick)
	.collect()
}

/// Every tick the replay credited a deployment-point grant at.
fn replayed_grant_ticks(output: &ExpectedReplayOutput) -> Vec<u64> {
	non_regeneration_dp(output)
		.into_iter()
		.filter(|(_, kind, _, _, _)| *kind == "granted")
		.map(|(tick, _, _, _, _)| tick)
		.collect()
}

/// The grant amounts the replay credited at ticks where the stage cap discarded none of them.
fn uncapped_grant_amounts(output: &ExpectedReplayOutput) -> Vec<u32> {
	non_regeneration_dp(output)
		.into_iter()
		.filter(|(_, kind, _, _, after)| *kind == "granted" && *after < STAGE_MAX_DP)
		.map(|(_, _, _, amount, _)| amount)
		.collect()
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_planned_grant_cadence_predicts_every_tick_replay_grants_at() {
	let (snapshot, prefabs) = local_battle_snapshots();

	// 讯使's automatic first skill, on the authored roster. She deploys at tick 0 and nothing
	// gates her casts, so the cadence alone must reproduce every tick the replay grants at.
	let bindings: Vec<ExactReplayBuildBinding> = roster_bindings(&snapshot);
	let (_, output) = replay_guide(
		&snapshot,
		&prefabs,
		GUIDE,
		&bindings,
		"main-01-01-authored-grant-cadence",
	);
	let deploy_tick: u64 = executed_ticks(&output)[0].expect("讯使 deploys");
	let automatic: ChargeCostGrantCadence =
		planned_grant_cadence(&snapshot, &prefabs, "char_198_blackd", 1);
	let replayed: Vec<u64> = replayed_grant_ticks(&output);
	assert_eq!(replayed, activation_ticks(&output, "char_198_blackd"));
	assert_eq!(
		predicted_grant_ticks(
			automatic,
			deploy_tick + automatic.first_offset_ticks,
			*replayed.last().expect("replay credited a grant"),
		),
		replayed
	);
	// The amount is the grant before the stage cap truncates it, so the uncapped credits match.
	assert_eq!(automatic.amount, 9);
	assert!(uncapped_grant_amounts(&output)
		.iter()
		.all(|amount: &u32| *amount == automatic.amount));

	// Her manual second skill. A retained operator never casts it, so this run hands it to the
	// skill daemon: the cadence is checked on its period and amount, not on its first tick.
	let mut manual_bindings: Vec<ExactReplayBuildBinding> = roster_bindings(&snapshot);
	manual_bindings[0] = maximal_no_module_binding(&snapshot, "讯使", "char_198_blackd", 2);
	manual_bindings[0].skill_usage = 1;
	let manual_guide: String = GUIDE.replace(
		"\"name\": \"讯使\",\n\t\t\t\"skill\": 1,\n\t\t\t\"skill_usage\": 0",
		"\"name\": \"讯使\",\n\t\t\t\"skill\": 2,\n\t\t\t\"skill_usage\": 1",
	);
	let (_, manual_output) = replay_guide(
		&snapshot,
		&prefabs,
		&manual_guide,
		&manual_bindings,
		"main-01-01-courier-second-skill-grant-cadence",
	);
	let manual: ChargeCostGrantCadence =
		planned_grant_cadence(&snapshot, &prefabs, "char_198_blackd", 2);
	let manual_replayed: Vec<u64> = replayed_grant_ticks(&manual_output);
	assert_eq!(
		predicted_grant_ticks(
			manual,
			*manual_replayed.first().expect("the daemon cast the skill"),
			*manual_replayed.last().expect("the daemon cast the skill"),
		),
		manual_replayed
	);
	assert_eq!(manual.amount, 3);
	assert!(uncapped_grant_amounts(&manual_output)
		.iter()
		.all(|amount: &u32| *amount == manual.amount));
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_couriers_manual_second_skill_grants_on_each_cast_and_omits_the_rest() {
	let (snapshot, prefabs) = local_battle_snapshots();
	// The same job with 讯使 on her manual second skill, left to the skill daemon. Its buff
	// ability is the reviewed charge_cost owner: a one-time grant on each cast, a fifteen-second
	// active span, and a periodic grant with a defence buff that no reviewed template describes.
	let mut bindings: Vec<ExactReplayBuildBinding> = roster_bindings(&snapshot);
	bindings[0] = maximal_no_module_binding(&snapshot, "讯使", "char_198_blackd", 2);
	bindings[0].skill_usage = 1;
	let source_selection: &str =
		"\"name\": \"讯使\",\n\t\t\t\"skill\": 1,\n\t\t\t\"skill_usage\": 0";
	assert_eq!(GUIDE.matches(source_selection).count(), 1);
	let guide: String = GUIDE.replace(
		source_selection,
		"\"name\": \"讯使\",\n\t\t\t\"skill\": 2,\n\t\t\t\"skill_usage\": 1",
	);

	let (_, output) = replay_guide(
		&snapshot,
		&prefabs,
		&guide,
		&bindings,
		"main-01-01-courier-second-skill",
	);

	assert_the_stage_reaches_a_verdict(&output);
	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	// The daemon is the job's last action, so the first cast waits for every deployment, which
	// therefore keeps the regeneration-only schedule. Each cast then grants three DP, and the
	// next follows the fifteen-second active span and a full thirty-second recharge.
	assert_eq!(
		executed_ticks(&output),
		[
			Some(0),
			Some(570),
			Some(900),
			Some(1290),
			Some(1290),
			Some(1290)
		]
	);
	assert_eq!(
		activation_ticks(&output, "char_198_blackd"),
		[1290, 2640, 3990]
	);
	assert_eq!(
		non_regeneration_dp(&output),
		[
			(0, "spent", 10, 10, 0),
			(570, "spent", 19, 19, 0),
			(900, "spent", 11, 11, 0),
			(1290, "spent", 13, 13, 0),
			(1290, "granted", 0, 3, 3),
			(2640, "granted", 48, 3, 51),
			(3990, "granted", 96, 3, 99),
		]
	);

	// The grant executes, so the skill's omission now names only what the template leaves out.
	let courier_skill = output
		.report
		.unmodeled_omissions
		.iter()
		.find(|entry| entry.mechanic_id == "char_198_blackd/skill/skchr_blackd_2")
		.expect("the rest of the skill stays omitted");
	let [reason] = courier_skill.reasons.as_slice() else {
		panic!("{:?}", courier_skill.reasons);
	};
	assert!(
		reason.starts_with("selected skill behavior beyond its deployment-point grant")
			&& reason.contains("\"blackd_s_2[period]\" (\"periodic_cost\")"),
		"{reason}"
	);
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
}

const COURIER_TALENT: &str = "char_198_blackd/template/blackd_t_1/block-count-toggle";

/// The authored job without 杜宾, whose deployment on `[3, 3]` otherwise takes the enemies 讯使
/// would hold two at a time.
fn job_without_doberm() -> String {
	const DOBERM_ACTION: &str = "\t\t{\n\t\t\t\"type\": \"Deploy\",\n\t\t\t\"name\": \"杜宾\",\n\t\t\t\"location\": [3, 3],\n\t\t\t\"direction\": \"Left\"\n\t\t},\n";
	const DOBERM_OPERATOR: &str =
		",\n\t\t{\n\t\t\t\"name\": \"杜宾\",\n\t\t\t\"skill\": 1,\n\t\t\t\"skill_usage\": 0\n\t\t}";
	assert_eq!(GUIDE.matches(DOBERM_ACTION).count(), 1);
	assert_eq!(GUIDE.matches(DOBERM_OPERATOR).count(), 1);
	GUIDE
		.replace(DOBERM_ACTION, "")
		.replace(DOBERM_OPERATOR, "")
}

/// Every span in which one operator holds at least two enemies, as `[start, end)` ticks. A block
/// that ends in the tick another starts, as a death and its recapture do, keeps the span whole.
fn two_block_spans(output: &ExpectedReplayOutput, operator_instance: u32) -> Vec<(u64, u64)> {
	let mut changes: Vec<(u64, i32)> = output
		.report
		.blocks
		.iter()
		.filter(|block| {
			block.blocker_kind == ReplayBlockerKind::Operator
				&& block.blocker_instance_id == Some(operator_instance)
		})
		.flat_map(|block| {
			let end: u64 = block
				.end_tick
				.expect("every block ends by the battle's end");
			[(block.start_tick, 1), (end, -1)]
		})
		.collect();
	// Starts sort before ends in one tick, so a same-tick replacement never dips below two.
	changes.sort_by_key(|&(tick, delta)| (tick, -delta));
	let mut held: i32 = 0;
	let mut spans: Vec<(u64, u64)> = Vec::new();
	let mut opened: Option<u64> = None;
	for (tick, delta) in changes {
		held += delta;
		match (held >= 2, opened) {
			(true, None) => opened = Some(tick),
			(false, Some(start)) => {
				spans.push((start, tick));
				opened = None;
			}
			_ => {}
		}
	}
	spans
}

fn courier_hp_micros(input: &ExpectedReplayInput, end_at: Option<u64>, variant_id: &str) -> u64 {
	let mut input: ExpectedReplayInput = input.clone();
	if let Some(end_at) = end_at {
		input.facts.end_at = Some(SimTick::new(end_at));
	}
	let output: ExpectedReplayOutput = execute_expected_replay(&input, [0_u8; 32], variant_id);
	output
		.report
		.operators
		.iter()
		.find(|operator| operator.operator_id == "char_198_blackd")
		.expect("讯使 is deployed")
		.remaining_hp_micros
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn the_couriers_talent_never_lowers_a_hit_she_takes_while_holding_one() {
	let (snapshot, prefabs) = local_battle_snapshots();
	let bindings: Vec<ExactReplayBuildBinding> = ROSTER[..3]
		.iter()
		.map(|(source_name, operator_id)| {
			maximal_no_module_binding(&snapshot, source_name, operator_id, 1)
		})
		.collect();
	let (input, output) = replay_guide(
		&snapshot,
		&prefabs,
		&job_without_doberm(),
		&bindings,
		"main-01-01-without-doberm",
	);

	assert_the_stage_reaches_a_verdict(&output);
	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert_eq!(unmodeled_ids(&output), ["char_204_platnm/effect/0/1"]);
	let courier_talent = input.guide.builds[0]
		.reached_mechanics
		.iter()
		.find(|mechanic| mechanic.id.as_str() == COURIER_TALENT)
		.expect("the block-count toggle template binds 讯使's talent");
	assert_eq!(courier_talent.behavior, MechanicBehavior::StatModifier);
	assert!(courier_talent
		.origin
		.starts_with("verified-prefab:char_198_blackd:"));
	// Without 杜宾 on [3, 3], 讯使 holds two enemies at a time in three spans. The first is the
	// same-tick handover the span rule keeps whole: one block ends in tick 3990 and the next
	// begins in it, so she never holds two for any duration there.
	assert_eq!(
		two_block_spans(&output, 0),
		[(3990, 3990), (4065, 4080), (4230, 4290)]
	);

	// The same materialized battle without the talent's one deployment effect isolates it.
	assert_eq!(input.guide.builds[0].on_deploy.len(), 1);
	let mut without_talent: ExpectedReplayInput = input.clone();
	without_talent.guide.builds[0].on_deploy.clear();
	let hp_after = |tick: Option<u64>| -> (u64, u64) {
		(
			courier_hp_micros(&input, tick, "with-talent"),
			courier_hp_micros(&without_talent, tick, "without-talent"),
		)
	};
	// At this stage's real movement speed the talent changes nothing here, and the two reasons are
	// worth stating because only one of them is about the talent.
	//
	// Her defense is 405. Every hit that lands strictly inside a span is a 9-point one already on
	// the minimum-damage floor, which a higher defense cannot lower. The one hit large enough to
	// tell the two replays apart is the 500-attack enemy_1030_wteeth at tick 4230 — 500 - 405 is
	// 95, while 500 - 405 x 1.19 would fall below the 5% floor and give 25 — and it lands in the
	// very tick the third span opens, resolving before that block is counted, so it reads one held
	// enemy and takes 95 either way.
	//
	// So this job pins the "only" half of the talent: it never lowers a hit taken while she holds
	// one. The +19% itself is pinned by the decoder test and the synthetic replay, not here.
	let probes: [u64; 7] = [3980, 4140, 4225, 4230, 4245, 4290, 4306];
	assert_eq!(
		probes.map(|tick| hp_after(Some(tick))),
		[
			(1_916_250_000, 1_916_250_000),
			(1_821_250_000, 1_821_250_000),
			(1_812_250_000, 1_812_250_000),
			(1_717_250_000, 1_717_250_000),
			(1_708_000_000, 1_708_000_000),
			(1_708_000_000, 1_708_000_000),
			(1_699_000_000, 1_699_000_000),
		]
	);
	assert_eq!(hp_after(None), (1_699_000_000, 1_699_000_000));
}
