use std::collections::BTreeMap;

use crate::simulation::result::CHOICE_MASS_TOLERANCE;
use crate::simulation::scheduler::{EventQueue, ScheduleError};
use crate::simulation::{
	ChoiceId, ModelStep, Policy, PolicyDecision, SimTick, SimulationIssue, SimulationModel,
	Simulator, Transition, Verification, Weighted,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ProbabilityAssessment<O> {
	pub verification: Verification,
	pub success_probability: f64,
	pub covered_probability_mass: f64,
	pub outcome_weights: BTreeMap<O, f64>,
	pub issues: Vec<SimulationIssue>,
	pub processed_steps: usize,
	pub peak_branches: usize,
}

impl<O: Clone + Ord> ProbabilityAssessment<O> {
	fn new() -> Self {
		Self {
			verification: Verification::Unverified,
			success_probability: 0.0,
			covered_probability_mass: 0.0,
			outcome_weights: BTreeMap::new(),
			issues: Vec::new(),
			processed_steps: 0,
			peak_branches: 0,
		}
	}

	fn record_terminal<M: SimulationModel<Outcome = O>>(
		&mut self,
		model: &M,
		outcome: O,
		mass: f64,
	) {
		let outcome_mass = self.outcome_weights.entry(outcome.clone()).or_default();
		let next_outcome_mass = *outcome_mass + mass;
		let next_covered = self.covered_probability_mass + mass;
		let next_success = if model.is_success(&outcome) {
			self.success_probability + mass
		} else {
			self.success_probability
		};
		if !next_outcome_mass.is_finite() || !next_covered.is_finite() || !next_success.is_finite()
		{
			self.issues.push(SimulationIssue::NonFiniteArithmetic);
			return;
		}
		*outcome_mass = next_outcome_mass;
		self.covered_probability_mass = next_covered;
		self.success_probability = next_success;
	}

	fn finish(mut self) -> Self {
		if self.issues.is_empty()
			&& (self.covered_probability_mass - 1.0).abs() <= CHOICE_MASS_TOLERANCE
		{
			self.verification = Verification::Verified;
		} else if self.issues.is_empty() {
			self.issues
				.push(SimulationIssue::IncompleteProbabilityMass {
					mass: self.covered_probability_mass,
				});
		}
		self
	}
}

struct Branch<S, P, E> {
	state: S,
	policy: P,
	now: SimTick,
	queue: EventQueue<E>,
	mass: f64,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct BranchKey<SK, PK, E> {
	now: SimTick,
	state: SK,
	policy: PK,
	queue: EventQueue<E>,
}

type BranchIndexes<SK, PK, E> = BTreeMap<BranchKey<SK, PK, E>, usize>;

impl Simulator {
	pub fn evaluate<M, P>(
		&self,
		model: &M,
		scenario: &M::Scenario,
		policy: P,
	) -> ProbabilityAssessment<M::Outcome>
	where
		M: SimulationModel,
		P: Policy<M>,
	{
		let mut assessment = ProbabilityAssessment::new();
		let initial = match model.initialize(scenario) {
			Ok(transition) => transition,
			Err(error) => {
				assessment.issues.push(SimulationIssue::Model(error));
				return assessment;
			}
		};
		let (state, events) = initial.into_parts();
		let mut queue = EventQueue::new();
		if let Err(error) = queue.schedule_all(SimTick::new(0), events) {
			assessment.issues.push(schedule_issue(error));
			return assessment;
		}
		if self.limits.max_branches == 0 {
			assessment.issues.push(SimulationIssue::BranchLimit {
				limit: self.limits.max_branches,
			});
			return assessment;
		}
		let mut frontier = vec![Branch {
			state,
			policy,
			now: SimTick::new(0),
			queue,
			mass: 1.0,
		}];
		assessment.peak_branches = 1;

		while !frontier.is_empty() {
			let mut next = Vec::new();
			let mut indexes = BTreeMap::new();
			for mut branch in frontier {
				if let Some(outcome) = model.terminal(&branch.state) {
					assessment.record_terminal(model, outcome, branch.mass);
					continue;
				}
				if assessment.processed_steps >= self.limits.max_steps {
					assessment.issues.push(SimulationIssue::StepLimit {
						limit: self.limits.max_steps,
					});
					return assessment;
				}
				assessment.processed_steps += 1;
				let observation = model.observe(&branch.state, branch.now);
				let step = match branch.policy.next_action(&observation) {
					PolicyDecision::Command(command) => {
						model.apply_command(&branch.state, &command, branch.now)
					}
					PolicyDecision::Wait => {
						let Some(event) = branch.queue.pop() else {
							assessment
								.issues
								.push(SimulationIssue::Deadlock { at: branch.now });
							continue;
						};
						branch.now = event.at();
						model.apply_event(&branch.state, &event)
					}
				};
				let step = match step {
					Ok(step) => step,
					Err(error) => {
						assessment.issues.push(SimulationIssue::Model(error));
						continue;
					}
				};

				match step {
					ModelStep::Deterministic(transition) => insert_transition(
						model,
						&mut assessment,
						&mut next,
						&mut indexes,
						branch,
						transition,
						1.0,
					),
					ModelStep::Choice {
						choice_id,
						branches,
					} => {
						if !valid_choice_mass(&mut assessment, choice_id, &branches) {
							continue;
						}
						for weighted in branches {
							let (probability, transition) = weighted.into_parts();
							if probability == 0.0 {
								continue;
							}
							insert_transition(
								model,
								&mut assessment,
								&mut next,
								&mut indexes,
								Branch {
									state: branch.state.clone(),
									policy: branch.policy.clone(),
									now: branch.now,
									queue: branch.queue.clone(),
									mass: branch.mass,
								},
								transition,
								probability,
							);
						}
					}
				}
				if next.len() > self.limits.max_branches {
					assessment.issues.push(SimulationIssue::BranchLimit {
						limit: self.limits.max_branches,
					});
					return assessment;
				}
			}
			assessment.peak_branches = assessment.peak_branches.max(next.len());
			frontier = next;
		}

		assessment.finish()
	}
}

fn valid_choice_mass<O, T>(
	assessment: &mut ProbabilityAssessment<O>,
	choice_id: ChoiceId,
	branches: &[Weighted<T>],
) -> bool
where
	O: Clone + Ord,
{
	let Some(mass) = branches.iter().try_fold(0.0, |sum, branch| {
		let next = sum + branch.probability();
		next.is_finite().then_some(next)
	}) else {
		assessment
			.issues
			.push(SimulationIssue::NonFiniteProbability { choice_id });
		return false;
	};
	if (mass - 1.0).abs() > CHOICE_MASS_TOLERANCE {
		assessment
			.issues
			.push(SimulationIssue::InvalidChoiceMass { choice_id, mass });
		return false;
	}
	true
}

fn insert_transition<M, P>(
	model: &M,
	assessment: &mut ProbabilityAssessment<M::Outcome>,
	next: &mut Vec<Branch<M::State, P, M::Event>>,
	indexes: &mut BranchIndexes<M::StateKey, P::PolicyKey, M::Event>,
	mut branch: Branch<M::State, P, M::Event>,
	transition: Transition<M::State, M::Event>,
	probability: f64,
) where
	M: SimulationModel,
	P: Policy<M>,
{
	let mass = branch.mass * probability;
	if !mass.is_finite() {
		assessment.issues.push(SimulationIssue::NonFiniteArithmetic);
		return;
	}
	let (state, events) = transition.into_parts();
	if let Err(error) = branch.queue.schedule_all(branch.now, events) {
		assessment.issues.push(schedule_issue(error));
		return;
	}
	branch.state = state;
	branch.mass = mass;
	let key = BranchKey {
		now: branch.now,
		state: model.state_key(&branch.state),
		policy: branch.policy.policy_key(),
		queue: branch.queue.clone(),
	};
	if let Some(index) = indexes.get(&key).copied() {
		let merged = next[index].mass + branch.mass;
		if !merged.is_finite() {
			assessment.issues.push(SimulationIssue::NonFiniteArithmetic);
			return;
		}
		next[index].mass = merged;
	} else {
		indexes.insert(key, next.len());
		next.push(branch);
	}
}

fn schedule_issue(error: ScheduleError) -> SimulationIssue {
	match error {
		ScheduleError::PastEvent { now, at } => SimulationIssue::PastEvent { now, at },
		ScheduleError::SequenceOverflow => SimulationIssue::EventSequenceOverflow,
	}
}
