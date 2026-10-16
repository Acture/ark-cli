use std::collections::{BTreeMap, BTreeSet};

use crate::data::{GridPosition, MapDimensions, RangeOffset};
use crate::simulation::SimTick;

use crate::mechanics::{
	battle_grid_metadata::BattleGridCellMetadata,
	geometry::Facing,
	normal_attack::{NormalAttackTargetFilter, TargetClass},
	tile_layer_propagation::{ContributionMergePolicy, PulseSourceOrder},
};

/// Millionth-scaled, non-negative value used by the deterministic expected-value lane.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedValue(u64);

impl ExpectedValue {
	pub const SCALE: u64 = 1_000_000;
	pub const ZERO: Self = Self(0);
	pub const ONE: Self = Self(Self::SCALE);

	#[must_use]
	pub const fn from_scaled(scaled: u64) -> Self {
		Self(scaled)
	}

	#[must_use]
	pub const fn from_whole(value: u32) -> Self {
		Self(value as u64 * Self::SCALE)
	}

	#[must_use]
	pub fn from_ratio(numerator: u64, denominator: u64) -> Option<Self> {
		if denominator == 0 {
			return None;
		}
		let scaled: u128 = u128::from(numerator)
			.checked_mul(u128::from(Self::SCALE))?
			.checked_div(u128::from(denominator))?;
		u64::try_from(scaled).ok().map(Self)
	}

	#[must_use]
	pub const fn scaled(self) -> u64 {
		self.0
	}

	#[must_use]
	pub const fn whole_floor(self) -> u64 {
		self.0 / Self::SCALE
	}

	pub(crate) fn checked_add(self, other: Self) -> Option<Self> {
		self.0.checked_add(other.0).map(Self)
	}

	pub(crate) const fn saturating_sub(self, other: Self) -> Self {
		Self(self.0.saturating_sub(other.0))
	}

	pub(crate) fn checked_mul(self, multiplier: Self) -> Option<Self> {
		let product: u128 = u128::from(self.0).checked_mul(u128::from(multiplier.0))?;
		let scaled: u128 = product.checked_div(u128::from(Self::SCALE))?;
		u64::try_from(scaled).ok().map(Self)
	}

	pub(crate) fn checked_div(self, divisor: Self) -> Option<Self> {
		let quotient: u128 = u128::from(self.0)
			.checked_mul(u128::from(Self::SCALE))?
			.checked_div(u128::from(divisor.0))?;
		u64::try_from(quotient).ok().map(Self)
	}

	pub(crate) const fn min(self, other: Self) -> Self {
		if self.0 <= other.0 {
			self
		} else {
			other
		}
	}
}

/// Stable identity of a deployed operator within one materialized strategy.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperatorInstanceId(u32);

impl OperatorInstanceId {
	#[must_use]
	pub const fn new(value: u32) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}
}

/// Stable identity of one concrete enemy spawn.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemyInstanceId(u32);

impl EnemyInstanceId {
	#[must_use]
	pub const fn new(value: u32) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PathId(u32);

impl PathId {
	#[must_use]
	pub const fn new(value: u32) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WaveId(u32);

impl WaveId {
	#[must_use]
	pub const fn new(value: u32) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GoalId(u32);

impl GoalId {
	#[must_use]
	pub const fn new(value: u32) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SummonTemplateId(u32);

impl SummonTemplateId {
	#[must_use]
	pub const fn new(value: u32) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u32 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BattleActorId {
	Operator(OperatorInstanceId),
	Enemy(EnemyInstanceId),
	Summon(u32),
	Stage,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicId(String);

impl MechanicId {
	#[must_use]
	pub fn new(value: impl Into<String>) -> Self {
		Self(value.into())
	}

	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

/// Behavior family supplied by a reviewed snapshot adapter.
///
/// The executor never infers behavior from the mechanic ID. An active effect may be
/// `UnmodeledOmission` when it is deliberately left out of an otherwise executable partial replay;
/// that state prevents both victory and defeat certification. `DeclaredApproximation` marks an
/// effect replay does execute, in a declared simplified form, and prevents both certifications once
/// the battle exercises it. `Unsupported` is reserved for invalid or drifted evidence that prevents
/// safe compilation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MechanicBehavior {
	/// The reviewed mechanic exists, but battle facts prove its activation condition is false.
	ProvenInactive,
	Damage,
	Healing,
	LethalRecoveryAndStatus,
	LayeredTileDamageAndSlow,
	StatModifier,
	AttackCadence,
	AttackRange,
	TargetCount,
	SpAndSkillActivation,
	DpGain,
	Status,
	MovementSlowOrShift,
	Blocking,
	SpawnOrSummon,
	Death,
	CrossAttack,
	PowerHit,
	SnowColdFreeze,
	ConditionalAura,
	Gravity,
	ShelterExposureOrDirectHit,
	/// A reviewed strategy-beneficial effect omitted to produce an explicitly conservative replay.
	///
	/// It still permits victory certification, but withholds defeat certification because the
	/// omitted benefit might have prevented the leak.
	ConservativeBeneficialOmission {
		reason: String,
	},
	/// A reached effect intentionally omitted from simulation without a conservative guarantee, so
	/// it withholds both victory and defeat certification.
	UnmodeledOmission {
		reason: String,
	},
	/// A reached behavior replay executes in a declared simplified form rather than omitting.
	///
	/// The simplification moves when or where an executed effect lands rather than removing it, and
	/// a battle outcome is not monotone in either, so an exercised approximation withholds both
	/// victory and defeat certification. The reason states what review established, including any
	/// direction it suspects; a suspected direction is evidence for a later proof, never a
	/// certification lever. `exercised_by` names the observation that decides whether this battle
	/// depended on the simplification at all.
	DeclaredApproximation {
		reason: String,
		exercised_by: ApproximationTrigger,
	},
	Unsupported {
		reason: String,
	},
}

/// What a replay must observe before one [`MechanicBehavior::DeclaredApproximation`] can have moved
/// its outcome.
///
/// A battle that never observes the trigger never executed the approximated value, so the
/// approximation withholds nothing from that battle's verdict. Anything the trigger cannot rule out
/// counts as exercised.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ApproximationTrigger {
	/// An enemy of this record fires a normal attack.
	EnemyNormalAttack { enemy_key: String },
	/// This operator fires a normal attack.
	OperatorNormalAttack { operator_id: String },
	/// An enemy spawns onto this path.
	PathSpawn { path: PathId },
}

/// One mechanic reached from the materialized build's active prefab roots.
///
/// A conditional branch may still be [`MechanicBehavior::ProvenInactive`] when battle facts prove
/// its activation predicate cannot become true.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReachedMechanic {
	pub id: MechanicId,
	pub origin: String,
	pub behavior: MechanicBehavior,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnsupportedMechanicEntry {
	pub id: MechanicId,
	pub origins: Vec<String>,
	pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BattleCompileIssue {
	EmptyBattleId,
	EmptyMechanicId {
		origin: String,
	},
	InvalidDpRules {
		detail: String,
	},
	InvalidDeploymentRules {
		detail: String,
	},
	DuplicatePath(PathId),
	InvalidPath {
		path: PathId,
		detail: String,
	},
	DuplicateWave(WaveId),
	InvalidWave {
		wave: WaveId,
		detail: String,
	},
	DuplicateEnemyDefinition {
		enemy_key: String,
	},
	UnsupportedEnemyOnDeath {
		enemy_key: String,
	},
	DuplicateEnemySpawn(EnemyInstanceId),
	UnknownEnemyDefinition {
		enemy: EnemyInstanceId,
		enemy_key: String,
	},
	UnknownPath {
		enemy: EnemyInstanceId,
		path: PathId,
	},
	DuplicateOperator(OperatorInstanceId),
	UnknownGuideOperator {
		action_index: usize,
		operator: OperatorInstanceId,
	},
	InvalidGuideOrder {
		action_index: usize,
	},
	InvalidGuideCell {
		action_index: usize,
		cell: GridPosition,
	},
	InvalidCombatFacts {
		detail: String,
	},
	ExpectedDiscreteEffect {
		mechanic: MechanicId,
	},
	ArithmeticOverflow {
		detail: String,
	},
	DuplicatePreplacedActor {
		instance: u32,
	},
	UnknownSummonTemplate {
		instance: u32,
		template: SummonTemplateId,
	},
	UnsupportedHostileSummon {
		template: SummonTemplateId,
		key: String,
	},
	UnsupportedHostilePreplacedActor {
		instance: u32,
		template: SummonTemplateId,
	},
	InvalidReplayPrimitive {
		mechanic: MechanicId,
		detail: String,
	},
	ReplayTriggerCycle {
		triggers: Vec<ReplayTriggerKind>,
	},
}

/// Full fail-closed compile report. Unsupported IDs are deduplicated and sorted.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicSupportReport {
	unsupported: Vec<UnsupportedMechanicEntry>,
	issues: Vec<BattleCompileIssue>,
}

impl MechanicSupportReport {
	#[must_use]
	pub fn is_complete(&self) -> bool {
		self.unsupported.is_empty() && self.issues.is_empty()
	}

	#[must_use]
	pub fn unsupported(&self) -> &[UnsupportedMechanicEntry] {
		&self.unsupported
	}

	pub fn unsupported_ids(&self) -> impl ExactSizeIterator<Item = &MechanicId> {
		self.unsupported.iter().map(|entry| &entry.id)
	}

	#[must_use]
	pub fn issues(&self) -> &[BattleCompileIssue] {
		&self.issues
	}

	pub(crate) fn from_parts(
		unsupported: Vec<UnsupportedMechanicEntry>,
		issues: Vec<BattleCompileIssue>,
	) -> Self {
		Self {
			unsupported,
			issues,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedDpRules {
	pub initial: u32,
	pub maximum: u32,
	pub regen_amount: u32,
	pub regen_interval_ticks: u64,
}

/// Battle-wide limits and standard DP accounting for operator deployment episodes.
///
/// The first deployment uses the build's base cost. The second deployment uses
/// `second_deployment_cost_multiplier`, and every subsequent deployment uses
/// `later_deployment_cost_multiplier`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedDeploymentRules {
	/// Hard upper bound on simultaneously deployed guide operators. Zero prohibits deployment.
	pub simultaneous_limit: u32,
	/// Multiplier applied to the build's base cost for its second deployment episode.
	pub second_deployment_cost_multiplier: ExpectedValue,
	/// Multiplier applied to the build's base cost for its third and later deployment episodes.
	pub later_deployment_cost_multiplier: ExpectedValue,
	/// Fraction of the current episode's paid deployment cost returned by a manual retreat.
	pub retreat_refund_ratio: ExpectedValue,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BattleGrid {
	pub dimensions: MapDimensions,
	pub cells: BTreeMap<GridPosition, BattleGridCellMetadata>,
	pub deployable_cells: BTreeSet<GridPosition>,
	/// Lowland cells that can contain ground actors and ground-only tile effects.
	pub ground_cells: BTreeSet<GridPosition>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BattlePath {
	pub id: PathId,
	pub goal: GoalId,
	pub start: GridPosition,
	/// Exact causal route steps. A supported path must end in an explicit `Goal`.
	pub steps: Vec<BattlePathStep>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BattlePathStep {
	/// Time from the preceding step to this step.
	pub duration_ticks: u64,
	/// Whether slow and movement-cadence effects multiply `duration_ticks`.
	pub affected_by_movement_speed: bool,
	pub action: BattlePathStepAction,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BattlePathStepAction {
	Move { cell: GridPosition, facing: Facing },
	Wait,
	Disappear,
	Reappear { cell: GridPosition, facing: Facing },
	Goal { goal: GoalId },
}

/// Causal anchor recorded by the authoritative event model.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BattleTimeAnchor {
	BattleStarted,
	WaveStarted(WaveId),
	WaveCleared(WaveId),
	EnemySpawned(EnemyInstanceId),
	EnemyDefeated(EnemyInstanceId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelativeBattleTime {
	pub anchor: BattleTimeAnchor,
	pub offset_ticks: u64,
}

/// A wave starts at the first resolved bound. `earliest` usually represents clearance and
/// `latest` the game's maximum-wait fallback; both retain their own causal anchors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WaveStartWindow {
	pub earliest: RelativeBattleTime,
	pub latest: Option<RelativeBattleTime>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemySpawnFacts {
	pub id: EnemyInstanceId,
	pub enemy_key: String,
	pub after_wave_start_ticks: u64,
	pub path: PathId,
	pub facing: Facing,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BattleWave {
	pub id: WaveId,
	pub start: WaveStartWindow,
	pub spawns: Vec<EnemySpawnFacts>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DamageType {
	Physical,
	Arts,
	True,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DamageDelivery {
	Normal,
	DirectHit {
		ignore_defense: bool,
		ignore_shelter: bool,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackAction {
	Damage {
		damage_type: DamageType,
		delivery: DamageDelivery,
	},
	Heal,
	None,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttackPattern {
	Single,
	CrossAttack {
		arm_length: u16,
	},
	/// The actor starts armed and becomes armed again whenever its current block session ends.
	FirstAttackAfterBlock {
		/// Multiplier consumed by the next attack while armed.
		damage_multiplier: ExpectedValue,
		/// Multiplier applied to every attack whose target is a shelter.
		shelter_damage_multiplier: ExpectedValue,
	},
	PowerHit {
		every: u32,
		damage_multiplier: ExpectedValue,
		shift: Option<ShiftEffect>,
	},
	/// One plain single-target hit carried by a projectile that lands `flight_ticks` after firing.
	///
	/// Attack is sampled when the projectile fires. The hit is void if its target is gone when it
	/// lands, and still lands after the attacker has died.
	Projectile {
		flight_ticks: u64,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttackProfile {
	pub first_delay_ticks: u64,
	pub interval_ticks: u64,
	/// Right-facing source offsets, including the anchor when appropriate.
	pub range: Vec<RangeOffset>,
	pub target_count: u16,
	pub target_filter: NormalAttackTargetFilter,
	pub action: NormalAttackAction,
	pub pattern: AttackPattern,
	pub extra_effects: Vec<TaggedEffect>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActorCombatFacts {
	pub max_hp: ExpectedValue,
	pub attack: ExpectedValue,
	pub defense: ExpectedValue,
	pub target_class: TargetClass,
	/// Fraction in `[0, 1]`.
	pub arts_resistance: ExpectedValue,
	/// Fraction of the raw hit guaranteed by the damage floor, for both mitigated families.
	pub minimum_damage_ratio: ExpectedValue,
}

impl ActorCombatFacts {
	/// Both mitigated damage families floor at five percent of the raw hit, so a target whose
	/// defense or resistance exceeds the attack still takes a sliver rather than nothing:
	/// physical resolves as `max(0.05A, A - [(1 - I_p) * max(0, DEF - I_v)])` and Arts as
	/// `max(0.05A, 0.01A * max[0, 100 - (1 - I_p) * max(0, RES - I_v)])`. Reviewed from the PRTS
	/// 游戏数据基础 page; no pinned table carries the fraction, so it is stated here once rather
	/// than read from gamedata. Only the flat reduction lane `I_v` is modelled; a proportional
	/// defense- or resistance-ignore `I_p` has no representation yet, and a mechanic that declares
	/// one must stay an omission rather than silently resolve as `I_p = 0`.
	pub const REVIEWED_MINIMUM_DAMAGE_RATIO: ExpectedValue =
		ExpectedValue::from_scaled(ExpectedValue::SCALE / 20);
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillActivationRule {
	Manual,
	Auto,
	Passive,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SpChargeRule {
	None,
	Time { amount: u32, interval_ticks: u64 },
	Attack { amount: u32 },
	Hit { amount: u32 },
}

/// One independently scheduled, fixed-point natural SP-recovery source.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PeriodicSpSource {
	pub amount: ExpectedValue,
	pub interval_ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StoredAttackTargetRelationship {
	Ally,
	Enemy,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StoredAttackHpRatioComparison {
	LessThan,
	LessThanOrEqual,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StoredAttackTargetOrder {
	LowestHpRatio,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StoredAttackReplacementDelivery {
	SourceStatScaledHealing {
		stat: StatKind,
		multiplier: ExpectedValue,
	},
}

/// What happens to SP already spent on a reservation that cannot be delivered.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StoredAttackReservationInvalidationAction {
	RefundSpentSp,
	ConsumeSpentSp,
	/// Evidence did not establish this transition; compilation must reject the facts.
	Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StoredAttackReservationInvalidationPolicy {
	pub target_death: StoredAttackReservationInvalidationAction,
	pub target_disappeared: StoredAttackReservationInvalidationAction,
	pub target_out_of_range: StoredAttackReservationInvalidationAction,
	pub target_generation_changed: StoredAttackReservationInvalidationAction,
}

/// A normal-attack replacement that spends stored SP only after reserving one stable target.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StoredAttackReplacementFacts {
	/// Right-facing offsets projected through the operator's current facing.
	pub range: Vec<RangeOffset>,
	pub relationship: StoredAttackTargetRelationship,
	pub require_healable: bool,
	pub target_filter: NormalAttackTargetFilter,
	pub include_source: bool,
	pub hp_ratio_comparison: StoredAttackHpRatioComparison,
	pub hp_ratio_threshold: ExpectedValue,
	pub target_order: StoredAttackTargetOrder,
	/// Maximum stored uses represented by `skill.sp_cost * max_charges` SP.
	pub max_charges: u16,
	pub delivery_delay_ticks: u64,
	pub delivery: StoredAttackReplacementDelivery,
	pub invalidation: StoredAttackReservationInvalidationPolicy,
}

/// Which active skill uses may be terminated by pressing the skill button again.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillManualDiscardActivationLimit {
	Unlimited,
	FirstNActivations(u32),
}

impl SkillManualDiscardActivationLimit {
	#[must_use]
	pub const fn allows(self, activation_count: u32) -> bool {
		match self {
			Self::Unlimited => true,
			Self::FirstNActivations(limit) => activation_count <= limit,
		}
	}
}

/// Controller-derived policy for manually terminating an active skill.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SkillManualDiscardPolicy {
	pub activation_limit: SkillManualDiscardActivationLimit,
	pub allow_while_dozing: bool,
	/// Effects executed only by a manual discard, before ordinary skill-end cleanup.
	pub on_discard: Vec<TaggedEffect>,
}

impl SkillManualDiscardPolicy {
	#[must_use]
	pub const fn allows(&self, activation_count: u32, is_dozing: bool) -> bool {
		self.activation_limit.allows(activation_count) && (self.allow_while_dozing || !is_dozing)
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SkillFacts {
	pub id: MechanicId,
	pub activation: SkillActivationRule,
	pub initial_sp: u32,
	pub sp_cost: u32,
	pub charge: SpChargeRule,
	/// Additional natural-recovery sources, used only with [`SpChargeRule::Time`].
	pub periodic_sp_sources: Vec<PeriodicSpSource>,
	/// `Some(0)` is instantaneous; `None` remains active indefinitely.
	pub active_duration_ticks: Option<u64>,
	pub manual_discard: Option<SkillManualDiscardPolicy>,
	pub stored_attack_replacement: Option<StoredAttackReplacementFacts>,
	pub effects: Vec<TaggedEffect>,
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperatorClassificationFacts {
	/// One profession-category bit, or zero when the operator is unclassified.
	pub profession_category_mask: u16,
	/// Optional fixed-roster group membership supplied by official operator metadata.
	pub group_id: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperatorBuildFacts {
	pub id: OperatorInstanceId,
	pub operator_key: String,
	pub classification: OperatorClassificationFacts,
	/// Persistent effects installed from the immutable roster at battle start.
	pub on_battle_start: Vec<TaggedEffect>,
	pub deployment_cost: u32,
	pub redeployment_cooldown_ticks: u64,
	pub block_count: u16,
	pub block_target_filter: BlockTargetFilter,
	pub combat: ActorCombatFacts,
	pub attack: AttackProfile,
	pub skill: Option<SkillFacts>,
	pub on_deploy: Vec<TaggedEffect>,
	pub on_death: Vec<TaggedEffect>,
	pub reached_mechanics: Vec<ReachedMechanic>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemyFacts {
	pub enemy_key: String,
	pub block_demand: u16,
	pub mass: i16,
	pub combat: ActorCombatFacts,
	pub attack: AttackProfile,
	pub on_spawn: Vec<TaggedEffect>,
	pub on_death: Vec<TaggedEffect>,
	pub reached_mechanics: Vec<ReachedMechanic>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Faction {
	Friendly,
	Hostile,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Targetability {
	All,
	BlockedOnly,
	/// Blocks axis projectiles even when ordinary attacks cannot select it.
	Shelter,
	None,
}

/// Whether a friendly summon reserves its cell against ordinary operator deployment.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SummonCellOccupancy {
	Exclusive,
	NonExclusive,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SummonTemplate {
	pub id: SummonTemplateId,
	pub key: String,
	pub faction: Faction,
	pub cell_occupancy: SummonCellOccupancy,
	pub block_count: u16,
	pub block_target_filter: BlockTargetFilter,
	pub targetability: Targetability,
	pub combat: ActorCombatFacts,
	pub attack: AttackProfile,
	pub lifetime_ticks: Option<u64>,
	pub reached_mechanics: Vec<ReachedMechanic>,
}

/// One stage-owned actor present before guide execution, such as BD-7 shelter traps.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PreplacedActorFacts {
	pub instance: u32,
	pub template: SummonTemplateId,
	pub cell: GridPosition,
	pub facing: Facing,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArkBattleFacts {
	pub battle_id: String,
	/// Optional caller-supplied hard cap. Stage-derived replay normally runs to a causal terminal.
	pub end_at: Option<SimTick>,
	pub dp: ExpectedDpRules,
	pub deployment: ExpectedDeploymentRules,
	pub grid: BattleGrid,
	pub paths: Vec<BattlePath>,
	pub waves: Vec<BattleWave>,
	pub enemies: Vec<EnemyFacts>,
	pub summons: Vec<SummonTemplate>,
	pub preplaced_actors: Vec<PreplacedActorFacts>,
	pub stage_effects: Vec<ScheduledStageEffect>,
	pub reached_mechanics: Vec<ReachedMechanic>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MaterializedGuideAction {
	Deploy {
		requested_at: SimTick,
		operator: OperatorInstanceId,
		cell: GridPosition,
		facing: Facing,
	},
	/// MAA job file `Skill` action; replay resolves this to activation or manual discard.
	ActivateSkill {
		requested_at: SimTick,
		operator: OperatorInstanceId,
	},
	Retreat {
		requested_at: SimTick,
		operator: OperatorInstanceId,
	},
	SpeedUp {
		requested_at: SimTick,
	},
	SkillDaemon {
		requested_at: SimTick,
	},
}

impl MaterializedGuideAction {
	#[must_use]
	pub const fn requested_at(&self) -> SimTick {
		match self {
			Self::Deploy { requested_at, .. }
			| Self::ActivateSkill { requested_at, .. }
			| Self::Retreat { requested_at, .. }
			| Self::SpeedUp { requested_at }
			| Self::SkillDaemon { requested_at } => *requested_at,
		}
	}
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MaterializedGuide {
	pub builds: Vec<OperatorBuildFacts>,
	pub actions: Vec<MaterializedGuideAction>,
	pub reached_mechanics: Vec<ReachedMechanic>,
}

/// Executor policy values materialized from guide operator metadata.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MaterializedGuidePolicy {
	skill_usage: BTreeMap<OperatorInstanceId, u8>,
}

impl MaterializedGuidePolicy {
	#[must_use]
	pub fn new(skill_usage: BTreeMap<OperatorInstanceId, u8>) -> Self {
		Self { skill_usage }
	}

	#[must_use]
	pub fn skill_usage(&self, operator: OperatorInstanceId) -> Option<u8> {
		self.skill_usage.get(&operator).copied()
	}

	#[must_use]
	pub fn entries(&self) -> &BTreeMap<OperatorInstanceId, u8> {
		&self.skill_usage
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EffectTarget {
	CurrentTargets,
	/// The actor that caused the listener's current trigger.
	///
	/// This target is only valid on an effect directly owned by a [`ReplayListener`].
	TriggerEffectSource,
	SelfActor,
	Blocker,
	BlockedEnemies,
	LowestHpAllyInRange,
	AlliesInRange,
	EnemiesInRange,
	AllAllies,
	AllEnemies,
}

/// A cell set resolved when a listener or tile effect is installed.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReplayCellSet {
	Explicit(BTreeSet<GridPosition>),
	SourceCell,
	/// The source actor's current effective attack range, intersected with lowland cells.
	SourceCurrentAttackRangeGroundCells,
	/// Right-facing offsets projected from the current effect or trigger target.
	/// Only valid for tile effects.
	EffectTargetRelative(Vec<RangeOffset>),
	/// Right-facing offsets projected from the source actor's current cell and facing.
	SourceRelative(Vec<RangeOffset>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReplayTriggerKind {
	Deploy,
	SkillStarted,
	SkillEnded,
	NormalAttackHit,
	Heal,
	Damage,
	BeforeLethal,
	Death,
	Periodic,
	ActorEnteredCells,
	ActorLeftCells,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReplayTrigger {
	Deploy,
	SkillStarted,
	SkillEnded,
	NormalAttackHit,
	Heal,
	Damage,
	BeforeLethal,
	Death,
	Periodic {
		initial_delay_ticks: u64,
		interval_ticks: u64,
	},
	ActorEnteredCells {
		cells: ReplayCellSet,
	},
	ActorLeftCells {
		cells: ReplayCellSet,
	},
}

impl ReplayTrigger {
	#[must_use]
	pub const fn kind(&self) -> ReplayTriggerKind {
		match self {
			Self::Deploy => ReplayTriggerKind::Deploy,
			Self::SkillStarted => ReplayTriggerKind::SkillStarted,
			Self::SkillEnded => ReplayTriggerKind::SkillEnded,
			Self::NormalAttackHit => ReplayTriggerKind::NormalAttackHit,
			Self::Heal => ReplayTriggerKind::Heal,
			Self::Damage => ReplayTriggerKind::Damage,
			Self::BeforeLethal => ReplayTriggerKind::BeforeLethal,
			Self::Death => ReplayTriggerKind::Death,
			Self::Periodic { .. } => ReplayTriggerKind::Periodic,
			Self::ActorEnteredCells { .. } => ReplayTriggerKind::ActorEnteredCells,
			Self::ActorLeftCells { .. } => ReplayTriggerKind::ActorLeftCells,
		}
	}
}

/// Which actor in a runtime trigger must match a listener.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TriggerActorFilter {
	/// The actor affected by the event is the listener owner.
	SelfActor,
	/// The actor that caused the event is the listener owner.
	EffectSource,
	AnyActor,
	AllyOfSource,
	EnemyOfSource,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PredicateActor {
	ListenerSource,
	TriggerActor,
	EffectSource,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HpRatioComparison {
	LessThan,
	LessThanOrEqual,
	GreaterThan,
	GreaterThanOrEqual,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReplayPredicate {
	Always,
	HpRatio {
		actor: PredicateActor,
		comparison: HpRatioComparison,
		threshold: ExpectedValue,
	},
	All(Vec<Self>),
	Any(Vec<Self>),
	Not(Box<Self>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReplayRepeat {
	Unlimited,
	Once,
}

/// Lifetime of a runtime primitive installed by one source actor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RuntimeEffectLifetime {
	Persistent,
	SourceActor,
	SkillActivation,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplayListener {
	pub key: MechanicId,
	pub trigger: ReplayTrigger,
	pub actor_filter: TriggerActorFilter,
	pub predicate: ReplayPredicate,
	pub repeat: ReplayRepeat,
	pub lifetime: RuntimeEffectLifetime,
	pub effects: Vec<TaggedEffect>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BuffModifier {
	Stat(StatModifier),
	/// Additive Arts-resistance points represented as a ratio (`20` points is `0.20`).
	ArtsResistanceAddition {
		ratio: ExpectedValue,
	},
	/// Incoming-damage multiplier bypassed by attacks that explicitly ignore shelter.
	Shelter {
		incoming_damage_multiplier: ExpectedValue,
	},
	/// Additive reduction to movement speed. Stacks combine before converting speed to interval.
	MovementSpeedReduction {
		reduction: ExpectedValue,
	},
	AttackCadence(AttackCadenceModifier),
	AttackSpeed(AttackSpeedModifier),
	AttackRange(AttackRangeModifier),
	TargetCount(TargetCountModifier),
	HitCount(HitCountModifier),
	Blocking(BlockCountModifier),
	BlockTarget(BlockTargetModifier),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeBuff {
	pub key: MechanicId,
	pub max_stacks: u16,
	pub duration_ticks: Option<u64>,
	pub lifetime: RuntimeEffectLifetime,
	pub modifiers_per_stack: Vec<BuffModifier>,
}

/// A condition evaluated over deployed operators or the immutable guide roster.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TeamCompositionCondition {
	DeployedDistinctProfessions {
		minimum_count: u16,
		profession_mask: u16,
	},
	DeployedSameProfession {
		minimum_count: u16,
		profession_mask: u16,
	},
	FixedRosterGroup {
		minimum_count: u16,
		group_id: String,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConditionalAuraPeriodicEffect {
	pub initial_delay_ticks: u64,
	pub interval_ticks: u64,
	pub effects: Vec<TaggedEffect>,
}

/// One deployment-owned aura reconciled against typed operator-classification facts.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeConditionalAura {
	pub key: MechanicId,
	pub condition: TeamCompositionCondition,
	pub target_profession_mask: u16,
	pub includes_source: bool,
	pub lifetime: RuntimeEffectLifetime,
	pub buff: RuntimeBuff,
	pub periodic: Option<ConditionalAuraPeriodicEffect>,
}

/// One source-owned stance that is active exactly while its skill is inactive.
///
/// Installation applies the stance immediately. Skill start removes its attack gate and
/// modifiers; skill end restores them. The stance itself lasts for the source deployment.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeSkillInactiveStance {
	pub key: MechanicId,
	pub disable_normal_attacks: bool,
	pub modifiers: Vec<BuffModifier>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BuffRemovalSource {
	EffectSource,
	AnySource,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PeriodicTileEffect {
	pub initial_delay_ticks: u64,
	pub interval_ticks: u64,
	pub effects_per_layer: Vec<TaggedEffect>,
}

/// The exact layer mutation that may activate a cell-scoped reaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TileLayerThresholdTransition {
	/// The mutation moved from below the threshold to the threshold or above it.
	ReachedFromBelow,
	/// The cell was already saturated and another layer was requested.
	SaturatedAttempt,
}

/// Collision policy for an actor created on the layer mutation's target cell.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetCellSpawnCollision {
	/// Suppress creation when an operator or an exclusive friendly summon reserves the cell.
	SuppressIfFriendlyOccupied,
	AllowCoexistence,
}

/// Runtime lifetime relation for an actor created by a tile-layer reaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetCellSpawnOwnership {
	Independent,
	SourceActor,
	SkillActivation,
	TriggeringTileEffect,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TargetCellActorSpawn {
	pub template: SummonTemplateId,
	pub collision: TargetCellSpawnCollision,
	pub ownership: TargetCellSpawnOwnership,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TileLayerThresholdReactionEffect {
	SpawnActor(TargetCellActorSpawn),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeTileLayerThresholdReaction {
	pub threshold: u16,
	pub transition: TileLayerThresholdTransition,
	pub effect: TileLayerThresholdReactionEffect,
}

/// Runtime owner of the reaction definition that requested a target-cell spawn.
///
/// The mutated tile identity is carried separately because an overlay observes, but does not own,
/// that tile. Overlay-owned reactions must still belong to the same activation when the queued
/// spawn resolves.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TileLayerReactionOwner {
	TileEffect,
	TileFieldOverlay {
		overlay_id: u64,
		overlay_generation: u32,
	},
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TargetCellActorSpawnEvent {
	pub source: BattleActorId,
	pub source_deployment_generation: Option<u32>,
	pub tile_effect_id: u64,
	pub tile_effect_generation: u32,
	pub reaction_owner: TileLayerReactionOwner,
	pub template: SummonTemplateId,
	pub instance: u32,
	pub cell: GridPosition,
	pub collision: TargetCellSpawnCollision,
	pub ownership: TargetCellSpawnOwnership,
}

/// One keyed, layered tile effect. Reapplying the same source/key adds one layer and refreshes it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeTileEffect {
	pub key: MechanicId,
	pub cells: ReplayCellSet,
	pub actor_filter: TriggerActorFilter,
	pub target_filter: NormalAttackTargetFilter,
	pub max_layers: u16,
	pub duration_ticks: Option<u64>,
	pub lifetime: RuntimeEffectLifetime,
	pub apply_new_layer_to_occupants: bool,
	/// Effects applied once whenever an actor crosses into the cell, independent of layer count.
	pub on_enter_once: Vec<TaggedEffect>,
	pub on_enter_per_layer: Vec<TaggedEffect>,
	pub on_leave_per_layer: Vec<TaggedEffect>,
	pub on_remove_per_layer: Vec<TaggedEffect>,
	pub layer_threshold_reactions: Vec<RuntimeTileLayerThresholdReaction>,
	/// Remove only this source/key/cell field after the first matching actor leaves it.
	pub remove_on_first_matching_leave: bool,
	pub periodic: Option<PeriodicTileEffect>,
}

/// A source-owned runtime view over every active tile cell with one mechanic key.
///
/// Unlike [`RuntimeTileEffect`], overlay effects are independent of the observed tile's layer
/// count. The overlay does not create, mutate, or own any tile cells.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TileFieldOverlayLeaveScope {
	/// Fire when an actor leaves one observed cell, including entry into another observed cell.
	ObservedCell,
	/// Fire only when an actor leaves the union of every currently observed cell.
	ObservedField,
}

/// Operator-independent rules for one cached propagation pass after an observed tile-field pulse.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeCachedTileLayerPropagation {
	/// Maximum currently existing temporary cells outside the source's effective attack range.
	pub maximum_outside_temporary_cells: u16,
	/// Source-block order is evidence-bearing; unknown order must never fall back to map order.
	pub source_order: PulseSourceOrder<GridPosition>,
	/// Multi-source destination merging remains explicit when the official behavior is unknown.
	pub contribution_merge: ContributionMergePolicy,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeTileFieldOverlay {
	pub key: MechanicId,
	pub observed_tile_key: MechanicId,
	/// Required when this overlay owns layer-threshold reactions; checked against every observed
	/// runtime cell before a reaction can fire.
	pub observed_tile_max_layers: Option<u16>,
	pub actor_filter: TriggerActorFilter,
	pub target_filter: NormalAttackTargetFilter,
	pub leave_scope: TileFieldOverlayLeaveScope,
	pub initial_delay_ticks: u64,
	pub interval_ticks: u64,
	pub lifetime: RuntimeEffectLifetime,
	pub periodic_effects: Vec<TaggedEffect>,
	pub on_leave_once: Vec<TaggedEffect>,
	pub layer_threshold_reactions: Vec<RuntimeTileLayerThresholdReaction>,
	pub cached_propagation: Option<RuntimeCachedTileLayerPropagation>,
}

/// No-op trace event emitted after a resolved cached tile-layer mutation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RuntimeTileLayerPropagationAudit {
	LayersAdded {
		cell: GridPosition,
		cause: RuntimeTileLayerMutationCause,
		previous_layers: u16,
		added_layers: u16,
		current_layers: u16,
	},
	SaturatedAttempt {
		cell: GridPosition,
		cause: RuntimeTileLayerMutationCause,
		at_layers: u16,
		attempted_layers: usize,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RuntimeTileLayerMutationCause {
	PulseIncrement,
	CachedPropagation {
		contributing_sources: Vec<GridPosition>,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaggedEffect {
	pub mechanic: MechanicId,
	pub target: EffectTarget,
	pub effect: ExpectedEffect,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StatKind {
	Attack,
	Defense,
	MaxHp,
	IncomingDamage,
}

/// Which multiplier bucket (乘区) a stat modifier resolves in.
///
/// Percentage buffs that share a bucket sum inside it; separate buckets multiply. Two 12% panel
/// buffs therefore give `x1.24`, while a 12% panel buff and a `x1.12` scale give `x1.2544`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StatModifierBucket {
	/// A percentage the game shows on the operator's panel, worded "攻击力+12%". Every panel ratio
	/// on one stat resolves into a single sum before it multiplies the base value.
	Panel,
	/// A standalone multiplier that never reaches the panel, worded "造成相当于攻击力180%的伤害"
	/// or "攻击时攻击力提高至180%". Each one is its own bucket and multiplies independently.
	Scale,
}

/// A modifier's `multiplier` is always what it would do alone, whichever bucket it resolves in: a
/// twelve-percent buff is `1.12` either way. The bucket decides only how it combines with others.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StatModifier {
	pub stat: StatKind,
	pub bucket: StatModifierBucket,
	pub multiplier: ExpectedValue,
	pub duration_ticks: Option<u64>,
}

/// A predicate over the live battle state of the actor that carries a modifier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActorCondition {
	/// Blocked-Enemy Count Condition: the carrier currently blocks at least this many enemies.
	///
	/// It counts the carrier's live blocking relations, one per enemy, so neither its block
	/// capacity nor the block demand of the enemies it holds enters the comparison.
	BlockedEnemiesAtLeast(u16),
}

/// A stat modifier that counts only while its carrier satisfies `condition`.
///
/// The condition is evaluated whenever the stat is read, so the modifier follows every change to
/// the state it reads from the event that makes that change, with no reconciliation of its own.
/// Its lifetime and bucket are those of `modifier`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConditionalStatModifier {
	pub condition: ActorCondition,
	pub modifier: StatModifier,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttackCadenceModifier {
	pub interval_multiplier: ExpectedValue,
	pub duration_ticks: Option<u64>,
}

/// Additive attack-speed points. Effective cadence scales by `100 / (100 + total_delta)`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttackSpeedModifier {
	pub delta: i16,
	pub duration_ticks: Option<u64>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttackRangeModifier {
	pub additional_offsets: Vec<RangeOffset>,
	pub duration_ticks: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetCountModifier {
	Delta {
		delta: i16,
		duration_ticks: Option<u64>,
	},
	Unlimited {
		duration_ticks: Option<u64>,
	},
}

impl TargetCountModifier {
	#[must_use]
	pub const fn duration_ticks(self) -> Option<u64> {
		match self {
			Self::Delta { duration_ticks, .. } | Self::Unlimited { duration_ticks } => {
				duration_ticks
			}
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HitCountModifier {
	pub delta: i16,
	pub duration_ticks: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockCountModifier {
	pub delta: i16,
	pub duration_ticks: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BlockTargetFilter {
	GroundAndAir,
	GroundOnly,
	AirOnly,
}

impl BlockTargetFilter {
	#[must_use]
	pub const fn allows(self, target_class: TargetClass) -> bool {
		match (self, target_class) {
			(Self::GroundAndAir, _)
			| (Self::GroundOnly, TargetClass::Ground)
			| (Self::AirOnly, TargetClass::Air) => true,
			(Self::GroundOnly, TargetClass::Air) | (Self::AirOnly, TargetClass::Ground) => false,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockTargetModifier {
	pub filter: BlockTargetFilter,
	pub duration_ticks: Option<u64>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StatusKind {
	Cold,
	Frozen,
	Stun,
	Slow,
	Shelter,
	Exposure,
	Gravity,
	/// Permanent marker: damage to this actor exposes the damage source.
	ExposeDamageSource {
		exposure_duration_ticks: u64,
		incoming_damage_multiplier: ExpectedValue,
	},
	/// Permanent marker: hostile ordinary targeting requires this actor to be blocked.
	StealthWhileUnblocked,
	Custom(MechanicId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StatusFactionFilter {
	Any,
	HostileOnly,
}

impl StatusFactionFilter {
	#[must_use]
	pub const fn allows(self, faction: Faction) -> bool {
		match self {
			Self::Any => true,
			Self::HostileOnly => matches!(faction, Faction::Hostile),
		}
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StatusEffect {
	pub kind: StatusKind,
	pub duration_ticks: u64,
	pub movement_interval_multiplier: ExpectedValue,
	pub attack_speed_delta: i16,
	pub incoming_damage_multiplier: ExpectedValue,
	/// Additive Arts-resistance reduction in ratio units; `0.15` means 15 percentage points.
	pub arts_resistance_reduction: ExpectedValue,
	pub arts_resistance_reduction_target: StatusFactionFilter,
}

impl StatusEffect {
	/// Canonical Cold: additive ASPD -30.
	#[must_use]
	pub fn cold(duration_ticks: u64) -> Self {
		Self {
			kind: StatusKind::Cold,
			duration_ticks,
			movement_interval_multiplier: ExpectedValue::ONE,
			attack_speed_delta: -30,
			incoming_damage_multiplier: ExpectedValue::ONE,
			arts_resistance_reduction: ExpectedValue::ZERO,
			arts_resistance_reduction_target: StatusFactionFilter::Any,
		}
	}

	/// Canonical Frozen control and RES -15 profile.
	///
	/// Action pausing is interpreted from [`StatusKind::Frozen`] by the replay runtime.
	#[must_use]
	pub const fn frozen(duration_ticks: u64) -> Self {
		Self {
			kind: StatusKind::Frozen,
			duration_ticks,
			movement_interval_multiplier: ExpectedValue::ONE,
			attack_speed_delta: 0,
			incoming_damage_multiplier: ExpectedValue::ONE,
			arts_resistance_reduction: ExpectedValue::from_scaled(150_000),
			arts_resistance_reduction_target: StatusFactionFilter::HostileOnly,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ShiftDirection {
	TowardGoal,
	AwayFromGoal,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ShiftEffect {
	pub direction: ShiftDirection,
	pub cells: u16,
	pub force: i16,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SummonEffect {
	pub template: SummonTemplateId,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemyAxisDirectHitFacts {
	pub initial_cooldown_ticks: u64,
	pub cooldown_ticks: u64,
	/// Delay from skill activation until the first projectile resolves.
	pub windup_ticks: u64,
	/// Total action lockout from activation; cooldown starts after this duration.
	pub skill_duration_ticks: u64,
	pub projectiles: u16,
	pub damage_multiplier: ExpectedValue,
	pub damage_type: DamageType,
	pub delivery: DamageDelivery,
	pub status: Option<StatusEffect>,
	pub requires_unblocked: bool,
	/// Delay after a blocking episode ends before conditional stealth returns.
	pub stealth_recovery_ticks: Option<u64>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedEffect {
	Damage {
		amount: ExpectedValue,
		damage_type: DamageType,
		delivery: DamageDelivery,
	},
	/// Damage sampled from the source's current effective stat when this effect resolves.
	SourceStatScaledDamage {
		stat: StatKind,
		multiplier: ExpectedValue,
		damage_type: DamageType,
		delivery: DamageDelivery,
	},
	Healing {
		amount: ExpectedValue,
	},
	/// Restore HP to at least this ratio of the target's current effective maximum HP.
	///
	/// This is a direct state restoration and deliberately does not emit a Heal trigger.
	RestoreHpToCurrentMaxRatio {
		ratio: ExpectedValue,
	},
	StatModifier(StatModifier),
	/// A stat modifier gated by a condition over the actor it is applied to.
	ConditionalStatModifier(ConditionalStatModifier),
	AttackCadence(AttackCadenceModifier),
	AttackSpeed(AttackSpeedModifier),
	AttackRange(AttackRangeModifier),
	TargetCount(TargetCountModifier),
	HitCount(HitCountModifier),
	GainSp {
		amount: u32,
	},
	/// Whole deployment points credited once to the shared battle economy, capped at its maximum.
	///
	/// DP belongs to the battle rather than to any actor, so the effect names its own source and
	/// resolves once however many actors that target would select.
	GainDp {
		amount: u32,
	},
	ActivateSkill,
	ApplyStatus(StatusEffect),
	Shift(ShiftEffect),
	Blocking(BlockCountModifier),
	BlockTarget(BlockTargetModifier),
	Summon(SummonEffect),
	StartAxisDirectHit(EnemyAxisDirectHitFacts),
	RegisterListener(ReplayListener),
	RemoveListener {
		key: MechanicId,
	},
	ApplyBuff(RuntimeBuff),
	InstallConditionalAura(RuntimeConditionalAura),
	InstallSkillInactiveStance(RuntimeSkillInactiveStance),
	RemoveBuff {
		key: MechanicId,
		source: BuffRemovalSource,
	},
	AddTileLayer(RuntimeTileEffect),
	InstallTileFieldOverlay(RuntimeTileFieldOverlay),
	RemoveTileEffect {
		key: MechanicId,
	},
	Death,
	Sequence(Vec<TaggedEffect>),
	/// Explicit RNG collapsed by the compiler. Discrete effects are rejected.
	Expected {
		probability: ExpectedValue,
		effect: Box<ExpectedEffect>,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScheduledStageEffect {
	pub at: SimTick,
	pub repeat_interval_ticks: Option<u64>,
	pub repeat_until: Option<SimTick>,
	pub effect: TaggedEffect,
}
