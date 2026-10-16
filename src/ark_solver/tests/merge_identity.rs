use ark_solver::simulation::{
	ChoiceId, EntityId, EventRequest, ModelError, ModelStep, Policy, PolicyDecision,
	ScheduledEvent, SimTick, SimulationIssue, SimulationLimits, SimulationModel, Simulator,
	Transition, Verification, Weighted,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Never {}

#[derive(Clone, Copy)]
struct QueueIdentityModel;

impl SimulationModel for QueueIdentityModel {
	type Scenario = ();
	type State = u8;
	type StateKey = u8;
	type Command = Never;
	type Observation = ();
	type Event = u8;
	type Outcome = bool;

	fn initialize(
		&self,
		_scenario: &Self::Scenario,
	) -> Result<Transition<Self::State, Self::Event>, ModelError> {
		Ok(Transition::new(
			0,
			vec![EventRequest::new(SimTick::new(0), 0, EntityId::new(0), 0)],
		))
	}

	fn observe(&self, _state: &Self::State, _at: SimTick) -> Self::Observation {}

	fn apply_command(
		&self,
		_state: &Self::State,
		command: &Self::Command,
		_at: SimTick,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		match *command {}
	}

	fn apply_event(
		&self,
		_state: &Self::State,
		event: &ScheduledEvent<Self::Event>,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		if *event.event() == 0 {
			let transition = |event| {
				Transition::new(
					1,
					vec![EventRequest::new(
						SimTick::new(1),
						0,
						EntityId::new(0),
						event,
					)],
				)
			};
			Ok(ModelStep::Choice {
				choice_id: ChoiceId::new(1),
				branches: vec![
					Weighted::new(0.5, transition(1)).expect("valid"),
					Weighted::new(0.5, transition(2)).expect("valid"),
				],
			})
		} else {
			Ok(ModelStep::Deterministic(Transition::without_events(
				*event.event() + 1,
			)))
		}
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		(*state > 1).then_some(true)
	}

	fn is_success(&self, outcome: &Self::Outcome) -> bool {
		*outcome
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		*state
	}
}

#[derive(Clone, Copy)]
struct QueuePolicy;

impl Policy<QueueIdentityModel> for QueuePolicy {
	type PolicyKey = ();

	fn policy_key(&self) -> Self::PolicyKey {}

	fn next_action(&mut self, _observation: &()) -> PolicyDecision<Never> {
		PolicyDecision::Wait
	}
}

#[test]
fn queued_events_are_part_of_branch_merge_identity() {
	let assessment = Simulator::new(SimulationLimits {
		max_steps: 100,
		max_branches: 1,
	})
	.evaluate(&QueueIdentityModel, &(), QueuePolicy);

	assert_eq!(assessment.verification, Verification::Unverified);
	assert!(matches!(
		assessment.issues.as_slice(),
		[SimulationIssue::BranchLimit { limit: 1 }]
	));
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Command {
	Converge,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Event {
	Choose,
	Finish,
}

#[derive(Clone, Copy)]
struct PolicyIdentityModel;

impl SimulationModel for PolicyIdentityModel {
	type Scenario = ();
	type State = u8;
	type StateKey = u8;
	type Command = Command;
	type Observation = u8;
	type Event = Event;
	type Outcome = bool;

	fn initialize(
		&self,
		_scenario: &Self::Scenario,
	) -> Result<Transition<Self::State, Self::Event>, ModelError> {
		Ok(Transition::new(
			0,
			vec![EventRequest::new(
				SimTick::new(0),
				0,
				EntityId::new(0),
				Event::Choose,
			)],
		))
	}

	fn observe(&self, state: &Self::State, _at: SimTick) -> Self::Observation {
		*state
	}

	fn apply_command(
		&self,
		_state: &Self::State,
		_command: &Self::Command,
		_at: SimTick,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		Ok(ModelStep::Deterministic(Transition::new(
			3,
			vec![EventRequest::new(
				SimTick::new(1),
				0,
				EntityId::new(0),
				Event::Finish,
			)],
		)))
	}

	fn apply_event(
		&self,
		_state: &Self::State,
		event: &ScheduledEvent<Self::Event>,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		match event.event() {
			Event::Choose => Ok(ModelStep::Choice {
				choice_id: ChoiceId::new(2),
				branches: vec![
					Weighted::new(0.5, Transition::without_events(1)).expect("valid"),
					Weighted::new(0.5, Transition::without_events(2)).expect("valid"),
				],
			}),
			Event::Finish => Ok(ModelStep::Deterministic(Transition::without_events(4))),
		}
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		(*state == 4).then_some(true)
	}

	fn is_success(&self, outcome: &Self::Outcome) -> bool {
		*outcome
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		*state
	}
}

#[derive(Clone, Copy)]
struct StatefulPolicy {
	marker: u8,
}

impl Policy<PolicyIdentityModel> for StatefulPolicy {
	type PolicyKey = u8;

	fn policy_key(&self) -> Self::PolicyKey {
		self.marker
	}

	fn next_action(&mut self, observation: &u8) -> PolicyDecision<Command> {
		match observation {
			0 | 3 => PolicyDecision::Wait,
			value => {
				self.marker = *value;
				PolicyDecision::Command(Command::Converge)
			}
		}
	}
}

#[test]
fn mutable_policy_key_is_part_of_branch_merge_identity() {
	let assessment =
		Simulator::default().evaluate(&PolicyIdentityModel, &(), StatefulPolicy { marker: 0 });

	assert_eq!(assessment.verification, Verification::Verified);
	assert_eq!(assessment.processed_steps, 5);
	assert_eq!(assessment.covered_probability_mass, 1.0);
}
