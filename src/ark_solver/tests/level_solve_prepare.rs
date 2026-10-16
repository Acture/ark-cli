use std::num::{NonZeroU32, NonZeroU64};

use ark_solver::data::{SkillActivationType, SkillSpType};
use ark_solver::mechanics::domain::{ExpectedSkillDuration, ExpectedValue, PeriodicSpSource};
use ark_solver::planning::TickSpan;
use ark_solver::workflow::{
	enumerate_expected_damage_activation_schedules, ExpectedDamageActivationTiming,
	LevelSolveRoster, LevelSolveRosterError, EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION,
	EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION, EXPECTED_DAMAGE_PREPARED_REPORT_SCHEMA_VERSION,
	LEVEL_SOLVE_ROSTER_SCHEMA_VERSION,
};

#[test]
fn maa_job_output_schema_versions_are_locked() {
	assert_eq!(EXPECTED_DAMAGE_PREPARED_REPORT_SCHEMA_VERSION, 15);
	assert_eq!(EXPECTED_DAMAGE_PREPARED_JOB_SCHEMA_VERSION, 5);
	assert_eq!(EXPECTED_DAMAGE_EXPORT_VALIDATION_SCHEMA_VERSION, 7);
}

#[test]
fn roster_schema_rejects_duplicates_and_zero_skill_slots() {
	let duplicate: LevelSolveRoster = serde_json::from_str(
		r#"{
			"schema_version": 1,
			"operators": [
				{"operator": "char_test", "skill": 2},
				{"operator": "char_test", "skill": 2}
			]
		}"#,
	)
	.unwrap();
	assert_eq!(
		duplicate.validate(),
		Err(LevelSolveRosterError::DuplicateEntry {
			operator: "char_test".to_owned(),
			skill: 2,
		})
	);

	let zero: LevelSolveRoster = serde_json::from_str(
		r#"{
			"schema_version": 1,
			"operators": [{"operator": "char_test", "skill": 0}]
		}"#,
	)
	.unwrap();
	assert_eq!(
		zero.validate(),
		Err(LevelSolveRosterError::ZeroSkill { index: 0 })
	);
	assert_eq!(duplicate.schema_version, LEVEL_SOLVE_ROSTER_SCHEMA_VERSION);
}

#[test]
fn roster_schema_rejects_removed_collection_aliases() {
	for removed_field in ["builds", "entries"] {
		let json: String = format!(
			r#"{{
				"schema_version": 1,
				"{removed_field}": [{{"operator": "char_test", "skill": 1}}]
			}}"#
		);
		assert!(
			serde_json::from_str::<LevelSolveRoster>(&json).is_err(),
			"removed field {removed_field:?} must not remain a compatibility alias"
		);
	}
}

#[test]
fn manual_time_recovery_enumerates_earliest_and_boundary_aligned_schedules() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Manual,
		sp_type: SkillSpType::IncreaseWithTime,
		sp_cost: 10,
		initial_sp: 5,
		sp_increment: 1.0,
		periodic_sp_sources: Vec::new(),
		duration: ExpectedSkillDuration::Timed {
			ticks: NonZeroU64::new(60).unwrap(),
		},
		active_attack_interval_ticks: NonZeroU64::new(30).unwrap(),
	};
	let schedules = enumerate_expected_damage_activation_schedules(
		&timing,
		100,
		TickSpan {
			start: 200,
			end: 800,
		},
		&[280, 400],
		NonZeroU32::new(30).unwrap(),
		8,
	)
	.unwrap();

	assert_eq!(
		schedules[0][0],
		TickSpan {
			start: 250,
			end: 310
		}
	);
	assert!(schedules.iter().any(|schedule| schedule[0]
		== TickSpan {
			start: 280,
			end: 340
		}));
	assert!(schedules.iter().any(|schedule| schedule[0]
		== TickSpan {
			start: 340,
			end: 400
		}));
	assert_eq!(
		schedules[0][1],
		TickSpan {
			start: 610,
			end: 670,
		}
	);
}

#[test]
fn auto_skill_uses_pinned_sp_increment_for_readiness_and_recharge() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Auto,
		sp_type: SkillSpType::IncreaseWithTime,
		sp_cost: 4,
		initial_sp: 0,
		sp_increment: 2.0,
		periodic_sp_sources: Vec::new(),
		duration: ExpectedSkillDuration::Timed {
			ticks: NonZeroU64::new(30).unwrap(),
		},
		active_attack_interval_ticks: NonZeroU64::new(15).unwrap(),
	};
	let schedules = enumerate_expected_damage_activation_schedules(
		&timing,
		10,
		TickSpan { start: 0, end: 400 },
		&[200, 300],
		NonZeroU32::new(30).unwrap(),
		8,
	)
	.unwrap();

	assert_eq!(schedules.len(), 1);
	assert_eq!(
		schedules[0],
		vec![
			TickSpan {
				start: 70,
				end: 100,
			},
			TickSpan {
				start: 160,
				end: 190,
			},
			TickSpan {
				start: 250,
				end: 280,
			},
			TickSpan {
				start: 340,
				end: 370,
			},
		]
	);

	let mut invalid = timing;
	invalid.sp_increment = 0.0;
	let error = enumerate_expected_damage_activation_schedules(
		&invalid,
		10,
		TickSpan { start: 0, end: 400 },
		&[],
		NonZeroU32::new(30).unwrap(),
		1,
	)
	.unwrap_err();
	assert_eq!(error.code(), "invalid_sp_increment");
}

#[test]
fn activation_schedule_limit_fails_closed_instead_of_truncating() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Manual,
		sp_type: SkillSpType::IncreaseWithTime,
		sp_cost: 1,
		initial_sp: 1,
		sp_increment: 1.0,
		periodic_sp_sources: Vec::new(),
		duration: ExpectedSkillDuration::Timed {
			ticks: NonZeroU64::new(30).unwrap(),
		},
		active_attack_interval_ticks: NonZeroU64::new(15).unwrap(),
	};
	let error = enumerate_expected_damage_activation_schedules(
		&timing,
		0,
		TickSpan { start: 0, end: 500 },
		&[100, 200],
		NonZeroU32::new(30).unwrap(),
		1,
	)
	.unwrap_err();

	assert_eq!(error.code(), "activation_schedule_limit_exceeded");
	assert!(error.detail().contains("requires 5 schedules"));
}

#[test]
fn ammo_span_contains_all_n_delayed_attacks() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Passive,
		sp_type: SkillSpType::IncreaseWithTime,
		sp_cost: 0,
		initial_sp: 0,
		sp_increment: 1.0,
		periodic_sp_sources: Vec::new(),
		duration: ExpectedSkillDuration::Ammo {
			attacks: NonZeroU32::new(3).unwrap(),
		},
		active_attack_interval_ticks: NonZeroU64::new(10).unwrap(),
	};
	let schedules = enumerate_expected_damage_activation_schedules(
		&timing,
		5,
		TickSpan { start: 0, end: 100 },
		&[],
		NonZeroU32::new(30).unwrap(),
		1,
	)
	.unwrap();

	assert_eq!(schedules, vec![vec![TickSpan { start: 5, end: 36 }]]);
}

#[test]
fn time_recovery_combines_fractional_periodic_passive_sp_by_pulse() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Auto,
		sp_type: SkillSpType::IncreaseWithTime,
		sp_cost: 13,
		initial_sp: 0,
		sp_increment: 1.0,
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: ExpectedValue::from_scaled(300_000),
			interval_ticks: 30,
		}],
		duration: ExpectedSkillDuration::Timed {
			ticks: NonZeroU64::new(30).unwrap(),
		},
		active_attack_interval_ticks: NonZeroU64::new(15).unwrap(),
	};
	let schedules = enumerate_expected_damage_activation_schedules(
		&timing,
		0,
		TickSpan { start: 0, end: 700 },
		&[],
		NonZeroU32::new(30).unwrap(),
		1,
	)
	.unwrap();

	assert_eq!(
		schedules[0][0],
		TickSpan {
			start: 300,
			end: 330
		}
	);
	assert_eq!(
		schedules[0][1],
		TickSpan {
			start: 630,
			end: 660
		}
	);
}

#[test]
fn natural_sp_recovery_does_not_charge_attack_recovery_skills() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Auto,
		sp_type: SkillSpType::IncreaseWhenAttack,
		sp_cost: 2,
		initial_sp: 0,
		sp_increment: 1.0,
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: ExpectedValue::from_scaled(500_000),
			interval_ticks: 30,
		}],
		duration: ExpectedSkillDuration::Timed {
			ticks: NonZeroU64::new(30).unwrap(),
		},
		active_attack_interval_ticks: NonZeroU64::new(15).unwrap(),
	};
	let error = enumerate_expected_damage_activation_schedules(
		&timing,
		0,
		TickSpan { start: 0, end: 400 },
		&[],
		NonZeroU32::new(30).unwrap(),
		1,
	)
	.unwrap_err();

	assert_eq!(error.code(), "unsupported_sp_recovery");
}

#[test]
fn periodic_passive_sp_with_unmodeled_interval_fails_closed() {
	let timing = ExpectedDamageActivationTiming {
		activation_type: SkillActivationType::Auto,
		sp_type: SkillSpType::IncreaseWithTime,
		sp_cost: 1,
		initial_sp: 0,
		sp_increment: 1.0,
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: ExpectedValue::ONE,
			interval_ticks: 15,
		}],
		duration: ExpectedSkillDuration::Instant,
		active_attack_interval_ticks: NonZeroU64::new(15).unwrap(),
	};
	let error = enumerate_expected_damage_activation_schedules(
		&timing,
		0,
		TickSpan { start: 0, end: 100 },
		&[],
		NonZeroU32::new(30).unwrap(),
		1,
	)
	.unwrap_err();

	assert_eq!(error.code(), "unsupported_periodic_sp_interval");
}
