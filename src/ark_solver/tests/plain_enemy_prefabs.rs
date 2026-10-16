//! The Plain Enemy Prefab Template against the pinned gamedata and battle-prefab snapshots.
//!
//! The six 1-1 enemy records must decode from their enemy packs with table facts compiled from the
//! stage, the ranged linear projectile form must decode for a record outside 1-1, and the splash
//! caster whose prefab shares the ranged trigger layout must be rejected.

use ark_solver::data::{
	ArkDataSnapshot, BattlePrefabEntryKind, BattlePrefabSnapshot, CompiledEnemy, CompiledStage,
	StageDifficulty, StageIdentity, StageRef,
};
use ark_solver::mechanics::compile::{
	built_in_battle_prefab_handler_registry, decode_plain_enemy_prefab, review_undescribed_enemy,
};
use ark_solver::mechanics::domain::{
	DecodedPlainEnemyPrefab, PlainEnemyAttackForm, PlainEnemyPrefabDecode,
	PlainEnemyPrefabDecodeError, PlainEnemyTableFacts, ProjectileMovementSchema,
	ReviewedEnemyDamageType,
};
use ark_solver::mechanics::prefab::{BattlePrefabHandlerRegistry, BattlePrefabSemanticIssue};

mod support;
use support::local_battle_snapshots;

const GAMEDATA_REVISION: &str = "67472377b1ac84a45d38b8277b79e3a298985675";
const MECHANICS_REVISION_PREFIX: &str = "824157d2";
const STAGE_ID: &str = "main_01-01";

#[derive(Clone, Copy, Debug)]
enum ExpectedForm {
	Melee,
	Ranged(&'static str, ProjectileMovementSchema),
}

/// Each 1-1 record: prefab key, enemy pack, form and damage family.
const STAGE_RECORDS: [(&str, &str, ExpectedForm, ReviewedEnemyDamageType); 6] = [
	(
		"enemy_1000_gopro",
		"enm_pfb_0",
		ExpectedForm::Melee,
		ReviewedEnemyDamageType::Physical,
	),
	(
		"enemy_1000_gopro_2",
		"enm_pfb_0",
		ExpectedForm::Melee,
		ReviewedEnemyDamageType::Physical,
	),
	(
		"enemy_1007_slime_2",
		"enm_pfb_2",
		ExpectedForm::Melee,
		ReviewedEnemyDamageType::Physical,
	),
	(
		"enemy_1027_mob",
		"enm_pfb_3",
		ExpectedForm::Melee,
		ReviewedEnemyDamageType::Physical,
	),
	(
		"enemy_1028_mocock",
		"enm_pfb_3",
		ExpectedForm::Ranged("projectile_mocock", ProjectileMovementSchema::Paracurve),
		ReviewedEnemyDamageType::Physical,
	),
	(
		"enemy_1030_wteeth",
		"enm_pfb_3",
		ExpectedForm::Melee,
		ReviewedEnemyDamageType::Physical,
	),
];

/// Table facts exactly as compile builds them: stage-overridden apply way and range radius, block
/// demand defaulting to 1, and the handbook family accepted by the undescribed-record gate.
fn stage_facts(enemy: &CompiledEnemy) -> PlainEnemyTableFacts {
	let undescribed = review_undescribed_enemy(enemy).expect("1-1 records describe no behaviour");
	PlainEnemyTableFacts {
		prefab_key: enemy
			.prefab_key()
			.expect("1-1 records declare their prefab key")
			.to_owned(),
		apply_way: enemy.apply_way_name().map(str::to_owned),
		range_radius: enemy.stats().range_radius(),
		block_demand: u16::try_from(enemy.stats().block_count().unwrap_or(1))
			.expect("block count fits the replay's block demand"),
		damage: undescribed.normal_attack_damage_type,
	}
}

/// Neither record is in a stage this test compiles, so their facts are the level-0 values of
/// `levels/enemydata/enemy_database.json` `.enemies[].Value[0].enemyData` (`applyWay`,
/// `rangeRadius`, `attributes.blockCnt` undefined, hence 1) and `enemy_handbook_table.json`
/// `.enemyData[].damageType` (`["MAGIC"]`, hence Arts) at gamedata revision `67472377…`.
fn literal_ranged_arts_facts(prefab_key: &str, range_radius: f64) -> PlainEnemyTableFacts {
	PlainEnemyTableFacts {
		prefab_key: prefab_key.to_owned(),
		apply_way: Some("RANGED".to_owned()),
		range_radius: Some(range_radius),
		block_demand: 1,
		damage: ReviewedEnemyDamageType::Arts,
	}
}

fn decoded(
	prefabs: &BattlePrefabSnapshot,
	registry: &BattlePrefabHandlerRegistry,
	facts: &PlainEnemyTableFacts,
) -> DecodedPlainEnemyPrefab {
	match decode_plain_enemy_prefab(prefabs, registry, facts) {
		Ok(PlainEnemyPrefabDecode::Decoded(decoded)) => *decoded,
		Ok(PlainEnemyPrefabDecode::NoEnemyPacks) => {
			panic!("the pinned snapshot carries enemy packs")
		}
		Err(error) => panic!("{} must decode: {error}", facts.prefab_key),
	}
}

fn describe(decoded: &DecodedPlainEnemyPrefab) -> String {
	let form: String = match &decoded.form {
		PlainEnemyAttackForm::Melee => "Melee".to_owned(),
		PlainEnemyAttackForm::RangedProjectile {
			projectile_key,
			projectile,
			movement,
			speed,
		} => format!(
			"RangedProjectile({projectile_key}, {movement:?}, speed {speed}) in {} ({}, pathID {})",
			projectile.logical_path, projectile.root.cab_name, projectile.root.path_id
		),
	};
	format!(
		"{} ({}, pathID {}) {form} {:?}; {} consumed issues, {} provenance entries",
		decoded.location.logical_path,
		decoded.location.root.cab_name,
		decoded.location.root.path_id,
		decoded.damage,
		decoded.consumed_issues.len(),
		decoded.provenance.len()
	)
}

fn assert_form(key: &str, decoded: &DecodedPlainEnemyPrefab, expected: ExpectedForm) {
	match (expected, &decoded.form) {
		(ExpectedForm::Melee, PlainEnemyAttackForm::Melee) => {}
		(
			ExpectedForm::Ranged(expected_key, expected_movement),
			PlainEnemyAttackForm::RangedProjectile {
				projectile_key,
				projectile,
				movement,
				..
			},
		) => {
			assert_eq!(projectile_key, expected_key, "{key}");
			assert_eq!(*movement, expected_movement, "{key}");
			assert_eq!(projectile.logical_path, "[uc]projectiles", "{key}");
			assert_eq!(
				projectile.bundle_kind,
				BattlePrefabEntryKind::Common,
				"{key}"
			);
		}
		(expected, found) => panic!("{key}: expected {expected:?}, found {found:?}"),
	}
}

#[test]
#[ignore = "requires configured gamedata and battle-prefab snapshots"]
fn pinned_plain_enemy_prefabs_decode_and_the_splash_caster_does_not() {
	let (snapshot, prefabs): (ArkDataSnapshot, BattlePrefabSnapshot) = local_battle_snapshots();
	eprintln!("gamedata revision {}", snapshot.resolved_revision());
	eprintln!("mechanics revision {}", prefabs.mechanics_revision());
	assert_eq!(snapshot.resolved_revision(), GAMEDATA_REVISION);
	assert!(prefabs
		.mechanics_revision()
		.starts_with(MECHANICS_REVISION_PREFIX));
	assert!(prefabs
		.entries()
		.iter()
		.any(|entry| entry.bundle_kind == BattlePrefabEntryKind::Enemypack));

	let registry: BattlePrefabHandlerRegistry = built_in_battle_prefab_handler_registry();
	let stage: CompiledStage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(STAGE_ID.to_owned()),
			difficulty: StageDifficulty::Normal,
		})
		.expect("the pinned 1-1 stage compiles");
	assert_eq!(stage.enemies().len(), STAGE_RECORDS.len());
	for (key, pack, form, damage) in STAGE_RECORDS {
		let enemy: &CompiledEnemy = stage
			.enemies()
			.find(|(enemy_key, _)| enemy_key.as_str() == key)
			.map(|(_, enemy)| enemy)
			.unwrap_or_else(|| panic!("1-1 compiles {key}"));
		let decoded: DecodedPlainEnemyPrefab = decoded(&prefabs, &registry, &stage_facts(enemy));
		eprintln!("{key}: {}", describe(&decoded));
		assert_eq!(decoded.location.logical_path, pack, "{key}");
		assert_eq!(
			decoded.location.bundle_kind,
			BattlePrefabEntryKind::Enemypack,
			"{key}"
		);
		assert_eq!(decoded.damage, damage, "{key}");
		assert!(decoded
			.provenance
			.iter()
			.all(|entry| entry.mechanics_revision == prefabs.mechanics_revision()));
		assert_form(key, &decoded, form);
		let expected_issues: usize = match form {
			ExpectedForm::Melee => 9,
			ExpectedForm::Ranged(..) => 18,
		};
		assert_consumed_issues(key, &decoded, expected_issues);
	}

	let wizard: DecodedPlainEnemyPrefab = decoded(
		&prefabs,
		&registry,
		&literal_ranged_arts_facts("enemy_1011_wizard_2", 2.0),
	);
	eprintln!("enemy_1011_wizard_2: {}", describe(&wizard));
	assert_eq!(wizard.damage, ReviewedEnemyDamageType::Arts);
	assert_eq!(wizard.location.logical_path, "enm_pfb_3");
	assert_eq!(
		wizard.location.bundle_kind,
		BattlePrefabEntryKind::Enemypack
	);
	assert!(matches!(
		wizard.form,
		PlainEnemyAttackForm::RangedProjectile { speed, .. } if speed == 10.0
	));
	assert_consumed_issues("enemy_1011_wizard_2", &wizard, 16);
	assert_form(
		"enemy_1011_wizard_2",
		&wizard,
		ExpectedForm::Ranged(
			"projectile_enemy_magic_ball",
			ProjectileMovementSchema::Linear,
		),
	);

	let splash: PlainEnemyPrefabDecodeError = decode_plain_enemy_prefab(
		&prefabs,
		&registry,
		&literal_ranged_arts_facts("enemy_1018_aoemag", 2.2),
	)
	.expect_err("the splash caster is not a plain form");
	eprintln!("enemy_1018_aoemag: {splash}");
	assert!(
		matches!(
			splash,
			PlainEnemyPrefabDecodeError::UnsupportedModeWiring { .. }
		),
		"{splash}"
	);
	assert!(
		splash
			.to_string()
			.starts_with("plain enemy prefab R11 mode wiring: "),
		"{splash}"
	);
}

/// Every labelled MonoBehaviour except the handled attack ability is an unknown script, and the
/// enemy's Spine skeleton asset is the one reference outside the snapshot.
fn assert_consumed_issues(key: &str, decoded: &DecodedPlainEnemyPrefab, expected: usize) {
	assert_eq!(decoded.consumed_issues.len(), expected, "{key}");
	let unavailable: Vec<&BattlePrefabSemanticIssue> = decoded
		.consumed_issues
		.iter()
		.filter(|issue| {
			matches!(
				issue,
				BattlePrefabSemanticIssue::UnavailableExternalReference { .. }
			)
		})
		.collect();
	assert!(
		matches!(
			unavailable.as_slice(),
			[BattlePrefabSemanticIssue::UnavailableExternalReference { source, field_path, .. }]
				if field_path == "$.skeletonDataAsset"
					&& source.logical_path == decoded.location.logical_path
		),
		"{key}: {unavailable:?}"
	);
	assert_eq!(
		decoded.consumed_issues.len() - unavailable.len(),
		decoded.provenance.len() - usize::from(decoded.form == PlainEnemyAttackForm::Melee),
		"{key}: one unknown script per labelled behaviour without a handler"
	);
}
