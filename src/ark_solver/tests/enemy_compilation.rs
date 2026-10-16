mod common;

use ark_solver::data::{
	ArkDataError, ArkDataSnapshot, CompiledEnemy, CompiledStage, EnemyKey, Spawn, StageDifficulty,
	StageIdentity, StageRef,
};
use common::{
	defined, minimal_enemy_database, minimal_level, undefined, Fixture, ENEMY_KEY, LEVEL_PATH,
};
use serde_json::{json, Value};

const STAGE_ID: &str = "Enemy_Normal";

#[derive(Clone, Copy, Debug)]
enum ExpectedEnemyError {
	DuplicateId,
	DuplicateLevel,
	DuplicateReference,
	MissingId,
	MissingLevel,
	MissingLocalData,
}

fn stage_table() -> Value {
	json!({
		"stages": {
			"Enemy_Normal": {
				"stageId": STAGE_ID,
				"code": "ENEMY-1",
				"levelId": "Activities/Test/Level_Test_01",
				"difficulty": "NORMAL",
				"hardStagedId": null,
				"diffGroup": null,
			},
		},
		"diffGroupTable": {},
		"runeStageGroups": {},
	})
}

fn compile_payload(level: &Value, enemy_database: &Value) -> Result<CompiledStage, ArkDataError> {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json("stage_table.json", &stage_table());
	fixture.set_verified_entry_json("enemy_database.json", enemy_database);
	fixture.set_verified_entry_json(LEVEL_PATH, level);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path())?;
	snapshot.compile_stage(StageRef {
		identity: StageIdentity::StageId(STAGE_ID.to_owned()),
		difficulty: StageDifficulty::Normal,
	})
}

fn payload_error(level: &Value, enemy_database: &Value) -> ArkDataError {
	match compile_payload(level, enemy_database) {
		Ok(_) => panic!("invalid enemy payload unexpectedly compiled"),
		Err(error) => error,
	}
}

fn database_with_level_one() -> Value {
	let mut database: Value = minimal_enemy_database();
	database["enemies"][0]["Value"]
		.as_array_mut()
		.expect("enemy levels are an array")
		.push(json!({
			"level": 1,
			"enemyData": {
				"name": undefined(json!("ignored level-one name")),
				"description": undefined(Value::Null),
				"prefabKey": undefined(json!("ignored_level_one_prefab")),
				"attributes": {
					"maxHp": defined(json!(250)),
					"atk": defined(json!(40)),
					"def": undefined(json!(500)),
					"blockCnt": defined(json!(2)),
					"magicResistance": undefined(json!(99.0)),
					"moveSpeed": undefined(json!(9.0)),
					"attackSpeed": undefined(json!(9.0)),
					"baseAttackTime": undefined(json!(9.0)),
					"massLevel": undefined(json!(9)),
				},
				"applyWay": undefined(json!("RANGED")),
				"motion": undefined(json!(1)),
				"enemyTags": undefined(json!(["ignored"])),
				"lifePointReduce": defined(json!(2)),
				"levelType": undefined(json!(2)),
				"rangeRadius": undefined(json!(9.0)),
				"numOfExtraDrops": undefined(json!(9)),
				"viewRadius": undefined(json!(9.0)),
				"notCountInTotal": defined(json!(true)),
				"skills": null,
				"talentBlackboard": null,
				"spData": null,
			},
		}));
	database
}

fn level_requesting_database_level_one(overwritten_data: Value) -> Value {
	let mut level: Value = minimal_level();
	level["enemyDbRefs"][0]["level"] = json!(1);
	level["enemyDbRefs"][0]["overwrittenData"] = overwritten_data;
	level
}

fn without_actions(level: &mut Value) {
	level["waves"] = json!([]);
	level["branches"] = json!({});
}

fn local_enemy(key: &str, prefab_key: &str, max_hp: i32) -> Value {
	json!({
		"key": key,
		"name": key,
		"description": "local fixture enemy",
		"prefabKey": prefab_key,
		"attributes": {
			"maxHp": max_hp,
			"atk": 1,
			"def": 0,
			"blockCnt": 1,
			"magicResistance": 0.0,
			"moveSpeed": 1.0,
			"attackSpeed": 100.0,
			"baseAttackTime": 1.0,
			"massLevel": 1,
		},
		"applyWay": "MELEE",
		"motion": "WALK",
		"enemyTags": [],
		"lifePointReduce": 1,
		"levelType": "NORMAL",
		"rangeRadius": 0.0,
		"notCountInTotal": false,
		"talentBlackboard": [],
		"skills": [],
		"spData": null,
	})
}

fn enemy<'a>(compiled: &'a CompiledStage, key: &str) -> &'a CompiledEnemy {
	let key: EnemyKey = EnemyKey::new(key.to_owned()).expect("nonempty fixture enemy key");
	compiled
		.enemy(&key)
		.expect("fixture enemy should be compiled")
}

fn assert_enemy_error(error: ArkDataError, expected: ExpectedEnemyError) {
	let matches_expected: bool = match expected {
		ExpectedEnemyError::DuplicateId => matches!(
			error,
			ArkDataError::DuplicateEnemyId { ref enemy_id } if enemy_id == ENEMY_KEY
		),
		ExpectedEnemyError::DuplicateLevel => matches!(
			error,
			ArkDataError::DuplicateEnemyLevel {
				ref enemy_id,
				level: 0,
			} if enemy_id == ENEMY_KEY
		),
		ExpectedEnemyError::DuplicateReference => matches!(
			error,
			ArkDataError::DuplicateEnemyReference {
				ref enemy_id,
				level: 0,
			} if enemy_id == ENEMY_KEY
		),
		ExpectedEnemyError::MissingId => matches!(
			error,
			ArkDataError::MissingEnemyId { ref enemy_id } if enemy_id == "enemy_missing"
		),
		ExpectedEnemyError::MissingLevel => matches!(
			error,
			ArkDataError::MissingEnemyLevel {
				ref enemy_id,
				requested_level: 2,
				ref available_levels,
			} if enemy_id == ENEMY_KEY && available_levels == &vec![0, 1]
		),
		ExpectedEnemyError::MissingLocalData => matches!(
			error,
			ArkDataError::MissingLocalEnemyData { ref enemy_id }
				if enemy_id == "enemy_local"
		),
	};
	assert!(
		matches_expected,
		"expected {expected:?}, received {error:?}"
	);
}

#[test]
fn database_patches_fold_before_defined_stage_overrides() {
	let mut database: Value = database_with_level_one();
	database["enemies"][0]["Value"]
		.as_array_mut()
		.expect("enemy levels are an array")
		.reverse();
	let level: Value = level_requesting_database_level_one(json!({
		"prefabKey": undefined(json!("ignored_stage_prefab")),
		"attributes": {
			"maxHp": undefined(json!(9999)),
			"atk": undefined(json!(0)),
			"def": defined(json!(0)),
			"blockCnt": undefined(json!(999)),
		},
		"lifePointReduce": defined(json!(0)),
		"notCountInTotal": defined(json!(false)),
		"skills": null,
		"talentBlackboard": null,
		"spData": null,
	}));

	let compiled: CompiledStage =
		compile_payload(&level, &database).expect("valid inherited enemy should compile");
	let enemy: &CompiledEnemy = enemy(&compiled, ENEMY_KEY);

	assert_eq!(enemy.key().as_str(), ENEMY_KEY);
	assert_eq!(enemy.database_level(), Some(1));
	assert_eq!(enemy.prefab_key(), Some("enemy_prefab_test"));
	assert_eq!(enemy.stats().max_hp(), Some(250));
	assert_eq!(enemy.stats().attack(), Some(40));
	assert_eq!(enemy.stats().defense(), Some(0));
	assert_eq!(enemy.stats().block_count(), Some(2));
	assert_eq!(enemy.stats().magic_resistance(), Some(10.0));
	assert_eq!(enemy.stats().move_speed(), Some(1.0));
	assert_eq!(enemy.stats().attack_speed(), Some(100.0));
	assert_eq!(enemy.stats().base_attack_time(), Some(1.5));
	assert_eq!(enemy.stats().mass_level(), Some(1));
	assert_eq!(enemy.stats().life_point_reduce(), Some(0));
	assert_eq!(enemy.stats().range_radius(), Some(0.5));
	assert_eq!(enemy.stats().not_count_in_total(), Some(false));
	assert_eq!(enemy.skills().len(), 1);
	assert_eq!(enemy.skills()[0].prefab_key(), "enemy_skill_test");
	assert_eq!(enemy.skills()[0].priority(), 0);
	assert_eq!(enemy.skills()[0].cooldown_seconds(), 5.0);
	assert_eq!(enemy.skills()[0].initial_cooldown_seconds(), 0.0);
	assert_eq!(enemy.skills()[0].sp_cost(), 0);
	assert!(enemy.skills()[0].blackboard().is_empty());
	assert_eq!(enemy.talent_blackboard(), Some(&json!([])));
	assert_eq!(enemy.sp_data(), None);
}

#[test]
fn apply_way_name_reads_the_folded_name_and_ignores_numeric_codes() {
	let compiled: CompiledStage = compile_payload(&minimal_level(), &minimal_enemy_database())
		.expect("minimal enemy should compile");
	assert_eq!(enemy(&compiled, ENEMY_KEY).apply_way_name(), Some("MELEE"));

	// An undefined database patch keeps the inherited value; a defined stage override wins.
	let inherited: CompiledStage = compile_payload(
		&level_requesting_database_level_one(Value::Null),
		&database_with_level_one(),
	)
	.expect("inherited enemy should compile");
	assert_eq!(enemy(&inherited, ENEMY_KEY).apply_way_name(), Some("MELEE"));
	let overridden: CompiledStage = compile_payload(
		&level_requesting_database_level_one(json!({"applyWay": defined(json!("RANGED"))})),
		&database_with_level_one(),
	)
	.expect("overridden enemy should compile");
	assert_eq!(
		enemy(&overridden, ENEMY_KEY).apply_way_name(),
		Some("RANGED")
	);

	let mut numeric: Value = minimal_enemy_database();
	numeric["enemies"][0]["Value"][0]["enemyData"]["applyWay"] = defined(json!(1));
	let compiled: CompiledStage =
		compile_payload(&minimal_level(), &numeric).expect("numeric apply way should compile");
	assert_eq!(enemy(&compiled, ENEMY_KEY).apply_way_name(), None);
}

#[test]
fn undefined_database_block_count_remains_unset() {
	let mut database: Value = minimal_enemy_database();
	database["enemies"][0]["Value"][0]["enemyData"]["attributes"]["blockCnt"] = undefined(json!(0));

	let compiled: CompiledStage = compile_payload(&minimal_level(), &database)
		.expect("undefined block count should preserve the game-level default");

	assert_eq!(enemy(&compiled, ENEMY_KEY).stats().block_count(), None);
}

#[test]
fn defined_null_and_present_empty_skills_replace_inherited_values() {
	let database: Value = database_with_level_one();
	let level: Value = level_requesting_database_level_one(json!({
		"prefabKey": defined(Value::Null),
		"skills": [],
		"talentBlackboard": null,
		"spData": null,
	}));

	let compiled: CompiledStage = compile_payload(&level, &database)
		.expect("defined null and explicit empty array should compile");
	let enemy: &CompiledEnemy = enemy(&compiled, ENEMY_KEY);

	assert_eq!(enemy.prefab_key(), None);
	assert_eq!(enemy.stats().max_hp(), Some(250));
	assert!(enemy.skills().is_empty());
}

#[test]
fn null_database_override_is_a_no_op() {
	let database: Value = database_with_level_one();
	let level: Value = level_requesting_database_level_one(Value::Null);

	let compiled: CompiledStage =
		compile_payload(&level, &database).expect("null override should inherit database patches");
	let enemy: &CompiledEnemy = enemy(&compiled, ENEMY_KEY);

	assert_eq!(enemy.database_level(), Some(1));
	assert_eq!(enemy.prefab_key(), Some("enemy_prefab_test"));
	assert_eq!(enemy.stats().max_hp(), Some(250));
	assert_eq!(enemy.stats().attack(), Some(40));
	assert_eq!(enemy.stats().defense(), Some(5));
	assert_eq!(enemy.stats().block_count(), Some(2));
	assert_eq!(enemy.stats().life_point_reduce(), Some(2));
	assert_eq!(enemy.stats().not_count_in_total(), Some(true));
	assert_eq!(enemy.skills().len(), 1);
}

#[test]
fn use_db_false_materializes_from_empty_without_database_provenance() {
	let mut level: Value = minimal_level();
	without_actions(&mut level);
	level["enemyDbRefs"] = json!([{
		"useDb": false,
		"id": "enemy_local",
		"level": 7,
		"overwrittenData": {
			"prefabKey": defined(json!("enemy_prefab_local")),
			"attributes": {
				"maxHp": defined(json!(77)),
				"atk": undefined(json!(999)),
				"def": defined(json!(0)),
				"blockCnt": defined(json!(3)),
			},
			"lifePointReduce": defined(json!(0)),
			"notCountInTotal": defined(json!(false)),
			"skills": [],
			"talentBlackboard": [],
			"spData": null,
		},
	}]);

	let compiled: CompiledStage = compile_payload(&level, &minimal_enemy_database())
		.expect("complete local enemy should compile independently");
	let enemy: &CompiledEnemy = enemy(&compiled, "enemy_local");

	assert_eq!(enemy.database_level(), None);
	assert_eq!(enemy.prefab_key(), Some("enemy_prefab_local"));
	assert_eq!(enemy.stats().max_hp(), Some(77));
	assert_eq!(enemy.stats().attack(), None);
	assert_eq!(enemy.stats().defense(), Some(0));
	assert_eq!(enemy.stats().block_count(), Some(3));
	assert_eq!(enemy.stats().life_point_reduce(), Some(0));
	assert_eq!(enemy.stats().not_count_in_total(), Some(false));
	assert!(enemy.skills().is_empty());
}

#[test]
fn top_level_local_enemy_materializes_and_joins_direct_spawns() {
	let mut level: Value = minimal_level();
	level["enemyDbRefs"] = json!([]);
	level["enemies"] = json!([{
		"key": "enemy_top_local",
		"name": "Top-level Local Enemy",
		"description": "unwrapped fixture enemy",
		"prefabKey": "enemy_prefab_top_local",
		"attributes": {
			"maxHp": 321,
			"atk": 45,
			"def": 6,
			"blockCnt": 4,
			"magicResistance": 7.5,
			"moveSpeed": 0.8,
			"attackSpeed": 90.0,
			"baseAttackTime": 2.0,
			"massLevel": 2,
		},
		"applyWay": "RANGED",
		"motion": "WALK",
		"enemyTags": [],
		"lifePointReduce": 0,
		"levelType": "NORMAL",
		"rangeRadius": 1.25,
		"notCountInTotal": false,
		"talentBlackboard": [],
		"skills": [{
			"prefabKey": "enemy_skill_top_local",
			"priority": 1,
			"cooldown": 4.0,
			"initCooldown": 1.0,
			"spCost": 2,
			"blackboard": [{
				"key": "atk_scale",
				"value": 1.5,
				"valueStr": "reviewed"
			}],
		}],
		"spData": null,
	}]);
	level["waves"][0]["fragments"][0]["actions"][0]["key"] = json!("enemy_top_local");
	level["branches"]["alpha"]["phases"][0]["actions"][0]["key"] = json!("enemy_top_local");
	level["branches"]["zeta"]["phases"][0]["actions"][0]["key"] = json!("enemy_top_local");

	let compiled: CompiledStage = compile_payload(&level, &minimal_enemy_database())
		.expect("top-level local enemy should satisfy direct spawn joins");
	let enemy: &CompiledEnemy = enemy(&compiled, "enemy_top_local");

	assert_eq!(enemy.database_level(), None);
	assert_eq!(enemy.prefab_key(), Some("enemy_prefab_top_local"));
	assert_eq!(enemy.stats().max_hp(), Some(321));
	assert_eq!(enemy.stats().attack(), Some(45));
	assert_eq!(enemy.stats().defense(), Some(6));
	assert_eq!(enemy.stats().block_count(), Some(4));
	assert_eq!(enemy.stats().magic_resistance(), Some(7.5));
	assert_eq!(enemy.stats().life_point_reduce(), Some(0));
	assert_eq!(enemy.stats().not_count_in_total(), Some(false));
	assert_eq!(enemy.skills().len(), 1);
	assert_eq!(enemy.skills()[0].prefab_key(), "enemy_skill_top_local");
	assert_eq!(enemy.skills()[0].priority(), 1);
	assert_eq!(enemy.skills()[0].cooldown_seconds(), 4.0);
	assert_eq!(enemy.skills()[0].initial_cooldown_seconds(), 1.0);
	assert_eq!(enemy.skills()[0].sp_cost(), 2);
	assert_eq!(enemy.skills()[0].blackboard().len(), 1);
	assert_eq!(enemy.skills()[0].blackboard()[0].key(), "atk_scale");
	assert_eq!(enemy.skills()[0].blackboard()[0].value(), 1.5);
	assert_eq!(
		enemy.skills()[0].blackboard()[0].value_string(),
		Some("reviewed")
	);
	assert_eq!(compiled.spawns().len(), 3);
	assert!(compiled
		.spawns()
		.iter()
		.all(|spawn: &Spawn| spawn.enemy().as_str() == "enemy_top_local"));
}

#[test]
fn enemy_keys_reject_empty_and_stage_iteration_is_lexical() {
	assert!(EnemyKey::new(String::new()).is_err());

	let mut level: Value = minimal_level();
	without_actions(&mut level);
	level["enemyDbRefs"] = json!([]);
	level["enemies"] = json!([
		local_enemy("enemy_zeta", "prefab_zeta", 2),
		local_enemy("enemy_alpha", "prefab_alpha", 1),
	]);

	let compiled: CompiledStage = compile_payload(&level, &minimal_enemy_database())
		.expect("distinct local enemies should compile");
	assert_eq!(
		compiled
			.enemies()
			.map(|(key, _enemy): (&EnemyKey, &CompiledEnemy)| key.as_str())
			.collect::<Vec<_>>(),
		vec!["enemy_alpha", "enemy_zeta"]
	);
}

#[test]
fn stage_enemy_identity_is_unique_across_levels_and_sources() {
	let mut duplicate_ref_level: Value = minimal_level();
	without_actions(&mut duplicate_ref_level);
	let mut second_ref: Value = duplicate_ref_level["enemyDbRefs"][0].clone();
	second_ref["level"] = json!(1);
	duplicate_ref_level["enemyDbRefs"]
		.as_array_mut()
		.expect("enemy refs are an array")
		.push(second_ref);
	let duplicate_ref_error: ArkDataError =
		payload_error(&duplicate_ref_level, &database_with_level_one());
	assert!(
		matches!(
			duplicate_ref_error,
			ArkDataError::DuplicateEnemyReference {
				ref enemy_id,
				level: 1,
			} if enemy_id == ENEMY_KEY
		),
		"received {duplicate_ref_error:?}"
	);

	let mut cross_source_level: Value = minimal_level();
	without_actions(&mut cross_source_level);
	cross_source_level["enemies"] = json!([local_enemy(ENEMY_KEY, "prefab_local_collision", 1)]);
	let cross_source_error: ArkDataError =
		payload_error(&cross_source_level, &minimal_enemy_database());
	assert!(
		matches!(
			cross_source_error,
			ArkDataError::DuplicateEnemyId { ref enemy_id } if enemy_id == ENEMY_KEY
		),
		"received {cross_source_error:?}"
	);

	let mut duplicate_local_level: Value = minimal_level();
	without_actions(&mut duplicate_local_level);
	duplicate_local_level["enemyDbRefs"] = json!([]);
	duplicate_local_level["enemies"] = json!([
		local_enemy("enemy_local", "prefab_one", 1),
		local_enemy("enemy_local", "prefab_two", 2),
	]);
	let duplicate_local_error: ArkDataError =
		payload_error(&duplicate_local_level, &minimal_enemy_database());
	assert!(
		matches!(
			duplicate_local_error,
			ArkDataError::DuplicateEnemyId { ref enemy_id } if enemy_id == "enemy_local"
		),
		"received {duplicate_local_error:?}"
	);
}

#[test]
fn enemy_database_and_level_references_fail_closed_with_typed_errors() {
	type Mutation = fn(&mut Value, &mut Value);
	let cases: [(Mutation, ExpectedEnemyError); 6] = [
		(
			|level: &mut Value, _database: &mut Value| {
				without_actions(level);
				level["enemyDbRefs"][0]["id"] = json!("enemy_missing");
			},
			ExpectedEnemyError::MissingId,
		),
		(
			|level: &mut Value, _database: &mut Value| {
				without_actions(level);
				level["enemyDbRefs"][0]["level"] = json!(2);
			},
			ExpectedEnemyError::MissingLevel,
		),
		(
			|_level: &mut Value, database: &mut Value| {
				let duplicate: Value = database["enemies"][0].clone();
				database["enemies"]
					.as_array_mut()
					.expect("database enemies are an array")
					.push(duplicate);
			},
			ExpectedEnemyError::DuplicateId,
		),
		(
			|_level: &mut Value, database: &mut Value| {
				let duplicate: Value = database["enemies"][0]["Value"][0].clone();
				database["enemies"][0]["Value"]
					.as_array_mut()
					.expect("database levels are an array")
					.push(duplicate);
			},
			ExpectedEnemyError::DuplicateLevel,
		),
		(
			|level: &mut Value, _database: &mut Value| {
				without_actions(level);
				let duplicate: Value = level["enemyDbRefs"][0].clone();
				level["enemyDbRefs"]
					.as_array_mut()
					.expect("level enemy refs are an array")
					.push(duplicate);
			},
			ExpectedEnemyError::DuplicateReference,
		),
		(
			|level: &mut Value, _database: &mut Value| {
				without_actions(level);
				level["enemyDbRefs"] = json!([{
					"useDb": false,
					"id": "enemy_local",
					"level": 0,
					"overwrittenData": null,
				}]);
			},
			ExpectedEnemyError::MissingLocalData,
		),
	];

	for (mutate, expected) in cases {
		let mut level: Value = minimal_level();
		let mut database: Value = database_with_level_one();
		mutate(&mut level, &mut database);
		assert_enemy_error(payload_error(&level, &database), expected);
	}
}
