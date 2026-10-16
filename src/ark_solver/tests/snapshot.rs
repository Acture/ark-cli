use ark_solver::data::ArkDataSnapshot;

mod common;

use common::{sha256_hex, Fixture, REVISION};

#[test]
fn loads_a_valid_snapshot_and_reports_its_revision() {
	let fixture: Fixture = Fixture::valid();
	let snapshot: ArkDataSnapshot =
		ArkDataSnapshot::load(fixture.path()).expect("valid snapshot should load");

	assert_eq!(snapshot.resolved_revision(), REVISION);
}

#[test]
fn frozen_empty_object_digest_matches_the_snapshot_contract() {
	assert_eq!(
		sha256_hex(b"{}"),
		"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
	);
}
