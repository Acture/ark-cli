//! Structural parsing and explicit semantic binding for natural SP-recovery effects.

use crate::data::OperatorEffectCandidate;
use crate::simulation::TickRate;

use crate::mechanics::{
	combat::{ExpectedValue, PeriodicSpSource},
	normal_attack::FixedDamage,
};

pub const PASSIVE_SP_RECOVERY_KEY: &str = "sp_recovery_per_sec";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassiveSpBlackboardFact<'a> {
	pub key: &'a str,
	pub value: f64,
	pub value_string: Option<&'a str>,
}

impl<'a> PassiveSpBlackboardFact<'a> {
	#[must_use]
	pub const fn new(key: &'a str, value: f64, value_string: Option<&'a str>) -> Self {
		Self {
			key,
			value,
			value_string,
		}
	}
}

/// Parsed numeric payload without any claim about its target, condition, or stacking rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParsedNaturalSpRecovery {
	pub amount: ExpectedValue,
}

/// Structured semantic scope supplied by a reviewed mechanic binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NaturalSpRecoveryScope {
	SelfActor,
	OtherOrArea,
	Unknown,
}

/// Structured activation condition supplied by a reviewed mechanic binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NaturalSpRecoveryCondition {
	Unconditional,
	Conditional,
	Unknown,
}

/// Structured stacking behavior supplied by a reviewed mechanic binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NaturalSpRecoveryStacking {
	Independent,
	ExclusiveOrUnknown,
}

/// Parses exactly one numeric, finite, non-negative natural-SP blackboard entry.
///
/// Parsing does not prove that the effect applies to the owner or is always active. Callers must
/// retain the candidate as unmodeled until a separate semantic binding proves both properties.
#[must_use]
pub fn parse_natural_sp_recovery_facts(
	blackboard: &[PassiveSpBlackboardFact<'_>],
) -> Option<ParsedNaturalSpRecovery> {
	let [entry] = blackboard else {
		return None;
	};
	if entry.key != PASSIVE_SP_RECOVERY_KEY || entry.value_string.is_some() {
		return None;
	}
	let amount: FixedDamage = FixedDamage::try_from_f64(entry.value).ok()?;
	Some(ParsedNaturalSpRecovery {
		amount: ExpectedValue::from_scaled(amount.scaled()),
	})
}

/// Extracts the structural payload from an operator effect candidate without binding semantics.
#[must_use]
pub fn parse_natural_sp_recovery_candidate(
	candidate: &OperatorEffectCandidate,
) -> Option<ParsedNaturalSpRecovery> {
	let blackboard: Vec<PassiveSpBlackboardFact<'_>> = candidate
		.blackboard()
		.iter()
		.map(|entry| PassiveSpBlackboardFact::new(entry.key(), entry.value(), entry.value_string()))
		.collect();
	parse_natural_sp_recovery_facts(&blackboard)
}

/// Binds a parsed payload only after structured semantics prove self-only, unconditional recovery.
#[must_use]
pub fn bind_self_unconditional_natural_sp_recovery(
	parsed: ParsedNaturalSpRecovery,
	scope: NaturalSpRecoveryScope,
	condition: NaturalSpRecoveryCondition,
	stacking: NaturalSpRecoveryStacking,
	tick_rate: TickRate,
) -> Option<PeriodicSpSource> {
	if scope != NaturalSpRecoveryScope::SelfActor
		|| condition != NaturalSpRecoveryCondition::Unconditional
		|| stacking != NaturalSpRecoveryStacking::Independent
	{
		return None;
	}
	Some(PeriodicSpSource {
		amount: parsed.amount,
		interval_ticks: u64::from(tick_rate.ticks_per_second().get()),
	})
}

#[cfg(test)]
mod tests {
	use std::num::NonZeroU32;

	use crate::simulation::TickRate;

	use super::{
		bind_self_unconditional_natural_sp_recovery, parse_natural_sp_recovery_facts,
		NaturalSpRecoveryCondition, NaturalSpRecoveryScope, NaturalSpRecoveryStacking,
		PassiveSpBlackboardFact, PASSIVE_SP_RECOVERY_KEY,
	};
	use crate::mechanics::combat::ExpectedValue;

	fn tick_rate() -> TickRate {
		TickRate::new(NonZeroU32::new(30).expect("test tick rate is non-zero"))
	}

	#[test]
	fn parsing_does_not_imply_semantic_binding() {
		let parsed = parse_natural_sp_recovery_facts(&[PassiveSpBlackboardFact::new(
			PASSIVE_SP_RECOVERY_KEY,
			0.3,
			None,
		)])
		.expect("exact natural SP fact is structurally valid");

		assert_eq!(parsed.amount, ExpectedValue::from_scaled(300_000));
		assert!(bind_self_unconditional_natural_sp_recovery(
			parsed,
			NaturalSpRecoveryScope::Unknown,
			NaturalSpRecoveryCondition::Unconditional,
			NaturalSpRecoveryStacking::Independent,
			tick_rate(),
		)
		.is_none());
		assert!(bind_self_unconditional_natural_sp_recovery(
			parsed,
			NaturalSpRecoveryScope::SelfActor,
			NaturalSpRecoveryCondition::Conditional,
			NaturalSpRecoveryStacking::Independent,
			tick_rate(),
		)
		.is_none());
		assert!(bind_self_unconditional_natural_sp_recovery(
			parsed,
			NaturalSpRecoveryScope::SelfActor,
			NaturalSpRecoveryCondition::Unconditional,
			NaturalSpRecoveryStacking::ExclusiveOrUnknown,
			tick_rate(),
		)
		.is_none());

		let source = bind_self_unconditional_natural_sp_recovery(
			parsed,
			NaturalSpRecoveryScope::SelfActor,
			NaturalSpRecoveryCondition::Unconditional,
			NaturalSpRecoveryStacking::Independent,
			tick_rate(),
		)
		.expect("reviewed self-unconditional semantics bind");
		assert_eq!(source.amount, ExpectedValue::from_scaled(300_000));
		assert_eq!(source.interval_ticks, 30);
	}

	#[test]
	fn negative_nonfinite_string_and_extra_blackboard_facts_fail_closed() {
		for facts in [
			vec![PassiveSpBlackboardFact::new(
				PASSIVE_SP_RECOVERY_KEY,
				-0.3,
				None,
			)],
			vec![PassiveSpBlackboardFact::new(
				PASSIVE_SP_RECOVERY_KEY,
				f64::NAN,
				None,
			)],
			vec![PassiveSpBlackboardFact::new(
				PASSIVE_SP_RECOVERY_KEY,
				0.3,
				Some("0.3"),
			)],
			vec![
				PassiveSpBlackboardFact::new(PASSIVE_SP_RECOVERY_KEY, 0.3, None),
				PassiveSpBlackboardFact::new("extra", 1.0, None),
			],
		] {
			assert!(parse_natural_sp_recovery_facts(&facts).is_none());
		}
	}
}
