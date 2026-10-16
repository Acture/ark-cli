//! Exact stage-planning selection over additive candidate upper bounds.
//!
//! The caller supplies every target requirement and every per-candidate upper-bound contribution.
//! This relaxation only generates proposals: each selected set still requires joint stage
//! evaluation before it can be treated as feasible.

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
	ops::ControlFlow,
};

use crate::data::GridPosition;
use good_lp::{
	microlp, variable, Expression, ProblemVariables, ResolutionError, Solution, SolutionStatus,
	SolverModel, Variable,
};

use serde::{Deserialize, Serialize};

use crate::mechanics::{domain::FixedDamage, stage::SpawnInstanceId};

/// Largest integer represented exactly by an IEEE-754 `f64`.
pub const MAX_EXACT_SELECTION_INTEGER: u64 = 1_u64 << 53;

/// Arknights squads contain at most twelve simultaneously selected distinct operators.
const MAX_OPERATOR_SET_CARDINALITY: usize = 12;

const BINARY_SOLUTION_TOLERANCE: f64 = 1.0e-7;

/// Stable identity of one independently evaluated contribution candidate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContributionCandidateKey(String);

impl ContributionCandidateKey {
	pub fn new(value: impl Into<String>) -> Result<Self, ContributionIdentityError> {
		let value: String = value.into();
		if value.trim().is_empty() {
			return Err(ContributionIdentityError::BlankCandidateKey);
		}
		Ok(Self(value))
	}

	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl Display for ContributionCandidateKey {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(&self.0)
	}
}

/// Stable identity shared by all builds or placements of the same operator.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContributionOperatorIdentity(String);

impl ContributionOperatorIdentity {
	pub fn new(value: impl Into<String>) -> Result<Self, ContributionIdentityError> {
		let value: String = value.into();
		if value.trim().is_empty() {
			return Err(ContributionIdentityError::BlankOperatorIdentity);
		}
		Ok(Self(value))
	}

	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl Display for ContributionOperatorIdentity {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(&self.0)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContributionIdentityError {
	BlankCandidateKey,
	BlankConstraintKey,
	BlankOperatorIdentity,
}

impl Display for ContributionIdentityError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::BlankCandidateKey => formatter.write_str("contribution candidate key is blank"),
			Self::BlankConstraintKey => formatter.write_str("contribution constraint key is blank"),
			Self::BlankOperatorIdentity => {
				formatter.write_str("contribution operator identity is blank")
			}
		}
	}
}

impl Error for ContributionIdentityError {}

/// Stable identity of one additive planning constraint.
///
/// Enemy constraints remain available to the legacy per-enemy audit helper. Full-battle planning
/// uses cumulative space-time HP groups so target-order changes are not pruned before joint replay.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContributionConstraintKey {
	Enemy { target: SpawnInstanceId },
	SpaceTimeHpGroup { group_id: String },
}

impl ContributionConstraintKey {
	#[must_use]
	pub fn enemy(target: SpawnInstanceId) -> Self {
		Self::Enemy { target }
	}

	pub fn space_time_hp_group(
		group_id: impl Into<String>,
	) -> Result<Self, ContributionIdentityError> {
		let group_id: String = group_id.into();
		if group_id.trim().is_empty() {
			return Err(ContributionIdentityError::BlankConstraintKey);
		}
		Ok(Self::SpaceTimeHpGroup { group_id })
	}
}

impl Display for ContributionConstraintKey {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Enemy { target } => write!(formatter, "enemy:{target:?}"),
			Self::SpaceTimeHpGroup { group_id } => {
				write!(formatter, "space_time_hp_group:{group_id}")
			}
		}
	}
}

/// Caller-supplied optimistic upper-bound contribution for one candidate against each constraint.
///
/// Summing these independently established bounds is a planning relaxation, not an exact
/// simulation of joint combat, targeting, blocking, buffs, or kill-order interactions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndependentContributionCandidate {
	pub key: ContributionCandidateKey,
	pub operator_identity: ContributionOperatorIdentity,
	pub operator_name: String,
	pub deployment_cost: u32,
	pub deployment_cell: GridPosition,
	pub optimistic_damage_by_constraint: BTreeMap<ContributionConstraintKey, FixedDamage>,
	/// Ranking-only score for choosing among otherwise legal planning candidates.
	///
	/// Higher is better. Unlike `optimistic_damage_by_constraint`, this value is never used to
	/// prove feasibility; callers may therefore derive it from a retained-witness timeline that
	/// authoritative replay can later reject.
	pub ranking_score: u64,
}

/// One caller-supplied additive requirement that the selected configuration must satisfy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContributionDemand {
	pub constraint: ContributionConstraintKey,
	pub required: FixedDamage,
}

/// Complete caller-defined input for one additive candidate-set selection.
#[derive(Clone, Copy, Debug)]
pub struct IndependentSelectionRequest<'a> {
	pub candidates: &'a [IndependentContributionCandidate],
	pub demands: &'a [ContributionDemand],
	/// Whether each demand is proven to be a necessary lower bound on selected-candidate damage.
	///
	/// Set this to false when retained battlefield damage can be redirected after a candidate
	/// changes kill order. In that mode, demands remain ranking targets and audit fields but cannot
	/// reject a configuration before authoritative replay.
	pub enforce_demands: bool,
	pub top_k: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixedDamageInputLocation {
	Demand {
		constraint: ContributionConstraintKey,
	},
	IndependentContribution {
		candidate_key: ContributionCandidateKey,
		constraint: ContributionConstraintKey,
	},
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndependentSelectionInputError {
	NoDemands,
	ZeroTopK,
	BlankOperatorName {
		candidate_key: ContributionCandidateKey,
	},
	DuplicateCandidateKey {
		candidate_key: ContributionCandidateKey,
	},
	DuplicateDemandConstraint {
		constraint: ContributionConstraintKey,
	},
	MissingIndependentContribution {
		candidate_key: ContributionCandidateKey,
		constraint: ContributionConstraintKey,
	},
	UnexpectedIndependentContribution {
		candidate_key: ContributionCandidateKey,
		constraint: ContributionConstraintKey,
	},
	FixedDamageExceedsExactSolverIntegerRange {
		location: FixedDamageInputLocation,
		scaled: u64,
	},
	RankingScoreExceedsExactSolverIntegerRange {
		candidate_key: ContributionCandidateKey,
		score: u64,
	},
}

impl Display for IndependentSelectionInputError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NoDemands => formatter.write_str("selection demand is empty"),
			Self::ZeroTopK => formatter.write_str("selection top_k must be positive"),
			Self::BlankOperatorName { candidate_key } => {
				write!(
					formatter,
					"candidate {candidate_key} has a blank operator name"
				)
			}
			Self::DuplicateCandidateKey { candidate_key } => {
				write!(
					formatter,
					"duplicate contribution candidate key {candidate_key}"
				)
			}
			Self::DuplicateDemandConstraint { constraint } => {
				write!(
					formatter,
					"duplicate selection demand constraint {constraint}"
				)
			}
			Self::MissingIndependentContribution {
				candidate_key,
				constraint,
			} => write!(
				formatter,
				"candidate {candidate_key} has no optimistic contribution for {constraint}"
			),
			Self::UnexpectedIndependentContribution {
				candidate_key,
				constraint,
			} => write!(
				formatter,
				"candidate {candidate_key} has an optimistic contribution for unknown constraint \
				 {constraint}"
			),
			Self::FixedDamageExceedsExactSolverIntegerRange { location, scaled } => write!(
				formatter,
				"fixed damage {scaled} at {location:?} exceeds the exact f64 integer range \
				 {MAX_EXACT_SELECTION_INTEGER}"
			),
			Self::RankingScoreExceedsExactSolverIntegerRange {
				candidate_key,
				score,
			} => write!(
				formatter,
				"candidate {candidate_key} ranking score {score} exceeds the exact f64 integer range \
				 {MAX_EXACT_SELECTION_INTEGER}"
			),
		}
	}
}

impl Error for IndependentSelectionInputError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndependentSelectionSolveError {
	InvalidInput(IndependentSelectionInputError),
	Infeasible,
	SolverFailure { message: String },
}

impl Display for IndependentSelectionSolveError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::InvalidInput(error) => {
				write!(formatter, "invalid independent selection input: {error}")
			}
			Self::Infeasible => formatter.write_str(
				"independent-contribution selection MILP is infeasible under the supplied \
				 estimates and constraints",
			),
			Self::SolverFailure { message } => {
				write!(formatter, "independent selection solver failed: {message}")
			}
		}
	}
}

impl Error for IndependentSelectionSolveError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::InvalidInput(error) => Some(error),
			Self::Infeasible | Self::SolverFailure { .. } => None,
		}
	}
}

impl From<IndependentSelectionInputError> for IndependentSelectionSolveError {
	fn from(error: IndependentSelectionInputError) -> Self {
		Self::InvalidInput(error)
	}
}

/// Whether enumeration stopped at the requested count or exhausted all feasible configurations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndependentSelectionSearchStatus {
	TopKReached,
	Exhausted,
	/// The visitor requested cancellation before enumeration reached a terminal search condition.
	VisitorStopped,
}

/// One selected deployment, sorted by stable candidate key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndependentSelectionMember {
	pub candidate_key: ContributionCandidateKey,
	pub operator_identity: ContributionOperatorIdentity,
	pub operator_name: String,
	pub deployment_cost: u32,
	pub deployment_cell: GridPosition,
}

/// Auditable coverage of one enforced demand or unenforced ranking target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContributionConstraintAudit {
	pub constraint: ContributionConstraintKey,
	pub required: FixedDamage,
	pub aggregate_capped_optimistic_damage: FixedDamage,
	pub remaining_demand: FixedDamage,
}

/// One proposal from the independent-contribution planning model.
///
/// Primary and secondary objective values are explicit. `remaining_demand` may be positive when
/// the request marks demands as unsafe to enforce. Planning feasibility never establishes joint
/// stage feasibility; callers must evaluate the selected set jointly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndependentSelectionSolution {
	pub operator_count: usize,
	pub total_deployment_cost: u64,
	pub selected_candidates: Vec<IndependentSelectionMember>,
	pub constraint_audit: Vec<ContributionConstraintAudit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndependentSelectionSolveResult {
	pub status: IndependentSelectionSearchStatus,
	pub solutions: Vec<IndependentSelectionSolution>,
}

struct NormalizedInput<'a> {
	candidates: Vec<&'a IndependentContributionCandidate>,
	demands: Vec<&'a ContributionDemand>,
	enforce_demands: bool,
	maximum_ranking_score: u64,
	operator_groups: Vec<Vec<usize>>,
	operator_group_by_candidate: Vec<usize>,
	cell_groups: Vec<Vec<usize>>,
}

/// Canonical roster identity within one normalized selection request.
///
/// The contained operator-group indices are sorted. Cardinality is deliberately derived from
/// `len()` instead of being duplicated in a second owning search-state hierarchy.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct OperatorSet(Vec<usize>);

impl OperatorSet {
	fn new(mut operator_groups: Vec<usize>) -> Self {
		operator_groups.sort_unstable();
		operator_groups.dedup();
		Self(operator_groups)
	}

	fn len(&self) -> usize {
		self.0.len()
	}

	fn iter(&self) -> impl Iterator<Item = usize> + '_ {
		self.0.iter().copied()
	}
}

/// Assignment enumeration owned by exactly one operator set.
///
/// Candidate configurations encode build, cell, facing, deployment timing, and skill timing.
/// Keeping their no-good cuts here prevents unrelated rosters and cardinalities from polluting a
/// fixed-roster solve.
struct AssignmentSearchState {
	excluded_configurations: Vec<Vec<usize>>,
	next_configuration: Option<Vec<usize>>,
	emitted_count: usize,
	exhausted: bool,
}

impl AssignmentSearchState {
	fn with_first_configuration(first_configuration: Vec<usize>) -> Self {
		Self {
			excluded_configurations: vec![first_configuration],
			next_configuration: None,
			emitted_count: 1,
			exhausted: false,
		}
	}
}

/// Solves an independent-contribution planning model with binary candidate variables.
///
/// The objective is strictly lexicographic: minimize operator count, minimize total deployment cost,
/// then maximize the caller-supplied ranking score. When `enforce_demands` is false,
/// positive demands require at least one selected candidate but otherwise remain ranking and audit
/// targets; this prevents incomplete retained-fire modeling from producing false-negative cuts.
/// Each operator set owns its assignment frontier and exact no-good cuts. Distinct rosters are
/// covered before later placement and skill-time variants of those rosters, so diversity changes
/// ordering without pruning a configuration. Operator-set enumeration enforces the game's
/// twelve-operator squad limit; simultaneous deployment limits remain authoritative replay rules.
pub fn solve_independent_selection(
	request: IndependentSelectionRequest<'_>,
) -> Result<IndependentSelectionSolveResult, IndependentSelectionSolveError> {
	let mut solutions: Vec<IndependentSelectionSolution> = Vec::new();
	let status: IndependentSelectionSearchStatus = visit_independent_selection(
		request,
		|_solution_index: usize, solution: &IndependentSelectionSolution| {
			solutions.push(solution.clone());
			ControlFlow::Continue(())
		},
	)?;
	Ok(IndependentSelectionSolveResult { status, solutions })
}

/// Streams independent-selection solutions to `visitor` as soon as each MILP result is built.
///
/// Returning [`ControlFlow::Break`] cancels enumeration and yields
/// [`IndependentSelectionSearchStatus::VisitorStopped`]. This lets a producer stop when its
/// downstream queue closes without misreporting complete search exhaustion.
pub fn visit_independent_selection<F>(
	request: IndependentSelectionRequest<'_>,
	mut visitor: F,
) -> Result<IndependentSelectionSearchStatus, IndependentSelectionSolveError>
where
	F: FnMut(usize, &IndependentSelectionSolution) -> ControlFlow<()>,
{
	let input: NormalizedInput<'_> = validate_and_normalize(request)?;
	let has_positive_demand: bool = input
		.demands
		.iter()
		.any(|demand| !demand.required.is_zero());

	if input.candidates.is_empty() && has_positive_demand {
		return Err(IndependentSelectionSolveError::Infeasible);
	}
	let mut emitted_count: usize = 0;
	if !has_positive_demand {
		let solution: IndependentSelectionSolution = build_solution(&input, &[])?;
		if visitor(0, &solution).is_break() {
			return Ok(IndependentSelectionSearchStatus::VisitorStopped);
		}
		emitted_count = 1;
		if emitted_count == request.top_k {
			return Ok(IndependentSelectionSearchStatus::TopKReached);
		}
	}
	if input.candidates.is_empty() {
		return Ok(IndependentSelectionSearchStatus::Exhausted);
	}

	let maximum_cardinality: usize = maximum_operator_set_cardinality(&input);
	let mut assignment_states: BTreeMap<OperatorSet, AssignmentSearchState> = BTreeMap::new();

	'distinct_sets: for cardinality in 1..=maximum_cardinality {
		while emitted_count < request.top_k {
			let Some((operator_set, selected)) =
				discover_operator_set_configuration(&input, &mut assignment_states, cardinality)?
			else {
				break;
			};
			validate_selected_configuration(&input, &selected)?;
			if selected_operator_set(&input, &selected) != operator_set {
				return Err(solver_failure(
					"operator-set discovery returned a configuration from another roster",
				));
			}
			let solution: IndependentSelectionSolution = build_solution(&input, &selected)?;
			let solution_index: usize = emitted_count;
			emitted_count += 1;
			if visitor(solution_index, &solution).is_break() {
				return Ok(IndependentSelectionSearchStatus::VisitorStopped);
			}
			if emitted_count == request.top_k {
				break 'distinct_sets;
			}
		}
	}

	'variants: for cardinality in 1..=maximum_cardinality {
		while emitted_count < request.top_k {
			let Some((operator_set, selected)) =
				next_known_operator_set_configuration(&input, &mut assignment_states, cardinality)?
			else {
				break;
			};
			validate_selected_configuration(&input, &selected)?;
			if selected_operator_set(&input, &selected) != operator_set {
				return Err(solver_failure(
					"fixed-roster solver returned a configuration from another operator set",
				));
			}
			let solution: IndependentSelectionSolution = build_solution(&input, &selected)?;
			let solution_index: usize = emitted_count;
			emitted_count += 1;
			if visitor(solution_index, &solution).is_break() {
				return Ok(IndependentSelectionSearchStatus::VisitorStopped);
			}
			if emitted_count == request.top_k {
				break 'variants;
			}
		}
	}
	let exhausted: bool = emitted_count < request.top_k;

	if emitted_count == 0 {
		return Err(IndependentSelectionSolveError::Infeasible);
	}
	Ok(if exhausted {
		IndependentSelectionSearchStatus::Exhausted
	} else {
		IndependentSelectionSearchStatus::TopKReached
	})
}

/// Enumerates additive solutions across feasible operator counts in round-robin order.
///
/// This is an exploration policy for incomplete relaxations: a large number of optimistic
/// one-member variants must not prevent two- or three-member configurations from reaching the
/// authoritative replay gate. Within each fixed cardinality, deployment cost remains the
/// secondary objective and the caller-supplied ranking score the tertiary objective. Temporary
/// independent operator-set states diversify early proposals; each state then enumerates exact
/// candidate variants without importing cuts from another roster or cardinality.
pub fn solve_independent_selection_cardinality_round_robin(
	request: IndependentSelectionRequest<'_>,
) -> Result<IndependentSelectionSolveResult, IndependentSelectionSolveError> {
	let mut solutions: Vec<IndependentSelectionSolution> = Vec::new();
	let status: IndependentSelectionSearchStatus =
		visit_independent_selection_cardinality_round_robin(
			request,
			|_solution_index: usize, solution: &IndependentSelectionSolution| {
				solutions.push(solution.clone());
				ControlFlow::Continue(())
			},
		)?;
	Ok(IndependentSelectionSolveResult { status, solutions })
}

/// Streams round-robin cardinality solutions as soon as each MILP result is built.
pub fn visit_independent_selection_cardinality_round_robin<F>(
	request: IndependentSelectionRequest<'_>,
	mut visitor: F,
) -> Result<IndependentSelectionSearchStatus, IndependentSelectionSolveError>
where
	F: FnMut(usize, &IndependentSelectionSolution) -> ControlFlow<()>,
{
	let input: NormalizedInput<'_> = validate_and_normalize(request)?;
	let has_positive_demand: bool = input
		.demands
		.iter()
		.any(|demand| !demand.required.is_zero());

	if input.candidates.is_empty() && has_positive_demand {
		return Err(IndependentSelectionSolveError::Infeasible);
	}
	let mut emitted_count: usize = 0;
	if !has_positive_demand {
		let solution: IndependentSelectionSolution = build_solution(&input, &[])?;
		if visitor(0, &solution).is_break() {
			return Ok(IndependentSelectionSearchStatus::VisitorStopped);
		}
		emitted_count = 1;
		if emitted_count == request.top_k {
			return Ok(IndependentSelectionSearchStatus::TopKReached);
		}
	}
	if input.candidates.is_empty() {
		return Ok(IndependentSelectionSearchStatus::Exhausted);
	}

	let maximum_cardinality: usize = maximum_operator_set_cardinality(&input);
	let mut assignment_states: BTreeMap<OperatorSet, AssignmentSearchState> = BTreeMap::new();
	let mut exhausted_distinct_cardinalities: BTreeSet<usize> = BTreeSet::new();
	let mut exhausted: bool = false;
	let mut exploring_distinct_operator_sets: bool = true;

	while emitted_count < request.top_k {
		let mut found_in_round: bool = false;
		for cardinality in 1..=maximum_cardinality {
			let next: Option<(OperatorSet, Vec<usize>)> = if exploring_distinct_operator_sets {
				if exhausted_distinct_cardinalities.contains(&cardinality) {
					None
				} else {
					let discovered: Option<(OperatorSet, Vec<usize>)> =
						discover_operator_set_configuration(
							&input,
							&mut assignment_states,
							cardinality,
						)?;
					if discovered.is_none() {
						exhausted_distinct_cardinalities.insert(cardinality);
					}
					discovered
				}
			} else {
				next_known_operator_set_configuration(&input, &mut assignment_states, cardinality)?
			};
			let Some((operator_set, solution)) = next else {
				continue;
			};
			validate_selected_configuration(&input, &solution)?;
			if selected_operator_set(&input, &solution) != operator_set {
				return Err(solver_failure(
					"fixed-roster solver returned a configuration from another operator set",
				));
			}
			let built_solution: IndependentSelectionSolution = build_solution(&input, &solution)?;
			found_in_round = true;
			let solution_index: usize = emitted_count;
			emitted_count += 1;
			if visitor(solution_index, &built_solution).is_break() {
				return Ok(IndependentSelectionSearchStatus::VisitorStopped);
			}
			if emitted_count == request.top_k {
				break;
			}
		}
		if !found_in_round {
			if exploring_distinct_operator_sets {
				exploring_distinct_operator_sets = false;
				continue;
			}
			exhausted = true;
			break;
		}
	}

	if emitted_count == 0 {
		return Err(IndependentSelectionSolveError::Infeasible);
	}
	Ok(if exhausted {
		IndependentSelectionSearchStatus::Exhausted
	} else {
		IndependentSelectionSearchStatus::TopKReached
	})
}

fn maximum_operator_set_cardinality(input: &NormalizedInput<'_>) -> usize {
	input
		.operator_groups
		.len()
		.min(input.cell_groups.len())
		.min(input.candidates.len())
		.min(MAX_OPERATOR_SET_CARDINALITY)
}

fn discover_operator_set_configuration(
	input: &NormalizedInput<'_>,
	states: &mut BTreeMap<OperatorSet, AssignmentSearchState>,
	cardinality: usize,
) -> Result<Option<(OperatorSet, Vec<usize>)>, IndependentSelectionSolveError> {
	let excluded_operator_sets: Vec<OperatorSet> = states
		.keys()
		.filter(|operator_set| operator_set.len() == cardinality)
		.cloned()
		.collect();
	let Some(configuration) =
		solve_new_operator_set_model(input, cardinality, &excluded_operator_sets)?
	else {
		return Ok(None);
	};
	let operator_set: OperatorSet = selected_operator_set(input, &configuration);
	if operator_set.len() != cardinality {
		return Err(solver_failure(format!(
			"operator-set discovery selected {} operators for cardinality {cardinality}",
			operator_set.len()
		)));
	}
	if states.contains_key(&operator_set) {
		return Err(solver_failure(
			"operator-set discovery returned an already discovered roster",
		));
	}
	states.insert(
		operator_set.clone(),
		AssignmentSearchState::with_first_configuration(configuration.clone()),
	);
	Ok(Some((operator_set, configuration)))
}

fn next_known_operator_set_configuration(
	input: &NormalizedInput<'_>,
	states: &mut BTreeMap<OperatorSet, AssignmentSearchState>,
	cardinality: usize,
) -> Result<Option<(OperatorSet, Vec<usize>)>, IndependentSelectionSolveError> {
	let eligible_sets: Vec<OperatorSet> = states
		.iter()
		.filter(|(operator_set, state)| operator_set.len() == cardinality && !state.exhausted)
		.map(|(operator_set, _state)| operator_set.clone())
		.collect();
	let mut best: Option<(OperatorSet, Vec<usize>)> = None;
	for operator_set in eligible_sets {
		let state: &mut AssignmentSearchState = states
			.get_mut(&operator_set)
			.expect("eligible operator set must retain its search state");
		if state.next_configuration.is_none() {
			state.next_configuration =
				solve_operator_set_model(input, &operator_set, &state.excluded_configurations)?;
			if state.next_configuration.is_none() {
				state.exhausted = true;
				continue;
			}
		}
		let configuration: &Vec<usize> = state
			.next_configuration
			.as_ref()
			.expect("a non-exhausted materialized state must have a next configuration");
		let replace: bool = best.as_ref().is_none_or(|(_best_set, best_configuration)| {
			configuration_is_better(input, configuration, best_configuration)
		});
		if replace {
			best = Some((operator_set, configuration.clone()));
		}
	}

	let Some((operator_set, _configuration)) = best else {
		return Ok(None);
	};
	let state: &mut AssignmentSearchState = states
		.get_mut(&operator_set)
		.expect("selected operator set must retain its search state");
	let configuration: Vec<usize> = state
		.next_configuration
		.take()
		.expect("selected operator set must have a cached configuration");
	if state.excluded_configurations.contains(&configuration) {
		return Err(solver_failure(
			"fixed-roster solver returned an excluded candidate configuration",
		));
	}
	state.excluded_configurations.push(configuration.clone());
	state.emitted_count += 1;
	Ok(Some((operator_set, configuration)))
}

fn solve_new_operator_set_model(
	input: &NormalizedInput<'_>,
	fixed_cardinality: usize,
	excluded_operator_sets: &[OperatorSet],
) -> Result<Option<Vec<usize>>, IndependentSelectionSolveError> {
	if !input.enforce_demands && fixed_cardinality == 1 {
		let excluded_operator_groups: BTreeSet<usize> = excluded_operator_sets
			.iter()
			.flat_map(OperatorSet::iter)
			.collect();
		let cost_priority_weight: f64 = deployment_cost_priority_weight(input);
		let selected: Option<usize> = input
			.candidates
			.iter()
			.enumerate()
			.filter(|(candidate_index, _candidate)| {
				let operator_group: usize = input.operator_group_by_candidate[*candidate_index];
				!excluded_operator_groups.contains(&operator_group)
			})
			.min_by(|(left_index, left), (right_index, right)| {
				let left_coefficient: f64 =
					singleton_objective_coefficient(input, left, cost_priority_weight);
				let right_coefficient: f64 =
					singleton_objective_coefficient(input, right, cost_priority_weight);
				left_coefficient
					.total_cmp(&right_coefficient)
					.then_with(|| left_index.cmp(right_index))
			})
			.map(|(candidate_index, _candidate)| candidate_index);
		return Ok(selected.map(|candidate_index| vec![candidate_index]));
	}

	let mut problem_variables: ProblemVariables = ProblemVariables::new();
	let variables: Vec<Variable> = (0..input.candidates.len())
		.map(|_candidate_index| problem_variables.add(variable().binary()))
		.collect();
	let cost_priority_weight: f64 = deployment_cost_priority_weight(input);
	let mut objective: Expression = Expression::with_capacity(variables.len());
	for (candidate, &candidate_variable) in input.candidates.iter().zip(&variables) {
		objective.add_mul(
			f64::from(candidate.deployment_cost) * cost_priority_weight
				- normalized_ranking_score(input, candidate),
			candidate_variable,
		);
	}
	let mut model = problem_variables.minimise(objective).using(microlp);

	if input.enforce_demands {
		for demand in &input.demands {
			if demand.required.is_zero() {
				continue;
			}
			let coefficient_divisor: u64 =
				input
					.candidates
					.iter()
					.fold(demand.required.scaled(), |divisor, candidate| {
						let optimistic_damage: FixedDamage =
							candidate.optimistic_damage_by_constraint[&demand.constraint];
						greatest_common_divisor(
							divisor,
							optimistic_damage.scaled().min(demand.required.scaled()),
						)
					});
			let mut contribution: Expression = Expression::with_capacity(variables.len());
			for (candidate, &candidate_variable) in input.candidates.iter().zip(&variables) {
				let optimistic_damage: FixedDamage =
					candidate.optimistic_damage_by_constraint[&demand.constraint];
				let capped: u64 = optimistic_damage.scaled().min(demand.required.scaled());
				contribution.add_mul(
					damage_as_exact_f64(FixedDamage::from_scaled(capped / coefficient_divisor)),
					candidate_variable,
				);
			}
			model.add_constraint(
				contribution.geq(damage_as_exact_f64(FixedDamage::from_scaled(
					demand.required.scaled() / coefficient_divisor,
				))),
			);
		}
	}

	let cardinality: Expression = variables.iter().copied().sum();
	model.add_constraint(cardinality.eq(fixed_cardinality as f64));
	add_at_most_one_constraints(&mut model, &variables, &input.operator_groups);
	add_at_most_one_constraints(&mut model, &variables, &input.cell_groups);

	for excluded_operator_set in excluded_operator_sets {
		if excluded_operator_set.len() != fixed_cardinality {
			return Err(solver_failure(
				"operator-set discovery received a cut from another cardinality",
			));
		}
		let excluded_variables: Vec<Variable> = excluded_operator_set
			.iter()
			.flat_map(|operator_group| input.operator_groups[operator_group].iter().copied())
			.map(|candidate_index| variables[candidate_index])
			.collect();
		let exact_roster: Expression = excluded_variables.iter().copied().sum();
		model.add_constraint(exact_roster.leq((fixed_cardinality - 1) as f64));
	}

	let solution = match model.solve() {
		Ok(solution) => solution,
		Err(ResolutionError::Infeasible) => return Ok(None),
		Err(error) => return Err(solver_failure(error.to_string())),
	};
	if !matches!(solution.status(), SolutionStatus::Optimal) {
		return Err(solver_failure(format!(
			"microlp returned non-optimal operator-set discovery status {:?}",
			solution.status()
		)));
	}

	let mut selected: Vec<usize> = Vec::with_capacity(fixed_cardinality);
	for (candidate_index, candidate_variable) in variables.into_iter().enumerate() {
		let value: f64 = solution.value(candidate_variable);
		if !value.is_finite() {
			return Err(solver_failure(format!(
				"operator-set discovery variable for candidate {candidate_index} has non-finite value {value}"
			)));
		}
		if value.abs() <= BINARY_SOLUTION_TOLERANCE {
			continue;
		}
		if (value - 1.0).abs() <= BINARY_SOLUTION_TOLERANCE {
			selected.push(candidate_index);
			continue;
		}
		return Err(solver_failure(format!(
			"operator-set discovery variable for candidate {candidate_index} has non-integral value {value}"
		)));
	}
	selected.sort_unstable();
	if selected.len() != fixed_cardinality {
		return Err(solver_failure(format!(
			"operator-set discovery selected {} candidates for cardinality {fixed_cardinality}",
			selected.len()
		)));
	}
	Ok(Some(selected))
}

fn configuration_is_better(input: &NormalizedInput<'_>, left: &[usize], right: &[usize]) -> bool {
	configuration_objective_score(input, left)
		.total_cmp(&configuration_objective_score(input, right))
		.then_with(|| left.cmp(right))
		.is_lt()
}

fn configuration_objective_score(input: &NormalizedInput<'_>, selected: &[usize]) -> f64 {
	let weight: f64 = deployment_cost_priority_weight(input);
	selected
		.iter()
		.map(|&candidate_index| {
			let candidate: &IndependentContributionCandidate = input.candidates[candidate_index];
			f64::from(candidate.deployment_cost) * weight
				- normalized_ranking_score(input, candidate)
		})
		.sum()
}

fn solve_operator_set_model(
	input: &NormalizedInput<'_>,
	operator_set: &OperatorSet,
	excluded_configurations: &[Vec<usize>],
) -> Result<Option<Vec<usize>>, IndependentSelectionSolveError> {
	for excluded in excluded_configurations {
		if selected_operator_set(input, excluded) != *operator_set {
			return Err(solver_failure(
				"operator-set assignment state contains a configuration from another roster",
			));
		}
	}
	if !input.enforce_demands && operator_set.len() == 1 {
		let operator_group: usize = operator_set
			.iter()
			.next()
			.expect("operator sets used for enumeration are non-empty");
		let cost_priority_weight: f64 = deployment_cost_priority_weight(input);
		let selected: Option<Vec<usize>> = input.operator_groups[operator_group]
			.iter()
			.copied()
			.filter(|candidate_index| {
				!excluded_configurations
					.iter()
					.any(|excluded| excluded.as_slice() == [*candidate_index])
			})
			.min_by(|&left_index, &right_index| {
				let left: &IndependentContributionCandidate = input.candidates[left_index];
				let right: &IndependentContributionCandidate = input.candidates[right_index];
				let left_coefficient: f64 =
					singleton_objective_coefficient(input, left, cost_priority_weight);
				let right_coefficient: f64 =
					singleton_objective_coefficient(input, right, cost_priority_weight);
				left_coefficient
					.total_cmp(&right_coefficient)
					.then_with(|| left_index.cmp(&right_index))
			})
			.map(|candidate_index| vec![candidate_index]);
		return Ok(selected);
	}

	let mut candidate_indices: Vec<usize> = operator_set
		.iter()
		.flat_map(|operator_group| input.operator_groups[operator_group].iter().copied())
		.collect();
	candidate_indices.sort_unstable();
	let mut problem_variables: ProblemVariables = ProblemVariables::new();
	let variables: Vec<Variable> = candidate_indices
		.iter()
		.map(|_candidate_index| problem_variables.add(variable().binary()))
		.collect();
	let mut variable_by_candidate: Vec<Option<Variable>> = vec![None; input.candidates.len()];
	for (&candidate_index, &candidate_variable) in candidate_indices.iter().zip(&variables) {
		variable_by_candidate[candidate_index] = Some(candidate_variable);
	}

	let cost_priority_weight: f64 = deployment_cost_priority_weight(input);
	let mut objective: Expression = Expression::with_capacity(variables.len());
	for (&candidate_index, &candidate_variable) in candidate_indices.iter().zip(&variables) {
		let candidate: &IndependentContributionCandidate = input.candidates[candidate_index];
		objective.add_mul(
			f64::from(candidate.deployment_cost) * cost_priority_weight
				- normalized_ranking_score(input, candidate),
			candidate_variable,
		);
	}
	let mut model = problem_variables.minimise(objective).using(microlp);

	for operator_group in operator_set.iter() {
		let exactly_one: Expression = input.operator_groups[operator_group]
			.iter()
			.filter_map(|&candidate_index| variable_by_candidate[candidate_index])
			.sum();
		model.add_constraint(exactly_one.eq(1.0));
	}
	for cell_group in &input.cell_groups {
		let cell_variables: Vec<Variable> = cell_group
			.iter()
			.filter_map(|&candidate_index| variable_by_candidate[candidate_index])
			.collect();
		if cell_variables.len() > 1 {
			let at_most_one: Expression = cell_variables.iter().copied().sum();
			model.add_constraint(at_most_one.leq(1.0));
		}
	}

	if input.enforce_demands {
		for demand in &input.demands {
			if demand.required.is_zero() {
				continue;
			}
			let coefficient_divisor: u64 = candidate_indices.iter().fold(
				demand.required.scaled(),
				|divisor, &candidate_index| {
					let optimistic_damage: FixedDamage = input.candidates[candidate_index]
						.optimistic_damage_by_constraint[&demand.constraint];
					greatest_common_divisor(
						divisor,
						optimistic_damage.scaled().min(demand.required.scaled()),
					)
				},
			);
			let mut contribution: Expression = Expression::with_capacity(variables.len());
			for (&candidate_index, &candidate_variable) in candidate_indices.iter().zip(&variables)
			{
				let optimistic_damage: FixedDamage = input.candidates[candidate_index]
					.optimistic_damage_by_constraint[&demand.constraint];
				let capped: u64 = optimistic_damage.scaled().min(demand.required.scaled());
				contribution.add_mul(
					damage_as_exact_f64(FixedDamage::from_scaled(capped / coefficient_divisor)),
					candidate_variable,
				);
			}
			model.add_constraint(
				contribution.geq(damage_as_exact_f64(FixedDamage::from_scaled(
					demand.required.scaled() / coefficient_divisor,
				))),
			);
		}
	}

	for excluded in excluded_configurations {
		let selected_variables: Vec<Variable> = excluded
			.iter()
			.map(|&candidate_index| {
				variable_by_candidate[candidate_index]
					.expect("same-roster excluded candidate must exist in the fixed-roster model")
			})
			.collect();
		let exact_match: Expression = selected_variables.iter().copied().sum();
		model.add_constraint(exact_match.leq((operator_set.len() - 1) as f64));
	}

	let solution = match model.solve() {
		Ok(solution) => solution,
		Err(ResolutionError::Infeasible) => return Ok(None),
		Err(error) => return Err(solver_failure(error.to_string())),
	};
	if !matches!(solution.status(), SolutionStatus::Optimal) {
		return Err(solver_failure(format!(
			"microlp returned non-optimal fixed-roster status {:?}",
			solution.status()
		)));
	}

	let mut selected: Vec<usize> = Vec::with_capacity(operator_set.len());
	for (&candidate_index, candidate_variable) in candidate_indices.iter().zip(variables) {
		let value: f64 = solution.value(candidate_variable);
		if !value.is_finite() {
			return Err(solver_failure(format!(
				"fixed-roster binary variable for candidate {candidate_index} has non-finite value {value}"
			)));
		}
		if value.abs() <= BINARY_SOLUTION_TOLERANCE {
			continue;
		}
		if (value - 1.0).abs() <= BINARY_SOLUTION_TOLERANCE {
			selected.push(candidate_index);
			continue;
		}
		return Err(solver_failure(format!(
			"fixed-roster binary variable for candidate {candidate_index} has non-integral value {value}"
		)));
	}
	selected.sort_unstable();
	if selected.len() != operator_set.len() {
		return Err(solver_failure(format!(
			"fixed-roster solver selected {} candidates for {} operators",
			selected.len(),
			operator_set.len()
		)));
	}
	Ok(Some(selected))
}

fn validate_and_normalize(
	request: IndependentSelectionRequest<'_>,
) -> Result<NormalizedInput<'_>, IndependentSelectionInputError> {
	if request.demands.is_empty() {
		return Err(IndependentSelectionInputError::NoDemands);
	}
	if request.top_k == 0 {
		return Err(IndependentSelectionInputError::ZeroTopK);
	}

	let mut demands: Vec<&ContributionDemand> = request.demands.iter().collect();
	demands.sort_by(|left, right| left.constraint.cmp(&right.constraint));
	let mut demand_constraints: BTreeSet<ContributionConstraintKey> = BTreeSet::new();
	for demand in &demands {
		if !demand_constraints.insert(demand.constraint.clone()) {
			return Err(IndependentSelectionInputError::DuplicateDemandConstraint {
				constraint: demand.constraint.clone(),
			});
		}
		validate_damage(
			demand.required,
			FixedDamageInputLocation::Demand {
				constraint: demand.constraint.clone(),
			},
		)?;
	}

	let mut candidates: Vec<&IndependentContributionCandidate> =
		request.candidates.iter().collect();
	candidates.sort_by(|left, right| left.key.cmp(&right.key));
	let mut candidate_keys: BTreeSet<ContributionCandidateKey> = BTreeSet::new();
	for candidate in &candidates {
		if !candidate_keys.insert(candidate.key.clone()) {
			return Err(IndependentSelectionInputError::DuplicateCandidateKey {
				candidate_key: candidate.key.clone(),
			});
		}
		if candidate.operator_name.trim().is_empty() {
			return Err(IndependentSelectionInputError::BlankOperatorName {
				candidate_key: candidate.key.clone(),
			});
		}
		if candidate.ranking_score > MAX_EXACT_SELECTION_INTEGER {
			return Err(
				IndependentSelectionInputError::RankingScoreExceedsExactSolverIntegerRange {
					candidate_key: candidate.key.clone(),
					score: candidate.ranking_score,
				},
			);
		}

		for constraint in &demand_constraints {
			let Some(&optimistic_damage) =
				candidate.optimistic_damage_by_constraint.get(constraint)
			else {
				return Err(
					IndependentSelectionInputError::MissingIndependentContribution {
						candidate_key: candidate.key.clone(),
						constraint: constraint.clone(),
					},
				);
			};
			validate_damage(
				optimistic_damage,
				FixedDamageInputLocation::IndependentContribution {
					candidate_key: candidate.key.clone(),
					constraint: constraint.clone(),
				},
			)?;
		}
		for constraint in candidate.optimistic_damage_by_constraint.keys() {
			if !demand_constraints.contains(constraint) {
				return Err(
					IndependentSelectionInputError::UnexpectedIndependentContribution {
						candidate_key: candidate.key.clone(),
						constraint: constraint.clone(),
					},
				);
			}
		}
	}

	let mut operator_groups: BTreeMap<ContributionOperatorIdentity, Vec<usize>> = BTreeMap::new();
	let mut cell_groups: BTreeMap<GridPosition, Vec<usize>> = BTreeMap::new();
	for (index, candidate) in candidates.iter().enumerate() {
		operator_groups
			.entry(candidate.operator_identity.clone())
			.or_default()
			.push(index);
		cell_groups
			.entry(candidate.deployment_cell)
			.or_default()
			.push(index);
	}
	let operator_groups: Vec<Vec<usize>> = operator_groups.into_values().collect();
	let mut operator_group_by_candidate: Vec<usize> = vec![0; candidates.len()];
	for (group_index, group) in operator_groups.iter().enumerate() {
		for &candidate_index in group {
			operator_group_by_candidate[candidate_index] = group_index;
		}
	}

	let maximum_ranking_score: u64 = candidates
		.iter()
		.map(|candidate| candidate.ranking_score)
		.max()
		.unwrap_or(0);
	Ok(NormalizedInput {
		candidates,
		demands,
		enforce_demands: request.enforce_demands,
		maximum_ranking_score,
		operator_groups,
		operator_group_by_candidate,
		cell_groups: cell_groups.into_values().collect(),
	})
}

fn validate_damage(
	damage: FixedDamage,
	location: FixedDamageInputLocation,
) -> Result<(), IndependentSelectionInputError> {
	if damage.scaled() > MAX_EXACT_SELECTION_INTEGER {
		return Err(
			IndependentSelectionInputError::FixedDamageExceedsExactSolverIntegerRange {
				location,
				scaled: damage.scaled(),
			},
		);
	}
	Ok(())
}

fn normalized_ranking_score(
	input: &NormalizedInput<'_>,
	candidate: &IndependentContributionCandidate,
) -> f64 {
	if input.maximum_ranking_score == 0 {
		return 0.0;
	}
	candidate.ranking_score as f64 / input.maximum_ranking_score as f64
}

fn deployment_cost_priority_weight(input: &NormalizedInput<'_>) -> f64 {
	let maximum_cardinality: usize = maximum_operator_set_cardinality(input);
	// Each selected candidate contributes at most one normalized ranking unit. Therefore this
	// weight makes one DP dominate the largest possible ranking difference.
	maximum_cardinality as f64 + 1.0
}

fn selected_operator_set(input: &NormalizedInput<'_>, selected: &[usize]) -> OperatorSet {
	let mut operator_set: Vec<usize> = selected
		.iter()
		.map(|&candidate_index| input.operator_group_by_candidate[candidate_index])
		.collect();
	operator_set.sort_unstable();
	operator_set.dedup();
	debug_assert_eq!(
		operator_set.len(),
		selected.len(),
		"at-most-one constraints must select at most one variant per operator"
	);
	OperatorSet::new(operator_set)
}

fn singleton_objective_coefficient(
	input: &NormalizedInput<'_>,
	candidate: &IndependentContributionCandidate,
	cost_priority_weight: f64,
) -> f64 {
	f64::from(candidate.deployment_cost) * cost_priority_weight
		- normalized_ranking_score(input, candidate)
}

fn add_at_most_one_constraints(
	model: &mut good_lp::solvers::microlp::MicroLpProblem,
	variables: &[Variable],
	groups: &[Vec<usize>],
) {
	for group in groups.iter().filter(|group| group.len() > 1) {
		let expression: Expression = group.iter().map(|&index| variables[index]).sum();
		model.add_constraint(expression.leq(1.0));
	}
}

fn damage_as_exact_f64(damage: FixedDamage) -> f64 {
	debug_assert!(damage.scaled() <= MAX_EXACT_SELECTION_INTEGER);
	damage.scaled() as f64
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
	while right != 0 {
		let remainder: u64 = left % right;
		left = right;
		right = remainder;
	}
	left
}

fn validate_selected_configuration(
	input: &NormalizedInput<'_>,
	selected: &[usize],
) -> Result<(), IndependentSelectionSolveError> {
	let mut operators: BTreeSet<&ContributionOperatorIdentity> = BTreeSet::new();
	let mut cells: BTreeSet<GridPosition> = BTreeSet::new();
	for &index in selected {
		let candidate: &IndependentContributionCandidate = input.candidates[index];
		if !operators.insert(&candidate.operator_identity) {
			return Err(solver_failure(format!(
				"solver selected operator identity {} more than once",
				candidate.operator_identity
			)));
		}
		if !cells.insert(candidate.deployment_cell) {
			return Err(solver_failure(format!(
				"solver selected deployment cell {:?} more than once",
				candidate.deployment_cell
			)));
		}
	}

	if input.enforce_demands {
		for demand in &input.demands {
			let aggregate: u128 = selected
				.iter()
				.map(|&index| {
					u128::from(
						input.candidates[index].optimistic_damage_by_constraint[&demand.constraint]
							.scaled(),
					)
				})
				.sum();
			if aggregate < u128::from(demand.required.scaled()) {
				return Err(solver_failure(format!(
					"solver configuration leaves constraint {} below required damage",
					demand.constraint
				)));
			}
		}
	}
	Ok(())
}

fn build_solution(
	input: &NormalizedInput<'_>,
	selected: &[usize],
) -> Result<IndependentSelectionSolution, IndependentSelectionSolveError> {
	let mut selected_candidates: Vec<IndependentSelectionMember> =
		Vec::with_capacity(selected.len());
	let mut total_deployment_cost: u64 = 0;
	for &index in selected {
		let candidate: &IndependentContributionCandidate = input.candidates[index];
		total_deployment_cost = total_deployment_cost
			.checked_add(u64::from(candidate.deployment_cost))
			.ok_or_else(|| solver_failure("selected deployment cost overflowed u64"))?;
		selected_candidates.push(IndependentSelectionMember {
			candidate_key: candidate.key.clone(),
			operator_identity: candidate.operator_identity.clone(),
			operator_name: candidate.operator_name.clone(),
			deployment_cost: candidate.deployment_cost,
			deployment_cell: candidate.deployment_cell,
		});
	}
	selected_candidates.sort_by(|left, right| left.candidate_key.cmp(&right.candidate_key));

	let constraint_audit: Vec<ContributionConstraintAudit> = input
		.demands
		.iter()
		.map(|demand| {
			let aggregate: u128 = selected
				.iter()
				.map(|&index| {
					u128::from(
						input.candidates[index].optimistic_damage_by_constraint[&demand.constraint]
							.scaled(),
					)
				})
				.sum();
			let capped_scaled: u64 =
				u64::try_from(aggregate.min(u128::from(demand.required.scaled())))
					.expect("capped contribution cannot exceed a u64 FixedDamage demand");
			ContributionConstraintAudit {
				constraint: demand.constraint.clone(),
				required: demand.required,
				aggregate_capped_optimistic_damage: FixedDamage::from_scaled(capped_scaled),
				remaining_demand: FixedDamage::from_scaled(
					demand.required.scaled() - capped_scaled,
				),
			}
		})
		.collect();

	Ok(IndependentSelectionSolution {
		operator_count: selected_candidates.len(),
		total_deployment_cost,
		selected_candidates,
		constraint_audit,
	})
}

fn solver_failure(message: impl Into<String>) -> IndependentSelectionSolveError {
	IndependentSelectionSolveError::SolverFailure {
		message: message.into(),
	}
}

#[cfg(test)]
mod tests {
	use super::{
		discover_operator_set_configuration, maximum_operator_set_cardinality,
		next_known_operator_set_configuration, solve_independent_selection,
		solve_independent_selection_cardinality_round_robin, solve_new_operator_set_model,
		solve_operator_set_model, validate_and_normalize, visit_independent_selection,
		visit_independent_selection_cardinality_round_robin, AssignmentSearchState,
		ContributionCandidateKey, ContributionConstraintKey, ContributionDemand,
		ContributionOperatorIdentity, FixedDamageInputLocation, IndependentContributionCandidate,
		IndependentSelectionInputError, IndependentSelectionRequest,
		IndependentSelectionSearchStatus, IndependentSelectionSolveError, OperatorSet,
		MAX_EXACT_SELECTION_INTEGER,
	};
	use crate::data::GridPosition;
	use crate::mechanics::{
		domain::FixedDamage,
		stage::{SpawnCohortId, SpawnInstanceId},
	};
	use std::{collections::BTreeMap, ops::ControlFlow};

	fn target(id: &str) -> SpawnInstanceId {
		SpawnInstanceId {
			cohort: SpawnCohortId::new(id.to_owned()).expect("test cohort id should be valid"),
			ordinal: 0,
		}
	}

	fn damage(whole: u64) -> FixedDamage {
		FixedDamage::from_whole(whole).expect("test damage should fit")
	}

	fn constraint(target: &SpawnInstanceId) -> ContributionConstraintKey {
		ContributionConstraintKey::enemy(target.clone())
	}

	fn demand(target: &SpawnInstanceId, required: u64) -> ContributionDemand {
		ContributionDemand {
			constraint: constraint(target),
			required: damage(required),
		}
	}

	fn candidate(
		key: &str,
		operator_identity: &str,
		cost: u32,
		cell_col: u32,
		target: &SpawnInstanceId,
		credited: u64,
	) -> IndependentContributionCandidate {
		IndependentContributionCandidate {
			key: ContributionCandidateKey::new(key).expect("test candidate key should be valid"),
			operator_identity: ContributionOperatorIdentity::new(operator_identity)
				.expect("test operator identity should be valid"),
			operator_name: operator_identity.to_owned(),
			deployment_cost: cost,
			deployment_cell: GridPosition {
				row: 0,
				col: cell_col,
			},
			optimistic_damage_by_constraint: BTreeMap::from([(
				constraint(target),
				damage(credited),
			)]),
			ranking_score: credited,
		}
	}

	fn request<'a>(
		candidates: &'a [IndependentContributionCandidate],
		demands: &'a [ContributionDemand],
		top_k: usize,
	) -> IndependentSelectionRequest<'a> {
		IndependentSelectionRequest {
			candidates,
			demands,
			enforce_demands: true,
			top_k,
		}
	}

	#[test]
	fn one_operator_can_satisfy_the_requirement() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> =
			vec![candidate("a", "operator-a", 10, 0, &enemy, 100)];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("one candidate should be feasible");

		assert_eq!(result.status, IndependentSelectionSearchStatus::TopKReached);
		assert_eq!(result.solutions[0].operator_count, 1);
		assert_eq!(
			result.solutions[0].selected_candidates[0]
				.candidate_key
				.as_str(),
			"a"
		);
	}

	#[test]
	fn combines_two_operators_when_neither_is_sufficient() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a", "operator-a", 10, 0, &enemy, 60),
			candidate("b", "operator-b", 12, 1, &enemy, 40),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("the pair should be feasible");

		assert_eq!(result.solutions[0].operator_count, 2);
		assert_eq!(result.solutions[0].total_deployment_cost, 22);
	}

	#[test]
	fn enforces_each_constraint_independently() {
		let first_enemy: SpawnInstanceId = target("first-enemy");
		let second_enemy: SpawnInstanceId = target("second-enemy");
		let mut first: IndependentContributionCandidate =
			candidate("a", "operator-a", 10, 0, &first_enemy, 100);
		first
			.optimistic_damage_by_constraint
			.insert(constraint(&second_enemy), FixedDamage::ZERO);
		let mut second: IndependentContributionCandidate =
			candidate("b", "operator-b", 12, 1, &first_enemy, 0);
		second
			.optimistic_damage_by_constraint
			.insert(constraint(&second_enemy), damage(100));
		let candidates: Vec<IndependentContributionCandidate> = vec![first, second];
		let demands: Vec<ContributionDemand> =
			vec![demand(&first_enemy, 100), demand(&second_enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("both target-specific candidates should be selected");

		assert_eq!(result.solutions[0].operator_count, 2);
		assert!(result.solutions[0]
			.constraint_audit
			.iter()
			.all(|audit| audit.remaining_demand.is_zero()));
	}

	#[test]
	fn cumulative_group_allows_damage_reallocation_across_enemy_ids() {
		let group: ContributionConstraintKey =
			ContributionConstraintKey::space_time_hp_group("deadline-120")
				.expect("group id should be valid");
		let candidate: IndependentContributionCandidate = IndependentContributionCandidate {
			key: ContributionCandidateKey::new("candidate").expect("candidate key"),
			operator_identity: ContributionOperatorIdentity::new("operator")
				.expect("operator identity"),
			operator_name: "operator".to_owned(),
			deployment_cost: 10,
			deployment_cell: GridPosition { row: 0, col: 0 },
			optimistic_damage_by_constraint: BTreeMap::from([(group.clone(), damage(100))]),
			ranking_score: 100,
		};
		let demands: Vec<ContributionDemand> = vec![ContributionDemand {
			constraint: group,
			required: damage(100),
		}];

		let result = solve_independent_selection(request(&[candidate], &demands, 1))
			.expect("group capacity should not encode which member receives the damage");

		assert_eq!(result.solutions[0].operator_count, 1);
	}

	#[test]
	fn zero_demand_starts_with_empty_then_continues_standard_enumeration() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> =
			vec![candidate("candidate", "operator", 10, 0, &enemy, 0)];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 0)];

		let result = solve_independent_selection(request(&candidates, &demands, 2))
			.expect("zero demand should admit the empty proposal and later alternatives");

		assert_eq!(result.status, IndependentSelectionSearchStatus::TopKReached);
		assert_eq!(result.solutions.len(), 2);
		assert_eq!(result.solutions[0].operator_count, 0);
		assert_eq!(result.solutions[1].operator_count, 1);
	}

	#[test]
	fn zero_demand_starts_with_empty_then_continues_round_robin_enumeration() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> =
			vec![candidate("candidate", "operator", 10, 0, &enemy, 0)];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 0)];

		let result =
			solve_independent_selection_cardinality_round_robin(request(&candidates, &demands, 2))
				.expect("round-robin zero demand should preserve the empty optimum");

		assert_eq!(result.status, IndependentSelectionSearchStatus::TopKReached);
		assert_eq!(result.solutions.len(), 2);
		assert_eq!(result.solutions[0].operator_count, 0);
		assert_eq!(result.solutions[1].operator_count, 1);
	}

	#[test]
	fn unenforced_demands_rank_incomplete_candidates_without_pruning_them() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> =
			vec![candidate("candidate", "operator", 10, 0, &enemy, 90)];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(IndependentSelectionRequest {
			candidates: &candidates,
			demands: &demands,
			enforce_demands: false,
			top_k: 1,
		})
		.expect("an incomplete damage model must preserve a replay candidate");

		assert_eq!(result.solutions[0].operator_count, 1);
		assert_eq!(
			result.solutions[0].constraint_audit[0].remaining_demand,
			damage(10)
		);
	}

	#[test]
	fn rejects_two_builds_of_the_same_operator() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a", "same-operator", 10, 0, &enemy, 60),
			candidate("b", "same-operator", 11, 1, &enemy, 40),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		assert_eq!(
			solve_independent_selection(request(&candidates, &demands, 1)),
			Err(IndependentSelectionSolveError::Infeasible)
		);
	}

	#[test]
	fn rejects_two_operators_on_the_same_cell() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a", "operator-a", 10, 0, &enemy, 60),
			candidate("b", "operator-b", 11, 0, &enemy, 40),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		assert_eq!(
			solve_independent_selection(request(&candidates, &demands, 1)),
			Err(IndependentSelectionSolveError::Infeasible)
		);
	}

	#[test]
	fn solver_does_not_invent_an_operator_limit() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a", "operator-a", 10, 0, &enemy, 60),
			candidate("b", "operator-b", 11, 1, &enemy, 40),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("stage legality, not this relaxation, owns deployment limits");

		assert_eq!(result.solutions[0].operator_count, 2);
	}

	#[test]
	fn cardinality_has_priority_over_deployment_cost() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("expensive", "operator-expensive", 100, 0, &enemy, 100),
			candidate("cheap-a", "operator-cheap-a", 1, 1, &enemy, 60),
			candidate("cheap-b", "operator-cheap-b", 1, 2, &enemy, 40),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("a feasible solution should exist");

		assert_eq!(result.solutions[0].operator_count, 1);
		assert_eq!(
			result.solutions[0].selected_candidates[0]
				.candidate_key
				.as_str(),
			"expensive"
		);
	}

	#[test]
	fn deployment_cost_breaks_equal_cardinality_ties() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("expensive", "operator-expensive", 20, 0, &enemy, 100),
			candidate("cheap", "operator-cheap", 8, 1, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("a feasible solution should exist");

		assert_eq!(result.solutions[0].total_deployment_cost, 8);
		assert_eq!(
			result.solutions[0].selected_candidates[0]
				.candidate_key
				.as_str(),
			"cheap"
		);
	}

	#[test]
	fn ranking_score_breaks_equal_cardinality_and_cost_ties() {
		let small_enemy: SpawnInstanceId = target("small-enemy");
		let large_enemy: SpawnInstanceId = target("large-enemy");
		let mut low_normalized_a: IndependentContributionCandidate =
			candidate("a-low", "operator-a", 5, 0, &small_enemy, 50);
		low_normalized_a
			.optimistic_damage_by_constraint
			.insert(constraint(&large_enemy), damage(9_000));
		let mut low_normalized_b: IndependentContributionCandidate =
			candidate("b-low", "operator-b", 5, 1, &small_enemy, 50);
		low_normalized_b
			.optimistic_damage_by_constraint
			.insert(constraint(&large_enemy), damage(9_000));
		let mut high_normalized_a: IndependentContributionCandidate =
			candidate("c-high", "operator-a", 5, 1, &small_enemy, 100);
		high_normalized_a
			.optimistic_damage_by_constraint
			.insert(constraint(&large_enemy), damage(5_000));
		let mut high_normalized_b: IndependentContributionCandidate =
			candidate("d-high", "operator-b", 5, 0, &small_enemy, 100);
		high_normalized_b
			.optimistic_damage_by_constraint
			.insert(constraint(&large_enemy), damage(5_000));
		let candidates: Vec<IndependentContributionCandidate> = vec![
			low_normalized_a,
			low_normalized_b,
			high_normalized_a,
			high_normalized_b,
		];
		let demands: Vec<ContributionDemand> =
			vec![demand(&small_enemy, 100), demand(&large_enemy, 10_000)];

		let result = solve_independent_selection(request(&candidates, &demands, 2))
			.expect("both equal-cost pairs should be feasible");
		let selected_keys: Vec<Vec<&str>> = result
			.solutions
			.iter()
			.map(|solution| {
				solution
					.selected_candidates
					.iter()
					.map(|member| member.candidate_key.as_str())
					.collect()
			})
			.collect();

		assert_eq!(
			selected_keys,
			vec![vec!["c-high", "d-high"], vec!["a-low", "b-low"]]
		);
	}

	#[test]
	fn aggregate_audit_caps_at_requirement_and_reports_remaining_demand() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a", "operator-a", 10, 0, &enemy, 70),
			candidate("b", "operator-b", 11, 1, &enemy, 70),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 1))
			.expect("the pair should be feasible");
		let audit = &result.solutions[0].constraint_audit[0];

		assert_eq!(audit.aggregate_capped_optimistic_damage, damage(100));
		assert_eq!(audit.remaining_demand, FixedDamage::ZERO);
	}

	#[test]
	fn top_k_uses_distinct_exact_configurations() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("cheap", "operator-cheap", 8, 0, &enemy, 100),
			candidate("expensive", "operator-expensive", 20, 1, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 2))
			.expect("two distinct solutions should exist");

		assert_eq!(result.status, IndependentSelectionSearchStatus::TopKReached);
		assert_eq!(result.solutions.len(), 2);
		assert_eq!(result.solutions[0].total_deployment_cost, 8);
		assert_eq!(result.solutions[1].total_deployment_cost, 20);
	}

	#[test]
	fn visitor_observes_first_solution_and_can_stop_before_later_enumeration() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("cheap", "operator-cheap", 8, 0, &enemy, 100),
			candidate("expensive", "operator-expensive", 20, 1, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];
		let mut observed_keys: Vec<String> = Vec::new();

		let status: IndependentSelectionSearchStatus = visit_independent_selection(
			request(&candidates, &demands, 2),
			|solution_index: usize, solution| {
				assert_eq!(solution_index, 0);
				observed_keys.push(
					solution.selected_candidates[0]
						.candidate_key
						.as_str()
						.to_owned(),
				);
				ControlFlow::Break(())
			},
		)
		.expect("the first proposal should be streamed");

		assert_eq!(status, IndependentSelectionSearchStatus::VisitorStopped);
		assert_eq!(observed_keys, vec!["cheap"]);
	}

	#[test]
	fn distinct_operator_sets_precede_exact_candidate_variants() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a-cheap", "operator-a", 1, 0, &enemy, 100),
			candidate("a-variant", "operator-a", 2, 1, &enemy, 100),
			candidate("b", "operator-b", 10, 2, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result = solve_independent_selection(request(&candidates, &demands, 4))
			.expect("distinct operator sets and exact variants should be enumerable");
		let selected_keys: Vec<Vec<&str>> = result
			.solutions
			.iter()
			.map(|solution| {
				solution
					.selected_candidates
					.iter()
					.map(|member| member.candidate_key.as_str())
					.collect()
			})
			.collect();

		assert_eq!(
			selected_keys,
			vec![
				vec!["a-cheap"],
				vec!["b"],
				vec!["a-cheap", "b"],
				vec!["a-variant"],
			]
		);
	}

	#[test]
	fn round_robin_reaches_joint_cardinalities_before_exhausting_single_variants() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("single-a", "operator-a", 1, 0, &enemy, 100),
			candidate("single-b", "operator-b", 2, 1, &enemy, 100),
			candidate("pair-a", "operator-c", 3, 2, &enemy, 60),
			candidate("pair-b", "operator-d", 4, 3, &enemy, 40),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result =
			solve_independent_selection_cardinality_round_robin(request(&candidates, &demands, 2))
				.expect("round-robin exploration should find both cardinality layers");

		assert_eq!(result.solutions.len(), 2);
		assert_eq!(result.solutions[0].operator_count, 1);
		assert_eq!(result.solutions[1].operator_count, 2);
	}

	#[test]
	fn round_robin_falls_back_to_variants_after_distinct_operator_sets() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a-cheap", "operator-a", 1, 0, &enemy, 100),
			candidate("a-variant", "operator-a", 2, 1, &enemy, 100),
			candidate("b", "operator-b", 10, 2, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		let result =
			solve_independent_selection_cardinality_round_robin(request(&candidates, &demands, 4))
				.expect("round-robin search should fall back to exact variants");
		let selected_keys: Vec<Vec<&str>> = result
			.solutions
			.iter()
			.map(|solution| {
				solution
					.selected_candidates
					.iter()
					.map(|member| member.candidate_key.as_str())
					.collect()
			})
			.collect();

		assert_eq!(
			selected_keys,
			vec![
				vec!["a-cheap"],
				vec!["a-cheap", "b"],
				vec!["b"],
				vec!["a-variant"],
			]
		);
	}

	#[test]
	fn singleton_discovery_fast_path_matches_microlp_on_a_small_domain() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a", "operator-a", 9, 0, &enemy, 100),
			candidate("b", "operator-b", 8, 1, &enemy, 100),
			candidate("c", "operator-c", 10, 2, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];
		let fast_input = validate_and_normalize(IndependentSelectionRequest {
			candidates: &candidates,
			demands: &demands,
			enforce_demands: false,
			top_k: 1,
		})
		.expect("small singleton domain should validate");
		let milp_input = validate_and_normalize(IndependentSelectionRequest {
			candidates: &candidates,
			demands: &demands,
			enforce_demands: true,
			top_k: 1,
		})
		.expect("the same enforced singleton domain should validate");

		let fast = solve_new_operator_set_model(&fast_input, 1, &[])
			.expect("singleton fast path should solve");
		let milp = solve_new_operator_set_model(&milp_input, 1, &[])
			.expect("MicroLP singleton path should solve");

		assert_eq!(fast, milp);
		assert_eq!(fast, Some(vec![1]));
	}

	#[test]
	fn singleton_fast_path_streams_later_variants_when_visitor_continues() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a-best", "operator-a", 8, 0, &enemy, 100),
			candidate("a-next", "operator-a", 9, 1, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];
		let mut observed: Vec<String> = Vec::new();

		let status: IndependentSelectionSearchStatus =
			visit_independent_selection_cardinality_round_robin(
				IndependentSelectionRequest {
					candidates: &candidates,
					demands: &demands,
					enforce_demands: false,
					top_k: 2,
				},
				|_solution_index, solution| {
					observed.push(
						solution.selected_candidates[0]
							.candidate_key
							.as_str()
							.to_owned(),
					);
					ControlFlow::Continue(())
				},
			)
			.expect("continuing visitor should receive exact singleton variants");

		assert_eq!(status, IndependentSelectionSearchStatus::TopKReached);
		assert_eq!(observed, vec!["a-best", "a-next"]);
	}

	#[test]
	fn operator_set_states_are_created_only_for_discovered_rosters() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = (0..12)
			.map(|index| {
				candidate(
					&format!("candidate-{index:02}"),
					&format!("operator-{index:02}"),
					10,
					index,
					&enemy,
					100,
				)
			})
			.collect();
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];
		let input = validate_and_normalize(IndependentSelectionRequest {
			candidates: &candidates,
			demands: &demands,
			enforce_demands: false,
			top_k: 1,
		})
		.expect("twelve-operator state domain should validate");
		let mut states: BTreeMap<OperatorSet, AssignmentSearchState> = BTreeMap::new();

		let first = discover_operator_set_configuration(&input, &mut states, 1)
			.expect("first roster discovery should solve")
			.expect("one singleton roster should exist");
		let second = discover_operator_set_configuration(&input, &mut states, 1)
			.expect("second roster discovery should solve")
			.expect("another singleton roster should exist");

		assert_ne!(first.0, second.0);
		assert_eq!(states.len(), 2);
		assert!(states.values().all(|state| state.emitted_count == 1));
		assert!(states
			.values()
			.all(|state| state.excluded_configurations.len() == 1));
	}

	#[test]
	fn candidate_pool_above_squad_limit_is_accepted() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = (0..13)
			.map(|index| {
				candidate(
					&format!("candidate-{index:02}"),
					&format!("operator-{index:02}"),
					10,
					index,
					&enemy,
					100,
				)
			})
			.collect();
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];
		let request = IndependentSelectionRequest {
			candidates: &candidates,
			demands: &demands,
			enforce_demands: false,
			top_k: 1,
		};
		let input = validate_and_normalize(request)
			.expect("candidate pools may contain more than one squad of operators");
		let result = solve_independent_selection_cardinality_round_robin(request)
			.expect("the selected operator set, not the candidate pool, owns the squad cap");

		assert_eq!(maximum_operator_set_cardinality(&input), 12);
		assert_eq!(result.solutions.len(), 1);
		assert_eq!(result.solutions[0].operator_count, 1);
	}

	#[test]
	fn operator_set_states_advance_their_assignment_frontiers_independently() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> = vec![
			candidate("a-best", "operator-a", 8, 0, &enemy, 100),
			candidate("a-next", "operator-a", 9, 1, &enemy, 100),
			candidate("b-best", "operator-b", 10, 2, &enemy, 100),
			candidate("b-next", "operator-b", 11, 3, &enemy, 100),
		];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];
		let input = validate_and_normalize(IndependentSelectionRequest {
			candidates: &candidates,
			demands: &demands,
			enforce_demands: false,
			top_k: 3,
		})
		.expect("fixed-roster locality domain should validate");
		let operator_a = OperatorSet::new(vec![0]);
		let operator_b = OperatorSet::new(vec![1]);
		let first_a = solve_operator_set_model(&input, &operator_a, &[])
			.expect("operator A should solve")
			.expect("operator A should have a first assignment");
		let first_b = solve_operator_set_model(&input, &operator_b, &[])
			.expect("operator B should solve")
			.expect("operator B should have a first assignment");
		let mut states: BTreeMap<OperatorSet, AssignmentSearchState> = BTreeMap::from([
			(
				operator_a.clone(),
				AssignmentSearchState::with_first_configuration(first_a),
			),
			(
				operator_b.clone(),
				AssignmentSearchState::with_first_configuration(first_b),
			),
		]);

		let (first_set, first_variant) =
			next_known_operator_set_configuration(&input, &mut states, 1)
				.expect("the independent frontiers should solve")
				.expect("operator A should own the cheapest next assignment");
		assert_eq!(first_set, operator_a);
		assert_eq!(input.candidates[first_variant[0]].key.as_str(), "a-next");
		assert_eq!(states[&operator_a].excluded_configurations.len(), 2);
		assert_eq!(states[&operator_b].excluded_configurations.len(), 1);

		let (second_set, second_variant) =
			next_known_operator_set_configuration(&input, &mut states, 1)
				.expect("the remaining independent frontier should solve")
				.expect("operator B should retain its own next assignment");
		assert_eq!(second_set, operator_b);
		assert_eq!(input.candidates[second_variant[0]].key.as_str(), "b-next");
		assert_eq!(states[&operator_a].excluded_configurations.len(), 2);
		assert_eq!(states[&operator_b].excluded_configurations.len(), 2);
	}

	#[test]
	fn rejects_zero_top_k() {
		let enemy: SpawnInstanceId = target("enemy");
		let candidates: Vec<IndependentContributionCandidate> =
			vec![candidate("a", "operator-a", 10, 0, &enemy, 100)];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 100)];

		assert_eq!(
			solve_independent_selection(request(&candidates, &demands, 0)),
			Err(IndependentSelectionSolveError::InvalidInput(
				IndependentSelectionInputError::ZeroTopK
			))
		);
	}

	#[test]
	fn damage_above_exact_f64_integer_range_is_invalid() {
		let enemy: SpawnInstanceId = target("enemy");
		let mut too_large: IndependentContributionCandidate =
			candidate("a", "operator-a", 10, 0, &enemy, 1);
		too_large.optimistic_damage_by_constraint.insert(
			constraint(&enemy),
			FixedDamage::from_scaled(MAX_EXACT_SELECTION_INTEGER + 1),
		);
		let candidates: Vec<IndependentContributionCandidate> = vec![too_large];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 1)];

		assert_eq!(
			solve_independent_selection(request(&candidates, &demands, 1)),
			Err(IndependentSelectionSolveError::InvalidInput(
				IndependentSelectionInputError::FixedDamageExceedsExactSolverIntegerRange {
					location: FixedDamageInputLocation::IndependentContribution {
						candidate_key: ContributionCandidateKey::new("a")
							.expect("test candidate key should be valid"),
						constraint: constraint(&enemy),
					},
					scaled: MAX_EXACT_SELECTION_INTEGER + 1,
				}
			))
		);
	}

	#[test]
	fn ranking_score_above_exact_f64_integer_range_is_invalid() {
		let enemy: SpawnInstanceId = target("enemy");
		let mut too_large: IndependentContributionCandidate =
			candidate("a", "operator-a", 10, 0, &enemy, 1);
		too_large.ranking_score = MAX_EXACT_SELECTION_INTEGER + 1;
		let candidates: Vec<IndependentContributionCandidate> = vec![too_large];
		let demands: Vec<ContributionDemand> = vec![demand(&enemy, 1)];

		assert_eq!(
			solve_independent_selection(request(&candidates, &demands, 1)),
			Err(IndependentSelectionSolveError::InvalidInput(
				IndependentSelectionInputError::RankingScoreExceedsExactSolverIntegerRange {
					candidate_key: ContributionCandidateKey::new("a")
						.expect("test candidate key should be valid"),
					score: MAX_EXACT_SELECTION_INTEGER + 1,
				}
			))
		);
	}
}
