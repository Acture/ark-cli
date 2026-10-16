mod compile;
mod engine;
mod report;

pub use compile::{
	materialize_expected_replay_input, ExactReplayBuildBinding, ExpectedReplayAdapterError,
	ExpectedReplayCompileInput, ExpectedReplayInput, PreparedReplayGuide,
};
pub use engine::{
	compile_battle, evaluate_defensive_probability, replay_expected, replay_expected_to_resolution,
	ActionAuditStatus, AxisDirectHitPhase, BlockAuditEntry, BlockEndReason, CompiledArkBattle,
	DefensiveProbabilityAssessment, DpAuditEntry, DpAuditKind, ExpectedCommand, ExpectedDpAudit,
	ExpectedEnemyAssessment, ExpectedEvent, ExpectedOperatorAssessment, ExpectedTerminalStatus,
	ExpectedValueAssessment, GuideActionAudit, GuideExecutorActionKind, SkillActivationOrigin,
	SkillEndReason, StoredAttackTargetRef,
};
pub use report::{
	execute_expected_replay, ExpectedReplayAuditReport, ExpectedReplayOutput,
	ExpectedReplayReportStatus, ReplayActionAudit, ReplayBlockAudit, ReplayBlockEndReason,
	ReplayBlockerKind, ReplayConservativeOmissionEntry, ReplayDefensiveProbabilityAudit,
	ReplayDpAudit, ReplayDpEntryAudit, ReplayEnemyAudit, ReplayOperatorAudit,
	ReplaySkillActivationAudit, ReplaySkillActivationSource, ReplayUnmodeledOmissionEntry,
	ReplayUnsupportedEntry, ReplayWaveAnchorAudit, EXPECTED_REPLAY_REPORT_SCHEMA_VERSION,
};
