use std::{fs, path::PathBuf};

use ark_solver::data::{ArkDataError, ArkDataSnapshot};

mod common;

use common::{Fixture, LEVEL_PATH};

fn load_error(fixture: &Fixture) -> ArkDataError {
	ArkDataSnapshot::load(fixture.path()).expect_err("invalid snapshot should fail")
}

fn same_length_mutation(bytes: &[u8]) -> Vec<u8> {
	let mut mutated: Vec<u8> = bytes.to_vec();
	let first: &mut u8 = mutated.first_mut().expect("fixture entry is nonempty");
	*first = if *first == b'[' { b'{' } else { b'[' };
	mutated
}

#[test]
fn rejects_a_missing_manifest_entry_file() {
	let fixture: Fixture = Fixture::valid();
	fs::remove_file(fixture.entry_path(LEVEL_PATH)).expect("remove level entry");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::EntryRead {
			ref destination_path,
			..
		} if destination_path == LEVEL_PATH
	));
}

#[test]
fn rejects_a_directory_instead_of_an_entry_file() {
	let fixture: Fixture = Fixture::valid();
	let destination_path: &str = "skill_table.json";
	let path: PathBuf = fixture.entry_path(destination_path);
	fs::remove_file(&path).expect("remove entry file");
	fs::create_dir(&path).expect("create entry directory");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::EntryNotRegular {
			destination_path: ref actual,
		} if actual == destination_path
	));
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_instead_of_an_entry_file() {
	use std::os::unix::fs::symlink;

	let fixture: Fixture = Fixture::valid();
	let destination_path: &str = "stage_table.json";
	let path: PathBuf = fixture.entry_path(destination_path);
	fs::remove_file(&path).expect("remove entry file");
	symlink(fixture.entry_path("character_table.json"), &path).expect("create entry symlink");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::EntryNotRegular {
			destination_path: ref actual,
		} if actual == destination_path
	));
}

#[cfg(unix)]
#[test]
fn rejects_a_symlink_in_an_entry_parent_path() {
	use std::os::unix::fs::symlink;

	let fixture: Fixture = Fixture::valid();
	let levels: PathBuf = fixture.entry_path("levels");
	let real_levels: PathBuf = fixture.entry_path("real_levels");
	fs::rename(&levels, &real_levels).expect("move real levels directory");
	symlink(&real_levels, &levels).expect("create parent symlink");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::EntryNotRegular {
			ref destination_path,
		} if destination_path == LEVEL_PATH
	));
}

#[cfg(unix)]
#[test]
fn rejects_a_symlinked_manifest() {
	use std::os::unix::fs::symlink;

	let fixture: Fixture = Fixture::valid();
	let manifest_path: PathBuf = fixture.manifest_path();
	let real_manifest_path: PathBuf = fixture.path().join("real-manifest.json");
	fs::rename(&manifest_path, &real_manifest_path).expect("move real manifest");
	symlink(&real_manifest_path, &manifest_path).expect("create manifest symlink");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::ManifestNotRegular { .. }
	));
}

#[test]
fn rejects_an_entry_with_the_wrong_byte_length() {
	let fixture: Fixture = Fixture::valid();
	let destination_path: &str = "range_table.json";
	let expected: u64 = fs::metadata(fixture.entry_path(destination_path))
		.expect("read original entry metadata")
		.len();
	fs::write(
		fixture.entry_path(destination_path),
		vec![b' '; expected as usize + 1],
	)
	.expect("tamper entry length");

	let error: ArkDataError = load_error(&fixture);
	assert!(
		matches!(
			error,
			ArkDataError::ByteLengthMismatch {
				destination_path: ref actual_path,
				expected: actual_expected,
				actual,
			} if actual_path == destination_path
				&& actual_expected == expected
				&& actual == expected + 1
		),
		"received {error:?}"
	);
}

#[test]
fn rejects_same_length_sha256_tampering() {
	let fixture: Fixture = Fixture::valid();
	let destination_path: &str = "favor_table.json";
	fs::write(fixture.entry_path(destination_path), b"[]").expect("tamper entry digest");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::Sha256Mismatch {
			destination_path: ref actual,
			..
		} if actual == destination_path
	));
}

#[test]
fn verifies_non_root_level_entries() {
	let fixture: Fixture = Fixture::valid();
	let original: Vec<u8> = fs::read(fixture.entry_path(LEVEL_PATH)).expect("read level bytes");
	fs::write(
		fixture.entry_path(LEVEL_PATH),
		same_length_mutation(&original),
	)
	.expect("tamper level digest");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::Sha256Mismatch {
			ref destination_path,
			..
		} if destination_path == LEVEL_PATH
	));
}

#[test]
fn verifies_every_entry_before_parsing_any_root() {
	let mut fixture: Fixture = Fixture::valid();
	let original_level: Vec<u8> =
		fs::read(fixture.entry_path(LEVEL_PATH)).expect("read level bytes");
	fixture.set_verified_entry_bytes("character_table.json", b"{");
	fs::write(
		fixture.entry_path(LEVEL_PATH),
		same_length_mutation(&original_level),
	)
	.expect("tamper later level entry");

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::Sha256Mismatch {
			ref destination_path,
			..
		} if destination_path == LEVEL_PATH
	));

	fixture.set_verified_entry_bytes(LEVEL_PATH, &original_level);
	assert!(matches!(
		load_error(&fixture),
		ArkDataError::RootParse {
			ref destination_path,
			..
		} if destination_path == "character_table.json"
	));
}
