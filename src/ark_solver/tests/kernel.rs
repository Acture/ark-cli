use ark_solver::simulation::{
	ChoiceId, ChoiceSelection, ChoiceTape, EntityId, EventRequest, ModelError, ModelStep, Policy,
	PolicyDecision, ScheduledEvent, SimTick, SimulationIssue, SimulationModel, SimulationSeed,
	Simulator, TraceEntry, Transition, Verification, Weighted,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QueueState {
	seen: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Never {}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QueueOutcome(Vec<u8>);

#[derive(Clone)]
struct QueueModel;

impl SimulationModel for QueueModel {
	type Scenario = ();
	type State = QueueState;
	type StateKey = Vec<u8>;
	type Command = Never;
	type Observation = ();
	type Event = u8;
	type Outcome = QueueOutcome;

	fn initialize(
		&self,
		_scenario: &Self::Scenario,
	) -> Result<Transition<Self::State, Self::Event>, ModelError> {
		Ok(Transition::new(
			QueueState { seen: Vec::new() },
			vec![
				EventRequest::new(SimTick::new(1), 1, EntityId::new(2), 20),
				EventRequest::new(SimTick::new(1), 0, EntityId::new(9), 10),
				EventRequest::new(SimTick::new(1), 1, EntityId::new(1), 30),
				EventRequest::new(SimTick::new(1), 1, EntityId::new(1), 31),
			],
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
		state: &Self::State,
		event: &ScheduledEvent<Self::Event>,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		let mut next = state.clone();
		next.seen.push(*event.event());
		Ok(ModelStep::Deterministic(Transition::without_events(next)))
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		(state.seen.len() == 4).then(|| QueueOutcome(state.seen.clone()))
	}

	fn is_success(&self, _outcome: &Self::Outcome) -> bool {
		true
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		state.seen.clone()
	}
}

#[derive(Clone)]
struct WaitPolicy;

impl Policy<QueueModel> for WaitPolicy {
	type PolicyKey = ();

	fn policy_key(&self) -> Self::PolicyKey {}

	fn next_action(&mut self, _observation: &()) -> PolicyDecision<Never> {
		PolicyDecision::Wait
	}
}

#[test]
fn scheduler_uses_canonical_same_tick_order_and_produces_stable_traces() {
	let simulator = Simulator::default();
	let first = simulator.replay(&QueueModel, &(), WaitPolicy, SimulationSeed::new(7));
	let second = simulator.replay(&QueueModel, &(), WaitPolicy, SimulationSeed::new(7));

	assert_eq!(first.verification, Verification::Verified);
	assert_eq!(first.outcome, Some(QueueOutcome(vec![10, 30, 31, 20])));
	assert_eq!(first.trace, second.trace);
	let events: Vec<u8> = first
		.trace
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled } => Some(*scheduled.event()),
			_ => None,
		})
		.collect();
	assert_eq!(events, vec![10, 30, 31, 20]);
}

#[derive(Clone)]
struct ChoiceModel;

impl SimulationModel for ChoiceModel {
	type Scenario = ();
	type State = u8;
	type StateKey = u8;
	type Command = Never;
	type Observation = ();
	type Event = ();
	type Outcome = bool;

	fn initialize(
		&self,
		_scenario: &Self::Scenario,
	) -> Result<Transition<Self::State, Self::Event>, ModelError> {
		Ok(Transition::new(
			0,
			vec![EventRequest::new(SimTick::new(0), 0, EntityId::new(0), ())],
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
		_event: &ScheduledEvent<Self::Event>,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError> {
		Ok(ModelStep::Choice {
			choice_id: ChoiceId::new(42),
			branches: vec![
				Weighted::new(0.25, Transition::without_events(1)).expect("valid weight"),
				Weighted::new(0.75, Transition::without_events(2)).expect("valid weight"),
			],
		})
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		match state {
			1 => Some(false),
			2 => Some(true),
			_ => None,
		}
	}

	fn is_success(&self, outcome: &Self::Outcome) -> bool {
		*outcome
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		*state
	}
}

#[derive(Clone)]
struct ChoicePolicy;

impl Policy<ChoiceModel> for ChoicePolicy {
	type PolicyKey = ();

	fn policy_key(&self) -> Self::PolicyKey {}

	fn next_action(&mut self, _observation: &()) -> PolicyDecision<Never> {
		PolicyDecision::Wait
	}
}

#[test]
fn replay_tape_reproduces_the_seeded_choice_exactly() {
	let simulator = Simulator::default();
	let seeded = simulator.replay(&ChoiceModel, &(), ChoicePolicy, SimulationSeed::new(0));

	assert_eq!(seeded.verification, Verification::Verified);
	assert_eq!(
		seeded.choice_tape.entries(),
		&[ChoiceSelection {
			choice_id: ChoiceId::new(42),
			branch_index: 1,
		}]
	);
	let reproduced =
		simulator.replay_with_tape(&ChoiceModel, &(), ChoicePolicy, &seeded.choice_tape);
	assert_eq!(reproduced.verification, Verification::Verified);
	assert_eq!(reproduced.outcome, seeded.outcome);
	assert_eq!(reproduced.trace, seeded.trace);
	assert_eq!(reproduced.choice_tape, seeded.choice_tape);
}

#[test]
fn replay_rejects_a_tape_for_a_different_declared_choice() {
	let tape = ChoiceTape::new(vec![ChoiceSelection {
		choice_id: ChoiceId::new(99),
		branch_index: 0,
	}]);
	let result = Simulator::default().replay_with_tape(&ChoiceModel, &(), ChoicePolicy, &tape);

	assert_eq!(result.verification, Verification::Unverified);
	assert!(matches!(
		result.issues.as_slice(),
		[SimulationIssue::TapeChoiceMismatch {
			expected,
			actual
		}] if *expected == ChoiceId::new(42) && *actual == ChoiceId::new(99)
	));
}
