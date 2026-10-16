use ark_solver::data::{
	resolve_available_stage_levels, ArkDataError, AvailableStageLevel, StageDifficulty,
	StageIdentity,
};
use serde_json::{json, Value};

fn resolve(table: Value, code: &str) -> Result<Vec<AvailableStageLevel>, ArkDataError> {
	let bytes: Vec<u8> = serde_json::to_vec(&table).unwrap();
	resolve_available_stage_levels(&bytes, StageIdentity::Code(code.to_owned()))
}

#[test]
fn resolves_normal_and_valid_raid_levels_from_one_canonical_family() {
	let levels = resolve(
		json!({
			"stages": {
				"normal": {
					"stageId": "normal",
					"code": "BD-7",
					"levelId": "Activities/Test/Normal",
					"difficulty": "NORMAL",
					"hardStagedId": "normal#f#"
				},
				"normal#f#": {
					"stageId": "normal#f#",
					"code": "BD-7",
					"levelId": "Activities/Test/Raid",
					"difficulty": "FOUR_STAR",
					"hardStagedId": null
				}
			}
		}),
		"bd-7",
	)
	.unwrap();

	assert_eq!(
		levels,
		vec![
			AvailableStageLevel {
				stage_id: "normal".to_owned(),
				difficulty: StageDifficulty::Normal,
				level_id: "Activities/Test/Normal".to_owned(),
			},
			AvailableStageLevel {
				stage_id: "normal#f#".to_owned(),
				difficulty: StageDifficulty::Raid,
				level_id: "Activities/Test/Raid".to_owned(),
			},
		]
	);
}

#[test]
fn canonicalizes_difficulty_groups_and_rejects_unrelated_duplicate_codes() {
	let grouped = resolve(
		json!({
			"stages": {
				"easy": {
					"stageId": "easy",
					"code": "X-1",
					"levelId": "Levels/Easy",
					"difficulty": "NORMAL",
					"hardStagedId": null
				},
				"normal": {
					"stageId": "normal",
					"code": "X-1",
					"levelId": "Levels/Normal",
					"difficulty": "NORMAL",
					"hardStagedId": null
				},
				"tough": {
					"stageId": "tough",
					"code": "X-1",
					"levelId": "Levels/Tough",
					"difficulty": "NORMAL",
					"hardStagedId": null
				}
			},
			"diffGroupTable": {
				"group": {
					"easyId": "easy",
					"normalId": "normal",
					"toughId": "tough"
				}
			}
		}),
		"X-1",
	)
	.unwrap();
	assert_eq!(grouped.len(), 1);
	assert_eq!(grouped[0].stage_id, "normal");
	assert_eq!(grouped[0].level_id, "Levels/Normal");

	let ambiguous = resolve(
		json!({
			"stages": {
				"alpha": {
					"stageId": "alpha",
					"code": "DUP",
					"levelId": "Levels/Alpha",
					"difficulty": "NORMAL",
					"hardStagedId": null
				},
				"beta": {
					"stageId": "beta",
					"code": "DUP",
					"levelId": "Levels/Beta",
					"difficulty": "NORMAL",
					"hardStagedId": null
				}
			}
		}),
		"DUP",
	)
	.unwrap_err();
	assert!(matches!(ambiguous, ArkDataError::AmbiguousStageCode { .. }));
}

#[test]
fn rejects_an_invalid_hard_stage_link() {
	let error = resolve(
		json!({
			"stages": {
				"normal": {
					"stageId": "normal",
					"code": "BAD-1",
					"levelId": "Levels/Normal",
					"difficulty": "NORMAL",
					"hardStagedId": "not-raid"
				},
				"not-raid": {
					"stageId": "not-raid",
					"code": "BAD-RAID",
					"levelId": "Levels/Other",
					"difficulty": "NORMAL",
					"hardStagedId": null
				}
			}
		}),
		"BAD-1",
	)
	.unwrap_err();
	assert!(matches!(error, ArkDataError::InvalidHardStageLink { .. }));
}

#[test]
fn ignores_unsupported_difficulties_when_a_normal_family_exists() {
	let levels = resolve(
		json!({
			"stages": {
				"normal": {
					"stageId": "normal",
					"code": "17-2",
					"levelId": "Levels/Normal",
					"difficulty": "NORMAL",
					"hardStagedId": null
				},
				"six-star": {
					"stageId": "six-star",
					"code": "17-2",
					"levelId": "Levels/SixStar",
					"difficulty": "SIX_STAR",
					"hardStagedId": null
				}
			}
		}),
		"17-2",
	)
	.unwrap();

	assert_eq!(levels.len(), 1);
	assert_eq!(levels[0].stage_id, "normal");
	assert_eq!(levels[0].level_id, "Levels/Normal");
}
