use crate::simulation::{ChoiceId, ScheduledEvent, SimTick};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trace<C, E> {
	entries: Vec<TraceEntry<C, E>>,
}

impl<C, E> Trace<C, E> {
	pub(in crate::simulation) const fn new() -> Self {
		Self {
			entries: Vec::new(),
		}
	}

	#[must_use]
	pub fn entries(&self) -> &[TraceEntry<C, E>] {
		&self.entries
	}

	pub(in crate::simulation) fn push(&mut self, entry: TraceEntry<C, E>) {
		self.entries.push(entry);
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraceEntry<C, E> {
	Command {
		at: SimTick,
		command: C,
	},
	Event {
		scheduled: ScheduledEvent<E>,
	},
	Choice {
		at: SimTick,
		choice_id: ChoiceId,
		branch_index: usize,
	},
}
