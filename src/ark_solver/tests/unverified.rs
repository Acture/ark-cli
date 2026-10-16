use ark_solver::simulation::{
	ChoiceId, EntityId, EventRequest, ModelError, ModelStep, Policy, PolicyDecision,
	ScheduledEvent, SimTick, SimulationIssue, SimulationLimits, SimulationModel, SimulationSeed,
	Simulator, Transition, Verification, Weighted,
};

#[derive(Clone, Copy)]
enum FailureMode {
	Unsupported,
	Deadlock,
	PastEvent,
	Loop,
	BranchLimit,
}

#[derive(Clone, Copy)]
struct FailureModel(FailureMode);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Never {}

impl SimulationModel for FailureModel {
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
		let events = match self.0 {
			FailureMode::Deadlock => Vec::new(),
			_ => vec![EventRequest::new(SimTick::new(1), 0, EntityId::new(0), ())],
		};
		Ok(Transition::new(0, events))
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
		match self.0 {
			FailureMode::Unsupported => Err(ModelError::unsupported("fixture mechanic")),
			FailureMode::PastEvent => Ok(ModelStep::Deterministic(Transition::new(
				0,
				vec![EventRequest::new(SimTick::new(0), 0, EntityId::new(0), ())],
			))),
			FailureMode::Loop => Ok(ModelStep::Deterministic(Transition::new(
				0,
				vec![EventRequest::new(event.at(), 0, EntityId::new(0), ())],
			))),
			FailureMode::BranchLimit => Ok(ModelStep::Choice {
				choice_id: ChoiceId::new(1),
				branches: vec![
					Weighted::new(0.5, Transition::without_events(1)).expect("valid"),
					Weighted::new(0.5, Transition::without_events(2)).expect("valid"),
				],
			}),
			FailureMode::Deadlock => unreachable!("deadlock has no event"),
		}
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		matches!(self.0, FailureMode::BranchLimit)
			.then(|| (*state != 0).then_some(true))
			.flatten()
	}

	fn is_success(&self, outcome: &Self::Outcome) -> bool {
		*outcome
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		*state
	}
}

#[derive(Clone, Copy)]
struct WaitPolicy;

type IssuePredicate = fn(&SimulationIssue) -> bool;
type FailureCase = (FailureMode, IssuePredicate);

impl Policy<FailureModel> for WaitPolicy {
	type PolicyKey = ();

	fn policy_key(&self) -> Self::PolicyKey {}

	fn next_action(&mut self, _observation: &()) -> PolicyDecision<Never> {
		PolicyDecision::Wait
	}
}

#[test]
fn unsupported_mechanics_deadlocks_and_past_events_are_unverified() {
	let cases: [FailureCase; 3] = [
		(FailureMode::Unsupported, |issue: &SimulationIssue| {
			matches!(issue, SimulationIssue::Model(ModelError::Unsupported(_)))
		}),
		(FailureMode::Deadlock, |issue: &SimulationIssue| {
			matches!(issue, SimulationIssue::Deadlock { .. })
		}),
		(FailureMode::PastEvent, |issue: &SimulationIssue| {
			matches!(issue, SimulationIssue::PastEvent { .. })
		}),
	];

	for (mode, expected_issue) in cases {
		let assessment = Simulator::default().evaluate(&FailureModel(mode), &(), WaitPolicy);
		assert_eq!(assessment.verification, Verification::Unverified);
		assert!(assessment.issues.iter().any(expected_issue));
	}
}

#[test]
fn step_and_branch_limits_are_unverified() {
	let step_limited = Simulator::new(SimulationLimits {
		max_steps: 2,
		max_branches: 100,
	})
	.evaluate(&FailureModel(FailureMode::Loop), &(), WaitPolicy);
	assert!(matches!(
		step_limited.issues.as_slice(),
		[SimulationIssue::StepLimit { limit: 2 }]
	));

	let branch_limited = Simulator::new(SimulationLimits {
		max_steps: 100,
		max_branches: 1,
	})
	.evaluate(&FailureModel(FailureMode::BranchLimit), &(), WaitPolicy);
	assert!(matches!(
		branch_limited.issues.as_slice(),
		[SimulationIssue::BranchLimit { limit: 1 }]
	));

	let replay_limited = Simulator::new(SimulationLimits {
		max_steps: 100,
		max_branches: 0,
	})
	.replay(
		&FailureModel(FailureMode::BranchLimit),
		&(),
		WaitPolicy,
		SimulationSeed::new(0),
	);
	assert!(matches!(
		replay_limited.issues.as_slice(),
		[SimulationIssue::BranchLimit { limit: 0 }]
	));
}
