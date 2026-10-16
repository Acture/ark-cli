//! Strict, versioned Arknights battle-data snapshots.

mod battle_prefab;
mod enemy;
mod error;
mod manifest;
mod mechanics;
mod operator;
mod range;
mod raw_roots;
mod snapshot;
mod stage;
mod stage_payload;

pub use battle_prefab::{
	compute_mechanics_revision, BattlePrefabBundle, BattlePrefabBundleMetadata,
	BattlePrefabEntryKind, BattlePrefabExternal, BattlePrefabFile, BattlePrefabManifestEntry,
	BattlePrefabObject, BattlePrefabObjectHandle, BattlePrefabObjectRef, BattlePrefabSchemaNode,
	BattlePrefabScriptClass, BattlePrefabSerializedType, BattlePrefabSnapshot, BattlePrefabSource,
	BattlePrefabTooling,
};
pub use enemy::{
	CompiledEnemy, EnemyBlackboardEntry, EnemyDamageKind, EnemyHandbookFacts, EnemyKey, EnemySkill,
	EnemyStats,
};
pub use error::ArkDataError;
pub use mechanics::{
	MechanicIdentity, MechanicKind, MechanicOrigin, MechanicReference, MechanicRequirements,
	MechanicSupportReport, PredefinedCollection,
};
pub use operator::{
	BasePanelBuild, BlackboardEntry, LegalOperatorModule, OperatorBaseStats, OperatorBuild,
	OperatorCatalog, OperatorEffectCandidate, OperatorIdentity, OperatorModule, OperatorModulePart,
	OperatorModulePartTarget, OperatorModuleTalentCandidate, OperatorModuleTraitCandidate,
	OperatorPosition, OperatorTalentGroup, PotentialCustomEffect, PotentialRank,
	ResolvedBasePanelBuild, ResolvedOperatorBuild, ResolvedOperatorModule, SelectedSkillMetadata,
	SkillActivationType, SkillDurationType, SkillSpMetadata, SkillSpType,
};
pub use range::{AttackRange, RangeOffset};
pub use snapshot::ArkDataSnapshot;
pub use stage::{
	resolve_available_stage_levels, AvailableStageLevel, CompiledStage, StageDifficulty,
	StageDifficultySelection, StageIdentity, StageRef,
};
pub use stage_payload::{
	BlockEdge, Buildable, FunctionDisableMask, GridPosition, MapCell, MapDimensions, MapDirection,
	MotionMode, Passability, PredeployedStageActor, Route, RouteCheckpoint, RouteCheckpointKind,
	Spawn, SpawnAction, StageAction, StageActionKind, StageActionSchedulePeriod,
	StageActorCollection, StageBranch, StageBranchPhase, StageFragment, StageOptions, StageVector2,
	StageWave, TileHeight, WaveActionContainer, WaveActionOrigin,
};
