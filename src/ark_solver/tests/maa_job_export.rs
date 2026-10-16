use std::collections::BTreeMap;

use ark_solver::data::{GridPosition, MapDimensions};
use ark_solver::maa_job::{
	compile_maa_job, parse_maa_job, MaaExactRequirements, MaaJob, MaaJobAction, MaaJobCompileError,
	MaaJobCompileRequest, MaaJobDifficulty, MaaJobDirection, MaaJobExportMetadata, MaaJobSeed,
	MaaReplacementAction, MaaReplacementActionKind, MaaReplacementOperator,
};
use serde_json::{json, Value};

const BD7_GUIDE: &str = include_str!("fixtures/maa_job_bd7_sanitized.json");

fn normal_seeds(job: &MaaJob) -> Vec<MaaJobSeed> {
	job.seed_selections(MaaJobDifficulty::Normal, 2)
		.expect("Normal difficulty must be selected")
		.map(|selection| selection.materialize())
		.collect()
}

fn first_seed(job: &MaaJob) -> MaaJobSeed {
	normal_seeds(job)
		.into_iter()
		.next()
		.expect("job must contain one Normal selection")
}

#[test]
fn bd7_export_is_independent_deterministic_and_round_trips() {
	let guide = parse_maa_job(BD7_GUIDE).expect("tracked BD-7 guide should parse");
	let seed = guide
		.seed_selections(MaaJobDifficulty::Normal, 1)
		.expect("normal difficulty should be selected")
		.next()
		.expect("first selected group alternative should exist")
		.materialize();
	let replacement = exusiai();
	let replacement_actions =
		single_replacement_actions(&replacement.name, source_cell(), 660, 960, 960);
	let retained_exact_requirements = BTreeMap::from([("黍".to_owned(), exact_requirements())]);
	let request = MaaJobCompileRequest {
		seed: &seed,
		metadata: MaaJobExportMetadata {
			minimum_required: guide.minimum_required(),
			title: guide.title(),
			details: guide.details(),
		},
		source_slot_name: "圣聆初雪",
		replacements: std::slice::from_ref(&replacement),
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: MapDimensions {
			width: 10,
			height: 8,
		},
	};

	let first = compile_maa_job(request).expect("earliest-ready candidate should export");
	let second = compile_maa_job(request).expect("same candidate should export deterministically");
	assert_eq!(first, second);
	assert!(first.groups.is_empty(), "selected group must be frozen");
	assert_eq!(
		first
			.opers
			.iter()
			.map(|operator| operator.name.as_str())
			.collect::<Vec<_>>(),
		vec!["黍", "能天使"]
	);

	let value: Value = serde_json::to_value(&first).expect("wire document should serialize");
	assert_eq!(
		value,
		json!({
			"minimum_required": "v6.0.0",
			"stage_name": "act21mini_07",
			"actions": [
				{
					"type": "Deploy",
					"name": "黍",
					"location": [3, 3],
					"direction": "Right"
				},
				{
					"type": "Deploy",
					"name": "能天使",
					"location": [3, 2],
					"direction": "Right"
				},
				{"type": "Skill", "name": "能天使"},
				{"type": "SpeedUp"},
				{"type": "SkillDaemon"}
			],
			"doc": {
				"title": "BD-7",
				"details": "Sanitized tracked fixture derived from a PRTS Plus guide."
			},
			"groups": [],
			"opers": [
				{
					"name": "黍",
					"skill": 1,
					"skill_usage": 0,
					"requirements": {
						"elite": 2,
						"level": 90,
						"potential": 6,
						"skill_level": 10
					}
				},
				{
					"name": "能天使",
					"skill": 2,
					"skill_usage": 1,
					"requirements": {
						"elite": 2,
						"level": 90,
						"potential": 6,
						"skill_level": 10
					}
				}
			],
			"difficulty": 1
		})
	);
	assert_eq!(
		serde_json::to_string(&first).unwrap(),
		serde_json::to_string(&second).unwrap()
	);
	let deterministic_json: String = first.to_deterministic_json().unwrap();
	assert_eq!(
		deterministic_json,
		serde_json::to_string_pretty(&first).unwrap() + "\n"
	);
	let deterministic_sha256: String = first.deterministic_sha256_hex().unwrap();
	assert_eq!(
		deterministic_sha256,
		"112ec8108ae86f80985439d9fb01a13a567bc97641422ef793fcb77411ff51aa"
	);

	let serialized = serde_json::to_string(&first).unwrap();
	let imported = parse_maa_job(&serialized).expect("exported strategy must re-import cleanly");
	let round_trip = normal_seeds(&imported);
	assert_eq!(round_trip.len(), 1);
	assert_eq!(
		round_trip[0]
			.operators()
			.iter()
			.map(|operator| operator.name())
			.collect::<Vec<_>>(),
		vec!["黍", "能天使"]
	);
	let actions: &[MaaJobAction] = round_trip[0].actions();
	assert!(matches!(
		actions,
		[
			MaaJobAction::Deploy {
				name: shu,
				location: [3, 3],
				..
			},
			MaaJobAction::Deploy {
				name: exusiai,
				location: [3, 2],
				..
			},
			MaaJobAction::Skill {
				name: Some(skill_target),
				location: None,
				..
			},
			MaaJobAction::SpeedUp { .. },
			MaaJobAction::SkillDaemon { .. },
		] if shu == "黍" && exusiai == "能天使" && skill_target == "能天使"
	));
}

#[test]
fn export_rejects_later_activation_instead_of_inventing_timing() {
	let guide = parse_maa_job(BD7_GUIDE).unwrap();
	let seed = first_seed(&guide);
	let replacement = exusiai();
	let replacement_actions =
		single_replacement_actions(&replacement.name, source_cell(), 660, 961, 960);
	let retained_exact_requirements = BTreeMap::from([("黍".to_owned(), exact_requirements())]);

	let error = compile_maa_job(MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "圣聆初雪",
		replacements: std::slice::from_ref(&replacement),
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: bd7_dimensions(),
	})
	.unwrap_err();

	assert_eq!(
		error,
		MaaJobCompileError::ReplacementActionIsNotEarliest {
			action_index: 1,
			name: "能天使".to_owned(),
			action_type: "Skill",
			chosen_tick: 961,
			earliest_legal_tick: 960,
		}
	);
}

#[test]
fn export_materializes_multiple_replacements_with_global_action_order() {
	let guide_json = minimal_guide(
		"",
		r#","doc":"replacement slot""#,
		r#","doc_color":"Gray""#,
		r#","skill_usage":0"#,
	);
	let guide = parse_maa_job(&guide_json).unwrap();
	let seed = first_seed(&guide);
	let replacements = [exusiai(), kroos()];
	let replacement_actions = [
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 10),
		skill_action(&replacements[0].name, 20, 20),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 30),
		skill_action(&replacements[1].name, 40, 40),
	];
	let retained_exact_requirements =
		BTreeMap::from([("retained".to_owned(), exact_requirements())]);
	let request = MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "source",
		replacements: &replacements,
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: MapDimensions {
			width: 3,
			height: 1,
		},
	};

	let first = compile_maa_job(request).unwrap();
	let second = compile_maa_job(request).unwrap();
	assert_eq!(first, second);
	assert_eq!(
		first
			.opers
			.iter()
			.map(|operator| operator.name.as_str())
			.collect::<Vec<_>>(),
		vec!["retained", "能天使", "克洛丝"]
	);
	let value = serde_json::to_value(&first).unwrap();
	assert_eq!(
		value["actions"]
			.as_array()
			.unwrap()
			.iter()
			.map(|action| action["type"].as_str().unwrap())
			.collect::<Vec<_>>(),
		vec![
			"Deploy",
			"Deploy",
			"Skill",
			"Deploy",
			"Skill",
			"SpeedUp",
			"SkillDaemon"
		]
	);
	assert_eq!(value["actions"][1]["name"], "能天使");
	assert_eq!(value["actions"][1]["doc"], "replacement slot");
	assert_eq!(value["actions"][2]["name"], "能天使");
	assert!(value["actions"][2].get("doc").is_none());
	assert_eq!(value["actions"][3]["name"], "克洛丝");
	assert_eq!(value["actions"][4]["name"], "克洛丝");
	assert_eq!(value["actions"][5]["doc_color"], "Gray");

	let serialized = serde_json::to_string(&first).unwrap();
	let round_trip = normal_seeds(&parse_maa_job(&serialized).unwrap());
	assert_eq!(round_trip.len(), 1);
	assert_eq!(
		round_trip[0]
			.operators()
			.iter()
			.map(|operator| operator.name())
			.collect::<Vec<_>>(),
		vec!["retained", "能天使", "克洛丝"]
	);
}

#[test]
fn export_rejects_invalid_multi_replacement_identity_and_action_sets() {
	let duplicate_replacements = [exusiai(), exusiai()];
	assert_eq!(
		compile_multi_export(&duplicate_replacements, &[]).unwrap_err(),
		MaaJobCompileError::DuplicateReplacementName {
			name: "能天使".to_owned(),
		}
	);
	assert_eq!(
		compile_multi_export(&[], &[]).unwrap_err(),
		MaaJobCompileError::EmptyReplacements
	);

	let replacements = [exusiai(), kroos()];
	let unknown_actions = [
		deploy_action("unknown", 0, 1, MaaJobDirection::Right, 10),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 20),
		skill_action(&replacements[1].name, 30, 30),
		skill_action(&replacements[0].name, 40, 40),
	];
	assert_eq!(
		compile_multi_export(&replacements, &unknown_actions).unwrap_err(),
		MaaJobCompileError::UnknownReplacementActionOperator {
			action_index: 0,
			name: "unknown".to_owned(),
		}
	);

	let missing_skill_actions = [
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 10),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 20),
		skill_action(&replacements[0].name, 30, 30),
	];
	assert_eq!(
		compile_multi_export(&replacements, &missing_skill_actions).unwrap_err(),
		MaaJobCompileError::MissingReplacementSkill {
			name: "克洛丝".to_owned(),
		}
	);
}

#[test]
fn export_rejects_invalid_multi_replacement_geometry_and_order() {
	let replacements = [exusiai(), kroos()];
	let duplicate_cell_actions = [
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 10),
		deploy_action(&replacements[1].name, 0, 1, MaaJobDirection::Left, 20),
		skill_action(&replacements[0].name, 30, 30),
		skill_action(&replacements[1].name, 40, 40),
	];
	assert_eq!(
		compile_multi_export(&replacements, &duplicate_cell_actions).unwrap_err(),
		MaaJobCompileError::DuplicateReplacementCell {
			row: 0,
			col: 1,
			first_name: "能天使".to_owned(),
			second_name: "克洛丝".to_owned(),
		}
	);

	let interleaved_actions = [
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 10),
		skill_action(&replacements[0].name, 20, 20),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 30),
		skill_action(&replacements[1].name, 40, 40),
	];
	compile_multi_export(&replacements, &interleaved_actions)
		.expect("a member skill may precede another member's deployment");

	let reversed_equal_tick_deployments = [
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 10),
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 10),
		skill_action(&replacements[0].name, 20, 20),
		skill_action(&replacements[1].name, 30, 30),
	];
	assert_eq!(
		compile_multi_export(&replacements, &reversed_equal_tick_deployments).unwrap_err(),
		MaaJobCompileError::ReplacementDeploymentOrderMismatch {
			action_index: 0,
			expected_name: "能天使".to_owned(),
			actual_name: "克洛丝".to_owned(),
		}
	);

	let skill_before_own_deploy_actions = [
		skill_action(&replacements[0].name, 10, 10),
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 20),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 30),
		skill_action(&replacements[1].name, 40, 40),
	];
	assert_eq!(
		compile_multi_export(&replacements, &skill_before_own_deploy_actions).unwrap_err(),
		MaaJobCompileError::ReplacementSkillBeforeDeployment {
			action_index: 0,
			name: "能天使".to_owned(),
		}
	);

	let out_of_order_actions = [
		deploy_action(&replacements[0].name, 0, 1, MaaJobDirection::Right, 20),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 10),
		skill_action(&replacements[0].name, 30, 30),
		skill_action(&replacements[1].name, 40, 40),
	];
	assert_eq!(
		compile_multi_export(&replacements, &out_of_order_actions).unwrap_err(),
		MaaJobCompileError::ReplacementActionsOutOfOrder {
			previous_action_index: 0,
			previous_action_tick: 20,
			action_index: 1,
			action_tick: 10,
		}
	);

	let out_of_bounds_actions = [
		deploy_action(&replacements[0].name, 0, 3, MaaJobDirection::Right, 10),
		deploy_action(&replacements[1].name, 0, 2, MaaJobDirection::Left, 20),
		skill_action(&replacements[0].name, 30, 30),
		skill_action(&replacements[1].name, 40, 40),
	];
	assert_eq!(
		compile_multi_export(&replacements, &out_of_bounds_actions).unwrap_err(),
		MaaJobCompileError::ReplacementCellOutOfBounds {
			row: 0,
			col: 3,
			map_width: 3,
			map_height: 1,
		}
	);
}

#[test]
fn export_rejects_replacement_cell_still_occupied_by_retained_operator() {
	let guide_json = minimal_guide("", "", "", r#","skill_usage":0"#);
	let guide = parse_maa_job(&guide_json).unwrap();
	let seed = first_seed(&guide);
	let replacement = exusiai();
	let replacement_actions = single_replacement_actions(
		&replacement.name,
		GridPosition { row: 0, col: 0 },
		0,
		10,
		10,
	);
	let retained_exact_requirements =
		BTreeMap::from([("retained".to_owned(), exact_requirements())]);

	let error = compile_maa_job(MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "source",
		replacements: std::slice::from_ref(&replacement),
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: MapDimensions {
			width: 2,
			height: 1,
		},
	})
	.unwrap_err();

	assert_eq!(
		error,
		MaaJobCompileError::ReplacementCellOccupiedByRetained {
			action_index: 0,
			replacement_name: "能天使".to_owned(),
			retained_name: "retained".to_owned(),
			location: [0, 0],
		}
	);
	assert!(error.to_string().contains("explicit preceding Retreat"));
}

#[test]
fn explicit_retreat_by_name_or_location_releases_retained_occupancy() {
	let source_action = r#"{"type":"Deploy","name":"source","location":[1,0],"direction":"Right"}"#;
	for retreat in [
		r#"{"type":"Retreat","name":"retained"}"#,
		r#"{"type":"Retreat","location":[0,0]}"#,
	] {
		let guide_json = minimal_guide("", "", "", r#","skill_usage":0"#).replacen(
			source_action,
			&format!("{retreat},{source_action}"),
			1,
		);
		let guide = parse_maa_job(&guide_json).unwrap();
		let seed = first_seed(&guide);
		let replacement = exusiai();
		let replacement_actions = single_replacement_actions(
			&replacement.name,
			GridPosition { row: 0, col: 0 },
			0,
			10,
			10,
		);
		let retained_exact_requirements =
			BTreeMap::from([("retained".to_owned(), exact_requirements())]);

		compile_maa_job(MaaJobCompileRequest {
			seed: &seed,
			metadata: metadata(&guide),
			source_slot_name: "source",
			replacements: std::slice::from_ref(&replacement),
			replacement_actions: &replacement_actions,
			retained_exact_requirements: &retained_exact_requirements,
			map_dimensions: MapDimensions {
				width: 2,
				height: 1,
			},
		})
		.expect("an explicit Retreat should release inferred occupancy");
	}
}

#[test]
fn export_rejects_stopwatch_input() {
	let replacement = exusiai();
	let json = r#"{
				"stage_name":"test",
				"minimum_required":"v6.0.0",
				"doc":{"title":"test"},
				"opers":[{"name":"source","skill":1}],
				"actions":[
					{"type":"Deploy","name":"source","location":[0,0],"direction":"Right"},
					{"type":"ResetStopwatch"},
					{"type":"SkillDaemon"}
				],
				"difficulty":1
			}"#;
	let guide = parse_maa_job(json).unwrap();
	let seed = first_seed(&guide);
	let retained_exact_requirements = BTreeMap::new();
	let replacement_actions = single_replacement_actions(
		&replacement.name,
		GridPosition { row: 0, col: 0 },
		0,
		10,
		10,
	);
	let error = compile_maa_job(MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "source",
		replacements: std::slice::from_ref(&replacement),
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: MapDimensions {
			width: 1,
			height: 1,
		},
	})
	.unwrap_err();
	assert_eq!(
		error,
		MaaJobCompileError::ResetStopwatchUnsupported { action_index: 1 }
	);
}

#[test]
fn export_rejects_missing_retained_exact_requirements() {
	let guide = parse_maa_job(BD7_GUIDE).unwrap();
	let seed = first_seed(&guide);
	let replacement = exusiai();
	let replacement_actions =
		single_replacement_actions(&replacement.name, source_cell(), 660, 960, 960);
	let retained_exact_requirements = BTreeMap::new();

	let error = compile_maa_job(MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "圣聆初雪",
		replacements: std::slice::from_ref(&replacement),
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: bd7_dimensions(),
	})
	.unwrap_err();

	assert_eq!(
		error,
		MaaJobCompileError::MissingRetainedExactRequirements {
			name: "黍".to_owned(),
		}
	);
}

#[test]
fn export_rejects_branching_timeout_and_normalizes_unlimited_timeout() {
	for (retained_conditions, source_conditions, expected_action_index) in
		[(r#","timeout":0"#, "", 0), ("", r#","timeout":1"#, 1)]
	{
		let guide_json = minimal_guide(
			retained_conditions,
			source_conditions,
			"",
			r#","skill_usage":0"#,
		);
		let error = compile_minimal_export(&guide_json).unwrap_err();
		assert_eq!(
			error,
			MaaJobCompileError::TimeoutUnsupported {
				action_index: expected_action_index,
			}
		);
	}

	let unlimited = minimal_guide(r#","timeout":-1"#, "", "", r#","skill_usage":0"#);
	let exported =
		compile_minimal_export(&unlimited).expect("unlimited timeout is default-equivalent");
	assert!(exported
		.actions()
		.iter()
		.all(|action| action.timeout().is_none()));
}

#[test]
fn export_rejects_unmodeled_conditions_by_action_role() {
	let cases = [
		(
			minimal_guide(r#","kills":1"#, "", "", r#","skill_usage":0"#),
			MaaJobCompileError::RetainedDeploymentConditionUnsupported {
				action_index: 0,
				field: "kills",
			},
		),
		(
			minimal_guide("", r#","post_delay":100"#, "", r#","skill_usage":0"#),
			MaaJobCompileError::SourceDeploymentConditionUnsupported {
				action_index: 1,
				field: "post_delay",
			},
		),
		(
			minimal_guide("", "", r#","pre_delay":100"#, r#","skill_usage":0"#),
			MaaJobCompileError::NonDeploymentConditionUnsupported {
				action_index: 2,
				action_type: "SpeedUp",
				field: "pre_delay",
			},
		),
	];

	for (guide_json, expected) in cases {
		assert_eq!(compile_minimal_export(&guide_json).unwrap_err(), expected);
	}
}

#[test]
fn export_preserves_only_scheduler_modeled_retained_deploy_conditions() {
	let guide_json = minimal_guide(
		r#","doc":"wait DP","costs":12,"post_delay":100"#,
		r#","doc":"replacement slot""#,
		r#","doc_color":"Gray""#,
		"",
	);
	let document =
		compile_minimal_export(&guide_json).expect("modeled retained conditions should export");
	let value = serde_json::to_value(document).unwrap();

	assert_eq!(value["actions"][0]["costs"], 12);
	assert_eq!(value["actions"][0]["post_delay"], 100);
	assert_eq!(value["actions"][0]["doc"], "wait DP");
	assert_eq!(value["actions"][1]["doc"], "replacement slot");
	assert_eq!(value["actions"][3]["doc_color"], "Gray");
}

#[test]
fn export_rejects_retained_daemon_skill_usage() {
	let guide_json = minimal_guide("", "", "", r#","skill_usage":1"#);
	let error = compile_minimal_export(&guide_json).unwrap_err();

	assert_eq!(
		error,
		MaaJobCompileError::RetainedDaemonSkillUsageUnsupported {
			name: "retained".to_owned(),
			skill_usage: 1,
		}
	);
}

#[test]
fn export_rejects_exact_requirements_outside_the_wire_profile() {
	let mut invalid_level = exact_requirements();
	invalid_level.level = 91;
	let mut invalid_module = exact_requirements();
	invalid_module.module = Some(0);
	invalid_module.module_level = 1;

	for (invalid, expected) in [
		(
			invalid_level,
			MaaJobCompileError::InvalidLevel { value: 91 },
		),
		(
			invalid_module,
			MaaJobCompileError::InvalidModule { value: 0 },
		),
	] {
		let guide_json = minimal_guide("", "", "", r#","skill_usage":0"#);
		assert_eq!(
			compile_minimal_export_with_requirements(
				&guide_json,
				invalid.clone(),
				exact_requirements(),
			)
			.unwrap_err(),
			expected
		);
		assert_eq!(
			compile_minimal_export_with_requirements(&guide_json, exact_requirements(), invalid,)
				.unwrap_err(),
			expected
		);
	}
}

fn compile_multi_export(
	replacements: &[MaaReplacementOperator],
	replacement_actions: &[MaaReplacementAction<'_>],
) -> Result<MaaJob, MaaJobCompileError> {
	let guide_json = minimal_guide("", "", "", r#","skill_usage":0"#);
	let guide = parse_maa_job(&guide_json).expect("minimal guide should parse");
	let seed = first_seed(&guide);
	let retained_exact_requirements =
		BTreeMap::from([("retained".to_owned(), exact_requirements())]);
	compile_maa_job(MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "source",
		replacements,
		replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: MapDimensions {
			width: 3,
			height: 1,
		},
	})
}

fn compile_minimal_export(guide_json: &str) -> Result<MaaJob, MaaJobCompileError> {
	compile_minimal_export_with_requirements(guide_json, exact_requirements(), exact_requirements())
}

fn compile_minimal_export_with_requirements(
	guide_json: &str,
	replacement_requirements: MaaExactRequirements,
	retained_requirements: MaaExactRequirements,
) -> Result<MaaJob, MaaJobCompileError> {
	let guide = parse_maa_job(guide_json).expect("minimal guide should parse");
	let seed = first_seed(&guide);
	let replacement = MaaReplacementOperator {
		name: "能天使".to_owned(),
		skill: 2,
		requirements: replacement_requirements,
	};
	let replacement_actions = single_replacement_actions(
		&replacement.name,
		GridPosition { row: 0, col: 1 },
		0,
		10,
		10,
	);
	let retained_exact_requirements =
		BTreeMap::from([("retained".to_owned(), retained_requirements)]);
	compile_maa_job(MaaJobCompileRequest {
		seed: &seed,
		metadata: metadata(&guide),
		source_slot_name: "source",
		replacements: std::slice::from_ref(&replacement),
		replacement_actions: &replacement_actions,
		retained_exact_requirements: &retained_exact_requirements,
		map_dimensions: MapDimensions {
			width: 2,
			height: 1,
		},
	})
}

fn single_replacement_actions<'a>(
	operator_name: &'a str,
	cell: GridPosition,
	deploy_tick: u64,
	chosen_activation_tick: u64,
	earliest_legal_activation_tick: u64,
) -> [MaaReplacementAction<'a>; 2] {
	[
		MaaReplacementAction {
			operator_name,
			kind: MaaReplacementActionKind::Deploy {
				cell,
				direction: MaaJobDirection::Right,
			},
			chosen_tick: deploy_tick,
			earliest_legal_tick: deploy_tick,
		},
		skill_action(
			operator_name,
			chosen_activation_tick,
			earliest_legal_activation_tick,
		),
	]
}

fn deploy_action(
	operator_name: &str,
	row: u32,
	col: u32,
	direction: MaaJobDirection,
	tick: u64,
) -> MaaReplacementAction<'_> {
	MaaReplacementAction {
		operator_name,
		kind: MaaReplacementActionKind::Deploy {
			cell: GridPosition { row, col },
			direction,
		},
		chosen_tick: tick,
		earliest_legal_tick: tick,
	}
}

fn skill_action(
	operator_name: &str,
	chosen_tick: u64,
	earliest_legal_tick: u64,
) -> MaaReplacementAction<'_> {
	MaaReplacementAction {
		operator_name,
		kind: MaaReplacementActionKind::Skill,
		chosen_tick,
		earliest_legal_tick,
	}
}

fn minimal_guide(
	retained_conditions: &str,
	source_conditions: &str,
	speed_conditions: &str,
	retained_skill_usage: &str,
) -> String {
	format!(
		r#"{{
			"stage_name":"test",
			"minimum_required":"v6.0.0",
			"doc":{{"title":"test"}},
			"opers":[
				{{"name":"retained","skill":1{retained_skill_usage}}},
				{{"name":"source","skill":1,"skill_usage":0}}
			],
			"actions":[
				{{"type":"Deploy","name":"retained","location":[0,0],"direction":"Right"{retained_conditions}}},
				{{"type":"Deploy","name":"source","location":[1,0],"direction":"Right"{source_conditions}}},
				{{"type":"SpeedUp"{speed_conditions}}},
				{{"type":"SkillDaemon"}}
			],
			"difficulty":1
		}}"#
	)
}

fn exusiai() -> MaaReplacementOperator {
	MaaReplacementOperator {
		name: "能天使".to_owned(),
		skill: 2,
		requirements: exact_requirements(),
	}
}

fn kroos() -> MaaReplacementOperator {
	MaaReplacementOperator {
		name: "克洛丝".to_owned(),
		skill: 1,
		requirements: exact_requirements(),
	}
}

const fn exact_requirements() -> MaaExactRequirements {
	MaaExactRequirements {
		elite: 2,
		level: 90,
		potential: 6,
		skill_level: 10,
		module: None,
		module_level: 0,
	}
}

fn metadata(guide: &MaaJob) -> MaaJobExportMetadata<'_> {
	MaaJobExportMetadata {
		minimum_required: guide.minimum_required(),
		title: guide.title(),
		details: guide.details(),
	}
}

const fn source_cell() -> GridPosition {
	GridPosition { row: 5, col: 3 }
}

const fn bd7_dimensions() -> MapDimensions {
	MapDimensions {
		width: 10,
		height: 8,
	}
}
