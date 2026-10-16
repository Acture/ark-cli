use ark_solver::simulation::{
	ChoiceId, EntityId, EventRequest, ModelError, ModelStep, Policy, PolicyDecision,
	ScheduledEvent, SimTick, SimulationIssue, SimulationLimits, SimulationModel, Simulator,
	Transition, Verification, Weighted,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Never {}

#[derive(Clone)]
struct MassModel {
	branches: Vec<(f64, u8)>,
}

impl SimulationModel for MassModel {
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
			choice_id: ChoiceId::new(9),
			branches: self
				.branches
				.iter()
				.map(|(probability, state)| {
					Weighted::new(*probability, Transition::without_events(*state))
						.expect("fixture probabilities are individually valid")
				})
				.collect(),
		})
	}

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome> {
		(*state != 0).then_some(*state == 1)
	}

	fn is_success(&self, outcome: &Self::Outcome) -> bool {
		*outcome
	}

	fn state_key(&self, state: &Self::State) -> Self::StateKey {
		*state
	}
}

#[derive(Clone)]
struct MassPolicy;

impl Policy<MassModel> for MassPolicy {
	type PolicyKey = ();

	fn policy_key(&self) -> Self::PolicyKey {}

	fn next_action(&mut self, _observation: &()) -> PolicyDecision<Never> {
		PolicyDecision::Wait
	}
}

#[test]
fn accounts_for_all_terminal_probability_mass() {
	let assessment = Simulator::default().evaluate(
		&MassModel {
			branches: vec![(0.25, 2), (0.75, 1)],
		},
		&(),
		MassPolicy,
	);

	assert_eq!(assessment.verification, Verification::Verified);
	assert_eq!(assessment.success_probability, 0.75);
	assert_eq!(assessment.covered_probability_mass, 1.0);
	assert_eq!(assessment.outcome_weights.get(&false), Some(&0.25));
	assert_eq!(assessment.outcome_weights.get(&true), Some(&0.75));
}

#[test]
fn merges_identical_complete_configurations_before_branch_limit() {
	let simulator = Simulator::new(SimulationLimits {
		max_steps: 100,
		max_branches: 1,
	});
	let assessment = simulator.evaluate(
		&MassModel {
			branches: vec![(0.4, 1), (0.6, 1)],
		},
		&(),
		MassPolicy,
	);

	assert_eq!(assessment.verification, Verification::Verified);
	assert_eq!(assessment.success_probability, 1.0);
	assert_eq!(assessment.covered_probability_mass, 1.0);
	assert_eq!(assessment.peak_branches, 1);
}

#[test]
fn rejects_incomplete_choice_mass_without_renormalizing() {
	let assessment = Simulator::default().evaluate(
		&MassModel {
			branches: vec![(0.3, 1), (0.3, 2)],
		},
		&(),
		MassPolicy,
	);

	assert_eq!(assessment.verification, Verification::Unverified);
	assert_eq!(assessment.success_probability, 0.0);
	assert_eq!(assessment.covered_probability_mass, 0.0);
	assert!(matches!(
		assessment.issues.as_slice(),
		[SimulationIssue::InvalidChoiceMass {
			choice_id,
			mass
		}] if *choice_id == ChoiceId::new(9) && (*mass - 0.6).abs() < 1e-12
	));
}
