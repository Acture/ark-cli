use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use crate::data::{
	enemy::EnemyCatalog,
	manifest::{
		read_verified_entry, validate_relative_posix_path, verify_snapshot, VerifiedEntry,
		VerifiedManifest,
	},
	operator::OperatorCatalog,
	range::{AttackRange, RangeCatalog},
	raw_roots::RawRoots,
	stage::StageCatalog,
	ArkDataError, CompiledStage, StageRef,
};

#[derive(Debug)]
struct VerifiedLevel {
	destination_path: String,
	byte_length: u64,
	sha256: String,
}

/// A fully integrity-checked Arknights analysis-data snapshot.
#[derive(Debug)]
pub struct ArkDataSnapshot {
	root: PathBuf,
	resolved_revision: String,
	_raw_roots: RawRoots,
	range_catalog: RangeCatalog,
	operator_catalog: OperatorCatalog,
	enemy_catalog: EnemyCatalog,
	stage_catalog: StageCatalog,
	verified_levels: BTreeMap<String, VerifiedLevel>,
}

impl ArkDataSnapshot {
	/// Loads and verifies every manifest entry before parsing any root table.
	///
	/// The snapshot tree must remain unchanged for the duration of this call. Static symlinks and
	/// non-regular entries are rejected, but concurrent filesystem mutation is outside this API's
	/// trust model.
	pub fn load(path: &Path) -> Result<Self, ArkDataError> {
		let VerifiedManifest {
			root,
			resolved_revision,
			entries,
		}: VerifiedManifest = verify_snapshot(path)?;
		let raw_roots: RawRoots = RawRoots::parse(&entries)?;
		let range_catalog: RangeCatalog = RangeCatalog::from_entries(&entries)?;
		let operator_catalog: OperatorCatalog =
			OperatorCatalog::from_roots(&raw_roots, resolved_revision.clone())?;
		let enemy_catalog: EnemyCatalog = EnemyCatalog::from_entries(&entries)?;
		let stage_catalog: StageCatalog = StageCatalog::parse(&entries)?;
		let verified_levels: BTreeMap<String, VerifiedLevel> = index_verified_levels(entries);

		Ok(Self {
			root,
			resolved_revision,
			_raw_roots: raw_roots,
			range_catalog,
			operator_catalog,
			enemy_catalog,
			stage_catalog,
			verified_levels,
		})
	}

	/// Returns the immutable 40-character Git revision recorded by the snapshot.
	pub fn resolved_revision(&self) -> &str {
		&self.resolved_revision
	}

	/// Returns the immutable operator catalog compiled from this verified snapshot.
	pub const fn operators(&self) -> &OperatorCatalog {
		&self.operator_catalog
	}

	/// Resolves an exact range ID from this verified snapshot.
	pub fn resolve_range(&self, range_id: &str) -> Result<&AttackRange, ArkDataError> {
		self.range_catalog.resolve(range_id)
	}

	/// Resolves a stage variant and parses its freshly reverified level document.
	pub fn compile_stage(&self, stage_ref: StageRef) -> Result<CompiledStage, ArkDataError> {
		let difficulty = stage_ref.difficulty;
		let stage = self.stage_catalog.resolve(&stage_ref)?;
		let level_id: String = stage
			.level_id
			.clone()
			.filter(|level_id: &String| !level_id.is_empty())
			.ok_or_else(|| ArkDataError::MissingStageLevelId {
				stage_id: stage.stage_id.clone(),
			})?;
		let requested_destination: String = format!("levels/{level_id}.json");
		validate_relative_posix_path("stage levelId", &requested_destination)?;
		let normalized_destination: String = requested_destination.to_ascii_lowercase();
		let verified_level: &VerifiedLevel = self
			.verified_levels
			.get(&normalized_destination)
			.ok_or_else(|| ArkDataError::MissingLevelManifestEntry {
				stage_id: stage.stage_id.clone(),
				level_id: level_id.clone(),
				destination_path: requested_destination.clone(),
			})?;
		let bytes: Vec<u8> = read_verified_entry(
			&self.root,
			&verified_level.destination_path,
			verified_level.byte_length,
			&verified_level.sha256,
		)?;

		CompiledStage::parse(
			stage,
			difficulty,
			level_id,
			&verified_level.destination_path,
			&bytes,
			&self.enemy_catalog,
		)
	}
}

fn index_verified_levels(
	entries: BTreeMap<String, VerifiedEntry>,
) -> BTreeMap<String, VerifiedLevel> {
	entries
		.into_iter()
		.filter_map(|(normalized_path, entry): (String, VerifiedEntry)| {
			if normalized_path.starts_with("levels/") && normalized_path.ends_with(".json") {
				Some((
					normalized_path,
					VerifiedLevel {
						destination_path: entry.destination_path,
						byte_length: entry.byte_length,
						sha256: entry.sha256,
					},
				))
			} else {
				None
			}
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::{index_verified_levels, VerifiedLevel};
	use crate::data::manifest::VerifiedEntry;
	use std::collections::BTreeMap;

	fn entry(destination_path: &str, byte_length: u64) -> VerifiedEntry {
		VerifiedEntry {
			destination_path: destination_path.to_string(),
			byte_length,
			sha256: "ab".repeat(32),
			retained_bytes: None,
		}
	}

	#[test]
	fn indexes_only_nested_json_levels_with_verified_metadata() {
		let normalized_level: &str = "levels/activities/test/level_test_01.json";
		let mut entries: BTreeMap<String, VerifiedEntry> = BTreeMap::new();
		entries.insert(
			normalized_level.to_string(),
			entry("LeVeLs/Activities/Test/Level_Test_01.JsOn", 731),
		);
		entries.insert(
			"levels/activities/test/readme.txt".to_string(),
			entry("levels/activities/test/readme.txt", 1),
		);
		entries.insert(
			"other/level_test_02.json".to_string(),
			entry("other/level_test_02.json", 2),
		);

		let levels: BTreeMap<String, VerifiedLevel> = index_verified_levels(entries);

		assert_eq!(levels.len(), 1);
		let level: &VerifiedLevel = levels
			.get(normalized_level)
			.expect("normalized nested JSON level should be indexed");
		assert_eq!(
			level.destination_path,
			"LeVeLs/Activities/Test/Level_Test_01.JsOn"
		);
		assert_eq!(level.byte_length, 731);
		assert_eq!(level.sha256, "ab".repeat(32));
	}
}
