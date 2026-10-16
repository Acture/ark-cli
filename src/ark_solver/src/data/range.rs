use std::collections::BTreeMap;

use serde::Deserialize;

use crate::data::{manifest::VerifiedEntry, ArkDataError};

const RANGE_TABLE_PATH: &str = "range_table.json";

#[derive(Debug)]
pub(in crate::data) struct RangeCatalog {
	records: BTreeMap<String, AttackRange>,
}

/// A range definition resolved from a verified snapshot.
///
/// Offsets remain in the source table's coordinate system. Consumers are responsible for applying
/// facing and translating them to stage coordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttackRange {
	id: String,
	source_direction: i32,
	offsets: Vec<RangeOffset>,
}

impl AttackRange {
	/// Returns the exact, case-sensitive range ID from the verified table.
	pub fn id(&self) -> &str {
		&self.id
	}

	/// Returns the uninterpreted numeric `direction` value from the source table.
	pub const fn source_direction(&self) -> i32 {
		self.source_direction
	}

	/// Returns source-order signed offsets without applying facing or translation.
	pub fn offsets(&self) -> &[RangeOffset] {
		&self.offsets
	}
}

/// A signed row/column offset in a source range definition.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RangeOffset {
	row: i32,
	col: i32,
}

impl RangeOffset {
	/// Creates an exact signed source-grid offset.
	pub const fn new(row: i32, col: i32) -> Self {
		Self { row, col }
	}

	pub const fn row(self) -> i32 {
		self.row
	}

	pub const fn col(self) -> i32 {
		self.col
	}
}

impl RangeCatalog {
	pub(in crate::data) fn from_entries(
		entries: &BTreeMap<String, VerifiedEntry>,
	) -> Result<Self, ArkDataError> {
		let entry: &VerifiedEntry =
			entries
				.get(RANGE_TABLE_PATH)
				.ok_or(ArkDataError::MissingRequiredRoot {
					destination_path: RANGE_TABLE_PATH,
				})?;
		let bytes: &[u8] =
			entry
				.retained_bytes
				.as_deref()
				.ok_or_else(|| ArkDataError::InvalidManifest {
					field: "entries",
					reason: format!("verified root {RANGE_TABLE_PATH:?} did not retain its bytes"),
				})?;
		Self::parse(bytes, &entry.destination_path)
	}

	fn parse(bytes: &[u8], destination_path: &str) -> Result<Self, ArkDataError> {
		let document: BTreeMap<String, RawRangeRecord> =
			serde_json::from_slice(bytes).map_err(|source| ArkDataError::RootParse {
				destination_path: destination_path.to_string(),
				source,
			})?;
		let mut normalized_ids: BTreeMap<String, String> = BTreeMap::new();
		let mut records: BTreeMap<String, AttackRange> = BTreeMap::new();
		for (map_key, raw) in document {
			if raw.id.is_empty() {
				return Err(ArkDataError::InvalidRangeId {
					range_id: raw.id,
					reason: "identity must not be empty".to_owned(),
				});
			}
			if map_key != raw.id {
				return Err(ArkDataError::RangeMapKeyMismatch {
					map_key,
					range_id: raw.id,
				});
			}
			let normalized_id: String = raw.id.to_ascii_lowercase();
			if let Some(first_range_id) = normalized_ids.get(&normalized_id) {
				return Err(ArkDataError::DuplicateRangeId {
					first_range_id: first_range_id.clone(),
					second_range_id: raw.id,
				});
			}
			normalized_ids.insert(normalized_id, raw.id.clone());
			records.insert(
				raw.id.clone(),
				AttackRange {
					id: raw.id,
					source_direction: raw.direction,
					offsets: raw.grids.into_iter().map(RangeOffset::from).collect(),
				},
			);
		}
		Ok(Self { records })
	}

	pub(in crate::data) fn resolve(&self, range_id: &str) -> Result<&AttackRange, ArkDataError> {
		self.records
			.get(range_id)
			.ok_or_else(|| ArkDataError::InvalidRangeId {
				range_id: range_id.to_owned(),
				reason: format!("{range_id:?} is absent from the verified range table"),
			})
	}
}

#[derive(Debug, Deserialize)]
struct RawRangeRecord {
	id: String,
	direction: i32,
	#[serde(default)]
	grids: Vec<RawSignedOffset>,
}

#[derive(Debug, Deserialize)]
struct RawSignedOffset {
	row: i32,
	col: i32,
}

impl From<RawSignedOffset> for RangeOffset {
	fn from(raw: RawSignedOffset) -> Self {
		Self {
			row: raw.row,
			col: raw.col,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{RangeCatalog, RangeOffset};

	#[test]
	fn parses_ranges_lexically_and_preserves_signed_offsets() {
		let bytes: &[u8] = br#"{
			"zeta": {"id":"zeta","direction":2,"grids":[{"row":-3,"col":4}]},
			"alpha": {"id":"alpha","direction":-1,"grids":[{"row":5,"col":-6}]}
		}"#;

		let catalog: RangeCatalog =
			RangeCatalog::parse(bytes, "range_table.json").expect("valid private range catalog");

		assert_eq!(
			catalog
				.records
				.keys()
				.map(String::as_str)
				.collect::<Vec<_>>(),
			vec!["alpha", "zeta"]
		);
		assert_eq!(catalog.records["alpha"].source_direction, -1);
		assert_eq!(
			catalog.records["alpha"].offsets,
			vec![RangeOffset { row: 5, col: -6 }]
		);
		assert_eq!(
			catalog.records["zeta"].offsets,
			vec![RangeOffset { row: -3, col: 4 }]
		);
	}
}
