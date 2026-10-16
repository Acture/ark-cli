#![deny(unsafe_code)]

extern crate alloc;

pub mod config;
pub mod data;
pub mod ingest;
pub mod maa_job;
pub mod mechanics;
pub mod planning;
pub mod replay;
pub mod simulation;
#[cfg(test)]
pub(crate) mod test_support;
mod verification_artifact;
pub mod workflow;
pub use verification_artifact::{
	ArtifactAction, ArtifactEnemyAssessment, ArtifactExactBuild, ArtifactFacing,
	ArtifactOperatorAssessment, ArtifactUnsupportedMechanic, DeterministicDamageStatus,
	ExpectedStrategyStatus, MaterializedActionKind, StrategyArtifact, StrategyArtifactError,
	STRATEGY_ARTIFACT_SCHEMA_VERSION,
};
