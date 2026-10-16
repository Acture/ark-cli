use ark_solver::maa_job::{
	parse_maa_job, validate_maa_job_executable, MaaJob, MaaJobAction, MaaJobCodecError,
	MaaJobDifficulty, MaaJobDirection, MaaJobSeed, MaaJobSeedSelection, MaaJobSourceLineage,
	MaaJobTimeout, MaaSkillUsage,
};

const BD7_JOB: &str = include_str!("fixtures/maa_job_bd7_sanitized.json");
const MINIMAL_JOB: &str = r#"{
	"stage_name":"TEST-1",
	"minimum_required":"v6.15.1",
	"opers":[{"name":"a"}],
	"actions":[{"name":"a","location":[0,0],"direction":"Right"}],
	"difficulty":1
}"#;

fn materialize_all(job: &MaaJob) -> Result<Vec<MaaJobSeed>, MaaJobCodecError> {
	let mut seeds: Vec<MaaJobSeed> = Vec::new();
	for difficulty in job.difficulty.selected() {
		seeds.extend(
			job.seed_selections(difficulty, usize::MAX)?
				.map(MaaJobSeedSelection::materialize),
		);
	}
	Ok(seeds)
}

fn with_first_deploy_field(field: &str) -> String {
	BD7_JOB.replacen(
		r#""direction": "Right""#,
		&format!(r#""direction": "Right", {field}"#),
		1,
	)
}

fn with_operator_field(field: &str) -> String {
	MINIMAL_JOB.replacen(r#""name":"a""#, &format!(r#""name":"a",{field}"#), 1)
}

#[test]
fn bd7_job_parses_and_selects_groups_in_source_order() {
	let job = parse_maa_job(BD7_JOB).expect("tracked BD-7 job should parse");
	assert_eq!(job.stage_name(), "act21mini_07");
	assert_eq!(job.minimum_required(), "v6.0.0");
	assert!(matches!(
		job.actions()[0],
		MaaJobAction::Deploy {
			ref name,
			location: [3, 3],
			direction: MaaJobDirection::Right,
			timeout: None,
			..
		} if name == "奶盾"
	));

	let seeds = materialize_all(&job).unwrap();
	let signatures: Vec<(MaaJobDifficulty, Vec<&str>)> = seeds
		.iter()
		.map(|seed| {
			(
				seed.difficulty(),
				seed.operators()
					.iter()
					.map(|operator| operator.name())
					.collect(),
			)
		})
		.collect();
	assert_eq!(
		signatures,
		vec![
			(MaaJobDifficulty::Normal, vec!["黍", "圣聆初雪"]),
			(MaaJobDifficulty::Normal, vec!["塞雷娅", "圣聆初雪"]),
			(MaaJobDifficulty::Challenge, vec!["黍", "圣聆初雪"]),
			(MaaJobDifficulty::Challenge, vec!["塞雷娅", "圣聆初雪"]),
		]
	);
	assert!(matches!(
		seeds[1].actions()[0],
		MaaJobAction::Deploy { ref name, .. } if name == "塞雷娅"
	));
}

#[test]
fn every_action_and_document_field_round_trips_canonically() {
	let json = r##"
	{
		"stage_name":"TEST-1",
		"minimum_required":"v6.15.1",
		"doc":{
			"title":"all fields","title_color":"#112233",
			"details":"details","details_color":"#445566"
		},
		"opers":[{
			"name":"测试干员","skill":0,"skill_usage":0,"skill_times":0,
			"requirements":{
				"elite":0,"level":0,"skill_level":0,
				"module":0,"module_level":0,"potential":0
			}
		}],
		"actions":[
			{"name":"测试干员","location":[0,0],"direction":"左","doc":"d","doc_color":"Gray","costs":0,"cost_changes":-2,"kills":0,"cooling":-1,"pre_delay":0,"post_delay":0},
			{"type":"技能","name":"测试干员"},
			{"type":"撤退","location":[1,2]},
			{"type":"二倍速"},
			{"type":"子弹时间","name":"测试干员"},
			{"type":"技能用法","name":"测试干员","skill_usage":2,"skill_times":3},
			{"type":"打印","doc":"checkpoint"},
			{"type":"摆完挂机"},
			{"type":"移动镜头","distance":[1.5,-2.25]},
			{"type":"重置全局计时器"},
			{"type":"Skill","name":"测试干员","time_elapsed":0}
		],
		"difficulty":0
	}
	"##;
	let job = parse_maa_job(json).unwrap();
	assert_eq!(job.doc.title_color.as_deref(), Some("#112233"));
	assert_eq!(job.doc.details_color.as_deref(), Some("#445566"));
	assert_eq!(job.opers[0].skill_usage(), Some(0));
	let requirements = job.opers[0].requirements().unwrap();
	assert_eq!(requirements.level(), Some(0));
	assert_eq!(requirements.module(), Some(0));
	assert_eq!(requirements.module_level(), Some(0));
	assert_eq!(requirements.potential(), Some(0));
	assert_eq!(requirements.skill_level(), Some(0));
	assert!(matches!(
		job.actions()[0],
		MaaJobAction::Deploy {
			direction: MaaJobDirection::Left,
			..
		}
	));
	assert!(matches!(job.actions()[1], MaaJobAction::Skill { .. }));
	assert!(matches!(job.actions()[2], MaaJobAction::Retreat { .. }));
	assert!(matches!(job.actions()[3], MaaJobAction::SpeedUp { .. }));
	assert!(matches!(job.actions()[4], MaaJobAction::BulletTime { .. }));
	assert!(matches!(
		job.actions()[5],
		MaaJobAction::SkillUsage {
			skill_usage: MaaSkillUsage::SpecifiedTimes,
			skill_times: Some(3),
			..
		}
	));
	assert!(matches!(job.actions()[6], MaaJobAction::Output { .. }));
	assert!(matches!(job.actions()[7], MaaJobAction::SkillDaemon { .. }));
	assert!(matches!(job.actions()[8], MaaJobAction::MoveCamera { .. }));
	assert!(matches!(
		job.actions()[9],
		MaaJobAction::ResetStopwatch { .. }
	));
	assert_eq!(job.actions()[10].conditions().time_elapsed(), Some(0));

	let canonical = job.to_deterministic_json().unwrap();
	assert!(canonical.ends_with('\n'));
	assert!(canonical.contains("\"type\": \"Deploy\""));
	assert!(canonical.contains("\"direction\": \"Left\""));
	assert!(!canonical.contains("技能"));
	assert_eq!(parse_maa_job(&canonical).unwrap(), job);
}

#[test]
fn typed_timeouts_and_legacy_skill_flag_canonicalize() {
	let json = r#"
	{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"opers":[{"name":"a","skill_usage":3}],
		"actions":[
			{"name":"a","location":[0,0],"direction":"Right","timeout":1200},
			{"type":"Skill","name":"a","timeout":-1},
			{"type":"Skill","name":"a","timeout":0},
			{"type":"Skill","name":"a","skip_if_not_ready":false},
			{"type":"Skill","name":"a","skip_if_not_ready":true}
		],
		"difficulty":1
	}
	"#;
	let job = parse_maa_job(json).unwrap();
	assert_eq!(job.opers[0].skill_usage, Some(MaaSkillUsage::AutoDetermine));
	assert_eq!(
		job.actions()[0].timeout(),
		Some(MaaJobTimeout::Milliseconds(1200))
	);
	assert_eq!(job.actions()[1].timeout(), Some(MaaJobTimeout::Unlimited));
	assert_eq!(
		job.actions()[2].timeout(),
		Some(MaaJobTimeout::Milliseconds(0))
	);
	assert_eq!(job.actions()[3].timeout(), Some(MaaJobTimeout::Unlimited));
	assert_eq!(
		job.actions()[4].timeout(),
		Some(MaaJobTimeout::Milliseconds(0))
	);
	let canonical = job.to_deterministic_json().unwrap();
	assert!(!canonical.contains("skip_if_not_ready"));
	assert_eq!(canonical.matches("\"timeout\"").count(), 5);
	assert_eq!(parse_maa_job(&canonical).unwrap(), job);
	assert_eq!(
		job.seed_selections(MaaJobDifficulty::Normal, 1)
			.unwrap()
			.len(),
		1
	);
	assert!(matches!(
		job.executable_seed_selections(MaaJobDifficulty::Normal, 1),
		Err(MaaJobCodecError::InvalidJob { .. })
	));

	let unlimited_only = r#"{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"opers":[{"name":"a"}],
		"actions":[
			{"name":"a","location":[0,0],"direction":"Right","timeout":-1},
			{"type":"Skill","name":"a","timeout":-1}
		]
	}"#;
	let unlimited_job = parse_maa_job(unlimited_only).unwrap();
	validate_maa_job_executable(&unlimited_job).unwrap();
}

#[test]
fn overlapping_slots_round_trip_but_executable_expansion_fails_closed() {
	let fixed_and_group = r#"{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"opers":[{"name":"shared"}],
		"groups":[{"name":"slot","opers":[{"name":"shared"},{"name":"other"}]}],
		"actions":[{"name":"slot","location":[0,0],"direction":"Right"}],
		"difficulty":1
	}"#;
	let multiple_groups = r#"{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"groups":[
			{"name":"left","opers":[{"name":"shared"}]},
			{"name":"right","opers":[{"name":"shared"}]}
		],
		"actions":[
			{"name":"left","location":[0,0],"direction":"Right"},
			{"name":"right","location":[1,0],"direction":"Right"}
		],
		"difficulty":1
	}"#;

	for json in [fixed_and_group, multiple_groups] {
		let job = parse_maa_job(json).expect("official overlap shape should import");
		assert_eq!(
			parse_maa_job(&job.to_deterministic_json().unwrap()).unwrap(),
			job
		);
		assert!(job.seed_selections(MaaJobDifficulty::Normal, 1).is_ok());
		assert!(matches!(
			job.executable_seed_selections(MaaJobDifficulty::Normal, 1),
			Err(MaaJobCodecError::InvalidJob { .. })
		));
	}
}

#[test]
fn duplicate_alternatives_and_group_slot_aliases_are_structurally_rejected() {
	let duplicate_alternative = r#"{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"groups":[{"name":"slot","opers":[{"name":"same"},{"name":"same"}]}],
		"actions":[{"name":"slot","location":[0,0],"direction":"Right"}]
	}"#;
	let group_slot_alias = r#"{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"groups":[
			{"name":"left","opers":[{"name":"right"}]},
			{"name":"right","opers":[{"name":"other"}]}
		],
		"actions":[{"name":"left","location":[0,0],"direction":"Right"}]
	}"#;

	for json in [duplicate_alternative, group_slot_alias] {
		assert!(matches!(
			parse_maa_job(json),
			Err(MaaJobCodecError::InvalidJob { .. })
		));
	}
}

#[test]
fn public_job_mutation_is_validated_before_expansion_or_canonical_serialization() {
	let mut job = parse_maa_job(BD7_JOB).unwrap();
	job.groups[0].opers.clear();
	assert!(serde_json::to_string(&job).is_ok());
	assert!(matches!(
		job.seed_selections(MaaJobDifficulty::Normal, 1),
		Err(MaaJobCodecError::InvalidJob { .. })
	));
	let content_sha256 = "ab".repeat(32);
	let lineage = MaaJobSourceLineage::new("mutation-test", &content_sha256).unwrap();
	assert!(matches!(
		job.guide_seeds(lineage, MaaJobDifficulty::Normal, 1),
		Err(MaaJobCodecError::InvalidJob { .. })
	));
	assert!(matches!(
		job.to_deterministic_json(),
		Err(MaaJobCodecError::InvalidJob { .. })
	));
}

#[test]
fn deprecated_or_ambiguous_fields_fail_closed() {
	for replacement in [
		r#""timeout": 0, "skip_if_not_ready": true"#,
		r#""rear_delay": 100"#,
	] {
		let json = with_first_deploy_field(replacement);
		assert!(matches!(
			parse_maa_job(&json),
			Err(MaaJobCodecError::MalformedJson { .. })
		));
	}
	let potentiality = BD7_JOB.replacen(
		"\"skill_usage\": 0",
		"\"skill_usage\": 0, \"requirements\": { \"potentiality\": 6 }",
		1,
	);
	assert!(matches!(
		parse_maa_job(&potentiality),
		Err(MaaJobCodecError::MalformedJson { .. })
	));
	let deploy_skip = BD7_JOB.replace(
		r#""direction": "Right""#,
		r#""direction": "Right", "skip_if_not_ready": true"#,
	);
	assert!(matches!(
		parse_maa_job(&deploy_skip),
		Err(MaaJobCodecError::MalformedJson { .. })
	));
}

#[test]
fn stopwatch_order_round_trips_but_executable_profile_rejects_it() {
	let ordered = r#"
	{
		"stage_name":"TEST-1","minimum_required":"v6.15.1",
		"opers":[{"name":"a"}],
		"actions":[
			{"type":"ResetStopwatch"},
			{"type":"Skill","name":"a","time_elapsed":1250}
		],
		"difficulty":1
	}
	"#;
	let job = parse_maa_job(ordered).unwrap();
	assert_eq!(
		parse_maa_job(&job.to_deterministic_json().unwrap()).unwrap(),
		job
	);
	assert!(matches!(
		validate_maa_job_executable(&job),
		Err(MaaJobCodecError::InvalidJob { .. })
	));

	let unordered = ordered.replace("{\"type\":\"ResetStopwatch\"},", "");
	assert!(matches!(
		parse_maa_job(&unordered),
		Err(MaaJobCodecError::InvalidJob { .. })
	));
}

#[test]
fn required_fields_and_condition_ranges_are_validated() {
	for invalid in [
		BD7_JOB.replace("\"minimum_required\": \"v6.0.0\",", ""),
		BD7_JOB.replace(
			"\"minimum_required\": \"v6.0.0\"",
			"\"minimum_required\": \" \"",
		),
		with_first_deploy_field("\"costs\": -1"),
		with_first_deploy_field("\"kills\": -1"),
		with_first_deploy_field("\"pre_delay\": -1"),
		with_first_deploy_field("\"post_delay\": -1"),
		with_first_deploy_field("\"cooling\": -2"),
	] {
		assert!(parse_maa_job(&invalid).is_err(), "{invalid}");
	}
	let legal = with_first_deploy_field("\"cost_changes\": -10, \"cooling\": -1");
	parse_maa_job(&legal).unwrap();
}

#[test]
fn numeric_protocol_ranges_reject_out_of_profile_values() {
	for invalid in [
		with_operator_field(r#""skill":4"#),
		with_operator_field(r#""skill_usage":4"#),
		with_operator_field(r#""requirements":{"elite":3}"#),
		with_operator_field(r#""requirements":{"level":91}"#),
		with_operator_field(r#""requirements":{"module":-1}"#),
		with_operator_field(r#""requirements":{"module":4}"#),
		with_operator_field(r#""requirements":{"module_level":4}"#),
		with_operator_field(r#""requirements":{"potential":7}"#),
		with_operator_field(r#""requirements":{"skill_level":11}"#),
		MINIMAL_JOB.replace("\"difficulty\":1", "\"difficulty\":4"),
		MINIMAL_JOB.replace(
			r#""direction":"Right""#,
			r#""direction":"Right","timeout":-2"#,
		),
		MINIMAL_JOB.replace(
			r#""direction":"Right""#,
			r#""direction":"Right","timeout":4294967296"#,
		),
	] {
		assert!(parse_maa_job(&invalid).is_err(), "{invalid}");
	}
}

#[test]
fn unspecified_difficulty_binds_to_the_callers_stage_mode() {
	let without_difficulty = MINIMAL_JOB.replace(",\n\t\"difficulty\":1", "");
	let job = parse_maa_job(&without_difficulty).unwrap();
	assert_eq!(
		job.difficulty.selected(),
		vec![MaaJobDifficulty::Unspecified]
	);
	assert_eq!(
		job.seed_selections(MaaJobDifficulty::Normal, 1)
			.unwrap()
			.len(),
		1
	);
	assert_eq!(
		job.seed_selections(MaaJobDifficulty::Challenge, 1)
			.unwrap()
			.len(),
		1
	);
}

#[test]
fn unknown_action_types_and_fields_are_rejected() {
	for invalid in [
		BD7_JOB.replace("SpeedUp", "Teleport"),
		BD7_JOB.replace(
			r#""direction": "Right""#,
			r#""direction": "Right", "unknown_behavioral_guard": 7"#,
		),
	] {
		assert!(matches!(
			parse_maa_job(&invalid),
			Err(MaaJobCodecError::MalformedJson { .. })
		));
	}
}
