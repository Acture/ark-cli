use crate::simulation::{ChoiceId, ModelError, SimTick, Trace};

pub(in crate::simulation) const CHOICE_MASS_TOLERANCE: f64 = 1e-12;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SimulationSeed(pub(in crate::simulation) u64);

impl SimulationSeed {
	#[must_use]
	pub const fn new(value: u64) -> Self {
		Self(value)
	}
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChoiceTape {
	pub(in crate::simulation) entries: Vec<ChoiceSelection>,
}

impl ChoiceTape {
	#[must_use]
	pub const fn new(entries: Vec<ChoiceSelection>) -> Self {
		Self { entries }
	}

	#[must_use]
	pub fn entries(&self) -> &[ChoiceSelection] {
		&self.entries
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceSelection {
	pub choice_id: ChoiceId,
	pub branch_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verification {
	Verified,
	Unverified,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SimulationIssue {
	Model(ModelError),
	Deadlock {
		at: SimTick,
	},
	StepLimit {
		limit: usize,
	},
	PastEvent {
		now: SimTick,
		at: SimTick,
	},
	EventSequenceOverflow,
	BranchLimit {
		limit: usize,
	},
	IncompleteProbabilityMass {
		mass: f64,
	},
	NonFiniteArithmetic,
	InvalidChoiceMass {
		choice_id: ChoiceId,
		mass: f64,
	},
	NonFiniteProbability {
		choice_id: ChoiceId,
	},
	MissingChoiceMass {
		choice_id: ChoiceId,
	},
	MissingTapeEntry {
		choice_id: ChoiceId,
	},
	TapeChoiceMismatch {
		expected: ChoiceId,
		actual: ChoiceId,
	},
	TapeBranchOutOfRange {
		choice_id: ChoiceId,
		branch_index: usize,
	},
	ImpossibleTapeBranch {
		choice_id: ChoiceId,
		branch_index: usize,
	},
	UnusedTapeEntries {
		count: usize,
	},
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationLimits {
	pub max_steps: usize,
	pub max_branches: usize,
}

impl Default for SimulationLimits {
	fn default() -> Self {
		Self {
			max_steps: 1_000_000,
			max_branches: 100_000,
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayResult<O, C, E> {
	pub verification: Verification,
	pub outcome: Option<O>,
	pub trace: Trace<C, E>,
	pub choice_tape: ChoiceTape,
	pub issues: Vec<SimulationIssue>,
	pub steps: usize,
}

impl<O, C, E> ReplayResult<O, C, E> {
	pub(in crate::simulation) fn running() -> Self {
		Self {
			verification: Verification::Unverified,
			outcome: None,
			trace: Trace::new(),
			choice_tape: ChoiceTape::default(),
			issues: Vec::new(),
			steps: 0,
		}
	}

	pub(in crate::simulation) fn unverified(issue: SimulationIssue) -> Self {
		let mut result = Self::running();
		result.issues.push(issue);
		result
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Simulator {
	pub(in crate::simulation) limits: SimulationLimits,
}

impl Simulator {
	#[must_use]
	pub const fn new(limits: SimulationLimits) -> Self {
		Self { limits }
	}
}
