mod common;

use std::collections::BTreeSet;

use ark_solver::data::{
	ArkDataError, ArkDataSnapshot, CompiledStage, EnemyKey, GridPosition, MechanicIdentity,
	MechanicKind, MechanicOrigin, MechanicReference, PredefinedCollection, StageDifficulty,
	StageIdentity, StageRef, WaveActionContainer, WaveActionOrigin,
};
use common::{minimal_level, Fixture, ENEMY_KEY, LEVEL_PATH};
use serde_json::{json, Value};

const NORMAL_STAGE_ID: &str = "Mechanics_Normal";
const RAID_STAGE_ID: &str = "Mechanics_Normal#f#";

fn stage_table() -> Value {
	json!({
		"stages": {
			"Mechanics_Normal": {
				"stageId": NORMAL_STAGE_ID,
				"code": "MECHANICS-1",
				"levelId": "Activities/Test/Level_Test_01",
				"difficulty": "NORMAL",
				"hardStagedId": RAID_STAGE_ID,
				"diffGroup": null,
			},
			"Mechanics_Normal#f#": {
				"stageId": RAID_STAGE_ID,
				"code": "MECHANICS-1",
				"levelId": "Activities/Test/Level_Test_01",
				"difficulty": "FOUR_STAR",
				"hardStagedId": null,
				"diffGroup": null,
			},
		},
		"diffGroupTable": {},
		"runeStageGroups": {},
	})
}

fn compile_level(
	level: &Value,
	difficulty: StageDifficulty,
) -> Result<CompiledStage, ArkDataError> {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json("stage_table.json", &stage_table());
	fixture.set_verified_entry_json(LEVEL_PATH, level);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path())?;
	snapshot.compile_stage(StageRef {
		identity: StageIdentity::StageId(NORMAL_STAGE_ID.to_owned()),
		difficulty,
	})
}

fn rune(difficulty_mask: Value, key: &str) -> Value {
	json!({
		"difficultyMask": difficulty_mask,
		"key": key,
		"professionMask": "NONE",
		"buildableMask": "NONE",
		"blackboard": [],
	})
}

fn predefined_entry(character_key: &str) -> Value {
	json!({
		"position": {"row": 0, "col": 0},
		"direction": "UP",
		"hidden": false,
		"alias": "",
		"uniEquipIds": [],
		"showSpIllust": false,
		"masterInfos": [],
		"inst": {
			"characterKey": character_key,
			"level": 1,
			"phase": "PHASE_0",
			"favorPoint": 0,
			"potentialRank": 0,
		},
		"skillIndex": 0,
		"mainSkillLvl": 1,
		"skinId": "",
		"tmplId": "",
		"overrideSkillBlackboard": [],
		"overrideTalents": [],
	})
}

fn predefines(prefix: &str) -> Value {
	json!({
		"characterInsts": [predefined_entry(&format!("{prefix}_character_inst"))],
		"tokenInsts": [predefined_entry(&format!("{prefix}_token_inst"))],
		"characterCards": [predefined_entry(&format!("{prefix}_character_card"))],
		"tokenCards": [predefined_entry(&format!("{prefix}_token_card"))],
	})
}

fn mechanics_level() -> Value {
	let mut level: Value = minimal_level();
	level["mapData"]["tiles"][2]["effects"] = json!([{
		"key": "map_effect_start",
		"offset": {"x": 0.0, "y": 0.0, "z": 0.0},
		"direction": "UP",
	}]);
	level["mapData"]["effects"] = json!([{
		"key": "map_effect_global",
		"offset": {"x": 0.0, "y": 0.0, "z": 0.0},
		"direction": "UP",
	}]);
	level["runes"] = json!([
		rune(json!("NORMAL"), "rune_normal"),
		rune(json!("ALL"), "rune_all"),
		rune(json!(3), "rune_numeric_all"),
		rune(json!("FOUR_STAR"), "rune_raid"),
		rune(json!("SIX_STAR"), "rune_six_star"),
	]);
	level["optionalRunes"] = json!({
		"all-choice": [rune(json!(3), "rune_all")],
		"raid-choice": [rune(json!("FOUR_STAR"), "rune_optional_raid")],
	});
	level["globalBuffs"] = json!([{
		"prefabKey": "global_buff_fixture",
		"blackboard": [],
		"overrideCameraEffect": "",
		"passProfessionMaskFlag": false,
		"professionMask": "NONE",
		"playerSideMask": "ALL",
	}]);
	level["predefines"] = predefines("normal");
	level["hardPredefines"] = predefines("hard");
	let actions: &mut Vec<Value> = level["waves"][0]["fragments"][0]["actions"]
		.as_array_mut()
		.expect("wave actions are an array");
	actions.push(json!({
		"actionType": "PLAY_BGM",
		"managedByScheduler": false,
		"key": "bgm_fixture",
		"count": 0,
		"preDelay": 0.0,
		"interval": 0.0,
		"routeIndex": 0,
	}));
	actions.push(json!({
		"actionType": 4,
		"managedByScheduler": false,
		"key": "bgm_fixture_legacy",
		"count": 0,
		"preDelay": 0.0,
		"interval": 0.0,
		"routeIndex": 0,
	}));
	actions.push(json!({
		"actionType": "SPAWN",
		"managedByScheduler": false,
		"key": "",
		"count": 1,
		"preDelay": 0.0,
		"interval": 1.0,
		"routeIndex": 0,
		"randomSpawnGroupKey": "group_fixture",
		"randomSpawnGroupPackKey": "pack_fixture",
		"hiddenGroup": "hidden_fixture",
	}));
	actions.push(json!({
		"actionType": 99,
		"managedByScheduler": false,
		"key": "future_numeric_action",
		"count": 0,
		"preDelay": 0.0,
		"interval": 0.0,
		"routeIndex": 0,
	}));
	actions.push(json!({
		"actionType": "FUTURE_ACTION",
		"managedByScheduler": false,
		"key": "future_named_action",
		"count": 0,
		"preDelay": 0.0,
		"interval": 0.0,
		"routeIndex": 0,
	}));
	level
}

fn references(compiled: &CompiledStage) -> &[MechanicReference] {
	compiled.mechanic_requirements().references()
}

fn references_of(compiled: &CompiledStage, kind: MechanicKind) -> Vec<(String, MechanicOrigin)> {
	references(compiled)
		.iter()
		.filter(|reference: &&MechanicReference| reference.identity().kind() == kind)
		.map(|reference: &MechanicReference| {
			(
				reference.identity().id().to_owned(),
				reference.origin().clone(),
			)
		})
		.collect()
}

fn wave_origin(action: u32) -> MechanicOrigin {
	MechanicOrigin::WaveAction(WaveActionOrigin {
		container: WaveActionContainer::Wave {
			wave: 0,
			fragment: 0,
		},
		action,
	})
}

fn assert_predefined_reference(
	references: &[(String, MechanicOrigin)],
	id: &str,
	hard: bool,
	collection: PredefinedCollection,
) {
	assert!(references.contains(&(
		id.to_owned(),
		MechanicOrigin::Predefined {
			hard,
			collection,
			index: 0,
		},
	)));
}

#[test]
fn normal_and_raid_filter_regular_and_optional_runes_by_mask() {
	let level: Value = mechanics_level();
	let normal: CompiledStage =
		compile_level(&level, StageDifficulty::Normal).expect("normal mechanics should compile");
	let raid: CompiledStage =
		compile_level(&level, StageDifficulty::Raid).expect("raid mechanics should compile");

	assert_eq!(
		references_of(&normal, MechanicKind::Rune),
		vec![
			(
				"rune_all".to_owned(),
				MechanicOrigin::Rune {
					optional_group: None,
					index: 1,
				},
			),
			(
				"rune_all".to_owned(),
				MechanicOrigin::Rune {
					optional_group: Some("all-choice".to_owned()),
					index: 0,
				},
			),
			(
				"rune_normal".to_owned(),
				MechanicOrigin::Rune {
					optional_group: None,
					index: 0,
				},
			),
			(
				"rune_numeric_all".to_owned(),
				MechanicOrigin::Rune {
					optional_group: None,
					index: 2,
				},
			),
		]
	);
	assert_eq!(
		references_of(&raid, MechanicKind::Rune),
		vec![
			(
				"rune_all".to_owned(),
				MechanicOrigin::Rune {
					optional_group: None,
					index: 1,
				},
			),
			(
				"rune_all".to_owned(),
				MechanicOrigin::Rune {
					optional_group: Some("all-choice".to_owned()),
					index: 0,
				},
			),
			(
				"rune_numeric_all".to_owned(),
				MechanicOrigin::Rune {
					optional_group: None,
					index: 2,
				},
			),
			(
				"rune_optional_raid".to_owned(),
				MechanicOrigin::Rune {
					optional_group: Some("raid-choice".to_owned()),
					index: 0,
				},
			),
			(
				"rune_raid".to_owned(),
				MechanicOrigin::Rune {
					optional_group: None,
					index: 3,
				},
			),
		]
	);
}

#[test]
fn unknown_string_rune_mask_fails_with_typed_context() {
	for invalid_mask in [json!("FUTURE_DIFFICULTY"), json!(-1)] {
		let mut level: Value = minimal_level();
		level["runes"] = json!([rune(invalid_mask, "rune_unknown")]);
		let error: ArkDataError = compile_level(&level, StageDifficulty::Normal)
			.expect_err("invalid rune mask must fail closed");
		assert!(
			matches!(
				error,
				ArkDataError::InvalidRuneDifficultyMask {
					ref mask,
					ref reason,
					..
				} if !mask.is_empty() && !reason.is_empty()
			),
			"received {error:?}"
		);
	}
}

#[test]
fn mechanic_identity_rejects_empty_ids() {
	assert!(MechanicIdentity::new(MechanicKind::Rune, "").is_err());
}

#[test]
fn nullable_level_collections_are_accepted_independently() {
	let fields: [&str; 10] = [
		"runes",
		"optionalRunes",
		"globalBuffs",
		"routes",
		"extraRoutes",
		"enemyDbRefs",
		"waves",
		"branches",
		"predefines",
		"hardPredefines",
	];

	for field in fields {
		let mut level: Value = mechanics_level();
		if matches!(field, "routes" | "enemyDbRefs") {
			level["waves"] = json!([]);
			level["branches"] = json!({});
		}
		level[field] = Value::Null;
		let compiled: CompiledStage = compile_level(&level, StageDifficulty::Normal)
			.unwrap_or_else(|error| panic!("null {field} should compile independently: {error:?}"));
		match field {
			"runes" => assert_eq!(
				references_of(&compiled, MechanicKind::Rune),
				vec![(
					"rune_all".to_owned(),
					MechanicOrigin::Rune {
						optional_group: Some("all-choice".to_owned()),
						index: 0,
					},
				)]
			),
			"optionalRunes" => assert_eq!(
				references_of(&compiled, MechanicKind::Rune),
				vec![
					(
						"rune_all".to_owned(),
						MechanicOrigin::Rune {
							optional_group: None,
							index: 1,
						},
					),
					(
						"rune_normal".to_owned(),
						MechanicOrigin::Rune {
							optional_group: None,
							index: 0,
						},
					),
					(
						"rune_numeric_all".to_owned(),
						MechanicOrigin::Rune {
							optional_group: None,
							index: 2,
						},
					),
				]
			),
			"globalBuffs" => {
				assert!(references_of(&compiled, MechanicKind::GlobalBuffPrefab).is_empty());
			}
			"routes" => assert_eq!(compiled.extra_routes().len(), 1),
			"extraRoutes" => assert_eq!(compiled.routes().len(), 1),
			"waves" => assert_eq!(compiled.spawns().len(), 2),
			"branches" => assert_eq!(compiled.spawns().len(), 1),
			"predefines" => {
				let remaining: Vec<(String, MechanicOrigin)> =
					references_of(&compiled, MechanicKind::Predefined);
				assert_eq!(remaining.len(), 2);
				assert!(remaining
					.iter()
					.all(|(_id, origin): &(String, MechanicOrigin)| matches!(
						origin,
						MechanicOrigin::Predefined { hard: true, .. }
					)));
			}
			"hardPredefines" => {
				let remaining: Vec<(String, MechanicOrigin)> =
					references_of(&compiled, MechanicKind::Predefined);
				assert_eq!(remaining.len(), 2);
				assert!(remaining
					.iter()
					.all(|(_id, origin): &(String, MechanicOrigin)| matches!(
						origin,
						MechanicOrigin::Predefined { hard: false, .. }
					)));
			}
			"enemyDbRefs" => {}
			_ => unreachable!("all nullable fields are covered"),
		}
	}
}

#[test]
fn mechanic_references_are_sorted_and_retain_exact_origins() {
	let compiled: CompiledStage = compile_level(&mechanics_level(), StageDifficulty::Normal)
		.expect("complete mechanic fixture should compile");
	let all: &[MechanicReference] = references(&compiled);
	assert!(all
		.windows(2)
		.all(|pair: &[MechanicReference]| pair[0] < pair[1]));

	assert_eq!(
		references_of(&compiled, MechanicKind::Tile),
		vec![
			(
				"tile_floor_0".to_owned(),
				MechanicOrigin::MapCell {
					position: GridPosition { row: 0, col: 0 },
				},
			),
			(
				"tile_floor_1".to_owned(),
				MechanicOrigin::MapCell {
					position: GridPosition { row: 0, col: 1 },
				},
			),
			(
				"tile_floor_4".to_owned(),
				MechanicOrigin::MapCell {
					position: GridPosition { row: 1, col: 1 },
				},
			),
			(
				"tile_floor_5".to_owned(),
				MechanicOrigin::MapCell {
					position: GridPosition { row: 1, col: 2 },
				},
			),
			(
				"tile_goal".to_owned(),
				MechanicOrigin::MapCell {
					position: GridPosition { row: 1, col: 0 },
				},
			),
			(
				"tile_start".to_owned(),
				MechanicOrigin::MapCell {
					position: GridPosition { row: 0, col: 2 },
				},
			),
		]
	);
	assert_eq!(
		references_of(&compiled, MechanicKind::MapEffect),
		vec![
			(
				"map_effect_global".to_owned(),
				MechanicOrigin::MapEffect {
					position: None,
					index: 0,
				},
			),
			(
				"map_effect_start".to_owned(),
				MechanicOrigin::MapEffect {
					position: Some(GridPosition { row: 0, col: 2 }),
					index: 0,
				},
			),
		]
	);
	let checkpoints: Vec<(String, MechanicOrigin)> =
		references_of(&compiled, MechanicKind::RouteCheckpoint);
	assert!(checkpoints.contains(&(
		"WAIT_FOR_SECONDS".to_owned(),
		MechanicOrigin::RouteCheckpoint {
			extra: false,
			route: 0,
			checkpoint: 1,
		},
	)));
	assert!(checkpoints.contains(&(
		"WAIT_FOR_SECONDS".to_owned(),
		MechanicOrigin::RouteCheckpoint {
			extra: true,
			route: 0,
			checkpoint: 0,
		},
	)));
	assert!(checkpoints.contains(&(
		"DISAPPEAR".to_owned(),
		MechanicOrigin::RouteCheckpoint {
			extra: false,
			route: 0,
			checkpoint: 2,
		},
	)));
	for (id, checkpoint) in [
		("APPEAR_AT_POS", 3),
		("PATROL_MOVE", 4),
		("MAP_OFFSET_MOVE", 5),
	] {
		assert!(checkpoints.contains(&(
			id.to_owned(),
			MechanicOrigin::RouteCheckpoint {
				extra: false,
				route: 0,
				checkpoint,
			},
		)));
	}
	assert_eq!(
		references_of(&compiled, MechanicKind::GlobalBuffPrefab),
		vec![(
			"global_buff_fixture".to_owned(),
			MechanicOrigin::GlobalBuff { index: 0 },
		)]
	);
	let enemy_key: EnemyKey = EnemyKey::new(ENEMY_KEY.to_owned()).expect("fixture enemy key");
	assert_eq!(
		references_of(&compiled, MechanicKind::EnemyPrefab),
		vec![(
			"enemy_prefab_test".to_owned(),
			MechanicOrigin::Enemy {
				enemy: enemy_key.clone(),
			},
		)]
	);
	assert_eq!(
		references_of(&compiled, MechanicKind::EnemySkillPrefab),
		vec![(
			"enemy_skill_test".to_owned(),
			MechanicOrigin::EnemySkill {
				enemy: enemy_key,
				index: 0,
			},
		)]
	);

	let predefined: Vec<(String, MechanicOrigin)> =
		references_of(&compiled, MechanicKind::Predefined);
	assert_predefined_reference(
		&predefined,
		"hard_character_card",
		true,
		PredefinedCollection::CharacterCards,
	);
	assert_predefined_reference(
		&predefined,
		"hard_character_inst",
		true,
		PredefinedCollection::CharacterInsts,
	);
	assert_predefined_reference(
		&predefined,
		"normal_character_card",
		false,
		PredefinedCollection::CharacterCards,
	);
	assert_predefined_reference(
		&predefined,
		"normal_character_inst",
		false,
		PredefinedCollection::CharacterInsts,
	);
	let tokens: Vec<(String, MechanicOrigin)> = references_of(&compiled, MechanicKind::Token);
	assert_predefined_reference(
		&tokens,
		"hard_token_card",
		true,
		PredefinedCollection::TokenCards,
	);
	assert_predefined_reference(
		&tokens,
		"hard_token_inst",
		true,
		PredefinedCollection::TokenInsts,
	);
	assert_predefined_reference(
		&tokens,
		"normal_token_card",
		false,
		PredefinedCollection::TokenCards,
	);
	assert_predefined_reference(
		&tokens,
		"normal_token_inst",
		false,
		PredefinedCollection::TokenInsts,
	);

	let wave_actions: Vec<(String, MechanicOrigin)> =
		references_of(&compiled, MechanicKind::WaveAction);
	assert!(wave_actions.contains(&("PLAY_BGM".to_owned(), wave_origin(1))));
	assert!(wave_actions.contains(&("PLAY_BGM".to_owned(), wave_origin(2))));
	assert!(wave_actions.contains(&("numeric:99".to_owned(), wave_origin(4))));
	assert!(wave_actions.contains(&("name:FUTURE_ACTION".to_owned(), wave_origin(5))));
	for id in [
		"SPAWN_RANDOM_GROUP:group_fixture",
		"SPAWN_RANDOM_PACK:pack_fixture",
		"SPAWN_HIDDEN_GROUP:hidden_fixture",
	] {
		assert!(wave_actions.contains(&(id.to_owned(), wave_origin(3))));
	}
}

#[test]
fn every_known_numeric_action_maps_to_its_canonical_mechanic_id() {
	let mut level: Value = minimal_level();
	let actions: &mut Vec<Value> = level["waves"][0]["fragments"][0]["actions"]
		.as_array_mut()
		.expect("wave actions are an array");
	let expected: [(i32, &str); 14] = [
		(1, "PREVIEW_CURSOR"),
		(2, "STORY"),
		(3, "TUTORIAL"),
		(4, "PLAY_BGM"),
		(5, "DISPLAY_ENEMY_INFO"),
		(6, "ACTIVATE_PREDEFINED"),
		(7, "PLAY_OPERA"),
		(8, "TRIGGER_PREDEFINED"),
		(9, "BATTLE_EVENTS"),
		(10, "WITHDRAW_PREDEFINED"),
		(11, "DIALOG"),
		(12, "SHOW_ALL_HIDDEN_CARDS"),
		(13, "EMPTY"),
		(14, "E_NUM"),
	];
	for (code, id) in expected {
		actions.push(json!({
			"actionType": code,
			"managedByScheduler": false,
			"key": id,
			"count": 0,
			"preDelay": 0.0,
			"interval": 0.0,
			"routeIndex": 0,
		}));
	}

	let compiled: CompiledStage = compile_level(&level, StageDifficulty::Normal)
		.expect("known numeric actions should compile");
	let wave_actions: Vec<(String, MechanicOrigin)> =
		references_of(&compiled, MechanicKind::WaveAction);
	for (offset, (_code, id)) in expected.into_iter().enumerate() {
		assert!(wave_actions.contains(&(
			id.to_owned(),
			wave_origin(u32::try_from(offset + 1).expect("small fixture index")),
		)));
	}
}

#[test]
fn unknown_checkpoint_is_retained_as_a_requirement() {
	let mut level: Value = minimal_level();
	level["routes"][0]["checkpoints"][1]["type"] = json!("FUTURE_WAIT");

	let compiled: CompiledStage = compile_level(&level, StageDifficulty::Normal)
		.expect("unknown checkpoint should be inventoried");
	assert!(
		references_of(&compiled, MechanicKind::RouteCheckpoint).contains(&(
			"name:FUTURE_WAIT".to_owned(),
			MechanicOrigin::RouteCheckpoint {
				extra: false,
				route: 0,
				checkpoint: 1,
			},
		))
	);
}

#[test]
fn support_report_is_a_pure_identity_membership_filter() {
	let compiled: CompiledStage = compile_level(&mechanics_level(), StageDifficulty::Normal)
		.expect("complete mechanic fixture should compile");
	let requirements = compiled.mechanic_requirements();
	let play_bgm: MechanicIdentity = MechanicIdentity::new(MechanicKind::WaveAction, "PLAY_BGM")
		.expect("nonempty mechanic identity");
	let global_buff: MechanicIdentity =
		MechanicIdentity::new(MechanicKind::GlobalBuffPrefab, "global_buff_fixture")
			.expect("nonempty mechanic identity");
	let supported: BTreeSet<MechanicIdentity> =
		BTreeSet::from([play_bgm.clone(), global_buff.clone()]);

	let report = requirements.support_report(&supported);
	assert!(!report.is_complete());
	let expected_unsupported: Vec<MechanicReference> = requirements
		.references()
		.iter()
		.filter(|reference: &&MechanicReference| !supported.contains(reference.identity()))
		.cloned()
		.collect();
	assert_eq!(report.unsupported(), expected_unsupported);
	assert_eq!(
		report.unsupported().len(),
		requirements.references().len() - 3
	);
	assert!(report
		.unsupported()
		.iter()
		.all(
			|reference: &MechanicReference| reference.identity() != &play_bgm
				&& reference.identity() != &global_buff
		));
	let unsupported_rune_all: Vec<&MechanicReference> = report
		.unsupported()
		.iter()
		.filter(|reference: &&MechanicReference| {
			reference.identity().kind() == MechanicKind::Rune
				&& reference.identity().id() == "rune_all"
		})
		.collect();
	assert_eq!(unsupported_rune_all.len(), 2);

	let every_identity: BTreeSet<MechanicIdentity> = requirements
		.references()
		.iter()
		.map(|reference: &MechanicReference| reference.identity().clone())
		.collect();
	let complete = requirements.support_report(&every_identity);
	assert!(complete.is_complete());
	assert!(complete.unsupported().is_empty());
}

#[test]
fn empty_key_random_spawn_requires_nonempty_group_metadata() {
	let valid: CompiledStage = compile_level(&mechanics_level(), StageDifficulty::Normal)
		.expect("three random-spawn identifiers should compile as requirements");
	assert_eq!(valid.spawns().len(), 3);

	let mut invalid_level: Value = mechanics_level();
	let random_action: &mut Value = &mut invalid_level["waves"][0]["fragments"][0]["actions"][3];
	random_action["randomSpawnGroupKey"] = json!("");
	random_action["randomSpawnGroupPackKey"] = json!("");
	random_action["hiddenGroup"] = json!("");
	let error: ArkDataError = compile_level(&invalid_level, StageDifficulty::Normal)
		.expect_err("empty-key spawn without group metadata must fail");
	assert!(
		matches!(
			error,
			ArkDataError::InvalidRandomSpawn {
				ref origin,
				ref reason,
				..
			} if origin == &WaveActionOrigin {
				container: WaveActionContainer::Wave { wave: 0, fragment: 0 },
				action: 3,
			} && !reason.is_empty()
		),
		"received {error:?}"
	);
}

#[test]
fn random_spawn_route_index_uses_primary_routes_only() {
	let mut level: Value = mechanics_level();
	level["waves"][0]["fragments"][0]["actions"][3]["routeIndex"] = json!(1);

	let error: ArkDataError = compile_level(&level, StageDifficulty::Normal)
		.expect_err("random spawn cannot address an extra route");
	assert!(
		matches!(
			error,
			ArkDataError::InvalidSpawnRouteIndex {
				route_index: 1,
				route_count: 1,
				ref origin,
				..
			} if origin == &WaveActionOrigin {
				container: WaveActionContainer::Wave { wave: 0, fragment: 0 },
				action: 3,
			}
		),
		"received {error:?}"
	);
}
