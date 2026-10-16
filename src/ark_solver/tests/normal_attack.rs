use ark_solver::mechanics::domain::{
	effective_damage, hp_capped_damage, ArtsResistance, ArtsResistanceError, DamageAccounting,
	EnemyDamageState, EnemyDamageStateError, EnemyDefenses, ExpectedRawDamageOutcome, FixedDamage,
	FixedDamageError, LockedTargetDamagePacket, NormalAttackApplicationError,
	NormalAttackDamageType, NormalAttackProfile, NormalAttackProfileError,
	NormalAttackProfileSupport, NormalAttackTargetFilter, NormalAttackTargetPriority, TargetClass,
	UnsupportedNormalAttackProfile, UnsupportedNormalAttackProfileError,
	UnsupportedNormalAttackReason,
};

fn damage(value: u64) -> FixedDamage {
	FixedDamage::from_whole(value).unwrap()
}

fn resistance(value: f64) -> ArtsResistance {
	ArtsResistance::try_from_percent_f64(value).unwrap()
}

fn defenses(physical: u64, arts_percent: f64) -> EnemyDefenses {
	EnemyDefenses::new(damage(physical), resistance(arts_percent))
}

fn packet(damage_type: NormalAttackDamageType, base_damage: u64) -> LockedTargetDamagePacket {
	LockedTargetDamagePacket::new(damage_type, damage(base_damage))
}

#[test]
fn fixed_damage_rounds_floating_inputs_to_six_decimals() {
	let rounded_down: FixedDamage = FixedDamage::try_from_f64(12.345_678_4).unwrap();
	let rounded_up: FixedDamage = FixedDamage::try_from_f64(12.345_678_6).unwrap();

	assert_eq!(rounded_down, FixedDamage::from_parts(12, 345_678).unwrap());
	assert_eq!(rounded_up, FixedDamage::from_parts(12, 345_679).unwrap());
	assert_eq!(rounded_up.to_string(), "12.345679");
	assert_eq!(rounded_up.whole_floor(), 12);
	assert_eq!(
		FixedDamage::from_parts(0, 1_000_000),
		Err(FixedDamageError::FractionOutOfRange)
	);
	assert_eq!(
		FixedDamage::try_from_f64(f64::NAN),
		Err(FixedDamageError::NonFinite)
	);
	assert_eq!(
		FixedDamage::try_from_f64(-0.000_001),
		Err(FixedDamageError::Negative)
	);
}

#[test]
fn fixed_damage_reports_checked_arithmetic_failures() {
	assert_eq!(
		FixedDamage::MAX.checked_add(FixedDamage::from_scaled(1)),
		Err(FixedDamageError::Overflow)
	);
	assert_eq!(
		FixedDamage::ZERO.checked_sub(FixedDamage::from_scaled(1)),
		Err(FixedDamageError::Underflow)
	);
	assert_eq!(
		FixedDamage::from_whole(u64::MAX),
		Err(FixedDamageError::Overflow)
	);
	assert_eq!(
		damage(1).checked_mul_ratio(1, 0),
		Err(FixedDamageError::DivisionByZero)
	);
}

#[test]
fn effective_damage_applies_physical_arts_and_true_defenses() {
	let enemy: EnemyDefenses = defenses(40, 30.0);

	assert_eq!(
		effective_damage(NormalAttackDamageType::Physical, damage(100), enemy).unwrap(),
		damage(60)
	);
	assert_eq!(
		effective_damage(
			NormalAttackDamageType::Physical,
			damage(100),
			defenses(200, 0.0),
		)
		.unwrap(),
		damage(5)
	);
	assert_eq!(
		effective_damage(NormalAttackDamageType::Arts, damage(100), enemy).unwrap(),
		damage(70)
	);
	assert_eq!(
		effective_damage(NormalAttackDamageType::True, damage(100), enemy).unwrap(),
		damage(100)
	);
	assert_eq!(
		ArtsResistance::try_from_percent_f64(100.000_001),
		Err(ArtsResistanceError::AboveOneHundredPercent)
	);
}

#[test]
fn expected_physical_outcomes_apply_defense_before_averaging() {
	let packet = LockedTargetDamagePacket::from_expected_outcomes(
		NormalAttackDamageType::Physical,
		vec![
			ExpectedRawDamageOutcome::new(500_000, damage(100)).unwrap(),
			ExpectedRawDamageOutcome::new(500_000, damage(300)).unwrap(),
		],
	)
	.unwrap();

	assert_eq!(packet.base_damage(), damage(200));
	assert_eq!(
		packet.effective_damage(defenses(150, 0.0)).unwrap(),
		FixedDamage::from_parts(77, 500_000).unwrap()
	);
	assert_ne!(
		packet.effective_damage(defenses(150, 0.0)).unwrap(),
		effective_damage(
			NormalAttackDamageType::Physical,
			packet.base_damage(),
			defenses(150, 0.0)
		)
		.unwrap()
	);
}

#[test]
fn defense_ratios_round_down_to_a_micro_unit() {
	let just_over_one: FixedDamage = FixedDamage::from_parts(1, 1).unwrap();
	let arts: FixedDamage = effective_damage(
		NormalAttackDamageType::Arts,
		just_over_one,
		defenses(0, 50.0),
	)
	.unwrap();
	let physical_floor: FixedDamage = effective_damage(
		NormalAttackDamageType::Physical,
		FixedDamage::from_parts(1, 19).unwrap(),
		defenses(100, 0.0),
	)
	.unwrap();

	assert_eq!(arts, FixedDamage::from_parts(0, 500_000).unwrap());
	assert_eq!(physical_floor, FixedDamage::from_parts(0, 50_000).unwrap());
}

/// Neither mitigation family may take a hit below five percent of the raw damage, and True damage
/// has no mitigation to floor.
#[test]
fn both_mitigated_families_floor_at_five_percent_of_the_raw_hit() {
	let raw: FixedDamage = damage(100);

	// Defense above the attack, and full resistance, each swallow the whole hit before the floor.
	assert_eq!(
		effective_damage(NormalAttackDamageType::Physical, raw, defenses(400, 0.0)).unwrap(),
		damage(5)
	);
	assert_eq!(
		effective_damage(NormalAttackDamageType::Arts, raw, defenses(0, 100.0)).unwrap(),
		damage(5)
	);
	// Just above the floor on either side, the ordinary term still wins.
	assert_eq!(
		effective_damage(NormalAttackDamageType::Physical, raw, defenses(94, 0.0)).unwrap(),
		damage(6)
	);
	assert_eq!(
		effective_damage(NormalAttackDamageType::Arts, raw, defenses(0, 94.0)).unwrap(),
		damage(6)
	);
	assert_eq!(
		effective_damage(NormalAttackDamageType::True, raw, defenses(400, 100.0)).unwrap(),
		raw
	);
}

#[test]
fn profile_constructor_enforces_target_packet_and_interval_invariants() {
	let packets: Vec<LockedTargetDamagePacket> =
		vec![packet(NormalAttackDamageType::Physical, 100)];
	assert_eq!(
		NormalAttackProfile::new(
			0,
			packets.clone(),
			10,
			30,
			NormalAttackTargetFilter::GroundOnly,
		),
		Err(NormalAttackProfileError::ZeroTargetCount)
	);
	assert_eq!(
		NormalAttackProfile::new(1, Vec::new(), 10, 30, NormalAttackTargetFilter::GroundOnly),
		Err(NormalAttackProfileError::EmptyDamagePackets)
	);
	assert_eq!(
		NormalAttackProfile::new(
			1,
			packets.clone(),
			10,
			0,
			NormalAttackTargetFilter::GroundOnly,
		),
		Err(NormalAttackProfileError::ZeroAttackInterval)
	);

	let profile: NormalAttackProfile =
		NormalAttackProfile::new(2, packets, 10, 30, NormalAttackTargetFilter::GroundOnly).unwrap();
	assert_eq!(
		profile.target_priority(),
		NormalAttackTargetPriority::Standard
	);
	assert_eq!(profile.target_count().get(), 2);
	assert_eq!(profile.first_attack_delay_ticks(), 10);
	assert_eq!(profile.attack_interval_ticks().get(), 30);
	assert_eq!(profile.checked_attack_tick(100, 0), Some(110));
	assert_eq!(profile.checked_attack_tick(100, 2), Some(170));
	assert_eq!(profile.checked_attack_tick(u64::MAX, 0), None);
}

#[test]
fn packets_apply_in_order_to_locked_targets_with_hp_caps() {
	let profile: NormalAttackProfile = NormalAttackProfile::new(
		2,
		vec![
			packet(NormalAttackDamageType::Physical, 100),
			packet(NormalAttackDamageType::Arts, 50),
		],
		0,
		30,
		NormalAttackTargetFilter::GroundAndAir,
	)
	.unwrap();
	let mut targets: Vec<EnemyDamageState<&str>> = vec![
		EnemyDamageState::new("a", TargetClass::Ground, damage(70), defenses(20, 0.0)),
		EnemyDamageState::new("b", TargetClass::Air, damage(200), defenses(150, 50.0)),
	];

	let report = profile.apply_locked_targets(&mut targets).unwrap();
	let application_order: Vec<(usize, usize)> = report
		.applications()
		.iter()
		.map(|application| {
			(
				application.packet_index(),
				application.locked_target_index(),
			)
		})
		.collect();

	assert_eq!(application_order, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
	assert_eq!(
		report.accounting(),
		DamageAccounting::ZERO
			.checked_add(hp_capped_damage(damage(70), damage(80)).accounting())
			.unwrap()
			.checked_add(hp_capped_damage(damage(200), damage(5)).accounting())
			.unwrap()
			.checked_add(hp_capped_damage(FixedDamage::ZERO, damage(50)).accounting())
			.unwrap()
			.checked_add(hp_capped_damage(damage(195), damage(25)).accounting())
			.unwrap()
	);
	assert_eq!(report.accounting().raw(), damage(160));
	assert_eq!(report.accounting().credited(), damage(100));
	assert_eq!(report.accounting().overkill(), damage(60));
	assert_eq!(targets[0].remaining_hp(), FixedDamage::ZERO);
	assert_eq!(targets[0].accounting().raw(), damage(130));
	assert_eq!(targets[0].accounting().credited(), damage(70));
	assert_eq!(targets[0].accounting().overkill(), damage(60));
	assert_eq!(targets[1].remaining_hp(), damage(170));
	assert_eq!(targets[1].accounting().raw(), damage(30));
	assert_eq!(targets[1].accounting().credited(), damage(30));
	assert_eq!(targets[1].accounting().overkill(), FixedDamage::ZERO);
}

#[test]
fn locked_target_validation_rejects_excess_duplicates_and_filter_mismatches() {
	let profile: NormalAttackProfile = NormalAttackProfile::new(
		1,
		vec![packet(NormalAttackDamageType::True, 10)],
		0,
		30,
		NormalAttackTargetFilter::GroundOnly,
	)
	.unwrap();
	let ground =
		|| EnemyDamageState::new("same", TargetClass::Ground, damage(50), defenses(0, 0.0));
	let mut too_many: Vec<EnemyDamageState<&str>> = vec![
		ground(),
		EnemyDamageState::new("other", TargetClass::Ground, damage(50), defenses(0, 0.0)),
	];
	assert!(matches!(
		profile.apply_locked_targets(&mut too_many),
		Err(NormalAttackApplicationError::TooManyLockedTargets { .. })
	));

	let duplicate_profile: NormalAttackProfile = NormalAttackProfile::new(
		2,
		vec![packet(NormalAttackDamageType::True, 10)],
		0,
		30,
		NormalAttackTargetFilter::GroundOnly,
	)
	.unwrap();
	let mut duplicates: Vec<EnemyDamageState<&str>> = vec![ground(), ground()];
	assert_eq!(
		duplicate_profile.apply_locked_targets(&mut duplicates),
		Err(NormalAttackApplicationError::DuplicateLockedTarget {
			first_index: 0,
			duplicate_index: 1,
		})
	);

	let mut air: Vec<EnemyDamageState<&str>> = vec![EnemyDamageState::new(
		"air",
		TargetClass::Air,
		damage(50),
		defenses(0, 0.0),
	)];
	assert!(matches!(
		profile.apply_locked_targets(&mut air),
		Err(NormalAttackApplicationError::TargetRejectedByFilter {
			locked_target_index: 0,
			..
		})
	));
	assert_eq!(air[0].remaining_hp(), damage(50));
	assert_eq!(air[0].accounting(), DamageAccounting::ZERO);
}

#[test]
fn arithmetic_failure_leaves_all_locked_targets_unchanged() {
	let profile: NormalAttackProfile = NormalAttackProfile::new(
		1,
		vec![
			LockedTargetDamagePacket::new(NormalAttackDamageType::True, FixedDamage::MAX),
			LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_scaled(1),
			),
		],
		0,
		1,
		NormalAttackTargetFilter::GroundOnly,
	)
	.unwrap();
	let mut targets: Vec<EnemyDamageState<&str>> = vec![EnemyDamageState::new(
		"dead",
		TargetClass::Ground,
		FixedDamage::ZERO,
		defenses(0, 0.0),
	)];

	assert_eq!(
		profile.apply_locked_targets(&mut targets),
		Err(NormalAttackApplicationError::Arithmetic(
			FixedDamageError::Overflow,
		))
	);
	assert_eq!(targets[0].remaining_hp(), FixedDamage::ZERO);
	assert_eq!(targets[0].accounting(), DamageAccounting::ZERO);
}

#[test]
fn initial_remaining_hp_and_unsupported_profiles_fail_closed() {
	assert_eq!(
		EnemyDamageState::with_remaining_hp(
			"enemy",
			TargetClass::Ground,
			damage(10),
			damage(11),
			defenses(0, 0.0),
		),
		Err(EnemyDamageStateError::RemainingHpExceedsMaximum)
	);
	assert_eq!(
		UnsupportedNormalAttackProfile::from_reasons(Vec::new()),
		Err(UnsupportedNormalAttackProfileError::NoReasons)
	);

	let unsupported: UnsupportedNormalAttackProfile =
		UnsupportedNormalAttackProfile::new(UnsupportedNormalAttackReason::Splash)
			.with_reason(UnsupportedNormalAttackReason::Chain);
	let support: NormalAttackProfileSupport = NormalAttackProfileSupport::Unsupported(unsupported);
	assert_eq!(support.as_supported(), None);
	assert_eq!(
		support.unsupported_reasons(),
		Some(
			[
				UnsupportedNormalAttackReason::Splash,
				UnsupportedNormalAttackReason::Chain,
			]
			.as_slice()
		)
	);
}
