use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	io::BufReader,
	path::{absolute, Path, PathBuf},
};

use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::data::{
	manifest::{open_verified_entry, validate_relative_posix_path, VerifiedEntryReader},
	ArkDataError, ArkDataSnapshot,
};

const MANIFEST_FILE_NAME: &str = "manifest.json";
const MANIFEST_SCHEMA_VERSION: u32 = 2;
const BUNDLE_SCHEMA_VERSION: u32 = 1;
const GAMEDATA_REVISION_HEX_LENGTH: usize = 40;
const SHA256_HEX_LENGTH: usize = 64;
const HASH128_HEX_LENGTH: usize = 32;

/// Provenance of the APK used to produce a battle-prefab snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabSource {
	pub apk_sha256: String,
	pub apk_byte_length: u64,
}

/// Versions of every tool whose output contributes to a battle-prefab snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabTooling {
	pub ark_ingest_version: String,
	pub exporter_version: String,
	pub python_version: String,
	pub unitypy_version: String,
	pub ark_unpacker_version: String,
}

/// Kind of Unity bundle represented by one rich TypeTree export.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BattlePrefabEntryKind {
	Common,
	Charpack,
	/// One `enm_pfb_*` bundle of enemy battle prefabs.
	Enemypack,
}

/// Canonical identity and integrity record for one rich TypeTree export.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabManifestEntry {
	pub logical_path: String,
	pub bundle_kind: BattlePrefabEntryKind,
	pub source_apk_path: String,
	pub source_sha256: String,
	pub source_byte_length: u64,
	pub export_path: String,
	pub export_sha256: String,
	pub export_byte_length: u64,
}

/// Computes the canonical revision used by both snapshot writers and readers.
///
/// The digest is SHA-256 over compact JSON containing, in order, `schema_version`,
/// `gamedata_revision`, `source`, `tooling`, and entries sorted bytewise by logical and export
/// path. Timestamps are deliberately excluded.
pub fn compute_mechanics_revision(
	gamedata_revision: &str,
	source: &BattlePrefabSource,
	tooling: &BattlePrefabTooling,
	entries: &[BattlePrefabManifestEntry],
) -> String {
	#[derive(Serialize)]
	struct CanonicalIdentity<'a> {
		schema_version: u32,
		gamedata_revision: &'a str,
		source: &'a BattlePrefabSource,
		tooling: &'a BattlePrefabTooling,
		entries: Vec<&'a BattlePrefabManifestEntry>,
	}

	let mut canonical_entries: Vec<&BattlePrefabManifestEntry> = entries.iter().collect();
	canonical_entries.sort_by(|left, right| {
		(&left.logical_path, &left.export_path).cmp(&(&right.logical_path, &right.export_path))
	});
	let identity = CanonicalIdentity {
		schema_version: MANIFEST_SCHEMA_VERSION,
		gamedata_revision,
		source,
		tooling,
		entries: canonical_entries,
	};
	let bytes: Vec<u8> = serde_json::to_vec(&identity)
		.expect("canonical battle-prefab identity contains only JSON-safe values");
	format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocument {
	schema_version: u32,
	mechanics_revision: String,
	gamedata_revision: String,
	generated_at: String,
	source: BattlePrefabSource,
	tooling: BattlePrefabTooling,
	entries: Vec<BattlePrefabManifestEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BattlePrefabDocument {
	schema_version: u32,
	source_bundle: String,
	source_sha256: String,
	bundle: BattlePrefabBundleMetadata,
	serialized_files: Vec<BattlePrefabFile>,
}

/// Unity asset-bundle header data retained for diagnostics and drift detection.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabBundleMetadata {
	pub signature: String,
	pub format_version: u32,
	pub player_version: String,
	pub engine_version: String,
}

/// One resolved IL2CPP script class.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabScriptClass {
	pub class_name: String,
	pub namespace: String,
	pub assembly_name: String,
}

/// One node in a Unity serialized type schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabSchemaNode {
	pub index: i32,
	pub level: u32,
	#[serde(rename = "type")]
	pub type_name: String,
	pub name: String,
	pub byte_size: i32,
	pub version: i32,
	pub type_flags: u32,
	pub meta_flags: u32,
	pub ref_type_hash: u64,
	pub variable_count: Option<u32>,
	pub parent_index: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FixedHash128([u8; 16]);

impl FixedHash128 {
	const fn as_bytes(&self) -> &[u8; 16] {
		&self.0
	}
}

impl<'de> Deserialize<'de> for FixedHash128 {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let value: String = String::deserialize(deserializer)?;
		if !is_exact_lowercase_hex(&value, HASH128_HEX_LENGTH) {
			return Err(de::Error::custom(
				"expected exactly 32 lowercase hexadecimal characters",
			));
		}
		let mut bytes: [u8; 16] = [0; 16];
		for (index, byte) in bytes.iter_mut().enumerate() {
			let offset: usize = index * 2;
			*byte =
				u8::from_str_radix(&value[offset..offset + 2], 16).map_err(de::Error::custom)?;
		}
		Ok(Self(bytes))
	}
}

/// One Unity serialized type, including its complete TypeTree schema.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabSerializedType {
	pub type_index: i32,
	pub class_id: i32,
	pub is_stripped_type: bool,
	pub script_type_index: i32,
	script_id: Option<FixedHash128>,
	old_type_hash: Option<FixedHash128>,
	pub type_dependencies: Vec<i32>,
	pub script_class: Option<BattlePrefabScriptClass>,
	pub schema: Vec<BattlePrefabSchemaNode>,
}

impl BattlePrefabSerializedType {
	/// Returns the exact 16-byte Unity script ID, when the serialized type has one.
	pub fn script_id(&self) -> Option<&[u8; 16]> {
		self.script_id.as_ref().map(FixedHash128::as_bytes)
	}

	/// Returns the exact 16-byte Unity old-type hash, when present.
	pub fn old_type_hash(&self) -> Option<&[u8; 16]> {
		self.old_type_hash.as_ref().map(FixedHash128::as_bytes)
	}
}

/// One external serialized-file reference. `file_id` follows Unity PPtr numbering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabExternal {
	pub file_id: i32,
	pub guid: String,
	#[serde(rename = "type")]
	pub external_type: i32,
	pub path: String,
	pub name: String,
}

impl BattlePrefabExternal {
	/// The serialized file this entry points at: its name, or else the last segment of its path.
	pub fn target_cab_name(&self) -> Option<&str> {
		if !self.name.is_empty() {
			return Some(&self.name);
		}
		self.path.rsplit('/').next().filter(|name| !name.is_empty())
	}
}

/// A Unity PPtr-like object reference relative to a source serialized file.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BattlePrefabObjectRef {
	pub file_id: i32,
	pub path_id: i64,
}

/// One losslessly decoded serialized object.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabObject {
	pub path_id: i64,
	pub class_id: i32,
	pub type_index: i32,
	pub unity_type: String,
	pub byte_start: u64,
	pub byte_size: u64,
	pub script_class: Option<BattlePrefabScriptClass>,
	pub fields: Value,
}

impl BattlePrefabObject {
	/// Returns Unity's conventional `m_Name` field when it is a string.
	pub fn prefab_name(&self) -> Option<&str> {
		self.fields.get("m_Name").and_then(Value::as_str)
	}
}

/// One serialized file contained by an asset bundle.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BattlePrefabFile {
	pub cab_name: String,
	pub unity_version: String,
	pub target_platform: i32,
	pub enable_type_tree: bool,
	pub externals: Vec<BattlePrefabExternal>,
	pub types: Vec<BattlePrefabSerializedType>,
	pub objects: Vec<BattlePrefabObject>,
}

impl BattlePrefabFile {
	/// Resolves a serialized type by the stable index recorded on an object.
	pub fn serialized_type(&self, type_index: i32) -> Option<&BattlePrefabSerializedType> {
		self.types
			.binary_search_by_key(&type_index, |serialized_type| serialized_type.type_index)
			.ok()
			.map(|index| &self.types[index])
	}

	/// Resolves one external-table entry by its Unity PPtr file ID.
	pub fn external(&self, file_id: i32) -> Option<&BattlePrefabExternal> {
		self.externals
			.binary_search_by_key(&file_id, |external| external.file_id)
			.ok()
			.map(|index| &self.externals[index])
	}
}

/// A verified rich TypeTree export for one Unity asset bundle.
#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabBundle {
	logical_path: String,
	source_bundle: String,
	source_sha256: String,
	metadata: BattlePrefabBundleMetadata,
	serialized_files: Vec<BattlePrefabFile>,
}

impl BattlePrefabBundle {
	pub fn logical_path(&self) -> &str {
		&self.logical_path
	}

	pub fn source_bundle(&self) -> &str {
		&self.source_bundle
	}

	pub fn source_sha256(&self) -> &str {
		&self.source_sha256
	}

	pub const fn metadata(&self) -> &BattlePrefabBundleMetadata {
		&self.metadata
	}

	pub fn serialized_files(&self) -> &[BattlePrefabFile] {
		&self.serialized_files
	}
}

/// Borrowed location of an object and the type metadata needed to interpret it.
#[derive(Clone, Copy, Debug)]
pub struct BattlePrefabObjectHandle<'a> {
	bundle: &'a BattlePrefabBundle,
	file: &'a BattlePrefabFile,
	object: &'a BattlePrefabObject,
}

impl<'a> BattlePrefabObjectHandle<'a> {
	pub const fn bundle(&self) -> &'a BattlePrefabBundle {
		self.bundle
	}

	pub const fn file(&self) -> &'a BattlePrefabFile {
		self.file
	}

	pub const fn object(&self) -> &'a BattlePrefabObject {
		self.object
	}

	pub fn serialized_type(&self) -> &'a BattlePrefabSerializedType {
		self.file
			.serialized_type(self.object.type_index)
			.expect("verified object type index remains present")
	}
}

#[derive(Clone, Debug)]
struct FileLocation {
	bundle_key: String,
	file_index: usize,
}

/// A fully integrity-checked collection of rich official battle-prefab TypeTrees.
#[derive(Debug)]
pub struct BattlePrefabSnapshot {
	root: PathBuf,
	mechanics_revision: String,
	gamedata_revision: String,
	source: BattlePrefabSource,
	tooling: BattlePrefabTooling,
	entries: Vec<BattlePrefabManifestEntry>,
	bundles: BTreeMap<String, BattlePrefabBundle>,
	files_by_cab: BTreeMap<String, FileLocation>,
}

impl BattlePrefabSnapshot {
	/// Loads every rich TypeTree document through the same stream that verifies its integrity.
	pub fn load(path: &Path) -> Result<Self, ArkDataError> {
		let root: PathBuf = validate_snapshot_root(path)?;
		let manifest: ManifestDocument = read_manifest(&root.join(MANIFEST_FILE_NAME))?;
		validate_manifest(&manifest)?;
		let expected_revision: String = compute_mechanics_revision(
			&manifest.gamedata_revision,
			&manifest.source,
			&manifest.tooling,
			&manifest.entries,
		);
		if expected_revision != manifest.mechanics_revision {
			return Err(ArkDataError::MechanicsRevisionMismatch {
				expected: expected_revision,
				actual: manifest.mechanics_revision,
			});
		}

		let mut bundles: BTreeMap<String, BattlePrefabBundle> = BTreeMap::new();
		let mut files_by_cab: BTreeMap<String, FileLocation> = BTreeMap::new();
		for entry in &manifest.entries {
			let mut reader: VerifiedEntryReader = open_verified_entry(
				&root,
				&entry.export_path,
				entry.export_byte_length,
				&entry.export_sha256,
			)?;
			let document: Result<BattlePrefabDocument, serde_json::Error> = {
				let buffered: BufReader<&mut VerifiedEntryReader> = BufReader::new(&mut reader);
				serde_json::from_reader(buffered)
			};
			// Prefer an integrity failure over a parser failure: malformed tampering must still be
			// diagnosed against the manifest, and `finish` verifies the exact descriptor parsed above.
			reader.finish()?;
			let document: BattlePrefabDocument =
				document.map_err(|source| ArkDataError::BattlePrefabEntryParse {
					destination_path: entry.export_path.clone(),
					source,
				})?;
			let bundle: BattlePrefabBundle = compile_bundle(entry, document)?;
			let normalized_bundle: String = entry.logical_path.to_ascii_lowercase();
			if let Some(first) = bundles.get(&normalized_bundle) {
				return Err(ArkDataError::DuplicateBattlePrefabBundle {
					first: first.logical_path.clone(),
					second: entry.logical_path.clone(),
				});
			}
			for (file_index, file) in bundle.serialized_files.iter().enumerate() {
				let normalized_cab: String = file.cab_name.to_ascii_lowercase();
				if let Some(first_location) = files_by_cab.get(&normalized_cab) {
					let first: &str = &bundles[&first_location.bundle_key].serialized_files
						[first_location.file_index]
						.cab_name;
					return Err(ArkDataError::DuplicateBattlePrefabCab {
						first: first.to_string(),
						second: file.cab_name.clone(),
					});
				}
				files_by_cab.insert(
					normalized_cab,
					FileLocation {
						bundle_key: normalized_bundle.clone(),
						file_index,
					},
				);
			}
			bundles.insert(normalized_bundle, bundle);
		}
		Ok(Self {
			root,
			mechanics_revision: manifest.mechanics_revision,
			gamedata_revision: manifest.gamedata_revision,
			source: manifest.source,
			tooling: manifest.tooling,
			entries: manifest.entries,
			bundles,
			files_by_cab,
		})
	}

	pub fn mechanics_revision(&self) -> &str {
		&self.mechanics_revision
	}

	pub fn gamedata_revision(&self) -> &str {
		&self.gamedata_revision
	}

	pub const fn source(&self) -> &BattlePrefabSource {
		&self.source
	}

	pub const fn tooling(&self) -> &BattlePrefabTooling {
		&self.tooling
	}

	pub fn entries(&self) -> &[BattlePrefabManifestEntry] {
		&self.entries
	}

	pub fn root(&self) -> &Path {
		&self.root
	}

	pub fn ensure_compatible_with(&self, gamedata: &ArkDataSnapshot) -> Result<(), ArkDataError> {
		if self.gamedata_revision == gamedata.resolved_revision() {
			return Ok(());
		}
		Err(ArkDataError::BattlePrefabGamedataMismatch {
			snapshot_gamedata_revision: self.gamedata_revision.clone(),
			gamedata_revision: gamedata.resolved_revision().to_string(),
		})
	}

	pub fn bundles(&self) -> impl ExactSizeIterator<Item = &BattlePrefabBundle> {
		self.bundles.values()
	}

	pub fn bundle(&self, logical_path: &str) -> Option<&BattlePrefabBundle> {
		self.bundles.get(&logical_path.to_ascii_lowercase())
	}

	pub fn file(&self, cab_name: &str) -> Option<&BattlePrefabFile> {
		self.file_at(cab_name).map(|(_, file)| file)
	}

	/// Resolves a local or external PPtr from the external table of `source_cab`.
	pub fn object(
		&self,
		source_cab: &str,
		file_id: i32,
		path_id: i64,
	) -> Result<BattlePrefabObjectHandle<'_>, ArkDataError> {
		let (source_bundle, source_file): (&BattlePrefabBundle, &BattlePrefabFile) =
			self.file_at(source_cab).ok_or_else(|| {
				self.object_resolution_error(source_cab, file_id, path_id, "source CAB is absent")
			})?;
		let (target_bundle, target_file): (&BattlePrefabBundle, &BattlePrefabFile) = if file_id == 0
		{
			(source_bundle, source_file)
		} else {
			let external: &BattlePrefabExternal =
				source_file.external(file_id).ok_or_else(|| {
					self.object_resolution_error(
						source_cab,
						file_id,
						path_id,
						"fileID is absent from the source external table",
					)
				})?;
			let target_cab: &str = external.target_cab_name().ok_or_else(|| {
				self.object_resolution_error(
					source_cab,
					file_id,
					path_id,
					"external has no target CAB name",
				)
			})?;
			self.file_at(target_cab).ok_or_else(|| {
				self.object_resolution_error(
					source_cab,
					file_id,
					path_id,
					format!("target CAB {target_cab:?} is absent"),
				)
			})?
		};
		let object: &BattlePrefabObject = target_file
			.objects
			.binary_search_by_key(&path_id, |object| object.path_id)
			.ok()
			.map(|index| &target_file.objects[index])
			.ok_or_else(|| {
				self.object_resolution_error(
					source_cab,
					file_id,
					path_id,
					format!("target CAB {:?} has no such object", target_file.cab_name),
				)
			})?;
		Ok(BattlePrefabObjectHandle {
			bundle: target_bundle,
			file: target_file,
			object,
		})
	}

	/// Returns all exact `m_Name` matches; callers decide whether multiple matches are ambiguous.
	pub fn find_prefab(&self, name: &str) -> Vec<BattlePrefabObjectHandle<'_>> {
		let mut matches: Vec<BattlePrefabObjectHandle<'_>> = Vec::new();
		for bundle in self.bundles.values() {
			for file in &bundle.serialized_files {
				for object in &file.objects {
					if object.prefab_name() == Some(name) {
						matches.push(BattlePrefabObjectHandle {
							bundle,
							file,
							object,
						});
					}
				}
			}
		}
		matches
	}

	fn file_at(&self, cab_name: &str) -> Option<(&BattlePrefabBundle, &BattlePrefabFile)> {
		let location: &FileLocation = self.files_by_cab.get(&cab_name.to_ascii_lowercase())?;
		let bundle: &BattlePrefabBundle = self.bundles.get(&location.bundle_key)?;
		Some((bundle, &bundle.serialized_files[location.file_index]))
	}

	fn object_resolution_error(
		&self,
		source_cab: &str,
		file_id: i32,
		path_id: i64,
		reason: impl Into<String>,
	) -> ArkDataError {
		ArkDataError::BattlePrefabObjectResolution {
			source_cab: source_cab.to_string(),
			file_id,
			path_id,
			reason: reason.into(),
		}
	}
}

fn compile_bundle(
	entry: &BattlePrefabManifestEntry,
	document: BattlePrefabDocument,
) -> Result<BattlePrefabBundle, ArkDataError> {
	let invalid = |reason: String| ArkDataError::InvalidBattlePrefabEntry {
		destination_path: entry.export_path.clone(),
		reason,
	};
	if document.schema_version != BUNDLE_SCHEMA_VERSION {
		return Err(invalid(format!(
			"expected schema_version {BUNDLE_SCHEMA_VERSION}, found {}",
			document.schema_version
		)));
	}
	if document.source_bundle != entry.source_apk_path {
		return Err(invalid(format!(
			"source_bundle {:?} does not match manifest source_apk_path {:?}",
			document.source_bundle, entry.source_apk_path
		)));
	}
	if document.source_sha256 != entry.source_sha256 {
		return Err(invalid(format!(
			"source_sha256 {} does not match manifest value {}",
			document.source_sha256, entry.source_sha256
		)));
	}
	if document.serialized_files.is_empty() {
		return Err(invalid("serialized_files is empty".to_string()));
	}
	ensure_sorted_unique_by(
		&document.serialized_files,
		|file| file.cab_name.as_str(),
		"serialized_files[].cab_name",
		&invalid,
	)?;
	for file in &document.serialized_files {
		validate_file(file, &invalid)?;
	}

	Ok(BattlePrefabBundle {
		logical_path: entry.logical_path.clone(),
		source_bundle: document.source_bundle,
		source_sha256: document.source_sha256,
		metadata: document.bundle,
		serialized_files: document.serialized_files,
	})
}

fn validate_file(
	file: &BattlePrefabFile,
	invalid: &impl Fn(String) -> ArkDataError,
) -> Result<(), ArkDataError> {
	if file.cab_name.is_empty() {
		return Err(invalid("serialized_files[].cab_name is empty".to_string()));
	}
	ensure_sorted_unique_by_key(
		&file.externals,
		|external| external.file_id,
		"serialized_files[].externals[].file_id",
		invalid,
	)?;
	if file.externals.iter().any(|external| external.file_id <= 0) {
		return Err(invalid(
			"serialized_files[].externals[].file_id must be positive".to_string(),
		));
	}
	ensure_sorted_unique_by_key(
		&file.types,
		|serialized_type| serialized_type.type_index,
		"serialized_files[].types[].type_index",
		invalid,
	)?;
	ensure_sorted_unique_by_key(
		&file.objects,
		|object| object.path_id,
		"serialized_files[].objects[].path_id",
		invalid,
	)?;
	for object in &file.objects {
		if !object.fields.is_object() {
			return Err(invalid(format!(
				"CAB {:?} pathID {} fields is not an object",
				file.cab_name, object.path_id
			)));
		}
		let serialized_type: &BattlePrefabSerializedType =
			file.serialized_type(object.type_index).ok_or_else(|| {
				invalid(format!(
					"CAB {:?} pathID {} references missing type_index {}",
					file.cab_name, object.path_id, object.type_index
				))
			})?;
		if serialized_type.class_id != object.class_id {
			return Err(invalid(format!(
				"CAB {:?} pathID {} class_id {} disagrees with type_index {} class_id {}",
				file.cab_name,
				object.path_id,
				object.class_id,
				object.type_index,
				serialized_type.class_id
			)));
		}
	}
	Ok(())
}

fn ensure_sorted_unique_by<T>(
	items: &[T],
	key: impl Fn(&T) -> &str,
	field: &'static str,
	invalid: &impl Fn(String) -> ArkDataError,
) -> Result<(), ArkDataError> {
	let mut previous: Option<&str> = None;
	let mut normalized: BTreeSet<String> = BTreeSet::new();
	for item in items {
		let current: &str = key(item);
		if previous.is_some_and(|previous| previous > current) {
			return Err(invalid(format!("{field} is not bytewise sorted")));
		}
		if !normalized.insert(current.to_ascii_lowercase()) {
			return Err(invalid(format!("{field} contains duplicate {current:?}")));
		}
		previous = Some(current);
	}
	Ok(())
}

fn ensure_sorted_unique_by_key<T, K: Copy + Ord + std::fmt::Display>(
	items: &[T],
	key: impl Fn(&T) -> K,
	field: &'static str,
	invalid: &impl Fn(String) -> ArkDataError,
) -> Result<(), ArkDataError> {
	let mut previous: Option<K> = None;
	for item in items {
		let current: K = key(item);
		if previous.is_some_and(|previous| previous >= current) {
			return Err(invalid(format!(
				"{field} must be strictly increasing; found {current}"
			)));
		}
		previous = Some(current);
	}
	Ok(())
}

fn validate_snapshot_root(root: &Path) -> Result<PathBuf, ArkDataError> {
	let metadata: fs::Metadata = fs::symlink_metadata(root)
		.map_err(|_| ArkDataError::InvalidSnapshotRoot { path: root.into() })?;
	if !metadata.file_type().is_dir() {
		return Err(ArkDataError::InvalidSnapshotRoot { path: root.into() });
	}
	absolute(root).map_err(|_| ArkDataError::InvalidSnapshotRoot { path: root.into() })
}

fn read_manifest(path: &Path) -> Result<ManifestDocument, ArkDataError> {
	let metadata: fs::Metadata =
		fs::symlink_metadata(path).map_err(|source| ArkDataError::ManifestRead {
			path: path.into(),
			source,
		})?;
	if !metadata.file_type().is_file() {
		return Err(ArkDataError::ManifestNotRegular { path: path.into() });
	}
	let bytes: Vec<u8> = fs::read(path).map_err(|source| ArkDataError::ManifestRead {
		path: path.into(),
		source,
	})?;
	serde_json::from_slice(&bytes).map_err(|source| ArkDataError::ManifestParse {
		path: path.into(),
		source,
	})
}

fn validate_manifest(manifest: &ManifestDocument) -> Result<(), ArkDataError> {
	let invalid = |field: &'static str, reason: String| ArkDataError::InvalidBattlePrefabManifest {
		field,
		reason,
	};
	if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
		return Err(invalid(
			"schema_version",
			format!(
				"expected {MANIFEST_SCHEMA_VERSION}, found {}",
				manifest.schema_version
			),
		));
	}
	if !is_exact_lowercase_hex(&manifest.mechanics_revision, SHA256_HEX_LENGTH) {
		return Err(invalid(
			"mechanics_revision",
			"expected exactly 64 lowercase hexadecimal characters".to_string(),
		));
	}
	if !is_exact_lowercase_hex(&manifest.gamedata_revision, GAMEDATA_REVISION_HEX_LENGTH) {
		return Err(invalid(
			"gamedata_revision",
			"expected exactly 40 lowercase hexadecimal characters".to_string(),
		));
	}
	if manifest.generated_at.is_empty() {
		return Err(invalid("generated_at", "value is empty".to_string()));
	}
	if !is_exact_lowercase_hex(&manifest.source.apk_sha256, SHA256_HEX_LENGTH) {
		return Err(invalid(
			"source.apk_sha256",
			"expected exactly 64 lowercase hexadecimal characters".to_string(),
		));
	}
	if manifest.source.apk_byte_length == 0 {
		return Err(invalid(
			"source.apk_byte_length",
			"value must be positive".to_string(),
		));
	}
	for (field, value) in [
		(
			"tooling.ark_ingest_version",
			manifest.tooling.ark_ingest_version.as_str(),
		),
		(
			"tooling.exporter_version",
			manifest.tooling.exporter_version.as_str(),
		),
		(
			"tooling.python_version",
			manifest.tooling.python_version.as_str(),
		),
		(
			"tooling.unitypy_version",
			manifest.tooling.unitypy_version.as_str(),
		),
		(
			"tooling.ark_unpacker_version",
			manifest.tooling.ark_unpacker_version.as_str(),
		),
	] {
		if value.is_empty() {
			return Err(invalid(field, "value is empty".to_string()));
		}
	}
	if manifest.entries.is_empty() {
		return Err(invalid(
			"entries",
			"at least one export is required".to_string(),
		));
	}

	let mut previous: Option<(&str, &str)> = None;
	let mut logical_paths: BTreeMap<String, &str> = BTreeMap::new();
	let mut export_paths: BTreeMap<String, &str> = BTreeMap::new();
	for entry in &manifest.entries {
		validate_relative_posix_path("entries[].logical_path", &entry.logical_path)?;
		validate_relative_posix_path("entries[].source_apk_path", &entry.source_apk_path)?;
		validate_relative_posix_path("entries[].export_path", &entry.export_path)?;
		for (field, value) in [
			("entries[].source_sha256", &entry.source_sha256),
			("entries[].export_sha256", &entry.export_sha256),
		] {
			if !is_exact_lowercase_hex(value, SHA256_HEX_LENGTH) {
				return Err(invalid(
					field,
					"expected exactly 64 lowercase hexadecimal characters".to_string(),
				));
			}
		}
		if entry.source_byte_length == 0 || entry.export_byte_length == 0 {
			return Err(invalid(
				"entries[].*_byte_length",
				"source and export lengths must be positive".to_string(),
			));
		}
		let sort_key: (&str, &str) = (&entry.logical_path, &entry.export_path);
		if previous.is_some_and(|previous| previous >= sort_key) {
			return Err(invalid(
				"entries",
				"entries must be strictly sorted by logical_path, then export_path".to_string(),
			));
		}
		previous = Some(sort_key);

		let normalized_logical: String = entry.logical_path.to_ascii_lowercase();
		if let Some(first) = logical_paths.insert(normalized_logical, &entry.logical_path) {
			return Err(ArkDataError::DuplicateBattlePrefabBundle {
				first: first.to_string(),
				second: entry.logical_path.clone(),
			});
		}
		let normalized_export: String = entry.export_path.to_ascii_lowercase();
		if let Some(first) = export_paths.insert(normalized_export, &entry.export_path) {
			return Err(ArkDataError::DuplicateDestination {
				first: first.to_string(),
				second: entry.export_path.clone(),
			});
		}
	}
	Ok(())
}

fn is_exact_lowercase_hex(value: &str, expected_length: usize) -> bool {
	value.len() == expected_length
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
