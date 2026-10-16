use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::num::{NonZeroU32, NonZeroU64};

use crate::data::{
	OperatorPosition, ResolvedOperatorBuild, SkillActivationType, SkillDurationType, SkillSpType,
};
use crate::simulation::TickRate;

use crate::mechanics::normal_attack_catalog::{
	deterministic_attack_interval_ticks, AttackIntervalError,
};

const TICKS_NUMERIC_SCALE: f64 = 1_000_000.0;

/// One operator/skill pair covered by the built-in reviewed binding registry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReviewedSkillDamageSelection {
	pub operator_id: &'static str,
	pub skill: u8,
}

pub const REVIEWED_SKILL_DAMAGE_SELECTIONS: &[ReviewedSkillDamageSelection] = &[
	ReviewedSkillDamageSelection {
		operator_id: "char_002_amiya",
		skill: 1,
	},
	ReviewedSkillDamageSelection {
		operator_id: "char_4133_logos",
		skill: 3,
	},
	ReviewedSkillDamageSelection {
		operator_id: "char_190_clour",
		skill: 2,
	},
	ReviewedSkillDamageSelection {
		operator_id: "char_103_angel",
		skill: 2,
	},
];

/// Damage type used by the expected-damage relaxation before enemy defenses are applied.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedDamageType {
	Physical,
	Arts,
	True,
}

/// Which enemies an attack may lock.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedTargetFilter {
	GroundAndAir,
	GroundOnly,
	AirOnly,
}

/// A recognized multi-target delivery rule. Unrecognized splash or chain mechanics fail closed.
#[derive(Clone, Debug, PartialEq)]
pub enum ExpectedDamageDelivery {
	Direct,
	Splash {
		radius_tiles: f64,
		secondary_damage_scale: f64,
	},
	Chain {
		maximum_targets: NonZeroU32,
		damage_retention_per_jump: f64,
	},
}

/// Reviewed damage mechanic shapes. A template determines how the bound blackboard fields are
/// interpreted; blackboard names alone never select a mechanic.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillDamageMechanicTemplate {
	ModifiedNormal,
	InstantPacket,
	PeriodicAoe,
	AmmoModifiedNormal,
}

/// Semantic fields that a reviewed skill/prefab binding may consume.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillDamageSemantic {
	AdditiveAttackRatio,
	PrimaryAttackScale,
	PrimaryHitCount,
	AlternateAttackScale,
	AlternateProbability,
	AttackSpeedBonus,
	BaseAttackTimeDeltaSeconds,
	MaximumPrimaryTargets,
	ForwardRangeExtensionTiles,
	PeriodicIntervalSeconds,
	AmmoCount,
	SplashRadiusTiles,
	SplashSecondaryDamageScale,
	ChainMaximumTargets,
	ChainDamageRetention,
	PhysicalDefenseReductionRatio,
	PhysicalDefenseReductionFixed,
	ArtsResistanceReductionRatio,
	ArtsResistanceReductionFixed,
}

/// Exact selector for a declarative mechanic binding.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillDamageBindingSelector {
	SkillId(String),
	PrefabId(String),
}

/// Declarative review record that assigns skill-specific blackboard keys their semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDamageTemplateBinding {
	selector: SkillDamageBindingSelector,
	template: SkillDamageMechanicTemplate,
	allowed_operator_ids: BTreeSet<String>,
	require_no_module: bool,
	semantics: BTreeMap<String, SkillDamageSemantic>,
	ignored_keys: BTreeSet<String>,
}

impl SkillDamageTemplateBinding {
	#[must_use]
	pub fn new(
		selector: SkillDamageBindingSelector,
		template: SkillDamageMechanicTemplate,
	) -> Self {
		Self {
			selector,
			template,
			allowed_operator_ids: BTreeSet::new(),
			require_no_module: false,
			semantics: BTreeMap::new(),
			ignored_keys: BTreeSet::new(),
		}
	}

	#[must_use]
	pub fn for_operator_id(mut self, operator_id: impl Into<String>) -> Self {
		self.allowed_operator_ids.insert(operator_id.into());
		self
	}

	#[must_use]
	pub fn requiring_no_module(mut self) -> Self {
		self.require_no_module = true;
		self
	}

	#[must_use]
	pub fn with_semantic(mut self, key: impl Into<String>, semantic: SkillDamageSemantic) -> Self {
		self.semantics.insert(key.into(), semantic);
		self
	}

	#[must_use]
	pub fn with_ignored_key(mut self, key: impl Into<String>) -> Self {
		self.ignored_keys.insert(key.into());
		self
	}

	#[must_use]
	pub const fn selector(&self) -> &SkillDamageBindingSelector {
		&self.selector
	}

	#[must_use]
	pub const fn template(&self) -> SkillDamageMechanicTemplate {
		self.template
	}

	#[must_use]
	pub const fn allowed_operator_ids(&self) -> &BTreeSet<String> {
		&self.allowed_operator_ids
	}

	#[must_use]
	pub const fn require_no_module(&self) -> bool {
		self.require_no_module
	}

	#[must_use]
	pub const fn semantics(&self) -> &BTreeMap<String, SkillDamageSemantic> {
		&self.semantics
	}

	#[must_use]
	pub const fn ignored_keys(&self) -> &BTreeSet<String> {
		&self.ignored_keys
	}
}

/// Auditable result of applying a declarative binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDamageKeyAudit {
	consumed_keys: Vec<String>,
	ignored_keys: Vec<String>,
}

impl SkillDamageKeyAudit {
	#[must_use]
	pub fn consumed_keys(&self) -> &[String] {
		&self.consumed_keys
	}

	#[must_use]
	pub fn ignored_keys(&self) -> &[String] {
		&self.ignored_keys
	}
}

/// Defense adjustments applied by every hit in an attack phase.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExpectedDefenseAdjustment {
	physical_defense_reduction_ratio: f64,
	physical_defense_reduction_fixed: f64,
	arts_resistance_reduction_ratio: f64,
	arts_resistance_reduction_fixed: f64,
}

impl ExpectedDefenseAdjustment {
	#[must_use]
	pub const fn physical_defense_reduction_ratio(self) -> f64 {
		self.physical_defense_reduction_ratio
	}

	#[must_use]
	pub const fn physical_defense_reduction_fixed(self) -> f64 {
		self.physical_defense_reduction_fixed
	}

	#[must_use]
	pub const fn arts_resistance_reduction_ratio(self) -> f64 {
		self.arts_resistance_reduction_ratio
	}

	#[must_use]
	pub const fn arts_resistance_reduction_fixed(self) -> f64 {
		self.arts_resistance_reduction_fixed
	}
}

/// One raw-damage outcome retained until enemy-specific mitigation has been applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpectedRawDamageOutcome {
	probability: f64,
	raw_damage: f64,
}

impl ExpectedRawDamageOutcome {
	#[must_use]
	pub const fn probability(self) -> f64 {
		self.probability
	}

	#[must_use]
	pub const fn raw_damage(self) -> f64 {
		self.raw_damage
	}
}

/// Discrete raw-damage distribution for one primary hit.
///
/// The distribution must remain intact across nonlinear DEF, RES, and minimum-damage
/// calculations. The current solver then advances one deterministic expected-HP flow rather than
/// propagating the full distribution of possible enemy HP states.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedRawDamageMixture {
	outcomes: Vec<ExpectedRawDamageOutcome>,
}

impl ExpectedRawDamageMixture {
	fn deterministic(raw_damage: f64) -> Self {
		Self {
			outcomes: vec![ExpectedRawDamageOutcome {
				probability: 1.0,
				raw_damage,
			}],
		}
	}

	fn from_binary(
		primary_raw_damage: f64,
		alternate_raw_damage: f64,
		alternate_probability: f64,
	) -> Self {
		let mut outcomes: Vec<ExpectedRawDamageOutcome> = Vec::with_capacity(2);
		let primary_probability: f64 = 1.0 - alternate_probability;
		if primary_probability > 0.0 {
			outcomes.push(ExpectedRawDamageOutcome {
				probability: primary_probability,
				raw_damage: primary_raw_damage,
			});
		}
		if alternate_probability > 0.0 {
			outcomes.push(ExpectedRawDamageOutcome {
				probability: alternate_probability,
				raw_damage: alternate_raw_damage,
			});
		}
		Self { outcomes }
	}

	#[must_use]
	pub fn outcomes(&self) -> &[ExpectedRawDamageOutcome] {
		&self.outcomes
	}

	/// Raw expectation for reporting only. Do not apply nonlinear mitigation to this value.
	#[must_use]
	pub fn expected_raw_damage(&self) -> f64 {
		self.expected_value_by(|raw_damage| raw_damage)
	}

	/// Applies an enemy-specific transformation to every outcome before taking its expectation.
	#[must_use]
	pub fn expected_value_by(&self, mut transform: impl FnMut(f64) -> f64) -> f64 {
		self.outcomes
			.iter()
			.map(|outcome| outcome.probability * transform(outcome.raw_damage))
			.sum()
	}
}

/// One constant expected-damage phase.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedAttackPhase {
	primary_hit_outcomes: ExpectedRawDamageMixture,
	primary_hit_count: NonZeroU32,
	damage_type: ExpectedDamageType,
	attack_interval_ticks: NonZeroU64,
	maximum_primary_targets: NonZeroU32,
	target_filter: ExpectedTargetFilter,
	delivery: ExpectedDamageDelivery,
	defense_adjustment: ExpectedDefenseAdjustment,
}

impl ExpectedAttackPhase {
	/// Raw expectation for reporting compatibility. Mitigation must instead consume
	/// [`Self::primary_hit_outcomes`] outcome-by-outcome.
	#[must_use]
	pub fn expected_damage_per_primary_hit(&self) -> f64 {
		self.primary_hit_outcomes.expected_raw_damage()
	}

	#[must_use]
	pub const fn primary_hit_outcomes(&self) -> &ExpectedRawDamageMixture {
		&self.primary_hit_outcomes
	}

	#[must_use]
	pub const fn primary_hit_count(&self) -> NonZeroU32 {
		self.primary_hit_count
	}

	#[must_use]
	pub const fn damage_type(&self) -> ExpectedDamageType {
		self.damage_type
	}

	#[must_use]
	pub const fn attack_interval_ticks(&self) -> NonZeroU64 {
		self.attack_interval_ticks
	}

	#[must_use]
	pub const fn maximum_primary_targets(&self) -> NonZeroU32 {
		self.maximum_primary_targets
	}

	#[must_use]
	pub const fn target_filter(&self) -> ExpectedTargetFilter {
		self.target_filter
	}

	#[must_use]
	pub const fn delivery(&self) -> &ExpectedDamageDelivery {
		&self.delivery
	}

	#[must_use]
	pub const fn defense_adjustment(&self) -> ExpectedDefenseAdjustment {
		self.defense_adjustment
	}
}

/// How long the active phase can contribute after one activation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedSkillDuration {
	Instant,
	Timed { ticks: NonZeroU64 },
	Ammo { attacks: NonZeroU32 },
	Indefinite,
}

/// Range geometry retained for the space-time intersection layer.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedSkillRange {
	range_id: String,
	forward_extension_tiles: u32,
}

impl ExpectedSkillRange {
	#[must_use]
	pub fn range_id(&self) -> &str {
		&self.range_id
	}

	#[must_use]
	pub const fn forward_extension_tiles(&self) -> u32 {
		self.forward_extension_tiles
	}
}

/// A damage-only, expected-value profile compiled from one exact operator build.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedDamageSkillProfile {
	operator_id: String,
	skill_id: String,
	template: SkillDamageMechanicTemplate,
	activation_type: SkillActivationType,
	sp_type: SkillSpType,
	sp_cost: u32,
	initial_sp: u32,
	sp_increment: f64,
	duration: ExpectedSkillDuration,
	range: ExpectedSkillRange,
	key_audit: SkillDamageKeyAudit,
	additive_attack_ratio: f64,
	primary_attack_scale: f64,
	attack_speed_bonus: f64,
	active_interval_before_attack_speed_ticks: NonZeroU64,
	base: ExpectedAttackPhase,
	active: ExpectedAttackPhase,
}

impl ExpectedDamageSkillProfile {
	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub fn skill_id(&self) -> &str {
		&self.skill_id
	}

	#[must_use]
	pub const fn template(&self) -> SkillDamageMechanicTemplate {
		self.template
	}

	/// The panel percentage the active phase grants, as read from its `AdditiveAttackRatio` key.
	/// Kept apart from [`Self::primary_attack_scale`] because the two resolve in different
	/// multiplier buckets: this one sums with every other panel percentage on the operator.
	#[must_use]
	pub const fn additive_attack_ratio(&self) -> f64 {
		self.additive_attack_ratio
	}

	/// The active phase's damage scale, as read from its `PrimaryAttackScale` key. Never reaches
	/// the panel, so it is its own bucket and multiplies independently.
	#[must_use]
	pub const fn primary_attack_scale(&self) -> f64 {
		self.primary_attack_scale
	}

	/// The attack-speed points the active phase grants. Additive: every source's points land in
	/// one sum, and the cadence follows from that sum rather than from each source in turn.
	#[must_use]
	pub const fn attack_speed_bonus(&self) -> f64 {
		self.attack_speed_bonus
	}

	/// The active phase's cadence with the attack-speed points left out, so the base-attack-time
	/// change the skill makes can be lowered apart from [`Self::attack_speed_bonus`].
	#[must_use]
	pub const fn active_interval_before_attack_speed_ticks(&self) -> NonZeroU64 {
		self.active_interval_before_attack_speed_ticks
	}

	#[must_use]
	pub const fn activation_type(&self) -> &SkillActivationType {
		&self.activation_type
	}

	#[must_use]
	pub const fn sp_type(&self) -> &SkillSpType {
		&self.sp_type
	}

	#[must_use]
	pub const fn sp_cost(&self) -> u32 {
		self.sp_cost
	}

	#[must_use]
	pub const fn initial_sp(&self) -> u32 {
		self.initial_sp
	}

	#[must_use]
	pub const fn sp_increment(&self) -> f64 {
		self.sp_increment
	}

	#[must_use]
	pub const fn duration(&self) -> ExpectedSkillDuration {
		self.duration
	}

	#[must_use]
	pub const fn range(&self) -> &ExpectedSkillRange {
		&self.range
	}

	#[must_use]
	pub const fn key_audit(&self) -> &SkillDamageKeyAudit {
		&self.key_audit
	}

	#[must_use]
	pub const fn base(&self) -> &ExpectedAttackPhase {
		&self.base
	}

	#[must_use]
	pub const fn active(&self) -> &ExpectedAttackPhase {
		&self.active
	}
}

/// Owned blackboard input for pure compiler tests and non-ArkData adapters.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillDamageBlackboardFact {
	key: String,
	value: f64,
	value_string: Option<String>,
}

impl SkillDamageBlackboardFact {
	#[must_use]
	pub fn new(key: String, value: f64, value_string: Option<String>) -> Self {
		Self {
			key,
			value,
			value_string,
		}
	}

	#[must_use]
	pub fn key(&self) -> &str {
		&self.key
	}

	#[must_use]
	pub const fn value(&self) -> f64 {
		self.value
	}

	#[must_use]
	pub fn value_string(&self) -> Option<&str> {
		self.value_string.as_deref()
	}
}

/// Pure facts consumed by the generic compiler.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillDamageFacts {
	operator_id: String,
	skill_id: String,
	prefab_id: Option<String>,
	has_selected_module: bool,
	profession: String,
	sub_profession_id: String,
	position: OperatorPosition,
	base_attack: f64,
	base_attack_time_seconds: f64,
	base_attack_speed: f64,
	base_range_id: String,
	skill_range_id: Option<String>,
	activation_type: SkillActivationType,
	duration_type: SkillDurationType,
	duration_seconds: f64,
	sp_type: SkillSpType,
	sp_cost: u32,
	initial_sp: u32,
	sp_increment: f64,
	blackboard: Vec<SkillDamageBlackboardFact>,
}

impl SkillDamageFacts {
	#[allow(clippy::too_many_arguments)]
	#[must_use]
	pub fn new(
		operator_id: String,
		skill_id: String,
		prefab_id: Option<String>,
		has_selected_module: bool,
		profession: String,
		sub_profession_id: String,
		position: OperatorPosition,
		base_attack: f64,
		base_attack_time_seconds: f64,
		base_attack_speed: f64,
		base_range_id: String,
		skill_range_id: Option<String>,
		activation_type: SkillActivationType,
		duration_type: SkillDurationType,
		duration_seconds: f64,
		sp_type: SkillSpType,
		sp_cost: u32,
		initial_sp: u32,
		blackboard: Vec<SkillDamageBlackboardFact>,
	) -> Self {
		Self {
			operator_id,
			skill_id,
			prefab_id,
			has_selected_module,
			profession,
			sub_profession_id,
			position,
			base_attack,
			base_attack_time_seconds,
			base_attack_speed,
			base_range_id,
			skill_range_id,
			activation_type,
			duration_type,
			duration_seconds,
			sp_type,
			sp_cost,
			initial_sp,
			sp_increment: 1.0,
			blackboard,
		}
	}

	#[must_use]
	pub fn with_sp_increment(mut self, sp_increment: f64) -> Self {
		self.sp_increment = sp_increment;
		self
	}
}

impl From<&ResolvedOperatorBuild> for SkillDamageFacts {
	fn from(build: &ResolvedOperatorBuild) -> Self {
		let stats = build.static_stats();
		let skill = build.selected_skill();
		Self {
			operator_id: build.operator_id().to_owned(),
			skill_id: skill.skill_id().to_owned(),
			prefab_id: skill.prefab_id().map(str::to_owned),
			has_selected_module: build.selected_module().is_some(),
			profession: build.profession().to_owned(),
			sub_profession_id: build.sub_profession_id().to_owned(),
			position: build.position().clone(),
			base_attack: stats.attack(),
			base_attack_time_seconds: stats.base_attack_time_seconds(),
			base_attack_speed: stats.attack_speed(),
			base_range_id: build.range_id().to_owned(),
			skill_range_id: skill.range_id().map(str::to_owned),
			activation_type: skill.activation_type().clone(),
			duration_type: skill.duration_type().clone(),
			duration_seconds: skill.duration_seconds(),
			sp_type: skill.sp().sp_type().clone(),
			sp_cost: skill.sp().cost(),
			initial_sp: skill.sp().initial(),
			sp_increment: skill.sp().increment(),
			blackboard: skill
				.blackboard()
				.iter()
				.map(|entry| {
					SkillDamageBlackboardFact::new(
						entry.key().to_owned(),
						entry.value(),
						entry.value_string().map(str::to_owned),
					)
				})
				.collect(),
		}
	}
}

/// Why a candidate was excluded from the damage-only relaxation.
#[derive(Clone, Debug, PartialEq)]
pub enum SkillDamageUnsupportedReason {
	MissingReviewedBinding {
		skill_id: String,
		prefab_id: Option<String>,
	},
	BindingDoesNotMatch {
		selector: SkillDamageBindingSelector,
	},
	SelectedModuleNotReviewed,
	UnknownDamageType {
		profession: String,
		sub_profession_id: String,
	},
	NonAttackingArchetype {
		profession: String,
		sub_profession_id: String,
	},
	NonFiniteBaseField {
		field: &'static str,
	},
	NonPositiveBaseAttack,
	InvalidBlackboardValue {
		key: String,
	},
	StringValuedDamageMechanic {
		key: String,
	},
	ConflictingAliases {
		field: &'static str,
		keys: Vec<String>,
	},
	UnknownDamageMechanic {
		key: String,
	},
	InvalidModifier {
		key: String,
		value: f64,
	},
	InvalidAttackInterval(AttackIntervalError),
	InvalidDuration {
		seconds: f64,
	},
	UnsupportedDurationType {
		duration_type: SkillDurationType,
	},
	TemplateRequirement {
		template: SkillDamageMechanicTemplate,
		field: &'static str,
	},
}

impl Display for SkillDamageUnsupportedReason {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::MissingReviewedBinding {
				skill_id,
				prefab_id,
			} => write!(
				formatter,
				"no reviewed damage binding for skill {skill_id} (prefab {prefab_id:?})"
			),
			Self::BindingDoesNotMatch { selector } => {
				write!(
					formatter,
					"damage binding {selector:?} does not match the skill"
				)
			}
			Self::SelectedModuleNotReviewed => {
				formatter.write_str("selected module is outside this reviewed damage binding")
			}
			Self::UnknownDamageType {
				profession,
				sub_profession_id,
			} => write!(
				formatter,
				"cannot infer damage type for {profession}/{sub_profession_id}"
			),
			Self::NonAttackingArchetype {
				profession,
				sub_profession_id,
			} => write!(
				formatter,
				"{profession}/{sub_profession_id} has no generic damaging basic attack"
			),
			Self::NonFiniteBaseField { field } => {
				write!(formatter, "base field {field} is not finite")
			}
			Self::NonPositiveBaseAttack => formatter.write_str("base attack must be positive"),
			Self::InvalidBlackboardValue { key } => {
				write!(formatter, "blackboard value for {key} is not finite")
			}
			Self::StringValuedDamageMechanic { key } => {
				write!(
					formatter,
					"damage-related blackboard key {key} has a string value"
				)
			}
			Self::ConflictingAliases { field, keys } => {
				write!(
					formatter,
					"conflicting aliases for {field}: {}",
					keys.join(", ")
				)
			}
			Self::UnknownDamageMechanic { key } => {
				write!(
					formatter,
					"unrecognized damage-affecting blackboard key {key}"
				)
			}
			Self::InvalidModifier { key, value } => {
				write!(formatter, "invalid modifier {key}={value}")
			}
			Self::InvalidAttackInterval(source) => {
				write!(formatter, "invalid attack interval: {source}")
			}
			Self::InvalidDuration { seconds } => {
				write!(formatter, "invalid skill duration {seconds}")
			}
			Self::UnsupportedDurationType { duration_type } => {
				write!(
					formatter,
					"unsupported skill duration type {duration_type:?}"
				)
			}
			Self::TemplateRequirement { template, field } => {
				write!(formatter, "{template:?} requires semantic field {field}")
			}
		}
	}
}

/// Structured fail-closed result for one exact build.
#[derive(Clone, Debug, PartialEq)]
pub struct UnsupportedExpectedDamageSkill {
	operator_id: String,
	skill_id: String,
	reasons: Vec<SkillDamageUnsupportedReason>,
}

impl UnsupportedExpectedDamageSkill {
	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub fn skill_id(&self) -> &str {
		&self.skill_id
	}

	#[must_use]
	pub fn reasons(&self) -> &[SkillDamageUnsupportedReason] {
		&self.reasons
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpectedDamageSkillEntry {
	Supported(Box<ExpectedDamageSkillProfile>),
	Unsupported(UnsupportedExpectedDamageSkill),
}

impl ExpectedDamageSkillEntry {
	#[must_use]
	pub const fn as_supported(&self) -> Option<&ExpectedDamageSkillProfile> {
		match self {
			Self::Supported(profile) => Some(profile),
			Self::Unsupported(_) => None,
		}
	}

	#[must_use]
	pub const fn as_unsupported(&self) -> Option<&UnsupportedExpectedDamageSkill> {
		match self {
			Self::Supported(_) => None,
			Self::Unsupported(unsupported) => Some(unsupported),
		}
	}
}

#[must_use]
pub fn compile_expected_damage_skill(
	build: &ResolvedOperatorBuild,
	tick_rate: TickRate,
) -> ExpectedDamageSkillEntry {
	compile_expected_damage_skill_facts(&build.into(), tick_rate)
}

/// Compiles a reviewed built-in skill or prefab binding.
#[must_use]
pub fn compile_expected_damage_skill_facts(
	facts: &SkillDamageFacts,
	tick_rate: TickRate,
) -> ExpectedDamageSkillEntry {
	let Some(binding) = reviewed_skill_damage_binding(facts) else {
		return ExpectedDamageSkillEntry::Unsupported(UnsupportedExpectedDamageSkill {
			operator_id: facts.operator_id.clone(),
			skill_id: facts.skill_id.clone(),
			reasons: vec![SkillDamageUnsupportedReason::MissingReviewedBinding {
				skill_id: facts.skill_id.clone(),
				prefab_id: facts.prefab_id.clone(),
			}],
		});
	};
	compile_expected_damage_skill_facts_with_binding(facts, tick_rate, &binding)
}

/// Compiles one caller-reviewed declarative binding. This is the extension point for generated
/// registries without adding operator-name branches to the compiler.
#[must_use]
pub fn compile_expected_damage_skill_facts_with_binding(
	facts: &SkillDamageFacts,
	tick_rate: TickRate,
	binding: &SkillDamageTemplateBinding,
) -> ExpectedDamageSkillEntry {
	match compile_facts(facts, tick_rate, binding) {
		Ok(profile) => ExpectedDamageSkillEntry::Supported(Box::new(profile)),
		Err(reasons) => ExpectedDamageSkillEntry::Unsupported(UnsupportedExpectedDamageSkill {
			operator_id: facts.operator_id.clone(),
			skill_id: facts.skill_id.clone(),
			reasons,
		}),
	}
}

/// Small reviewed starter registry. Every mapping is selected by exact skill or prefab ID; the
/// same raw key may intentionally have different meanings in another binding.
#[must_use]
pub fn reviewed_skill_damage_binding(
	facts: &SkillDamageFacts,
) -> Option<SkillDamageTemplateBinding> {
	let prefab: Option<&str> = facts.prefab_id.as_deref();
	let binding: SkillDamageTemplateBinding = match (facts.skill_id.as_str(), prefab) {
		("skcom_magic_rage[3]", Some("skcom_attack_speed_up"))
			if facts.operator_id == "char_002_amiya" =>
		{
			skill_binding(
				"skcom_magic_rage[3]",
				SkillDamageMechanicTemplate::ModifiedNormal,
			)
			.for_operator_id("char_002_amiya")
			.requiring_no_module()
			.with_semantic("attack_speed", SkillDamageSemantic::AttackSpeedBonus)
		}
		("skchr_logos_3", Some("skchr_logos_3")) if facts.operator_id == "char_4133_logos" => {
			skill_binding("skchr_logos_3", SkillDamageMechanicTemplate::ModifiedNormal)
				.for_operator_id("char_4133_logos")
				.requiring_no_module()
				.with_semantic("atk", SkillDamageSemantic::AdditiveAttackRatio)
				.with_semantic(
					"attack@max_target",
					SkillDamageSemantic::MaximumPrimaryTargets,
				)
				.with_ignored_key("projectile_move_scale")
		}
		("skchr_clour_2", Some("skchr_clour_2")) if facts.operator_id == "char_190_clour" => {
			skill_binding("skchr_clour_2", SkillDamageMechanicTemplate::ModifiedNormal)
				.for_operator_id("char_190_clour")
				.requiring_no_module()
				.with_semantic("atk", SkillDamageSemantic::AdditiveAttackRatio)
				.with_semantic(
					"attack@max_target",
					SkillDamageSemantic::MaximumPrimaryTargets,
				)
		}
		("skchr_angel_2", Some("skchr_angel_2")) if facts.operator_id == "char_103_angel" => {
			skill_binding("skchr_angel_2", SkillDamageMechanicTemplate::ModifiedNormal)
				.for_operator_id("char_103_angel")
				.requiring_no_module()
				.with_semantic("attack@atk_scale", SkillDamageSemantic::PrimaryAttackScale)
				.with_semantic("attack@times", SkillDamageSemantic::PrimaryHitCount)
		}
		_ => return None,
	};
	Some(binding)
}

fn skill_binding(
	skill_id: &str,
	template: SkillDamageMechanicTemplate,
) -> SkillDamageTemplateBinding {
	SkillDamageTemplateBinding::new(
		SkillDamageBindingSelector::SkillId(skill_id.to_owned()),
		template,
	)
}

fn compile_facts(
	facts: &SkillDamageFacts,
	tick_rate: TickRate,
	binding: &SkillDamageTemplateBinding,
) -> Result<ExpectedDamageSkillProfile, Vec<SkillDamageUnsupportedReason>> {
	let mut reasons: Vec<SkillDamageUnsupportedReason> = validate_base_facts(facts);
	let entries: BlackboardIndex<'_> = BlackboardIndex::new(&facts.blackboard, &mut reasons);
	if !binding_matches(facts, binding) {
		reasons.push(SkillDamageUnsupportedReason::BindingDoesNotMatch {
			selector: binding.selector.clone(),
		});
	}
	validate_binding_preconditions(facts, binding, &mut reasons);
	let semantics: BoundSemantics = bind_semantics(&entries, binding, &mut reasons);

	let damage_type: Option<ExpectedDamageType> =
		infer_damage_type(&facts.profession, &facts.sub_profession_id, &mut reasons);
	let target_filter: ExpectedTargetFilter = infer_target_filter(&facts.position);
	let base_interval: Option<NonZeroU64> = interval_ticks(
		facts.base_attack_time_seconds,
		facts.base_attack_speed,
		tick_rate,
		&mut reasons,
	);
	let duration: Option<ExpectedSkillDuration> =
		compile_duration(facts, tick_rate, binding.template, &semantics, &mut reasons);

	let additive_attack: f64 = semantics.value(SkillDamageSemantic::AdditiveAttackRatio, 0.0);
	let primary_scale: f64 = semantics.value(SkillDamageSemantic::PrimaryAttackScale, 1.0);
	let alternate_scale: Option<(String, f64)> =
		semantics.optional(SkillDamageSemantic::AlternateAttackScale);
	let probability: Option<(String, f64)> =
		semantics.optional(SkillDamageSemantic::AlternateProbability);
	let attack_speed_bonus: f64 = semantics.value(SkillDamageSemantic::AttackSpeedBonus, 0.0);
	let base_attack_time_delta: f64 =
		semantics.value(SkillDamageSemantic::BaseAttackTimeDeltaSeconds, 0.0);
	let maximum_targets: u32 = semantic_whole_u32(
		&semantics,
		SkillDamageSemantic::MaximumPrimaryTargets,
		1,
		&mut reasons,
	);
	let primary_hit_count: u32 = semantic_whole_u32(
		&semantics,
		SkillDamageSemantic::PrimaryHitCount,
		1,
		&mut reasons,
	);
	if primary_hit_count == 0 {
		reasons.push(SkillDamageUnsupportedReason::TemplateRequirement {
			template: binding.template,
			field: "positive PrimaryHitCount",
		});
	}
	let active_interval: Option<NonZeroU64> = match binding.template {
		SkillDamageMechanicTemplate::PeriodicAoe => {
			let seconds: Option<(String, f64)> =
				semantics.optional(SkillDamageSemantic::PeriodicIntervalSeconds);
			match seconds {
				Some((key, value)) => seconds_to_ticks(value, tick_rate).or_else(|| {
					reasons.push(SkillDamageUnsupportedReason::InvalidModifier { key, value });
					None
				}),
				None => {
					reasons.push(SkillDamageUnsupportedReason::TemplateRequirement {
						template: binding.template,
						field: "PeriodicIntervalSeconds",
					});
					None
				}
			}
		}
		_ => interval_ticks(
			facts.base_attack_time_seconds + base_attack_time_delta,
			facts.base_attack_speed + attack_speed_bonus,
			tick_rate,
			&mut reasons,
		),
	};
	// The same cadence with the attack-speed points left out, so the lowering can separate the
	// skill's base-attack-time change from its attack-speed points and give each its own lane.
	let active_interval_before_attack_speed: Option<NonZeroU64> = match binding.template {
		SkillDamageMechanicTemplate::PeriodicAoe => active_interval,
		_ => interval_ticks(
			facts.base_attack_time_seconds + base_attack_time_delta,
			facts.base_attack_speed,
			tick_rate,
			&mut reasons,
		),
	};
	let forward_extension_tiles: u32 = semantic_whole_u32(
		&semantics,
		SkillDamageSemantic::ForwardRangeExtensionTiles,
		0,
		&mut reasons,
	);
	let delivery: ExpectedDamageDelivery =
		compile_delivery(&semantics, binding.template, &mut reasons);
	let defense_adjustment: ExpectedDefenseAdjustment =
		compile_defense_adjustment(&semantics, &mut reasons);

	if additive_attack < -1.0 {
		reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
			key: "AdditiveAttackRatio".to_owned(),
			value: additive_attack,
		});
	}
	let modified_attack: f64 = facts.base_attack * (1.0 + additive_attack);
	if !modified_attack.is_finite() || modified_attack <= 0.0 {
		reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
			key: "modified_attack".to_owned(),
			value: modified_attack,
		});
	}
	let active_outcomes: ExpectedRawDamageMixture = compile_raw_damage_mixture(
		modified_attack,
		primary_scale,
		alternate_scale,
		probability,
		&mut reasons,
	);

	if !reasons.is_empty() {
		reasons.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
		reasons.dedup();
		return Err(reasons);
	}

	let damage_type: ExpectedDamageType =
		damage_type.expect("absence produces an unsupported reason");
	let base_interval: NonZeroU64 = base_interval.expect("absence produces an unsupported reason");
	let active_interval: NonZeroU64 =
		active_interval.expect("absence produces an unsupported reason");
	let duration: ExpectedSkillDuration = duration.expect("absence produces an unsupported reason");
	let maximum_primary_targets: NonZeroU32 =
		NonZeroU32::new(maximum_targets).expect("target parser rejects zero");
	let primary_hit_count: NonZeroU32 =
		NonZeroU32::new(primary_hit_count).expect("zero hit count produces an unsupported reason");
	Ok(ExpectedDamageSkillProfile {
		operator_id: facts.operator_id.clone(),
		skill_id: facts.skill_id.clone(),
		template: binding.template,
		activation_type: facts.activation_type.clone(),
		sp_type: facts.sp_type.clone(),
		sp_cost: facts.sp_cost,
		initial_sp: facts.initial_sp,
		sp_increment: facts.sp_increment,
		duration,
		range: ExpectedSkillRange {
			range_id: facts
				.skill_range_id
				.as_deref()
				.unwrap_or(&facts.base_range_id)
				.to_owned(),
			forward_extension_tiles,
		},
		key_audit: semantics.audit,
		additive_attack_ratio: additive_attack,
		primary_attack_scale: primary_scale,
		attack_speed_bonus,
		active_interval_before_attack_speed_ticks: active_interval_before_attack_speed
			.expect("absence produces an unsupported reason"),
		base: ExpectedAttackPhase {
			primary_hit_outcomes: ExpectedRawDamageMixture::deterministic(facts.base_attack),
			primary_hit_count: NonZeroU32::MIN,
			damage_type,
			attack_interval_ticks: base_interval,
			maximum_primary_targets: NonZeroU32::MIN,
			target_filter,
			delivery: ExpectedDamageDelivery::Direct,
			defense_adjustment: ExpectedDefenseAdjustment::default(),
		},
		active: ExpectedAttackPhase {
			primary_hit_outcomes: active_outcomes,
			primary_hit_count,
			damage_type,
			attack_interval_ticks: active_interval,
			maximum_primary_targets,
			target_filter,
			delivery,
			defense_adjustment,
		},
	})
}

fn validate_base_facts(facts: &SkillDamageFacts) -> Vec<SkillDamageUnsupportedReason> {
	let fields: [(&'static str, f64); 5] = [
		("base_attack", facts.base_attack),
		("base_attack_time_seconds", facts.base_attack_time_seconds),
		("base_attack_speed", facts.base_attack_speed),
		("duration_seconds", facts.duration_seconds),
		("sp_increment", facts.sp_increment),
	];
	let mut reasons: Vec<SkillDamageUnsupportedReason> = fields
		.into_iter()
		.filter_map(|(field, value)| {
			(!value.is_finite())
				.then_some(SkillDamageUnsupportedReason::NonFiniteBaseField { field })
		})
		.collect();
	if facts.base_attack <= 0.0 {
		reasons.push(SkillDamageUnsupportedReason::NonPositiveBaseAttack);
	}
	if facts.sp_increment <= 0.0 {
		reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
			key: "sp_increment".to_owned(),
			value: facts.sp_increment,
		});
	}
	reasons
}

fn infer_damage_type(
	profession: &str,
	sub_profession_id: &str,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> Option<ExpectedDamageType> {
	if matches!(
		sub_profession_id,
		"bard" | "craftsman" | "healer" | "ringhealer" | "physician" | "wandermedic"
	) || profession == "MEDIC"
	{
		reasons.push(SkillDamageUnsupportedReason::NonAttackingArchetype {
			profession: profession.to_owned(),
			sub_profession_id: sub_profession_id.to_owned(),
		});
		return None;
	}
	let damage_type: ExpectedDamageType = match (profession, sub_profession_id) {
		("CASTER" | "SUPPORT", _) | ("WARRIOR", "artsfghter") | ("TANK", "artsprotector") => {
			ExpectedDamageType::Arts
		}
		("SNIPER" | "WARRIOR" | "TANK" | "PIONEER" | "SPECIAL", _) => ExpectedDamageType::Physical,
		_ => {
			reasons.push(SkillDamageUnsupportedReason::UnknownDamageType {
				profession: profession.to_owned(),
				sub_profession_id: sub_profession_id.to_owned(),
			});
			return None;
		}
	};
	Some(damage_type)
}

fn infer_target_filter(position: &OperatorPosition) -> ExpectedTargetFilter {
	match position {
		OperatorPosition::Melee => ExpectedTargetFilter::GroundOnly,
		OperatorPosition::Ranged | OperatorPosition::All => ExpectedTargetFilter::GroundAndAir,
		OperatorPosition::None | OperatorPosition::Unknown(_) => ExpectedTargetFilter::GroundOnly,
	}
}

fn interval_ticks(
	base_attack_time_seconds: f64,
	attack_speed: f64,
	tick_rate: TickRate,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> Option<NonZeroU64> {
	match deterministic_attack_interval_ticks(base_attack_time_seconds, attack_speed, tick_rate) {
		Ok(value) => Some(value),
		Err(source) => {
			reasons.push(SkillDamageUnsupportedReason::InvalidAttackInterval(source));
			None
		}
	}
}

fn compile_duration(
	facts: &SkillDamageFacts,
	tick_rate: TickRate,
	template: SkillDamageMechanicTemplate,
	semantics: &BoundSemantics,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> Option<ExpectedSkillDuration> {
	if template == SkillDamageMechanicTemplate::InstantPacket {
		return Some(ExpectedSkillDuration::Instant);
	}
	if template == SkillDamageMechanicTemplate::AmmoModifiedNormal {
		if facts.duration_type != SkillDurationType::Ammo {
			reasons.push(SkillDamageUnsupportedReason::UnsupportedDurationType {
				duration_type: facts.duration_type.clone(),
			});
		}
		let attacks: u32 =
			semantic_whole_u32(semantics, SkillDamageSemantic::AmmoCount, 0, reasons);
		let Some(attacks) = NonZeroU32::new(attacks) else {
			reasons.push(SkillDamageUnsupportedReason::TemplateRequirement {
				template,
				field: "AmmoCount",
			});
			return None;
		};
		return Some(ExpectedSkillDuration::Ammo { attacks });
	}
	match &facts.duration_type {
		SkillDurationType::Ammo | SkillDurationType::Unknown(_) => {
			reasons.push(SkillDamageUnsupportedReason::UnsupportedDurationType {
				duration_type: facts.duration_type.clone(),
			});
			None
		}
		SkillDurationType::None => {
			if facts.duration_seconds < 0.0 {
				return Some(ExpectedSkillDuration::Indefinite);
			}
			if facts.duration_seconds == 0.0 {
				return Some(ExpectedSkillDuration::Instant);
			}
			match seconds_to_ticks(facts.duration_seconds, tick_rate) {
				Some(ticks) => Some(ExpectedSkillDuration::Timed { ticks }),
				None => {
					reasons.push(SkillDamageUnsupportedReason::InvalidDuration {
						seconds: facts.duration_seconds,
					});
					None
				}
			}
		}
	}
}

fn seconds_to_ticks(seconds: f64, tick_rate: TickRate) -> Option<NonZeroU64> {
	if !seconds.is_finite() || seconds <= 0.0 {
		return None;
	}
	let scaled: f64 = (seconds * TICKS_NUMERIC_SCALE).round();
	if !(1.0..=(u64::MAX as f64)).contains(&scaled) {
		return None;
	}
	let scaled_seconds: u64 = scaled as u64;
	let numerator: u128 =
		u128::from(scaled_seconds) * u128::from(tick_rate.ticks_per_second().get());
	let denominator: u128 = TICKS_NUMERIC_SCALE as u128;
	let ticks: u128 = numerator.div_ceil(denominator);
	NonZeroU64::new(u64::try_from(ticks).ok()?)
}

struct BlackboardIndex<'a> {
	entries: BTreeMap<&'a str, &'a SkillDamageBlackboardFact>,
}

impl<'a> BlackboardIndex<'a> {
	fn new(
		facts: &'a [SkillDamageBlackboardFact],
		reasons: &mut Vec<SkillDamageUnsupportedReason>,
	) -> Self {
		let mut entries: BTreeMap<&'a str, &'a SkillDamageBlackboardFact> = BTreeMap::new();
		for fact in facts {
			if !fact.value().is_finite() {
				reasons.push(SkillDamageUnsupportedReason::InvalidBlackboardValue {
					key: fact.key().to_owned(),
				});
			}
			if let Some(previous) = entries.insert(fact.key(), fact) {
				if previous.value() != fact.value() {
					reasons.push(SkillDamageUnsupportedReason::ConflictingAliases {
						field: "duplicate blackboard key",
						keys: vec![fact.key().to_owned()],
					});
				}
			}
		}
		Self { entries }
	}
}

struct BoundSemantics {
	values: BTreeMap<SkillDamageSemantic, (String, f64)>,
	audit: SkillDamageKeyAudit,
}

impl BoundSemantics {
	fn value(&self, semantic: SkillDamageSemantic, default: f64) -> f64 {
		self.values
			.get(&semantic)
			.map_or(default, |(_, value)| *value)
	}

	fn optional(&self, semantic: SkillDamageSemantic) -> Option<(String, f64)> {
		self.values.get(&semantic).cloned()
	}
}

fn binding_matches(facts: &SkillDamageFacts, binding: &SkillDamageTemplateBinding) -> bool {
	let selector_matches: bool = match binding.selector() {
		SkillDamageBindingSelector::SkillId(skill_id) => skill_id == &facts.skill_id,
		SkillDamageBindingSelector::PrefabId(prefab_id) => {
			Some(prefab_id.as_str()) == facts.prefab_id.as_deref()
		}
	};
	selector_matches
		&& (binding.allowed_operator_ids().is_empty()
			|| binding.allowed_operator_ids().contains(&facts.operator_id))
}

fn validate_binding_preconditions(
	facts: &SkillDamageFacts,
	binding: &SkillDamageTemplateBinding,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) {
	if binding.require_no_module() && facts.has_selected_module {
		reasons.push(SkillDamageUnsupportedReason::SelectedModuleNotReviewed);
	}
}

fn bind_semantics(
	entries: &BlackboardIndex<'_>,
	binding: &SkillDamageTemplateBinding,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> BoundSemantics {
	let mut values: BTreeMap<SkillDamageSemantic, (String, f64)> = BTreeMap::new();
	let mut consumed_keys: Vec<String> = Vec::new();
	let mut ignored_keys: Vec<String> = Vec::new();
	for entry in entries.entries.values() {
		if let Some(semantic) = binding.semantics().get(entry.key()) {
			consumed_keys.push(entry.key().to_owned());
			if entry.value_string().is_some() {
				reasons.push(SkillDamageUnsupportedReason::StringValuedDamageMechanic {
					key: entry.key().to_owned(),
				});
			}
			if let Some((previous_key, _)) =
				values.insert(*semantic, (entry.key().to_owned(), entry.value()))
			{
				reasons.push(SkillDamageUnsupportedReason::ConflictingAliases {
					field: "bound semantic",
					keys: vec![previous_key, entry.key().to_owned()],
				});
			}
			continue;
		}

		ignored_keys.push(entry.key().to_owned());
		if is_damage_suspicious(entry.key()) && !binding.ignored_keys().contains(entry.key()) {
			reasons.push(SkillDamageUnsupportedReason::UnknownDamageMechanic {
				key: entry.key().to_owned(),
			});
		}
	}
	BoundSemantics {
		values,
		audit: SkillDamageKeyAudit {
			consumed_keys,
			ignored_keys,
		},
	}
}

fn compile_raw_damage_mixture(
	modified_attack: f64,
	primary: f64,
	alternate: Option<(String, f64)>,
	probability: Option<(String, f64)>,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> ExpectedRawDamageMixture {
	if !primary.is_finite() || primary <= 0.0 {
		reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
			key: "attack scale".to_owned(),
			value: primary,
		});
	}
	let primary_raw_damage: f64 = modified_attack * primary;
	if !primary_raw_damage.is_finite() || primary_raw_damage <= 0.0 {
		reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
			key: "primary raw damage".to_owned(),
			value: primary_raw_damage,
		});
	}
	match (alternate, probability) {
		(Some((scale_key, alternate)), Some((probability_key, probability))) => {
			let mut valid: bool = true;
			if !alternate.is_finite() || alternate <= 0.0 {
				reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
					key: scale_key,
					value: alternate,
				});
				valid = false;
			}
			if !(0.0..=1.0).contains(&probability) {
				reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
					key: probability_key,
					value: probability,
				});
				valid = false;
			}
			let alternate_raw_damage: f64 = modified_attack * alternate;
			if !alternate_raw_damage.is_finite() || alternate_raw_damage <= 0.0 {
				reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
					key: "alternate raw damage".to_owned(),
					value: alternate_raw_damage,
				});
				valid = false;
			}
			if valid {
				ExpectedRawDamageMixture::from_binary(
					primary_raw_damage,
					alternate_raw_damage,
					probability,
				)
			} else {
				ExpectedRawDamageMixture::deterministic(primary_raw_damage)
			}
		}
		(Some((key, _)), None) => {
			reasons.push(SkillDamageUnsupportedReason::UnknownDamageMechanic { key });
			ExpectedRawDamageMixture::deterministic(primary_raw_damage)
		}
		(None, Some((key, _))) => {
			reasons.push(SkillDamageUnsupportedReason::UnknownDamageMechanic { key });
			ExpectedRawDamageMixture::deterministic(primary_raw_damage)
		}
		(None, None) => ExpectedRawDamageMixture::deterministic(primary_raw_damage),
	}
}

fn semantic_whole_u32(
	semantics: &BoundSemantics,
	semantic: SkillDamageSemantic,
	default: u32,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> u32 {
	let Some((key, value)) = semantics.optional(semantic) else {
		return default;
	};
	if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
		reasons.push(SkillDamageUnsupportedReason::InvalidModifier { key, value });
		return default;
	}
	value as u32
}

fn compile_delivery(
	semantics: &BoundSemantics,
	template: SkillDamageMechanicTemplate,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> ExpectedDamageDelivery {
	let chain_targets: Option<(String, f64)> =
		semantics.optional(SkillDamageSemantic::ChainMaximumTargets);
	let chain_retention: Option<(String, f64)> =
		semantics.optional(SkillDamageSemantic::ChainDamageRetention);
	if chain_targets.is_some() || chain_retention.is_some() {
		let targets: u32 = semantic_whole_u32(
			semantics,
			SkillDamageSemantic::ChainMaximumTargets,
			0,
			reasons,
		);
		let retention: f64 = chain_retention.map_or(1.0, |(_, value)| value);
		if targets == 0 || !(0.0..=1.0).contains(&retention) {
			reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
				key: "ChainDamageRetention".to_owned(),
				value: retention,
			});
			return ExpectedDamageDelivery::Direct;
		}
		return ExpectedDamageDelivery::Chain {
			maximum_targets: NonZeroU32::new(targets).expect("validated nonzero"),
			damage_retention_per_jump: retention,
		};
	}
	let radius: Option<(String, f64)> = semantics.optional(SkillDamageSemantic::SplashRadiusTiles);
	let secondary_scale: Option<(String, f64)> =
		semantics.optional(SkillDamageSemantic::SplashSecondaryDamageScale);
	if radius.is_some() || secondary_scale.is_some() {
		let radius_tiles: f64 = radius.map_or(0.0, |(_, value)| value);
		let secondary_damage_scale: f64 = secondary_scale.map_or_else(
			|| {
				if template == SkillDamageMechanicTemplate::PeriodicAoe
					|| template == SkillDamageMechanicTemplate::InstantPacket
				{
					1.0
				} else {
					0.0
				}
			},
			|(_, value)| value,
		);
		if radius_tiles <= 0.0 || secondary_damage_scale <= 0.0 {
			reasons.push(SkillDamageUnsupportedReason::UnknownDamageMechanic {
				key: "incomplete splash pair".to_owned(),
			});
			return ExpectedDamageDelivery::Direct;
		}
		return ExpectedDamageDelivery::Splash {
			radius_tiles,
			secondary_damage_scale,
		};
	}
	if template == SkillDamageMechanicTemplate::PeriodicAoe {
		reasons.push(SkillDamageUnsupportedReason::TemplateRequirement {
			template,
			field: "SplashRadiusTiles",
		});
	}
	ExpectedDamageDelivery::Direct
}

fn compile_defense_adjustment(
	semantics: &BoundSemantics,
	reasons: &mut Vec<SkillDamageUnsupportedReason>,
) -> ExpectedDefenseAdjustment {
	let physical_ratio: f64 =
		semantics.value(SkillDamageSemantic::PhysicalDefenseReductionRatio, 0.0);
	let physical_fixed: f64 =
		semantics.value(SkillDamageSemantic::PhysicalDefenseReductionFixed, 0.0);
	let arts_ratio: f64 = semantics.value(SkillDamageSemantic::ArtsResistanceReductionRatio, 0.0);
	let arts_fixed: f64 = semantics.value(SkillDamageSemantic::ArtsResistanceReductionFixed, 0.0);
	for (key, value, maximum) in [
		("def_penetrate", physical_ratio, 1.0),
		("def_penetrate_fixed", physical_fixed, f64::INFINITY),
		("magic_resist_penetrate", arts_ratio, 1.0),
		("magic_resist_penetrate_fixed", arts_fixed, f64::INFINITY),
	] {
		if value < 0.0 || value > maximum {
			reasons.push(SkillDamageUnsupportedReason::InvalidModifier {
				key: key.to_owned(),
				value,
			});
		}
	}
	ExpectedDefenseAdjustment {
		physical_defense_reduction_ratio: physical_ratio,
		physical_defense_reduction_fixed: physical_fixed,
		arts_resistance_reduction_ratio: arts_ratio,
		arts_resistance_reduction_fixed: arts_fixed,
	}
}

fn is_damage_suspicious(key: &str) -> bool {
	[
		"atk",
		"damage",
		"target",
		"attack_speed",
		"base_attack_time",
		"interval",
		"times",
		"hit",
		"chain",
		"aoe",
		"radius",
		"prob",
		"crit",
		"penetrate",
		"element",
	]
	.iter()
	.any(|needle| key.contains(needle))
}
