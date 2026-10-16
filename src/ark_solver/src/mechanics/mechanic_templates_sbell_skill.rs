//! Fail-closed decoder for the reusable layered-snowfield skill template.
//!
//! The skill is split across a common skill prefab and a character-mode prefab. Recognition uses
//! exact schemas, configurations, graph edges, custom-buff identities, and selected-skill metadata;
//! it never branches on operator identity. Lowering executes the supported attack and snow-overlay
//! subset and reports the remaining spread, token, and custom discard-effect policies.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{
	ArkDataSnapshot, SelectedSkillMetadata, SkillActivationType, SkillDurationType, SkillSpType,
};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_interpreter::{
		BattlePrefabAttackAbility, BattlePrefabGraph, BattlePrefabMonoBehaviour,
		BattlePrefabObjectSource, BattlePrefabRootSection, BattlePrefabSemantic,
		BattlePrefabSemanticIssue,
	},
	combat::{
		DamageDelivery, DamageType, ExpectedValue, MechanicBehavior, MechanicId, ReplayRepeat,
		SkillManualDiscardActivationLimit, StatKind, StatModifierBucket, StatusKind,
		TileFieldOverlayLeaveScope,
	},
	mechanic_semantics::{
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticActorFilter,
		SemanticBuffModifier, SemanticEffect, SemanticEffectTarget, SemanticEvent,
		SemanticLifetime, SemanticProvenance, SemanticSkillControllerLifecycle,
		SemanticSourceLocation, SemanticTargetedEffect, SemanticTileFieldOverlay, SemanticTrigger,
		SemanticTriggerFilter, SemanticValueExpression,
	},
	mechanic_templates_sbell::snowfield_mechanic_id,
	normal_attack::NormalAttackTargetFilter,
};

const COMMON_LOGICAL_PATH: &str = "[uc]skills";
const COMMON_ASSET_DIRECTORY: &str = "dyn/battle/prefabs/[uc]skills";
const MODE_PREFAB_NAME: &str = "S2";

const SWITCH_MODE_SCRIPT: &str = "94d0ebfda9ef298a570ee810608eff8f";
const SWITCH_MODE_SCHEMA: &str = "eb9a2d754589e3364f4b9b337fda93db";
const SKILL_CONTROLLER_SCRIPT: &str = "190f7d8906be6ffed7b30f1c0cc0fdf8";
const SKILL_CONTROLLER_SCHEMA: &str = "3e9f8f5c40b0fa2111323e099ba4d958";
const SKILL_EFFECTS_SCRIPT: &str = "47bb0d494c37b0fab5e248f3a41f9e1b";
const SKILL_EFFECTS_SCHEMA: &str = "25cc46718df8888025a8c18239b13dda";

const MODE_BINDER_SCRIPT: &str = "fef8a056370e18af843c5e5925ee80f8";
const MODE_BINDER_SCHEMA: &str = "237f52a73adae7e87e66a1372409859c";
const MODE_ANIMATOR_SCRIPT: &str = "e92edf708d4c4daad99c592816123ab1";
const MODE_ANIMATOR_SCHEMA: &str = "e78860b3f37e3b0f5ae573f707484380";
const ATTACK_SCRIPT: &str = "c1b44c3bb3490f6d64ea0cd1d609440c";
const ATTACK_SCHEMA: &str = "620b15910b23f3f2fc515105148ba9e2";
const EFFECT_HOOK_SCRIPT: &str = "33f2cf8eeccb67cd86fa554b93e178ad";
const EFFECT_HOOK_SCHEMA: &str = "78a80a284e4b68170bc79447219d63c3";
const EXTENDABLE_MARKER_SCRIPT: &str = "c385ccbc4766ba332e89aa830d922182";
const EXTENDABLE_MARKER_SCHEMA: &str = "be6ed90d722c123373912b37f0c813ef";
const TARGET_SELECTOR_SCRIPT: &str = "7f215a8296c7f945d539ac5d7db1ff7a";
const TARGET_SELECTOR_SCHEMA: &str = "0b01d694f140798a39efce8ee828e447";
const ATTACK_TRIGGER_SCRIPT: &str = "e79f9cdc1738eb7cf612b213533905cb";
const ATTACK_TRIGGER_SCHEMA: &str = "95d0b5928e12e465b31b080fb4682302";

const SWITCH_MODE_BUFF: &str = "sbell2_s2[switch_mode]";
const SWITCH_MODE_TEMPLATE: &str = "switch_mode_restart_fsm";
const SWITCH_BACK_BUFF: &str = "sbell2_s2[switch_back]";
const MARK_BUFF: &str = "sbell2_s2[mark]";
const TRAIT_OVERRIDE_BUFF: &str = "sbell2_trait_override";
const EFFECT_1_BUFF: &str = "sbell2_s2[effect_1]";
const EFFECT_2_BUFF: &str = "sbell2_s2[effect_2]";
const EFFECT_3_BUFF: &str = "sbell2_s2[effect_3]";
const EFFECT_4_BUFF: &str = "sbell2_s2[effect_4]";
const IDLE_EFFECT_BUFF: &str = "sbell2_s[idle_effect]";
const RANGE_EFFECT_BUFF: &str = "sbell2_s2[range_effect]";

pub const ATTACK_SCALE_BLACKBOARD_KEY: &str = "attack@atk_scale_s2";
pub const SNOW_DAMAGE_SCALE_BLACKBOARD_KEY: &str = "talent@s2_magic_scale";
pub const MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY: &str = "talent@max_cast_tile_count";
pub const COLD_DURATION_BLACKBOARD_KEY: &str = "talent@cold";
pub const FROZEN_TOKEN_BLACKBOARD_KEY: &str = "talent@token_key";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillTemplateEvidence {
	pub common_sources: Vec<SemanticProvenance>,
	pub mode_sources: Vec<SemanticProvenance>,
	pub attack_source: SemanticProvenance,
	pub snow_overlay_source: SemanticProvenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillSnowOverlayLifetime {
	WhileSkillModeActive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillSnowSpreadTrigger {
	SaturatedLayerAttempt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillSnowSpreadDestination {
	AdjacentWalkableLowlandCells,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillSnowSpreadSemantics {
	pub trigger: SbellSkillSnowSpreadTrigger,
	pub destination: SbellSkillSnowSpreadDestination,
	pub maximum_outside_temporary_cells: SemanticValueExpression,
	pub selected_level_maximum_outside_temporary_cells: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillPeriodicDamageDelivery {
	OncePerSecondWhileEnemyOccupiesSnow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillPeriodicDamageSemantics {
	pub source_stat: StatKind,
	pub scale: SemanticValueExpression,
	pub damage_type: DamageType,
	pub delivery: SbellSkillPeriodicDamageDelivery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillLeaveSnowSemantics {
	pub status: StatusKind,
	pub duration_seconds: SemanticValueExpression,
	pub leave_scope: TileFieldOverlayLeaveScope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Friendly ground objective spawned by the skill after a cell reaches the reviewed layer count.
/// Despite its data key, this is an actor token rather than a status applied to enemies.
pub struct SbellSkillFrozenObjectiveTokenSemantics {
	pub token_key: String,
	pub required_snow_layers: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillModeEntry {
	RestartFiniteStateMachine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillModeLifecycleSemantics {
	pub entry: SbellSkillModeEntry,
	pub controller: SemanticSkillControllerLifecycle,
	/// Serialized custom buff whose effect remains deliberately unmodeled.
	pub on_discard_custom_buff: MechanicId,
	pub indefinite_until_stopped: bool,
	pub attack_blackboard_mode_index: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillTargetSelection {
	AllMatchingEnemiesInCurrentAttackRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbellSkillTargetSelectionTiming {
	SelectForEachCast,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillAttackSemantics {
	pub attack_scale: SemanticValueExpression,
	pub damage_type: DamageType,
	pub target_selection: SbellSkillTargetSelection,
	pub target_selection_timing: SbellSkillTargetSelectionTiming,
	pub minimum_targets_to_cast: u16,
}

/// Exact operator-independent behavior recovered by composing the common skill and mode graphs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillTemplateSemantics {
	pub evidence: SbellSkillTemplateEvidence,
	pub observed_snowfield_key: MechanicId,
	pub overlay_lifetime: SbellSkillSnowOverlayLifetime,
	pub spread: SbellSkillSnowSpreadSemantics,
	pub periodic_damage: SbellSkillPeriodicDamageSemantics,
	pub on_leave_snow: SbellSkillLeaveSnowSemantics,
	pub frozen_objective_token: SbellSkillFrozenObjectiveTokenSemantics,
	pub lifecycle: SbellSkillModeLifecycleSemantics,
	pub attack: SbellSkillAttackSemantics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SbellSkillUnmodeledRequirement {
	SnowSpreadSourceOrderingAndContributionMerge,
	FrozenObjectiveTokenCollisionAndCardinality,
	FrozenObjectiveTokenOwnershipAndCleanup,
	FrozenObjectiveTokenLayerStateAfterSpawn,
	FrozenObjectiveTokenRetriggerAfterDeath,
	FrozenObjectiveTokenSameTickOrdering,
	UnknownOnDiscardCustomBuffSemantics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillUnmodeledGap {
	pub requirements: Vec<SbellSkillUnmodeledRequirement>,
}

impl fmt::Display for SbellSkillUnmodeledGap {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		let phrases: Vec<&'static str> =
			self.requirements
				.iter()
				.map(|requirement: &SbellSkillUnmodeledRequirement| {
					match requirement {
					SbellSkillUnmodeledRequirement::SnowSpreadSourceOrderingAndContributionMerge => {
						"snow-spread source ordering, destination merge, and budget contention"
					}
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenCollisionAndCardinality => {
						"frozen-objective token same-cell collision and global cardinality"
					}
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenOwnershipAndCleanup => {
						"frozen-objective token ownership and cleanup paths"
					}
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenLayerStateAfterSpawn => {
						"snow-layer retention, consumption, or reset after token creation"
					}
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenRetriggerAfterDeath => {
						"frozen-objective token retrigger behavior after token death"
					}
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenSameTickOrdering => {
						"same-tick ordering between token creation, spread, damage, and cleanup"
					}
					SbellSkillUnmodeledRequirement::UnknownOnDiscardCustomBuffSemantics => {
						"the custom switch-back buff executed by manual discard"
					}
				}
				})
				.collect();
		write!(
			formatter,
			"decoded layered-snowfield skill remains unmodeled for: {}",
			phrases.join("; ")
		)
	}
}

impl Error for SbellSkillUnmodeledGap {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedSbellSkillTemplate {
	pub mechanic_id: MechanicId,
	pub semantics: SbellSkillTemplateSemantics,
	pub consumed_skill_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_mode_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_semantic_fact_sources: Vec<BattlePrefabObjectSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillSemanticLowering {
	pub program: MechanicSemanticProgram,
	pub remaining_gap: SbellSkillUnmodeledGap,
}

impl DecodedSbellSkillTemplate {
	#[must_use]
	pub fn lower_program(&self) -> SbellSkillSemanticLowering {
		let attack_mechanic: MechanicId =
			MechanicId::new(format!("{}/active-attack", self.mechanic_id.as_str()));
		let target_count_mechanic: MechanicId =
			MechanicId::new(format!("{}/all-targets", self.mechanic_id.as_str()));
		let overlay_mechanic: MechanicId =
			MechanicId::new(format!("{}/snow-overlay", self.mechanic_id.as_str()));
		SbellSkillSemanticLowering {
			program: MechanicSemanticProgram {
				rules: vec![
					MechanicSemanticRule {
						mechanic: attack_mechanic.clone(),
						provenance: self.semantics.evidence.attack_source.clone(),
						trigger: SemanticTrigger {
							event: SemanticEvent::SkillStarted,
							filter: SemanticTriggerFilter::AffectedOwner,
							repeat: ReplayRepeat::Unlimited,
						},
						lifetime: SemanticLifetime::OwnerDeployment,
						action: SemanticAction::ApplyEffects(vec![SemanticTargetedEffect {
							target: SemanticEffectTarget::Owner,
							effect: SemanticEffect::ApplyBuff {
								key: attack_mechanic,
								max_stacks: SemanticValueExpression::Literal(ExpectedValue::ONE),
								lifetime: SemanticLifetime::SkillActivation,
								modifiers_per_stack: vec![SemanticBuffModifier::StatMultiplier {
									bucket: StatModifierBucket::Scale,
									stat: StatKind::Attack,
									multiplier: self.semantics.attack.attack_scale.clone(),
								}],
							},
						}]),
						behavior: MechanicBehavior::StatModifier,
					},
					MechanicSemanticRule {
						mechanic: target_count_mechanic.clone(),
						provenance: self.semantics.evidence.attack_source.clone(),
						trigger: SemanticTrigger {
							event: SemanticEvent::SkillStarted,
							filter: SemanticTriggerFilter::AffectedOwner,
							repeat: ReplayRepeat::Unlimited,
						},
						lifetime: SemanticLifetime::OwnerDeployment,
						action: SemanticAction::ApplyEffects(vec![SemanticTargetedEffect {
							target: SemanticEffectTarget::Owner,
							effect: SemanticEffect::ApplyBuff {
								key: target_count_mechanic,
								max_stacks: SemanticValueExpression::Literal(ExpectedValue::ONE),
								lifetime: SemanticLifetime::SkillActivation,
								modifiers_per_stack: vec![
									SemanticBuffModifier::TargetCountUnlimited,
								],
							},
						}]),
						behavior: MechanicBehavior::TargetCount,
					},
					MechanicSemanticRule {
						mechanic: overlay_mechanic.clone(),
						provenance: self.semantics.evidence.snow_overlay_source.clone(),
						trigger: SemanticTrigger {
							event: SemanticEvent::SkillStarted,
							filter: SemanticTriggerFilter::AffectedOwner,
							repeat: ReplayRepeat::Unlimited,
						},
						lifetime: SemanticLifetime::OwnerDeployment,
						action: SemanticAction::InstallTileFieldOverlay(SemanticTileFieldOverlay {
							key: overlay_mechanic,
							observed_tile_key: self.semantics.observed_snowfield_key.clone(),
							observed_tile_max_layers: None,
							actor_filter: SemanticActorFilter::EnemyOfOwner,
							target_filter: NormalAttackTargetFilter::GroundOnly,
							leave_scope: self.semantics.on_leave_snow.leave_scope,
							initial_delay_seconds: SemanticValueExpression::Literal(
								ExpectedValue::ONE,
							),
							interval_seconds: SemanticValueExpression::Literal(ExpectedValue::ONE),
							lifetime: SemanticLifetime::SkillActivation,
							periodic_effects: vec![SemanticEffect::SourceStatScaledDamage {
								stat: self.semantics.periodic_damage.source_stat,
								multiplier: self.semantics.periodic_damage.scale.clone(),
								damage_type: self.semantics.periodic_damage.damage_type,
								delivery: DamageDelivery::Normal,
							}],
							on_leave_once: vec![SemanticEffect::ApplyStatus {
								kind: self.semantics.on_leave_snow.status.clone(),
								duration_seconds: self
									.semantics
									.on_leave_snow
									.duration_seconds
									.clone(),
							}],
							layer_threshold_reactions: Vec::new(),
							cached_propagation: None,
						}),
						behavior: MechanicBehavior::SnowColdFreeze,
					},
				],
			},
			remaining_gap: SbellSkillUnmodeledGap {
				requirements: vec![
					SbellSkillUnmodeledRequirement::SnowSpreadSourceOrderingAndContributionMerge,
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenCollisionAndCardinality,
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenOwnershipAndCleanup,
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenLayerStateAfterSpawn,
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenRetriggerAfterDeath,
					SbellSkillUnmodeledRequirement::FrozenObjectiveTokenSameTickOrdering,
					SbellSkillUnmodeledRequirement::UnknownOnDiscardCustomBuffSemantics,
				],
			},
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SbellSkillTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl SbellSkillTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for SbellSkillTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"layered-snowfield skill template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for SbellSkillTemplateDecodeError {}

/// Recognizes and decodes the reviewed dual-root snowfield skill without operator-ID branching.
pub fn try_decode_sbell_skill_template(
	skill_graph: &BattlePrefabGraph,
	mode_graph: &BattlePrefabGraph,
	selected_skill: &SelectedSkillMetadata,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedSbellSkillTemplate>, SbellSkillTemplateDecodeError> {
	if !claims_sbell_skill_template(skill_graph) {
		return Ok(None);
	}
	validate_revisions(skill_graph, mode_graph, snapshot, gamedata_revision)?;
	let selected: SelectedSkillEvidence = validate_selected_skill(selected_skill, skill_graph)?;
	let skill_layout: CommonSkillLayout<'_> = validate_common_layout(skill_graph)?;
	let mode_layout: ModeLayout<'_> = validate_mode_layout(mode_graph)?;
	validate_common_configurations(&skill_layout)?;
	validate_mode_configurations(&mode_layout)?;
	validate_mode_edges(mode_graph, &mode_layout)?;
	validate_attack_fact(mode_graph, &mode_layout)?;
	let controller_lifecycle: SemanticSkillControllerLifecycle =
		decode_skill_controller_lifecycle(skill_layout.controller)?;
	let consumed_skill_issues: Vec<BattlePrefabSemanticIssue> =
		validate_common_issues(skill_graph, &skill_layout)?;
	let consumed_mode_issues: Vec<BattlePrefabSemanticIssue> =
		validate_mode_issues(mode_graph, &mode_layout)?;
	let common_sources: Vec<SemanticProvenance> =
		common_provenance(skill_graph, &skill_layout, gamedata_revision);
	let mode_sources: Vec<SemanticProvenance> =
		mode_provenance(mode_graph, &mode_layout, gamedata_revision);
	let snow_overlay_source: SemanticProvenance = common_sources[2].clone();
	let attack_source: SemanticProvenance = mode_sources[2].clone();
	let evidence: SbellSkillTemplateEvidence = SbellSkillTemplateEvidence {
		common_sources,
		mode_sources,
		attack_source,
		snow_overlay_source,
	};
	let semantics: SbellSkillTemplateSemantics = SbellSkillTemplateSemantics {
		evidence,
		observed_snowfield_key: snowfield_mechanic_id(&mode_graph.logical_path),
		overlay_lifetime: SbellSkillSnowOverlayLifetime::WhileSkillModeActive,
		spread: SbellSkillSnowSpreadSemantics {
			trigger: SbellSkillSnowSpreadTrigger::SaturatedLayerAttempt,
			destination: SbellSkillSnowSpreadDestination::AdjacentWalkableLowlandCells,
			maximum_outside_temporary_cells: SemanticValueExpression::Blackboard(
				MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY.to_owned(),
			),
			selected_level_maximum_outside_temporary_cells: selected
				.maximum_outside_temporary_cells,
		},
		periodic_damage: SbellSkillPeriodicDamageSemantics {
			source_stat: StatKind::Attack,
			scale: SemanticValueExpression::Blackboard(SNOW_DAMAGE_SCALE_BLACKBOARD_KEY.to_owned()),
			damage_type: DamageType::Arts,
			delivery: SbellSkillPeriodicDamageDelivery::OncePerSecondWhileEnemyOccupiesSnow,
		},
		on_leave_snow: SbellSkillLeaveSnowSemantics {
			status: StatusKind::Cold,
			duration_seconds: SemanticValueExpression::Blackboard(
				COLD_DURATION_BLACKBOARD_KEY.to_owned(),
			),
			leave_scope: TileFieldOverlayLeaveScope::ObservedCell,
		},
		frozen_objective_token: SbellSkillFrozenObjectiveTokenSemantics {
			token_key: selected.token_key,
			required_snow_layers: 5,
		},
		lifecycle: SbellSkillModeLifecycleSemantics {
			entry: SbellSkillModeEntry::RestartFiniteStateMachine,
			controller: controller_lifecycle,
			on_discard_custom_buff: MechanicId::new(SWITCH_BACK_BUFF),
			indefinite_until_stopped: true,
			attack_blackboard_mode_index: 1,
		},
		attack: SbellSkillAttackSemantics {
			attack_scale: SemanticValueExpression::Blackboard(
				ATTACK_SCALE_BLACKBOARD_KEY.to_owned(),
			),
			damage_type: DamageType::Arts,
			target_selection: SbellSkillTargetSelection::AllMatchingEnemiesInCurrentAttackRange,
			target_selection_timing: SbellSkillTargetSelectionTiming::SelectForEachCast,
			minimum_targets_to_cast: 1,
		},
	};
	Ok(Some(DecodedSbellSkillTemplate {
		mechanic_id: MechanicId::new(format!(
			"{}/template/layered-snowfield-skill",
			selected_skill.skill_id()
		)),
		semantics,
		consumed_skill_issues,
		consumed_mode_issues,
		consumed_semantic_fact_sources: vec![mode_layout.attack.source.clone()],
	}))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedSkillEvidence {
	token_key: String,
	maximum_outside_temporary_cells: u16,
}

#[derive(Clone, Copy)]
struct CommonSkillLayout<'a> {
	switch_mode: &'a BattlePrefabMonoBehaviour,
	controller: &'a BattlePrefabMonoBehaviour,
	effects: &'a BattlePrefabMonoBehaviour,
}

#[derive(Clone)]
struct ModeLayout<'a> {
	binder: &'a BattlePrefabMonoBehaviour,
	animator: &'a BattlePrefabMonoBehaviour,
	attack: &'a BattlePrefabMonoBehaviour,
	effect_hook: &'a BattlePrefabMonoBehaviour,
	extendable_marker: &'a BattlePrefabMonoBehaviour,
	target_selector: &'a BattlePrefabMonoBehaviour,
	attack_trigger: &'a BattlePrefabMonoBehaviour,
	body_transform: BattlePrefabObjectSource,
}

pub(crate) fn claims_sbell_skill_template(graph: &BattlePrefabGraph) -> bool {
	graph
		.mono_behaviours
		.iter()
		.any(|behavior: &BattlePrefabMonoBehaviour| {
			behavior.buffs.iter().any(|buff| {
				matches!(
					buff.buff_key.as_deref(),
					Some(
						SWITCH_MODE_BUFF
							| SWITCH_BACK_BUFF | MARK_BUFF
							| TRAIT_OVERRIDE_BUFF | EFFECT_1_BUFF
							| EFFECT_2_BUFF | EFFECT_3_BUFF
							| EFFECT_4_BUFF | IDLE_EFFECT_BUFF
							| RANGE_EFFECT_BUFF
					)
				)
			})
		})
}

fn validate_revisions(
	skill_graph: &BattlePrefabGraph,
	mode_graph: &BattlePrefabGraph,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<(), SbellSkillTemplateDecodeError> {
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(SbellSkillTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if skill_graph.mechanics_revision != mode_graph.mechanics_revision {
		return Err(SbellSkillTemplateDecodeError::new(
			"mechanics revision",
			format!(
				"common graph is {:?}, mode graph is {:?}",
				skill_graph.mechanics_revision, mode_graph.mechanics_revision
			),
		));
	}
	Ok(())
}

fn validate_selected_skill(
	selected_skill: &SelectedSkillMetadata,
	skill_graph: &BattlePrefabGraph,
) -> Result<SelectedSkillEvidence, SbellSkillTemplateDecodeError> {
	let prefab_id: &str = selected_skill.prefab_id().ok_or_else(|| {
		SbellSkillTemplateDecodeError::new("selected skill", "missing common skill prefab ID")
	})?;
	if prefab_id != skill_graph.prefab_name {
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill",
			format!(
				"prefab ID {prefab_id:?} does not match common root {:?}",
				skill_graph.prefab_name
			),
		));
	}
	if selected_skill.override_prefab_key().is_some() || selected_skill.range_id().is_some() {
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill",
			format!(
				"expected no prefab/range override, found prefab={:?}, range={:?}",
				selected_skill.override_prefab_key(),
				selected_skill.range_id()
			),
		));
	}
	if selected_skill.activation_type() != &SkillActivationType::Auto
		|| selected_skill.duration_type() != &SkillDurationType::None
		|| selected_skill.duration_seconds() != -1.0
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill lifecycle",
			format!(
				"expected AUTO/NONE/-1, found {:?}/{:?}/{}",
				selected_skill.activation_type(),
				selected_skill.duration_type(),
				selected_skill.duration_seconds()
			),
		));
	}
	let sp = selected_skill.sp();
	if sp.sp_type() != &SkillSpType::IncreaseWithTime
		|| sp.max_charges() != 1
		|| !sp.increment().is_finite()
		|| sp.increment() <= 0.0
		|| sp.initial() > sp.cost()
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill SP",
			format!(
				"expected positive time recovery, one charge, and initial<=cost; found {:?}, max={}, increment={}, initial={}, cost={}",
				sp.sp_type(),
				sp.max_charges(),
				sp.increment(),
				sp.initial(),
				sp.cost()
			),
		));
	}

	let mut values: BTreeMap<&str, (f64, Option<&str>)> = BTreeMap::new();
	for entry in selected_skill.blackboard() {
		if values
			.insert(entry.key(), (entry.value(), entry.value_string()))
			.is_some()
		{
			return Err(SbellSkillTemplateDecodeError::new(
				"selected skill blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: Vec<&str> = vec![
		ATTACK_SCALE_BLACKBOARD_KEY,
		COLD_DURATION_BLACKBOARD_KEY,
		MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY,
		SNOW_DAMAGE_SCALE_BLACKBOARD_KEY,
		FROZEN_TOKEN_BLACKBOARD_KEY,
	];
	if values.keys().copied().collect::<Vec<_>>() != expected_keys {
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				values.keys().collect::<Vec<_>>()
			),
		));
	}
	for key in [
		ATTACK_SCALE_BLACKBOARD_KEY,
		COLD_DURATION_BLACKBOARD_KEY,
		SNOW_DAMAGE_SCALE_BLACKBOARD_KEY,
		MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY,
	] {
		let (value, value_string): (f64, Option<&str>) = values[key];
		if !value.is_finite() || value < 0.0 || value_string.is_some() {
			return Err(SbellSkillTemplateDecodeError::new(
				"selected skill blackboard",
				format!(
					"numeric entry {key:?} must be finite, nonnegative, and string-free; found {value}, {value_string:?}"
				),
			));
		}
	}
	if values[ATTACK_SCALE_BLACKBOARD_KEY].0 == 0.0 {
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill blackboard",
			"attack scale must be positive",
		));
	}
	let maximum_outside_temporary_cells: f64 = values[MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY].0;
	if maximum_outside_temporary_cells.fract() != 0.0
		|| !(1.0..=f64::from(u16::MAX)).contains(&maximum_outside_temporary_cells)
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill blackboard",
			format!(
				"maximum outside-range temporary cells must be a positive u16, found {maximum_outside_temporary_cells}"
			),
		));
	}
	let (token_numeric, token_string): (f64, Option<&str>) = values[FROZEN_TOKEN_BLACKBOARD_KEY];
	let override_token: &str = selected_skill.override_token_key().ok_or_else(|| {
		SbellSkillTemplateDecodeError::new("selected skill token", "missing token override")
	})?;
	if token_numeric != 0.0 || token_string != Some(override_token) {
		return Err(SbellSkillTemplateDecodeError::new(
			"selected skill token",
			format!(
				"expected zero numeric value and string matching override {override_token:?}, found {token_numeric}, {token_string:?}"
			),
		));
	}
	Ok(SelectedSkillEvidence {
		token_key: override_token.to_owned(),
		maximum_outside_temporary_cells: maximum_outside_temporary_cells as u16,
	})
}

fn validate_common_layout(
	graph: &BattlePrefabGraph,
) -> Result<CommonSkillLayout<'_>, SbellSkillTemplateDecodeError> {
	let expected_asset_path: String =
		format!("{COMMON_ASSET_DIRECTORY}/{}.prefab", graph.prefab_name);
	if graph.logical_path != COMMON_LOGICAL_PATH
		|| graph.asset_path != expected_asset_path
		|| graph.section != BattlePrefabRootSection::MainAsset
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"common root identity",
			format!(
				"expected logical path {COMMON_LOGICAL_PATH:?}, asset {expected_asset_path:?}, MainAsset; found {:?}, {:?}, {:?}",
				graph.logical_path, graph.asset_path, graph.section
			),
		));
	}
	if graph.root_game_object_indices.len() != 1
		|| graph.game_objects.len() != 1
		|| graph.mono_behaviours.len() != 3
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"common graph shape",
			format!(
				"expected one root/GameObject and three behaviors, found roots={}, GameObjects={}, behaviors={}",
				graph.root_game_object_indices.len(),
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	let root_index: usize = graph.root_game_object_indices[0];
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		SbellSkillTemplateDecodeError::new(
			"common graph root",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	if root.name != graph.prefab_name
		|| root.hierarchy_path != graph.prefab_name
		|| !root.child_indices.is_empty()
		|| root.mono_behaviour_indices.len() != 3
		|| root.component_sources.len() != 4
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"common graph root",
			format!(
				"expected leaf {:?} with one Transform and three behaviors, found name={:?}, path={:?}, children={}, behaviors={}, components={}",
				graph.prefab_name,
				root.name,
				root.hierarchy_path,
				root.child_indices.len(),
				root.mono_behaviour_indices.len(),
				root.component_sources.len()
			),
		));
	}
	Ok(CommonSkillLayout {
		switch_mode: exact_owned_behavior(
			graph,
			root_index,
			SWITCH_MODE_SCRIPT,
			SWITCH_MODE_SCHEMA,
			"switch-mode ability",
		)?,
		controller: exact_owned_behavior(
			graph,
			root_index,
			SKILL_CONTROLLER_SCRIPT,
			SKILL_CONTROLLER_SCHEMA,
			"skill controller",
		)?,
		effects: exact_owned_behavior(
			graph,
			root_index,
			SKILL_EFFECTS_SCRIPT,
			SKILL_EFFECTS_SCHEMA,
			"skill effect buffs",
		)?,
	})
}

fn validate_mode_layout(
	graph: &BattlePrefabGraph,
) -> Result<ModeLayout<'_>, SbellSkillTemplateDecodeError> {
	if graph.section != BattlePrefabRootSection::Modes || graph.prefab_name != MODE_PREFAB_NAME {
		return Err(SbellSkillTemplateDecodeError::new(
			"mode root identity",
			format!(
				"expected Modes/{MODE_PREFAB_NAME}, found {:?}/{:?}",
				graph.section, graph.prefab_name
			),
		));
	}
	if graph.root_game_object_indices.len() != 1
		|| graph.game_objects.len() != 4
		|| graph.mono_behaviours.len() != 7
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"mode graph shape",
			format!(
				"expected one root, four GameObjects, and seven behaviors; found roots={}, GameObjects={}, behaviors={}",
				graph.root_game_object_indices.len(),
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	let root_index: usize = graph.root_game_object_indices[0];
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		SbellSkillTemplateDecodeError::new(
			"mode graph root",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	let body_index: usize = exact_child(graph, root_index, "Body", "mode Body")?;
	let attack_index: usize = exact_child(graph, body_index, "Attack", "mode Attack")?;
	let selector_index: usize = exact_child(graph, attack_index, "Selector", "mode Selector")?;
	let body = &graph.game_objects[body_index];
	let attack_object = &graph.game_objects[attack_index];
	let selector_object = &graph.game_objects[selector_index];
	if root.name != MODE_PREFAB_NAME
		|| root.hierarchy_path != format!("Modes/{MODE_PREFAB_NAME}#0")
		|| root.mono_behaviour_indices.len() != 2
		|| root.component_sources.len() != 3
		|| !body.mono_behaviour_indices.is_empty()
		|| body.component_sources.len() != 1
		|| attack_object.mono_behaviour_indices.len() != 2
		|| attack_object.component_sources.len() != 3
		|| selector_object.mono_behaviour_indices.len() != 3
		|| selector_object.component_sources.len() != 4
		|| !selector_object.child_indices.is_empty()
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"mode hierarchy",
			format!(
				"expected S2(2 behaviors)/Body(0)/Attack(2)/Selector(3) chain with component counts 3/1/3/4; found root={:?}, root_path={:?}, behavior counts={}/{}/{}/{}, component counts={}/{}/{}/{}, selector children={}",
				root.name,
				root.hierarchy_path,
				root.mono_behaviour_indices.len(),
				body.mono_behaviour_indices.len(),
				attack_object.mono_behaviour_indices.len(),
				selector_object.mono_behaviour_indices.len(),
				root.component_sources.len(),
				body.component_sources.len(),
				attack_object.component_sources.len(),
				selector_object.component_sources.len(),
				selector_object.child_indices.len()
			),
		));
	}
	Ok(ModeLayout {
		binder: exact_owned_behavior(
			graph,
			root_index,
			MODE_BINDER_SCRIPT,
			MODE_BINDER_SCHEMA,
			"mode binder",
		)?,
		animator: exact_owned_behavior(
			graph,
			root_index,
			MODE_ANIMATOR_SCRIPT,
			MODE_ANIMATOR_SCHEMA,
			"mode animator",
		)?,
		attack: exact_owned_behavior(
			graph,
			attack_index,
			ATTACK_SCRIPT,
			ATTACK_SCHEMA,
			"mode attack",
		)?,
		effect_hook: exact_owned_behavior(
			graph,
			attack_index,
			EFFECT_HOOK_SCRIPT,
			EFFECT_HOOK_SCHEMA,
			"mode effect hook",
		)?,
		extendable_marker: exact_owned_behavior(
			graph,
			selector_index,
			EXTENDABLE_MARKER_SCRIPT,
			EXTENDABLE_MARKER_SCHEMA,
			"mode extendable marker",
		)?,
		target_selector: exact_owned_behavior(
			graph,
			selector_index,
			TARGET_SELECTOR_SCRIPT,
			TARGET_SELECTOR_SCHEMA,
			"mode target selector",
		)?,
		attack_trigger: exact_owned_behavior(
			graph,
			selector_index,
			ATTACK_TRIGGER_SCRIPT,
			ATTACK_TRIGGER_SCHEMA,
			"mode attack trigger",
		)?,
		body_transform: body.component_sources[0].clone(),
	})
}

fn exact_child(
	graph: &BattlePrefabGraph,
	owner_index: usize,
	name: &str,
	component: &'static str,
) -> Result<usize, SbellSkillTemplateDecodeError> {
	let owner = &graph.game_objects[owner_index];
	if owner.child_indices.len() != 1 {
		return Err(SbellSkillTemplateDecodeError::new(
			component,
			format!(
				"expected one child named {name:?}, found {} children",
				owner.child_indices.len()
			),
		));
	}
	let child_index: usize = owner.child_indices[0];
	let child = graph.game_objects.get(child_index).ok_or_else(|| {
		SbellSkillTemplateDecodeError::new(
			component,
			format!("child index {child_index} is outside the GameObject table"),
		)
	})?;
	if child.name != name || child.hierarchy_path != format!("{}/{name}", owner.hierarchy_path) {
		return Err(SbellSkillTemplateDecodeError::new(
			component,
			format!(
				"expected child {name:?} below {:?}, found name={:?}, path={:?}",
				owner.hierarchy_path, child.name, child.hierarchy_path
			),
		));
	}
	Ok(child_index)
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_hex: &str,
	schema_hex: &str,
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, SbellSkillTemplateDecodeError> {
	let script_id: [u8; 16] = parse_id(script_hex);
	let old_type_hash: [u8; 16] = parse_id(schema_hex);
	let owner = &graph.game_objects[owner_index];
	let matches: Vec<&BattlePrefabMonoBehaviour> = owner
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index: &usize| graph.mono_behaviours.get(index))
		.filter(|behavior: &&BattlePrefabMonoBehaviour| {
			behavior.script_id == Some(script_id) && behavior.old_type_hash == Some(old_type_hash)
		})
		.collect();
	if matches.len() != 1 {
		return Err(SbellSkillTemplateDecodeError::new(
			component,
			format!(
				"expected one behavior with script {script_hex} and schema {schema_hex}, found {}",
				matches.len()
			),
		));
	}
	let behavior: &BattlePrefabMonoBehaviour = matches[0];
	if behavior.game_object_index != Some(owner_index) {
		return Err(SbellSkillTemplateDecodeError::new(
			component,
			format!(
				"expected behavior owned by GameObject {owner_index}, found {:?}",
				behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

fn validate_common_configurations(
	layout: &CommonSkillLayout<'_>,
) -> Result<(), SbellSkillTemplateDecodeError> {
	validate_configuration(
		layout.switch_mode,
		"switch-mode ability configuration",
		json!({
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [expected_common_buff(CommonBuffExpectation::SwitchMode)],
			"_checkBuffEarlyFinish": 0,
			"_enableUpdateCooldown": 0,
			"_escapeTime": 0.0,
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_ignoreInfiniteBuffCooldown": 0,
			"_interruptAbilityOnDetach": 1,
			"_interuptIfTargetDead": 0,
			"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
			"_passiveBuffs": [],
			"_preDelay": 0.0,
			"_resetCdStrategy": 1,
			"_selector": {"m_FileID":0, "m_PathID":0},
			"_updateLifetimeWhenSpellStart": 0,
			"_waitForAttackEvent": 1,
			"_waitForCasting": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	validate_configuration(
		layout.controller,
		"skill controller configuration",
		merge_configuration_parts(vec![
			json!({
				"_allowNoTarget": 1,
				"_allowSpRecoveryWhenAffecting": 0,
				"_attachSignalIds": 0,
				"_attackBlackboardModeIndex": 1,
				"_availableValidator": {"m_FileID":0, "m_PathID":0},
				"_beginAnim": "Skill_2_Begin",
				"_beginAnimDown": "",
				"_beginAnimValidator": {"m_FileID":0, "m_PathID":0},
				"_beginEffect": [],
				"_buffsWhenDiscard": [expected_common_buff(CommonBuffExpectation::SwitchBack)],
				"_canCastDuringBorn": 1,
				"_canCastWithNoSp": 0,
				"_canSilenced": 0,
				"_canSwitchDuringBorn": 1,
				"_canUseInAbnormalState": 0,
				"_checkFinishedBuffWhenStopAffect": 0,
				"_checkHasTargetBeforeDoCast": 0,
				"_defaultModeIndex": -1,
				"_discardableInDoze": 0,
				"_earlySkillFinishAtAttackFinished": 0,
				"_extraAbilities": [],
				"_extraAttackBlackboardModeIndices": [],
				"_extraRangeIdModeIndices": [],
				"_forceToShowRange": 0,
				"_forceUseBaseShowRange": 0,
				"_hasUniStackColor": 0,
				"_hidden": 0,
				"_hideHudSkillStackCount": 0,
				"_hideRangeToShow": 0,
				"_hideSkillStackCount": 0
			}),
			json!({
				"_isOverloadSkill": 0,
				"_isRemoteControlled": 0,
				"_limitGlobalTriggerTime": 0,
				"_maxTriggerTime": -1,
				"_onlyAvailableWhenTokenValid": 0,
				"_playCharWordVoice": 1,
				"_playCharWordWithPassiveType": 0,
				"_playSkillBeginAnim": 1,
				"_rangeIdModeIndex": -1,
				"_rangeIdUsageType": 7,
				"_rangeToShow": {"m_FileID":0, "m_PathID":0},
				"_recoverSpIfNoTarget": 0,
				"_resetAbilityCooldownWhenCastEnd": 0,
				"_shouldCastLikeAttack": 0,
				"_showProgressToNext": 0,
				"_showSpAsBulletMode": 0,
				"_showSpBarEvenHidden": 0,
				"_showSpRatioAsProgress": 0,
				"_suspendableLimitedTimes": -1,
				"_switchToState": 0,
				"_trigger": {"m_FileID":0, "m_PathID":0},
				"_uninterruptibleOnAbilityPredelay": 1,
				"_useEscapeTime": 0,
				"_useFaceVector": 0,
				"_useTriggerInManualMode": 0,
				"_writeDurationToAttackBlackboard": 0,
				"m_Enabled": 1,
				"m_Name": ""
			}),
		]),
	)?;
	validate_configuration(
		layout.effects,
		"skill effect-buff configuration",
		json!({
			"_buffs": [
				expected_common_buff(CommonBuffExpectation::Mark),
				expected_common_buff(CommonBuffExpectation::TraitOverride),
				expected_common_buff(CommonBuffExpectation::Effect1),
				expected_common_buff(CommonBuffExpectation::Effect2),
				expected_common_buff(CommonBuffExpectation::Effect3),
				expected_common_buff(CommonBuffExpectation::Effect4),
				expected_common_buff(CommonBuffExpectation::IdleEffect),
				expected_common_buff(CommonBuffExpectation::RangeEffect)
			],
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	for (component, behavior) in [
		("switch-mode ability", layout.switch_mode),
		("skill controller", layout.controller),
		("skill effect buffs", layout.effects),
	] {
		if !behavior.references.is_empty() || !behavior.semantic_facts.is_empty() {
			return Err(SbellSkillTemplateDecodeError::new(
				component,
				format!(
					"expected no non-null graph edges or semantic facts, found edges={:?}, facts={:?}",
					behavior.references,
					behavior
						.semantic_facts
						.iter()
						.map(|fact| fact.kind())
						.collect::<Vec<_>>()
				),
			));
		}
	}
	Ok(())
}

fn decode_skill_controller_lifecycle(
	controller: &BattlePrefabMonoBehaviour,
) -> Result<SemanticSkillControllerLifecycle, SbellSkillTemplateDecodeError> {
	const COMPONENT: &str = "skill controller lifecycle";
	if controller.script_id != Some(parse_id(SKILL_CONTROLLER_SCRIPT))
		|| controller.old_type_hash != Some(parse_id(SKILL_CONTROLLER_SCHEMA))
	{
		return Err(SbellSkillTemplateDecodeError::new(
			COMPONENT,
			"controller script/schema identity does not match the verified lifecycle schema",
		));
	}
	let fields: &Map<String, Value> = controller.raw_fields.as_object().ok_or_else(|| {
		SbellSkillTemplateDecodeError::new(COMPONENT, "raw TypeTree fields are not an object")
	})?;
	let integer = |field: &'static str| -> Result<i64, SbellSkillTemplateDecodeError> {
		fields.get(field).and_then(Value::as_i64).ok_or_else(|| {
			SbellSkillTemplateDecodeError::new(
				COMPONENT,
				format!("{field} must be a serialized integer"),
			)
		})
	};
	let flag = |field: &'static str| -> Result<bool, SbellSkillTemplateDecodeError> {
		match integer(field)? {
			0 => Ok(false),
			1 => Ok(true),
			value => Err(SbellSkillTemplateDecodeError::new(
				COMPONENT,
				format!("{field} must be 0 or 1, found {value}"),
			)),
		}
	};
	let manual_discard_limit: SkillManualDiscardActivationLimit =
		match integer("_suspendableLimitedTimes")? {
			-1 => SkillManualDiscardActivationLimit::Unlimited,
			value if value >= 0 => SkillManualDiscardActivationLimit::FirstNActivations(
				u32::try_from(value).map_err(|_| {
					SbellSkillTemplateDecodeError::new(
						COMPONENT,
						"_suspendableLimitedTimes exceeds u32",
					)
				})?,
			),
			value => {
				return Err(SbellSkillTemplateDecodeError::new(
					COMPONENT,
					format!("_suspendableLimitedTimes has unsupported value {value}"),
				));
			}
		};
	Ok(SemanticSkillControllerLifecycle {
		manual_discard_limit,
		allow_manual_discard_while_dozing: flag("_discardableInDoze")?,
		allow_sp_recovery_while_active: flag("_allowSpRecoveryWhenAffecting")?,
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommonBuffExpectation {
	SwitchMode,
	SwitchBack,
	Mark,
	TraitOverride,
	Effect1,
	Effect2,
	Effect3,
	Effect4,
	IdleEffect,
	RangeEffect,
}

fn expected_common_buff(expectation: CommonBuffExpectation) -> Value {
	let (key, template, lifetime_type): (&str, &str, i64) = match expectation {
		CommonBuffExpectation::SwitchMode => (SWITCH_MODE_BUFF, SWITCH_MODE_TEMPLATE, 2),
		CommonBuffExpectation::SwitchBack => (SWITCH_BACK_BUFF, SWITCH_BACK_BUFF, 0),
		CommonBuffExpectation::Mark => (MARK_BUFF, "empty", 2),
		CommonBuffExpectation::TraitOverride => (TRAIT_OVERRIDE_BUFF, "empty", 2),
		CommonBuffExpectation::Effect1 => (EFFECT_1_BUFF, "empty", 2),
		CommonBuffExpectation::Effect2 => (EFFECT_2_BUFF, "empty", 2),
		CommonBuffExpectation::Effect3 => (EFFECT_3_BUFF, "empty", 2),
		CommonBuffExpectation::Effect4 => (EFFECT_4_BUFF, "empty", 2),
		CommonBuffExpectation::IdleEffect => (IDLE_EFFECT_BUFF, IDLE_EFFECT_BUFF, 2),
		CommonBuffExpectation::RangeEffect => (RANGE_EFFECT_BUFF, "empty", 2),
	};
	let mut buff: Value = merge_configuration_parts(vec![
		json!({
			"attributes": {
				"abnormalAntis": [],
				"abnormalComboImmunes": [],
				"abnormalCombos": [],
				"abnormalFlags": [],
				"abnormalImmunes": [],
				"attributeModifiers": []
			},
			"audioSignal": "",
			"blackboard": [],
			"buffKey": key,
			"clearAllStackCntWhenTimeUp": 0,
			"disableOverride": 0,
			"durationKey": "duration",
			"enableInitDirectionFromSource": 0,
			"firstTriggerInterval": -1.0,
			"independentCharacterSource": 0,
			"isDamageMissable": 0,
			"isDurableBuff": 0,
			"isFreezable": 0,
			"isLevitatable": 0,
			"isSilenceable": 0,
			"isStunnable": 0,
			"lifeTime": 0.0,
			"lifeTimeType": lifetime_type
		}),
		json!({
			"loadFromDB": 0,
			"maxStackCnt": 1,
			"maxValidStackCnt": -1,
			"onEventPriority": 0,
			"overrideEffectKey": "",
			"overrideKey": "",
			"overrideOnEventPriority": 0,
			"overrideType": 0,
			"priority": -1,
			"priorityBBKeys": [],
			"refreshRemainingTimeWhenStackMax": 0,
			"statusResistable": 2,
			"stripBlackboardParamsWithBuffKey": 0,
			"takeSnapshotWhenExtend": 0,
			"templateKey": template,
			"triggerCnt": 0,
			"triggerInterval": -1.0,
			"triggerLifeType": 0,
			"waitFirstTriggerInterval": 1
		}),
	]);
	match expectation {
		CommonBuffExpectation::TraitOverride => {
			buff["blackboard"] = json!([{"key":"priority_flag", "value":1.0, "valueStr":""}]);
			buff["overrideKey"] = json!("sbell2_trait");
			buff["priorityBBKeys"] = json!(["priority_flag"]);
		}
		CommonBuffExpectation::Effect1 => {
			buff["overrideEffectKey"] = json!("sbell2_skill_02_buff_01");
		}
		CommonBuffExpectation::Effect2 => {
			buff["overrideEffectKey"] = json!("sbell2_skill_02_buff_02");
		}
		CommonBuffExpectation::Effect3 => {
			buff["overrideEffectKey"] = json!("sbell2_skill_02_buff_03");
		}
		CommonBuffExpectation::Effect4 => {
			buff["overrideEffectKey"] = json!("sbell2_skill_02_buff_04");
		}
		CommonBuffExpectation::RangeEffect => {
			buff["overrideEffectKey"] = json!("sbell2_range");
		}
		CommonBuffExpectation::SwitchMode
		| CommonBuffExpectation::SwitchBack
		| CommonBuffExpectation::Mark
		| CommonBuffExpectation::IdleEffect => {}
	}
	buff
}

fn validate_mode_configurations(
	layout: &ModeLayout<'_>,
) -> Result<(), SbellSkillTemplateDecodeError> {
	validate_configuration(
		layout.binder,
		"mode binder configuration",
		json!({
			"_addIndexToSignalId": 1,
			"_alwaysHideHp": {"isDefined":0, "value":0},
			"_alwaysHideSp": {"isDefined":0, "value":0},
			"_animatorHooker": {"m_FileID":0, "m_PathID":layout.animator.source.path_id},
			"_attack": {"m_FileID":0, "m_PathID":layout.attack.source.path_id},
			"_attackTrigger": {"m_FileID":0, "m_PathID":layout.attack_trigger.source.path_id},
			"_bodyTransform": {"m_FileID":0, "m_PathID":layout.body_transform.path_id},
			"_combat": {"m_FileID":0, "m_PathID":0},
			"_extraRangesToShow": [],
			"_generalAbilities": [],
			"_hideUIAttackRange": 0,
			"_includeIndexZero": 1,
			"_overrideDeadEffect": "",
			"_overrideStartEffect": "",
			"_rangeToShow": {"m_FileID":0, "m_PathID":0},
			"_updateAttackRangeExtendByAttributeDirty": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	validate_configuration(
		layout.animator,
		"mode animator configuration",
		json!({
			"_disableSwitchSettings": [],
			"_excludeAnimKeys": ["Born"],
			"_replaceAnimPairs": [{"fromAnimKey":"Idle", "toAnimKey":"Skill_2_Idle"}],
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	validate_configuration(
		layout.attack,
		"mode attack configuration",
		merge_configuration_parts(vec![
			json!({
				"_activeBuffs": [],
				"_affectedBySlowDown": 0,
				"_allowNoTarget": 0,
				"_alwaysIncludeTarget": 1,
				"_alwaysPutIncludeTargetFirst": 0,
				"_animKey": "Skill_2_Attack",
				"_atkScale": 1.0,
				"_atkScaleKey": "atk_scale_s2",
				"_attachPassiveBuffsOnDummy": 1,
				"_attackType": 1,
				"_checkFaceToFirstCastTargetDownAttack": 0,
				"_cooldown": 0.0,
				"_cooldownKey": "duration",
				"_damageType": 2,
				"_deliveryParaWhenReplaceActionNode": 0,
				"_downAnimKey": "",
				"_downEndAnimKey": "",
				"_elementDamageType": 0,
				"_endAnimKey": "Idle",
				"_epDamageRatio": 0.0,
				"_escapeTime": 0.0,
				"_extraDamageType": 0,
				"_faceToDefault": 0,
				"_faceToFirstCastTarget": 0,
				"_faceToFront": 0,
				"_faceToTarget": 0
			}),
			json!({
				"_forceAsDmgOrHealAbility": 0,
				"_forceFaceToDirection": 0,
				"_ignoreIfOwnerDead": 0,
				"_ignorePalsyInterrupt": 0,
				"_interruptAbilityOnDetach": 1,
				"_interuptIfTargetDead": 0,
				"_maxAnimScale": 1.0,
				"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
				"_onlyPlayEndAnimWhenIsLastAbility": 0,
				"_preDelay": 0.0,
				"_resetCdStrategy": 1,
				"_selectTargetSource": 1,
				"_selectTargetTiming": 1,
				"_selector": {"m_FileID":0, "m_PathID":layout.target_selector.source.path_id},
				"_timeMode": 0,
				"_upAnimKey": "",
				"_upEndAnimKey": "",
				"_useDynamicAttackType": 0,
				"_waitForAttackEvent": 1,
				"m_Enabled": 1,
				"m_Name": ""
			}),
		]),
	)?;
	validate_configuration(
		layout.effect_hook,
		"mode effect-hook configuration",
		json!({
			"_attachEffects": [],
			"_castEffects": [{
				"_delayIfFirstAttack": 0.0,
				"_directionType": 0,
				"_effects": ["sbell2_skill_02_start_01"],
				"_endEvent": 3,
				"_hookEffectAfterFirstAttack": "",
				"_oneshot": 0,
				"_onlyOnce": 0,
				"_startEvent": 4,
				"_useFourDirectionalFace": 0
			}],
			"_castTargetEffects": [],
			"_convertCastEndToAttachFinish": 0,
			"_hitEffects": [{
				"_directionType": 0,
				"_effects": ["sbell2_skill_01_hit"],
				"_endEvent": 3,
				"_oneshot": 1,
				"_onlyOnce": 0,
				"_playIfNotInputTarget": 0,
				"_playOnlyIfInputTarget": 0,
				"_useFourDirectionalFace": 0,
				"_useOwnerToTargetDirection": 0
			}],
			"_inputTargetEffects": [],
			"_preDelayFactor": 0.0,
			"_recalculatePlaybackSpeed": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	validate_configuration(
		layout.extendable_marker,
		"mode extendable-marker configuration",
		json!({"_extendable":0, "m_Enabled":1, "m_Name":""}),
	)?;
	validate_configuration(
		layout.target_selector,
		"mode target-selector configuration",
		json!({
			"_abilityInputTargetAsSortSource": 0,
			"_abnormalCombo": 2,
			"_abnormalFlag": 42,
			"_checkUnitType": 0,
			"_excludeAbnormalFlag": 45,
			"_excludeOwner": 0,
			"_excludeSomeAbnormalFlags": 0,
			"_forceIgnoreCamouflage": 0,
			"_ignoreAllyTargetFree": 0,
			"_ignoreHealFree": 0,
			"_ignoreHitRange": 0,
			"_ignoreTargetFree": 0,
			"_limitTargetNum": 0,
			"_maxNum": 1,
			"_maxTargetKey": "max_target",
			"_needProfessionMask": 0,
			"_onlyIgnoreSomeOfTargetFreeCase": 0,
			"_postFilter": 0,
			"_professionMask": 0,
			"_sortByTauntAtLast": 0,
			"_targetCategory": 1,
			"_targetMotion": 3,
			"_targetSide": 2,
			"_unitTypeMask": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	validate_configuration(
		layout.attack_trigger,
		"mode attack-trigger configuration",
		json!({
			"_keepTarget": 0,
			"_minTargetNum": 1,
			"_overrideSearchTargetTick": -1,
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;
	if !layout.binder.semantic_facts.is_empty() {
		return Err(SbellSkillTemplateDecodeError::new(
			"mode binder",
			format!(
				"unknown binder unexpectedly emitted semantic facts {:?}",
				layout
					.binder
					.semantic_facts
					.iter()
					.map(|fact| fact.kind())
					.collect::<Vec<_>>()
			),
		));
	}
	for (component, behavior) in [
		("mode animator", layout.animator),
		("mode effect hook", layout.effect_hook),
		("mode extendable marker", layout.extendable_marker),
		("mode target selector", layout.target_selector),
		("mode attack trigger", layout.attack_trigger),
	] {
		if !behavior.references.is_empty() || !behavior.semantic_facts.is_empty() {
			return Err(SbellSkillTemplateDecodeError::new(
				component,
				format!(
					"expected no non-null graph edges or semantic facts, found edges={:?}, facts={:?}",
					behavior.references,
					behavior
						.semantic_facts
						.iter()
						.map(|fact| fact.kind())
						.collect::<Vec<_>>()
				),
			));
		}
	}
	Ok(())
}

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	expected: Value,
) -> Result<(), SbellSkillTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		SbellSkillTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(SbellSkillTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	let actual: Value = Value::Object(actual);
	if let Some(detail) = first_value_mismatch(&expected, &actual, "$") {
		return Err(SbellSkillTemplateDecodeError::new(component, detail));
	}
	Ok(())
}

fn merge_configuration_parts(parts: Vec<Value>) -> Value {
	let mut merged: Map<String, Value> = Map::new();
	for part in parts {
		let fields: Map<String, Value> = part
			.as_object()
			.expect("internal expected configuration part must be an object")
			.clone();
		for (key, value) in fields {
			assert!(
				merged.insert(key.clone(), value).is_none(),
				"duplicate internal expected configuration field {key:?}"
			);
		}
	}
	Value::Object(merged)
}

fn first_value_mismatch(expected: &Value, actual: &Value, path: &str) -> Option<String> {
	match (expected, actual) {
		(Value::Object(expected), Value::Object(actual)) => {
			for (key, expected_value) in expected {
				let field_path: String = format!("{path}.{key}");
				let Some(actual_value) = actual.get(key) else {
					return Some(format!("missing field {field_path}"));
				};
				if let Some(detail) =
					first_value_mismatch(expected_value, actual_value, &field_path)
				{
					return Some(detail);
				}
			}
			actual
				.keys()
				.find(|key: &&String| !expected.contains_key(*key))
				.map(|key: &String| format!("unexpected field {path}.{key}"))
		}
		(Value::Array(expected), Value::Array(actual)) => {
			if expected.len() != actual.len() {
				return Some(format!(
					"{path} expected {} entries, found {}",
					expected.len(),
					actual.len()
				));
			}
			expected.iter().zip(actual).enumerate().find_map(
				|(index, (expected, actual)): (usize, (&Value, &Value))| {
					first_value_mismatch(expected, actual, &format!("{path}[{index}]"))
				},
			)
		}
		_ if expected == actual => None,
		_ => Some(format!("{path} expected {expected}, found {actual}")),
	}
}

fn validate_mode_edges(
	graph: &BattlePrefabGraph,
	layout: &ModeLayout<'_>,
) -> Result<(), SbellSkillTemplateDecodeError> {
	let expected_binder_edges: Vec<(&str, &BattlePrefabObjectSource)> = vec![
		("$._animatorHooker", &layout.animator.source),
		("$._attack", &layout.attack.source),
		("$._attackTrigger", &layout.attack_trigger.source),
		("$._bodyTransform", &layout.body_transform),
	];
	if layout.binder.references.len() != expected_binder_edges.len()
		|| expected_binder_edges.iter().any(|(field, target)| {
			!layout
				.binder
				.references
				.iter()
				.any(|reference| reference.field_path == *field && reference.target == **target)
		}) {
		return Err(SbellSkillTemplateDecodeError::new(
			"mode binder edges",
			format!(
				"expected animator/attack/trigger/body edges, found {:?}",
				layout.binder.references
			),
		));
	}
	if layout.attack.references.len() != 1
		|| layout.attack.references[0].field_path != "$._selector"
		|| layout.attack.references[0].target != layout.target_selector.source
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"mode attack edge",
			format!(
				"expected sole _selector edge to {:?}, found {:?}",
				layout.target_selector.source, layout.attack.references
			),
		));
	}
	let expected_referenced_indices: Vec<usize> = [
		layout.animator.source.clone(),
		layout.attack.source.clone(),
		layout.attack_trigger.source.clone(),
	]
	.into_iter()
	.filter_map(|source: BattlePrefabObjectSource| {
		graph
			.mono_behaviours
			.iter()
			.position(|behavior: &BattlePrefabMonoBehaviour| behavior.source == source)
	})
	.collect();
	if expected_referenced_indices.len() != 3
		|| !expected_referenced_indices.iter().all(|index: &usize| {
			layout
				.binder
				.referenced_mono_behaviour_indices
				.contains(index)
		}) || layout.binder.referenced_mono_behaviour_indices.len() != 3
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"mode binder MonoBehaviour edges",
			format!(
				"expected exactly animator/attack/trigger behavior indices, found {:?}",
				layout.binder.referenced_mono_behaviour_indices
			),
		));
	}
	let selector_index: usize = graph
		.mono_behaviours
		.iter()
		.position(|behavior: &BattlePrefabMonoBehaviour| {
			behavior.source == layout.target_selector.source
		})
		.expect("validated target selector is in the graph");
	if layout.attack.referenced_mono_behaviour_indices != vec![selector_index] {
		return Err(SbellSkillTemplateDecodeError::new(
			"mode attack MonoBehaviour edge",
			format!(
				"expected selector behavior index {selector_index}, found {:?}",
				layout.attack.referenced_mono_behaviour_indices
			),
		));
	}
	Ok(())
}

fn validate_attack_fact(
	graph: &BattlePrefabGraph,
	layout: &ModeLayout<'_>,
) -> Result<(), SbellSkillTemplateDecodeError> {
	if layout.attack.semantic_facts.len() != 1 {
		return Err(SbellSkillTemplateDecodeError::new(
			"mode attack fact",
			format!(
				"expected exactly one AttackAbility fact, found {:?}",
				layout
					.attack
					.semantic_facts
					.iter()
					.map(|fact| fact.kind())
					.collect::<Vec<_>>()
			),
		));
	}
	let fact = &layout.attack.semantic_facts[0];
	let BattlePrefabSemantic::AttackAbility(attack) = &fact.semantic else {
		return Err(SbellSkillTemplateDecodeError::new(
			"mode attack fact",
			format!("expected AttackAbility, found {}", fact.kind()),
		));
	};
	let expected: BattlePrefabAttackAbility = BattlePrefabAttackAbility {
		target_selector: crate::mechanics::battle_prefab_interpreter::BattlePrefabObjectReference {
			file_id: 0,
			path_id: layout.target_selector.source.path_id,
		},
		attack_scale: 1.0,
		attack_scale_key: "atk_scale_s2".to_owned(),
		attack_type: 1,
		damage_type: 2,
		element_damage_type: 0,
		extra_damage_type: 0,
		select_target_source: 1,
		select_target_timing: 1,
	};
	if attack != &expected
		|| fact.provenance.source != layout.attack.source
		|| fact.provenance.script_id != parse_id(ATTACK_SCRIPT)
		|| fact.provenance.old_type_hash != parse_id(ATTACK_SCHEMA)
		|| fact.provenance.mechanics_revision != graph.mechanics_revision
	{
		return Err(SbellSkillTemplateDecodeError::new(
			"mode attack fact",
			format!(
				"expected exact attack semantics/provenance {expected:?}, found semantic={attack:?}, provenance={:?}",
				fact.provenance
			),
		));
	}
	Ok(())
}

fn validate_common_issues(
	graph: &BattlePrefabGraph,
	layout: &CommonSkillLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, SbellSkillTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.switch_mode, SWITCH_MODE_SCRIPT),
		unknown_issue(layout.controller, SKILL_CONTROLLER_SCRIPT),
		unknown_issue(layout.effects, SKILL_EFFECTS_SCRIPT),
		custom_buff_issue(
			layout.switch_mode,
			"$._buffs[0]",
			SWITCH_MODE_BUFF,
			SWITCH_MODE_TEMPLATE,
		),
		custom_buff_issue(
			layout.controller,
			"$._buffsWhenDiscard[0]",
			SWITCH_BACK_BUFF,
			SWITCH_BACK_BUFF,
		),
		custom_buff_issue(layout.effects, "$._buffs[0]", MARK_BUFF, "empty"),
		custom_buff_issue(layout.effects, "$._buffs[1]", TRAIT_OVERRIDE_BUFF, "empty"),
		custom_buff_issue(layout.effects, "$._buffs[2]", EFFECT_1_BUFF, "empty"),
		custom_buff_issue(layout.effects, "$._buffs[3]", EFFECT_2_BUFF, "empty"),
		custom_buff_issue(layout.effects, "$._buffs[4]", EFFECT_3_BUFF, "empty"),
		custom_buff_issue(layout.effects, "$._buffs[5]", EFFECT_4_BUFF, "empty"),
		custom_buff_issue(
			layout.effects,
			"$._buffs[6]",
			IDLE_EFFECT_BUFF,
			IDLE_EFFECT_BUFF,
		),
		custom_buff_issue(layout.effects, "$._buffs[7]", RANGE_EFFECT_BUFF, "empty"),
	];
	validate_exact_issues("common semantic issues", graph, &mut expected)
}

fn validate_mode_issues(
	graph: &BattlePrefabGraph,
	layout: &ModeLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, SbellSkillTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.binder, MODE_BINDER_SCRIPT),
		unknown_issue(layout.animator, MODE_ANIMATOR_SCRIPT),
		unknown_issue(layout.effect_hook, EFFECT_HOOK_SCRIPT),
		unknown_issue(layout.extendable_marker, EXTENDABLE_MARKER_SCRIPT),
		unknown_issue(layout.target_selector, TARGET_SELECTOR_SCRIPT),
		unknown_issue(layout.attack_trigger, ATTACK_TRIGGER_SCRIPT),
	];
	validate_exact_issues("mode semantic issues", graph, &mut expected)
}

fn validate_exact_issues(
	component: &'static str,
	graph: &BattlePrefabGraph,
	expected: &mut Vec<BattlePrefabSemanticIssue>,
) -> Result<Vec<BattlePrefabSemanticIssue>, SbellSkillTemplateDecodeError> {
	expected.sort_by_key(issue_sort_key);
	let mut actual: Vec<BattlePrefabSemanticIssue> = graph.issues.clone();
	actual.sort_by_key(issue_sort_key);
	if actual != *expected {
		let missing: Vec<&BattlePrefabSemanticIssue> = expected
			.iter()
			.filter(|issue: &&BattlePrefabSemanticIssue| !actual.contains(*issue))
			.collect();
		let unexpected: Vec<&BattlePrefabSemanticIssue> = actual
			.iter()
			.filter(|issue: &&BattlePrefabSemanticIssue| !expected.contains(*issue))
			.collect();
		return Err(SbellSkillTemplateDecodeError::new(
			component,
			format!("missing {missing:?}; unexpected {unexpected:?}"),
		));
	}
	Ok(actual)
}

fn unknown_issue(
	behavior: &BattlePrefabMonoBehaviour,
	script_hex: &str,
) -> BattlePrefabSemanticIssue {
	BattlePrefabSemanticIssue::UnknownScriptId {
		source: behavior.source.clone(),
		script_id: parse_id(script_hex),
	}
}

fn custom_buff_issue(
	behavior: &BattlePrefabMonoBehaviour,
	field_path: &str,
	key: &str,
	template: &str,
) -> BattlePrefabSemanticIssue {
	BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
		source: behavior.source.clone(),
		field_path: field_path.to_owned(),
		buff_key: Some(key.to_owned()),
		template_key: Some(template.to_owned()),
	}
}

fn common_provenance(
	graph: &BattlePrefabGraph,
	layout: &CommonSkillLayout<'_>,
	gamedata_revision: &str,
) -> Vec<SemanticProvenance> {
	vec![
		provenance(
			graph,
			layout.switch_mode,
			SWITCH_MODE_SCRIPT,
			SWITCH_MODE_SCHEMA,
			gamedata_revision,
			vec!["$._buffs[0]".to_owned()],
		),
		provenance(
			graph,
			layout.controller,
			SKILL_CONTROLLER_SCRIPT,
			SKILL_CONTROLLER_SCHEMA,
			gamedata_revision,
			vec![
				"$._allowSpRecoveryWhenAffecting".to_owned(),
				"$._attackBlackboardModeIndex".to_owned(),
				"$._buffsWhenDiscard[0]".to_owned(),
				"$._discardableInDoze".to_owned(),
				"$._suspendableLimitedTimes".to_owned(),
			],
		),
		provenance(
			graph,
			layout.effects,
			SKILL_EFFECTS_SCRIPT,
			SKILL_EFFECTS_SCHEMA,
			gamedata_revision,
			vec!["$._buffs".to_owned()],
		),
	]
}

fn mode_provenance(
	graph: &BattlePrefabGraph,
	layout: &ModeLayout<'_>,
	gamedata_revision: &str,
) -> Vec<SemanticProvenance> {
	[
		(
			layout.binder,
			MODE_BINDER_SCRIPT,
			MODE_BINDER_SCHEMA,
			vec!["$._attack".to_owned(), "$._attackTrigger".to_owned()],
		),
		(
			layout.animator,
			MODE_ANIMATOR_SCRIPT,
			MODE_ANIMATOR_SCHEMA,
			vec!["$._replaceAnimPairs".to_owned()],
		),
		(
			layout.attack,
			ATTACK_SCRIPT,
			ATTACK_SCHEMA,
			vec![
				"$._atkScaleKey".to_owned(),
				"$._selectTargetTiming".to_owned(),
			],
		),
		(
			layout.effect_hook,
			EFFECT_HOOK_SCRIPT,
			EFFECT_HOOK_SCHEMA,
			vec!["$._castEffects".to_owned(), "$._hitEffects".to_owned()],
		),
		(
			layout.extendable_marker,
			EXTENDABLE_MARKER_SCRIPT,
			EXTENDABLE_MARKER_SCHEMA,
			vec!["$._extendable".to_owned()],
		),
		(
			layout.target_selector,
			TARGET_SELECTOR_SCRIPT,
			TARGET_SELECTOR_SCHEMA,
			vec!["$._limitTargetNum".to_owned(), "$._targetSide".to_owned()],
		),
		(
			layout.attack_trigger,
			ATTACK_TRIGGER_SCRIPT,
			ATTACK_TRIGGER_SCHEMA,
			vec!["$._minTargetNum".to_owned()],
		),
	]
	.into_iter()
	.map(|(behavior, script, schema, fields)| {
		provenance(graph, behavior, script, schema, gamedata_revision, fields)
	})
	.collect()
}

fn provenance(
	graph: &BattlePrefabGraph,
	behavior: &BattlePrefabMonoBehaviour,
	script_hex: &str,
	schema_hex: &str,
	gamedata_revision: &str,
	field_paths: Vec<String>,
) -> SemanticProvenance {
	SemanticProvenance::BattlePrefab {
		mechanics_revision: graph.mechanics_revision.clone(),
		gamedata_revision: gamedata_revision.to_owned(),
		script_id: parse_id(script_hex),
		old_type_hash: parse_id(schema_hex),
		source: SemanticSourceLocation {
			logical_path: behavior.source.logical_path.clone(),
			cab_name: behavior.source.cab_name.clone(),
			path_id: behavior.source.path_id,
			field_paths,
		},
	}
}

fn issue_sort_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
}

fn parse_id(hex: &str) -> [u8; 16] {
	assert_eq!(
		hex.len(),
		32,
		"internal script/schema ID must have 32 hex digits"
	);
	let mut bytes: [u8; 16] = [0; 16];
	for (index, byte) in bytes.iter_mut().enumerate() {
		let start: usize = index * 2;
		*byte = u8::from_str_radix(&hex[start..start + 2], 16)
			.expect("internal script/schema ID must be hexadecimal");
	}
	bytes
}

#[cfg(test)]
mod tests {
	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
		PotentialRank,
	};
	use serde_json::json;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab, interpret_battle_prefab_asset, BattlePrefabGraph,
			BattlePrefabRootSection, BattlePrefabSemanticIssue,
		},
		combat::{
			DamageType, SkillManualDiscardActivationLimit, StatKind, StatusKind,
			TileFieldOverlayLeaveScope,
		},
		mechanic_semantics::{SemanticAction, SemanticValueExpression},
	};

	use super::{
		decode_skill_controller_lifecycle, first_value_mismatch, parse_id,
		try_decode_sbell_skill_template, SbellSkillSemanticLowering, SbellSkillSnowOverlayLifetime,
		SbellSkillSnowSpreadTrigger, SbellSkillTargetSelection, SbellSkillTargetSelectionTiming,
		SbellSkillUnmodeledRequirement, ATTACK_SCALE_BLACKBOARD_KEY, ATTACK_SCHEMA, ATTACK_SCRIPT,
		COLD_DURATION_BLACKBOARD_KEY, COMMON_ASSET_DIRECTORY, MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY,
		SKILL_CONTROLLER_SCRIPT, SNOW_DAMAGE_SCALE_BLACKBOARD_KEY, SWITCH_BACK_BUFF,
	};

	#[test]
	fn exact_value_comparison_reports_drift_path() {
		let expected = json!({"a":{"b":1}});
		let actual = json!({"a":{"b":2}});
		assert_eq!(
			first_value_mismatch(&expected, &actual, "$"),
			Some("$.a.b expected 1, found 2".to_owned())
		);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_dual_root_skill_decodes_and_one_field_drift_fails_closed() {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&gamedata_path).expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		let build: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id("char_1046_sbell2".to_owned()),
			2,
			90,
			200,
			2,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		);
		let resolved = snapshot
			.operators()
			.resolve(&build)
			.expect("resolve reviewed local build");
		let selected_skill = resolved.selected_skill();
		let prefab_id: &str = selected_skill.prefab_id().expect("common skill prefab ID");
		let common_asset_path: String = format!("{COMMON_ASSET_DIRECTORY}/{prefab_id}.prefab");
		let skill_graph: BattlePrefabGraph = interpret_battle_prefab_asset(
			&prefabs,
			"[uc]skills",
			&common_asset_path,
			BattlePrefabRootSection::MainAsset,
			prefab_id,
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed common skill graph");
		let mode_graph: BattlePrefabGraph = interpret_battle_prefab(
			&prefabs,
			resolved.operator_id(),
			BattlePrefabRootSection::Modes,
			"S2",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed Modes/S2 graph");
		let decoded = try_decode_sbell_skill_template(
			&skill_graph,
			&mode_graph,
			selected_skill,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect("reviewed dual-root template must not drift")
		.expect("reviewed common buff identity must match");
		let mut finite_controller = skill_graph
			.mono_behaviours
			.iter()
			.find(|behavior| behavior.script_id == Some(parse_id(SKILL_CONTROLLER_SCRIPT)))
			.expect("reviewed generic skill controller")
			.clone();
		finite_controller.raw_fields["_suspendableLimitedTimes"] = json!(1);
		finite_controller.raw_fields["_discardableInDoze"] = json!(1);
		let finite_lifecycle = decode_skill_controller_lifecycle(&finite_controller)
			.expect("generic controller decoder accepts finite discard policy");
		assert_eq!(
			finite_lifecycle.manual_discard_limit,
			SkillManualDiscardActivationLimit::FirstNActivations(1)
		);
		assert!(finite_lifecycle.allow_manual_discard_while_dozing);

		assert_eq!(decoded.consumed_skill_issues.len(), 13);
		assert_eq!(
			decoded
				.consumed_skill_issues
				.iter()
				.filter(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. }))
				.count(),
			3
		);
		assert_eq!(
			decoded
				.consumed_skill_issues
				.iter()
				.filter(|issue| matches!(
					issue,
					BattlePrefabSemanticIssue::CustomBuffSemanticsMissing { .. }
				))
				.count(),
			10
		);
		assert_eq!(decoded.consumed_mode_issues.len(), 6);
		assert!(decoded
			.consumed_mode_issues
			.iter()
			.all(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. })));
		assert_eq!(decoded.consumed_semantic_fact_sources.len(), 1);
		let source = &decoded.consumed_semantic_fact_sources[0];
		let attack_fact = mode_graph
			.mono_behaviours
			.iter()
			.find(|behavior| behavior.source == *source)
			.expect("consumed attack source");
		assert_eq!(attack_fact.script_id, Some(parse_id(ATTACK_SCRIPT)));
		assert_eq!(attack_fact.old_type_hash, Some(parse_id(ATTACK_SCHEMA)));

		let semantics = &decoded.semantics;
		assert_eq!(
			semantics.overlay_lifetime,
			SbellSkillSnowOverlayLifetime::WhileSkillModeActive
		);
		assert_eq!(
			semantics.spread.trigger,
			SbellSkillSnowSpreadTrigger::SaturatedLayerAttempt
		);
		assert_eq!(
			semantics
				.spread
				.selected_level_maximum_outside_temporary_cells,
			20
		);
		assert_eq!(
			semantics.spread.maximum_outside_temporary_cells,
			SemanticValueExpression::Blackboard(MAXIMUM_SPREAD_TILES_BLACKBOARD_KEY.to_owned())
		);
		assert_eq!(semantics.periodic_damage.source_stat, StatKind::Attack);
		assert_eq!(semantics.periodic_damage.damage_type, DamageType::Arts);
		assert_eq!(
			semantics.periodic_damage.scale,
			SemanticValueExpression::Blackboard(SNOW_DAMAGE_SCALE_BLACKBOARD_KEY.to_owned())
		);
		assert_eq!(semantics.on_leave_snow.status, StatusKind::Cold);
		assert_eq!(
			semantics.on_leave_snow.leave_scope,
			TileFieldOverlayLeaveScope::ObservedCell
		);
		assert_eq!(
			semantics.on_leave_snow.duration_seconds,
			SemanticValueExpression::Blackboard(COLD_DURATION_BLACKBOARD_KEY.to_owned())
		);
		assert_eq!(semantics.frozen_objective_token.required_snow_layers, 5);
		assert_eq!(
			semantics.frozen_objective_token.token_key,
			"token_10058_sbell2_icetgt"
		);
		assert!(semantics.lifecycle.indefinite_until_stopped);
		assert_eq!(semantics.lifecycle.attack_blackboard_mode_index, 1);
		assert_eq!(
			semantics.lifecycle.controller.manual_discard_limit,
			SkillManualDiscardActivationLimit::Unlimited
		);
		assert!(
			!semantics
				.lifecycle
				.controller
				.allow_manual_discard_while_dozing
		);
		assert!(
			!semantics
				.lifecycle
				.controller
				.allow_sp_recovery_while_active
		);
		assert_eq!(
			semantics.lifecycle.on_discard_custom_buff.as_str(),
			SWITCH_BACK_BUFF
		);
		assert_eq!(
			semantics.attack.attack_scale,
			SemanticValueExpression::Blackboard(ATTACK_SCALE_BLACKBOARD_KEY.to_owned())
		);
		assert_eq!(
			semantics.attack.target_selection,
			SbellSkillTargetSelection::AllMatchingEnemiesInCurrentAttackRange
		);
		assert_eq!(
			semantics.attack.target_selection_timing,
			SbellSkillTargetSelectionTiming::SelectForEachCast
		);
		assert_eq!(semantics.evidence.common_sources.len(), 3);
		assert_eq!(semantics.evidence.mode_sources.len(), 7);
		let lowering: SbellSkillSemanticLowering = decoded.lower_program();
		assert_eq!(lowering.program.rules.len(), 3);
		let overlay = lowering
			.program
			.rules
			.iter()
			.find_map(|rule| match &rule.action {
				SemanticAction::InstallTileFieldOverlay(overlay) => Some(overlay),
				_ => None,
			})
			.expect("supported S2 subset must install a snowfield overlay");
		assert_eq!(
			overlay.leave_scope,
			TileFieldOverlayLeaveScope::ObservedCell
		);
		assert_eq!(overlay.observed_tile_key, semantics.observed_snowfield_key);
		assert!(
			overlay.cached_propagation.is_none(),
			"unproven spread policies must remain an explicit omission"
		);
		assert_eq!(
			lowering.remaining_gap.requirements,
			vec![
				SbellSkillUnmodeledRequirement::SnowSpreadSourceOrderingAndContributionMerge,
				SbellSkillUnmodeledRequirement::FrozenObjectiveTokenCollisionAndCardinality,
				SbellSkillUnmodeledRequirement::FrozenObjectiveTokenOwnershipAndCleanup,
				SbellSkillUnmodeledRequirement::FrozenObjectiveTokenLayerStateAfterSpawn,
				SbellSkillUnmodeledRequirement::FrozenObjectiveTokenRetriggerAfterDeath,
				SbellSkillUnmodeledRequirement::FrozenObjectiveTokenSameTickOrdering,
				SbellSkillUnmodeledRequirement::UnknownOnDiscardCustomBuffSemantics,
			]
		);
		let gap: String = lowering.remaining_gap.to_string();
		assert!(gap.contains("snow-spread source ordering"));
		assert!(gap.contains("same-cell collision and global cardinality"));
		assert!(gap.contains("snow-layer retention, consumption, or reset"));
		assert!(!gap.contains("still needs: a five-layer cell transition"));

		let mut drifted: BattlePrefabGraph = mode_graph.clone();
		let attack_trigger = drifted
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.script_id == Some(parse_id(super::ATTACK_TRIGGER_SCRIPT)))
			.expect("reviewed attack trigger");
		attack_trigger.raw_fields["_minTargetNum"] = json!(2);
		let drift = try_decode_sbell_skill_template(
			&skill_graph,
			&drifted,
			selected_skill,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect_err("one-field mode drift must fail closed");
		assert!(drift.to_string().contains("$._minTargetNum"));
	}
}
