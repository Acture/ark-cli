use std::num::NonZeroU32;

use ark_solver::data::ResolvedBasePanelBuild;
use ark_solver::mechanics::{
	compile::{
		compile_base_panel_normal_attack, compile_base_panel_normal_attack_facts,
		deterministic_attack_interval_ticks,
	},
	domain::{
		AttackIntervalError, AttackTimingField, BasePanelNormalAttackFacts, FixedDamage,
		FixedDamageError, NormalAttackCatalogEntry, NormalAttackCatalogUnsupportedReason,
		NormalAttackDamageType, NormalAttackFirstDelayRule, NormalAttackTargetFilter,
		NormalAttackTargetPriority, ReviewedNormalAttackArchetype,
	},
};
use ark_solver::simulation::TickRate;

fn tick_rate(value: u32) -> TickRate {
	TickRate::new(NonZeroU32::new(value).unwrap())
}

fn facts(
	operator_id: &str,
	profession: &str,
	sub_profession_id: &str,
	attack: f64,
	base_attack_time_seconds: f64,
	attack_speed: f64,
) -> BasePanelNormalAttackFacts {
	BasePanelNormalAttackFacts::new(
		operator_id.to_owned(),
		profession.to_owned(),
		sub_profession_id.to_owned(),
		attack,
		base_attack_time_seconds,
		attack_speed,
	)
}

#[test]
fn resolved_build_adapter_has_the_public_typed_signature() {
	let compiler: fn(&ResolvedBasePanelBuild, TickRate) -> NormalAttackCatalogEntry =
		compile_base_panel_normal_attack;
	let _compiler = compiler;
}

#[test]
fn corecaster_compiles_to_reviewed_single_target_arts_profile() {
	let entry: NormalAttackCatalogEntry = compile_base_panel_normal_attack_facts(
		&facts(
			"char_caster",
			"CASTER",
			"corecaster",
			123.456_789_4,
			1.25,
			100.0,
		),
		tick_rate(30),
	);
	let supported = entry.as_supported().expect("core caster is reviewed");
	let profile = supported.profile();

	assert_eq!(
		supported.archetype(),
		ReviewedNormalAttackArchetype::CoreCaster
	);
	assert_eq!(
		supported.first_delay_rule(),
		NormalAttackFirstDelayRule::OneAttackInterval
	);
	assert_eq!(
		profile.target_priority(),
		NormalAttackTargetPriority::Standard
	);
	assert_eq!(profile.target_count().get(), 1);
	assert_eq!(
		profile.target_filter(),
		NormalAttackTargetFilter::GroundAndAir
	);
	assert_eq!(profile.packets().len(), 1);
	assert_eq!(
		profile.packets()[0].damage_type(),
		NormalAttackDamageType::Arts
	);
	assert_eq!(
		profile.packets()[0].base_damage(),
		FixedDamage::from_parts(123, 456_789).unwrap()
	);
	assert_eq!(profile.attack_interval_ticks().get(), 38);
	assert_eq!(profile.first_attack_delay_ticks(), 38);
}

#[test]
fn fastshot_compiles_to_reviewed_single_target_physical_ground_and_air_profile() {
	let entry: NormalAttackCatalogEntry = compile_base_panel_normal_attack_facts(
		&facts("char_fastshot", "SNIPER", "fastshot", 456.789, 1.0, 125.0),
		tick_rate(30),
	);
	let supported = entry.as_supported().expect("fastshot sniper is reviewed");
	let profile = supported.profile();

	assert_eq!(
		supported.archetype(),
		ReviewedNormalAttackArchetype::FastshotSniper
	);
	assert_eq!(
		supported.first_delay_rule(),
		NormalAttackFirstDelayRule::OneAttackInterval
	);
	assert_eq!(
		profile.target_priority(),
		NormalAttackTargetPriority::Standard
	);
	assert_eq!(profile.target_count().get(), 1);
	assert_eq!(
		profile.target_filter(),
		NormalAttackTargetFilter::GroundAndAir
	);
	assert_eq!(profile.packets().len(), 1);
	assert_eq!(
		profile.packets()[0].damage_type(),
		NormalAttackDamageType::Physical
	);
	assert_eq!(
		profile.packets()[0].base_damage(),
		FixedDamage::from_parts(456, 789_000).unwrap()
	);
	assert_eq!(profile.attack_interval_ticks().get(), 24);
	assert_eq!(profile.first_attack_delay_ticks(), 24);
}

#[test]
fn pioneer_compiles_to_reviewed_single_target_physical_profile() {
	let entry: NormalAttackCatalogEntry = compile_base_panel_normal_attack_facts(
		&facts("char_vanguard", "PIONEER", "pioneer", 100.0, 1.0, 125.0),
		tick_rate(30),
	);
	let supported = entry.as_supported().expect("pioneer is reviewed");
	let profile = supported.profile();

	assert_eq!(
		supported.archetype(),
		ReviewedNormalAttackArchetype::Pioneer
	);
	assert_eq!(profile.target_count().get(), 1);
	assert_eq!(
		profile.target_filter(),
		NormalAttackTargetFilter::GroundOnly
	);
	assert_eq!(
		profile.packets()[0].damage_type(),
		NormalAttackDamageType::Physical
	);
	assert_eq!(profile.attack_interval_ticks().get(), 24);
	assert_eq!(profile.first_attack_delay_ticks(), 24);
}

#[test]
fn guardian_compiles_to_reviewed_single_target_physical_profile() {
	let entry = compile_base_panel_normal_attack_facts(
		&facts("char_guardian", "TANK", "guardian", 485.0, 1.2, 100.0),
		tick_rate(30),
	);
	let supported = entry.as_supported().expect("guardian is reviewed");

	assert_eq!(
		supported.archetype(),
		ReviewedNormalAttackArchetype::GuardianDefender
	);
	assert_eq!(
		supported.profile().target_filter(),
		NormalAttackTargetFilter::GroundOnly
	);
	assert_eq!(
		supported.profile().packets()[0].damage_type(),
		NormalAttackDamageType::Physical
	);
}

#[test]
fn unknown_special_and_mismatched_archetypes_never_default_to_physical() {
	let cases: [(&str, &str); 3] = [
		("SNIPER", "marksman"),
		("CASTER", "splashcaster"),
		("PIONEER", "corecaster"),
	];

	for (profession, sub_profession_id) in cases {
		let entry: NormalAttackCatalogEntry = compile_base_panel_normal_attack_facts(
			&facts(
				"char_unknown",
				profession,
				sub_profession_id,
				100.0,
				1.0,
				100.0,
			),
			tick_rate(30),
		);
		let unsupported = entry
			.as_unsupported()
			.expect("unreviewed archetype must fail closed");
		assert_eq!(unsupported.operator_id(), "char_unknown");
		assert_eq!(
			unsupported.reason(),
			&NormalAttackCatalogUnsupportedReason::UnreviewedArchetype {
				profession: profession.to_owned(),
				sub_profession_id: sub_profession_id.to_owned(),
			}
		);
	}
}

#[test]
fn interval_conversion_rounds_up_after_exact_six_decimal_normalization() {
	assert_eq!(
		deterministic_attack_interval_ticks(1.25, 100.0, tick_rate(30))
			.unwrap()
			.get(),
		38
	);
	assert_eq!(
		deterministic_attack_interval_ticks(0.1, 100.0, tick_rate(30))
			.unwrap()
			.get(),
		3
	);
	assert_eq!(
		deterministic_attack_interval_ticks(1.0, 333.333_333, tick_rate(30))
			.unwrap()
			.get(),
		10
	);
	assert_eq!(
		deterministic_attack_interval_ticks(9_000_000_000_000.0, 0.000_001, tick_rate(u32::MAX),),
		Err(AttackIntervalError::TickOverflow)
	);
}

#[test]
fn invalid_timing_values_return_typed_unsupported_reasons() {
	let cases: [(f64, f64, AttackIntervalError); 4] = [
		(
			f64::NAN,
			100.0,
			AttackIntervalError::NonFinite {
				field: AttackTimingField::BaseAttackTime,
			},
		),
		(
			1.0,
			0.0,
			AttackIntervalError::NonPositive {
				field: AttackTimingField::AttackSpeed,
			},
		),
		(
			0.000_000_1,
			100.0,
			AttackIntervalError::BelowSixDecimalResolution {
				field: AttackTimingField::BaseAttackTime,
			},
		),
		(
			f64::MAX,
			100.0,
			AttackIntervalError::InputOverflow {
				field: AttackTimingField::BaseAttackTime,
			},
		),
	];

	for (base_attack_time, attack_speed, expected) in cases {
		let entry: NormalAttackCatalogEntry = compile_base_panel_normal_attack_facts(
			&facts(
				"char_caster",
				"CASTER",
				"corecaster",
				100.0,
				base_attack_time,
				attack_speed,
			),
			tick_rate(30),
		);
		assert_eq!(
			entry.as_unsupported().unwrap().reason(),
			&NormalAttackCatalogUnsupportedReason::InvalidAttackInterval(expected)
		);
	}
}

#[test]
fn invalid_attack_value_is_not_silently_coerced() {
	for (attack, expected) in [
		(f64::NAN, FixedDamageError::NonFinite),
		(-1.0, FixedDamageError::Negative),
	] {
		let entry: NormalAttackCatalogEntry = compile_base_panel_normal_attack_facts(
			&facts("char_caster", "CASTER", "corecaster", attack, 1.0, 100.0),
			tick_rate(30),
		);
		assert_eq!(
			entry.as_unsupported().unwrap().reason(),
			&NormalAttackCatalogUnsupportedReason::InvalidBaseDamage(expected)
		);
	}
}
