use ark_solver::{
	ArtifactAction, ArtifactEnemyAssessment, ArtifactExactBuild, ArtifactFacing,
	ArtifactOperatorAssessment, ArtifactUnsupportedMechanic, DeterministicDamageStatus,
	ExpectedStrategyStatus, MaterializedActionKind, StrategyArtifact, StrategyArtifactError,
	STRATEGY_ARTIFACT_SCHEMA_VERSION,
};

const V4_GOLDEN_BYTE_LENGTH: usize = 656;
// c6140b710719742c2abb1c1b7a3e57c8675503ad422112b6d85889e151f86a1f
const V4_GOLDEN_SHA256: [u8; 32] = [
	198, 20, 11, 113, 7, 25, 116, 44, 42, 187, 28, 27, 122, 62, 87, 200, 103, 85, 3, 173, 66, 33,
	18, 182, 216, 88, 137, 225, 81, 248, 106, 31,
];

fn artifact() -> StrategyArtifact {
	StrategyArtifact {
		schema_version: STRATEGY_ARTIFACT_SCHEMA_VERSION,
		data_revision: "revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		guide_sha256: [7; 32],
		guide_variant_id: "bd7-normal-shu".to_owned(),
		damage_status: DeterministicDamageStatus::Feasible,
		battlefield_status: ExpectedStrategyStatus::ExpectedVictory,
		minimum_deadline_margin_ticks: 200,
		minimum_operator_hp_micros: 15_000,
		actions: vec![ArtifactAction {
			action_index: 0,
			kind: MaterializedActionKind::Deploy,
			operator_id: Some("char_2024_chyue2".to_owned()),
			row: Some(3),
			column: Some(3),
			facing: ArtifactFacing::Right,
			scheduled_tick: 300,
		}],
		builds: vec![ArtifactExactBuild {
			operator_id: "char_2024_chyue2".to_owned(),
			elite: 2,
			level: 90,
			trust_percent: 200,
			potential_rank: 6,
			skill_slot: 1,
			skill_level: 10,
			module_id: None,
			module_level: 0,
		}],
		enemies: vec![ArtifactEnemyAssessment {
			enemy_instance_id: "wave0-fragment0-action0-instance0".to_owned(),
			spawned_tick: Some(90),
			deadline_tick: Some(900),
			max_hp_micros: 10_000,
			remaining_hp_micros: 0,
			credited_damage_micros: 10_000,
			death_tick: Some(700),
			leaked_tick: None,
			deadline_margin_ticks: 200,
		}],
		operators: vec![ArtifactOperatorAssessment {
			operator_id: "char_2024_chyue2".to_owned(),
			max_hp_micros: 20_000,
			remaining_hp_micros: 15_000,
			deployed_tick: Some(300),
			death_tick: None,
		}],
		unsupported: vec![],
		trace_sha256: [9; 32],
	}
}

#[test]
fn v4_canonical_bytes_round_trip_and_match_golden() {
	let value: StrategyArtifact = artifact();
	let first: Vec<u8> = value.encode().unwrap();
	let second: Vec<u8> = value.encode().unwrap();
	assert_eq!(first, second);
	assert_eq!(StrategyArtifact::decode(&first).unwrap(), value);
	assert_eq!(first.len(), V4_GOLDEN_BYTE_LENGTH);
	assert_eq!(value.artifact_id().unwrap(), V4_GOLDEN_SHA256);
}

#[test]
fn canonicalization_sorts_identity_vectors_without_changing_action_sequence() {
	let mut value: StrategyArtifact = artifact();
	let mut later = value.builds[0].clone();
	later.operator_id = "z".to_owned();
	value.builds.insert(0, later);
	value.canonicalize();
	assert_eq!(value.builds[0].operator_id, "char_2024_chyue2");
	assert_eq!(value.actions[0].action_index, 0);
}

#[test]
fn malformed_identifier_and_semantics_fail_closed() {
	let mut bytes: Vec<u8> = artifact().encode().unwrap();
	bytes[4..8].copy_from_slice(b"NOPE");
	assert_eq!(
		StrategyArtifact::decode(&bytes).unwrap_err(),
		StrategyArtifactError::InvalidIdentifier
	);

	let mut duplicate: StrategyArtifact = artifact();
	duplicate.builds.push(duplicate.builds[0].clone());
	assert!(matches!(
		duplicate.encode(),
		Err(StrategyArtifactError::DuplicateIdentity {
			field: "build operator"
		})
	));
}

#[test]
fn mechanics_revision_must_be_canonical_sha256() {
	let invalid_revisions: [String; 4] = [
		String::new(),
		"A".to_owned(),
		"A".repeat(64),
		"a".repeat(63),
	];
	for invalid in invalid_revisions {
		let mut value: StrategyArtifact = artifact();
		value.mechanics_revision = invalid;
		assert_eq!(
			value.encode().unwrap_err(),
			StrategyArtifactError::InvalidSha256Hex {
				field: "mechanics_revision",
			}
		);
	}
}

#[test]
fn unsupported_mechanics_are_preserved_as_auditable_data() {
	let mut value: StrategyArtifact = artifact();
	value.damage_status = DeterministicDamageStatus::Unsupported;
	value.battlefield_status = ExpectedStrategyStatus::Unsupported;
	value.unsupported.push(ArtifactUnsupportedMechanic {
		mechanic_id: "enemy:cross_attack".to_owned(),
		origin: "enemy_100".to_owned(),
	});
	let decoded: StrategyArtifact = StrategyArtifact::decode(&value.encode().unwrap()).unwrap();
	assert_eq!(decoded, value);
	assert_eq!(
		decoded.damage_status,
		DeterministicDamageStatus::Unsupported
	);
}

#[test]
fn damage_feasibility_is_independent_from_unsupported_battlefield_replay() {
	let mut value: StrategyArtifact = artifact();
	value.damage_status = DeterministicDamageStatus::Feasible;
	value.battlefield_status = ExpectedStrategyStatus::Unsupported;
	value.unsupported.push(ArtifactUnsupportedMechanic {
		mechanic_id: "stage:snow_layer".to_owned(),
		origin: "level_bd_7".to_owned(),
	});

	let decoded: StrategyArtifact = StrategyArtifact::decode(&value.encode().unwrap()).unwrap();
	assert_eq!(decoded.damage_status, DeterministicDamageStatus::Feasible);
	assert_eq!(
		decoded.battlefield_status,
		ExpectedStrategyStatus::Unsupported
	);
}

#[test]
fn infeasible_damage_status_round_trips_without_implying_battlefield_defeat() {
	let mut value: StrategyArtifact = artifact();
	value.damage_status = DeterministicDamageStatus::Infeasible;
	value.battlefield_status = ExpectedStrategyStatus::Incomplete;
	value.enemies[0].remaining_hp_micros = 2_000;
	value.enemies[0].credited_damage_micros = 8_000;
	value.enemies[0].death_tick = None;

	let decoded: StrategyArtifact = StrategyArtifact::decode(&value.encode().unwrap()).unwrap();
	assert_eq!(decoded.damage_status, DeterministicDamageStatus::Infeasible);
	assert_eq!(
		decoded.battlefield_status,
		ExpectedStrategyStatus::Incomplete
	);
}

#[test]
fn optional_ticks_outside_the_wire_range_fail_closed() {
	let mut value: StrategyArtifact = artifact();
	value.enemies[0].death_tick = Some(i64::MAX.cast_unsigned() + 1);

	assert_eq!(
		value.encode().unwrap_err(),
		StrategyArtifactError::TickOverflow {
			field: "enemy.death_tick",
			value: i64::MAX.cast_unsigned() + 1,
		}
	);
}
