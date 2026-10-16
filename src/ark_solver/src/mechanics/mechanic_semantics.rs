//! Typed, operator-independent mechanic semantics lowered into replay primitives.
//!
//! Prefab decoders produce these rules only after their source schema and references have been
//! verified. Blackboard values remain named expressions until this module binds them against the
//! exact selected talent or skill candidate.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{BlackboardEntry, GridPosition, RangeOffset};
use crate::simulation::TickRate;

use crate::mechanics::{
	combat::{
		ActorCondition, AttackSpeedModifier, BuffModifier, BuffRemovalSource,
		ConditionalAuraPeriodicEffect, ConditionalStatModifier, EffectTarget, ExpectedEffect,
		ExpectedValue, MechanicBehavior, MechanicId, PeriodicTileEffect, ReachedMechanic,
		ReplayCellSet, ReplayListener, ReplayPredicate, ReplayRepeat, ReplayTrigger, RuntimeBuff,
		RuntimeCachedTileLayerPropagation, RuntimeConditionalAura, RuntimeEffectLifetime,
		RuntimeSkillInactiveStance, RuntimeTileEffect, RuntimeTileFieldOverlay,
		RuntimeTileLayerThresholdReaction, SkillManualDiscardActivationLimit, StatKind,
		StatModifier, StatModifierBucket, StatusEffect, StatusKind, SummonTemplateId, TaggedEffect,
		TargetCellActorSpawn, TargetCellSpawnCollision, TargetCellSpawnOwnership,
		TargetCountModifier, TeamCompositionCondition, TileFieldOverlayLeaveScope,
		TileLayerThresholdReactionEffect, TileLayerThresholdTransition, TriggerActorFilter,
	},
	normal_attack::{FixedDamage, NormalAttackTargetFilter},
	tile_layer_propagation::{ContributionMergePolicy, PulseSourceOrder},
};

/// Exact serialized object location for battle-prefab provenance.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticSourceLocation {
	pub logical_path: String,
	pub cab_name: String,
	pub path_id: i64,
	pub field_paths: Vec<String>,
}

/// Exact table record and typed-projection evidence for gamedata provenance.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GamedataSemanticSourceLocation {
	pub table_path: String,
	pub record_key: String,
	/// Paths that exist in the verified source table.
	pub field_paths: Vec<String>,
	/// Shape checks exposed by the resolved typed projection but absent from raw serialized data.
	pub resolved_projection_fields: Vec<String>,
}

/// Evidence attached to one semantic rule without fabricating provenance from another source kind.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticProvenance {
	BattlePrefab {
		mechanics_revision: String,
		gamedata_revision: String,
		script_id: [u8; 16],
		old_type_hash: [u8; 16],
		source: SemanticSourceLocation,
	},
	Gamedata {
		gamedata_revision: String,
		decoder_rule_revision: String,
		source: GamedataSemanticSourceLocation,
	},
}

impl SemanticProvenance {
	#[must_use]
	pub fn origin(&self) -> String {
		match self {
			Self::BattlePrefab { source, .. } => format!(
				"verified-prefab:{}:{}:{}",
				source.logical_path, source.cab_name, source.path_id
			),
			Self::Gamedata { source, .. } => format!(
				"verified-gamedata:{}:{}",
				source.table_path, source.record_key
			),
		}
	}

	#[must_use]
	pub fn field_paths(&self) -> &[String] {
		match self {
			Self::BattlePrefab { source, .. } => &source.field_paths,
			Self::Gamedata { source, .. } => &source.field_paths,
		}
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticValueExpression {
	Literal(ExpectedValue),
	Blackboard(String),
	/// Magnitude of a blackboard value that must be finite and non-positive.
	NegatedBlackboard(String),
	OnePlus(Box<Self>),
	OneMinus(Box<Self>),
	Product(Box<Self>, Box<Self>),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticEvent {
	BattleStarted,
	Deploy,
	SkillStarted,
	SkillEnded,
	NormalAttackHit,
	Heal,
	Damage,
	BeforeLethal,
	Death,
	Periodic {
		initial_delay_seconds: SemanticValueExpression,
		interval_seconds: SemanticValueExpression,
	},
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticTriggerFilter {
	AffectedOwner,
	EffectSourceOwner,
	AnyAffected,
	AllyAffected,
	EnemyAffected,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticTrigger {
	pub event: SemanticEvent,
	pub filter: SemanticTriggerFilter,
	pub repeat: ReplayRepeat,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticActorFilter {
	Owner,
	Any,
	AllyOfOwner,
	EnemyOfOwner,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticLifetime {
	Persistent,
	OwnerDeployment,
	SkillActivation,
}

/// Lifecycle flags decoded from a verified generic skill-controller schema.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticSkillControllerLifecycle {
	pub manual_discard_limit: SkillManualDiscardActivationLimit,
	pub allow_manual_discard_while_dozing: bool,
	pub allow_sp_recovery_while_active: bool,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticBuffModifier {
	StatMultiplier {
		stat: StatKind,
		/// Which bucket the multiplier resolves in. A value the source builds as `1 + ratio` from
		/// a percentage blackboard key is a panel percentage; one read straight from a scale key
		/// is its own bucket.
		bucket: StatModifierBucket,
		multiplier: SemanticValueExpression,
	},
	/// Additive Arts-resistance points represented as a ratio (`20` points is `0.20`).
	ArtsResistanceAddition {
		ratio: SemanticValueExpression,
	},
	Shelter {
		incoming_damage_multiplier: SemanticValueExpression,
	},
	MovementSpeedReduction {
		reduction: SemanticValueExpression,
	},
	/// Non-negative additive attack-speed points.
	AttackSpeedAddition {
		points: SemanticValueExpression,
	},
	TargetCountUnlimited,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticResidentBuff {
	pub key_suffix: String,
	pub max_stacks: SemanticValueExpression,
	pub modifiers_per_stack: Vec<SemanticBuffModifier>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticEffect {
	Healing {
		amount: SemanticValueExpression,
	},
	Damage {
		amount: SemanticValueExpression,
		damage_type: crate::mechanics::combat::DamageType,
		delivery: crate::mechanics::combat::DamageDelivery,
	},
	SourceStatScaledDamage {
		stat: StatKind,
		multiplier: SemanticValueExpression,
		damage_type: crate::mechanics::combat::DamageType,
		delivery: crate::mechanics::combat::DamageDelivery,
	},
	/// Restore HP directly without emitting a Heal trigger.
	RestoreHpToCurrentMaxRatio {
		ratio: SemanticValueExpression,
	},
	/// Apply a standard battle status for a blackboard-bound duration.
	ApplyStatus {
		kind: StatusKind,
		duration_seconds: SemanticValueExpression,
	},
	GainSp {
		amount: SemanticValueExpression,
	},
	ApplyBuff {
		key: MechanicId,
		max_stacks: SemanticValueExpression,
		lifetime: SemanticLifetime,
		modifiers_per_stack: Vec<SemanticBuffModifier>,
	},
	/// A stat multiplier on the target that counts only while the target satisfies `condition`.
	ConditionalStatMultiplier {
		condition: SemanticActorCondition,
		stat: StatKind,
		bucket: StatModifierBucket,
		multiplier: SemanticValueExpression,
	},
}

/// A predicate over the live battle state of the actor an effect is applied to.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticActorCondition {
	/// The actor currently blocks at least `count` enemies, counted one per blocking relation.
	BlockedEnemiesAtLeast { count: SemanticValueExpression },
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticEffectTarget {
	/// The actor affected by the trigger.
	TriggerActor,
	/// The actor that caused the trigger. Only valid while resolving a listener event.
	TriggerEffectSource,
	/// The mechanic owner that registered the listener.
	Owner,
	/// Enemies in the owner's effective attack range when the effect resolves.
	EnemiesInOwnerCurrentAttackRange,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticTargetedEffect {
	pub target: SemanticEffectTarget,
	pub effect: SemanticEffect,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticPeriodicEffect {
	pub initial_delay_millis: u64,
	pub interval_millis: u64,
	pub effects_per_layer: Vec<SemanticEffect>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticLayerThresholdTransition {
	ReachedFromBelow,
	SaturatedAttempt,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticTargetCellSpawnCollision {
	SuppressIfFriendlyOccupied,
	AllowCoexistence,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticTargetCellSpawnOwnership {
	Independent,
	SourceActor,
	SkillActivation,
	TriggeringTileEffect,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticTargetCellActorSpawn {
	pub actor_template_key: String,
	pub collision: SemanticTargetCellSpawnCollision,
	pub ownership: SemanticTargetCellSpawnOwnership,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticLayerThresholdReactionEffect {
	SpawnActor(SemanticTargetCellActorSpawn),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticLayerThresholdReaction {
	pub threshold: SemanticValueExpression,
	pub transition: SemanticLayerThresholdTransition,
	pub effect: SemanticLayerThresholdReactionEffect,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticTileField {
	pub key: MechanicId,
	pub cells: SemanticCellSet,
	pub actor_filter: SemanticActorFilter,
	pub target_filter: NormalAttackTargetFilter,
	pub max_layers_per_cell: SemanticValueExpression,
	pub lifetime: SemanticLifetime,
	pub on_enter_once: Vec<SemanticEffect>,
	pub resident_buffs: Vec<SemanticResidentBuff>,
	pub layer_threshold_reactions: Vec<SemanticLayerThresholdReaction>,
	pub periodic: Option<SemanticPeriodicEffect>,
	pub remove_on_first_matching_leave: bool,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticTileFieldOverlay {
	pub key: MechanicId,
	pub observed_tile_key: MechanicId,
	pub observed_tile_max_layers: Option<SemanticValueExpression>,
	pub actor_filter: SemanticActorFilter,
	pub target_filter: NormalAttackTargetFilter,
	pub leave_scope: TileFieldOverlayLeaveScope,
	pub initial_delay_seconds: SemanticValueExpression,
	pub interval_seconds: SemanticValueExpression,
	pub lifetime: SemanticLifetime,
	pub periodic_effects: Vec<SemanticEffect>,
	pub on_leave_once: Vec<SemanticEffect>,
	pub layer_threshold_reactions: Vec<SemanticLayerThresholdReaction>,
	pub cached_propagation: Option<SemanticCachedTileLayerPropagation>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticCachedTileLayerPropagation {
	pub maximum_outside_temporary_cells: SemanticValueExpression,
	pub source_order: PulseSourceOrder<GridPosition>,
	pub contribution_merge: ContributionMergePolicy,
}

/// One owner-deployment stance applied exactly while the owner's skill is inactive.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticSkillInactiveStance {
	pub key: MechanicId,
	pub disable_normal_attacks: bool,
	pub modifiers: Vec<SemanticBuffModifier>,
}

/// A condition evaluated over either currently deployed operators or the fixed guide roster.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticTeamCompositionCondition {
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
pub struct SemanticConditionalAuraPeriodicEffect {
	pub initial_delay_seconds: SemanticValueExpression,
	pub interval_seconds: SemanticValueExpression,
	pub effects: Vec<SemanticEffect>,
}

/// One source-owned aura whose target buff follows a typed team-composition condition.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticConditionalAura {
	pub key: MechanicId,
	pub condition: SemanticTeamCompositionCondition,
	pub target_profession_mask: u16,
	pub includes_source: bool,
	pub lifetime: SemanticLifetime,
	pub modifiers: Vec<SemanticBuffModifier>,
	pub periodic: Option<SemanticConditionalAuraPeriodicEffect>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticCellSet {
	/// Offsets centered on the actor affected by the triggering event.
	TriggerActorRelative(Vec<RangeOffset>),
	/// Lowland cells in the source actor's effective attack range at application time.
	SourceCurrentAttackRangeGroundCells,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticAction {
	AddTileField(SemanticTileField),
	InstallTileFieldOverlay(SemanticTileFieldOverlay),
	InstallSkillInactiveStance(SemanticSkillInactiveStance),
	InstallConditionalAura(SemanticConditionalAura),
	ApplyEffects(Vec<SemanticTargetedEffect>),
	Repeat {
		count: SemanticValueExpression,
		action: Box<Self>,
	},
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicSemanticRule {
	pub mechanic: MechanicId,
	pub provenance: SemanticProvenance,
	pub trigger: SemanticTrigger,
	pub lifetime: SemanticLifetime,
	pub action: SemanticAction,
	pub behavior: MechanicBehavior,
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicSemanticProgram {
	pub rules: Vec<MechanicSemanticRule>,
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LoweredMechanicSemanticProgram {
	pub on_battle_start: Vec<TaggedEffect>,
	pub on_deploy: Vec<TaggedEffect>,
	pub reached_mechanics: Vec<ReachedMechanic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MechanicSemanticError {
	EmptyMechanicId,
	EmptySourceFieldPaths {
		mechanic: String,
	},
	MissingBlackboardValue {
		mechanic: String,
		key: String,
	},
	DuplicateBlackboardValue {
		mechanic: String,
		key: String,
	},
	InvalidBlackboardValue {
		mechanic: String,
		key: String,
	},
	ArithmeticOverflow {
		mechanic: String,
	},
	InvalidTileField {
		mechanic: String,
		detail: &'static str,
	},
	MissingActorTemplate {
		mechanic: String,
		key: String,
	},
	InvalidLayerThresholdReaction {
		mechanic: String,
		detail: &'static str,
	},
	InvalidTileFieldOverlay {
		mechanic: String,
		detail: &'static str,
	},
	InvalidBuffModifier {
		mechanic: String,
		detail: &'static str,
	},
	InvalidSkillInactiveStance {
		mechanic: String,
		detail: &'static str,
	},
	InvalidConditionalAura {
		mechanic: String,
		detail: &'static str,
	},
	InvalidDuration {
		mechanic: String,
		field: &'static str,
	},
	InvalidStatus {
		mechanic: String,
		detail: &'static str,
	},
	InvalidEffect {
		mechanic: String,
		detail: &'static str,
	},
}

impl fmt::Display for MechanicSemanticError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::EmptyMechanicId => formatter.write_str("mechanic semantic rule has an empty ID"),
			Self::EmptySourceFieldPaths { mechanic } => write!(
				formatter,
				"mechanic {mechanic:?} has no source field-path provenance"
			),
			Self::MissingBlackboardValue { mechanic, key } => write!(
				formatter,
				"mechanic {mechanic:?} requires missing blackboard value {key:?}"
			),
			Self::DuplicateBlackboardValue { mechanic, key } => write!(
				formatter,
				"mechanic {mechanic:?} has duplicate blackboard value {key:?}"
			),
			Self::InvalidBlackboardValue { mechanic, key } => write!(
				formatter,
				"mechanic {mechanic:?} has invalid blackboard value {key:?}"
			),
			Self::ArithmeticOverflow { mechanic } => {
				write!(
					formatter,
					"mechanic {mechanic:?} value expression overflowed"
				)
			}
			Self::InvalidTileField { mechanic, detail } => {
				write!(
					formatter,
					"mechanic {mechanic:?} has invalid tile field: {detail}"
				)
			}
			Self::MissingActorTemplate { mechanic, key } => write!(
				formatter,
				"mechanic {mechanic:?} references missing actor template {key:?}"
			),
			Self::InvalidLayerThresholdReaction { mechanic, detail } => write!(
				formatter,
				"mechanic {mechanic:?} has an invalid layer-threshold reaction: {detail}"
			),
			Self::InvalidTileFieldOverlay { mechanic, detail } => {
				write!(
					formatter,
					"mechanic {mechanic:?} has an invalid tile-field overlay: {detail}"
				)
			}
			Self::InvalidBuffModifier { mechanic, detail } => write!(
				formatter,
				"mechanic {mechanic:?} has an invalid buff modifier: {detail}"
			),
			Self::InvalidSkillInactiveStance { mechanic, detail } => write!(
				formatter,
				"mechanic {mechanic:?} has an invalid skill-inactive stance: {detail}"
			),
			Self::InvalidConditionalAura { mechanic, detail } => write!(
				formatter,
				"mechanic {mechanic:?} has an invalid conditional aura: {detail}"
			),
			Self::InvalidDuration { mechanic, field } => write!(
				formatter,
				"mechanic {mechanic:?} has a non-integral or overflowing {field}"
			),
			Self::InvalidStatus { mechanic, detail } => {
				write!(
					formatter,
					"mechanic {mechanic:?} has an invalid status: {detail}"
				)
			}
			Self::InvalidEffect { mechanic, detail } => {
				write!(
					formatter,
					"mechanic {mechanic:?} has an invalid effect: {detail}"
				)
			}
		}
	}
}

impl Error for MechanicSemanticError {}

impl MechanicSemanticProgram {
	pub fn lower(
		&self,
		blackboard: &[BlackboardEntry],
		tick_rate: TickRate,
	) -> Result<LoweredMechanicSemanticProgram, MechanicSemanticError> {
		self.lower_with_actor_templates(blackboard, tick_rate, &BTreeMap::new())
	}

	pub fn lower_with_actor_templates(
		&self,
		blackboard: &[BlackboardEntry],
		tick_rate: TickRate,
		actor_templates: &BTreeMap<String, SummonTemplateId>,
	) -> Result<LoweredMechanicSemanticProgram, MechanicSemanticError> {
		let mut lowered: LoweredMechanicSemanticProgram = LoweredMechanicSemanticProgram::default();
		for rule in &self.rules {
			let mechanic_id: &str = rule.mechanic.as_str();
			if mechanic_id.trim().is_empty() {
				return Err(MechanicSemanticError::EmptyMechanicId);
			}
			if rule.provenance.field_paths().is_empty() {
				return Err(MechanicSemanticError::EmptySourceFieldPaths {
					mechanic: mechanic_id.to_owned(),
				});
			}
			let battle_started: bool = matches!(rule.trigger.event, SemanticEvent::BattleStarted);
			if battle_started {
				if rule.lifetime != SemanticLifetime::Persistent {
					return Err(MechanicSemanticError::InvalidEffect {
						mechanic: mechanic_id.to_owned(),
						detail: "battle-start rule lifetime must be persistent",
					});
				}
				if rule.trigger.filter != SemanticTriggerFilter::AffectedOwner
					|| rule.trigger.repeat != ReplayRepeat::Once
				{
					return Err(MechanicSemanticError::InvalidEffect {
						mechanic: mechanic_id.to_owned(),
						detail: "battle-start rule must target the affected owner exactly once",
					});
				}
			}
			let effects: Vec<TaggedEffect> =
				lower_action(rule, &rule.action, blackboard, tick_rate, actor_templates)?;
			if battle_started {
				lowered.on_battle_start.extend(effects);
			} else {
				let listener: ReplayListener = ReplayListener {
					key: rule.mechanic.clone(),
					trigger: lower_trigger_event(rule, &rule.trigger.event, blackboard, tick_rate)?,
					actor_filter: lower_trigger_filter(rule.trigger.filter),
					predicate: ReplayPredicate::Always,
					repeat: rule.trigger.repeat,
					lifetime: lower_lifetime(rule.lifetime),
					effects,
				};
				lowered.on_deploy.push(TaggedEffect {
					mechanic: rule.mechanic.clone(),
					target: EffectTarget::SelfActor,
					effect: ExpectedEffect::RegisterListener(listener),
				});
			}
			lowered.reached_mechanics.push(ReachedMechanic {
				id: rule.mechanic.clone(),
				origin: rule.provenance.origin(),
				behavior: rule.behavior.clone(),
			});
		}
		Ok(lowered)
	}
}

fn lower_action(
	rule: &MechanicSemanticRule,
	action: &SemanticAction,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
	actor_templates: &BTreeMap<String, SummonTemplateId>,
) -> Result<Vec<TaggedEffect>, MechanicSemanticError> {
	match action {
		SemanticAction::AddTileField(field) => Ok(vec![lower_tile_field(
			rule,
			field,
			blackboard,
			tick_rate,
			actor_templates,
		)?]),
		SemanticAction::InstallTileFieldOverlay(overlay) => Ok(vec![lower_tile_field_overlay(
			rule,
			overlay,
			blackboard,
			tick_rate,
			actor_templates,
		)?]),
		SemanticAction::InstallSkillInactiveStance(stance) => {
			Ok(vec![lower_skill_inactive_stance(rule, stance, blackboard)?])
		}
		SemanticAction::InstallConditionalAura(aura) => Ok(vec![lower_conditional_aura(
			rule, aura, blackboard, tick_rate,
		)?]),
		SemanticAction::ApplyEffects(effects) => {
			if effects.is_empty() {
				return Err(MechanicSemanticError::InvalidEffect {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "targeted effect list must not be empty",
				});
			}
			effects
				.iter()
				.map(|effect| lower_targeted_effect(rule, effect, blackboard, tick_rate))
				.collect()
		}
		SemanticAction::Repeat { count, action } => {
			let count: u16 = resolve_exact_u16(
				count,
				blackboard,
				rule.mechanic.as_str(),
				"repeat count",
				true,
			)?;
			let nested: Vec<TaggedEffect> =
				lower_action(rule, action, blackboard, tick_rate, actor_templates)?;
			let capacity: usize =
				nested
					.len()
					.checked_mul(usize::from(count))
					.ok_or_else(|| MechanicSemanticError::ArithmeticOverflow {
						mechanic: rule.mechanic.as_str().to_owned(),
					})?;
			let mut effects: Vec<TaggedEffect> = Vec::with_capacity(capacity);
			for _ in 0..count {
				effects.extend(nested.iter().cloned());
			}
			Ok(effects)
		}
	}
}

fn lower_skill_inactive_stance(
	rule: &MechanicSemanticRule,
	stance: &SemanticSkillInactiveStance,
	blackboard: &[BlackboardEntry],
) -> Result<TaggedEffect, MechanicSemanticError> {
	let mechanic: String = rule.mechanic.as_str().to_owned();
	if stance.key.as_str().trim().is_empty() {
		return Err(MechanicSemanticError::InvalidSkillInactiveStance {
			mechanic,
			detail: "stance key must not be empty",
		});
	}
	if !stance.disable_normal_attacks && stance.modifiers.is_empty() {
		return Err(MechanicSemanticError::InvalidSkillInactiveStance {
			mechanic,
			detail: "stance must change at least one behavior",
		});
	}
	let modifiers: Vec<BuffModifier> = stance
		.modifiers
		.iter()
		.map(|modifier: &SemanticBuffModifier| lower_buff_modifier(rule, modifier, blackboard))
		.collect::<Result<_, _>>()?;
	Ok(TaggedEffect {
		mechanic: rule.mechanic.clone(),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::InstallSkillInactiveStance(RuntimeSkillInactiveStance {
			key: stance.key.clone(),
			disable_normal_attacks: stance.disable_normal_attacks,
			modifiers,
		}),
	})
}

fn lower_conditional_aura(
	rule: &MechanicSemanticRule,
	aura: &SemanticConditionalAura,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
) -> Result<TaggedEffect, MechanicSemanticError> {
	let mechanic: String = rule.mechanic.as_str().to_owned();
	if aura.key.as_str().trim().is_empty() {
		return Err(MechanicSemanticError::InvalidConditionalAura {
			mechanic,
			detail: "aura key must not be empty",
		});
	}
	if !matches!(
		aura.lifetime,
		SemanticLifetime::Persistent | SemanticLifetime::OwnerDeployment
	) {
		return Err(MechanicSemanticError::InvalidConditionalAura {
			mechanic,
			detail: "aura lifetime must be persistent or one owner deployment",
		});
	}
	if aura.lifetime != rule.lifetime {
		return Err(MechanicSemanticError::InvalidConditionalAura {
			mechanic,
			detail: "aura lifetime must match its semantic rule lifetime",
		});
	}
	if aura.target_profession_mask == 0 {
		return Err(MechanicSemanticError::InvalidConditionalAura {
			mechanic,
			detail: "target profession mask must not be zero",
		});
	}
	if aura.modifiers.is_empty() {
		return Err(MechanicSemanticError::InvalidConditionalAura {
			mechanic,
			detail: "aura modifier list must not be empty",
		});
	}
	let condition: TeamCompositionCondition = match &aura.condition {
		SemanticTeamCompositionCondition::DeployedDistinctProfessions {
			minimum_count,
			profession_mask,
		} => {
			validate_team_condition(*minimum_count, *profession_mask, &mechanic)?;
			TeamCompositionCondition::DeployedDistinctProfessions {
				minimum_count: *minimum_count,
				profession_mask: *profession_mask,
			}
		}
		SemanticTeamCompositionCondition::DeployedSameProfession {
			minimum_count,
			profession_mask,
		} => {
			validate_team_condition(*minimum_count, *profession_mask, &mechanic)?;
			TeamCompositionCondition::DeployedSameProfession {
				minimum_count: *minimum_count,
				profession_mask: *profession_mask,
			}
		}
		SemanticTeamCompositionCondition::FixedRosterGroup {
			minimum_count,
			group_id,
		} => {
			if *minimum_count == 0 || group_id.trim().is_empty() {
				return Err(MechanicSemanticError::InvalidConditionalAura {
					mechanic,
					detail: "fixed-roster group condition requires a positive count and group ID",
				});
			}
			TeamCompositionCondition::FixedRosterGroup {
				minimum_count: *minimum_count,
				group_id: group_id.clone(),
			}
		}
	};
	let modifiers_per_stack: Vec<BuffModifier> = aura
		.modifiers
		.iter()
		.map(|modifier: &SemanticBuffModifier| lower_buff_modifier(rule, modifier, blackboard))
		.collect::<Result<_, _>>()?;
	let periodic: Option<ConditionalAuraPeriodicEffect> = aura
		.periodic
		.as_ref()
		.map(|periodic: &SemanticConditionalAuraPeriodicEffect| {
			if periodic.effects.is_empty() {
				return Err(MechanicSemanticError::InvalidConditionalAura {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "periodic aura effect list must not be empty",
				});
			}
			let initial_delay_ticks: u64 = resolve_duration_ticks(
				&periodic.initial_delay_seconds,
				blackboard,
				rule.mechanic.as_str(),
				"conditional aura initial delay",
				tick_rate,
				false,
			)?;
			let interval_ticks: u64 = resolve_duration_ticks(
				&periodic.interval_seconds,
				blackboard,
				rule.mechanic.as_str(),
				"conditional aura interval",
				tick_rate,
				false,
			)?;
			let effects: Vec<TaggedEffect> = periodic
				.effects
				.iter()
				.map(|effect: &SemanticEffect| {
					lower_semantic_effect(rule, effect, blackboard, tick_rate)
				})
				.collect::<Result<_, _>>()?;
			Ok(ConditionalAuraPeriodicEffect {
				initial_delay_ticks,
				interval_ticks,
				effects,
			})
		})
		.transpose()?;
	Ok(TaggedEffect {
		mechanic: rule.mechanic.clone(),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::InstallConditionalAura(RuntimeConditionalAura {
			key: aura.key.clone(),
			condition,
			target_profession_mask: aura.target_profession_mask,
			includes_source: aura.includes_source,
			lifetime: lower_lifetime(aura.lifetime),
			buff: RuntimeBuff {
				key: aura.key.clone(),
				max_stacks: 1,
				duration_ticks: None,
				lifetime: lower_lifetime(aura.lifetime),
				modifiers_per_stack,
			},
			periodic,
		}),
	})
}

fn validate_team_condition(
	minimum_count: u16,
	profession_mask: u16,
	mechanic: &str,
) -> Result<(), MechanicSemanticError> {
	if minimum_count == 0 || profession_mask == 0 {
		return Err(MechanicSemanticError::InvalidConditionalAura {
			mechanic: mechanic.to_owned(),
			detail: "deployed-composition condition requires a positive count and profession mask",
		});
	}
	Ok(())
}

fn lower_buff_modifier(
	rule: &MechanicSemanticRule,
	modifier: &SemanticBuffModifier,
	blackboard: &[BlackboardEntry],
) -> Result<BuffModifier, MechanicSemanticError> {
	match modifier {
		SemanticBuffModifier::StatMultiplier {
			stat,
			bucket,
			multiplier,
		} => Ok(BuffModifier::Stat(StatModifier {
			bucket: *bucket,
			stat: *stat,
			multiplier: resolve_value_expression(multiplier, blackboard, rule.mechanic.as_str())?,
			duration_ticks: None,
		})),
		SemanticBuffModifier::ArtsResistanceAddition { ratio } => {
			Ok(BuffModifier::ArtsResistanceAddition {
				ratio: resolve_value_expression(ratio, blackboard, rule.mechanic.as_str())?,
			})
		}
		SemanticBuffModifier::Shelter {
			incoming_damage_multiplier,
		} => Ok(BuffModifier::Shelter {
			incoming_damage_multiplier: resolve_value_expression(
				incoming_damage_multiplier,
				blackboard,
				rule.mechanic.as_str(),
			)?,
		}),
		SemanticBuffModifier::MovementSpeedReduction { reduction } => {
			let reduction: ExpectedValue =
				resolve_value_expression(reduction, blackboard, rule.mechanic.as_str())?;
			if reduction >= ExpectedValue::ONE {
				return Err(MechanicSemanticError::InvalidBuffModifier {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "movement-speed reduction must be below one",
				});
			}
			Ok(BuffModifier::MovementSpeedReduction { reduction })
		}
		SemanticBuffModifier::AttackSpeedAddition { points } => {
			let points: ExpectedValue =
				resolve_value_expression(points, blackboard, rule.mechanic.as_str())?;
			if !points.scaled().is_multiple_of(ExpectedValue::SCALE) {
				return Err(MechanicSemanticError::InvalidBuffModifier {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "attack-speed addition must be an integer",
				});
			}
			let delta: i16 = i16::try_from(points.whole_floor()).map_err(|_| {
				MechanicSemanticError::InvalidBuffModifier {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "attack-speed addition must fit i16",
				}
			})?;
			Ok(BuffModifier::AttackSpeed(AttackSpeedModifier {
				delta,
				duration_ticks: None,
			}))
		}
		SemanticBuffModifier::TargetCountUnlimited => {
			Ok(BuffModifier::TargetCount(TargetCountModifier::Unlimited {
				duration_ticks: None,
			}))
		}
	}
}

fn lower_targeted_effect(
	rule: &MechanicSemanticRule,
	targeted: &SemanticTargetedEffect,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
) -> Result<TaggedEffect, MechanicSemanticError> {
	let target: EffectTarget = match targeted.target {
		SemanticEffectTarget::TriggerActor => EffectTarget::CurrentTargets,
		SemanticEffectTarget::TriggerEffectSource => EffectTarget::TriggerEffectSource,
		SemanticEffectTarget::Owner => EffectTarget::SelfActor,
		SemanticEffectTarget::EnemiesInOwnerCurrentAttackRange => EffectTarget::EnemiesInRange,
	};
	let mut effect: TaggedEffect =
		lower_semantic_effect(rule, &targeted.effect, blackboard, tick_rate)?;
	effect.target = target;
	Ok(effect)
}

fn lower_trigger_event(
	rule: &MechanicSemanticRule,
	event: &SemanticEvent,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
) -> Result<ReplayTrigger, MechanicSemanticError> {
	match event {
		SemanticEvent::BattleStarted => Err(MechanicSemanticError::InvalidEffect {
			mechanic: rule.mechanic.as_str().to_owned(),
			detail: "battle-start event must lower as a direct effect",
		}),
		SemanticEvent::Deploy => Ok(ReplayTrigger::Deploy),
		SemanticEvent::SkillStarted => Ok(ReplayTrigger::SkillStarted),
		SemanticEvent::SkillEnded => Ok(ReplayTrigger::SkillEnded),
		SemanticEvent::NormalAttackHit => Ok(ReplayTrigger::NormalAttackHit),
		SemanticEvent::Heal => Ok(ReplayTrigger::Heal),
		SemanticEvent::Damage => Ok(ReplayTrigger::Damage),
		SemanticEvent::BeforeLethal => Ok(ReplayTrigger::BeforeLethal),
		SemanticEvent::Death => Ok(ReplayTrigger::Death),
		SemanticEvent::Periodic {
			initial_delay_seconds,
			interval_seconds,
		} => {
			let initial_delay_ticks: u64 = resolve_duration_ticks(
				initial_delay_seconds,
				blackboard,
				rule.mechanic.as_str(),
				"periodic initial delay",
				tick_rate,
				true,
			)?;
			let interval_ticks: u64 = resolve_duration_ticks(
				interval_seconds,
				blackboard,
				rule.mechanic.as_str(),
				"periodic interval",
				tick_rate,
				false,
			)?;
			Ok(ReplayTrigger::Periodic {
				initial_delay_ticks,
				interval_ticks,
			})
		}
	}
}

fn lower_trigger_filter(filter: SemanticTriggerFilter) -> TriggerActorFilter {
	match filter {
		SemanticTriggerFilter::AffectedOwner => TriggerActorFilter::SelfActor,
		SemanticTriggerFilter::EffectSourceOwner => TriggerActorFilter::EffectSource,
		SemanticTriggerFilter::AnyAffected => TriggerActorFilter::AnyActor,
		SemanticTriggerFilter::AllyAffected => TriggerActorFilter::AllyOfSource,
		SemanticTriggerFilter::EnemyAffected => TriggerActorFilter::EnemyOfSource,
	}
}

fn lower_actor_filter(filter: SemanticActorFilter) -> TriggerActorFilter {
	match filter {
		SemanticActorFilter::Owner => TriggerActorFilter::SelfActor,
		SemanticActorFilter::Any => TriggerActorFilter::AnyActor,
		SemanticActorFilter::AllyOfOwner => TriggerActorFilter::AllyOfSource,
		SemanticActorFilter::EnemyOfOwner => TriggerActorFilter::EnemyOfSource,
	}
}

fn lower_lifetime(lifetime: SemanticLifetime) -> RuntimeEffectLifetime {
	match lifetime {
		SemanticLifetime::Persistent => RuntimeEffectLifetime::Persistent,
		SemanticLifetime::OwnerDeployment => RuntimeEffectLifetime::SourceActor,
		SemanticLifetime::SkillActivation => RuntimeEffectLifetime::SkillActivation,
	}
}

fn lower_tile_field(
	rule: &MechanicSemanticRule,
	field: &SemanticTileField,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
	actor_templates: &BTreeMap<String, SummonTemplateId>,
) -> Result<TaggedEffect, MechanicSemanticError> {
	let mechanic: String = rule.mechanic.as_str().to_owned();
	if field.key.as_str().trim().is_empty() {
		return Err(MechanicSemanticError::InvalidTileField {
			mechanic,
			detail: "tile key must not be empty",
		});
	}
	if matches!(&field.cells, SemanticCellSet::TriggerActorRelative(offsets) if offsets.is_empty())
	{
		return Err(MechanicSemanticError::InvalidTileField {
			mechanic,
			detail: "trigger-actor offsets must not be empty",
		});
	}
	let max_layers: u16 = resolve_exact_u16(
		&field.max_layers_per_cell,
		blackboard,
		rule.mechanic.as_str(),
		"maximum layers per cell",
		false,
	)?;
	let lifetime: RuntimeEffectLifetime = lower_lifetime(field.lifetime);
	let layer_threshold_reactions: Vec<RuntimeTileLayerThresholdReaction> = field
		.layer_threshold_reactions
		.iter()
		.map(|reaction| {
			lower_layer_threshold_reaction(
				rule,
				reaction,
				field.lifetime,
				Some(max_layers),
				blackboard,
				actor_templates,
			)
		})
		.collect::<Result<_, _>>()?;
	let on_enter_once: Vec<TaggedEffect> = field
		.on_enter_once
		.iter()
		.map(|effect| lower_semantic_effect(rule, effect, blackboard, tick_rate))
		.collect::<Result<_, _>>()?;
	let mut on_enter_per_layer: Vec<TaggedEffect> = Vec::new();
	let mut on_leave_per_layer: Vec<TaggedEffect> = Vec::new();
	let mut on_remove_per_layer: Vec<TaggedEffect> = Vec::new();
	for resident in &field.resident_buffs {
		if resident.key_suffix.trim().is_empty() {
			return Err(MechanicSemanticError::InvalidTileField {
				mechanic: rule.mechanic.as_str().to_owned(),
				detail: "resident buff key must not be empty",
			});
		}
		let resident_max_stacks: u16 = resolve_exact_u16(
			&resident.max_stacks,
			blackboard,
			rule.mechanic.as_str(),
			"resident buff maximum stacks",
			false,
		)?;
		let buff_key: MechanicId =
			MechanicId::new(format!("{}/{}", field.key.as_str(), resident.key_suffix));
		let modifiers_per_stack: Vec<BuffModifier> = resident
			.modifiers_per_stack
			.iter()
			.map(|modifier: &SemanticBuffModifier| lower_buff_modifier(rule, modifier, blackboard))
			.collect::<Result<_, MechanicSemanticError>>()?;
		on_enter_per_layer.push(TaggedEffect {
			mechanic: buff_key.clone(),
			target: EffectTarget::CurrentTargets,
			effect: ExpectedEffect::ApplyBuff(RuntimeBuff {
				key: buff_key.clone(),
				max_stacks: resident_max_stacks,
				duration_ticks: None,
				lifetime,
				modifiers_per_stack,
			}),
		});
		let removal: TaggedEffect = TaggedEffect {
			mechanic: buff_key.clone(),
			target: EffectTarget::CurrentTargets,
			effect: ExpectedEffect::RemoveBuff {
				key: buff_key,
				source: BuffRemovalSource::EffectSource,
			},
		};
		on_leave_per_layer.push(removal.clone());
		on_remove_per_layer.push(removal);
	}
	let periodic: Option<PeriodicTileEffect> = field
		.periodic
		.as_ref()
		.map(|periodic| {
			if periodic.interval_millis == 0 {
				return Err(MechanicSemanticError::InvalidDuration {
					mechanic: rule.mechanic.as_str().to_owned(),
					field: "periodic interval",
				});
			}
			let initial_delay_ticks: u64 = tick_rate
				.ticks_from_millis(periodic.initial_delay_millis)
				.map_err(|_| MechanicSemanticError::InvalidDuration {
					mechanic: rule.mechanic.as_str().to_owned(),
					field: "periodic initial delay",
				})?
				.get();
			let interval_ticks: u64 = tick_rate
				.ticks_from_millis(periodic.interval_millis)
				.map_err(|_| MechanicSemanticError::InvalidDuration {
					mechanic: rule.mechanic.as_str().to_owned(),
					field: "periodic interval",
				})?
				.get();
			let effects_per_layer: Vec<TaggedEffect> = periodic
				.effects_per_layer
				.iter()
				.map(|effect| lower_semantic_effect(rule, effect, blackboard, tick_rate))
				.collect::<Result<_, _>>()?;
			Ok(PeriodicTileEffect {
				initial_delay_ticks,
				interval_ticks,
				effects_per_layer,
			})
		})
		.transpose()?;
	Ok(TaggedEffect {
		mechanic: rule.mechanic.clone(),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::AddTileLayer(RuntimeTileEffect {
			key: field.key.clone(),
			cells: match &field.cells {
				SemanticCellSet::TriggerActorRelative(offsets) => {
					ReplayCellSet::EffectTargetRelative(offsets.clone())
				}
				SemanticCellSet::SourceCurrentAttackRangeGroundCells => {
					ReplayCellSet::SourceCurrentAttackRangeGroundCells
				}
			},
			actor_filter: lower_actor_filter(field.actor_filter),
			target_filter: field.target_filter,
			max_layers,
			duration_ticks: None,
			lifetime,
			apply_new_layer_to_occupants: true,
			on_enter_once,
			on_enter_per_layer,
			on_leave_per_layer,
			on_remove_per_layer,
			layer_threshold_reactions,
			periodic,
			remove_on_first_matching_leave: field.remove_on_first_matching_leave,
		}),
	})
}

fn lower_layer_threshold_reaction(
	rule: &MechanicSemanticRule,
	reaction: &SemanticLayerThresholdReaction,
	host_lifetime: SemanticLifetime,
	max_layers: Option<u16>,
	blackboard: &[BlackboardEntry],
	actor_templates: &BTreeMap<String, SummonTemplateId>,
) -> Result<RuntimeTileLayerThresholdReaction, MechanicSemanticError> {
	let mechanic: String = rule.mechanic.as_str().to_owned();
	let threshold: u16 = resolve_exact_u16(
		&reaction.threshold,
		blackboard,
		rule.mechanic.as_str(),
		"layer reaction threshold",
		false,
	)?;
	if max_layers.is_some_and(|max_layers| threshold > max_layers) {
		return Err(MechanicSemanticError::InvalidLayerThresholdReaction {
			mechanic,
			detail: "threshold exceeds the tile layer cap",
		});
	}
	if reaction.transition == SemanticLayerThresholdTransition::SaturatedAttempt
		&& max_layers.is_some_and(|max_layers| threshold != max_layers)
	{
		return Err(MechanicSemanticError::InvalidLayerThresholdReaction {
			mechanic,
			detail: "saturated-attempt threshold must equal the tile layer cap",
		});
	}
	let SemanticLayerThresholdReactionEffect::SpawnActor(spawn) = &reaction.effect;
	if spawn.actor_template_key.trim().is_empty() {
		return Err(MechanicSemanticError::InvalidLayerThresholdReaction {
			mechanic,
			detail: "actor template key must not be empty",
		});
	}
	if spawn.ownership == SemanticTargetCellSpawnOwnership::SourceActor
		&& host_lifetime == SemanticLifetime::Persistent
	{
		return Err(MechanicSemanticError::InvalidLayerThresholdReaction {
			mechanic,
			detail: "source-owned actor requires a live source lifetime",
		});
	}
	if spawn.ownership == SemanticTargetCellSpawnOwnership::SkillActivation
		&& host_lifetime != SemanticLifetime::SkillActivation
	{
		return Err(MechanicSemanticError::InvalidLayerThresholdReaction {
			mechanic,
			detail: "skill-owned actor requires skill-activation tile lifetime",
		});
	}
	let template: SummonTemplateId = actor_templates
		.get(&spawn.actor_template_key)
		.copied()
		.ok_or_else(|| MechanicSemanticError::MissingActorTemplate {
			mechanic: rule.mechanic.as_str().to_owned(),
			key: spawn.actor_template_key.clone(),
		})?;
	Ok(RuntimeTileLayerThresholdReaction {
		threshold,
		transition: match reaction.transition {
			SemanticLayerThresholdTransition::ReachedFromBelow => {
				TileLayerThresholdTransition::ReachedFromBelow
			}
			SemanticLayerThresholdTransition::SaturatedAttempt => {
				TileLayerThresholdTransition::SaturatedAttempt
			}
		},
		effect: TileLayerThresholdReactionEffect::SpawnActor(TargetCellActorSpawn {
			template,
			collision: match spawn.collision {
				SemanticTargetCellSpawnCollision::SuppressIfFriendlyOccupied => {
					TargetCellSpawnCollision::SuppressIfFriendlyOccupied
				}
				SemanticTargetCellSpawnCollision::AllowCoexistence => {
					TargetCellSpawnCollision::AllowCoexistence
				}
			},
			ownership: match spawn.ownership {
				SemanticTargetCellSpawnOwnership::Independent => {
					TargetCellSpawnOwnership::Independent
				}
				SemanticTargetCellSpawnOwnership::SourceActor => {
					TargetCellSpawnOwnership::SourceActor
				}
				SemanticTargetCellSpawnOwnership::SkillActivation => {
					TargetCellSpawnOwnership::SkillActivation
				}
				SemanticTargetCellSpawnOwnership::TriggeringTileEffect => {
					TargetCellSpawnOwnership::TriggeringTileEffect
				}
			},
		}),
	})
}

fn lower_tile_field_overlay(
	rule: &MechanicSemanticRule,
	overlay: &SemanticTileFieldOverlay,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
	actor_templates: &BTreeMap<String, SummonTemplateId>,
) -> Result<TaggedEffect, MechanicSemanticError> {
	let mechanic: String = rule.mechanic.as_str().to_owned();
	if overlay.key.as_str().trim().is_empty() {
		return Err(MechanicSemanticError::InvalidTileFieldOverlay {
			mechanic,
			detail: "overlay key must not be empty",
		});
	}
	if overlay.observed_tile_key.as_str().trim().is_empty() {
		return Err(MechanicSemanticError::InvalidTileFieldOverlay {
			mechanic,
			detail: "observed tile key must not be empty",
		});
	}
	if overlay.lifetime != SemanticLifetime::SkillActivation {
		return Err(MechanicSemanticError::InvalidTileFieldOverlay {
			mechanic,
			detail: "overlay lifetime must be one skill activation",
		});
	}
	if overlay.periodic_effects.is_empty()
		&& overlay.on_leave_once.is_empty()
		&& overlay.layer_threshold_reactions.is_empty()
		&& overlay.cached_propagation.is_none()
	{
		return Err(MechanicSemanticError::InvalidTileFieldOverlay {
			mechanic,
			detail: "overlay must define a periodic, leave, layer-reaction, or propagation effect",
		});
	}
	let observed_tile_max_layers: Option<u16> = overlay
		.observed_tile_max_layers
		.as_ref()
		.map(|maximum| {
			resolve_exact_u16(
				maximum,
				blackboard,
				rule.mechanic.as_str(),
				"observed tile maximum layers",
				false,
			)
		})
		.transpose()?;
	if !overlay.layer_threshold_reactions.is_empty() && observed_tile_max_layers.is_none() {
		return Err(MechanicSemanticError::InvalidTileFieldOverlay {
			mechanic,
			detail: "layer reactions require the observed tile layer cap",
		});
	}
	let layer_threshold_reactions: Vec<RuntimeTileLayerThresholdReaction> = overlay
		.layer_threshold_reactions
		.iter()
		.map(|reaction| {
			lower_layer_threshold_reaction(
				rule,
				reaction,
				overlay.lifetime,
				observed_tile_max_layers,
				blackboard,
				actor_templates,
			)
		})
		.collect::<Result<_, _>>()?;
	let initial_delay_ticks: u64 = resolve_duration_ticks(
		&overlay.initial_delay_seconds,
		blackboard,
		rule.mechanic.as_str(),
		"tile-field overlay initial delay",
		tick_rate,
		true,
	)?;
	let interval_ticks: u64 = resolve_duration_ticks(
		&overlay.interval_seconds,
		blackboard,
		rule.mechanic.as_str(),
		"tile-field overlay interval",
		tick_rate,
		false,
	)?;
	let periodic_effects: Vec<TaggedEffect> = overlay
		.periodic_effects
		.iter()
		.map(|effect| lower_semantic_effect(rule, effect, blackboard, tick_rate))
		.collect::<Result<_, _>>()?;
	let on_leave_once: Vec<TaggedEffect> = overlay
		.on_leave_once
		.iter()
		.map(|effect| lower_semantic_effect(rule, effect, blackboard, tick_rate))
		.collect::<Result<_, _>>()?;
	let cached_propagation: Option<RuntimeCachedTileLayerPropagation> = overlay
		.cached_propagation
		.as_ref()
		.map(|propagation| {
			Ok(RuntimeCachedTileLayerPropagation {
				maximum_outside_temporary_cells: resolve_exact_u16(
					&propagation.maximum_outside_temporary_cells,
					blackboard,
					rule.mechanic.as_str(),
					"maximum outside-range temporary tile cells",
					true,
				)?,
				source_order: propagation.source_order.clone(),
				contribution_merge: propagation.contribution_merge,
			})
		})
		.transpose()?;
	Ok(TaggedEffect {
		mechanic: rule.mechanic.clone(),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::InstallTileFieldOverlay(RuntimeTileFieldOverlay {
			key: overlay.key.clone(),
			observed_tile_key: overlay.observed_tile_key.clone(),
			observed_tile_max_layers,
			actor_filter: lower_actor_filter(overlay.actor_filter),
			target_filter: overlay.target_filter,
			leave_scope: overlay.leave_scope,
			initial_delay_ticks,
			interval_ticks,
			lifetime: lower_lifetime(overlay.lifetime),
			periodic_effects,
			on_leave_once,
			layer_threshold_reactions,
			cached_propagation,
		}),
	})
}

fn lower_semantic_effect(
	rule: &MechanicSemanticRule,
	effect: &SemanticEffect,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
) -> Result<TaggedEffect, MechanicSemanticError> {
	let expected: ExpectedEffect = match effect {
		SemanticEffect::Healing { amount } => ExpectedEffect::Healing {
			amount: resolve_value_expression(amount, blackboard, rule.mechanic.as_str())?,
		},
		SemanticEffect::Damage {
			amount,
			damage_type,
			delivery,
		} => ExpectedEffect::Damage {
			amount: resolve_value_expression(amount, blackboard, rule.mechanic.as_str())?,
			damage_type: *damage_type,
			delivery: *delivery,
		},
		SemanticEffect::SourceStatScaledDamage {
			stat,
			multiplier,
			damage_type,
			delivery,
		} => ExpectedEffect::SourceStatScaledDamage {
			stat: *stat,
			multiplier: resolve_value_expression(multiplier, blackboard, rule.mechanic.as_str())?,
			damage_type: *damage_type,
			delivery: *delivery,
		},
		SemanticEffect::RestoreHpToCurrentMaxRatio { ratio } => {
			let ratio: ExpectedValue =
				resolve_value_expression(ratio, blackboard, rule.mechanic.as_str())?;
			if ratio == ExpectedValue::ZERO || ratio > ExpectedValue::ONE {
				return Err(MechanicSemanticError::InvalidEffect {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "HP restore ratio must be greater than zero and at most one",
				});
			}
			ExpectedEffect::RestoreHpToCurrentMaxRatio { ratio }
		}
		SemanticEffect::ApplyStatus {
			kind,
			duration_seconds,
		} => {
			let duration_ticks: u64 = resolve_duration_ticks(
				duration_seconds,
				blackboard,
				rule.mechanic.as_str(),
				"status duration",
				tick_rate,
				false,
			)?;
			let status: StatusEffect = match kind {
				StatusKind::Cold => StatusEffect::cold(duration_ticks),
				StatusKind::Frozen => StatusEffect::frozen(duration_ticks),
				_ => {
					return Err(MechanicSemanticError::InvalidStatus {
						mechanic: rule.mechanic.as_str().to_owned(),
						detail: "only standard Cold and Frozen are currently supported",
					});
				}
			};
			ExpectedEffect::ApplyStatus(status)
		}
		SemanticEffect::GainSp { amount } => {
			let amount: ExpectedValue =
				resolve_value_expression(amount, blackboard, rule.mechanic.as_str())?;
			if !amount.scaled().is_multiple_of(ExpectedValue::SCALE) {
				return Err(MechanicSemanticError::InvalidEffect {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "SP gain must be an integer",
				});
			}
			ExpectedEffect::GainSp {
				amount: u32::try_from(amount.whole_floor()).map_err(|_| {
					MechanicSemanticError::InvalidEffect {
						mechanic: rule.mechanic.as_str().to_owned(),
						detail: "SP gain must fit u32",
					}
				})?,
			}
		}
		SemanticEffect::ApplyBuff {
			key,
			max_stacks,
			lifetime,
			modifiers_per_stack,
		} => {
			if key.as_str().trim().is_empty() || modifiers_per_stack.is_empty() {
				return Err(MechanicSemanticError::InvalidEffect {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "buff key and modifier list must not be empty",
				});
			}
			let max_stacks_value: ExpectedValue =
				resolve_value_expression(max_stacks, blackboard, rule.mechanic.as_str())?;
			if max_stacks_value == ExpectedValue::ZERO
				|| !max_stacks_value
					.scaled()
					.is_multiple_of(ExpectedValue::SCALE)
			{
				return Err(MechanicSemanticError::InvalidEffect {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "buff maximum stacks must be a positive integer",
				});
			}
			let max_stacks: u16 = u16::try_from(max_stacks_value.whole_floor()).map_err(|_| {
				MechanicSemanticError::InvalidEffect {
					mechanic: rule.mechanic.as_str().to_owned(),
					detail: "buff maximum stacks must fit u16",
				}
			})?;
			let modifiers_per_stack: Vec<BuffModifier> = modifiers_per_stack
				.iter()
				.map(|modifier: &SemanticBuffModifier| {
					lower_buff_modifier(rule, modifier, blackboard)
				})
				.collect::<Result<_, MechanicSemanticError>>()?;
			ExpectedEffect::ApplyBuff(RuntimeBuff {
				key: key.clone(),
				max_stacks,
				duration_ticks: None,
				lifetime: lower_lifetime(*lifetime),
				modifiers_per_stack,
			})
		}
		SemanticEffect::ConditionalStatMultiplier {
			condition,
			stat,
			bucket,
			multiplier,
		} => ExpectedEffect::ConditionalStatModifier(ConditionalStatModifier {
			condition: lower_actor_condition(rule, condition, blackboard)?,
			modifier: StatModifier {
				stat: *stat,
				bucket: *bucket,
				multiplier: resolve_value_expression(
					multiplier,
					blackboard,
					rule.mechanic.as_str(),
				)?,
				duration_ticks: None,
			},
		}),
	};
	Ok(TaggedEffect {
		mechanic: rule.mechanic.clone(),
		target: EffectTarget::CurrentTargets,
		effect: expected,
	})
}

fn lower_actor_condition(
	rule: &MechanicSemanticRule,
	condition: &SemanticActorCondition,
	blackboard: &[BlackboardEntry],
) -> Result<ActorCondition, MechanicSemanticError> {
	match condition {
		SemanticActorCondition::BlockedEnemiesAtLeast { count } => {
			let invalid = |detail: &'static str| MechanicSemanticError::InvalidEffect {
				mechanic: rule.mechanic.as_str().to_owned(),
				detail,
			};
			let count: ExpectedValue =
				resolve_value_expression(count, blackboard, rule.mechanic.as_str())?;
			if count == ExpectedValue::ZERO || !count.scaled().is_multiple_of(ExpectedValue::SCALE)
			{
				return Err(invalid(
					"blocked-enemy count threshold must be a positive integer",
				));
			}
			let count: u16 = u16::try_from(count.whole_floor())
				.map_err(|_| invalid("blocked-enemy count threshold must fit u16"))?;
			Ok(ActorCondition::BlockedEnemiesAtLeast(count))
		}
	}
}

pub(crate) fn resolve_value_expression(
	expression: &SemanticValueExpression,
	blackboard: &[BlackboardEntry],
	mechanic: &str,
) -> Result<ExpectedValue, MechanicSemanticError> {
	match expression {
		SemanticValueExpression::Literal(value) => Ok(*value),
		SemanticValueExpression::Blackboard(key) => {
			let mut values = blackboard.iter().filter(|entry| entry.key() == key);
			let entry: &BlackboardEntry =
				values
					.next()
					.ok_or_else(|| MechanicSemanticError::MissingBlackboardValue {
						mechanic: mechanic.to_owned(),
						key: key.clone(),
					})?;
			if values.next().is_some() {
				return Err(MechanicSemanticError::DuplicateBlackboardValue {
					mechanic: mechanic.to_owned(),
					key: key.clone(),
				});
			}
			let fixed: FixedDamage = FixedDamage::try_from_f64(entry.value()).map_err(|_| {
				MechanicSemanticError::InvalidBlackboardValue {
					mechanic: mechanic.to_owned(),
					key: key.clone(),
				}
			})?;
			Ok(ExpectedValue::from_scaled(fixed.scaled()))
		}
		SemanticValueExpression::NegatedBlackboard(key) => {
			let mut values = blackboard.iter().filter(|entry| entry.key() == key);
			let entry: &BlackboardEntry =
				values
					.next()
					.ok_or_else(|| MechanicSemanticError::MissingBlackboardValue {
						mechanic: mechanic.to_owned(),
						key: key.clone(),
					})?;
			if values.next().is_some() {
				return Err(MechanicSemanticError::DuplicateBlackboardValue {
					mechanic: mechanic.to_owned(),
					key: key.clone(),
				});
			}
			let value: f64 = entry.value();
			if !value.is_finite() || value > 0.0 {
				return Err(MechanicSemanticError::InvalidBlackboardValue {
					mechanic: mechanic.to_owned(),
					key: key.clone(),
				});
			}
			let fixed: FixedDamage = FixedDamage::try_from_f64(-value).map_err(|_| {
				MechanicSemanticError::InvalidBlackboardValue {
					mechanic: mechanic.to_owned(),
					key: key.clone(),
				}
			})?;
			Ok(ExpectedValue::from_scaled(fixed.scaled()))
		}
		SemanticValueExpression::OnePlus(nested) => {
			let nested: ExpectedValue = resolve_value_expression(nested, blackboard, mechanic)?;
			ExpectedValue::ONE.checked_add(nested).ok_or_else(|| {
				MechanicSemanticError::ArithmeticOverflow {
					mechanic: mechanic.to_owned(),
				}
			})
		}
		SemanticValueExpression::OneMinus(nested) => {
			let nested: ExpectedValue = resolve_value_expression(nested, blackboard, mechanic)?;
			if nested > ExpectedValue::ONE {
				return Err(MechanicSemanticError::ArithmeticOverflow {
					mechanic: mechanic.to_owned(),
				});
			}
			Ok(ExpectedValue::from_scaled(
				ExpectedValue::ONE.scaled() - nested.scaled(),
			))
		}
		SemanticValueExpression::Product(left, right) => {
			let left: ExpectedValue = resolve_value_expression(left, blackboard, mechanic)?;
			let right: ExpectedValue = resolve_value_expression(right, blackboard, mechanic)?;
			let product: u128 = u128::from(left.scaled())
				.checked_mul(u128::from(right.scaled()))
				.and_then(|value| value.checked_div(u128::from(ExpectedValue::SCALE)))
				.ok_or_else(|| MechanicSemanticError::ArithmeticOverflow {
					mechanic: mechanic.to_owned(),
				})?;
			Ok(ExpectedValue::from_scaled(u64::try_from(product).map_err(
				|_| MechanicSemanticError::ArithmeticOverflow {
					mechanic: mechanic.to_owned(),
				},
			)?))
		}
	}
}

fn resolve_exact_u16(
	expression: &SemanticValueExpression,
	blackboard: &[BlackboardEntry],
	mechanic: &str,
	field: &'static str,
	allow_zero: bool,
) -> Result<u16, MechanicSemanticError> {
	let value: ExpectedValue = resolve_value_expression(expression, blackboard, mechanic)?;
	if !value.scaled().is_multiple_of(ExpectedValue::SCALE)
		|| (!allow_zero && value == ExpectedValue::ZERO)
	{
		return Err(MechanicSemanticError::InvalidTileField {
			mechanic: mechanic.to_owned(),
			detail: field,
		});
	}
	u16::try_from(value.whole_floor()).map_err(|_| MechanicSemanticError::InvalidTileField {
		mechanic: mechanic.to_owned(),
		detail: field,
	})
}

fn resolve_duration_ticks(
	expression: &SemanticValueExpression,
	blackboard: &[BlackboardEntry],
	mechanic: &str,
	field: &'static str,
	tick_rate: TickRate,
	allow_zero: bool,
) -> Result<u64, MechanicSemanticError> {
	let seconds: ExpectedValue = resolve_value_expression(expression, blackboard, mechanic)?;
	let numerator: u128 = u128::from(seconds.scaled())
		.checked_mul(u128::from(tick_rate.ticks_per_second().get()))
		.ok_or_else(|| MechanicSemanticError::ArithmeticOverflow {
			mechanic: mechanic.to_owned(),
		})?;
	let ticks: u128 = numerator
		.checked_add(u128::from(ExpectedValue::SCALE - 1))
		.and_then(|value| value.checked_div(u128::from(ExpectedValue::SCALE)))
		.ok_or_else(|| MechanicSemanticError::ArithmeticOverflow {
			mechanic: mechanic.to_owned(),
		})?;
	let ticks: u64 =
		u64::try_from(ticks).map_err(|_| MechanicSemanticError::ArithmeticOverflow {
			mechanic: mechanic.to_owned(),
		})?;
	if !allow_zero && ticks == 0 {
		return Err(MechanicSemanticError::InvalidDuration {
			mechanic: mechanic.to_owned(),
			field,
		});
	}
	Ok(ticks)
}

#[cfg(test)]
mod tests {
	use std::{collections::BTreeMap, num::NonZeroU32};

	use crate::data::RangeOffset;
	use crate::simulation::TickRate;

	use crate::mechanics::combat::{
		ActorCondition, BuffModifier, ConditionalStatModifier, DamageDelivery, DamageType,
		EffectTarget, ExpectedEffect, ExpectedValue, MechanicBehavior, MechanicId, ReplayCellSet,
		ReplayRepeat, ReplayTrigger, RuntimeEffectLifetime, StatKind, StatModifier,
		StatModifierBucket, StatusFactionFilter, StatusKind, SummonTemplateId,
		TargetCellSpawnCollision, TargetCellSpawnOwnership, TeamCompositionCondition,
		TileFieldOverlayLeaveScope, TileLayerThresholdReactionEffect, TileLayerThresholdTransition,
		TriggerActorFilter,
	};
	use crate::mechanics::normal_attack::NormalAttackTargetFilter;

	use super::{
		GamedataSemanticSourceLocation, MechanicSemanticError, MechanicSemanticProgram,
		MechanicSemanticRule, SemanticAction, SemanticActorCondition, SemanticActorFilter,
		SemanticBuffModifier, SemanticCellSet, SemanticConditionalAura,
		SemanticConditionalAuraPeriodicEffect, SemanticEffect, SemanticEffectTarget, SemanticEvent,
		SemanticLayerThresholdReaction, SemanticLayerThresholdReactionEffect,
		SemanticLayerThresholdTransition, SemanticLifetime, SemanticPeriodicEffect,
		SemanticProvenance, SemanticResidentBuff, SemanticSkillInactiveStance,
		SemanticSourceLocation, SemanticTargetCellActorSpawn, SemanticTargetCellSpawnCollision,
		SemanticTargetCellSpawnOwnership, SemanticTargetedEffect, SemanticTeamCompositionCondition,
		SemanticTileField, SemanticTileFieldOverlay, SemanticTrigger, SemanticTriggerFilter,
		SemanticValueExpression,
	};

	fn provenance() -> SemanticProvenance {
		SemanticProvenance::BattlePrefab {
			mechanics_revision: "a".repeat(64),
			gamedata_revision: "b".repeat(40),
			script_id: [1; 16],
			old_type_hash: [2; 16],
			source: SemanticSourceLocation {
				logical_path: "charpack/fixture".to_owned(),
				cab_name: "CAB-fixture".to_owned(),
				path_id: 7,
				field_paths: vec!["$._buffs[0]".to_owned()],
			},
		}
	}

	#[test]
	fn provenance_variants_expose_typed_origins_and_field_paths() {
		let prefab: SemanticProvenance = provenance();
		assert_eq!(
			prefab.origin(),
			"verified-prefab:charpack/fixture:CAB-fixture:7"
		);
		assert_eq!(prefab.field_paths(), &["$._buffs[0]".to_owned()]);

		let gamedata: SemanticProvenance = SemanticProvenance::Gamedata {
			gamedata_revision: "revision".to_owned(),
			decoder_rule_revision: "fixture/v1".to_owned(),
			source: GamedataSemanticSourceLocation {
				table_path: "character_table.json".to_owned(),
				record_key: "char_fixture".to_owned(),
				field_paths: vec!["$.char_fixture.subProfessionId".to_owned()],
				resolved_projection_fields: vec!["selected_trait_candidate.hidden".to_owned()],
			},
		};
		assert_eq!(
			gamedata.origin(),
			"verified-gamedata:character_table.json:char_fixture"
		);
		assert_eq!(
			gamedata.field_paths(),
			&["$.char_fixture.subProfessionId".to_owned()]
		);
	}

	fn skill_inactive_stance_program() -> MechanicSemanticProgram {
		let mechanic: MechanicId = MechanicId::new("fixture/skill-inactive-stance");
		MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: mechanic.clone(),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::Deploy,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::InstallSkillInactiveStance(SemanticSkillInactiveStance {
					key: mechanic,
					disable_normal_attacks: true,
					modifiers: vec![
						SemanticBuffModifier::StatMultiplier {
							bucket: StatModifierBucket::Panel,
							stat: StatKind::Defense,
							multiplier: SemanticValueExpression::Literal(
								ExpectedValue::from_whole(3),
							),
						},
						SemanticBuffModifier::ArtsResistanceAddition {
							ratio: SemanticValueExpression::Literal(
								ExpectedValue::from_ratio(1, 5).expect("valid ratio"),
							),
						},
					],
				}),
				behavior: MechanicBehavior::StatModifier,
			}],
		}
	}

	#[test]
	fn skill_inactive_stance_lowers_generic_defense_and_additive_res() {
		let lowered = skill_inactive_stance_program()
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect("valid skill-inactive stance");
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected deploy listener");
		};
		assert_eq!(listener.trigger, ReplayTrigger::Deploy);
		let ExpectedEffect::InstallSkillInactiveStance(stance) = &listener.effects[0].effect else {
			panic!("expected runtime stance installation");
		};
		assert!(stance.disable_normal_attacks);
		assert!(matches!(
			&stance.modifiers[0],
			BuffModifier::Stat(modifier)
				if modifier.stat == StatKind::Defense
					&& modifier.multiplier == ExpectedValue::from_whole(3)
		));
		assert_eq!(
			stance.modifiers[1],
			BuffModifier::ArtsResistanceAddition {
				ratio: ExpectedValue::from_ratio(1, 5).expect("valid ratio"),
			}
		);
	}

	#[test]
	fn skill_inactive_stance_rejects_empty_behavior() {
		let mut program: MechanicSemanticProgram = skill_inactive_stance_program();
		let SemanticAction::InstallSkillInactiveStance(stance) = &mut program.rules[0].action
		else {
			panic!("fixture must install a skill-inactive stance");
		};
		stance.disable_normal_attacks = false;
		stance.modifiers.clear();

		assert!(matches!(
			program.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero"))),
			Err(MechanicSemanticError::InvalidSkillInactiveStance { .. })
		));
	}

	fn blocked_count_defense_program(count: ExpectedValue) -> MechanicSemanticProgram {
		MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: MechanicId::new("fixture/blocked-count-defense"),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::Deploy,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::ApplyEffects(vec![SemanticTargetedEffect {
					target: SemanticEffectTarget::Owner,
					effect: SemanticEffect::ConditionalStatMultiplier {
						condition: SemanticActorCondition::BlockedEnemiesAtLeast {
							count: SemanticValueExpression::Literal(count),
						},
						stat: StatKind::Defense,
						bucket: StatModifierBucket::Panel,
						multiplier: SemanticValueExpression::Literal(
							ExpectedValue::from_ratio(119, 100).expect("valid multiplier"),
						),
					},
				}]),
				behavior: MechanicBehavior::StatModifier,
			}],
		}
	}

	#[test]
	fn blocked_count_multiplier_lowers_a_whole_positive_threshold_on_its_owner() {
		let tick_rate: TickRate = TickRate::new(NonZeroU32::new(30).expect("nonzero"));
		let lowered = blocked_count_defense_program(ExpectedValue::from_whole(2))
			.lower(&[], tick_rate)
			.expect("valid blocked-count multiplier");
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected deploy listener");
		};
		assert_eq!(listener.lifetime, RuntimeEffectLifetime::SourceActor);
		assert_eq!(listener.effects[0].target, EffectTarget::SelfActor);
		assert_eq!(
			listener.effects[0].effect,
			ExpectedEffect::ConditionalStatModifier(ConditionalStatModifier {
				condition: ActorCondition::BlockedEnemiesAtLeast(2),
				modifier: StatModifier {
					stat: StatKind::Defense,
					bucket: StatModifierBucket::Panel,
					multiplier: ExpectedValue::from_ratio(119, 100).expect("valid multiplier"),
					duration_ticks: None,
				},
			})
		);

		for count in [
			ExpectedValue::ZERO,
			ExpectedValue::from_ratio(3, 2).expect("valid ratio"),
			ExpectedValue::from_whole(u32::from(u16::MAX) + 1),
		] {
			assert!(
				matches!(
					blocked_count_defense_program(count).lower(&[], tick_rate),
					Err(MechanicSemanticError::InvalidEffect { .. })
				),
				"{count:?}"
			);
		}
	}

	#[test]
	fn conditional_aura_lowers_composition_buff_and_periodic_sp() {
		let mechanic: MechanicId = MechanicId::new("fixture/conditional-aura");
		let program: MechanicSemanticProgram = MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: mechanic.clone(),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::BattleStarted,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				},
				lifetime: SemanticLifetime::Persistent,
				action: SemanticAction::InstallConditionalAura(SemanticConditionalAura {
					key: mechanic.clone(),
					condition: SemanticTeamCompositionCondition::FixedRosterGroup {
						minimum_count: 4,
						group_id: "sui".to_owned(),
					},
					target_profession_mask: 639,
					includes_source: true,
					lifetime: SemanticLifetime::Persistent,
					modifiers: vec![
						SemanticBuffModifier::StatMultiplier {
							bucket: StatModifierBucket::Panel,
							stat: StatKind::Attack,
							multiplier: SemanticValueExpression::OnePlus(Box::new(
								SemanticValueExpression::Literal(
									ExpectedValue::from_ratio(3, 25).expect("valid ratio"),
								),
							)),
						},
						SemanticBuffModifier::AttackSpeedAddition {
							points: SemanticValueExpression::Literal(ExpectedValue::from_whole(5)),
						},
					],
					periodic: Some(SemanticConditionalAuraPeriodicEffect {
						initial_delay_seconds: SemanticValueExpression::Literal(
							ExpectedValue::from_whole(4),
						),
						interval_seconds: SemanticValueExpression::Literal(
							ExpectedValue::from_whole(4),
						),
						effects: vec![SemanticEffect::GainSp {
							amount: SemanticValueExpression::Literal(ExpectedValue::from_whole(1)),
						}],
					}),
				}),
				behavior: MechanicBehavior::ConditionalAura,
			}],
		};

		let lowered = program
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect("valid conditional aura");
		assert!(lowered.on_deploy.is_empty());
		assert_eq!(lowered.on_battle_start.len(), 1);
		let ExpectedEffect::InstallConditionalAura(aura) = &lowered.on_battle_start[0].effect
		else {
			panic!("expected conditional aura");
		};
		assert_eq!(
			aura.condition,
			TeamCompositionCondition::FixedRosterGroup {
				minimum_count: 4,
				group_id: "sui".to_owned(),
			}
		);
		assert_eq!(aura.target_profession_mask, 639);
		assert_eq!(aura.lifetime, RuntimeEffectLifetime::Persistent);
		assert_eq!(aura.buff.lifetime, RuntimeEffectLifetime::Persistent);
		assert_eq!(aura.buff.modifiers_per_stack.len(), 2);
		let BuffModifier::Stat(attack) = &aura.buff.modifiers_per_stack[0] else {
			panic!("expected attack multiplier");
		};
		assert_eq!(
			attack.multiplier,
			ExpectedValue::from_ratio(28, 25).expect("valid ratio")
		);
		let periodic = aura.periodic.as_ref().expect("periodic SP");
		assert_eq!(periodic.initial_delay_ticks, 120);
		assert_eq!(periodic.interval_ticks, 120);
		assert!(matches!(
			periodic.effects[0].effect,
			ExpectedEffect::GainSp { amount: 1 }
		));

		let mut invalid_trigger: MechanicSemanticProgram = program.clone();
		invalid_trigger.rules[0].trigger.repeat = ReplayRepeat::Unlimited;
		assert!(matches!(
			invalid_trigger.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero"))),
			Err(MechanicSemanticError::InvalidEffect {
				detail: "battle-start rule must target the affected owner exactly once",
				..
			})
		));

		let mut mismatched_lifetime: MechanicSemanticProgram = program;
		let SemanticAction::InstallConditionalAura(aura) = &mut mismatched_lifetime.rules[0].action
		else {
			panic!("fixture must install a conditional aura");
		};
		aura.lifetime = SemanticLifetime::OwnerDeployment;
		assert!(matches!(
			mismatched_lifetime.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero"))),
			Err(MechanicSemanticError::InvalidConditionalAura {
				detail: "aura lifetime must match its semantic rule lifetime",
				..
			})
		));
	}

	fn sowed_field_program() -> MechanicSemanticProgram {
		let mechanic: MechanicId = MechanicId::new("fixture/sowed_field");
		MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: mechanic.clone(),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::Heal,
					filter: SemanticTriggerFilter::EffectSourceOwner,
					repeat: crate::mechanics::combat::ReplayRepeat::Unlimited,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::AddTileField(SemanticTileField {
					key: mechanic,
					cells: SemanticCellSet::TriggerActorRelative(vec![
						RangeOffset::new(0, 0),
						RangeOffset::new(-1, 0),
						RangeOffset::new(1, 0),
						RangeOffset::new(0, -1),
						RangeOffset::new(0, 1),
					]),
					actor_filter: SemanticActorFilter::AllyOfOwner,
					target_filter: NormalAttackTargetFilter::GroundAndAir,
					max_layers_per_cell: SemanticValueExpression::Literal(
						ExpectedValue::from_whole(1),
					),
					lifetime: SemanticLifetime::Persistent,
					on_enter_once: Vec::new(),
					resident_buffs: vec![SemanticResidentBuff {
						key_suffix: "shelter".to_owned(),
						max_stacks: SemanticValueExpression::Literal(ExpectedValue::from_whole(1)),
						modifiers_per_stack: vec![SemanticBuffModifier::Shelter {
							incoming_damage_multiplier: SemanticValueExpression::OneMinus(
								Box::new(SemanticValueExpression::Literal(
									ExpectedValue::from_ratio(3, 25).expect("valid ratio"),
								)),
							),
						}],
					}],
					layer_threshold_reactions: Vec::new(),
					periodic: Some(SemanticPeriodicEffect {
						initial_delay_millis: 1_000,
						interval_millis: 1_000,
						effects_per_layer: vec![super::SemanticEffect::Healing {
							amount: SemanticValueExpression::Literal(ExpectedValue::from_whole(75)),
						}],
					}),
					remove_on_first_matching_leave: false,
				}),
				behavior: MechanicBehavior::ShelterExposureOrDirectHit,
			}],
		}
	}

	#[test]
	fn tile_field_rule_lowers_to_heal_listener_and_target_relative_cells() {
		let lowered = sowed_field_program()
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect("valid semantic program");
		assert_eq!(lowered.on_deploy.len(), 1);
		assert_eq!(lowered.reached_mechanics.len(), 1);
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected listener");
		};
		assert_eq!(listener.trigger, ReplayTrigger::Heal);
		assert_eq!(listener.actor_filter, TriggerActorFilter::EffectSource);
		let ExpectedEffect::AddTileLayer(tile) = &listener.effects[0].effect else {
			panic!("expected tile field");
		};
		assert!(matches!(tile.cells, ReplayCellSet::EffectTargetRelative(_)));
		assert_eq!(tile.actor_filter, TriggerActorFilter::AllyOfSource);
		assert_eq!(tile.periodic.as_ref().expect("periodic").interval_ticks, 30);
	}

	#[test]
	fn tile_threshold_spawn_lowers_only_with_an_explicit_actor_template_binding() {
		let mut program: MechanicSemanticProgram = sowed_field_program();
		let SemanticAction::AddTileField(field) = &mut program.rules[0].action else {
			panic!("fixture must add a tile field");
		};
		field.max_layers_per_cell = SemanticValueExpression::Literal(ExpectedValue::from_whole(5));
		field.layer_threshold_reactions = vec![SemanticLayerThresholdReaction {
			threshold: SemanticValueExpression::Literal(ExpectedValue::from_whole(5)),
			transition: SemanticLayerThresholdTransition::ReachedFromBelow,
			effect: SemanticLayerThresholdReactionEffect::SpawnActor(
				SemanticTargetCellActorSpawn {
					actor_template_key: "frozen-objective".to_owned(),
					collision: SemanticTargetCellSpawnCollision::SuppressIfFriendlyOccupied,
					ownership: SemanticTargetCellSpawnOwnership::TriggeringTileEffect,
				},
			),
		}];
		let tick_rate: TickRate = TickRate::new(NonZeroU32::new(30).expect("nonzero"));
		assert!(matches!(
			program.lower(&[], tick_rate),
			Err(MechanicSemanticError::MissingActorTemplate { key, .. })
				if key == "frozen-objective"
		));

		let templates: BTreeMap<String, SummonTemplateId> =
			BTreeMap::from([("frozen-objective".to_owned(), SummonTemplateId::new(7))]);
		let lowered = program
			.lower_with_actor_templates(&[], tick_rate, &templates)
			.expect("explicit actor-template binding must lower");
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected listener");
		};
		let ExpectedEffect::AddTileLayer(tile) = &listener.effects[0].effect else {
			panic!("expected tile field");
		};
		assert_eq!(tile.layer_threshold_reactions.len(), 1);
		let reaction = tile.layer_threshold_reactions[0];
		assert_eq!(reaction.threshold, 5);
		assert_eq!(
			reaction.transition,
			TileLayerThresholdTransition::ReachedFromBelow
		);
		let TileLayerThresholdReactionEffect::SpawnActor(spawn) = reaction.effect;
		assert_eq!(spawn.template, SummonTemplateId::new(7));
		assert_eq!(
			spawn.collision,
			TargetCellSpawnCollision::SuppressIfFriendlyOccupied
		);
		assert_eq!(
			spawn.ownership,
			TargetCellSpawnOwnership::TriggeringTileEffect
		);
	}

	#[test]
	fn semantic_saturated_reaction_requires_the_layer_cap() {
		let mut program: MechanicSemanticProgram = sowed_field_program();
		let SemanticAction::AddTileField(field) = &mut program.rules[0].action else {
			panic!("fixture must add a tile field");
		};
		field.max_layers_per_cell = SemanticValueExpression::Literal(ExpectedValue::from_whole(5));
		field.layer_threshold_reactions = vec![SemanticLayerThresholdReaction {
			threshold: SemanticValueExpression::Literal(ExpectedValue::from_whole(4)),
			transition: SemanticLayerThresholdTransition::SaturatedAttempt,
			effect: SemanticLayerThresholdReactionEffect::SpawnActor(
				SemanticTargetCellActorSpawn {
					actor_template_key: "frozen-objective".to_owned(),
					collision: SemanticTargetCellSpawnCollision::AllowCoexistence,
					ownership: SemanticTargetCellSpawnOwnership::Independent,
				},
			),
		}];
		let templates: BTreeMap<String, SummonTemplateId> =
			BTreeMap::from([("frozen-objective".to_owned(), SummonTemplateId::new(7))]);

		assert!(matches!(
			program.lower_with_actor_templates(
				&[],
				TickRate::new(NonZeroU32::new(30).expect("nonzero")),
				&templates,
			),
			Err(MechanicSemanticError::InvalidLayerThresholdReaction { .. })
		));
	}

	#[test]
	fn tile_field_overlay_lowers_periodic_damage_and_per_cell_leave_status() {
		let mechanic: MechanicId = MechanicId::new("fixture/snow-overlay");
		let program: MechanicSemanticProgram = MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: mechanic.clone(),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::SkillStarted,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Unlimited,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::InstallTileFieldOverlay(SemanticTileFieldOverlay {
					key: mechanic.clone(),
					observed_tile_key: MechanicId::new("fixture/base-snow"),
					observed_tile_max_layers: None,
					actor_filter: SemanticActorFilter::EnemyOfOwner,
					target_filter: NormalAttackTargetFilter::GroundOnly,
					leave_scope: TileFieldOverlayLeaveScope::ObservedCell,
					initial_delay_seconds: SemanticValueExpression::Literal(ExpectedValue::ONE),
					interval_seconds: SemanticValueExpression::Literal(ExpectedValue::ONE),
					lifetime: SemanticLifetime::SkillActivation,
					periodic_effects: vec![SemanticEffect::SourceStatScaledDamage {
						stat: StatKind::Attack,
						multiplier: SemanticValueExpression::Literal(
							ExpectedValue::from_ratio(1, 5).expect("valid ratio"),
						),
						damage_type: DamageType::Arts,
						delivery: DamageDelivery::Normal,
					}],
					on_leave_once: vec![SemanticEffect::ApplyStatus {
						kind: StatusKind::Cold,
						duration_seconds: SemanticValueExpression::Literal(
							ExpectedValue::from_whole(5),
						),
					}],
					layer_threshold_reactions: Vec::new(),
					cached_propagation: None,
				}),
				behavior: MechanicBehavior::SnowColdFreeze,
			}],
		};
		let lowered = program
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect("valid overlay semantic program");
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected skill-start listener");
		};
		assert_eq!(listener.trigger, ReplayTrigger::SkillStarted);
		let ExpectedEffect::InstallTileFieldOverlay(overlay) = &listener.effects[0].effect else {
			panic!("expected tile-field overlay installation");
		};
		assert_eq!(
			overlay.observed_tile_key,
			MechanicId::new("fixture/base-snow")
		);
		assert_eq!(
			overlay.leave_scope,
			TileFieldOverlayLeaveScope::ObservedCell
		);
		assert_eq!(overlay.initial_delay_ticks, 30);
		assert_eq!(overlay.interval_ticks, 30);
		assert_eq!(overlay.periodic_effects.len(), 1);
		assert_eq!(overlay.on_leave_once.len(), 1);
	}

	#[test]
	fn tile_field_overlay_threshold_reaction_requires_an_actor_template_binding() {
		let mechanic: MechanicId = MechanicId::new("fixture/skill-layer-reaction");
		let program: MechanicSemanticProgram = MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: mechanic.clone(),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::SkillStarted,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Unlimited,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::InstallTileFieldOverlay(SemanticTileFieldOverlay {
					key: mechanic,
					observed_tile_key: MechanicId::new("fixture/base-snow"),
					observed_tile_max_layers: Some(SemanticValueExpression::Literal(
						ExpectedValue::from_whole(5),
					)),
					actor_filter: SemanticActorFilter::EnemyOfOwner,
					target_filter: NormalAttackTargetFilter::GroundOnly,
					leave_scope: TileFieldOverlayLeaveScope::ObservedCell,
					initial_delay_seconds: SemanticValueExpression::Literal(ExpectedValue::ONE),
					interval_seconds: SemanticValueExpression::Literal(ExpectedValue::ONE),
					lifetime: SemanticLifetime::SkillActivation,
					periodic_effects: Vec::new(),
					on_leave_once: Vec::new(),
					layer_threshold_reactions: vec![SemanticLayerThresholdReaction {
						threshold: SemanticValueExpression::Literal(ExpectedValue::from_whole(5)),
						transition: SemanticLayerThresholdTransition::ReachedFromBelow,
						effect: SemanticLayerThresholdReactionEffect::SpawnActor(
							SemanticTargetCellActorSpawn {
								actor_template_key: "frozen-objective".to_owned(),
								collision:
									SemanticTargetCellSpawnCollision::SuppressIfFriendlyOccupied,
								ownership: SemanticTargetCellSpawnOwnership::SkillActivation,
							},
						),
					}],
					cached_propagation: None,
				}),
				behavior: MechanicBehavior::SnowColdFreeze,
			}],
		};
		let tick_rate: TickRate = TickRate::new(NonZeroU32::new(30).expect("nonzero"));
		assert!(matches!(
			program.lower(&[], tick_rate),
			Err(MechanicSemanticError::MissingActorTemplate { key, .. })
				if key == "frozen-objective"
		));

		let templates: BTreeMap<String, SummonTemplateId> =
			BTreeMap::from([("frozen-objective".to_owned(), SummonTemplateId::new(7))]);
		let lowered = program
			.lower_with_actor_templates(&[], tick_rate, &templates)
			.expect("bound overlay reaction must lower");
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected skill-start listener");
		};
		let ExpectedEffect::InstallTileFieldOverlay(overlay) = &listener.effects[0].effect else {
			panic!("expected tile-field overlay");
		};
		let [reaction] = overlay.layer_threshold_reactions.as_slice() else {
			panic!("expected exactly one overlay reaction");
		};
		assert_eq!(reaction.threshold, 5);
		assert_eq!(
			reaction.transition,
			TileLayerThresholdTransition::ReachedFromBelow
		);
		let TileLayerThresholdReactionEffect::SpawnActor(spawn) = reaction.effect;
		assert_eq!(spawn.template, SummonTemplateId::new(7));
		assert_eq!(spawn.ownership, TargetCellSpawnOwnership::SkillActivation);
	}

	#[test]
	fn value_expressions_bind_named_blackboard_values_fail_closed() {
		let mut program: MechanicSemanticProgram = sowed_field_program();
		let SemanticAction::AddTileField(field) = &mut program.rules[0].action else {
			panic!("fixture must add a tile field");
		};
		field.periodic.as_mut().expect("periodic").effects_per_layer[0] =
			super::SemanticEffect::Healing {
				amount: SemanticValueExpression::Blackboard("missing".to_owned()),
			};
		let error = program
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect_err("missing blackboard value must fail closed");
		assert!(error.to_string().contains("missing"));
	}

	#[test]
	fn lethal_recovery_and_status_rules_lower_to_listener_context_targets() {
		let program: MechanicSemanticProgram = MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: MechanicId::new("fixture/lethal_recovery"),
				provenance: provenance(),
				trigger: SemanticTrigger {
					event: SemanticEvent::BeforeLethal,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::ApplyEffects(vec![
					SemanticTargetedEffect {
						target: SemanticEffectTarget::Owner,
						effect: SemanticEffect::RestoreHpToCurrentMaxRatio {
							ratio: SemanticValueExpression::Literal(
								ExpectedValue::from_ratio(1, 2).expect("valid ratio"),
							),
						},
					},
					SemanticTargetedEffect {
						target: SemanticEffectTarget::EnemiesInOwnerCurrentAttackRange,
						effect: SemanticEffect::ApplyStatus {
							kind: StatusKind::Frozen,
							duration_seconds: SemanticValueExpression::Literal(
								ExpectedValue::from_whole(2),
							),
						},
					},
				]),
				behavior: MechanicBehavior::LethalRecoveryAndStatus,
			}],
		};
		let lowered = program
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect("valid lethal-recovery semantics");
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("expected listener");
		};
		assert_eq!(listener.trigger, ReplayTrigger::BeforeLethal);
		assert_eq!(listener.repeat, ReplayRepeat::Once);
		assert_eq!(listener.effects[0].target, EffectTarget::SelfActor);
		assert!(matches!(
			&listener.effects[0].effect,
			ExpectedEffect::RestoreHpToCurrentMaxRatio { .. }
		));
		assert_eq!(listener.effects[1].target, EffectTarget::EnemiesInRange);
		let ExpectedEffect::ApplyStatus(status) = &listener.effects[1].effect else {
			panic!("expected Frozen status");
		};
		assert_eq!(status.kind, StatusKind::Frozen);
		assert_eq!(status.duration_ticks, 60);
		assert_eq!(
			status.arts_resistance_reduction,
			ExpectedValue::from_ratio(15, 100).expect("valid ratio")
		);
		assert_eq!(
			status.arts_resistance_reduction_target,
			StatusFactionFilter::HostileOnly
		);
	}
}
