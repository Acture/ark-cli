mod common;

use std::fs;

use ark_solver::data::{
	ArkDataError, ArkDataSnapshot, StageDifficulty, StageDifficultySelection, StageIdentity,
	StageRef,
};
use common::{Fixture, LEVEL_PATH};
use serde_json::{json, Map, Value};

const LEVEL_ID: &str = "Activities/Test/Level_Test_01";
const NORMAL_ID: &str = "Main_Normal";
const RAID_ID: &str = "Main_Normal#f#";
const STAGE_CODE: &str = "MiXeD-Code";

#[derive(Clone, Copy, Debug)]
enum ExpectedCompileError {
	InvalidHardLink,
	MissingLevelManifestEntry,
	MissingStageLevelId,
	RaidUnavailable,
	StageNotFound,
}

#[derive(Clone, Copy, Debug)]
enum ExpectedLoadError {
	DuplicateStageId,
	StageMapKeyMismatch,
}

fn stage_record(
	stage_id: &str,
	code: Option<&str>,
	level_id: Option<&str>,
	difficulty: &str,
	hard_staged_id: Option<&str>,
	diff_group: Option<&str>,
) -> Value {
	json!({
		"stageId": stage_id,
		"code": code,
		"levelId": level_id,
		"difficulty": difficulty,
		"hardStagedId": hard_staged_id,
		"diffGroup": diff_group,
	})
}

fn canonical_stage_table() -> Value {
	let mut stages: Map<String, Value> = Map::new();
	stages.insert(
		"Main_Easy".to_owned(),
		stage_record(
			"Main_Easy",
			Some(STAGE_CODE),
			Some(LEVEL_ID),
			"NORMAL",
			None,
			Some("EASY"),
		),
	);
	stages.insert(
		NORMAL_ID.to_owned(),
		stage_record(
			NORMAL_ID,
			Some(STAGE_CODE),
			Some(LEVEL_ID),
			"NORMAL",
			Some(RAID_ID),
			Some("NORMAL"),
		),
	);
	stages.insert(
		"Main_Tough".to_owned(),
		stage_record(
			"Main_Tough",
			Some(STAGE_CODE),
			Some(LEVEL_ID),
			"NORMAL",
			None,
			Some("TOUGH"),
		),
	);
	stages.insert(
		RAID_ID.to_owned(),
		stage_record(
			RAID_ID,
			Some(STAGE_CODE),
			Some(LEVEL_ID),
			"FOUR_STAR",
			None,
			None,
		),
	);

	json!({
		"stages": stages,
		"diffGroupTable": {
			"main-family": {
				"easyId": "Main_Easy",
				"normalId": NORMAL_ID,
				"toughId": "Main_Tough",
			}
		},
		"runeStageGroups": {},
	})
}

fn fixture_with_stage_table(stage_table: &Value) -> Fixture {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json("stage_table.json", stage_table);
	fixture
}

fn load_snapshot(stage_table: &Value) -> (Fixture, ArkDataSnapshot) {
	let fixture: Fixture = fixture_with_stage_table(stage_table);
	let snapshot: ArkDataSnapshot =
		ArkDataSnapshot::load(fixture.path()).expect("synthetic snapshot should load");
	(fixture, snapshot)
}

fn stage_ref(identity: StageIdentity, difficulty: StageDifficulty) -> StageRef {
	StageRef {
		identity,
		difficulty,
	}
}

fn set_stage_field(stage_table: &mut Value, stage_id: &str, field: &str, value: Value) {
	stage_table["stages"][stage_id][field] = value;
}

fn compile_error(stage_table: &Value, stage_ref: StageRef) -> ArkDataError {
	let (_fixture, snapshot): (Fixture, ArkDataSnapshot) = load_snapshot(stage_table);
	match snapshot.compile_stage(stage_ref) {
		Ok(_) => panic!("stage compilation unexpectedly succeeded"),
		Err(error) => error,
	}
}

fn load_error(stage_table: &Value) -> ArkDataError {
	let fixture: Fixture = fixture_with_stage_table(stage_table);
	match ArkDataSnapshot::load(fixture.path()) {
		Ok(_) => panic!("snapshot load unexpectedly succeeded"),
		Err(error) => error,
	}
}

fn assert_compile_error(error: ArkDataError, expected: ExpectedCompileError) {
	let matches_expected: bool = match expected {
		ExpectedCompileError::InvalidHardLink => {
			matches!(error, ArkDataError::InvalidHardStageLink { .. })
		}
		ExpectedCompileError::MissingLevelManifestEntry => {
			matches!(error, ArkDataError::MissingLevelManifestEntry { .. })
		}
		ExpectedCompileError::MissingStageLevelId => {
			matches!(error, ArkDataError::MissingStageLevelId { .. })
		}
		ExpectedCompileError::RaidUnavailable => {
			matches!(error, ArkDataError::RaidUnavailable { .. })
		}
		ExpectedCompileError::StageNotFound => {
			matches!(error, ArkDataError::StageNotFound { .. })
		}
	};
	assert!(
		matches_expected,
		"expected {expected:?}, received {error:?}"
	);
}

fn assert_load_error(error: ArkDataError, expected: ExpectedLoadError) {
	let matches_expected: bool = match expected {
		ExpectedLoadError::DuplicateStageId => {
			matches!(error, ArkDataError::DuplicateStageId { .. })
		}
		ExpectedLoadError::StageMapKeyMismatch => {
			matches!(error, ArkDataError::StageMapKeyMismatch { .. })
		}
	};
	assert!(
		matches_expected,
		"expected {expected:?}, received {error:?}"
	);
}

#[test]
fn both_expands_to_independent_normal_then_raid_references() {
	let identity: StageIdentity = StageIdentity::Code("1-7".to_owned());
	assert_eq!(
		StageDifficultySelection::Normal.expand(identity.clone()),
		vec![stage_ref(identity.clone(), StageDifficulty::Normal)]
	);
	assert_eq!(
		StageDifficultySelection::Raid.expand(identity.clone()),
		vec![stage_ref(identity.clone(), StageDifficulty::Raid)]
	);
	let refs: Vec<StageRef> = StageDifficultySelection::Both.expand(identity.clone());

	assert_eq!(
		refs,
		vec![
			stage_ref(identity.clone(), StageDifficulty::Normal),
			stage_ref(identity, StageDifficulty::Raid),
		]
	);
}

#[test]
fn mixed_case_base_id_resolves_normal_directly_and_raid_through_hard_link() {
	let (_fixture, snapshot): (Fixture, ArkDataSnapshot) = load_snapshot(&canonical_stage_table());

	let normal = snapshot
		.compile_stage(stage_ref(
			StageIdentity::StageId("mAiN_nOrMaL".to_owned()),
			StageDifficulty::Normal,
		))
		.expect("mixed-case normal ID should compile");
	let raid = snapshot
		.compile_stage(stage_ref(
			StageIdentity::StageId("MAIN_NORMAL".to_owned()),
			StageDifficulty::Raid,
		))
		.expect("mixed-case normal ID should follow the hard link");

	assert_eq!(normal.stage_id(), NORMAL_ID);
	assert_eq!(normal.code(), Some(STAGE_CODE));
	assert_eq!(normal.difficulty(), StageDifficulty::Normal);
	assert_eq!(normal.level_id(), LEVEL_ID);
	assert_eq!(raid.stage_id(), RAID_ID);
	assert_eq!(raid.code(), Some(STAGE_CODE));
	assert_eq!(raid.difficulty(), StageDifficulty::Raid);
	assert_eq!(raid.level_id(), LEVEL_ID);
}

#[test]
fn explicit_four_star_id_compiles_only_for_raid() {
	let (_fixture, snapshot): (Fixture, ArkDataSnapshot) = load_snapshot(&canonical_stage_table());
	let raid = snapshot
		.compile_stage(stage_ref(
			StageIdentity::StageId("main_normal#F#".to_owned()),
			StageDifficulty::Raid,
		))
		.expect("explicit FOUR_STAR ID should compile for raid");

	assert_eq!(raid.stage_id(), RAID_ID);
	assert_eq!(raid.difficulty(), StageDifficulty::Raid);
	let error: ArkDataError = match snapshot.compile_stage(stage_ref(
		StageIdentity::StageId(RAID_ID.to_owned()),
		StageDifficulty::Normal,
	)) {
		Ok(_) => panic!("FOUR_STAR ID must not compile for normal difficulty"),
		Err(error) => error,
	};
	assert!(matches!(
		error,
		ArkDataError::StageDifficultyConflict { .. }
	));
}

#[test]
fn case_insensitive_code_collapses_easy_tough_and_four_star_aliases() {
	let mut stage_table: Value = canonical_stage_table();
	for (stage_id, code) in [
		("Main_Easy", "Easy-Alias"),
		(NORMAL_ID, "Normal-Alias"),
		("Main_Tough", "Tough-Alias"),
		(RAID_ID, "Raid-Alias"),
	] {
		set_stage_field(&mut stage_table, stage_id, "code", json!(code));
	}
	let (_fixture, snapshot): (Fixture, ArkDataSnapshot) = load_snapshot(&stage_table);

	for alias in ["eAsY-aLiAs", "nOrMaL-aLiAs", "tOuGh-aLiAs", "rAiD-aLiAs"] {
		for (difficulty, expected_stage_id) in [
			(StageDifficulty::Normal, NORMAL_ID),
			(StageDifficulty::Raid, RAID_ID),
		] {
			let compiled = snapshot
				.compile_stage(stage_ref(StageIdentity::Code(alias.to_owned()), difficulty))
				.expect("every code alias should collapse to one canonical family");
			assert_eq!(compiled.stage_id(), expected_stage_id);
			assert_eq!(compiled.difficulty(), difficulty);
		}
	}
}

#[test]
fn orphan_four_star_code_is_not_a_stage_family() {
	let mut stage_table: Value = canonical_stage_table();
	set_stage_field(&mut stage_table, RAID_ID, "code", json!("Orphan-Raid"));
	set_stage_field(&mut stage_table, NORMAL_ID, "hardStagedId", Value::Null);

	let error: ArkDataError = compile_error(
		&stage_table,
		stage_ref(
			StageIdentity::Code("orphan-raid".to_owned()),
			StageDifficulty::Raid,
		),
	);
	assert_compile_error(error, ExpectedCompileError::StageNotFound);
}

#[test]
fn non_normal_hard_link_source_does_not_make_raid_code_resolvable() {
	let mut stage_table: Value = canonical_stage_table();
	set_stage_field(&mut stage_table, RAID_ID, "code", json!("Corrupt-Raid"));
	set_stage_field(&mut stage_table, NORMAL_ID, "hardStagedId", Value::Null);
	stage_table["stages"]["Nested_Raid#f#"] = stage_record(
		"Nested_Raid#f#",
		Some("Different-Code"),
		Some(LEVEL_ID),
		"FOUR_STAR",
		Some(RAID_ID),
		None,
	);

	let error: ArkDataError = compile_error(
		&stage_table,
		stage_ref(
			StageIdentity::Code("corrupt-raid".to_owned()),
			StageDifficulty::Raid,
		),
	);
	assert_compile_error(error, ExpectedCompileError::StageNotFound);
}

#[test]
fn unrelated_code_families_report_lexically_sorted_canonical_ids() {
	let mut stage_table: Value = canonical_stage_table();
	stage_table["stages"]["Alpha_Normal"] = stage_record(
		"Alpha_Normal",
		Some(STAGE_CODE),
		Some(LEVEL_ID),
		"NORMAL",
		None,
		None,
	);
	let error: ArkDataError = compile_error(
		&stage_table,
		stage_ref(
			StageIdentity::Code("MIXED-CODE".to_owned()),
			StageDifficulty::Normal,
		),
	);

	match error {
		ArkDataError::AmbiguousStageCode {
			candidate_stage_ids,
			..
		} => assert_eq!(candidate_stage_ids, vec!["Alpha_Normal", NORMAL_ID]),
		other => panic!("expected ambiguous stage-code error, received {other:?}"),
	}
}

#[test]
fn catalog_and_resolution_fail_closed_with_typed_errors() {
	type CompileMutation = fn(&mut Value);
	let compile_cases: [(CompileMutation, StageRef, ExpectedCompileError); 5] = [
		(
			|_stage_table: &mut Value| {},
			stage_ref(
				StageIdentity::Code("does-not-exist".to_owned()),
				StageDifficulty::Normal,
			),
			ExpectedCompileError::StageNotFound,
		),
		(
			|stage_table: &mut Value| {
				set_stage_field(stage_table, NORMAL_ID, "hardStagedId", Value::Null);
			},
			stage_ref(
				StageIdentity::StageId(NORMAL_ID.to_owned()),
				StageDifficulty::Raid,
			),
			ExpectedCompileError::RaidUnavailable,
		),
		(
			|stage_table: &mut Value| {
				set_stage_field(stage_table, NORMAL_ID, "levelId", Value::Null);
			},
			stage_ref(
				StageIdentity::StageId(NORMAL_ID.to_owned()),
				StageDifficulty::Normal,
			),
			ExpectedCompileError::MissingStageLevelId,
		),
		(
			|stage_table: &mut Value| {
				set_stage_field(
					stage_table,
					NORMAL_ID,
					"levelId",
					json!("Activities/Test/Not_In_Manifest"),
				);
			},
			stage_ref(
				StageIdentity::StageId(NORMAL_ID.to_owned()),
				StageDifficulty::Normal,
			),
			ExpectedCompileError::MissingLevelManifestEntry,
		),
		(
			|stage_table: &mut Value| {
				set_stage_field(stage_table, NORMAL_ID, "hardStagedId", json!("Missing#f#"));
			},
			stage_ref(
				StageIdentity::StageId(NORMAL_ID.to_owned()),
				StageDifficulty::Raid,
			),
			ExpectedCompileError::InvalidHardLink,
		),
	];

	for (mutate, stage_ref, expected) in compile_cases {
		let mut stage_table: Value = canonical_stage_table();
		mutate(&mut stage_table);
		let error: ArkDataError = compile_error(&stage_table, stage_ref);
		assert_compile_error(error, expected);
	}

	type LoadMutation = fn(&mut Value);
	let load_cases: [(LoadMutation, ExpectedLoadError); 2] = [
		(
			|stage_table: &mut Value| {
				stage_table["stages"]["DuP"] =
					stage_record("DuP", None, Some(LEVEL_ID), "NORMAL", None, None);
				stage_table["stages"]["dUp"] =
					stage_record("dUp", None, Some(LEVEL_ID), "NORMAL", None, None);
			},
			ExpectedLoadError::DuplicateStageId,
		),
		(
			|stage_table: &mut Value| {
				stage_table["stages"]["Wrong_Key"] =
					stage_record("Right_Id", None, Some(LEVEL_ID), "NORMAL", None, None);
			},
			ExpectedLoadError::StageMapKeyMismatch,
		),
	];

	for (mutate, expected) in load_cases {
		let mut stage_table: Value = canonical_stage_table();
		mutate(&mut stage_table);
		let error: ArkDataError = load_error(&stage_table);
		assert_load_error(error, expected);
	}
}

#[test]
fn caseful_level_id_uses_only_the_normalized_manifest_destination() {
	let (fixture, snapshot): (Fixture, ArkDataSnapshot) = load_snapshot(&canonical_stage_table());
	assert!(fixture.entry_path(LEVEL_PATH).is_file());
	let manifest_bytes: Vec<u8> = fs::read(fixture.manifest_path()).expect("read fixture manifest");
	let manifest: Value =
		serde_json::from_slice(&manifest_bytes).expect("parse fixture manifest JSON");
	let destinations: Vec<&str> = manifest["entries"]
		.as_array()
		.expect("manifest entries are an array")
		.iter()
		.filter_map(|entry: &Value| entry["destination_path"].as_str())
		.collect();
	assert!(destinations.contains(&LEVEL_PATH));
	assert!(!destinations.contains(&"levels/Activities/Test/Level_Test_01.json"));

	let compiled = snapshot
		.compile_stage(stage_ref(
			StageIdentity::StageId(NORMAL_ID.to_owned()),
			StageDifficulty::Normal,
		))
		.expect("caseful level ID should resolve through normalized manifest path");
	assert_eq!(compiled.level_id(), LEVEL_ID);
}

#[test]
fn compile_reverifies_level_length_and_sha_before_parsing() {
	let (fixture, snapshot): (Fixture, ArkDataSnapshot) = load_snapshot(&canonical_stage_table());
	let original_level_bytes: Vec<u8> =
		fs::read(fixture.entry_path(LEVEL_PATH)).expect("read manifest-era level bytes");
	let stage_ref: StageRef = stage_ref(
		StageIdentity::StageId(NORMAL_ID.to_owned()),
		StageDifficulty::Normal,
	);

	fs::write(fixture.entry_path(LEVEL_PATH), b"{").expect("shorten verified level");
	let length_error: ArkDataError = match snapshot.compile_stage(stage_ref.clone()) {
		Ok(_) => panic!("mutated level length must fail"),
		Err(error) => error,
	};
	assert!(matches!(
		length_error,
		ArkDataError::ByteLengthMismatch { ref destination_path, .. }
			if destination_path == LEVEL_PATH
	));

	let mut hash_mutation: Vec<u8> = original_level_bytes.clone();
	hash_mutation[0] = b'[';
	fs::write(fixture.entry_path(LEVEL_PATH), hash_mutation)
		.expect("change verified level hash without changing its length");
	let sha_error: ArkDataError = match snapshot.compile_stage(stage_ref.clone()) {
		Ok(_) => panic!("mutated level hash must fail"),
		Err(error) => error,
	};
	assert!(matches!(
		sha_error,
		ArkDataError::Sha256Mismatch { ref destination_path, .. }
			if destination_path == LEVEL_PATH
	));

	fs::write(fixture.entry_path(LEVEL_PATH), original_level_bytes)
		.expect("restore manifest-era level bytes");
	let compiled = snapshot
		.compile_stage(stage_ref)
		.expect("restoring exact manifest-era bytes should recover");
	assert_eq!(compiled.level_id(), LEVEL_ID);
}

#[test]
fn verified_malformed_or_non_object_level_json_is_a_typed_parse_error() {
	for (name, bytes) in [
		("malformed JSON", b"{".as_slice()),
		("non-object JSON", b"[]".as_slice()),
	] {
		let mut fixture: Fixture = fixture_with_stage_table(&canonical_stage_table());
		fixture.set_verified_entry_bytes(LEVEL_PATH, bytes);
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(fixture.path()).expect("verified level is parsed lazily");
		let error: ArkDataError = match snapshot.compile_stage(stage_ref(
			StageIdentity::StageId(NORMAL_ID.to_owned()),
			StageDifficulty::Normal,
		)) {
			Ok(_) => panic!("{name} must not compile as a level document"),
			Err(error) => error,
		};
		assert!(
			matches!(error, ArkDataError::LevelParse { .. }),
			"{name} returned {error:?}"
		);
	}
}
