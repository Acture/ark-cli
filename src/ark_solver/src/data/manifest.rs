use std::{
	collections::BTreeMap,
	fs::{self, File},
	io::{self, Read},
	path::{absolute, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::data::ArkDataError;

const MANIFEST_FILE_NAME: &str = "manifest.json";
const REPOSITORY: &str = "Kengxxiao/ArknightsGameData";
const SCHEMA_VERSION: u32 = 1;
const REVISION_HEX_LENGTH: usize = 40;
const SHA256_HEX_LENGTH: usize = 64;

pub(in crate::data) const REQUIRED_ROOTS: [&str; 12] = [
	"character_table.json",
	"char_patch_table.json",
	"skill_table.json",
	"range_table.json",
	"favor_table.json",
	"token_table.json",
	"uniequip_table.json",
	"battle_equip_table.json",
	"gamedata_const.json",
	"stage_table.json",
	"enemy_database.json",
	"enemy_handbook_table.json",
];

#[derive(Debug)]
pub(in crate::data) struct VerifiedEntry {
	pub(in crate::data) destination_path: String,
	pub(in crate::data) byte_length: u64,
	pub(in crate::data) sha256: String,
	pub(in crate::data) retained_bytes: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(in crate::data) struct VerifiedManifest {
	pub(in crate::data) root: PathBuf,
	pub(in crate::data) resolved_revision: String,
	pub(in crate::data) entries: BTreeMap<String, VerifiedEntry>,
}

/// A snapshot entry reader that hashes exactly the bytes returned to its consumer.
///
/// Call [`Self::finish`] before accepting data read through this stream. `finish` drains the
/// remaining bytes, then checks both the byte length and SHA-256 digest. This avoids verifying one
/// file pass and parsing a later, potentially changed pass.
pub(in crate::data) struct VerifiedEntryReader {
	file: File,
	destination_path: String,
	expected_byte_length: u64,
	expected_sha256: String,
	actual_byte_length: u64,
	digest: Sha256,
}

impl VerifiedEntryReader {
	pub(in crate::data) fn finish(&mut self) -> Result<(), ArkDataError> {
		let mut buffer: [u8; 1024 * 1024] = [0; 1024 * 1024];
		loop {
			let read: usize = self
				.read(&mut buffer)
				.map_err(|source| ArkDataError::EntryRead {
					destination_path: self.destination_path.clone(),
					source,
				})?;
			if read == 0 {
				break;
			}
		}

		if self.actual_byte_length != self.expected_byte_length {
			return Err(ArkDataError::ByteLengthMismatch {
				destination_path: self.destination_path.clone(),
				expected: self.expected_byte_length,
				actual: self.actual_byte_length,
			});
		}
		let actual_sha256: String = format!("{:x}", self.digest.clone().finalize());
		if actual_sha256 != self.expected_sha256 {
			return Err(ArkDataError::Sha256Mismatch {
				destination_path: self.destination_path.clone(),
				expected: self.expected_sha256.clone(),
				actual: actual_sha256,
			});
		}
		Ok(())
	}
}

impl Read for VerifiedEntryReader {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let read: usize = self.file.read(buffer)?;
		self.actual_byte_length = self
			.actual_byte_length
			.checked_add(u64::try_from(read).expect("read length fits u64"))
			.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "entry length overflow"))?;
		self.digest.update(&buffer[..read]);
		Ok(read)
	}
}

#[derive(Debug, Deserialize)]
struct ManifestDocument {
	schema_version: u32,
	repository: String,
	resolved_revision: String,
	entries: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
	destination_path: String,
	source_path: String,
	byte_length: u64,
	sha256: String,
}

pub(in crate::data) fn verify_snapshot(root: &Path) -> Result<VerifiedManifest, ArkDataError> {
	let root: PathBuf = validate_snapshot_root(root)?;
	let manifest_path: PathBuf = root.join(MANIFEST_FILE_NAME);
	let manifest: ManifestDocument = read_manifest(&manifest_path)?;
	validate_manifest_identity(&manifest)?;

	let entries: BTreeMap<String, ManifestEntry> = validate_entries(manifest.entries)?;
	validate_required_roots(&entries)?;
	let verified_entries: BTreeMap<String, VerifiedEntry> = verify_entries(&root, entries)?;

	Ok(VerifiedManifest {
		root,
		resolved_revision: manifest.resolved_revision,
		entries: verified_entries,
	})
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

fn validate_manifest_identity(manifest: &ManifestDocument) -> Result<(), ArkDataError> {
	if manifest.schema_version != SCHEMA_VERSION {
		return Err(ArkDataError::InvalidManifest {
			field: "schema_version",
			reason: format!(
				"expected {SCHEMA_VERSION}, found {}",
				manifest.schema_version
			),
		});
	}
	if manifest.repository != REPOSITORY {
		return Err(ArkDataError::InvalidManifest {
			field: "repository",
			reason: format!("expected {REPOSITORY:?}, found {:?}", manifest.repository),
		});
	}
	if !is_exact_lowercase_hex(&manifest.resolved_revision, REVISION_HEX_LENGTH) {
		return Err(ArkDataError::InvalidManifest {
			field: "resolved_revision",
			reason: format!(
				"expected exactly {REVISION_HEX_LENGTH} lowercase hexadecimal characters"
			),
		});
	}
	Ok(())
}

fn validate_entries(
	manifest_entries: Vec<ManifestEntry>,
) -> Result<BTreeMap<String, ManifestEntry>, ArkDataError> {
	let mut entries: BTreeMap<String, ManifestEntry> = BTreeMap::new();
	for entry in manifest_entries {
		validate_relative_posix_path("destination_path", &entry.destination_path)?;
		validate_relative_posix_path("source_path", &entry.source_path)?;
		if !is_exact_lowercase_hex(&entry.sha256, SHA256_HEX_LENGTH) {
			return Err(ArkDataError::InvalidManifest {
				field: "entries[].sha256",
				reason: format!(
					"expected exactly {SHA256_HEX_LENGTH} lowercase hexadecimal characters"
				),
			});
		}

		let normalized_destination: String = entry.destination_path.to_ascii_lowercase();
		if let Some(first) = entries.get(&normalized_destination) {
			return Err(ArkDataError::DuplicateDestination {
				first: first.destination_path.clone(),
				second: entry.destination_path,
			});
		}
		entries.insert(normalized_destination, entry);
	}
	Ok(entries)
}

fn validate_required_roots(entries: &BTreeMap<String, ManifestEntry>) -> Result<(), ArkDataError> {
	for destination_path in REQUIRED_ROOTS {
		if !entries.contains_key(destination_path) {
			return Err(ArkDataError::MissingRequiredRoot { destination_path });
		}
	}
	Ok(())
}

fn verify_entries(
	root: &Path,
	entries: BTreeMap<String, ManifestEntry>,
) -> Result<BTreeMap<String, VerifiedEntry>, ArkDataError> {
	let mut verified: BTreeMap<String, VerifiedEntry> = BTreeMap::new();
	for (normalized_destination, entry) in entries {
		let retained_bytes: Option<Vec<u8>> = if is_required_root(&normalized_destination) {
			Some(read_verified_entry(
				root,
				&entry.destination_path,
				entry.byte_length,
				&entry.sha256,
			)?)
		} else {
			verify_entry_integrity(
				root,
				&entry.destination_path,
				entry.byte_length,
				&entry.sha256,
			)?;
			None
		};
		verified.insert(
			normalized_destination,
			VerifiedEntry {
				destination_path: entry.destination_path,
				byte_length: entry.byte_length,
				sha256: entry.sha256,
				retained_bytes,
			},
		);
	}
	Ok(verified)
}

pub(in crate::data) fn read_verified_entry(
	root: &Path,
	destination_path: &str,
	expected_byte_length: u64,
	expected_sha256: &str,
) -> Result<Vec<u8>, ArkDataError> {
	let mut reader: VerifiedEntryReader = open_verified_entry(
		root,
		destination_path,
		expected_byte_length,
		expected_sha256,
	)?;
	let initial_capacity: usize = usize::try_from(expected_byte_length.min(16 * 1024 * 1024))
		.expect("bounded entry capacity fits usize");
	let mut bytes: Vec<u8> = Vec::with_capacity(initial_capacity);
	reader
		.read_to_end(&mut bytes)
		.map_err(|source| ArkDataError::EntryRead {
			destination_path: destination_path.to_string(),
			source,
		})?;
	reader.finish()?;
	Ok(bytes)
}

pub(in crate::data) fn open_verified_entry(
	root: &Path,
	destination_path: &str,
	expected_byte_length: u64,
	expected_sha256: &str,
) -> Result<VerifiedEntryReader, ArkDataError> {
	let absolute_path: PathBuf = verify_regular_entry_path(root, destination_path)?;
	let file: File = File::open(&absolute_path).map_err(|source| ArkDataError::EntryRead {
		destination_path: destination_path.to_string(),
		source,
	})?;
	Ok(VerifiedEntryReader {
		file,
		destination_path: destination_path.to_string(),
		expected_byte_length,
		expected_sha256: expected_sha256.to_string(),
		actual_byte_length: 0,
		digest: Sha256::new(),
	})
}

fn verify_entry_integrity(
	root: &Path,
	destination_path: &str,
	expected_byte_length: u64,
	expected_sha256: &str,
) -> Result<(), ArkDataError> {
	open_verified_entry(
		root,
		destination_path,
		expected_byte_length,
		expected_sha256,
	)?
	.finish()
}

fn verify_regular_entry_path(root: &Path, destination_path: &str) -> Result<PathBuf, ArkDataError> {
	let segments: Vec<&str> = destination_path.split('/').collect();
	let mut current_path: PathBuf = root.to_path_buf();
	for (index, segment) in segments.iter().enumerate() {
		current_path.push(*segment);
		let metadata: fs::Metadata =
			fs::symlink_metadata(&current_path).map_err(|source| ArkDataError::EntryRead {
				destination_path: destination_path.to_string(),
				source,
			})?;
		let is_final_component: bool = index + 1 == segments.len();
		let is_expected_type: bool = if is_final_component {
			metadata.file_type().is_file()
		} else {
			metadata.file_type().is_dir()
		};
		if !is_expected_type {
			return Err(ArkDataError::EntryNotRegular {
				destination_path: destination_path.to_string(),
			});
		}
	}
	Ok(current_path)
}

pub(in crate::data) fn validate_relative_posix_path(
	field: &'static str,
	path: &str,
) -> Result<(), ArkDataError> {
	let reason: Option<&'static str> = if path.is_empty() {
		Some("path is empty")
	} else if path.starts_with('/') {
		Some("absolute paths are not allowed")
	} else if has_windows_drive_prefix(path) {
		Some("Windows drive prefixes are not allowed")
	} else if path.contains('\\') {
		Some("backslashes are not allowed")
	} else if path.ends_with('/') {
		Some("trailing slashes are not allowed")
	} else if path.split('/').any(|segment| segment.is_empty()) {
		Some("empty path segments are not allowed")
	} else if path.split('/').any(|segment| segment == ".") {
		Some("current-directory segments are not allowed")
	} else if path.split('/').any(|segment| segment == "..") {
		Some("parent-directory segments are not allowed")
	} else {
		None
	};

	match reason {
		Some(reason) => Err(ArkDataError::UnsafePath {
			field,
			path: path.to_string(),
			reason,
		}),
		None => Ok(()),
	}
}

fn has_windows_drive_prefix(path: &str) -> bool {
	let bytes: &[u8] = path.as_bytes();
	bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_exact_lowercase_hex(value: &str, expected_length: usize) -> bool {
	value.len() == expected_length
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_required_root(destination_path: &str) -> bool {
	REQUIRED_ROOTS.contains(&destination_path)
}

#[cfg(test)]
mod tests {
	use std::{
		fs::{self, File, OpenOptions},
		io::{Read, Seek, SeekFrom, Write},
		path::PathBuf,
		sync::atomic::{AtomicU64, Ordering},
	};

	use sha2::{Digest, Sha256};

	use super::{open_verified_entry, VerifiedEntryReader};
	use crate::data::ArkDataError;

	static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(0);

	#[test]
	fn verified_reader_detects_in_place_changes_after_consumption_starts() {
		let id: u64 = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
		let root: PathBuf = std::env::temp_dir().join(format!(
			"rogue_verified_entry_reader_{}_{}",
			std::process::id(),
			id
		));
		fs::create_dir(&root).unwrap();
		let path: PathBuf = root.join("entry.json");
		let original: &[u8] = b"abcdef";
		fs::write(&path, original).unwrap();
		let expected_sha256: String = format!("{:x}", Sha256::digest(original));
		let mut reader: VerifiedEntryReader =
			open_verified_entry(&root, "entry.json", original.len() as u64, &expected_sha256)
				.unwrap();
		let mut prefix: [u8; 3] = [0; 3];
		reader.read_exact(&mut prefix).unwrap();
		assert_eq!(&prefix, b"abc");

		let mut writer: File = OpenOptions::new().write(true).open(&path).unwrap();
		writer.seek(SeekFrom::Start(3)).unwrap();
		writer.write_all(b"XYZ").unwrap();
		writer.sync_all().unwrap();

		assert!(matches!(
			reader.finish(),
			Err(ArkDataError::Sha256Mismatch { .. })
		));
		fs::remove_dir_all(root).unwrap();
	}
}
