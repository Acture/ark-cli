//! Expected-damage solve orchestration across MAA, mechanics, planning, and replay.
//!
//! This adapter deliberately models only reviewed skill damage. It uses the guide to preserve
//! source-ordered DP spending and retained deployments, materializes exact no-module roster
//! builds, scores each legal skill-phase schedule against stage trajectories, and delegates
//! additive proposal generation plus shared-HP validation to [`crate::planning`].

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
	num::{NonZeroU32, NonZeroU64},
	ops::ControlFlow,
	panic::{catch_unwind, AssertUnwindSafe},
	sync::{
		atomic::{AtomicBool, AtomicUsize, Ordering},
		Arc, Mutex,
	},
	thread,
	time::{Duration, Instant},
};

use crate::data::{
	ArkDataSnapshot, BattlePrefabSnapshot, CompiledStage, GridPosition, OperatorBuild,
	OperatorIdentity, OperatorModule, PotentialRank, RangeOffset, ResolvedOperatorBuild,
	SkillActivationType, SkillSpType, StageDifficulty, StageIdentity, StageRef,
};
use crate::simulation::TickRate;
use crossbeam_channel::{
	bounded, Receiver, RecvTimeoutError, SendTimeoutError, Sender, TrySendError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
	candidate::{
		materialize_candidate_expected_replay_input,
		materialize_source_removed_expected_replay_input, CandidateReplayMemberPlan,
		CandidateReplayPlacement, CandidateReplayPlan,
	},
	retained_gap::derive_full_battle_damage_gap_ledger,
	strategy_binding::{
		guide_deployment_domain_from_maa_job, prepare_replay_guide, replay_stage_difficulty,
	},
	validate::{validate_export, ExportValidationRequest},
};

use crate::maa_job::{
	compile_maa_job, parse_maa_job, MaaActionConditions, MaaExactRequirements, MaaJob,
	MaaJobAction, MaaJobCompileRequest, MaaJobDifficulty, MaaJobDirection, MaaJobExportMetadata,
	MaaJobGuideSeed, MaaJobGuideSeedIter, MaaJobOperator, MaaJobRequirements, MaaJobSeed,
	MaaJobSeedSelection, MaaJobSourceLineage, MaaJobTimeout, MaaReplacementAction,
	MaaReplacementActionKind, MaaReplacementOperator,
};
use crate::mechanics::{
	compile::{
		built_in_battle_prefab_handler_registry, compile_expected_damage_skill,
		interpret_battle_prefab_asset, try_decode_charge_cost_skill_template,
	},
	domain::{
		project_attack_cells, project_offsets, ChargeCostGrantCadence, ExpectedAttackPhase,
		ExpectedDamageDelivery, ExpectedDamageSkillEntry, ExpectedDamageSkillProfile,
		ExpectedDamageType, ExpectedRawDamageOutcome as PacketExpectedRawDamageOutcome,
		ExpectedSkillDuration, ExpectedTargetFilter, ExpectedValue, Facing, FixedDamage,
		LockedTargetDamagePacket, NormalAttackDamageType, NormalAttackProfile,
		NormalAttackTargetFilter, PeriodicSpSource, SkillDamageMechanicTemplate,
		EXPECTED_DAMAGE_PROBABILITY_SCALE,
	},
	prefab::{BattlePrefabHandlerRegistry, BattlePrefabRootSection},
};
use crate::planning::internal::{
	deployment_allowed, prepare_interleaved_export_deployment_schedule_with_cost_overrides,
	prepare_optimistic_hp_group_capacity, OptimisticHpGroupCapacityPrepareRequest,
	PreparedOptimisticHpGroupCapacityScorer,
};
use crate::planning::{
	prepare_full_battle_unblocked_planning_context,
	prepare_guide_deployment_schedule_with_cost_overrides, schedule_ordered_deployments,
	score_joint_deployments, score_optimistic_wave_local_reachability_capacity,
	visit_expected_damage_level, BasePanelBuildSpec, CandidatePoolSpec, ContributionCandidateKey,
	ContributionConstraintKey, ContributionDemand, ContributionOperatorIdentity, DeploymentAnchor,
	ExpectedDamageCandidateExclusion, ExpectedDamageJointEvaluationFailure,
	ExpectedDamageJointEvaluator, ExpectedDamageJointEvidence, ExpectedDamageLevelSolveReport,
	ExpectedDamageLevelSolveRequest, ExpectedDamageLevelSolveStatus, ExpectedDamageProposalAudit,
	ExpectedDamageProposalDisposition, FullBattleDamageGapLedger, FullBattlePlanningContext,
	GuideDeploymentActionOverrides, GuideDeploymentDomain, HpGroupId,
	IndependentContributionCandidate, JointCoverageScore, JointCoverageScoreRequest,
	OptimisticHpGroupScore, OptimisticWaveLocalReachabilityScore,
	OptimisticWaveLocalReachabilityScoreRequest, OrderedDeploymentDpAction,
	PlannedDeploymentDpGrant, PreparedGuideDeployment, PreparedGuideDeploymentSchedule,
	ResolvedCoverageCandidate as Candidate, ResolvedCoverageDeployment,
	ScheduledDeploymentDpAction, SpaceTimeHpGroup, StageSpaceTimeModel, StageTimeAnchor, TickSpan,
	WaveLocalHpGroup, WaveLocalHpGroupId,
};
use crate::replay::{
	execute_expected_replay, materialize_expected_replay_input, ExactReplayBuildBinding,
	ExpectedReplayAuditReport, ExpectedReplayCompileInput, ExpectedReplayReportStatus,
	PreparedReplayGuide, ReplayDefensiveProbabilityAudit,
};
use crate::{
	ArtifactExactBuild, DeterministicDamageStatus, ExpectedStrategyStatus, MaterializedActionKind,
	StrategyArtifact,
};

pub const LEVEL_SOLVE_ROSTER_SCHEMA_VERSION: u32 = 1;
pub const EXPECTED_DAMAGE_PREPARED_REPORT_SCHEMA_VERSION: u32 = 15;
pub const EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION: u32 = 5;
pub const EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION: u32 = 7;
const LEVEL_SOLVE_TICKS_PER_SECOND: u32 = 30;
const LEVEL_SOLVE_CANDIDATE_BUDGET: u64 = 100_000;
const DEFAULT_LEVEL_SOLVE_TOP_K: usize = 20;
const MAX_ACTIVATIONS_PER_SCHEDULE: usize = 1_024;
const REPLAY_PIPELINE_CANCELLATION_POLL: Duration = Duration::from_millis(10);

/// Explicit operator/skill inventory. Builds are resolved as E2/max-level, trust 200, M3, P6,
/// with no module in this first damage-model revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LevelSolveRoster {
	pub schema_version: u32,
	pub operators: Vec<LevelSolveRosterEntry>,
}

impl LevelSolveRoster {
	pub fn validate(&self) -> Result<(), LevelSolveRosterError> {
		if self.schema_version != LEVEL_SOLVE_ROSTER_SCHEMA_VERSION {
			return Err(LevelSolveRosterError::UnsupportedSchemaVersion {
				found: self.schema_version,
			});
		}
		if self.operators.is_empty() {
			return Err(LevelSolveRosterError::EmptyRoster);
		}
		let mut seen: BTreeSet<(&str, u8)> = BTreeSet::new();
		for (index, entry) in self.operators.iter().enumerate() {
			if entry.operator.trim().is_empty() {
				return Err(LevelSolveRosterError::BlankOperator { index });
			}
			if entry.skill == 0 {
				return Err(LevelSolveRosterError::ZeroSkill { index });
			}
			if !seen.insert((entry.operator.as_str(), entry.skill)) {
				return Err(LevelSolveRosterError::DuplicateEntry {
					operator: entry.operator.clone(),
					skill: entry.skill,
				});
			}
		}
		Ok(())
	}
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LevelSolveRosterEntry {
	pub operator: String,
	pub skill: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LevelSolveRosterError {
	UnsupportedSchemaVersion { found: u32 },
	EmptyRoster,
	BlankOperator { index: usize },
	ZeroSkill { index: usize },
	DuplicateEntry { operator: String, skill: u8 },
}

impl Display for LevelSolveRosterError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::UnsupportedSchemaVersion { found } => write!(
				formatter,
				"unsupported level-solve roster schema {found}; expected \
				 {LEVEL_SOLVE_ROSTER_SCHEMA_VERSION}"
			),
			Self::EmptyRoster => formatter.write_str("level-solve roster is empty"),
			Self::BlankOperator { index } => {
				write!(formatter, "roster entry {index} has a blank operator")
			}
			Self::ZeroSkill { index } => {
				write!(
					formatter,
					"roster entry {index} must select a positive skill slot"
				)
			}
			Self::DuplicateEntry { operator, skill } => {
				write!(
					formatter,
					"duplicate roster entry {operator:?} skill {skill}"
				)
			}
		}
	}
}

impl Error for LevelSolveRosterError {}

/// Pure timing facts used to enumerate deterministic active-skill schedules.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedDamageActivationTiming {
	pub activation_type: SkillActivationType,
	pub sp_type: SkillSpType,
	pub sp_cost: u32,
	pub initial_sp: u32,
	pub sp_increment: f64,
	pub periodic_sp_sources: Vec<PeriodicSpSource>,
	pub duration: ExpectedSkillDuration,
	pub active_attack_interval_ticks: NonZeroU64,
}

impl From<&ExpectedDamageSkillProfile> for ExpectedDamageActivationTiming {
	fn from(profile: &ExpectedDamageSkillProfile) -> Self {
		Self {
			activation_type: profile.activation_type().clone(),
			sp_type: profile.sp_type().clone(),
			sp_cost: profile.sp_cost(),
			initial_sp: profile.initial_sp(),
			sp_increment: profile.sp_increment(),
			periodic_sp_sources: Vec::new(),
			duration: profile.duration(),
			active_attack_interval_ticks: profile.active().attack_interval_ticks(),
		}
	}
}

fn expected_damage_activation_timing(
	_build: &ResolvedOperatorBuild,
	profile: &ExpectedDamageSkillProfile,
	_tick_rate: TickRate,
) -> ExpectedDamageActivationTiming {
	ExpectedDamageActivationTiming::from(profile)
}

/// Enumerates a bounded, deterministic set of complete skill schedules.
///
/// Time-recovery skills combine the revision-pinned SP increment with generic periodic passive-SP
/// sources and conservatively begin recharging after the active phase. Manual skills include the
/// earliest legal schedule and schedules aligned to supplied responsibility boundaries. Attack-
/// and damage-recovery skills require modeled combat SP events; natural-recovery modifiers never
/// charge them.
pub fn enumerate_expected_damage_activation_schedules(
	timing: &ExpectedDamageActivationTiming,
	deploy_tick: u64,
	duty_window: TickSpan,
	responsibility_boundaries: &[u64],
	ticks_per_second: NonZeroU32,
	maximum_schedules: usize,
) -> Result<Vec<Vec<TickSpan>>, LevelSolvePrepareError> {
	if maximum_schedules == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_activation_schedule_limit",
			"activation schedule limit must be positive",
		));
	}
	if deploy_tick >= duty_window.end {
		return Ok(Vec::new());
	}
	if !matches!(timing.activation_type, SkillActivationType::Passive) && timing.sp_cost == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_skill_sp_cost",
			"active skill with zero SP cost cannot be scheduled by the v1 recurrence model",
		));
	}

	let (initial_recovery_ticks, recurring_recovery_ticks): (u64, Option<u64>) = match timing
		.sp_type
	{
		SkillSpType::IncreaseWithTime => {
			let sources: Vec<PeriodicSpSource> = periodic_sp_sources(timing, ticks_per_second)?;
			(
				periodic_sp_recovery_ticks(
					timing.sp_cost.saturating_sub(timing.initial_sp),
					&sources,
					ticks_per_second,
				)?,
				Some(periodic_sp_recovery_ticks(
					timing.sp_cost,
					&sources,
					ticks_per_second,
				)?),
			)
		}
		SkillSpType::IncreaseWhenAttack | SkillSpType::IncreaseWhenTakenDamage
			if timing.initial_sp >= timing.sp_cost =>
		{
			(0, None)
		}
		SkillSpType::IncreaseWhenAttack | SkillSpType::IncreaseWhenTakenDamage => {
			return Err(LevelSolvePrepareError::new(
					"unsupported_sp_recovery",
					"attack- and damage-recovery skills require modeled combat SP events; natural SP recovery modifiers do not charge them",
				));
		}
		SkillSpType::UnknownName(_) | SkillSpType::UnknownCode(_) => {
			return Err(LevelSolvePrepareError::new(
				"unsupported_sp_recovery",
				format!("unknown SP recovery type {:?}", timing.sp_type),
			));
		}
	};
	let first_ready: u64 = if matches!(timing.activation_type, SkillActivationType::Passive) {
		deploy_tick
	} else {
		deploy_tick
			.checked_add(initial_recovery_ticks)
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"activation_tick_overflow",
					"first skill readiness overflowed",
				)
			})?
	};
	if first_ready >= duty_window.end {
		return Ok(Vec::new());
	}

	let active_ticks: Option<u64> = active_duration_ticks(timing)?;
	let mut first_activation_ticks: BTreeSet<u64> = BTreeSet::from([first_ready]);
	match &timing.activation_type {
		SkillActivationType::Auto | SkillActivationType::Passive => {}
		SkillActivationType::Manual => {
			for &boundary in responsibility_boundaries {
				if boundary >= first_ready && boundary < duty_window.end {
					first_activation_ticks.insert(boundary);
				}
				if let Some(duration) = active_ticks {
					if let Some(aligned_start) = boundary.checked_sub(duration) {
						if aligned_start >= first_ready && aligned_start < duty_window.end {
							first_activation_ticks.insert(aligned_start);
						}
					}
				}
			}
		}
		SkillActivationType::Unknown(value) => {
			return Err(LevelSolvePrepareError::new(
				"unsupported_activation_type",
				format!("unknown skill activation type {value:?}"),
			));
		}
	}

	if first_activation_ticks.len() > maximum_schedules {
		return Err(LevelSolvePrepareError::new(
			"activation_schedule_limit_exceeded",
			format!(
				"complete activation enumeration requires {} schedules, limit is \
				 {maximum_schedules}",
				first_activation_ticks.len()
			),
		));
	}
	let mut schedules: Vec<Vec<TickSpan>> = Vec::new();
	for first_activation in first_activation_ticks {
		let schedule: Vec<TickSpan> = build_activation_schedule(
			timing,
			first_activation,
			duty_window,
			recurring_recovery_ticks,
		)?;
		if !schedule.is_empty() {
			schedules.push(schedule);
		}
	}
	schedules.sort();
	schedules.dedup();
	Ok(schedules)
}

/// Complete internal request after raw input decoding.
#[derive(Clone, Copy)]
struct LevelSolvePrepareRequest<'a> {
	pub snapshot: &'a ArkDataSnapshot,
	pub prefabs: &'a BattlePrefabSnapshot,
	pub requested_level: &'a str,
	pub guide_json: &'a str,
	pub guide_source_id: &'a str,
	pub guide_content_sha256: &'a str,
	pub difficulty: MaaJobDifficulty,
	pub roster: Option<&'a LevelSolveRoster>,
	pub excluded_operators: &'a BTreeSet<String>,
	pub top_k: usize,
	pub additive_proposal_limit: usize,
	pub replay_worker_count: usize,
	pub guide_selection_budget: usize,
}

/// Raw workflow inputs for one expected-damage solve.
#[derive(Clone, Copy)]
pub struct SolveRequest<'a> {
	pub snapshot: &'a ArkDataSnapshot,
	pub prefabs: &'a BattlePrefabSnapshot,
	pub requested_level: &'a str,
	pub guide_bytes: &'a [u8],
	pub guide_source_id: &'a str,
	pub difficulty: MaaJobDifficulty,
	pub roster_json: Option<&'a [u8]>,
	pub excluded_operators: &'a BTreeSet<String>,
	pub top_k: Option<usize>,
	pub additive_proposal_limit: usize,
	pub replay_worker_count: usize,
	pub guide_selection_budget: usize,
}

/// Thread-safe observer for long-running preparation, proposal generation, and replay work.
///
/// The solver never writes progress output itself. Callers that do not need progress reporting
/// should use [`solve`]; interactive adapters can provide an observer through
/// [`solve_with_observer`]. Implementations must return quickly because callbacks execute on the
/// proposal producer, replay workers, and ordered coordinator.
pub trait LevelSolveProgressObserver: Send + Sync {
	fn observe(&self, update: LevelSolveProgressUpdate);
}

#[derive(Clone, Debug)]
pub struct LevelSolveProgressUpdate {
	pub elapsed: Duration,
	pub event: LevelSolveProgressEvent,
}

#[derive(Clone, Debug)]
pub enum LevelSolveProgressEvent {
	PreparationStarted {
		proposal_limit: usize,
		requested_top_k: usize,
		ticks_per_second: u32,
	},
	SearchStarted {
		replay_worker_count: usize,
		replay_queue_capacity: usize,
		replay_inflight_capacity: usize,
	},
	SelectionStarted {
		proposal_index: usize,
	},
	SelectionFinished {
		proposal_index: usize,
		duration: Duration,
		engine: LevelSolveSelectionEngine,
		selected_candidate_count: usize,
	},
	JointPlanningStarted {
		proposal_index: usize,
		selected_candidates: Vec<LevelSolveCandidateProgress>,
	},
	JointPlanningFinished {
		proposal_index: usize,
		duration: Duration,
		outcome: LevelSolveJointPlanningOutcome,
	},
	ProposalGenerated {
		proposal_index: usize,
		selected_candidates: Vec<LevelSolveCandidateProgress>,
		total_deployment_cost: u64,
		replay_required: bool,
		proposal_limit: usize,
	},
	ProposalQueued {
		proposal_index: usize,
		replay_sequence: usize,
		queue_depth: usize,
		in_flight: usize,
	},
	ReplayProposalStarted {
		proposal_index: usize,
		replay_sequence: usize,
		selected_candidates: Vec<LevelSolveCandidateProgress>,
	},
	ReplayVariantStarted {
		proposal_index: usize,
		replay_sequence: usize,
		/// One-based index within this proposal.
		variant_index: usize,
		variant_count: usize,
		guide_variant_id: Option<String>,
		members: Vec<LevelSolveReplayMemberProgress>,
		dp_order: Vec<LevelSolveReplayDpStepProgress>,
	},
	ReplayVariantFinished {
		proposal_index: usize,
		replay_sequence: usize,
		/// One-based index within this proposal, or zero when no insertion plan materialized.
		variant_index: usize,
		variant_count: usize,
		duration: Duration,
		outcome: LevelSolveReplayVariantOutcome,
	},
	ReplayProposalExportFailed {
		proposal_index: usize,
		replay_sequence: usize,
		reason_code: String,
		detail: String,
	},
	ReplayProposalMaterialized {
		proposal_index: usize,
		replay_sequence: usize,
		artifact_id: String,
	},
	ReplayProposalAccepted {
		proposal_index: usize,
		replay_sequence: usize,
		artifact_id: String,
	},
	ReplayCompleted {
		proposal_index: usize,
		replay_sequence: usize,
		replay_completed: usize,
		verified_jobs: usize,
		queue_depth: usize,
		in_flight: usize,
		duration: Duration,
	},
	SearchFinished {
		proposals_generated: usize,
		proposals_queued: usize,
		replay_completed: usize,
		verified_jobs: usize,
	},
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LevelSolveSelectionEngine {
	SingletonFastPath,
	MixedInteger,
}

#[derive(Clone, Debug)]
pub enum LevelSolveJointPlanningOutcome {
	Scored {
		damage_feasible: bool,
		minimum_cumulative_coverage_millionths: u64,
		total_due_remaining_hp_scaled: u64,
	},
	Failed {
		reason_code: String,
		detail: String,
	},
}

#[derive(Clone, Debug)]
pub struct LevelSolveCandidateProgress {
	pub candidate_key: String,
	pub operator_id: Option<String>,
	pub operator_name: Option<String>,
	pub stable_build_id: Option<String>,
	pub elite: Option<u8>,
	pub level: Option<u32>,
	pub skill: Option<u8>,
	pub skill_level: Option<u8>,
	pub potential: Option<u8>,
	pub module: Option<String>,
	pub deployment_cell: Option<GridPosition>,
	pub facing: Option<Facing>,
	pub deploy_tick: Option<u64>,
	pub active_spans: Vec<TickSpan>,
}

#[derive(Clone, Debug)]
pub struct LevelSolveReplayMemberProgress {
	pub candidate: LevelSolveCandidateProgress,
	pub deploy_tick: u64,
	pub active_spans: Vec<TickSpan>,
}

#[derive(Clone, Debug)]
pub struct LevelSolveReplayDpStepProgress {
	pub stable_key: String,
	pub deploy_tick: u64,
	pub cost: u64,
	pub dp_before: u64,
	pub dp_after: u64,
}

#[derive(Clone, Debug)]
pub struct LevelSolveEnemyProgress {
	pub instance_id: u32,
	pub enemy_key: String,
	pub remaining_hp_micros: u64,
	pub leaked_tick: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct LevelSolveDeadOperatorProgress {
	pub instance_id: u32,
	pub operator_id: String,
	pub death_tick: u64,
}

#[derive(Clone, Debug)]
pub struct LevelSolvePendingActionProgress {
	pub action_index: usize,
	pub kind: String,
	pub operator_id: Option<String>,
	pub requested_tick: u64,
	pub status: String,
}

#[derive(Clone, Debug)]
pub enum LevelSolveReplayVariantOutcome {
	ExpectedVictory {
		success_probability_millionths: u32,
	},
	UnverifiedVictory {
		success_probability_millionths: Option<u32>,
		issues: Vec<String>,
	},
	GoalLeak {
		terminal: Option<String>,
		leaked_enemies: Vec<LevelSolveEnemyProgress>,
		dead_operators: Vec<LevelSolveDeadOperatorProgress>,
	},
	ExpectedDefeat {
		terminal: Option<String>,
		remaining_enemies: Vec<LevelSolveEnemyProgress>,
		dead_operators: Vec<LevelSolveDeadOperatorProgress>,
	},
	/// A replay without a certified verdict. A modelled leak lands here when an omitted effect
	/// might have prevented it, so its leaked enemies stay visible.
	Incomplete {
		terminal: Option<String>,
		issues: Vec<String>,
		pending_actions: Vec<LevelSolvePendingActionProgress>,
		leaked_enemies: Vec<LevelSolveEnemyProgress>,
		dead_operators: Vec<LevelSolveDeadOperatorProgress>,
		unsupported_count: usize,
		unmodeled_count: usize,
		conservative_count: usize,
		approximated_count: usize,
	},
	Unsupported {
		issues: Vec<String>,
		unsupported_count: usize,
		unmodeled_count: usize,
		conservative_count: usize,
		approximated_count: usize,
	},
	MaterializationFailed {
		reason_code: String,
		detail: String,
	},
}

fn observe_level_solve_progress<F>(
	observer: Option<&dyn LevelSolveProgressObserver>,
	started: Instant,
	event: F,
) where
	F: FnOnce() -> LevelSolveProgressEvent,
{
	if let Some(observer) = observer {
		let _ignored_observer_panic = catch_unwind(AssertUnwindSafe(|| {
			observer.observe(LevelSolveProgressUpdate {
				elapsed: started.elapsed(),
				event: event(),
			});
		}));
	}
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedDamagePreparedClaim {
	/// Reviewed base attacks plus reviewed active skill phases. Buffs, blocking, and unreviewed
	/// mechanics remain outside this expected-damage claim.
	ExpectedDamageBaseAndSkillPhases,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedDamageBuildPolicy {
	E2MaxTrust200Skill10Potential6NoModule,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageActivationAudit {
	pub start_tick: u64,
	pub end_tick: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamagePreparedCandidateAudit {
	pub candidate_key: String,
	pub operator_id: String,
	pub operator_name: String,
	pub skill_id: String,
	pub deployment_cost: u32,
	pub deployment_cell: GridPosition,
	pub facing: Facing,
	pub deploy_tick: u64,
	pub activations: Vec<ExpectedDamageActivationAudit>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedDamageProbabilityBasis {
	FullBattlefieldReplay,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageDpRulesAudit {
	pub initial_dp: u64,
	pub max_dp: u64,
	pub regen_interval_ticks: u64,
	pub dp_per_interval: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageDpEntryAudit {
	pub action_index: usize,
	pub operator_name: String,
	pub deploy_tick: u64,
	pub deployment_cost: u64,
	pub regenerated_dp: u64,
	/// DP this deployment received from automatic skill grants planning scheduled.
	pub granted_dp: u64,
	pub dp_before_deployment: u64,
	pub dp_after_deployment: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageDpAudit {
	pub rules: ExpectedDamageDpRulesAudit,
	pub entries: Vec<ExpectedDamageDpEntryAudit>,
}

/// One independently executable strategy whose full battlefield replay reached expected victory.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpectedDamagePreparedJob {
	pub schema_version: u32,
	pub data_revision: String,
	pub mechanics_revision: String,
	pub proposal_index: usize,
	pub candidate_keys: Vec<String>,
	pub estimated_success_probability_millionths: u32,
	pub probability_basis: ExpectedDamageProbabilityBasis,
	pub strategy_sha256: String,
	pub artifact_id: String,
	pub maa_job: MaaJob,
	pub exact_builds: Vec<ArtifactExactBuild>,
	pub dp: ExpectedDamageDpAudit,
	pub expected_damage: ExpectedDamageJointEvidence,
	pub battlefield_status: ExpectedReplayReportStatus,
	#[serde(skip_serializing)]
	pub maa_job_json: String,
	#[serde(skip_serializing)]
	pub artifact: StrategyArtifact,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJobMaterializationFailure {
	pub proposal_index: usize,
	pub candidate_keys: Vec<String>,
	pub reason_code: String,
	pub detail: String,
}

/// Complete input for independently validating one exported MAA job file.
///
/// The original source guide is required to rebuild the retained-only full-battle gap and exact
/// planning context. The exported strategy and artifact are treated only as untrusted inputs:
/// exact builds, DP timing, skill readiness, geometry, and joint damage are recomputed from the
/// snapshot.
#[derive(Clone, Copy)]
pub(super) struct RebuiltExportValidationRequest<'a> {
	pub snapshot: &'a ArkDataSnapshot,
	pub prefabs: &'a BattlePrefabSnapshot,
	pub requested_level: &'a str,
	pub source_guide_json: &'a str,
	pub source_guide_source_id: &'a str,
	pub source_guide_content_sha256: &'a str,
	pub maa_job_json: &'a str,
	pub artifact: &'a StrategyArtifact,
	/// Exact artifact file bytes. They must equal the artifact's canonical encoding.
	pub artifact_bytes: &'a [u8],
	pub difficulty: MaaJobDifficulty,
	pub guide_selection_budget: usize,
}

/// Independent expected-damage validation result for one exported strategy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpectedDamageExportValidationReport {
	pub schema_version: u32,
	pub data_revision: String,
	pub mechanics_revision: String,
	pub requested_level: String,
	pub stage_id: String,
	pub source_guide_source_id: String,
	pub source_guide_content_sha256: String,
	pub source_guide_difficulty: MaaJobDifficulty,
	pub source_guide_selection_index: usize,
	pub source_guide_selected_slots: BTreeMap<String, String>,
	pub source_action_index: usize,
	pub source_operator_id: String,
	pub source_operator_name: String,
	pub strategy_sha256: String,
	pub artifact_id: String,
	pub damage_status: DeterministicDamageStatus,
	pub estimated_success_probability_millionths: u32,
	pub probability_basis: ExpectedDamageProbabilityBasis,
	pub dp: ExpectedDamageDpAudit,
	pub expected_damage: ExpectedDamageJointEvidence,
	pub battlefield_status: ExpectedReplayReportStatus,
	pub limitations: Vec<String>,
}

/// Serializable output around the core MILP/joint-validation report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpectedDamagePreparedLevelReport {
	pub schema_version: u32,
	pub claim: ExpectedDamagePreparedClaim,
	pub build_policy: ExpectedDamageBuildPolicy,
	pub data_revision: String,
	pub mechanics_revision: String,
	pub requested_level: String,
	pub stage_id: String,
	pub level_id: String,
	pub guide_source_id: String,
	pub guide_content_sha256: String,
	pub guide_difficulty: MaaJobDifficulty,
	pub guide_selection_index: usize,
	pub guide_selected_slots: BTreeMap<String, String>,
	pub source_action_index: usize,
	pub source_operator_id: String,
	pub source_operator_name: String,
	/// DP-derived deployment tick of the source guide slot before replacement.
	pub source_guide_deploy_tick: u64,
	/// Every cohort in the complete active stage model used for joint expected-damage scoring.
	pub effective_target_cohort_ids: Vec<String>,
	pub materialized_candidates: Vec<ExpectedDamagePreparedCandidateAudit>,
	/// Authoritative retained-only counterfactual across every causally started wave.
	pub full_battle_damage_gap: FullBattleDamageGapLedger,
	/// Requested count of fully replay-verified and exportable jobs.
	pub requested_top_k: usize,
	/// Sole budget for distinct additive proposals enumerated and jointly checked.
	pub additive_proposal_limit: usize,
	/// CPU workers that consumed the bounded replay/export queue.
	pub replay_worker_count: usize,
	/// Maximum number of replay tasks waiting between producer and consumers.
	pub replay_queue_capacity: usize,
	/// Final verified-search outcome. Planning candidates do not count toward this status.
	pub verified_search_status: ExpectedDamageVerifiedSearchStatus,
	pub solve: ExpectedDamageLevelSolveReport,
	/// Complete, round-tripped MAA job files accepted by full battlefield replay.
	pub battlefield_verified_jobs: Vec<ExpectedDamagePreparedJob>,
	pub job_materialization_failures: Vec<ExpectedDamageJobMaterializationFailure>,
	/// Full battlefield replay for each proposal actually attempted before verified search stopped.
	pub battlefield_replays: Vec<ExpectedDamageBattlefieldReplayAudit>,
	/// Attempted proposals whose separate battlefield replay reached expected victory, including
	/// victories that could not be exported as jobs.
	pub battlefield_expected_victory_proposal_indices: Vec<usize>,
	pub limitations: Vec<String>,
}

/// Why replay-driven verified-job search stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedDamageVerifiedSearchStatus {
	TopKFilled,
	ProposalBudgetExhausted,
	AdditiveInfeasible,
	AdditiveSearchExhausted,
	PlanningModelIncomplete,
	CandidateMaterializationIncomplete,
	ProposalStreamCancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpectedDamageBattlefieldReplayAudit {
	pub proposal_index: usize,
	pub candidate_keys: Vec<String>,
	pub result: ExpectedDamageBattlefieldReplayResult,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExpectedDamageBattlefieldReplayResult {
	Completed {
		report: Box<ExpectedReplayAuditReport>,
	},
	MaterializationFailed {
		reason_code: String,
		detail: String,
	},
}

/// Stable adapter error with a machine-readable code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelSolvePrepareError {
	code: &'static str,
	detail: String,
}

impl LevelSolvePrepareError {
	#[must_use]
	pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
		Self {
			code,
			detail: detail.into(),
		}
	}

	#[must_use]
	pub const fn code(&self) -> &'static str {
		self.code
	}

	#[must_use]
	pub fn detail(&self) -> &str {
		&self.detail
	}
}

impl Display for LevelSolvePrepareError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		write!(formatter, "{}: {}", self.code, self.detail)
	}
}

impl Error for LevelSolvePrepareError {}

#[derive(Clone, Copy)]
enum SourceGuideDerivationPurpose {
	Solve,
	IndependentValidation,
}

impl SourceGuideDerivationPurpose {
	const fn import_error_code(self) -> &'static str {
		match self {
			Self::Solve => "guide_import_failed",
			Self::IndependentValidation => "source_guide_import_failed",
		}
	}

	const fn lineage_error_code(self) -> &'static str {
		match self {
			Self::Solve => "invalid_guide_lineage",
			Self::IndependentValidation => "invalid_source_guide_lineage",
		}
	}

	const fn selection_error_code(self) -> &'static str {
		match self {
			Self::Solve => "guide_selection_failed",
			Self::IndependentValidation => "source_guide_selection_failed",
		}
	}

	const fn selection_not_found_error_code(self) -> &'static str {
		match self {
			Self::Solve => "guide_selection_not_found",
			Self::IndependentValidation => "source_guide_selection_not_found",
		}
	}

	const fn ambiguous_selection_error_code(self) -> &'static str {
		match self {
			Self::Solve => "ambiguous_guide_selection",
			Self::IndependentValidation => "ambiguous_source_guide_selection",
		}
	}

	const fn truncated_selection_error_code(self) -> &'static str {
		match self {
			Self::Solve => "guide_selection_truncated",
			Self::IndependentValidation => "source_guide_selection_truncated",
		}
	}

	const fn domain_error_code(self) -> &'static str {
		match self {
			Self::Solve => "guide_domain_materialization_failed",
			Self::IndependentValidation => "source_guide_domain_materialization_failed",
		}
	}

	const fn ambiguous_selection_detail(self) -> &'static str {
		match self {
			Self::Solve => {
				"level solve requires a source guide that materializes exactly one executable \
				 selection; collapse MAA groups to the intended retained operators"
			}
			Self::IndependentValidation => {
				"independent validation requires a source guide that materializes exactly one \
				 executable selection"
			}
		}
	}
}

#[derive(Clone, Copy)]
struct SourceGuideDerivationRequest<'a> {
	snapshot: &'a ArkDataSnapshot,
	requested_level: &'a str,
	guide_json: &'a str,
	guide_source_id: &'a str,
	guide_content_sha256: &'a str,
	difficulty: MaaJobDifficulty,
	selection_budget: usize,
	purpose: SourceGuideDerivationPurpose,
}

struct DerivedSourceGuide {
	guide: MaaJob,
	seed: MaaJobSeed,
	provenance: SourceGuideProvenance,
	stage: CompiledStage,
	domain: GuideDeploymentDomain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceGuideProvenance {
	source_id: String,
	content_sha256: String,
	difficulty: MaaJobDifficulty,
	selection_index: usize,
	selected_slots: BTreeMap<String, String>,
	source_action_index: usize,
	source_operator_name: String,
}

fn source_guide_provenance(
	seed: &MaaJobGuideSeed<'_>,
	selection_index: usize,
) -> Result<SourceGuideProvenance, LevelSolvePrepareError> {
	let selected_slots: BTreeMap<String, String> = seed
		.build_hints()
		.map(|build| {
			(
				build.slot_name().to_owned(),
				build.operator_name().to_owned(),
			)
		})
		.collect();
	let fixed_source_deployments: Vec<(usize, String)> = seed
		.deployment_hints()
		.filter(|deployment| deployment.build().group_name().is_none())
		.map(|deployment| {
			(
				deployment.action_index(),
				deployment.build().operator_name().to_owned(),
			)
		})
		.collect();
	let [(source_action_index, source_operator_name)]: [(usize, String); 1] =
		fixed_source_deployments
			.try_into()
			.map_err(|deployments: Vec<(usize, String)>| {
				LevelSolvePrepareError::new(
					"ambiguous_source_deployment",
					format!(
						"source guide must have exactly one fixed-operator Deploy action, found \
						 {deployments:?}"
					),
				)
			})?;

	Ok(SourceGuideProvenance {
		source_id: seed.source_id().to_owned(),
		content_sha256: seed.content_sha256().to_owned(),
		difficulty: seed.difficulty(),
		selection_index,
		selected_slots,
		source_action_index,
		source_operator_name,
	})
}

fn derive_source_guide(
	request: SourceGuideDerivationRequest<'_>,
) -> Result<DerivedSourceGuide, LevelSolvePrepareError> {
	let guide: MaaJob = parse_maa_job(request.guide_json).map_err(|error| {
		LevelSolvePrepareError::new(request.purpose.import_error_code(), error.to_string())
	})?;
	let lineage: MaaJobSourceLineage<'_> =
		MaaJobSourceLineage::new(request.guide_source_id, request.guide_content_sha256).map_err(
			|error| {
				LevelSolvePrepareError::new(request.purpose.lineage_error_code(), error.to_string())
			},
		)?;
	let mut guide_seeds: MaaJobGuideSeedIter<'_> = guide
		.executable_guide_seeds(lineage, request.difficulty, request.selection_budget)
		.map_err(|error| {
			LevelSolvePrepareError::new(request.purpose.selection_error_code(), error.to_string())
		})?;
	let seed: MaaJobGuideSeed<'_> = match guide_seeds.next() {
		Some(seed) => seed,
		None if guide_seeds.report().truncated() => {
			return Err(LevelSolvePrepareError::new(
				request.purpose.truncated_selection_error_code(),
				format!(
					"source guide expansion exceeded budget {}; selection is not proven unique",
					request.selection_budget
				),
			));
		}
		None => {
			return Err(LevelSolvePrepareError::new(
				request.purpose.selection_not_found_error_code(),
				format!(
					"source guide has no executable {:?} selection",
					request.difficulty
				),
			));
		}
	};
	if guide_seeds.next().is_some() {
		return Err(LevelSolvePrepareError::new(
			request.purpose.ambiguous_selection_error_code(),
			request.purpose.ambiguous_selection_detail(),
		));
	}
	let selection_truncated: bool = guide_seeds.report().truncated();
	if selection_truncated {
		return Err(LevelSolvePrepareError::new(
			request.purpose.truncated_selection_error_code(),
			format!(
				"source guide expansion exceeded budget {}; selection is not proven unique",
				request.selection_budget
			),
		));
	}

	let selection_index: usize = 0;
	let provenance: SourceGuideProvenance = source_guide_provenance(&seed, selection_index)?;
	let stage_difficulty: StageDifficulty =
		replay_stage_difficulty(seed.difficulty()).map_err(|error| {
			LevelSolvePrepareError::new("difficulty_mapping_failed", error.to_string())
		})?;
	let stage: CompiledStage = request
		.snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(seed.stage_name().to_owned()),
			difficulty: stage_difficulty,
		})
		.map_err(|error| {
			LevelSolvePrepareError::new("stage_resolution_failed", error.to_string())
		})?;
	validate_requested_level(request.requested_level, &stage)?;
	let build_pool: CandidatePoolSpec = guide_base_panel_build_pool(request.snapshot, &seed)?;
	let domain: GuideDeploymentDomain = guide_deployment_domain_from_maa_job(
		&seed,
		provenance.source_action_index,
		stage.map_dimensions(),
		build_pool,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new(request.purpose.domain_error_code(), error.to_string())
	})?;
	let materialized_seed: MaaJobSeed = seed.materialize();

	Ok(DerivedSourceGuide {
		guide,
		seed: materialized_seed,
		provenance,
		stage,
		domain,
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CanonicalStageIdentity<'a> {
	stage_id: &'a str,
	level_id: &'a str,
	difficulty: StageDifficulty,
}

impl<'a> From<&'a CompiledStage> for CanonicalStageIdentity<'a> {
	fn from(stage: &'a CompiledStage) -> Self {
		Self {
			stage_id: stage.stage_id(),
			level_id: stage.level_id(),
			difficulty: stage.difficulty(),
		}
	}
}

fn require_matching_stage_identity(
	source: CanonicalStageIdentity<'_>,
	exported: CanonicalStageIdentity<'_>,
) -> Result<(), LevelSolvePrepareError> {
	if exported == source {
		return Ok(());
	}
	Err(LevelSolvePrepareError::new(
		"exported_strategy_stage_mismatch",
		format!("exported stage {exported:?} does not match source stage {source:?}"),
	))
}

fn validate_exported_stage_matches_source(
	snapshot: &ArkDataSnapshot,
	source_stage: &CompiledStage,
	exported_seed: &MaaJobSeed,
) -> Result<(), LevelSolvePrepareError> {
	let exported_difficulty: StageDifficulty = replay_stage_difficulty(exported_seed.difficulty())
		.map_err(|error| {
			LevelSolvePrepareError::new("export_difficulty_mapping_failed", error.to_string())
		})?;
	let exported_stage: CompiledStage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(exported_seed.stage_name().to_owned()),
			difficulty: exported_difficulty,
		})
		.map_err(|error| {
			LevelSolvePrepareError::new(
				"exported_strategy_stage_resolution_failed",
				error.to_string(),
			)
		})?;
	require_matching_stage_identity(source_stage.into(), (&exported_stage).into())
}

struct MaterializedSkillCandidate {
	key: ContributionCandidateKey,
	candidate: Candidate,
	exact_build: OperatorBuild,
	skill_id: String,
	manual_activation: bool,
	/// Baseline tick used while enumerating this candidate's relative activation schedule.
	deploy_tick: u64,
	earliest_legal_activation_tick: Option<u64>,
	active_spans: Vec<TickSpan>,
}

#[derive(Clone, Debug)]
struct JointInsertionMemberTiming {
	candidate_key: ContributionCandidateKey,
	deploy_tick: u64,
	earliest_legal_activation_tick: Option<u64>,
	active_spans: Vec<TickSpan>,
}

#[derive(Clone, Debug)]
struct JointInsertionPlan {
	members: Vec<JointInsertionMemberTiming>,
	retained_deploy_ticks: BTreeMap<usize, u64>,
	dp_schedule: Vec<ScheduledDeploymentDpAction>,
}

struct JointExpectedDamageEvaluator {
	original_schedule: PreparedGuideDeploymentSchedule,
	candidates: BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	model: StageSpaceTimeModel,
	target_cohort_ids: BTreeSet<String>,
	duty_window: TickSpan,
}

#[derive(Clone, Debug)]
struct ReplayWork {
	sequence: usize,
	proposal_index: usize,
	candidate_keys: Vec<String>,
}

struct ReplayWorkerContext<'a> {
	evaluator: &'a JointExpectedDamageEvaluator,
	snapshot: &'a ArkDataSnapshot,
	prefabs: &'a BattlePrefabSnapshot,
	stage: &'a CompiledStage,
	source_guide: &'a MaaJob,
	source_guide_json: &'a str,
	source_guide_source_id: &'a str,
	requested_level: &'a str,
	source_seed: &'a MaaJobSeed,
	source_bindings: &'a [ExactReplayBuildBinding],
	source_slot_name: &'a str,
	guide_selection_budget: usize,
	guide_sha256: [u8; 32],
	tick_rate: TickRate,
	progress_observer: Option<&'a dyn LevelSolveProgressObserver>,
	progress_started: Instant,
}

struct ReplayAttempt {
	sequence: usize,
	proposal_index: usize,
	replay: ExpectedDamageBattlefieldReplayAudit,
	expected_victory: bool,
	job: Option<ExpectedDamagePreparedJob>,
	job_failure: Option<ExpectedDamageJobMaterializationFailure>,
}

struct ReplayWorkerMessage {
	sequence: usize,
	proposal_index: usize,
	duration: Duration,
	result: Result<ReplayAttempt, LevelSolvePrepareError>,
}

fn complete_replay_work<F>(
	sequence: usize,
	proposal_index: usize,
	started: Instant,
	execute: F,
) -> ReplayWorkerMessage
where
	F: FnOnce() -> Result<ReplayAttempt, LevelSolvePrepareError>,
{
	let result: Result<ReplayAttempt, LevelSolvePrepareError> =
		match catch_unwind(AssertUnwindSafe(execute)) {
			Ok(result) => result,
			Err(payload) => {
				let panic_detail: &str = payload
					.downcast_ref::<&str>()
					.copied()
					.or_else(|| payload.downcast_ref::<String>().map(String::as_str))
					.unwrap_or("non-string panic payload");
				Err(LevelSolvePrepareError::new(
					"replay_worker_panicked",
					format!(
						"replay work sequence {sequence} for proposal {proposal_index} panicked: \
						 {panic_detail}"
					),
				))
			}
		};
	ReplayWorkerMessage {
		sequence,
		proposal_index,
		duration: started.elapsed(),
		result,
	}
}

struct VerifiedSearchPipelineResult {
	solve: ExpectedDamageLevelSolveReport,
	battlefield_verified_jobs: Vec<ExpectedDamagePreparedJob>,
	job_materialization_failures: Vec<ExpectedDamageJobMaterializationFailure>,
	battlefield_replays: Vec<ExpectedDamageBattlefieldReplayAudit>,
	battlefield_expected_victory_proposal_indices: Vec<usize>,
	replay_queue_capacity: usize,
}

impl JointExpectedDamageEvaluator {
	fn insertion_plans(
		&self,
		selected_candidate_keys: &[ContributionCandidateKey],
	) -> Result<Vec<JointInsertionPlan>, ExpectedDamageJointEvaluationFailure> {
		enumerate_joint_insertion_plans(
			&self.original_schedule,
			&self.candidates,
			selected_candidate_keys,
		)
	}

	fn score_plan(
		&self,
		plan: &JointInsertionPlan,
	) -> Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure> {
		let mut deployments: Vec<ResolvedCoverageDeployment<'_>> = self
			.original_schedule
			.deployments
			.iter()
			.filter_map(|deployment| {
				let candidate = deployment.retained_coverage_candidate.as_ref()?;
				let deploy_tick = plan
					.retained_deploy_ticks
					.get(&deployment.action.action_index)
					.copied()?;
				Some(ResolvedCoverageDeployment::new(candidate, deploy_tick))
			})
			.collect();
		for member in &plan.members {
			let materialized = self.candidates.get(&member.candidate_key).ok_or_else(|| {
				ExpectedDamageJointEvaluationFailure::new(
					"missing_materialized_candidate",
					format!(
						"candidate {} is absent from the adapter",
						member.candidate_key
					),
				)
			})?;
			deployments.push(
				ResolvedCoverageDeployment::new(&materialized.candidate, member.deploy_tick)
					.with_active_spans(member.active_spans.clone()),
			);
		}
		let score: JointCoverageScore = score_joint_deployments(JointCoverageScoreRequest {
			model: &self.model,
			deployments: &deployments,
			target_cohort_ids: &self.target_cohort_ids,
			duty_window: self.duty_window,
		})
		.map_err(|error| {
			ExpectedDamageJointEvaluationFailure::new(
				"joint_expected_damage_scoring_failed",
				error.to_string(),
			)
		})?;
		Ok(ExpectedDamageJointEvidence::from(&score))
	}
}

impl ExpectedDamageJointEvaluator for JointExpectedDamageEvaluator {
	fn evaluate(
		&self,
		selected_candidate_keys: &[ContributionCandidateKey],
	) -> Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure> {
		let plans: Vec<JointInsertionPlan> = self.insertion_plans(selected_candidate_keys)?;
		let mut best: Option<ExpectedDamageJointEvidence> = None;
		for plan in &plans {
			let evidence: ExpectedDamageJointEvidence = self.score_plan(plan)?;
			let replace: bool = best
				.as_ref()
				.is_none_or(|current| expected_damage_evidence_is_better(&evidence, current));
			if replace {
				best = Some(evidence);
			}
		}
		best.ok_or_else(|| {
			ExpectedDamageJointEvaluationFailure::new(
				"joint_insertion_schedule_not_materialized",
				"selected proposal produced no legal joint deployment ordering",
			)
		})
	}
}

struct ProgressJointExpectedDamageEvaluator<'a> {
	inner: &'a JointExpectedDamageEvaluator,
	observer: Option<&'a dyn LevelSolveProgressObserver>,
	progress_started: Instant,
	explore_cardinalities: bool,
	planning_model_complete: bool,
	next_proposal_index: AtomicUsize,
	selection_started: Mutex<Option<(usize, Instant)>>,
}

impl ProgressJointExpectedDamageEvaluator<'_> {
	fn start_selection(&self, proposal_index: usize) {
		if let Ok(mut started) = self.selection_started.lock() {
			*started = Some((proposal_index, Instant::now()));
		}
		observe_level_solve_progress(self.observer, self.progress_started, || {
			LevelSolveProgressEvent::SelectionStarted { proposal_index }
		});
	}

	fn selection_duration(&self, proposal_index: usize) -> Duration {
		let Ok(mut started) = self.selection_started.lock() else {
			return Duration::ZERO;
		};
		started
			.take()
			.filter(|(index, _started)| *index == proposal_index)
			.map_or(Duration::ZERO, |(_index, started)| started.elapsed())
	}

	fn selection_engine(&self, selected_candidate_count: usize) -> LevelSolveSelectionEngine {
		if self.explore_cardinalities
			&& !self.planning_model_complete
			&& selected_candidate_count == 1
		{
			LevelSolveSelectionEngine::SingletonFastPath
		} else {
			LevelSolveSelectionEngine::MixedInteger
		}
	}
}

impl ExpectedDamageJointEvaluator for ProgressJointExpectedDamageEvaluator<'_> {
	fn evaluate(
		&self,
		selected_candidate_keys: &[ContributionCandidateKey],
	) -> Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure> {
		let proposal_index: usize = self.next_proposal_index.fetch_add(1, Ordering::AcqRel);
		let selection_duration: Duration = self.selection_duration(proposal_index);
		let selection_engine: LevelSolveSelectionEngine =
			self.selection_engine(selected_candidate_keys.len());
		observe_level_solve_progress(self.observer, self.progress_started, || {
			LevelSolveProgressEvent::SelectionFinished {
				proposal_index,
				duration: selection_duration,
				engine: selection_engine,
				selected_candidate_count: selected_candidate_keys.len(),
			}
		});
		observe_level_solve_progress(self.observer, self.progress_started, || {
			LevelSolveProgressEvent::JointPlanningStarted {
				proposal_index,
				selected_candidates: candidate_progress_many(self.inner, selected_candidate_keys),
			}
		});
		let joint_started: Instant = Instant::now();
		let result: Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure> =
			self.inner.evaluate(selected_candidate_keys);
		let joint_duration: Duration = joint_started.elapsed();
		observe_level_solve_progress(self.observer, self.progress_started, || {
			let outcome: LevelSolveJointPlanningOutcome = match &result {
				Ok(evidence) => LevelSolveJointPlanningOutcome::Scored {
					damage_feasible: evidence.deterministic_expected_damage_feasible,
					minimum_cumulative_coverage_millionths: evidence
						.minimum_cumulative_coverage_millionths,
					total_due_remaining_hp_scaled: evidence.total_due_remaining_hp_scaled,
				},
				Err(error) => LevelSolveJointPlanningOutcome::Failed {
					reason_code: error.reason_code.clone(),
					detail: error.detail.clone(),
				},
			};
			LevelSolveProgressEvent::JointPlanningFinished {
				proposal_index,
				duration: joint_duration,
				outcome,
			}
		});
		result
	}
}

fn candidate_progress(
	evaluator: &JointExpectedDamageEvaluator,
	candidate_key: &ContributionCandidateKey,
) -> LevelSolveCandidateProgress {
	let candidate = evaluator.candidates.get(candidate_key);
	LevelSolveCandidateProgress {
		candidate_key: candidate_key.as_str().to_owned(),
		operator_id: candidate.map(|candidate| candidate.candidate.operator_id().to_owned()),
		operator_name: candidate.map(|candidate| candidate.candidate.operator_name().to_owned()),
		stable_build_id: candidate
			.map(|candidate| candidate.candidate.stable_build_id().to_owned()),
		elite: candidate.map(|candidate| candidate.exact_build.elite()),
		level: candidate.map(|candidate| candidate.exact_build.level()),
		skill: candidate.map(|candidate| candidate.exact_build.skill()),
		skill_level: candidate.map(|candidate| candidate.exact_build.skill_level()),
		potential: candidate.map(|candidate| candidate.exact_build.potential().number()),
		module: candidate.map(|candidate| format!("{:?}", candidate.exact_build.module())),
		deployment_cell: candidate.map(|candidate| candidate.candidate.deployment_cell()),
		facing: candidate.map(|candidate| candidate.candidate.facing()),
		deploy_tick: candidate.map(|candidate| candidate.deploy_tick),
		active_spans: candidate
			.map(|candidate| candidate.active_spans.clone())
			.unwrap_or_default(),
	}
}

fn candidate_progress_many(
	evaluator: &JointExpectedDamageEvaluator,
	candidate_keys: &[ContributionCandidateKey],
) -> Vec<LevelSolveCandidateProgress> {
	candidate_keys
		.iter()
		.map(|candidate_key| candidate_progress(evaluator, candidate_key))
		.collect()
}

fn replay_variant_plan_progress(
	evaluator: &JointExpectedDamageEvaluator,
	plan: &JointInsertionPlan,
) -> (
	Vec<LevelSolveReplayMemberProgress>,
	Vec<LevelSolveReplayDpStepProgress>,
) {
	let members: Vec<LevelSolveReplayMemberProgress> = plan
		.members
		.iter()
		.map(|member| LevelSolveReplayMemberProgress {
			candidate: candidate_progress(evaluator, &member.candidate_key),
			deploy_tick: member.deploy_tick,
			active_spans: member.active_spans.clone(),
		})
		.collect();
	let dp_order: Vec<LevelSolveReplayDpStepProgress> = plan
		.dp_schedule
		.iter()
		.map(|step| LevelSolveReplayDpStepProgress {
			stable_key: step.action.stable_key.clone(),
			deploy_tick: step.deploy_tick,
			cost: step.action.cost,
			dp_before: step.dp_before_deployment,
			dp_after: step.dp_after_deployment,
		})
		.collect();
	(members, dp_order)
}

fn replay_dead_operators(
	report: &ExpectedReplayAuditReport,
) -> Vec<LevelSolveDeadOperatorProgress> {
	report
		.operators
		.iter()
		.filter_map(|operator| {
			operator
				.death_tick
				.map(|death_tick| LevelSolveDeadOperatorProgress {
					instance_id: operator.instance_id,
					operator_id: operator.operator_id.clone(),
					death_tick,
				})
		})
		.collect()
}

fn replay_enemy_progress(
	report: &ExpectedReplayAuditReport,
	filter: impl Fn(&crate::replay::ReplayEnemyAudit) -> bool,
) -> Vec<LevelSolveEnemyProgress> {
	report
		.enemies
		.iter()
		.filter(|enemy| filter(enemy))
		.map(|enemy| LevelSolveEnemyProgress {
			instance_id: enemy.instance_id,
			enemy_key: enemy.enemy_key.clone(),
			remaining_hp_micros: enemy.remaining_hp_micros,
			leaked_tick: enemy.leaked_tick,
		})
		.collect()
}

fn replay_pending_actions(
	report: &ExpectedReplayAuditReport,
) -> Vec<LevelSolvePendingActionProgress> {
	report
		.actions
		.iter()
		.filter(|action| action.executed_tick.is_none())
		.map(|action| LevelSolvePendingActionProgress {
			action_index: action.action_index,
			kind: action.kind.clone(),
			operator_id: action.operator_id.clone(),
			requested_tick: action.requested_tick,
			status: action.status.clone(),
		})
		.collect()
}

fn replay_failure_issues(report: &ExpectedReplayAuditReport) -> Vec<String> {
	let mut issues: Vec<String> = report.compile_issues.clone();
	issues.extend(report.simulation_issues.iter().cloned());
	issues.extend(report.unsupported.iter().map(|mechanic| {
		format!(
			"unsupported {}: {}",
			mechanic.mechanic_id,
			mechanic.reasons.join(", ")
		)
	}));
	issues.extend(report.unmodeled_omissions.iter().map(|mechanic| {
		format!(
			"unmodeled {}: {}",
			mechanic.mechanic_id,
			mechanic.reasons.join(", ")
		)
	}));
	issues.extend(report.conservative_omissions.iter().map(|mechanic| {
		format!(
			"conservative {}: {}",
			mechanic.mechanic_id,
			mechanic.reasons.join(", ")
		)
	}));
	issues.extend(
		report
			.declared_approximations
			.iter()
			.filter(|mechanic| mechanic.exercised)
			.map(|mechanic| {
				format!(
					"approximated {}: {}",
					mechanic.mechanic_id,
					mechanic.reasons.join(", ")
				)
			}),
	);
	issues
}

/// The declared approximations this battle exercised, which are the ones that withheld a verdict.
fn exercised_approximation_count(report: &ExpectedReplayAuditReport) -> usize {
	report
		.declared_approximations
		.iter()
		.filter(|mechanic| mechanic.exercised)
		.count()
}

fn replay_variant_outcome(report: &ExpectedReplayAuditReport) -> LevelSolveReplayVariantOutcome {
	match &report.status {
		ExpectedReplayReportStatus::ExpectedVictory => {
			if verified_expected_victory_probability(
				&report.status,
				report.defensive_probability.as_ref(),
			)
			.is_some()
			{
				LevelSolveReplayVariantOutcome::ExpectedVictory {
					success_probability_millionths: report
						.defensive_probability
						.as_ref()
						.expect("verified probability was checked above")
						.success_probability_millionths,
				}
			} else {
				let mut issues: Vec<String> = report
					.defensive_probability
					.as_ref()
					.map_or_else(Vec::new, |audit| audit.issues.clone());
				if report.defensive_probability.is_none() {
					issues.push("defensive probability audit is unavailable".to_owned());
				} else if issues.is_empty() {
					issues.push(
						"defensive probability is unverified or has zero success mass".to_owned(),
					);
				}
				LevelSolveReplayVariantOutcome::UnverifiedVictory {
					success_probability_millionths: report
						.defensive_probability
						.as_ref()
						.map(|audit| audit.success_probability_millionths),
					issues,
				}
			}
		}
		ExpectedReplayReportStatus::ExpectedDefeat => {
			let leaked_enemies: Vec<LevelSolveEnemyProgress> =
				replay_enemy_progress(report, |enemy| enemy.leaked_tick.is_some());
			let dead_operators: Vec<LevelSolveDeadOperatorProgress> = replay_dead_operators(report);
			if leaked_enemies.is_empty() {
				LevelSolveReplayVariantOutcome::ExpectedDefeat {
					terminal: report.terminal.clone(),
					remaining_enemies: replay_enemy_progress(report, |enemy| {
						enemy.remaining_hp_micros > 0
					}),
					dead_operators,
				}
			} else {
				LevelSolveReplayVariantOutcome::GoalLeak {
					terminal: report.terminal.clone(),
					leaked_enemies,
					dead_operators,
				}
			}
		}
		ExpectedReplayReportStatus::Incomplete => LevelSolveReplayVariantOutcome::Incomplete {
			terminal: report.terminal.clone(),
			issues: replay_failure_issues(report),
			pending_actions: replay_pending_actions(report),
			leaked_enemies: replay_enemy_progress(report, |enemy| enemy.leaked_tick.is_some()),
			dead_operators: replay_dead_operators(report),
			unsupported_count: report.unsupported.len(),
			unmodeled_count: report.unmodeled_omissions.len(),
			conservative_count: report.conservative_omissions.len(),
			approximated_count: exercised_approximation_count(report),
		},
		ExpectedReplayReportStatus::Unsupported => LevelSolveReplayVariantOutcome::Unsupported {
			issues: replay_failure_issues(report),
			unsupported_count: report.unsupported.len(),
			unmodeled_count: report.unmodeled_omissions.len(),
			conservative_count: report.conservative_omissions.len(),
			approximated_count: exercised_approximation_count(report),
		},
	}
}

fn observe_replay_variant_materialization_failure(
	context: &ReplayWorkerContext<'_>,
	work: &ReplayWork,
	variant_index: usize,
	variant_count: usize,
	duration: Duration,
	reason_code: impl Into<String>,
	detail: impl Into<String>,
) {
	observe_level_solve_progress(context.progress_observer, context.progress_started, || {
		LevelSolveProgressEvent::ReplayVariantFinished {
			proposal_index: work.proposal_index,
			replay_sequence: work.sequence,
			variant_index,
			variant_count,
			duration,
			outcome: LevelSolveReplayVariantOutcome::MaterializationFailed {
				reason_code: reason_code.into(),
				detail: detail.into(),
			},
		}
	});
}

fn execute_replay_work(
	context: &ReplayWorkerContext<'_>,
	work: ReplayWork,
) -> Result<ReplayAttempt, LevelSolvePrepareError> {
	let insertion_started: Instant = Instant::now();
	let selected_keys: Vec<ContributionCandidateKey> = work
		.candidate_keys
		.iter()
		.map(|candidate_key| {
			ContributionCandidateKey::new(candidate_key.clone()).map_err(|error| {
				LevelSolvePrepareError::new("candidate_identity_failed", error.to_string())
			})
		})
		.collect::<Result<_, _>>()?;
	observe_level_solve_progress(context.progress_observer, context.progress_started, || {
		LevelSolveProgressEvent::ReplayProposalStarted {
			proposal_index: work.proposal_index,
			replay_sequence: work.sequence,
			selected_candidates: candidate_progress_many(context.evaluator, &selected_keys),
		}
	});
	let mut best_completed_report: Option<ExpectedReplayAuditReport> = None;
	let mut best_success_probability: Option<f64> = None;
	let mut best_victorious_plan: Option<(
		JointInsertionPlan,
		Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure>,
	)> = None;
	let mut last_materialization_error: Option<String> = None;
	let plans: Vec<JointInsertionPlan> = match context.evaluator.insertion_plans(&selected_keys) {
		Ok(plans) => plans,
		Err(error) => {
			last_materialization_error = Some(error.detail.clone());
			observe_replay_variant_materialization_failure(
				context,
				&work,
				0,
				0,
				insertion_started.elapsed(),
				error.reason_code,
				error.detail,
			);
			Vec::new()
		}
	};
	let variant_count: usize = plans.len();
	if variant_count == 0 && last_materialization_error.is_none() {
		let detail: String = "no legal joint insertion order materialized".to_owned();
		last_materialization_error = Some(detail.clone());
		observe_replay_variant_materialization_failure(
			context,
			&work,
			0,
			0,
			insertion_started.elapsed(),
			"joint_insertion_schedule_not_materialized",
			detail,
		);
	}
	for (variant_offset, plan) in plans.into_iter().enumerate() {
		let variant_started: Instant = Instant::now();
		let variant_index: usize = variant_offset + 1;
		let guide_variant_id_result: Result<String, LevelSolvePrepareError> =
			joint_plan_variant_id(&plan);
		observe_level_solve_progress(context.progress_observer, context.progress_started, || {
			let (members, dp_order): (
				Vec<LevelSolveReplayMemberProgress>,
				Vec<LevelSolveReplayDpStepProgress>,
			) = replay_variant_plan_progress(context.evaluator, &plan);
			LevelSolveProgressEvent::ReplayVariantStarted {
				proposal_index: work.proposal_index,
				replay_sequence: work.sequence,
				variant_index,
				variant_count,
				guide_variant_id: guide_variant_id_result.as_ref().ok().cloned(),
				members,
				dp_order,
			}
		});
		let guide_variant_id: String = match guide_variant_id_result {
			Ok(guide_variant_id) => guide_variant_id,
			Err(error) => {
				last_materialization_error = Some(error.detail().to_owned());
				observe_replay_variant_materialization_failure(
					context,
					&work,
					variant_index,
					variant_count,
					variant_started.elapsed(),
					error.code(),
					error.detail(),
				);
				continue;
			}
		};
		let replay_members: Vec<CandidateReplayMemberPlan<'_>> =
			match candidate_replay_members(&context.evaluator.candidates, &plan) {
				Ok(members) => members,
				Err(error) => {
					last_materialization_error = Some(error.detail().to_owned());
					observe_replay_variant_materialization_failure(
						context,
						&work,
						variant_index,
						variant_count,
						variant_started.elapsed(),
						error.code(),
						error.detail(),
					);
					continue;
				}
			};
		let input = match materialize_candidate_expected_replay_input(
			context.snapshot,
			context.prefabs,
			context.source_seed,
			context.source_bindings,
			context.source_slot_name,
			CandidateReplayPlan {
				members: &replay_members,
			},
			context.tick_rate,
		) {
			Ok(input) => input,
			Err(error) => {
				last_materialization_error = Some(error.to_string());
				observe_replay_variant_materialization_failure(
					context,
					&work,
					variant_index,
					variant_count,
					variant_started.elapsed(),
					"candidate_replay_materialization_failed",
					error.to_string(),
				);
				continue;
			}
		};
		let output = execute_expected_replay(&input, context.guide_sha256, &guide_variant_id);
		observe_level_solve_progress(context.progress_observer, context.progress_started, || {
			LevelSolveProgressEvent::ReplayVariantFinished {
				proposal_index: work.proposal_index,
				replay_sequence: work.sequence,
				variant_index,
				variant_count,
				duration: variant_started.elapsed(),
				outcome: replay_variant_outcome(&output.report),
			}
		});
		let success_probability: Option<f64> = verified_expected_victory_probability(
			&output.report.status,
			output.report.defensive_probability.as_ref(),
		);
		let replace_report: bool = match (success_probability, best_success_probability) {
			(Some(candidate), Some(current)) => candidate > current,
			(Some(_), None) => true,
			(None, _) => best_completed_report.is_none(),
		};
		if replace_report {
			best_completed_report = Some(output.report);
			if let Some(success_probability) = success_probability {
				best_success_probability = Some(success_probability);
				let planning_evidence: Result<
					ExpectedDamageJointEvidence,
					ExpectedDamageJointEvaluationFailure,
				> = context.evaluator.score_plan(&plan);
				best_victorious_plan = Some((plan, planning_evidence));
			}
		}
	}
	let result: ExpectedDamageBattlefieldReplayResult = if let Some(report) = best_completed_report
	{
		ExpectedDamageBattlefieldReplayResult::Completed {
			report: Box::new(report),
		}
	} else {
		ExpectedDamageBattlefieldReplayResult::MaterializationFailed {
			reason_code: "candidate_replay_materialization_failed".to_owned(),
			detail: last_materialization_error
				.unwrap_or_else(|| "no legal joint insertion order could be replayed".to_owned()),
		}
	};
	let replay: ExpectedDamageBattlefieldReplayAudit = ExpectedDamageBattlefieldReplayAudit {
		proposal_index: work.proposal_index,
		candidate_keys: work.candidate_keys.clone(),
		result,
	};
	let Some((joint_plan, expected_damage)) = best_victorious_plan else {
		return Ok(ReplayAttempt {
			sequence: work.sequence,
			proposal_index: work.proposal_index,
			replay,
			expected_victory: false,
			job: None,
			job_failure: None,
		});
	};
	let expected_damage: ExpectedDamageJointEvidence = match expected_damage {
		Ok(expected_damage) => expected_damage,
		Err(error) => {
			observe_level_solve_progress(
				context.progress_observer,
				context.progress_started,
				|| LevelSolveProgressEvent::ReplayProposalExportFailed {
					proposal_index: work.proposal_index,
					replay_sequence: work.sequence,
					reason_code: "planning_evidence_unavailable".to_owned(),
					detail: format!("{}: {}", error.reason_code, error.detail),
				},
			);
			return Ok(ReplayAttempt {
				sequence: work.sequence,
				proposal_index: work.proposal_index,
				replay,
				expected_victory: true,
				job: None,
				job_failure: Some(ExpectedDamageJobMaterializationFailure {
					proposal_index: work.proposal_index,
					candidate_keys: work.candidate_keys,
					reason_code: "planning_evidence_unavailable".to_owned(),
					detail: format!("{}: {}", error.reason_code, error.detail),
				}),
			});
		}
	};
	let activation_is_exportable: bool = joint_plan.members.iter().all(|member| {
		context
			.evaluator
			.candidates
			.get(&member.candidate_key)
			.is_some_and(|candidate| {
				candidate.manual_activation
					&& !member.active_spans.is_empty()
					&& member.active_spans.first().map(|span| span.start)
						== member.earliest_legal_activation_tick
			})
	});
	if !activation_is_exportable {
		observe_level_solve_progress(context.progress_observer, context.progress_started, || {
			LevelSolveProgressEvent::ReplayProposalExportFailed {
				proposal_index: work.proposal_index,
				replay_sequence: work.sequence,
				reason_code: "activation_not_exportable".to_owned(),
				detail:
					"victorious replay activation is not exactly representable in an MAA job file"
						.to_owned(),
			}
		});
		return Ok(ReplayAttempt {
			sequence: work.sequence,
			proposal_index: work.proposal_index,
			replay,
			expected_victory: true,
			job: None,
			job_failure: Some(ExpectedDamageJobMaterializationFailure {
				proposal_index: work.proposal_index,
				candidate_keys: work.candidate_keys,
				reason_code: "activation_not_exportable".to_owned(),
				detail:
					"victorious replay activation is not exactly representable in an MAA job file"
						.to_owned(),
			}),
		});
	}
	let job: Result<ExpectedDamagePreparedJob, LevelSolvePrepareError> =
		prepare_damage_feasible_job(PreparedJobRequest {
			snapshot: context.snapshot,
			prefabs: context.prefabs,
			stage: context.stage,
			source_guide: context.source_guide,
			source_guide_json: context.source_guide_json,
			source_guide_source_id: context.source_guide_source_id,
			requested_level: context.requested_level,
			source_seed: context.source_seed,
			source_bindings: context.source_bindings,
			source_slot_name: context.source_slot_name,
			guide_selection_budget: context.guide_selection_budget,
			candidates: &context.evaluator.candidates,
			joint_plan: &joint_plan,
			expected_damage: &expected_damage,
			original_schedule: &context.evaluator.original_schedule,
			planning_model: &context.evaluator.model,
			planning_target_cohort_ids: &context.evaluator.target_cohort_ids,
			planning_duty_window: context.evaluator.duty_window,
			proposal_index: work.proposal_index,
			candidate_keys: &work.candidate_keys,
			tick_rate: context.tick_rate,
		});
	let (job, job_failure): (
		Option<ExpectedDamagePreparedJob>,
		Option<ExpectedDamageJobMaterializationFailure>,
	) = match job {
		Ok(job) => {
			observe_level_solve_progress(
				context.progress_observer,
				context.progress_started,
				|| LevelSolveProgressEvent::ReplayProposalMaterialized {
					proposal_index: work.proposal_index,
					replay_sequence: work.sequence,
					artifact_id: job.artifact_id.clone(),
				},
			);
			(Some(job), None)
		}
		Err(error) => {
			observe_level_solve_progress(
				context.progress_observer,
				context.progress_started,
				|| LevelSolveProgressEvent::ReplayProposalExportFailed {
					proposal_index: work.proposal_index,
					replay_sequence: work.sequence,
					reason_code: error.code().to_owned(),
					detail: error.detail().to_owned(),
				},
			);
			(
				None,
				Some(ExpectedDamageJobMaterializationFailure {
					proposal_index: work.proposal_index,
					candidate_keys: work.candidate_keys,
					reason_code: error.code().to_owned(),
					detail: error.detail().to_owned(),
				}),
			)
		}
	};
	Ok(ReplayAttempt {
		sequence: work.sequence,
		proposal_index: work.proposal_index,
		replay,
		expected_victory: true,
		job,
		job_failure,
	})
}

fn proposal_requires_replay(proposal: &ExpectedDamageProposalAudit) -> bool {
	matches!(
		proposal.disposition,
		ExpectedDamageProposalDisposition::DamageFeasibleCandidate
			| ExpectedDamageProposalDisposition::PlanningReplayCandidate { .. }
	)
}

fn truncate_solve_after_verified_top_k(
	solve: &mut ExpectedDamageLevelSolveReport,
	last_proposal_index: usize,
) {
	let retained_len: usize = last_proposal_index.saturating_add(1);
	solve.proposals.truncate(retained_len);
	solve
		.damage_feasible_proposal_indices
		.retain(|&proposal_index| proposal_index <= last_proposal_index);
	solve
		.replay_candidate_indices
		.retain(|&proposal_index| proposal_index <= last_proposal_index);
	// The producer may have independently reached its additive limit before the ordered consumer
	// observed the top-k result. Normalize the exposed, truncated stream so thread scheduling cannot
	// change the nested solve status.
	solve.status = ExpectedDamageLevelSolveStatus::ProposalVisitorStopped;
}

struct OrderedReplayCoordinator<T> {
	pending: BTreeMap<usize, T>,
	next_sequence: usize,
	verified_job_count: usize,
	requested_top_k: usize,
}

impl<T> OrderedReplayCoordinator<T> {
	fn new(requested_top_k: usize) -> Self {
		Self {
			pending: BTreeMap::new(),
			next_sequence: 0,
			verified_job_count: 0,
			requested_top_k,
		}
	}

	fn push(&mut self, sequence: usize, value: T) -> Result<Vec<T>, LevelSolvePrepareError> {
		if sequence < self.next_sequence || self.pending.contains_key(&sequence) {
			return Err(LevelSolvePrepareError::new(
				"duplicate_replay_sequence",
				format!("replay sequence {sequence} completed more than once"),
			));
		}
		self.pending.insert(sequence, value);
		let mut ready: Vec<T> = Vec::new();
		while let Some(value) = self.pending.remove(&self.next_sequence) {
			ready.push(value);
			self.next_sequence += 1;
		}
		Ok(ready)
	}

	fn record_verified_job(&mut self) {
		self.verified_job_count += 1;
	}

	fn top_k_filled(&self) -> bool {
		self.verified_job_count >= self.requested_top_k
	}
}

fn replay_inflight_credits(capacity: usize) -> (Sender<()>, Receiver<()>) {
	let (sender, receiver): (Sender<()>, Receiver<()>) = bounded(capacity);
	for _ in 0..capacity {
		sender
			.send(())
			.expect("new in-flight credit channel remains connected");
	}
	(sender, receiver)
}

fn acquire_replay_inflight_credit(receiver: &Receiver<()>, cancellation: &AtomicBool) -> bool {
	loop {
		if cancellation.load(Ordering::Acquire) {
			return false;
		}
		match receiver.recv_timeout(REPLAY_PIPELINE_CANCELLATION_POLL) {
			Ok(()) => return true,
			Err(RecvTimeoutError::Timeout) => {}
			Err(RecvTimeoutError::Disconnected) => return false,
		}
	}
}

fn send_replay_work(
	sender: &Sender<ReplayWork>,
	cancellation: &AtomicBool,
	mut work: ReplayWork,
) -> bool {
	loop {
		if cancellation.load(Ordering::Acquire) {
			return false;
		}
		match sender.send_timeout(work, REPLAY_PIPELINE_CANCELLATION_POLL) {
			Ok(()) => return true,
			Err(SendTimeoutError::Timeout(returned)) => work = returned,
			Err(SendTimeoutError::Disconnected(_)) => return false,
		}
	}
}

fn release_replay_inflight_credit(sender: &Sender<()>) -> Result<(), LevelSolvePrepareError> {
	match sender.try_send(()) {
		Ok(()) => Ok(()),
		Err(TrySendError::Full(())) => Err(LevelSolvePrepareError::new(
			"replay_inflight_credit_overflow",
			"ordered replay commit returned more in-flight credits than were acquired",
		)),
		Err(TrySendError::Disconnected(())) => Err(LevelSolvePrepareError::new(
			"replay_inflight_credit_disconnected",
			"in-flight credit receiver disconnected before replay search completed",
		)),
	}
}

fn run_verified_search_pipeline(
	solve_request: ExpectedDamageLevelSolveRequest<'_>,
	worker_context: &ReplayWorkerContext<'_>,
	replay_worker_count: usize,
	requested_top_k: usize,
) -> Result<VerifiedSearchPipelineResult, LevelSolvePrepareError> {
	let replay_queue_capacity: usize = replay_worker_count.checked_mul(2).ok_or_else(|| {
		LevelSolvePrepareError::new(
			"replay_queue_capacity_overflow",
			"twice the replay worker count does not fit usize",
		)
	})?;
	let replay_inflight_capacity: usize = replay_queue_capacity
		.checked_add(replay_worker_count)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"replay_inflight_capacity_overflow",
				"replay queue plus worker count does not fit usize",
			)
		})?;
	observe_level_solve_progress(
		worker_context.progress_observer,
		worker_context.progress_started,
		|| LevelSolveProgressEvent::SearchStarted {
			replay_worker_count,
			replay_queue_capacity,
			replay_inflight_capacity,
		},
	);
	let cancellation: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
	let proposals_generated: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
	let proposals_queued: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
	let (work_sender, work_receiver): (Sender<ReplayWork>, Receiver<ReplayWork>) =
		bounded(replay_queue_capacity);
	let (credit_sender, credit_receiver): (Sender<()>, Receiver<()>) =
		replay_inflight_credits(replay_inflight_capacity);
	let (result_sender, result_receiver): (
		Sender<ReplayWorkerMessage>,
		Receiver<ReplayWorkerMessage>,
	) = bounded(replay_inflight_capacity);
	let queue_monitor: Receiver<ReplayWork> = work_receiver.clone();

	thread::scope(
		|scope| -> Result<VerifiedSearchPipelineResult, LevelSolvePrepareError> {
			let producer_cancellation: Arc<AtomicBool> = Arc::clone(&cancellation);
			let producer_credit_receiver: Receiver<()> = credit_receiver.clone();
			let producer_proposals_generated: Arc<AtomicUsize> = Arc::clone(&proposals_generated);
			let producer_proposals_queued: Arc<AtomicUsize> = Arc::clone(&proposals_queued);
			let producer = scope.spawn(move || {
				let progress_joint_evaluator = ProgressJointExpectedDamageEvaluator {
					inner: worker_context.evaluator,
					observer: worker_context.progress_observer,
					progress_started: worker_context.progress_started,
					explore_cardinalities: solve_request.explore_cardinalities,
					planning_model_complete: solve_request.planning_model_complete,
					next_proposal_index: AtomicUsize::new(0),
					selection_started: Mutex::new(None),
				};
				let observed_solve_request = ExpectedDamageLevelSolveRequest {
					joint_evaluator: &progress_joint_evaluator,
					..solve_request
				};
				let mut replay_sequence: usize = 0;
				progress_joint_evaluator.start_selection(0);
				visit_expected_damage_level(
					observed_solve_request,
					|proposal_index: usize, proposal: &ExpectedDamageProposalAudit| {
						let replay_required: bool = proposal_requires_replay(proposal);
						let selected_candidate_keys: Vec<ContributionCandidateKey> = proposal
							.additive
							.selected_candidates
							.iter()
							.map(|candidate| {
								ContributionCandidateKey::new(candidate.candidate_key.clone())
									.expect(
										"generated proposal candidate keys were already validated",
									)
							})
							.collect();
						producer_proposals_generated.store(proposal_index + 1, Ordering::Release);
						observe_level_solve_progress(
							worker_context.progress_observer,
							worker_context.progress_started,
							|| LevelSolveProgressEvent::ProposalGenerated {
								proposal_index,
								selected_candidates: candidate_progress_many(
									worker_context.evaluator,
									&selected_candidate_keys,
								),
								total_deployment_cost: proposal.additive.total_deployment_cost,
								replay_required,
								proposal_limit: observed_solve_request.additive_proposal_limit,
							},
						);
						if !replay_required {
							if proposal_index + 1 < observed_solve_request.additive_proposal_limit {
								progress_joint_evaluator.start_selection(proposal_index + 1);
							}
							return ControlFlow::Continue(());
						}
						let candidate_keys: Vec<String> = selected_candidate_keys
							.iter()
							.map(|candidate_key| candidate_key.as_str().to_owned())
							.collect();
						let work: ReplayWork = ReplayWork {
							sequence: replay_sequence,
							proposal_index,
							candidate_keys,
						};
						if !acquire_replay_inflight_credit(
							&producer_credit_receiver,
							&producer_cancellation,
						) || !send_replay_work(&work_sender, &producer_cancellation, work)
						{
							return ControlFlow::Break(());
						}
						producer_proposals_queued.store(replay_sequence + 1, Ordering::Release);
						observe_level_solve_progress(
							worker_context.progress_observer,
							worker_context.progress_started,
							|| LevelSolveProgressEvent::ProposalQueued {
								proposal_index,
								replay_sequence,
								queue_depth: work_sender.len(),
								in_flight: replay_inflight_capacity
									.saturating_sub(producer_credit_receiver.len()),
							},
						);
						replay_sequence += 1;
						if proposal_index + 1 < observed_solve_request.additive_proposal_limit {
							progress_joint_evaluator.start_selection(proposal_index + 1);
						}
						ControlFlow::Continue(())
					},
				)
			});

			let mut workers = Vec::with_capacity(replay_worker_count);
			for _worker_index in 0..replay_worker_count {
				let worker_receiver: Receiver<ReplayWork> = work_receiver.clone();
				let worker_sender: Sender<ReplayWorkerMessage> = result_sender.clone();
				let worker_cancellation: Arc<AtomicBool> = Arc::clone(&cancellation);
				workers.push(scope.spawn(move || {
					while let Ok(work) = worker_receiver.recv() {
						if worker_cancellation.load(Ordering::Acquire) {
							break;
						}
						let sequence: usize = work.sequence;
						let proposal_index: usize = work.proposal_index;
						let message: ReplayWorkerMessage =
							complete_replay_work(sequence, proposal_index, Instant::now(), || {
								execute_replay_work(worker_context, work)
							});
						if worker_sender.send(message).is_err() {
							break;
						}
					}
				}));
			}
			drop(work_receiver);
			drop(result_sender);

			let mut coordinator: OrderedReplayCoordinator<
				Result<ReplayAttempt, LevelSolvePrepareError>,
			> = OrderedReplayCoordinator::new(requested_top_k);
			let mut battlefield_verified_jobs: Vec<ExpectedDamagePreparedJob> = Vec::new();
			let mut job_materialization_failures: Vec<ExpectedDamageJobMaterializationFailure> =
				Vec::new();
			let mut battlefield_replays: Vec<ExpectedDamageBattlefieldReplayAudit> = Vec::new();
			let mut battlefield_expected_victory_proposal_indices: Vec<usize> = Vec::new();
			let mut last_committed_proposal_index: Option<usize> = None;
			let mut pipeline_error: Option<LevelSolvePrepareError> = None;
			let mut replay_completed: usize = 0;

			while let Ok(message) = result_receiver.recv() {
				let completed_sequence: usize = message.sequence;
				let completed_proposal_index: usize = message.proposal_index;
				let completed_duration: Duration = message.duration;
				let ready = match coordinator.push(message.sequence, message.result) {
					Ok(ready) => ready,
					Err(error) => {
						pipeline_error = Some(error);
						cancellation.store(true, Ordering::Release);
						break;
					}
				};
				for result in ready {
					let attempt: ReplayAttempt = match result {
						Ok(attempt) => attempt,
						Err(error) => {
							pipeline_error = Some(error);
							cancellation.store(true, Ordering::Release);
							break;
						}
					};
					debug_assert_eq!(attempt.sequence, battlefield_replays.len());
					last_committed_proposal_index = Some(attempt.proposal_index);
					battlefield_replays.push(attempt.replay);
					if attempt.expected_victory {
						battlefield_expected_victory_proposal_indices.push(attempt.proposal_index);
					}
					if let Some(failure) = attempt.job_failure {
						job_materialization_failures.push(failure);
					}
					if let Some(job) = attempt.job {
						observe_level_solve_progress(
							worker_context.progress_observer,
							worker_context.progress_started,
							|| LevelSolveProgressEvent::ReplayProposalAccepted {
								proposal_index: attempt.proposal_index,
								replay_sequence: attempt.sequence,
								artifact_id: job.artifact_id.clone(),
							},
						);
						battlefield_verified_jobs.push(job);
						coordinator.record_verified_job();
					}
					if coordinator.top_k_filled() {
						cancellation.store(true, Ordering::Release);
						break;
					}
					if let Err(error) = release_replay_inflight_credit(&credit_sender) {
						pipeline_error = Some(error);
						cancellation.store(true, Ordering::Release);
						break;
					}
				}
				replay_completed += 1;
				observe_level_solve_progress(
					worker_context.progress_observer,
					worker_context.progress_started,
					|| LevelSolveProgressEvent::ReplayCompleted {
						proposal_index: completed_proposal_index,
						replay_sequence: completed_sequence,
						replay_completed,
						verified_jobs: battlefield_verified_jobs.len(),
						queue_depth: queue_monitor.len(),
						in_flight: replay_inflight_capacity.saturating_sub(credit_sender.len()),
						duration: completed_duration,
					},
				);
				if pipeline_error.is_some() || coordinator.top_k_filled() {
					break;
				}
			}
			cancellation.store(true, Ordering::Release);
			drop(result_receiver);

			let mut solve: ExpectedDamageLevelSolveReport = producer
				.join()
				.map_err(|_| {
					LevelSolvePrepareError::new(
						"proposal_producer_panicked",
						"proposal producer thread panicked",
					)
				})?
				.map_err(|error| {
					LevelSolvePrepareError::new("expected_damage_solve_failed", error.to_string())
				})?;
			for worker in workers {
				worker.join().map_err(|_| {
					LevelSolvePrepareError::new(
						"replay_worker_panicked",
						"a replay worker thread panicked",
					)
				})?;
			}
			if let Some(error) = pipeline_error {
				return Err(error);
			}
			if coordinator.top_k_filled() {
				if let Some(last_proposal_index) = last_committed_proposal_index {
					truncate_solve_after_verified_top_k(&mut solve, last_proposal_index);
				}
			}
			observe_level_solve_progress(
				worker_context.progress_observer,
				worker_context.progress_started,
				|| LevelSolveProgressEvent::SearchFinished {
					proposals_generated: proposals_generated.load(Ordering::Acquire),
					proposals_queued: proposals_queued.load(Ordering::Acquire),
					replay_completed,
					verified_jobs: battlefield_verified_jobs.len(),
				},
			);
			Ok(VerifiedSearchPipelineResult {
				solve,
				battlefield_verified_jobs,
				job_materialization_failures,
				battlefield_replays,
				battlefield_expected_victory_proposal_indices,
				replay_queue_capacity,
			})
		},
	)
}

const MAX_JOINT_INSERTION_PERMUTATIONS: usize = 40_320;
const MAX_BOUNDED_JOINT_INSERTION_ORDERS: usize = 64;

fn expected_damage_evidence_is_better(
	candidate: &ExpectedDamageJointEvidence,
	current: &ExpectedDamageJointEvidence,
) -> bool {
	candidate
		.deterministic_expected_damage_feasible
		.cmp(&current.deterministic_expected_damage_feasible)
		.then_with(|| {
			current
				.total_due_remaining_hp_scaled
				.cmp(&candidate.total_due_remaining_hp_scaled)
		})
		.then_with(|| {
			candidate
				.minimum_cumulative_coverage_millionths
				.cmp(&current.minimum_cumulative_coverage_millionths)
		})
		.is_gt()
}

fn enumerate_joint_insertion_plans(
	original_schedule: &PreparedGuideDeploymentSchedule,
	candidates: &BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	selected_candidate_keys: &[ContributionCandidateKey],
) -> Result<Vec<JointInsertionPlan>, ExpectedDamageJointEvaluationFailure> {
	if selected_candidate_keys.is_empty() {
		return Err(ExpectedDamageJointEvaluationFailure::new(
			"empty_joint_insertion",
			"joint replacement proposal selected no candidates",
		));
	}
	let mut unique: BTreeSet<&ContributionCandidateKey> = BTreeSet::new();
	for candidate_key in selected_candidate_keys {
		if !unique.insert(candidate_key) {
			return Err(ExpectedDamageJointEvaluationFailure::new(
				"duplicate_joint_candidate",
				format!("joint proposal repeats candidate {candidate_key}"),
			));
		}
		if !candidates.contains_key(candidate_key) {
			return Err(ExpectedDamageJointEvaluationFailure::new(
				"missing_materialized_candidate",
				format!("candidate {candidate_key} is absent from the adapter"),
			));
		}
	}
	let permutation_count: usize = (2..=selected_candidate_keys.len())
		.try_fold(1_usize, |count, factor| count.checked_mul(factor))
		.unwrap_or(usize::MAX);
	let mut sorted: Vec<ContributionCandidateKey> = selected_candidate_keys.to_vec();
	sorted.sort();
	let permutations: Vec<Vec<ContributionCandidateKey>> =
		if permutation_count <= MAX_JOINT_INSERTION_PERMUTATIONS {
			let mut permutations: Vec<Vec<ContributionCandidateKey>> =
				Vec::with_capacity(permutation_count);
			collect_permutations(&mut sorted, 0, &mut permutations);
			permutations
		} else {
			bounded_joint_insertion_orders(candidates, &sorted)
		};
	let mut plans: Vec<JointInsertionPlan> = Vec::with_capacity(permutations.len());
	let mut last_error: Option<ExpectedDamageJointEvaluationFailure> = None;
	for order in permutations {
		match materialize_joint_insertion_plan(original_schedule, candidates, &order) {
			Ok(plan) => plans.push(plan),
			Err(error) => last_error = Some(error),
		}
	}
	if plans.is_empty() {
		return Err(last_error.unwrap_or_else(|| {
			ExpectedDamageJointEvaluationFailure::new(
				"joint_insertion_schedule_not_materialized",
				"no deployment order produced a legal shared-DP schedule",
			)
		}));
	}
	Ok(plans)
}

fn bounded_joint_insertion_orders(
	candidates: &BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	sorted_keys: &[ContributionCandidateKey],
) -> Vec<Vec<ContributionCandidateKey>> {
	let mut cost_order: Vec<ContributionCandidateKey> = sorted_keys.to_vec();
	cost_order.sort_by(|left, right| {
		candidates[left]
			.candidate
			.deployment_cost()
			.cmp(&candidates[right].candidate.deployment_cost())
			.then_with(|| left.cmp(right))
	});
	let mut baseline_tick_order: Vec<ContributionCandidateKey> = sorted_keys.to_vec();
	baseline_tick_order.sort_by(|left, right| {
		candidates[left]
			.deploy_tick
			.cmp(&candidates[right].deploy_tick)
			.then_with(|| left.cmp(right))
	});
	let base_orders: [Vec<ContributionCandidateKey>; 3] =
		[sorted_keys.to_vec(), cost_order, baseline_tick_order];
	bounded_rotated_orders(&base_orders)
}

fn bounded_rotated_orders(
	base_orders: &[Vec<ContributionCandidateKey>],
) -> Vec<Vec<ContributionCandidateKey>> {
	let order_length: usize = base_orders.first().map(Vec::len).unwrap_or_default();
	debug_assert!(base_orders.iter().all(|order| order.len() == order_length));
	let mut seen: BTreeSet<Vec<ContributionCandidateKey>> = BTreeSet::new();
	let mut orders: Vec<Vec<ContributionCandidateKey>> = Vec::new();
	for offset in 0..order_length {
		for base in base_orders {
			let mut rotated: Vec<ContributionCandidateKey> = base.clone();
			rotated.rotate_left(offset);
			push_unique_bounded_order(&mut orders, &mut seen, rotated.clone());
			rotated.reverse();
			push_unique_bounded_order(&mut orders, &mut seen, rotated);
			if orders.len() == MAX_BOUNDED_JOINT_INSERTION_ORDERS {
				return orders;
			}
		}
	}
	orders
}

fn push_unique_bounded_order(
	orders: &mut Vec<Vec<ContributionCandidateKey>>,
	seen: &mut BTreeSet<Vec<ContributionCandidateKey>>,
	order: Vec<ContributionCandidateKey>,
) {
	if orders.len() < MAX_BOUNDED_JOINT_INSERTION_ORDERS && seen.insert(order.clone()) {
		orders.push(order);
	}
}

fn collect_permutations(
	values: &mut [ContributionCandidateKey],
	start: usize,
	output: &mut Vec<Vec<ContributionCandidateKey>>,
) {
	if start == values.len() {
		output.push(values.to_vec());
		return;
	}
	for index in start..values.len() {
		values.swap(start, index);
		collect_permutations(values, start + 1, output);
		values.swap(start, index);
	}
}

fn materialize_joint_insertion_plan(
	original_schedule: &PreparedGuideDeploymentSchedule,
	candidates: &BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	order: &[ContributionCandidateKey],
) -> Result<JointInsertionPlan, ExpectedDamageJointEvaluationFailure> {
	let source_count: usize = original_schedule
		.deployments
		.iter()
		.filter(|deployment| deployment.action.is_source_slot)
		.count();
	if source_count != 1 {
		return Err(ExpectedDamageJointEvaluationFailure::new(
			"source_deployment_count_invalid",
			format!("expected one source deployment, found {source_count}"),
		));
	}
	let mut actions: Vec<OrderedDeploymentDpAction> = Vec::with_capacity(
		original_schedule
			.deployments
			.len()
			.checked_sub(1)
			.and_then(|count| count.checked_add(order.len()))
			.ok_or_else(|| {
				ExpectedDamageJointEvaluationFailure::new(
					"joint_insertion_count_overflow",
					"joint deployment action count overflowed",
				)
			})?,
	);
	let mut candidate_action_keys: BTreeMap<ContributionCandidateKey, String> = BTreeMap::new();
	for deployment in &original_schedule.deployments {
		if !deployment.action.is_source_slot {
			actions.push(deployment.dp_schedule.action.clone());
			continue;
		}
		let source_action: &OrderedDeploymentDpAction = &deployment.dp_schedule.action;
		for (member_index, candidate_key) in order.iter().enumerate() {
			let candidate = &candidates[candidate_key];
			let stable_key: String = format!("replacement:{candidate_key}");
			let earliest_tick: Option<u64> = if member_index == 0 {
				source_action.earliest_tick
			} else {
				None
			};
			let completion_delay_ticks: u64 = if member_index == 0 {
				source_action.completion_delay_ticks
			} else {
				0
			};
			// A replacement brings no planned grants: no reviewed damage skill is also a
			// reviewed deployment-point granter, and the source operator it displaces takes its
			// own grants with it. If a replacement ever does grant, replay diverges from this
			// schedule and the export's deploy-tick equality rejects it.
			let mut action = OrderedDeploymentDpAction::new(
				stable_key.clone(),
				u64::from(candidate.candidate.deployment_cost()),
				earliest_tick,
				completion_delay_ticks,
			);
			if member_index == 0 {
				action = action
					.with_minimum_dp_before_deployment(source_action.minimum_dp_before_deployment);
			}
			candidate_action_keys.insert(candidate_key.clone(), stable_key);
			actions.push(action);
		}
	}
	let dp_schedule: Vec<ScheduledDeploymentDpAction> =
		schedule_ordered_deployments(original_schedule.dp_rules, &actions).map_err(|error| {
			ExpectedDamageJointEvaluationFailure::new("joint_dp_schedule_failed", error.to_string())
		})?;
	let scheduled_by_key: BTreeMap<&str, &ScheduledDeploymentDpAction> = dp_schedule
		.iter()
		.map(|scheduled| (scheduled.action.stable_key.as_str(), scheduled))
		.collect();
	let mut retained_deploy_ticks: BTreeMap<usize, u64> = BTreeMap::new();
	for deployment in &original_schedule.deployments {
		if deployment.action.is_source_slot {
			continue;
		}
		let stable_key: &str = deployment.dp_schedule.action.stable_key.as_str();
		let scheduled = scheduled_by_key.get(stable_key).ok_or_else(|| {
			ExpectedDamageJointEvaluationFailure::new(
				"retained_dp_schedule_missing",
				format!("retained action {stable_key:?} is absent from joint schedule"),
			)
		})?;
		retained_deploy_ticks.insert(deployment.action.action_index, scheduled.deploy_tick);
	}
	let mut members: Vec<JointInsertionMemberTiming> = Vec::with_capacity(order.len());
	for candidate_key in order {
		let candidate = &candidates[candidate_key];
		let stable_key = &candidate_action_keys[candidate_key];
		let deploy_tick: u64 = scheduled_by_key
			.get(stable_key.as_str())
			.expect("candidate action key came from scheduled input")
			.deploy_tick;
		let deploy_delay: u64 =
			deploy_tick
				.checked_sub(candidate.deploy_tick)
				.ok_or_else(|| {
					ExpectedDamageJointEvaluationFailure::new(
						"candidate_deploy_tick_moved_earlier",
						format!(
					"candidate {candidate_key} moved from baseline tick {} to {deploy_tick}",
					candidate.deploy_tick
				),
					)
				})?;
		let active_spans: Vec<TickSpan> =
			shift_tick_spans(candidate_key, &candidate.active_spans, deploy_delay)?;
		let earliest_legal_activation_tick: Option<u64> = shift_optional_tick(
			candidate_key,
			candidate.earliest_legal_activation_tick,
			deploy_delay,
		)?;
		members.push(JointInsertionMemberTiming {
			candidate_key: candidate_key.clone(),
			deploy_tick,
			earliest_legal_activation_tick,
			active_spans,
		});
	}
	Ok(JointInsertionPlan {
		members,
		retained_deploy_ticks,
		dp_schedule,
	})
}

fn shift_tick_spans(
	candidate_key: &ContributionCandidateKey,
	spans: &[TickSpan],
	delay: u64,
) -> Result<Vec<TickSpan>, ExpectedDamageJointEvaluationFailure> {
	spans
		.iter()
		.map(|span| {
			Ok(TickSpan {
				start: span.start.checked_add(delay).ok_or_else(|| {
					ExpectedDamageJointEvaluationFailure::new(
						"candidate_activation_tick_overflow",
						format!("candidate {candidate_key} activation start overflowed"),
					)
				})?,
				end: span.end.checked_add(delay).ok_or_else(|| {
					ExpectedDamageJointEvaluationFailure::new(
						"candidate_activation_tick_overflow",
						format!("candidate {candidate_key} activation end overflowed"),
					)
				})?,
			})
		})
		.collect()
}

fn shift_optional_tick(
	candidate_key: &ContributionCandidateKey,
	tick: Option<u64>,
	delay: u64,
) -> Result<Option<u64>, ExpectedDamageJointEvaluationFailure> {
	tick.map(|tick| {
		tick.checked_add(delay).ok_or_else(|| {
			ExpectedDamageJointEvaluationFailure::new(
				"candidate_activation_tick_overflow",
				format!("candidate {candidate_key} earliest activation overflowed"),
			)
		})
	})
	.transpose()
}

fn candidate_replay_members<'a>(
	candidates: &'a BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	plan: &'a JointInsertionPlan,
) -> Result<Vec<CandidateReplayMemberPlan<'a>>, LevelSolvePrepareError> {
	plan.members
		.iter()
		.map(|member| {
			let candidate = candidates.get(&member.candidate_key).ok_or_else(|| {
				LevelSolvePrepareError::new(
					"candidate_materialization_missing",
					format!(
						"candidate {} is absent from the replay registry",
						member.candidate_key
					),
				)
			})?;
			Ok(CandidateReplayMemberPlan {
				candidate_key: member.candidate_key.as_str(),
				build: &candidate.exact_build,
				skill_active_spans: &member.active_spans,
				placement: CandidateReplayPlacement {
					cell: candidate.candidate.deployment_cell(),
					facing: candidate.candidate.facing(),
					requested_at: crate::simulation::SimTick::new(member.deploy_tick),
				},
			})
		})
		.collect()
}

/// Materializes and solves one unambiguous source-guide context.
fn prepare_expected_damage_level_solve(
	request: LevelSolvePrepareRequest<'_>,
) -> Result<ExpectedDamagePreparedLevelReport, LevelSolvePrepareError> {
	prepare_expected_damage_level_solve_observed(request, None, Instant::now())
}

/// Decodes raw workflow inputs and runs a complete expected-damage solve.
pub fn solve(
	request: SolveRequest<'_>,
) -> Result<ExpectedDamagePreparedLevelReport, LevelSolvePrepareError> {
	solve_inner(request, None)
}

/// Runs a complete solve while reporting workflow progress to the caller-owned renderer.
pub fn solve_with_observer(
	request: SolveRequest<'_>,
	observer: &dyn LevelSolveProgressObserver,
) -> Result<ExpectedDamagePreparedLevelReport, LevelSolvePrepareError> {
	solve_inner(request, Some(observer))
}

fn solve_inner(
	request: SolveRequest<'_>,
	observer: Option<&dyn LevelSolveProgressObserver>,
) -> Result<ExpectedDamagePreparedLevelReport, LevelSolvePrepareError> {
	let guide_json: &str = std::str::from_utf8(request.guide_bytes).map_err(|error| {
		LevelSolvePrepareError::new("guide_not_utf8", format!("guide is not UTF-8: {error}"))
	})?;
	let roster: Option<LevelSolveRoster> = request
		.roster_json
		.map(|bytes| {
			let roster: LevelSolveRoster = serde_json::from_slice(bytes).map_err(|error| {
				LevelSolvePrepareError::new(
					"roster_decode_failed",
					format!("failed to decode level-solve roster: {error}"),
				)
			})?;
			roster.validate().map_err(|error| {
				LevelSolvePrepareError::new("invalid_roster", error.to_string())
			})?;
			Ok(roster)
		})
		.transpose()?;
	let top_k: usize = request.top_k.unwrap_or(DEFAULT_LEVEL_SOLVE_TOP_K);
	let guide_content_sha256: String = hex_digest(&Sha256::digest(request.guide_bytes).into());
	let prepared_request = LevelSolvePrepareRequest {
		snapshot: request.snapshot,
		prefabs: request.prefabs,
		requested_level: request.requested_level,
		guide_json,
		guide_source_id: request.guide_source_id,
		guide_content_sha256: &guide_content_sha256,
		difficulty: request.difficulty,
		roster: roster.as_ref(),
		excluded_operators: request.excluded_operators,
		top_k,
		additive_proposal_limit: request.additive_proposal_limit,
		replay_worker_count: request.replay_worker_count,
		guide_selection_budget: request.guide_selection_budget,
	};
	match observer {
		Some(observer) => {
			prepare_expected_damage_level_solve_with_observer(prepared_request, observer)
		}
		None => prepare_expected_damage_level_solve(prepared_request),
	}
}

/// Materializes and solves one source-guide context while emitting typed progress updates.
fn prepare_expected_damage_level_solve_with_observer(
	request: LevelSolvePrepareRequest<'_>,
	observer: &dyn LevelSolveProgressObserver,
) -> Result<ExpectedDamagePreparedLevelReport, LevelSolvePrepareError> {
	prepare_expected_damage_level_solve_observed(request, Some(observer), Instant::now())
}

fn prepare_expected_damage_level_solve_observed(
	request: LevelSolvePrepareRequest<'_>,
	progress_observer: Option<&dyn LevelSolveProgressObserver>,
	progress_started: Instant,
) -> Result<ExpectedDamagePreparedLevelReport, LevelSolvePrepareError> {
	observe_level_solve_progress(progress_observer, progress_started, || {
		LevelSolveProgressEvent::PreparationStarted {
			proposal_limit: request.additive_proposal_limit,
			requested_top_k: request.top_k,
			ticks_per_second: LEVEL_SOLVE_TICKS_PER_SECOND,
		}
	});
	request
		.prefabs
		.ensure_compatible_with(request.snapshot)
		.map_err(|error| {
			LevelSolvePrepareError::new("battle_prefab_revision_mismatch", error.to_string())
		})?;
	if let Some(roster) = request.roster {
		roster
			.validate()
			.map_err(|error| LevelSolvePrepareError::new("invalid_roster", error.to_string()))?;
	}
	if request.guide_selection_budget == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_guide_selection_budget",
			"guide selection budget must be positive",
		));
	}
	if request.top_k == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_top_k",
			"verified job top_k must be positive",
		));
	}
	if request.additive_proposal_limit == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_additive_proposal_limit",
			"additive proposal limit must be positive",
		));
	}
	if request.replay_worker_count == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_replay_worker_count",
			"replay worker count must be positive",
		));
	}
	validate_content_digest(
		"guide",
		request.guide_json.as_bytes(),
		request.guide_content_sha256,
	)?;

	let derived_source: DerivedSourceGuide = derive_source_guide(SourceGuideDerivationRequest {
		snapshot: request.snapshot,
		requested_level: request.requested_level,
		guide_json: request.guide_json,
		guide_source_id: request.guide_source_id,
		guide_content_sha256: request.guide_content_sha256,
		difficulty: request.difficulty,
		selection_budget: request.guide_selection_budget,
		purpose: SourceGuideDerivationPurpose::Solve,
	})?;
	let guide: MaaJob = derived_source.guide;
	let replay_seed: MaaJobSeed = derived_source.seed;
	let source_provenance: SourceGuideProvenance = derived_source.provenance;
	let stage: CompiledStage = derived_source.stage;
	let domain: GuideDeploymentDomain = derived_source.domain;
	let replay_bindings: Vec<ExactReplayBuildBinding> =
		guide_no_module_replay_bindings(request.snapshot, &replay_seed)?;

	let tick_rate: TickRate = TickRate::new(
		NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("level-solve tick rate is nonzero"),
	);
	let guide_action_overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(request.snapshot, request.prefabs, &domain, tick_rate)?;
	let original_schedule: PreparedGuideDeploymentSchedule =
		prepare_guide_deployment_schedule_with_cost_overrides(
			request.snapshot,
			NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND)
				.expect("level-solve tick rate is nonzero"),
			&domain,
			&guide_action_overrides,
		)
		.map_err(|error| {
			LevelSolvePrepareError::new(
				"retained_schedule_materialization_failed",
				error.to_string(),
			)
		})?;
	let source_deployment: PreparedGuideDeployment = original_schedule
		.deployments
		.iter()
		.find(|deployment| deployment.action.is_source_slot)
		.cloned()
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"source_deployment_missing",
				"original guide schedule contains no source-slot action",
			)
		})?;
	let source_deploy_tick: u64 = source_deployment.deploy_tick();
	let retained_only_input = materialize_source_removed_expected_replay_input(
		request.snapshot,
		request.prefabs,
		&replay_seed,
		&replay_bindings,
		&source_deployment.action.operator_name,
		tick_rate,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("full_battle_gap_materialization_failed", error.to_string())
	})?;
	let full_battle_damage_gap: FullBattleDamageGapLedger =
		derive_full_battle_damage_gap_ledger(&retained_only_input).map_err(|error| {
			LevelSolvePrepareError::new("full_battle_gap_derivation_failed", error.to_string())
		})?;
	let full_battle_planning: FullBattlePlanningContext =
		prepare_full_battle_unblocked_planning_context(&stage, tick_rate, &full_battle_damage_gap)
			.map_err(|error| {
				LevelSolvePrepareError::new(
					"full_battle_planning_materialization_failed",
					error.to_string(),
				)
			})?;
	let demands: Vec<ContributionDemand> =
		wave_local_hp_group_demands(&full_battle_planning.wave_local_hp_groups)?;
	let retained_ranking_groups: Vec<RetainedWaveLocalHpGroupBatch> =
		project_retained_wave_local_hp_groups(
			&full_battle_planning.model,
			&full_battle_planning.wave_local_hp_groups,
		)?;
	let target_cohort_ids: BTreeSet<String> = full_battle_planning.target_cohort_ids.clone();
	let planning_duty_window: TickSpan = full_battle_planning.duty_window;
	let roster_entries: Vec<LevelSolveRosterEntry> = roster_entries(request.roster);
	let mut exclusions: Vec<ExpectedDamageCandidateExclusion> = Vec::new();
	let mut resolved_builds: Vec<(ResolvedOperatorBuild, ExpectedDamageSkillProfile)> = Vec::new();
	let mut seen_resolved: BTreeSet<(String, u8)> = BTreeSet::new();
	for entry in roster_entries {
		let (build, module_variants): (ResolvedOperatorBuild, Vec<String>) =
			match resolve_no_module_max_build(request.snapshot, &entry.operator, entry.skill) {
				Ok(value) => value,
				Err(error) => {
					exclusions.push(exclusion(
						None,
						Some(entry.operator),
						"build_resolution_failed",
						error.to_string(),
					));
					continue;
				}
			};
		if !seen_resolved.insert((build.operator_id().to_owned(), entry.skill)) {
			continue;
		}
		for module in module_variants {
			exclusions.push(exclusion(
				None,
				Some(build.operator_id().to_owned()),
				"module_variant_excluded_v1",
				format!(
					"module variant {module} excluded; v1 policy resolves the same max build \
					 with module=None"
				),
			));
		}
		if build.operator_id() == source_deployment.resolved_build.operator_id()
			|| build.name() == source_deployment.resolved_build.name()
			|| request.excluded_operators.contains(build.operator_id())
			|| request.excluded_operators.contains(build.name())
			|| request.excluded_operators.contains(&entry.operator)
		{
			exclusions.push(exclusion(
				None,
				Some(build.operator_id().to_owned()),
				"operator_excluded",
				"source operator or caller exclusion".to_owned(),
			));
			continue;
		}
		let profile: ExpectedDamageSkillProfile =
			match compile_expected_damage_skill(&build, tick_rate) {
				ExpectedDamageSkillEntry::Supported(profile) => *profile,
				ExpectedDamageSkillEntry::Unsupported(unsupported) => {
					exclusions.push(exclusion(
						None,
						Some(build.operator_id().to_owned()),
						"unsupported_damage_skill",
						unsupported
							.reasons()
							.iter()
							.map(ToString::to_string)
							.collect::<Vec<_>>()
							.join("; "),
					));
					continue;
				}
			};
		resolved_builds.push((build, profile));
	}
	resolved_builds.sort_by(|(left, _), (right, _)| left.stable_id().cmp(right.stable_id()));

	let mut schedules_by_cost: BTreeMap<u32, PreparedGuideDeploymentSchedule> = BTreeMap::new();
	for (build, _) in &resolved_builds {
		if schedules_by_cost.contains_key(&build.deployment_cost()) {
			continue;
		}
		let mut candidate_overrides: GuideDeploymentActionOverrides =
			guide_action_overrides.clone();
		candidate_overrides
			.deployment_cost_by_action
			.insert(domain.source_action_index, build.deployment_cost());
		let schedule: PreparedGuideDeploymentSchedule =
			prepare_guide_deployment_schedule_with_cost_overrides(
				request.snapshot,
				NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND)
					.expect("level-solve tick rate is nonzero"),
				&domain,
				&candidate_overrides,
			)
			.map_err(|error| {
				LevelSolvePrepareError::new(
					"candidate_dp_schedule_failed",
					format!("cost {}: {error}", build.deployment_cost()),
				)
			})?;
		schedules_by_cost.insert(build.deployment_cost(), schedule);
	}

	let mut materialized_candidates: BTreeMap<
		ContributionCandidateKey,
		MaterializedSkillCandidate,
	> = BTreeMap::new();
	let mut independent_candidates: Vec<IndependentContributionCandidate> = Vec::new();
	let mut budget_exhausted: bool = false;
	'builds: for (build, profile) in resolved_builds {
		let base_attack: NormalAttackProfile =
			match attack_phase_normal_attack(&profile, profile.base(), false) {
				Ok(value) => value,
				Err(error) => {
					exclusions.push(exclusion(
						None,
						Some(build.operator_id().to_owned()),
						error.code(),
						error.detail().to_owned(),
					));
					continue;
				}
			};
		let active_attack: NormalAttackProfile =
			match attack_phase_normal_attack(&profile, profile.active(), true) {
				Ok(value) => value,
				Err(error) => {
					exclusions.push(exclusion(
						None,
						Some(build.operator_id().to_owned()),
						error.code(),
						error.detail().to_owned(),
					));
					continue;
				}
			};
		let base_range = request
			.snapshot
			.resolve_range(build.range_id())
			.map_err(|error| {
				LevelSolvePrepareError::new(
					"base_range_resolution_failed",
					format!("{}: {error}", build.range_id()),
				)
			})?;
		let skill_range = request
			.snapshot
			.resolve_range(profile.range().range_id())
			.map_err(|error| {
				LevelSolvePrepareError::new(
					"skill_range_resolution_failed",
					format!("{}: {error}", profile.range().range_id()),
				)
			})?;
		let schedule: &PreparedGuideDeploymentSchedule =
			&schedules_by_cost[&build.deployment_cost()];
		let deploy_tick: u64 = schedule
			.deployments
			.iter()
			.find(|deployment| deployment.action.is_source_slot)
			.map(|deployment| deployment.deploy_tick())
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"source_deployment_missing",
					"candidate DP schedule contains no source-slot action",
				)
			})?;
		for stage_cell in stage.cells() {
			let deployment_cell: GridPosition = stage_cell.position();
			if !deployment_allowed(build.position(), stage_cell.buildable()) {
				exclusions.push(exclusion(
					None,
					Some(build.operator_id().to_owned()),
					"incompatible_deployment_cell",
					format!(
						"position {:?} cannot deploy on {:?} at ({}, {})",
						build.position(),
						stage_cell.buildable(),
						deployment_cell.row,
						deployment_cell.col
					),
				));
				continue;
			}
			for facing in [Facing::Up, Facing::Right, Facing::Down, Facing::Left] {
				let base_attack_cells = match project_attack_cells(
					deployment_cell,
					facing,
					base_range,
					stage.map_dimensions(),
				) {
					Ok(cells) if !cells.is_empty() => cells,
					Ok(_) => {
						exclusions.push(exclusion(
							None,
							Some(build.operator_id().to_owned()),
							"empty_projected_base_range",
							format!(
								"base range is empty at ({}, {}) facing {facing:?}",
								deployment_cell.row, deployment_cell.col
							),
						));
						continue;
					}
					Err(error) => {
						exclusions.push(exclusion(
							None,
							Some(build.operator_id().to_owned()),
							"base_range_projection_failed",
							error.to_string(),
						));
						continue;
					}
				};
				let skill_attack_cells = match projected_skill_attack_cells(
					deployment_cell,
					facing,
					skill_range,
					stage.map_dimensions(),
					profile.range().forward_extension_tiles(),
				) {
					Ok(cells) if !cells.is_empty() => cells,
					Ok(_) => {
						exclusions.push(exclusion(
							None,
							Some(build.operator_id().to_owned()),
							"empty_projected_skill_range",
							format!(
								"skill range is empty at ({}, {}) facing {facing:?}",
								deployment_cell.row, deployment_cell.col
							),
						));
						continue;
					}
					Err(error) => {
						exclusions.push(exclusion(
							None,
							Some(build.operator_id().to_owned()),
							error.code(),
							error.detail().to_owned(),
						));
						continue;
					}
				};
				let activation_boundaries: Vec<u64> = candidate_activation_boundaries(
					&full_battle_planning.model,
					&base_attack_cells,
					&skill_attack_cells,
					planning_duty_window,
				);
				let activation_schedule_limit: usize = activation_boundaries
					.len()
					.checked_mul(2)
					.and_then(|count| count.checked_add(1))
					.ok_or_else(|| {
						LevelSolvePrepareError::new(
							"activation_schedule_count_overflow",
							"full-battle candidate-boundary schedule count overflowed",
						)
					})?;
				let mut activation_schedules: Vec<Vec<TickSpan>> =
					match enumerate_expected_damage_activation_schedules(
						&expected_damage_activation_timing(&build, &profile, tick_rate),
						deploy_tick,
						planning_duty_window,
						&activation_boundaries,
						tick_rate.ticks_per_second(),
						activation_schedule_limit,
					) {
						Ok(schedules) if !schedules.is_empty() => schedules,
						Ok(_) => vec![Vec::new()],
						Err(error) => {
							exclusions.push(exclusion(
								None,
								Some(build.operator_id().to_owned()),
								"skill_activation_unsupported_base_only",
								error.to_string(),
							));
							vec![Vec::new()]
						}
					};
				include_manual_base_only_schedule(
					profile.activation_type(),
					&mut activation_schedules,
				);
				let earliest_legal_activation_tick: Option<u64> = activation_schedules
					.iter()
					.filter_map(|schedule| schedule.first().map(|span| span.start))
					.min();
				let materialize_candidate =
					|stable_build_id: String| -> Result<Candidate, LevelSolvePrepareError> {
						let candidate: Candidate = Candidate::new(
							stable_build_id,
							build.operator_id().to_owned(),
							build.name().to_owned(),
							build.deployment_cost(),
							deployment_cell,
							facing,
							base_attack_cells.clone(),
							base_attack.clone(),
						)
						.map_err(|error| {
							LevelSolvePrepareError::new(
								"candidate_materialization_failed",
								error.to_string(),
							)
						})?;
						Ok(match profile.template() {
							SkillDamageMechanicTemplate::InstantPacket
							| SkillDamageMechanicTemplate::PeriodicAoe => candidate
								.with_activation_triggered_active_attack(active_attack.clone()),
							SkillDamageMechanicTemplate::ModifiedNormal
							| SkillDamageMechanicTemplate::AmmoModifiedNormal => {
								candidate.with_active_attack(active_attack.clone())
							}
						}
						.with_active_attack_cells(skill_attack_cells.clone()))
					};
				let ranking_candidate: Candidate =
					materialize_candidate("retained-ranking-probe".to_owned())?;
				let ranking_scorers: Vec<PreparedOptimisticHpGroupCapacityScorer<'_>> =
					prepare_retained_wave_local_hp_group_scorers(
						&full_battle_planning.model,
						&ranking_candidate,
						&retained_ranking_groups,
					)?;
				for active_spans in &activation_schedules {
					if u64::try_from(materialized_candidates.len()).unwrap_or(u64::MAX)
						>= LEVEL_SOLVE_CANDIDATE_BUDGET
					{
						budget_exhausted = true;
						break 'builds;
					}
					let candidate_id: String = candidate_id(
						build.stable_id(),
						profile.skill_id(),
						candidate_search_priority(
							&source_deployment.action.deployment_anchor,
							deployment_cell,
							facing,
						),
						deployment_cell,
						facing,
						deploy_tick,
						active_spans,
					);
					let key: ContributionCandidateKey =
						ContributionCandidateKey::new(candidate_id.clone()).map_err(|error| {
							LevelSolvePrepareError::new(
								"candidate_identity_failed",
								error.to_string(),
							)
						})?;
					let candidate: Candidate = materialize_candidate(candidate_id)?;
					let single_deployment: Vec<ResolvedCoverageDeployment<'_>> =
						vec![ResolvedCoverageDeployment::new(&candidate, deploy_tick)
							.with_active_spans(active_spans.clone())];
					let ranking_contributions: BTreeMap<ContributionConstraintKey, FixedDamage> =
						retained_wave_local_hp_group_contributions(
							deploy_tick,
							active_spans,
							&retained_ranking_groups,
							&ranking_scorers,
						)?;
					let ranking_score: u64 = retained_damage_ranking_score(
						&full_battle_planning.wave_local_hp_groups,
						&ranking_contributions,
					)?;
					let reachability_score: OptimisticWaveLocalReachabilityScore =
						score_optimistic_wave_local_reachability_capacity(
							OptimisticWaveLocalReachabilityScoreRequest {
								template: &full_battle_planning.template,
								deployments: &single_deployment,
								hp_groups: &full_battle_planning.wave_local_hp_groups,
							},
						)
						.map_err(|error| {
							LevelSolvePrepareError::new(
								"candidate_wave_local_capacity_failed",
								error.to_string(),
							)
						})?;
					let contributions: BTreeMap<ContributionConstraintKey, FixedDamage> =
						wave_local_hp_group_contributions(&reachability_score)?;
					independent_candidates.push(IndependentContributionCandidate {
						key: key.clone(),
						operator_identity: ContributionOperatorIdentity::new(
							build.operator_id().to_owned(),
						)
						.map_err(|error| {
							LevelSolvePrepareError::new(
								"operator_identity_failed",
								error.to_string(),
							)
						})?,
						operator_name: build.name().to_owned(),
						deployment_cost: build.deployment_cost(),
						deployment_cell,
						optimistic_damage_by_constraint: contributions,
						ranking_score,
					});
					materialized_candidates.insert(
						key.clone(),
						MaterializedSkillCandidate {
							key,
							candidate,
							exact_build: build.build().clone(),
							skill_id: profile.skill_id().to_owned(),
							manual_activation: matches!(
								profile.activation_type(),
								SkillActivationType::Manual
							),
							deploy_tick,
							earliest_legal_activation_tick,
							active_spans: active_spans.clone(),
						},
					);
				}
			}
		}
	}
	if budget_exhausted {
		exclusions.push(exclusion(
			None,
			None,
			"candidate_budget_exhausted",
			format!(
				"materialization stopped at workflow candidate budget {}",
				LEVEL_SOLVE_CANDIDATE_BUDGET
			),
		));
	}
	if independent_candidates.is_empty() {
		return Err(LevelSolvePrepareError::new(
			"no_supported_candidates",
			format!(
				"no roster build produced a legal reviewed skill-damage candidate; {} exclusions",
				exclusions.len()
			),
		));
	}
	independent_candidates.sort_by(|left, right| left.key.cmp(&right.key));
	exclusions.sort_by(|left, right| {
		left.operator_id
			.cmp(&right.operator_id)
			.then_with(|| left.reason_code.cmp(&right.reason_code))
			.then_with(|| left.detail.cmp(&right.detail))
	});

	let candidate_audits: Vec<ExpectedDamagePreparedCandidateAudit> = materialized_candidates
		.values()
		.map(|materialized| ExpectedDamagePreparedCandidateAudit {
			candidate_key: materialized.key.as_str().to_owned(),
			operator_id: materialized.candidate.operator_id().to_owned(),
			operator_name: materialized.candidate.operator_name().to_owned(),
			skill_id: materialized.skill_id.clone(),
			deployment_cost: materialized.candidate.deployment_cost(),
			deployment_cell: materialized.candidate.deployment_cell(),
			facing: materialized.candidate.facing(),
			deploy_tick: materialized.deploy_tick,
			activations: materialized
				.active_spans
				.iter()
				.map(|span| ExpectedDamageActivationAudit {
					start_tick: span.start,
					end_tick: span.end,
				})
				.collect(),
		})
		.collect();
	let effective_target_cohort_ids: Vec<String> = target_cohort_ids.iter().cloned().collect();
	let evaluator = JointExpectedDamageEvaluator {
		original_schedule,
		candidates: materialized_candidates,
		model: full_battle_planning.model,
		target_cohort_ids,
		duty_window: planning_duty_window,
	};
	let guide_sha256: [u8; 32] = Sha256::digest(request.guide_json.as_bytes()).into();
	let worker_context = ReplayWorkerContext {
		evaluator: &evaluator,
		snapshot: request.snapshot,
		prefabs: request.prefabs,
		stage: &stage,
		source_guide: &guide,
		source_guide_json: request.guide_json,
		source_guide_source_id: request.guide_source_id,
		requested_level: request.requested_level,
		source_seed: &replay_seed,
		source_bindings: &replay_bindings,
		source_slot_name: &source_deployment.action.operator_name,
		guide_selection_budget: request.guide_selection_budget,
		guide_sha256,
		tick_rate,
		progress_observer,
		progress_started,
	};
	let pipeline: VerifiedSearchPipelineResult = run_verified_search_pipeline(
		ExpectedDamageLevelSolveRequest {
			demands: &demands,
			candidates: &independent_candidates,
			candidate_exclusions: &exclusions,
			candidate_materialization_complete: !budget_exhausted,
			planning_model_complete: false,
			additive_proposal_limit: request.additive_proposal_limit,
			explore_cardinalities: true,
			joint_evaluator: &evaluator,
		},
		&worker_context,
		request.replay_worker_count,
		request.top_k,
	)?;
	let VerifiedSearchPipelineResult {
		solve,
		battlefield_verified_jobs,
		job_materialization_failures,
		battlefield_replays,
		battlefield_expected_victory_proposal_indices,
		replay_queue_capacity,
	} = pipeline;
	let verified_search_status: ExpectedDamageVerifiedSearchStatus =
		verified_search_status(battlefield_verified_jobs.len(), request.top_k, solve.status);

	Ok(ExpectedDamagePreparedLevelReport {
		schema_version: EXPECTED_DAMAGE_PREPARED_REPORT_SCHEMA_VERSION,
		claim: ExpectedDamagePreparedClaim::ExpectedDamageBaseAndSkillPhases,
		build_policy: ExpectedDamageBuildPolicy::E2MaxTrust200Skill10Potential6NoModule,
		data_revision: request.snapshot.resolved_revision().to_owned(),
		mechanics_revision: request.prefabs.mechanics_revision().to_owned(),
		requested_level: request.requested_level.to_owned(),
		stage_id: stage.stage_id().to_owned(),
		level_id: stage.level_id().to_owned(),
		guide_source_id: source_provenance.source_id,
		guide_content_sha256: source_provenance.content_sha256,
		guide_difficulty: source_provenance.difficulty,
		guide_selection_index: source_provenance.selection_index,
		guide_selected_slots: source_provenance.selected_slots,
		source_action_index: source_provenance.source_action_index,
		source_operator_id: source_deployment.resolved_build.operator_id().to_owned(),
		source_operator_name: source_provenance.source_operator_name,
		source_guide_deploy_tick: source_deploy_tick,
		effective_target_cohort_ids,
		materialized_candidates: candidate_audits,
		full_battle_damage_gap,
		requested_top_k: request.top_k,
		additive_proposal_limit: request.additive_proposal_limit,
		replay_worker_count: request.replay_worker_count,
		replay_queue_capacity,
		verified_search_status,
		solve,
		battlefield_verified_jobs,
		job_materialization_failures,
		battlefield_replays,
		battlefield_expected_victory_proposal_indices,
		limitations: vec![
			"the MILP uses cumulative full-HP deadline groups over every stage enemy; incomplete \
			 candidate-only route-intersection capacities rank proposals but are not hard damage \
			 cuts"
				.to_owned(),
			"each proposal is rescored with retained-plus-replacement group output, shared HP, \
			 target priority, HP caps, and attack contention before full battlefield replay"
				.to_owned(),
			"only direct delivery is materialized; splash and chain candidates fail closed"
				.to_owned(),
			"full-stage expected-damage feasibility is diagnostic evidence only; a MAA job file is \
			 materialized only after full battlefield replay reaches expected victory"
				.to_owned(),
			"planning currently uses unblocked trajectories; blocking, survival, and dynamic \
			 occupancy are authoritative only in the full replay and are not yet searched as \
			 planning decisions"
				.to_owned(),
			"offensive RNG is evaluated as expected damage; single-target defensive damage RNG \
			 branches through the full replay, while mixed or multi-friendly RNG fails closed"
				.to_owned(),
			"candidate placement searches every stage-legal cell and facing, but deployment time \
			 remains the source action's earliest DP-legal tick; later deployment choices are not \
			 searched"
				.to_owned(),
			"DP scheduling models stage regeneration, guide costs and delays, and the deployment \
			 points a retained operator's automatic skill grants on its own cadence; a manual \
			 skill's grants, a replacement's own grants and combat-state DP changes are not \
			 modeled, and the export's deploy-tick equality rejects a replay that diverges"
				.to_owned(),
			"retained guide members use base-panel normal attacks only; retained skills are not \
			 modeled, so additive exhaustion is reported as planning_model_incomplete rather than \
			 battle infeasibility"
				.to_owned(),
			"multi-operator proposals exhaust shared-DP deployment orders through eight members and \
			 sample up to 64 deterministic cost/timing orders above that; any victorious order is \
			 reused for full replay and MAA job file export"
				.to_owned(),
			"time-recovery SP uses the pinned increment and conservatively recharges after the \
			 active phase"
				.to_owned(),
			"unsupported skill activation timing downgrades that candidate to base attacks only \
			 and is recorded as an exclusion"
				.to_owned(),
		],
	})
}

/// Independently replays a full-battlefield exported strategy.
///
/// This path does not consume the solver proposal that produced the strategy. It rebuilds the
/// retained-only full-stage gap and planning scope from the original guide, then recomputes exact
/// builds, DP timing, skill readiness, geometry, joint damage, and full-battle replay from the
/// exported strategy and canonical artifact.
pub(super) fn validate_expected_damage_export_rebuilt(
	request: RebuiltExportValidationRequest<'_>,
) -> Result<ExpectedDamageExportValidationReport, LevelSolvePrepareError> {
	request
		.prefabs
		.ensure_compatible_with(request.snapshot)
		.map_err(|error| {
			LevelSolvePrepareError::new("battle_prefab_revision_mismatch", error.to_string())
		})?;
	if request.guide_selection_budget == 0 {
		return Err(LevelSolvePrepareError::new(
			"zero_guide_selection_budget",
			"guide selection budget must be positive",
		));
	}
	validate_content_digest(
		"source_guide",
		request.source_guide_json.as_bytes(),
		request.source_guide_content_sha256,
	)?;
	request.artifact.validate().map_err(|error| {
		LevelSolvePrepareError::new("invalid_strategy_artifact", error.to_string())
	})?;
	let canonical_artifact_bytes: Vec<u8> = request.artifact.encode().map_err(|error| {
		LevelSolvePrepareError::new("artifact_encoding_failed", error.to_string())
	})?;
	if canonical_artifact_bytes != request.artifact_bytes {
		return Err(LevelSolvePrepareError::new(
			"noncanonical_strategy_artifact",
			"artifact input bytes do not equal the decoded artifact's canonical encoding",
		));
	}
	if request.artifact.data_revision != request.snapshot.resolved_revision() {
		return Err(LevelSolvePrepareError::new(
			"artifact_revision_mismatch",
			format!(
				"artifact revision {:?} does not match snapshot {:?}",
				request.artifact.data_revision,
				request.snapshot.resolved_revision()
			),
		));
	}
	if request.artifact.mechanics_revision != request.prefabs.mechanics_revision() {
		return Err(LevelSolvePrepareError::new(
			"artifact_mechanics_revision_mismatch",
			format!(
				"artifact mechanics revision {:?} does not match battle-prefab snapshot {:?}",
				request.artifact.mechanics_revision,
				request.prefabs.mechanics_revision()
			),
		));
	}
	let strategy_digest: [u8; 32] = Sha256::digest(request.maa_job_json.as_bytes()).into();
	if request.artifact.guide_sha256 != strategy_digest {
		return Err(LevelSolvePrepareError::new(
			"artifact_strategy_digest_mismatch",
			"artifact guide digest does not match the exact exported strategy bytes",
		));
	}
	let strategy_sha256: String = hex_digest(&strategy_digest);
	let artifact_id: String = hex_digest(&request.artifact.artifact_id().map_err(|error| {
		LevelSolvePrepareError::new("artifact_encoding_failed", error.to_string())
	})?);
	ensure_export_validation_artifact_status(request.artifact.battlefield_status)?;

	let derived_source: DerivedSourceGuide = derive_source_guide(SourceGuideDerivationRequest {
		snapshot: request.snapshot,
		requested_level: request.requested_level,
		guide_json: request.source_guide_json,
		guide_source_id: request.source_guide_source_id,
		guide_content_sha256: request.source_guide_content_sha256,
		difficulty: request.difficulty,
		selection_budget: request.guide_selection_budget,
		purpose: SourceGuideDerivationPurpose::IndependentValidation,
	})?;
	let _source_guide: MaaJob = derived_source.guide;
	let source_materialized_seed: MaaJobSeed = derived_source.seed;
	let source_provenance: SourceGuideProvenance = derived_source.provenance;
	let source_action_index: usize = source_provenance.source_action_index;
	let stage: CompiledStage = derived_source.stage;
	let source_domain: GuideDeploymentDomain = derived_source.domain;
	let tick_rate: TickRate = TickRate::new(
		NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("level-solve tick rate is nonzero"),
	);
	let source_overrides: GuideDeploymentActionOverrides =
		guide_exact_action_overrides(request.snapshot, request.prefabs, &source_domain, tick_rate)?;
	let source_schedule = prepare_guide_deployment_schedule_with_cost_overrides(
		request.snapshot,
		NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("level-solve tick rate is nonzero"),
		&source_domain,
		&source_overrides,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("source_schedule_materialization_failed", error.to_string())
	})?;
	let source_deployment: &PreparedGuideDeployment = source_schedule
		.deployments
		.iter()
		.find(|deployment| deployment.action.is_source_slot)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"source_deployment_missing",
				"source guide schedule contains no source deployment",
			)
		})?;
	let source_replay_bindings: Vec<ExactReplayBuildBinding> =
		guide_no_module_replay_bindings(request.snapshot, &source_materialized_seed)?;
	let retained_only_input = materialize_source_removed_expected_replay_input(
		request.snapshot,
		request.prefabs,
		&source_materialized_seed,
		&source_replay_bindings,
		&source_deployment.action.operator_name,
		tick_rate,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("full_battle_gap_materialization_failed", error.to_string())
	})?;
	let full_battle_damage_gap: FullBattleDamageGapLedger =
		derive_full_battle_damage_gap_ledger(&retained_only_input).map_err(|error| {
			LevelSolvePrepareError::new("full_battle_gap_derivation_failed", error.to_string())
		})?;
	let full_battle_planning: FullBattlePlanningContext =
		prepare_full_battle_unblocked_planning_context(&stage, tick_rate, &full_battle_damage_gap)
			.map_err(|error| {
				LevelSolvePrepareError::new(
					"full_battle_planning_materialization_failed",
					error.to_string(),
				)
			})?;

	let exported_guide = parse_maa_job(request.maa_job_json).map_err(|error| {
		LevelSolvePrepareError::new("strategy_import_failed", error.to_string())
	})?;
	if !exported_guide.groups().is_empty() {
		return Err(LevelSolvePrepareError::new(
			"exported_strategy_groups_forbidden",
			"exported strategy must contain no operator groups",
		));
	}
	let mut exported_seeds: Vec<MaaJobSeed> = exported_guide
		.executable_seed_selections(request.difficulty, 2)
		.map_err(|error| {
			LevelSolvePrepareError::new("strategy_selection_failed", error.to_string())
		})?
		.map(MaaJobSeedSelection::materialize)
		.collect();
	if exported_seeds.len() != 1 {
		return Err(LevelSolvePrepareError::new(
			"exported_strategy_seed_count_mismatch",
			format!(
				"exported strategy must materialize exactly one seed, found {}",
				exported_seeds.len()
			),
		));
	}
	let exported_owned_seed: MaaJobSeed = exported_seeds
		.pop()
		.expect("one exported seed was validated");
	if exported_owned_seed.difficulty() != request.difficulty {
		return Err(LevelSolvePrepareError::new(
			"exported_strategy_difficulty_mismatch",
			format!(
				"exported strategy difficulty {:?} does not match requested {:?}",
				exported_owned_seed.difficulty(),
				request.difficulty
			),
		));
	}
	validate_exported_stage_matches_source(request.snapshot, &stage, &exported_owned_seed)?;

	validate_retained_artifact_build_policy(
		&source_materialized_seed,
		source_action_index,
		&source_replay_bindings,
		request.artifact,
	)?;
	let resolved_builds: BTreeMap<String, ResolvedOperatorBuild> =
		resolve_artifact_builds(request.snapshot, request.artifact)?;
	validate_retained_resolved_build_policy(
		&source_materialized_seed,
		source_action_index,
		&source_replay_bindings,
		&resolved_builds,
	)?;
	let shape: ValidatedJointExportShape = validate_joint_exported_strategy_contract(
		&source_materialized_seed,
		&exported_owned_seed,
		source_action_index,
		source_deployment.resolved_build.operator_id(),
		request.artifact,
		&resolved_builds,
	)?;
	validate_joint_expected_damage_export(
		request,
		&stage,
		&source_domain,
		source_provenance,
		source_deployment,
		&full_battle_planning,
		&exported_owned_seed,
		&resolved_builds,
		&shape,
		strategy_digest,
		strategy_sha256,
		artifact_id,
		tick_rate,
	)
}

#[allow(
	clippy::too_many_arguments,
	reason = "independent export validation consumes every provenance input explicitly"
)]
fn validate_joint_expected_damage_export(
	request: RebuiltExportValidationRequest<'_>,
	stage: &CompiledStage,
	source_domain: &GuideDeploymentDomain,
	source_provenance: SourceGuideProvenance,
	source_deployment: &PreparedGuideDeployment,
	full_battle_planning: &FullBattlePlanningContext,
	exported_seed: &MaaJobSeed,
	resolved_builds: &BTreeMap<String, ResolvedOperatorBuild>,
	shape: &ValidatedJointExportShape,
	strategy_digest: [u8; 32],
	strategy_sha256: String,
	artifact_id: String,
	tick_rate: TickRate,
) -> Result<ExpectedDamageExportValidationReport, LevelSolvePrepareError> {
	let exported_guide = parse_maa_job(request.maa_job_json).map_err(|error| {
		LevelSolvePrepareError::new("strategy_import_failed", error.to_string())
	})?;
	let lineage =
		MaaJobSourceLineage::new("ark-solver-export", &strategy_sha256).map_err(|error| {
			LevelSolvePrepareError::new("strategy_lineage_failed", error.to_string())
		})?;
	let exported_seed_view = exported_guide
		.executable_guide_seeds(lineage, exported_seed.difficulty(), 1)
		.map_err(|error| {
			LevelSolvePrepareError::new("strategy_selection_failed", error.to_string())
		})?
		.next()
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_strategy_seed_missing",
				"validated exported strategy produced no seed",
			)
		})?;
	let build_pool: CandidatePoolSpec = CandidatePoolSpec::Roster {
		builds: resolved_builds
			.values()
			.map(|build| BasePanelBuildSpec {
				operator_id: build.operator_id().to_owned(),
				elite: build.build().elite(),
				level: build.build().level(),
			})
			.collect(),
	};
	let source_action_index: usize = shape
		.replacement_names
		.first()
		.and_then(|name| shape.deploy_action_index_by_name.get(name))
		.copied()
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_replacement_deploy_missing",
				"validated joint export has no first replacement deployment",
			)
		})?;
	if source_action_index != source_domain.source_action_index {
		return Err(LevelSolvePrepareError::new(
			"exported_source_action_index_mismatch",
			format!(
				"exported replacement starts at action {source_action_index}, source guide uses {}",
				source_domain.source_action_index
			),
		));
	}
	let exported_domain = guide_deployment_domain_from_maa_job(
		&exported_seed_view,
		source_action_index,
		stage.map_dimensions(),
		build_pool,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("strategy_domain_materialization_failed", error.to_string())
	})?;
	let replacement_names: BTreeSet<&str> =
		shape.replacement_names.iter().map(String::as_str).collect();
	let replacement_actions: Vec<_> = exported_domain
		.actions
		.iter()
		.filter(|action| replacement_names.contains(action.operator_name.as_str()))
		.collect();
	if replacement_actions.len() != shape.replacement_names.len() {
		return Err(LevelSolvePrepareError::new(
			"exported_replacement_domain_mismatch",
			"deployment domain does not contain every replacement exactly once",
		));
	}
	if replacement_actions
		.iter()
		.all(|action| action.action_index != source_action_index)
	{
		return Err(LevelSolvePrepareError::new(
			"strategy_source_deployment_missing",
			"exported domain has no first replacement deployment",
		));
	}
	// Every replacement is excluded from the planned grants, not only the first: the joint
	// scheduler that produced this strategy inserted them all without grants, so validation must
	// schedule the same DP the solver did and let the equality check judge replay.
	let registry: BattlePrefabHandlerRegistry = built_in_battle_prefab_handler_registry();
	let mut exact_costs: BTreeMap<usize, u32> = BTreeMap::new();
	let mut exported_grants: BTreeMap<usize, Vec<PlannedDeploymentDpGrant>> = BTreeMap::new();
	for action in &exported_domain.actions {
		let build = resolved_builds.get(&action.operator_name).ok_or_else(|| {
			LevelSolvePrepareError::new(
				"strategy_build_missing",
				format!("artifact has no exact build for {:?}", action.operator_name),
			)
		})?;
		exact_costs.insert(action.action_index, build.deployment_cost());
		if replacement_names.contains(action.operator_name.as_str()) {
			continue;
		}
		let planned: Vec<PlannedDeploymentDpGrant> = planned_dp_grants(
			request.snapshot,
			request.prefabs,
			&registry,
			build,
			tick_rate,
		);
		if !planned.is_empty() {
			exported_grants.insert(action.action_index, planned);
		}
	}
	let exported_overrides = GuideDeploymentActionOverrides {
		deployment_cost_by_action: exact_costs,
		dp_grants_by_action: exported_grants,
	};
	let exported_schedule = prepare_interleaved_export_deployment_schedule_with_cost_overrides(
		request.snapshot,
		NonZeroU32::new(LEVEL_SOLVE_TICKS_PER_SECOND).expect("level-solve tick rate is nonzero"),
		&exported_domain,
		&exported_overrides,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new(
			"strategy_schedule_materialization_failed",
			error.to_string(),
		)
	})?;
	let exported_bindings: Vec<ExactReplayBuildBinding> = exported_seed
		.operators()
		.iter()
		.map(|operator| {
			let build = resolved_builds.get(operator.name()).ok_or_else(|| {
				LevelSolvePrepareError::new(
					"strategy_build_missing",
					format!("artifact has no exact build for {:?}", operator.name()),
				)
			})?;
			Ok(ExactReplayBuildBinding {
				source_name: operator.name().to_owned(),
				build: build.build().clone(),
				skill_usage: operator.skill_usage().unwrap_or(0),
				skill_times: operator.skill_times(),
			})
		})
		.collect::<Result<_, LevelSolvePrepareError>>()?;
	let prepared_guide: PreparedReplayGuide =
		prepare_replay_guide(request.snapshot, exported_seed, &exported_bindings).map_err(
			|error| {
				LevelSolvePrepareError::new("export_guide_preparation_failed", error.to_string())
			},
		)?;
	let replay_input = materialize_expected_replay_input(
		request.snapshot,
		request.prefabs,
		ExpectedReplayCompileInput {
			stage_name: exported_seed.stage_name(),
			difficulty: replay_stage_difficulty(exported_seed.difficulty()).map_err(|error| {
				LevelSolvePrepareError::new("export_difficulty_mapping_failed", error.to_string())
			})?,
			bindings: &exported_bindings,
			guide: &prepared_guide,
			tick_rate,
		},
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("export_replay_materialization_failed", error.to_string())
	})?;
	let replay_output = execute_expected_replay(&replay_input, strategy_digest, &strategy_sha256);
	let (estimated_success_probability_millionths, probability_basis) =
		verified_expected_victory_job_probability(
			&replay_output.report.status,
			replay_output.report.defensive_probability.as_ref(),
		)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_strategy_probability_unverified",
				"independent full-battle replay did not produce a verified positive success \
				 probability",
			)
		})?;
	if replay_output.artifact.battlefield_status != request.artifact.battlefield_status {
		return Err(LevelSolvePrepareError::new(
			"artifact_battlefield_status_mismatch",
			format!(
				"artifact claims {:?}, replay computed {:?}",
				request.artifact.battlefield_status, replay_output.artifact.battlefield_status
			),
		));
	}
	let schedule_by_action_index: BTreeMap<usize, &PreparedGuideDeployment> = exported_schedule
		.deployments
		.iter()
		.map(|deployment| (deployment.action.action_index, deployment))
		.collect();
	let replay_actions_by_index: BTreeMap<usize, &crate::replay::ReplayActionAudit> = replay_output
		.report
		.actions
		.iter()
		.map(|action| (action.action_index, action))
		.collect();
	for deployment in &exported_schedule.deployments {
		let action_index: usize = deployment.action.action_index;
		let replay_deploy_tick: Option<u64> = replay_actions_by_index
			.get(&action_index)
			.and_then(|action| action.executed_tick);
		validate_replayed_deployment_tick(
			&deployment.action.operator_name,
			action_index,
			deployment.deploy_tick(),
			replay_deploy_tick,
		)?;
	}
	let daemon_action_index: usize = exported_seed
		.actions()
		.len()
		.checked_sub(1)
		.expect("validated exported strategy has a final SkillDaemon");
	let daemon_tick: u64 = replay_actions_by_index
		.get(&daemon_action_index)
		.and_then(|action| action.executed_tick)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"strategy_skill_daemon_not_executed",
				"exported strategy's final SkillDaemon did not execute",
			)
		})?;
	let mut candidate_storage: Vec<Candidate> = Vec::with_capacity(shape.replacement_names.len());
	let mut active_span_storage: Vec<Vec<TickSpan>> =
		Vec::with_capacity(shape.replacement_names.len());
	let mut candidate_deploy_ticks: Vec<u64> = Vec::with_capacity(shape.replacement_names.len());
	for replacement_name in &shape.replacement_names {
		let deploy_action_index: usize = shape.deploy_action_index_by_name[replacement_name];
		let skill_action_index: usize = shape.skill_action_index_by_name[replacement_name];
		let deployment = schedule_by_action_index
			.get(&deploy_action_index)
			.copied()
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"strategy_replacement_schedule_missing",
					format!("replacement {replacement_name:?} has no scheduled deployment"),
				)
			})?;
		let deploy_tick: u64 = deployment.deploy_tick();
		let first_activation_tick: u64 = replay_actions_by_index
			.get(&skill_action_index)
			.and_then(|action| action.executed_tick)
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"strategy_replacement_skill_not_executed",
					format!("replacement {replacement_name:?} explicit Skill did not execute"),
				)
			})?;
		let build: &ResolvedOperatorBuild = &resolved_builds[replacement_name];
		let profile: ExpectedDamageSkillProfile =
			match compile_expected_damage_skill(build, tick_rate) {
				ExpectedDamageSkillEntry::Supported(profile) => *profile,
				ExpectedDamageSkillEntry::Unsupported(unsupported) => {
					return Err(LevelSolvePrepareError::new(
						"unsupported_exported_damage_skill",
						unsupported
							.reasons()
							.iter()
							.map(ToString::to_string)
							.collect::<Vec<_>>()
							.join("; "),
					));
				}
			};
		if !matches!(profile.activation_type(), SkillActivationType::Manual) {
			return Err(LevelSolvePrepareError::new(
				"exported_skill_activation_not_manual",
				format!(
					"explicit Skill for {replacement_name:?} requires manual activation, found \
					 {:?}",
					profile.activation_type()
				),
			));
		}
		let action = exported_domain
			.actions
			.iter()
			.find(|action| action.action_index == deploy_action_index)
			.expect("validated deployment action exists in domain");
		let probe: Candidate = materialize_validated_candidate(
			request.snapshot,
			stage,
			build,
			&profile,
			action.deployment_anchor,
			deploy_tick,
			&[],
		)?;
		let boundaries: Vec<u64> = candidate_activation_boundaries(
			&full_battle_planning.model,
			probe.attack_cells(),
			probe.active_attack_cells(),
			full_battle_planning.duty_window,
		);
		let schedules: Vec<Vec<TickSpan>> = enumerate_expected_damage_activation_schedules(
			&expected_damage_activation_timing(build, &profile, tick_rate),
			deploy_tick,
			full_battle_planning.duty_window,
			&boundaries,
			tick_rate.ticks_per_second(),
			boundaries
				.len()
				.checked_mul(2)
				.and_then(|count| count.checked_add(1))
				.ok_or_else(|| {
					LevelSolvePrepareError::new(
						"activation_schedule_count_overflow",
						"activation schedule count overflowed",
					)
				})?,
		)?;
		let active_spans: Vec<TickSpan> = schedules
			.into_iter()
			.filter(|schedule| !schedule.is_empty())
			.min_by_key(|schedule| schedule[0].start)
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"exported_skill_never_ready",
					format!(
						"replacement {replacement_name:?} has no legal activation in the duty \
						 window"
					),
				)
			})?;
		validate_exported_activation_timing(
			replacement_name,
			first_activation_tick,
			&active_spans,
			daemon_tick,
		)?;
		candidate_storage.push(materialize_validated_candidate(
			request.snapshot,
			stage,
			build,
			&profile,
			action.deployment_anchor,
			deploy_tick,
			&active_spans,
		)?);
		active_span_storage.push(active_spans);
		candidate_deploy_ticks.push(deploy_tick);
	}
	let mut deployments: Vec<ResolvedCoverageDeployment<'_>> = exported_schedule
		.deployments
		.iter()
		.filter(|deployment| !replacement_names.contains(deployment.action.operator_name.as_str()))
		.filter_map(PreparedGuideDeployment::as_retained_coverage_deployment)
		.collect();
	for ((candidate, active_spans), &deploy_tick) in candidate_storage
		.iter()
		.zip(&active_span_storage)
		.zip(&candidate_deploy_ticks)
	{
		deployments.push(
			ResolvedCoverageDeployment::new(candidate, deploy_tick)
				.with_active_spans(active_spans.clone()),
		);
	}
	let score: JointCoverageScore = score_joint_deployments(JointCoverageScoreRequest {
		model: &full_battle_planning.model,
		deployments: &deployments,
		target_cohort_ids: &full_battle_planning.target_cohort_ids,
		duty_window: full_battle_planning.duty_window,
	})
	.map_err(|error| {
		LevelSolvePrepareError::new(
			"exported_joint_expected_damage_scoring_failed",
			error.to_string(),
		)
	})?;
	let expected_damage: ExpectedDamageJointEvidence = ExpectedDamageJointEvidence::from(&score);
	validate_joint_planning_scope(
		&expected_damage,
		&full_battle_planning.model,
		&full_battle_planning.target_cohort_ids,
		full_battle_planning.duty_window,
	)?;
	let damage_status: DeterministicDamageStatus = replay_output.artifact.damage_status;
	let expected_bytes: Vec<u8> = replay_output.artifact.encode().map_err(|error| {
		LevelSolvePrepareError::new("artifact_encoding_failed", error.to_string())
	})?;
	if expected_bytes != request.artifact_bytes {
		return Err(LevelSolvePrepareError::new(
			"artifact_independent_replay_mismatch",
			"artifact does not equal independently replayed battlefield and damage evidence",
		));
	}
	Ok(ExpectedDamageExportValidationReport {
		schema_version: EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION,
		data_revision: request.snapshot.resolved_revision().to_owned(),
		mechanics_revision: request.prefabs.mechanics_revision().to_owned(),
		requested_level: request.requested_level.to_owned(),
		stage_id: stage.stage_id().to_owned(),
		source_guide_source_id: source_provenance.source_id,
		source_guide_content_sha256: source_provenance.content_sha256,
		source_guide_difficulty: source_provenance.difficulty,
		source_guide_selection_index: source_provenance.selection_index,
		source_guide_selected_slots: source_provenance.selected_slots,
		source_action_index: source_provenance.source_action_index,
		source_operator_id: source_deployment.resolved_build.operator_id().to_owned(),
		source_operator_name: source_provenance.source_operator_name,
		strategy_sha256,
		artifact_id,
		damage_status,
		estimated_success_probability_millionths,
		probability_basis,
		dp: dp_audit(&exported_schedule),
		expected_damage,
		battlefield_status: replay_output.report.status,
		limitations: vec![
			"offensive RNG is evaluated by expected damage; verified defensive RNG probability \
			 comes from independent full-battle replay"
				.to_owned(),
			"the planning damage gate still uses unblocked trajectories; blocking, survival, \
			 redeployment, and deaths are authoritative in the full-battle replay"
				.to_owned(),
		],
	})
}

fn validate_requested_level(
	requested_level: &str,
	stage: &CompiledStage,
) -> Result<(), LevelSolvePrepareError> {
	if requested_level.trim().is_empty()
		|| !(requested_level == stage.stage_id()
			|| requested_level == stage.level_id()
			|| stage.code().is_some_and(|code| requested_level == code))
	{
		return Err(LevelSolvePrepareError::new(
			"level_mismatch",
			format!(
				"requested {:?} does not identify stage {:?}, level {:?}, or code {:?}",
				requested_level,
				stage.stage_id(),
				stage.level_id(),
				stage.code()
			),
		));
	}
	Ok(())
}

fn validate_content_digest(
	kind: &'static str,
	content: &[u8],
	claimed_sha256: &str,
) -> Result<(), LevelSolvePrepareError> {
	let actual: [u8; 32] = Sha256::digest(content).into();
	let actual: String = hex_digest(&actual);
	if claimed_sha256 != actual {
		let code: &'static str = match kind {
			"guide" => "guide_content_digest_mismatch",
			"source_guide" => "source_guide_content_digest_mismatch",
			_ => "content_digest_mismatch",
		};
		return Err(LevelSolvePrepareError::new(
			code,
			format!(
				"{kind} content SHA-256 claim {claimed_sha256:?} does not match exact bytes \
				 {actual:?}"
			),
		));
	}
	Ok(())
}

fn resolve_artifact_builds(
	snapshot: &ArkDataSnapshot,
	artifact: &StrategyArtifact,
) -> Result<BTreeMap<String, ResolvedOperatorBuild>, LevelSolvePrepareError> {
	let mut resolved: BTreeMap<String, ResolvedOperatorBuild> = BTreeMap::new();
	for record in &artifact.builds {
		let trust_percent: u8 = u8::try_from(record.trust_percent).map_err(|_| {
			LevelSolvePrepareError::new(
				"artifact_trust_overflow",
				format!(
					"operator {:?} trust {} does not fit u8",
					record.operator_id, record.trust_percent
				),
			)
		})?;
		let potential: PotentialRank = potential_rank(record.potential_rank)?;
		let module: OperatorModule = record
			.module_id
			.as_ref()
			.map_or(OperatorModule::None, |id| OperatorModule::Id(id.clone()));
		let build: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id(record.operator_id.clone()),
			record.elite,
			record.level,
			trust_percent,
			record.skill_slot,
			record.skill_level,
			potential,
			module,
			record.module_level,
		);
		let build: ResolvedOperatorBuild =
			snapshot.operators().resolve(&build).map_err(|error| {
				LevelSolvePrepareError::new(
					"artifact_build_resolution_failed",
					format!("operator {:?}: {error}", record.operator_id),
				)
			})?;
		if resolved.insert(build.name().to_owned(), build).is_some() {
			return Err(LevelSolvePrepareError::new(
				"duplicate_artifact_operator_name",
				format!(
					"multiple artifact builds resolve to operator name {:?}",
					record.operator_id
				),
			));
		}
	}
	if resolved.is_empty() {
		return Err(LevelSolvePrepareError::new(
			"empty_artifact_builds",
			"strategy artifact contains no exact builds",
		));
	}
	Ok(resolved)
}

fn potential_rank(value: u8) -> Result<PotentialRank, LevelSolvePrepareError> {
	match value {
		1 => Ok(PotentialRank::P1),
		2 => Ok(PotentialRank::P2),
		3 => Ok(PotentialRank::P3),
		4 => Ok(PotentialRank::P4),
		5 => Ok(PotentialRank::P5),
		6 => Ok(PotentialRank::P6),
		_ => Err(LevelSolvePrepareError::new(
			"invalid_artifact_potential",
			format!("potential rank {value} is outside P1..P6"),
		)),
	}
}

fn validate_retained_artifact_build_policy(
	source_seed: &MaaJobSeed,
	source_action_index: usize,
	source_bindings: &[ExactReplayBuildBinding],
	artifact: &StrategyArtifact,
) -> Result<(), LevelSolvePrepareError> {
	let replaced_operator_name: &str =
		replaced_source_operator_name(source_seed, source_action_index)?;
	for operator in source_seed
		.operators()
		.iter()
		.filter(|operator| operator.name() != replaced_operator_name)
	{
		let source_binding: &ExactReplayBuildBinding =
			exact_retained_source_binding(source_bindings, operator.name())?;
		let expected_operator_id: &str = match source_binding.build.identity() {
			OperatorIdentity::Id(operator_id) => operator_id,
			OperatorIdentity::Name(_) => {
				return Err(LevelSolvePrepareError::new(
					"exported_retained_build_policy_mismatch",
					format!(
						"retained operator {:?} source build is not revision-canonical",
						operator.name()
					),
				));
			}
		};
		let mut matching_records = artifact
			.builds
			.iter()
			.filter(|record| record.operator_id == expected_operator_id);
		let record: &ArtifactExactBuild = matching_records.next().ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_retained_build_policy_mismatch",
				format!(
					"retained operator {:?} has no artifact build for {:?}",
					operator.name(),
					expected_operator_id
				),
			)
		})?;
		if matching_records.next().is_some() {
			return Err(LevelSolvePrepareError::new(
				"exported_retained_build_policy_mismatch",
				format!(
					"retained operator {:?} has multiple artifact builds",
					operator.name()
				),
			));
		}
		validate_retained_artifact_build_record(operator.name(), &source_binding.build, record)?;
	}
	Ok(())
}

fn validate_retained_resolved_build_policy(
	source_seed: &MaaJobSeed,
	source_action_index: usize,
	source_bindings: &[ExactReplayBuildBinding],
	resolved_builds: &BTreeMap<String, ResolvedOperatorBuild>,
) -> Result<(), LevelSolvePrepareError> {
	let replaced_operator_name: &str =
		replaced_source_operator_name(source_seed, source_action_index)?;
	for operator in source_seed
		.operators()
		.iter()
		.filter(|operator| operator.name() != replaced_operator_name)
	{
		let source_binding: &ExactReplayBuildBinding =
			exact_retained_source_binding(source_bindings, operator.name())?;
		let exported_build: &ResolvedOperatorBuild =
			resolved_builds.get(operator.name()).ok_or_else(|| {
				LevelSolvePrepareError::new(
					"exported_retained_build_policy_mismatch",
					format!(
						"retained operator {:?} has no resolved artifact build",
						operator.name()
					),
				)
			})?;
		if exported_build.build() != &source_binding.build {
			return Err(LevelSolvePrepareError::new(
				"exported_retained_build_policy_mismatch",
				format!(
					"retained operator {:?} resolved artifact build differs from the \
					 workflow-derived source build",
					operator.name()
				),
			));
		}
	}
	Ok(())
}

fn replaced_source_operator_name(
	source_seed: &MaaJobSeed,
	source_action_index: usize,
) -> Result<&str, LevelSolvePrepareError> {
	match source_seed.actions().get(source_action_index) {
		Some(MaaJobAction::Deploy { name, .. }) => Ok(name),
		_ => Err(LevelSolvePrepareError::new(
			"source_action_not_deploy",
			format!("source action {source_action_index} is not a deployment"),
		)),
	}
}

fn exact_retained_source_binding<'a>(
	source_bindings: &'a [ExactReplayBuildBinding],
	operator_name: &str,
) -> Result<&'a ExactReplayBuildBinding, LevelSolvePrepareError> {
	let mut matching_bindings = source_bindings
		.iter()
		.filter(|binding| binding.source_name == operator_name);
	let binding: &ExactReplayBuildBinding = matching_bindings.next().ok_or_else(|| {
		LevelSolvePrepareError::new(
			"exported_retained_build_policy_mismatch",
			format!("retained operator {operator_name:?} has no workflow-derived source build"),
		)
	})?;
	if matching_bindings.next().is_some() {
		return Err(LevelSolvePrepareError::new(
			"exported_retained_build_policy_mismatch",
			format!(
				"retained operator {operator_name:?} has multiple workflow-derived source builds"
			),
		));
	}
	Ok(binding)
}

fn validate_retained_artifact_build_record(
	operator_name: &str,
	expected: &OperatorBuild,
	record: &ArtifactExactBuild,
) -> Result<(), LevelSolvePrepareError> {
	let expected_operator_id: &str = match expected.identity() {
		OperatorIdentity::Id(operator_id) => operator_id,
		OperatorIdentity::Name(_) => {
			return Err(LevelSolvePrepareError::new(
				"exported_retained_build_policy_mismatch",
				format!("retained operator {operator_name:?} source build is not canonical"),
			));
		}
	};
	let expected_module_id: Option<&str> = match expected.module() {
		OperatorModule::None => None,
		OperatorModule::Id(module_id) => Some(module_id),
	};
	let matches_policy: bool = record.operator_id == expected_operator_id
		&& record.elite == expected.elite()
		&& record.level == expected.level()
		&& record.trust_percent == u16::from(expected.trust_percent())
		&& record.potential_rank == expected.potential().number()
		&& record.skill_slot == expected.skill()
		&& record.skill_level == expected.skill_level()
		&& record.module_id.as_deref() == expected_module_id
		&& record.module_level == expected.module_level();
	if !matches_policy {
		return Err(LevelSolvePrepareError::new(
			"exported_retained_build_policy_mismatch",
			format!(
				"retained operator {operator_name:?} artifact build differs from the \
				 workflow-derived source build"
			),
		));
	}
	Ok(())
}

#[derive(Clone, Debug)]
struct ValidatedJointExportShape {
	replacement_names: Vec<String>,
	deploy_action_index_by_name: BTreeMap<String, usize>,
	skill_action_index_by_name: BTreeMap<String, usize>,
}

fn validate_joint_exported_strategy_contract(
	source_seed: &MaaJobSeed,
	exported_seed: &MaaJobSeed,
	source_action_index: usize,
	source_operator_id: &str,
	artifact: &StrategyArtifact,
	resolved_builds: &BTreeMap<String, ResolvedOperatorBuild>,
) -> Result<ValidatedJointExportShape, LevelSolvePrepareError> {
	let source_actions: &[MaaJobAction] = source_seed.actions();
	let exported_actions: &[MaaJobAction] = exported_seed.actions();
	let source_daemon_index: usize = source_actions.len().checked_sub(1).ok_or_else(|| {
		LevelSolvePrepareError::new("source_guide_actions_empty", "source guide has no actions")
	})?;
	if !matches!(
		source_actions.get(source_daemon_index),
		Some(MaaJobAction::SkillDaemon { .. })
	) {
		return Err(LevelSolvePrepareError::new(
			"source_skill_daemon_not_final",
			"source guide must end with exactly one SkillDaemon",
		));
	}
	let source_operator_name: &str = match source_actions.get(source_action_index) {
		Some(MaaJobAction::Deploy { name, .. }) => name,
		_ => {
			return Err(LevelSolvePrepareError::new(
				"source_action_not_deploy",
				format!("source action {source_action_index} is not a deployment"),
			));
		}
	};
	let source_operator_index: usize = source_seed
		.operators()
		.iter()
		.position(|operator| operator.name() == source_operator_name)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"source_operator_missing",
				format!("source operator {source_operator_name:?} is absent from seed"),
			)
		})?;
	let replacement_count: usize = exported_seed
		.operators()
		.len()
		.checked_add(1)
		.and_then(|count| count.checked_sub(source_seed.operators().len()))
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_operator_delta_mismatch",
				"exported strategy removed more operators than the source strategy",
			)
		})?;
	if replacement_count == 0 {
		return Err(LevelSolvePrepareError::new(
			"empty_exported_replacement_plan",
			"exported strategy contains no source replacement",
		));
	}
	let expected_action_count: usize = source_actions
		.len()
		.checked_add(
			replacement_count
				.checked_mul(2)
				.and_then(|count| count.checked_sub(1))
				.ok_or_else(|| {
					LevelSolvePrepareError::new(
						"exported_action_count_overflow",
						"replacement action count overflowed",
					)
				})?,
		)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_action_count_overflow",
				"exported action count overflowed",
			)
		})?;
	if exported_actions.len() != expected_action_count {
		return Err(LevelSolvePrepareError::new(
			"exported_action_delta_mismatch",
			format!(
				"{replacement_count} replacements require {expected_action_count} actions, \
				 export has {}",
				exported_actions.len()
			),
		));
	}
	let replacement_operators = exported_seed
		.operators()
		.get(source_operator_index..source_operator_index + replacement_count)
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_replacement_operator_slice_missing",
				"replacement operator slice is outside the exported roster",
			)
		})?;
	let replacement_names: Vec<String> = replacement_operators
		.iter()
		.map(|operator| operator.name().to_owned())
		.collect();
	if replacement_names.iter().collect::<BTreeSet<_>>().len() != replacement_names.len() {
		return Err(LevelSolvePrepareError::new(
			"duplicate_exported_replacement",
			"replacement operator names must be unique",
		));
	}
	let replacement_operator_ids: Vec<&str> = replacement_names
		.iter()
		.map(|name| {
			resolved_builds
				.get(name)
				.map(ResolvedOperatorBuild::operator_id)
				.ok_or_else(|| {
					LevelSolvePrepareError::new(
						"artifact_operator_set_mismatch",
						format!("artifact has no exact build for replacement {name:?}"),
					)
				})
		})
		.collect::<Result<_, _>>()?;
	validate_source_operator_id_replaced(source_operator_id, &replacement_operator_ids)?;
	for operator in replacement_operators {
		if operator.skill_usage() != Some(1) || operator.skill_times().is_some() {
			return Err(LevelSolvePrepareError::new(
				"exported_source_operator_policy_mismatch",
				format!(
					"replacement operator {:?} must pin its first activation and enable the daemon",
					operator.name()
				),
			));
		}
	}
	for (source_index, source_operator) in source_seed.operators().iter().enumerate() {
		if source_index == source_operator_index {
			continue;
		}
		let exported_index: usize = if source_index < source_operator_index {
			source_index
		} else {
			source_index + replacement_count - 1
		};
		let exported_operator = &exported_seed.operators()[exported_index];
		if source_operator.name() != exported_operator.name()
			|| source_operator.skill() != exported_operator.skill()
			|| source_operator.skill_usage() != exported_operator.skill_usage()
			|| source_operator.skill_times() != exported_operator.skill_times()
		{
			return Err(LevelSolvePrepareError::new(
				"exported_retained_operator_changed",
				format!(
					"retained operator {:?} changed in export",
					source_operator.name()
				),
			));
		}
		validate_retained_operator_policy(
			exported_operator.name(),
			exported_operator.skill_usage(),
		)?;
	}

	let replacement_action_count: usize = replacement_count.checked_mul(2).ok_or_else(|| {
		LevelSolvePrepareError::new(
			"exported_action_count_overflow",
			"replacement action count overflowed",
		)
	})?;
	let replacement_action_end: usize = source_action_index
		.checked_add(replacement_action_count)
		.ok_or_else(|| {
		LevelSolvePrepareError::new(
			"exported_action_count_overflow",
			"replacement action range overflowed",
		)
	})?;
	let replacement_action_range = source_action_index..replacement_action_end;
	let retained_action_shift: usize = replacement_action_count - 1;
	let mut deploy_action_index_by_name: BTreeMap<String, usize> = BTreeMap::new();
	let mut skill_action_index_by_name: BTreeMap<String, usize> = BTreeMap::new();
	let mut next_deployment_index: usize = 0;
	for action_index in replacement_action_range.clone() {
		match &exported_actions[action_index] {
			MaaJobAction::Deploy {
				name, conditions, ..
			} => {
				let expected_name: &str =
					replacement_names
						.get(next_deployment_index)
						.ok_or_else(|| {
							LevelSolvePrepareError::new(
								"duplicate_exported_replacement_deploy",
								format!(
									"exported action {action_index} is an extra replacement Deploy"
								),
							)
						})?;
				if name != expected_name {
					return Err(LevelSolvePrepareError::new(
						"exported_replacement_deploy_order_mismatch",
						format!(
							"replacement operator {expected_name:?} has deploy action for {name:?}"
						),
					));
				}
				if action_index == source_action_index {
					let source_conditions = source_actions[source_action_index].conditions();
					if conditions != source_conditions {
						return Err(LevelSolvePrepareError::new(
							"exported_source_deploy_conditions_changed",
							"first replacement Deploy must preserve source conditions",
						));
					}
				} else {
					validate_no_timing_conditions(action_index, conditions)?;
				}
				if deploy_action_index_by_name
					.insert(name.clone(), action_index)
					.is_some()
				{
					return Err(LevelSolvePrepareError::new(
						"duplicate_exported_replacement_deploy",
						format!("replacement {name:?} has multiple Deploy actions"),
					));
				}
				next_deployment_index += 1;
			}
			MaaJobAction::Skill {
				name,
				location,
				conditions,
				..
			} => {
				let name: &str = name.as_deref().ok_or_else(|| {
					LevelSolvePrepareError::new(
						"invalid_explicit_skill_action",
						format!("exported Skill {action_index} is unnamed"),
					)
				})?;
				if !replacement_names
					.iter()
					.any(|replacement| replacement == name)
					|| location.is_some()
					|| !deploy_action_index_by_name.contains_key(name)
					|| skill_action_index_by_name
						.insert(name.to_owned(), action_index)
						.is_some()
				{
					return Err(LevelSolvePrepareError::new(
						"invalid_explicit_skill_action",
						format!(
							"Skill {action_index} must uniquely target an already deployed \
							 replacement without geometry"
						),
					));
				}
				validate_no_timing_conditions(action_index, conditions)?;
			}
			_ => {
				return Err(LevelSolvePrepareError::new(
					"invalid_mixed_replacement_action",
					format!("exported action {action_index} must be a replacement Deploy or Skill"),
				));
			}
		}
	}
	if deploy_action_index_by_name.len() != replacement_count
		|| skill_action_index_by_name.len() != replacement_count
	{
		return Err(LevelSolvePrepareError::new(
			"missing_explicit_replacement_action",
			"every replacement requires exactly one ordered Deploy and one explicit first Skill",
		));
	}
	for (source_index, source_action) in source_actions.iter().enumerate() {
		if source_index == source_action_index {
			continue;
		}
		let exported_index: usize = if source_index < source_action_index {
			source_index
		} else {
			source_index + retained_action_shift
		};
		if source_action != &exported_actions[exported_index] {
			return Err(LevelSolvePrepareError::new(
				"exported_retained_action_changed",
				format!(
					"exported action {exported_index} differs from retained source action \
					 {source_index}"
				),
			));
		}
	}
	for (index, action) in exported_actions.iter().enumerate() {
		match action {
			MaaJobAction::Deploy { timeout, .. } => {
				if timeout.is_some_and(|timeout| timeout.is_branching()) {
					return Err(LevelSolvePrepareError::new(
						"unsupported_export_action_condition",
						format!("Deploy action {index} has branching timeout"),
					));
				}
			}
			MaaJobAction::Skill {
				conditions,
				timeout: None | Some(MaaJobTimeout::Unlimited),
				..
			}
			| MaaJobAction::SpeedUp { conditions }
			| MaaJobAction::SkillDaemon { conditions } => {
				validate_no_timing_conditions(index, conditions)?;
			}
			MaaJobAction::Skill {
				timeout: Some(MaaJobTimeout::Milliseconds(_)),
				..
			} => {
				return Err(LevelSolvePrepareError::new(
					"unsupported_export_action_condition",
					format!("Skill action {index} has branching timeout"),
				));
			}
			_ => {
				return Err(LevelSolvePrepareError::new(
					"unsupported_export_action",
					format!("exported action {index} is outside the checkpoint subset"),
				));
			}
		}
	}
	validate_exported_build_set(exported_seed, artifact, resolved_builds)?;
	if artifact.actions.len() != exported_actions.len() {
		return Err(LevelSolvePrepareError::new(
			"artifact_action_count_mismatch",
			format!(
				"strategy has {} actions while artifact has {}",
				exported_actions.len(),
				artifact.actions.len()
			),
		));
	}
	for (index, (action, artifact_action)) in
		exported_actions.iter().zip(&artifact.actions).enumerate()
	{
		let (expected_kind, expected_operator_id): (MaterializedActionKind, Option<&str>) =
			match action {
				MaaJobAction::Deploy { name, .. } => (
					MaterializedActionKind::Deploy,
					Some(
						resolved_builds
							.get(name)
							.expect("validated exported build exists")
							.operator_id(),
					),
				),
				MaaJobAction::Skill { name, .. } => (
					MaterializedActionKind::Skill,
					Some(
						resolved_builds
							.get(name.as_deref().expect("validated skill is named"))
							.expect("validated exported build exists")
							.operator_id(),
					),
				),
				MaaJobAction::SpeedUp { .. } => (MaterializedActionKind::SpeedUp, None),
				MaaJobAction::SkillDaemon { .. } => (MaterializedActionKind::SkillDaemon, None),
				_ => unreachable!("checkpoint action subset was validated"),
			};
		if artifact_action.action_index != u32::try_from(index).unwrap_or(u32::MAX)
			|| artifact_action.kind != expected_kind
			|| artifact_action.operator_id.as_deref() != expected_operator_id
		{
			return Err(LevelSolvePrepareError::new(
				"artifact_action_shape_mismatch",
				format!(
					"artifact action {index} kind/operator {:?}/{:?}, expected {:?}/{:?}",
					artifact_action.kind,
					artifact_action.operator_id,
					expected_kind,
					expected_operator_id
				),
			));
		}
	}
	Ok(ValidatedJointExportShape {
		replacement_names,
		deploy_action_index_by_name,
		skill_action_index_by_name,
	})
}

fn validate_source_operator_id_replaced(
	source_operator_id: &str,
	replacement_operator_ids: &[&str],
) -> Result<(), LevelSolvePrepareError> {
	if replacement_operator_ids.contains(&source_operator_id) {
		return Err(LevelSolvePrepareError::new(
			"exported_source_operator_not_replaced",
			format!("replacement plan retains source operator ID {source_operator_id:?}"),
		));
	}
	Ok(())
}

fn validate_exported_build_set(
	exported_seed: &MaaJobSeed,
	artifact: &StrategyArtifact,
	resolved_builds: &BTreeMap<String, ResolvedOperatorBuild>,
) -> Result<(), LevelSolvePrepareError> {
	let exported_names: BTreeSet<&str> = exported_seed
		.operators()
		.iter()
		.map(MaaJobOperator::name)
		.collect();
	let build_names: BTreeSet<&str> = resolved_builds.keys().map(String::as_str).collect();
	if exported_names != build_names || artifact.builds.len() != exported_names.len() {
		return Err(LevelSolvePrepareError::new(
			"artifact_operator_set_mismatch",
			format!(
				"exported operators {:?} do not exactly match artifact builds {:?}",
				exported_names, build_names
			),
		));
	}
	for operator in exported_seed.operators() {
		let resolved: &ResolvedOperatorBuild =
			resolved_builds.get(operator.name()).ok_or_else(|| {
				LevelSolvePrepareError::new(
					"artifact_operator_set_mismatch",
					format!("artifact has no exact build for {:?}", operator.name()),
				)
			})?;
		let record: &ArtifactExactBuild = artifact
			.builds
			.iter()
			.find(|record| record.operator_id == resolved.operator_id())
			.expect("resolved build originated from artifact");
		if operator.skill() != Some(record.skill_slot)
			|| resolved.build().skill() != record.skill_slot
		{
			return Err(LevelSolvePrepareError::new(
				"artifact_selected_skill_mismatch",
				format!(
					"operator {:?} selects {:?}, artifact build selects {}",
					operator.name(),
					operator.skill(),
					record.skill_slot
				),
			));
		}
		validate_exact_maa_requirements(operator, record)?;
	}
	Ok(())
}

fn validate_no_timing_conditions(
	action_index: usize,
	conditions: &MaaActionConditions,
) -> Result<(), LevelSolvePrepareError> {
	let unsupported: Option<&'static str> = [
		("costs", conditions.costs().is_some()),
		("cost_changes", conditions.cost_changes().is_some()),
		("kills", conditions.kills().is_some()),
		("cooling", conditions.cooling().is_some()),
		("pre_delay", conditions.pre_delay().is_some()),
		("post_delay", conditions.post_delay().is_some()),
		("time_elapsed", conditions.time_elapsed().is_some()),
	]
	.into_iter()
	.find_map(|(field, present)| present.then_some(field));
	if let Some(field) = unsupported {
		return Err(LevelSolvePrepareError::new(
			"unsupported_export_action_condition",
			format!("exported action {action_index} has unsupported condition {field}"),
		));
	}
	Ok(())
}

fn validate_exact_maa_requirements(
	operator: &MaaJobOperator,
	record: &ArtifactExactBuild,
) -> Result<(), LevelSolvePrepareError> {
	let requirements: &MaaJobRequirements = operator.requirements().ok_or_else(|| {
		LevelSolvePrepareError::new(
			"maa_exact_requirements_mismatch",
			format!(
				"operator {:?} has no exact MAA requirements",
				operator.name()
			),
		)
	})?;
	let level: u16 = u16::try_from(record.level).map_err(|_| {
		LevelSolvePrepareError::new(
			"maa_exact_requirements_mismatch",
			format!(
				"artifact level {} does not fit MAA requirements",
				record.level
			),
		)
	})?;
	let module_matches: bool = match &record.module_id {
		None => {
			requirements.module().is_none()
				&& requirements.module_level().is_none()
				&& record.module_level == 0
		}
		Some(_) => false,
	};
	if requirements.elite() != Some(record.elite)
		|| requirements.level() != Some(level)
		|| requirements.potential() != Some(record.potential_rank)
		|| requirements.skill_level() != Some(record.skill_level)
		|| !module_matches
		|| record.trust_percent != 200
	{
		return Err(LevelSolvePrepareError::new(
			"maa_exact_requirements_mismatch",
			format!(
				"operator {:?} MAA requirements do not exactly match artifact build",
				operator.name()
			),
		));
	}
	Ok(())
}

fn materialize_validated_candidate(
	snapshot: &ArkDataSnapshot,
	stage: &CompiledStage,
	build: &ResolvedOperatorBuild,
	profile: &ExpectedDamageSkillProfile,
	anchor: DeploymentAnchor,
	deploy_tick: u64,
	active_spans: &[TickSpan],
) -> Result<Candidate, LevelSolvePrepareError> {
	let deployment_cell: GridPosition = anchor.cell;
	let stage_cell = stage.cell(deployment_cell).ok_or_else(|| {
		LevelSolvePrepareError::new(
			"missing_stage_cell",
			format!(
				"cell ({}, {}) is outside the stage",
				anchor.cell.row, anchor.cell.col
			),
		)
	})?;
	if !deployment_allowed(build.position(), stage_cell.buildable()) {
		return Err(LevelSolvePrepareError::new(
			"incompatible_deployment_cell",
			format!(
				"operator {:?} cannot deploy at ({}, {})",
				build.name(),
				anchor.cell.row,
				anchor.cell.col
			),
		));
	}
	let base_attack: NormalAttackProfile =
		attack_phase_normal_attack(profile, profile.base(), false)?;
	let active_attack: NormalAttackProfile =
		attack_phase_normal_attack(profile, profile.active(), true)?;
	let base_range = snapshot.resolve_range(build.range_id()).map_err(|error| {
		LevelSolvePrepareError::new("base_range_resolution_failed", error.to_string())
	})?;
	let skill_range = snapshot
		.resolve_range(profile.range().range_id())
		.map_err(|error| {
			LevelSolvePrepareError::new("skill_range_resolution_failed", error.to_string())
		})?;
	let base_attack_cells: BTreeSet<GridPosition> = project_attack_cells(
		deployment_cell,
		anchor.facing,
		base_range,
		stage.map_dimensions(),
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("base_range_projection_failed", error.to_string())
	})?;
	let skill_attack_cells: BTreeSet<GridPosition> = projected_skill_attack_cells(
		deployment_cell,
		anchor.facing,
		skill_range,
		stage.map_dimensions(),
		profile.range().forward_extension_tiles(),
	)?;
	let candidate: Candidate = Candidate::new(
		candidate_id(
			build.stable_id(),
			profile.skill_id(),
			0,
			deployment_cell,
			anchor.facing,
			deploy_tick,
			active_spans,
		),
		build.operator_id().to_owned(),
		build.name().to_owned(),
		build.deployment_cost(),
		deployment_cell,
		anchor.facing,
		base_attack_cells,
		base_attack,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("candidate_materialization_failed", error.to_string())
	})?;
	Ok(match profile.template() {
		SkillDamageMechanicTemplate::InstantPacket | SkillDamageMechanicTemplate::PeriodicAoe => {
			candidate.with_activation_triggered_active_attack(active_attack)
		}
		SkillDamageMechanicTemplate::ModifiedNormal
		| SkillDamageMechanicTemplate::AmmoModifiedNormal => candidate.with_active_attack(active_attack),
	}
	.with_active_attack_cells(skill_attack_cells))
}

const fn replay_status_from_artifact(status: ExpectedStrategyStatus) -> ExpectedReplayReportStatus {
	match status {
		ExpectedStrategyStatus::Unsupported => ExpectedReplayReportStatus::Unsupported,
		ExpectedStrategyStatus::ExpectedVictory => ExpectedReplayReportStatus::ExpectedVictory,
		ExpectedStrategyStatus::ExpectedDefeat => ExpectedReplayReportStatus::ExpectedDefeat,
		ExpectedStrategyStatus::Incomplete => ExpectedReplayReportStatus::Incomplete,
	}
}

fn ensure_export_validation_artifact_status(
	status: ExpectedStrategyStatus,
) -> Result<(), LevelSolvePrepareError> {
	if status == ExpectedStrategyStatus::ExpectedVictory {
		return Ok(());
	}
	Err(LevelSolvePrepareError::new(
		"artifact_battlefield_status_not_expected_victory",
		format!("export validation requires ExpectedVictory, artifact claims {status:?}"),
	))
}

fn ensure_exported_battlefield_victory(
	status: ExpectedStrategyStatus,
) -> Result<(), LevelSolvePrepareError> {
	if status == ExpectedStrategyStatus::ExpectedVictory {
		return Ok(());
	}
	Err(LevelSolvePrepareError::new(
		"exported_strategy_battlefield_not_victory",
		format!(
			"independent replay of the exported strategy reached {status:?}, not expected victory"
		),
	))
}

#[derive(Clone, Copy)]
struct ExportPlanningScope<'a> {
	model: &'a StageSpaceTimeModel,
	target_cohort_ids: &'a BTreeSet<String>,
	duty_window: TickSpan,
}

struct PreparedJobRequest<'a> {
	snapshot: &'a ArkDataSnapshot,
	prefabs: &'a BattlePrefabSnapshot,
	stage: &'a CompiledStage,
	source_guide: &'a MaaJob,
	source_guide_json: &'a str,
	source_guide_source_id: &'a str,
	requested_level: &'a str,
	source_seed: &'a MaaJobSeed,
	source_bindings: &'a [ExactReplayBuildBinding],
	source_slot_name: &'a str,
	guide_selection_budget: usize,
	candidates: &'a BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	joint_plan: &'a JointInsertionPlan,
	expected_damage: &'a ExpectedDamageJointEvidence,
	original_schedule: &'a PreparedGuideDeploymentSchedule,
	planning_model: &'a StageSpaceTimeModel,
	planning_target_cohort_ids: &'a BTreeSet<String>,
	planning_duty_window: TickSpan,
	proposal_index: usize,
	candidate_keys: &'a [String],
	tick_rate: TickRate,
}

fn prepare_damage_feasible_job(
	request: PreparedJobRequest<'_>,
) -> Result<ExpectedDamagePreparedJob, LevelSolvePrepareError> {
	let replacement_stream_end_tick: u64 = request
		.joint_plan
		.members
		.iter()
		.flat_map(|member| {
			std::iter::once(member.deploy_tick)
				.chain(member.active_spans.first().map(|span| span.start))
		})
		.max()
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"export_replacement_plan_empty",
				"joint replacement plan has no deployment or skill action",
			)
		})?;
	let members: Vec<(&JointInsertionMemberTiming, &MaterializedSkillCandidate)> = request
		.joint_plan
		.members
		.iter()
		.map(|member| {
			let candidate = request
				.candidates
				.get(&member.candidate_key)
				.ok_or_else(|| {
					LevelSolvePrepareError::new(
						"candidate_materialization_missing",
						format!(
							"candidate {} is absent from the export registry",
							member.candidate_key
						),
					)
				})?;
			let resolved: ResolvedOperatorBuild = request
				.snapshot
				.operators()
				.resolve(&candidate.exact_build)
				.map_err(|error| {
					LevelSolvePrepareError::new(
						"export_candidate_build_resolution_failed",
						error.to_string(),
					)
				})?;
			ensure_manual_export_activation(resolved.selected_skill().activation_type())?;
			if member.active_spans.is_empty() {
				return Err(LevelSolvePrepareError::new(
					"export_requires_first_activation",
					format!(
						"candidate {} has no explicit first activation",
						member.candidate_key
					),
				));
			}
			if let Some(second_span) = member.active_spans.get(1) {
				if second_span.start < replacement_stream_end_tick {
					return Err(LevelSolvePrepareError::new(
						"export_recurring_skill_before_daemon",
						format!(
							"candidate {} recurs at tick {} before the mixed replacement stream \
							 can start SkillDaemon at tick {replacement_stream_end_tick}",
							member.candidate_key, second_span.start
						),
					));
				}
			}
			let chosen: u64 = member.active_spans[0].start;
			let earliest: u64 = member.earliest_legal_activation_tick.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"export_skill_never_ready",
					format!(
						"candidate {} has no legal activation in the duty window",
						member.candidate_key
					),
				)
			})?;
			if chosen != earliest {
				return Err(LevelSolvePrepareError::new(
					"export_activation_not_earliest",
					format!(
						"candidate {} activates at {chosen}, earliest legal tick is {earliest}",
						member.candidate_key
					),
				));
			}
			Ok((member, candidate))
		})
		.collect::<Result<_, LevelSolvePrepareError>>()?;
	let replacements: Vec<MaaReplacementOperator> = members
		.iter()
		.map(|(_, candidate)| {
			Ok(MaaReplacementOperator {
				name: candidate.candidate.operator_name().to_owned(),
				skill: candidate.exact_build.skill(),
				requirements: maa_exact_requirements(&candidate.exact_build)?,
			})
		})
		.collect::<Result<_, LevelSolvePrepareError>>()?;
	let scheduled_by_key: BTreeMap<&str, &ScheduledDeploymentDpAction> = request
		.joint_plan
		.dp_schedule
		.iter()
		.map(|scheduled| (scheduled.action.stable_key.as_str(), scheduled))
		.collect();
	let mut indexed_replacement_actions: Vec<(usize, MaaReplacementAction<'_>)> =
		Vec::with_capacity(members.len() * 2);
	for (member_index, (member, candidate)) in members.iter().enumerate() {
		let stable_key: String = format!("replacement:{}", member.candidate_key);
		let scheduled = scheduled_by_key.get(stable_key.as_str()).ok_or_else(|| {
			LevelSolvePrepareError::new(
				"export_deployment_schedule_missing",
				format!("candidate {} has no joint DP event", member.candidate_key),
			)
		})?;
		indexed_replacement_actions.extend([
			(
				member_index,
				MaaReplacementAction {
					operator_name: candidate.candidate.operator_name(),
					kind: MaaReplacementActionKind::Deploy {
						cell: candidate.candidate.deployment_cell(),
						direction: maa_direction(candidate.candidate.facing()),
					},
					chosen_tick: member.deploy_tick,
					earliest_legal_tick: scheduled.deploy_tick,
				},
			),
			(
				member_index,
				MaaReplacementAction {
					operator_name: candidate.candidate.operator_name(),
					kind: MaaReplacementActionKind::Skill,
					chosen_tick: member.active_spans[0].start,
					earliest_legal_tick: member
						.earliest_legal_activation_tick
						.expect("validated member has an earliest activation"),
				},
			),
		]);
	}
	indexed_replacement_actions
		.sort_by_key(|(member_index, action)| replacement_action_sort_key(*member_index, action));
	let replacement_actions: Vec<MaaReplacementAction<'_>> = indexed_replacement_actions
		.into_iter()
		.map(|(_, action)| action)
		.collect();
	let retained_exact_requirements: BTreeMap<String, MaaExactRequirements> = request
		.source_bindings
		.iter()
		.filter(|binding| binding.source_name != request.source_slot_name)
		.map(|binding| {
			Ok((
				binding.source_name.clone(),
				maa_exact_requirements(&binding.build)?,
			))
		})
		.collect::<Result<_, LevelSolvePrepareError>>()?;
	let maa_job: MaaJob = compile_maa_job(MaaJobCompileRequest {
		seed: request.source_seed,
		metadata: MaaJobExportMetadata {
			minimum_required: request.source_guide.minimum_required(),
			title: request.source_guide.title(),
			details: request.source_guide.details(),
		},
		source_slot_name: request.source_slot_name,
		replacements: &replacements,
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: request.stage.map_dimensions(),
	})
	.map_err(|error| LevelSolvePrepareError::new("maa_job_export_failed", error.to_string()))?;
	let maa_job_json: String = maa_job.to_deterministic_json().map_err(|error| {
		LevelSolvePrepareError::new("strategy_serialization_failed", error.to_string())
	})?;
	let strategy_digest: [u8; 32] = Sha256::digest(maa_job_json.as_bytes()).into();
	let strategy_sha256: String = hex_digest(&strategy_digest);

	let exported_guide = parse_maa_job(&maa_job_json).map_err(|error| {
		LevelSolvePrepareError::new("export_round_trip_failed", error.to_string())
	})?;
	let exported_lineage = MaaJobSourceLineage::new("ark-solver-export", &strategy_sha256)
		.map_err(|error| LevelSolvePrepareError::new("export_lineage_failed", error.to_string()))?;
	let exported_seed: MaaJobSeed = exported_guide
		.executable_guide_seeds(exported_lineage, request.source_seed.difficulty(), 1)
		.map_err(|error| LevelSolvePrepareError::new("export_seed_failed", error.to_string()))?
		.next()
		.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"export_seed_missing",
				"round-tripped strategy produced no guide seed",
			)
		})?
		.materialize();
	let mut exported_bindings: Vec<ExactReplayBuildBinding> = Vec::with_capacity(
		request
			.source_bindings
			.len()
			.checked_sub(1)
			.and_then(|count| count.checked_add(members.len()))
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"export_binding_count_overflow",
					"replacement binding count overflowed",
				)
			})?,
	);
	for binding in request.source_bindings {
		if binding.source_name != request.source_slot_name {
			exported_bindings.push(binding.clone());
			continue;
		}
		for (_, candidate) in &members {
			exported_bindings.push(ExactReplayBuildBinding {
				source_name: candidate.candidate.operator_name().to_owned(),
				build: candidate.exact_build.clone(),
				skill_usage: 1,
				skill_times: None,
			});
		}
	}
	let prepared_guide: PreparedReplayGuide =
		prepare_replay_guide(request.snapshot, &exported_seed, &exported_bindings).map_err(
			|error| {
				LevelSolvePrepareError::new("export_guide_preparation_failed", error.to_string())
			},
		)?;
	let replay_input = materialize_expected_replay_input(
		request.snapshot,
		request.prefabs,
		ExpectedReplayCompileInput {
			stage_name: exported_seed.stage_name(),
			difficulty: replay_stage_difficulty(exported_seed.difficulty()).map_err(|error| {
				LevelSolvePrepareError::new("export_difficulty_mapping_failed", error.to_string())
			})?,
			bindings: &exported_bindings,
			guide: &prepared_guide,
			tick_rate: request.tick_rate,
		},
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("export_replay_materialization_failed", error.to_string())
	})?;
	let candidate_keys: Vec<String> = request.candidate_keys.to_vec();
	let mut replay_output =
		execute_expected_replay(&replay_input, strategy_digest, &strategy_sha256);
	ensure_exported_battlefield_victory(replay_output.artifact.battlefield_status)?;
	let (estimated_success_probability_millionths, probability_basis): (
		u32,
		ExpectedDamageProbabilityBasis,
	) = verified_expected_victory_job_probability(
		&replay_output.report.status,
		replay_output.report.defensive_probability.as_ref(),
	)
	.ok_or_else(|| {
		LevelSolvePrepareError::new(
			"exported_strategy_probability_unverified",
			"independent full-battle replay did not produce a verified positive success \
				 probability",
		)
	})?;
	rewrite_joint_export_artifact(
		&mut replay_output.artifact,
		&exported_seed,
		request.original_schedule,
		request.joint_plan,
		ExportPlanningScope {
			model: request.planning_model,
			target_cohort_ids: request.planning_target_cohort_ids,
			duty_window: request.planning_duty_window,
		},
		request.candidates,
		request.expected_damage,
	)?;
	let artifact_bytes: Vec<u8> = replay_output.artifact.encode().map_err(|error| {
		LevelSolvePrepareError::new("artifact_encoding_failed", error.to_string())
	})?;
	let artifact_id: String = format!("{:x}", Sha256::digest(&artifact_bytes));

	let validation: ExpectedDamageExportValidationReport =
		validate_export(ExportValidationRequest {
			snapshot: request.snapshot,
			prefabs: request.prefabs,
			requested_level: request.requested_level,
			source_guide_bytes: request.source_guide_json.as_bytes(),
			source_guide_source_id: request.source_guide_source_id,
			maa_job_bytes: maa_job_json.as_bytes(),
			artifact_bytes: &artifact_bytes,
			difficulty: request.source_seed.difficulty(),
			guide_selection_budget: request.guide_selection_budget,
		})?;
	if validation.damage_status != DeterministicDamageStatus::Feasible {
		return Err(LevelSolvePrepareError::new(
			"exported_strategy_damage_infeasible",
			"the independently round-tripped strategy did not clear its expected-damage obligation",
		));
	}
	if validation.artifact_id != artifact_id {
		return Err(LevelSolvePrepareError::new(
			"artifact_id_changed_during_validation",
			"independent validation did not preserve the canonical artifact identity",
		));
	}

	Ok(ExpectedDamagePreparedJob {
		schema_version: EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION,
		data_revision: request.snapshot.resolved_revision().to_owned(),
		mechanics_revision: request.prefabs.mechanics_revision().to_owned(),
		proposal_index: request.proposal_index,
		candidate_keys,
		estimated_success_probability_millionths,
		probability_basis,
		strategy_sha256,
		artifact_id,
		maa_job,
		exact_builds: replay_output.artifact.builds.clone(),
		dp: validation.dp,
		expected_damage: validation.expected_damage,
		battlefield_status: replay_status_from_artifact(replay_output.artifact.battlefield_status),
		maa_job_json,
		artifact: replay_output.artifact,
	})
}

fn replacement_action_sort_key(
	member_index: usize,
	action: &MaaReplacementAction<'_>,
) -> (u64, u8, usize) {
	(
		action.chosen_tick,
		u8::from(matches!(action.kind, MaaReplacementActionKind::Skill)),
		member_index,
	)
}

const fn maa_direction(facing: Facing) -> MaaJobDirection {
	match facing {
		Facing::Up => MaaJobDirection::Up,
		Facing::Right => MaaJobDirection::Right,
		Facing::Down => MaaJobDirection::Down,
		Facing::Left => MaaJobDirection::Left,
	}
}

fn maa_exact_requirements(
	build: &OperatorBuild,
) -> Result<MaaExactRequirements, LevelSolvePrepareError> {
	let level: u16 = u16::try_from(build.level()).map_err(|_| {
		LevelSolvePrepareError::new(
			"build_level_overflow",
			format!(
				"build level {} does not fit the MAA job file format",
				build.level()
			),
		)
	})?;
	let module: Option<i8> = match build.module() {
		OperatorModule::None => None,
		OperatorModule::Id(module_id) => {
			return Err(LevelSolvePrepareError::new(
				"module_export_unsupported",
				format!("module {module_id:?} has no stable MAA job file numeric identifier"),
			));
		}
	};
	Ok(MaaExactRequirements {
		elite: build.elite(),
		level,
		potential: build.potential().number(),
		skill_level: build.skill_level(),
		module,
		module_level: build.module_level(),
	})
}

fn rewrite_joint_export_artifact(
	artifact: &mut StrategyArtifact,
	exported_seed: &MaaJobSeed,
	original_schedule: &PreparedGuideDeploymentSchedule,
	joint_plan: &JointInsertionPlan,
	planning: ExportPlanningScope<'_>,
	candidates: &BTreeMap<ContributionCandidateKey, MaterializedSkillCandidate>,
	joint: &ExpectedDamageJointEvidence,
) -> Result<(), LevelSolvePrepareError> {
	validate_joint_planning_scope(
		joint,
		planning.model,
		planning.target_cohort_ids,
		planning.duty_window,
	)?;
	let mut deployment_ticks_by_operator: BTreeMap<String, u64> = original_schedule
		.deployments
		.iter()
		.filter(|deployment| !deployment.action.is_source_slot)
		.map(|deployment| {
			let deploy_tick: u64 = joint_plan
				.retained_deploy_ticks
				.get(&deployment.action.action_index)
				.copied()
				.expect("joint plan contains every retained deployment");
			(
				deployment.resolved_build.operator_id().to_owned(),
				deploy_tick,
			)
		})
		.collect();
	let mut activation_ticks_by_operator: BTreeMap<String, u64> = BTreeMap::new();
	for member in &joint_plan.members {
		let candidate = candidates.get(&member.candidate_key).ok_or_else(|| {
			LevelSolvePrepareError::new(
				"candidate_materialization_missing",
				format!(
					"candidate {} is absent from artifact rewrite",
					member.candidate_key
				),
			)
		})?;
		deployment_ticks_by_operator.insert(
			candidate.candidate.operator_id().to_owned(),
			member.deploy_tick,
		);
		activation_ticks_by_operator.insert(
			candidate.candidate.operator_id().to_owned(),
			member
				.active_spans
				.first()
				.expect("exported candidate has one activation")
				.start,
		);
	}
	for action in &mut artifact.actions {
		let action_index: usize = usize::try_from(action.action_index).map_err(|_| {
			LevelSolvePrepareError::new(
				"artifact_action_index_overflow",
				"artifact action index does not fit usize",
			)
		})?;
		match action.kind {
			MaterializedActionKind::Deploy => {
				let operator_id: &str = action.operator_id.as_deref().ok_or_else(|| {
					LevelSolvePrepareError::new(
						"artifact_deployment_operator_missing",
						format!("exported deploy action {action_index} has no operator"),
					)
				})?;
				let expected_tick: u64 = *deployment_ticks_by_operator
					.get(operator_id)
					.ok_or_else(|| {
						LevelSolvePrepareError::new(
							"export_deployment_schedule_missing",
							format!(
								"no joint DP schedule exists for exported operator {operator_id:?}"
							),
						)
					})?;
				if action.scheduled_tick != expected_tick {
					return Err(LevelSolvePrepareError::new(
						"artifact_deployment_tick_mismatch",
						format!(
							"operator {operator_id:?} replayed at {}, planner scheduled \
							 {expected_tick}",
							action.scheduled_tick
						),
					));
				}
			}
			MaterializedActionKind::Skill => {
				let operator_id: &str = action.operator_id.as_deref().ok_or_else(|| {
					LevelSolvePrepareError::new(
						"artifact_skill_operator_missing",
						format!("exported skill action {action_index} has no operator"),
					)
				})?;
				let expected_tick: u64 = *activation_ticks_by_operator
					.get(operator_id)
					.ok_or_else(|| {
						LevelSolvePrepareError::new(
							"export_activation_schedule_missing",
							format!(
								"no activation schedule exists for exported operator \
								 {operator_id:?}"
							),
						)
					})?;
				if action.scheduled_tick != expected_tick {
					return Err(LevelSolvePrepareError::new(
						"artifact_activation_tick_mismatch",
						format!(
							"operator {operator_id:?} replayed skill at {}, planner scheduled \
							 {expected_tick}",
							action.scheduled_tick
						),
					));
				}
			}
			MaterializedActionKind::SpeedUp | MaterializedActionKind::SkillDaemon => {}
			MaterializedActionKind::Retreat | MaterializedActionKind::Other => {
				return Err(LevelSolvePrepareError::new(
					"unsupported_export_action",
					format!(
						"exported action {action_index} has unsupported kind {:?}",
						action.kind
					),
				));
			}
		}
	}
	if artifact.actions.len() != exported_seed.actions().len() {
		return Err(LevelSolvePrepareError::new(
			"artifact_action_count_mismatch",
			format!(
				"artifact has {} actions while exported strategy has {}",
				artifact.actions.len(),
				exported_seed.actions().len()
			),
		));
	}

	// The full replay already populated battlefield HP, deaths, leaks, and expected-damage
	// status with blocking and survival included. `joint` is an intentionally incomplete
	// unblocked planning audit; it must never overwrite the authoritative replay artifact.
	Ok(())
}

fn validate_joint_planning_scope(
	joint: &ExpectedDamageJointEvidence,
	model: &StageSpaceTimeModel,
	target_cohort_ids: &BTreeSet<String>,
	duty_window: TickSpan,
) -> Result<(), LevelSolvePrepareError> {
	let expected_targets: BTreeSet<_> = model
		.enemies
		.iter()
		.filter(|enemy| target_cohort_ids.contains(enemy.instance_id.cohort.as_str()))
		.map(|enemy| enemy.instance_id.clone())
		.collect();
	let actual_targets: BTreeSet<_> = joint
		.targets
		.iter()
		.filter(|target| target.is_duty_target)
		.map(|target| target.target.clone())
		.collect();
	if actual_targets != expected_targets {
		return Err(LevelSolvePrepareError::new(
			"artifact_planning_scope_mismatch",
			format!(
				"joint evidence duty targets {:?} do not match full planning targets {:?}",
				actual_targets, expected_targets
			),
		));
	}
	if joint.targets.iter().any(|target| {
		target.is_duty_target
			&& target
				.deadline_tick
				.is_none_or(|deadline| deadline < duty_window.start || deadline > duty_window.end)
	}) {
		return Err(LevelSolvePrepareError::new(
			"artifact_planning_window_mismatch",
			format!(
				"joint evidence contains a duty deadline outside [{}, {}]",
				duty_window.start, duty_window.end
			),
		));
	}
	Ok(())
}

fn dp_audit(schedule: &PreparedGuideDeploymentSchedule) -> ExpectedDamageDpAudit {
	ExpectedDamageDpAudit {
		rules: ExpectedDamageDpRulesAudit {
			initial_dp: schedule.dp_rules.initial_dp,
			max_dp: schedule.dp_rules.max_dp,
			regen_interval_ticks: schedule.dp_rules.regen_interval_ticks,
			dp_per_interval: schedule.dp_rules.dp_per_interval,
		},
		entries: schedule
			.deployments
			.iter()
			.map(|deployment| ExpectedDamageDpEntryAudit {
				action_index: deployment.action.action_index,
				operator_name: deployment.action.operator_name.clone(),
				deploy_tick: deployment.dp_schedule.deploy_tick,
				deployment_cost: deployment.dp_schedule.action.cost,
				regenerated_dp: deployment.dp_schedule.regenerated_dp,
				granted_dp: deployment.dp_schedule.granted_dp,
				dp_before_deployment: deployment.dp_schedule.dp_before_deployment,
				dp_after_deployment: deployment.dp_schedule.dp_after_deployment,
			})
			.collect(),
	}
}

fn hex_digest(digest: &[u8; 32]) -> String {
	digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Serialize)]
struct JointPlanMemberIdentity<'a> {
	candidate_key: &'a str,
	deploy_tick: u64,
	active_spans: &'a [TickSpan],
}

#[derive(Serialize)]
struct JointPlanIdentity<'a> {
	members: Vec<JointPlanMemberIdentity<'a>>,
	retained_deploy_ticks: &'a BTreeMap<usize, u64>,
}

fn joint_plan_variant_id(plan: &JointInsertionPlan) -> Result<String, LevelSolvePrepareError> {
	let identity = JointPlanIdentity {
		members: plan
			.members
			.iter()
			.map(|member| JointPlanMemberIdentity {
				candidate_key: member.candidate_key.as_str(),
				deploy_tick: member.deploy_tick,
				active_spans: &member.active_spans,
			})
			.collect(),
		retained_deploy_ticks: &plan.retained_deploy_ticks,
	};
	let encoded: Vec<u8> = serde_json::to_vec(&identity).map_err(|error| {
		LevelSolvePrepareError::new(
			"candidate_identity_serialization_failed",
			format!("failed to encode exact joint plan identity: {error}"),
		)
	})?;
	let mut hasher: Sha256 = Sha256::new();
	hasher.update(b"ark-solver:joint-plan:v1:");
	hasher.update(encoded);
	let digest: [u8; 32] = hasher.finalize().into();
	Ok(format!("joint:{}", hex_digest(&digest)))
}

fn guide_base_panel_build_pool(
	snapshot: &ArkDataSnapshot,
	seed: &MaaJobGuideSeed<'_>,
) -> Result<CandidatePoolSpec, LevelSolvePrepareError> {
	let mut builds_by_operator: BTreeMap<String, BasePanelBuildSpec> = BTreeMap::new();
	for hint in seed.build_hints() {
		let skill: u8 = hint.operator().skill().unwrap_or(1);
		let (resolved, _) = resolve_no_module_max_build(snapshot, hint.operator_name(), skill)?;
		builds_by_operator
			.entry(resolved.operator_id().to_owned())
			.or_insert_with(|| BasePanelBuildSpec {
				operator_id: resolved.operator_id().to_owned(),
				elite: resolved.build().elite(),
				level: resolved.build().level(),
			});
	}
	Ok(CandidatePoolSpec::Roster {
		builds: builds_by_operator.into_values().collect(),
	})
}

/// Exact deployment costs for every guide action, plus the DP grants the retained ones bring.
///
/// The source slot contributes no grants: the joint scheduler replaces it with candidates whose
/// own skills planning does not schedule, so the export equality check stays their guard.
fn guide_exact_action_overrides(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	domain: &GuideDeploymentDomain,
	tick_rate: TickRate,
) -> Result<GuideDeploymentActionOverrides, LevelSolvePrepareError> {
	let registry: BattlePrefabHandlerRegistry = built_in_battle_prefab_handler_registry();
	let mut costs: BTreeMap<usize, u32> = BTreeMap::new();
	let mut grants: BTreeMap<usize, Vec<PlannedDeploymentDpGrant>> = BTreeMap::new();
	for action in &domain.actions {
		let skill: u8 = action.skill.ok_or_else(|| {
			LevelSolvePrepareError::new(
				"guide_skill_missing",
				format!(
					"guide action {} operator {:?} has no selected skill",
					action.action_index, action.operator_name
				),
			)
		})?;
		let (resolved, _) = resolve_no_module_max_build(snapshot, &action.operator_name, skill)?;
		costs.insert(action.action_index, resolved.deployment_cost());
		if action.is_source_slot {
			continue;
		}
		let planned: Vec<PlannedDeploymentDpGrant> =
			planned_dp_grants(snapshot, prefabs, &registry, &resolved, tick_rate);
		if !planned.is_empty() {
			grants.insert(action.action_index, planned);
		}
	}
	Ok(GuideDeploymentActionOverrides {
		deployment_cost_by_action: costs,
		dp_grants_by_action: grants,
	})
}

const COMMON_SKILLS_LOGICAL_PATH: &str = "[uc]skills";
const COMMON_SKILLS_ASSET_DIRECTORY: &str = "dyn/battle/prefabs/[uc]skills";

/// The deployment-point grants one retained operator credits on its own once deployed.
///
/// Only an automatic skill qualifies. A manual skill casts only when something casts it, which in
/// a retained slot is nothing, and whose timing planning does not model in any case. A skill whose
/// common prefab does not decode, or whose reviewed charge-cost owner does not claim the grant,
/// stays out of the schedule; replay then diverges and the export equality check rejects it.
fn planned_dp_grants(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	registry: &BattlePrefabHandlerRegistry,
	resolved: &ResolvedOperatorBuild,
	tick_rate: TickRate,
) -> Vec<PlannedDeploymentDpGrant> {
	let selected_skill = resolved.selected_skill();
	if selected_skill.activation_type() != &SkillActivationType::Auto {
		return Vec::new();
	}
	let Some(prefab_id) = selected_skill.prefab_id() else {
		return Vec::new();
	};
	let Ok(graph) = interpret_battle_prefab_asset(
		prefabs,
		COMMON_SKILLS_LOGICAL_PATH,
		&format!("{COMMON_SKILLS_ASSET_DIRECTORY}/{prefab_id}.prefab"),
		BattlePrefabRootSection::MainAsset,
		prefab_id,
		registry,
	) else {
		return Vec::new();
	};
	let Ok(Some(decoded)) = try_decode_charge_cost_skill_template(
		&graph,
		selected_skill,
		snapshot,
		snapshot.resolved_revision(),
	) else {
		return Vec::new();
	};
	let Ok(cadences) = decoded.grant_cadences(selected_skill, tick_rate) else {
		return Vec::new();
	};
	cadences
		.into_iter()
		.map(|cadence: ChargeCostGrantCadence| {
			PlannedDeploymentDpGrant::new(
				cadence.first_offset_ticks,
				cadence.period_ticks,
				u64::from(cadence.amount),
			)
		})
		.collect()
}

fn guide_no_module_replay_bindings(
	snapshot: &ArkDataSnapshot,
	seed: &MaaJobSeed,
) -> Result<Vec<ExactReplayBuildBinding>, LevelSolvePrepareError> {
	seed.operators()
		.iter()
		.map(|operator| {
			let skill: u8 = operator.skill().ok_or_else(|| {
				LevelSolvePrepareError::new(
					"guide_skill_missing",
					format!("guide operator {:?} has no selected skill", operator.name()),
				)
			})?;
			let (resolved, _) = resolve_no_module_max_build(snapshot, operator.name(), skill)?;
			Ok(ExactReplayBuildBinding {
				source_name: operator.name().to_owned(),
				build: resolved.build().clone(),
				skill_usage: operator.skill_usage().unwrap_or(0),
				skill_times: operator.skill_times(),
			})
		})
		.collect()
}

fn resolve_no_module_max_build(
	snapshot: &ArkDataSnapshot,
	operator: &str,
	skill: u8,
) -> Result<(ResolvedOperatorBuild, Vec<String>), LevelSolvePrepareError> {
	let maximal: Vec<OperatorBuild> = snapshot
		.operators()
		.maximal_builds(operator, skill)
		.map_err(|error| {
			LevelSolvePrepareError::new(
				"maximal_build_resolution_failed",
				format!("{operator:?} skill {skill}: {error}"),
			)
		})?;
	let template: &OperatorBuild = maximal.first().ok_or_else(|| {
		LevelSolvePrepareError::new(
			"no_maximal_build",
			format!("{operator:?} skill {skill} has no maximal build"),
		)
	})?;
	let no_module: OperatorBuild = OperatorBuild::new(
		template.identity().clone(),
		template.elite(),
		template.level(),
		template.trust_percent(),
		template.skill(),
		template.skill_level(),
		template.potential(),
		OperatorModule::None,
		0,
	);
	let resolved: ResolvedOperatorBuild =
		snapshot.operators().resolve(&no_module).map_err(|error| {
			LevelSolvePrepareError::new(
				"no_module_build_resolution_failed",
				format!("{operator:?} skill {skill}: {error}"),
			)
		})?;
	let module_variants: Vec<String> = maximal
		.iter()
		.filter_map(|build| match build.module() {
			OperatorModule::None => None,
			OperatorModule::Id(module_id) => Some(module_id.clone()),
		})
		.collect();
	Ok((resolved, module_variants))
}

fn roster_entries(roster: Option<&LevelSolveRoster>) -> Vec<LevelSolveRosterEntry> {
	let mut entries: Vec<LevelSolveRosterEntry> = match roster {
		Some(roster) => roster.operators.clone(),
		None => crate::mechanics::compile::REVIEWED_SKILL_DAMAGE_SELECTIONS
			.iter()
			.map(|selection| LevelSolveRosterEntry {
				operator: selection.operator_id.to_owned(),
				skill: selection.skill,
			})
			.collect(),
	};
	entries.sort_by(|left, right| {
		left.operator
			.cmp(&right.operator)
			.then_with(|| left.skill.cmp(&right.skill))
	});
	entries
}

fn attack_phase_normal_attack(
	profile: &ExpectedDamageSkillProfile,
	phase: &ExpectedAttackPhase,
	is_active: bool,
) -> Result<NormalAttackProfile, LevelSolvePrepareError> {
	if !matches!(phase.delivery(), ExpectedDamageDelivery::Direct) {
		return Err(LevelSolvePrepareError::new(
			"unsupported_damage_delivery",
			format!("skill {} uses {:?}", profile.skill_id(), phase.delivery()),
		));
	}
	let adjustment = phase.defense_adjustment();
	if adjustment.physical_defense_reduction_ratio() != 0.0
		|| adjustment.physical_defense_reduction_fixed() != 0.0
		|| adjustment.arts_resistance_reduction_ratio() != 0.0
		|| adjustment.arts_resistance_reduction_fixed() != 0.0
	{
		return Err(LevelSolvePrepareError::new(
			"unsupported_defense_adjustment",
			format!(
				"skill {} modifies defenses and cannot be represented by a static damage packet",
				profile.skill_id()
			),
		));
	}
	let damage_type: NormalAttackDamageType = match phase.damage_type() {
		ExpectedDamageType::Physical => NormalAttackDamageType::Physical,
		ExpectedDamageType::Arts => NormalAttackDamageType::Arts,
		ExpectedDamageType::True => NormalAttackDamageType::True,
	};
	let target_filter: NormalAttackTargetFilter = match phase.target_filter() {
		ExpectedTargetFilter::GroundAndAir => NormalAttackTargetFilter::GroundAndAir,
		ExpectedTargetFilter::GroundOnly => NormalAttackTargetFilter::GroundOnly,
		ExpectedTargetFilter::AirOnly => NormalAttackTargetFilter::AirOnly,
	};
	let packets: Vec<LockedTargetDamagePacket> =
		expected_damage_packets(profile.skill_id(), phase, damage_type)?;
	let first_attack_delay: u64 = match (is_active, profile.template()) {
		(
			true,
			SkillDamageMechanicTemplate::InstantPacket | SkillDamageMechanicTemplate::PeriodicAoe,
		) => 0,
		_ => phase.attack_interval_ticks().get(),
	};
	NormalAttackProfile::new(
		phase.maximum_primary_targets().get(),
		packets,
		first_attack_delay,
		phase.attack_interval_ticks().get(),
		target_filter,
	)
	.map_err(|error| {
		LevelSolvePrepareError::new("skill_profile_conversion_failed", error.to_string())
	})
}

fn expected_damage_packets(
	skill_id: &str,
	phase: &ExpectedAttackPhase,
	damage_type: NormalAttackDamageType,
) -> Result<Vec<LockedTargetDamagePacket>, LevelSolvePrepareError> {
	let source_outcomes = phase.primary_hit_outcomes().outcomes();
	if source_outcomes.is_empty() {
		return Err(LevelSolvePrepareError::new(
			"empty_expected_damage_mixture",
			format!("skill {skill_id} has no raw-damage outcomes"),
		));
	}
	let probability_sum: f64 = source_outcomes
		.iter()
		.map(|outcome| outcome.probability())
		.sum();
	if !probability_sum.is_finite() || (probability_sum - 1.0).abs() > 1.0e-9 {
		return Err(LevelSolvePrepareError::new(
			"invalid_expected_damage_probability_sum",
			format!("skill {skill_id} outcome probabilities sum to {probability_sum}"),
		));
	}

	// Quantize cumulative probability rather than each outcome independently. Prefix rounding
	// bounds quantization error while the final outcome receives the exact remaining mass.
	let mut cumulative_probability: f64 = 0.0;
	let mut previous_quantized: u32 = 0;
	let mut packet_outcomes: Vec<PacketExpectedRawDamageOutcome> =
		Vec::with_capacity(source_outcomes.len());
	for (index, outcome) in source_outcomes.iter().enumerate() {
		let probability: f64 = outcome.probability();
		if !probability.is_finite() || probability <= 0.0 || probability > 1.0 {
			return Err(LevelSolvePrepareError::new(
				"invalid_expected_damage_probability",
				format!("skill {skill_id} outcome {index} has invalid probability {probability}"),
			));
		}
		cumulative_probability += probability;
		let cumulative_quantized: u32 = if index + 1 == source_outcomes.len() {
			EXPECTED_DAMAGE_PROBABILITY_SCALE
		} else {
			let scaled: f64 =
				(cumulative_probability * f64::from(EXPECTED_DAMAGE_PROBABILITY_SCALE)).round();
			if !(0.0..=f64::from(EXPECTED_DAMAGE_PROBABILITY_SCALE)).contains(&scaled) {
				return Err(LevelSolvePrepareError::new(
					"expected_damage_probability_overflow",
					format!("skill {skill_id} cumulative outcome probability overflowed"),
				));
			}
			scaled as u32
		};
		let probability_millionths: u32 = cumulative_quantized
			.checked_sub(previous_quantized)
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"expected_damage_probability_order",
					format!("skill {skill_id} outcome probabilities are not cumulative"),
				)
			})?;
		if probability_millionths == 0 {
			return Err(LevelSolvePrepareError::new(
				"expected_damage_probability_below_resolution",
				format!(
					"skill {skill_id} outcome {index} is below the one-millionth probability \
					 resolution"
				),
			));
		}
		let raw_damage: FixedDamage =
			FixedDamage::try_from_f64(outcome.raw_damage()).map_err(|error| {
				LevelSolvePrepareError::new(
					"invalid_expected_skill_damage",
					format!("skill {skill_id} outcome {index}: {error}"),
				)
			})?;
		packet_outcomes.push(
			PacketExpectedRawDamageOutcome::new(probability_millionths, raw_damage).map_err(
				|error| {
					LevelSolvePrepareError::new(
						"expected_damage_outcome_conversion_failed",
						format!("skill {skill_id} outcome {index}: {error}"),
					)
				},
			)?,
		);
		previous_quantized = cumulative_quantized;
	}
	let packet: LockedTargetDamagePacket =
		LockedTargetDamagePacket::from_expected_outcomes(damage_type, packet_outcomes).map_err(
			|error| {
				LevelSolvePrepareError::new(
					"expected_damage_packet_conversion_failed",
					format!("skill {skill_id}: {error}"),
				)
			},
		)?;
	let hit_count: usize = usize::try_from(phase.primary_hit_count().get()).map_err(|_| {
		LevelSolvePrepareError::new(
			"skill_hit_count_overflow",
			format!(
				"skill {skill_id} hit count {} does not fit usize",
				phase.primary_hit_count()
			),
		)
	})?;
	Ok(vec![packet; hit_count])
}

fn projected_skill_attack_cells(
	anchor: GridPosition,
	facing: Facing,
	range: &crate::data::AttackRange,
	dimensions: crate::data::MapDimensions,
	forward_extension_tiles: u32,
) -> Result<BTreeSet<GridPosition>, LevelSolvePrepareError> {
	let mut cells: BTreeSet<GridPosition> = project_attack_cells(anchor, facing, range, dimensions)
		.map_err(|error| {
			LevelSolvePrepareError::new("skill_range_projection_failed", error.to_string())
		})?;
	if forward_extension_tiles == 0 {
		return Ok(cells);
	}
	let mut extension_offsets: Vec<RangeOffset> = Vec::new();
	for offset in range.offsets() {
		for extension in 1..=forward_extension_tiles {
			let extension: i32 = i32::try_from(extension).map_err(|_| {
				LevelSolvePrepareError::new(
					"skill_range_extension_overflow",
					"forward skill-range extension does not fit i32",
				)
			})?;
			let col: i32 = offset.col().checked_add(extension).ok_or_else(|| {
				LevelSolvePrepareError::new(
					"skill_range_extension_overflow",
					"forward skill-range offset overflowed",
				)
			})?;
			extension_offsets.push(RangeOffset::new(offset.row(), col));
		}
	}
	cells.extend(project_offsets(
		anchor,
		facing,
		&extension_offsets,
		dimensions,
	));
	Ok(cells)
}

fn active_duration_ticks(
	timing: &ExpectedDamageActivationTiming,
) -> Result<Option<u64>, LevelSolvePrepareError> {
	match timing.duration {
		ExpectedSkillDuration::Instant => Ok(Some(1)),
		ExpectedSkillDuration::Timed { ticks } => Ok(Some(ticks.get())),
		ExpectedSkillDuration::Ammo { attacks } => {
			// Ammo-modified normal attacks retain the normal first delay of one interval. An
			// N-shot span must therefore include ticks interval, 2*interval, ..., N*interval.
			let intervals: u64 = u64::from(attacks.get());
			let last_attack_offset: u64 = timing
				.active_attack_interval_ticks
				.get()
				.checked_mul(intervals)
				.ok_or_else(|| {
					LevelSolvePrepareError::new(
						"skill_duration_overflow",
						"ammo skill duration overflowed",
					)
				})?;
			Ok(Some(last_attack_offset.saturating_add(1)))
		}
		ExpectedSkillDuration::Indefinite => Ok(None),
	}
}

fn include_manual_base_only_schedule(
	activation_type: &SkillActivationType,
	schedules: &mut Vec<Vec<TickSpan>>,
) {
	if matches!(activation_type, SkillActivationType::Manual)
		&& !schedules.iter().any(Vec::is_empty)
	{
		schedules.insert(0, Vec::new());
	}
}

fn build_activation_schedule(
	timing: &ExpectedDamageActivationTiming,
	first_activation: u64,
	duty_window: TickSpan,
	recurring_recovery_ticks: Option<u64>,
) -> Result<Vec<TickSpan>, LevelSolvePrepareError> {
	let active_ticks: Option<u64> = active_duration_ticks(timing)?;
	let mut schedule: Vec<TickSpan> = Vec::new();
	let mut activation: u64 = first_activation;
	for _ in 0..MAX_ACTIVATIONS_PER_SCHEDULE {
		if activation >= duty_window.end {
			return Ok(schedule);
		}
		let end: u64 = match active_ticks {
			Some(ticks) => activation.checked_add(ticks).ok_or_else(|| {
				LevelSolvePrepareError::new(
					"activation_tick_overflow",
					"skill active span overflowed",
				)
			})?,
			None => duty_window.end,
		};
		if end > duty_window.start {
			schedule.push(TickSpan {
				start: activation,
				end: end.min(duty_window.end),
			});
		}
		if matches!(timing.activation_type, SkillActivationType::Passive)
			|| active_ticks.is_none()
			|| recurring_recovery_ticks.is_none()
		{
			return Ok(schedule);
		}
		let recharge_ticks: u64 =
			recurring_recovery_ticks.expect("checked recurring recovery above");
		activation = end.checked_add(recharge_ticks).ok_or_else(|| {
			LevelSolvePrepareError::new(
				"activation_tick_overflow",
				"next skill activation overflowed",
			)
		})?;
	}
	Err(LevelSolvePrepareError::new(
		"too_many_skill_activations",
		format!(
			"skill schedule exceeded {MAX_ACTIVATIONS_PER_SCHEDULE} activations in the duty window"
		),
	))
}

fn periodic_sp_sources(
	timing: &ExpectedDamageActivationTiming,
	ticks_per_second: NonZeroU32,
) -> Result<Vec<PeriodicSpSource>, LevelSolvePrepareError> {
	if !timing.sp_increment.is_finite() || timing.sp_increment <= 0.0 {
		return Err(LevelSolvePrepareError::new(
			"invalid_sp_increment",
			format!(
				"SP increment must be positive and finite, found {}",
				timing.sp_increment
			),
		));
	}
	let base_amount: FixedDamage =
		FixedDamage::try_from_f64(timing.sp_increment).map_err(|_| {
			LevelSolvePrepareError::new(
				"invalid_sp_increment",
				format!(
					"SP increment {} cannot be fixed-point normalized",
					timing.sp_increment
				),
			)
		})?;
	if base_amount.is_zero() {
		return Err(LevelSolvePrepareError::new(
			"invalid_sp_increment",
			format!(
				"SP increment {} rounds to zero at millionth precision",
				timing.sp_increment
			),
		));
	}
	let mut sources: Vec<PeriodicSpSource> = timing.periodic_sp_sources.clone();
	sources.push(PeriodicSpSource {
		amount: ExpectedValue::from_scaled(base_amount.scaled()),
		interval_ticks: u64::from(ticks_per_second.get()),
	});
	Ok(sources)
}

fn periodic_sp_recovery_ticks(
	sp: u32,
	sources: &[PeriodicSpSource],
	ticks_per_second: NonZeroU32,
) -> Result<u64, LevelSolvePrepareError> {
	if sp == 0 {
		return Ok(0);
	}
	let pulse_interval: u64 = u64::from(ticks_per_second.get());
	let mut amount_per_pulse: u128 = 0;
	for source in sources {
		if source.interval_ticks != pulse_interval {
			return Err(LevelSolvePrepareError::new(
				"unsupported_periodic_sp_interval",
				format!(
					"periodic SP interval {} does not match the modeled one-second pulse {pulse_interval}",
					source.interval_ticks
				),
			));
		}
		amount_per_pulse = amount_per_pulse
			.checked_add(u128::from(source.amount.scaled()))
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"activation_tick_overflow",
					"periodic SP amount overflowed",
				)
			})?;
	}
	if amount_per_pulse == 0 {
		return Err(LevelSolvePrepareError::new(
			"unsupported_sp_recovery",
			"no positive modeled periodic SP source can make the skill ready",
		));
	}
	let required: u128 = u128::from(sp)
		.checked_mul(u128::from(ExpectedValue::SCALE))
		.ok_or_else(|| {
			LevelSolvePrepareError::new("activation_tick_overflow", "required SP amount overflowed")
		})?;
	let pulses: u128 = required.div_ceil(amount_per_pulse);
	let ticks: u128 = pulses
		.checked_mul(u128::from(pulse_interval))
		.ok_or_else(|| {
			LevelSolvePrepareError::new("activation_tick_overflow", "SP recovery ticks overflowed")
		})?;
	if ticks > u128::from(u64::MAX) {
		return Err(LevelSolvePrepareError::new(
			"activation_tick_overflow",
			"SP recovery interval overflowed",
		));
	}
	Ok(ticks as u64)
}

#[derive(Clone, Debug)]
struct RetainedWaveLocalHpGroupBatch {
	wave_local_ids: Vec<WaveLocalHpGroupId>,
	absolute_groups: Vec<SpaceTimeHpGroup>,
	duty_window: TickSpan,
}

fn wave_local_hp_group_identity(group_id: &WaveLocalHpGroupId) -> String {
	let anchor: String = match &group_id.anchor {
		StageTimeAnchor::BattleStart => "battle".to_owned(),
		StageTimeAnchor::WaveStart { wave } => format!("wave:{wave}"),
		StageTimeAnchor::WaveFragmentStart { wave, fragment } => {
			format!("wave:{wave}/fragment:{fragment}")
		}
		StageTimeAnchor::BranchPhaseStart { branch, phase } => {
			format!("branch:{}:{branch}/phase:{phase}", branch.len())
		}
	};
	format!(
		"causal/{anchor}/goal_offset:{}",
		group_id.goal_deadline_offset_ticks
	)
}

fn wave_local_hp_group_constraint(
	group_id: &WaveLocalHpGroupId,
) -> Result<ContributionConstraintKey, LevelSolvePrepareError> {
	ContributionConstraintKey::space_time_hp_group(wave_local_hp_group_identity(group_id))
		.map_err(|error| LevelSolvePrepareError::new("hp_group_identity_failed", error.to_string()))
}

fn wave_local_hp_group_demands(
	groups: &[WaveLocalHpGroup],
) -> Result<Vec<ContributionDemand>, LevelSolvePrepareError> {
	groups
		.iter()
		.map(|group| {
			Ok(ContributionDemand {
				constraint: wave_local_hp_group_constraint(&group.id)?,
				required: group.required_hp,
			})
		})
		.collect()
}

fn wave_local_hp_group_contributions(
	score: &OptimisticWaveLocalReachabilityScore,
) -> Result<BTreeMap<ContributionConstraintKey, FixedDamage>, LevelSolvePrepareError> {
	score
		.capacity_by_group
		.iter()
		.map(|(group_id, &capacity)| Ok((wave_local_hp_group_constraint(group_id)?, capacity)))
		.collect()
}

fn retained_damage_ranking_score(
	groups: &[WaveLocalHpGroup],
	contributions: &BTreeMap<ContributionConstraintKey, FixedDamage>,
) -> Result<u64, LevelSolvePrepareError> {
	let mut score: u128 = 0;
	for group in groups {
		let constraint: ContributionConstraintKey = wave_local_hp_group_constraint(&group.id)?;
		let capacity: FixedDamage = contributions.get(&constraint).copied().ok_or_else(|| {
			LevelSolvePrepareError::new(
				"retained_ranking_contribution_missing",
				format!("retained ranking has no contribution for {constraint}"),
			)
		})?;
		let required: u64 = group.required_hp.scaled();
		if required == 0 {
			continue;
		}
		let normalized: u128 = u128::from(capacity.scaled().min(required))
			.checked_mul(u128::from(EXPECTED_DAMAGE_PROBABILITY_SCALE))
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"retained_ranking_score_overflow",
					"retained ranking normalization overflowed",
				)
			})? / u128::from(required);
		score = score.checked_add(normalized).ok_or_else(|| {
			LevelSolvePrepareError::new(
				"retained_ranking_score_overflow",
				"retained ranking sum overflowed",
			)
		})?;
	}
	u64::try_from(score).map_err(|_| {
		LevelSolvePrepareError::new(
			"retained_ranking_score_overflow",
			"retained ranking score does not fit u64",
		)
	})
}

fn project_retained_wave_local_hp_groups(
	model: &StageSpaceTimeModel,
	groups: &[WaveLocalHpGroup],
) -> Result<Vec<RetainedWaveLocalHpGroupBatch>, LevelSolvePrepareError> {
	let anchor_ticks: BTreeMap<&StageTimeAnchor, u64> = model
		.bound_anchors
		.iter()
		.map(|binding| (&binding.anchor, binding.tick))
		.collect();
	let mut batches: Vec<RetainedWaveLocalHpGroupBatch> = Vec::new();
	for group in groups {
		let anchor_tick: u64 = anchor_ticks.get(&group.id.anchor).copied().ok_or_else(|| {
			LevelSolvePrepareError::new(
				"wave_local_ranking_anchor_missing",
				format!(
					"retained witness has no absolute binding for {:?}",
					group.id.anchor
				),
			)
		})?;
		let deadline_tick: u64 = anchor_tick
			.checked_add(group.id.goal_deadline_offset_ticks)
			.ok_or_else(|| {
				LevelSolvePrepareError::new(
					"wave_local_ranking_deadline_overflow",
					format!(
						"anchor {anchor_tick} plus goal offset {} overflowed",
						group.id.goal_deadline_offset_ticks
					),
				)
			})?;
		let absolute_group: SpaceTimeHpGroup = SpaceTimeHpGroup {
			id: HpGroupId::new(deadline_tick),
			deadline_tick,
			members: group.members.clone(),
			required_hp: group.required_hp,
		};
		let batch: &mut RetainedWaveLocalHpGroupBatch = if let Some(index) =
			batches.iter().position(|batch| {
				batch.duty_window.start == anchor_tick
					&& !batch
						.absolute_groups
						.iter()
						.any(|existing| existing.id == absolute_group.id)
			}) {
			&mut batches[index]
		} else {
			batches.push(RetainedWaveLocalHpGroupBatch {
				wave_local_ids: Vec::new(),
				absolute_groups: Vec::new(),
				duty_window: TickSpan {
					start: anchor_tick,
					end: deadline_tick,
				},
			});
			batches
				.last_mut()
				.expect("a retained ranking group batch was just inserted")
		};
		batch.duty_window.end = batch.duty_window.end.max(deadline_tick);
		batch.wave_local_ids.push(group.id.clone());
		batch.absolute_groups.push(absolute_group);
	}
	Ok(batches)
}

fn prepare_retained_wave_local_hp_group_scorers<'candidate>(
	model: &StageSpaceTimeModel,
	candidate: &'candidate Candidate,
	batches: &[RetainedWaveLocalHpGroupBatch],
) -> Result<Vec<PreparedOptimisticHpGroupCapacityScorer<'candidate>>, LevelSolvePrepareError> {
	batches
		.iter()
		.map(|batch| {
			prepare_optimistic_hp_group_capacity(OptimisticHpGroupCapacityPrepareRequest {
				model,
				candidate,
				hp_groups: &batch.absolute_groups,
				duty_window: batch.duty_window,
			})
			.map_err(|error| {
				LevelSolvePrepareError::new(
					"candidate_hp_group_capacity_prepare_failed",
					error.to_string(),
				)
			})
		})
		.collect()
}

fn retained_wave_local_hp_group_contributions(
	deploy_tick: u64,
	active_spans: &[TickSpan],
	batches: &[RetainedWaveLocalHpGroupBatch],
	scorers: &[PreparedOptimisticHpGroupCapacityScorer<'_>],
) -> Result<BTreeMap<ContributionConstraintKey, FixedDamage>, LevelSolvePrepareError> {
	if batches.len() != scorers.len() {
		return Err(LevelSolvePrepareError::new(
			"retained_ranking_scorer_count_mismatch",
			format!(
				"{} retained ranking batches have {} prepared scorers",
				batches.len(),
				scorers.len()
			),
		));
	}
	let mut contributions: BTreeMap<ContributionConstraintKey, FixedDamage> = BTreeMap::new();
	for (batch, scorer) in batches.iter().zip(scorers) {
		let score: OptimisticHpGroupScore =
			scorer.score(deploy_tick, active_spans).map_err(|error| {
				LevelSolvePrepareError::new("candidate_hp_group_capacity_failed", error.to_string())
			})?;
		for (wave_local_id, absolute_group) in
			batch.wave_local_ids.iter().zip(&batch.absolute_groups)
		{
			let capacity: FixedDamage = score.capacity_by_group[&absolute_group.id];
			let constraint: ContributionConstraintKey =
				wave_local_hp_group_constraint(wave_local_id)?;
			if contributions.insert(constraint.clone(), capacity).is_some() {
				return Err(LevelSolvePrepareError::new(
					"duplicate_wave_local_ranking_group",
					format!("duplicate contribution constraint {constraint}"),
				));
			}
		}
	}
	Ok(contributions)
}

fn candidate_activation_boundaries(
	model: &StageSpaceTimeModel,
	base_attack_cells: &BTreeSet<GridPosition>,
	skill_attack_cells: &BTreeSet<GridPosition>,
	duty_window: TickSpan,
) -> Vec<u64> {
	let mut boundaries: BTreeSet<u64> = model
		.bound_anchors
		.iter()
		.map(|binding| binding.tick)
		.filter(|tick| duty_window.start <= *tick && *tick < duty_window.end)
		.collect();
	boundaries.insert(duty_window.start);
	for enemy in &model.enemies {
		let mut intersects_candidate_range: bool = false;
		for occupancy in &enemy.occupancy {
			if !base_attack_cells.contains(&occupancy.cell)
				&& !skill_attack_cells.contains(&occupancy.cell)
			{
				continue;
			}
			intersects_candidate_range = true;
			for tick in [occupancy.enter_tick, occupancy.exit_tick] {
				if duty_window.start <= tick && tick < duty_window.end {
					boundaries.insert(tick);
				}
			}
		}
		if intersects_candidate_range {
			for tick in [enemy.spawn_tick, enemy.goal_deadline_tick] {
				if duty_window.start <= tick && tick < duty_window.end {
					boundaries.insert(tick);
				}
			}
		}
	}
	boundaries.into_iter().collect()
}

fn candidate_id(
	stable_build_id: &str,
	skill_id: &str,
	search_priority: u64,
	cell: GridPosition,
	facing: Facing,
	deploy_tick: u64,
	active_spans: &[TickSpan],
) -> String {
	let activation_key: String = active_spans
		.iter()
		.map(|span| format!("{:020}-{:020}", span.start, span.end))
		.collect::<Vec<_>>()
		.join(",");
	format!(
		"{stable_build_id}#skill={skill_id}#priority={search_priority:020}#cell={},{}\
		 #facing={facing:?}#deploy={deploy_tick:020}\
		 #active={activation_key}",
		cell.row, cell.col
	)
}

fn candidate_search_priority(
	source_anchor: &DeploymentAnchor,
	cell: GridPosition,
	facing: Facing,
) -> u64 {
	let source_cell: GridPosition = source_anchor.cell;
	let distance: u64 = u64::from(source_cell.row.abs_diff(cell.row))
		.saturating_add(u64::from(source_cell.col.abs_diff(cell.col)));
	distance
		.saturating_mul(2)
		.saturating_add(u64::from(facing != source_anchor.facing))
}

fn exclusion(
	candidate_key: Option<String>,
	operator_id: Option<String>,
	reason_code: impl Into<String>,
	detail: impl Into<String>,
) -> ExpectedDamageCandidateExclusion {
	ExpectedDamageCandidateExclusion {
		candidate_key,
		operator_id,
		operator_name: None,
		reason_code: reason_code.into(),
		detail: detail.into(),
	}
}

const fn verified_search_status(
	verified_job_count: usize,
	requested_top_k: usize,
	solve_status: ExpectedDamageLevelSolveStatus,
) -> ExpectedDamageVerifiedSearchStatus {
	if verified_job_count >= requested_top_k {
		ExpectedDamageVerifiedSearchStatus::TopKFilled
	} else {
		match solve_status {
			ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached => {
				ExpectedDamageVerifiedSearchStatus::ProposalBudgetExhausted
			}
			ExpectedDamageLevelSolveStatus::PlanningModelIncomplete => {
				ExpectedDamageVerifiedSearchStatus::PlanningModelIncomplete
			}
			ExpectedDamageLevelSolveStatus::CandidateMaterializationIncomplete => {
				ExpectedDamageVerifiedSearchStatus::CandidateMaterializationIncomplete
			}
			ExpectedDamageLevelSolveStatus::AdditiveInfeasible => {
				ExpectedDamageVerifiedSearchStatus::AdditiveInfeasible
			}
			ExpectedDamageLevelSolveStatus::AdditiveSearchExhausted => {
				ExpectedDamageVerifiedSearchStatus::AdditiveSearchExhausted
			}
			ExpectedDamageLevelSolveStatus::ProposalVisitorStopped => {
				ExpectedDamageVerifiedSearchStatus::ProposalStreamCancelled
			}
		}
	}
}

fn verified_expected_victory_probability(
	status: &ExpectedReplayReportStatus,
	probability: Option<&ReplayDefensiveProbabilityAudit>,
) -> Option<f64> {
	if !matches!(status, ExpectedReplayReportStatus::ExpectedVictory) {
		return None;
	}
	let probability: &ReplayDefensiveProbabilityAudit = probability?;
	if !probability.verified {
		return None;
	}
	probability
		.success_probability
		.filter(|success_probability| *success_probability > 0.0)
}

fn verified_expected_victory_job_probability(
	status: &ExpectedReplayReportStatus,
	probability: Option<&ReplayDefensiveProbabilityAudit>,
) -> Option<(u32, ExpectedDamageProbabilityBasis)> {
	verified_expected_victory_probability(status, probability)?;
	probability.map(|audit| {
		(
			audit.success_probability_millionths,
			ExpectedDamageProbabilityBasis::FullBattlefieldReplay,
		)
	})
}

fn validate_replayed_deployment_tick(
	operator_name: &str,
	action_index: usize,
	scheduled_tick: u64,
	replayed_tick: Option<u64>,
) -> Result<(), LevelSolvePrepareError> {
	let replayed_tick: u64 = replayed_tick.ok_or_else(|| {
		LevelSolvePrepareError::new(
			"strategy_deployment_not_executed",
			format!("operator {operator_name:?} deployment action {action_index} did not execute"),
		)
	})?;
	if replayed_tick != scheduled_tick {
		return Err(LevelSolvePrepareError::new(
			"strategy_deployment_dp_tick_mismatch",
			format!(
				"operator {operator_name:?} replayed at {replayed_tick}, DP schedule computed \
				 {scheduled_tick}"
			),
		));
	}
	Ok(())
}

fn validate_exported_activation_timing(
	replacement_name: &str,
	replayed_first_tick: u64,
	active_spans: &[TickSpan],
	daemon_tick: u64,
) -> Result<(), LevelSolvePrepareError> {
	let earliest_activation_tick: u64 =
		active_spans.first().map(|span| span.start).ok_or_else(|| {
			LevelSolvePrepareError::new(
				"exported_skill_never_ready",
				format!(
					"replacement {replacement_name:?} has no legal activation in the duty window"
				),
			)
		})?;
	if replayed_first_tick != earliest_activation_tick {
		return Err(LevelSolvePrepareError::new(
			"exported_activation_not_earliest",
			format!(
				"replacement {replacement_name:?} activates at {replayed_first_tick}, earliest \
				 legal tick is {earliest_activation_tick}"
			),
		));
	}
	if let Some(second_span) = active_spans.get(1) {
		if second_span.start < daemon_tick {
			return Err(LevelSolvePrepareError::new(
				"exported_recurring_skill_before_daemon",
				format!(
					"replacement {replacement_name:?} recurs at tick {} before SkillDaemon \
					 executes at tick {daemon_tick}",
					second_span.start
				),
			));
		}
	}
	Ok(())
}

fn validate_retained_operator_policy(
	operator_name: &str,
	skill_usage: Option<u8>,
) -> Result<(), LevelSolvePrepareError> {
	if skill_usage.unwrap_or(0) == 0 {
		return Ok(());
	}
	Err(LevelSolvePrepareError::new(
		"exported_retained_operator_policy_mismatch",
		format!(
			"retained operator {operator_name:?} has unmodeled daemon skill usage {skill_usage:?}"
		),
	))
}

fn ensure_manual_export_activation(
	activation_type: &SkillActivationType,
) -> Result<(), LevelSolvePrepareError> {
	if matches!(activation_type, SkillActivationType::Manual) {
		return Ok(());
	}
	Err(LevelSolvePrepareError::new(
		"exported_skill_activation_not_manual",
		format!("explicit MAA Skill actions require a manual skill, found {activation_type:?}"),
	))
}

#[cfg(test)]
#[path = "tests/main_story_01_01_dp_grants.rs"]
mod main_story_01_01_dp_grants_tests;

#[cfg(test)]
mod tests {
	use std::{
		collections::{BTreeMap, BTreeSet},
		sync::atomic::{AtomicBool, Ordering},
		time::Instant,
	};

	use crate::data::{
		GridPosition, OperatorBuild, OperatorIdentity, OperatorModule, PotentialRank,
		SkillActivationType, StageDifficulty,
	};
	use sha2::{Digest, Sha256};

	use super::{
		acquire_replay_inflight_credit, bounded_rotated_orders, complete_replay_work,
		ensure_export_validation_artifact_status, ensure_exported_battlefield_victory,
		ensure_manual_export_activation, hex_digest, include_manual_base_only_schedule,
		joint_plan_variant_id, prepare_retained_wave_local_hp_group_scorers,
		project_retained_wave_local_hp_groups, release_replay_inflight_credit,
		replacement_action_sort_key, replay_inflight_credits, replay_variant_outcome,
		require_matching_stage_identity, retained_damage_ranking_score,
		retained_wave_local_hp_group_contributions, send_replay_work, source_guide_provenance,
		truncate_solve_after_verified_top_k, validate_exported_activation_timing,
		validate_replayed_deployment_tick, validate_retained_artifact_build_record,
		validate_retained_operator_policy, validate_source_operator_id_replaced,
		verified_expected_victory_job_probability, verified_expected_victory_probability,
		verified_search_status, wave_local_hp_group_demands, ArtifactExactBuild,
		CanonicalStageIdentity, ExpectedDamageProbabilityBasis, ExpectedDamageVerifiedSearchStatus,
		JointExpectedDamageEvaluator, JointInsertionMemberTiming, JointInsertionPlan,
		LevelSolvePrepareError, LevelSolveReplayVariantOutcome, OrderedReplayCoordinator,
		ReplayAttempt, ReplayWork, MAX_BOUNDED_JOINT_INSERTION_ORDERS,
	};
	use crate::maa_job::{
		parse_maa_job, MaaJobDifficulty, MaaJobDirection, MaaJobSourceLineage,
		MaaReplacementAction, MaaReplacementActionKind,
	};
	use crate::mechanics::{
		domain::{
			Facing, FixedDamage, LockedTargetDamagePacket, NormalAttackDamageType,
			NormalAttackProfile, NormalAttackTargetFilter,
		},
		stage::{SpawnCohortId, SpawnInstanceId},
	};
	use crate::planning::{
		solve_independent_selection_cardinality_round_robin, CellOccupancy,
		ContributionCandidateKey, ContributionOperatorIdentity, ExpectedDamageLevelSolveReport,
		ExpectedDamageLevelSolveStatus, IndependentContributionCandidate,
		IndependentSelectionRequest, ResolvedCoverageCandidate, ResolvedCoverageDeployment,
		StageSpaceTimeModel, StageTimeAnchor, StageTimeBinding, TickSpan, TrajectoryClass,
		WaveLocalHpGroup, WaveLocalHpGroupId,
	};

	#[test]
	fn exported_strategy_must_resolve_to_the_source_stage() {
		let source: CanonicalStageIdentity<'_> = CanonicalStageIdentity {
			stage_id: "stage_source",
			level_id: "level_source",
			difficulty: StageDifficulty::Normal,
		};
		require_matching_stage_identity(source, source)
			.expect("the same canonical stage identity must be accepted");

		let exported: CanonicalStageIdentity<'_> = CanonicalStageIdentity {
			stage_id: "stage_other",
			level_id: "level_other",
			difficulty: StageDifficulty::Normal,
		};
		let error: LevelSolvePrepareError = require_matching_stage_identity(source, exported)
			.expect_err("a different exported stage must fail before replay");

		assert_eq!(error.code(), "exported_strategy_stage_mismatch");
		assert!(error.detail().contains("stage_other"));
		assert!(error.detail().contains("stage_source"));
	}

	#[test]
	fn exact_source_guide_provenance_is_derived_without_local_snapshots() {
		let exact_guide: &[u8] =
			include_bytes!("../../tests/fixtures/maa_job_bd7_exact_sanitized.json");
		let guide_json: &str =
			std::str::from_utf8(exact_guide).expect("tracked exact guide must be UTF-8");
		let guide = parse_maa_job(guide_json).expect("tracked exact guide must import");
		let digest_bytes: [u8; 32] = Sha256::digest(exact_guide).into();
		let digest: String = hex_digest(&digest_bytes);
		let lineage = MaaJobSourceLineage::new("fixture:bd7-exact", &digest)
			.expect("tracked source provenance must be valid");
		let mut seeds = guide
			.executable_guide_seeds(lineage, MaaJobDifficulty::Normal, 10)
			.expect("tracked Normal guide selection must be executable");
		let seed = seeds
			.next()
			.expect("tracked exact guide must have one selection");
		let provenance = source_guide_provenance(&seed, 0)
			.expect("tracked exact guide must have one fixed source deployment");

		assert!(seeds.next().is_none());
		assert!(!seeds.report().truncated());
		assert_eq!(provenance.source_id, "fixture:bd7-exact");
		assert_eq!(provenance.content_sha256, digest);
		assert_eq!(provenance.difficulty, MaaJobDifficulty::Normal);
		assert_eq!(provenance.selection_index, 0);
		assert_eq!(
			provenance.selected_slots,
			BTreeMap::from([
				("奶盾".to_owned(), "黍".to_owned()),
				("圣聆初雪".to_owned(), "圣聆初雪".to_owned()),
			])
		);
		assert_eq!(provenance.source_action_index, 1);
		assert_eq!(provenance.source_operator_name, "圣聆初雪");
	}

	#[test]
	fn worker_panic_preserves_sequence_as_a_typed_failure() {
		let message =
			complete_replay_work(7, 11, Instant::now(), || panic!("synthetic worker failure"));

		assert_eq!(message.sequence, 7);
		assert_eq!(message.proposal_index, 11);
		let Err(error) = message.result else {
			panic!("panic must become a typed result");
		};
		assert_eq!(error.code(), "replay_worker_panicked");
		assert!(error.detail().contains("sequence 7"));
	}

	use crate::replay::{
		ExpectedReplayAuditReport, ExpectedReplayReportStatus, ReplayConservativeOmissionEntry,
		ReplayDefensiveProbabilityAudit, ReplayEnemyAudit, EXPECTED_REPLAY_REPORT_SCHEMA_VERSION,
	};
	use crate::ExpectedStrategyStatus;

	#[test]
	fn out_of_order_replay_results_commit_only_in_proposal_sequence() {
		let mut coordinator: OrderedReplayCoordinator<&str> = OrderedReplayCoordinator::new(1);
		assert!(coordinator.push(2, "two").unwrap().is_empty());
		assert!(coordinator.push(1, "one").unwrap().is_empty());
		assert_eq!(
			coordinator.push(0, "zero").unwrap(),
			vec!["zero", "one", "two"]
		);
	}

	#[test]
	fn replay_inflight_credits_bound_uncommitted_work() {
		let cancellation: AtomicBool = AtomicBool::new(false);
		let (sender, receiver) = replay_inflight_credits(2);

		assert!(acquire_replay_inflight_credit(&receiver, &cancellation));
		assert!(acquire_replay_inflight_credit(&receiver, &cancellation));
		cancellation.store(true, Ordering::Release);
		assert!(!acquire_replay_inflight_credit(&receiver, &cancellation));

		cancellation.store(false, Ordering::Release);
		release_replay_inflight_credit(&sender).expect("ordered commit returns one credit");
		assert!(acquire_replay_inflight_credit(&receiver, &cancellation));
	}

	#[test]
	fn full_replay_queue_observes_cancellation_without_spinning() {
		let cancellation: AtomicBool = AtomicBool::new(false);
		let (sender, _receiver) = crossbeam_channel::bounded(1);
		sender
			.send(ReplayWork {
				sequence: 0,
				proposal_index: 0,
				candidate_keys: Vec::new(),
			})
			.unwrap();
		cancellation.store(true, Ordering::Release);

		assert!(!send_replay_work(
			&sender,
			&cancellation,
			ReplayWork {
				sequence: 1,
				proposal_index: 1,
				candidate_keys: Vec::new(),
			},
		));
	}

	#[test]
	fn verified_top_k_normalizes_nested_solve_status() {
		let mut solve = ExpectedDamageLevelSolveReport {
			schema_version: 0,
			status: ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached,
			residual_demands: Vec::new(),
			candidate_exclusions: Vec::new(),
			proposals: Vec::new(),
			damage_feasible_proposal_indices: Vec::new(),
			replay_candidate_indices: Vec::new(),
		};

		truncate_solve_after_verified_top_k(&mut solve, 0);

		assert_eq!(
			solve.status,
			ExpectedDamageLevelSolveStatus::ProposalVisitorStopped
		);
	}

	#[test]
	fn replay_pipeline_types_remain_thread_safe() {
		fn assert_send<T: Send>() {}
		fn assert_sync<T: Sync>() {}

		assert_sync::<crate::data::ArkDataSnapshot>();
		assert_sync::<JointExpectedDamageEvaluator>();
		assert_send::<ReplayWork>();
		assert_send::<ReplayAttempt>();
	}

	#[test]
	fn manual_skill_candidates_always_include_a_base_only_variant() {
		let active: TickSpan = TickSpan { start: 10, end: 20 };
		let mut manual_schedules: Vec<Vec<TickSpan>> = vec![vec![active]];
		include_manual_base_only_schedule(&SkillActivationType::Manual, &mut manual_schedules);
		assert_eq!(manual_schedules, vec![Vec::new(), vec![active]]);

		let mut auto_schedules: Vec<Vec<TickSpan>> = vec![vec![active]];
		include_manual_base_only_schedule(&SkillActivationType::Auto, &mut auto_schedules);
		assert_eq!(auto_schedules, vec![vec![active]]);
	}

	#[test]
	fn manual_skill_schedule_outscores_base_only_for_planning_selection() {
		let attack_cell: GridPosition = GridPosition { row: 0, col: 1 };
		let outside_cell: GridPosition = GridPosition { row: 0, col: 2 };
		let deployment_cell: GridPosition = GridPosition { row: 0, col: 0 };
		let anchor: StageTimeAnchor = StageTimeAnchor::BattleStart;
		let instance_id: SpawnInstanceId = SpawnInstanceId {
			cohort: SpawnCohortId::new("duty".to_owned()).unwrap(),
			ordinal: 0,
		};
		let model: StageSpaceTimeModel = StageSpaceTimeModel {
			stage_id: "manual-skill-ranking".to_owned(),
			code: None,
			level_id: "manual-skill-ranking".to_owned(),
			ticks_per_second: 30,
			cohorts: Vec::new(),
			enemies: vec![crate::planning::EnemyTrajectory {
				instance_id: instance_id.clone(),
				enemy_id: "enemy".to_owned(),
				route_index: 0,
				spawn_tick: 0,
				goal_deadline_tick: 10,
				max_hp: 1_000,
				defense: 0,
				magic_resistance: 0.0,
				target_class: TrajectoryClass::Ground,
				occupancy: vec![
					CellOccupancy {
						cell: outside_cell,
						enter_tick: 0,
						exit_tick: 5,
						remaining_route_distance_microunits: 2,
					},
					CellOccupancy {
						cell: attack_cell,
						enter_tick: 5,
						exit_tick: 10,
						remaining_route_distance_microunits: 1,
					},
				],
			}],
			bound_anchors: vec![StageTimeBinding {
				anchor: anchor.clone(),
				tick: 0,
			}],
			mechanic_requirements: Vec::new(),
		};
		let hp_group: WaveLocalHpGroup = WaveLocalHpGroup {
			id: WaveLocalHpGroupId {
				anchor,
				goal_deadline_offset_ticks: 10,
			},
			members: BTreeSet::from([instance_id]),
			required_hp: FixedDamage::from_whole(1_000).unwrap(),
		};
		let profile = |damage: u64| {
			NormalAttackProfile::new(
				1,
				vec![LockedTargetDamagePacket::new(
					NormalAttackDamageType::True,
					FixedDamage::from_whole(damage).unwrap(),
				)],
				0,
				1,
				NormalAttackTargetFilter::GroundAndAir,
			)
			.unwrap()
		};
		let candidate: ResolvedCoverageCandidate = ResolvedCoverageCandidate::new(
			"same-build".to_owned(),
			"char_manual".to_owned(),
			"Manual".to_owned(),
			10,
			deployment_cell,
			Facing::Right,
			BTreeSet::from([attack_cell]),
			profile(10),
		)
		.unwrap()
		.with_active_attack(profile(100));
		let base_only: [ResolvedCoverageDeployment<'_>; 1] =
			[ResolvedCoverageDeployment::new(&candidate, 0)];
		let early_skill: [ResolvedCoverageDeployment<'_>; 1] =
			[ResolvedCoverageDeployment::new(&candidate, 0)
				.with_active_span(TickSpan { start: 0, end: 2 })];
		let manual_skill: [ResolvedCoverageDeployment<'_>; 1] =
			[ResolvedCoverageDeployment::new(&candidate, 0)
				.with_active_span(TickSpan { start: 5, end: 7 })];
		assert_eq!(base_only[0].key(), manual_skill[0].key());
		assert!(base_only[0].active_spans().is_empty());
		assert_eq!(
			manual_skill[0].active_spans(),
			&[TickSpan { start: 5, end: 7 }]
		);
		let ranking_groups =
			project_retained_wave_local_hp_groups(&model, std::slice::from_ref(&hp_group)).unwrap();
		let ranking_scorers =
			prepare_retained_wave_local_hp_group_scorers(&model, &candidate, &ranking_groups)
				.unwrap();
		let base_contributions = retained_wave_local_hp_group_contributions(
			base_only[0].deploy_tick(),
			base_only[0].active_spans(),
			&ranking_groups,
			&ranking_scorers,
		)
		.unwrap();
		let skill_contributions = retained_wave_local_hp_group_contributions(
			manual_skill[0].deploy_tick(),
			manual_skill[0].active_spans(),
			&ranking_groups,
			&ranking_scorers,
		)
		.unwrap();
		let early_skill_contributions = retained_wave_local_hp_group_contributions(
			early_skill[0].deploy_tick(),
			early_skill[0].active_spans(),
			&ranking_groups,
			&ranking_scorers,
		)
		.unwrap();
		let base_ranking_score =
			retained_damage_ranking_score(std::slice::from_ref(&hp_group), &base_contributions)
				.unwrap();
		let skill_ranking_score =
			retained_damage_ranking_score(std::slice::from_ref(&hp_group), &skill_contributions)
				.unwrap();
		let demands = wave_local_hp_group_demands(std::slice::from_ref(&hp_group)).unwrap();
		let constraint = &demands[0].constraint;

		assert!(
			skill_contributions[constraint] > base_contributions[constraint],
			"a legal manual-skill schedule with higher expected damage must contribute more than \
			 base-only"
		);
		assert!(
			skill_contributions[constraint] > early_skill_contributions[constraint],
			"a skill active while the enemy occupies the attack range must outrank an equally long \
			 early activation"
		);
		assert_eq!(
			early_skill_contributions[constraint], base_contributions[constraint],
			"skill attacks before the enemy enters range must contribute no damage"
		);

		let operator_identity = ContributionOperatorIdentity::new("char_manual").unwrap();
		let candidates: [IndependentContributionCandidate; 2] = [
			IndependentContributionCandidate {
				key: ContributionCandidateKey::new("base-only").unwrap(),
				operator_identity: operator_identity.clone(),
				operator_name: "Manual".to_owned(),
				deployment_cost: 10,
				deployment_cell,
				optimistic_damage_by_constraint: BTreeMap::from([(
					constraint.clone(),
					hp_group.required_hp,
				)]),
				ranking_score: base_ranking_score,
			},
			IndependentContributionCandidate {
				key: ContributionCandidateKey::new("manual-skill").unwrap(),
				operator_identity,
				operator_name: "Manual".to_owned(),
				deployment_cost: 10,
				deployment_cell,
				optimistic_damage_by_constraint: BTreeMap::from([(
					constraint.clone(),
					hp_group.required_hp,
				)]),
				ranking_score: skill_ranking_score,
			},
		];
		let result =
			solve_independent_selection_cardinality_round_robin(IndependentSelectionRequest {
				candidates: &candidates,
				demands: &demands,
				enforce_demands: false,
				top_k: 1,
			})
			.unwrap();

		assert_eq!(
			result.solutions[0].selected_candidates[0]
				.candidate_key
				.as_str(),
			"manual-skill"
		);
	}

	#[test]
	fn export_validation_rejects_non_victory_artifacts() {
		for status in [
			ExpectedStrategyStatus::Unsupported,
			ExpectedStrategyStatus::ExpectedDefeat,
			ExpectedStrategyStatus::Incomplete,
		] {
			let error = ensure_export_validation_artifact_status(status)
				.expect_err("non-victory artifacts must not enter export validation");
			assert_eq!(
				error.code(),
				"artifact_battlefield_status_not_expected_victory"
			);
		}
		assert!(
			ensure_export_validation_artifact_status(ExpectedStrategyStatus::ExpectedVictory)
				.is_ok()
		);
	}

	#[test]
	fn top_k_status_counts_only_verified_exported_jobs() {
		assert_eq!(
			verified_search_status(
				0,
				1,
				ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached,
			),
			ExpectedDamageVerifiedSearchStatus::ProposalBudgetExhausted
		);
		assert_eq!(
			verified_search_status(
				1,
				2,
				ExpectedDamageLevelSolveStatus::PlanningModelIncomplete,
			),
			ExpectedDamageVerifiedSearchStatus::PlanningModelIncomplete
		);
		assert_eq!(
			verified_search_status(
				1,
				2,
				ExpectedDamageLevelSolveStatus::CandidateMaterializationIncomplete,
			),
			ExpectedDamageVerifiedSearchStatus::CandidateMaterializationIncomplete
		);
		assert_eq!(
			verified_search_status(0, 1, ExpectedDamageLevelSolveStatus::AdditiveInfeasible),
			ExpectedDamageVerifiedSearchStatus::AdditiveInfeasible
		);
		assert_eq!(
			verified_search_status(
				0,
				1,
				ExpectedDamageLevelSolveStatus::AdditiveSearchExhausted,
			),
			ExpectedDamageVerifiedSearchStatus::AdditiveSearchExhausted
		);
		assert_eq!(
			verified_search_status(
				2,
				2,
				ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached,
			),
			ExpectedDamageVerifiedSearchStatus::TopKFilled
		);
	}

	#[test]
	fn verified_job_search_continues_after_every_non_exportable_outcome() {
		#[derive(Clone, Copy)]
		enum SyntheticAttempt {
			ReplayDefeat,
			ReplayIncomplete,
			ReplayMaterializationFailed,
			ExportFailed,
			Verified(&'static str),
		}

		let mut outcomes: Vec<SyntheticAttempt> = vec![SyntheticAttempt::ReplayDefeat; 16];
		outcomes[1] = SyntheticAttempt::ReplayIncomplete;
		outcomes[2] = SyntheticAttempt::ReplayMaterializationFailed;
		outcomes[3] = SyntheticAttempt::ExportFailed;
		outcomes.push(SyntheticAttempt::Verified("verified-job"));
		outcomes.push(SyntheticAttempt::Verified("must-not-be-attempted"));
		let mut coordinator: OrderedReplayCoordinator<SyntheticAttempt> =
			OrderedReplayCoordinator::new(1);
		let mut attempted: Vec<usize> = Vec::new();
		let mut jobs: Vec<&str> = Vec::new();
		for (proposal_index, outcome) in outcomes.into_iter().enumerate() {
			for outcome in coordinator.push(proposal_index, outcome).unwrap() {
				attempted.push(proposal_index);
				if let SyntheticAttempt::Verified(job) = outcome {
					jobs.push(job);
					coordinator.record_verified_job();
				}
				if coordinator.top_k_filled() {
					break;
				}
			}
			if coordinator.top_k_filled() {
				break;
			}
		}

		assert_eq!(attempted, (0..=16).collect::<Vec<_>>());
		assert_eq!(jobs, vec!["verified-job"]);
	}

	#[test]
	fn joint_plan_variant_identity_includes_order_and_actual_ticks() {
		let plan = |members: &[(&str, u64)]| JointInsertionPlan {
			members: members
				.iter()
				.map(|&(candidate_key, deploy_tick)| JointInsertionMemberTiming {
					candidate_key: ContributionCandidateKey::new(candidate_key).unwrap(),
					deploy_tick,
					earliest_legal_activation_tick: Some(deploy_tick + 10),
					active_spans: vec![TickSpan {
						start: deploy_tick + 10,
						end: deploy_tick + 20,
					}],
				})
				.collect(),
			retained_deploy_ticks: BTreeMap::from([(7, 90)]),
			dp_schedule: Vec::new(),
		};
		let first_plan: JointInsertionPlan = plan(&[("a+b", 10), ("c", 20)]);
		let first: String = joint_plan_variant_id(&first_plan).unwrap();
		let repeated: String = joint_plan_variant_id(&first_plan).unwrap();
		let different_partition: String =
			joint_plan_variant_id(&plan(&[("a", 10), ("b+c", 20)])).unwrap();
		let different_order: String =
			joint_plan_variant_id(&plan(&[("c", 10), ("a+b", 20)])).unwrap();
		let different_tick: String =
			joint_plan_variant_id(&plan(&[("a+b", 11), ("c", 20)])).unwrap();

		assert_eq!(first, repeated);
		assert_ne!(first, different_partition);
		assert_ne!(first, different_order);
		assert_ne!(first, different_tick);
		assert!(first.starts_with("joint:"));
	}

	#[test]
	fn large_joint_sets_receive_bounded_deterministic_orders() {
		let ascending: Vec<ContributionCandidateKey> = (0..12)
			.map(|index| ContributionCandidateKey::new(format!("candidate-{index:02}")).unwrap())
			.collect();
		let mut descending: Vec<ContributionCandidateKey> = ascending.clone();
		descending.reverse();
		let orders: Vec<Vec<ContributionCandidateKey>> =
			bounded_rotated_orders(&[ascending.clone(), descending]);

		assert!(!orders.is_empty());
		assert!(orders.len() <= MAX_BOUNDED_JOINT_INSERTION_ORDERS);
		assert!(orders.iter().all(|order| order.len() == ascending.len()));
		assert_eq!(
			orders.iter().cloned().collect::<BTreeSet<_>>().len(),
			orders.len()
		);
	}

	#[test]
	fn battlefield_victory_gate_rejects_incomplete_and_expected_defeat_reports() {
		let probability = ReplayDefensiveProbabilityAudit {
			verified: true,
			success_probability: Some(0.75),
			covered_probability_mass: Some(1.0),
			success_probability_millionths: 750_000,
			covered_probability_millionths: 1_000_000,
			processed_steps: 1,
			peak_branches: 1,
			issues: Vec::new(),
		};

		assert_eq!(
			verified_expected_victory_probability(
				&ExpectedReplayReportStatus::ExpectedVictory,
				Some(&probability),
			),
			Some(0.75)
		);
		assert_eq!(
			verified_expected_victory_job_probability(
				&ExpectedReplayReportStatus::ExpectedVictory,
				Some(&probability),
			),
			Some((
				750_000,
				ExpectedDamageProbabilityBasis::FullBattlefieldReplay
			))
		);
		assert_eq!(
			serde_json::to_string(&ExpectedDamageProbabilityBasis::FullBattlefieldReplay).unwrap(),
			r#""full_battlefield_replay""#
		);
		let mut unverified_probability = probability.clone();
		unverified_probability.verified = false;
		assert_eq!(
			verified_expected_victory_probability(
				&ExpectedReplayReportStatus::ExpectedVictory,
				Some(&unverified_probability),
			),
			None
		);
		assert_eq!(
			verified_expected_victory_job_probability(
				&ExpectedReplayReportStatus::ExpectedVictory,
				Some(&unverified_probability),
			),
			None
		);
		let mut zero_probability = probability.clone();
		zero_probability.success_probability = Some(0.0);
		assert_eq!(
			verified_expected_victory_probability(
				&ExpectedReplayReportStatus::ExpectedVictory,
				Some(&zero_probability),
			),
			None
		);
		for status in [
			ExpectedReplayReportStatus::Incomplete,
			ExpectedReplayReportStatus::ExpectedDefeat,
		] {
			assert_eq!(
				verified_expected_victory_probability(&status, Some(&probability)),
				None
			);
		}
	}

	#[test]
	fn a_withheld_defeat_keeps_its_leaks_and_names_its_omission() {
		let report: ExpectedReplayAuditReport = ExpectedReplayAuditReport {
			schema_version: EXPECTED_REPLAY_REPORT_SCHEMA_VERSION,
			data_revision: String::new(),
			mechanics_revision: String::new(),
			stage_id: String::new(),
			guide_variant_id: String::new(),
			status: ExpectedReplayReportStatus::Incomplete,
			path_count: 0,
			wave_count: 0,
			enemy_spawn_count: 0,
			enemy_definition_count: 0,
			preplaced_actor_count: 0,
			exact_build_count: 0,
			exact_builds: Vec::new(),
			unsupported: Vec::new(),
			conservative_omissions: vec![ReplayConservativeOmissionEntry {
				mechanic_id: "friendly_buff".to_owned(),
				origins: vec!["operator:test".to_owned()],
				reasons: vec!["beneficial".to_owned()],
			}],
			unmodeled_omissions: Vec::new(),
			declared_approximations: Vec::new(),
			compile_issues: Vec::new(),
			terminal: Some("GoalLeak { enemies: [EnemyInstanceId(1)] }".to_owned()),
			wave_anchors: Vec::new(),
			enemies: vec![ReplayEnemyAudit {
				instance_id: 1,
				enemy_key: "enemy".to_owned(),
				spawned_tick: Some(0),
				deadline_tick: None,
				max_hp_micros: 1,
				remaining_hp_micros: 1,
				expected_damage_taken_micros: 0,
				death_tick: None,
				leaked_tick: Some(20),
				deadline_margin_ticks: None,
			}],
			operators: Vec::new(),
			blocks: Vec::new(),
			actions: Vec::new(),
			skill_activations: Vec::new(),
			dp: None,
			defensive_probability: None,
			simulation_issues: Vec::new(),
			trace_sha256: String::new(),
		};

		let LevelSolveReplayVariantOutcome::Incomplete {
			issues,
			leaked_enemies,
			unmodeled_count,
			conservative_count,
			..
		} = replay_variant_outcome(&report)
		else {
			panic!("an Incomplete report must stay an Incomplete outcome");
		};

		assert_eq!(leaked_enemies.len(), 1);
		assert_eq!(leaked_enemies[0].leaked_tick, Some(20));
		assert_eq!((unmodeled_count, conservative_count), (0, 1));
		assert_eq!(issues, ["conservative friendly_buff: beneficial"]);
	}

	#[test]
	fn export_activation_gate_rejects_auto_and_passive_selected_skills() {
		assert!(ensure_manual_export_activation(&SkillActivationType::Manual).is_ok());
		for activation_type in [SkillActivationType::Auto, SkillActivationType::Passive] {
			let error = ensure_manual_export_activation(&activation_type)
				.expect_err("non-manual skill must fail before export");
			assert_eq!(error.code(), "exported_skill_activation_not_manual");
		}
	}

	#[test]
	fn equal_tick_replacement_actions_follow_kind_then_member_order() {
		let later_named_first_member = MaaReplacementAction {
			operator_name: "zulu",
			kind: MaaReplacementActionKind::Deploy {
				cell: GridPosition { row: 0, col: 0 },
				direction: MaaJobDirection::Right,
			},
			chosen_tick: 10,
			earliest_legal_tick: 10,
		};
		let earlier_named_second_member = MaaReplacementAction {
			operator_name: "alpha",
			kind: MaaReplacementActionKind::Deploy {
				cell: GridPosition { row: 0, col: 1 },
				direction: MaaJobDirection::Left,
			},
			chosen_tick: 10,
			earliest_legal_tick: 10,
		};
		let first_member_skill = MaaReplacementAction {
			operator_name: "zulu",
			kind: MaaReplacementActionKind::Skill,
			chosen_tick: 10,
			earliest_legal_tick: 10,
		};

		assert!(
			replacement_action_sort_key(0, &later_named_first_member)
				< replacement_action_sort_key(1, &earlier_named_second_member)
		);
		assert!(
			replacement_action_sort_key(1, &earlier_named_second_member)
				< replacement_action_sort_key(0, &first_member_skill)
		);
	}

	#[test]
	fn deployment_tick_validation_applies_to_retained_members() {
		validate_replayed_deployment_tick("retained", 4, 30, Some(30)).unwrap();

		let error = validate_replayed_deployment_tick("retained", 4, 30, Some(31)).unwrap_err();

		assert_eq!(error.code(), "strategy_deployment_dp_tick_mismatch");
	}

	#[test]
	fn export_activation_must_be_earliest_and_recurrence_must_wait_for_daemon() {
		let spans = [
			TickSpan { start: 20, end: 25 },
			TickSpan { start: 30, end: 35 },
		];
		let delayed = validate_exported_activation_timing("candidate", 21, &spans, 30).unwrap_err();
		assert_eq!(delayed.code(), "exported_activation_not_earliest");

		let before_daemon =
			validate_exported_activation_timing("candidate", 20, &spans, 31).unwrap_err();
		assert_eq!(
			before_daemon.code(),
			"exported_recurring_skill_before_daemon"
		);

		validate_exported_activation_timing("candidate", 20, &spans, 30).unwrap();
	}

	#[test]
	fn retained_operator_daemon_usage_is_unmodeled() {
		validate_retained_operator_policy("retained", None).unwrap();
		validate_retained_operator_policy("retained", Some(0)).unwrap();

		let error = validate_retained_operator_policy("retained", Some(1)).unwrap_err();

		assert_eq!(error.code(), "exported_retained_operator_policy_mismatch");
	}

	#[test]
	fn retained_artifact_build_mutations_are_rejected_before_replay() {
		let source_build: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id("char_retained".to_owned()),
			2,
			90,
			200,
			2,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		);
		let exact_record: ArtifactExactBuild = ArtifactExactBuild {
			operator_id: "char_retained".to_owned(),
			elite: 2,
			level: 90,
			trust_percent: 200,
			potential_rank: 6,
			skill_slot: 2,
			skill_level: 10,
			module_id: None,
			module_level: 0,
		};
		validate_retained_artifact_build_record("Retained", &source_build, &exact_record)
			.expect("the workflow-derived source build must pass unchanged");

		let mut altered_level: ArtifactExactBuild = exact_record.clone();
		altered_level.level = 89;
		let mut altered_potential: ArtifactExactBuild = exact_record.clone();
		altered_potential.potential_rank = 5;
		let mut altered_skill: ArtifactExactBuild = exact_record.clone();
		altered_skill.skill_slot = 1;
		let mut altered_module: ArtifactExactBuild = exact_record;
		altered_module.module_id = Some("uniequip_mutated".to_owned());
		altered_module.module_level = 3;

		for altered_record in [
			altered_level,
			altered_potential,
			altered_skill,
			altered_module,
		] {
			let error: LevelSolvePrepareError =
				validate_retained_artifact_build_record("Retained", &source_build, &altered_record)
					.expect_err("retained source-build mutations must fail before replay");
			assert_eq!(error.code(), "exported_retained_build_policy_mismatch");
		}
	}

	#[test]
	fn replacement_plan_must_remove_the_canonical_source_operator_id() {
		validate_source_operator_id_replaced("char_source", &["char_replacement"])
			.expect("a different canonical operator must pass");

		let source_guide_identity: (&str, &str) = ("char_source", "char_source");
		let exported_identity: (&str, &str) = ("Source", "char_source");
		assert_ne!(source_guide_identity.0, exported_identity.0);
		let error: LevelSolvePrepareError =
			validate_source_operator_id_replaced(source_guide_identity.1, &[exported_identity.1])
				.expect_err("different spellings of the same canonical operator cannot replace it");

		assert_eq!(error.code(), "exported_source_operator_not_replaced");
	}

	#[test]
	fn exported_jobs_require_an_independent_battlefield_victory() {
		assert!(
			ensure_exported_battlefield_victory(ExpectedStrategyStatus::ExpectedVictory).is_ok()
		);
		for status in [
			ExpectedStrategyStatus::Incomplete,
			ExpectedStrategyStatus::ExpectedDefeat,
			ExpectedStrategyStatus::Unsupported,
		] {
			let error = ensure_exported_battlefield_victory(status)
				.expect_err("a non-victory exported replay must never become a job");
			assert_eq!(error.code(), "exported_strategy_battlefield_not_victory");
		}
	}
}
