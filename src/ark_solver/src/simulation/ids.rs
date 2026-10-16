#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityId(u64);

impl EntityId {
	#[must_use]
	pub const fn new(value: u64) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u64 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChoiceId(u64);

impl ChoiceId {
	#[must_use]
	pub const fn new(value: u64) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u64 {
		self.0
	}
}
