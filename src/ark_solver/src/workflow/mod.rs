//! Application workflows composing import, planning, replay, and artifact boundaries.

mod candidate;
mod replay;
mod retained_gap;
mod solve;
mod strategy_binding;
mod validate;

pub use candidate::{
	materialize_candidate_expected_replay_input, materialize_source_removed_expected_replay_input,
	CandidateExpectedReplayError, CandidateReplayMemberPlan, CandidateReplayPlacement,
	CandidateReplayPlan,
};
pub use replay::{
	replay, ReplayIndex, ReplayIndexEntry, ReplayRequest, ReplayVariantOutput, ReplayWorkflowError,
	ReplayWorkflowOutput, REPLAY_INDEX_SCHEMA_VERSION,
};
pub use retained_gap::{derive_full_battle_damage_gap_ledger, FullBattleGapError};
pub use solve::{
	enumerate_expected_damage_activation_schedules, solve, solve_with_observer,
	ExpectedDamageActivationAudit, ExpectedDamageActivationTiming,
	ExpectedDamageBattlefieldReplayAudit, ExpectedDamageBattlefieldReplayResult,
	ExpectedDamageBuildPolicy, ExpectedDamageDpAudit, ExpectedDamageDpEntryAudit,
	ExpectedDamageDpRulesAudit, ExpectedDamageJobMaterializationFailure,
	ExpectedDamagePreparedCandidateAudit, ExpectedDamagePreparedClaim, ExpectedDamagePreparedJob,
	ExpectedDamagePreparedLevelReport, ExpectedDamageProbabilityBasis,
	ExpectedDamageVerifiedSearchStatus, LevelSolveCandidateProgress,
	LevelSolveDeadOperatorProgress, LevelSolveEnemyProgress, LevelSolveJointPlanningOutcome,
	LevelSolvePendingActionProgress, LevelSolvePrepareError, LevelSolveProgressEvent,
	LevelSolveProgressObserver, LevelSolveProgressUpdate, LevelSolveReplayDpStepProgress,
	LevelSolveReplayMemberProgress, LevelSolveReplayVariantOutcome, LevelSolveRoster,
	LevelSolveRosterEntry, LevelSolveRosterError, LevelSolveSelectionEngine, SolveRequest,
	EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION, EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION,
	EXPECTED_DAMAGE_PREPARED_REPORT_SCHEMA_VERSION, LEVEL_SOLVE_ROSTER_SCHEMA_VERSION,
};
pub use strategy_binding::{
	enumerate_maximal_guide_build_variants, enumerate_replay_guide_build_variants,
	prepare_guide_actions, ExpectedReplayPrepareError, MaximalGuideBuildVariant,
};
pub use validate::{
	validate_export, ExpectedDamageExportValidationReport, ExportValidationRequest,
};
