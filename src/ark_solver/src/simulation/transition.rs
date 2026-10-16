use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::simulation::{ChoiceId, EventRequest};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Transition<S, E> {
	state: S,
	events: Vec<EventRequest<E>>,
}

impl<S, E> Transition<S, E> {
	#[must_use]
	pub const fn new(state: S, events: Vec<EventRequest<E>>) -> Self {
		Self { state, events }
	}

	#[must_use]
	pub fn without_events(state: S) -> Self {
		Self::new(state, Vec::new())
	}

	pub(in crate::simulation) fn into_parts(self) -> (S, Vec<EventRequest<E>>) {
		(self.state, self.events)
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelStep<S, E> {
	Deterministic(Transition<S, E>),
	Choice {
		choice_id: ChoiceId,
		branches: Vec<Weighted<Transition<S, E>>>,
	},
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weighted<T> {
	probability: f64,
	value: T,
}

impl<T> Weighted<T> {
	pub fn new(probability: f64, value: T) -> Result<Self, ProbabilityError> {
		if !probability.is_finite() {
			return Err(ProbabilityError::NotFinite(probability));
		}
		if !(0.0..=1.0).contains(&probability) {
			return Err(ProbabilityError::OutOfRange(probability));
		}
		Ok(Self { probability, value })
	}

	#[must_use]
	pub const fn probability(&self) -> f64 {
		self.probability
	}

	#[must_use]
	pub const fn value(&self) -> &T {
		&self.value
	}

	#[must_use]
	pub fn into_value(self) -> T {
		self.value
	}

	#[must_use]
	pub fn into_parts(self) -> (f64, T) {
		(self.probability, self.value)
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProbabilityError {
	NotFinite(f64),
	OutOfRange(f64),
}

impl Display for ProbabilityError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NotFinite(value) => write!(formatter, "probability {value} is not finite"),
			Self::OutOfRange(value) => {
				write!(formatter, "probability {value} is outside [0, 1]")
			}
		}
	}
}

impl Error for ProbabilityError {}
