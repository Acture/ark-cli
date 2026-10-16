use std::num::NonZeroU32;

use ark_solver::data::{OperatorPosition, SkillActivationType, SkillDurationType, SkillSpType};
use ark_solver::mechanics::{
	compile::{
		compile_expected_damage_skill_facts, compile_expected_damage_skill_facts_with_binding,
	},
	domain::{
		effective_damage, ArtsResistance, EnemyDefenses, ExpectedDamageDelivery,
		ExpectedDamageSkillEntry, ExpectedDamageType, ExpectedSkillDuration, FixedDamage,
		NormalAttackDamageType, SkillDamageBindingSelector, SkillDamageBlackboardFact,
		SkillDamageFacts, SkillDamageMechanicTemplate, SkillDamageSemantic,
		SkillDamageTemplateBinding, SkillDamageUnsupportedReason, FIXED_DAMAGE_SCALE,
	},
};
use ark_solver::simulation::TickRate;

fn tick_rate() -> TickRate {
	TickRate::new(NonZeroU32::new(30).unwrap())
}

fn fact(key: &str, value: f64) -> SkillDamageBlackboardFact {
	SkillDamageBlackboardFact::new(key.to_owned(), value, None)
}

fn caster_facts(blackboard: Vec<SkillDamageBlackboardFact>) -> SkillDamageFacts {
	SkillDamageFacts::new(
		"char_test".to_owned(),
		"skill_test".to_owned(),
		None,
		false,
		"CASTER".to_owned(),
		"corecaster".to_owned(),
		OperatorPosition::Ranged,
		1_000.0,
		1.6,
		100.0,
		"base-range".to_owned(),
		Some("skill-range".to_owned()),
		SkillActivationType::Manual,
		SkillDurationType::None,
		20.0,
		SkillSpType::IncreaseWithTime,
		30,
		10,
		blackboard,
	)
}

fn binding(template: SkillDamageMechanicTemplate) -> SkillDamageTemplateBinding {
	SkillDamageTemplateBinding::new(
		SkillDamageBindingSelector::SkillId("skill_test".to_owned()),
		template,
	)
	.for_operator_id("char_test")
}

#[test]
fn compiles_common_attack_buff_into_a_timed_space_time_profile() {
	let binding = binding(SkillDamageMechanicTemplate::ModifiedNormal)
		.with_semantic("atk", SkillDamageSemantic::AdditiveAttackRatio)
		.with_semantic("attack_speed", SkillDamageSemantic::AttackSpeedBonus)
		.with_semantic(
			"attack@max_target",
			SkillDamageSemantic::MaximumPrimaryTargets,
		)
		.with_ignored_key("projectile_move_scale");
	let entry = compile_expected_damage_skill_facts_with_binding(
		&caster_facts(vec![
			fact("atk", 1.0),
			fact("attack_speed", 60.0),
			fact("attack@max_target", 4.0),
			fact("projectile_move_scale", 0.05),
		]),
		tick_rate(),
		&binding,
	);
	let profile = entry.as_supported().unwrap();

	assert_eq!(profile.range().range_id(), "skill-range");
	assert_eq!(
		profile.duration(),
		ExpectedSkillDuration::Timed {
			ticks: 600.try_into().unwrap()
		}
	);
	assert_eq!(profile.active().expected_damage_per_primary_hit(), 2_000.0);
	assert_eq!(profile.active().attack_interval_ticks().get(), 30);
	assert_eq!(profile.active().maximum_primary_targets().get(), 4);
	assert_eq!(profile.active().damage_type(), ExpectedDamageType::Arts);
	assert_eq!(profile.active().delivery(), &ExpectedDamageDelivery::Direct);
	assert_eq!(
		profile.key_audit().consumed_keys(),
		["atk", "attack@max_target", "attack_speed"]
	);
	assert_eq!(
		profile.key_audit().ignored_keys(),
		["projectile_move_scale"]
	);
}

#[test]
fn compounds_additive_attack_and_per_hit_scale() {
	let binding = binding(SkillDamageMechanicTemplate::ModifiedNormal)
		.with_semantic("atk", SkillDamageSemantic::AdditiveAttackRatio)
		.with_semantic("attack@atk_scale", SkillDamageSemantic::PrimaryAttackScale);
	let entry = compile_expected_damage_skill_facts_with_binding(
		&caster_facts(vec![fact("atk", 0.5), fact("attack@atk_scale", 2.0)]),
		tick_rate(),
		&binding,
	);

	assert_eq!(
		entry
			.as_supported()
			.unwrap()
			.active()
			.expected_damage_per_primary_hit(),
		3_000.0
	);
}

#[test]
fn retains_explicitly_bound_rng_branches_as_raw_damage_outcomes() {
	let binding = binding(SkillDamageMechanicTemplate::ModifiedNormal)
		.with_semantic("attack@atk_scale", SkillDamageSemantic::PrimaryAttackScale)
		.with_semantic(
			"attack@atk_scale_2",
			SkillDamageSemantic::AlternateAttackScale,
		)
		.with_semantic("attack@prob", SkillDamageSemantic::AlternateProbability);
	let entry = compile_expected_damage_skill_facts_with_binding(
		&caster_facts(vec![
			fact("attack@atk_scale", 2.0),
			fact("attack@atk_scale_2", 4.0),
			fact("attack@prob", 0.25),
		]),
		tick_rate(),
		&binding,
	);

	let phase = entry.as_supported().unwrap().active();
	let outcomes = phase.primary_hit_outcomes().outcomes();
	assert_eq!(outcomes.len(), 2);
	assert_eq!(outcomes[0].probability(), 0.75);
	assert_eq!(outcomes[0].raw_damage(), 2_000.0);
	assert_eq!(outcomes[1].probability(), 0.25);
	assert_eq!(outcomes[1].raw_damage(), 4_000.0);
	assert_eq!(phase.expected_damage_per_primary_hit(), 2_500.0);
}

#[test]
fn averages_rng_only_after_nonlinear_physical_defense_and_minimum_damage() {
	let binding = binding(SkillDamageMechanicTemplate::ModifiedNormal)
		.with_semantic("attack@atk_scale", SkillDamageSemantic::PrimaryAttackScale)
		.with_semantic(
			"attack@atk_scale_2",
			SkillDamageSemantic::AlternateAttackScale,
		)
		.with_semantic("attack@prob", SkillDamageSemantic::AlternateProbability);
	let entry = compile_expected_damage_skill_facts_with_binding(
		&caster_facts(vec![
			fact("attack@atk_scale", 2.0),
			fact("attack@atk_scale_2", 4.0),
			fact("attack@prob", 0.25),
		]),
		tick_rate(),
		&binding,
	);
	let mixture = entry
		.as_supported()
		.unwrap()
		.active()
		.primary_hit_outcomes();
	let defenses = EnemyDefenses::new(
		FixedDamage::from_whole(3_000).unwrap(),
		ArtsResistance::ZERO,
	);
	let post_defense_expectation = mixture.expected_value_by(|raw_damage| {
		effective_damage(
			NormalAttackDamageType::Physical,
			FixedDamage::try_from_f64(raw_damage).unwrap(),
			defenses,
		)
		.unwrap()
		.scaled() as f64
			/ FIXED_DAMAGE_SCALE as f64
	});
	let defense_of_raw_expectation = effective_damage(
		NormalAttackDamageType::Physical,
		FixedDamage::try_from_f64(mixture.expected_raw_damage()).unwrap(),
		defenses,
	)
	.unwrap()
	.scaled() as f64
		/ FIXED_DAMAGE_SCALE as f64;

	assert_eq!(post_defense_expectation, 325.0);
	assert_eq!(defense_of_raw_expectation, 125.0);
	assert_ne!(post_defense_expectation, defense_of_raw_expectation);
}

#[test]
fn retains_explicitly_bound_splash_geometry_and_secondary_scale() {
	let binding = binding(SkillDamageMechanicTemplate::ModifiedNormal)
		.with_semantic("atk", SkillDamageSemantic::AdditiveAttackRatio)
		.with_semantic(
			"attack@range_radius",
			SkillDamageSemantic::SplashRadiusTiles,
		)
		.with_semantic(
			"attack@aoe_atk_scale",
			SkillDamageSemantic::SplashSecondaryDamageScale,
		);
	let entry = compile_expected_damage_skill_facts_with_binding(
		&caster_facts(vec![
			fact("atk", 0.6),
			fact("attack@range_radius", 0.9),
			fact("attack@aoe_atk_scale", 0.25),
		]),
		tick_rate(),
		&binding,
	);

	assert_eq!(
		entry.as_supported().unwrap().active().delivery(),
		&ExpectedDamageDelivery::Splash {
			radius_tiles: 0.9,
			secondary_damage_scale: 0.25,
		}
	);
}

#[test]
fn rejects_unconsumed_keys_that_may_change_damage() {
	let binding = binding(SkillDamageMechanicTemplate::ModifiedNormal)
		.with_semantic("atk", SkillDamageSemantic::AdditiveAttackRatio);
	let entry = compile_expected_damage_skill_facts_with_binding(
		&caster_facts(vec![
			fact("atk", 1.0),
			fact("attack@kill_damage", 9_999_999.0),
		]),
		tick_rate(),
		&binding,
	);
	let ExpectedDamageSkillEntry::Unsupported(unsupported) = entry else {
		panic!("kill damage must not silently compile");
	};

	assert!(unsupported.reasons().iter().any(|reason| matches!(
		reason,
		SkillDamageUnsupportedReason::UnknownDamageMechanic { key }
			if key == "attack@kill_damage"
	)));
}

#[test]
fn rejects_ammo_duration_under_a_non_ammo_template() {
	let facts = SkillDamageFacts::new(
		"char_test".to_owned(),
		"skill_test".to_owned(),
		None,
		false,
		"CASTER".to_owned(),
		"corecaster".to_owned(),
		OperatorPosition::Ranged,
		1_000.0,
		1.6,
		100.0,
		"base-range".to_owned(),
		None,
		SkillActivationType::Manual,
		SkillDurationType::Ammo,
		0.0,
		SkillSpType::IncreaseWithTime,
		30,
		10,
		Vec::new(),
	);
	let entry = compile_expected_damage_skill_facts_with_binding(
		&facts,
		tick_rate(),
		&binding(SkillDamageMechanicTemplate::ModifiedNormal),
	);

	assert!(matches!(entry, ExpectedDamageSkillEntry::Unsupported(_)));
}

#[test]
fn builtin_registry_is_exact_and_starts_with_amiya_s1_without_a_module() {
	let facts = SkillDamageFacts::new(
		"char_002_amiya".to_owned(),
		"skcom_magic_rage[3]".to_owned(),
		Some("skcom_attack_speed_up".to_owned()),
		false,
		"CASTER".to_owned(),
		"corecaster".to_owned(),
		OperatorPosition::Ranged,
		600.0,
		1.6,
		100.0,
		"base-range".to_owned(),
		None,
		SkillActivationType::Manual,
		SkillDurationType::None,
		30.0,
		SkillSpType::IncreaseWithTime,
		30,
		10,
		vec![fact("attack_speed", 90.0)],
	)
	.with_sp_increment(2.0);
	let profile = compile_expected_damage_skill_facts(&facts, tick_rate())
		.as_supported()
		.unwrap()
		.clone();

	assert_eq!(profile.active().attack_interval_ticks().get(), 26);
	assert_eq!(profile.sp_increment(), 2.0);
	assert_eq!(profile.key_audit().consumed_keys(), ["attack_speed"]);
}

#[test]
fn builtin_logos_s3_compiles_reviewed_attack_range_and_target_count() {
	let facts = SkillDamageFacts::new(
		"char_4133_logos".to_owned(),
		"skchr_logos_3".to_owned(),
		Some("skchr_logos_3".to_owned()),
		false,
		"CASTER".to_owned(),
		"corecaster".to_owned(),
		OperatorPosition::Ranged,
		1_000.0,
		1.6,
		100.0,
		"base-range".to_owned(),
		Some("3-4".to_owned()),
		SkillActivationType::Manual,
		SkillDurationType::None,
		30.0,
		SkillSpType::IncreaseWithTime,
		45,
		30,
		vec![
			fact("atk", 3.0),
			fact("attack@max_target", 4.0),
			fact("projectile_move_scale", 0.05),
		],
	);
	let profile = compile_expected_damage_skill_facts(&facts, tick_rate())
		.as_supported()
		.expect("reviewed Logos S3 should compile")
		.clone();

	assert_eq!(profile.active().expected_damage_per_primary_hit(), 4_000.0);
	assert_eq!(profile.active().maximum_primary_targets().get(), 4);
	assert_eq!(profile.range().range_id(), "3-4");
	assert_eq!(
		profile.key_audit().consumed_keys(),
		["atk", "attack@max_target"]
	);
	assert_eq!(
		profile.key_audit().ignored_keys(),
		["projectile_move_scale"]
	);
}

#[test]
fn builtin_exusiai_s2_preserves_four_hits_per_attack() {
	let facts = SkillDamageFacts::new(
		"char_103_angel".to_owned(),
		"skchr_angel_2".to_owned(),
		Some("skchr_angel_2".to_owned()),
		false,
		"SNIPER".to_owned(),
		"fastshot".to_owned(),
		OperatorPosition::Ranged,
		600.0,
		1.0,
		100.0,
		"3-3".to_owned(),
		None,
		SkillActivationType::Manual,
		SkillDurationType::None,
		15.0,
		SkillSpType::IncreaseWithTime,
		35,
		25,
		vec![fact("attack@atk_scale", 1.25), fact("attack@times", 4.0)],
	);
	let profile = compile_expected_damage_skill_facts(&facts, tick_rate())
		.as_supported()
		.expect("reviewed Exusiai S2 should compile")
		.clone();

	assert_eq!(profile.active().expected_damage_per_primary_hit(), 750.0);
	assert_eq!(profile.active().primary_hit_count().get(), 4);
	assert_eq!(
		profile.key_audit().consumed_keys(),
		["attack@atk_scale", "attack@times"]
	);
}
