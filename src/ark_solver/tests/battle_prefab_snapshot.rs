use std::{
	fs,
	path::{Path, PathBuf},
	sync::atomic::{AtomicU64, Ordering},
};

use ark_solver::data::{
	compute_mechanics_revision, ArkDataError, ArkDataSnapshot, BattlePrefabExternal,
	BattlePrefabFile, BattlePrefabManifestEntry, BattlePrefabSnapshot, BattlePrefabSource,
	BattlePrefabTooling,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod common;

const GAMEDATA_REVISION: &str = common::REVISION;
const APK_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SOURCE_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct PrefabFixture {
	root: PathBuf,
	manifest: Value,
}

impl PrefabFixture {
	fn valid() -> Self {
		let id: u64 = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
		let root: PathBuf = std::env::temp_dir().join(format!(
			"rogue_battle_prefab_snapshot_{}_{}",
			std::process::id(),
			id
		));
		fs::create_dir_all(root.join("exports")).expect("create fixture root");

		let definitions: [(&str, &str, &str, Value); 2] = [
			(
				"battle/prefabs/[uc]skills",
				"assets/AB/Android/battle/prefabs/[uc]skills.ab",
				"exports/skills.json",
				rich_export(
					"assets/AB/Android/battle/prefabs/[uc]skills.ab",
					"CAB-A",
					1,
					"SharedPrefab",
					Some((1, "CAB-B")),
				),
			),
			(
				"charpack/char_test",
				"assets/AB/Android/charpack/char_test.ab",
				"exports/char_test.json",
				rich_export(
					"assets/AB/Android/charpack/char_test.ab",
					"CAB-B",
					2,
					"SharedPrefab",
					None,
				),
			),
		];
		let entries: Vec<Value> = definitions
			.into_iter()
			.map(|(logical_path, source_apk_path, export_path, document)| {
				let bytes: Vec<u8> = serde_json::to_vec(&document).expect("serialize export");
				fs::write(root.join(export_path), &bytes).expect("write export");
				json!({
					"logical_path": logical_path,
					"bundle_kind": if logical_path.starts_with("charpack/") {
						"charpack"
					} else {
						"common"
					},
					"source_apk_path": source_apk_path,
					"source_sha256": SOURCE_SHA256,
					"source_byte_length": 4096,
					"export_path": export_path,
					"export_sha256": sha256_hex(&bytes),
					"export_byte_length": bytes.len(),
				})
			})
			.collect();
		let manifest: Value = json!({
			"schema_version": 2,
			"mechanics_revision": "0".repeat(64),
			"gamedata_revision": GAMEDATA_REVISION,
			"generated_at": "2026-08-04T00:00:00Z",
			"source": {
				"apk_sha256": APK_SHA256,
				"apk_byte_length": 8192,
			},
			"tooling": {
				"ark_ingest_version": "0.1.0",
				"exporter_version": "1",
				"python_version": "3.13.5",
				"unitypy_version": "1.23.0",
				"ark_unpacker_version": "fixture",
			},
			"entries": entries,
		});
		let mut fixture: Self = Self { root, manifest };
		fixture.refresh_revision();
		fixture.write_manifest();
		fixture
	}

	fn path(&self) -> &Path {
		&self.root
	}

	fn manifest_path(&self) -> PathBuf {
		self.root.join("manifest.json")
	}

	fn export_path(&self, index: usize) -> PathBuf {
		self.root.join(
			self.manifest["entries"][index]["export_path"]
				.as_str()
				.expect("export path string"),
		)
	}

	fn set_export(&mut self, index: usize, document: &Value) {
		let bytes: Vec<u8> = serde_json::to_vec(document).expect("serialize export");
		fs::write(self.export_path(index), &bytes).expect("write export");
		self.manifest["entries"][index]["export_sha256"] = json!(sha256_hex(&bytes));
		self.manifest["entries"][index]["export_byte_length"] = json!(bytes.len());
		self.refresh_revision();
		self.write_manifest();
	}

	fn refresh_revision(&mut self) {
		let gamedata_revision: String = self.manifest["gamedata_revision"]
			.as_str()
			.expect("gamedata revision string")
			.to_string();
		let source: BattlePrefabSource =
			serde_json::from_value(self.manifest["source"].clone()).expect("parse source");
		let tooling: BattlePrefabTooling =
			serde_json::from_value(self.manifest["tooling"].clone()).expect("parse tooling");
		let entries: Vec<BattlePrefabManifestEntry> =
			serde_json::from_value(self.manifest["entries"].clone()).expect("parse entries");
		self.manifest["mechanics_revision"] = json!(compute_mechanics_revision(
			&gamedata_revision,
			&source,
			&tooling,
			&entries,
		));
	}

	fn write_manifest(&self) {
		fs::write(
			self.manifest_path(),
			serde_json::to_vec_pretty(&self.manifest).expect("serialize manifest"),
		)
		.expect("write manifest");
	}
}

impl Drop for PrefabFixture {
	fn drop(&mut self) {
		let _result: std::io::Result<()> = fs::remove_dir_all(&self.root);
	}
}

fn rich_export(
	source_bundle: &str,
	cab_name: &str,
	path_id: i64,
	name: &str,
	external: Option<(i32, &str)>,
) -> Value {
	let externals: Vec<Value> = external
		.into_iter()
		.map(|(file_id, target)| {
			json!({
				"file_id": file_id,
				"guid": "00000000000000000000000000000000",
				"type": 0,
				"path": format!("archive:/{target}"),
				"name": target,
			})
		})
		.collect();
	json!({
		"schema_version": 1,
		"source_bundle": source_bundle,
		"source_sha256": SOURCE_SHA256,
		"bundle": {
			"signature": "UnityFS",
			"format_version": 7,
			"player_version": "2022.3.22f1",
			"engine_version": "2022.3.22f1",
		},
		"serialized_files": [{
			"cab_name": cab_name,
			"unity_version": "2022.3.22f1",
			"target_platform": 13,
			"enable_type_tree": true,
			"externals": externals,
			"types": [{
				"type_index": 0,
				"class_id": 114,
				"is_stripped_type": false,
				"script_type_index": 0,
				"script_id": "00112233445566778899aabbccddeeff",
				"old_type_hash": "ffeeddccbbaa99887766554433221100",
				"type_dependencies": [],
				"script_class": {
					"class_name": "FixtureAbility",
					"namespace": "Torappu.Battle",
					"assembly_name": "Assembly-CSharp.dll",
				},
				"schema": [{
					"index": 0,
					"level": 0,
					"type": "FixtureAbility",
					"name": "Base",
					"byte_size": -1,
					"version": 1,
					"type_flags": 0,
					"meta_flags": 0,
					"ref_type_hash": 0,
					"variable_count": 0,
					"parent_index": -1,
				}],
			}],
			"objects": [{
				"path_id": path_id,
				"class_id": 114,
				"type_index": 0,
				"unity_type": "MonoBehaviour",
				"byte_start": 128,
				"byte_size": 64,
				"script_class": {
					"class_name": "FixtureAbility",
					"namespace": "Torappu.Battle",
					"assembly_name": "Assembly-CSharp.dll",
				},
				"fields": {
					"m_Name": name,
					"m_Value": 12.5,
				},
			}],
		}],
	})
}

fn sha256_hex(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

fn load_error(fixture: &PrefabFixture) -> ArkDataError {
	BattlePrefabSnapshot::load(fixture.path()).expect_err("invalid snapshot should fail")
}

#[test]
fn loads_rich_exports_and_resolves_local_and_external_objects() {
	let fixture: PrefabFixture = PrefabFixture::valid();
	let snapshot: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(fixture.path()).expect("load valid fixture");

	assert_eq!(snapshot.mechanics_revision().len(), 64);
	assert_eq!(snapshot.gamedata_revision(), GAMEDATA_REVISION);
	assert_eq!(snapshot.bundles().len(), 2);
	assert!(snapshot.bundle("BATTLE/PREFABS/[UC]SKILLS").is_some());
	let local = snapshot
		.object("cab-a", 0, 1)
		.expect("resolve local object");
	assert_eq!(local.object().prefab_name(), Some("SharedPrefab"));
	assert_eq!(
		local.serialized_type().script_id(),
		Some(&[
			0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
			0xee, 0xff,
		])
	);
	let external = snapshot
		.object("CAB-A", 1, 2)
		.expect("resolve external object");
	assert_eq!(external.file().cab_name, "CAB-B");
	assert_eq!(external.object().path_id, 2);
	assert_eq!(snapshot.find_prefab("SharedPrefab").len(), 2);
}

#[test]
fn reports_the_source_reference_when_object_resolution_fails() {
	let fixture: PrefabFixture = PrefabFixture::valid();
	let snapshot: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(fixture.path()).expect("load valid fixture");

	assert!(matches!(
		snapshot.object("CAB-A", 7, 99),
		Err(ArkDataError::BattlePrefabObjectResolution {
			ref source_cab,
			file_id: 7,
			path_id: 99,
			..
		}) if source_cab == "CAB-A"
	));
}

#[test]
fn looks_up_external_entries_by_file_id_and_names_their_target_cab() {
	let mut fixture: PrefabFixture = PrefabFixture::valid();
	let snapshot: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(fixture.path()).expect("load valid fixture");
	let file: &BattlePrefabFile = snapshot.file("CAB-A").expect("source CAB is pinned");

	let external: &BattlePrefabExternal = file.external(1).expect("fileID 1 is declared");
	assert_eq!(external.target_cab_name(), Some("CAB-B"));
	assert!(file.external(9).is_none());
	drop(snapshot);

	// An empty name falls back to the last path segment.
	let mut document: Value =
		serde_json::from_slice(&fs::read(fixture.export_path(0)).expect("read export"))
			.expect("parse export");
	document["serialized_files"][0]["externals"][0]["name"] = json!("");
	fixture.set_export(0, &document);
	let snapshot: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(fixture.path()).expect("load path-only external");
	let file: &BattlePrefabFile = snapshot.file("CAB-A").expect("source CAB is pinned");
	assert_eq!(
		file.external(1)
			.and_then(BattlePrefabExternal::target_cab_name),
		Some("CAB-B")
	);
}

#[test]
fn permits_unresolved_external_metadata_until_the_pptr_is_requested() {
	let mut fixture: PrefabFixture = PrefabFixture::valid();
	let mut document: Value =
		serde_json::from_slice(&fs::read(fixture.export_path(0)).expect("read export"))
			.expect("parse export");
	document["serialized_files"][0]["externals"][0]["path"] =
		json!("archive:/CAB-VISUAL-NOT-SELECTED");
	document["serialized_files"][0]["externals"][0]["name"] = json!("CAB-VISUAL-NOT-SELECTED");
	fixture.set_export(0, &document);

	let snapshot: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(fixture.path()).expect("unused external may remain unresolved");

	assert!(matches!(
		snapshot.object("CAB-A", 1, 2),
		Err(ArkDataError::BattlePrefabObjectResolution { ref reason, .. })
			if reason.contains("target CAB \"CAB-VISUAL-NOT-SELECTED\" is absent")
	));
}

#[test]
fn validates_gamedata_compatibility() {
	let fixture: PrefabFixture = PrefabFixture::valid();
	let snapshot: BattlePrefabSnapshot =
		BattlePrefabSnapshot::load(fixture.path()).expect("load valid fixture");
	let gamedata_fixture: common::Fixture = common::Fixture::valid();
	let gamedata: ArkDataSnapshot =
		ArkDataSnapshot::load(gamedata_fixture.path()).expect("load gamedata fixture");

	snapshot
		.ensure_compatible_with(&gamedata)
		.expect("matching gamedata revision");
}

#[test]
fn rejects_noncanonical_mechanics_revision() {
	let mut fixture: PrefabFixture = PrefabFixture::valid();
	fixture.manifest["mechanics_revision"] = json!("1".repeat(64));
	fixture.write_manifest();

	assert!(matches!(
		load_error(&fixture),
		ArkDataError::MechanicsRevisionMismatch { .. }
	));
}

#[test]
fn rejects_unsorted_and_case_insensitive_duplicate_entries() {
	let mut unsorted: PrefabFixture = PrefabFixture::valid();
	unsorted.manifest["entries"]
		.as_array_mut()
		.expect("entries array")
		.reverse();
	unsorted.write_manifest();
	assert!(matches!(
		load_error(&unsorted),
		ArkDataError::InvalidBattlePrefabManifest {
			field: "entries",
			..
		}
	));

	let mut duplicate: PrefabFixture = PrefabFixture::valid();
	duplicate.manifest["entries"][0]["logical_path"] = json!("BATTLE/PREFABS/[UC]SKILLS");
	duplicate.manifest["entries"][1]["logical_path"] = json!("battle/prefabs/[uc]skills");
	duplicate.write_manifest();
	assert!(matches!(
		load_error(&duplicate),
		ArkDataError::DuplicateBattlePrefabBundle { .. }
	));
}

#[test]
fn rejects_path_traversal_missing_files_and_integrity_mismatches() {
	let mut traversal: PrefabFixture = PrefabFixture::valid();
	traversal.manifest["entries"][0]["export_path"] = json!("../outside.json");
	traversal.write_manifest();
	assert!(matches!(
		load_error(&traversal),
		ArkDataError::UnsafePath {
			field: "entries[].export_path",
			..
		}
	));

	let missing: PrefabFixture = PrefabFixture::valid();
	fs::remove_file(missing.export_path(0)).expect("remove export");
	assert!(matches!(
		load_error(&missing),
		ArkDataError::EntryRead { .. }
	));

	let length: PrefabFixture = PrefabFixture::valid();
	fs::write(length.export_path(0), b"x").expect("replace export");
	assert!(matches!(
		load_error(&length),
		ArkDataError::ByteLengthMismatch { .. }
	));

	let hash: PrefabFixture = PrefabFixture::valid();
	let mut bytes: Vec<u8> = fs::read(hash.export_path(0)).expect("read export");
	bytes[0] = if bytes[0] == b'{' { b'[' } else { b'{' };
	fs::write(hash.export_path(0), bytes).expect("tamper export");
	assert!(matches!(
		load_error(&hash),
		ArkDataError::Sha256Mismatch { .. }
	));
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_roots_manifests_entries_and_entry_parents() {
	use std::os::unix::fs::symlink;

	let real_root: PrefabFixture = PrefabFixture::valid();
	let linked_root: PathBuf = real_root.path().with_extension("linked");
	symlink(real_root.path(), &linked_root).expect("link root");
	assert!(matches!(
		BattlePrefabSnapshot::load(&linked_root),
		Err(ArkDataError::InvalidSnapshotRoot { .. })
	));
	fs::remove_file(&linked_root).expect("remove root link");

	let manifest: PrefabFixture = PrefabFixture::valid();
	let real_manifest: PathBuf = manifest.root.join("real-manifest.json");
	fs::rename(manifest.manifest_path(), &real_manifest).expect("move manifest");
	symlink(&real_manifest, manifest.manifest_path()).expect("link manifest");
	assert!(matches!(
		load_error(&manifest),
		ArkDataError::ManifestNotRegular { .. }
	));

	let entry: PrefabFixture = PrefabFixture::valid();
	let real_entry: PathBuf = entry.root.join("real-export.json");
	fs::rename(entry.export_path(0), &real_entry).expect("move export");
	symlink(&real_entry, entry.export_path(0)).expect("link export");
	assert!(matches!(
		load_error(&entry),
		ArkDataError::EntryNotRegular { .. }
	));

	let parent: PrefabFixture = PrefabFixture::valid();
	let exports: PathBuf = parent.root.join("exports");
	let real_exports: PathBuf = parent.root.join("real-exports");
	fs::rename(&exports, &real_exports).expect("move exports");
	symlink(&real_exports, &exports).expect("link export parent");
	assert!(matches!(
		load_error(&parent),
		ArkDataError::EntryNotRegular { .. }
	));
}

#[test]
fn rejects_duplicate_cabs_and_invalid_type_references() {
	let mut duplicate: PrefabFixture = PrefabFixture::valid();
	let second_source: String = duplicate.manifest["entries"][1]["source_apk_path"]
		.as_str()
		.expect("source path")
		.to_string();
	let duplicate_document: Value = rich_export(&second_source, "cab-a", 2, "OtherPrefab", None);
	duplicate.set_export(1, &duplicate_document);
	assert!(matches!(
		load_error(&duplicate),
		ArkDataError::DuplicateBattlePrefabCab { .. }
	));

	let mut invalid_type: PrefabFixture = PrefabFixture::valid();
	let mut document: Value =
		serde_json::from_slice(&fs::read(invalid_type.export_path(0)).expect("read export"))
			.expect("parse export");
	document["serialized_files"][0]["objects"][0]["type_index"] = json!(9);
	invalid_type.set_export(0, &document);
	assert!(matches!(
		load_error(&invalid_type),
		ArkDataError::InvalidBattlePrefabEntry { .. }
	));
}
