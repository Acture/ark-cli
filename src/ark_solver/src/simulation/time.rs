use std::error::Error;
use std::fmt::{Display, Formatter};
use std::num::NonZeroU32;

const MILLIS_PER_SECOND: u128 = 1_000;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SimTick(u64);

impl SimTick {
	#[must_use]
	pub const fn new(value: u64) -> Self {
		Self(value)
	}

	#[must_use]
	pub const fn get(self) -> u64 {
		self.0
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TickRate(NonZeroU32);

impl TickRate {
	#[must_use]
	pub const fn new(ticks_per_second: NonZeroU32) -> Self {
		Self(ticks_per_second)
	}

	#[must_use]
	pub const fn ticks_per_second(self) -> NonZeroU32 {
		self.0
	}

	pub fn ticks_from_millis(self, millis: u64) -> Result<SimTick, TimeConversionError> {
		let numerator = u128::from(millis) * u128::from(self.0.get());
		let ticks = numerator / MILLIS_PER_SECOND;
		if ticks > u128::from(u64::MAX) {
			return Err(TimeConversionError::Overflow);
		}
		if !numerator.is_multiple_of(MILLIS_PER_SECOND) {
			return Err(TimeConversionError::NonIntegral);
		}
		Ok(SimTick::new(ticks as u64))
	}

	pub fn millis_from_ticks(self, ticks: SimTick) -> Result<u64, TimeConversionError> {
		let numerator = u128::from(ticks.get()) * MILLIS_PER_SECOND;
		let divisor = u128::from(self.0.get());
		let millis = numerator / divisor;
		if millis > u128::from(u64::MAX) {
			return Err(TimeConversionError::Overflow);
		}
		if !numerator.is_multiple_of(divisor) {
			return Err(TimeConversionError::NonIntegral);
		}
		Ok(millis as u64)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeConversionError {
	NonIntegral,
	Overflow,
}

impl Display for TimeConversionError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NonIntegral => formatter.write_str("time does not map to a whole tick"),
			Self::Overflow => formatter.write_str("time conversion overflowed"),
		}
	}
}

impl Error for TimeConversionError {}
