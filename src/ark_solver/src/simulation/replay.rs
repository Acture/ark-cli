use crate::simulation::rng::{ChoiceSource, StableRng};
use crate::simulation::scheduler::{EventQueue, ScheduleError};
use crate::simulation::{
	ChoiceSelection, ChoiceTape, ModelStep, Policy, PolicyDecision, ReplayResult, SimTick,
	SimulationIssue, SimulationModel, SimulationSeed, Simulator, TraceEntry, Transition,
	Verification,
};

impl Simulator {
	pub fn replay<M, P>(
		&self,
		model: &M,
		scenario: &M::Scenario,
		policy: P,
		seed: SimulationSeed,
	) -> ReplayResult<M::Outcome, M::Command, M::Event>
	where
		M: SimulationModel,
		P: Policy<M>,
	{
		let initial = match model.initialize(scenario) {
			Ok(transition) => transition,
			Err(error) => return ReplayResult::unverified(SimulationIssue::Model(error)),
		};
		self.replay_inner(
			model,
			policy,
			initial,
			ChoiceSource::Seed(StableRng::new(seed)),
		)
	}

	pub fn replay_with_tape<M, P>(
		&self,
		model: &M,
		scenario: &M::Scenario,
		policy: P,
		tape: &ChoiceTape,
	) -> ReplayResult<M::Outcome, M::Command, M::Event>
	where
		M: SimulationModel,
		P: Policy<M>,
	{
		let initial = match model.initialize(scenario) {
			Ok(transition) => transition,
			Err(error) => return ReplayResult::unverified(SimulationIssue::Model(error)),
		};
		self.replay_inner(
			model,
			policy,
			initial,
			ChoiceSource::Tape {
				entries: tape.entries(),
				cursor: 0,
			},
		)
	}

	fn replay_inner<M, P>(
		&self,
		model: &M,
		mut policy: P,
		initial: Transition<M::State, M::Event>,
		mut choices: ChoiceSource<'_>,
	) -> ReplayResult<M::Outcome, M::Command, M::Event>
	where
		M: SimulationModel,
		P: Policy<M>,
	{
		let mut result = ReplayResult::running();
		let mut queue = EventQueue::new();
		let (mut state, initial_events) = initial.into_parts();
		if let Err(error) = queue.schedule_all(SimTick::new(0), initial_events) {
			return ReplayResult::unverified(schedule_issue(error));
		}
		if self.limits.max_branches == 0 {
			return ReplayResult::unverified(SimulationIssue::BranchLimit {
				limit: self.limits.max_branches,
			});
		}
		let mut now = SimTick::new(0);

		loop {
			if let Some(outcome) = model.terminal(&state) {
				if let Some(issue) = choices.finished_issue() {
					result.issues.push(issue);
					return result;
				}
				result.verification = Verification::Verified;
				result.outcome = Some(outcome);
				return result;
			}
			if result.steps >= self.limits.max_steps {
				result.issues.push(SimulationIssue::StepLimit {
					limit: self.limits.max_steps,
				});
				return result;
			}
			result.steps += 1;

			let observation = model.observe(&state, now);
			let step = match policy.next_action(&observation) {
				PolicyDecision::Command(command) => {
					result.trace.push(TraceEntry::Command {
						at: now,
						command: command.clone(),
					});
					model.apply_command(&state, &command, now)
				}
				PolicyDecision::Wait => {
					let Some(event) = queue.pop() else {
						result.issues.push(SimulationIssue::Deadlock { at: now });
						return result;
					};
					now = event.at();
					result.trace.push(TraceEntry::Event {
						scheduled: event.clone(),
					});
					model.apply_event(&state, &event)
				}
			};

			match step {
				Ok(ModelStep::Deterministic(transition)) => {
					let (next_state, events) = transition.into_parts();
					if let Err(error) = queue.schedule_all(now, events) {
						result.issues.push(schedule_issue(error));
						return result;
					}
					state = next_state;
				}
				Ok(ModelStep::Choice {
					choice_id,
					branches,
				}) => {
					let branch_index = match choices.select(choice_id, &branches) {
						Ok(index) => index,
						Err(issue) => {
							result.issues.push(issue);
							return result;
						}
					};
					let selection = ChoiceSelection {
						choice_id,
						branch_index,
					};
					result.choice_tape.entries.push(selection);
					result.trace.push(TraceEntry::Choice {
						at: now,
						choice_id,
						branch_index,
					});
					let transition = branches
						.into_iter()
						.nth(branch_index)
						.expect("choice index was validated")
						.into_value();
					let (next_state, events) = transition.into_parts();
					if let Err(error) = queue.schedule_all(now, events) {
						result.issues.push(schedule_issue(error));
						return result;
					}
					state = next_state;
				}
				Err(error) => {
					result.issues.push(SimulationIssue::Model(error));
					return result;
				}
			}
		}
	}
}

fn schedule_issue(error: ScheduleError) -> SimulationIssue {
	match error {
		ScheduleError::PastEvent { now, at } => SimulationIssue::PastEvent { now, at },
		ScheduleError::SequenceOverflow => SimulationIssue::EventSequenceOverflow,
	}
}
