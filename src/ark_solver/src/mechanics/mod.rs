mod battle_grid_metadata;
mod battle_prefab_handlers;
mod battle_prefab_interpreter;
mod charge_cost_buff;
mod combat;
mod compile_api;
mod geometry;
mod mechanic_semantics;
mod mechanic_templates;
mod mechanic_templates_block_toggle;
mod mechanic_templates_charge_cost_skill;
mod mechanic_templates_phalanx;
mod mechanic_templates_plain_enemy;
mod mechanic_templates_sbell;
mod mechanic_templates_sbell_blessing;
mod mechanic_templates_sbell_skill;
mod mechanic_templates_shu_roster;
mod normal_attack;
mod normal_attack_catalog;
mod operator_mechanic_overrides;
mod passive_sp;
mod skill_damage;
mod stage_facts;
mod summon_template_catalog;
mod tile_layer_propagation;
mod undescribed_enemy_catalog;

/// Stable entry points for compiling revision-pinned data into typed mechanic semantics.
pub mod compile {
	pub use crate::mechanics::battle_prefab_handlers::built_in_battle_prefab_handler_registry;
	pub use crate::mechanics::battle_prefab_interpreter::{
		find_battle_prefab_containers, interpret_battle_prefab, interpret_battle_prefab_asset,
		BattlePrefabInterpretError,
	};
	pub use crate::mechanics::charge_cost_buff::decode_charge_cost_buff;
	pub use crate::mechanics::compile_api::{
		compile_operator_mechanics, CompileSupportDisposition, CompileSupportEvidence,
		MechanicCompileComponent, MechanicCompileRequest, MechanicCompileResult,
		OperatorMechanicCompileResult, PhalanxTraitCompileEntry,
	};
	pub(crate) use crate::mechanics::mechanic_templates::shu_skill::claims_conditional_heal_attack_template;
	pub use crate::mechanics::mechanic_templates::shu_skill::{
		try_decode_shu_skill_template, ShuSkillTemplateDecodeError,
	};
	pub use crate::mechanics::mechanic_templates::{
		try_decode_sowed_field_template, MechanicTemplateDecodeError,
	};
	pub use crate::mechanics::mechanic_templates_block_toggle::{
		try_decode_block_toggle_template, BlockToggleTemplateDecodeError,
	};
	pub(crate) use crate::mechanics::mechanic_templates_charge_cost_skill::claims_charge_cost_skill_template;
	pub use crate::mechanics::mechanic_templates_charge_cost_skill::{
		try_decode_charge_cost_skill_template, ChargeCostSkillTemplateDecodeError,
	};
	pub use crate::mechanics::mechanic_templates_phalanx::{
		try_decode_phalanx_trait, PhalanxTraitDecodeError,
	};
	pub use crate::mechanics::mechanic_templates_plain_enemy::{
		decode_plain_enemy_graphs, decode_plain_enemy_prefab,
	};
	pub use crate::mechanics::mechanic_templates_sbell::{
		try_decode_snowfield_template, SnowfieldTemplateDecodeError,
	};
	pub use crate::mechanics::mechanic_templates_sbell_blessing::{
		try_decode_blessing_template, BlessingTemplateDecodeError,
	};
	pub(crate) use crate::mechanics::mechanic_templates_sbell_skill::claims_sbell_skill_template;
	pub use crate::mechanics::mechanic_templates_sbell_skill::{
		try_decode_sbell_skill_template, SbellSkillTemplateDecodeError,
	};
	pub use crate::mechanics::mechanic_templates_shu_roster::{
		try_decode_shu_roster_template, ShuRosterTemplateDecodeError,
	};
	pub use crate::mechanics::normal_attack_catalog::{
		compile_base_panel_normal_attack, compile_base_panel_normal_attack_facts,
		deterministic_attack_interval_ticks,
	};
	pub use crate::mechanics::operator_mechanic_overrides::review_operator_mechanic_override;
	pub use crate::mechanics::passive_sp::{
		bind_self_unconditional_natural_sp_recovery, parse_natural_sp_recovery_candidate,
		parse_natural_sp_recovery_facts,
	};
	pub use crate::mechanics::skill_damage::{
		compile_expected_damage_skill, compile_expected_damage_skill_facts,
		compile_expected_damage_skill_facts_with_binding, reviewed_skill_damage_binding,
		ReviewedSkillDamageSelection, REVIEWED_SKILL_DAMAGE_SELECTIONS,
	};
	pub use crate::mechanics::undescribed_enemy_catalog::review_undescribed_enemy;
}

/// Typed factual battle, combat, and effect vocabulary shared by planning and replay.
pub mod domain {
	pub use crate::mechanics::battle_grid_metadata::{
		BattleGridCellMetadata, FriendlyProtectionGoal,
	};
	pub use crate::mechanics::charge_cost_buff::{
		ChargeCostBuffDecodeError, ChargeCostBuffRejection, ChargeCostGrant,
	};
	pub use crate::mechanics::combat::*;
	pub use crate::mechanics::geometry::{
		project_attack_cells, project_offsets, Facing, RangeProjectionError,
	};
	pub use crate::mechanics::mechanic_semantics::{
		GamedataSemanticSourceLocation, LoweredMechanicSemanticProgram, MechanicSemanticError,
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticActorCondition,
		SemanticActorFilter, SemanticBuffModifier, SemanticCachedTileLayerPropagation,
		SemanticCellSet, SemanticConditionalAura, SemanticConditionalAuraPeriodicEffect,
		SemanticEffect, SemanticEffectTarget, SemanticEvent, SemanticLayerThresholdReaction,
		SemanticLayerThresholdReactionEffect, SemanticLayerThresholdTransition, SemanticLifetime,
		SemanticPeriodicEffect, SemanticProvenance, SemanticResidentBuff,
		SemanticSkillControllerLifecycle, SemanticSkillInactiveStance, SemanticSourceLocation,
		SemanticTargetCellActorSpawn, SemanticTargetCellSpawnCollision,
		SemanticTargetCellSpawnOwnership, SemanticTargetedEffect, SemanticTeamCompositionCondition,
		SemanticTileField, SemanticTileFieldOverlay, SemanticTrigger, SemanticTriggerFilter,
		SemanticValueExpression,
	};
	pub use crate::mechanics::mechanic_templates::shu_skill::{
		DecodedShuSkillTemplate, ShuSkillChargeSemantics, ShuSkillDelivery, ShuSkillHpComparison,
		ShuSkillSemanticIrGap, ShuSkillSemanticIrRequirement, ShuSkillTargetEligibility,
		ShuSkillTargetSelection, ShuSkillTargetSelectionTiming, ShuSkillTemplateEvidence,
		ShuSkillTemplateSemantics,
	};
	pub use crate::mechanics::mechanic_templates::DecodedMechanicTemplate;
	pub use crate::mechanics::mechanic_templates_block_toggle::{
		BlockToggleSemantics, DecodedBlockToggleTemplate,
	};
	pub use crate::mechanics::mechanic_templates_charge_cost_skill::{
		charge_cost_grant_cadence, ChargeCostGrantCadence, ChargeCostGrantCadenceError,
		ChargeCostSkillActiveSpan, ChargeCostSkillSpLifecycle, DecodedChargeCostSkillTemplate,
	};
	pub use crate::mechanics::mechanic_templates_phalanx::{
		DecodedPhalanxTrait, PhalanxModeSemantics, PhalanxModeTransition, PhalanxNormalAttackRule,
		PhalanxTraitSemantics,
	};
	pub use crate::mechanics::mechanic_templates_plain_enemy::{
		DecodedPlainEnemyPrefab, PlainEnemyAttackForm, PlainEnemyPrefabDecode,
		PlainEnemyPrefabDecodeError, PlainEnemyTableFacts, ProjectileMovementSchema,
	};
	pub use crate::mechanics::mechanic_templates_sbell::{
		DecodedSnowfieldTemplate, SnowfieldCellSelection, SnowfieldRemovalRule,
		SnowfieldTemplateSemantics,
	};
	pub use crate::mechanics::mechanic_templates_sbell_blessing::{
		BlessingEffectTarget, BlessingTemplateSemantics, DecodedBlessingTemplate,
		RetaliationSemantics, RevivalSemantics,
	};
	pub use crate::mechanics::mechanic_templates_sbell_skill::{
		DecodedSbellSkillTemplate, SbellSkillAttackSemantics,
		SbellSkillFrozenObjectiveTokenSemantics, SbellSkillLeaveSnowSemantics, SbellSkillModeEntry,
		SbellSkillModeLifecycleSemantics, SbellSkillPeriodicDamageDelivery,
		SbellSkillPeriodicDamageSemantics, SbellSkillSemanticLowering,
		SbellSkillSnowOverlayLifetime, SbellSkillSnowSpreadDestination,
		SbellSkillSnowSpreadSemantics, SbellSkillSnowSpreadTrigger, SbellSkillTargetSelection,
		SbellSkillTargetSelectionTiming, SbellSkillTemplateEvidence, SbellSkillTemplateSemantics,
		SbellSkillUnmodeledGap, SbellSkillUnmodeledRequirement,
	};
	pub use crate::mechanics::mechanic_templates_shu_roster::{
		DecodedShuRosterTemplate, ShuRosterBranchCombination, ShuRosterBranchLifetime,
		ShuRosterCondition, ShuRosterConditionalBranch, ShuRosterEffect,
		ShuRosterModifierAggregation, ShuRosterStat, ShuRosterTargetFilter, ShuRosterTargetScope,
		ShuRosterTemplateSemantics,
	};
	pub use crate::mechanics::normal_attack::{
		effective_damage, hp_capped_damage, AppliedLockedTargetPacket, ArtsResistance,
		ArtsResistanceError, DamageAccounting, EnemyDamageState, EnemyDamageStateError,
		EnemyDefenses, ExpectedDamagePacketError, ExpectedRawDamageOutcome, FixedDamage,
		FixedDamageError, HpCappedDamage, LockedTargetDamagePacket, NormalAttackApplicationError,
		NormalAttackDamageReport, NormalAttackDamageType, NormalAttackProfile,
		NormalAttackProfileError, NormalAttackProfileSupport, NormalAttackTargetFilter,
		NormalAttackTargetPriority, TargetClass, UnsupportedNormalAttackProfile,
		UnsupportedNormalAttackProfileError, UnsupportedNormalAttackReason,
		EXPECTED_DAMAGE_PROBABILITY_SCALE, FIXED_DAMAGE_SCALE,
	};
	pub use crate::mechanics::normal_attack_catalog::{
		AttackIntervalError, AttackTimingField, BasePanelNormalAttackFacts,
		CatalogNormalAttackProfile, NormalAttackCatalogEntry, NormalAttackCatalogUnsupportedReason,
		NormalAttackFirstDelayRule, ReviewedNormalAttackArchetype, UnsupportedCatalogNormalAttack,
	};
	pub use crate::mechanics::operator_mechanic_overrides::{
		ExactRatio, ExusiaiMechanicOverride, OperatorMechanicBindingError,
		OperatorMechanicOverride, OperatorMechanicOverrideEntry, PermanentModifier, TalentTarget,
		EXUSIAI_OPERATOR_ID,
	};
	pub use crate::mechanics::passive_sp::{
		NaturalSpRecoveryCondition, NaturalSpRecoveryScope, NaturalSpRecoveryStacking,
		ParsedNaturalSpRecovery, PassiveSpBlackboardFact, PASSIVE_SP_RECOVERY_KEY,
	};
	pub use crate::mechanics::skill_damage::{
		ExpectedAttackPhase, ExpectedDamageDelivery, ExpectedDamageSkillEntry,
		ExpectedDamageSkillProfile, ExpectedDamageType, ExpectedDefenseAdjustment,
		ExpectedRawDamageMixture, ExpectedSkillDuration, ExpectedSkillRange, ExpectedTargetFilter,
		SkillDamageBindingSelector, SkillDamageBlackboardFact, SkillDamageFacts,
		SkillDamageKeyAudit, SkillDamageMechanicTemplate, SkillDamageSemantic,
		SkillDamageTemplateBinding, SkillDamageUnsupportedReason, UnsupportedExpectedDamageSkill,
	};
	pub use crate::mechanics::summon_template_catalog::{
		SummonTemplateCatalog, SummonTemplateCatalogError,
	};
	pub use crate::mechanics::tile_layer_propagation::{
		resolve_cached_tile_layer_pulse, AggregatedContribution, CachedTileLayerPulse,
		ContributionMergePolicy, LayerMutationCause, LayeredTileCell, OrderedOrthogonalNeighbors,
		OrthogonalDirection, PropagationDestination, PulseSourceOrder, SourceOrderWitness,
		SourceRangeRelation, TileLayerEvent, TileLayerPropagationAmbiguity,
		TileLayerPropagationError, TileLayerPropagationOutcome, TileLayerPropagationResolution,
		TilePersistence,
	};
	pub use crate::mechanics::undescribed_enemy_catalog::{
		DescribedEnemyBehavior, ReviewedEnemyDamageType, UndescribedEnemy,
	};
}

/// Verified battle-prefab graph and provenance types required by data adapters.
pub mod prefab {
	pub use crate::mechanics::battle_prefab_interpreter::{
		BattlePrefabAbilityConfiguration, BattlePrefabAttackAbility,
		BattlePrefabAttributeConfiguration, BattlePrefabAttributeModifier,
		BattlePrefabBlackboardEntry, BattlePrefabBuffConfiguration,
		BattlePrefabBuffConfigurationSet, BattlePrefabBuffSpec, BattlePrefabCompositeAbility,
		BattlePrefabContainerMatch, BattlePrefabGameObject, BattlePrefabGraph,
		BattlePrefabHandlerRegistry, BattlePrefabListenerTiming, BattlePrefabMonoBehaviour,
		BattlePrefabObjectReference, BattlePrefabObjectSource, BattlePrefabResolvedReference,
		BattlePrefabRootSection, BattlePrefabScriptHandler, BattlePrefabSemantic,
		BattlePrefabSemanticFact, BattlePrefabSemanticIssue, BattlePrefabSemanticProvenance,
		BattlePrefabStaticBuffAbility, BattlePrefabTileListener, BattlePrefabUnavailableReference,
	};
}

/// Canonical stage identities and route events shared by planning and replay.
pub mod stage {
	pub use crate::mechanics::stage_facts::{
		compile_route_timeline, ticks_from_seconds_ceil, RouteTimeline, RouteTimelineAction,
		RouteTimelineStep, SpawnCohortId, SpawnInstanceId, StageCompileError,
	};
}
