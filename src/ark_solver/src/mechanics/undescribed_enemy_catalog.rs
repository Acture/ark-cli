//! Generic decoder for gamedata enemy records that describe no behaviour of their own.
//!
//! The reviewed BD-7 catalog proves a record's behaviour from a manual per-record prefab review.
//! This decoder cannot do that: no enemy prefab is part of any pinned battle-prefab snapshot. What
//! it can prove is the weaker statement that the pinned data **describes** no behaviour beyond the
//! base panel — no skills, no talent blackboard, no SP metadata, no handbook ability, and exactly
//! one readable attacking damage family.
//!
//! That is deliberately not the same as proving the record has no behaviour. The record's battle
//! prefab is never read, so anything it declares that the pinned tables do not describe — splash,
//! targetability, on-hit effects — is neither modelled nor visible here. A caller must therefore
//! record the unread prefab as an omission rather than as supported behaviour; see
//! `replay::compile::compile_undescribed_enemy`.
//!
//! This module owns no replay-domain types.

use std::{
	collections::BTreeSet,
	error::Error,
	fmt::{Display, Formatter},
};

/// Damage family a gamedata enemy record's handbook row names for its attack.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReviewedEnemyDamageType {
	Physical,
	Arts,
}

use crate::data::{CompiledEnemy, EnemyDamageKind, EnemyHandbookFacts};
use serde_json::Value;

/// Exact decoded result for one record whose pinned data describes no behaviour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UndescribedEnemy {
	pub normal_attack_damage_type: ReviewedEnemyDamageType,
}

/// The pinned data describes behaviour this decoder cannot represent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescribedEnemyBehavior {
	enemy_key: String,
	reason: String,
}

impl DescribedEnemyBehavior {
	#[must_use]
	pub fn enemy_key(&self) -> &str {
		&self.enemy_key
	}

	#[must_use]
	pub fn reason(&self) -> &str {
		&self.reason
	}
}

impl Display for DescribedEnemyBehavior {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		write!(
			formatter,
			"enemy {:?} describes behaviour beyond its base panel: {}",
			self.enemy_key, self.reason
		)
	}
}

impl Error for DescribedEnemyBehavior {}

/// Decodes one compiled enemy whose pinned data describes nothing beyond its base panel.
///
/// Every field that could describe behaviour must be absent or empty, the record must own the
/// prefab key it is compiled under at a declared database level, and the handbook must name exactly
/// one attacking damage family. Anything else returns an auditable reason.
pub fn review_undescribed_enemy(
	enemy: &CompiledEnemy,
) -> Result<UndescribedEnemy, DescribedEnemyBehavior> {
	review_shape(ObservedEnemyDescription::from(enemy))
}

/// The exact pinned fields that decide whether a record describes behaviour of its own.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedEnemyDescription {
	key: String,
	database_level: Option<u32>,
	prefab_key: Option<String>,
	skill_count: usize,
	talent_blackboard: Option<Value>,
	has_sp_data: bool,
	declared_ability_count: Option<usize>,
	damage_kinds: Option<BTreeSet<EnemyDamageKind>>,
	unreadable_damage_families: BTreeSet<String>,
}

impl From<&CompiledEnemy> for ObservedEnemyDescription {
	fn from(enemy: &CompiledEnemy) -> Self {
		let handbook: Option<&EnemyHandbookFacts> = enemy.handbook();
		Self {
			key: enemy.key().as_str().to_owned(),
			database_level: enemy.database_level(),
			prefab_key: enemy.prefab_key().map(str::to_owned),
			skill_count: enemy.skills().len(),
			talent_blackboard: enemy.talent_blackboard().cloned(),
			has_sp_data: enemy.sp_data().is_some(),
			declared_ability_count: handbook.map(EnemyHandbookFacts::declared_ability_count),
			damage_kinds: handbook.map(|facts| facts.damage_kinds().clone()),
			unreadable_damage_families: handbook
				.map(|facts| facts.unreadable_damage_families().clone())
				.unwrap_or_default(),
		}
	}
}

fn review_shape(
	shape: ObservedEnemyDescription,
) -> Result<UndescribedEnemy, DescribedEnemyBehavior> {
	// The record must own the identity it is compiled under, exactly as the reviewed catalog
	// requires, so a stage-local override cannot borrow another record's description.
	if shape.database_level.is_none() {
		return Err(described(&shape.key, "record declares no database level"));
	}
	if shape.prefab_key.as_deref() != Some(shape.key.as_str()) {
		return Err(described(
			&shape.key,
			format!(
				"prefab_key expected Some({:?}), found {:?}",
				shape.key, shape.prefab_key
			),
		));
	}
	if shape.skill_count != 0 {
		return Err(described(
			&shape.key,
			format!("record declares {} skill(s)", shape.skill_count),
		));
	}
	let declares_talent: bool = shape
		.talent_blackboard
		.as_ref()
		.is_some_and(|blackboard: &Value| !is_empty_blackboard(blackboard));
	if declares_talent {
		return Err(described(&shape.key, "record declares a talent blackboard"));
	}
	if shape.has_sp_data {
		return Err(described(&shape.key, "record declares SP metadata"));
	}
	match shape.declared_ability_count {
		None => {
			return Err(described(
				&shape.key,
				"the pinned enemy handbook has no row for this key",
			))
		}
		Some(0) => {}
		Some(count) => {
			return Err(described(
				&shape.key,
				format!("the handbook describes {count} ability/abilities"),
			))
		}
	}
	if !shape.unreadable_damage_families.is_empty() {
		let unreadable: Vec<&str> = shape
			.unreadable_damage_families
			.iter()
			.map(String::as_str)
			.collect();
		return Err(described(
			&shape.key,
			format!("handbook declares unreadable damage families {unreadable:?}"),
		));
	}
	let kinds: BTreeSet<EnemyDamageKind> = shape.damage_kinds.unwrap_or_default();
	let normal_attack_damage_type: ReviewedEnemyDamageType = match single_kind(&kinds) {
		Some(EnemyDamageKind::Physical) => ReviewedEnemyDamageType::Physical,
		Some(EnemyDamageKind::Arts) => ReviewedEnemyDamageType::Arts,
		_ => {
			let declared: Vec<&str> = kinds.iter().map(|kind| kind.stable_id()).collect();
			return Err(described(
				&shape.key,
				format!(
					"handbook damage families {declared:?} do not resolve to one attacking family"
				),
			));
		}
	};
	Ok(UndescribedEnemy {
		normal_attack_damage_type,
	})
}

/// A record may carry an explicitly empty blackboard; only entries describe behaviour.
fn is_empty_blackboard(blackboard: &Value) -> bool {
	match blackboard {
		Value::Null => true,
		Value::Array(entries) => entries.is_empty(),
		Value::Object(fields) => fields.is_empty(),
		_ => false,
	}
}

fn single_kind(kinds: &BTreeSet<EnemyDamageKind>) -> Option<EnemyDamageKind> {
	let mut iter = kinds.iter();
	let first: EnemyDamageKind = *iter.next()?;
	iter.next().is_none().then_some(first)
}

fn described(enemy_key: &str, reason: impl Into<String>) -> DescribedEnemyBehavior {
	DescribedEnemyBehavior {
		enemy_key: enemy_key.to_owned(),
		reason: reason.into(),
	}
}

#[cfg(test)]
mod tests {
	use super::{
		review_shape, DescribedEnemyBehavior, EnemyDamageKind, ObservedEnemyDescription,
		ReviewedEnemyDamageType, UndescribedEnemy,
	};
	use serde_json::json;
	use std::collections::BTreeSet;

	fn kinds(values: &[EnemyDamageKind]) -> BTreeSet<EnemyDamageKind> {
		values.iter().copied().collect()
	}

	fn undescribed_shape() -> ObservedEnemyDescription {
		ObservedEnemyDescription {
			key: "enemy_test".to_owned(),
			database_level: Some(0),
			prefab_key: Some("enemy_test".to_owned()),
			skill_count: 0,
			talent_blackboard: None,
			has_sp_data: false,
			declared_ability_count: Some(0),
			damage_kinds: Some(kinds(&[EnemyDamageKind::Physical])),
			unreadable_damage_families: BTreeSet::new(),
		}
	}

	#[test]
	fn an_undescribed_physical_record_decodes_to_a_physical_normal_attack() {
		let decoded: UndescribedEnemy =
			review_shape(undescribed_shape()).expect("undescribed record should decode");
		assert_eq!(
			decoded.normal_attack_damage_type,
			ReviewedEnemyDamageType::Physical
		);
	}

	#[test]
	fn an_undescribed_arts_record_decodes_to_an_arts_normal_attack() {
		let mut shape: ObservedEnemyDescription = undescribed_shape();
		shape.damage_kinds = Some(kinds(&[EnemyDamageKind::Arts]));
		let decoded: UndescribedEnemy =
			review_shape(shape).expect("undescribed record should decode");
		assert_eq!(
			decoded.normal_attack_damage_type,
			ReviewedEnemyDamageType::Arts
		);
	}

	#[test]
	fn an_explicitly_empty_talent_blackboard_describes_nothing() {
		for empty in [json!(null), json!([]), json!({})] {
			let mut shape: ObservedEnemyDescription = undescribed_shape();
			shape.talent_blackboard = Some(empty.clone());
			assert!(
				review_shape(shape).is_ok(),
				"{empty} should not describe behaviour"
			);
		}
	}

	#[test]
	fn a_described_ability_fails_closed() {
		// The pinned handbook describes exactly this way the splash casters and the untargetable
		// ambushers differ from a record with no behaviour of its own.
		let mut shape: ObservedEnemyDescription = undescribed_shape();
		shape.declared_ability_count = Some(1);
		let error: DescribedEnemyBehavior =
			review_shape(shape).expect_err("a described ability is behaviour");
		assert!(error.reason().contains("describes 1 ability"));
	}

	#[test]
	fn a_borrowed_identity_fails_closed() {
		let mut no_level: ObservedEnemyDescription = undescribed_shape();
		no_level.database_level = None;
		assert!(review_shape(no_level)
			.expect_err("an undeclared database level is not reviewable")
			.reason()
			.contains("no database level"));

		let mut other_prefab: ObservedEnemyDescription = undescribed_shape();
		other_prefab.prefab_key = Some("enemy_other".to_owned());
		assert!(review_shape(other_prefab)
			.expect_err("a record must own its prefab key")
			.reason()
			.contains("prefab_key expected"));
	}

	#[test]
	fn behaviour_bearing_fields_fail_closed() {
		let mut with_skill: ObservedEnemyDescription = undescribed_shape();
		with_skill.skill_count = 1;
		assert!(review_shape(with_skill)
			.expect_err("a skill is behaviour")
			.reason()
			.contains("1 skill(s)"));

		let mut with_talent: ObservedEnemyDescription = undescribed_shape();
		with_talent.talent_blackboard = Some(json!([{"key": "value", "value": 1.0}]));
		assert!(review_shape(with_talent)
			.expect_err("a talent blackboard is behaviour")
			.reason()
			.contains("talent blackboard"));

		let mut with_sp: ObservedEnemyDescription = undescribed_shape();
		with_sp.has_sp_data = true;
		assert!(review_shape(with_sp)
			.expect_err("SP metadata is behaviour")
			.reason()
			.contains("SP metadata"));
	}

	#[test]
	fn an_undecidable_damage_family_fails_closed() {
		let mut missing: ObservedEnemyDescription = undescribed_shape();
		missing.declared_ability_count = None;
		missing.damage_kinds = None;
		assert!(review_shape(missing)
			.expect_err("a missing handbook row is undecidable")
			.reason()
			.contains("no row for this key"));

		let mut unreadable: ObservedEnemyDescription = undescribed_shape();
		unreadable.unreadable_damage_families = ["ELEMENTAL".to_owned()].into_iter().collect();
		assert!(review_shape(unreadable)
			.expect_err("an unnameable family is undecidable")
			.reason()
			.contains("unreadable damage families"));

		for undecidable in [
			kinds(&[EnemyDamageKind::Physical, EnemyDamageKind::Arts]),
			kinds(&[EnemyDamageKind::NoDamage]),
			kinds(&[EnemyDamageKind::Healing]),
			BTreeSet::new(),
		] {
			let mut shape: ObservedEnemyDescription = undescribed_shape();
			shape.damage_kinds = Some(undecidable);
			assert!(review_shape(shape)
				.expect_err("only one attacking family may decode")
				.reason()
				.contains("do not resolve to one attacking family"));
		}
	}
}
