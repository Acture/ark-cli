use std::collections::{BTreeMap, BTreeSet};

use crate::data::{GridPosition, MapDimensions, RangeOffset};
use crate::simulation::{
	ChoiceId, EntityId, EventRequest, ModelError, ModelStep, Policy, PolicyDecision,
	ScheduledEvent, SimTick, SimulationIssue, SimulationModel, SimulationSeed, Simulator, Trace,
	Transition, Verification, Weighted,
};

use crate::mechanics::domain::{
	resolve_cached_tile_layer_pulse, CachedTileLayerPulse, Facing, LayerMutationCause,
	LayeredTileCell, OrderedOrthogonalNeighbors, PropagationDestination, SourceRangeRelation,
	TileLayerEvent, TileLayerPropagationAmbiguity, TileLayerPropagationError,
	TileLayerPropagationResolution, TilePersistence,
};

use crate::mechanics::domain::{
	ActorCombatFacts, ActorCondition, ArkBattleFacts, AttackCadenceModifier, AttackPattern,
	AttackProfile, AttackRangeModifier, AttackSpeedModifier, BattleActorId, BattleCompileIssue,
	BattlePath, BattlePathStepAction, BattleTimeAnchor, BlockCountModifier, BlockTargetFilter,
	BlockTargetModifier, BuffModifier, BuffRemovalSource, ConditionalStatModifier, DamageDelivery,
	DamageType, EffectTarget, EnemyAxisDirectHitFacts, EnemyFacts, EnemyInstanceId,
	EnemySpawnFacts, ExpectedEffect, ExpectedValue, Faction, HitCountModifier, HpRatioComparison,
	MaterializedGuide, MaterializedGuideAction, MaterializedGuidePolicy, MechanicBehavior,
	MechanicId, MechanicSupportReport, NormalAttackAction, OperatorBuildFacts, OperatorInstanceId,
	PathId, PeriodicSpSource, PredicateActor, ReplayCellSet, ReplayListener, ReplayPredicate,
	ReplayRepeat, ReplayTrigger, ReplayTriggerKind, RuntimeBuff, RuntimeConditionalAura,
	RuntimeEffectLifetime, RuntimeSkillInactiveStance, RuntimeTileEffect, RuntimeTileFieldOverlay,
	RuntimeTileLayerMutationCause, RuntimeTileLayerPropagationAudit,
	RuntimeTileLayerThresholdReaction, ScheduledStageEffect, ShiftDirection, SkillActivationRule,
	SkillFacts, SpChargeRule, StatKind, StatModifier, StatModifierBucket, StatusEffect,
	StatusFactionFilter, StatusKind, StoredAttackHpRatioComparison,
	StoredAttackReplacementDelivery, StoredAttackReplacementFacts,
	StoredAttackReservationInvalidationAction, StoredAttackTargetOrder,
	StoredAttackTargetRelationship, SummonCellOccupancy, SummonTemplate, SummonTemplateId,
	TaggedEffect, TargetCellActorSpawn, TargetCellActorSpawnEvent, TargetCellSpawnCollision,
	TargetCellSpawnOwnership, TargetCountModifier, Targetability, TeamCompositionCondition,
	TileFieldOverlayLeaveScope, TileLayerReactionOwner, TileLayerThresholdReactionEffect,
	TileLayerThresholdTransition, TriggerActorFilter, UnsupportedMechanicEntry, WaveId,
};

/// Fully preflighted deterministic battle. Construct through [`compile_battle`].
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompiledArkBattle {
	pub(crate) facts: ArkBattleFacts,
	pub(crate) guide: MaterializedGuide,
}

impl CompiledArkBattle {
	#[must_use]
	pub fn battle_id(&self) -> &str {
		&self.facts.battle_id
	}

	#[must_use]
	pub const fn end_at(&self) -> Option<SimTick> {
		self.facts.end_at
	}

	#[must_use]
	pub fn guide(&self) -> &MaterializedGuide {
		&self.guide
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GuideExecutorActionKind {
	Deploy,
	ActivateSkill,
	Retreat,
	SpeedUp,
	SkillDaemon,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedCommand {
	Deploy {
		action_index: usize,
		operator: OperatorInstanceId,
		cell: GridPosition,
		facing: Facing,
	},
	ActivateSkill {
		action_index: usize,
		operator: OperatorInstanceId,
	},
	DiscardSkill {
		action_index: usize,
		operator: OperatorInstanceId,
	},
	Retreat {
		action_index: usize,
		operator: OperatorInstanceId,
	},
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillActivationOrigin {
	GuideAction,
	Automatic,
	SkillDaemon,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillEndReason {
	DurationElapsed,
	ManualDiscard,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StoredAttackTargetRef {
	pub actor: BattleActorId,
	/// Present only for operator targets, whose instance ID can be redeployed.
	pub deployment_generation: Option<u32>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedEvent {
	DpRegen,
	OperatorRedeploymentReady {
		operator: OperatorInstanceId,
	},
	GuideActionReady {
		action_index: usize,
		kind: GuideExecutorActionKind,
	},
	WaveStart {
		wave: WaveId,
		forced: bool,
	},
	EnemySpawn {
		enemy: EnemyInstanceId,
	},
	Move {
		enemy: EnemyInstanceId,
		generation: u32,
	},
	NormalAttack {
		actor: BattleActorId,
		generation: u32,
	},
	StoredAttackReplacementDelivery {
		reservation_id: u64,
		source: BattleActorId,
		target: StoredAttackTargetRef,
	},
	AxisDirectHit {
		enemy: EnemyInstanceId,
		generation: u32,
		phase: AxisDirectHitPhase,
	},
	StealthRecovered {
		enemy: EnemyInstanceId,
		generation: u32,
	},
	SpPulse {
		operator: OperatorInstanceId,
		deployment_generation: u32,
		charge_generation: u32,
		source: PeriodicSpSource,
	},
	SkillActivation {
		operator: OperatorInstanceId,
		mechanic: MechanicId,
		deployment_generation: u32,
		origin: SkillActivationOrigin,
	},
	SkillDiscarded {
		operator: OperatorInstanceId,
		deployment_generation: u32,
		charge_generation: u32,
	},
	SkillEnded {
		operator: OperatorInstanceId,
		deployment_generation: u32,
		charge_generation: u32,
		reason: SkillEndReason,
	},
	FireTrigger {
		kind: ReplayTriggerKind,
		actor: BattleActorId,
		effect_source: BattleActorId,
	},
	ListenerPulse {
		listener_id: u64,
		generation: u32,
	},
	BuffExpired {
		actor: BattleActorId,
		buff_id: u64,
		generation: u32,
	},
	TileEffectPulse {
		tile_effect_id: u64,
		generation: u32,
	},
	TileEffectExpired {
		tile_effect_id: u64,
		generation: u32,
	},
	TileFieldOverlayPulse {
		overlay_id: u64,
		generation: u32,
	},
	ConditionalAuraPulse {
		aura_id: u64,
		target: OperatorInstanceId,
		target_deployment_generation: u32,
		buff_id: u64,
	},
	TileLayerPropagationAudit {
		overlay: MechanicId,
		tile: MechanicId,
		event: RuntimeTileLayerPropagationAudit,
	},
	ResolveEffect {
		source: BattleActorId,
		source_deployment_generation: Option<u32>,
		targets: Vec<BattleActorId>,
		effect: Box<TaggedEffect>,
	},
	ModifierExpired {
		actor: BattleActorId,
		modifier_id: u64,
	},
	ReconcileBlocking {
		actor: BattleActorId,
		release_reason: BlockEndReason,
	},
	StatusExpired {
		actor: BattleActorId,
		status: StatusKind,
		generation: u32,
	},
	SummonSpawn {
		source: BattleActorId,
		template: SummonTemplateId,
		instance: u32,
		cell: GridPosition,
	},
	TargetCellActorSpawn(TargetCellActorSpawnEvent),
	Death {
		actor: BattleActorId,
		operator_deployment_generation: Option<u32>,
	},
	EnemyDeathEffectsDrained {
		enemy: EnemyInstanceId,
	},
	BattleDeadline,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AxisDirectHitPhase {
	Ready,
	Projectile { index: u16, direction: u8 },
	End,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActionAuditStatus {
	Executed,
	ExecutedAfterDpWait,
	WaitedForDp,
	SkippedNotReady,
	ExecutorOnly,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GuideActionAudit {
	pub action_index: usize,
	pub kind: GuideExecutorActionKind,
	pub requested_at: SimTick,
	pub executed_at: Option<SimTick>,
	pub status: ActionAuditStatus,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DpAuditKind {
	Regenerated,
	Spent,
	Refunded,
	/// Credited by an effect, such as an instant deployment-point skill.
	Granted,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DpAuditEntry {
	pub at: SimTick,
	pub kind: DpAuditKind,
	pub before: u32,
	pub amount: u32,
	pub after: u32,
	pub action_index: Option<usize>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedDpAudit {
	pub initial: u32,
	pub final_amount: u32,
	pub entries: Vec<DpAuditEntry>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BlockEndReason {
	EnemyDeath,
	BlockerDeath,
	Retreat,
	BlockCountChange,
	BlockTargetChange,
	Shift,
	Goal,
	BattleDeadline,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockAuditEntry {
	pub enemy: EnemyInstanceId,
	pub blocker: BattleActorId,
	pub started_at: SimTick,
	pub ended_at: Option<SimTick>,
	pub end_reason: Option<BlockEndReason>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedEnemyAssessment {
	pub id: EnemyInstanceId,
	pub enemy_key: String,
	pub spawned_at: Option<SimTick>,
	pub deadline_at: Option<SimTick>,
	pub max_hp: ExpectedValue,
	pub remaining_hp: ExpectedValue,
	pub expected_damage_taken: ExpectedValue,
	pub death_at: Option<SimTick>,
	pub leaked_at: Option<SimTick>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedOperatorAssessment {
	pub id: OperatorInstanceId,
	pub operator_key: String,
	pub max_hp: ExpectedValue,
	/// HP remaining in the most recent deployment episode.
	pub remaining_hp: ExpectedValue,
	/// Start of the most recent deployment episode.
	pub deployed_at: Option<SimTick>,
	/// Death of the most recent episode, or `None` when it stayed alive or was retreated.
	pub death_at: Option<SimTick>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedTerminalStatus {
	Victory,
	GoalLeak { enemies: Vec<EnemyInstanceId> },
	Deadline { alive_enemies: Vec<EnemyInstanceId> },
	SimulationIncomplete,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ReplayTerminalMode {
	StopOnFirstLeak,
	ContinueUntilAllResolved,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ExpectedBattleOutcome {
	status: ExpectedTerminalStatus,
	enemies: BTreeMap<EnemyInstanceId, ExpectedEnemyAssessment>,
	operators: BTreeMap<OperatorInstanceId, ExpectedOperatorAssessment>,
	anchors: BTreeMap<BattleTimeAnchor, SimTick>,
	dp: ExpectedDpAudit,
	actions: Vec<GuideActionAudit>,
	blocks: Vec<BlockAuditEntry>,
}

/// Deterministic expected-value result. Deliberately has no probability or infeasibility field.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedValueAssessment {
	status: ExpectedTerminalStatus,
	enemies: BTreeMap<EnemyInstanceId, ExpectedEnemyAssessment>,
	operators: BTreeMap<OperatorInstanceId, ExpectedOperatorAssessment>,
	anchors: BTreeMap<BattleTimeAnchor, SimTick>,
	dp: ExpectedDpAudit,
	actions: Vec<GuideActionAudit>,
	blocks: Vec<BlockAuditEntry>,
	trace: Trace<ExpectedCommand, ExpectedEvent>,
	issues: Vec<SimulationIssue>,
}

/// Success audit for defensive RNG in one complete battlefield replay.
///
/// Probabilistic damage dealt to enemies is intentionally collapsed to expected damage. Only
/// probabilistic damage received by one friendly actor branches the simulation. Native
/// probabilities determine legality/ranking; millionths are retained only for stable display.
#[derive(Clone, Debug, PartialEq)]
pub struct DefensiveProbabilityAssessment {
	verified: bool,
	success_probability: Option<f64>,
	covered_probability_mass: Option<f64>,
	success_probability_millionths: u32,
	covered_probability_millionths: u32,
	processed_steps: usize,
	peak_branches: usize,
	issues: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProbabilityMeasurement {
	exact: Option<f64>,
	millionths: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReplayEvaluationEffort {
	processed_steps: usize,
	peak_branches: usize,
}

impl DefensiveProbabilityAssessment {
	pub const SCALE: u32 = 1_000_000;

	#[must_use]
	pub const fn verified(&self) -> bool {
		self.verified
	}

	#[must_use]
	pub const fn success_probability(&self) -> Option<f64> {
		self.success_probability
	}

	#[must_use]
	pub const fn covered_probability_mass(&self) -> Option<f64> {
		self.covered_probability_mass
	}

	#[must_use]
	pub const fn success_probability_millionths(&self) -> u32 {
		self.success_probability_millionths
	}

	#[must_use]
	pub const fn covered_probability_millionths(&self) -> u32 {
		self.covered_probability_millionths
	}

	#[must_use]
	pub const fn processed_steps(&self) -> usize {
		self.processed_steps
	}

	#[must_use]
	pub const fn peak_branches(&self) -> usize {
		self.peak_branches
	}

	#[must_use]
	pub fn issues(&self) -> &[String] {
		&self.issues
	}

	fn new(
		verified: bool,
		success: ProbabilityMeasurement,
		covered: ProbabilityMeasurement,
		effort: ReplayEvaluationEffort,
		issues: Vec<String>,
	) -> Self {
		Self {
			verified,
			success_probability: success.exact,
			covered_probability_mass: covered.exact,
			success_probability_millionths: success.millionths,
			covered_probability_millionths: covered.millionths,
			processed_steps: effort.processed_steps,
			peak_branches: effort.peak_branches,
			issues,
		}
	}
}

impl ExpectedValueAssessment {
	#[must_use]
	pub const fn status(&self) -> &ExpectedTerminalStatus {
		&self.status
	}

	#[must_use]
	pub const fn enemies(&self) -> &BTreeMap<EnemyInstanceId, ExpectedEnemyAssessment> {
		&self.enemies
	}

	#[must_use]
	pub const fn operators(&self) -> &BTreeMap<OperatorInstanceId, ExpectedOperatorAssessment> {
		&self.operators
	}

	#[must_use]
	pub const fn anchors(&self) -> &BTreeMap<BattleTimeAnchor, SimTick> {
		&self.anchors
	}

	#[must_use]
	pub const fn dp_audit(&self) -> &ExpectedDpAudit {
		&self.dp
	}

	#[must_use]
	pub fn action_audit(&self) -> &[GuideActionAudit] {
		&self.actions
	}

	#[must_use]
	pub fn block_audit(&self) -> &[BlockAuditEntry] {
		&self.blocks
	}

	#[must_use]
	pub const fn trace(&self) -> &Trace<ExpectedCommand, ExpectedEvent> {
		&self.trace
	}

	#[must_use]
	pub fn issues(&self) -> &[SimulationIssue] {
		&self.issues
	}

	fn new(
		outcome: ExpectedBattleOutcome,
		trace: Trace<ExpectedCommand, ExpectedEvent>,
		issues: Vec<SimulationIssue>,
	) -> Self {
		Self {
			status: outcome.status,
			enemies: outcome.enemies,
			operators: outcome.operators,
			anchors: outcome.anchors,
			dp: outcome.dp,
			actions: outcome.actions,
			blocks: outcome.blocks,
			trace,
			issues,
		}
	}
}

const DP_PRIORITY: i32 = -60;
const BATTLE_START_PRIORITY: i32 = -59;
const PREPLACED_PRIORITY: i32 = -55;
const GUIDE_PRIORITY: i32 = -50;
const SKILL_PRIORITY: i32 = -40;
const SPAWN_PRIORITY: i32 = -30;
const BEFORE_LETHAL_EFFECT_PRIORITY: i32 = -21;
const EFFECT_PRIORITY: i32 = -20;
const BLOCK_RECONCILE_PRIORITY: i32 = -19;
const TRIGGER_PRIORITY: i32 = -18;
const SKILL_DISCARD_END_PRIORITY: i32 = -17;
const DEATH_PRIORITY: i32 = -10;
const REDEPLOY_PRIORITY: i32 = -9;
const AXIS_PROJECTILE_PRIORITY: i32 = -5;
const AXIS_END_PRIORITY: i32 = -4;
const ATTACK_PRIORITY: i32 = 0;
const MOVE_PRIORITY: i32 = 10;
const DEADLINE_PRIORITY: i32 = 100;
const INSTANT_SKILL_END_PRIORITY: i32 = DEADLINE_PRIORITY - 1;
const TERMINAL_DRAIN_PRIORITY: i32 = i32::MAX;

const STAGE_ENTITY: EntityId = EntityId::new(0);
const OPERATOR_ENTITY_OFFSET: u64 = 10_000_000;
const ENEMY_ENTITY_OFFSET: u64 = 20_000_000;
const SUMMON_ENTITY_OFFSET: u64 = 30_000_000;
const GUIDE_ENTITY_OFFSET: u64 = 40_000_000;

/// Compile reviewed stage, enemy, skill, talent, and guide facts into a deterministic battle.
///
/// Every reached unknown mechanic is returned in one sorted report before simulation begins.
pub fn compile_battle(
	facts: &ArkBattleFacts,
	guide: &MaterializedGuide,
) -> Result<CompiledArkBattle, MechanicSupportReport> {
	let mut normalized_facts: ArkBattleFacts = facts.clone();
	let mut normalized_guide: MaterializedGuide = guide.clone();
	let mut issues: Vec<BattleCompileIssue> = validate_compile_input(facts, guide);
	let unsupported: Vec<UnsupportedMechanicEntry> =
		collect_unsupported_mechanics(facts, guide, &mut issues);
	normalize_battle_effects(&mut normalized_facts, &mut normalized_guide, &mut issues);

	if unsupported.is_empty() && issues.is_empty() {
		Ok(CompiledArkBattle {
			facts: normalized_facts,
			guide: normalized_guide,
		})
	} else {
		issues.sort();
		issues.dedup();
		Err(MechanicSupportReport::from_parts(unsupported, issues))
	}
}

/// Replay one supported battle through `sim_core` using deterministic expected effects.
#[must_use]
pub fn replay_expected(
	battle: &CompiledArkBattle,
	policy: MaterializedGuidePolicy,
) -> ExpectedValueAssessment {
	replay_expected_with_terminal_mode(battle, policy, ReplayTerminalMode::StopOnFirstLeak)
}

/// Replay until every enemy is defeated or leaked, preserving later waves after the first leak.
///
/// This mode is intended for counterfactual planning audits. Normal strategy validation should
/// stop at the first failure through [`replay_expected`].
#[must_use]
pub fn replay_expected_to_resolution(
	battle: &CompiledArkBattle,
	policy: MaterializedGuidePolicy,
) -> ExpectedValueAssessment {
	replay_expected_with_terminal_mode(battle, policy, ReplayTerminalMode::ContinueUntilAllResolved)
}

fn replay_expected_with_terminal_mode(
	battle: &CompiledArkBattle,
	policy: MaterializedGuidePolicy,
	terminal_mode: ReplayTerminalMode,
) -> ExpectedValueAssessment {
	let scenario = ExpectedReplayScenario {
		battle: battle.clone(),
		policy,
		mode: ReplayDamageMode::DeterministicExpected,
		terminal_mode,
	};
	let result = Simulator::default().replay(
		&ExpectedCombatModel,
		&scenario,
		ExpectedGuidePolicy,
		SimulationSeed::new(0),
	);
	let outcome: ExpectedBattleOutcome =
		result.outcome.unwrap_or_else(|| incomplete_outcome(battle));
	ExpectedValueAssessment::new(outcome, result.trace, result.issues)
}

/// Evaluate complete-strategy success probability from defensive damage RNG.
///
/// Probabilistic outgoing damage remains deterministic expected damage. A probabilistic damage
/// effect targeting exactly one friendly actor branches into hit and dodge outcomes.
#[must_use]
pub fn evaluate_defensive_probability(
	battle: &CompiledArkBattle,
	policy: MaterializedGuidePolicy,
) -> DefensiveProbabilityAssessment {
	let scenario = ExpectedReplayScenario {
		battle: battle.clone(),
		policy,
		mode: ReplayDamageMode::DefensiveProbability,
		terminal_mode: ReplayTerminalMode::StopOnFirstLeak,
	};
	let result =
		Simulator::default().evaluate(&ExpectedCombatModel, &scenario, ExpectedGuidePolicy);
	let verified: bool = result.verification == Verification::Verified;
	let mut issues: Vec<String> = result
		.issues
		.iter()
		.map(|issue| format!("{issue:?}"))
		.collect();
	let success_probability: Option<f64> = normalized_probability(result.success_probability);
	let success_probability_millionths: u32 = success_probability
		.map(probability_millionths)
		.unwrap_or_else(|| {
			issues.push(format!(
				"invalid success probability {}",
				result.success_probability
			));
			0
		});
	let covered_probability_mass: Option<f64> =
		normalized_probability(result.covered_probability_mass);
	let covered_probability_millionths: u32 = covered_probability_mass
		.map(probability_millionths)
		.unwrap_or_else(|| {
			issues.push(format!(
				"invalid covered probability mass {}",
				result.covered_probability_mass
			));
			0
		});
	DefensiveProbabilityAssessment::new(
		verified && issues.is_empty(),
		ProbabilityMeasurement {
			exact: success_probability,
			millionths: success_probability_millionths,
		},
		ProbabilityMeasurement {
			exact: covered_probability_mass,
			millionths: covered_probability_millionths,
		},
		ReplayEvaluationEffort {
			processed_steps: result.processed_steps,
			peak_branches: result.peak_branches,
		},
		issues,
	)
}

fn normalized_probability(probability: f64) -> Option<f64> {
	if !probability.is_finite() || !(-1e-12..=1.0 + 1e-12).contains(&probability) {
		return None;
	}
	Some(probability.clamp(0.0, 1.0))
}

fn probability_millionths(probability: f64) -> u32 {
	let scaled: f64 = probability * f64::from(DefensiveProbabilityAssessment::SCALE);
	scaled.round() as u32
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ReplayDamageMode {
	DeterministicExpected,
	DefensiveProbability,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ExpectedReplayScenario {
	battle: CompiledArkBattle,
	policy: MaterializedGuidePolicy,
	mode: ReplayDamageMode,
	terminal_mode: ReplayTerminalMode,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ExpectedCombatModel;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ExpectedGuidePolicy;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ExpectedObservation {
	command: Option<ExpectedCommand>,
}

impl Policy<ExpectedCombatModel> for ExpectedGuidePolicy {
	type PolicyKey = ();

	fn policy_key(&self) -> Self::PolicyKey {}

	fn next_action(
		&mut self,
		observation: &ExpectedObservation,
	) -> PolicyDecision<ExpectedCommand> {
		observation
			.command
			.clone()
			.map_or(PolicyDecision::Wait, PolicyDecision::Command)
	}
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ActiveModifierKind {
	Stat(StatModifier),
	ConditionalStat(ConditionalStatModifier),
	ArtsResistanceAddition(ExpectedValue),
	NormalAttackSuppression,
	Shelter(ExpectedValue),
	MovementSpeedReduction(ExpectedValue),
	AttackCadence(AttackCadenceModifier),
	AttackSpeed(AttackSpeedModifier),
	AttackRange(AttackRangeModifier),
	TargetCount(TargetCountModifier),
	HitCount(HitCountModifier),
	Blocking(BlockCountModifier),
	BlockTarget(BlockTargetModifier),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveModifier {
	id: u64,
	kind: ActiveModifierKind,
	buff_id: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveStatus {
	spec: StatusEffect,
	generation: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RuntimeActor {
	hp: ExpectedValue,
	cell: GridPosition,
	visible: bool,
	facing: Facing,
	attack_count: u32,
	attack_generation: u32,
	modifiers: Vec<ActiveModifier>,
	statuses: BTreeMap<StatusKind, ActiveStatus>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeployedOperator {
	build: OperatorBuildFacts,
	runtime: RuntimeActor,
	blocking: BTreeSet<EnemyInstanceId>,
	sp: ExpectedValue,
	skill_activation_pending: bool,
	skill_active: bool,
	skill_activation_count: u32,
	skill_discard_pending: bool,
	sp_charge_generation: u32,
	actual_paid_cost: u32,
	deployment_generation: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveEnemy {
	facts: EnemyFacts,
	spawn: EnemySpawnFacts,
	runtime: RuntimeActor,
	path_index: usize,
	blocked_by: Option<BattleActorId>,
	first_blocked_attack_pending: bool,
	axis_direct_hit: Option<ActiveAxisDirectHit>,
	stealth_recovery_generation: u32,
	stealth_recovery_pending: bool,
	move_generation: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveAxisDirectHit {
	facts: EnemyAxisDirectHitFacts,
	generation: u32,
	active_until: Option<SimTick>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveSummon {
	template: SummonTemplate,
	runtime: RuntimeActor,
	blocking: BTreeSet<EnemyInstanceId>,
	ownership: ActiveSummonOwnership,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ActiveSummonOwnership {
	Independent,
	SourceActor(RuntimeSource),
	SkillActivation(RuntimeSource),
	TriggeringTileEffect { tile_effect_id: u64 },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RuntimeSource {
	actor: BattleActorId,
	deployment_generation: Option<u32>,
	faction: Option<Faction>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveListener {
	source: RuntimeSource,
	spec: ReplayListener,
	cells: BTreeSet<GridPosition>,
	generation: u32,
	fired: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveBuff {
	source: RuntimeSource,
	target: BattleActorId,
	target_deployment_generation: Option<u32>,
	spec: RuntimeBuff,
	stacks: u16,
	generation: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveConditionalAura {
	source: RuntimeSource,
	spec: RuntimeConditionalAura,
	active: bool,
	target_buffs: BTreeMap<(OperatorInstanceId, u32), u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveTileEffect {
	source: RuntimeSource,
	spec: RuntimeTileEffect,
	cell: GridPosition,
	layers: u16,
	persistence: TilePersistence,
	generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ObservedTileLayerTransition {
	LayersAdded {
		previous_layers: u16,
		current_layers: u16,
	},
	SaturatedAttempt {
		at_layers: u16,
	},
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BoundTileLayerThresholdReaction {
	source: RuntimeSource,
	owner: TileLayerReactionOwner,
	reaction: RuntimeTileLayerThresholdReaction,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveTileFieldOverlay {
	source: RuntimeSource,
	spec: RuntimeTileFieldOverlay,
	generation: u32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveSkillInactiveStance {
	source: RuntimeSource,
	spec: RuntimeSkillInactiveStance,
	active: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActiveStoredAttackReservation {
	mechanic: MechanicId,
	source: RuntimeSource,
	target: StoredAttackTargetRef,
	facts: StoredAttackReplacementFacts,
	spent_sp: ExpectedValue,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TriggerContext {
	kind: ReplayTriggerKind,
	actor: BattleActorId,
	effect_source: BattleActorId,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ExpectedState {
	scenario: ExpectedReplayScenario,
	dp: u32,
	next_guide_action: usize,
	guide_ready: bool,
	skill_daemon_enabled: bool,
	deployed: BTreeMap<OperatorInstanceId, DeployedOperator>,
	deployment_counts: BTreeMap<OperatorInstanceId, u32>,
	redeployment_ready_at: BTreeMap<OperatorInstanceId, SimTick>,
	next_operator_attack_generation: BTreeMap<OperatorInstanceId, u32>,
	operator_status_generations: BTreeMap<(OperatorInstanceId, StatusKind), u32>,
	enemies: BTreeMap<EnemyInstanceId, ActiveEnemy>,
	summons: BTreeMap<u32, ActiveSummon>,
	anchors: BTreeMap<BattleTimeAnchor, SimTick>,
	scheduled_wave_bounds: BTreeSet<(WaveId, bool)>,
	started_waves: BTreeSet<WaveId>,
	defeated: BTreeSet<EnemyInstanceId>,
	leaked: BTreeSet<EnemyInstanceId>,
	deadline_reached: bool,
	next_modifier_id: u64,
	next_summon_id: u32,
	next_choice_id: u64,
	next_runtime_effect_id: u64,
	listeners: BTreeMap<u64, ActiveListener>,
	buffs: BTreeMap<u64, ActiveBuff>,
	conditional_auras: BTreeMap<u64, ActiveConditionalAura>,
	tile_effects: BTreeMap<u64, ActiveTileEffect>,
	tile_field_overlays: BTreeMap<u64, ActiveTileFieldOverlay>,
	skill_inactive_stances: BTreeMap<u64, ActiveSkillInactiveStance>,
	stored_attack_reservations: BTreeMap<u64, ActiveStoredAttackReservation>,
	pending_death_sources: BTreeMap<BattleActorId, BattleActorId>,
	pending_enemy_death_drains: BTreeSet<EnemyInstanceId>,
	enemy_reports: BTreeMap<EnemyInstanceId, ExpectedEnemyAssessment>,
	operator_reports: BTreeMap<OperatorInstanceId, ExpectedOperatorAssessment>,
	dp_entries: Vec<DpAuditEntry>,
	action_audits: Vec<GuideActionAudit>,
	block_audits: Vec<BlockAuditEntry>,
}

type InitialAssessments = (
	BTreeMap<EnemyInstanceId, ExpectedEnemyAssessment>,
	BTreeMap<OperatorInstanceId, ExpectedOperatorAssessment>,
);

impl SimulationModel for ExpectedCombatModel {
	type Scenario = ExpectedReplayScenario;
	type State = ExpectedState;
	type StateKey = ExpectedState;
	type Command = ExpectedCommand;
	type Observation = ExpectedObservation;
	type Event = ExpectedEvent;
	type Outcome = ExpectedBattleOutcome;

	fn initialize(
		&self,
		scenario: &Self::Scenario,
	) -> Result<Transition<Self::State, Self::Event>, ModelError> {
		let battle: &CompiledArkBattle = &scenario.battle;
		let (enemy_reports, operator_reports) = initial_assessments(battle)?;
		let action_audits: Vec<GuideActionAudit> = battle
			.guide
			.actions
			.iter()
			.enumerate()
			.map(|(action_index, action)| GuideActionAudit {
				action_index,
				kind: guide_action_kind(action),
				requested_at: action.requested_at(),
				executed_at: None,
				status: ActionAuditStatus::SkippedNotReady,
			})
			.collect();
		let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
		if battle.facts.dp.regen_amount > 0 {
			events.push(EventRequest::new(
				SimTick::new(battle.facts.dp.regen_interval_ticks),
				DP_PRIORITY,
				STAGE_ENTITY,
				ExpectedEvent::DpRegen,
			));
		}
		if let Some(first) = battle.guide.actions.first() {
			events.push(guide_ready_event(0, first, first.requested_at())?);
		}
		for build in &battle.guide.builds {
			events.extend(effect_events_with_priority(
				BattleActorId::Operator(build.id),
				None,
				&build.on_battle_start,
				Vec::new(),
				BATTLE_START_PRIORITY,
				SimTick::new(0),
			));
		}
		for preplaced in &battle.facts.preplaced_actors {
			events.push(EventRequest::new(
				SimTick::new(0),
				PREPLACED_PRIORITY,
				summon_entity(preplaced.instance),
				ExpectedEvent::SummonSpawn {
					source: BattleActorId::Stage,
					template: preplaced.template,
					instance: preplaced.instance,
					cell: preplaced.cell,
				},
			));
		}
		for stage_effect in &battle.facts.stage_effects {
			events.push(stage_effect_event(stage_effect, stage_effect.at));
		}
		if let Some(end_at) = battle.facts.end_at {
			events.push(EventRequest::new(
				end_at,
				DEADLINE_PRIORITY,
				STAGE_ENTITY,
				ExpectedEvent::BattleDeadline,
			));
		}
		let mut state = ExpectedState {
			scenario: scenario.clone(),
			dp: battle.facts.dp.initial,
			next_guide_action: 0,
			guide_ready: false,
			skill_daemon_enabled: false,
			deployed: BTreeMap::new(),
			deployment_counts: BTreeMap::new(),
			redeployment_ready_at: BTreeMap::new(),
			next_operator_attack_generation: BTreeMap::new(),
			operator_status_generations: BTreeMap::new(),
			enemies: BTreeMap::new(),
			summons: BTreeMap::new(),
			anchors: [(BattleTimeAnchor::BattleStarted, SimTick::new(0))]
				.into_iter()
				.collect(),
			scheduled_wave_bounds: BTreeSet::new(),
			started_waves: BTreeSet::new(),
			defeated: BTreeSet::new(),
			leaked: BTreeSet::new(),
			deadline_reached: false,
			next_modifier_id: 0,
			next_summon_id: battle
				.facts
				.preplaced_actors
				.iter()
				.map(|actor| actor.instance)
				.max()
				.map_or(0, |maximum| maximum.saturating_add(1)),
			next_choice_id: 0,
			next_runtime_effect_id: 0,
			listeners: BTreeMap::new(),
			buffs: BTreeMap::new(),
			conditional_auras: BTreeMap::new(),
			tile_effects: BTreeMap::new(),
			tile_field_overlays: BTreeMap::new(),
			skill_inactive_stances: BTreeMap::new(),
			stored_attack_reservations: BTreeMap::new(),
			pending_death_sources: BTreeMap::new(),
			pending_enemy_death_drains: BTreeSet::new(),
			enemy_reports,
			operator_reports,
			dp_entries: Vec::new(),
			action_audits,
			block_audits: Vec::new(),
		};
		events.extend(schedule_resolved_wave_bounds(&mut state)?);
		Ok(Transition::new(state, events))
	}

	fn observe(&self, state: &Self::State, at: SimTick) -> Self::Observation {
		let command: Option<ExpectedCommand> = if state.guide_ready {
			state
				.scenario
				.battle
				.guide
				.actions
				.get(state.next_guide_action)
				.and_then(|action| command_for_action(state, action, at))
		} else {
			None
		};
		ExpectedObservation { command }
	}

	fn apply_command(
		&self,
		state: &Self::State,
		command: &Self::Command,
		at: SimTick,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		let mut next: ExpectedState = state.clone();
		let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
		match command {
			ExpectedCommand::Deploy {
				action_index,
				operator,
				cell,
				facing,
			} => {
				validate_current_command(&next, *action_index)?;
				events.extend(deploy_operator(&mut next, *operator, *cell, *facing, at)?);
				mark_action_executed(&mut next, *action_index, at);
			}
			ExpectedCommand::ActivateSkill {
				action_index,
				operator,
			} => {
				validate_current_command(&next, *action_index)?;
				let (skill, deployment_generation) = next
					.deployed
					.get(operator)
					.and_then(|deployed| {
						deployed
							.build
							.skill
							.as_ref()
							.cloned()
							.map(|skill| (skill, deployed.deployment_generation))
					})
					.ok_or_else(|| ModelError::invalid_command("operator has no selected skill"))?;
				if skill.activation != SkillActivationRule::Manual
					|| !skill_is_ready(&next, *operator)
				{
					return Err(ModelError::invalid_command("manual skill is not ready"));
				}
				next.deployed
					.get_mut(operator)
					.expect("skill owner was checked")
					.skill_activation_pending = true;
				events.push(skill_activation_event(
					*operator,
					skill.id,
					deployment_generation,
					SkillActivationOrigin::GuideAction,
					at,
				));
				mark_action_executed(&mut next, *action_index, at);
			}
			ExpectedCommand::DiscardSkill {
				action_index,
				operator,
			} => {
				validate_current_command(&next, *action_index)?;
				if !skill_can_be_manually_discarded(&next, *operator) {
					return Err(ModelError::invalid_command(
						"active skill cannot be manually discarded",
					));
				}
				let deployed = next
					.deployed
					.get_mut(operator)
					.expect("discardable skill owner was checked");
				deployed.skill_discard_pending = true;
				events.push(skill_discard_event(
					*operator,
					deployed.deployment_generation,
					deployed.sp_charge_generation,
					at,
				));
				mark_action_executed(&mut next, *action_index, at);
			}
			ExpectedCommand::Retreat {
				action_index,
				operator,
			} => {
				validate_current_command(&next, *action_index)?;
				events.extend(retreat_operator(&mut next, *operator, at)?);
				mark_action_executed(&mut next, *action_index, at);
			}
		}
		next.guide_ready = false;
		events.extend(advance_guide(&mut next, at)?);
		Ok(ModelStep::Deterministic(Transition::new(next, events)))
	}

	fn apply_event(
		&self,
		state: &Self::State,
		scheduled: &ScheduledEvent<Self::Event>,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		let at: SimTick = scheduled.at();
		if let ExpectedEvent::ResolveEffect {
			source,
			source_deployment_generation,
			targets,
			effect,
		} = scheduled.event()
		{
			if live_operator_source_is_stale(state, *source, *source_deployment_generation) {
				return Ok(ModelStep::Deterministic(Transition::new(
					state.clone(),
					Vec::new(),
				)));
			}
			return resolve_effect_step(
				state.clone(),
				*source,
				*source_deployment_generation,
				targets,
				effect,
				scheduled.priority(),
				at,
			);
		}
		let mut next: ExpectedState = state.clone();
		let events: Vec<EventRequest<ExpectedEvent>> = match scheduled.event() {
			ExpectedEvent::DpRegen => apply_dp_regen(&mut next, at)?,
			ExpectedEvent::GuideActionReady { action_index, kind } => {
				apply_guide_ready(&mut next, *action_index, *kind, at)?
			}
			ExpectedEvent::OperatorRedeploymentReady { operator } => {
				apply_operator_redeployment_ready(&mut next, *operator, at);
				Vec::new()
			}
			ExpectedEvent::WaveStart { wave, forced } => start_wave(&mut next, *wave, *forced, at)?,
			ExpectedEvent::EnemySpawn { enemy } => spawn_enemy(&mut next, *enemy, at)?,
			ExpectedEvent::Move { enemy, generation } => {
				move_enemy(&mut next, *enemy, *generation, at)?
			}
			ExpectedEvent::NormalAttack { actor, generation } => {
				apply_normal_attack(&mut next, *actor, *generation, at)?
			}
			ExpectedEvent::StoredAttackReplacementDelivery {
				reservation_id,
				source,
				target,
			} => apply_stored_attack_replacement_delivery(
				&mut next,
				*reservation_id,
				*source,
				*target,
				at,
			)?,
			ExpectedEvent::AxisDirectHit {
				enemy,
				generation,
				phase,
			} => apply_axis_direct_hit(&mut next, *enemy, *generation, *phase, at)?,
			ExpectedEvent::StealthRecovered { enemy, generation } => {
				apply_stealth_recovered(&mut next, *enemy, *generation);
				Vec::new()
			}
			ExpectedEvent::SpPulse {
				operator,
				deployment_generation,
				charge_generation,
				source,
			} => apply_sp_pulse(
				&mut next,
				*operator,
				*deployment_generation,
				*charge_generation,
				*source,
				at,
			)?,
			ExpectedEvent::SkillActivation {
				operator,
				mechanic,
				deployment_generation,
				..
			} => activate_skill(&mut next, *operator, mechanic, *deployment_generation, at)?,
			ExpectedEvent::SkillDiscarded {
				operator,
				deployment_generation,
				charge_generation,
			} => apply_skill_discarded(
				&mut next,
				*operator,
				*deployment_generation,
				*charge_generation,
				at,
			)?,
			ExpectedEvent::SkillEnded {
				operator,
				deployment_generation,
				charge_generation,
				reason,
			} => apply_skill_ended(
				&mut next,
				*operator,
				*deployment_generation,
				*charge_generation,
				*reason,
				at,
			)?,
			ExpectedEvent::FireTrigger {
				kind,
				actor,
				effect_source,
			} => fire_trigger(
				&mut next,
				TriggerContext {
					kind: *kind,
					actor: *actor,
					effect_source: *effect_source,
				},
				at,
			)?,
			ExpectedEvent::ListenerPulse {
				listener_id,
				generation,
			} => apply_listener_pulse(&mut next, *listener_id, *generation, at)?,
			ExpectedEvent::BuffExpired {
				actor,
				buff_id,
				generation,
			} => expire_buff(&mut next, *actor, *buff_id, *generation, at)?,
			ExpectedEvent::TileEffectPulse {
				tile_effect_id,
				generation,
			} => apply_tile_effect_pulse(&mut next, *tile_effect_id, *generation, at)?,
			ExpectedEvent::TileEffectExpired {
				tile_effect_id,
				generation,
			} => remove_tile_effect_by_id(&mut next, *tile_effect_id, Some(*generation), None, at)?,
			ExpectedEvent::TileFieldOverlayPulse {
				overlay_id,
				generation,
			} => apply_tile_field_overlay_pulse(&mut next, *overlay_id, *generation, at)?,
			ExpectedEvent::ConditionalAuraPulse {
				aura_id,
				target,
				target_deployment_generation,
				buff_id,
			} => apply_conditional_aura_pulse(
				&mut next,
				*aura_id,
				*target,
				*target_deployment_generation,
				*buff_id,
				at,
			)?,
			ExpectedEvent::TileLayerPropagationAudit { .. } => Vec::new(),
			ExpectedEvent::ResolveEffect { .. } => unreachable!("resolved above"),
			ExpectedEvent::ModifierExpired { actor, modifier_id } => {
				expire_modifier(&mut next, *actor, *modifier_id, at)?
			}
			ExpectedEvent::ReconcileBlocking {
				actor,
				release_reason,
			} => reconcile_blocking(&mut next, *actor, at, *release_reason)?,
			ExpectedEvent::StatusExpired {
				actor,
				status,
				generation,
			} => expire_status(&mut next, *actor, status, *generation, at)?,
			ExpectedEvent::SummonSpawn {
				source: _,
				template,
				instance,
				cell,
			} => spawn_summon(&mut next, *template, *instance, *cell, at)?,
			ExpectedEvent::TargetCellActorSpawn(event) => {
				spawn_target_cell_actor(&mut next, *event, at)?
			}
			ExpectedEvent::Death {
				actor,
				operator_deployment_generation,
			} => apply_death(&mut next, *actor, *operator_deployment_generation, at)?,
			ExpectedEvent::EnemyDeathEffectsDrained { enemy } => {
				next.pending_enemy_death_drains.remove(enemy);
				Vec::new()
			}
			ExpectedEvent::BattleDeadline => {
				end_all_blocks(&mut next, at, BlockEndReason::BattleDeadline)?;
				next.deadline_reached = true;
				Vec::new()
			}
		};
		Ok(ModelStep::Deterministic(Transition::new(next, events)))
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		if !state.leaked.is_empty()
			&& state.scenario.terminal_mode == ReplayTerminalMode::StopOnFirstLeak
		{
			return Some(outcome_from_state(
				state,
				ExpectedTerminalStatus::GoalLeak {
					enemies: state.leaked.iter().copied().collect(),
				},
			));
		}
		let total: usize = state
			.scenario
			.battle
			.facts
			.waves
			.iter()
			.map(|wave| wave.spawns.len())
			.sum();
		let resolved: usize = state.defeated.len().saturating_add(state.leaked.len());
		if resolved == total && state.pending_enemy_death_drains.is_empty() {
			let status: ExpectedTerminalStatus = if !state.leaked.is_empty() {
				ExpectedTerminalStatus::GoalLeak {
					enemies: state.leaked.iter().copied().collect(),
				}
			} else if has_pending_required_actions(state) {
				ExpectedTerminalStatus::SimulationIncomplete
			} else {
				ExpectedTerminalStatus::Victory
			};
			return Some(outcome_from_state(state, status));
		}
		if proves_inert_blocking_stalemate(state) {
			return Some(outcome_from_state(
				state,
				ExpectedTerminalStatus::SimulationIncomplete,
			));
		}
		(state.deadline_reached && state.pending_enemy_death_drains.is_empty()).then(|| {
			let alive_enemies: Vec<EnemyInstanceId> = state
				.enemy_reports
				.iter()
				.filter_map(|(id, report)| {
					(report.death_at.is_none() && report.leaked_at.is_none()).then_some(*id)
				})
				.collect();
			outcome_from_state(state, ExpectedTerminalStatus::Deadline { alive_enemies })
		})
	}

	fn is_success(&self, outcome: &Self::Outcome) -> bool {
		matches!(outcome.status, ExpectedTerminalStatus::Victory)
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		state.clone()
	}
}

fn has_pending_required_actions(state: &ExpectedState) -> bool {
	state.action_audits.iter().any(|audit| {
		matches!(
			audit.status,
			ActionAuditStatus::WaitedForDp | ActionAuditStatus::SkippedNotReady
		)
	})
}

/// Proves a narrow no-progress cycle without imposing a false route-derived battle deadline.
///
/// This intentionally recognizes only a fully materialized, all-blocked battle whose remaining
/// actors have no attacks, skills, delayed effects, or expirations capable of changing HP or
/// topology. Any uncertainty keeps the replay running.
fn proves_inert_blocking_stalemate(state: &ExpectedState) -> bool {
	let battle = &state.scenario.battle;
	if battle.facts.end_at.is_some()
		|| !battle.facts.stage_effects.is_empty()
		|| state.next_guide_action != battle.guide.actions.len()
		|| state.guide_ready
		|| state.started_waves.len() != battle.facts.waves.len()
		|| state.enemies.is_empty()
	{
		return false;
	}
	if state
		.enemy_reports
		.values()
		.any(|report| report.spawned_at.is_none())
	{
		return false;
	}
	if state.enemies.values().any(|enemy| {
		enemy.blocked_by.is_none()
			|| enemy.runtime.hp == ExpectedValue::ZERO
			|| enemy.axis_direct_hit.is_some()
			|| !runtime_has_no_expirations(&enemy.runtime)
			|| !attack_is_inert(&enemy.facts.attack)
			|| !enemy.facts.on_spawn.is_empty()
	}) {
		return false;
	}
	if state.deployed.values().any(|operator| {
		operator.runtime.hp == ExpectedValue::ZERO
			|| !runtime_has_no_expirations(&operator.runtime)
			|| !attack_is_inert(&operator.build.attack)
			|| operator.build.skill.is_some()
			|| !operator.build.on_deploy.is_empty()
			|| !operator.build.on_death.is_empty()
	}) {
		return false;
	}
	!state.summons.values().any(|summon| {
		summon.runtime.hp == ExpectedValue::ZERO
			|| !runtime_has_no_expirations(&summon.runtime)
			|| !attack_is_inert(&summon.template.attack)
			|| summon.template.lifetime_ticks.is_some()
	})
}

fn runtime_has_no_expirations(runtime: &RuntimeActor) -> bool {
	runtime.modifiers.is_empty() && runtime.statuses.is_empty()
}

fn attack_is_inert(attack: &AttackProfile) -> bool {
	attack.target_count == 0
		&& attack.action == NormalAttackAction::None
		&& attack.extra_effects.is_empty()
}

fn validate_compile_input(
	facts: &ArkBattleFacts,
	guide: &MaterializedGuide,
) -> Vec<BattleCompileIssue> {
	let mut issues: Vec<BattleCompileIssue> = Vec::new();
	if facts.battle_id.trim().is_empty() {
		issues.push(BattleCompileIssue::EmptyBattleId);
	}
	if facts.dp.initial > facts.dp.maximum {
		issues.push(BattleCompileIssue::InvalidDpRules {
			detail: "initial DP exceeds maximum".to_owned(),
		});
	}
	if facts.dp.regen_amount > 0 && facts.dp.regen_interval_ticks == 0 {
		issues.push(BattleCompileIssue::InvalidDpRules {
			detail: "positive DP regeneration requires a non-zero interval".to_owned(),
		});
	}
	if facts.deployment.second_deployment_cost_multiplier < ExpectedValue::ONE {
		issues.push(BattleCompileIssue::InvalidDeploymentRules {
			detail: "second-deployment cost multiplier must be at least one".to_owned(),
		});
	}
	if facts.deployment.later_deployment_cost_multiplier
		< facts.deployment.second_deployment_cost_multiplier
	{
		issues.push(BattleCompileIssue::InvalidDeploymentRules {
			detail:
				"later-deployment cost multiplier must not be below the second-deployment multiplier"
					.to_owned(),
		});
	}
	if facts.deployment.retreat_refund_ratio > ExpectedValue::ONE {
		issues.push(BattleCompileIssue::InvalidDeploymentRules {
			detail: "retreat refund ratio must not exceed one".to_owned(),
		});
	}
	if facts
		.grid
		.ground_cells
		.iter()
		.any(|cell| !cell_in_bounds(*cell, facts.grid.dimensions))
	{
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: "ground cell set contains an out-of-bounds cell".to_owned(),
		});
	}
	let mut path_ids: BTreeSet<PathId> = BTreeSet::new();
	for path in &facts.paths {
		if !path_ids.insert(path.id) {
			issues.push(BattleCompileIssue::DuplicatePath(path.id));
		}
		if !cell_in_bounds(path.start, facts.grid.dimensions) {
			issues.push(BattleCompileIssue::InvalidPath {
				path: path.id,
				detail: "path start is outside the grid".to_owned(),
			});
		}
		if !matches!(
			path.steps.last().map(|step| &step.action),
			Some(BattlePathStepAction::Goal { goal }) if *goal == path.goal
		) {
			issues.push(BattleCompileIssue::InvalidPath {
				path: path.id,
				detail: "path must end in an explicit matching goal step".to_owned(),
			});
		}
		for step in &path.steps {
			let cell: Option<GridPosition> = match step.action {
				BattlePathStepAction::Move { cell, .. }
				| BattlePathStepAction::Reappear { cell, .. } => Some(cell),
				BattlePathStepAction::Wait
				| BattlePathStepAction::Disappear
				| BattlePathStepAction::Goal { .. } => None,
			};
			if cell.is_some_and(|cell| !cell_in_bounds(cell, facts.grid.dimensions)) {
				issues.push(BattleCompileIssue::InvalidPath {
					path: path.id,
					detail: "route step is outside the grid".to_owned(),
				});
			}
		}
	}
	let mut enemy_keys: BTreeSet<&str> = BTreeSet::new();
	for enemy in &facts.enemies {
		if !enemy_keys.insert(enemy.enemy_key.as_str()) {
			issues.push(BattleCompileIssue::DuplicateEnemyDefinition {
				enemy_key: enemy.enemy_key.clone(),
			});
		}
		validate_actor_facts(&enemy.enemy_key, enemy.combat, &enemy.attack, &mut issues);
	}
	let mut spawn_ids: BTreeSet<EnemyInstanceId> = BTreeSet::new();
	let mut wave_ids: BTreeSet<WaveId> = BTreeSet::new();
	for wave in &facts.waves {
		if !wave_ids.insert(wave.id) {
			issues.push(BattleCompileIssue::DuplicateWave(wave.id));
		}
		for spawn in &wave.spawns {
			if !spawn_ids.insert(spawn.id) {
				issues.push(BattleCompileIssue::DuplicateEnemySpawn(spawn.id));
			}
			if !enemy_keys.contains(spawn.enemy_key.as_str()) {
				issues.push(BattleCompileIssue::UnknownEnemyDefinition {
					enemy: spawn.id,
					enemy_key: spawn.enemy_key.clone(),
				});
			}
			if !path_ids.contains(&spawn.path) {
				issues.push(BattleCompileIssue::UnknownPath {
					enemy: spawn.id,
					path: spawn.path,
				});
			}
		}
	}
	for wave in &facts.waves {
		for relative in [Some(wave.start.earliest), wave.start.latest]
			.into_iter()
			.flatten()
		{
			match relative.anchor {
				BattleTimeAnchor::WaveStarted(anchor) | BattleTimeAnchor::WaveCleared(anchor)
					if !wave_ids.contains(&anchor) =>
				{
					issues.push(BattleCompileIssue::InvalidWave {
						wave: wave.id,
						detail: format!("anchor references unknown wave {}", anchor.get()),
					});
				}
				BattleTimeAnchor::EnemySpawned(anchor)
				| BattleTimeAnchor::EnemyDefeated(anchor)
					if !spawn_ids.contains(&anchor) =>
				{
					issues.push(BattleCompileIssue::InvalidWave {
						wave: wave.id,
						detail: format!("anchor references unknown enemy {}", anchor.get()),
					});
				}
				BattleTimeAnchor::BattleStarted
				| BattleTimeAnchor::WaveStarted(_)
				| BattleTimeAnchor::WaveCleared(_)
				| BattleTimeAnchor::EnemySpawned(_)
				| BattleTimeAnchor::EnemyDefeated(_) => {}
			}
		}
	}
	let enemy_waves: BTreeMap<EnemyInstanceId, WaveId> = facts
		.waves
		.iter()
		.flat_map(|wave| wave.spawns.iter().map(move |spawn| (spawn.id, wave.id)))
		.collect();
	let mut reachable_waves: BTreeSet<WaveId> = BTreeSet::new();
	loop {
		let before: usize = reachable_waves.len();
		for wave in &facts.waves {
			let anchors = [Some(wave.start.earliest), wave.start.latest];
			if anchors.into_iter().flatten().any(|relative| {
				anchor_is_reachable(relative.anchor, &reachable_waves, &enemy_waves)
			}) {
				reachable_waves.insert(wave.id);
			}
		}
		if reachable_waves.len() == before {
			break;
		}
	}
	for wave in &facts.waves {
		if !reachable_waves.contains(&wave.id) {
			issues.push(BattleCompileIssue::InvalidWave {
				wave: wave.id,
				detail: "causal start window is not reachable from battle start".to_owned(),
			});
		}
	}
	if spawn_ids.is_empty() {
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: "battle must contain at least one enemy spawn".to_owned(),
		});
	}
	let mut operator_ids: BTreeSet<OperatorInstanceId> = BTreeSet::new();
	for build in &guide.builds {
		if !operator_ids.insert(build.id) {
			issues.push(BattleCompileIssue::DuplicateOperator(build.id));
		}
		if build.classification.profession_category_mask.count_ones() > 1 {
			issues.push(BattleCompileIssue::InvalidCombatFacts {
				detail: format!(
					"operator {:?} has a non-one-hot profession category mask",
					build.operator_key
				),
			});
		}
		if [0, 1, 2].into_iter().any(|deployment_count| {
			deployment_cost_for_count(
				build.deployment_cost,
				deployment_count,
				facts.deployment.second_deployment_cost_multiplier,
				facts.deployment.later_deployment_cost_multiplier,
			)
			.is_none()
		}) {
			issues.push(BattleCompileIssue::InvalidDeploymentRules {
				detail: format!(
					"operator {:?} has a deployment cost that exceeds u32 after a configured \
					 redeployment multiplier",
					build.operator_key
				),
			});
		}
		validate_actor_facts(
			&build.operator_key,
			build.combat,
			&build.attack,
			&mut issues,
		);
		if let Some(skill) = &build.skill {
			if matches!(
				skill.charge,
				SpChargeRule::Time {
					interval_ticks: 0,
					..
				}
			) {
				issues.push(BattleCompileIssue::InvalidCombatFacts {
					detail: format!(
						"operator {:?} has a zero time-SP interval",
						build.operator_key
					),
				});
			}
			if skill
				.periodic_sp_sources
				.iter()
				.any(|source: &PeriodicSpSource| source.interval_ticks == 0)
			{
				issues.push(BattleCompileIssue::InvalidCombatFacts {
					detail: format!(
						"operator {:?} has a zero periodic-SP interval",
						build.operator_key
					),
				});
			}
			if let Some(replacement) = &skill.stored_attack_replacement {
				let invalidation_is_supported = [
					replacement.invalidation.target_death,
					replacement.invalidation.target_disappeared,
					replacement.invalidation.target_out_of_range,
					replacement.invalidation.target_generation_changed,
				]
				.into_iter()
				.all(|action| action != StoredAttackReservationInvalidationAction::Unsupported);
				let valid_shape: bool = skill.activation == SkillActivationRule::Auto
					&& matches!(skill.charge, SpChargeRule::Time { .. })
					&& skill.sp_cost > 0
					&& replacement.max_charges > 0
					&& !replacement.range.is_empty()
					&& replacement.hp_ratio_threshold <= ExpectedValue::ONE
					&& skill
						.sp_cost
						.checked_mul(u32::from(replacement.max_charges))
						.is_some() && invalidation_is_supported;
				if !valid_shape {
					issues.push(BattleCompileIssue::InvalidCombatFacts {
						detail: format!(
							"operator {:?} has an invalid stored-attack replacement skill",
							build.operator_key
						),
					});
				}
			}
		}
	}
	let summon_templates: BTreeMap<SummonTemplateId, &SummonTemplate> = facts
		.summons
		.iter()
		.map(|template| (template.id, template))
		.collect();
	for template in &facts.summons {
		if template.faction == Faction::Hostile {
			issues.push(BattleCompileIssue::UnsupportedHostileSummon {
				template: template.id,
				key: template.key.clone(),
			});
		}
	}
	let mut preplaced_ids: BTreeSet<u32> = BTreeSet::new();
	for preplaced in &facts.preplaced_actors {
		if !preplaced_ids.insert(preplaced.instance) {
			issues.push(BattleCompileIssue::DuplicatePreplacedActor {
				instance: preplaced.instance,
			});
		}
		if !summon_templates.contains_key(&preplaced.template) {
			issues.push(BattleCompileIssue::UnknownSummonTemplate {
				instance: preplaced.instance,
				template: preplaced.template,
			});
		}
		if summon_templates
			.get(&preplaced.template)
			.is_some_and(|template| template.faction == Faction::Hostile)
		{
			issues.push(BattleCompileIssue::UnsupportedHostilePreplacedActor {
				instance: preplaced.instance,
				template: preplaced.template,
			});
		}
		if !cell_in_bounds(preplaced.cell, facts.grid.dimensions) {
			issues.push(BattleCompileIssue::InvalidCombatFacts {
				detail: format!(
					"preplaced actor {} is outside the battle grid",
					preplaced.instance
				),
			});
		}
	}
	let mut previous_at: Option<SimTick> = None;
	for (action_index, action) in guide.actions.iter().enumerate() {
		let requested_at: SimTick = action.requested_at();
		if previous_at.is_some_and(|previous| requested_at < previous) {
			issues.push(BattleCompileIssue::InvalidGuideOrder { action_index });
		}
		if let Some(operator) = guide_action_operator(action) {
			if !operator_ids.contains(&operator) {
				issues.push(BattleCompileIssue::UnknownGuideOperator {
					action_index,
					operator,
				});
			}
		}
		if let MaterializedGuideAction::Deploy { cell, .. } = action {
			if !cell_in_bounds(*cell, facts.grid.dimensions)
				|| !facts.grid.deployable_cells.contains(cell)
			{
				issues.push(BattleCompileIssue::InvalidGuideCell {
					action_index,
					cell: *cell,
				});
			}
		}
		previous_at = Some(requested_at);
	}
	validate_runtime_primitives(facts, guide, &mut issues);
	issues
}

fn validate_runtime_primitives(
	facts: &ArkBattleFacts,
	guide: &MaterializedGuide,
	issues: &mut Vec<BattleCompileIssue>,
) {
	let summon_templates: BTreeMap<SummonTemplateId, &SummonTemplate> = facts
		.summons
		.iter()
		.map(|template| (template.id, template))
		.collect();
	let mut trigger_graph: BTreeMap<ReplayTriggerKind, BTreeSet<ReplayTriggerKind>> =
		BTreeMap::new();
	for scheduled in &facts.stage_effects {
		if scheduled.repeat_interval_ticks == Some(0) {
			issues.push(BattleCompileIssue::InvalidReplayPrimitive {
				mechanic: scheduled.effect.mechanic.clone(),
				detail: "stage repeat interval must be positive".to_owned(),
			});
		}
		validate_runtime_effect(
			&scheduled.effect,
			RuntimeEffectScope::Root,
			facts.grid.dimensions,
			&summon_templates,
			&mut trigger_graph,
			issues,
		);
	}
	for effect in guide
		.builds
		.iter()
		.flat_map(|build| build.on_battle_start.iter())
	{
		validate_runtime_effect(
			effect,
			RuntimeEffectScope::BattleStart,
			facts.grid.dimensions,
			&summon_templates,
			&mut trigger_graph,
			issues,
		);
	}
	for effect in facts
		.enemies
		.iter()
		.flat_map(|enemy| {
			enemy
				.on_spawn
				.iter()
				.chain(enemy.on_death.iter())
				.chain(enemy.attack.extra_effects.iter())
		})
		.chain(guide.builds.iter().flat_map(|build| {
			build
				.on_deploy
				.iter()
				.chain(build.on_death.iter())
				.chain(build.attack.extra_effects.iter())
		}))
		.chain(guide.builds.iter().flat_map(|build| {
			build.skill.as_ref().into_iter().flat_map(|skill| {
				skill.effects.iter().chain(
					skill
						.manual_discard
						.as_ref()
						.into_iter()
						.flat_map(|policy| policy.on_discard.iter()),
				)
			})
		}))
		.chain(
			facts
				.summons
				.iter()
				.flat_map(|summon| summon.attack.extra_effects.iter()),
		) {
		validate_runtime_effect(
			effect,
			RuntimeEffectScope::Root,
			facts.grid.dimensions,
			&summon_templates,
			&mut trigger_graph,
			issues,
		);
	}
	for template in &facts.summons {
		if zero_delay_summon_chain_cycles(
			template.id,
			&summon_templates,
			&mut BTreeSet::new(),
			&mut BTreeSet::new(),
		) {
			issues.push(BattleCompileIssue::InvalidReplayPrimitive {
				mechanic: MechanicId::new(format!("summon_attack:{}", template.key)),
				detail: "zero-delay summon attacks recursively spawn summons".to_owned(),
			});
		}
	}
	if let Some(cycle) = find_trigger_cycle(&trigger_graph) {
		issues.push(BattleCompileIssue::ReplayTriggerCycle { triggers: cycle });
	}
}

fn zero_delay_summon_chain_cycles(
	template_id: SummonTemplateId,
	summon_templates: &BTreeMap<SummonTemplateId, &SummonTemplate>,
	visiting: &mut BTreeSet<SummonTemplateId>,
	visited: &mut BTreeSet<SummonTemplateId>,
) -> bool {
	if visiting.contains(&template_id) {
		return true;
	}
	if visited.contains(&template_id) {
		return false;
	}
	let Some(template) = summon_templates.get(&template_id) else {
		return false;
	};
	if template.attack.first_delay_ticks != 0 || template.attack.target_count == 0 {
		visited.insert(template_id);
		return false;
	}
	visiting.insert(template_id);
	let cycles: bool = template
		.attack
		.extra_effects
		.iter()
		.flat_map(immediate_summon_templates)
		.any(|nested| zero_delay_summon_chain_cycles(nested, summon_templates, visiting, visited));
	visiting.remove(&template_id);
	visited.insert(template_id);
	cycles
}

fn immediate_summon_templates(effect: &TaggedEffect) -> Vec<SummonTemplateId> {
	match &effect.effect {
		ExpectedEffect::Summon(summon) => vec![summon.template],
		ExpectedEffect::AddTileLayer(tile) => tile
			.layer_threshold_reactions
			.iter()
			.map(|reaction| match reaction.effect {
				TileLayerThresholdReactionEffect::SpawnActor(spawn) => spawn.template,
			})
			.collect(),
		ExpectedEffect::Sequence(sequence) => sequence
			.iter()
			.flat_map(immediate_summon_templates)
			.collect(),
		ExpectedEffect::Expected { effect: nested, .. } => {
			immediate_summon_templates(&TaggedEffect {
				mechanic: effect.mechanic.clone(),
				target: effect.target,
				effect: *nested.clone(),
			})
		}
		ExpectedEffect::Damage { .. }
		| ExpectedEffect::SourceStatScaledDamage { .. }
		| ExpectedEffect::Healing { .. }
		| ExpectedEffect::RestoreHpToCurrentMaxRatio { .. }
		| ExpectedEffect::StatModifier(_)
		| ExpectedEffect::ConditionalStatModifier(_)
		| ExpectedEffect::AttackCadence(_)
		| ExpectedEffect::AttackSpeed(_)
		| ExpectedEffect::AttackRange(_)
		| ExpectedEffect::TargetCount(_)
		| ExpectedEffect::HitCount(_)
		| ExpectedEffect::GainSp { .. }
		| ExpectedEffect::GainDp { .. }
		| ExpectedEffect::ActivateSkill
		| ExpectedEffect::ApplyStatus(_)
		| ExpectedEffect::Shift(_)
		| ExpectedEffect::Blocking(_)
		| ExpectedEffect::BlockTarget(_)
		| ExpectedEffect::StartAxisDirectHit(_)
		| ExpectedEffect::RegisterListener(_)
		| ExpectedEffect::RemoveListener { .. }
		| ExpectedEffect::ApplyBuff(_)
		| ExpectedEffect::InstallConditionalAura(_)
		| ExpectedEffect::InstallSkillInactiveStance(_)
		| ExpectedEffect::RemoveBuff { .. }
		| ExpectedEffect::InstallTileFieldOverlay(_)
		| ExpectedEffect::RemoveTileEffect { .. }
		| ExpectedEffect::Death => Vec::new(),
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeEffectScope {
	Root,
	BattleStart,
	DirectListenerEffect,
}

fn validate_runtime_effect(
	tagged: &TaggedEffect,
	scope: RuntimeEffectScope,
	dimensions: MapDimensions,
	summon_templates: &BTreeMap<SummonTemplateId, &SummonTemplate>,
	trigger_graph: &mut BTreeMap<ReplayTriggerKind, BTreeSet<ReplayTriggerKind>>,
	issues: &mut Vec<BattleCompileIssue>,
) {
	if tagged.target == EffectTarget::TriggerEffectSource
		&& scope != RuntimeEffectScope::DirectListenerEffect
	{
		invalid_runtime_primitive(
			tagged,
			"trigger-effect-source target is only valid on a direct listener effect",
			issues,
		);
	}
	match &tagged.effect {
		ExpectedEffect::RegisterListener(listener) => {
			if listener.key.as_str().trim().is_empty() {
				invalid_runtime_primitive(tagged, "listener key must not be empty", issues);
			}
			if matches!(
				listener.trigger,
				ReplayTrigger::Periodic {
					interval_ticks: 0,
					..
				}
			) {
				invalid_runtime_primitive(tagged, "listener interval must be positive", issues);
			}
			if let ReplayTrigger::ActorEnteredCells { cells }
			| ReplayTrigger::ActorLeftCells { cells } = &listener.trigger
			{
				if matches!(
					cells,
					ReplayCellSet::EffectTargetRelative(_)
						| ReplayCellSet::SourceCurrentAttackRangeGroundCells
				) {
					invalid_runtime_primitive(
						tagged,
						"dynamic effect cell sets are only valid for tile effects",
						issues,
					);
				}
				validate_cell_set(tagged, cells, dimensions, issues);
			}
			validate_predicate(tagged, &listener.predicate, issues);
			if listener.repeat == ReplayRepeat::Unlimited {
				let emitted: BTreeSet<ReplayTriggerKind> = listener
					.effects
					.iter()
					.flat_map(|effect| emitted_trigger_kinds(effect, summon_templates))
					.collect();
				trigger_graph
					.entry(listener.trigger.kind())
					.or_default()
					.extend(emitted);
			}
			for nested in &listener.effects {
				validate_runtime_effect(
					nested,
					RuntimeEffectScope::DirectListenerEffect,
					dimensions,
					summon_templates,
					trigger_graph,
					issues,
				);
			}
		}
		ExpectedEffect::ApplyBuff(buff) => {
			if buff.key.as_str().trim().is_empty() || buff.max_stacks == 0 {
				invalid_runtime_primitive(
					tagged,
					"buff key must be non-empty and max_stacks must be positive",
					issues,
				);
			}
			if buff
				.modifiers_per_stack
				.iter()
				.any(buff_modifier_has_duration)
			{
				invalid_runtime_primitive(
					tagged,
					"buff-owned modifiers must not declare independent durations",
					issues,
				);
			}
			if buff.modifiers_per_stack.iter().any(|modifier| {
				matches!(
					modifier,
					BuffModifier::Shelter {
						incoming_damage_multiplier,
					} if *incoming_damage_multiplier > ExpectedValue::ONE
				)
			}) {
				invalid_runtime_primitive(
					tagged,
					"shelter incoming-damage multiplier must not exceed one",
					issues,
				);
			}
			let reduction_per_stack: Option<u128> = buff
				.modifiers_per_stack
				.iter()
				.filter_map(|modifier| match modifier {
					BuffModifier::MovementSpeedReduction { reduction } => {
						Some(u128::from(reduction.scaled()))
					}
					_ => None,
				})
				.try_fold(0_u128, u128::checked_add);
			if reduction_per_stack
				.and_then(|per_stack| per_stack.checked_mul(u128::from(buff.max_stacks)))
				.is_none_or(|total| total >= u128::from(ExpectedValue::SCALE))
			{
				invalid_runtime_primitive(
					tagged,
					"movement-speed reduction must remain below one at maximum stacks",
					issues,
				);
			}
			let attack_speed_per_stack: Option<i64> = buff
				.modifiers_per_stack
				.iter()
				.filter_map(|modifier| match modifier {
					BuffModifier::AttackSpeed(modifier) => Some(i64::from(modifier.delta)),
					_ => None,
				})
				.try_fold(0_i64, i64::checked_add);
			if attack_speed_per_stack
				.and_then(|per_stack| per_stack.checked_mul(i64::from(buff.max_stacks)))
				.and_then(|total| 100_i64.checked_add(total))
				.is_none_or(|denominator| denominator <= 0)
			{
				invalid_runtime_primitive(
					tagged,
					"attack-speed denominator must remain positive at maximum stacks",
					issues,
				);
			}
		}
		ExpectedEffect::InstallConditionalAura(aura) => {
			if tagged.target != EffectTarget::SelfActor {
				invalid_runtime_primitive(
					tagged,
					"conditional aura must target its source actor",
					issues,
				);
			}
			if aura.key.as_str().trim().is_empty() {
				invalid_runtime_primitive(tagged, "conditional aura key must not be empty", issues);
			}
			if aura.target_profession_mask == 0 {
				invalid_runtime_primitive(
					tagged,
					"conditional aura target profession mask must not be zero",
					issues,
				);
			}
			match &aura.condition {
				TeamCompositionCondition::DeployedDistinctProfessions {
					minimum_count,
					profession_mask,
				}
				| TeamCompositionCondition::DeployedSameProfession {
					minimum_count,
					profession_mask,
				} => {
					if *minimum_count == 0 || *profession_mask == 0 {
						invalid_runtime_primitive(
							tagged,
							"deployed team condition requires a positive count and profession mask",
							issues,
						);
					}
				}
				TeamCompositionCondition::FixedRosterGroup {
					minimum_count,
					group_id,
				} => {
					if *minimum_count == 0 || group_id.trim().is_empty() {
						invalid_runtime_primitive(
							tagged,
							"fixed-roster group condition requires a positive count and group ID",
							issues,
						);
					}
				}
			}
			if scope == RuntimeEffectScope::BattleStart
				&& aura.lifetime != RuntimeEffectLifetime::Persistent
			{
				invalid_runtime_primitive(
					tagged,
					"battle-start conditional aura must have persistent lifetime",
					issues,
				);
			}
			if aura.buff.lifetime != aura.lifetime || aura.buff.duration_ticks.is_some() {
				invalid_runtime_primitive(
					tagged,
					"conditional aura buff must share the aura lifetime and have no duration",
					issues,
				);
			}
			if aura.buff.modifiers_per_stack.is_empty() {
				invalid_runtime_primitive(
					tagged,
					"conditional aura buff must contain at least one modifier",
					issues,
				);
			}
			let buff_effect = TaggedEffect {
				mechanic: tagged.mechanic.clone(),
				target: EffectTarget::CurrentTargets,
				effect: ExpectedEffect::ApplyBuff(aura.buff.clone()),
			};
			validate_runtime_effect(
				&buff_effect,
				RuntimeEffectScope::Root,
				dimensions,
				summon_templates,
				trigger_graph,
				issues,
			);
			if let Some(periodic) = &aura.periodic {
				if periodic.interval_ticks == 0 || periodic.effects.is_empty() {
					invalid_runtime_primitive(
						tagged,
						"conditional aura periodic effects require a positive interval and effects",
						issues,
					);
				}
				for nested in &periodic.effects {
					if nested.target != EffectTarget::CurrentTargets {
						invalid_runtime_primitive(
							tagged,
							"conditional aura periodic effects must target current aura targets",
							issues,
						);
					}
					validate_runtime_effect(
						nested,
						RuntimeEffectScope::Root,
						dimensions,
						summon_templates,
						trigger_graph,
						issues,
					);
				}
			}
		}
		ExpectedEffect::InstallSkillInactiveStance(stance) => {
			if tagged.target != EffectTarget::SelfActor {
				invalid_runtime_primitive(
					tagged,
					"skill-inactive stance must target its source actor",
					issues,
				);
			}
			if stance.key.as_str().trim().is_empty() {
				invalid_runtime_primitive(tagged, "stance key must not be empty", issues);
			}
			if !stance.disable_normal_attacks && stance.modifiers.is_empty() {
				invalid_runtime_primitive(
					tagged,
					"stance must change at least one behavior",
					issues,
				);
			}
			if stance.modifiers.iter().any(buff_modifier_has_duration) {
				invalid_runtime_primitive(
					tagged,
					"stance-owned modifiers must not declare independent durations",
					issues,
				);
			}
			if stance.modifiers.iter().any(|modifier| {
				matches!(
					modifier,
					BuffModifier::Shelter {
						incoming_damage_multiplier,
					} if *incoming_damage_multiplier > ExpectedValue::ONE
				)
			}) {
				invalid_runtime_primitive(
					tagged,
					"stance shelter incoming-damage multiplier must not exceed one",
					issues,
				);
			}
			let movement_reduction: Option<u128> = stance
				.modifiers
				.iter()
				.filter_map(|modifier| match modifier {
					BuffModifier::MovementSpeedReduction { reduction } => {
						Some(u128::from(reduction.scaled()))
					}
					_ => None,
				})
				.try_fold(0_u128, u128::checked_add);
			if movement_reduction.is_none_or(|total| total >= u128::from(ExpectedValue::SCALE)) {
				invalid_runtime_primitive(
					tagged,
					"stance movement-speed reduction must remain below one",
					issues,
				);
			}
			let attack_speed_delta: Option<i64> = stance
				.modifiers
				.iter()
				.filter_map(|modifier| match modifier {
					BuffModifier::AttackSpeed(modifier) => Some(i64::from(modifier.delta)),
					_ => None,
				})
				.try_fold(0_i64, i64::checked_add);
			if attack_speed_delta
				.and_then(|delta| 100_i64.checked_add(delta))
				.is_none_or(|denominator| denominator <= 0)
			{
				invalid_runtime_primitive(
					tagged,
					"stance attack-speed denominator must remain positive",
					issues,
				);
			}
		}
		ExpectedEffect::AddTileLayer(tile) => {
			if tile.key.as_str().trim().is_empty() || tile.max_layers == 0 {
				invalid_runtime_primitive(
					tagged,
					"tile key must be non-empty and max_layers must be positive",
					issues,
				);
			}
			validate_cell_set(tagged, &tile.cells, dimensions, issues);
			if tile
				.periodic
				.as_ref()
				.is_some_and(|periodic| periodic.interval_ticks == 0)
			{
				invalid_runtime_primitive(tagged, "tile interval must be positive", issues);
			}
			for reaction in &tile.layer_threshold_reactions {
				if reaction.threshold == 0 || reaction.threshold > tile.max_layers {
					invalid_runtime_primitive(
						tagged,
						"tile reaction threshold must be positive and at most max_layers",
						issues,
					);
				}
				if reaction.transition == TileLayerThresholdTransition::SaturatedAttempt
					&& reaction.threshold != tile.max_layers
				{
					invalid_runtime_primitive(
						tagged,
						"saturated tile reaction threshold must equal max_layers",
						issues,
					);
				}
				let TileLayerThresholdReactionEffect::SpawnActor(spawn) = reaction.effect;
				if !summon_templates.contains_key(&spawn.template) {
					invalid_runtime_primitive(
						tagged,
						"tile reaction summon template does not exist",
						issues,
					);
				}
				if spawn.ownership == TargetCellSpawnOwnership::SourceActor
					&& tile.lifetime == RuntimeEffectLifetime::Persistent
				{
					invalid_runtime_primitive(
						tagged,
						"source-owned tile reaction actor requires a live source lifetime",
						issues,
					);
				}
				if spawn.ownership == TargetCellSpawnOwnership::SkillActivation
					&& tile.lifetime != RuntimeEffectLifetime::SkillActivation
				{
					invalid_runtime_primitive(
						tagged,
						"skill-owned tile reaction actor requires skill-activation tile lifetime",
						issues,
					);
				}
			}
			for nested in tile
				.on_enter_once
				.iter()
				.chain(tile.on_enter_per_layer.iter())
				.chain(tile.on_leave_per_layer.iter())
				.chain(tile.on_remove_per_layer.iter())
				.chain(
					tile.periodic
						.as_ref()
						.into_iter()
						.flat_map(|periodic| periodic.effects_per_layer.iter()),
				) {
				validate_runtime_effect(
					nested,
					RuntimeEffectScope::Root,
					dimensions,
					summon_templates,
					trigger_graph,
					issues,
				);
			}
		}
		ExpectedEffect::InstallTileFieldOverlay(overlay) => {
			if tagged.target != EffectTarget::SelfActor {
				invalid_runtime_primitive(
					tagged,
					"tile-field overlay must target its source actor",
					issues,
				);
			}
			if overlay.key.as_str().trim().is_empty()
				|| overlay.observed_tile_key.as_str().trim().is_empty()
			{
				invalid_runtime_primitive(
					tagged,
					"tile-field overlay keys must not be empty",
					issues,
				);
			}
			if overlay.interval_ticks == 0 {
				invalid_runtime_primitive(
					tagged,
					"tile-field overlay interval must be positive",
					issues,
				);
			}
			if overlay.lifetime != RuntimeEffectLifetime::SkillActivation {
				invalid_runtime_primitive(
					tagged,
					"tile-field overlay lifetime must be skill activation",
					issues,
				);
			}
			if overlay.periodic_effects.is_empty()
				&& overlay.on_leave_once.is_empty()
				&& overlay.layer_threshold_reactions.is_empty()
				&& overlay.cached_propagation.is_none()
			{
				invalid_runtime_primitive(
					tagged,
					"tile-field overlay must define observable behavior",
					issues,
				);
			}
			if overlay.observed_tile_max_layers == Some(0) {
				invalid_runtime_primitive(
					tagged,
					"overlay observed tile layer cap must be positive",
					issues,
				);
			}
			if !overlay.layer_threshold_reactions.is_empty()
				&& overlay.observed_tile_max_layers.is_none()
			{
				invalid_runtime_primitive(
					tagged,
					"overlay tile reactions require the observed tile layer cap",
					issues,
				);
			}
			for reaction in &overlay.layer_threshold_reactions {
				if reaction.threshold == 0 {
					invalid_runtime_primitive(
						tagged,
						"overlay tile reaction threshold must be positive",
						issues,
					);
				}
				if overlay
					.observed_tile_max_layers
					.is_some_and(|maximum| reaction.threshold > maximum)
				{
					invalid_runtime_primitive(
						tagged,
						"overlay tile reaction threshold exceeds the observed tile layer cap",
						issues,
					);
				}
				if reaction.transition == TileLayerThresholdTransition::SaturatedAttempt
					&& overlay
						.observed_tile_max_layers
						.is_some_and(|maximum| reaction.threshold != maximum)
				{
					invalid_runtime_primitive(
						tagged,
						"saturated overlay reaction threshold must equal the observed tile layer cap",
						issues,
					);
				}
				let TileLayerThresholdReactionEffect::SpawnActor(spawn) = reaction.effect;
				if !summon_templates.contains_key(&spawn.template) {
					invalid_runtime_primitive(
						tagged,
						"overlay tile reaction summon template does not exist",
						issues,
					);
				}
				if spawn.ownership == TargetCellSpawnOwnership::SkillActivation
					&& overlay.lifetime != RuntimeEffectLifetime::SkillActivation
				{
					invalid_runtime_primitive(
						tagged,
						"skill-owned overlay reaction actor requires skill-activation lifetime",
						issues,
					);
				}
			}
			for nested in overlay
				.periodic_effects
				.iter()
				.chain(overlay.on_leave_once.iter())
			{
				validate_runtime_effect(
					nested,
					RuntimeEffectScope::Root,
					dimensions,
					summon_templates,
					trigger_graph,
					issues,
				);
			}
		}
		ExpectedEffect::Sequence(sequence) => {
			for nested in sequence {
				validate_runtime_effect(
					nested,
					RuntimeEffectScope::Root,
					dimensions,
					summon_templates,
					trigger_graph,
					issues,
				);
			}
		}
		ExpectedEffect::Expected { effect, .. } => {
			let nested = TaggedEffect {
				mechanic: tagged.mechanic.clone(),
				target: tagged.target,
				effect: *effect.clone(),
			};
			validate_runtime_effect(
				&nested,
				RuntimeEffectScope::Root,
				dimensions,
				summon_templates,
				trigger_graph,
				issues,
			);
		}
		ExpectedEffect::RestoreHpToCurrentMaxRatio { ratio } => {
			if *ratio == ExpectedValue::ZERO || *ratio > ExpectedValue::ONE {
				invalid_runtime_primitive(
					tagged,
					"current-max HP restore ratio must be greater than zero and at most one",
					issues,
				);
			}
		}
		ExpectedEffect::AttackSpeed(modifier) => {
			if 100_i32 + i32::from(modifier.delta) <= 0 {
				invalid_runtime_primitive(
					tagged,
					"attack-speed denominator must be positive",
					issues,
				);
			}
		}
		ExpectedEffect::ApplyStatus(status) => {
			let canonical: Option<StatusEffect> = match &status.kind {
				StatusKind::Cold => Some(StatusEffect::cold(status.duration_ticks)),
				StatusKind::Frozen => Some(StatusEffect::frozen(status.duration_ticks)),
				StatusKind::Stun
				| StatusKind::Slow
				| StatusKind::Shelter
				| StatusKind::Exposure
				| StatusKind::Gravity
				| StatusKind::ExposeDamageSource { .. }
				| StatusKind::StealthWhileUnblocked
				| StatusKind::Custom(_) => None,
			};
			if canonical
				.as_ref()
				.is_some_and(|canonical| canonical != status)
			{
				invalid_runtime_primitive(
					tagged,
					"Cold and Frozen must use their canonical replay status profiles",
					issues,
				);
			}
		}
		ExpectedEffect::Summon(summon) => {
			if !summon_templates.contains_key(&summon.template) {
				invalid_runtime_primitive(tagged, "summon template does not exist", issues);
			}
		}
		ExpectedEffect::RemoveListener { key }
		| ExpectedEffect::RemoveTileEffect { key }
		| ExpectedEffect::RemoveBuff { key, .. }
			if key.as_str().trim().is_empty() =>
		{
			invalid_runtime_primitive(tagged, "removal key must not be empty", issues);
		}
		ExpectedEffect::ConditionalStatModifier(conditional) => {
			match conditional.condition {
				ActorCondition::BlockedEnemiesAtLeast(0) => invalid_runtime_primitive(
					tagged,
					"blocked-enemy count threshold must be positive",
					issues,
				),
				ActorCondition::BlockedEnemiesAtLeast(_) => {}
			}
			if conditional.modifier.stat == StatKind::MaxHp {
				invalid_runtime_primitive(
					tagged,
					"conditional stat modifier cannot gate maximum HP, whose change clamps current HP",
					issues,
				);
			}
		}
		ExpectedEffect::GainDp { amount } => {
			if *amount == 0 {
				invalid_runtime_primitive(tagged, "deployment-point gain must be positive", issues);
			}
			if tagged.target != EffectTarget::SelfActor {
				invalid_runtime_primitive(
					tagged,
					"deployment-point gain credits the battle once and must target its source",
					issues,
				);
			}
		}
		ExpectedEffect::Damage { .. }
		| ExpectedEffect::SourceStatScaledDamage { .. }
		| ExpectedEffect::Healing { .. }
		| ExpectedEffect::StatModifier(_)
		| ExpectedEffect::AttackCadence(_)
		| ExpectedEffect::AttackRange(_)
		| ExpectedEffect::TargetCount(_)
		| ExpectedEffect::HitCount(_)
		| ExpectedEffect::GainSp { .. }
		| ExpectedEffect::ActivateSkill
		| ExpectedEffect::Shift(_)
		| ExpectedEffect::Blocking(_)
		| ExpectedEffect::BlockTarget(_)
		| ExpectedEffect::StartAxisDirectHit(_)
		| ExpectedEffect::Death
		| ExpectedEffect::RemoveListener { .. }
		| ExpectedEffect::RemoveBuff { .. }
		| ExpectedEffect::RemoveTileEffect { .. } => {}
	}
}

fn invalid_runtime_primitive(
	tagged: &TaggedEffect,
	detail: &str,
	issues: &mut Vec<BattleCompileIssue>,
) {
	issues.push(BattleCompileIssue::InvalidReplayPrimitive {
		mechanic: tagged.mechanic.clone(),
		detail: detail.to_owned(),
	});
}

fn validate_cell_set(
	tagged: &TaggedEffect,
	cells: &ReplayCellSet,
	dimensions: MapDimensions,
	issues: &mut Vec<BattleCompileIssue>,
) {
	if let ReplayCellSet::Explicit(cells) = cells {
		if cells.is_empty() {
			invalid_runtime_primitive(tagged, "explicit cell set must not be empty", issues);
		}
		if cells.iter().any(|cell| !cell_in_bounds(*cell, dimensions)) {
			invalid_runtime_primitive(tagged, "cell set contains an out-of-bounds cell", issues);
		}
	} else if matches!(
		cells,
		ReplayCellSet::SourceRelative(offsets)
			| ReplayCellSet::EffectTargetRelative(offsets)
			if offsets.is_empty()
	) {
		invalid_runtime_primitive(tagged, "relative cell set must not be empty", issues);
	}
}

fn validate_predicate(
	tagged: &TaggedEffect,
	predicate: &ReplayPredicate,
	issues: &mut Vec<BattleCompileIssue>,
) {
	match predicate {
		ReplayPredicate::HpRatio { threshold, .. } if *threshold > ExpectedValue::ONE => {
			invalid_runtime_primitive(tagged, "HP-ratio threshold exceeds one", issues);
		}
		ReplayPredicate::All(predicates) | ReplayPredicate::Any(predicates) => {
			for predicate in predicates {
				validate_predicate(tagged, predicate, issues);
			}
		}
		ReplayPredicate::Not(predicate) => validate_predicate(tagged, predicate, issues),
		ReplayPredicate::Always | ReplayPredicate::HpRatio { .. } => {}
	}
}

fn buff_modifier_has_duration(modifier: &BuffModifier) -> bool {
	match modifier {
		BuffModifier::Stat(modifier) => modifier.duration_ticks.is_some(),
		BuffModifier::ArtsResistanceAddition { .. } => false,
		BuffModifier::Shelter { .. } => false,
		BuffModifier::MovementSpeedReduction { .. } => false,
		BuffModifier::AttackCadence(modifier) => modifier.duration_ticks.is_some(),
		BuffModifier::AttackSpeed(modifier) => modifier.duration_ticks.is_some(),
		BuffModifier::AttackRange(modifier) => modifier.duration_ticks.is_some(),
		BuffModifier::TargetCount(modifier) => modifier.duration_ticks().is_some(),
		BuffModifier::HitCount(modifier) => modifier.duration_ticks.is_some(),
		BuffModifier::Blocking(modifier) => modifier.duration_ticks.is_some(),
		BuffModifier::BlockTarget(modifier) => modifier.duration_ticks.is_some(),
	}
}

fn emitted_trigger_kinds(
	effect: &TaggedEffect,
	summon_templates: &BTreeMap<SummonTemplateId, &SummonTemplate>,
) -> BTreeSet<ReplayTriggerKind> {
	emitted_trigger_kinds_inner(effect, summon_templates, &mut BTreeSet::new())
}

fn emitted_trigger_kinds_inner(
	effect: &TaggedEffect,
	summon_templates: &BTreeMap<SummonTemplateId, &SummonTemplate>,
	visiting_summons: &mut BTreeSet<SummonTemplateId>,
) -> BTreeSet<ReplayTriggerKind> {
	match &effect.effect {
		ExpectedEffect::Damage { .. } | ExpectedEffect::SourceStatScaledDamage { .. } => {
			BTreeSet::from([ReplayTriggerKind::Damage, ReplayTriggerKind::BeforeLethal])
		}
		ExpectedEffect::Healing { .. } => BTreeSet::from([ReplayTriggerKind::Heal]),
		ExpectedEffect::ActivateSkill => BTreeSet::from([ReplayTriggerKind::SkillStarted]),
		ExpectedEffect::Shift(_) => BTreeSet::from([
			ReplayTriggerKind::ActorEnteredCells,
			ReplayTriggerKind::ActorLeftCells,
		]),
		ExpectedEffect::Death => BTreeSet::from([ReplayTriggerKind::Death]),
		ExpectedEffect::Sequence(sequence) => sequence
			.iter()
			.flat_map(|nested| {
				emitted_trigger_kinds_inner(nested, summon_templates, visiting_summons)
			})
			.collect(),
		ExpectedEffect::Expected { effect: nested, .. } => emitted_trigger_kinds_inner(
			&TaggedEffect {
				mechanic: effect.mechanic.clone(),
				target: effect.target,
				effect: *nested.clone(),
			},
			summon_templates,
			visiting_summons,
		),
		ExpectedEffect::Summon(summon) => {
			summon_emitted_trigger_kinds(summon.template, summon_templates, visiting_summons)
		}
		ExpectedEffect::InstallTileFieldOverlay(overlay) => {
			if overlay.initial_delay_ticks == 0 {
				overlay
					.periodic_effects
					.iter()
					.flat_map(|nested| {
						emitted_trigger_kinds_inner(nested, summon_templates, visiting_summons)
					})
					.collect()
			} else {
				BTreeSet::new()
			}
		}
		ExpectedEffect::RegisterListener(listener) => {
			let zero_delay: bool = matches!(
				listener.trigger,
				ReplayTrigger::Periodic {
					initial_delay_ticks: 0,
					..
				}
			);
			if zero_delay {
				listener
					.effects
					.iter()
					.flat_map(|nested| {
						emitted_trigger_kinds_inner(nested, summon_templates, visiting_summons)
					})
					.collect()
			} else {
				BTreeSet::new()
			}
		}
		ExpectedEffect::InstallConditionalAura(aura) => aura
			.periodic
			.as_ref()
			.filter(|periodic| periodic.initial_delay_ticks == 0)
			.into_iter()
			.flat_map(|periodic| periodic.effects.iter())
			.flat_map(|nested| {
				emitted_trigger_kinds_inner(nested, summon_templates, visiting_summons)
			})
			.collect(),
		ExpectedEffect::AddTileLayer(tile) => {
			let mut emitted: BTreeSet<ReplayTriggerKind> = BTreeSet::new();
			for reaction in &tile.layer_threshold_reactions {
				let TileLayerThresholdReactionEffect::SpawnActor(spawn) = reaction.effect;
				emitted.extend(summon_emitted_trigger_kinds(
					spawn.template,
					summon_templates,
					visiting_summons,
				));
			}
			for nested in &tile.on_enter_once {
				emitted.extend(emitted_trigger_kinds_inner(
					nested,
					summon_templates,
					visiting_summons,
				));
			}
			if tile.apply_new_layer_to_occupants {
				for nested in &tile.on_enter_per_layer {
					emitted.extend(emitted_trigger_kinds_inner(
						nested,
						summon_templates,
						visiting_summons,
					));
				}
			}
			if tile.duration_ticks == Some(0) {
				for nested in &tile.on_remove_per_layer {
					emitted.extend(emitted_trigger_kinds_inner(
						nested,
						summon_templates,
						visiting_summons,
					));
				}
			}
			if let Some(periodic) = tile
				.periodic
				.as_ref()
				.filter(|periodic| periodic.initial_delay_ticks == 0)
			{
				for nested in &periodic.effects_per_layer {
					emitted.extend(emitted_trigger_kinds_inner(
						nested,
						summon_templates,
						visiting_summons,
					));
				}
			}
			emitted
		}
		ExpectedEffect::StatModifier(_)
		| ExpectedEffect::ConditionalStatModifier(_)
		| ExpectedEffect::AttackCadence(_)
		| ExpectedEffect::AttackSpeed(_)
		| ExpectedEffect::AttackRange(_)
		| ExpectedEffect::TargetCount(_)
		| ExpectedEffect::HitCount(_)
		| ExpectedEffect::GainSp { .. }
		| ExpectedEffect::GainDp { .. }
		| ExpectedEffect::ApplyStatus(_)
		| ExpectedEffect::RestoreHpToCurrentMaxRatio { .. }
		| ExpectedEffect::Blocking(_)
		| ExpectedEffect::BlockTarget(_)
		| ExpectedEffect::StartAxisDirectHit(_)
		| ExpectedEffect::RemoveListener { .. }
		| ExpectedEffect::ApplyBuff(_)
		| ExpectedEffect::InstallSkillInactiveStance(_)
		| ExpectedEffect::RemoveBuff { .. }
		| ExpectedEffect::RemoveTileEffect { .. } => BTreeSet::new(),
	}
}

fn summon_emitted_trigger_kinds(
	template_id: SummonTemplateId,
	summon_templates: &BTreeMap<SummonTemplateId, &SummonTemplate>,
	visiting_summons: &mut BTreeSet<SummonTemplateId>,
) -> BTreeSet<ReplayTriggerKind> {
	let Some(template) = summon_templates.get(&template_id) else {
		return BTreeSet::new();
	};
	if template.attack.first_delay_ticks != 0 || !visiting_summons.insert(template_id) {
		return BTreeSet::new();
	}
	let mut emitted: BTreeSet<ReplayTriggerKind> = BTreeSet::new();
	if template.attack.target_count > 0 {
		emitted.insert(ReplayTriggerKind::NormalAttackHit);
		match template.attack.action {
			NormalAttackAction::Damage { .. } => {
				emitted.insert(ReplayTriggerKind::Damage);
				emitted.insert(ReplayTriggerKind::BeforeLethal);
			}
			NormalAttackAction::Heal => {
				emitted.insert(ReplayTriggerKind::Heal);
			}
			NormalAttackAction::None => {}
		}
	}
	for nested in &template.attack.extra_effects {
		emitted.extend(emitted_trigger_kinds_inner(
			nested,
			summon_templates,
			visiting_summons,
		));
	}
	visiting_summons.remove(&template_id);
	emitted
}

fn find_trigger_cycle(
	graph: &BTreeMap<ReplayTriggerKind, BTreeSet<ReplayTriggerKind>>,
) -> Option<Vec<ReplayTriggerKind>> {
	fn visit(
		node: ReplayTriggerKind,
		graph: &BTreeMap<ReplayTriggerKind, BTreeSet<ReplayTriggerKind>>,
		visiting: &mut Vec<ReplayTriggerKind>,
		visited: &mut BTreeSet<ReplayTriggerKind>,
	) -> Option<Vec<ReplayTriggerKind>> {
		if let Some(index) = visiting.iter().position(|candidate| *candidate == node) {
			let mut cycle: Vec<ReplayTriggerKind> = visiting[index..].to_vec();
			cycle.push(node);
			return Some(cycle);
		}
		if visited.contains(&node) {
			return None;
		}
		visiting.push(node);
		for next in graph.get(&node).into_iter().flatten().copied() {
			if let Some(cycle) = visit(next, graph, visiting, visited) {
				return Some(cycle);
			}
		}
		visiting.pop();
		visited.insert(node);
		None
	}

	let mut visited: BTreeSet<ReplayTriggerKind> = BTreeSet::new();
	for node in graph.keys().copied() {
		if let Some(cycle) = visit(node, graph, &mut Vec::new(), &mut visited) {
			return Some(cycle);
		}
	}
	None
}

fn anchor_is_reachable(
	anchor: BattleTimeAnchor,
	reachable_waves: &BTreeSet<WaveId>,
	enemy_waves: &BTreeMap<EnemyInstanceId, WaveId>,
) -> bool {
	match anchor {
		BattleTimeAnchor::BattleStarted => true,
		BattleTimeAnchor::WaveStarted(wave) | BattleTimeAnchor::WaveCleared(wave) => {
			reachable_waves.contains(&wave)
		}
		BattleTimeAnchor::EnemySpawned(enemy) | BattleTimeAnchor::EnemyDefeated(enemy) => {
			enemy_waves
				.get(&enemy)
				.is_some_and(|wave| reachable_waves.contains(wave))
		}
	}
}

fn validate_actor_facts(
	key: &str,
	combat: ActorCombatFacts,
	attack: &AttackProfile,
	issues: &mut Vec<BattleCompileIssue>,
) {
	if combat.max_hp == ExpectedValue::ZERO {
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has zero max HP"),
		});
	}
	if combat.arts_resistance > ExpectedValue::ONE
		|| combat.minimum_damage_ratio > ExpectedValue::ONE
	{
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has a ratio outside [0, 1]"),
		});
	}
	if attack.interval_ticks == 0 {
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has a zero attack interval"),
		});
	}
	if attack.target_count == 0 && attack.action != NormalAttackAction::None {
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has zero attack targets"),
		});
	}
	if matches!(attack.pattern, AttackPattern::PowerHit { every: 0, .. }) {
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has a zero PowerHit cadence"),
		});
	}
	if matches!(attack.pattern, AttackPattern::FirstAttackAfterBlock { .. })
		&& !matches!(attack.action, NormalAttackAction::Damage { .. })
	{
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has a non-damage first-after-block attack"),
		});
	}
	if matches!(
		attack.pattern,
		AttackPattern::Projectile { flight_ticks: 0 }
	) {
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has a zero projectile flight"),
		});
	}
	if matches!(attack.pattern, AttackPattern::Projectile { .. })
		&& (!matches!(attack.action, NormalAttackAction::Damage { .. })
			|| !attack.extra_effects.is_empty()
			|| attack.target_count != 1)
	{
		issues.push(BattleCompileIssue::InvalidCombatFacts {
			detail: format!("{key:?} has a projectile attack that is not one plain damage hit"),
		});
	}
}

fn collect_unsupported_mechanics(
	facts: &ArkBattleFacts,
	guide: &MaterializedGuide,
	issues: &mut Vec<BattleCompileIssue>,
) -> Vec<UnsupportedMechanicEntry> {
	let mut aggregate: BTreeMap<MechanicId, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
	let mechanics = facts
		.reached_mechanics
		.iter()
		.chain(guide.reached_mechanics.iter())
		.chain(
			guide
				.builds
				.iter()
				.flat_map(|build| build.reached_mechanics.iter()),
		)
		.chain(
			facts
				.enemies
				.iter()
				.flat_map(|enemy| enemy.reached_mechanics.iter()),
		)
		.chain(
			facts
				.summons
				.iter()
				.flat_map(|summon| summon.reached_mechanics.iter()),
		);
	for mechanic in mechanics {
		if mechanic.id.as_str().trim().is_empty() {
			issues.push(BattleCompileIssue::EmptyMechanicId {
				origin: mechanic.origin.clone(),
			});
		}
		if let MechanicBehavior::Unsupported { reason } = &mechanic.behavior {
			let entry = aggregate.entry(mechanic.id.clone()).or_default();
			entry.0.insert(mechanic.origin.clone());
			entry.1.insert(reason.clone());
		}
	}
	aggregate
		.into_iter()
		.map(|(id, (origins, reasons))| UnsupportedMechanicEntry {
			id,
			origins: origins.into_iter().collect(),
			reasons: reasons.into_iter().collect(),
		})
		.collect()
}

fn normalize_battle_effects(
	facts: &mut ArkBattleFacts,
	guide: &mut MaterializedGuide,
	issues: &mut Vec<BattleCompileIssue>,
) {
	for stage_effect in &mut facts.stage_effects {
		normalize_tagged_effect(&mut stage_effect.effect, issues);
	}
	for enemy in &mut facts.enemies {
		normalize_attack(&mut enemy.attack, issues);
		normalize_effect_list(&mut enemy.on_spawn, issues);
		normalize_effect_list(&mut enemy.on_death, issues);
	}
	for summon in &mut facts.summons {
		normalize_attack(&mut summon.attack, issues);
	}
	for build in &mut guide.builds {
		normalize_attack(&mut build.attack, issues);
		normalize_effect_list(&mut build.on_battle_start, issues);
		normalize_effect_list(&mut build.on_deploy, issues);
		normalize_effect_list(&mut build.on_death, issues);
		if let Some(skill) = &mut build.skill {
			normalize_effect_list(&mut skill.effects, issues);
			if let Some(policy) = &mut skill.manual_discard {
				normalize_effect_list(&mut policy.on_discard, issues);
			}
		}
	}
}

fn normalize_attack(attack: &mut AttackProfile, issues: &mut Vec<BattleCompileIssue>) {
	normalize_effect_list(&mut attack.extra_effects, issues);
}

fn normalize_effect_list(effects: &mut Vec<TaggedEffect>, issues: &mut Vec<BattleCompileIssue>) {
	for effect in effects {
		normalize_tagged_effect(effect, issues);
	}
}

fn normalize_tagged_effect(effect: &mut TaggedEffect, issues: &mut Vec<BattleCompileIssue>) {
	match normalize_expected_effect(effect.effect.clone(), &effect.mechanic) {
		Ok(normalized) => effect.effect = normalized,
		Err(issue) => issues.push(issue),
	}
}

fn normalize_expected_effect(
	effect: ExpectedEffect,
	mechanic: &MechanicId,
) -> Result<ExpectedEffect, BattleCompileIssue> {
	match effect {
		ExpectedEffect::Sequence(mut effects) => {
			for effect in &mut effects {
				effect.effect = normalize_expected_effect(effect.effect.clone(), &effect.mechanic)?;
			}
			Ok(ExpectedEffect::Sequence(effects))
		}
		ExpectedEffect::Expected {
			probability,
			effect,
		} => {
			if probability > ExpectedValue::ONE {
				return Err(BattleCompileIssue::InvalidCombatFacts {
					detail: format!(
						"expected-effect probability for {:?} exceeds one",
						mechanic.as_str()
					),
				});
			}
			let nested: ExpectedEffect = normalize_expected_effect(*effect, mechanic)?;
			if probability == ExpectedValue::ONE {
				return Ok(nested);
			}
			if probability == ExpectedValue::ZERO {
				return Ok(ExpectedEffect::Sequence(Vec::new()));
			}
			match nested {
				ExpectedEffect::Damage {
					amount,
					damage_type,
					delivery,
				} => Ok(ExpectedEffect::Expected {
					probability,
					effect: Box::new(ExpectedEffect::Damage {
						amount,
						damage_type,
						delivery,
					}),
				}),
				ExpectedEffect::SourceStatScaledDamage {
					stat,
					multiplier,
					damage_type,
					delivery,
				} => Ok(ExpectedEffect::SourceStatScaledDamage {
					stat,
					multiplier: multiplier.checked_mul(probability).ok_or_else(|| {
						BattleCompileIssue::ArithmeticOverflow {
							detail: format!(
								"expected source-stat damage for mechanic {:?}",
								mechanic.as_str()
							),
						}
					})?,
					damage_type,
					delivery,
				}),
				ExpectedEffect::Healing { amount } => Ok(ExpectedEffect::Healing {
					amount: amount.checked_mul(probability).ok_or_else(|| {
						BattleCompileIssue::ArithmeticOverflow {
							detail: format!(
								"expected healing for mechanic {:?}",
								mechanic.as_str()
							),
						}
					})?,
				}),
				ExpectedEffect::StatModifier(mut modifier) => {
					modifier.multiplier = expected_multiplier(modifier.multiplier, probability)
						.ok_or_else(|| BattleCompileIssue::ArithmeticOverflow {
							detail: format!(
								"expected stat modifier for mechanic {:?}",
								mechanic.as_str()
							),
						})?;
					Ok(ExpectedEffect::StatModifier(modifier))
				}
				ExpectedEffect::GainSp { amount } => {
					let expected: ExpectedValue = ExpectedValue::from_whole(amount)
						.checked_mul(probability)
						.ok_or_else(|| BattleCompileIssue::ArithmeticOverflow {
							detail: format!("expected SP for mechanic {:?}", mechanic.as_str()),
						})?;
					if !expected.scaled().is_multiple_of(ExpectedValue::SCALE) {
						return Err(BattleCompileIssue::ExpectedDiscreteEffect {
							mechanic: mechanic.clone(),
						});
					}
					let whole: u32 = u32::try_from(expected.whole_floor()).map_err(|_| {
						BattleCompileIssue::ArithmeticOverflow {
							detail: format!("expected SP for mechanic {:?}", mechanic.as_str()),
						}
					})?;
					Ok(ExpectedEffect::GainSp { amount: whole })
				}
				ExpectedEffect::Sequence(_) => Err(BattleCompileIssue::ExpectedDiscreteEffect {
					mechanic: mechanic.clone(),
				}),
				ExpectedEffect::Expected {
					probability: nested_probability,
					effect,
				} => {
					let combined: ExpectedValue = probability
						.checked_mul(nested_probability)
						.ok_or_else(|| BattleCompileIssue::ArithmeticOverflow {
							detail: format!(
								"nested expected probability for mechanic {:?}",
								mechanic.as_str()
							),
						})?;
					normalize_expected_effect(
						ExpectedEffect::Expected {
							probability: combined,
							effect,
						},
						mechanic,
					)
				}
				ExpectedEffect::ConditionalStatModifier(_)
				| ExpectedEffect::AttackCadence(_)
				| ExpectedEffect::AttackSpeed(_)
				| ExpectedEffect::AttackRange(_)
				| ExpectedEffect::TargetCount(_)
				| ExpectedEffect::HitCount(_)
				| ExpectedEffect::GainDp { .. }
				| ExpectedEffect::ActivateSkill
				| ExpectedEffect::ApplyStatus(_)
				| ExpectedEffect::RestoreHpToCurrentMaxRatio { .. }
				| ExpectedEffect::Shift(_)
				| ExpectedEffect::Blocking(_)
				| ExpectedEffect::BlockTarget(_)
				| ExpectedEffect::Summon(_)
				| ExpectedEffect::StartAxisDirectHit(_)
				| ExpectedEffect::RegisterListener(_)
				| ExpectedEffect::RemoveListener { .. }
				| ExpectedEffect::ApplyBuff(_)
				| ExpectedEffect::InstallConditionalAura(_)
				| ExpectedEffect::InstallSkillInactiveStance(_)
				| ExpectedEffect::RemoveBuff { .. }
				| ExpectedEffect::AddTileLayer(_)
				| ExpectedEffect::InstallTileFieldOverlay(_)
				| ExpectedEffect::RemoveTileEffect { .. }
				| ExpectedEffect::Death => Err(BattleCompileIssue::ExpectedDiscreteEffect {
					mechanic: mechanic.clone(),
				}),
			}
		}
		ExpectedEffect::RegisterListener(mut listener) => {
			for nested in &mut listener.effects {
				nested.effect = normalize_expected_effect(nested.effect.clone(), &nested.mechanic)?;
			}
			Ok(ExpectedEffect::RegisterListener(listener))
		}
		ExpectedEffect::InstallConditionalAura(mut aura) => {
			if let Some(periodic) = &mut aura.periodic {
				for nested in &mut periodic.effects {
					nested.effect =
						normalize_expected_effect(nested.effect.clone(), &nested.mechanic)?;
				}
			}
			Ok(ExpectedEffect::InstallConditionalAura(aura))
		}
		ExpectedEffect::AddTileLayer(mut tile_effect) => {
			for effects in [
				&mut tile_effect.on_enter_once,
				&mut tile_effect.on_enter_per_layer,
				&mut tile_effect.on_leave_per_layer,
				&mut tile_effect.on_remove_per_layer,
			] {
				for nested in effects {
					nested.effect =
						normalize_expected_effect(nested.effect.clone(), &nested.mechanic)?;
				}
			}
			if let Some(periodic) = &mut tile_effect.periodic {
				for nested in &mut periodic.effects_per_layer {
					nested.effect =
						normalize_expected_effect(nested.effect.clone(), &nested.mechanic)?;
				}
			}
			Ok(ExpectedEffect::AddTileLayer(tile_effect))
		}
		ExpectedEffect::InstallTileFieldOverlay(mut overlay) => {
			for nested in overlay
				.periodic_effects
				.iter_mut()
				.chain(overlay.on_leave_once.iter_mut())
			{
				nested.effect = normalize_expected_effect(nested.effect.clone(), &nested.mechanic)?;
			}
			Ok(ExpectedEffect::InstallTileFieldOverlay(overlay))
		}
		ExpectedEffect::StartAxisDirectHit(facts)
			if facts.cooldown_ticks == 0 || facts.projectiles == 0 =>
		{
			Err(BattleCompileIssue::InvalidCombatFacts {
				detail: format!(
					"axis direct hit for {:?} requires positive cooldown and projectile count",
					mechanic.as_str()
				),
			})
		}
		other => Ok(other),
	}
}

fn expected_multiplier(
	multiplier: ExpectedValue,
	probability: ExpectedValue,
) -> Option<ExpectedValue> {
	if multiplier >= ExpectedValue::ONE {
		let excess: ExpectedValue = multiplier.saturating_sub(ExpectedValue::ONE);
		ExpectedValue::ONE.checked_add(excess.checked_mul(probability)?)
	} else {
		let reduction: ExpectedValue = ExpectedValue::ONE.saturating_sub(multiplier);
		Some(ExpectedValue::ONE.saturating_sub(reduction.checked_mul(probability)?))
	}
}

fn deployment_cost_for_count(
	base_cost: u32,
	previous_deployment_count: u32,
	second_deployment_cost_multiplier: ExpectedValue,
	later_deployment_cost_multiplier: ExpectedValue,
) -> Option<u32> {
	let multiplier: ExpectedValue = match previous_deployment_count {
		0 => ExpectedValue::ONE,
		1 => second_deployment_cost_multiplier,
		_ => later_deployment_cost_multiplier,
	};
	multiplied_dp_amount(base_cost, multiplier)
}

fn multiplied_dp_amount(amount: u32, multiplier: ExpectedValue) -> Option<u32> {
	let multiplied: ExpectedValue = ExpectedValue::from_whole(amount).checked_mul(multiplier)?;
	u32::try_from(multiplied.whole_floor()).ok()
}

fn current_deployment_cost(state: &ExpectedState, operator: OperatorInstanceId) -> Option<u32> {
	let build: &OperatorBuildFacts = operator_build(state, operator)?;
	let previous_deployment_count: u32 =
		state.deployment_counts.get(&operator).copied().unwrap_or(0);
	deployment_cost_for_count(
		build.deployment_cost,
		previous_deployment_count,
		state
			.scenario
			.battle
			.facts
			.deployment
			.second_deployment_cost_multiplier,
		state
			.scenario
			.battle
			.facts
			.deployment
			.later_deployment_cost_multiplier,
	)
}

fn deployment_is_legal(
	state: &ExpectedState,
	operator: OperatorInstanceId,
	cell: GridPosition,
) -> bool {
	let deployed_count: u32 = u32::try_from(state.deployed.len()).unwrap_or(u32::MAX);
	current_deployment_cost(state, operator).is_some_and(|cost| state.dp >= cost)
		&& deployed_count < state.scenario.battle.facts.deployment.simultaneous_limit
		&& !state.deployed.contains_key(&operator)
		&& !friendly_cell_occupied(state, cell)
		&& !state.redeployment_ready_at.contains_key(&operator)
}

fn command_for_action(
	state: &ExpectedState,
	action: &MaterializedGuideAction,
	at: SimTick,
) -> Option<ExpectedCommand> {
	if action.requested_at() > at {
		return None;
	}
	match action {
		MaterializedGuideAction::Deploy {
			operator,
			cell,
			facing,
			..
		} => deployment_is_legal(state, *operator, *cell).then_some(ExpectedCommand::Deploy {
			action_index: state.next_guide_action,
			operator: *operator,
			cell: *cell,
			facing: *facing,
		}),
		MaterializedGuideAction::ActivateSkill { operator, .. } => {
			state.deployed.get(operator).and_then(|deployed| {
				if deployed.skill_active {
					skill_can_be_manually_discarded(state, *operator).then_some(
						ExpectedCommand::DiscardSkill {
							action_index: state.next_guide_action,
							operator: *operator,
						},
					)
				} else {
					skill_is_ready(state, *operator).then_some(ExpectedCommand::ActivateSkill {
						action_index: state.next_guide_action,
						operator: *operator,
					})
				}
			})
		}
		MaterializedGuideAction::Retreat { operator, .. } => state
			.deployed
			.contains_key(operator)
			.then_some(ExpectedCommand::Retreat {
				action_index: state.next_guide_action,
				operator: *operator,
			}),
		MaterializedGuideAction::SpeedUp { .. } | MaterializedGuideAction::SkillDaemon { .. } => {
			None
		}
	}
}

fn apply_guide_ready(
	state: &mut ExpectedState,
	action_index: usize,
	kind: GuideExecutorActionKind,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if action_index != state.next_guide_action {
		return Ok(Vec::new());
	}
	let action: MaterializedGuideAction = state
		.scenario
		.battle
		.guide
		.actions
		.get(action_index)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("guide wake references a missing action"))?;
	match action {
		MaterializedGuideAction::SpeedUp { .. } => {
			mark_executor_action(state, action_index, at);
			advance_guide(state, at)
		}
		MaterializedGuideAction::SkillDaemon { .. } => {
			state.skill_daemon_enabled = true;
			let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
			let eligible: Vec<OperatorInstanceId> = state
				.deployed
				.keys()
				.copied()
				.filter(|operator| state.scenario.policy.skill_usage(*operator) == Some(1))
				.filter(|operator| {
					state
						.deployed
						.get(operator)
						.and_then(|deployed| deployed.build.skill.as_ref())
						.is_some_and(|skill| skill.activation == SkillActivationRule::Manual)
				})
				.filter(|operator| skill_is_ready(state, *operator))
				.collect();
			for operator in eligible {
				let (mechanic, deployment_generation): (MechanicId, u32) = state
					.deployed
					.get(&operator)
					.and_then(|deployed| {
						deployed
							.build
							.skill
							.as_ref()
							.map(|skill| (skill.id.clone(), deployed.deployment_generation))
					})
					.expect("eligible daemon skill exists");
				state
					.deployed
					.get_mut(&operator)
					.expect("eligible daemon operator is deployed")
					.skill_activation_pending = true;
				events.push(skill_activation_event(
					operator,
					mechanic,
					deployment_generation,
					SkillActivationOrigin::SkillDaemon,
					at,
				));
			}
			mark_executor_action(state, action_index, at);
			events.extend(advance_guide(state, at)?);
			Ok(events)
		}
		MaterializedGuideAction::Deploy { operator, .. } => {
			state.guide_ready = true;
			if current_deployment_cost(state, operator)
				.is_some_and(|deployment_cost| state.dp < deployment_cost)
			{
				state.action_audits[action_index].status = ActionAuditStatus::WaitedForDp;
			}
			Ok(Vec::new())
		}
		MaterializedGuideAction::ActivateSkill { .. } | MaterializedGuideAction::Retreat { .. } => {
			debug_assert!(matches!(
				kind,
				GuideExecutorActionKind::ActivateSkill | GuideExecutorActionKind::Retreat
			));
			state.guide_ready = true;
			Ok(Vec::new())
		}
	}
}

fn begin_redeployment_cooldown(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	cooldown_ticks: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let ready_at: SimTick = checked_tick_add(at, cooldown_ticks)?;
	if state
		.redeployment_ready_at
		.insert(operator, ready_at)
		.is_some()
	{
		return Err(ModelError::invalid_transition(
			"operator started redeployment cooldown twice",
		));
	}
	Ok(vec![EventRequest::new(
		ready_at,
		REDEPLOY_PRIORITY,
		operator_entity(operator),
		ExpectedEvent::OperatorRedeploymentReady { operator },
	)])
}

fn apply_operator_redeployment_ready(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	at: SimTick,
) {
	if state
		.redeployment_ready_at
		.get(&operator)
		.is_some_and(|ready_at| *ready_at == at)
	{
		state.redeployment_ready_at.remove(&operator);
	}
}

fn retire_operator_attack_generation(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	retired_generation: u32,
) -> Result<(), ModelError> {
	let next_generation: u32 = retired_generation
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("operator attack generation overflow"))?;
	state
		.next_operator_attack_generation
		.insert(operator, next_generation);
	Ok(())
}

fn operator_deployment_generation(state: &ExpectedState, actor: BattleActorId) -> Option<u32> {
	let BattleActorId::Operator(operator) = actor else {
		return None;
	};
	state
		.deployed
		.get(&operator)
		.map(|deployed| deployed.deployment_generation)
}

/// A generation-bearing operator effect belongs to one live deployment episode.
///
/// Effects without a generation are source-independent, including posthumous on-death effects.
fn live_operator_source_is_stale(
	state: &ExpectedState,
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
) -> bool {
	let (BattleActorId::Operator(operator), Some(source_generation)) =
		(source, source_deployment_generation)
	else {
		return false;
	};
	state
		.deployed
		.get(&operator)
		.map(|deployed| deployed.deployment_generation)
		!= Some(source_generation)
}

/// Credits an effect's deployment points on the natural-regeneration terms: whole points, capped at
/// the battle maximum, with only the credited amount audited. The regeneration grid is untouched,
/// and a waiting deployment sees the new balance as soon as this event resolves. Like every queued
/// effect of an operator, a grant whose source deployment retired before it resolved never arrives.
fn grant_dp(state: &mut ExpectedState, amount: u32, at: SimTick) {
	let before: u32 = state.dp;
	state.dp = state
		.dp
		.saturating_add(amount)
		.min(state.scenario.battle.facts.dp.maximum);
	state.dp_entries.push(DpAuditEntry {
		at,
		kind: DpAuditKind::Granted,
		before,
		amount: state.dp.saturating_sub(before),
		after: state.dp,
		action_index: None,
	});
}

fn apply_dp_regen(
	state: &mut ExpectedState,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let rules = state.scenario.battle.facts.dp;
	let before: u32 = state.dp;
	state.dp = state
		.dp
		.saturating_add(rules.regen_amount)
		.min(rules.maximum);
	state.dp_entries.push(DpAuditEntry {
		at,
		kind: DpAuditKind::Regenerated,
		before,
		amount: state.dp.saturating_sub(before),
		after: state.dp,
		action_index: None,
	});
	let next_at: SimTick = checked_tick_add(at, rules.regen_interval_ticks)?;
	Ok(vec![EventRequest::new(
		next_at,
		DP_PRIORITY,
		STAGE_ENTITY,
		ExpectedEvent::DpRegen,
	)])
}

fn schedule_resolved_wave_bounds(
	state: &mut ExpectedState,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let mut resolved: Vec<(WaveId, bool, SimTick)> = Vec::new();
	for wave in &state.scenario.battle.facts.waves {
		if state.started_waves.contains(&wave.id) {
			continue;
		}
		if !state.scheduled_wave_bounds.contains(&(wave.id, false)) {
			if let Some(anchor_at) = state.anchors.get(&wave.start.earliest.anchor) {
				resolved.push((
					wave.id,
					false,
					checked_tick_add(*anchor_at, wave.start.earliest.offset_ticks)?,
				));
			}
		}
		if !state.scheduled_wave_bounds.contains(&(wave.id, true)) {
			if let Some(latest) = wave.start.latest {
				if let Some(anchor_at) = state.anchors.get(&latest.anchor) {
					resolved.push((
						wave.id,
						true,
						checked_tick_add(*anchor_at, latest.offset_ticks)?,
					));
				}
			}
		}
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::with_capacity(resolved.len());
	for (wave, forced, at) in resolved {
		state.scheduled_wave_bounds.insert((wave, forced));
		events.push(EventRequest::new(
			at,
			SPAWN_PRIORITY,
			wave_entity(wave),
			ExpectedEvent::WaveStart { wave, forced },
		));
	}
	Ok(events)
}

fn start_wave(
	state: &mut ExpectedState,
	wave: WaveId,
	forced: bool,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if state.started_waves.contains(&wave) {
		return Ok(Vec::new());
	}
	let wave_facts = state
		.scenario
		.battle
		.facts
		.waves
		.iter()
		.find(|candidate| candidate.id == wave)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("wave start references an unknown wave"))?;
	state.started_waves.insert(wave);
	state
		.anchors
		.entry(BattleTimeAnchor::WaveStarted(wave))
		.or_insert(at);
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for spawn in &wave_facts.spawns {
		events.push(EventRequest::new(
			checked_tick_add(at, spawn.after_wave_start_ticks)?,
			SPAWN_PRIORITY,
			enemy_entity(spawn.id),
			ExpectedEvent::EnemySpawn { enemy: spawn.id },
		));
	}
	events.extend(schedule_resolved_wave_bounds(state)?);
	if wave_facts.spawns.is_empty() {
		state
			.anchors
			.entry(BattleTimeAnchor::WaveCleared(wave))
			.or_insert(at);
		events.extend(schedule_resolved_wave_bounds(state)?);
	}
	let _ = forced;
	Ok(events)
}

fn record_anchor(
	state: &mut ExpectedState,
	anchor: BattleTimeAnchor,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	state.anchors.entry(anchor).or_insert(at);
	schedule_resolved_wave_bounds(state)
}

fn record_wave_clearance(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(wave) = state
		.scenario
		.battle
		.facts
		.waves
		.iter()
		.find(|wave| wave.spawns.iter().any(|spawn| spawn.id == enemy))
	else {
		return Ok(Vec::new());
	};
	let wave_id: WaveId = wave.id;
	let cleared: bool = wave
		.spawns
		.iter()
		.all(|spawn| state.defeated.contains(&spawn.id) || state.leaked.contains(&spawn.id));
	if !cleared {
		return Ok(Vec::new());
	}
	record_anchor(state, BattleTimeAnchor::WaveCleared(wave_id), at)
}

fn deploy_operator(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	cell: GridPosition,
	facing: Facing,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let build: OperatorBuildFacts = operator_build(state, operator)
		.cloned()
		.ok_or_else(|| ModelError::invalid_command("deployment references an unknown operator"))?;
	if !deployment_is_legal(state, operator, cell) {
		return Err(ModelError::invalid_command("deployment is not legal"));
	}
	let actual_paid_cost: u32 = current_deployment_cost(state, operator)
		.ok_or_else(|| ModelError::invalid_transition("deployment cost overflow"))?;
	let deployment_generation: u32 = state
		.deployment_counts
		.get(&operator)
		.copied()
		.unwrap_or(0)
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("operator deployment count overflow"))?;
	let attack_generation: u32 = state
		.next_operator_attack_generation
		.remove(&operator)
		.unwrap_or(0);
	let before: u32 = state.dp;
	state.dp -= actual_paid_cost;
	state.dp_entries.push(DpAuditEntry {
		at,
		kind: DpAuditKind::Spent,
		before,
		amount: actual_paid_cost,
		after: state.dp,
		action_index: Some(state.next_guide_action),
	});
	state
		.deployment_counts
		.insert(operator, deployment_generation);
	state.redeployment_ready_at.remove(&operator);
	let mut deployed = DeployedOperator {
		runtime: RuntimeActor {
			hp: build.combat.max_hp,
			cell,
			visible: true,
			facing,
			attack_count: 0,
			attack_generation,
			modifiers: Vec::new(),
			statuses: BTreeMap::new(),
		},
		sp: build.skill.as_ref().map_or(ExpectedValue::ZERO, |skill| {
			ExpectedValue::from_whole(skill.initial_sp).min(skill_sp_capacity(skill))
		}),
		skill_activation_pending: false,
		skill_active: false,
		skill_activation_count: 0,
		skill_discard_pending: false,
		sp_charge_generation: 0,
		blocking: BTreeSet::new(),
		actual_paid_cost,
		deployment_generation,
		build,
	};
	if let Some(report) = state.operator_reports.get_mut(&operator) {
		report.deployed_at = Some(at);
		report.death_at = None;
		report.max_hp = deployed.build.combat.max_hp;
		report.remaining_hp = deployed.runtime.hp;
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	events.push(normal_attack_event(
		BattleActorId::Operator(operator),
		attack_generation,
		checked_tick_add(at, deployed.build.attack.first_delay_ticks)?,
	));
	if let Some(skill) = &deployed.build.skill {
		events.extend(periodic_sp_pulse_events(
			operator,
			deployment_generation,
			deployed.sp_charge_generation,
			skill,
			at,
		)?);
		let sp_cost: ExpectedValue = skill_sp_cost(skill);
		let daemon_activates_manual: bool = state.skill_daemon_enabled
			&& state.scenario.policy.skill_usage(operator) == Some(1)
			&& skill.activation == SkillActivationRule::Manual;
		if skill.activation == SkillActivationRule::Passive
			|| (skill.activation == SkillActivationRule::Auto
				&& skill.stored_attack_replacement.is_none()
				&& deployed.sp >= sp_cost)
			|| (daemon_activates_manual && deployed.sp >= sp_cost)
		{
			deployed.skill_activation_pending = true;
			let origin: SkillActivationOrigin = if daemon_activates_manual {
				SkillActivationOrigin::SkillDaemon
			} else {
				SkillActivationOrigin::Automatic
			};
			events.push(skill_activation_event(
				operator,
				skill.id.clone(),
				deployment_generation,
				origin,
				at,
			));
		}
	}
	let on_deploy: Vec<TaggedEffect> = deployed.build.on_deploy.clone();
	state.deployed.insert(operator, deployed);
	let actor = BattleActorId::Operator(operator);
	events.extend(reconcile_conditional_auras(state, at)?);
	events.extend(effect_events(
		actor,
		Some(deployment_generation),
		&on_deploy,
		Vec::new(),
		at,
	));
	events.extend(actor_presence_events(state, actor, None, Some(cell), at)?);
	events.push(trigger_event(ReplayTriggerKind::Deploy, actor, actor, at));
	events.push(reconcile_blocking_event(
		actor,
		BlockEndReason::BlockTargetChange,
		at,
	));
	Ok(events)
}

fn retreat_operator(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let actor = BattleActorId::Operator(operator);
	let deployment_generation: u32 = state
		.deployed
		.get(&operator)
		.map(|deployed| deployed.deployment_generation)
		.ok_or_else(|| ModelError::invalid_command("cannot retreat an undeployed operator"))?;
	let source: RuntimeSource = runtime_source(state, actor, Some(deployment_generation));
	let before_cell: Option<GridPosition> = actor_cell(state, actor);
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		actor_presence_events(state, actor, before_cell, None, at)?;
	events.extend(cleanup_runtime_effects(state, source, false, at)?);
	events.extend(cleanup_target_buffs(
		state,
		actor,
		source.deployment_generation,
		at,
	)?);
	let removed = state
		.deployed
		.remove(&operator)
		.ok_or_else(|| ModelError::invalid_command("cannot retreat an undeployed operator"))?;
	events.extend(reconcile_conditional_auras(state, at)?);
	retire_operator_attack_generation(state, operator, removed.runtime.attack_generation)?;
	let refund: u32 = multiplied_dp_amount(
		removed.actual_paid_cost,
		state.scenario.battle.facts.deployment.retreat_refund_ratio,
	)
	.ok_or_else(|| ModelError::invalid_transition("retreat refund overflow"))?;
	let before: u32 = state.dp;
	state.dp = state
		.dp
		.saturating_add(refund)
		.min(state.scenario.battle.facts.dp.maximum);
	state.dp_entries.push(DpAuditEntry {
		at,
		kind: DpAuditKind::Refunded,
		before,
		amount: state.dp.saturating_sub(before),
		after: state.dp,
		action_index: Some(state.next_guide_action),
	});
	if let Some(report) = state.operator_reports.get_mut(&operator) {
		report.remaining_hp = removed.runtime.hp;
	}
	events.extend(begin_redeployment_cooldown(
		state,
		operator,
		removed.build.redeployment_cooldown_ticks,
		at,
	)?);
	events.extend(unblock_enemies(
		state,
		BattleActorId::Operator(operator),
		at,
		BlockEndReason::Retreat,
	)?);
	Ok(events)
}

fn spawn_enemy(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if state.enemies.contains_key(&enemy)
		|| state.defeated.contains(&enemy)
		|| state.leaked.contains(&enemy)
	{
		return Err(ModelError::invalid_transition(
			"enemy instance spawned more than once",
		));
	}
	let spawn: EnemySpawnFacts = find_spawn(&state.scenario.battle, enemy)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("spawn references an unknown enemy"))?;
	let facts: EnemyFacts = state
		.scenario
		.battle
		.facts
		.enemies
		.iter()
		.find(|facts| facts.enemy_key == spawn.enemy_key)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("enemy facts disappeared"))?;
	let path: &BattlePath = find_path(&state.scenario.battle, spawn.path)
		.ok_or_else(|| ModelError::invalid_transition("enemy path disappeared"))?;
	let cell: GridPosition = path.start;
	let spawn_path: PathId = spawn.path;
	let first_blocked_attack_pending: bool = matches!(
		facts.attack.pattern,
		AttackPattern::FirstAttackAfterBlock { .. }
	);
	let active = ActiveEnemy {
		runtime: RuntimeActor {
			hp: facts.combat.max_hp,
			cell,
			visible: true,
			facing: spawn.facing,
			attack_count: 0,
			attack_generation: 0,
			modifiers: Vec::new(),
			statuses: BTreeMap::new(),
		},
		facts,
		spawn,
		path_index: 0,
		blocked_by: None,
		first_blocked_attack_pending,
		axis_direct_hit: None,
		stealth_recovery_generation: 0,
		stealth_recovery_pending: false,
		move_generation: 0,
	};
	let on_spawn: Vec<TaggedEffect> = active.facts.on_spawn.clone();
	state.enemies.insert(enemy, active);
	let travel_ticks: u64 = find_path(&state.scenario.battle, spawn_path)
		.expect("enemy path was checked")
		.steps
		.iter()
		.try_fold(0_u64, |total, step| total.checked_add(step.duration_ticks))
		.ok_or_else(|| ModelError::invalid_transition("enemy deadline overflow"))?;
	if let Some(report) = state.enemy_reports.get_mut(&enemy) {
		report.spawned_at = Some(at);
		report.deadline_at = Some(checked_tick_add(at, travel_ticks)?);
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		effect_events(BattleActorId::Enemy(enemy), None, &on_spawn, Vec::new(), at);
	events.extend(actor_presence_events(
		state,
		BattleActorId::Enemy(enemy),
		None,
		Some(cell),
		at,
	)?);
	if !try_block_enemy(state, enemy, at) {
		events.push(move_event(state, enemy, at)?);
	}
	events.push(normal_attack_event(
		BattleActorId::Enemy(enemy),
		0,
		checked_tick_add(
			at,
			state
				.enemies
				.get(&enemy)
				.expect("enemy was inserted")
				.facts
				.attack
				.first_delay_ticks,
		)?,
	));
	events.extend(record_anchor(
		state,
		BattleTimeAnchor::EnemySpawned(enemy),
		at,
	)?);
	Ok(events)
}

fn move_enemy(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.enemies.get(&enemy) else {
		return Ok(Vec::new());
	};
	if active.move_generation != generation || active.blocked_by.is_some() {
		return Ok(Vec::new());
	}
	let path: &BattlePath = find_path(&state.scenario.battle, active.spawn.path)
		.ok_or_else(|| ModelError::invalid_transition("active enemy path disappeared"))?;
	let step = path
		.steps
		.get(active.path_index)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("enemy path ended without a goal"))?;
	if step.affected_by_movement_speed && actor_is_action_paused(state, BattleActorId::Enemy(enemy))
	{
		return Ok(Vec::new());
	}
	let expected_goal = path.goal;
	let is_goal: bool = matches!(step.action, BattlePathStepAction::Goal { .. });
	let actor = BattleActorId::Enemy(enemy);
	let before_cell: Option<GridPosition> = actor_cell(state, actor);
	if let Some(active) = state.enemies.get_mut(&enemy) {
		active.path_index = active
			.path_index
			.checked_add(1)
			.ok_or_else(|| ModelError::invalid_transition("enemy path index overflow"))?;
		match step.action {
			BattlePathStepAction::Move { cell, facing } => {
				active.runtime.cell = cell;
				active.runtime.visible = true;
				active.runtime.facing = facing;
			}
			BattlePathStepAction::Wait => {}
			BattlePathStepAction::Disappear => active.runtime.visible = false,
			BattlePathStepAction::Reappear { cell, facing } => {
				active.runtime.cell = cell;
				active.runtime.visible = true;
				active.runtime.facing = facing;
			}
			BattlePathStepAction::Goal { goal } => {
				if goal != expected_goal {
					return Err(ModelError::invalid_transition(
						"enemy reached a mismatched goal",
					));
				}
			}
		}
	}
	if is_goal {
		return leak_enemy(state, enemy, at);
	}
	let after_cell: Option<GridPosition> = actor_cell(state, actor);
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		actor_presence_events(state, actor, before_cell, after_cell, at)?;
	if after_cell.is_some() && try_block_enemy(state, enemy, at) {
		return Ok(events);
	}
	events.push(move_event(state, enemy, at)?);
	Ok(events)
}

fn apply_normal_attack(
	state: &mut ExpectedState,
	actor: BattleActorId,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !actor_exists(state, actor)
		|| actor_attack_generation(state, actor) != Some(generation)
		|| actor_is_action_paused(state, actor)
		|| normal_attacks_suppressed(state, actor)
	{
		return Ok(Vec::new());
	}
	let source_deployment_generation: Option<u32> = operator_deployment_generation(state, actor);
	let attack: AttackProfile = actor_attack_profile(state, actor)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("attacker profile disappeared"))?;
	let stored_replacement: Option<(
		OperatorInstanceId,
		MechanicId,
		ExpectedValue,
		StoredAttackReplacementFacts,
	)> = match actor {
		BattleActorId::Operator(operator) => state.deployed.get(&operator).and_then(|deployed| {
			let skill = deployed.build.skill.as_ref()?;
			let facts = skill.stored_attack_replacement.as_ref()?;
			(deployed.sp >= skill_sp_cost(skill)).then(|| {
				(
					operator,
					skill.id.clone(),
					skill_sp_cost(skill),
					facts.clone(),
				)
			})
		}),
		BattleActorId::Enemy(_) | BattleActorId::Summon(_) | BattleActorId::Stage => None,
	};
	if let Some((operator, mechanic, spent_sp, facts)) = stored_replacement {
		if let Some(target) = select_stored_attack_replacement_target(state, actor, &facts)? {
			return reserve_stored_attack_replacement(
				state, operator, mechanic, target, spent_sp, facts, at,
			);
		}
	}
	let actor_attack: ExpectedValue = effective_actor_stat(state, actor, StatKind::Attack)?;
	let mut targets: Vec<BattleActorId> = select_normal_targets(state, actor, &attack);
	let attack_number: u32 = increment_attack_count(state, actor)?;
	let hit_count: u32 = effective_hit_count(state, actor)?;
	let first_after_block: bool = !targets.is_empty()
		&& matches!(&attack.pattern, AttackPattern::FirstAttackAfterBlock { .. })
		&& take_first_blocked_attack(state, actor);
	let mut amount: ExpectedValue = actor_attack;
	let mut shift = None;
	match &attack.pattern {
		AttackPattern::Single => {}
		AttackPattern::CrossAttack { arm_length } => {
			if let Some(primary) = targets.first().copied() {
				targets = cross_targets(state, actor, primary, *arm_length);
			}
		}
		AttackPattern::FirstAttackAfterBlock { .. } => {}
		AttackPattern::PowerHit {
			every,
			damage_multiplier,
			shift: power_shift,
		} if attack_number.is_multiple_of(*every) => {
			amount = amount.checked_mul(*damage_multiplier).ok_or_else(|| {
				ModelError::invalid_transition("PowerHit expected damage overflow")
			})?;
			shift = *power_shift;
		}
		AttackPattern::PowerHit { .. } => {}
		AttackPattern::Projectile { .. } => {}
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	if !targets.is_empty() {
		match attack.action {
			NormalAttackAction::Damage {
				damage_type,
				delivery,
			} => {
				// Attack is sampled now, when the projectile fires; only its hit waits for the flight.
				let resolve_at: SimTick = match &attack.pattern {
					AttackPattern::Projectile { flight_ticks } => {
						checked_tick_add(at, *flight_ticks)?
					}
					AttackPattern::Single
					| AttackPattern::CrossAttack { .. }
					| AttackPattern::FirstAttackAfterBlock { .. }
					| AttackPattern::PowerHit { .. } => at,
				};
				for target in &targets {
					let target_amount: ExpectedValue =
						if let AttackPattern::FirstAttackAfterBlock {
							damage_multiplier,
							shelter_damage_multiplier,
						} = &attack.pattern
						{
							let shelter_amount: ExpectedValue = if target_is_shelter(state, *target)
							{
								amount
									.checked_mul(*shelter_damage_multiplier)
									.ok_or_else(|| {
										ModelError::invalid_transition(
											"shelter expected damage overflow",
										)
									})?
							} else {
								amount
							};
							if first_after_block {
								shelter_amount
									.checked_mul(*damage_multiplier)
									.ok_or_else(|| {
										ModelError::invalid_transition(
											"first-after-block expected damage overflow",
										)
									})?
							} else {
								shelter_amount
							}
						} else {
							amount
						};
					for _ in 0..hit_count {
						events.push(resolve_effect_event(
							actor,
							source_deployment_generation,
							vec![*target],
							TaggedEffect {
								mechanic: MechanicId::new("normal_attack"),
								target: EffectTarget::CurrentTargets,
								effect: ExpectedEffect::Damage {
									amount: target_amount,
									damage_type,
									delivery,
								},
							},
							resolve_at,
						));
					}
				}
			}
			NormalAttackAction::Heal => {
				for _ in 0..hit_count {
					events.push(resolve_effect_event(
						actor,
						source_deployment_generation,
						targets.clone(),
						TaggedEffect {
							mechanic: MechanicId::new("normal_heal"),
							target: EffectTarget::CurrentTargets,
							effect: ExpectedEffect::Healing { amount },
						},
						at,
					));
				}
			}
			NormalAttackAction::None => {}
		}
		if let Some(shift) = shift {
			events.push(resolve_effect_event(
				actor,
				source_deployment_generation,
				targets.clone(),
				TaggedEffect {
					mechanic: MechanicId::new("power_hit"),
					target: EffectTarget::CurrentTargets,
					effect: ExpectedEffect::Shift(shift),
				},
				at,
			));
		}
		let hit_targets: Vec<BattleActorId> = targets.clone();
		events.extend(effect_events(
			actor,
			source_deployment_generation,
			&attack.extra_effects,
			targets,
			at,
		));
		for target in hit_targets {
			for _ in 0..hit_count {
				events.extend(fire_trigger(
					state,
					TriggerContext {
						kind: ReplayTriggerKind::NormalAttackHit,
						actor: target,
						effect_source: actor,
					},
					at,
				)?);
			}
		}
	}
	events.extend(charge_sp_for_attack(state, actor, at)?);
	events.push(schedule_next_attack(state, actor, at)?);
	Ok(events)
}

fn start_axis_direct_hit(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	facts: EnemyAxisDirectHitFacts,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if facts.projectiles == 0 {
		return Err(ModelError::invalid_transition(
			"axis direct hit must fire at least one projectile",
		));
	}
	if facts.windup_ticks > facts.skill_duration_ticks {
		return Err(ModelError::invalid_transition(
			"axis direct hit windup exceeds skill duration",
		));
	}
	let active = state
		.enemies
		.get_mut(&enemy)
		.ok_or_else(|| ModelError::invalid_transition("axis direct hit owner disappeared"))?;
	if active.axis_direct_hit.is_some() {
		return Err(ModelError::invalid_transition(
			"axis direct hit was started more than once",
		));
	}
	let generation: u32 = 0;
	let initial_cooldown_ticks: u64 = facts.initial_cooldown_ticks;
	active.axis_direct_hit = Some(ActiveAxisDirectHit {
		facts,
		generation,
		active_until: None,
	});
	Ok(vec![axis_direct_hit_event(
		enemy,
		generation,
		AxisDirectHitPhase::Ready,
		checked_tick_add(at, initial_cooldown_ticks)?,
	)])
}

fn apply_axis_direct_hit(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	generation: u32,
	phase: AxisDirectHitPhase,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active_skill) = state
		.enemies
		.get(&enemy)
		.and_then(|active| active.axis_direct_hit.as_ref())
		.filter(|active| active.generation == generation)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	match phase {
		AxisDirectHitPhase::Ready => {
			if active_skill.active_until.is_some() {
				return Ok(Vec::new());
			}
			if active_skill.facts.requires_unblocked
				&& state
					.enemies
					.get(&enemy)
					.is_some_and(|active| active.blocked_by.is_some())
			{
				return Ok(vec![axis_direct_hit_event(
					enemy,
					generation,
					AxisDirectHitPhase::Ready,
					checked_tick_add(at, 1)?,
				)]);
			}
			let Some(direction) = axis_direct_hit_direction(state, enemy) else {
				return Ok(vec![axis_direct_hit_event(
					enemy,
					generation,
					AxisDirectHitPhase::Ready,
					checked_tick_add(at, 1)?,
				)]);
			};
			let cast_generation: u32 = generation.checked_add(1).ok_or_else(|| {
				ModelError::invalid_transition("axis direct hit generation overflow")
			})?;
			let end_at: SimTick = checked_tick_add(at, active_skill.facts.skill_duration_ticks)?;
			{
				let active = state
					.enemies
					.get_mut(&enemy)
					.expect("axis direct hit owner was checked");
				let runtime_skill = active
					.axis_direct_hit
					.as_mut()
					.expect("axis direct hit presence was checked");
				runtime_skill.generation = cast_generation;
				runtime_skill.active_until = Some(end_at);
			}
			let mut events: Vec<EventRequest<ExpectedEvent>> =
				reschedule_attack(state, BattleActorId::Enemy(enemy), at)?;
			events.extend(reschedule_action_affected_movement(state, enemy, at)?);
			events.push(axis_direct_hit_event(
				enemy,
				cast_generation,
				AxisDirectHitPhase::Projectile {
					index: 0,
					direction,
				},
				checked_tick_add(at, active_skill.facts.windup_ticks)?,
			));
			events.push(axis_direct_hit_event(
				enemy,
				cast_generation,
				AxisDirectHitPhase::End,
				end_at,
			));
			Ok(events)
		}
		AxisDirectHitPhase::Projectile { index, direction } => {
			if active_skill.active_until.is_none() || index >= active_skill.facts.projectiles {
				return Ok(Vec::new());
			}
			let source: BattleActorId = BattleActorId::Enemy(enemy);
			let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
			if let Some(target) = axis_direct_hit_collision(state, enemy, direction) {
				let attack: ExpectedValue = effective_actor_stat(state, source, StatKind::Attack)?;
				let amount: ExpectedValue = attack
					.checked_mul(active_skill.facts.damage_multiplier)
					.ok_or_else(|| {
						ModelError::invalid_transition("axis direct hit damage overflow")
					})?;
				events.extend(apply_damage(
					state,
					source,
					target,
					amount,
					active_skill.facts.damage_type,
					active_skill.facts.delivery,
					at,
				)?);
				if let Some(status) = &active_skill.facts.status {
					events.extend(apply_status(state, target, status.clone(), at)?);
				}
			}
			let next_index: u16 = index
				.checked_add(1)
				.ok_or_else(|| ModelError::invalid_transition("axis projectile index overflow"))?;
			if next_index < active_skill.facts.projectiles {
				events.push(axis_direct_hit_event(
					enemy,
					generation,
					AxisDirectHitPhase::Projectile {
						index: next_index,
						direction,
					},
					at,
				));
			}
			Ok(events)
		}
		AxisDirectHitPhase::End => {
			if active_skill.active_until != Some(at) {
				return Ok(Vec::new());
			}
			let cooldown_ticks: u64 = active_skill.facts.cooldown_ticks;
			state
				.enemies
				.get_mut(&enemy)
				.expect("axis direct hit owner was checked")
				.axis_direct_hit
				.as_mut()
				.expect("axis direct hit presence was checked")
				.active_until = None;
			let mut events: Vec<EventRequest<ExpectedEvent>> = vec![axis_direct_hit_event(
				enemy,
				generation,
				AxisDirectHitPhase::Ready,
				checked_tick_add(at, cooldown_ticks)?,
			)];
			events.extend(reschedule_attack(state, BattleActorId::Enemy(enemy), at)?);
			events.extend(reschedule_action_affected_movement(state, enemy, at)?);
			Ok(events)
		}
	}
}

fn apply_stealth_recovered(state: &mut ExpectedState, enemy: EnemyInstanceId, generation: u32) {
	let Some(active) = state.enemies.get_mut(&enemy) else {
		return;
	};
	if active.stealth_recovery_generation == generation && active.blocked_by.is_none() {
		active.stealth_recovery_pending = false;
	}
}

fn resolve_effect_step(
	mut state: ExpectedState,
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	event_targets: &[BattleActorId],
	tagged: &TaggedEffect,
	effect_priority: i32,
	at: SimTick,
) -> Result<ModelStep<ExpectedState, ExpectedEvent>, ModelError> {
	let repeat_events: Vec<EventRequest<ExpectedEvent>> =
		repeat_stage_effect(&state, source, tagged, at);
	let ExpectedEffect::Expected {
		probability,
		effect,
	} = &tagged.effect
	else {
		let mut events: Vec<EventRequest<ExpectedEvent>> = resolve_effect(
			&mut state,
			source,
			source_deployment_generation,
			event_targets,
			tagged,
			effect_priority,
			at,
		)?;
		events.extend(repeat_events);
		return Ok(ModelStep::Deterministic(Transition::new(state, events)));
	};
	let ExpectedEffect::Damage {
		amount,
		damage_type,
		delivery,
	} = effect.as_ref()
	else {
		return Err(ModelError::invalid_transition(
			"uncompiled non-damage expected effect reached execution",
		));
	};
	let targets: Vec<BattleActorId> = effect_targets(&state, source, event_targets, tagged.target)
		.into_iter()
		.filter(|target| {
			actor_exists(&state, *target) && actor_hp(&state, *target) != Some(ExpectedValue::ZERO)
		})
		.collect();
	if state.scenario.mode == ReplayDamageMode::DeterministicExpected
		|| targets.is_empty()
		|| targets
			.iter()
			.all(|target| actor_faction(&state, *target) == Some(Faction::Hostile))
	{
		let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
		for target in targets {
			let reaction: Option<(u64, ExpectedValue)> =
				expose_damage_source_reaction(&state, target);
			let hit_damage: ExpectedValue =
				damage_after_mitigation(&state, source, target, *amount, *damage_type, *delivery)?;
			let expected_damage: ExpectedValue =
				hit_damage.checked_mul(*probability).ok_or_else(|| {
					ModelError::invalid_transition("expected damage multiplication overflow")
				})?;
			events.extend(apply_mitigated_damage(
				&mut state,
				source,
				target,
				expected_damage,
				at,
			)?);
			if let Some((duration_ticks, incoming_damage_multiplier)) = reaction {
				if source != target && actor_runtime(&state, source).is_some() {
					events.extend(apply_status(
						&mut state,
						source,
						StatusEffect {
							kind: StatusKind::Exposure,
							duration_ticks,
							movement_interval_multiplier: ExpectedValue::ONE,
							attack_speed_delta: 0,
							incoming_damage_multiplier,
							arts_resistance_reduction: ExpectedValue::ZERO,
							arts_resistance_reduction_target: StatusFactionFilter::Any,
						},
						at,
					)?);
				}
			}
		}
		events.extend(repeat_events);
		return Ok(ModelStep::Deterministic(Transition::new(state, events)));
	}
	if targets.len() != 1 || actor_faction(&state, targets[0]) != Some(Faction::Friendly) {
		return Err(ModelError::unsupported(
			"defensive probability requires exactly one live friendly damage target",
		));
	}

	let target: BattleActorId = targets[0];
	let choice_id: ChoiceId = ChoiceId::new(state.next_choice_id);
	state.next_choice_id = state
		.next_choice_id
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("defensive choice identity overflow"))?;
	let mut hit_state: ExpectedState = state.clone();
	let mut hit_events: Vec<EventRequest<ExpectedEvent>> = apply_damage(
		&mut hit_state,
		source,
		target,
		*amount,
		*damage_type,
		*delivery,
		at,
	)?;
	hit_events.extend(repeat_events.clone());
	let hit_probability: f64 = probability.scaled() as f64 / ExpectedValue::SCALE as f64;
	let hit = Weighted::new(hit_probability, Transition::new(hit_state, hit_events)).map_err(
		|error| {
			ModelError::invalid_transition(format!("invalid defensive hit probability: {error}"))
		},
	)?;
	let dodge = Weighted::new(1.0 - hit_probability, Transition::new(state, repeat_events))
		.map_err(|error| {
			ModelError::invalid_transition(format!("invalid defensive dodge probability: {error}"))
		})?;
	Ok(ModelStep::Choice {
		choice_id,
		branches: vec![hit, dodge],
	})
}

fn resolve_effect(
	state: &mut ExpectedState,
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	event_targets: &[BattleActorId],
	tagged: &TaggedEffect,
	effect_priority: i32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let targets: Vec<BattleActorId> = effect_targets(state, source, event_targets, tagged.target);
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	match &tagged.effect {
		ExpectedEffect::Damage {
			amount,
			damage_type,
			delivery,
		} => {
			for target in targets {
				events.extend(apply_damage(
					state,
					source,
					target,
					*amount,
					*damage_type,
					*delivery,
					at,
				)?);
			}
		}
		ExpectedEffect::SourceStatScaledDamage {
			stat,
			multiplier,
			damage_type,
			delivery,
		} => {
			let amount: ExpectedValue = effective_actor_stat(state, source, *stat)?
				.checked_mul(*multiplier)
				.ok_or_else(|| {
					ModelError::invalid_transition("source-stat damage arithmetic overflow")
				})?;
			for target in targets {
				events.extend(apply_damage(
					state,
					source,
					target,
					amount,
					*damage_type,
					*delivery,
					at,
				)?);
			}
		}
		ExpectedEffect::Healing { amount } => {
			for target in targets {
				events.extend(apply_healing(state, source, target, *amount, at)?);
			}
		}
		ExpectedEffect::RestoreHpToCurrentMaxRatio { ratio } => {
			for target in targets {
				restore_hp_to_current_max_ratio(state, target, *ratio)?;
			}
		}
		ExpectedEffect::StatModifier(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::Stat(modifier.clone()),
					modifier.duration_ticks,
					at,
				)?);
			}
		}
		ExpectedEffect::ConditionalStatModifier(conditional) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::ConditionalStat(conditional.clone()),
					conditional.modifier.duration_ticks,
					at,
				)?);
			}
		}
		ExpectedEffect::AttackCadence(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::AttackCadence(modifier.clone()),
					modifier.duration_ticks,
					at,
				)?);
				events.extend(reschedule_attack(state, target, at)?);
			}
		}
		ExpectedEffect::AttackSpeed(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::AttackSpeed(*modifier),
					modifier.duration_ticks,
					at,
				)?);
				events.extend(reschedule_attack(state, target, at)?);
			}
		}
		ExpectedEffect::AttackRange(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::AttackRange(modifier.clone()),
					modifier.duration_ticks,
					at,
				)?);
			}
		}
		ExpectedEffect::TargetCount(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::TargetCount(*modifier),
					modifier.duration_ticks(),
					at,
				)?);
			}
		}
		ExpectedEffect::HitCount(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::HitCount(*modifier),
					modifier.duration_ticks,
					at,
				)?);
			}
		}
		ExpectedEffect::GainSp { amount } => {
			for target in targets {
				if let BattleActorId::Operator(operator) = target {
					events.extend(charge_operator_sp(
						state,
						operator,
						ExpectedValue::from_whole(*amount),
						at,
					)?);
				}
			}
		}
		ExpectedEffect::GainDp { amount } => grant_dp(state, *amount, at),
		ExpectedEffect::ActivateSkill => {
			for target in targets {
				if let BattleActorId::Operator(operator) = target {
					if skill_is_ready(state, operator) {
						let (mechanic, deployment_generation): (MechanicId, u32) =
							state
								.deployed
								.get(&operator)
								.and_then(|deployed| {
									deployed.build.skill.as_ref().map(|skill| {
										(skill.id.clone(), deployed.deployment_generation)
									})
								})
								.expect("ready skill exists");
						if let Some(deployed) = state.deployed.get_mut(&operator) {
							deployed.skill_activation_pending = true;
						}
						events.push(skill_activation_event(
							operator,
							mechanic,
							deployment_generation,
							SkillActivationOrigin::Automatic,
							at,
						));
					}
				}
			}
		}
		ExpectedEffect::ApplyStatus(status) => {
			for target in targets {
				events.extend(apply_status(state, target, status.clone(), at)?);
			}
		}
		ExpectedEffect::Shift(shift) => {
			for target in targets {
				if let BattleActorId::Enemy(enemy) = target {
					events.extend(shift_enemy(state, enemy, *shift, at)?);
				}
			}
		}
		ExpectedEffect::Blocking(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::Blocking(*modifier),
					modifier.duration_ticks,
					at,
				)?);
				events.push(reconcile_blocking_event(
					target,
					BlockEndReason::BlockCountChange,
					at,
				));
			}
		}
		ExpectedEffect::BlockTarget(modifier) => {
			for target in targets {
				events.extend(add_modifier(
					state,
					target,
					ActiveModifierKind::BlockTarget(*modifier),
					modifier.duration_ticks,
					at,
				)?);
				events.push(reconcile_blocking_event(
					target,
					BlockEndReason::BlockTargetChange,
					at,
				));
			}
		}
		ExpectedEffect::Summon(summon) => {
			let source_cell: GridPosition =
				actor_cell(state, source).unwrap_or(GridPosition { row: 0, col: 0 });
			let instance: u32 = state.next_summon_id;
			state.next_summon_id = state
				.next_summon_id
				.checked_add(1)
				.ok_or_else(|| ModelError::invalid_transition("summon identity overflow"))?;
			events.push(EventRequest::new(
				at,
				SPAWN_PRIORITY,
				summon_entity(instance),
				ExpectedEvent::SummonSpawn {
					source,
					template: summon.template,
					instance,
					cell: source_cell,
				},
			));
		}
		ExpectedEffect::StartAxisDirectHit(facts) => {
			for target in targets {
				let BattleActorId::Enemy(enemy) = target else {
					return Err(ModelError::invalid_transition(
						"axis direct hit can only start on an enemy",
					));
				};
				events.extend(start_axis_direct_hit(state, enemy, facts.clone(), at)?);
			}
		}
		ExpectedEffect::RegisterListener(listener) => {
			let runtime_source: RuntimeSource =
				runtime_source(state, source, source_deployment_generation);
			events.extend(register_listener(
				state,
				runtime_source,
				listener.clone(),
				at,
			)?);
		}
		ExpectedEffect::RemoveListener { key } => {
			remove_listeners(state, source, source_deployment_generation, key);
		}
		ExpectedEffect::ApplyBuff(buff) => {
			let runtime_source: RuntimeSource =
				runtime_source(state, source, source_deployment_generation);
			for target in targets {
				events.extend(apply_buff(state, runtime_source, target, buff.clone(), at)?);
			}
		}
		ExpectedEffect::InstallConditionalAura(aura) => {
			let runtime_source: RuntimeSource =
				runtime_source(state, source, source_deployment_generation);
			events.extend(install_conditional_aura(
				state,
				runtime_source,
				aura.clone(),
				at,
			)?);
		}
		ExpectedEffect::InstallSkillInactiveStance(stance) => {
			let runtime_source: RuntimeSource =
				runtime_source(state, source, source_deployment_generation);
			events.extend(install_skill_inactive_stance(
				state,
				runtime_source,
				stance.clone(),
				at,
			)?);
		}
		ExpectedEffect::RemoveBuff {
			key,
			source: removal_source,
		} => {
			for target in targets {
				events.extend(remove_matching_buffs(
					state,
					target,
					source,
					source_deployment_generation,
					key,
					*removal_source,
					at,
				)?);
			}
		}
		ExpectedEffect::AddTileLayer(tile_effect) => {
			let runtime_source: RuntimeSource =
				runtime_source(state, source, source_deployment_generation);
			let effect_targets: &[BattleActorId] =
				if matches!(tile_effect.cells, ReplayCellSet::EffectTargetRelative(_)) {
					&targets
				} else {
					&[]
				};
			events.extend(add_tile_layer(
				state,
				runtime_source,
				effect_targets,
				tile_effect.clone(),
				at,
			)?);
		}
		ExpectedEffect::InstallTileFieldOverlay(overlay) => {
			let runtime_source: RuntimeSource =
				runtime_source(state, source, source_deployment_generation);
			events.extend(install_tile_field_overlay(
				state,
				runtime_source,
				overlay.clone(),
				at,
			)?);
		}
		ExpectedEffect::RemoveTileEffect { key } => {
			events.extend(remove_matching_tile_effects(
				state,
				source,
				source_deployment_generation,
				key,
				at,
			)?);
		}
		ExpectedEffect::Death => {
			for target in targets {
				events.extend(apply_explicit_death(state, source, target, at)?);
			}
		}
		ExpectedEffect::Sequence(sequence) => {
			events.extend(effect_events_with_priority(
				source,
				source_deployment_generation,
				sequence,
				targets,
				effect_priority,
				at,
			));
		}
		ExpectedEffect::Expected { .. } => {
			return Err(ModelError::invalid_transition(
				"uncompiled expected effect reached execution",
			));
		}
	}
	Ok(events)
}

fn effect_targets(
	state: &ExpectedState,
	source: BattleActorId,
	event_targets: &[BattleActorId],
	target: EffectTarget,
) -> Vec<BattleActorId> {
	if event_targets.is_empty() {
		select_effect_targets(state, source, target)
	} else {
		event_targets.to_vec()
	}
}

fn allocate_runtime_effect_id(state: &mut ExpectedState) -> Result<u64, ModelError> {
	let id: u64 = state.next_runtime_effect_id;
	state.next_runtime_effect_id = state
		.next_runtime_effect_id
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("runtime effect identity overflow"))?;
	Ok(id)
}

fn resolve_replay_cells(
	state: &ExpectedState,
	source: BattleActorId,
	effect_target: Option<BattleActorId>,
	cells: &ReplayCellSet,
) -> Result<BTreeSet<GridPosition>, ModelError> {
	match cells {
		ReplayCellSet::Explicit(cells) => Ok(cells.clone()),
		ReplayCellSet::SourceCell => actor_cell(state, source)
			.map(|cell| BTreeSet::from([cell]))
			.ok_or_else(|| ModelError::invalid_transition("cell-set source is not on the field")),
		ReplayCellSet::SourceCurrentAttackRangeGroundCells => {
			let attack: &AttackProfile = actor_attack_profile(state, source).ok_or_else(|| {
				ModelError::invalid_transition(
					"current-attack-range cell-set source has no attack profile",
				)
			})?;
			Ok(effective_range_cells(state, source, attack)
				.intersection(&state.scenario.battle.facts.grid.ground_cells)
				.copied()
				.collect())
		}
		ReplayCellSet::EffectTargetRelative(offsets) => {
			let target: BattleActorId = effect_target.ok_or_else(|| {
				ModelError::invalid_transition(
					"effect-target-relative cell set has no current effect target",
				)
			})?;
			if actor_cell(state, target).is_none() {
				return Err(ModelError::invalid_transition(
					"effect-target-relative cell-set target is not on the field",
				));
			}
			Ok(projected_range_cells(
				state,
				target,
				offsets.iter().copied(),
			))
		}
		ReplayCellSet::SourceRelative(offsets) => {
			if actor_cell(state, source).is_none() {
				return Err(ModelError::invalid_transition(
					"relative cell-set source is not on the field",
				));
			}
			Ok(projected_range_cells(
				state,
				source,
				offsets.iter().copied(),
			))
		}
	}
}

fn register_listener(
	state: &mut ExpectedState,
	mut source: RuntimeSource,
	spec: ReplayListener,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if spec.lifetime != RuntimeEffectLifetime::Persistent && !runtime_source_is_live(state, source)
	{
		return Ok(Vec::new());
	}
	source = runtime_source_for_lifetime(source, spec.lifetime);
	let cells: BTreeSet<GridPosition> = match &spec.trigger {
		ReplayTrigger::ActorEnteredCells { cells } | ReplayTrigger::ActorLeftCells { cells } => {
			resolve_replay_cells(state, source.actor, None, cells)?
		}
		ReplayTrigger::Deploy
		| ReplayTrigger::SkillStarted
		| ReplayTrigger::SkillEnded
		| ReplayTrigger::NormalAttackHit
		| ReplayTrigger::Heal
		| ReplayTrigger::Damage
		| ReplayTrigger::BeforeLethal
		| ReplayTrigger::Death
		| ReplayTrigger::Periodic { .. } => BTreeSet::new(),
	};
	remove_listeners(state, source.actor, source.deployment_generation, &spec.key);
	let id: u64 = allocate_runtime_effect_id(state)?;
	let generation: u32 = 1;
	let periodic_delay: Option<u64> = match spec.trigger {
		ReplayTrigger::Periodic {
			initial_delay_ticks,
			..
		} => Some(initial_delay_ticks),
		_ => None,
	};
	state.listeners.insert(
		id,
		ActiveListener {
			source,
			spec,
			cells,
			generation,
			fired: false,
		},
	);
	let Some(delay) = periodic_delay else {
		return Ok(Vec::new());
	};
	Ok(vec![EventRequest::new(
		checked_tick_add(at, delay)?,
		EFFECT_PRIORITY,
		actor_entity(source.actor),
		ExpectedEvent::ListenerPulse {
			listener_id: id,
			generation,
		},
	)])
}

fn remove_listeners(
	state: &mut ExpectedState,
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	key: &MechanicId,
) {
	state.listeners.retain(|_, listener| {
		listener.source.actor != source
			|| (listener.spec.lifetime != RuntimeEffectLifetime::Persistent
				&& listener.source.deployment_generation != source_deployment_generation)
			|| &listener.spec.key != key
	});
}

fn runtime_source_for_lifetime(
	mut source: RuntimeSource,
	lifetime: RuntimeEffectLifetime,
) -> RuntimeSource {
	if lifetime == RuntimeEffectLifetime::Persistent {
		source.deployment_generation = None;
	}
	source
}

fn runtime_source(
	state: &ExpectedState,
	actor: BattleActorId,
	deployment_generation: Option<u32>,
) -> RuntimeSource {
	RuntimeSource {
		actor,
		deployment_generation,
		faction: actor_faction(state, actor),
	}
}

fn apply_listener_pulse(
	state: &mut ExpectedState,
	listener_id: u64,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(listener) = state
		.listeners
		.get(&listener_id)
		.filter(|listener| listener.generation == generation)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	let ReplayTrigger::Periodic { interval_ticks, .. } = listener.spec.trigger else {
		return Err(ModelError::invalid_transition(
			"non-periodic listener received a pulse",
		));
	};
	if listener.spec.lifetime != RuntimeEffectLifetime::Persistent
		&& !runtime_source_is_live(state, listener.source)
	{
		state.listeners.remove(&listener_id);
		return Ok(Vec::new());
	}
	if listener.spec.repeat == ReplayRepeat::Once && listener.fired {
		return Ok(Vec::new());
	}
	let context = TriggerContext {
		kind: ReplayTriggerKind::Periodic,
		actor: listener.source.actor,
		effect_source: listener.source.actor,
	};
	if !predicate_matches(state, &listener, context)? {
		return Ok(vec![listener_pulse_event(
			listener_id,
			generation,
			listener.source.actor,
			checked_tick_add(at, interval_ticks)?,
		)]);
	}
	if let Some(active) = state.listeners.get_mut(&listener_id) {
		active.fired = true;
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> = effect_events(
		listener.source.actor,
		listener.source.deployment_generation,
		&listener.spec.effects,
		actor_exists(state, listener.source.actor)
			.then_some(listener.source.actor)
			.into_iter()
			.collect(),
		at,
	);
	if listener.spec.repeat == ReplayRepeat::Unlimited {
		events.push(listener_pulse_event(
			listener_id,
			generation,
			listener.source.actor,
			checked_tick_add(at, interval_ticks)?,
		));
	}
	Ok(events)
}

fn listener_pulse_event(
	listener_id: u64,
	generation: u32,
	source: BattleActorId,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		EFFECT_PRIORITY,
		actor_entity(source),
		ExpectedEvent::ListenerPulse {
			listener_id,
			generation,
		},
	)
}

fn runtime_source_is_live(state: &ExpectedState, source: RuntimeSource) -> bool {
	actor_exists(state, source.actor)
		&& match (source.actor, source.deployment_generation) {
			(BattleActorId::Operator(operator), Some(generation)) => state
				.deployed
				.get(&operator)
				.is_some_and(|deployed| deployed.deployment_generation == generation),
			(BattleActorId::Operator(_), None)
			| (BattleActorId::Enemy(_), _)
			| (BattleActorId::Summon(_), _)
			| (BattleActorId::Stage, _) => true,
		}
}

fn fire_trigger(
	state: &mut ExpectedState,
	context: TriggerContext,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let ids: Vec<u64> = state
		.listeners
		.iter()
		.filter_map(|(id, listener)| {
			(listener.spec.trigger.kind() == context.kind
				&& context.kind != ReplayTriggerKind::Periodic
				&& listener_matches_context(state, listener, context))
			.then_some(*id)
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for id in ids {
		events.extend(fire_listener(state, id, context, at)?);
	}
	Ok(events)
}

fn fire_listener(
	state: &mut ExpectedState,
	id: u64,
	context: TriggerContext,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(listener) = state.listeners.get(&id).cloned() else {
		return Ok(Vec::new());
	};
	if listener.spec.repeat == ReplayRepeat::Once && listener.fired {
		return Ok(Vec::new());
	}
	if !predicate_matches(state, &listener, context)? {
		return Ok(Vec::new());
	}
	if let Some(active) = state.listeners.get_mut(&id) {
		active.fired = true;
	}
	let source_generation: Option<u32> =
		if context.kind == ReplayTriggerKind::Death && context.actor == listener.source.actor {
			None
		} else {
			listener.source.deployment_generation
		};
	let priority: i32 = if context.kind == ReplayTriggerKind::BeforeLethal {
		BEFORE_LETHAL_EFFECT_PRIORITY
	} else {
		EFFECT_PRIORITY
	};
	Ok(listener_effect_events(
		listener.source.actor,
		source_generation,
		&listener.spec.effects,
		context,
		priority,
		at,
	))
}

fn listener_matches_context(
	state: &ExpectedState,
	listener: &ActiveListener,
	context: TriggerContext,
) -> bool {
	if listener.spec.lifetime != RuntimeEffectLifetime::Persistent
		&& !runtime_source_is_live(state, listener.source)
	{
		return false;
	}
	actor_filter_matches(
		state,
		listener.source,
		context.actor,
		context.effect_source,
		listener.spec.actor_filter,
	)
}

fn actor_filter_matches(
	state: &ExpectedState,
	source: RuntimeSource,
	actor: BattleActorId,
	effect_source: BattleActorId,
	filter: TriggerActorFilter,
) -> bool {
	match filter {
		TriggerActorFilter::SelfActor => actor == source.actor,
		TriggerActorFilter::EffectSource => effect_source == source.actor,
		TriggerActorFilter::AnyActor => true,
		TriggerActorFilter::AllyOfSource => {
			actor_faction(state, actor).is_some() && actor_faction(state, actor) == source.faction
		}
		TriggerActorFilter::EnemyOfSource => {
			source.faction.is_some()
				&& actor_faction(state, actor).is_some()
				&& actor_faction(state, actor) != source.faction
		}
	}
}

fn tile_actor_matches(
	state: &ExpectedState,
	tile: &ActiveTileEffect,
	actor: BattleActorId,
) -> bool {
	actor_filter_matches(state, tile.source, actor, actor, tile.spec.actor_filter)
		&& actor_combat_facts(state, actor)
			.is_some_and(|combat| tile.spec.target_filter.allows(combat.target_class))
}

fn tile_field_overlay_actor_matches(
	state: &ExpectedState,
	overlay: &ActiveTileFieldOverlay,
	actor: BattleActorId,
) -> bool {
	actor_filter_matches(
		state,
		overlay.source,
		actor,
		actor,
		overlay.spec.actor_filter,
	) && actor_combat_facts(state, actor)
		.is_some_and(|combat| overlay.spec.target_filter.allows(combat.target_class))
}

fn observed_tile_cells(
	state: &ExpectedState,
	overlay: &ActiveTileFieldOverlay,
) -> BTreeSet<GridPosition> {
	state
		.tile_effects
		.values()
		.filter(|tile| {
			tile.source == overlay.source && tile.spec.key == overlay.spec.observed_tile_key
		})
		.map(|tile| tile.cell)
		.collect()
}

fn predicate_matches(
	state: &ExpectedState,
	listener: &ActiveListener,
	context: TriggerContext,
) -> Result<bool, ModelError> {
	fn evaluate(
		state: &ExpectedState,
		listener: &ActiveListener,
		context: TriggerContext,
		predicate: &ReplayPredicate,
	) -> Result<bool, ModelError> {
		match predicate {
			ReplayPredicate::Always => Ok(true),
			ReplayPredicate::HpRatio {
				actor,
				comparison,
				threshold,
			} => {
				let actor: BattleActorId = match actor {
					PredicateActor::ListenerSource => listener.source.actor,
					PredicateActor::TriggerActor => context.actor,
					PredicateActor::EffectSource => context.effect_source,
				};
				let Some(hp) = actor_hp(state, actor) else {
					return Ok(false);
				};
				let max_hp: ExpectedValue = effective_actor_stat(state, actor, StatKind::MaxHp)?;
				let ratio: ExpectedValue = ExpectedValue::from_ratio(hp.scaled(), max_hp.scaled())
					.ok_or_else(|| ModelError::invalid_transition("zero predicate max HP"))?;
				Ok(match comparison {
					HpRatioComparison::LessThan => ratio < *threshold,
					HpRatioComparison::LessThanOrEqual => ratio <= *threshold,
					HpRatioComparison::GreaterThan => ratio > *threshold,
					HpRatioComparison::GreaterThanOrEqual => ratio >= *threshold,
				})
			}
			ReplayPredicate::All(predicates) => {
				for predicate in predicates {
					if !evaluate(state, listener, context, predicate)? {
						return Ok(false);
					}
				}
				Ok(true)
			}
			ReplayPredicate::Any(predicates) => {
				for predicate in predicates {
					if evaluate(state, listener, context, predicate)? {
						return Ok(true);
					}
				}
				Ok(false)
			}
			ReplayPredicate::Not(predicate) => Ok(!evaluate(state, listener, context, predicate)?),
		}
	}
	evaluate(state, listener, context, &listener.spec.predicate)
}

fn actor_presence_events(
	state: &mut ExpectedState,
	actor: BattleActorId,
	before: Option<GridPosition>,
	after: Option<GridPosition>,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if before == after {
		return Ok(Vec::new());
	}
	let listener_ids: Vec<(u64, ReplayTriggerKind)> = state
		.listeners
		.iter()
		.filter_map(|(id, listener)| {
			let (kind, crossed): (ReplayTriggerKind, bool) = match listener.spec.trigger {
				ReplayTrigger::ActorEnteredCells { .. } => (
					ReplayTriggerKind::ActorEnteredCells,
					!before.is_some_and(|cell| listener.cells.contains(&cell))
						&& after.is_some_and(|cell| listener.cells.contains(&cell)),
				),
				ReplayTrigger::ActorLeftCells { .. } => (
					ReplayTriggerKind::ActorLeftCells,
					before.is_some_and(|cell| listener.cells.contains(&cell))
						&& !after.is_some_and(|cell| listener.cells.contains(&cell)),
				),
				_ => return None,
			};
			crossed.then_some((*id, kind))
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for (id, kind) in listener_ids {
		let context = TriggerContext {
			kind,
			actor,
			effect_source: actor,
		};
		if state
			.listeners
			.get(&id)
			.is_some_and(|listener| listener_matches_context(state, listener, context))
		{
			events.extend(fire_listener(state, id, context, at)?);
		}
	}
	let overlays: Vec<(ActiveTileFieldOverlay, BTreeSet<GridPosition>)> = state
		.tile_field_overlays
		.values()
		.cloned()
		.map(|overlay| {
			let observed_cells: BTreeSet<GridPosition> = observed_tile_cells(state, &overlay);
			(overlay, observed_cells)
		})
		.collect();
	let tiles: Vec<(u64, ActiveTileEffect)> = state
		.tile_effects
		.iter()
		.map(|(id, tile)| (*id, tile.clone()))
		.collect();
	for (tile_effect_id, tile) in tiles {
		if !tile_actor_matches(state, &tile, actor) {
			continue;
		}
		let entered: bool = before != Some(tile.cell) && after == Some(tile.cell);
		let left: bool = before == Some(tile.cell) && after != Some(tile.cell);
		if entered {
			events.extend(effect_events(
				tile.source.actor,
				tile.source.deployment_generation,
				&tile.spec.on_enter_once,
				vec![actor],
				at,
			));
		} else if !left {
			continue;
		}
		let effects: &[TaggedEffect] = if entered {
			&tile.spec.on_enter_per_layer
		} else {
			&tile.spec.on_leave_per_layer
		};
		for _ in 0..tile.layers {
			events.extend(effect_events(
				tile.source.actor,
				tile.source.deployment_generation,
				effects,
				vec![actor],
				at,
			));
		}
		if left && tile.spec.remove_on_first_matching_leave {
			events.extend(remove_tile_effect_by_id(
				state,
				tile_effect_id,
				Some(tile.generation),
				Some(actor),
				at,
			)?);
		}
	}
	for (overlay, observed_cells) in overlays {
		if !tile_field_overlay_actor_matches(state, &overlay, actor) {
			continue;
		}
		let Some(before) = before else {
			continue;
		};
		if !observed_cells.contains(&before) {
			continue;
		}
		let left_scope: bool = match overlay.spec.leave_scope {
			TileFieldOverlayLeaveScope::ObservedCell => after != Some(before),
			TileFieldOverlayLeaveScope::ObservedField => {
				!after.is_some_and(|cell| observed_cells.contains(&cell))
			}
		};
		if !left_scope {
			continue;
		}
		events.extend(effect_events(
			overlay.source.actor,
			overlay.source.deployment_generation,
			&overlay.spec.on_leave_once,
			vec![actor],
			at,
		));
	}
	Ok(events)
}

fn all_visible_actors(state: &ExpectedState) -> Vec<BattleActorId> {
	state
		.deployed
		.keys()
		.map(|id| BattleActorId::Operator(*id))
		.chain(state.enemies.keys().map(|id| BattleActorId::Enemy(*id)))
		.chain(state.summons.keys().map(|id| BattleActorId::Summon(*id)))
		.filter(|actor| actor_cell(state, *actor).is_some())
		.collect()
}

fn add_tile_layer(
	state: &mut ExpectedState,
	mut source: RuntimeSource,
	effect_targets: &[BattleActorId],
	spec: RuntimeTileEffect,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if spec.lifetime != RuntimeEffectLifetime::Persistent && !runtime_source_is_live(state, source)
	{
		return Ok(Vec::new());
	}
	source = runtime_source_for_lifetime(source, spec.lifetime);
	let mut cells: BTreeSet<GridPosition> = BTreeSet::new();
	if matches!(spec.cells, ReplayCellSet::EffectTargetRelative(_)) {
		for target in effect_targets {
			cells.extend(resolve_replay_cells(
				state,
				source.actor,
				Some(*target),
				&spec.cells,
			)?);
		}
	} else {
		cells.extend(resolve_replay_cells(
			state,
			source.actor,
			None,
			&spec.cells,
		)?);
	}
	if matches!(
		spec.cells,
		ReplayCellSet::SourceCurrentAttackRangeGroundCells
	) {
		let matching_overlays: Vec<ActiveTileFieldOverlay> = state
			.tile_field_overlays
			.values()
			.filter(|overlay| {
				overlay.source == source
					&& overlay.spec.observed_tile_key == spec.key
					&& overlay.spec.cached_propagation.is_some()
			})
			.cloned()
			.collect();
		if matching_overlays.len() > 1 {
			return Err(ModelError::unsupported(format!(
				"tile {:?} has multiple active cached-propagation overlays",
				spec.key.as_str()
			)));
		}
		if let Some(overlay) = matching_overlays.into_iter().next() {
			return apply_cached_tile_layer_pulse(state, source, spec, overlay, at);
		}
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for cell in cells {
		let mut cell_spec: RuntimeTileEffect = spec.clone();
		cell_spec.cells = ReplayCellSet::Explicit(BTreeSet::from([cell]));
		events.extend(add_tile_cell_layer(state, source, cell, cell_spec, at)?);
	}
	Ok(events)
}

fn apply_cached_tile_layer_pulse(
	state: &mut ExpectedState,
	source: RuntimeSource,
	spec: RuntimeTileEffect,
	overlay: ActiveTileFieldOverlay,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let propagation = overlay
		.spec
		.cached_propagation
		.clone()
		.expect("caller selected an overlay with cached propagation");
	let attack: &AttackProfile = actor_attack_profile(state, source.actor).ok_or_else(|| {
		ModelError::invalid_transition("cached tile propagation source has no attack profile")
	})?;
	let current_range: BTreeSet<GridPosition> = effective_range_cells(state, source.actor, attack)
		.into_iter()
		.filter(|cell| {
			state
				.scenario
				.battle
				.facts
				.grid
				.cells
				.get(cell)
				.is_some_and(|metadata| metadata.is_walkable_lowland())
		})
		.collect();

	let existing_tiles: Vec<(u64, ActiveTileEffect)> = state
		.tile_effects
		.iter()
		.filter(|(_, active)| active.source == source && active.spec.key == spec.key)
		.map(|(id, active)| (*id, active.clone()))
		.collect();
	let mut existing_by_cell: BTreeMap<GridPosition, (u64, ActiveTileEffect)> = BTreeMap::new();
	for (id, active) in existing_tiles {
		if !same_tile_component_spec(&active.spec, &spec) {
			return Err(ModelError::invalid_transition(
				"cached propagation observed tile cells with inconsistent specifications",
			));
		}
		if existing_by_cell.insert(active.cell, (id, active)).is_some() {
			return Err(ModelError::invalid_transition(
				"cached propagation observed duplicate source/key/cell tile state",
			));
		}
	}

	let mut pulse_cells: BTreeMap<GridPosition, LayeredTileCell> = existing_by_cell
		.iter()
		.map(|(cell, (_, active))| {
			let relation: SourceRangeRelation = if current_range.contains(cell) {
				SourceRangeRelation::Inside
			} else {
				SourceRangeRelation::Outside
			};
			(
				*cell,
				LayeredTileCell {
					layers: active.layers,
					persistence: active.persistence,
					source_range: relation,
				},
			)
		})
		.collect();
	for cell in &current_range {
		pulse_cells
			.entry(*cell)
			.or_insert_with(|| LayeredTileCell::permanent(0, SourceRangeRelation::Inside));
	}

	let eligible_destinations: BTreeMap<GridPosition, PropagationDestination> = state
		.scenario
		.battle
		.facts
		.grid
		.cells
		.iter()
		.filter(|(_, metadata)| metadata.is_walkable_lowland())
		.map(|(cell, _)| {
			let destination: PropagationDestination = if current_range.contains(cell) {
				PropagationDestination::permanent(SourceRangeRelation::Inside)
			} else {
				PropagationDestination::temporary(SourceRangeRelation::Outside)
			};
			(*cell, destination)
		})
		.collect();
	let neighbors_by_source: BTreeMap<GridPosition, OrderedOrthogonalNeighbors<GridPosition>> =
		pulse_cells
			.keys()
			.map(|cell| {
				(
					*cell,
					ordered_orthogonal_neighbors(
						*cell,
						state.scenario.battle.facts.grid.dimensions,
					),
				)
			})
			.collect();
	let resolution = resolve_cached_tile_layer_pulse(CachedTileLayerPulse {
		cells: pulse_cells,
		eligible_destinations,
		neighbors_by_source,
		maximum_layers: spec.max_layers,
		maximum_outside_temporary_cells: propagation.maximum_outside_temporary_cells,
		source_order: propagation.source_order,
		contribution_merge: propagation.contribution_merge,
	})
	.map_err(|error| cached_propagation_error(&overlay.spec.key, &spec.key, error))?;
	let outcome = match resolution {
		TileLayerPropagationResolution::Resolved(outcome) => outcome,
		TileLayerPropagationResolution::Ambiguous(ambiguity) => {
			return Err(cached_propagation_ambiguity(
				&overlay.spec.key,
				&spec.key,
				ambiguity,
			));
		}
	};

	let touched_cells: BTreeSet<GridPosition> = outcome
		.events
		.iter()
		.map(|event| match event {
			TileLayerEvent::LayersAdded { cell, .. }
			| TileLayerEvent::SaturatedAttempt { cell, .. } => *cell,
		})
		.collect();
	let removed_cells: Vec<GridPosition> = existing_by_cell
		.keys()
		.filter(|cell| !outcome.cells.contains_key(cell))
		.copied()
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for cell in removed_cells {
		if let Some((id, _)) = existing_by_cell.get(&cell) {
			events.extend(remove_tile_effect_by_id(state, *id, None, None, at)?);
		}
	}

	let mut runtime_ids: BTreeMap<GridPosition, u64> = BTreeMap::new();
	for (cell, resolved) in &outcome.cells {
		let id: u64 = if let Some((id, _)) = existing_by_cell.get(cell) {
			let active: &mut ActiveTileEffect = state
				.tile_effects
				.get_mut(id)
				.expect("cached propagation source tile still exists");
			active.layers = resolved.layers;
			active.persistence = resolved.persistence;
			if touched_cells.contains(cell) {
				active.generation = active
					.generation
					.checked_add(1)
					.ok_or_else(|| ModelError::invalid_transition("tile generation overflow"))?;
			}
			*id
		} else {
			let id: u64 = allocate_runtime_effect_id(state)?;
			let mut cell_spec: RuntimeTileEffect = spec.clone();
			cell_spec.cells = ReplayCellSet::Explicit(BTreeSet::from([*cell]));
			state.tile_effects.insert(
				id,
				ActiveTileEffect {
					source,
					spec: cell_spec,
					cell: *cell,
					layers: resolved.layers,
					persistence: resolved.persistence,
					generation: 1,
				},
			);
			id
		};
		runtime_ids.insert(*cell, id);
	}

	for event in outcome.events {
		let (cell, added_layers, transition): (
			GridPosition,
			Option<u16>,
			ObservedTileLayerTransition,
		) = match &event {
			TileLayerEvent::LayersAdded {
				cell,
				previous_layers,
				added_layers,
				current_layers,
				..
			} => (
				*cell,
				Some(*added_layers),
				ObservedTileLayerTransition::LayersAdded {
					previous_layers: *previous_layers,
					current_layers: *current_layers,
				},
			),
			TileLayerEvent::SaturatedAttempt {
				cell, at_layers, ..
			} => (
				*cell,
				None,
				ObservedTileLayerTransition::SaturatedAttempt {
					at_layers: *at_layers,
				},
			),
		};
		let id: u64 = runtime_ids[&cell];
		let active: ActiveTileEffect = state
			.tile_effects
			.get(&id)
			.expect("resolved cached propagation tile exists")
			.clone();
		if let Some(added_layers) = added_layers {
			for actor in all_visible_actors(state)
				.into_iter()
				.filter(|actor| actor_cell(state, *actor) == Some(cell))
				.filter(|actor| tile_actor_matches(state, &active, *actor))
			{
				if !existing_by_cell.contains_key(&cell) {
					events.extend(effect_events(
						source.actor,
						source.deployment_generation,
						&active.spec.on_enter_once,
						vec![actor],
						at,
					));
				}
				if active.spec.apply_new_layer_to_occupants {
					for _ in 0..added_layers {
						events.extend(effect_events(
							source.actor,
							source.deployment_generation,
							&active.spec.on_enter_per_layer,
							vec![actor],
							at,
						));
					}
				}
			}
		}
		events.extend(tile_layer_threshold_reaction_events(
			state, id, &active, transition, at,
		)?);
		events.push(tile_layer_propagation_audit_event(
			source.actor,
			overlay.spec.key.clone(),
			spec.key.clone(),
			event,
			at,
		));
	}
	for cell in touched_cells {
		let id: u64 = runtime_ids[&cell];
		let active: &ActiveTileEffect = state
			.tile_effects
			.get(&id)
			.expect("touched cached propagation tile exists");
		if let Some(duration) = active.spec.duration_ticks {
			events.push(EventRequest::new(
				checked_tick_add(at, duration)?,
				EFFECT_PRIORITY,
				actor_entity(source.actor),
				ExpectedEvent::TileEffectExpired {
					tile_effect_id: id,
					generation: active.generation,
				},
			));
		}
		if let Some(periodic) = &active.spec.periodic {
			events.push(EventRequest::new(
				checked_tick_add(at, periodic.initial_delay_ticks)?,
				EFFECT_PRIORITY,
				actor_entity(source.actor),
				ExpectedEvent::TileEffectPulse {
					tile_effect_id: id,
					generation: active.generation,
				},
			));
		}
	}
	Ok(events)
}

fn same_tile_component_spec(left: &RuntimeTileEffect, right: &RuntimeTileEffect) -> bool {
	let mut left: RuntimeTileEffect = left.clone();
	let mut right: RuntimeTileEffect = right.clone();
	left.cells = ReplayCellSet::Explicit(BTreeSet::new());
	right.cells = ReplayCellSet::Explicit(BTreeSet::new());
	left == right
}

fn ordered_orthogonal_neighbors(
	cell: GridPosition,
	dimensions: MapDimensions,
) -> OrderedOrthogonalNeighbors<GridPosition> {
	let down: Option<GridPosition> = cell
		.row
		.checked_sub(1)
		.map(|row| GridPosition { row, col: cell.col });
	let left: Option<GridPosition> = cell
		.col
		.checked_sub(1)
		.map(|col| GridPosition { row: cell.row, col });
	let up: Option<GridPosition> = cell
		.row
		.checked_add(1)
		.filter(|row| *row < dimensions.height)
		.map(|row| GridPosition { row, col: cell.col });
	let right: Option<GridPosition> = cell
		.col
		.checked_add(1)
		.filter(|col| *col < dimensions.width)
		.map(|col| GridPosition { row: cell.row, col });
	OrderedOrthogonalNeighbors {
		down,
		left,
		up,
		right,
	}
}

fn cached_propagation_error(
	overlay: &MechanicId,
	tile: &MechanicId,
	error: TileLayerPropagationError<GridPosition>,
) -> ModelError {
	ModelError::invalid_transition(format!(
		"cached tile propagation overlay {:?} for tile {:?} violated its runtime invariants: {error:?}",
		overlay.as_str(),
		tile.as_str()
	))
}

fn cached_propagation_ambiguity(
	overlay: &MechanicId,
	tile: &MechanicId,
	ambiguity: TileLayerPropagationAmbiguity<GridPosition>,
) -> ModelError {
	ModelError::unsupported(format!(
		"cached tile propagation overlay {:?} for tile {:?} is ambiguous: {ambiguity:?}",
		overlay.as_str(),
		tile.as_str()
	))
}

fn tile_layer_propagation_audit_event(
	source: BattleActorId,
	overlay: MechanicId,
	tile: MechanicId,
	event: TileLayerEvent<GridPosition>,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	let event: RuntimeTileLayerPropagationAudit = match event {
		TileLayerEvent::LayersAdded {
			cell,
			cause,
			previous_layers,
			added_layers,
			current_layers,
		} => RuntimeTileLayerPropagationAudit::LayersAdded {
			cell,
			cause: runtime_tile_layer_mutation_cause(cause),
			previous_layers,
			added_layers,
			current_layers,
		},
		TileLayerEvent::SaturatedAttempt {
			cell,
			cause,
			at_layers,
			attempted_layers,
		} => RuntimeTileLayerPropagationAudit::SaturatedAttempt {
			cell,
			cause: runtime_tile_layer_mutation_cause(cause),
			at_layers,
			attempted_layers,
		},
	};
	EventRequest::new(
		at,
		TRIGGER_PRIORITY,
		actor_entity(source),
		ExpectedEvent::TileLayerPropagationAudit {
			overlay,
			tile,
			event,
		},
	)
}

fn runtime_tile_layer_mutation_cause(
	cause: LayerMutationCause<GridPosition>,
) -> RuntimeTileLayerMutationCause {
	match cause {
		LayerMutationCause::PulseIncrement => RuntimeTileLayerMutationCause::PulseIncrement,
		LayerMutationCause::CachedPropagation {
			contributing_sources,
		} => RuntimeTileLayerMutationCause::CachedPropagation {
			contributing_sources,
		},
	}
}

fn tile_layer_threshold_reaction_events(
	state: &mut ExpectedState,
	tile_effect_id: u64,
	active: &ActiveTileEffect,
	transition: ObservedTileLayerTransition,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let mut bound: Vec<BoundTileLayerThresholdReaction> = active
		.spec
		.layer_threshold_reactions
		.iter()
		.copied()
		.map(|reaction| BoundTileLayerThresholdReaction {
			source: active.source,
			owner: TileLayerReactionOwner::TileEffect,
			reaction,
		})
		.collect();
	for (overlay_id, overlay) in &state.tile_field_overlays {
		if overlay.source != active.source
			|| overlay.spec.observed_tile_key != active.spec.key
			|| overlay.spec.layer_threshold_reactions.is_empty()
		{
			continue;
		}
		if overlay.spec.observed_tile_max_layers != Some(active.spec.max_layers) {
			return Err(ModelError::unsupported(format!(
				"tile-field overlay {:?} expected a different layer cap for observed tile {:?}",
				overlay.spec.key.as_str(),
				active.spec.key.as_str()
			)));
		}
		bound.extend(
			overlay
				.spec
				.layer_threshold_reactions
				.iter()
				.copied()
				.map(|reaction| BoundTileLayerThresholdReaction {
					source: overlay.source,
					owner: TileLayerReactionOwner::TileFieldOverlay {
						overlay_id: *overlay_id,
						overlay_generation: overlay.generation,
					},
					reaction,
				}),
		);
	}
	for entry in &bound {
		validate_reaction_against_active_tile(entry.reaction, active)?;
	}
	let matching: Vec<BoundTileLayerThresholdReaction> = bound
		.into_iter()
		.filter(|entry| tile_layer_reaction_matches(entry.reaction, transition))
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::with_capacity(matching.len());
	for entry in matching {
		let TileLayerThresholdReactionEffect::SpawnActor(TargetCellActorSpawn {
			template,
			collision,
			ownership,
		}) = entry.reaction.effect;
		let instance: u32 = state.next_summon_id;
		state.next_summon_id = state
			.next_summon_id
			.checked_add(1)
			.ok_or_else(|| ModelError::invalid_transition("summon identity overflow"))?;
		events.push(EventRequest::new(
			at,
			SPAWN_PRIORITY,
			summon_entity(instance),
			ExpectedEvent::TargetCellActorSpawn(TargetCellActorSpawnEvent {
				source: entry.source.actor,
				source_deployment_generation: entry.source.deployment_generation,
				tile_effect_id,
				tile_effect_generation: active.generation,
				reaction_owner: entry.owner,
				template,
				instance,
				cell: active.cell,
				collision,
				ownership,
			}),
		));
	}
	Ok(events)
}

fn validate_reaction_against_active_tile(
	reaction: RuntimeTileLayerThresholdReaction,
	active: &ActiveTileEffect,
) -> Result<(), ModelError> {
	if reaction.threshold == 0 || reaction.threshold > active.spec.max_layers {
		return Err(ModelError::invalid_transition(
			"tile-layer reaction threshold is outside the observed tile layer range",
		));
	}
	if reaction.transition == TileLayerThresholdTransition::SaturatedAttempt
		&& reaction.threshold != active.spec.max_layers
	{
		return Err(ModelError::invalid_transition(
			"saturated tile-layer reaction threshold differs from the observed tile cap",
		));
	}
	Ok(())
}

fn tile_layer_reaction_matches(
	reaction: RuntimeTileLayerThresholdReaction,
	transition: ObservedTileLayerTransition,
) -> bool {
	match (reaction.transition, transition) {
		(
			TileLayerThresholdTransition::ReachedFromBelow,
			ObservedTileLayerTransition::LayersAdded {
				previous_layers,
				current_layers,
			},
		) => previous_layers < reaction.threshold && current_layers >= reaction.threshold,
		(
			TileLayerThresholdTransition::SaturatedAttempt,
			ObservedTileLayerTransition::SaturatedAttempt { at_layers },
		) => at_layers == reaction.threshold,
		(
			TileLayerThresholdTransition::ReachedFromBelow,
			ObservedTileLayerTransition::SaturatedAttempt { .. },
		)
		| (
			TileLayerThresholdTransition::SaturatedAttempt,
			ObservedTileLayerTransition::LayersAdded { .. },
		) => false,
	}
}

fn add_tile_cell_layer(
	state: &mut ExpectedState,
	source: RuntimeSource,
	cell: GridPosition,
	spec: RuntimeTileEffect,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let existing_id: Option<u64> = state.tile_effects.iter().find_map(|(id, active)| {
		(active.source == source && active.spec.key == spec.key && active.cell == cell)
			.then_some(*id)
	});
	let id: u64 = match existing_id {
		Some(id) => id,
		None => allocate_runtime_effect_id(state)?,
	};
	let (generation, added_layer, created, previous_layers, layers): (u32, bool, bool, u16, u16) =
		if let Some(active) = state.tile_effects.get_mut(&id) {
			if active.spec != spec {
				return Err(ModelError::invalid_transition(
					"reapplied tile key changed its component specification",
				));
			}
			let previous_layers: u16 = active.layers;
			let added_layer: bool = previous_layers < active.spec.max_layers;
			if added_layer {
				active.layers += 1;
			}
			active.generation = active
				.generation
				.checked_add(1)
				.ok_or_else(|| ModelError::invalid_transition("tile generation overflow"))?;
			(
				active.generation,
				added_layer,
				false,
				previous_layers,
				active.layers,
			)
		} else {
			state.tile_effects.insert(
				id,
				ActiveTileEffect {
					source,
					spec: spec.clone(),
					cell,
					layers: 1,
					persistence: TilePersistence::Permanent,
					generation: 1,
				},
			);
			(1, true, true, 0, 1)
		};
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	let active_tile: ActiveTileEffect = state
		.tile_effects
		.get(&id)
		.expect("tile effect was inserted or updated")
		.clone();
	if created || (added_layer && spec.apply_new_layer_to_occupants) {
		for actor in all_visible_actors(state)
			.into_iter()
			.filter(|actor| actor_cell(state, *actor) == Some(cell))
			.filter(|actor| tile_actor_matches(state, &active_tile, *actor))
		{
			if created {
				events.extend(effect_events(
					source.actor,
					source.deployment_generation,
					&spec.on_enter_once,
					vec![actor],
					at,
				));
			}
			if !spec.apply_new_layer_to_occupants {
				continue;
			}
			events.extend(effect_events(
				source.actor,
				source.deployment_generation,
				&spec.on_enter_per_layer,
				vec![actor],
				at,
			));
		}
	}
	events.extend(tile_layer_threshold_reaction_events(
		state,
		id,
		&active_tile,
		if added_layer {
			ObservedTileLayerTransition::LayersAdded {
				previous_layers,
				current_layers: layers,
			}
		} else {
			ObservedTileLayerTransition::SaturatedAttempt { at_layers: layers }
		},
		at,
	)?);
	if let Some(duration) = spec.duration_ticks {
		events.push(EventRequest::new(
			checked_tick_add(at, duration)?,
			EFFECT_PRIORITY,
			actor_entity(source.actor),
			ExpectedEvent::TileEffectExpired {
				tile_effect_id: id,
				generation,
			},
		));
	}
	if let Some(periodic) = spec.periodic {
		events.push(EventRequest::new(
			checked_tick_add(at, periodic.initial_delay_ticks)?,
			EFFECT_PRIORITY,
			actor_entity(source.actor),
			ExpectedEvent::TileEffectPulse {
				tile_effect_id: id,
				generation,
			},
		));
	}
	Ok(events)
}

fn apply_tile_effect_pulse(
	state: &mut ExpectedState,
	tile_effect_id: u64,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state
		.tile_effects
		.get(&tile_effect_id)
		.filter(|active| active.generation == generation)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	let Some(periodic) = active.spec.periodic.clone() else {
		return Err(ModelError::invalid_transition(
			"non-periodic tile effect received a pulse",
		));
	};
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for actor in all_visible_actors(state)
		.into_iter()
		.filter(|actor| actor_cell(state, *actor) == Some(active.cell))
		.filter(|actor| tile_actor_matches(state, &active, *actor))
	{
		for _ in 0..active.layers {
			events.extend(effect_events(
				active.source.actor,
				active.source.deployment_generation,
				&periodic.effects_per_layer,
				vec![actor],
				at,
			));
		}
	}
	events.push(EventRequest::new(
		checked_tick_add(at, periodic.interval_ticks)?,
		EFFECT_PRIORITY,
		actor_entity(active.source.actor),
		ExpectedEvent::TileEffectPulse {
			tile_effect_id,
			generation,
		},
	));
	Ok(events)
}

fn install_tile_field_overlay(
	state: &mut ExpectedState,
	mut source: RuntimeSource,
	spec: RuntimeTileFieldOverlay,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if spec.lifetime != RuntimeEffectLifetime::SkillActivation {
		return Err(ModelError::invalid_transition(
			"tile-field overlay lifetime must be skill activation",
		));
	}
	if !runtime_source_is_live(state, source) {
		return Ok(Vec::new());
	}
	source = runtime_source_for_lifetime(source, spec.lifetime);
	let existing_id: Option<u64> = state.tile_field_overlays.iter().find_map(|(id, active)| {
		(active.source == source && active.spec.key == spec.key).then_some(*id)
	});
	let id: u64 = existing_id.unwrap_or(allocate_runtime_effect_id(state)?);
	let generation: u32 = if let Some(active) = state.tile_field_overlays.get_mut(&id) {
		if active.spec != spec {
			return Err(ModelError::invalid_transition(
				"reinstalled tile-field overlay key changed its specification",
			));
		}
		active.generation = active.generation.checked_add(1).ok_or_else(|| {
			ModelError::invalid_transition("tile-field overlay generation overflow")
		})?;
		active.generation
	} else {
		state.tile_field_overlays.insert(
			id,
			ActiveTileFieldOverlay {
				source,
				spec: spec.clone(),
				generation: 1,
			},
		);
		1
	};
	Ok(vec![EventRequest::new(
		checked_tick_add(at, spec.initial_delay_ticks)?,
		EFFECT_PRIORITY,
		actor_entity(source.actor),
		ExpectedEvent::TileFieldOverlayPulse {
			overlay_id: id,
			generation,
		},
	)])
}

fn apply_tile_field_overlay_pulse(
	state: &mut ExpectedState,
	overlay_id: u64,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state
		.tile_field_overlays
		.get(&overlay_id)
		.filter(|active| active.generation == generation)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	let observed_cells: BTreeSet<GridPosition> = observed_tile_cells(state, &active);
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for actor in all_visible_actors(state)
		.into_iter()
		.filter(|actor| {
			actor_cell(state, *actor).is_some_and(|cell| observed_cells.contains(&cell))
		})
		.filter(|actor| tile_field_overlay_actor_matches(state, &active, *actor))
	{
		events.extend(effect_events(
			active.source.actor,
			active.source.deployment_generation,
			&active.spec.periodic_effects,
			vec![actor],
			at,
		));
	}
	events.push(EventRequest::new(
		checked_tick_add(at, active.spec.interval_ticks)?,
		EFFECT_PRIORITY,
		actor_entity(active.source.actor),
		ExpectedEvent::TileFieldOverlayPulse {
			overlay_id,
			generation,
		},
	));
	Ok(events)
}

fn install_skill_inactive_stance(
	state: &mut ExpectedState,
	mut source: RuntimeSource,
	spec: RuntimeSkillInactiveStance,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !runtime_source_is_live(state, source) {
		return Ok(Vec::new());
	}
	source = runtime_source_for_lifetime(source, RuntimeEffectLifetime::SourceActor);
	let BattleActorId::Operator(operator) = source.actor else {
		return Err(ModelError::invalid_transition(
			"skill-inactive stance source must be an operator",
		));
	};
	let deployed: &DeployedOperator = state
		.deployed
		.get(&operator)
		.ok_or_else(|| ModelError::invalid_transition("stance owner disappeared"))?;
	if deployed.build.skill.is_none() {
		return Err(ModelError::invalid_transition(
			"skill-inactive stance owner has no skill",
		));
	}
	let should_activate: bool = !deployed.skill_active;
	let existing_id: Option<u64> = state
		.skill_inactive_stances
		.iter()
		.find_map(|(id, active)| {
			(active.source == source && active.spec.key == spec.key).then_some(*id)
		});
	let id: u64 = existing_id.unwrap_or(allocate_runtime_effect_id(state)?);
	if let Some(active) = state.skill_inactive_stances.get(&id) {
		if active.spec != spec {
			return Err(ModelError::invalid_transition(
				"reinstalled stance key changed its specification",
			));
		}
	} else {
		state.skill_inactive_stances.insert(
			id,
			ActiveSkillInactiveStance {
				source,
				spec,
				active: false,
			},
		);
	}
	if should_activate {
		activate_skill_inactive_stance(state, id, at)
	} else {
		Ok(Vec::new())
	}
}

fn activate_skill_inactive_stance(
	state: &mut ExpectedState,
	stance_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state
		.skill_inactive_stances
		.get(&stance_id)
		.filter(|active| !active.active)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	if !runtime_source_is_live(state, active.source) {
		return Ok(Vec::new());
	}
	let mut kinds: Vec<ActiveModifierKind> = Vec::new();
	if active.spec.disable_normal_attacks {
		kinds.push(ActiveModifierKind::NormalAttackSuppression);
	}
	kinds.extend(active.spec.modifiers.iter().map(buff_modifier_kind));
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for kind in kinds {
		events.extend(add_modifier_for_buff(
			state,
			active.source.actor,
			kind.clone(),
			None,
			Some(stance_id),
			at,
		)?);
		match kind {
			ActiveModifierKind::AttackCadence(_)
			| ActiveModifierKind::AttackSpeed(_)
			| ActiveModifierKind::NormalAttackSuppression => {
				events.extend(reschedule_attack(state, active.source.actor, at)?);
			}
			ActiveModifierKind::MovementSpeedReduction(_) => {
				if let BattleActorId::Enemy(enemy) = active.source.actor {
					events.extend(reschedule_action_affected_movement(state, enemy, at)?);
				}
			}
			ActiveModifierKind::Blocking(_) => {
				events.extend(reconcile_blocking(
					state,
					active.source.actor,
					at,
					BlockEndReason::BlockCountChange,
				)?);
			}
			ActiveModifierKind::BlockTarget(_) => {
				events.extend(reconcile_blocking(
					state,
					active.source.actor,
					at,
					BlockEndReason::BlockTargetChange,
				)?);
			}
			ActiveModifierKind::Stat(_)
			| ActiveModifierKind::ConditionalStat(_)
			| ActiveModifierKind::ArtsResistanceAddition(_)
			| ActiveModifierKind::Shelter(_)
			| ActiveModifierKind::AttackRange(_)
			| ActiveModifierKind::TargetCount(_)
			| ActiveModifierKind::HitCount(_) => {}
		}
	}
	state
		.skill_inactive_stances
		.get_mut(&stance_id)
		.expect("stance presence was checked")
		.active = true;
	Ok(events)
}

fn deactivate_skill_inactive_stance(
	state: &mut ExpectedState,
	stance_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.skill_inactive_stances.get_mut(&stance_id) else {
		return Ok(Vec::new());
	};
	if !active.active {
		return Ok(Vec::new());
	}
	active.active = false;
	let actor: BattleActorId = active.source.actor;
	let modifier_ids: Vec<u64> = actor_runtime(state, actor)
		.map(|runtime| {
			runtime
				.modifiers
				.iter()
				.filter_map(|modifier| (modifier.buff_id == Some(stance_id)).then_some(modifier.id))
				.collect()
		})
		.unwrap_or_default();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for modifier_id in modifier_ids {
		events.extend(expire_modifier(state, actor, modifier_id, at)?);
	}
	Ok(events)
}

fn set_skill_inactive_stances_active(
	state: &mut ExpectedState,
	source: RuntimeSource,
	active: bool,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let ids: Vec<u64> = state
		.skill_inactive_stances
		.iter()
		.filter_map(|(id, stance)| (stance.source == source).then_some(*id))
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for id in ids {
		if active {
			events.extend(activate_skill_inactive_stance(state, id, at)?);
		} else {
			events.extend(deactivate_skill_inactive_stance(state, id, at)?);
		}
	}
	Ok(events)
}

fn remove_matching_tile_effects(
	state: &mut ExpectedState,
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	key: &MechanicId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let ids: Vec<u64> = state
		.tile_effects
		.iter()
		.filter_map(|(id, active)| {
			(active.source.actor == source
				&& (active.spec.lifetime == RuntimeEffectLifetime::Persistent
					|| active.source.deployment_generation == source_deployment_generation)
				&& &active.spec.key == key)
				.then_some(*id)
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for id in ids {
		events.extend(remove_tile_effect_by_id(state, id, None, None, at)?);
	}
	Ok(events)
}

fn remove_tile_effect_by_id(
	state: &mut ExpectedState,
	id: u64,
	expected_generation: Option<u32>,
	excluded_actor: Option<BattleActorId>,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.tile_effects.get(&id) else {
		return Ok(Vec::new());
	};
	if expected_generation.is_some_and(|generation| active.generation != generation) {
		return Ok(Vec::new());
	}
	let active: ActiveTileEffect = state
		.tile_effects
		.remove(&id)
		.expect("tile effect presence was checked");
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for actor in all_visible_actors(state)
		.into_iter()
		.filter(|actor| Some(*actor) != excluded_actor)
		.filter(|actor| actor_cell(state, *actor) == Some(active.cell))
		.filter(|actor| tile_actor_matches(state, &active, *actor))
	{
		for _ in 0..active.layers {
			events.extend(effect_events(
				active.source.actor,
				active.source.deployment_generation,
				&active.spec.on_remove_per_layer,
				vec![actor],
				at,
			));
		}
	}
	let owned_summons: Vec<u32> = state
		.summons
		.iter()
		.filter_map(|(instance, summon)| {
			matches!(
				summon.ownership,
				ActiveSummonOwnership::TriggeringTileEffect {
					tile_effect_id
				} if tile_effect_id == id
			)
			.then_some(*instance)
		})
		.collect();
	for instance in owned_summons {
		if let Some(event) = schedule_owned_summon_death(state, active.source.actor, instance, at)?
		{
			events.push(event);
		}
	}
	Ok(events)
}

fn buff_modifier_kind(modifier: &BuffModifier) -> ActiveModifierKind {
	match modifier {
		BuffModifier::Stat(modifier) => ActiveModifierKind::Stat(modifier.clone()),
		BuffModifier::ArtsResistanceAddition { ratio } => {
			ActiveModifierKind::ArtsResistanceAddition(*ratio)
		}
		BuffModifier::Shelter {
			incoming_damage_multiplier,
		} => ActiveModifierKind::Shelter(*incoming_damage_multiplier),
		BuffModifier::MovementSpeedReduction { reduction } => {
			ActiveModifierKind::MovementSpeedReduction(*reduction)
		}
		BuffModifier::AttackCadence(modifier) => {
			ActiveModifierKind::AttackCadence(modifier.clone())
		}
		BuffModifier::AttackSpeed(modifier) => ActiveModifierKind::AttackSpeed(*modifier),
		BuffModifier::AttackRange(modifier) => ActiveModifierKind::AttackRange(modifier.clone()),
		BuffModifier::TargetCount(modifier) => ActiveModifierKind::TargetCount(*modifier),
		BuffModifier::HitCount(modifier) => ActiveModifierKind::HitCount(*modifier),
		BuffModifier::Blocking(modifier) => ActiveModifierKind::Blocking(*modifier),
		BuffModifier::BlockTarget(modifier) => ActiveModifierKind::BlockTarget(*modifier),
	}
}

fn conditional_aura_condition_matches(
	state: &ExpectedState,
	condition: &TeamCompositionCondition,
) -> bool {
	match condition {
		TeamCompositionCondition::DeployedDistinctProfessions {
			minimum_count,
			profession_mask,
		} => {
			let professions: BTreeSet<u16> = state
				.deployed
				.values()
				.map(|deployed| deployed.build.classification.profession_category_mask)
				.filter(|mask| *mask != 0 && *mask & *profession_mask != 0)
				.collect();
			professions.len() >= usize::from(*minimum_count)
		}
		TeamCompositionCondition::DeployedSameProfession {
			minimum_count,
			profession_mask,
		} => {
			let mut counts: BTreeMap<u16, u16> = BTreeMap::new();
			for profession in state
				.deployed
				.values()
				.map(|deployed| deployed.build.classification.profession_category_mask)
				.filter(|mask| *mask != 0 && *mask & *profession_mask != 0)
			{
				let count: &mut u16 = counts.entry(profession).or_default();
				*count = count.saturating_add(1);
			}
			counts.values().any(|count| count >= minimum_count)
		}
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count,
			group_id,
		} => {
			state
				.scenario
				.battle
				.guide
				.builds
				.iter()
				.filter(|build| build.classification.group_id.as_deref() == Some(group_id.as_str()))
				.count() >= usize::from(*minimum_count)
		}
	}
}

fn conditional_aura_targets(
	state: &ExpectedState,
	source: RuntimeSource,
	spec: &RuntimeConditionalAura,
) -> BTreeSet<(OperatorInstanceId, u32)> {
	state
		.deployed
		.iter()
		.filter_map(|(operator, deployed)| {
			let profession: u16 = deployed.build.classification.profession_category_mask;
			(profession != 0
				&& profession & spec.target_profession_mask != 0
				&& (spec.includes_source || source.actor != BattleActorId::Operator(*operator)))
			.then_some((*operator, deployed.deployment_generation))
		})
		.collect()
}

fn install_conditional_aura(
	state: &mut ExpectedState,
	mut source: RuntimeSource,
	spec: RuntimeConditionalAura,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !matches!(source.actor, BattleActorId::Operator(_)) {
		return Err(ModelError::invalid_transition(
			"conditional aura source must be an operator",
		));
	}
	if spec.lifetime != RuntimeEffectLifetime::Persistent && !runtime_source_is_live(state, source)
	{
		return Ok(Vec::new());
	}
	source = runtime_source_for_lifetime(source, spec.lifetime);
	let existing_id: Option<u64> = state.conditional_auras.iter().find_map(|(id, active)| {
		(active.source == source && active.spec.key == spec.key).then_some(*id)
	});
	let aura_id: u64 = existing_id.unwrap_or(allocate_runtime_effect_id(state)?);
	if let Some(active) = state.conditional_auras.get(&aura_id) {
		if active.spec != spec {
			return Err(ModelError::invalid_transition(
				"reinstalled conditional aura key changed its specification",
			));
		}
	} else {
		state.conditional_auras.insert(
			aura_id,
			ActiveConditionalAura {
				source,
				spec,
				active: false,
				target_buffs: BTreeMap::new(),
			},
		);
	}
	reconcile_conditional_aura_by_id(state, aura_id, at)
}

fn apply_conditional_aura_target_buff(
	state: &mut ExpectedState,
	source: RuntimeSource,
	target: BattleActorId,
	spec: RuntimeBuff,
	at: SimTick,
) -> Result<(u64, Vec<EventRequest<ExpectedEvent>>), ModelError> {
	let target_generation: Option<u32> = operator_deployment_generation(state, target);
	if state.buffs.values().any(|active| {
		active.source == source
			&& active.target == target
			&& active.target_deployment_generation == target_generation
			&& active.spec.key == spec.key
	}) {
		return Err(ModelError::invalid_transition(
			"conditional aura buff key collides with an existing source-scoped buff",
		));
	}
	let events: Vec<EventRequest<ExpectedEvent>> =
		apply_buff(state, source, target, spec.clone(), at)?;
	let buff_id: u64 = state
		.buffs
		.iter()
		.find_map(|(id, active)| {
			(active.source == source
				&& active.target == target
				&& active.target_deployment_generation == target_generation
				&& active.spec.key == spec.key)
				.then_some(*id)
		})
		.ok_or_else(|| ModelError::invalid_transition("conditional aura buff was not installed"))?;
	Ok((buff_id, events))
}

fn reconcile_conditional_auras(
	state: &mut ExpectedState,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let aura_ids: Vec<u64> = state.conditional_auras.keys().copied().collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for aura_id in aura_ids {
		events.extend(reconcile_conditional_aura_by_id(state, aura_id, at)?);
	}
	Ok(events)
}

fn reconcile_conditional_aura_by_id(
	state: &mut ExpectedState,
	aura_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.conditional_auras.get(&aura_id).cloned() else {
		return Ok(Vec::new());
	};
	if active.spec.lifetime != RuntimeEffectLifetime::Persistent
		&& !runtime_source_is_live(state, active.source)
	{
		return remove_conditional_aura_by_id(state, aura_id, at);
	}
	let condition_matches: bool = conditional_aura_condition_matches(state, &active.spec.condition);
	let desired_targets: BTreeSet<(OperatorInstanceId, u32)> = if condition_matches {
		conditional_aura_targets(state, active.source, &active.spec)
	} else {
		BTreeSet::new()
	};
	let mut target_buffs: BTreeMap<(OperatorInstanceId, u32), u64> = active.target_buffs;
	let removed_targets: Vec<(OperatorInstanceId, u32)> = target_buffs
		.iter()
		.filter_map(|(target, buff_id)| {
			let target_actor: BattleActorId = BattleActorId::Operator(target.0);
			let buff_is_current: bool = state.buffs.get(buff_id).is_some_and(|buff| {
				buff.target == target_actor
					&& buff.target_deployment_generation == Some(target.1)
					&& buff.source == active.source
			});
			(!desired_targets.contains(target) || !buff_is_current).then_some(*target)
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for target in removed_targets {
		let buff_id: u64 = target_buffs
			.remove(&target)
			.expect("target buff was listed");
		events.extend(remove_buff_by_id(
			state,
			BattleActorId::Operator(target.0),
			buff_id,
			at,
		)?);
	}
	for target in desired_targets {
		if target_buffs.contains_key(&target) {
			continue;
		}
		let (buff_id, buff_events) = apply_conditional_aura_target_buff(
			state,
			active.source,
			BattleActorId::Operator(target.0),
			active.spec.buff.clone(),
			at,
		)?;
		target_buffs.insert(target, buff_id);
		events.extend(buff_events);
		if let Some(periodic) = &active.spec.periodic {
			events.push(conditional_aura_pulse_event(
				aura_id,
				target.0,
				target.1,
				buff_id,
				checked_tick_add(at, periodic.initial_delay_ticks)?,
			));
		}
	}
	let current = state
		.conditional_auras
		.get_mut(&aura_id)
		.expect("conditional aura presence was checked");
	current.active = condition_matches;
	current.target_buffs = target_buffs;
	Ok(events)
}

fn remove_conditional_aura_by_id(
	state: &mut ExpectedState,
	aura_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.conditional_auras.remove(&aura_id) else {
		return Ok(Vec::new());
	};
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for ((operator, _), buff_id) in active.target_buffs {
		events.extend(remove_buff_by_id(
			state,
			BattleActorId::Operator(operator),
			buff_id,
			at,
		)?);
	}
	Ok(events)
}

fn apply_conditional_aura_pulse(
	state: &mut ExpectedState,
	aura_id: u64,
	target: OperatorInstanceId,
	target_deployment_generation: u32,
	buff_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		reconcile_conditional_aura_by_id(state, aura_id, at)?;
	let Some(active) = state
		.conditional_auras
		.get(&aura_id)
		.filter(|active| {
			active.active
				&& active
					.target_buffs
					.get(&(target, target_deployment_generation))
					== Some(&buff_id)
		})
		.cloned()
	else {
		return Ok(events);
	};
	let target_actor: BattleActorId = BattleActorId::Operator(target);
	let target_is_current: bool = state
		.deployed
		.get(&target)
		.is_some_and(|deployed| deployed.deployment_generation == target_deployment_generation)
		&& state.buffs.get(&buff_id).is_some_and(|buff| {
			buff.source == active.source
				&& buff.target == target_actor
				&& buff.target_deployment_generation == Some(target_deployment_generation)
		});
	if !target_is_current {
		return Ok(events);
	}
	let Some(periodic) = active.spec.periodic.as_ref() else {
		return Err(ModelError::invalid_transition(
			"non-periodic conditional aura received a pulse",
		));
	};
	events.extend(effect_events(
		active.source.actor,
		active.source.deployment_generation,
		&periodic.effects,
		vec![target_actor],
		at,
	));
	events.push(conditional_aura_pulse_event(
		aura_id,
		target,
		target_deployment_generation,
		buff_id,
		checked_tick_add(at, periodic.interval_ticks)?,
	));
	Ok(events)
}

fn conditional_aura_pulse_event(
	aura_id: u64,
	target: OperatorInstanceId,
	target_deployment_generation: u32,
	buff_id: u64,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		EFFECT_PRIORITY,
		actor_entity(BattleActorId::Operator(target)),
		ExpectedEvent::ConditionalAuraPulse {
			aura_id,
			target,
			target_deployment_generation,
			buff_id,
		},
	)
}

fn apply_buff(
	state: &mut ExpectedState,
	mut source: RuntimeSource,
	target: BattleActorId,
	spec: RuntimeBuff,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !actor_exists(state, target) {
		return Ok(Vec::new());
	}
	if spec.lifetime != RuntimeEffectLifetime::Persistent && !runtime_source_is_live(state, source)
	{
		return Ok(Vec::new());
	}
	source = runtime_source_for_lifetime(source, spec.lifetime);
	let target_deployment_generation: Option<u32> = operator_deployment_generation(state, target);
	let existing_id: Option<u64> = state.buffs.iter().find_map(|(id, active)| {
		(active.source == source
			&& active.target == target
			&& active.target_deployment_generation == target_deployment_generation
			&& active.spec.key == spec.key)
			.then_some(*id)
	});
	let id: u64 = existing_id.unwrap_or(allocate_runtime_effect_id(state)?);
	let (generation, added_stack): (u32, bool) = if let Some(active) = state.buffs.get_mut(&id) {
		if active.spec != spec {
			return Err(ModelError::invalid_transition(
				"reapplied buff key changed its component specification",
			));
		}
		let added: bool = active.stacks < active.spec.max_stacks;
		if added {
			active.stacks += 1;
		}
		active.generation = active
			.generation
			.checked_add(1)
			.ok_or_else(|| ModelError::invalid_transition("buff generation overflow"))?;
		(active.generation, added)
	} else {
		state.buffs.insert(
			id,
			ActiveBuff {
				source,
				target,
				target_deployment_generation,
				spec: spec.clone(),
				stacks: 1,
				generation: 1,
			},
		);
		(1, true)
	};
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	if added_stack {
		for modifier in &spec.modifiers_per_stack {
			let kind: ActiveModifierKind = buff_modifier_kind(modifier);
			events.extend(add_modifier_for_buff(
				state,
				target,
				kind.clone(),
				None,
				Some(id),
				at,
			)?);
			match kind {
				ActiveModifierKind::AttackCadence(_) | ActiveModifierKind::AttackSpeed(_) => {
					events.extend(reschedule_attack(state, target, at)?);
				}
				ActiveModifierKind::MovementSpeedReduction(_) => {
					if let BattleActorId::Enemy(enemy) = target {
						events.extend(reschedule_action_affected_movement(state, enemy, at)?);
					}
				}
				ActiveModifierKind::Blocking(_) => {
					events.extend(reconcile_blocking(
						state,
						target,
						at,
						BlockEndReason::BlockCountChange,
					)?);
				}
				ActiveModifierKind::BlockTarget(_) => {
					events.extend(reconcile_blocking(
						state,
						target,
						at,
						BlockEndReason::BlockTargetChange,
					)?);
				}
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::ArtsResistanceAddition(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::AttackRange(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::HitCount(_) => {}
			}
		}
	}
	if let Some(duration) = spec.duration_ticks {
		events.push(EventRequest::new(
			checked_tick_add(at, duration)?,
			EFFECT_PRIORITY,
			actor_entity(target),
			ExpectedEvent::BuffExpired {
				actor: target,
				buff_id: id,
				generation,
			},
		));
	}
	Ok(events)
}

fn remove_matching_buffs(
	state: &mut ExpectedState,
	target: BattleActorId,
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	key: &MechanicId,
	removal_source: BuffRemovalSource,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let ids: Vec<u64> = state
		.buffs
		.iter()
		.filter_map(|(id, active)| {
			(active.target == target
				&& active.target_deployment_generation
					== operator_deployment_generation(state, target)
				&& &active.spec.key == key
				&& (removal_source == BuffRemovalSource::AnySource
					|| (active.source.actor == source
						&& (active.spec.lifetime == RuntimeEffectLifetime::Persistent
							|| active.source.deployment_generation
								== source_deployment_generation))))
				.then_some(*id)
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for id in ids {
		events.extend(remove_buff_by_id(state, target, id, at)?);
	}
	Ok(events)
}

fn expire_buff(
	state: &mut ExpectedState,
	actor: BattleActorId,
	buff_id: u64,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !state.buffs.get(&buff_id).is_some_and(|active| {
		active.target == actor
			&& active.target_deployment_generation == operator_deployment_generation(state, actor)
			&& active.generation == generation
	}) {
		return Ok(Vec::new());
	}
	remove_buff_by_id(state, actor, buff_id, at)
}

fn remove_buff_by_id(
	state: &mut ExpectedState,
	actor: BattleActorId,
	buff_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if state
		.buffs
		.get(&buff_id)
		.is_none_or(|active| active.target != actor)
	{
		return Ok(Vec::new());
	}
	if state.buffs.remove(&buff_id).is_none() {
		return Ok(Vec::new());
	}
	let modifier_ids: Vec<u64> = actor_runtime(state, actor)
		.map(|runtime| {
			runtime
				.modifiers
				.iter()
				.filter_map(|modifier| (modifier.buff_id == Some(buff_id)).then_some(modifier.id))
				.collect()
		})
		.unwrap_or_default();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for modifier_id in modifier_ids {
		events.extend(expire_modifier(state, actor, modifier_id, at)?);
	}
	Ok(events)
}

fn cleanup_runtime_effects(
	state: &mut ExpectedState,
	source: RuntimeSource,
	skill_only: bool,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let should_remove = |lifetime: RuntimeEffectLifetime| {
		if skill_only {
			lifetime == RuntimeEffectLifetime::SkillActivation
		} else {
			matches!(
				lifetime,
				RuntimeEffectLifetime::SourceActor | RuntimeEffectLifetime::SkillActivation
			)
		}
	};
	state
		.listeners
		.retain(|_, listener| listener.source != source || !should_remove(listener.spec.lifetime));
	let aura_ids: Vec<u64> = state
		.conditional_auras
		.iter()
		.filter_map(|(id, aura)| {
			(aura.source == source && should_remove(aura.spec.lifetime)).then_some(*id)
		})
		.collect();
	let removed_overlays: Vec<ActiveTileFieldOverlay> = state
		.tile_field_overlays
		.values()
		.filter(|overlay| overlay.source == source && should_remove(overlay.spec.lifetime))
		.cloned()
		.collect();
	let mut tile_ids: Vec<u64> = state
		.tile_effects
		.iter()
		.filter_map(|(id, active)| {
			(active.source == source && should_remove(active.spec.lifetime)).then_some(*id)
		})
		.collect();
	if skill_only {
		for overlay in &removed_overlays {
			if overlay.spec.cached_propagation.is_none() {
				continue;
			}
			tile_ids.extend(state.tile_effects.iter().filter_map(|(id, active)| {
				(active.source == source
					&& active.spec.key == overlay.spec.observed_tile_key
					&& active.persistence == TilePersistence::Temporary)
					.then_some(*id)
			}));
		}
		tile_ids.sort_unstable();
		tile_ids.dedup();
	}
	state
		.tile_field_overlays
		.retain(|_, overlay| overlay.source != source || !should_remove(overlay.spec.lifetime));
	let stance_ids: Vec<u64> = if skill_only {
		Vec::new()
	} else {
		state
			.skill_inactive_stances
			.iter()
			.filter_map(|(id, stance)| (stance.source == source).then_some(*id))
			.collect()
	};
	let buff_targets: Vec<(u64, BattleActorId)> = state
		.buffs
		.iter()
		.filter_map(|(id, active)| {
			(active.source == source && should_remove(active.spec.lifetime))
				.then_some((*id, active.target))
		})
		.collect();
	let owned_summons: Vec<u32> = state
		.summons
		.iter()
		.filter_map(|(instance, summon)| {
			let remove: bool = match summon.ownership {
				ActiveSummonOwnership::SourceActor(owner) => !skill_only && owner == source,
				ActiveSummonOwnership::SkillActivation(owner) => owner == source,
				ActiveSummonOwnership::Independent
				| ActiveSummonOwnership::TriggeringTileEffect { .. } => false,
			};
			remove.then_some(*instance)
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for id in aura_ids {
		events.extend(remove_conditional_aura_by_id(state, id, at)?);
	}
	for id in stance_ids {
		events.extend(deactivate_skill_inactive_stance(state, id, at)?);
		state.skill_inactive_stances.remove(&id);
	}
	for id in tile_ids {
		if !skill_only {
			if let Some(active) = state.tile_effects.get_mut(&id) {
				active.source.deployment_generation = None;
			}
		}
		events.extend(remove_tile_effect_by_id(state, id, None, None, at)?);
	}
	for (id, target) in buff_targets {
		events.extend(remove_buff_by_id(state, target, id, at)?);
	}
	for instance in owned_summons {
		if let Some(event) = schedule_owned_summon_death(state, source.actor, instance, at)? {
			events.push(event);
		}
	}
	Ok(events)
}

fn schedule_owned_summon_death(
	state: &mut ExpectedState,
	source: BattleActorId,
	instance: u32,
	at: SimTick,
) -> Result<Option<EventRequest<ExpectedEvent>>, ModelError> {
	let actor: BattleActorId = BattleActorId::Summon(instance);
	if !actor_exists(state, actor) || actor_hp(state, actor) == Some(ExpectedValue::ZERO) {
		return Ok(None);
	}
	set_actor_hp(state, actor, ExpectedValue::ZERO)?;
	state.pending_death_sources.insert(actor, source);
	Ok(Some(death_event(state, actor, at)))
}

fn cleanup_target_buffs(
	state: &mut ExpectedState,
	target: BattleActorId,
	target_deployment_generation: Option<u32>,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let ids: Vec<u64> = state
		.buffs
		.iter()
		.filter_map(|(id, active)| {
			(active.target == target
				&& active.target_deployment_generation == target_deployment_generation)
				.then_some(*id)
		})
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for id in ids {
		events.extend(remove_buff_by_id(state, target, id, at)?);
	}
	Ok(events)
}

fn apply_damage(
	state: &mut ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
	raw: ExpectedValue,
	damage_type: DamageType,
	delivery: DamageDelivery,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !actor_exists(state, target) || actor_hp(state, target) == Some(ExpectedValue::ZERO) {
		return Ok(Vec::new());
	}
	let damage: ExpectedValue =
		damage_after_mitigation(state, source, target, raw, damage_type, delivery)?;
	let reaction: Option<(u64, ExpectedValue)> = expose_damage_source_reaction(state, target);
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		apply_mitigated_damage(state, source, target, damage, at)?;
	if let Some((duration_ticks, incoming_damage_multiplier)) = reaction {
		if source != target && actor_runtime(state, source).is_some() {
			events.extend(apply_status(
				state,
				source,
				StatusEffect {
					kind: StatusKind::Exposure,
					duration_ticks,
					movement_interval_multiplier: ExpectedValue::ONE,
					attack_speed_delta: 0,
					incoming_damage_multiplier,
					arts_resistance_reduction: ExpectedValue::ZERO,
					arts_resistance_reduction_target: StatusFactionFilter::Any,
				},
				at,
			)?);
		}
	}
	Ok(events)
}

fn expose_damage_source_reaction(
	state: &ExpectedState,
	target: BattleActorId,
) -> Option<(u64, ExpectedValue)> {
	actor_runtime(state, target)?
		.statuses
		.keys()
		.find_map(|kind| match kind {
			StatusKind::ExposeDamageSource {
				exposure_duration_ticks,
				incoming_damage_multiplier,
			} => Some((*exposure_duration_ticks, *incoming_damage_multiplier)),
			StatusKind::Cold
			| StatusKind::Frozen
			| StatusKind::Stun
			| StatusKind::Slow
			| StatusKind::Shelter
			| StatusKind::Exposure
			| StatusKind::Gravity
			| StatusKind::StealthWhileUnblocked
			| StatusKind::Custom(_) => None,
		})
}

fn damage_after_mitigation(
	state: &ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
	raw: ExpectedValue,
	damage_type: DamageType,
	delivery: DamageDelivery,
) -> Result<ExpectedValue, ModelError> {
	let ignore_defense: bool = matches!(
		delivery,
		DamageDelivery::DirectHit {
			ignore_defense: true,
			..
		}
	);
	let ignore_shelter: bool = matches!(
		delivery,
		DamageDelivery::DirectHit {
			ignore_shelter: true,
			..
		}
	);
	let target_combat: ActorCombatFacts = actor_combat_facts(state, target)
		.ok_or_else(|| ModelError::invalid_transition("damage target facts disappeared"))?;
	// The floor is a fraction of the raw hit, so it belongs to the attacker, and it guards both
	// mitigated families alike: defense and resistance can each swallow the whole attack, and
	// neither is allowed to take the hit below this sliver.
	let floor: ExpectedValue = raw
		.checked_mul(
			actor_combat_facts(state, source)
				.unwrap_or(target_combat)
				.minimum_damage_ratio,
		)
		.ok_or_else(|| ModelError::invalid_transition("damage floor overflow"))?;
	let mitigated: ExpectedValue = match damage_type {
		DamageType::Physical if !ignore_defense => {
			let defense: ExpectedValue = effective_actor_stat(state, target, StatKind::Defense)?;
			raw.saturating_sub(defense).max(floor)
		}
		DamageType::Arts if !ignore_defense => {
			let resistance_reduction: ExpectedValue =
				effective_arts_resistance_reduction(state, target)?;
			let resistance_addition: ExpectedValue =
				effective_arts_resistance_addition(state, target)?;
			// Resistance points are additive, so every addition and reduction resolves into one
			// effective resistance before it becomes a multiplier, and the clamp belongs on that
			// resolved value rather than on a partial sum. Reducing more resistance than a target
			// has leaves it at zero: negative resistance does not amplify a hit past its raw
			// damage. Applying the reduction straight to the multiplier used to do exactly that.
			let effective_resistance: ExpectedValue = target_combat
				.arts_resistance
				.checked_add(resistance_addition)
				.ok_or_else(|| ModelError::invalid_transition("Arts-resistance addition overflow"))?
				.saturating_sub(resistance_reduction)
				.min(ExpectedValue::ONE);
			let multiplier: ExpectedValue = ExpectedValue::ONE.saturating_sub(effective_resistance);
			raw.checked_mul(multiplier)
				.ok_or_else(|| ModelError::invalid_transition("arts damage overflow"))?
				.max(floor)
		}
		DamageType::Physical | DamageType::Arts | DamageType::True => raw,
	};
	let incoming_multiplier: ExpectedValue =
		effective_incoming_multiplier(state, target, ignore_shelter)?;
	let damage: ExpectedValue = mitigated
		.checked_mul(incoming_multiplier)
		.ok_or_else(|| ModelError::invalid_transition("incoming damage overflow"))?;
	Ok(damage)
}

fn effective_arts_resistance_addition(
	state: &ExpectedState,
	target: BattleActorId,
) -> Result<ExpectedValue, ModelError> {
	actor_runtime(state, target).map_or(Ok(ExpectedValue::ZERO), |runtime| {
		runtime.modifiers.iter().try_fold(
			ExpectedValue::ZERO,
			|total: ExpectedValue, modifier: &ActiveModifier| match &modifier.kind {
				ActiveModifierKind::ArtsResistanceAddition(ratio) => {
					total.checked_add(*ratio).ok_or_else(|| {
						ModelError::invalid_transition("active Arts-resistance addition overflow")
					})
				}
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::MovementSpeedReduction(_)
				| ActiveModifierKind::AttackCadence(_)
				| ActiveModifierKind::AttackSpeed(_)
				| ActiveModifierKind::AttackRange(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::HitCount(_)
				| ActiveModifierKind::Blocking(_)
				| ActiveModifierKind::BlockTarget(_) => Ok(total),
			},
		)
	})
}

fn effective_arts_resistance_reduction(
	state: &ExpectedState,
	target: BattleActorId,
) -> Result<ExpectedValue, ModelError> {
	let faction: Faction = actor_faction(state, target).ok_or_else(|| {
		ModelError::invalid_transition("Arts-resistance reduction target faction disappeared")
	})?;
	actor_runtime(state, target).map_or(Ok(ExpectedValue::ZERO), |runtime| {
		runtime.statuses.values().try_fold(
			ExpectedValue::ZERO,
			|total: ExpectedValue, status: &ActiveStatus| {
				if !status.spec.arts_resistance_reduction_target.allows(faction) {
					return Ok(total);
				}
				total
					.checked_add(status.spec.arts_resistance_reduction)
					.ok_or_else(|| {
						ModelError::invalid_transition("active Arts-resistance reduction overflow")
					})
			},
		)
	})
}

fn apply_mitigated_damage(
	state: &mut ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
	damage: ExpectedValue,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !actor_exists(state, target) || actor_hp(state, target) == Some(ExpectedValue::ZERO) {
		return Ok(Vec::new());
	}
	let before: ExpectedValue = actor_hp(state, target)
		.ok_or_else(|| ModelError::invalid_transition("damage target disappeared"))?;
	let credited: ExpectedValue = damage.min(before);
	set_actor_hp(state, target, before.saturating_sub(credited))?;
	update_damage_report(state, target, credited);
	let mut events: Vec<EventRequest<ExpectedEvent>> = charge_sp_for_hit(state, target, at)?;
	if credited >= before {
		state.pending_death_sources.insert(target, source);
		events.extend(fire_trigger(
			state,
			TriggerContext {
				kind: ReplayTriggerKind::BeforeLethal,
				actor: target,
				effect_source: source,
			},
			at,
		)?);
		events.push(death_event(state, target, at));
	}
	if credited != ExpectedValue::ZERO {
		events.extend(fire_trigger(
			state,
			TriggerContext {
				kind: ReplayTriggerKind::Damage,
				actor: target,
				effect_source: source,
			},
			at,
		)?);
	}
	Ok(events)
}

fn apply_explicit_death(
	state: &mut ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if target == BattleActorId::Stage || !actor_exists(state, target) {
		return Ok(Vec::new());
	}
	set_actor_hp(state, target, ExpectedValue::ZERO)?;
	update_hp_report(state, target, ExpectedValue::ZERO);
	state.pending_death_sources.insert(target, source);
	let target_deployment_generation: Option<u32> = operator_deployment_generation(state, target);
	apply_death(state, target, target_deployment_generation, at)
}

fn apply_healing(
	state: &mut ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
	amount: ExpectedValue,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(before) = actor_hp(state, target) else {
		return Ok(Vec::new());
	};
	let max_hp: ExpectedValue = effective_actor_stat(state, target, StatKind::MaxHp)?;
	let after: ExpectedValue = before
		.checked_add(amount)
		.unwrap_or(ExpectedValue::from_scaled(u64::MAX))
		.min(max_hp);
	set_actor_hp(state, target, after)?;
	update_hp_report(state, target, after);
	if after == before {
		return Ok(Vec::new());
	}
	fire_trigger(
		state,
		TriggerContext {
			kind: ReplayTriggerKind::Heal,
			actor: target,
			effect_source: source,
		},
		at,
	)
}

fn restore_hp_to_current_max_ratio(
	state: &mut ExpectedState,
	target: BattleActorId,
	ratio: ExpectedValue,
) -> Result<(), ModelError> {
	let Some(before) = actor_hp(state, target) else {
		return Ok(());
	};
	let max_hp: ExpectedValue = effective_actor_stat(state, target, StatKind::MaxHp)?;
	let restored: ExpectedValue = max_hp
		.checked_mul(ratio)
		.ok_or_else(|| ModelError::invalid_transition("current-max HP restore overflow"))?;
	let after: ExpectedValue = before.max(restored).min(max_hp);
	if after != before {
		set_actor_hp(state, target, after)?;
		update_hp_report(state, target, after);
	}
	Ok(())
}

fn add_modifier(
	state: &mut ExpectedState,
	actor: BattleActorId,
	kind: ActiveModifierKind,
	duration_ticks: Option<u64>,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	add_modifier_for_buff(state, actor, kind, duration_ticks, None, at)
}

fn add_modifier_for_buff(
	state: &mut ExpectedState,
	actor: BattleActorId,
	kind: ActiveModifierKind,
	duration_ticks: Option<u64>,
	buff_id: Option<u64>,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !actor_exists(state, actor) {
		return Ok(Vec::new());
	}
	let changes_hit_count: bool = matches!(&kind, ActiveModifierKind::HitCount(_));
	let changes_max_hp: bool = matches!(
		&kind,
		ActiveModifierKind::Stat(modifier) if modifier.stat == StatKind::MaxHp
	);
	let previous_max_hp: Option<ExpectedValue> = changes_max_hp
		.then(|| effective_actor_stat(state, actor, StatKind::MaxHp))
		.transpose()?;
	let was_full_hp: bool = previous_max_hp
		.zip(actor_hp(state, actor))
		.is_some_and(|(max_hp, hp)| hp == max_hp);
	let id: u64 = state.next_modifier_id;
	state.next_modifier_id = state
		.next_modifier_id
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("modifier identity overflow"))?;
	actor_runtime_mut(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("modifier target disappeared"))?
		.modifiers
		.push(ActiveModifier { id, kind, buff_id });
	if changes_hit_count {
		effective_hit_count(state, actor)?;
	}
	if changes_max_hp {
		let max_hp: ExpectedValue = effective_actor_stat(state, actor, StatKind::MaxHp)?;
		let hp: ExpectedValue = actor_hp(state, actor)
			.ok_or_else(|| ModelError::invalid_transition("max-HP modifier target disappeared"))?;
		let adjusted: ExpectedValue = if was_full_hp { max_hp } else { hp.min(max_hp) };
		set_actor_hp(state, actor, adjusted)?;
		update_max_hp_report(state, actor, max_hp);
		update_hp_report(state, actor, adjusted);
	}
	let Some(duration_ticks) = duration_ticks else {
		return Ok(Vec::new());
	};
	Ok(vec![EventRequest::new(
		checked_tick_add(at, duration_ticks)?,
		EFFECT_PRIORITY,
		actor_entity(actor),
		ExpectedEvent::ModifierExpired {
			actor,
			modifier_id: id,
		},
	)])
}

fn expire_modifier(
	state: &mut ExpectedState,
	actor: BattleActorId,
	modifier_id: u64,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let previous_max_hp: ExpectedValue =
		effective_actor_stat(state, actor, StatKind::MaxHp).unwrap_or(ExpectedValue::ZERO);
	let Some(runtime) = actor_runtime_mut(state, actor) else {
		return Ok(Vec::new());
	};
	let removed: Option<ActiveModifier> = runtime
		.modifiers
		.iter()
		.position(|modifier| modifier.id == modifier_id)
		.map(|index| runtime.modifiers.remove(index));
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	if let Some(modifier) = removed {
		let changed_max_hp: bool = matches!(
			&modifier.kind,
			ActiveModifierKind::Stat(modifier) if modifier.stat == StatKind::MaxHp
		);
		if matches!(&modifier.kind, ActiveModifierKind::HitCount(_)) {
			effective_hit_count(state, actor)?;
		}
		if changed_max_hp {
			let max_hp: ExpectedValue = effective_actor_stat(state, actor, StatKind::MaxHp)?;
			let hp: ExpectedValue = actor_hp(state, actor).ok_or_else(|| {
				ModelError::invalid_transition("expired max-HP modifier target disappeared")
			})?;
			let adjusted: ExpectedValue = hp.min(max_hp);
			if previous_max_hp != max_hp || adjusted != hp {
				set_actor_hp(state, actor, adjusted)?;
			}
			update_max_hp_report(state, actor, max_hp);
			update_hp_report(state, actor, adjusted);
		}
		if matches!(
			modifier.kind,
			ActiveModifierKind::AttackCadence(_)
				| ActiveModifierKind::AttackSpeed(_)
				| ActiveModifierKind::NormalAttackSuppression
		) {
			events.extend(reschedule_attack(state, actor, at)?);
		}
		if matches!(modifier.kind, ActiveModifierKind::MovementSpeedReduction(_)) {
			if let BattleActorId::Enemy(enemy) = actor {
				events.extend(reschedule_action_affected_movement(state, enemy, at)?);
			}
		}
		let blocking_release_reason: Option<BlockEndReason> = match modifier.kind {
			ActiveModifierKind::Blocking(_) => Some(BlockEndReason::BlockCountChange),
			ActiveModifierKind::BlockTarget(_) => Some(BlockEndReason::BlockTargetChange),
			ActiveModifierKind::Stat(_)
			| ActiveModifierKind::ConditionalStat(_)
			| ActiveModifierKind::ArtsResistanceAddition(_)
			| ActiveModifierKind::NormalAttackSuppression
			| ActiveModifierKind::Shelter(_)
			| ActiveModifierKind::MovementSpeedReduction(_)
			| ActiveModifierKind::AttackCadence(_)
			| ActiveModifierKind::AttackSpeed(_)
			| ActiveModifierKind::AttackRange(_)
			| ActiveModifierKind::TargetCount(_)
			| ActiveModifierKind::HitCount(_) => None,
		};
		if let Some(reason) = blocking_release_reason {
			events.extend(reconcile_blocking(state, actor, at, reason)?);
		}
	}
	Ok(events)
}

fn apply_status(
	state: &mut ExpectedState,
	actor: BattleActorId,
	mut spec: StatusEffect,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if !actor_exists(state, actor) {
		return Ok(Vec::new());
	}
	if spec.kind == StatusKind::Cold
		&& actor_runtime(state, actor)
			.is_some_and(|runtime| runtime.statuses.contains_key(&StatusKind::Cold))
	{
		if let Some(runtime) = actor_runtime_mut(state, actor) {
			runtime.statuses.remove(&StatusKind::Cold);
		}
		spec = StatusEffect::frozen(spec.duration_ticks);
	}
	let generation: u32 = match actor {
		BattleActorId::Operator(operator) => {
			let generation: &mut u32 = state
				.operator_status_generations
				.entry((operator, spec.kind.clone()))
				.or_insert(0);
			*generation = generation.checked_add(1).ok_or_else(|| {
				ModelError::invalid_transition("operator status generation overflow")
			})?;
			*generation
		}
		BattleActorId::Enemy(_) | BattleActorId::Summon(_) | BattleActorId::Stage => {
			actor_runtime(state, actor)
				.and_then(|runtime| runtime.statuses.get(&spec.kind))
				.map_or(0, |active| active.generation.saturating_add(1))
		}
	};
	let runtime = actor_runtime_mut(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("status target disappeared"))?;
	runtime.statuses.insert(
		spec.kind.clone(),
		ActiveStatus {
			spec: spec.clone(),
			generation,
		},
	);
	let permanent_marker: bool = matches!(
		&spec.kind,
		StatusKind::ExposeDamageSource { .. } | StatusKind::StealthWhileUnblocked
	);
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	if !permanent_marker {
		events.push(EventRequest::new(
			checked_tick_add(at, spec.duration_ticks)?,
			EFFECT_PRIORITY,
			actor_entity(actor),
			ExpectedEvent::StatusExpired {
				actor,
				status: spec.kind.clone(),
				generation,
			},
		));
	}
	if permanent_marker {
		return Ok(events);
	}
	let pauses_actor: bool = matches!(spec.kind, StatusKind::Frozen | StatusKind::Stun);
	if pauses_actor || spec.attack_speed_delta != 0 {
		events.extend(reschedule_attack(state, actor, at)?);
	}
	if spec.kind == StatusKind::Stun
		&& matches!(actor, BattleActorId::Operator(_) | BattleActorId::Summon(_))
	{
		events.extend(reconcile_blocking(
			state,
			actor,
			at,
			BlockEndReason::BlockCountChange,
		)?);
	}
	if pauses_actor || spec.movement_interval_multiplier != ExpectedValue::ONE {
		if let BattleActorId::Enemy(enemy) = actor {
			events.extend(reschedule_action_affected_movement(state, enemy, at)?);
		}
	}
	Ok(events)
}

fn expire_status(
	state: &mut ExpectedState,
	actor: BattleActorId,
	status: &StatusKind,
	generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(runtime) = actor_runtime_mut(state, actor) else {
		return Ok(Vec::new());
	};
	let Some(active_status) = runtime
		.statuses
		.get(status)
		.filter(|active| active.generation == generation)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	runtime.statuses.remove(status);
	let resumed_actor: bool = matches!(status, StatusKind::Frozen | StatusKind::Stun);
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	if resumed_actor || active_status.spec.attack_speed_delta != 0 {
		events.extend(reschedule_attack(state, actor, at)?);
	}
	if status == &StatusKind::Stun
		&& matches!(actor, BattleActorId::Operator(_) | BattleActorId::Summon(_))
	{
		events.extend(reconcile_blocking(
			state,
			actor,
			at,
			BlockEndReason::BlockCountChange,
		)?);
	}
	if resumed_actor || active_status.spec.movement_interval_multiplier != ExpectedValue::ONE {
		if let BattleActorId::Enemy(enemy) = actor {
			events.extend(reschedule_action_affected_movement(state, enemy, at)?);
		}
	}
	Ok(events)
}

fn shift_enemy(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	shift: crate::mechanics::domain::ShiftEffect,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.enemies.get(&enemy) else {
		return Ok(Vec::new());
	};
	if shift.force < active.facts.mass {
		return Ok(Vec::new());
	}
	let path: &BattlePath = find_path(&state.scenario.battle, active.spawn.path)
		.ok_or_else(|| ModelError::invalid_transition("shifted enemy path disappeared"))?;
	let mut positions: Vec<(usize, GridPosition)> = vec![(0, path.start)];
	for (index, step) in path.steps.iter().enumerate() {
		match step.action {
			BattlePathStepAction::Move { cell, .. }
			| BattlePathStepAction::Reappear { cell, .. } => positions.push((index + 1, cell)),
			BattlePathStepAction::Wait
			| BattlePathStepAction::Disappear
			| BattlePathStepAction::Goal { .. } => {}
		}
	}
	let current_position_index: usize = positions
		.iter()
		.enumerate()
		.rev()
		.find(|(_, (next_step, _))| *next_step <= active.path_index)
		.map_or(0, |(index, _)| index);
	let cells: usize = usize::from(shift.cells);
	let destination_index: usize = match shift.direction {
		ShiftDirection::TowardGoal => current_position_index.saturating_add(cells),
		ShiftDirection::AwayFromGoal => current_position_index.saturating_sub(cells),
	};
	if destination_index >= positions.len() {
		return leak_enemy(state, enemy, at);
	}
	let (next_path_index, next_cell): (usize, GridPosition) = positions[destination_index];
	let actor = BattleActorId::Enemy(enemy);
	let before_cell: Option<GridPosition> = actor_cell(state, actor);
	let former_blocker: Option<BattleActorId> =
		state.enemies.get(&enemy).and_then(|enemy| enemy.blocked_by);
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		end_block(state, enemy, at, BlockEndReason::Shift)?;
	let active = state
		.enemies
		.get_mut(&enemy)
		.expect("shift target presence was checked");
	active.path_index = next_path_index;
	active.runtime.cell = next_cell;
	active.runtime.visible = true;
	active.move_generation = active.move_generation.saturating_add(1);
	events.extend(actor_presence_events(
		state,
		actor,
		before_cell,
		Some(next_cell),
		at,
	)?);
	if let Some(blocker) = former_blocker {
		block_waiting_enemies(state, blocker, at)?;
	}
	if try_block_enemy(state, enemy, at) {
		return Ok(events);
	}
	events.push(move_event(state, enemy, at)?);
	Ok(events)
}

fn spawn_summon(
	state: &mut ExpectedState,
	template_id: SummonTemplateId,
	instance: u32,
	cell: GridPosition,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	spawn_summon_with_ownership(
		state,
		template_id,
		instance,
		cell,
		ActiveSummonOwnership::Independent,
		at,
	)
}

fn spawn_target_cell_actor(
	state: &mut ExpectedState,
	event: TargetCellActorSpawnEvent,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let (tile_source, tile_key, tile_max_layers): (RuntimeSource, MechanicId, u16) = {
		let Some(tile) = state
			.tile_effects
			.get(&event.tile_effect_id)
			.filter(|tile| {
				tile.generation == event.tile_effect_generation
					&& tile.cell == event.cell
					&& tile.source.actor == event.source
					&& tile.source.deployment_generation == event.source_deployment_generation
			})
		else {
			return Ok(Vec::new());
		};
		(tile.source, tile.spec.key.clone(), tile.spec.max_layers)
	};
	let runtime_source: RuntimeSource = match event.reaction_owner {
		TileLayerReactionOwner::TileEffect => tile_source,
		TileLayerReactionOwner::TileFieldOverlay {
			overlay_id,
			overlay_generation,
		} => {
			let Some(overlay) = state
				.tile_field_overlays
				.get(&overlay_id)
				.filter(|overlay| {
					overlay.generation == overlay_generation
						&& overlay.source == tile_source
						&& overlay.spec.observed_tile_key == tile_key
						&& overlay.spec.observed_tile_max_layers == Some(tile_max_layers)
				})
			else {
				return Ok(Vec::new());
			};
			overlay.source
		}
	};
	if matches!(
		event.ownership,
		TargetCellSpawnOwnership::SourceActor | TargetCellSpawnOwnership::SkillActivation
	) && !runtime_source_is_live(state, runtime_source)
	{
		return Ok(Vec::new());
	}
	if event.collision == TargetCellSpawnCollision::SuppressIfFriendlyOccupied
		&& friendly_cell_occupied(state, event.cell)
	{
		return Ok(Vec::new());
	}
	let active_ownership: ActiveSummonOwnership = match event.ownership {
		TargetCellSpawnOwnership::Independent => ActiveSummonOwnership::Independent,
		TargetCellSpawnOwnership::SourceActor => ActiveSummonOwnership::SourceActor(runtime_source),
		TargetCellSpawnOwnership::SkillActivation => {
			ActiveSummonOwnership::SkillActivation(runtime_source)
		}
		TargetCellSpawnOwnership::TriggeringTileEffect => {
			ActiveSummonOwnership::TriggeringTileEffect {
				tile_effect_id: event.tile_effect_id,
			}
		}
	};
	spawn_summon_with_ownership(
		state,
		event.template,
		event.instance,
		event.cell,
		active_ownership,
		at,
	)
}

fn spawn_summon_with_ownership(
	state: &mut ExpectedState,
	template_id: SummonTemplateId,
	instance: u32,
	cell: GridPosition,
	ownership: ActiveSummonOwnership,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let template: SummonTemplate = state
		.scenario
		.battle
		.facts
		.summons
		.iter()
		.find(|template| template.id == template_id)
		.cloned()
		.ok_or_else(|| ModelError::invalid_transition("summon template disappeared"))?;
	let summon = ActiveSummon {
		runtime: RuntimeActor {
			hp: template.combat.max_hp,
			cell,
			visible: true,
			facing: state
				.scenario
				.battle
				.facts
				.preplaced_actors
				.iter()
				.find(|actor| actor.instance == instance)
				.map_or(Facing::Right, |actor| actor.facing),
			attack_count: 0,
			attack_generation: 0,
			modifiers: Vec::new(),
			statuses: BTreeMap::new(),
		},
		template,
		blocking: BTreeSet::new(),
		ownership,
	};
	let first_delay: u64 = summon.template.attack.first_delay_ticks;
	state.summons.insert(instance, summon);
	let actor = BattleActorId::Summon(instance);
	let mut events = vec![normal_attack_event(
		actor,
		0,
		checked_tick_add(at, first_delay)?,
	)];
	events.extend(actor_presence_events(state, actor, None, Some(cell), at)?);
	if let Some(lifetime) = state
		.summons
		.get(&instance)
		.and_then(|summon| summon.template.lifetime_ticks)
	{
		events.push(resolve_effect_event(
			actor,
			None,
			vec![actor],
			TaggedEffect {
				mechanic: MechanicId::new("summon_lifetime"),
				target: EffectTarget::CurrentTargets,
				effect: ExpectedEffect::Death,
			},
			checked_tick_add(at, lifetime)?,
		));
	}
	if state
		.summons
		.get(&instance)
		.is_some_and(|summon| summon.template.faction == Faction::Friendly)
	{
		events.extend(block_waiting_enemies(state, actor, at)?);
	}
	Ok(events)
}

fn apply_death(
	state: &mut ExpectedState,
	actor: BattleActorId,
	operator_deployment_generation: Option<u32>,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	if actor != BattleActorId::Stage && actor_hp(state, actor) != Some(ExpectedValue::ZERO) {
		state.pending_death_sources.remove(&actor);
		return Ok(Vec::new());
	}
	let effect_source: BattleActorId = state.pending_death_sources.remove(&actor).unwrap_or(actor);
	match actor {
		BattleActorId::Operator(operator) => {
			if state
				.deployed
				.get(&operator)
				.map(|deployed| deployed.deployment_generation)
				!= operator_deployment_generation
			{
				return Ok(Vec::new());
			}
			invalidate_stored_attack_reservations_for_target_death(state, actor)?;
			let source: RuntimeSource =
				runtime_source(state, actor, operator_deployment_generation);
			let mut events = fire_trigger(
				state,
				TriggerContext {
					kind: ReplayTriggerKind::Death,
					actor,
					effect_source,
				},
				at,
			)?;
			events.extend(actor_presence_events(
				state,
				actor,
				actor_cell(state, actor),
				None,
				at,
			)?);
			events.extend(cleanup_runtime_effects(state, source, false, at)?);
			events.extend(cleanup_target_buffs(
				state,
				actor,
				operator_deployment_generation,
				at,
			)?);
			let Some(deployed) = state.deployed.remove(&operator) else {
				return Ok(Vec::new());
			};
			events.extend(reconcile_conditional_auras(state, at)?);
			retire_operator_attack_generation(state, operator, deployed.runtime.attack_generation)?;
			if let Some(report) = state.operator_reports.get_mut(&operator) {
				report.remaining_hp = ExpectedValue::ZERO;
				report.death_at.get_or_insert(at);
			}
			events.extend(begin_redeployment_cooldown(
				state,
				operator,
				deployed.build.redeployment_cooldown_ticks,
				at,
			)?);
			events.extend(unblock_enemies(
				state,
				actor,
				at,
				BlockEndReason::BlockerDeath,
			)?);
			events.extend(effect_events(
				actor,
				None,
				&deployed.build.on_death,
				Vec::new(),
				at,
			));
			Ok(events)
		}
		BattleActorId::Enemy(enemy) => {
			if !state.enemies.contains_key(&enemy) {
				return Ok(Vec::new());
			}
			invalidate_stored_attack_reservations_for_target_death(state, actor)?;
			let mut events = fire_trigger(
				state,
				TriggerContext {
					kind: ReplayTriggerKind::Death,
					actor,
					effect_source,
				},
				at,
			)?;
			events.extend(actor_presence_events(
				state,
				actor,
				actor_cell(state, actor),
				None,
				at,
			)?);
			let runtime_source: RuntimeSource = runtime_source(state, actor, None);
			events.extend(cleanup_runtime_effects(state, runtime_source, false, at)?);
			events.extend(cleanup_target_buffs(state, actor, None, at)?);
			let blocker: Option<BattleActorId> =
				state.enemies.get(&enemy).and_then(|enemy| enemy.blocked_by);
			let _ = end_block(state, enemy, at, BlockEndReason::EnemyDeath)?;
			let Some(active) = state.enemies.remove(&enemy) else {
				return Ok(Vec::new());
			};
			state.defeated.insert(enemy);
			state.pending_enemy_death_drains.insert(enemy);
			if let Some(report) = state.enemy_reports.get_mut(&enemy) {
				report.remaining_hp = ExpectedValue::ZERO;
				report.death_at.get_or_insert(at);
			}
			events.extend(effect_events(
				actor,
				None,
				&active.facts.on_death,
				Vec::new(),
				at,
			));
			events.extend(record_anchor(
				state,
				BattleTimeAnchor::EnemyDefeated(enemy),
				at,
			)?);
			events.extend(record_wave_clearance(state, enemy, at)?);
			if let Some(blocker) = blocker {
				events.extend(block_waiting_enemies(state, blocker, at)?);
			}
			events.push(enemy_death_effects_drained_event(enemy, at));
			Ok(events)
		}
		BattleActorId::Summon(instance) => {
			invalidate_stored_attack_reservations_for_target_death(state, actor)?;
			let mut events = fire_trigger(
				state,
				TriggerContext {
					kind: ReplayTriggerKind::Death,
					actor,
					effect_source,
				},
				at,
			)?;
			events.extend(actor_presence_events(
				state,
				actor,
				actor_cell(state, actor),
				None,
				at,
			)?);
			let runtime_source: RuntimeSource = runtime_source(state, actor, None);
			events.extend(cleanup_runtime_effects(state, runtime_source, false, at)?);
			events.extend(cleanup_target_buffs(state, actor, None, at)?);
			if state.summons.remove(&instance).is_none() {
				return Ok(Vec::new());
			}
			events.extend(unblock_enemies(
				state,
				actor,
				at,
				BlockEndReason::BlockerDeath,
			)?);
			Ok(events)
		}
		BattleActorId::Stage => Ok(Vec::new()),
	}
}

fn skill_periodic_sp_sources(skill: &SkillFacts) -> impl Iterator<Item = PeriodicSpSource> + '_ {
	let primary: Option<PeriodicSpSource> = match skill.charge {
		SpChargeRule::Time {
			amount,
			interval_ticks,
		} => Some(PeriodicSpSource {
			amount: ExpectedValue::from_whole(amount),
			interval_ticks,
		}),
		SpChargeRule::None | SpChargeRule::Attack { .. } | SpChargeRule::Hit { .. } => None,
	};
	let natural_recovery_modifiers: &[PeriodicSpSource] = if primary.is_some() {
		&skill.periodic_sp_sources
	} else {
		&[]
	};
	primary
		.into_iter()
		.chain(natural_recovery_modifiers.iter().copied())
}

fn periodic_sp_pulse_events(
	operator: OperatorInstanceId,
	deployment_generation: u32,
	charge_generation: u32,
	skill: &SkillFacts,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	skill_periodic_sp_sources(skill)
		.map(|source: PeriodicSpSource| {
			Ok(EventRequest::new(
				checked_tick_add(at, source.interval_ticks)?,
				SKILL_PRIORITY,
				operator_entity(operator),
				ExpectedEvent::SpPulse {
					operator,
					deployment_generation,
					charge_generation,
					source,
				},
			))
		})
		.collect()
}

fn apply_sp_pulse(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	deployment_generation: u32,
	charge_generation: u32,
	source: PeriodicSpSource,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(deployed) = state.deployed.get(&operator) else {
		return Ok(Vec::new());
	};
	if deployed.deployment_generation != deployment_generation
		|| deployed.sp_charge_generation != charge_generation
		|| deployed.skill_active
	{
		return Ok(Vec::new());
	}
	let Some(skill) = &deployed.build.skill else {
		return Ok(Vec::new());
	};
	if !skill_periodic_sp_sources(skill).any(|candidate: PeriodicSpSource| candidate == source) {
		return Ok(Vec::new());
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		charge_operator_sp(state, operator, source.amount, at)?;
	events.push(EventRequest::new(
		checked_tick_add(at, source.interval_ticks)?,
		SKILL_PRIORITY,
		operator_entity(operator),
		ExpectedEvent::SpPulse {
			operator,
			deployment_generation,
			charge_generation,
			source,
		},
	));
	Ok(events)
}

fn stored_attack_replacement_sp_capacity(skill: &SkillFacts) -> Option<ExpectedValue> {
	let facts: &StoredAttackReplacementFacts = skill.stored_attack_replacement.as_ref()?;
	skill
		.sp_cost
		.checked_mul(u32::from(facts.max_charges))
		.map(ExpectedValue::from_whole)
}

fn skill_sp_cost(skill: &SkillFacts) -> ExpectedValue {
	ExpectedValue::from_whole(skill.sp_cost)
}

fn skill_sp_capacity(skill: &SkillFacts) -> ExpectedValue {
	stored_attack_replacement_sp_capacity(skill).unwrap_or_else(|| skill_sp_cost(skill))
}

fn activate_skill(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	mechanic: &MechanicId,
	deployment_generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(deployed) = state.deployed.get(&operator) else {
		return Ok(Vec::new());
	};
	if deployed.deployment_generation != deployment_generation {
		return Ok(Vec::new());
	}
	let Some(skill) = deployed.build.skill.as_ref().cloned() else {
		return Ok(Vec::new());
	};
	if skill.stored_attack_replacement.is_some() {
		return Err(ModelError::invalid_transition(
			"stored-attack replacement skill cannot enter the active-skill lifecycle",
		));
	}
	if &skill.id != mechanic {
		return Err(ModelError::invalid_transition(
			"skill activation mechanic does not match selected skill",
		));
	}
	let passive: bool = skill.activation == SkillActivationRule::Passive;
	let sp_cost: ExpectedValue = skill_sp_cost(&skill);
	if !passive && (deployed.sp < sp_cost || deployed.skill_active) {
		if let Some(deployed) = state.deployed.get_mut(&operator) {
			deployed.skill_activation_pending = false;
		}
		return Ok(Vec::new());
	}
	let deployed = state
		.deployed
		.get_mut(&operator)
		.expect("skill owner presence was checked");
	if !passive {
		deployed.sp = deployed.sp.saturating_sub(sp_cost);
	}
	deployed.skill_activation_pending = false;
	let charge_generation: Option<u32> = if !passive {
		deployed.skill_active = true;
		deployed.skill_activation_count = deployed
			.skill_activation_count
			.checked_add(1)
			.ok_or_else(|| ModelError::invalid_transition("skill activation count overflow"))?;
		deployed.sp_charge_generation = deployed
			.sp_charge_generation
			.checked_add(1)
			.ok_or_else(|| ModelError::invalid_transition("skill charge generation overflow"))?;
		Some(deployed.sp_charge_generation)
	} else {
		None
	};
	let actor = BattleActorId::Operator(operator);
	let source: RuntimeSource = runtime_source(state, actor, Some(deployment_generation));
	let mut events: Vec<EventRequest<ExpectedEvent>> = if passive {
		Vec::new()
	} else {
		set_skill_inactive_stances_active(state, source, false, at)?
	};
	events.extend(fire_trigger(
		state,
		TriggerContext {
			kind: ReplayTriggerKind::SkillStarted,
			actor,
			effect_source: actor,
		},
		at,
	)?);
	events.extend(effect_events(
		actor,
		Some(deployment_generation),
		&skill.effects,
		Vec::new(),
		at,
	));
	if let (Some(duration_ticks), Some(charge_generation)) =
		(skill.active_duration_ticks, charge_generation)
	{
		let priority: i32 = if duration_ticks == 0 {
			INSTANT_SKILL_END_PRIORITY
		} else {
			SKILL_PRIORITY
		};
		events.push(EventRequest::new(
			checked_tick_add(at, duration_ticks)?,
			priority,
			operator_entity(operator),
			ExpectedEvent::SkillEnded {
				operator,
				deployment_generation,
				charge_generation,
				reason: SkillEndReason::DurationElapsed,
			},
		));
	}
	Ok(events)
}

fn apply_skill_discarded(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	deployment_generation: u32,
	charge_generation: u32,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(deployed) = state.deployed.get(&operator) else {
		return Ok(Vec::new());
	};
	if deployed.deployment_generation != deployment_generation
		|| deployed.sp_charge_generation != charge_generation
		|| !deployed.skill_active
		|| !deployed.skill_discard_pending
	{
		return Ok(Vec::new());
	}
	let Some(policy) = deployed
		.build
		.skill
		.as_ref()
		.and_then(|skill| skill.manual_discard.clone())
	else {
		return Err(ModelError::invalid_transition(
			"pending skill discard has no controller policy",
		));
	};
	if !policy.allows(deployed.skill_activation_count, false) {
		return Err(ModelError::invalid_transition(
			"pending skill discard exceeds its activation limit",
		));
	}
	let actor = BattleActorId::Operator(operator);
	let mut events: Vec<EventRequest<ExpectedEvent>> = effect_events(
		actor,
		Some(deployment_generation),
		&policy.on_discard,
		Vec::new(),
		at,
	);
	events.push(EventRequest::new(
		at,
		SKILL_DISCARD_END_PRIORITY,
		operator_entity(operator),
		ExpectedEvent::SkillEnded {
			operator,
			deployment_generation,
			charge_generation,
			reason: SkillEndReason::ManualDiscard,
		},
	));
	Ok(events)
}

fn apply_skill_ended(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	deployment_generation: u32,
	charge_generation: u32,
	reason: SkillEndReason,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(deployed) = state.deployed.get_mut(&operator) else {
		return Ok(Vec::new());
	};
	if deployed.deployment_generation != deployment_generation
		|| deployed.sp_charge_generation != charge_generation
		|| !deployed.skill_active
		|| (reason == SkillEndReason::ManualDiscard && !deployed.skill_discard_pending)
	{
		return Ok(Vec::new());
	}
	deployed.skill_active = false;
	deployed.skill_discard_pending = false;
	let Some(skill) = deployed.build.skill.clone() else {
		return Ok(Vec::new());
	};
	let actor = BattleActorId::Operator(operator);
	let source: RuntimeSource = runtime_source(state, actor, Some(deployment_generation));
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		set_skill_inactive_stances_active(state, source, true, at)?;
	events.extend(fire_trigger(
		state,
		TriggerContext {
			kind: ReplayTriggerKind::SkillEnded,
			actor,
			effect_source: actor,
		},
		at,
	)?);
	events.extend(cleanup_runtime_effects(state, source, true, at)?);
	events.extend(periodic_sp_pulse_events(
		operator,
		deployment_generation,
		charge_generation,
		&skill,
		at,
	)?);
	Ok(events)
}

fn charge_operator_sp(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	amount: ExpectedValue,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let daemon_activates_manual: bool =
		state.skill_daemon_enabled && state.scenario.policy.skill_usage(operator) == Some(1);
	let Some(deployed) = state.deployed.get_mut(&operator) else {
		return Ok(Vec::new());
	};
	if deployed.skill_active {
		return Ok(Vec::new());
	}
	let Some(skill) = deployed.build.skill.as_ref() else {
		return Ok(Vec::new());
	};
	let replacement_capacity: Option<ExpectedValue> = stored_attack_replacement_sp_capacity(skill);
	let capacity: ExpectedValue = skill_sp_capacity(skill);
	deployed.sp = deployed
		.sp
		.checked_add(amount)
		.unwrap_or(capacity)
		.min(capacity);
	let deployment_generation: u32 = deployed.deployment_generation;
	if replacement_capacity.is_some() {
		return Ok(Vec::new());
	}
	if (skill.activation == SkillActivationRule::Auto
		|| (skill.activation == SkillActivationRule::Manual && daemon_activates_manual))
		&& deployed.sp >= skill_sp_cost(skill)
		&& !deployed.skill_activation_pending
	{
		deployed.skill_activation_pending = true;
		let origin: SkillActivationOrigin = if skill.activation == SkillActivationRule::Auto {
			SkillActivationOrigin::Automatic
		} else {
			SkillActivationOrigin::SkillDaemon
		};
		return Ok(vec![skill_activation_event(
			operator,
			skill.id.clone(),
			deployment_generation,
			origin,
			at,
		)]);
	}
	Ok(Vec::new())
}

fn charge_sp_for_attack(
	state: &mut ExpectedState,
	actor: BattleActorId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let BattleActorId::Operator(operator) = actor else {
		return Ok(Vec::new());
	};
	let amount: Option<u32> = state
		.deployed
		.get(&operator)
		.and_then(|deployed| deployed.build.skill.as_ref())
		.and_then(|skill| match skill.charge {
			SpChargeRule::Attack { amount } => Some(amount),
			SpChargeRule::None | SpChargeRule::Time { .. } | SpChargeRule::Hit { .. } => None,
		});
	match amount {
		Some(amount) => charge_operator_sp(state, operator, ExpectedValue::from_whole(amount), at),
		None => Ok(Vec::new()),
	}
}

fn charge_sp_for_hit(
	state: &mut ExpectedState,
	target: BattleActorId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let BattleActorId::Operator(operator) = target else {
		return Ok(Vec::new());
	};
	let amount: Option<u32> = state
		.deployed
		.get(&operator)
		.and_then(|deployed| deployed.build.skill.as_ref())
		.and_then(|skill| match skill.charge {
			SpChargeRule::Hit { amount } => Some(amount),
			SpChargeRule::None | SpChargeRule::Time { .. } | SpChargeRule::Attack { .. } => None,
		});
	match amount {
		Some(amount) => charge_operator_sp(state, operator, ExpectedValue::from_whole(amount), at),
		None => Ok(Vec::new()),
	}
}

fn skill_is_ready(state: &ExpectedState, operator: OperatorInstanceId) -> bool {
	state.deployed.get(&operator).is_some_and(|deployed| {
		deployed.build.skill.as_ref().is_some_and(|skill| {
			skill.activation == SkillActivationRule::Manual
				&& deployed.sp >= skill_sp_cost(skill)
				&& !deployed.skill_activation_pending
				&& !deployed.skill_active
		})
	})
}

fn skill_can_be_manually_discarded(state: &ExpectedState, operator: OperatorInstanceId) -> bool {
	state.deployed.get(&operator).is_some_and(|deployed| {
		deployed.skill_active
			&& !deployed.skill_discard_pending
			&& deployed.build.skill.as_ref().is_some_and(|skill| {
				skill
					.manual_discard
					.as_ref()
					.is_some_and(|policy| policy.allows(deployed.skill_activation_count, false))
			})
	})
}

fn select_stored_attack_replacement_target(
	state: &ExpectedState,
	source: BattleActorId,
	facts: &StoredAttackReplacementFacts,
) -> Result<Option<BattleActorId>, ModelError> {
	let range: BTreeSet<GridPosition> =
		projected_range_cells(state, source, facts.range.iter().copied());
	let mut candidates: Vec<(BattleActorId, ExpectedValue, ExpectedValue)> = Vec::new();
	let related: Vec<BattleActorId> = match facts.relationship {
		StoredAttackTargetRelationship::Ally => allied_actors(state, source),
		StoredAttackTargetRelationship::Enemy => opposing_actors(state, source),
	};
	for actor in related {
		if (!facts.include_source && actor == source)
			|| (facts.require_healable && !actor_is_healable(state, actor))
			|| actor_combat_facts(state, actor)
				.is_none_or(|combat| !facts.target_filter.allows(combat.target_class))
			|| actor_cell(state, actor).is_none_or(|cell| !range.contains(&cell))
		{
			continue;
		}
		let Some(hp) = actor_hp(state, actor) else {
			continue;
		};
		let max_hp: ExpectedValue = effective_actor_stat(state, actor, StatKind::MaxHp)?;
		if hp > ExpectedValue::ZERO
			&& stored_attack_hp_ratio_matches(
				hp,
				max_hp,
				facts.hp_ratio_comparison,
				facts.hp_ratio_threshold,
			) {
			candidates.push((actor, hp, max_hp));
		}
	}
	match facts.target_order {
		StoredAttackTargetOrder::LowestHpRatio => candidates.sort_by(|left, right| {
			let left_ratio: u128 =
				u128::from(left.1.scaled()).saturating_mul(u128::from(right.2.scaled()));
			let right_ratio: u128 =
				u128::from(right.1.scaled()).saturating_mul(u128::from(left.2.scaled()));
			left_ratio
				.cmp(&right_ratio)
				.then_with(|| left.0.cmp(&right.0))
		}),
	}
	Ok(candidates.first().map(|candidate| candidate.0))
}

fn stored_attack_hp_ratio_matches(
	hp: ExpectedValue,
	max_hp: ExpectedValue,
	comparison: StoredAttackHpRatioComparison,
	threshold: ExpectedValue,
) -> bool {
	if max_hp == ExpectedValue::ZERO {
		return false;
	}
	let current: u128 = u128::from(hp.scaled()).saturating_mul(u128::from(ExpectedValue::SCALE));
	let limit: u128 = u128::from(max_hp.scaled()).saturating_mul(u128::from(threshold.scaled()));
	match comparison {
		StoredAttackHpRatioComparison::LessThan => current < limit,
		StoredAttackHpRatioComparison::LessThanOrEqual => current <= limit,
	}
}

fn reserve_stored_attack_replacement(
	state: &mut ExpectedState,
	operator: OperatorInstanceId,
	mechanic: MechanicId,
	target: BattleActorId,
	spent_sp: ExpectedValue,
	facts: StoredAttackReplacementFacts,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let source = BattleActorId::Operator(operator);
	let deployment_generation: u32 = state
		.deployed
		.get(&operator)
		.map(|deployed| deployed.deployment_generation)
		.ok_or_else(|| ModelError::invalid_transition("replacement source disappeared"))?;
	let target_ref = StoredAttackTargetRef {
		actor: target,
		deployment_generation: operator_deployment_generation(state, target),
	};
	increment_attack_count(state, source)?;
	let deployed = state
		.deployed
		.get_mut(&operator)
		.expect("replacement source presence was checked");
	if deployed.sp < spent_sp {
		return Err(ModelError::invalid_transition(
			"replacement reservation no longer has enough SP",
		));
	}
	deployed.sp = deployed.sp.saturating_sub(spent_sp);
	let reservation_id: u64 = allocate_runtime_effect_id(state)?;
	let source_ref: RuntimeSource = runtime_source(state, source, Some(deployment_generation));
	let delivery_at: SimTick = checked_tick_add(at, facts.delivery_delay_ticks)?;
	if state
		.stored_attack_reservations
		.insert(
			reservation_id,
			ActiveStoredAttackReservation {
				mechanic,
				source: source_ref,
				target: target_ref,
				facts,
				spent_sp,
			},
		)
		.is_some()
	{
		return Err(ModelError::invalid_transition(
			"replacement reservation identity collision",
		));
	}
	Ok(vec![
		EventRequest::new(
			delivery_at,
			EFFECT_PRIORITY,
			actor_entity(source),
			ExpectedEvent::StoredAttackReplacementDelivery {
				reservation_id,
				source,
				target: target_ref,
			},
		),
		schedule_next_attack(state, source, at)?,
	])
}

fn apply_stored_attack_replacement_delivery(
	state: &mut ExpectedState,
	reservation_id: u64,
	source: BattleActorId,
	target: StoredAttackTargetRef,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(reservation) = state
		.stored_attack_reservations
		.get(&reservation_id)
		.cloned()
	else {
		return Ok(Vec::new());
	};
	if reservation.source.actor != source || reservation.target != target {
		return Err(ModelError::invalid_transition(
			"replacement delivery does not match its reservation",
		));
	}
	if !runtime_source_is_live(state, reservation.source) {
		state.stored_attack_reservations.remove(&reservation_id);
		return Ok(Vec::new());
	}
	let invalidation: Option<StoredAttackReservationInvalidationAction> =
		if !actor_exists(state, target.actor) || actor_cell(state, target.actor).is_none() {
			Some(reservation.facts.invalidation.target_disappeared)
		} else if !stored_attack_target_generation_matches(state, target) {
			Some(reservation.facts.invalidation.target_generation_changed)
		} else if actor_hp(state, target.actor) == Some(ExpectedValue::ZERO) {
			Some(reservation.facts.invalidation.target_death)
		} else if !stored_attack_target_is_in_range(state, &reservation) {
			Some(reservation.facts.invalidation.target_out_of_range)
		} else {
			None
		};
	if let Some(action) = invalidation {
		return invalidate_stored_attack_reservation(state, reservation_id, action);
	}
	state.stored_attack_reservations.remove(&reservation_id);
	let amount: ExpectedValue = match reservation.facts.delivery {
		StoredAttackReplacementDelivery::SourceStatScaledHealing { stat, multiplier } => {
			effective_actor_stat(state, source, stat)?
				.checked_mul(multiplier)
				.ok_or_else(|| {
					ModelError::invalid_transition("replacement delivery amount overflow")
				})?
		}
	};
	let mut events = vec![resolve_effect_event(
		source,
		reservation.source.deployment_generation,
		vec![target.actor],
		TaggedEffect {
			mechanic: reservation.mechanic,
			target: EffectTarget::CurrentTargets,
			effect: ExpectedEffect::Healing { amount },
		},
		at,
	)];
	events.extend(fire_trigger(
		state,
		TriggerContext {
			kind: ReplayTriggerKind::NormalAttackHit,
			actor: target.actor,
			effect_source: source,
		},
		at,
	)?);
	Ok(events)
}

fn stored_attack_target_generation_matches(
	state: &ExpectedState,
	target: StoredAttackTargetRef,
) -> bool {
	match target.actor {
		BattleActorId::Operator(_) => {
			operator_deployment_generation(state, target.actor) == target.deployment_generation
		}
		BattleActorId::Enemy(_) | BattleActorId::Summon(_) => {
			target.deployment_generation.is_none()
		}
		BattleActorId::Stage => false,
	}
}

fn stored_attack_target_is_in_range(
	state: &ExpectedState,
	reservation: &ActiveStoredAttackReservation,
) -> bool {
	let cells: BTreeSet<GridPosition> = projected_range_cells(
		state,
		reservation.source.actor,
		reservation.facts.range.iter().copied(),
	);
	actor_cell(state, reservation.target.actor).is_some_and(|cell| cells.contains(&cell))
}

fn invalidate_stored_attack_reservation(
	state: &mut ExpectedState,
	reservation_id: u64,
	action: StoredAttackReservationInvalidationAction,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(reservation) = state.stored_attack_reservations.remove(&reservation_id) else {
		return Ok(Vec::new());
	};
	match action {
		StoredAttackReservationInvalidationAction::RefundSpentSp => {
			refund_stored_attack_reservation_sp(state, &reservation)?;
		}
		StoredAttackReservationInvalidationAction::ConsumeSpentSp => {}
		StoredAttackReservationInvalidationAction::Unsupported => {
			return Err(ModelError::invalid_transition(
				"replacement invalidation policy is unsupported",
			));
		}
	}
	Ok(Vec::new())
}

fn refund_stored_attack_reservation_sp(
	state: &mut ExpectedState,
	reservation: &ActiveStoredAttackReservation,
) -> Result<(), ModelError> {
	let BattleActorId::Operator(operator) = reservation.source.actor else {
		return Err(ModelError::invalid_transition(
			"replacement reservation source is not an operator",
		));
	};
	let Some(deployed) = state.deployed.get_mut(&operator) else {
		return Ok(());
	};
	if reservation.source.deployment_generation != Some(deployed.deployment_generation) {
		return Ok(());
	}
	let Some(skill) = deployed.build.skill.as_ref() else {
		return Err(ModelError::invalid_transition(
			"replacement reservation source lost its skill",
		));
	};
	if skill.id != reservation.mechanic || skill.stored_attack_replacement.is_none() {
		return Err(ModelError::invalid_transition(
			"replacement reservation source changed skill",
		));
	}
	let capacity: ExpectedValue = skill_sp_capacity(skill);
	deployed.sp = deployed
		.sp
		.checked_add(reservation.spent_sp)
		.unwrap_or(capacity)
		.min(capacity);
	Ok(())
}

fn invalidate_stored_attack_reservations_for_target_death(
	state: &mut ExpectedState,
	target: BattleActorId,
) -> Result<(), ModelError> {
	let reservations: Vec<(u64, StoredAttackReservationInvalidationAction)> = state
		.stored_attack_reservations
		.iter()
		.filter_map(|(id, reservation)| {
			(reservation.target.actor == target)
				.then_some((*id, reservation.facts.invalidation.target_death))
		})
		.collect();
	for (reservation_id, action) in reservations {
		invalidate_stored_attack_reservation(state, reservation_id, action)?;
	}
	Ok(())
}

fn actor_is_healable(state: &ExpectedState, actor: BattleActorId) -> bool {
	match actor {
		BattleActorId::Operator(_) => true,
		BattleActorId::Summon(instance) => state
			.summons
			.get(&instance)
			.is_some_and(|summon| summon.template.targetability == Targetability::All),
		BattleActorId::Enemy(_) | BattleActorId::Stage => false,
	}
}

fn select_normal_targets(
	state: &ExpectedState,
	actor: BattleActorId,
	attack: &AttackProfile,
) -> Vec<BattleActorId> {
	let action_targets_allies: bool = attack.action == NormalAttackAction::Heal;
	let mut candidates: Vec<BattleActorId> = if action_targets_allies {
		allied_actors(state, actor)
	} else {
		opposing_actors(state, actor)
	};
	candidates.retain(|candidate| actor_targetable_by(state, actor, *candidate));
	candidates.retain(|candidate| {
		actor_combat_facts(state, *candidate)
			.is_some_and(|combat| attack.target_filter.allows(combat.target_class))
	});
	let range: BTreeSet<GridPosition> = effective_range_cells(state, actor, attack);
	candidates.retain(|candidate| {
		actor_cell(state, *candidate).is_some_and(|cell| range.contains(&cell))
	});
	if action_targets_allies {
		candidates.sort_by_key(|candidate| {
			let hp: ExpectedValue = actor_hp(state, *candidate).unwrap_or(ExpectedValue::ZERO);
			let max_hp: ExpectedValue =
				effective_actor_stat(state, *candidate, StatKind::MaxHp).unwrap_or(hp);
			(std::cmp::Reverse(max_hp.saturating_sub(hp)), *candidate)
		});
	} else {
		let blocker: Option<BattleActorId> = match actor {
			BattleActorId::Enemy(enemy) => {
				state.enemies.get(&enemy).and_then(|enemy| enemy.blocked_by)
			}
			BattleActorId::Operator(_) | BattleActorId::Summon(_) | BattleActorId::Stage => None,
		};
		candidates.sort_by_key(|candidate| {
			(
				blocker.is_none_or(|blocker| blocker != *candidate),
				target_priority(state, *candidate),
			)
		});
	}
	let target_count: usize = effective_target_count(state, actor, attack);
	candidates.truncate(target_count);
	candidates
}

fn select_effect_targets(
	state: &ExpectedState,
	source: BattleActorId,
	target: EffectTarget,
) -> Vec<BattleActorId> {
	match target {
		EffectTarget::CurrentTargets => Vec::new(),
		EffectTarget::TriggerEffectSource => Vec::new(),
		EffectTarget::SelfActor => actor_exists(state, source)
			.then_some(source)
			.into_iter()
			.collect(),
		EffectTarget::Blocker => match source {
			BattleActorId::Enemy(enemy) => state
				.enemies
				.get(&enemy)
				.and_then(|enemy| enemy.blocked_by)
				.into_iter()
				.collect(),
			BattleActorId::Operator(_) | BattleActorId::Summon(_) | BattleActorId::Stage => {
				Vec::new()
			}
		},
		EffectTarget::BlockedEnemies => blocked_enemies(state, source)
			.into_iter()
			.map(BattleActorId::Enemy)
			.collect(),
		EffectTarget::LowestHpAllyInRange => {
			let mut allies: Vec<BattleActorId> = allied_actors(state, source);
			let range: BTreeSet<GridPosition> = actor_attack_profile(state, source)
				.map_or_else(BTreeSet::new, |attack| {
					effective_range_cells(state, source, attack)
				});
			allies.retain(|actor| {
				actor_cell(state, *actor).is_some_and(|cell| range.contains(&cell))
			});
			allies.sort_by_key(|actor| {
				let hp: ExpectedValue = actor_hp(state, *actor).unwrap_or(ExpectedValue::ZERO);
				let max_hp: ExpectedValue =
					effective_actor_stat(state, *actor, StatKind::MaxHp).unwrap_or(hp);
				(std::cmp::Reverse(max_hp.saturating_sub(hp)), *actor)
			});
			allies.truncate(1);
			allies
		}
		EffectTarget::AlliesInRange | EffectTarget::EnemiesInRange => {
			let mut actors: Vec<BattleActorId> = if target == EffectTarget::AlliesInRange {
				allied_actors(state, source)
			} else {
				opposing_actors(state, source)
			};
			let range: BTreeSet<GridPosition> = actor_attack_profile(state, source)
				.map_or_else(BTreeSet::new, |attack| {
					effective_range_cells(state, source, attack)
				});
			actors.retain(|actor| {
				actor_cell(state, *actor).is_some_and(|cell| range.contains(&cell))
			});
			actors
		}
		EffectTarget::AllAllies => allied_actors(state, source),
		EffectTarget::AllEnemies => opposing_actors(state, source),
	}
}

fn cross_targets(
	state: &ExpectedState,
	source: BattleActorId,
	primary: BattleActorId,
	arm_length: u16,
) -> Vec<BattleActorId> {
	let Some(center) = actor_cell(state, primary) else {
		return Vec::new();
	};
	let target_filter = actor_attack_profile(state, source).map(|attack| attack.target_filter);
	let distance: u32 = u32::from(arm_length);
	let mut targets: Vec<BattleActorId> = opposing_actors(state, source)
		.into_iter()
		.filter(|target| actor_targetable_by(state, source, *target))
		.filter(|target| {
			target_filter.is_none_or(|filter| {
				actor_combat_facts(state, *target)
					.is_some_and(|combat| filter.allows(combat.target_class))
			})
		})
		.filter(|target| {
			actor_cell(state, *target).is_some_and(|cell| {
				let same_row: bool =
					cell.row == center.row && cell.col.abs_diff(center.col) <= distance;
				let same_col: bool =
					cell.col == center.col && cell.row.abs_diff(center.row) <= distance;
				same_row || same_col
			})
		})
		.collect();
	targets.sort_by_key(|target| target_priority(state, *target));
	targets
}

fn axis_direct_hit_direction(state: &ExpectedState, enemy: EnemyInstanceId) -> Option<u8> {
	let source: BattleActorId = BattleActorId::Enemy(enemy);
	let origin: GridPosition = actor_cell(state, source)?;
	let mut collisions: BTreeMap<u8, Vec<(u32, BattleActorId)>> = BTreeMap::new();
	let mut exposed: Vec<(BattleActorId, u8)> = Vec::new();
	for target in opposing_actors(state, source) {
		let Some(cell) = actor_cell(state, target) else {
			continue;
		};
		let Some((direction, distance)) = cardinal_direction(origin, cell) else {
			continue;
		};
		if actor_has_status(state, target, &StatusKind::Exposure) {
			exposed.push((target, direction));
		}
		if actor_collides_with_axis_direct_hit(state, source, target) {
			collisions
				.entry(direction)
				.or_default()
				.push((distance, target));
		}
	}
	let first_collisions: BTreeMap<u8, BattleActorId> = collisions
		.into_iter()
		.filter_map(|(direction, mut candidates)| {
			candidates.sort_by_key(|(distance, actor)| (*distance, *actor));
			candidates.first().map(|(_, target)| (direction, *target))
		})
		.collect();
	let mut triggers: BTreeMap<BattleActorId, u8> = first_collisions
		.iter()
		.filter(|(_, target)| actor_targetable_by(state, source, **target))
		.map(|(direction, target)| (*target, *direction))
		.collect();
	triggers.extend(exposed);
	let (_, direction): (BattleActorId, u8) = triggers
		.into_iter()
		.min_by_key(|(target, _)| target_priority(state, *target))?;
	Some(direction)
}

fn axis_direct_hit_collision(
	state: &ExpectedState,
	enemy: EnemyInstanceId,
	direction: u8,
) -> Option<BattleActorId> {
	let source: BattleActorId = BattleActorId::Enemy(enemy);
	let origin: GridPosition = actor_cell(state, source)?;
	opposing_actors(state, source)
		.into_iter()
		.filter(|target| actor_hp(state, *target) != Some(ExpectedValue::ZERO))
		.filter_map(|target| {
			let cell: GridPosition = actor_cell(state, target)?;
			let (target_direction, distance): (u8, u32) = cardinal_direction(origin, cell)?;
			(target_direction == direction
				&& actor_collides_with_axis_direct_hit(state, source, target))
			.then_some((distance, target))
		})
		.min_by_key(|(distance, target)| (*distance, *target))
		.map(|(_, target)| target)
}

fn cardinal_direction(origin: GridPosition, target: GridPosition) -> Option<(u8, u32)> {
	if target.col == origin.col && target.row < origin.row {
		Some((0, origin.row - target.row))
	} else if target.row == origin.row && target.col > origin.col {
		Some((1, target.col - origin.col))
	} else if target.col == origin.col && target.row > origin.row {
		Some((2, target.row - origin.row))
	} else if target.row == origin.row && target.col < origin.col {
		Some((3, origin.col - target.col))
	} else {
		None
	}
}

fn actor_collides_with_axis_direct_hit(
	state: &ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
) -> bool {
	match target {
		BattleActorId::Operator(_) => true,
		BattleActorId::Summon(instance) => match state
			.summons
			.get(&instance)
			.map_or(Targetability::None, |summon| summon.template.targetability)
		{
			Targetability::All | Targetability::Shelter => true,
			Targetability::BlockedOnly => match source {
				BattleActorId::Enemy(enemy) => state
					.enemies
					.get(&enemy)
					.is_some_and(|enemy| enemy.blocked_by == Some(target)),
				BattleActorId::Operator(_) | BattleActorId::Summon(_) | BattleActorId::Stage => {
					false
				}
			},
			Targetability::None => false,
		},
		BattleActorId::Enemy(_) | BattleActorId::Stage => false,
	}
}

fn effective_range_cells(
	state: &ExpectedState,
	actor: BattleActorId,
	attack: &AttackProfile,
) -> BTreeSet<GridPosition> {
	let offsets = attack.range.iter().copied().chain(
		actor_runtime(state, actor)
			.into_iter()
			.flat_map(|runtime| runtime.modifiers.iter())
			.filter_map(|modifier| match &modifier.kind {
				ActiveModifierKind::AttackRange(modifier) => {
					Some(modifier.additional_offsets.as_slice())
				}
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::ArtsResistanceAddition(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::MovementSpeedReduction(_)
				| ActiveModifierKind::AttackCadence(_)
				| ActiveModifierKind::AttackSpeed(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::HitCount(_)
				| ActiveModifierKind::Blocking(_)
				| ActiveModifierKind::BlockTarget(_) => None,
			})
			.flatten()
			.copied(),
	);
	projected_range_cells(state, actor, offsets)
}

fn projected_range_cells(
	state: &ExpectedState,
	actor: BattleActorId,
	offsets: impl Iterator<Item = RangeOffset>,
) -> BTreeSet<GridPosition> {
	let Some(anchor) = actor_cell(state, actor) else {
		return BTreeSet::new();
	};
	let facing: Facing = actor_facing(state, actor).unwrap_or(Facing::Right);
	let dimensions: MapDimensions = state.scenario.battle.facts.grid.dimensions;
	offsets
		.filter_map(|offset| project_offset(anchor, facing, offset, dimensions))
		.collect()
}

fn project_offset(
	anchor: GridPosition,
	facing: Facing,
	offset: RangeOffset,
	dimensions: MapDimensions,
) -> Option<GridPosition> {
	let (row_offset, col_offset): (i32, i32) = match facing {
		Facing::Right => (offset.row(), offset.col()),
		Facing::Down => (-offset.col(), offset.row()),
		Facing::Left => (-offset.row(), -offset.col()),
		Facing::Up => (offset.col(), -offset.row()),
	};
	let row: i64 = i64::from(anchor.row) + i64::from(row_offset);
	let col: i64 = i64::from(anchor.col) + i64::from(col_offset);
	if row < 0
		|| col < 0
		|| row >= i64::from(dimensions.height)
		|| col >= i64::from(dimensions.width)
	{
		return None;
	}
	Some(GridPosition {
		row: row as u32,
		col: col as u32,
	})
}

fn effective_actor_stat(
	state: &ExpectedState,
	actor: BattleActorId,
	stat: StatKind,
) -> Result<ExpectedValue, ModelError> {
	let combat: ActorCombatFacts = actor_combat_facts(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("actor combat facts disappeared"))?;
	let value: ExpectedValue = match stat {
		StatKind::Attack => combat.attack,
		StatKind::Defense => combat.defense,
		StatKind::MaxHp => combat.max_hp,
		StatKind::IncomingDamage => ExpectedValue::ONE,
	};
	// Buffs that share a bucket sum inside it and the buckets then multiply, so every panel ratio
	// resolves into one factor before any standalone scale applies. Folding them all
	// multiplicatively would pay a stacking bonus the game does not.
	let Some(runtime) = actor_runtime(state, actor) else {
		return Ok(value);
	};
	let mut panel_ratio_total: ExpectedValue = ExpectedValue::ZERO;
	let mut scale: ExpectedValue = ExpectedValue::ONE;
	for modifier in &runtime.modifiers {
		let modifier: &StatModifier = match &modifier.kind {
			ActiveModifierKind::Stat(modifier) => modifier,
			ActiveModifierKind::ConditionalStat(conditional)
				if actor_condition_holds(state, actor, conditional.condition) =>
			{
				&conditional.modifier
			}
			_ => continue,
		};
		if modifier.stat != stat {
			continue;
		}
		match modifier.bucket {
			StatModifierBucket::Panel => {
				// A panel modifier states what it would do alone, so its contribution to the
				// bucket is the part above one. Nothing reviewed lowers a panel stat, and a
				// negative contribution cannot be represented, so it fails closed here.
				if modifier.multiplier < ExpectedValue::ONE {
					return Err(ModelError::invalid_transition(
						"panel stat modifier below one has no reviewed representation",
					));
				}
				let ratio: ExpectedValue = modifier.multiplier.saturating_sub(ExpectedValue::ONE);
				panel_ratio_total = panel_ratio_total.checked_add(ratio).ok_or_else(|| {
					ModelError::invalid_transition("panel stat bucket arithmetic overflow")
				})?;
			}
			StatModifierBucket::Scale => {
				scale = scale.checked_mul(modifier.multiplier).ok_or_else(|| {
					ModelError::invalid_transition("stat modifier arithmetic overflow")
				})?;
			}
		}
	}
	let panel: ExpectedValue = ExpectedValue::ONE
		.checked_add(panel_ratio_total)
		.ok_or_else(|| ModelError::invalid_transition("panel stat bucket arithmetic overflow"))?;
	value
		.checked_mul(panel)
		.and_then(|value| value.checked_mul(scale))
		.ok_or_else(|| ModelError::invalid_transition("stat modifier arithmetic overflow"))
}

fn effective_incoming_multiplier(
	state: &ExpectedState,
	actor: BattleActorId,
	ignore_shelter: bool,
) -> Result<ExpectedValue, ModelError> {
	let mut multiplier: ExpectedValue =
		effective_actor_stat(state, actor, StatKind::IncomingDamage)?;
	if let Some(runtime) = actor_runtime(state, actor) {
		if !ignore_shelter {
			for modifier in &runtime.modifiers {
				if let ActiveModifierKind::Shelter(shelter_multiplier) = &modifier.kind {
					multiplier = multiplier.checked_mul(*shelter_multiplier).ok_or_else(|| {
						ModelError::invalid_transition("shelter buff multiplier overflow")
					})?;
				}
			}
		}
		for (kind, status) in &runtime.statuses {
			if ignore_shelter && *kind == StatusKind::Shelter {
				continue;
			}
			multiplier = multiplier
				.checked_mul(status.spec.incoming_damage_multiplier)
				.ok_or_else(|| {
					ModelError::invalid_transition("status damage multiplier overflow")
				})?;
		}
	}
	Ok(multiplier)
}

fn effective_target_count(
	state: &ExpectedState,
	actor: BattleActorId,
	attack: &AttackProfile,
) -> usize {
	let Some(runtime) = actor_runtime(state, actor) else {
		return usize::from(attack.target_count);
	};
	if runtime.modifiers.iter().any(|modifier| {
		matches!(
			modifier.kind,
			ActiveModifierKind::TargetCount(TargetCountModifier::Unlimited { .. })
		)
	}) {
		return usize::MAX;
	}
	let delta: i32 = runtime
		.modifiers
		.iter()
		.filter_map(|modifier| match modifier.kind {
			ActiveModifierKind::TargetCount(TargetCountModifier::Delta { delta, .. }) => {
				Some(i32::from(delta))
			}
			ActiveModifierKind::TargetCount(TargetCountModifier::Unlimited { .. })
			| ActiveModifierKind::Stat(_)
			| ActiveModifierKind::ConditionalStat(_)
			| ActiveModifierKind::ArtsResistanceAddition(_)
			| ActiveModifierKind::NormalAttackSuppression
			| ActiveModifierKind::Shelter(_)
			| ActiveModifierKind::MovementSpeedReduction(_)
			| ActiveModifierKind::AttackCadence(_)
			| ActiveModifierKind::AttackSpeed(_)
			| ActiveModifierKind::AttackRange(_)
			| ActiveModifierKind::HitCount(_)
			| ActiveModifierKind::Blocking(_)
			| ActiveModifierKind::BlockTarget(_) => None,
		})
		.sum();
	let total: i32 = i32::from(attack.target_count).saturating_add(delta).max(0);
	usize::try_from(total).unwrap_or(usize::MAX)
}

fn effective_hit_count(state: &ExpectedState, actor: BattleActorId) -> Result<u32, ModelError> {
	let delta: i64 = actor_runtime(state, actor).map_or(Ok(0_i64), |runtime| {
		runtime
			.modifiers
			.iter()
			.filter_map(|modifier| match modifier.kind {
				ActiveModifierKind::HitCount(modifier) => Some(i64::from(modifier.delta)),
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::ArtsResistanceAddition(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::MovementSpeedReduction(_)
				| ActiveModifierKind::AttackCadence(_)
				| ActiveModifierKind::AttackSpeed(_)
				| ActiveModifierKind::AttackRange(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::Blocking(_)
				| ActiveModifierKind::BlockTarget(_) => None,
			})
			.try_fold(0_i64, i64::checked_add)
			.ok_or_else(|| ModelError::invalid_transition("normal-attack hit count overflow"))
	})?;
	let total: i64 = 1_i64
		.checked_add(delta)
		.ok_or_else(|| ModelError::invalid_transition("normal-attack hit count overflow"))?;
	if total <= 0 {
		return Err(ModelError::invalid_transition(
			"normal-attack hit count must remain positive",
		));
	}
	u32::try_from(total)
		.map_err(|_| ModelError::invalid_transition("normal-attack hit count exceeds u32"))
}

fn effective_attack_interval(
	state: &ExpectedState,
	actor: BattleActorId,
) -> Result<u64, ModelError> {
	let attack: &AttackProfile = actor_attack_profile(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("attack interval owner disappeared"))?;
	let mut interval: ExpectedValue =
		ExpectedValue::from_scaled(attack.interval_ticks.saturating_mul(ExpectedValue::SCALE));
	if let Some(runtime) = actor_runtime(state, actor) {
		let mut attack_speed_delta: i64 = 0;
		for modifier in &runtime.modifiers {
			match &modifier.kind {
				ActiveModifierKind::AttackCadence(modifier) => {
					interval = interval
						.checked_mul(modifier.interval_multiplier)
						.ok_or_else(|| ModelError::invalid_transition("attack cadence overflow"))?;
				}
				ActiveModifierKind::AttackSpeed(modifier) => {
					attack_speed_delta = attack_speed_delta
						.checked_add(i64::from(modifier.delta))
						.ok_or_else(|| {
							ModelError::invalid_transition("attack-speed delta overflow")
						})?;
				}
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::ArtsResistanceAddition(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::MovementSpeedReduction(_)
				| ActiveModifierKind::AttackRange(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::HitCount(_)
				| ActiveModifierKind::Blocking(_)
				| ActiveModifierKind::BlockTarget(_) => {}
			}
		}
		for status in runtime.statuses.values() {
			attack_speed_delta = attack_speed_delta
				.checked_add(i64::from(status.spec.attack_speed_delta))
				.ok_or_else(|| {
					ModelError::invalid_transition("status attack-speed delta overflow")
				})?;
		}
		let denominator: i64 = 100_i64
			.checked_add(attack_speed_delta)
			.ok_or_else(|| ModelError::invalid_transition("attack-speed denominator overflow"))?;
		if denominator <= 0 {
			return Err(ModelError::invalid_transition(
				"attack-speed denominator must remain positive",
			));
		}
		let speed_multiplier: ExpectedValue = ExpectedValue::from_ratio(
			100,
			u64::try_from(denominator).map_err(|_| {
				ModelError::invalid_transition("attack-speed denominator conversion failed")
			})?,
		)
		.ok_or_else(|| ModelError::invalid_transition("attack-speed division failed"))?;
		interval = interval
			.checked_mul(speed_multiplier)
			.ok_or_else(|| ModelError::invalid_transition("attack-speed cadence overflow"))?;
	}
	// A partial tick still has to elapse before the next attack, so the interval rounds up. This
	// is the same rule `deterministic_attack_interval_ticks` applies when it turns an attack time
	// and an attack speed into ticks, and the two must agree or a modifier that reproduces the
	// reviewed cadence exactly would still land a tick early.
	Ok(rounded_up_ticks(interval))
}

/// Whole ticks covering `interval`, rounding a partial tick up and never returning zero.
pub(super) fn rounded_up_ticks(interval: ExpectedValue) -> u64 {
	let partial: bool = !interval.scaled().is_multiple_of(ExpectedValue::SCALE);
	(interval.whole_floor() + u64::from(partial)).max(1)
}

fn effective_move_interval(
	state: &ExpectedState,
	enemy: EnemyInstanceId,
) -> Result<u64, ModelError> {
	let active = state
		.enemies
		.get(&enemy)
		.ok_or_else(|| ModelError::invalid_transition("movement owner disappeared"))?;
	let path: &BattlePath = find_path(&state.scenario.battle, active.spawn.path)
		.ok_or_else(|| ModelError::invalid_transition("movement path disappeared"))?;
	let step = path
		.steps
		.get(active.path_index)
		.ok_or_else(|| ModelError::invalid_transition("movement path ended without a goal"))?;
	let mut interval: ExpectedValue =
		ExpectedValue::from_scaled(step.duration_ticks.saturating_mul(ExpectedValue::SCALE));
	if step.affected_by_movement_speed {
		let total_reduction: ExpectedValue = active
			.runtime
			.modifiers
			.iter()
			.filter_map(|modifier| match modifier.kind {
				ActiveModifierKind::MovementSpeedReduction(reduction) => Some(reduction),
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::ArtsResistanceAddition(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::AttackCadence(_)
				| ActiveModifierKind::AttackSpeed(_)
				| ActiveModifierKind::AttackRange(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::HitCount(_)
				| ActiveModifierKind::Blocking(_)
				| ActiveModifierKind::BlockTarget(_) => None,
			})
			.try_fold(ExpectedValue::ZERO, ExpectedValue::checked_add)
			.ok_or_else(|| ModelError::invalid_transition("movement-speed reduction overflow"))?;
		if total_reduction >= ExpectedValue::ONE {
			return Err(ModelError::invalid_transition(
				"movement-speed reduction must remain below one",
			));
		}
		let speed: ExpectedValue = ExpectedValue::ONE.saturating_sub(total_reduction);
		interval = interval
			.checked_div(speed)
			.ok_or_else(|| ModelError::invalid_transition("movement-speed division overflow"))?;
		for status in active.runtime.statuses.values() {
			interval = interval
				.checked_mul(status.spec.movement_interval_multiplier)
				.ok_or_else(|| ModelError::invalid_transition("movement slow overflow"))?;
		}
	}
	Ok(interval.whole_floor())
}

fn effective_block_count(state: &ExpectedState, actor: BattleActorId) -> usize {
	if actor_has_status(state, actor, &StatusKind::Stun) {
		return 0;
	}
	let base: i32 = match actor {
		BattleActorId::Operator(operator) => state
			.deployed
			.get(&operator)
			.map_or(0, |deployed| i32::from(deployed.build.block_count)),
		BattleActorId::Summon(instance) => state
			.summons
			.get(&instance)
			.map_or(0, |summon| i32::from(summon.template.block_count)),
		BattleActorId::Enemy(_) | BattleActorId::Stage => 0,
	};
	let delta: i32 = actor_runtime(state, actor).map_or(0, |runtime| {
		runtime
			.modifiers
			.iter()
			.filter_map(|modifier| match modifier.kind {
				ActiveModifierKind::Blocking(modifier) => Some(i32::from(modifier.delta)),
				ActiveModifierKind::Stat(_)
				| ActiveModifierKind::ConditionalStat(_)
				| ActiveModifierKind::ArtsResistanceAddition(_)
				| ActiveModifierKind::NormalAttackSuppression
				| ActiveModifierKind::Shelter(_)
				| ActiveModifierKind::MovementSpeedReduction(_)
				| ActiveModifierKind::AttackCadence(_)
				| ActiveModifierKind::AttackSpeed(_)
				| ActiveModifierKind::AttackRange(_)
				| ActiveModifierKind::HitCount(_)
				| ActiveModifierKind::TargetCount(_)
				| ActiveModifierKind::BlockTarget(_) => None,
			})
			.sum()
	});
	usize::try_from(base.saturating_add(delta).max(0)).unwrap_or(usize::MAX)
}

fn effective_block_target_filter(
	state: &ExpectedState,
	actor: BattleActorId,
) -> Option<BlockTargetFilter> {
	let base: BlockTargetFilter = match actor {
		BattleActorId::Operator(operator) => {
			state.deployed.get(&operator)?.build.block_target_filter
		}
		BattleActorId::Summon(instance) => {
			state.summons.get(&instance)?.template.block_target_filter
		}
		BattleActorId::Enemy(_) | BattleActorId::Stage => return None,
	};
	Some(
		actor_runtime(state, actor)
			.and_then(|runtime| {
				runtime
					.modifiers
					.iter()
					.rev()
					.find_map(|modifier| match modifier.kind {
						ActiveModifierKind::BlockTarget(modifier) => Some(modifier.filter),
						ActiveModifierKind::Stat(_)
						| ActiveModifierKind::ConditionalStat(_)
						| ActiveModifierKind::ArtsResistanceAddition(_)
						| ActiveModifierKind::NormalAttackSuppression
						| ActiveModifierKind::Shelter(_)
						| ActiveModifierKind::MovementSpeedReduction(_)
						| ActiveModifierKind::AttackCadence(_)
						| ActiveModifierKind::AttackSpeed(_)
						| ActiveModifierKind::AttackRange(_)
						| ActiveModifierKind::TargetCount(_)
						| ActiveModifierKind::HitCount(_)
						| ActiveModifierKind::Blocking(_) => None,
					})
			})
			.unwrap_or(base),
	)
}

fn blocker_allows_enemy(
	state: &ExpectedState,
	blocker: BattleActorId,
	enemy: EnemyInstanceId,
) -> bool {
	let Some(target_class) = state
		.enemies
		.get(&enemy)
		.map(|enemy| enemy.facts.combat.target_class)
	else {
		return false;
	};
	effective_block_target_filter(state, blocker).is_some_and(|filter| filter.allows(target_class))
}

fn try_block_enemy(state: &mut ExpectedState, enemy: EnemyInstanceId, at: SimTick) -> bool {
	let Some(cell) = state.enemies.get(&enemy).map(|enemy| enemy.runtime.cell) else {
		return false;
	};
	let mut blockers: Vec<BattleActorId> = state
		.deployed
		.iter()
		.filter_map(|(operator, deployed)| {
			(deployed.runtime.cell == cell).then_some(BattleActorId::Operator(*operator))
		})
		.chain(state.summons.iter().filter_map(|(instance, summon)| {
			(summon.template.faction == Faction::Friendly && summon.runtime.cell == cell)
				.then_some(BattleActorId::Summon(*instance))
		}))
		.collect();
	blockers.sort();
	blockers
		.into_iter()
		.any(|blocker| try_block_with(state, enemy, blocker, at))
}

fn try_block_with(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	blocker: BattleActorId,
	at: SimTick,
) -> bool {
	// An enemy a Disappear step has hidden keeps its cell but is off the grid, so no blocker on
	// that cell may capture it until it reappears.
	if !state
		.enemies
		.get(&enemy)
		.is_some_and(|enemy| enemy.runtime.visible)
		|| !blocker_allows_enemy(state, blocker, enemy)
	{
		return false;
	}
	let capacity: usize = effective_block_count(state, blocker);
	let used: usize = blocked_enemies(state, blocker)
		.iter()
		.filter_map(|enemy| state.enemies.get(enemy))
		.map(|enemy| usize::from(enemy.facts.block_demand))
		.sum();
	let demand: usize = state
		.enemies
		.get(&enemy)
		.map_or(usize::MAX, |enemy| usize::from(enemy.facts.block_demand));
	if used.saturating_add(demand) > capacity {
		return false;
	}
	match blocker {
		BattleActorId::Operator(operator) => {
			state
				.deployed
				.get_mut(&operator)
				.expect("blocker operator exists")
				.blocking
				.insert(enemy);
		}
		BattleActorId::Summon(instance) => {
			state
				.summons
				.get_mut(&instance)
				.expect("blocker summon exists")
				.blocking
				.insert(enemy);
		}
		BattleActorId::Enemy(_) | BattleActorId::Stage => return false,
	}
	let active = state.enemies.get_mut(&enemy).expect("blocked enemy exists");
	active.blocked_by = Some(blocker);
	active.stealth_recovery_generation = active.stealth_recovery_generation.saturating_add(1);
	active.stealth_recovery_pending = false;
	state.block_audits.push(BlockAuditEntry {
		enemy,
		blocker,
		started_at: at,
		ended_at: None,
		end_reason: None,
	});
	true
}

fn block_waiting_enemies(
	state: &mut ExpectedState,
	blocker: BattleActorId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(cell) = actor_cell(state, blocker) else {
		return Ok(Vec::new());
	};
	let waiting: Vec<EnemyInstanceId> = state
		.enemies
		.iter()
		.filter_map(|(id, enemy)| {
			(enemy.runtime.cell == cell && enemy.blocked_by.is_none()).then_some(*id)
		})
		.collect();
	for enemy in waiting {
		try_block_with(state, enemy, blocker, at);
	}
	Ok(Vec::new())
}

fn reconcile_blocking(
	state: &mut ExpectedState,
	blocker: BattleActorId,
	at: SimTick,
	release_reason: BlockEndReason,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let capacity: usize = effective_block_count(state, blocker);
	let mut blocked: Vec<EnemyInstanceId> = blocked_enemies(state, blocker);
	blocked.sort();
	let mut used: usize = 0;
	let mut release: Vec<EnemyInstanceId> = Vec::new();
	for enemy in blocked {
		let demand: usize = state
			.enemies
			.get(&enemy)
			.map_or(0, |enemy| usize::from(enemy.facts.block_demand));
		if blocker_allows_enemy(state, blocker, enemy) && used.saturating_add(demand) <= capacity {
			used = used.saturating_add(demand);
		} else {
			release.push(enemy);
		}
	}
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for enemy in release {
		events.extend(end_block(state, enemy, at, release_reason)?);
		events.extend(reschedule_movement(state, enemy, at)?);
	}
	events.extend(block_waiting_enemies(state, blocker, at)?);
	Ok(events)
}

fn unblock_enemies(
	state: &mut ExpectedState,
	blocker: BattleActorId,
	at: SimTick,
	reason: BlockEndReason,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let blocked: Vec<EnemyInstanceId> = state
		.enemies
		.iter()
		.filter_map(|(enemy, active)| (active.blocked_by == Some(blocker)).then_some(*enemy))
		.collect();
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	for enemy in blocked {
		events.extend(end_block(state, enemy, at, reason)?);
		events.extend(reschedule_movement(state, enemy, at)?);
	}
	Ok(events)
}

fn blocked_enemies(state: &ExpectedState, blocker: BattleActorId) -> Vec<EnemyInstanceId> {
	match blocker {
		BattleActorId::Operator(operator) => state
			.deployed
			.get(&operator)
			.map_or_else(Vec::new, |deployed| {
				deployed.blocking.iter().copied().collect()
			}),
		BattleActorId::Summon(instance) => state
			.summons
			.get(&instance)
			.map_or_else(Vec::new, |summon| summon.blocking.iter().copied().collect()),
		BattleActorId::Enemy(_) | BattleActorId::Stage => Vec::new(),
	}
}

/// Evaluates a modifier's carrier condition against the live battle state.
fn actor_condition_holds(
	state: &ExpectedState,
	actor: BattleActorId,
	condition: ActorCondition,
) -> bool {
	match condition {
		ActorCondition::BlockedEnemiesAtLeast(minimum) => {
			blocked_enemies(state, actor).len() >= usize::from(minimum)
		}
	}
}

fn end_block(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
	reason: BlockEndReason,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(blocker) = state
		.enemies
		.get(&enemy)
		.and_then(|active| active.blocked_by)
	else {
		return Ok(Vec::new());
	};
	remove_enemy_from_blocker(state, blocker, enemy);
	let active = state.enemies.get_mut(&enemy).expect("blocked enemy exists");
	active.blocked_by = None;
	if matches!(
		&active.facts.attack.pattern,
		AttackPattern::FirstAttackAfterBlock { .. }
	) {
		active.first_blocked_attack_pending = true;
	}
	let recovery_ticks: Option<u64> = active
		.axis_direct_hit
		.as_ref()
		.and_then(|skill| skill.facts.stealth_recovery_ticks)
		.filter(|_| {
			!matches!(
				reason,
				BlockEndReason::EnemyDeath | BlockEndReason::Goal | BlockEndReason::BattleDeadline
			)
		});
	let recovery_generation: Option<u32> = recovery_ticks
		.map(|_| {
			active.stealth_recovery_generation = active
				.stealth_recovery_generation
				.checked_add(1)
				.ok_or_else(|| {
					ModelError::invalid_transition("stealth recovery generation overflow")
				})?;
			active.stealth_recovery_pending = true;
			Ok(active.stealth_recovery_generation)
		})
		.transpose()?;
	let audit = state
		.block_audits
		.iter_mut()
		.rev()
		.find(|audit| audit.enemy == enemy && audit.blocker == blocker && audit.ended_at.is_none())
		.ok_or_else(|| ModelError::invalid_transition("blocked enemy has no open block audit"))?;
	if at < audit.started_at {
		return Err(ModelError::invalid_transition(
			"block audit ended before it started",
		));
	}
	audit.ended_at = Some(at);
	audit.end_reason = Some(reason);
	let mut events: Vec<EventRequest<ExpectedEvent>> = Vec::new();
	if let Some((ticks, generation)) = recovery_ticks.zip(recovery_generation) {
		events.push(stealth_recovered_event(
			enemy,
			generation,
			checked_tick_add(at, ticks)?,
		));
	}
	Ok(events)
}

fn end_all_blocks(
	state: &mut ExpectedState,
	at: SimTick,
	reason: BlockEndReason,
) -> Result<(), ModelError> {
	let blocked: Vec<EnemyInstanceId> = state
		.enemies
		.iter()
		.filter_map(|(enemy, active)| active.blocked_by.is_some().then_some(*enemy))
		.collect();
	for enemy in blocked {
		let _ = end_block(state, enemy, at, reason)?;
	}
	Ok(())
}

fn remove_enemy_from_blocker(
	state: &mut ExpectedState,
	blocker: BattleActorId,
	enemy: EnemyInstanceId,
) {
	match blocker {
		BattleActorId::Operator(operator) => {
			if let Some(deployed) = state.deployed.get_mut(&operator) {
				deployed.blocking.remove(&enemy);
			}
		}
		BattleActorId::Summon(instance) => {
			if let Some(summon) = state.summons.get_mut(&instance) {
				summon.blocking.remove(&enemy);
			}
		}
		BattleActorId::Enemy(_) | BattleActorId::Stage => {}
	}
}

fn leak_enemy(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let actor = BattleActorId::Enemy(enemy);
	let before_cell: Option<GridPosition> = actor_cell(state, actor);
	let mut events: Vec<EventRequest<ExpectedEvent>> =
		actor_presence_events(state, actor, before_cell, None, at)?;
	let runtime_source: RuntimeSource = runtime_source(state, actor, None);
	events.extend(cleanup_runtime_effects(state, runtime_source, false, at)?);
	let former_blocker: Option<BattleActorId> =
		state.enemies.get(&enemy).and_then(|enemy| enemy.blocked_by);
	let _ = end_block(state, enemy, at, BlockEndReason::Goal)?;
	let active = state
		.enemies
		.remove(&enemy)
		.ok_or_else(|| ModelError::invalid_transition("leaking enemy disappeared"))?;
	state.leaked.insert(enemy);
	if let Some(report) = state.enemy_reports.get_mut(&enemy) {
		report.remaining_hp = active.runtime.hp;
		report.leaked_at = Some(at);
	}
	if let Some(blocker) = former_blocker {
		events.extend(block_waiting_enemies(state, blocker, at)?);
	}
	events.extend(record_wave_clearance(state, enemy, at)?);
	Ok(events)
}

fn reschedule_attack(
	state: &mut ExpectedState,
	actor: BattleActorId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(runtime) = actor_runtime_mut(state, actor) else {
		return Ok(Vec::new());
	};
	runtime.attack_generation = runtime.attack_generation.saturating_add(1);
	if actor_is_action_paused(state, actor) || normal_attacks_suppressed(state, actor) {
		return Ok(Vec::new());
	}
	let generation: u32 = actor_attack_generation(state, actor).unwrap_or(0);
	let next_at: SimTick = checked_tick_add(at, effective_attack_interval(state, actor)?)?;
	Ok(vec![normal_attack_event(actor, generation, next_at)])
}

fn schedule_next_attack(
	state: &ExpectedState,
	actor: BattleActorId,
	at: SimTick,
) -> Result<EventRequest<ExpectedEvent>, ModelError> {
	let generation: u32 = actor_attack_generation(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("attacker disappeared"))?;
	let next_at: SimTick = checked_tick_add(at, effective_attack_interval(state, actor)?)?;
	Ok(normal_attack_event(actor, generation, next_at))
}

fn reschedule_movement(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.enemies.get_mut(&enemy) else {
		return Ok(Vec::new());
	};
	active.move_generation = active.move_generation.saturating_add(1);
	if active.blocked_by.is_some() || actor_is_action_paused(state, BattleActorId::Enemy(enemy)) {
		return Ok(Vec::new());
	}
	Ok(vec![move_event(state, enemy, at)?])
}

fn reschedule_action_affected_movement(
	state: &mut ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	let Some(active) = state.enemies.get(&enemy) else {
		return Ok(Vec::new());
	};
	let path: &BattlePath = find_path(&state.scenario.battle, active.spawn.path)
		.ok_or_else(|| ModelError::invalid_transition("active enemy path disappeared"))?;
	if path
		.steps
		.get(active.path_index)
		.is_some_and(|step| !step.affected_by_movement_speed)
	{
		return Ok(Vec::new());
	}
	reschedule_movement(state, enemy, at)
}

fn move_event(
	state: &ExpectedState,
	enemy: EnemyInstanceId,
	at: SimTick,
) -> Result<EventRequest<ExpectedEvent>, ModelError> {
	let generation: u32 = state
		.enemies
		.get(&enemy)
		.ok_or_else(|| ModelError::invalid_transition("movement owner disappeared"))?
		.move_generation;
	Ok(EventRequest::new(
		checked_tick_add(at, effective_move_interval(state, enemy)?)?,
		MOVE_PRIORITY,
		enemy_entity(enemy),
		ExpectedEvent::Move { enemy, generation },
	))
}

fn repeat_stage_effect(
	state: &ExpectedState,
	source: BattleActorId,
	effect: &TaggedEffect,
	at: SimTick,
) -> Vec<EventRequest<ExpectedEvent>> {
	if source != BattleActorId::Stage {
		return Vec::new();
	}
	let Some(schedule) = state
		.scenario
		.battle
		.facts
		.stage_effects
		.iter()
		.find(|schedule| schedule.effect == *effect && schedule.at <= at)
	else {
		return Vec::new();
	};
	let Some(interval) = schedule.repeat_interval_ticks else {
		return Vec::new();
	};
	let Some(next_value) = at.get().checked_add(interval) else {
		return Vec::new();
	};
	let next_at: SimTick = SimTick::new(next_value);
	if schedule
		.repeat_until
		.is_some_and(|repeat_until| next_at > repeat_until)
	{
		return Vec::new();
	}
	vec![stage_effect_event(schedule, next_at)]
}

fn initial_assessments(battle: &CompiledArkBattle) -> Result<InitialAssessments, ModelError> {
	let mut enemies: BTreeMap<EnemyInstanceId, ExpectedEnemyAssessment> = BTreeMap::new();
	for wave in &battle.facts.waves {
		for spawn in &wave.spawns {
			let facts: &EnemyFacts = battle
				.facts
				.enemies
				.iter()
				.find(|facts| facts.enemy_key == spawn.enemy_key)
				.ok_or_else(|| ModelError::invalid_transition("enemy facts disappeared"))?;
			enemies.insert(
				spawn.id,
				ExpectedEnemyAssessment {
					id: spawn.id,
					enemy_key: spawn.enemy_key.clone(),
					spawned_at: None,
					deadline_at: None,
					max_hp: facts.combat.max_hp,
					remaining_hp: facts.combat.max_hp,
					expected_damage_taken: ExpectedValue::ZERO,
					death_at: None,
					leaked_at: None,
				},
			);
		}
	}
	let operators: BTreeMap<OperatorInstanceId, ExpectedOperatorAssessment> = battle
		.guide
		.builds
		.iter()
		.map(|build| {
			(
				build.id,
				ExpectedOperatorAssessment {
					id: build.id,
					operator_key: build.operator_key.clone(),
					max_hp: build.combat.max_hp,
					remaining_hp: build.combat.max_hp,
					deployed_at: None,
					death_at: None,
				},
			)
		})
		.collect();
	Ok((enemies, operators))
}

fn incomplete_outcome(battle: &CompiledArkBattle) -> ExpectedBattleOutcome {
	let (enemies, operators) =
		initial_assessments(battle).unwrap_or_else(|_| (BTreeMap::new(), BTreeMap::new()));
	ExpectedBattleOutcome {
		status: ExpectedTerminalStatus::SimulationIncomplete,
		enemies,
		operators,
		anchors: [(BattleTimeAnchor::BattleStarted, SimTick::new(0))]
			.into_iter()
			.collect(),
		dp: ExpectedDpAudit {
			initial: battle.facts.dp.initial,
			final_amount: battle.facts.dp.initial,
			entries: Vec::new(),
		},
		actions: battle
			.guide
			.actions
			.iter()
			.enumerate()
			.map(|(action_index, action)| GuideActionAudit {
				action_index,
				kind: guide_action_kind(action),
				requested_at: action.requested_at(),
				executed_at: None,
				status: ActionAuditStatus::SkippedNotReady,
			})
			.collect(),
		blocks: Vec::new(),
	}
}

fn outcome_from_state(
	state: &ExpectedState,
	status: ExpectedTerminalStatus,
) -> ExpectedBattleOutcome {
	ExpectedBattleOutcome {
		status,
		enemies: state.enemy_reports.clone(),
		operators: state.operator_reports.clone(),
		anchors: state.anchors.clone(),
		dp: ExpectedDpAudit {
			initial: state.scenario.battle.facts.dp.initial,
			final_amount: state.dp,
			entries: state.dp_entries.clone(),
		},
		actions: state.action_audits.clone(),
		blocks: state.block_audits.clone(),
	}
}

fn update_damage_report(state: &mut ExpectedState, target: BattleActorId, damage: ExpectedValue) {
	match target {
		BattleActorId::Enemy(enemy) => {
			if let Some(report) = state.enemy_reports.get_mut(&enemy) {
				report.remaining_hp = report.remaining_hp.saturating_sub(damage);
				report.expected_damage_taken = report
					.expected_damage_taken
					.checked_add(damage)
					.unwrap_or(ExpectedValue::from_scaled(u64::MAX));
			}
		}
		BattleActorId::Operator(operator) => {
			if let Some(report) = state.operator_reports.get_mut(&operator) {
				report.remaining_hp = report.remaining_hp.saturating_sub(damage);
			}
		}
		BattleActorId::Summon(_) | BattleActorId::Stage => {}
	}
}

fn update_hp_report(state: &mut ExpectedState, target: BattleActorId, hp: ExpectedValue) {
	match target {
		BattleActorId::Enemy(enemy) => {
			if let Some(report) = state.enemy_reports.get_mut(&enemy) {
				report.remaining_hp = hp;
			}
		}
		BattleActorId::Operator(operator) => {
			if let Some(report) = state.operator_reports.get_mut(&operator) {
				report.remaining_hp = hp;
			}
		}
		BattleActorId::Summon(_) | BattleActorId::Stage => {}
	}
}

fn update_max_hp_report(state: &mut ExpectedState, target: BattleActorId, max_hp: ExpectedValue) {
	match target {
		BattleActorId::Enemy(enemy) => {
			if let Some(report) = state.enemy_reports.get_mut(&enemy) {
				report.max_hp = max_hp;
			}
		}
		BattleActorId::Operator(operator) => {
			if let Some(report) = state.operator_reports.get_mut(&operator) {
				report.max_hp = max_hp;
			}
		}
		BattleActorId::Summon(_) | BattleActorId::Stage => {}
	}
}

fn mark_action_executed(state: &mut ExpectedState, action_index: usize, at: SimTick) {
	let audit: &mut GuideActionAudit = &mut state.action_audits[action_index];
	audit.executed_at = Some(at);
	audit.status = if audit.status == ActionAuditStatus::WaitedForDp {
		ActionAuditStatus::ExecutedAfterDpWait
	} else {
		ActionAuditStatus::Executed
	};
}

fn mark_executor_action(state: &mut ExpectedState, action_index: usize, at: SimTick) {
	let audit: &mut GuideActionAudit = &mut state.action_audits[action_index];
	audit.executed_at = Some(at);
	audit.status = ActionAuditStatus::ExecutorOnly;
	state.guide_ready = false;
}

fn advance_guide(
	state: &mut ExpectedState,
	at: SimTick,
) -> Result<Vec<EventRequest<ExpectedEvent>>, ModelError> {
	state.next_guide_action = state
		.next_guide_action
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("guide action index overflow"))?;
	let Some(action) = state
		.scenario
		.battle
		.guide
		.actions
		.get(state.next_guide_action)
	else {
		return Ok(Vec::new());
	};
	let ready_at: SimTick = if action.requested_at() < at {
		at
	} else {
		action.requested_at()
	};
	Ok(vec![guide_ready_event(
		state.next_guide_action,
		action,
		ready_at,
	)?])
}

fn validate_current_command(state: &ExpectedState, action_index: usize) -> Result<(), ModelError> {
	if !state.guide_ready || action_index != state.next_guide_action {
		return Err(ModelError::invalid_command(
			"guide command is not the next ready ordered action",
		));
	}
	Ok(())
}

fn guide_action_kind(action: &MaterializedGuideAction) -> GuideExecutorActionKind {
	match action {
		MaterializedGuideAction::Deploy { .. } => GuideExecutorActionKind::Deploy,
		MaterializedGuideAction::ActivateSkill { .. } => GuideExecutorActionKind::ActivateSkill,
		MaterializedGuideAction::Retreat { .. } => GuideExecutorActionKind::Retreat,
		MaterializedGuideAction::SpeedUp { .. } => GuideExecutorActionKind::SpeedUp,
		MaterializedGuideAction::SkillDaemon { .. } => GuideExecutorActionKind::SkillDaemon,
	}
}

fn guide_action_operator(action: &MaterializedGuideAction) -> Option<OperatorInstanceId> {
	match action {
		MaterializedGuideAction::Deploy { operator, .. }
		| MaterializedGuideAction::ActivateSkill { operator, .. }
		| MaterializedGuideAction::Retreat { operator, .. } => Some(*operator),
		MaterializedGuideAction::SpeedUp { .. } | MaterializedGuideAction::SkillDaemon { .. } => {
			None
		}
	}
}

fn guide_ready_event(
	action_index: usize,
	action: &MaterializedGuideAction,
	at: SimTick,
) -> Result<EventRequest<ExpectedEvent>, ModelError> {
	let entity_index: u64 = u64::try_from(action_index)
		.map_err(|_| ModelError::invalid_transition("guide action index exceeds u64"))?;
	let entity: EntityId = GUIDE_ENTITY_OFFSET
		.checked_add(entity_index)
		.map(EntityId::new)
		.ok_or_else(|| ModelError::invalid_transition("guide event entity overflow"))?;
	Ok(EventRequest::new(
		at,
		GUIDE_PRIORITY,
		entity,
		ExpectedEvent::GuideActionReady {
			action_index,
			kind: guide_action_kind(action),
		},
	))
}

fn stage_effect_event(effect: &ScheduledStageEffect, at: SimTick) -> EventRequest<ExpectedEvent> {
	resolve_effect_event(
		BattleActorId::Stage,
		None,
		Vec::new(),
		effect.effect.clone(),
		at,
	)
}

fn effect_events(
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	effects: &[TaggedEffect],
	current_targets: Vec<BattleActorId>,
	at: SimTick,
) -> Vec<EventRequest<ExpectedEvent>> {
	effect_events_with_priority(
		source,
		source_deployment_generation,
		effects,
		current_targets,
		EFFECT_PRIORITY,
		at,
	)
}

fn effect_events_with_priority(
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	effects: &[TaggedEffect],
	current_targets: Vec<BattleActorId>,
	priority: i32,
	at: SimTick,
) -> Vec<EventRequest<ExpectedEvent>> {
	effects
		.iter()
		.cloned()
		.map(|effect| {
			let targets: Vec<BattleActorId> = if effect.target == EffectTarget::CurrentTargets {
				current_targets.clone()
			} else {
				Vec::new()
			};
			resolve_effect_event_with_priority(
				source,
				source_deployment_generation,
				targets,
				effect,
				priority,
				at,
			)
		})
		.collect()
}

fn listener_effect_events(
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	effects: &[TaggedEffect],
	context: TriggerContext,
	priority: i32,
	at: SimTick,
) -> Vec<EventRequest<ExpectedEvent>> {
	effects
		.iter()
		.cloned()
		.map(|effect| {
			let targets: Vec<BattleActorId> = match effect.target {
				EffectTarget::CurrentTargets => vec![context.actor],
				EffectTarget::TriggerEffectSource => vec![context.effect_source],
				EffectTarget::SelfActor
				| EffectTarget::Blocker
				| EffectTarget::BlockedEnemies
				| EffectTarget::LowestHpAllyInRange
				| EffectTarget::AlliesInRange
				| EffectTarget::EnemiesInRange
				| EffectTarget::AllAllies
				| EffectTarget::AllEnemies => Vec::new(),
			};
			resolve_effect_event_with_priority(
				source,
				source_deployment_generation,
				targets,
				effect,
				priority,
				at,
			)
		})
		.collect()
}

fn resolve_effect_event(
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	targets: Vec<BattleActorId>,
	effect: TaggedEffect,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	resolve_effect_event_with_priority(
		source,
		source_deployment_generation,
		targets,
		effect,
		EFFECT_PRIORITY,
		at,
	)
}

fn resolve_effect_event_with_priority(
	source: BattleActorId,
	source_deployment_generation: Option<u32>,
	targets: Vec<BattleActorId>,
	effect: TaggedEffect,
	priority: i32,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		priority,
		actor_entity(source),
		ExpectedEvent::ResolveEffect {
			source,
			source_deployment_generation,
			targets,
			effect: Box::new(effect),
		},
	)
}

fn reconcile_blocking_event(
	actor: BattleActorId,
	release_reason: BlockEndReason,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		BLOCK_RECONCILE_PRIORITY,
		actor_entity(actor),
		ExpectedEvent::ReconcileBlocking {
			actor,
			release_reason,
		},
	)
}

fn trigger_event(
	kind: ReplayTriggerKind,
	actor: BattleActorId,
	effect_source: BattleActorId,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		TRIGGER_PRIORITY,
		actor_entity(actor),
		ExpectedEvent::FireTrigger {
			kind,
			actor,
			effect_source,
		},
	)
}

fn normal_attack_event(
	actor: BattleActorId,
	generation: u32,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		ATTACK_PRIORITY,
		actor_entity(actor),
		ExpectedEvent::NormalAttack { actor, generation },
	)
}

fn axis_direct_hit_event(
	enemy: EnemyInstanceId,
	generation: u32,
	phase: AxisDirectHitPhase,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	let priority: i32 = match phase {
		AxisDirectHitPhase::Ready => SKILL_PRIORITY,
		AxisDirectHitPhase::Projectile { .. } => AXIS_PROJECTILE_PRIORITY,
		AxisDirectHitPhase::End => AXIS_END_PRIORITY,
	};
	EventRequest::new(
		at,
		priority,
		enemy_entity(enemy),
		ExpectedEvent::AxisDirectHit {
			enemy,
			generation,
			phase,
		},
	)
}

fn stealth_recovered_event(
	enemy: EnemyInstanceId,
	generation: u32,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		EFFECT_PRIORITY,
		enemy_entity(enemy),
		ExpectedEvent::StealthRecovered { enemy, generation },
	)
}

fn death_event(
	state: &ExpectedState,
	actor: BattleActorId,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		DEATH_PRIORITY,
		actor_entity(actor),
		ExpectedEvent::Death {
			actor,
			operator_deployment_generation: operator_deployment_generation(state, actor),
		},
	)
}

fn enemy_death_effects_drained_event(
	enemy: EnemyInstanceId,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		TERMINAL_DRAIN_PRIORITY,
		enemy_entity(enemy),
		ExpectedEvent::EnemyDeathEffectsDrained { enemy },
	)
}

fn skill_activation_event(
	operator: OperatorInstanceId,
	mechanic: MechanicId,
	deployment_generation: u32,
	origin: SkillActivationOrigin,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		SKILL_PRIORITY,
		operator_entity(operator),
		ExpectedEvent::SkillActivation {
			operator,
			mechanic,
			deployment_generation,
			origin,
		},
	)
}

fn skill_discard_event(
	operator: OperatorInstanceId,
	deployment_generation: u32,
	charge_generation: u32,
	at: SimTick,
) -> EventRequest<ExpectedEvent> {
	EventRequest::new(
		at,
		SKILL_PRIORITY,
		operator_entity(operator),
		ExpectedEvent::SkillDiscarded {
			operator,
			deployment_generation,
			charge_generation,
		},
	)
}

fn actor_combat_facts(state: &ExpectedState, actor: BattleActorId) -> Option<ActorCombatFacts> {
	match actor {
		BattleActorId::Operator(operator) => state
			.deployed
			.get(&operator)
			.map(|actor| actor.build.combat),
		BattleActorId::Enemy(enemy) => state.enemies.get(&enemy).map(|actor| actor.facts.combat),
		BattleActorId::Summon(instance) => state
			.summons
			.get(&instance)
			.map(|actor| actor.template.combat),
		BattleActorId::Stage => None,
	}
}

fn actor_attack_profile(state: &ExpectedState, actor: BattleActorId) -> Option<&AttackProfile> {
	match actor {
		BattleActorId::Operator(operator) => state
			.deployed
			.get(&operator)
			.map(|actor| &actor.build.attack),
		BattleActorId::Enemy(enemy) => state.enemies.get(&enemy).map(|actor| &actor.facts.attack),
		BattleActorId::Summon(instance) => state
			.summons
			.get(&instance)
			.map(|actor| &actor.template.attack),
		BattleActorId::Stage => None,
	}
}

fn actor_runtime(state: &ExpectedState, actor: BattleActorId) -> Option<&RuntimeActor> {
	match actor {
		BattleActorId::Operator(operator) => {
			state.deployed.get(&operator).map(|actor| &actor.runtime)
		}
		BattleActorId::Enemy(enemy) => state.enemies.get(&enemy).map(|actor| &actor.runtime),
		BattleActorId::Summon(instance) => state.summons.get(&instance).map(|actor| &actor.runtime),
		BattleActorId::Stage => None,
	}
}

fn actor_runtime_mut(state: &mut ExpectedState, actor: BattleActorId) -> Option<&mut RuntimeActor> {
	match actor {
		BattleActorId::Operator(operator) => state
			.deployed
			.get_mut(&operator)
			.map(|actor| &mut actor.runtime),
		BattleActorId::Enemy(enemy) => state
			.enemies
			.get_mut(&enemy)
			.map(|actor| &mut actor.runtime),
		BattleActorId::Summon(instance) => state
			.summons
			.get_mut(&instance)
			.map(|actor| &mut actor.runtime),
		BattleActorId::Stage => None,
	}
}

fn actor_hp(state: &ExpectedState, actor: BattleActorId) -> Option<ExpectedValue> {
	actor_runtime(state, actor).map(|actor| actor.hp)
}

fn set_actor_hp(
	state: &mut ExpectedState,
	actor: BattleActorId,
	hp: ExpectedValue,
) -> Result<(), ModelError> {
	actor_runtime_mut(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("actor HP target disappeared"))?
		.hp = hp;
	Ok(())
}

fn actor_cell(state: &ExpectedState, actor: BattleActorId) -> Option<GridPosition> {
	actor_runtime(state, actor)
		.filter(|actor| actor.visible)
		.map(|actor| actor.cell)
}

fn actor_facing(state: &ExpectedState, actor: BattleActorId) -> Option<Facing> {
	actor_runtime(state, actor).map(|actor| actor.facing)
}

fn actor_attack_generation(state: &ExpectedState, actor: BattleActorId) -> Option<u32> {
	actor_runtime(state, actor).map(|actor| actor.attack_generation)
}

fn actor_exists(state: &ExpectedState, actor: BattleActorId) -> bool {
	match actor {
		BattleActorId::Operator(operator) => state.deployed.contains_key(&operator),
		BattleActorId::Enemy(enemy) => state.enemies.contains_key(&enemy),
		BattleActorId::Summon(instance) => state.summons.contains_key(&instance),
		BattleActorId::Stage => true,
	}
}

fn actor_is_frozen(state: &ExpectedState, actor: BattleActorId) -> bool {
	actor_runtime(state, actor).is_some_and(|runtime| {
		runtime.statuses.contains_key(&StatusKind::Frozen)
			|| runtime.statuses.contains_key(&StatusKind::Stun)
	})
}

fn actor_is_action_paused(state: &ExpectedState, actor: BattleActorId) -> bool {
	actor_is_frozen(state, actor)
		|| matches!(
			actor,
			BattleActorId::Enemy(enemy)
				if axis_direct_hit_is_active(state, enemy)
		)
}

fn normal_attacks_suppressed(state: &ExpectedState, actor: BattleActorId) -> bool {
	actor_runtime(state, actor).is_some_and(|runtime| {
		runtime
			.modifiers
			.iter()
			.any(|modifier| matches!(&modifier.kind, ActiveModifierKind::NormalAttackSuppression))
	})
}

fn axis_direct_hit_is_active(state: &ExpectedState, enemy: EnemyInstanceId) -> bool {
	state
		.enemies
		.get(&enemy)
		.and_then(|active| active.axis_direct_hit.as_ref())
		.is_some_and(|skill| skill.active_until.is_some())
}

fn take_first_blocked_attack(state: &mut ExpectedState, actor: BattleActorId) -> bool {
	let BattleActorId::Enemy(enemy) = actor else {
		return false;
	};
	let Some(active) = state.enemies.get_mut(&enemy) else {
		return false;
	};
	if !active.first_blocked_attack_pending {
		return false;
	}
	active.first_blocked_attack_pending = false;
	true
}

fn target_is_shelter(state: &ExpectedState, target: BattleActorId) -> bool {
	let BattleActorId::Summon(instance) = target else {
		return false;
	};
	state
		.summons
		.get(&instance)
		.is_some_and(|summon| summon.template.targetability == Targetability::Shelter)
}

fn actor_has_status(state: &ExpectedState, actor: BattleActorId, status: &StatusKind) -> bool {
	actor_runtime(state, actor).is_some_and(|runtime| runtime.statuses.contains_key(status))
}

fn increment_attack_count(
	state: &mut ExpectedState,
	actor: BattleActorId,
) -> Result<u32, ModelError> {
	let runtime = actor_runtime_mut(state, actor)
		.ok_or_else(|| ModelError::invalid_transition("attack counter owner disappeared"))?;
	runtime.attack_count = runtime
		.attack_count
		.checked_add(1)
		.ok_or_else(|| ModelError::invalid_transition("attack count overflow"))?;
	Ok(runtime.attack_count)
}

fn actor_faction(state: &ExpectedState, actor: BattleActorId) -> Option<Faction> {
	match actor {
		BattleActorId::Operator(_) => Some(Faction::Friendly),
		BattleActorId::Enemy(_) => Some(Faction::Hostile),
		BattleActorId::Summon(instance) => state
			.summons
			.get(&instance)
			.map(|summon| summon.template.faction),
		BattleActorId::Stage => None,
	}
}

fn actor_targetable_by(
	state: &ExpectedState,
	source: BattleActorId,
	target: BattleActorId,
) -> bool {
	if actor_faction(state, source) != actor_faction(state, target)
		&& matches!(
			target,
			BattleActorId::Enemy(enemy) if enemy_stealth_is_active(state, enemy)
		) {
		return false;
	}
	let BattleActorId::Summon(instance) = target else {
		return true;
	};
	match state
		.summons
		.get(&instance)
		.map_or(Targetability::None, |summon| summon.template.targetability)
	{
		Targetability::All => true,
		Targetability::None => false,
		Targetability::BlockedOnly | Targetability::Shelter => match source {
			BattleActorId::Enemy(enemy) => state
				.enemies
				.get(&enemy)
				.is_some_and(|enemy| enemy.blocked_by == Some(target)),
			BattleActorId::Operator(_) | BattleActorId::Summon(_) | BattleActorId::Stage => false,
		},
	}
}

fn enemy_stealth_is_active(state: &ExpectedState, enemy: EnemyInstanceId) -> bool {
	let Some(active) = state.enemies.get(&enemy) else {
		return false;
	};
	active.blocked_by.is_none()
		&& !active.stealth_recovery_pending
		&& !axis_direct_hit_is_active(state, enemy)
		&& active
			.runtime
			.statuses
			.contains_key(&StatusKind::StealthWhileUnblocked)
}

fn allied_actors(state: &ExpectedState, source: BattleActorId) -> Vec<BattleActorId> {
	let faction: Faction = actor_faction(state, source).unwrap_or(Faction::Friendly);
	all_actors(state)
		.into_iter()
		.filter(|actor| actor_faction(state, *actor) == Some(faction))
		.collect()
}

fn opposing_actors(state: &ExpectedState, source: BattleActorId) -> Vec<BattleActorId> {
	let faction: Faction = actor_faction(state, source).unwrap_or(Faction::Friendly);
	all_actors(state)
		.into_iter()
		.filter(|actor| {
			actor_faction(state, *actor).is_some_and(|actor_faction| actor_faction != faction)
		})
		.collect()
}

fn all_actors(state: &ExpectedState) -> Vec<BattleActorId> {
	state
		.deployed
		.keys()
		.copied()
		.map(BattleActorId::Operator)
		.chain(state.enemies.keys().copied().map(BattleActorId::Enemy))
		.chain(state.summons.keys().copied().map(BattleActorId::Summon))
		.collect()
}

fn target_priority(state: &ExpectedState, target: BattleActorId) -> (u8, u32, BattleActorId) {
	match target {
		BattleActorId::Enemy(enemy) => {
			let progress: usize = state
				.enemies
				.get(&enemy)
				.map_or(0, |enemy| enemy.path_index);
			let reverse_progress: u32 =
				u32::MAX.saturating_sub(u32::try_from(progress).unwrap_or(u32::MAX));
			(0, reverse_progress, target)
		}
		BattleActorId::Operator(operator) => (1, operator.get(), target),
		BattleActorId::Summon(instance) => (2, instance, target),
		BattleActorId::Stage => (3, 0, target),
	}
}

fn friendly_cell_occupied(state: &ExpectedState, cell: GridPosition) -> bool {
	state
		.deployed
		.values()
		.any(|deployed| deployed.runtime.cell == cell)
		|| state.summons.values().any(|summon| {
			summon.template.faction == Faction::Friendly
				&& summon.template.cell_occupancy == SummonCellOccupancy::Exclusive
				&& summon.runtime.cell == cell
		})
}

fn operator_build(
	state: &ExpectedState,
	operator: OperatorInstanceId,
) -> Option<&OperatorBuildFacts> {
	state
		.scenario
		.battle
		.guide
		.builds
		.iter()
		.find(|build| build.id == operator)
}

fn find_spawn(battle: &CompiledArkBattle, enemy: EnemyInstanceId) -> Option<&EnemySpawnFacts> {
	battle
		.facts
		.waves
		.iter()
		.flat_map(|wave| wave.spawns.iter())
		.find(|spawn| spawn.id == enemy)
}

fn find_path(battle: &CompiledArkBattle, path: PathId) -> Option<&BattlePath> {
	battle
		.facts
		.paths
		.iter()
		.find(|candidate| candidate.id == path)
}

fn cell_in_bounds(cell: GridPosition, dimensions: MapDimensions) -> bool {
	cell.row < dimensions.height && cell.col < dimensions.width
}

fn checked_tick_add(at: SimTick, delta: u64) -> Result<SimTick, ModelError> {
	at.get()
		.checked_add(delta)
		.map(SimTick::new)
		.ok_or_else(|| ModelError::invalid_transition("event tick overflow"))
}

fn actor_entity(actor: BattleActorId) -> EntityId {
	match actor {
		BattleActorId::Operator(operator) => operator_entity(operator),
		BattleActorId::Enemy(enemy) => enemy_entity(enemy),
		BattleActorId::Summon(instance) => summon_entity(instance),
		BattleActorId::Stage => STAGE_ENTITY,
	}
}

const fn operator_entity(operator: OperatorInstanceId) -> EntityId {
	EntityId::new(OPERATOR_ENTITY_OFFSET + operator.get() as u64)
}

const fn enemy_entity(enemy: EnemyInstanceId) -> EntityId {
	EntityId::new(ENEMY_ENTITY_OFFSET + enemy.get() as u64)
}

const fn summon_entity(instance: u32) -> EntityId {
	EntityId::new(SUMMON_ENTITY_OFFSET + instance as u64)
}

const fn wave_entity(wave: WaveId) -> EntityId {
	EntityId::new(GUIDE_ENTITY_OFFSET + 1_000_000 + wave.get() as u64)
}
