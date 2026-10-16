//! Deterministic allocation and lookup for replay summon templates.

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
};

use crate::mechanics::combat::{SummonTemplate, SummonTemplateId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SummonTemplateCatalogError {
	DuplicateKey {
		key: String,
	},
	DuplicateId {
		id: SummonTemplateId,
	},
	NonContiguousId {
		expected: SummonTemplateId,
		actual: SummonTemplateId,
	},
	IdSpaceExhausted,
	BuilderIdMismatch {
		expected: SummonTemplateId,
		actual: SummonTemplateId,
	},
	BuilderKeyMismatch {
		expected: String,
		actual: String,
	},
}

impl Display for SummonTemplateCatalogError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::DuplicateKey { key } => {
				write!(formatter, "duplicate summon-template key {key:?}")
			}
			Self::DuplicateId { id } => {
				write!(formatter, "duplicate summon-template ID {}", id.get())
			}
			Self::NonContiguousId { expected, actual } => write!(
				formatter,
				"non-contiguous summon-template ID: expected {}, found {}",
				expected.get(),
				actual.get()
			),
			Self::IdSpaceExhausted => formatter.write_str("summon-template ID space exhausted"),
			Self::BuilderIdMismatch { expected, actual } => write!(
				formatter,
				"summon-template builder returned ID {}, expected {}",
				actual.get(),
				expected.get()
			),
			Self::BuilderKeyMismatch { expected, actual } => write!(
				formatter,
				"summon-template builder returned key {actual:?}, expected {expected:?}"
			),
		}
	}
}

impl Error for SummonTemplateCatalogError {}

/// Owns summon templates in stable ID order and indexes their exact keys.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SummonTemplateCatalog {
	templates: Vec<SummonTemplate>,
	ids_by_key: BTreeMap<String, SummonTemplateId>,
}

impl SummonTemplateCatalog {
	#[must_use]
	pub fn new() -> Self {
		Self::default()
	}

	pub fn try_from_templates(
		templates: Vec<SummonTemplate>,
	) -> Result<Self, SummonTemplateCatalogError> {
		let mut ids_by_key: BTreeMap<String, SummonTemplateId> = BTreeMap::new();
		let mut seen_ids: BTreeSet<SummonTemplateId> = BTreeSet::new();
		for template in &templates {
			if ids_by_key
				.insert(template.key.clone(), template.id)
				.is_some()
			{
				return Err(SummonTemplateCatalogError::DuplicateKey {
					key: template.key.clone(),
				});
			}
			if !seen_ids.insert(template.id) {
				return Err(SummonTemplateCatalogError::DuplicateId { id: template.id });
			}
		}
		for (index, template) in templates.iter().enumerate() {
			let raw_expected: u32 =
				u32::try_from(index).map_err(|_| SummonTemplateCatalogError::IdSpaceExhausted)?;
			let expected: SummonTemplateId = SummonTemplateId::new(raw_expected);
			if template.id != expected {
				return Err(SummonTemplateCatalogError::NonContiguousId {
					expected,
					actual: template.id,
				});
			}
		}
		Ok(Self {
			templates,
			ids_by_key,
		})
	}

	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.templates.is_empty()
	}

	#[must_use]
	pub fn len(&self) -> usize {
		self.templates.len()
	}

	#[must_use]
	pub fn id_by_key(&self, key: &str) -> Option<SummonTemplateId> {
		self.ids_by_key.get(key).copied()
	}

	#[must_use]
	pub fn get_by_key(&self, key: &str) -> Option<&SummonTemplate> {
		self.id_by_key(key).and_then(|id| self.get_by_id(id))
	}

	#[must_use]
	pub fn get_by_id(&self, id: SummonTemplateId) -> Option<&SummonTemplate> {
		let index: usize = usize::try_from(id.get()).ok()?;
		self.templates
			.get(index)
			.filter(|template: &&SummonTemplate| template.id == id)
	}

	#[must_use]
	pub fn templates(&self) -> &[SummonTemplate] {
		&self.templates
	}

	/// Stable semantic key-to-runtime ID bindings for mechanic-program lowering.
	#[must_use]
	pub fn bindings(&self) -> &BTreeMap<String, SummonTemplateId> {
		&self.ids_by_key
	}

	pub fn get_or_insert_with<F>(
		&mut self,
		key: &str,
		builder: F,
	) -> Result<SummonTemplateId, SummonTemplateCatalogError>
	where
		F: FnOnce(SummonTemplateId) -> SummonTemplate,
	{
		if let Some(id) = self.id_by_key(key) {
			return Ok(id);
		}
		let raw_id: u32 = u32::try_from(self.templates.len())
			.map_err(|_| SummonTemplateCatalogError::IdSpaceExhausted)?;
		let id: SummonTemplateId = SummonTemplateId::new(raw_id);
		let template: SummonTemplate = builder(id);
		if template.id != id {
			return Err(SummonTemplateCatalogError::BuilderIdMismatch {
				expected: id,
				actual: template.id,
			});
		}
		if template.key != key {
			return Err(SummonTemplateCatalogError::BuilderKeyMismatch {
				expected: key.to_owned(),
				actual: template.key,
			});
		}
		self.ids_by_key.insert(key.to_owned(), id);
		self.templates.push(template);
		Ok(id)
	}

	#[must_use]
	pub fn into_templates(self) -> Vec<SummonTemplate> {
		self.templates
	}
}

#[cfg(test)]
mod tests {
	use crate::data::RangeOffset;

	use crate::mechanics::{
		combat::{
			ActorCombatFacts, AttackPattern, AttackProfile, BlockTargetFilter, ExpectedValue,
			Faction, NormalAttackAction, SummonCellOccupancy, SummonTemplate, SummonTemplateId,
			Targetability,
		},
		normal_attack::{NormalAttackTargetFilter, TargetClass},
	};

	use super::{SummonTemplateCatalog, SummonTemplateCatalogError};

	fn template(id: SummonTemplateId, key: &str) -> SummonTemplate {
		SummonTemplate {
			id,
			key: key.to_owned(),
			faction: Faction::Friendly,
			cell_occupancy: SummonCellOccupancy::Exclusive,
			block_count: 0,
			block_target_filter: BlockTargetFilter::GroundOnly,
			targetability: Targetability::All,
			combat: ActorCombatFacts {
				max_hp: ExpectedValue::ONE,
				attack: ExpectedValue::ZERO,
				defense: ExpectedValue::ZERO,
				target_class: TargetClass::Ground,
				arts_resistance: ExpectedValue::ZERO,
				minimum_damage_ratio: ExpectedValue::ZERO,
			},
			attack: AttackProfile {
				first_delay_ticks: 1,
				interval_ticks: 1,
				range: vec![RangeOffset::new(0, 0)],
				target_count: 0,
				target_filter: NormalAttackTargetFilter::GroundAndAir,
				action: NormalAttackAction::None,
				pattern: AttackPattern::Single,
				extra_effects: Vec::new(),
			},
			lifetime_ticks: None,
			reached_mechanics: Vec::new(),
		}
	}

	#[test]
	fn allocates_contiguous_ids_and_reuses_existing_key_without_rebuilding() {
		let mut catalog: SummonTemplateCatalog = SummonTemplateCatalog::new();
		let first: SummonTemplateId = catalog
			.get_or_insert_with("first", |id| template(id, "first"))
			.expect("first template");
		let second: SummonTemplateId = catalog
			.get_or_insert_with("second", |id| template(id, "second"))
			.expect("second template");
		let reused: SummonTemplateId = catalog
			.get_or_insert_with("first", |_| panic!("existing key must not call builder"))
			.expect("existing template");

		assert_eq!(first, SummonTemplateId::new(0));
		assert_eq!(second, SummonTemplateId::new(1));
		assert_eq!(reused, first);
		assert_eq!(catalog.len(), 2);
		assert_eq!(catalog.id_by_key("second"), Some(second));
		assert_eq!(catalog.bindings().get("first"), Some(&first));
		assert_eq!(catalog.bindings().get("second"), Some(&second));
		assert_eq!(
			catalog.get_by_key("first").map(|entry| entry.key.as_str()),
			Some("first")
		);
		assert_eq!(
			catalog.get_by_id(second).map(|entry| entry.key.as_str()),
			Some("second")
		);
		assert_eq!(
			catalog
				.templates()
				.iter()
				.map(|entry| entry.key.as_str())
				.collect::<Vec<_>>(),
			vec!["first", "second"]
		);
		assert_eq!(
			catalog
				.into_templates()
				.into_iter()
				.map(|entry| entry.id)
				.collect::<Vec<_>>(),
			vec![SummonTemplateId::new(0), SummonTemplateId::new(1)]
		);
	}

	#[test]
	fn builder_id_or_key_drift_is_explicit_and_does_not_mutate_catalog() {
		let mut catalog: SummonTemplateCatalog = SummonTemplateCatalog::new();
		let id_error: SummonTemplateCatalogError = catalog
			.get_or_insert_with("expected", |_| {
				template(SummonTemplateId::new(9), "expected")
			})
			.expect_err("builder ID drift");
		assert_eq!(
			id_error,
			SummonTemplateCatalogError::BuilderIdMismatch {
				expected: SummonTemplateId::new(0),
				actual: SummonTemplateId::new(9),
			}
		);

		let key_error: SummonTemplateCatalogError = catalog
			.get_or_insert_with("expected", |id| template(id, "different"))
			.expect_err("builder key drift");
		assert_eq!(
			key_error,
			SummonTemplateCatalogError::BuilderKeyMismatch {
				expected: "expected".to_owned(),
				actual: "different".to_owned(),
			}
		);
		assert!(catalog.is_empty());
	}

	#[test]
	fn initial_templates_reject_duplicate_keys_and_ids() {
		let duplicate_key: SummonTemplateCatalogError =
			SummonTemplateCatalog::try_from_templates(vec![
				template(SummonTemplateId::new(0), "same"),
				template(SummonTemplateId::new(1), "same"),
			])
			.expect_err("duplicate key");
		assert_eq!(
			duplicate_key,
			SummonTemplateCatalogError::DuplicateKey {
				key: "same".to_owned()
			}
		);

		let duplicate_id: SummonTemplateCatalogError =
			SummonTemplateCatalog::try_from_templates(vec![
				template(SummonTemplateId::new(0), "first"),
				template(SummonTemplateId::new(0), "second"),
			])
			.expect_err("duplicate ID");
		assert_eq!(
			duplicate_id,
			SummonTemplateCatalogError::DuplicateId {
				id: SummonTemplateId::new(0)
			}
		);
	}

	#[test]
	fn initial_templates_must_already_use_contiguous_stable_ids() {
		let error: SummonTemplateCatalogError = SummonTemplateCatalog::try_from_templates(vec![
			template(SummonTemplateId::new(0), "first"),
			template(SummonTemplateId::new(2), "third"),
		])
		.expect_err("ID gap");

		assert_eq!(
			error,
			SummonTemplateCatalogError::NonContiguousId {
				expected: SummonTemplateId::new(1),
				actual: SummonTemplateId::new(2),
			}
		);
	}
}
