//! Fail-closed decoder for the reviewed block-count toggle talent.
//!
//! A single talent root holds a talent bridge, a buff toggle and a block-count checker. The toggle
//! attaches its one buff while the checker passes, and the reviewed checker passes while its owner
//! blocks at least `_blockMinCnt` enemies, with no maximum and no buff on the blocked enemies. The
//! buff is one panel percentage loaded from the talent blackboard, so the decoded talent is a stat
//! multiplier gated by a Blocked-Enemy Count Condition for the owner's deployment.
//!
//! The pinned data does not settle what `_useBlockAsMin` does. 讯使's module holds the same checker
//! with a minimum of one on her two-enemy capacity for "阻挡敌人时", which rules out reading the
//! threshold as the owner's capacity and, since that module has no `cnt`, points to the checker's
//! own minimum. The template still holds only where the minimum, the blackboard `cnt` and the
//! capacity agree for the resolved build. A checker of any other configuration does not claim the
//! root.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{ArkDataSnapshot, OperatorEffectCandidate};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_interpreter::{
		BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabRootSection,
		BattlePrefabSemanticIssue,
	},
	combat::{MechanicBehavior, MechanicId, ReplayRepeat, StatKind, StatModifierBucket},
	mechanic_semantics::{
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticActorCondition,
		SemanticEffect, SemanticEffectTarget, SemanticEvent, SemanticLifetime, SemanticProvenance,
		SemanticSourceLocation, SemanticTargetedEffect, SemanticTrigger, SemanticTriggerFilter,
		SemanticValueExpression,
	},
};

const BRIDGE_SCRIPT_ID: [u8; 16] = [
	0x7f, 0xfd, 0xc8, 0x07, 0x91, 0x6c, 0xa1, 0xea, 0x67, 0x41, 0xf4, 0x11, 0x75, 0xed, 0xd3, 0x5f,
];
const BRIDGE_OLD_TYPE_HASH: [u8; 16] = [
	0x59, 0xd9, 0x43, 0x18, 0x68, 0x25, 0x4c, 0x1f, 0xea, 0x2a, 0x27, 0x5c, 0x39, 0x37, 0x95, 0x98,
];
const BUFF_TOGGLE_SCRIPT_ID: [u8; 16] = [
	0xea, 0x0b, 0x3b, 0x52, 0x81, 0x8a, 0x7f, 0x65, 0x4b, 0x8f, 0xc6, 0xd8, 0x4e, 0x83, 0x9c, 0x6a,
];
const BUFF_TOGGLE_OLD_TYPE_HASH: [u8; 16] = [
	0xca, 0x70, 0xbd, 0xf7, 0xc3, 0xbd, 0x5e, 0x8c, 0x98, 0x8e, 0xcd, 0x66, 0x88, 0x71, 0x04, 0x19,
];
const BLOCK_CHECKER_SCRIPT_ID: [u8; 16] = [
	0x17, 0x10, 0x82, 0x92, 0x15, 0x03, 0x8a, 0xd9, 0x9c, 0xc1, 0x44, 0x42, 0x4d, 0x3b, 0xfa, 0x60,
];
const BLOCK_CHECKER_OLD_TYPE_HASH: [u8; 16] = [
	0xc8, 0xda, 0xb5, 0x7b, 0x5d, 0xef, 0xd5, 0xe6, 0x9c, 0x27, 0xf6, 0x7e, 0xb5, 0x46, 0x65, 0x9d,
];

const EMPTY_BUFF_TEMPLATE_KEY: &str = "empty";
const THRESHOLD_BLACKBOARD_KEY: &str = "cnt";
const DEFENSE_BLACKBOARD_KEY: &str = "def";
/// Serialized `attributeType` of defense, read from the blackboard key [`DEFENSE_BLACKBOARD_KEY`].
const DEFENSE_ATTRIBUTE_TYPE: i64 = 2;
/// Serialized `formulaItem` of a panel percentage (`MULTIPLIER`).
const MULTIPLIER_FORMULA_ITEM: i64 = 1;

/// Exact, operator-independent meaning recovered from the reviewed toggle, checker and buff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockToggleSemantics {
	/// How many enemies the owner must currently block, bound from the talent blackboard.
	pub minimum_blocked_enemies: SemanticValueExpression,
	pub stat: StatKind,
	/// The panel percentage the buff adds while the condition holds.
	pub panel_ratio: SemanticValueExpression,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedBlockToggleTemplate {
	pub mechanic_id: MechanicId,
	pub provenance: SemanticProvenance,
	pub semantics: BlockToggleSemantics,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
}

impl DecodedBlockToggleTemplate {
	/// Lowers the talent to one owner-deployment stat multiplier gated by the blocked-enemy count.
	#[must_use]
	pub fn lower_program(&self) -> MechanicSemanticProgram {
		MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: self.mechanic_id.clone(),
				provenance: self.provenance.clone(),
				trigger: SemanticTrigger {
					event: SemanticEvent::Deploy,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::ApplyEffects(vec![SemanticTargetedEffect {
					target: SemanticEffectTarget::Owner,
					effect: SemanticEffect::ConditionalStatMultiplier {
						condition: SemanticActorCondition::BlockedEnemiesAtLeast {
							count: self.semantics.minimum_blocked_enemies.clone(),
						},
						stat: self.semantics.stat,
						bucket: StatModifierBucket::Panel,
						multiplier: SemanticValueExpression::OnePlus(Box::new(
							self.semantics.panel_ratio.clone(),
						)),
					},
				}]),
				behavior: MechanicBehavior::StatModifier,
			}],
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockToggleTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl BlockToggleTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for BlockToggleTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"block-count toggle template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for BlockToggleTemplateDecodeError {}

#[derive(Clone, Copy)]
struct BlockToggleLayout<'a> {
	bridge: &'a BattlePrefabMonoBehaviour,
	toggle: &'a BattlePrefabMonoBehaviour,
	checker: &'a BattlePrefabMonoBehaviour,
}

/// Decodes the reviewed block-count toggle without operator-ID branching.
///
/// `block_count` is the resolved build's block capacity, which must equal the threshold.
pub fn try_decode_block_toggle_template(
	graph: &BattlePrefabGraph,
	candidate: &OperatorEffectCandidate,
	block_count: u32,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedBlockToggleTemplate>, BlockToggleTemplateDecodeError> {
	let Some(minimum) = claimed_minimum_blocked_enemies(graph) else {
		return Ok(None);
	};
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(BlockToggleTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.section != BattlePrefabRootSection::Talents {
		return Err(BlockToggleTemplateDecodeError::new(
			"root section",
			format!("expected Talents, found {:?}", graph.section),
		));
	}
	validate_candidate(candidate, graph, minimum, block_count)?;
	let layout: BlockToggleLayout<'_> = validate_graph_layout(graph)?;
	validate_bridge(layout.bridge)?;
	validate_checker(layout.checker, minimum)?;
	let buff_key: String = validate_toggle(layout.toggle)?;
	validate_edges_and_facts(&layout)?;
	let consumed_issues: Vec<BattlePrefabSemanticIssue> =
		validate_and_collect_issues(graph, &layout)?;
	Ok(Some(DecodedBlockToggleTemplate {
		mechanic_id: MechanicId::new(format!(
			"{}/template/{buff_key}/block-count-toggle",
			graph.logical_path
		)),
		provenance: SemanticProvenance::BattlePrefab {
			mechanics_revision: graph.mechanics_revision.clone(),
			gamedata_revision: gamedata_revision.to_owned(),
			script_id: BLOCK_CHECKER_SCRIPT_ID,
			old_type_hash: BLOCK_CHECKER_OLD_TYPE_HASH,
			source: SemanticSourceLocation {
				logical_path: layout.checker.source.logical_path.clone(),
				cab_name: layout.checker.source.cab_name.clone(),
				path_id: layout.checker.source.path_id,
				field_paths: vec![
					"$._blockMinCnt".to_owned(),
					"$._blockMaxCnt".to_owned(),
					"$._useBlockAsMin".to_owned(),
					"$._buffsToBlockee".to_owned(),
					qualified_field(layout.toggle, "$._checker"),
					qualified_field(layout.toggle, "$._buffs[0]"),
					qualified_field(layout.toggle, "$._isInverseToggle"),
				],
			},
		},
		semantics: BlockToggleSemantics {
			minimum_blocked_enemies: SemanticValueExpression::Blackboard(
				THRESHOLD_BLACKBOARD_KEY.to_owned(),
			),
			stat: StatKind::Defense,
			panel_ratio: SemanticValueExpression::Blackboard(DEFENSE_BLACKBOARD_KEY.to_owned()),
		},
		consumed_issues,
	}))
}

/// The minimum of the one block-count checker whose configuration is the reviewed "blocking at
/// least N enemies" shape, which is what claims a root for this template. A checker with a maximum,
/// buffs for the blocked enemies or another minimum mode is not reviewed and leaves the root
/// unclaimed.
fn claimed_minimum_blocked_enemies(graph: &BattlePrefabGraph) -> Option<i64> {
	let mut minimums = graph
		.mono_behaviours
		.iter()
		.filter(|behavior| {
			behavior.script_id == Some(BLOCK_CHECKER_SCRIPT_ID)
				&& behavior.old_type_hash == Some(BLOCK_CHECKER_OLD_TYPE_HASH)
		})
		.filter_map(|behavior| {
			let fields: &Map<String, Value> = behavior.raw_fields.as_object()?;
			let minimum: i64 = fields.get("_blockMinCnt")?.as_i64()?;
			let reviewed: bool = minimum >= 1
				&& fields.get("_blockMaxCnt")?.as_i64()? == -1
				&& fields.get("_useBlockAsMin")?.as_i64()? == 1
				&& fields.get("_buffsToBlockee")?.as_array()?.is_empty();
			reviewed.then_some(minimum)
		});
	minimums.next()
}

fn validate_candidate(
	candidate: &OperatorEffectCandidate,
	graph: &BattlePrefabGraph,
	minimum: i64,
	block_count: u32,
) -> Result<(), BlockToggleTemplateDecodeError> {
	if candidate.prefab_key() != Some(graph.prefab_name.as_str()) {
		return Err(BlockToggleTemplateDecodeError::new(
			"candidate",
			format!(
				"prefab key {:?} does not select graph root {:?}",
				candidate.prefab_key(),
				graph.prefab_name
			),
		));
	}
	if candidate.range_id().is_some() || candidate.token_key().is_some() || candidate.hidden() {
		return Err(BlockToggleTemplateDecodeError::new(
			"candidate",
			format!(
				"expected no range/token override and a visible talent, found range={:?}, token={:?}, hidden={}",
				candidate.range_id(),
				candidate.token_key(),
				candidate.hidden()
			),
		));
	}
	let mut values: BTreeMap<&str, f64> = BTreeMap::new();
	for entry in candidate.blackboard() {
		if entry.value_string().is_some() {
			return Err(BlockToggleTemplateDecodeError::new(
				"candidate blackboard",
				format!("entry {:?} unexpectedly has a string value", entry.key()),
			));
		}
		if values.insert(entry.key(), entry.value()).is_some() {
			return Err(BlockToggleTemplateDecodeError::new(
				"candidate blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: [&str; 2] = [THRESHOLD_BLACKBOARD_KEY, DEFENSE_BLACKBOARD_KEY];
	if values.keys().copied().collect::<Vec<&str>>() != expected_keys {
		return Err(BlockToggleTemplateDecodeError::new(
			"candidate blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				values.keys().collect::<Vec<_>>()
			),
		));
	}
	let threshold: f64 = values[THRESHOLD_BLACKBOARD_KEY];
	// The checker's minimum, the blackboard count and the owner's capacity must name one number,
	// since the pinned data does not settle which of them `_useBlockAsMin` makes the threshold.
	if threshold != minimum as f64 || f64::from(block_count) != threshold {
		return Err(BlockToggleTemplateDecodeError::new(
			"threshold",
			format!(
				"checker minimum {minimum}, blackboard {THRESHOLD_BLACKBOARD_KEY:?} {threshold} and block count {block_count} must agree"
			),
		));
	}
	let ratio: f64 = values[DEFENSE_BLACKBOARD_KEY];
	if !ratio.is_finite() || ratio <= 0.0 {
		return Err(BlockToggleTemplateDecodeError::new(
			"candidate blackboard",
			format!("{DEFENSE_BLACKBOARD_KEY:?} must be finite and positive, found {ratio}"),
		));
	}
	Ok(())
}

fn validate_graph_layout(
	graph: &BattlePrefabGraph,
) -> Result<BlockToggleLayout<'_>, BlockToggleTemplateDecodeError> {
	let [root_index] = graph.root_game_object_indices.as_slice() else {
		return Err(BlockToggleTemplateDecodeError::new(
			"graph roots",
			format!(
				"expected one root, found {}",
				graph.root_game_object_indices.len()
			),
		));
	};
	let root_index: usize = *root_index;
	if graph.game_objects.len() != 1 || graph.mono_behaviours.len() != 3 {
		return Err(BlockToggleTemplateDecodeError::new(
			"graph shape",
			format!(
				"expected one GameObject and three behaviors, found {} and {}",
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		BlockToggleTemplateDecodeError::new(
			"graph root",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	if root.name != graph.prefab_name
		|| !root.child_indices.is_empty()
		|| root.mono_behaviour_indices.len() != 3
		|| root.component_sources.len() != 4
	{
		return Err(BlockToggleTemplateDecodeError::new(
			"graph root",
			format!(
				"expected a leaf named {:?} with three behaviors plus its Transform, found {:?}, children={}, behaviors={}, components={}",
				graph.prefab_name,
				root.name,
				root.child_indices.len(),
				root.mono_behaviour_indices.len(),
				root.component_sources.len()
			),
		));
	}
	Ok(BlockToggleLayout {
		bridge: exact_owned_behavior(
			graph,
			root_index,
			BRIDGE_SCRIPT_ID,
			BRIDGE_OLD_TYPE_HASH,
			"talent bridge",
		)?,
		toggle: exact_owned_behavior(
			graph,
			root_index,
			BUFF_TOGGLE_SCRIPT_ID,
			BUFF_TOGGLE_OLD_TYPE_HASH,
			"buff toggle",
		)?,
		checker: exact_owned_behavior(
			graph,
			root_index,
			BLOCK_CHECKER_SCRIPT_ID,
			BLOCK_CHECKER_OLD_TYPE_HASH,
			"block-count checker",
		)?,
	})
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_id: [u8; 16],
	old_type_hash: [u8; 16],
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, BlockToggleTemplateDecodeError> {
	let owner = graph.game_objects.get(owner_index).ok_or_else(|| {
		BlockToggleTemplateDecodeError::new(component, "owner GameObject index is invalid")
	})?;
	let matches: Vec<&BattlePrefabMonoBehaviour> = owner
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index| graph.mono_behaviours.get(index))
		.filter(|behavior| {
			behavior.script_id == Some(script_id) && behavior.old_type_hash == Some(old_type_hash)
		})
		.collect();
	let [behavior] = matches.as_slice() else {
		return Err(BlockToggleTemplateDecodeError::new(
			component,
			format!(
				"expected one behavior with script {} and schema {}, found {}",
				hex_id(script_id),
				hex_id(old_type_hash),
				matches.len()
			),
		));
	};
	if behavior.game_object_index != Some(owner_index) {
		return Err(BlockToggleTemplateDecodeError::new(
			component,
			format!(
				"behavior owner is {:?}, expected {owner_index}",
				behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

fn validate_bridge(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), BlockToggleTemplateDecodeError> {
	validate_configuration(
		behavior,
		"talent bridge",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_affectWhenNotRootTalent": 0,
			"_applyBlackboardBySkill": 0,
			"_applyStrBlackboardBySkill": 0,
			"_applyTalentRange": 0,
			"_applyTalentScale": 0,
			"_attachInDummy": 0,
			"_attackBlackboardModeIndex": -1,
			"_checkValidBySkillIndex": [],
			"_defaultModeIndex": -1,
			"_extraAttackBlackboardModeIndices": [],
			"_fromRawDataToSkill": 0,
			"_influenceSkillBlackboard": 0,
			"_overrideDefaultRangeId": 0,
			"_overwriteParentRangeId": 0,
			"_overwriteTalentKey": "",
			"_scaleCertainKeyFlag": 0,
			"_scaleCertainKeyList": [],
			"_scaleCertainMode": 0,
			"_scaleModeIndices": [],
			"_writeRangeIdToProjectileBlackboard": 0
		}),
	)
}

fn validate_checker(
	behavior: &BattlePrefabMonoBehaviour,
	minimum: i64,
) -> Result<(), BlockToggleTemplateDecodeError> {
	validate_configuration(
		behavior,
		"block-count checker",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_blockMaxCnt": -1,
			"_blockMinCnt": minimum,
			"_buffsToBlockee": [],
			"_useBlockAsMin": 1
		}),
	)
}

/// Checks the toggle attaches exactly one buff of the reviewed shape while its checker passes and
/// nothing when it fails, and returns that buff's key. The key names the buff and never selects
/// behavior, so any nonempty key is accepted.
fn validate_toggle(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<String, BlockToggleTemplateDecodeError> {
	let buff_key: String = behavior
		.raw_fields
		.pointer("/_buffs/0/buffKey")
		.and_then(Value::as_str)
		.filter(|key| !key.is_empty())
		.ok_or_else(|| {
			BlockToggleTemplateDecodeError::new(
				"buff toggle",
				"expected a nonempty $._buffs[0].buffKey",
			)
		})?
		.to_owned();
	validate_configuration(
		behavior,
		"buff toggle",
		&["_checker"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [expected_panel_buff(&buff_key)],
			"_buffsWhenToggleOff": [],
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 0,
			"_isInverseToggle": 0,
			"_metadata": {"blackboardPrefix": "", "namedAsAlias": ""},
			"_selector": {"m_FileID": 0, "m_PathID": 0},
			"_setToggleFalseOnDetached": 0,
			"_unmanagedBuffsWhenToggleOn": []
		}),
	)?;
	Ok(buff_key)
}

/// One unstacked, untimed buff whose sole modifier is a defense panel percentage read from the
/// blackboard key the attribute names, without a duration key or override.
fn expected_panel_buff(buff_key: &str) -> Value {
	json!({
		"attributes": {
			"abnormalAntis": [],
			"abnormalComboImmunes": [],
			"abnormalCombos": [],
			"abnormalFlags": [],
			"abnormalImmunes": [],
			"attributeModifiers": [{
				"attributeType": DEFENSE_ATTRIBUTE_TYPE,
				"fetchBaseValueFromSourceEntity": 0,
				"formulaItem": MULTIPLIER_FORMULA_ITEM,
				"loadFromBlackboard": 1,
				"value": 0.0
			}]
		},
		"audioSignal": "",
		"blackboard": [],
		"buffKey": buff_key,
		"clearAllStackCntWhenTimeUp": 0,
		"disableOverride": 0,
		"durationKey": "",
		"enableInitDirectionFromSource": 0,
		"firstTriggerInterval": -1.0,
		"independentCharacterSource": 0,
		"isDamageMissable": 0,
		"isDurableBuff": 0,
		"isFreezable": 0,
		"isLevitatable": 0,
		"isSilenceable": 0,
		"isStunnable": 0,
		"lifeTime": 0.0,
		"lifeTimeType": 2,
		"loadFromDB": 0,
		"maxStackCnt": 1,
		"maxValidStackCnt": -1,
		"onEventPriority": 0,
		"overrideEffectKey": "",
		"overrideKey": "",
		"overrideOnEventPriority": 0,
		"overrideType": 0,
		"priority": -1,
		"priorityBBKeys": [],
		"refreshRemainingTimeWhenStackMax": 0,
		"statusResistable": 2,
		"stripBlackboardParamsWithBuffKey": 0,
		"takeSnapshotWhenExtend": 0,
		"templateKey": EMPTY_BUFF_TEMPLATE_KEY,
		"triggerCnt": 0,
		"triggerInterval": -1.0,
		"triggerLifeType": 0,
		"waitFirstTriggerInterval": 1
	})
}

fn validate_edges_and_facts(
	layout: &BlockToggleLayout<'_>,
) -> Result<(), BlockToggleTemplateDecodeError> {
	let toggle: &BattlePrefabMonoBehaviour = layout.toggle;
	if !matches!(
		toggle.references.as_slice(),
		[edge] if edge.field_path == "$._checker" && edge.target == layout.checker.source
	) {
		return Err(BlockToggleTemplateDecodeError::new(
			"buff toggle edges",
			format!(
				"expected sole edge $._checker -> checker, found {:?}",
				toggle.references
			),
		));
	}
	for (behavior, component) in [
		(layout.bridge, "talent bridge"),
		(layout.checker, "block-count checker"),
	] {
		if !behavior.references.is_empty() {
			return Err(BlockToggleTemplateDecodeError::new(
				component,
				format!("expected no graph edges, found {:?}", behavior.references),
			));
		}
	}
	for (behavior, component) in [
		(layout.bridge, "talent bridge"),
		(layout.toggle, "buff toggle"),
		(layout.checker, "block-count checker"),
	] {
		if !behavior.semantic_facts.is_empty() || !behavior.unavailable_references.is_empty() {
			return Err(BlockToggleTemplateDecodeError::new(
				component,
				format!(
					"reviewed unknown script unexpectedly emitted typed facts {:?} or unavailable references {:?}",
					behavior
						.semantic_facts
						.iter()
						.map(|fact| fact.kind())
						.collect::<Vec<_>>(),
					behavior.unavailable_references
				),
			));
		}
	}
	Ok(())
}

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	dynamic_reference_fields: &[&str],
	expected: Value,
) -> Result<(), BlockToggleTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		BlockToggleTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(BlockToggleTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	for &field in dynamic_reference_fields {
		if actual.remove(field).is_none() {
			return Err(BlockToggleTemplateDecodeError::new(
				component,
				format!("missing graph-reference field {field:?}"),
			));
		}
	}
	if let Some(detail) = first_value_mismatch(&expected, &Value::Object(actual), "$") {
		return Err(BlockToggleTemplateDecodeError::new(component, detail));
	}
	Ok(())
}

fn first_value_mismatch(expected: &Value, actual: &Value, path: &str) -> Option<String> {
	match (expected, actual) {
		(Value::Object(expected), Value::Object(actual)) => {
			for (key, expected_value) in expected {
				let field_path: String = format!("{path}.{key}");
				let Some(actual_value) = actual.get(key) else {
					return Some(format!("missing field {field_path}"));
				};
				if let Some(detail) =
					first_value_mismatch(expected_value, actual_value, &field_path)
				{
					return Some(detail);
				}
			}
			actual
				.keys()
				.find(|key| !expected.contains_key(*key))
				.map(|key| format!("unexpected field {path}.{key}"))
		}
		(Value::Array(expected), Value::Array(actual)) => {
			if expected.len() != actual.len() {
				return Some(format!(
					"{path} expected {} entries, found {}",
					expected.len(),
					actual.len()
				));
			}
			expected
				.iter()
				.zip(actual)
				.enumerate()
				.find_map(|(index, (expected, actual))| {
					first_value_mismatch(expected, actual, &format!("{path}[{index}]"))
				})
		}
		_ if expected == actual => None,
		_ => Some(format!("{path} expected {expected}, found {actual}")),
	}
}

fn validate_and_collect_issues(
	graph: &BattlePrefabGraph,
	layout: &BlockToggleLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, BlockToggleTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.bridge, BRIDGE_SCRIPT_ID),
		unknown_issue(layout.toggle, BUFF_TOGGLE_SCRIPT_ID),
		unknown_issue(layout.checker, BLOCK_CHECKER_SCRIPT_ID),
	];
	expected.sort_by_key(issue_sort_key);
	let mut actual: Vec<BattlePrefabSemanticIssue> = graph.issues.clone();
	actual.sort_by_key(issue_sort_key);
	if actual != expected {
		let missing: Vec<&BattlePrefabSemanticIssue> = expected
			.iter()
			.filter(|issue| !actual.contains(issue))
			.collect();
		let unexpected: Vec<&BattlePrefabSemanticIssue> = actual
			.iter()
			.filter(|issue| !expected.contains(issue))
			.collect();
		return Err(BlockToggleTemplateDecodeError::new(
			"semantic issues",
			format!("missing {missing:?}; unexpected {unexpected:?}"),
		));
	}
	Ok(actual)
}

fn unknown_issue(
	behavior: &BattlePrefabMonoBehaviour,
	script_id: [u8; 16],
) -> BattlePrefabSemanticIssue {
	BattlePrefabSemanticIssue::UnknownScriptId {
		source: behavior.source.clone(),
		script_id,
	}
}

fn issue_sort_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
}

fn qualified_field(behavior: &BattlePrefabMonoBehaviour, field: &str) -> String {
	format!(
		"{}:{}{field}",
		behavior.source.cab_name, behavior.source.path_id
	)
}

fn hex_id(bytes: [u8; 16]) -> String {
	bytes.iter().fold(String::new(), |mut hex, byte| {
		use std::fmt::Write as _;
		write!(hex, "{byte:02x}").expect("writing to String cannot fail");
		hex
	})
}

#[cfg(test)]
mod tests {
	use std::num::NonZeroU32;

	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorEffectCandidate,
		OperatorIdentity, OperatorModule, PotentialRank, ResolvedOperatorBuild,
	};
	use crate::simulation::TickRate;
	use serde_json::json;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab, BattlePrefabGraph, BattlePrefabRootSection,
			BattlePrefabSemanticIssue,
		},
		combat::{
			ActorCondition, ConditionalStatModifier, EffectTarget, ExpectedEffect, ExpectedValue,
			ReplayTrigger, RuntimeEffectLifetime, StatKind, StatModifier, StatModifierBucket,
		},
		mechanic_semantics::SemanticValueExpression,
	};

	use super::{try_decode_block_toggle_template, DecodedBlockToggleTemplate};

	struct LocalSnapshots {
		snapshot: ArkDataSnapshot,
		prefabs: BattlePrefabSnapshot,
	}

	fn local_snapshots() -> LocalSnapshots {
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&crate::test_support::configured_gamedata_dir())
				.expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&crate::test_support::configured_battle_prefab_dir())
				.expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		LocalSnapshots { snapshot, prefabs }
	}

	fn resolve(
		local: &LocalSnapshots,
		operator_id: &str,
		elite: u8,
		level: u32,
		skill_level: u8,
		potential: PotentialRank,
	) -> ResolvedOperatorBuild {
		local
			.snapshot
			.operators()
			.resolve(&OperatorBuild::new(
				OperatorIdentity::Id(operator_id.to_owned()),
				elite,
				level,
				200,
				1,
				skill_level,
				potential,
				OperatorModule::None,
				0,
			))
			.expect("resolve local build")
	}

	fn talent_one(resolved: &ResolvedOperatorBuild) -> &OperatorEffectCandidate {
		resolved
			.selected_talent_candidates()
			.find(|candidate| candidate.prefab_key() == Some("1"))
			.expect("active talent 1")
	}

	fn talent_graph(local: &LocalSnapshots, operator_id: &str) -> BattlePrefabGraph {
		interpret_battle_prefab(
			&local.prefabs,
			operator_id,
			BattlePrefabRootSection::Talents,
			"1",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret talent root 1")
	}

	fn decode(
		local: &LocalSnapshots,
		graph: &BattlePrefabGraph,
		resolved: &ResolvedOperatorBuild,
	) -> Result<Option<DecodedBlockToggleTemplate>, super::BlockToggleTemplateDecodeError> {
		try_decode_block_toggle_template(
			graph,
			talent_one(resolved),
			resolved.static_stats().block_count(),
			&local.snapshot,
			local.snapshot.resolved_revision(),
		)
	}

	/// The one effect the talent's deployment listener applies, bound to `resolved`'s candidate.
	fn lowered_effect(
		decoded: &DecodedBlockToggleTemplate,
		resolved: &ResolvedOperatorBuild,
	) -> (EffectTarget, ExpectedEffect) {
		let lowered = decoded
			.lower_program()
			.lower(
				talent_one(resolved).blackboard(),
				TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
			)
			.expect("the reviewed talent lowers");
		assert!(lowered.on_battle_start.is_empty());
		let [listener] = lowered.on_deploy.as_slice() else {
			panic!("{:?}", lowered.on_deploy);
		};
		let ExpectedEffect::RegisterListener(listener) = &listener.effect else {
			panic!("{listener:?}");
		};
		assert_eq!(listener.trigger, ReplayTrigger::Deploy);
		assert_eq!(listener.lifetime, RuntimeEffectLifetime::SourceActor);
		let [effect] = listener.effects.as_slice() else {
			panic!("{:?}", listener.effects);
		};
		(effect.target, effect.effect.clone())
	}

	fn defense_while_blocking_two(multiplier: ExpectedValue) -> ExpectedEffect {
		ExpectedEffect::ConditionalStatModifier(ConditionalStatModifier {
			condition: ActorCondition::BlockedEnemiesAtLeast(2),
			modifier: StatModifier {
				stat: StatKind::Defense,
				bucket: StatModifierBucket::Panel,
				multiplier,
				duration_ticks: None,
			},
		})
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_courier_talent_binds_each_builds_defense_bonus() {
		let local: LocalSnapshots = local_snapshots();
		let graph: BattlePrefabGraph = talent_graph(&local, "char_198_blackd");
		let maximal: ResolvedOperatorBuild =
			resolve(&local, "char_198_blackd", 2, 70, 10, PotentialRank::P6);
		let decoded: DecodedBlockToggleTemplate = decode(&local, &graph, &maximal)
			.expect("the reviewed talent must not drift")
			.expect("the block-count checker claims the root");

		assert_eq!(
			decoded.mechanic_id.as_str(),
			"char_198_blackd/template/blackd_t_1/block-count-toggle"
		);
		assert_eq!(
			decoded.semantics.minimum_blocked_enemies,
			SemanticValueExpression::Blackboard("cnt".to_owned())
		);
		assert_eq!(decoded.semantics.stat, StatKind::Defense);
		assert_eq!(decoded.consumed_issues.len(), 3);
		assert!(decoded
			.consumed_issues
			.iter()
			.all(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. })));
		assert_eq!(decoded.consumed_issues.len(), graph.issues.len());

		// The E2 potential-5 candidate adds 19%; an E1 build below that potential adds 8%.
		assert_eq!(
			lowered_effect(&decoded, &maximal),
			(
				EffectTarget::SelfActor,
				defense_while_blocking_two(ExpectedValue::from_scaled(1_190_000))
			)
		);
		let first_elite: ResolvedOperatorBuild =
			resolve(&local, "char_198_blackd", 1, 60, 7, PotentialRank::P1);
		let first_elite_decoded: DecodedBlockToggleTemplate = decode(&local, &graph, &first_elite)
			.expect("the reviewed talent must not drift")
			.expect("the block-count checker claims the root");
		assert_eq!(
			lowered_effect(&first_elite_decoded, &first_elite),
			(
				EffectTarget::SelfActor,
				defense_while_blocking_two(ExpectedValue::from_scaled(1_080_000))
			)
		);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_courier_talent_rejects_drift_and_leaves_other_checkers_unclaimed() {
		let local: LocalSnapshots = local_snapshots();
		let graph: BattlePrefabGraph = talent_graph(&local, "char_198_blackd");
		let resolved: ResolvedOperatorBuild =
			resolve(&local, "char_198_blackd", 2, 70, 10, PotentialRank::P6);
		let with_field = |object: &str, pointer: &str, value: serde_json::Value| {
			let mut drifted: BattlePrefabGraph = graph.clone();
			let behavior = drifted
				.mono_behaviours
				.iter_mut()
				.find(|behavior| behavior.raw_fields.get(object).is_some())
				.expect("behavior carrying the field");
			*behavior
				.raw_fields
				.pointer_mut(pointer)
				.expect("existing field") = value;
			drifted
		};
		let drift_detail = |drifted: &BattlePrefabGraph| -> String {
			decode(&local, drifted, &resolved)
				.expect_err("claimed drift fails closed")
				.to_string()
		};

		assert!(drift_detail(&with_field(
			"_isInverseToggle",
			"/_isInverseToggle",
			json!(1)
		))
		.contains("$._isInverseToggle"));
		assert!(
			drift_detail(&with_field("_buffs", "/_buffs/0/maxStackCnt", json!(2)))
				.contains("$._buffs[0].maxStackCnt")
		);
		assert!(drift_detail(&with_field(
			"_buffs",
			"/_buffs/0/attributes/attributeModifiers/0/attributeType",
			json!(1)
		))
		.contains("attributeType"));
		// A minimum of three still claims the root but disagrees with the blackboard's two.
		assert!(
			drift_detail(&with_field("_blockMinCnt", "/_blockMinCnt", json!(3)))
				.contains("threshold")
		);
		// So does a build whose capacity differs from the threshold.
		assert!(try_decode_block_toggle_template(
			&graph,
			talent_one(&resolved),
			3,
			&local.snapshot,
			local.snapshot.resolved_revision(),
		)
		.expect_err("capacity must match the threshold")
		.to_string()
		.contains("threshold"));

		// A checker bounded above, or passing buffs to the blocked enemies, is another condition.
		for (field, value) in [
			("/_blockMaxCnt", json!(3)),
			("/_useBlockAsMin", json!(0)),
			("/_buffsToBlockee", json!([{}])),
		] {
			assert_eq!(
				decode(&local, &with_field("_blockMinCnt", field, value), &resolved),
				Ok(None),
				"{field}"
			);
		}
		// The other root-level talent roots carry no block-count checker.
		for operator_id in ["char_150_snakek", "char_204_platnm"] {
			let maximal: OperatorBuild = local
				.snapshot
				.operators()
				.maximal_builds(operator_id, 1)
				.expect("maximal builds resolve")
				.into_iter()
				.next()
				.expect("one maximal build");
			let other: ResolvedOperatorBuild = local
				.snapshot
				.operators()
				.resolve(&maximal)
				.expect("resolve maximal build");
			assert_eq!(
				decode(&local, &talent_graph(&local, operator_id), &other),
				Ok(None),
				"{operator_id}"
			);
		}
	}
}
