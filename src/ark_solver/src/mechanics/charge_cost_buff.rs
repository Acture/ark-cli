//! Fail-closed decoder for the `charge_cost` buff template: one Deployment-Point Grant per attach.
//!
//! The template is a building block rather than a skill. Every skill that grants deployment points
//! on cast carries it as one serialized buff, beside whatever else the skill does, under whichever
//! ability attaches it. This decoder reads only the buff: when and how often it is attached belongs
//! to its owner, which must be reviewed on its own before any grant executes.
//!
//! Across the verified common skills bundle every `charge_cost` buff shares one serialized envelope
//! and differs only in its key, lifetime, duration key, override effect, attribute modifiers and
//! whether it strips its key from blackboard names. The reviewed shape is the zero-lifetime,
//! modifier-free buff with no duration key or override effect: it adds the owning skill level's
//! `cost` once, read as `<buffKey>.cost` when the buff strips its key prefix. Every other observed
//! variant is a named, unreviewed shape rather than a guess.

use std::fmt;

use crate::data::BlackboardEntry;
use serde_json::{Map, Value};

use crate::mechanics::battle_prefab_interpreter::{
	BattlePrefabBuffSpec, BattlePrefabMonoBehaviour,
};

pub(crate) const CHARGE_COST_TEMPLATE: &str = "charge_cost";
const COST_BLACKBOARD_KEY: &str = "cost";

/// Why one `charge_cost` buff cannot be decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeCostBuffRejection {
	/// A field that is constant across every verified occurrence changed, or the object is not a
	/// `charge_cost` buff at all.
	Drift,
	/// An observed but unreviewed variant: a lifetime, duration key, override effect or attribute
	/// modifier whose effect on the grant is not established.
	UnreviewedVariant,
	/// The owning skill level does not carry exactly one whole, positive amount under the key the
	/// buff reads.
	Amount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChargeCostBuffDecodeError {
	pub field_path: String,
	pub rejection: ChargeCostBuffRejection,
	pub detail: String,
}

impl fmt::Display for ChargeCostBuffDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"charge_cost buff at {} is {:?}: {}",
			self.field_path, self.rejection, self.detail
		)
	}
}

impl std::error::Error for ChargeCostBuffDecodeError {}

/// One decoded grant: whole deployment points credited each time the owner attaches the buff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChargeCostGrant {
	pub field_path: String,
	pub buff_key: String,
	/// The skill blackboard key the amount was read from.
	pub amount_key: String,
	pub amount: u32,
}

/// Whether a serialized buff spec names the `charge_cost` template.
pub(crate) fn is_charge_cost_buff(spec: &BattlePrefabBuffSpec) -> bool {
	spec.template_key.as_deref() == Some(CHARGE_COST_TEMPLATE)
}

/// Whether a `charge_cost` buff has the reviewed shape, judged from its serialized fields alone.
/// Owners use this to decide what they claim; [`decode_charge_cost_buff`] still validates it.
pub(crate) fn has_reviewed_charge_cost_shape(
	spec: &BattlePrefabBuffSpec,
	raw_buff: &Value,
) -> bool {
	is_charge_cost_buff(spec)
		&& spec.attribute_modifiers.is_empty()
		&& unreviewed_variant(raw_buff).is_none()
}

/// Resolves a buff spec's `$`-rooted field path inside its behaviour's serialized fields.
pub(crate) fn serialized_buff<'a>(
	behavior: &'a BattlePrefabMonoBehaviour,
	spec: &BattlePrefabBuffSpec,
) -> Option<&'a Value> {
	let mut rest: &str = spec.field_path.strip_prefix('$')?;
	let mut value: &Value = &behavior.raw_fields;
	while !rest.is_empty() {
		if let Some(after) = rest.strip_prefix('.') {
			let end: usize = after.find(['.', '[']).unwrap_or(after.len());
			value = value.get(&after[..end])?;
			rest = &after[end..];
		} else {
			let after: &str = rest.strip_prefix('[')?;
			let end: usize = after.find(']')?;
			value = value.get(after[..end].parse::<usize>().ok()?)?;
			rest = &after[end + 1..];
		}
	}
	Some(value)
}

/// Decodes one `charge_cost` buff against the owning skill level's blackboard.
pub fn decode_charge_cost_buff(
	spec: &BattlePrefabBuffSpec,
	raw_buff: &Value,
	blackboard: &[BlackboardEntry],
) -> Result<ChargeCostGrant, ChargeCostBuffDecodeError> {
	let reject = |rejection: ChargeCostBuffRejection, detail: String| ChargeCostBuffDecodeError {
		field_path: spec.field_path.clone(),
		rejection,
		detail,
	};
	if !is_charge_cost_buff(spec) {
		return Err(reject(
			ChargeCostBuffRejection::Drift,
			format!(
				"template {:?} is not {CHARGE_COST_TEMPLATE:?}",
				spec.template_key
			),
		));
	}
	let buff_key: &str = spec
		.buff_key
		.as_deref()
		.filter(|key: &&str| !key.is_empty())
		.ok_or_else(|| {
			reject(
				ChargeCostBuffRejection::Drift,
				"the buff has no key".to_owned(),
			)
		})?;
	let fields: &Map<String, Value> = raw_buff.as_object().ok_or_else(|| {
		reject(
			ChargeCostBuffRejection::Drift,
			"the serialized buff is not an object".to_owned(),
		)
	})?;
	if fields.get("buffKey").and_then(Value::as_str) != Some(buff_key) {
		return Err(reject(
			ChargeCostBuffRejection::Drift,
			format!("serialized buffKey does not match the interpreted key {buff_key:?}"),
		));
	}
	let strips_key: bool = match fields.get("stripBlackboardParamsWithBuffKey") {
		Some(Value::Number(value)) if value.as_i64() == Some(0) => false,
		Some(Value::Number(value)) if value.as_i64() == Some(1) => true,
		other => {
			return Err(reject(
				ChargeCostBuffRejection::Drift,
				format!("stripBlackboardParamsWithBuffKey must be 0 or 1, found {other:?}"),
			));
		}
	};
	if let Some(detail) = constant_field_mismatch(fields) {
		return Err(reject(ChargeCostBuffRejection::Drift, detail));
	}
	if !spec.attribute_modifiers.is_empty() {
		return Err(reject(
			ChargeCostBuffRejection::UnreviewedVariant,
			format!(
				"{} attribute modifier(s) ride on the grant",
				spec.attribute_modifiers.len()
			),
		));
	}
	if let Some(detail) = unreviewed_variant(raw_buff) {
		return Err(reject(ChargeCostBuffRejection::UnreviewedVariant, detail));
	}

	let amount_key: String = if strips_key {
		format!("{buff_key}.{COST_BLACKBOARD_KEY}")
	} else {
		COST_BLACKBOARD_KEY.to_owned()
	};
	let matches: Vec<&BlackboardEntry> = blackboard
		.iter()
		.filter(|entry: &&BlackboardEntry| entry.key() == amount_key)
		.collect();
	let [entry] = matches.as_slice() else {
		return Err(reject(
			ChargeCostBuffRejection::Amount,
			format!(
				"expected one {amount_key:?} blackboard entry, found {}",
				matches.len()
			),
		));
	};
	let value: f64 = entry.value();
	if !value.is_finite()
		|| value <= 0.0
		|| value.fract() != 0.0
		|| value > f64::from(u32::MAX)
		|| entry.value_string().is_some()
	{
		return Err(reject(
			ChargeCostBuffRejection::Amount,
			format!(
				"{amount_key:?} must be a whole positive DP amount without text; found {value}, {:?}",
				entry.value_string()
			),
		));
	}
	Ok(ChargeCostGrant {
		field_path: spec.field_path.clone(),
		buff_key: buff_key.to_owned(),
		amount_key,
		amount: value as u32,
	})
}

/// Names the first observed-but-unreviewed variant field, if any.
fn unreviewed_variant(raw_buff: &Value) -> Option<String> {
	let reviewed: [(&str, Value); 4] = [
		("lifeTimeType", Value::from(0)),
		("lifeTime", Value::from(0.0)),
		("durationKey", Value::from("")),
		("overrideEffectKey", Value::from("")),
	];
	reviewed.into_iter().find_map(|(field, expected)| {
		let actual: Option<&Value> = raw_buff.get(field);
		(actual != Some(&expected))
			.then(|| format!("{field} is {actual:?}, reviewed only as {expected}"))
	})
}

/// Every field except the seven that vary between verified occurrences must hold its constant
/// value, and no field may be added or removed.
fn constant_field_mismatch(fields: &Map<String, Value>) -> Option<String> {
	const VARYING: [&str; 7] = [
		"buffKey",
		"stripBlackboardParamsWithBuffKey",
		"lifeTimeType",
		"lifeTime",
		"durationKey",
		"overrideEffectKey",
		"attributes",
	];
	let constants: Value = serde_json::from_str(
		r#"{
			"audioSignal": "",
			"blackboard": [],
			"clearAllStackCntWhenTimeUp": 0,
			"disableOverride": 0,
			"enableInitDirectionFromSource": 0,
			"firstTriggerInterval": -1.0,
			"independentCharacterSource": 0,
			"isDamageMissable": 0,
			"isDurableBuff": 0,
			"isFreezable": 0,
			"isLevitatable": 0,
			"isSilenceable": 0,
			"isStunnable": 0,
			"loadFromDB": 0,
			"maxStackCnt": 1,
			"maxValidStackCnt": -1,
			"onEventPriority": 0,
			"overrideKey": "",
			"overrideOnEventPriority": 0,
			"overrideType": 0,
			"priority": -1,
			"priorityBBKeys": [],
			"refreshRemainingTimeWhenStackMax": 0,
			"statusResistable": 2,
			"takeSnapshotWhenExtend": 0,
			"templateKey": "charge_cost",
			"triggerCnt": 0,
			"triggerInterval": -1.0,
			"triggerLifeType": 0,
			"waitFirstTriggerInterval": 1
		}"#,
	)
	.expect("internal charge_cost envelope must be valid JSON");
	let constants: &Map<String, Value> = constants
		.as_object()
		.expect("internal charge_cost envelope is an object");
	for (field, expected) in constants {
		match fields.get(field) {
			None => return Some(format!("missing field $.{field}")),
			Some(actual) if actual != expected => {
				return Some(format!("$.{field} expected {expected}, found {actual}"));
			}
			Some(_) => {}
		}
	}
	if let Some(field) = fields.keys().find(|field: &&String| {
		!constants.contains_key(*field) && !VARYING.contains(&field.as_str())
	}) {
		return Some(format!("unexpected field $.{field}"));
	}
	let Some(attributes) = fields.get("attributes").and_then(Value::as_object) else {
		return Some("$.attributes must be an object".to_owned());
	};
	let empty_lists: [&str; 5] = [
		"abnormalAntis",
		"abnormalComboImmunes",
		"abnormalCombos",
		"abnormalFlags",
		"abnormalImmunes",
	];
	for field in empty_lists {
		if attributes.get(field) != Some(&Value::Array(Vec::new())) {
			return Some(format!("$.attributes.{field} must be empty"));
		}
	}
	if !attributes
		.get("attributeModifiers")
		.is_some_and(Value::is_array)
	{
		return Some("$.attributes.attributeModifiers must be a list".to_owned());
	}
	attributes
		.keys()
		.find(|field: &&String| {
			!empty_lists.contains(&field.as_str()) && field.as_str() != "attributeModifiers"
		})
		.map(|field: &String| format!("unexpected field $.attributes.{field}"))
}

#[cfg(test)]
mod tests {
	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, BlackboardEntry, OperatorBuild, OperatorIdentity,
		OperatorModule, PotentialRank,
	};
	use serde_json::Value;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab_asset, BattlePrefabGraph, BattlePrefabRootSection,
		},
	};

	use super::{
		decode_charge_cost_buff, has_reviewed_charge_cost_shape, is_charge_cost_buff,
		serialized_buff, ChargeCostBuffRejection,
	};

	/// Every common skill root that carries a `charge_cost` buff, with one operator and skill slot
	/// that selects it.
	const CHARGE_COST_PREFABS: [(&str, &str, u8); 25] = [
		("skcom_charge_cost", "char_198_blackd", 1),
		("skchr_acpion_1", "char_608_acpion", 1),
		("skchr_acpion_3", "char_608_acpion", 3),
		("skchr_apionr_1", "char_513_apionr", 1),
		("skchr_blackd_2", "char_198_blackd", 2),
		("skchr_blkngt_1", "char_476_blkngt", 1),
		("skchr_blkngt_2", "char_476_blkngt", 2),
		("skchr_bstalk_1", "char_452_bstalk", 1),
		("skchr_buildr_2", "char_488_buildr", 2),
		("skchr_chiave_2", "char_349_chiave", 2),
		("skchr_closur_2", "char_4228_closur", 2),
		("skchr_flamtl_1", "char_420_flamtl", 1),
		("skchr_flamtl_2", "char_420_flamtl", 2),
		("skchr_makiri_1", "char_4199_makiri", 1),
		("skchr_mlyss_2", "char_249_mlyss", 2),
		("skchr_mlyss_3", "char_249_mlyss", 3),
		("skchr_saga_2", "char_362_saga", 2),
		("skchr_scave_2", "char_149_scave", 2),
		("skchr_sleach_3", "char_479_sleach", 3),
		("skchr_texas_2", "char_102_texas", 2),
		("skchr_vigil_1", "char_427_vigil", 1),
		("skchr_vigil_2", "char_427_vigil", 2),
		("skchr_vulpis_1", "char_4026_vulpis", 1),
		("skchr_wintim_1", "char_4208_wintim", 1),
		("skchr_wyvern_1", "char_240_wyvern", 1),
	];

	/// The reviewed-shape buffs, with the amount each grants at the selected skill's top level.
	const REVIEWED_SHAPE_GRANTS: [(&str, &str, u32); 11] = [
		("skchr_blackd_2", "blackd_s_2[once]", 3),
		("skchr_blkngt_1", "charge_cost", 11),
		("skchr_blkngt_2", "charge_cost", 4),
		("skchr_bstalk_1", "charge_cost", 8),
		("skchr_chiave_2", "charge_cost", 13),
		("skchr_flamtl_1", "flamtl_s_1[cost]", 6),
		("skchr_flamtl_2", "flamtl_s_2[cost]", 13),
		("skchr_saga_2", "charge_cost", 4),
		("skchr_saga_2", "charge_cost", 4),
		("skchr_texas_2", "charge_cost", 12),
		("skcom_charge_cost", "charge_cost", 9),
	];

	/// Blackboards of every level of one operator's skill slot, built at its highest elite and
	/// level with no potential, since event operators have none and potential never touches a
	/// skill blackboard. A three-star operator stops at E1 and skill level 7.
	fn skill_level_blackboards(
		snapshot: &ArkDataSnapshot,
		characters: &Value,
		prefab_id: &str,
		operator_id: &str,
		skill: u8,
	) -> Vec<Vec<BlackboardEntry>> {
		let phases: &Vec<Value> = characters[operator_id]["phases"]
			.as_array()
			.unwrap_or_else(|| panic!("{operator_id} has phases"));
		let elite: u8 = u8::try_from(phases.len() - 1).expect("few phases");
		let level: u32 = phases[usize::from(elite)]["maxLevel"]
			.as_u64()
			.and_then(|level| u32::try_from(level).ok())
			.expect("phase max level");
		let mut levels: Vec<Vec<BlackboardEntry>> = Vec::new();
		for skill_level in 1..=10 {
			let build: OperatorBuild = OperatorBuild::new(
				OperatorIdentity::Id(operator_id.to_owned()),
				elite,
				level,
				200,
				skill,
				skill_level,
				PotentialRank::P1,
				OperatorModule::None,
				0,
			);
			let resolved = match snapshot.operators().resolve(&build) {
				Ok(resolved) => resolved,
				Err(error) => {
					assert_eq!(skill_level, 8, "{operator_id} skill {skill}: {error}");
					break;
				}
			};
			assert_eq!(resolved.selected_skill().prefab_id(), Some(prefab_id));
			levels.push(resolved.selected_skill().blackboard().to_vec());
		}
		levels
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn drift_and_missing_amounts_are_rejected_by_name() {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&gamedata_path).expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		let graph: BattlePrefabGraph = interpret_battle_prefab_asset(
			&prefabs,
			"[uc]skills",
			"dyn/battle/prefabs/[uc]skills/skcom_charge_cost.prefab",
			BattlePrefabRootSection::MainAsset,
			"skcom_charge_cost",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret verified common skill graph");
		let (behavior, spec) = graph
			.mono_behaviours
			.iter()
			.find_map(|behavior| {
				behavior
					.buffs
					.iter()
					.find(|spec| is_charge_cost_buff(spec))
					.map(|spec| (behavior, spec))
			})
			.expect("the reviewed buff");
		let raw: &Value = serialized_buff(behavior, spec).expect("the reviewed buff resolves");
		let blackboard: Vec<BlackboardEntry> = snapshot
			.operators()
			.resolve(&OperatorBuild::new(
				OperatorIdentity::Id("char_198_blackd".to_owned()),
				2,
				70,
				200,
				1,
				10,
				PotentialRank::P1,
				OperatorModule::None,
				0,
			))
			.expect("resolve 讯使")
			.selected_skill()
			.blackboard()
			.to_vec();
		let rejection = |raw: &Value, blackboard: &[BlackboardEntry]| {
			decode_charge_cost_buff(spec, raw, blackboard)
				.expect_err("the buff must be rejected")
				.rejection
		};
		assert_eq!(
			decode_charge_cost_buff(spec, raw, &blackboard)
				.expect("the reviewed buff decodes")
				.amount,
			9
		);

		// Stripping the key would read `charge_cost.cost`, which the level does not carry.
		let mut stripped: Value = raw.clone();
		stripped["stripBlackboardParamsWithBuffKey"] = Value::from(1);
		assert_eq!(
			rejection(&stripped, &blackboard),
			ChargeCostBuffRejection::Amount
		);
		assert_eq!(rejection(raw, &[]), ChargeCostBuffRejection::Amount);
		let doubled: Vec<BlackboardEntry> = blackboard.iter().chain(&blackboard).cloned().collect();
		assert_eq!(rejection(raw, &doubled), ChargeCostBuffRejection::Amount);

		for drifted in [
			("stripBlackboardParamsWithBuffKey", Value::from(2)),
			("maxStackCnt", Value::from(2)),
			("buffKey", Value::from("another")),
			("unseenField", Value::from(0)),
		] {
			let mut changed: Value = raw.clone();
			changed[drifted.0] = drifted.1;
			assert_eq!(
				rejection(&changed, &blackboard),
				ChargeCostBuffRejection::Drift,
				"{}",
				drifted.0
			);
		}
		let mut missing: Value = raw.clone();
		missing
			.as_object_mut()
			.expect("object")
			.remove("triggerCnt");
		assert_eq!(
			rejection(&missing, &blackboard),
			ChargeCostBuffRejection::Drift
		);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn every_charge_cost_buff_in_the_common_bundle_is_decoded_or_named() {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&gamedata_path).expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		let characters: Value = serde_json::from_slice(
			&std::fs::read(gamedata_path.join("character_table.json"))
				.expect("read the verified character table"),
		)
		.expect("parse the verified character table");

		let mut occurrences: usize = 0;
		let mut reviewed: Vec<(&str, String, u32)> = Vec::new();
		for (prefab_id, operator_id, skill) in CHARGE_COST_PREFABS {
			let graph: BattlePrefabGraph = interpret_battle_prefab_asset(
				&prefabs,
				"[uc]skills",
				&format!("dyn/battle/prefabs/[uc]skills/{prefab_id}.prefab"),
				BattlePrefabRootSection::MainAsset,
				prefab_id,
				&built_in_battle_prefab_handler_registry(),
			)
			.expect("interpret verified common skill graph");
			let levels: Vec<Vec<BlackboardEntry>> =
				skill_level_blackboards(&snapshot, &characters, prefab_id, operator_id, skill);
			assert!(
				matches!(levels.len(), 7 | 10),
				"{prefab_id} has {} levels",
				levels.len()
			);
			for behavior in &graph.mono_behaviours {
				for spec in behavior
					.buffs
					.iter()
					.filter(|spec| is_charge_cost_buff(spec))
				{
					occurrences += 1;
					let raw = serialized_buff(behavior, spec)
						.unwrap_or_else(|| panic!("{prefab_id} {} resolves", spec.field_path));
					let outcomes: Vec<_> = levels
						.iter()
						.map(|blackboard| decode_charge_cost_buff(spec, raw, blackboard))
						.collect();
					if has_reviewed_charge_cost_shape(spec, raw) {
						// Every level carries exactly one whole amount under the key the buff reads.
						let amounts: Vec<u32> = outcomes
							.into_iter()
							.map(|outcome| {
								outcome
									.unwrap_or_else(|error| panic!("{prefab_id}: {error}"))
									.amount
							})
							.collect();
						reviewed.push((
							prefab_id,
							spec.buff_key.clone().expect("keyed buff"),
							*amounts.last().expect("at least one level"),
						));
					} else {
						for outcome in outcomes {
							let error = outcome.expect_err("an unreviewed variant must not decode");
							assert_eq!(
								error.rejection,
								ChargeCostBuffRejection::UnreviewedVariant,
								"{prefab_id}: {error}"
							);
						}
					}
				}
			}
		}

		assert_eq!(occurrences, 29);
		reviewed.sort();
		assert_eq!(
			reviewed,
			REVIEWED_SHAPE_GRANTS
				.iter()
				.map(|(prefab, key, amount)| (*prefab, (*key).to_owned(), *amount))
				.collect::<Vec<_>>()
		);
	}
}
