mod common;

use ark_solver::data::{ArkDataError, ArkDataSnapshot};
use common::Fixture;
use serde_json::{json, Value};

fn load_error(range_table: &Value) -> ArkDataError {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json("range_table.json", range_table);
	ArkDataSnapshot::load(fixture.path()).expect_err("invalid range table must fail during load")
}

#[test]
fn range_map_key_must_match_record_id_during_snapshot_load() {
	let error: ArkDataError = load_error(&json!({
		"range_map_key": {
			"id": "range_record_id",
			"direction": 0,
			"grids": [{"row": -2, "col": 3}],
		},
	}));
	assert!(
		matches!(
			error,
			ArkDataError::RangeMapKeyMismatch {
				ref map_key,
				ref range_id,
			} if map_key == "range_map_key" && range_id == "range_record_id"
		),
		"received {error:?}"
	);
}

#[test]
fn range_ids_must_be_nonempty_and_ascii_case_unique() {
	let empty_error: ArkDataError = load_error(&json!({
		"": {"id": "", "direction": 0, "grids": []},
	}));
	assert!(
		matches!(
			empty_error,
			ArkDataError::InvalidRangeId {
				ref range_id,
				ref reason,
			} if range_id.is_empty() && !reason.is_empty()
		),
		"received {empty_error:?}"
	);

	let duplicate_error: ArkDataError = load_error(&json!({
		"Range_X": {"id": "Range_X", "direction": 0, "grids": []},
		"range_x": {"id": "range_x", "direction": 1, "grids": []},
	}));
	assert!(
		matches!(
			duplicate_error,
			ArkDataError::DuplicateRangeId {
				ref first_range_id,
				ref second_range_id,
			} if first_range_id == "Range_X" && second_range_id == "range_x"
		),
		"received {duplicate_error:?}"
	);
}
