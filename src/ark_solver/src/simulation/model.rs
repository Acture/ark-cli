use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::simulation::{ModelStep, ScheduledEvent, SimTick, Transition};

pub trait SimulationModel {
	type Scenario;
	type State: Clone;
	type StateKey: Clone + Ord;
	type Command: Clone;
	type Observation;
	type Event: Clone + Ord;
	type Outcome: Clone + Ord;

	fn initialize(
		&self,
		scenario: &Self::Scenario,
	) -> Result<Transition<Self::State, Self::Event>, ModelError>;

	fn observe(&self, state: &Self::State, at: SimTick) -> Self::Observation;

	fn apply_command(
		&self,
		state: &Self::State,
		command: &Self::Command,
		at: SimTick,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError>;

	fn apply_event(
		&self,
		state: &Self::State,
		event: &ScheduledEvent<Self::Event>,
	) -> Result<ModelStep<Self::State, Self::Event>, ModelError>;

	fn terminal(&self, state: &Self::State) -> Option<Self::Outcome>;

	fn is_success(&self, outcome: &Self::Outcome) -> bool;

	fn state_key(&self, state: &Self::State) -> Self::StateKey;
}

pub trait Policy<M: SimulationModel>: Clone {
	type PolicyKey: Clone + Ord;

	fn policy_key(&self) -> Self::PolicyKey;

	fn next_action(&mut self, observation: &M::Observation) -> PolicyDecision<M::Command>;
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PolicyDecision<C> {
	Command(C),
	Wait,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelError {
	Unsupported(String),
	InvalidCommand(String),
	InvalidTransition(String),
}

impl ModelError {
	#[must_use]
	pub fn unsupported(detail: impl Into<String>) -> Self {
		Self::Unsupported(detail.into())
	}

	#[must_use]
	pub fn invalid_command(detail: impl Into<String>) -> Self {
		Self::InvalidCommand(detail.into())
	}

	#[must_use]
	pub fn invalid_transition(detail: impl Into<String>) -> Self {
		Self::InvalidTransition(detail.into())
	}
}

impl Display for ModelError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Unsupported(detail) => write!(formatter, "unsupported mechanic: {detail}"),
			Self::InvalidCommand(detail) => write!(formatter, "invalid command: {detail}"),
			Self::InvalidTransition(detail) => write!(formatter, "invalid transition: {detail}"),
		}
	}
}

impl Error for ModelError {}
