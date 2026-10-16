mod common;

use ark_solver::data::{ArkDataError, ArkDataSnapshot, AttackRange};
use common::Fixture;
use serde_json::json;

#[test]
fn resolves_exact_range_with_signed_source_order_offsets() {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json(
		"range_table.json",
		&json!({
			"attack-test": {
				"id": "attack-test",
				"direction": -2,
				"grids": [
					{"row": 0, "col": 0},
					{"row": -3, "col": 4},
					{"row": 5, "col": -6}
				]
			}
		}),
	);
	let snapshot: ArkDataSnapshot =
		ArkDataSnapshot::load(fixture.path()).expect("valid verified snapshot");

	let range: &AttackRange = snapshot
		.resolve_range("attack-test")
		.expect("exact verified range");

	assert_eq!(range.id(), "attack-test");
	assert_eq!(range.source_direction(), -2);
	let offsets: Vec<(i32, i32)> = range
		.offsets()
		.iter()
		.map(|offset| (offset.row(), offset.col()))
		.collect();
	assert_eq!(offsets, [(0, 0), (-3, 4), (5, -6)]);
	assert_eq!(range.offsets()[1].row(), -3);
	assert_eq!(range.offsets()[1].col(), 4);
}

#[test]
fn missing_or_case_mismatched_range_fails_closed() {
	let fixture: Fixture = Fixture::valid();
	let snapshot: ArkDataSnapshot =
		ArkDataSnapshot::load(fixture.path()).expect("valid verified snapshot");

	for requested in ["missing", "RANGE_TEST"] {
		let error: ArkDataError = snapshot
			.resolve_range(requested)
			.expect_err("unknown range must not use a fallback");

		assert!(
			matches!(
				error,
				ArkDataError::InvalidRangeId {
					ref range_id,
					ref reason,
				} if range_id == requested && reason.contains("absent")
			),
			"received {error:?}"
		);
	}
}
