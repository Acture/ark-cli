use std::{fs, io::ErrorKind, path::PathBuf};

use ark_solver::data::{ArkDataError, ArkDataSnapshot};
use serde_json::Value;

mod common;

use common::{sha256_hex, Fixture, REQUIRED_ROOTS, REVISION};

fn load_error(fixture: &Fixture) -> ArkDataError {
	ArkDataSnapshot::load(fixture.path()).expect_err("invalid snapshot must fail to load")
}

fn assert_invalid_manifest(error: ArkDataError, expected_field: &'static str) {
	match error {
		ArkDataError::InvalidManifest { field, .. } => assert_eq!(field, expected_field),
		other => panic!("expected InvalidManifest for {expected_field}, got {other:?}"),
	}
}

fn assert_unsafe_path(error: ArkDataError, expected_field: &'static str, expected_path: &str) {
	match error {
		ArkDataError::UnsafePath { field, path, .. } => {
			assert_eq!(field, expected_field);
			assert_eq!(path, expected_path);
		}
		other => {
			panic!("expected UnsafePath for {expected_field}={expected_path:?}, got {other:?}")
		}
	}
}

#[test]
fn manifest_previous_without_manifest_is_not_accepted() {
	let fixture: Fixture = Fixture::valid();
	let previous_manifest_path: PathBuf = fixture.path().join("manifest.previous.json");
	fs::rename(fixture.manifest_path(), previous_manifest_path)
		.expect("move manifest to previous-manifest location");

	match load_error(&fixture) {
		ArkDataError::ManifestRead { path, source } => {
			assert_eq!(path, fixture.manifest_path());
			assert_eq!(source.kind(), ErrorKind::NotFound);
		}
		other => panic!("expected missing manifest.json to report ManifestRead, got {other:?}"),
	}
}

#[test]
fn rejects_schema_versions_other_than_one() {
	for schema_version in [0_u64, 2_u64] {
		let mut fixture: Fixture = Fixture::valid();
		fixture.manifest_mut()["schema_version"] = Value::from(schema_version);
		fixture.write_manifest();

		assert_invalid_manifest(load_error(&fixture), "schema_version");
	}
}

#[test]
fn rejects_an_unexpected_repository() {
	let mut fixture: Fixture = Fixture::valid();
	fixture.manifest_mut()["repository"] = Value::from("example/NotArknightsGameData");
	fixture.write_manifest();

	assert_invalid_manifest(load_error(&fixture), "repository");
}

#[test]
fn rejects_resolved_revisions_that_are_not_exact_lowercase_hex() {
	let invalid_revisions: [String; 4] = [
		REVISION[..REVISION.len() - 1].to_string(),
		format!("{REVISION}0"),
		REVISION.to_ascii_uppercase(),
		format!("g{}", &REVISION[1..]),
	];

	for invalid_revision in invalid_revisions {
		let mut fixture: Fixture = Fixture::valid();
		fixture.manifest_mut()["resolved_revision"] = Value::from(invalid_revision);
		fixture.write_manifest();

		assert_invalid_manifest(load_error(&fixture), "resolved_revision");
	}
}

#[test]
fn rejects_entry_sha256_values_that_are_not_exact_lowercase_hex() {
	let valid_sha256: String = sha256_hex(b"{}");
	let invalid_sha256_values: [String; 4] = [
		valid_sha256[..valid_sha256.len() - 1].to_string(),
		format!("{valid_sha256}0"),
		valid_sha256.to_ascii_uppercase(),
		format!("g{}", &valid_sha256[1..]),
	];

	for invalid_sha256 in invalid_sha256_values {
		let mut fixture: Fixture = Fixture::valid();
		fixture.entry_mut(REQUIRED_ROOTS[0])["sha256"] = Value::from(invalid_sha256);
		fixture.write_manifest();

		assert_invalid_manifest(load_error(&fixture), "entries[].sha256");
	}
}

#[test]
fn rejects_unsafe_destination_and_source_paths() {
	let invalid_paths: [&str; 10] = [
		"",
		"/absolute.json",
		".",
		"..",
		"excel/./table.json",
		"excel/../table.json",
		"excel//table.json",
		"excel/table.json/",
		"excel\\table.json",
		"C:/table.json",
	];

	for field in ["destination_path", "source_path"] {
		for invalid_path in invalid_paths {
			let mut fixture: Fixture = Fixture::valid();
			fixture.entry_mut(REQUIRED_ROOTS[0])[field] = Value::from(invalid_path);
			fixture.write_manifest();

			assert_unsafe_path(load_error(&fixture), field, invalid_path);
		}
	}
}

#[test]
fn rejects_destination_duplicates_after_ascii_lowercase_normalization() {
	let mut fixture: Fixture = Fixture::valid();
	let original_destination: &str = REQUIRED_ROOTS[0];
	let duplicate_destination: String = original_destination.to_ascii_uppercase();
	let mut duplicate_entry: Value = fixture.entry_mut(original_destination).clone();
	duplicate_entry["destination_path"] = Value::from(duplicate_destination.clone());
	duplicate_entry["source_path"] = Value::from("zh_CN/gamedata/duplicate.json");
	fixture.manifest_mut()["entries"]
		.as_array_mut()
		.expect("entries is an array")
		.push(duplicate_entry);
	fixture.write_manifest();

	match load_error(&fixture) {
		ArkDataError::DuplicateDestination { first, second } => {
			let reported: [&str; 2] = [&first, &second];
			assert!(reported.contains(&original_destination));
			assert!(reported.contains(&duplicate_destination.as_str()));
		}
		other => panic!("expected DuplicateDestination, got {other:?}"),
	}
}

#[test]
fn accepts_a_mixed_case_required_root_by_normalized_identity() {
	let mut fixture: Fixture = Fixture::valid();
	let original_destination: &str = REQUIRED_ROOTS[0];
	let mixed_case_destination: &str = "ChArAcTeR_tAbLe.JsOn";
	let bytes: Vec<u8> =
		fs::read(fixture.entry_path(original_destination)).expect("read original root");
	fs::remove_file(fixture.entry_path(original_destination)).expect("remove original root");
	fs::write(fixture.entry_path(mixed_case_destination), bytes).expect("write mixed-case root");
	fixture.entry_mut(original_destination)["destination_path"] =
		Value::from(mixed_case_destination);
	fixture.write_manifest();

	ArkDataSnapshot::load(fixture.path())
		.expect("mixed-case required root should satisfy normalized identity");
}

#[test]
fn every_required_root_is_required() {
	for required_root in REQUIRED_ROOTS {
		let mut fixture: Fixture = Fixture::valid();
		fixture.remove_entry(required_root);

		match load_error(&fixture) {
			ArkDataError::MissingRequiredRoot { destination_path } => {
				assert_eq!(destination_path, required_root);
			}
			other => panic!("expected MissingRequiredRoot for {required_root}, got {other:?}"),
		}
	}
}
