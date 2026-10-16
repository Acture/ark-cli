//! Stage modeling and proposal generation for Ark strategy search.
//!
//! Planning may prepare, rank, and audit candidates. It does not execute authoritative replay or
//! claim battlefield victory.

mod battle_gap;
mod candidate_bounds;
mod coverage_prepare;
mod coverage_search;
mod deployment_dp;
mod envelope;
mod full_battle_planning;
mod guide_schedule;
mod level_solve;
mod pruning;
mod query;
mod relaxation;
mod stage_space_time;

/// Crate-internal seam used by workflow materialization without exposing planner implementation
/// helpers as supported library API.
pub(crate) mod internal {
	pub(crate) use super::coverage_prepare::deployment_allowed;
	pub(crate) use super::coverage_search::{
		prepare_optimistic_hp_group_capacity, OptimisticHpGroupCapacityPrepareRequest,
		PreparedOptimisticHpGroupCapacityScorer,
	};
	pub(crate) use super::guide_schedule::prepare_interleaved_export_deployment_schedule_with_cost_overrides;
}

pub use battle_gap::{
	FullBattleDamageGapLedger, FullBattleEnemyDamageGap, FullBattleEnemyResolution,
	FullBattleWaveTiming,
};
pub use candidate_bounds::compile_candidate_bounds;
pub(crate) use coverage_search::{
	score_joint_deployments, score_optimistic_wave_local_reachability_capacity, JointCoverageScore,
	JointCoverageScoreRequest, OptimisticHpGroupScore, OptimisticWaveLocalReachabilityScore,
	OptimisticWaveLocalReachabilityScoreRequest, ResolvedCoverageCandidate,
	ResolvedCoverageDeployment,
};
pub use deployment_dp::{
	schedule_ordered_deployments, verify_deployment_dp_prefix, DeploymentDpEvent,
	DeploymentDpEventAudit, DeploymentDpRules, DeploymentDpVerificationError,
	OrderedDeploymentDpAction, OrderedDeploymentDpScheduleError, PlannedDeploymentDpGrant,
	ScheduledDeploymentDpAction,
};
pub use envelope::{
	CandidateError, EnvelopeError, ReplacementCandidateBounds, SlotRequirementEnvelope, TickWindow,
};
pub use full_battle_planning::{
	prepare_full_battle_unblocked_planning_context, CausalWaveTiming, EventTickBounds,
	FullBattlePlanningContext, FullBattlePlanningError, HpGroupId, RetainedWaveTimingWitness,
	SpaceTimeHpGroup, WaveLocalHpGroup, WaveLocalHpGroupId,
};
pub(crate) use guide_schedule::{
	prepare_guide_deployment_schedule_with_cost_overrides, GuideDeploymentActionOverrides,
	GuideDeploymentActionSpec, GuideDeploymentDomain, GuideSchedulingConditions,
	PreparedGuideDeployment, PreparedGuideDeploymentSchedule,
};
pub use level_solve::{
	solve_expected_damage_level, visit_expected_damage_level,
	ExpectedDamageAdditiveConstraintAudit, ExpectedDamageAdditiveProposalEvidence,
	ExpectedDamageCandidateExclusion, ExpectedDamageConstraintContributionAudit,
	ExpectedDamageJointDeadlineAudit, ExpectedDamageJointEvaluationFailure,
	ExpectedDamageJointEvaluator, ExpectedDamageJointEvidence, ExpectedDamageJointMemberAudit,
	ExpectedDamageJointMemberDamageEventAudit, ExpectedDamageJointTargetAudit,
	ExpectedDamageJointTargetDamageEventAudit, ExpectedDamageLevelSolveError,
	ExpectedDamageLevelSolveInputError, ExpectedDamageLevelSolveReport,
	ExpectedDamageLevelSolveRequest, ExpectedDamageLevelSolveStatus, ExpectedDamageProposalAudit,
	ExpectedDamageProposalDisposition, ExpectedDamageProposalRejection,
	ExpectedDamageResidualDemandAudit, ExpectedDamageSelectedCandidateAudit,
	EXPECTED_DAMAGE_LEVEL_SOLVE_REPORT_SCHEMA_VERSION,
};
pub use pruning::{HardPruner, PruneDecision, PruneProof, Pruner};
#[cfg(test)]
pub(crate) use query::CatalogPreset;
pub(crate) use query::{BasePanelBuildSpec, CandidatePoolSpec, DeploymentAnchor};
pub use query::{StageTimeAnchor, StageTimeBinding, TickSpan};
pub use relaxation::{
	solve_independent_selection, solve_independent_selection_cardinality_round_robin,
	visit_independent_selection, visit_independent_selection_cardinality_round_robin,
	ContributionCandidateKey, ContributionConstraintAudit, ContributionConstraintKey,
	ContributionDemand, ContributionIdentityError, ContributionOperatorIdentity,
	FixedDamageInputLocation, IndependentContributionCandidate, IndependentSelectionInputError,
	IndependentSelectionMember, IndependentSelectionRequest, IndependentSelectionSearchStatus,
	IndependentSelectionSolution, IndependentSelectionSolveError, IndependentSelectionSolveResult,
	MAX_EXACT_SELECTION_INTEGER,
};
pub use stage_space_time::{
	bind_stage_space_time, compile_stage_space_time_template,
	compile_stage_space_time_template_for_bindings, CellOccupancy, EnemyTrajectory,
	RelativeEnemyTrajectory, RelativeSpawnCohort, SpawnCohort, StageSpaceTimeError,
	StageSpaceTimeModel, StageSpaceTimeTemplate, TrajectoryClass,
};
