mod common;

use ark_solver::data::{
	ArkDataError, ArkDataSnapshot, Buildable, CompiledStage, FunctionDisableMask, GridPosition,
	MapCell, MapDimensions, MapDirection, MotionMode, Passability, RouteCheckpointKind, Spawn,
	StageActorCollection, StageDifficulty, StageIdentity, StageOptions, StageRef, TileHeight,
	WaveActionContainer, WaveActionOrigin,
};
use common::{minimal_level, Fixture, ENEMY_KEY, LEVEL_PATH};
use serde_json::{json, Value};

const STAGE_ID: &str = "Payload_Normal";

#[derive(Clone, Copy, Debug)]
enum ExpectedPayloadError {
	InvalidMap,
	InvalidTilePaletteIndex,
	InvalidRoutePosition,
	InvalidRouteCheckpointPosition,
	InvalidSpawnRouteIndex(i64),
	InvalidActionField(&'static str),
	SpawnEnemyNotCompiled,
	LevelParse,
}

fn stage_table() -> Value {
	json!({
		"stages": {
			"Payload_Normal": {
				"stageId": STAGE_ID,
				"code": "PAYLOAD-1",
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

fn stage_ref() -> StageRef {
	StageRef {
		identity: StageIdentity::StageId(STAGE_ID.to_owned()),
		difficulty: StageDifficulty::Normal,
	}
}

fn compile_level(level: &Value) -> Result<CompiledStage, ArkDataError> {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json("stage_table.json", &stage_table());
	fixture.set_verified_entry_json(LEVEL_PATH, level);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path())?;
	snapshot.compile_stage(stage_ref())
}

fn compile_error(level: &Value) -> ArkDataError {
	match compile_level(level) {
		Ok(_) => panic!("invalid typed payload unexpectedly compiled"),
		Err(error) => error,
	}
}

fn assert_payload_error(error: ArkDataError, expected: ExpectedPayloadError) {
	let matches_expected: bool = match expected {
		ExpectedPayloadError::InvalidMap => {
			matches!(error, ArkDataError::InvalidMap { ref reason, .. } if !reason.is_empty())
		}
		ExpectedPayloadError::InvalidTilePaletteIndex => matches!(
			error,
			ArkDataError::InvalidTilePaletteIndex {
				tile_index: 6,
				tile_count: 6,
				..
			}
		),
		ExpectedPayloadError::InvalidRoutePosition => matches!(
			error,
			ArkDataError::InvalidRoutePosition {
				extra: false,
				route: 0,
				..
			}
		),
		ExpectedPayloadError::InvalidRouteCheckpointPosition => matches!(
			error,
			ArkDataError::InvalidRouteCheckpointPosition {
				extra: false,
				route: 0,
				checkpoint: 0,
				..
			}
		),
		ExpectedPayloadError::InvalidSpawnRouteIndex(expected_index) => matches!(
			error,
			ArkDataError::InvalidSpawnRouteIndex {
				route_index,
				route_count: 1,
				ref origin,
				..
			} if route_index == expected_index
				&& origin == &WaveActionOrigin {
					container: WaveActionContainer::Wave { wave: 0, fragment: 0 },
					action: 0,
				}
		),
		ExpectedPayloadError::InvalidActionField(expected_field) => matches!(
			error,
			ArkDataError::InvalidActionField {
				field,
				ref reason,
				..
			} if field == expected_field && !reason.is_empty()
		),
		ExpectedPayloadError::SpawnEnemyNotCompiled => matches!(
			error,
			ArkDataError::SpawnEnemyNotCompiled {
				ref key,
				ref origin,
				..
			} if key == "enemy_missing"
				&& origin == &WaveActionOrigin {
					container: WaveActionContainer::Wave { wave: 0, fragment: 0 },
					action: 0,
				}
		),
		ExpectedPayloadError::LevelParse => matches!(error, ArkDataError::LevelParse { .. }),
	};
	assert!(
		matches_expected,
		"expected {expected:?}, received {error:?}"
	);
}

#[test]
fn bd_7_like_options_preserve_dp_and_roster_rules() {
	let mut level: Value = minimal_level();
	level["options"] = json!({
		"initialCost": 10,
		"maxCost": 99,
		"costIncreaseTime": 1.0,
		"moveMultiplier": 0.5,
		"characterLimit": 8,
		"functionDisableMask": "NONE",
	});

	let compiled: CompiledStage = compile_level(&level).expect("BD-7-like options should compile");
	let options: &StageOptions = compiled.options();
	assert_eq!(options.initial_cost(), 10);
	assert_eq!(options.max_cost(), 99);
	assert_eq!(options.cost_increase_time(), 1.0);
	assert_eq!(options.move_multiplier(), 0.5);
	assert_eq!(options.character_limit(), 8);
	assert_eq!(options.function_disable_mask(), FunctionDisableMask::NONE);
	assert!(options.function_disable_mask().is_empty());
}

#[test]
fn predeployed_stage_actors_preserve_position_and_exact_build() {
	let mut level: Value = minimal_level();
	level["predefines"] = json!({
		"characterInsts": [],
		"tokenInsts": [{
			"position": {"row": 0, "col": 1},
			"direction": "DOWN",
			"hidden": false,
			"alias": "shelter-1",
			"inst": {
				"characterKey": "trap_shelter",
				"level": 30,
				"phase": "PHASE_0",
				"potentialRank": 0
			},
			"skillIndex": -1,
			"mainSkillLvl": 1
		}]
	});

	let compiled: CompiledStage = compile_level(&level).expect("predeployed actor should compile");
	let actors = compiled.predeployed_actors();
	assert_eq!(actors.len(), 1);
	assert!(!actors[0].hard());
	assert_eq!(actors[0].collection(), StageActorCollection::Token);
	assert_eq!(actors[0].character_key(), "trap_shelter");
	assert_eq!(actors[0].position(), GridPosition { row: 0, col: 1 });
	assert_eq!(actors[0].direction(), &MapDirection::Down);
	assert_eq!(actors[0].alias(), Some("shelter-1"));
	assert_eq!(actors[0].elite(), 0);
	assert_eq!(actors[0].level(), 30);
	assert_eq!(actors[0].potential_rank(), 1);
	assert_eq!(actors[0].selected_skill(), None);
	assert_eq!(actors[0].skill_level(), 1);
}

#[test]
fn difficulty_runes_select_active_hidden_spawn_groups() {
	let mut level: Value = minimal_level();
	level["runes"] = json!([{
		"difficultyMask": "NORMAL",
		"key": "level_hidden_group_enable",
		"professionMask": 1023,
		"buildableMask": "ALL",
		"blackboard": [{
			"key": "key",
			"value": 0.0,
			"valueStr": "normal"
		}]
	}]);
	level["waves"][0]["fragments"][0]["actions"][0]["hiddenGroup"] = json!("normal");
	level["branches"]["alpha"]["phases"][0]["actions"][0]["hiddenGroup"] = json!("raid");

	let compiled: CompiledStage = compile_level(&level).expect("hidden-group rune should compile");
	assert_eq!(
		compiled.active_hidden_groups(),
		&["normal".to_owned()].into_iter().collect()
	);
	assert!(compiled.action_is_active(&compiled.waves()[0].fragments()[0].actions()[0]));
	assert!(!compiled.action_is_active(&compiled.branches()["alpha"].phases()[0].actions()[0]));
}

#[test]
fn numeric_function_disable_masks_preserve_combined_bits() {
	let mut level: Value = minimal_level();
	level["options"]["functionDisableMask"] = json!(6);

	let compiled: CompiledStage =
		compile_level(&level).expect("defined combined function-mask bits should compile");
	let mask: FunctionDisableMask = compiled.options().function_disable_mask();
	assert_eq!(mask.bits(), 6);
	assert!(mask.contains(FunctionDisableMask::CHARACTER_MENU));
	assert!(mask.contains(FunctionDisableMask::CHARACTER_INFO));
	assert!(!mask.contains(FunctionDisableMask::CARD_LIST));
}

#[test]
fn option_numeric_values_fail_closed_without_lossy_coercion() {
	let cases: [(&str, Value); 12] = [
		("initialCost", json!(-1)),
		("initialCost", json!(2_147_483_648_i64)),
		("maxCost", json!(-1)),
		("maxCost", json!(1.5)),
		("costIncreaseTime", json!(0.0)),
		("costIncreaseTime", json!(-0.25)),
		("costIncreaseTime", json!(1.0e100_f64)),
		("costIncreaseTime", json!(1.0e-100_f64)),
		("characterLimit", json!(-1)),
		("functionDisableMask", json!(-1)),
		("functionDisableMask", json!(262_144)),
		("functionDisableMask", json!(1.5)),
	];

	for (field, value) in cases {
		let mut level: Value = minimal_level();
		level["options"][field] = value;
		assert_payload_error(compile_error(&level), ExpectedPayloadError::LevelParse);
	}
}

#[test]
fn required_option_fields_and_unknown_mask_names_fail_closed() {
	for field in [
		"initialCost",
		"maxCost",
		"costIncreaseTime",
		"characterLimit",
		"functionDisableMask",
	] {
		let mut level: Value = minimal_level();
		level["options"]
			.as_object_mut()
			.expect("options should be an object")
			.remove(field);
		assert_payload_error(compile_error(&level), ExpectedPayloadError::LevelParse);
	}

	let mut level: Value = minimal_level();
	level["options"]["functionDisableMask"] = json!("FUTURE_DISABLE");
	assert_payload_error(compile_error(&level), ExpectedPayloadError::LevelParse);
}

#[test]
fn map_is_bottom_origin_row_major_and_cell_lookup_agrees() {
	let compiled: CompiledStage =
		compile_level(&minimal_level()).expect("complete synthetic level should compile");

	assert_eq!(
		compiled.map_dimensions(),
		MapDimensions {
			width: 3,
			height: 2,
		}
	);
	let cells: &[MapCell] = compiled.cells();
	assert_eq!(cells.len(), 6);
	assert_eq!(
		cells
			.iter()
			.map(|cell: &MapCell| (cell.position(), cell.tile_key()))
			.collect::<Vec<_>>(),
		vec![
			(GridPosition { row: 0, col: 0 }, "tile_floor_0"),
			(GridPosition { row: 0, col: 1 }, "tile_floor_1"),
			(GridPosition { row: 0, col: 2 }, "tile_start"),
			(GridPosition { row: 1, col: 0 }, "tile_goal"),
			(GridPosition { row: 1, col: 1 }, "tile_floor_4"),
			(GridPosition { row: 1, col: 2 }, "tile_floor_5"),
		]
	);
	for cell in cells {
		assert!(std::ptr::eq(
			compiled
				.cell(cell.position())
				.expect("every canonical cell position should resolve"),
			cell,
		));
	}
	assert!(compiled.cell(GridPosition { row: 2, col: 0 }).is_none());
	assert_eq!(cells[0].height(), &TileHeight::Lowland);
	assert_eq!(cells[0].buildable(), &Buildable::All);
	assert_eq!(cells[0].passability(), &Passability::All);
	assert_eq!(cells[3].height(), &TileHeight::Highland);
	assert_eq!(cells[3].buildable(), &Buildable::None);
	assert_eq!(cells[3].passability(), &Passability::None);
}

#[test]
fn map_matrix_indices_select_palette_entries_with_repetition() {
	let mut level: Value = minimal_level();
	level["mapData"]["map"] = json!([[3, 4, 3], [2, 1, 2]]);

	let compiled: CompiledStage =
		compile_level(&level).expect("repeated and unused palette entries should compile");
	assert_eq!(
		compiled
			.cells()
			.iter()
			.map(MapCell::tile_key)
			.collect::<Vec<_>>(),
		vec![
			"tile_start",
			"tile_floor_1",
			"tile_start",
			"tile_goal",
			"tile_floor_4",
			"tile_goal",
		]
	);
}

#[test]
fn primary_and_extra_routes_preserve_numeric_enums_and_sentinel_positions() {
	let compiled: CompiledStage =
		compile_level(&minimal_level()).expect("complete synthetic level should compile");

	assert_eq!(compiled.block_edges().len(), 1);
	assert_eq!(
		compiled.block_edges()[0].position(),
		GridPosition { row: 0, col: 1 }
	);
	assert_eq!(compiled.block_edges()[0].direction().stable_id(), "RIGHT");
	assert_eq!(
		compiled.block_edges()[0].block_mask(),
		&Passability::WalkOnly
	);

	assert_eq!(compiled.routes().len(), 1);
	assert_eq!(compiled.extra_routes().len(), 1);
	let primary = &compiled.routes()[0];
	assert_eq!(primary.motion(), &MotionMode::Walk);
	assert_eq!(primary.start(), GridPosition { row: 0, col: 2 });
	assert_eq!(primary.end(), GridPosition { row: 1, col: 0 });
	assert_eq!(primary.spawn_random_range().x, 0.25);
	assert_eq!(primary.spawn_random_range().y, 0.5);
	assert_eq!(primary.spawn_offset().x, -0.5);
	assert_eq!(primary.spawn_offset().y, 0.75);
	assert!(primary.allow_diagonal_move());
	assert!(primary.visit_every_tile_center());
	assert!(!primary.visit_every_node_center());
	assert!(primary.visit_every_checkpoint());
	assert_eq!(primary.checkpoints().len(), 6);
	assert_eq!(primary.checkpoints()[0].kind(), &RouteCheckpointKind::Move);
	assert_eq!(primary.checkpoints()[0].kind().mechanic_id(), "MOVE");
	assert_eq!(
		primary.checkpoints()[0].position(),
		Some(GridPosition { row: 0, col: 1 })
	);
	assert_eq!(primary.checkpoints()[0].reach_offset().x, 0.125);
	assert_eq!(primary.checkpoints()[0].reach_offset().y, -0.25);
	assert!(primary.checkpoints()[0].randomize_reach_offset());
	assert_eq!(primary.checkpoints()[0].reach_distance(), 0.375);
	assert_eq!(
		primary.checkpoints()[1].kind(),
		&RouteCheckpointKind::WaitForSeconds
	);
	assert_eq!(primary.checkpoints()[1].position(), None);
	assert_eq!(primary.checkpoints()[1].delay_seconds(), 1.5);
	assert_eq!(
		primary.checkpoints()[2].kind(),
		&RouteCheckpointKind::Disappear
	);
	assert_eq!(primary.checkpoints()[2].position(), None);
	for (checkpoint, expected_kind, expected_position) in [
		(
			3,
			RouteCheckpointKind::AppearAtPosition,
			GridPosition { row: 0, col: 0 },
		),
		(
			4,
			RouteCheckpointKind::PatrolMove,
			GridPosition { row: 1, col: 1 },
		),
		(
			5,
			RouteCheckpointKind::MapOffsetMove,
			GridPosition { row: 1, col: 2 },
		),
	] {
		assert_eq!(primary.checkpoints()[checkpoint].kind(), &expected_kind);
		assert_eq!(
			primary.checkpoints()[checkpoint].position(),
			Some(expected_position)
		);
	}

	let extra = &compiled.extra_routes()[0];
	assert_eq!(extra.motion(), &MotionMode::Fly);
	assert_eq!(extra.start(), GridPosition { row: 1, col: 2 });
	assert_eq!(extra.end(), GridPosition { row: 0, col: 0 });
	assert_eq!(
		extra.checkpoints()[0].kind(),
		&RouteCheckpointKind::WaitForSeconds
	);
	assert_eq!(extra.checkpoints()[0].position(), None);
	assert_eq!(extra.spawn_random_range().x, 0.0);
	assert_eq!(extra.spawn_random_range().y, 0.0);
	assert_eq!(extra.spawn_offset().x, 0.0);
	assert_eq!(extra.spawn_offset().y, 0.0);
	assert!(!extra.allow_diagonal_move());
	assert!(!extra.visit_every_tile_center());
	assert!(!extra.visit_every_node_center());
	assert!(!extra.visit_every_checkpoint());
}

#[test]
fn direct_spawns_preserve_wave_then_lexical_branch_source_order() {
	let compiled: CompiledStage =
		compile_level(&minimal_level()).expect("complete synthetic level should compile");
	let spawns: &[Spawn] = compiled.spawns();

	assert_eq!(spawns.len(), 3);
	assert_eq!(
		spawns
			.iter()
			.map(|spawn: &Spawn| spawn.origin().clone())
			.collect::<Vec<_>>(),
		vec![
			WaveActionOrigin {
				container: WaveActionContainer::Wave {
					wave: 0,
					fragment: 0,
				},
				action: 0,
			},
			WaveActionOrigin {
				container: WaveActionContainer::Branch {
					branch: "alpha".to_owned(),
					phase: 0,
				},
				action: 0,
			},
			WaveActionOrigin {
				container: WaveActionContainer::Branch {
					branch: "zeta".to_owned(),
					phase: 0,
				},
				action: 0,
			},
		]
	);
	assert_eq!(
		spawns
			.iter()
			.map(|spawn: &Spawn| spawn.enemy().as_str())
			.collect::<Vec<_>>(),
		vec![ENEMY_KEY, ENEMY_KEY, ENEMY_KEY]
	);
	assert_eq!(
		spawns
			.iter()
			.map(|spawn: &Spawn| (
				spawn.route_index(),
				spawn.count(),
				spawn.action_pre_delay_seconds(),
				spawn.interval_seconds(),
			))
			.collect::<Vec<_>>(),
		vec![(0, 2, 0.5, 1.25), (0, 1, 2.0, 0.5), (0, 3, 3.0, 0.75)]
	);
}

#[test]
fn schedule_tree_preserves_delays_flags_and_typed_spawn_actions() {
	let compiled: CompiledStage =
		compile_level(&minimal_level()).expect("complete synthetic level should compile");

	assert_eq!(compiled.waves().len(), 1);
	let wave = &compiled.waves()[0];
	assert_eq!(wave.pre_delay_seconds(), 1.5);
	assert_eq!(wave.post_delay_seconds(), 2.5);
	assert_eq!(wave.max_time_waiting_for_next_wave_seconds(), 9.0);
	assert_eq!(wave.advanced_wave_tag(), Some("synthetic-advance"));
	assert_eq!(wave.fragments().len(), 1);

	let fragment = &wave.fragments()[0];
	assert_eq!(fragment.pre_delay_seconds(), 0.75);
	assert_eq!(fragment.actions().len(), 1);
	let action = &fragment.actions()[0];
	assert_eq!(action.kind().stable_id(), "SPAWN");
	assert!(action.kind().is_spawn());
	assert!(action.managed_by_scheduler());
	assert!(action.block_fragment());
	assert!(action.auto_preview_route());
	assert!(action.auto_display_enemy_info());
	assert!(action.is_unharmful_and_always_count_as_killed());
	assert!(action.dont_block_wave());
	assert!(action.force_block_wave_in_branch());
	assert_eq!(action.random_type().stable_id(), "PER_DAY");
	assert_eq!(action.refresh_type().stable_id(), "PER_SEASON");
	assert_eq!(action.weight(), 7);
	assert_eq!(
		action.origin(),
		&WaveActionOrigin {
			container: WaveActionContainer::Wave {
				wave: 0,
				fragment: 0,
			},
			action: 0,
		}
	);
	let spawn = action
		.spawn()
		.expect("SPAWN action should retain typed scheduling fields");
	assert_eq!(spawn.route_index(), 0);
	assert_eq!(spawn.count(), 2);
	assert_eq!(spawn.pre_delay_seconds(), 0.5);
	assert_eq!(spawn.interval_seconds(), 1.25);
	assert_eq!(compiled.spawns()[0].action(), spawn);

	assert_eq!(
		compiled
			.branches()
			.keys()
			.map(String::as_str)
			.collect::<Vec<_>>(),
		vec!["alpha", "zeta"]
	);
	let alpha = compiled
		.branches()
		.get("alpha")
		.expect("alpha branch should be retained");
	assert_eq!(alpha.phases().len(), 1);
	assert_eq!(alpha.phases()[0].pre_delay_seconds(), 4.0);
	assert_eq!(
		alpha.phases()[0].actions()[0].origin(),
		&WaveActionOrigin {
			container: WaveActionContainer::Branch {
				branch: "alpha".to_owned(),
				phase: 0,
			},
			action: 0,
		}
	);
	assert_eq!(
		compiled.branches()["zeta"].phases()[0].pre_delay_seconds(),
		6.0
	);
}

#[test]
fn stage_report_identifiers_round_trip_through_json() {
	let position = GridPosition { row: 3, col: 5 };
	let position_json: Value =
		serde_json::to_value(position).expect("grid position should serialize");
	assert_eq!(position_json, json!({"row": 3, "col": 5}));
	assert_eq!(
		serde_json::from_value::<GridPosition>(position_json)
			.expect("grid position should deserialize"),
		position
	);

	let reference = StageRef {
		identity: StageIdentity::StageId("main_01-07".to_owned()),
		difficulty: StageDifficulty::Normal,
	};
	let reference_json: Value =
		serde_json::to_value(&reference).expect("stage reference should serialize");
	assert_eq!(
		reference_json,
		json!({
			"identity": {"stage_id": "main_01-07"},
			"difficulty": "normal",
		})
	);
	assert_eq!(
		serde_json::from_value::<StageRef>(reference_json)
			.expect("stage reference should deserialize"),
		reference
	);
}

#[test]
fn unknown_action_scheduling_enums_and_shapes_fail_closed() {
	let cases: [(Value, &'static str, ExpectedPayloadError); 3] = [
		(
			json!("FUTURE_PERIOD"),
			"randomType",
			ExpectedPayloadError::InvalidActionField("randomType"),
		),
		(
			json!(99),
			"refreshType",
			ExpectedPayloadError::InvalidActionField("refreshType"),
		),
		(
			json!({"future": true}),
			"randomType",
			ExpectedPayloadError::LevelParse,
		),
	];

	for (value, field, expected) in cases {
		let mut level: Value = minimal_level();
		level["waves"][0]["fragments"][0]["actions"][0][field] = value;
		assert_payload_error(compile_error(&level), expected);
	}
}

#[test]
fn unknown_numeric_and_string_enums_are_retained_without_zero_coercion() {
	let mut level: Value = minimal_level();
	level["mapData"]["tiles"][0]["heightType"] = json!(91);
	level["mapData"]["tiles"][0]["buildableType"] = json!("FUTURE_BUILDABLE");
	level["routes"][0]["motionMode"] = json!(92);
	level["extraRoutes"][0]["motionMode"] = json!("BURROW");
	level["routes"][0]["checkpoints"][0]["type"] = json!(93);
	level["routes"][0]["checkpoints"][1]["type"] = json!("FUTURE_WAIT");

	let compiled: CompiledStage =
		compile_level(&level).expect("unknown enum payloads should be retained");
	assert_eq!(
		compiled.cells()[0].height(),
		&TileHeight::Unknown("numeric:91".to_owned())
	);
	assert_eq!(
		compiled.cells()[0].buildable(),
		&Buildable::Unknown("name:FUTURE_BUILDABLE".to_owned())
	);
	assert_eq!(
		compiled.routes()[0].motion(),
		&MotionMode::Unknown("numeric:92".to_owned())
	);
	assert_eq!(
		compiled.extra_routes()[0].motion(),
		&MotionMode::Unknown("name:BURROW".to_owned())
	);
	assert_eq!(
		compiled.routes()[0].checkpoints()[0].kind(),
		&RouteCheckpointKind::Unknown("numeric:93".to_owned())
	);
	assert_eq!(compiled.routes()[0].checkpoints()[0].position(), None);
	assert_eq!(
		compiled.routes()[0].checkpoints()[1].kind(),
		&RouteCheckpointKind::Unknown("name:FUTURE_WAIT".to_owned())
	);
	assert_eq!(compiled.routes()[0].checkpoints()[1].position(), None);
}

#[test]
fn spawn_route_index_is_validated_against_primary_routes_only() {
	for invalid_index in [-1_i64, 1_i64] {
		let mut level: Value = minimal_level();
		level["waves"][0]["fragments"][0]["actions"][0]["routeIndex"] = json!(invalid_index);

		assert_payload_error(
			compile_error(&level),
			ExpectedPayloadError::InvalidSpawnRouteIndex(invalid_index),
		);
	}
}

#[test]
fn malformed_map_route_coordinates_and_spawn_enemy_fail_with_typed_errors() {
	type Mutation = fn(&mut Value);
	let cases: [(Mutation, ExpectedPayloadError); 5] = [
		(
			|level: &mut Value| level["mapData"]["map"] = json!([[3, 4], [0, 1, 2]]),
			ExpectedPayloadError::InvalidMap,
		),
		(
			|level: &mut Value| level["mapData"]["map"][1][2] = json!(6),
			ExpectedPayloadError::InvalidTilePaletteIndex,
		),
		(
			|level: &mut Value| level["routes"][0]["endPosition"]["row"] = json!(2),
			ExpectedPayloadError::InvalidRoutePosition,
		),
		(
			|level: &mut Value| {
				level["routes"][0]["checkpoints"][0]["position"]["col"] = json!(3);
			},
			ExpectedPayloadError::InvalidRouteCheckpointPosition,
		),
		(
			|level: &mut Value| {
				level["waves"][0]["fragments"][0]["actions"][0]["key"] = json!("enemy_missing");
			},
			ExpectedPayloadError::SpawnEnemyNotCompiled,
		),
	];

	for (mutate, expected) in cases {
		let mut level: Value = minimal_level();
		mutate(&mut level);
		assert_payload_error(compile_error(&level), expected);
	}
}
