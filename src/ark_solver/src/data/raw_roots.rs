use std::collections::BTreeMap;

use serde_json::Value;

use crate::data::{
	manifest::{VerifiedEntry, REQUIRED_ROOTS},
	ArkDataError,
};

#[derive(Debug)]
pub(in crate::data) struct RawRoots {
	tables: BTreeMap<String, Value>,
}

impl RawRoots {
	pub(in crate::data) fn parse(
		entries: &BTreeMap<String, VerifiedEntry>,
	) -> Result<Self, ArkDataError> {
		let mut tables: BTreeMap<String, Value> = BTreeMap::new();
		for destination_path in REQUIRED_ROOTS {
			let entry: &VerifiedEntry = entries
				.get(destination_path)
				.ok_or(ArkDataError::MissingRequiredRoot { destination_path })?;
			let bytes: &[u8] =
				entry
					.retained_bytes
					.as_deref()
					.ok_or_else(|| ArkDataError::InvalidManifest {
						field: "entries",
						reason: format!(
							"verified root {destination_path:?} did not retain its bytes"
						),
					})?;
			let value: Value =
				serde_json::from_slice(bytes).map_err(|source| ArkDataError::RootParse {
					destination_path: entry.destination_path.clone(),
					source,
				})?;
			tables.insert(destination_path.to_string(), value);
		}
		Ok(Self { tables })
	}

	pub(in crate::data) fn table(&self, destination_path: &str) -> Option<&Value> {
		self.tables.get(destination_path)
	}
}
