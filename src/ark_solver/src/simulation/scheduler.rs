use crate::simulation::{EntityId, SimTick};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EventRequest<E> {
	at: SimTick,
	priority: i32,
	entity: EntityId,
	event: E,
}

impl<E> EventRequest<E> {
	#[must_use]
	pub const fn new(at: SimTick, priority: i32, entity: EntityId, event: E) -> Self {
		Self {
			at,
			priority,
			entity,
			event,
		}
	}

	pub(in crate::simulation) fn into_parts(self) -> (SimTick, i32, EntityId, E) {
		(self.at, self.priority, self.entity, self.event)
	}
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ScheduledEvent<E> {
	at: SimTick,
	priority: i32,
	entity: EntityId,
	sequence: u64,
	event: E,
}

impl<E> ScheduledEvent<E> {
	#[must_use]
	pub const fn at(&self) -> SimTick {
		self.at
	}

	#[must_use]
	pub const fn priority(&self) -> i32 {
		self.priority
	}

	#[must_use]
	pub const fn entity(&self) -> EntityId {
		self.entity
	}

	#[must_use]
	pub const fn sequence(&self) -> u64 {
		self.sequence
	}

	#[must_use]
	pub const fn event(&self) -> &E {
		&self.event
	}
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::simulation) struct EventQueue<E> {
	events: Vec<ScheduledEvent<E>>,
	next_sequence: u64,
}

impl<E: Ord> EventQueue<E> {
	pub(in crate::simulation) const fn new() -> Self {
		Self {
			events: Vec::new(),
			next_sequence: 0,
		}
	}

	pub(in crate::simulation) fn schedule_all(
		&mut self,
		now: SimTick,
		requests: Vec<EventRequest<E>>,
	) -> Result<(), ScheduleError> {
		for request in requests {
			let (at, priority, entity, event) = request.into_parts();
			if at < now {
				return Err(ScheduleError::PastEvent { now, at });
			}
			let sequence = self.next_sequence;
			self.next_sequence = self
				.next_sequence
				.checked_add(1)
				.ok_or(ScheduleError::SequenceOverflow)?;
			self.events.push(ScheduledEvent {
				at,
				priority,
				entity,
				sequence,
				event,
			});
		}
		self.events.sort_unstable();
		Ok(())
	}

	pub(in crate::simulation) fn pop(&mut self) -> Option<ScheduledEvent<E>> {
		(!self.events.is_empty()).then(|| self.events.remove(0))
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::simulation) enum ScheduleError {
	PastEvent { now: SimTick, at: SimTick },
	SequenceOverflow,
}
