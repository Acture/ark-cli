//! Expected-damage level-solve orchestration.
//!
//! This module intentionally starts after stage, guide, build, skill, placement, damage-gap, and
//! independent contribution materialization. It asks the planning MILP for lexicographically ranked
//! proposals, then validates those proposals against a shared-HP/shared-targeting evaluator. Unsafe
//! damage demands remain ranking targets when the planning model is incomplete; full battlefield
//! replay remains the export gate.

use std::{
	collections::BTreeMap,
	error::Error,
	fmt::{Display, Formatter},
	ops::ControlFlow,
};

use crate::data::GridPosition;
use serde::{Deserialize, Serialize};

use super::{
	coverage_search::JointCoverageScore,
	query::TickSpan,
	relaxation::{
		visit_independent_selection, visit_independent_selection_cardinality_round_robin,
		ContributionCandidateKey, ContributionConstraintKey, ContributionDemand,
		IndependentContributionCandidate, IndependentSelectionRequest,
		IndependentSelectionSearchStatus, IndependentSelectionSolution,
		IndependentSelectionSolveError,
	},
};
use crate::mechanics::{domain::Facing, stage::SpawnInstanceId};

/// Stable schema for CLI/report adapters.
pub const EXPECTED_DAMAGE_LEVEL_SOLVE_REPORT_SCHEMA_VERSION: u32 = 8;

/// Candidate rejected before additive planning, for example because its damage-affecting skill
/// semantics are unknown.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageCandidateExclusion {
	pub candidate_key: Option<String>,
	pub operator_id: Option<String>,
	pub operator_name: Option<String>,
	/// Machine-stable reason such as `unsupported_damage_blackboard`.
	pub reason_code: String,
	/// Human-readable evidence from the materialization layer.
	pub detail: String,
}

/// Result of evaluating one selected set against shared enemy HP and target selection.
///
/// Implementations may additionally enforce deployment legality or DP scheduling.
pub trait ExpectedDamageJointEvaluator: Sync {
	fn evaluate(
		&self,
		selected_candidate_keys: &[ContributionCandidateKey],
	) -> Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure>;
}

/// Typed failure to materialize or execute a joint proposal evaluation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointEvaluationFailure {
	pub reason_code: String,
	pub detail: String,
}

impl ExpectedDamageJointEvaluationFailure {
	#[must_use]
	pub fn new(reason_code: impl Into<String>, detail: impl Into<String>) -> Self {
		Self {
			reason_code: reason_code.into(),
			detail: detail.into(),
		}
	}
}

/// Fully materialized request for one expected-damage solve.
#[derive(Clone, Copy)]
pub struct ExpectedDamageLevelSolveRequest<'a> {
	/// Explicit upstream cumulative HP requirements.
	pub demands: &'a [ContributionDemand],
	/// Independently evaluated optimistic capacity vectors consumed by the MILP.
	pub candidates: &'a [IndependentContributionCandidate],
	/// Whether `candidates` contains every candidate in the upstream materialization domain.
	///
	/// When false, exhausting this slice cannot prove that the underlying candidate domain is
	/// infeasible.
	pub candidate_materialization_complete: bool,
	/// Whether every damage source and trajectory relaxation used to build the additive upper
	/// bounds is proven complete for infeasibility claims.
	///
	/// False still permits proposal generation and authoritative replay; it only prevents the
	/// report from presenting additive exhaustion as a proof about the real battle.
	pub planning_model_complete: bool,
	/// Upstream candidate exclusions retained in the final audit.
	pub candidate_exclusions: &'a [ExpectedDamageCandidateExclusion],
	/// Maximum additive proposals to enumerate and jointly score.
	pub additive_proposal_limit: usize,
	/// Explore feasible operator-count layers round-robin instead of exhausting every optimistic
	/// low-cardinality variant first.
	pub explore_cardinalities: bool,
	pub joint_evaluator: &'a dyn ExpectedDamageJointEvaluator,
}

/// Why an additive MILP proposal was not retained as a jointly damage-feasible candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExpectedDamageProposalRejection {
	JointExpectedDamageInfeasible {
		minimum_cumulative_coverage_millionths: u64,
		total_due_remaining_hp_scaled: u64,
	},
	JointEvaluationFailed {
		failure: ExpectedDamageJointEvaluationFailure,
	},
}

/// Final disposition of one additive proposal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExpectedDamageProposalDisposition {
	/// Passed shared-HP expected-damage scoring and may proceed to full battlefield replay.
	DamageFeasibleCandidate,
	/// Selected for replay by an incomplete space-time HP-group planning model.
	///
	/// In an incomplete planning model, HP demands are ranking targets rather than hard constraints
	/// because an earlier kill can redirect retained-plan fire to later enemies. The stricter joint
	/// scorer may also omit battlefield mechanics such as blocking. Neither negative is allowed to
	/// prune this proposal; full battlefield replay remains the authority.
	PlanningReplayCandidate {
		incomplete_planning_result: ExpectedDamageProposalRejection,
	},
	Rejected {
		reason: ExpectedDamageProposalRejection,
	},
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageResidualDemandAudit {
	pub constraint: ContributionConstraintKey,
	pub required_damage_scaled: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageConstraintContributionAudit {
	pub constraint: ContributionConstraintKey,
	pub optimistic_damage_scaled: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageSelectedCandidateAudit {
	pub candidate_key: String,
	pub operator_identity: String,
	pub operator_name: String,
	pub deployment_cost: u32,
	pub deployment_cell: GridPosition,
	pub optimistic_contributions: Vec<ExpectedDamageConstraintContributionAudit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageAdditiveConstraintAudit {
	pub constraint: ContributionConstraintKey,
	pub required_damage_scaled: u64,
	pub aggregate_capped_optimistic_damage_scaled: u64,
	pub remaining_demand_scaled: u64,
}

/// Complete additive evidence for one selected set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageAdditiveProposalEvidence {
	/// False means constraint deficits are ranking evidence, not replay-rejection evidence.
	pub demands_enforced: bool,
	pub operator_count: usize,
	pub total_deployment_cost: u64,
	pub selected_candidates: Vec<ExpectedDamageSelectedCandidateAudit>,
	pub constraints: Vec<ExpectedDamageAdditiveConstraintAudit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointMemberDamageEventAudit {
	pub attack_index: u64,
	pub target: SpawnInstanceId,
	pub tick: u64,
	pub raw_damage_scaled: u64,
	pub credited_damage_scaled: u64,
	pub overkill_damage_scaled: u64,
	pub remaining_hp_scaled: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointMemberAudit {
	pub stable_build_id: String,
	pub operator_id: String,
	pub operator_name: String,
	pub deployment_cost: u32,
	pub deployment_cell: GridPosition,
	pub facing: Facing,
	pub deploy_tick: u64,
	/// Attack cadence origin. Skill phases normally use the skill activation tick.
	pub attack_origin_tick: u64,
	/// Half-open skill/profile activity windows; empty means active for the duty window.
	pub active_spans: Vec<TickSpan>,
	pub attacks_with_targets: u64,
	pub raw_damage_scaled: u64,
	pub credited_damage_scaled: u64,
	pub overkill_damage_scaled: u64,
	pub damage_events: Vec<ExpectedDamageJointMemberDamageEventAudit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointTargetDamageEventAudit {
	pub tick: u64,
	pub raw_damage_scaled: u64,
	pub credited_damage_scaled: u64,
	pub overkill_damage_scaled: u64,
	pub remaining_hp_scaled: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointTargetAudit {
	pub target: SpawnInstanceId,
	pub is_duty_target: bool,
	pub deadline_tick: Option<u64>,
	pub max_hp_scaled: u64,
	pub remaining_hp_scaled: u64,
	pub kill_tick: Option<u64>,
	pub raw_damage_scaled: u64,
	pub credited_damage_scaled: u64,
	pub overkill_damage_scaled: u64,
	pub damage_events: Vec<ExpectedDamageJointTargetDamageEventAudit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointDeadlineAudit {
	pub deadline_tick: u64,
	pub required_hp_scaled: u64,
	pub credited_damage_scaled: u64,
	pub remaining_hp_scaled: u64,
	pub coverage_millionths: u64,
}

/// Serializable shared-state validation evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageJointEvidence {
	/// Whether this deterministic expected-damage replay clears every modeled deadline.
	///
	/// A feasible result certifies a damage-feasible planning candidate. It is neither a measured
	/// battle success probability nor full-game replay feasibility.
	pub deterministic_expected_damage_feasible: bool,
	pub minimum_cumulative_coverage_millionths: u64,
	pub total_due_remaining_hp_scaled: u64,
	pub clearance_slack_ticks: Option<u64>,
	pub members: Vec<ExpectedDamageJointMemberAudit>,
	pub targets: Vec<ExpectedDamageJointTargetAudit>,
	pub cumulative_deadlines: Vec<ExpectedDamageJointDeadlineAudit>,
}

impl From<&JointCoverageScore> for ExpectedDamageJointEvidence {
	fn from(score: &JointCoverageScore) -> Self {
		let members: Vec<ExpectedDamageJointMemberAudit> = score
			.member_scores
			.iter()
			.map(|member| ExpectedDamageJointMemberAudit {
				stable_build_id: member.key.stable_build_id.clone(),
				operator_id: member.operator_id.clone(),
				operator_name: member.operator_name.clone(),
				deployment_cost: member.deployment_cost,
				deployment_cell: member.key.deployment_cell,
				facing: member.key.facing,
				deploy_tick: member.key.deploy_tick,
				attack_origin_tick: member.attack_origin_tick,
				active_spans: member.active_spans.clone(),
				attacks_with_targets: member.attacks_with_targets,
				raw_damage_scaled: member.damage.raw().scaled(),
				credited_damage_scaled: member.damage.credited().scaled(),
				overkill_damage_scaled: member.damage.overkill().scaled(),
				damage_events: member
					.damage_events
					.iter()
					.map(|event| ExpectedDamageJointMemberDamageEventAudit {
						attack_index: event.attack_index,
						target: event.instance_id.clone(),
						tick: event.tick,
						raw_damage_scaled: event.raw.scaled(),
						credited_damage_scaled: event.credited.scaled(),
						overkill_damage_scaled: event.overkill.scaled(),
						remaining_hp_scaled: event.remaining_hp.scaled(),
					})
					.collect(),
			})
			.collect();
		let targets: Vec<ExpectedDamageJointTargetAudit> = score
			.enemy_scores
			.iter()
			.map(|enemy| ExpectedDamageJointTargetAudit {
				target: enemy.instance_id.clone(),
				is_duty_target: enemy.is_duty_target,
				deadline_tick: enemy.deadline_tick,
				max_hp_scaled: enemy.max_hp.scaled(),
				remaining_hp_scaled: enemy.remaining_hp.scaled(),
				kill_tick: enemy.kill_tick,
				raw_damage_scaled: enemy.damage.raw().scaled(),
				credited_damage_scaled: enemy.damage.credited().scaled(),
				overkill_damage_scaled: enemy.damage.overkill().scaled(),
				damage_events: enemy
					.damage_events
					.iter()
					.map(|event| ExpectedDamageJointTargetDamageEventAudit {
						tick: event.tick,
						raw_damage_scaled: event.raw.scaled(),
						credited_damage_scaled: event.credited.scaled(),
						overkill_damage_scaled: event.overkill.scaled(),
						remaining_hp_scaled: event.remaining_hp.scaled(),
					})
					.collect(),
			})
			.collect();
		let cumulative_deadlines: Vec<ExpectedDamageJointDeadlineAudit> = score
			.cumulative_deadlines
			.iter()
			.map(|deadline| ExpectedDamageJointDeadlineAudit {
				deadline_tick: deadline.deadline_tick,
				required_hp_scaled: deadline.required_hp.scaled(),
				credited_damage_scaled: deadline.credited_damage.scaled(),
				remaining_hp_scaled: deadline.remaining_hp.scaled(),
				coverage_millionths: deadline.coverage.millionths(),
			})
			.collect();
		Self {
			deterministic_expected_damage_feasible: score.is_feasible(),
			minimum_cumulative_coverage_millionths: score.minimum_cumulative_coverage.millionths(),
			total_due_remaining_hp_scaled: score.total_due_remaining_hp.scaled(),
			clearance_slack_ticks: score.clearance_slack_ticks,
			members,
			targets,
			cumulative_deadlines,
		}
	}
}

/// One additive proposal plus its joint validation result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageProposalAudit {
	pub additive: ExpectedDamageAdditiveProposalEvidence,
	pub joint: Option<ExpectedDamageJointEvidence>,
	pub disposition: ExpectedDamageProposalDisposition,
}

/// Why proposal enumeration stopped.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedDamageLevelSolveStatus {
	/// The supplied candidate slice was incomplete and yielded no further damage-feasible plan.
	///
	/// This is deliberately not an infeasibility certificate for the full candidate domain.
	CandidateMaterializationIncomplete,
	/// The additive model can still propose plans, but exhaustion is not an infeasibility proof.
	PlanningModelIncomplete,
	AdditiveInfeasible,
	AdditiveSearchExhausted,
	AdditiveProposalLimitReached,
	/// The proposal visitor requested cancellation before additive enumeration finished.
	ProposalVisitorStopped,
}

/// Complete expected-damage solve result, including rejected proposals and upstream exclusions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedDamageLevelSolveReport {
	pub schema_version: u32,
	pub status: ExpectedDamageLevelSolveStatus,
	pub residual_demands: Vec<ExpectedDamageResidualDemandAudit>,
	pub candidate_exclusions: Vec<ExpectedDamageCandidateExclusion>,
	pub proposals: Vec<ExpectedDamageProposalAudit>,
	/// Indices into `proposals`, in solver order, that satisfy the typed expected-damage claim.
	pub damage_feasible_proposal_indices: Vec<usize>,
	/// Indices that have a legal joint plan and must proceed to authoritative full-battle replay.
	///
	/// This is a superset of `damage_feasible_proposal_indices` when the planning model is
	/// incomplete and a negative unblocked score therefore cannot safely prune a candidate.
	pub replay_candidate_indices: Vec<usize>,
}

impl ExpectedDamageLevelSolveReport {
	pub fn damage_feasible_proposals(&self) -> impl Iterator<Item = &ExpectedDamageProposalAudit> {
		self.damage_feasible_proposal_indices
			.iter()
			.map(|&index| &self.proposals[index])
	}

	pub fn replay_candidates(&self) -> impl Iterator<Item = &ExpectedDamageProposalAudit> {
		self.replay_candidate_indices
			.iter()
			.map(|&index| &self.proposals[index])
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedDamageLevelSolveInputError {
	ZeroAdditiveProposalLimit,
}

impl Display for ExpectedDamageLevelSolveInputError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::ZeroAdditiveProposalLimit => {
				formatter.write_str("expected-damage additive proposal limit must be positive")
			}
		}
	}
}

impl Error for ExpectedDamageLevelSolveInputError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedDamageLevelSolveError {
	InvalidInput(ExpectedDamageLevelSolveInputError),
	AdditiveSelection(IndependentSelectionSolveError),
}

impl Display for ExpectedDamageLevelSolveError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::InvalidInput(error) => write!(formatter, "invalid level-solve input: {error}"),
			Self::AdditiveSelection(error) => {
				write!(formatter, "could not enumerate additive proposals: {error}")
			}
		}
	}
}

impl Error for ExpectedDamageLevelSolveError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::InvalidInput(error) => Some(error),
			Self::AdditiveSelection(error) => Some(error),
		}
	}
}

/// Enumerates additive proposals for explicit gaps and returns jointly damage-feasible candidates.
/// Full battlefield replay remains the only export gate.
pub fn solve_expected_damage_level(
	request: ExpectedDamageLevelSolveRequest<'_>,
) -> Result<ExpectedDamageLevelSolveReport, ExpectedDamageLevelSolveError> {
	visit_expected_damage_level(
		request,
		|_proposal_index: usize, _proposal: &ExpectedDamageProposalAudit| ControlFlow::Continue(()),
	)
}

/// Streams each fully audited proposal to `visitor` before enumerating the next MILP solution.
///
/// The returned report owns every proposal observed by the visitor. Returning
/// [`ControlFlow::Break`] stops additive enumeration immediately and reports
/// [`ExpectedDamageLevelSolveStatus::ProposalVisitorStopped`].
pub fn visit_expected_damage_level<F>(
	request: ExpectedDamageLevelSolveRequest<'_>,
	mut visitor: F,
) -> Result<ExpectedDamageLevelSolveReport, ExpectedDamageLevelSolveError>
where
	F: FnMut(usize, &ExpectedDamageProposalAudit) -> ControlFlow<()>,
{
	if request.additive_proposal_limit == 0 {
		return Err(ExpectedDamageLevelSolveError::InvalidInput(
			ExpectedDamageLevelSolveInputError::ZeroAdditiveProposalLimit,
		));
	}

	let demands: Vec<ContributionDemand> = request.demands.to_vec();

	let candidates_by_key: BTreeMap<ContributionCandidateKey, &IndependentContributionCandidate> =
		request
			.candidates
			.iter()
			.map(|candidate| (candidate.key.clone(), candidate))
			.collect();
	let mut proposals: Vec<ExpectedDamageProposalAudit> = Vec::new();
	let mut damage_feasible_proposal_indices: Vec<usize> = Vec::new();
	let mut replay_candidate_indices: Vec<usize> = Vec::new();
	let additive_request: IndependentSelectionRequest<'_> = IndependentSelectionRequest {
		candidates: request.candidates,
		demands: &demands,
		enforce_demands: request.planning_model_complete,
		top_k: request.additive_proposal_limit,
	};
	let mut emit_proposal =
		|solution_index: usize, solution: &IndependentSelectionSolution| -> ControlFlow<()> {
			debug_assert_eq!(solution_index, proposals.len());
			let selected_keys: Vec<ContributionCandidateKey> = solution
				.selected_candidates
				.iter()
				.map(|member| member.candidate_key.clone())
				.collect();
			let additive: ExpectedDamageAdditiveProposalEvidence = additive_evidence(
				solution,
				&candidates_by_key,
				request.planning_model_complete,
			);
			let (joint, disposition) = match request.joint_evaluator.evaluate(&selected_keys) {
				Ok(evidence) if evidence.deterministic_expected_damage_feasible => (
					Some(evidence),
					ExpectedDamageProposalDisposition::DamageFeasibleCandidate,
				),
				Ok(evidence) => {
					let rejection: ExpectedDamageProposalRejection =
						ExpectedDamageProposalRejection::JointExpectedDamageInfeasible {
							minimum_cumulative_coverage_millionths: evidence
								.minimum_cumulative_coverage_millionths,
							total_due_remaining_hp_scaled: evidence.total_due_remaining_hp_scaled,
						};
					let disposition: ExpectedDamageProposalDisposition =
						if request.planning_model_complete {
							ExpectedDamageProposalDisposition::Rejected { reason: rejection }
						} else {
							ExpectedDamageProposalDisposition::PlanningReplayCandidate {
								incomplete_planning_result: rejection,
							}
						};
					(Some(evidence), disposition)
				}
				Err(failure) => {
					let rejection: ExpectedDamageProposalRejection =
						ExpectedDamageProposalRejection::JointEvaluationFailed { failure };
					let disposition: ExpectedDamageProposalDisposition =
						if request.planning_model_complete {
							ExpectedDamageProposalDisposition::Rejected { reason: rejection }
						} else {
							ExpectedDamageProposalDisposition::PlanningReplayCandidate {
								incomplete_planning_result: rejection,
							}
						};
					(None, disposition)
				}
			};
			let proposal_index: usize = proposals.len();
			if matches!(
				disposition,
				ExpectedDamageProposalDisposition::DamageFeasibleCandidate
			) {
				damage_feasible_proposal_indices.push(proposal_index);
				replay_candidate_indices.push(proposal_index);
			} else if matches!(
				disposition,
				ExpectedDamageProposalDisposition::PlanningReplayCandidate { .. }
			) {
				replay_candidate_indices.push(proposal_index);
			}
			proposals.push(ExpectedDamageProposalAudit {
				additive,
				joint,
				disposition,
			});
			visitor(proposal_index, &proposals[proposal_index])
		};
	let additive_status: IndependentSelectionSearchStatus = match if request.explore_cardinalities {
		visit_independent_selection_cardinality_round_robin(additive_request, &mut emit_proposal)
	} else {
		visit_independent_selection(additive_request, &mut emit_proposal)
	} {
		Ok(status) => status,
		Err(IndependentSelectionSolveError::Infeasible) => {
			return Ok(ExpectedDamageLevelSolveReport {
				schema_version: EXPECTED_DAMAGE_LEVEL_SOLVE_REPORT_SCHEMA_VERSION,
				status: if !request.candidate_materialization_complete {
					ExpectedDamageLevelSolveStatus::CandidateMaterializationIncomplete
				} else if !request.planning_model_complete {
					ExpectedDamageLevelSolveStatus::PlanningModelIncomplete
				} else {
					ExpectedDamageLevelSolveStatus::AdditiveInfeasible
				},
				residual_demands: residual_demand_audits(demands),
				candidate_exclusions: request.candidate_exclusions.to_vec(),
				proposals,
				damage_feasible_proposal_indices,
				replay_candidate_indices,
			});
		}
		Err(error) => return Err(ExpectedDamageLevelSolveError::AdditiveSelection(error)),
	};

	let status: ExpectedDamageLevelSolveStatus = match additive_status {
		IndependentSelectionSearchStatus::Exhausted => {
			if !request.candidate_materialization_complete {
				ExpectedDamageLevelSolveStatus::CandidateMaterializationIncomplete
			} else if !request.planning_model_complete {
				ExpectedDamageLevelSolveStatus::PlanningModelIncomplete
			} else {
				ExpectedDamageLevelSolveStatus::AdditiveSearchExhausted
			}
		}
		IndependentSelectionSearchStatus::TopKReached => {
			ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached
		}
		IndependentSelectionSearchStatus::VisitorStopped => {
			ExpectedDamageLevelSolveStatus::ProposalVisitorStopped
		}
	};

	Ok(ExpectedDamageLevelSolveReport {
		schema_version: EXPECTED_DAMAGE_LEVEL_SOLVE_REPORT_SCHEMA_VERSION,
		status,
		residual_demands: residual_demand_audits(demands),
		candidate_exclusions: request.candidate_exclusions.to_vec(),
		proposals,
		damage_feasible_proposal_indices,
		replay_candidate_indices,
	})
}

fn residual_demand_audits(
	demands: Vec<ContributionDemand>,
) -> Vec<ExpectedDamageResidualDemandAudit> {
	demands
		.into_iter()
		.map(|demand| ExpectedDamageResidualDemandAudit {
			constraint: demand.constraint,
			required_damage_scaled: demand.required.scaled(),
		})
		.collect()
}

fn additive_evidence(
	solution: &IndependentSelectionSolution,
	candidates_by_key: &BTreeMap<ContributionCandidateKey, &IndependentContributionCandidate>,
	demands_enforced: bool,
) -> ExpectedDamageAdditiveProposalEvidence {
	let selected_candidates: Vec<ExpectedDamageSelectedCandidateAudit> = solution
		.selected_candidates
		.iter()
		.map(|member| {
			let candidate: &IndependentContributionCandidate =
				candidates_by_key[&member.candidate_key];
			ExpectedDamageSelectedCandidateAudit {
				candidate_key: member.candidate_key.as_str().to_owned(),
				operator_identity: member.operator_identity.as_str().to_owned(),
				operator_name: member.operator_name.clone(),
				deployment_cost: member.deployment_cost,
				deployment_cell: member.deployment_cell,
				optimistic_contributions: candidate
					.optimistic_damage_by_constraint
					.iter()
					.map(
						|(constraint, damage)| ExpectedDamageConstraintContributionAudit {
							constraint: constraint.clone(),
							optimistic_damage_scaled: damage.scaled(),
						},
					)
					.collect(),
			}
		})
		.collect();
	let constraints: Vec<ExpectedDamageAdditiveConstraintAudit> = solution
		.constraint_audit
		.iter()
		.map(|constraint| ExpectedDamageAdditiveConstraintAudit {
			constraint: constraint.constraint.clone(),
			required_damage_scaled: constraint.required.scaled(),
			aggregate_capped_optimistic_damage_scaled: constraint
				.aggregate_capped_optimistic_damage
				.scaled(),
			remaining_demand_scaled: constraint.remaining_demand.scaled(),
		})
		.collect();
	ExpectedDamageAdditiveProposalEvidence {
		demands_enforced,
		operator_count: solution.operator_count,
		total_deployment_cost: solution.total_deployment_cost,
		selected_candidates,
		constraints,
	}
}
