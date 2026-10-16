use std::num::NonZeroU32;

use ark_solver::simulation::{SimTick, TickRate, TimeConversionError};

#[test]
fn integer_time_conversion_is_exact_and_checked() {
	let rate = TickRate::new(NonZeroU32::new(30).expect("30 is non-zero"));

	assert_eq!(rate.ticks_from_millis(500), Ok(SimTick::new(15)));
	assert_eq!(rate.millis_from_ticks(SimTick::new(15)), Ok(500));
	assert_eq!(
		rate.ticks_from_millis(1),
		Err(TimeConversionError::NonIntegral)
	);
	assert_eq!(
		TickRate::new(NonZeroU32::MAX).ticks_from_millis(u64::MAX),
		Err(TimeConversionError::Overflow)
	);
}
