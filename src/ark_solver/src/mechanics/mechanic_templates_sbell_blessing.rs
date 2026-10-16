//! Fail-closed decoder for the retaliation-and-revival blessing prefab template.
//!
//! Recognition uses exact serialized schemas, graph shape, and custom-buff identities. Operator
//! identity and localized descriptions are deliberately outside the contract.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{ArkDataSnapshot, OperatorEffectCandidate};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_handlers::{STATIC_BUFF_ABILITY_OLD_TYPE_HASH, STATIC_BUFF_ABILITY_SCRIPT_ID},
	battle_prefab_interpreter::{
		BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemantic, BattlePrefabSemanticIssue,
	},
	combat::{MechanicBehavior, MechanicId, ReplayRepeat, StatusKind},
	mechanic_semantics::{
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticEffect,
		SemanticEffectTarget, SemanticEvent, SemanticLifetime, SemanticProvenance,
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

const RETALIATION_BUFF_KEY: &str = "sbell2_t2[hit_cold]";
const REVIVAL_BUFF_KEY: &str = "sbell2_t2[escape_dead]";
const HP_RATIO_KEY: &str = "hp_ratio";
const COLD_DURATION_KEY: &str = "cold";
const SELF_FREEZE_DURATION_KEY: &str = "freeze";
const ENEMY_FREEZE_DURATION_KEY: &str = "c2e_freeze";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlessingEffectTarget {
	DamageSource,
	Owner,
	EnemiesInOwnerCurrentAttackRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetaliationSemantics {
	pub trigger: SemanticEvent,
	pub target: BlessingEffectTarget,
	pub status: StatusKind,
	pub duration_seconds: SemanticValueExpression,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevivalSemantics {
	pub trigger: SemanticEvent,
	pub maximum_activations_per_deployment: u16,
	pub restored_max_hp_ratio: SemanticValueExpression,
	pub owner_target: BlessingEffectTarget,
	pub owner_status: StatusKind,
	pub owner_status_duration_seconds: SemanticValueExpression,
	pub enemy_target: BlessingEffectTarget,
	pub enemy_status: StatusKind,
	pub enemy_status_duration_seconds: SemanticValueExpression,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlessingTemplateSemantics {
	pub provenance: SemanticProvenance,
	pub retaliation: RetaliationSemantics,
	pub revival: RevivalSemantics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedBlessingTemplate {
	pub retaliation_mechanic_id: MechanicId,
	pub revival_mechanic_id: MechanicId,
	pub semantics: BlessingTemplateSemantics,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_semantic_fact_sources: Vec<BattlePrefabObjectSource>,
}

impl DecodedBlessingTemplate {
	#[must_use]
	pub fn lower_program(&self) -> MechanicSemanticProgram {
		let retaliation: &RetaliationSemantics = &self.semantics.retaliation;
		let revival: &RevivalSemantics = &self.semantics.revival;
		debug_assert_eq!(revival.maximum_activations_per_deployment, 1);
		MechanicSemanticProgram {
			rules: vec![
				MechanicSemanticRule {
					mechanic: self.retaliation_mechanic_id.clone(),
					provenance: self.semantics.provenance.clone(),
					trigger: SemanticTrigger {
						event: retaliation.trigger.clone(),
						filter: SemanticTriggerFilter::AffectedOwner,
						repeat: ReplayRepeat::Unlimited,
					},
					lifetime: SemanticLifetime::OwnerDeployment,
					action: SemanticAction::ApplyEffects(vec![SemanticTargetedEffect {
						target: lower_effect_target(retaliation.target),
						effect: SemanticEffect::ApplyStatus {
							kind: retaliation.status.clone(),
							duration_seconds: retaliation.duration_seconds.clone(),
						},
					}]),
					behavior: MechanicBehavior::SnowColdFreeze,
				},
				MechanicSemanticRule {
					mechanic: self.revival_mechanic_id.clone(),
					provenance: self.semantics.provenance.clone(),
					trigger: SemanticTrigger {
						event: revival.trigger.clone(),
						filter: SemanticTriggerFilter::AffectedOwner,
						repeat: ReplayRepeat::Once,
					},
					lifetime: SemanticLifetime::OwnerDeployment,
					action: SemanticAction::ApplyEffects(vec![
						SemanticTargetedEffect {
							target: lower_effect_target(revival.owner_target),
							effect: SemanticEffect::RestoreHpToCurrentMaxRatio {
								ratio: revival.restored_max_hp_ratio.clone(),
							},
						},
						SemanticTargetedEffect {
							target: lower_effect_target(revival.owner_target),
							effect: SemanticEffect::ApplyStatus {
								kind: revival.owner_status.clone(),
								duration_seconds: revival.owner_status_duration_seconds.clone(),
							},
						},
						SemanticTargetedEffect {
							target: lower_effect_target(revival.enemy_target),
							effect: SemanticEffect::ApplyStatus {
								kind: revival.enemy_status.clone(),
								duration_seconds: revival.enemy_status_duration_seconds.clone(),
							},
						},
					]),
					behavior: MechanicBehavior::LethalRecoveryAndStatus,
				},
			],
		}
	}
}

const fn lower_effect_target(target: BlessingEffectTarget) -> SemanticEffectTarget {
	match target {
		BlessingEffectTarget::DamageSource => SemanticEffectTarget::TriggerEffectSource,
		BlessingEffectTarget::Owner => SemanticEffectTarget::Owner,
		BlessingEffectTarget::EnemiesInOwnerCurrentAttackRange => {
			SemanticEffectTarget::EnemiesInOwnerCurrentAttackRange
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlessingTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl BlessingTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for BlessingTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"retaliation-and-revival template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for BlessingTemplateDecodeError {}

pub fn try_decode_blessing_template(
	graph: &BattlePrefabGraph,
	candidate: &OperatorEffectCandidate,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedBlessingTemplate>, BlessingTemplateDecodeError> {
	if !contains_blessing_identity(graph) {
		return Ok(None);
	}
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(BlessingTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.section != BattlePrefabRootSection::Talents {
		return Err(BlessingTemplateDecodeError::new(
			"root section",
			format!("expected Talents, found {:?}", graph.section),
		));
	}
	validate_candidate(candidate, graph)?;
	let layout: BlessingLayout<'_> = validate_graph_layout(graph)?;
	validate_bridge(layout.bridge)?;
	validate_static_blessing(layout.static_blessing)?;
	let consumed_issues: Vec<BattlePrefabSemanticIssue> =
		validate_and_collect_issues(graph, &layout)?;
	let provenance: SemanticProvenance = SemanticProvenance::BattlePrefab {
		mechanics_revision: graph.mechanics_revision.clone(),
		gamedata_revision: gamedata_revision.to_owned(),
		script_id: STATIC_BUFF_ABILITY_SCRIPT_ID,
		old_type_hash: STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
		source: SemanticSourceLocation {
			logical_path: layout.static_blessing.source.logical_path.clone(),
			cab_name: layout.static_blessing.source.cab_name.clone(),
			path_id: layout.static_blessing.source.path_id,
			field_paths: vec!["$._buffs[0]".to_owned(), "$._buffs[1]".to_owned()],
		},
	};
	let semantics: BlessingTemplateSemantics = BlessingTemplateSemantics {
		provenance,
		retaliation: RetaliationSemantics {
			trigger: SemanticEvent::Damage,
			target: BlessingEffectTarget::DamageSource,
			status: StatusKind::Cold,
			duration_seconds: SemanticValueExpression::Blackboard(COLD_DURATION_KEY.to_owned()),
		},
		revival: RevivalSemantics {
			trigger: SemanticEvent::BeforeLethal,
			maximum_activations_per_deployment: 1,
			restored_max_hp_ratio: SemanticValueExpression::Blackboard(HP_RATIO_KEY.to_owned()),
			owner_target: BlessingEffectTarget::Owner,
			owner_status: StatusKind::Frozen,
			owner_status_duration_seconds: SemanticValueExpression::Blackboard(
				SELF_FREEZE_DURATION_KEY.to_owned(),
			),
			enemy_target: BlessingEffectTarget::EnemiesInOwnerCurrentAttackRange,
			enemy_status: StatusKind::Frozen,
			enemy_status_duration_seconds: SemanticValueExpression::Blackboard(
				ENEMY_FREEZE_DURATION_KEY.to_owned(),
			),
		},
	};
	Ok(Some(DecodedBlessingTemplate {
		retaliation_mechanic_id: MechanicId::new(format!(
			"{}/template/{RETALIATION_BUFF_KEY}/retaliation",
			graph.logical_path
		)),
		revival_mechanic_id: MechanicId::new(format!(
			"{}/template/{REVIVAL_BUFF_KEY}/revival",
			graph.logical_path
		)),
		semantics,
		consumed_issues,
		consumed_semantic_fact_sources: vec![layout.static_blessing.source.clone()],
	}))
}

#[derive(Clone, Copy)]
struct BlessingLayout<'a> {
	bridge: &'a BattlePrefabMonoBehaviour,
	static_blessing: &'a BattlePrefabMonoBehaviour,
}

fn contains_blessing_identity(graph: &BattlePrefabGraph) -> bool {
	graph.mono_behaviours.iter().any(|behavior| {
		behavior.buffs.iter().any(|buff| {
			[buff.buff_key.as_deref(), buff.template_key.as_deref()]
				.into_iter()
				.flatten()
				.any(|key| matches!(key, RETALIATION_BUFF_KEY | REVIVAL_BUFF_KEY))
		})
	})
}

fn validate_candidate(
	candidate: &OperatorEffectCandidate,
	graph: &BattlePrefabGraph,
) -> Result<(), BlessingTemplateDecodeError> {
	if candidate.prefab_key() != Some(graph.prefab_name.as_str()) {
		return Err(BlessingTemplateDecodeError::new(
			"candidate",
			format!(
				"prefab key {:?} does not select graph root {:?}",
				candidate.prefab_key(),
				graph.prefab_name
			),
		));
	}
	if candidate.range_id().is_some() || candidate.token_key().is_some() || candidate.hidden() {
		return Err(BlessingTemplateDecodeError::new(
			"candidate",
			format!(
				"expected no range/token override and visible talent, found range={:?}, token={:?}, hidden={}",
				candidate.range_id(), candidate.token_key(), candidate.hidden()
			),
		));
	}
	let mut values: BTreeMap<&str, f64> = BTreeMap::new();
	for entry in candidate.blackboard() {
		if entry.value_string().is_some() {
			return Err(BlessingTemplateDecodeError::new(
				"candidate blackboard",
				format!("entry {:?} unexpectedly has a string value", entry.key()),
			));
		}
		if values.insert(entry.key(), entry.value()).is_some() {
			return Err(BlessingTemplateDecodeError::new(
				"candidate blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: Vec<&str> = vec![
		ENEMY_FREEZE_DURATION_KEY,
		COLD_DURATION_KEY,
		SELF_FREEZE_DURATION_KEY,
		HP_RATIO_KEY,
	];
	if values.keys().copied().collect::<Vec<_>>() != expected_keys {
		return Err(BlessingTemplateDecodeError::new(
			"candidate blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				values.keys().collect::<Vec<_>>()
			),
		));
	}
	for key in [
		COLD_DURATION_KEY,
		SELF_FREEZE_DURATION_KEY,
		ENEMY_FREEZE_DURATION_KEY,
	] {
		let value: f64 = values[key];
		if !value.is_finite() || value < 0.0 {
			return Err(BlessingTemplateDecodeError::new(
				"candidate blackboard",
				format!("duration {key:?} must be finite and nonnegative, found {value}"),
			));
		}
	}
	let hp_ratio: f64 = values[HP_RATIO_KEY];
	if !hp_ratio.is_finite() || !(0.0..=1.0).contains(&hp_ratio) {
		return Err(BlessingTemplateDecodeError::new(
			"candidate blackboard",
			format!("{HP_RATIO_KEY:?} must be in [0, 1], found {hp_ratio}"),
		));
	}
	Ok(())
}

fn validate_graph_layout(
	graph: &BattlePrefabGraph,
) -> Result<BlessingLayout<'_>, BlessingTemplateDecodeError> {
	if graph.root_game_object_indices.len() != 1
		|| graph.game_objects.len() != 1
		|| graph.mono_behaviours.len() != 2
	{
		return Err(BlessingTemplateDecodeError::new(
			"graph shape",
			format!(
				"expected one root GameObject and two behaviors, found roots={}, GameObjects={}, behaviors={}",
				graph.root_game_object_indices.len(),
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	let root_index: usize = graph.root_game_object_indices[0];
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		BlessingTemplateDecodeError::new(
			"graph root",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	if root.name != graph.prefab_name
		|| !root.child_indices.is_empty()
		|| root.mono_behaviour_indices.len() != 2
	{
		return Err(BlessingTemplateDecodeError::new(
			"graph root",
			format!(
				"root must be a leaf named {:?} with two behaviors, found {:?}, children={}, behaviors={}",
				graph.prefab_name,
				root.name,
				root.child_indices.len(),
				root.mono_behaviour_indices.len()
			),
		));
	}
	let bridge = exact_owned_behavior(
		graph,
		root_index,
		BRIDGE_SCRIPT_ID,
		BRIDGE_OLD_TYPE_HASH,
		"talent bridge",
	)?;
	let static_blessing = exact_owned_behavior(
		graph,
		root_index,
		STATIC_BUFF_ABILITY_SCRIPT_ID,
		STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
		"static blessing",
	)?;
	Ok(BlessingLayout {
		bridge,
		static_blessing,
	})
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_id: [u8; 16],
	old_type_hash: [u8; 16],
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, BlessingTemplateDecodeError> {
	let owner = &graph.game_objects[owner_index];
	let matches: Vec<&BattlePrefabMonoBehaviour> = owner
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index| graph.mono_behaviours.get(index))
		.filter(|behavior| {
			behavior.script_id == Some(script_id) && behavior.old_type_hash == Some(old_type_hash)
		})
		.collect();
	if matches.len() != 1 {
		return Err(BlessingTemplateDecodeError::new(
			component,
			format!(
				"expected one behavior with script {} and schema {}, found {}",
				hex_id(script_id),
				hex_id(old_type_hash),
				matches.len()
			),
		));
	}
	let behavior: &BattlePrefabMonoBehaviour = matches[0];
	if behavior.game_object_index != Some(owner_index) || !behavior.references.is_empty() {
		return Err(BlessingTemplateDecodeError::new(
			component,
			format!(
				"expected owned behavior with no non-null graph edges, found owner={:?}, references={:?}",
				behavior.game_object_index, behavior.references
			),
		));
	}
	Ok(behavior)
}

fn validate_bridge(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), BlessingTemplateDecodeError> {
	validate_configuration(
		behavior,
		"talent bridge",
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
	)?;
	if !behavior.semantic_facts.is_empty() {
		return Err(BlessingTemplateDecodeError::new(
			"talent bridge",
			"unknown bridge unexpectedly emitted semantic facts",
		));
	}
	Ok(())
}

fn validate_static_blessing(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), BlessingTemplateDecodeError> {
	validate_configuration(
		behavior,
		"static blessing",
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [
				expected_custom_buff(RETALIATION_BUFF_KEY),
				expected_custom_buff(REVIVAL_BUFF_KEY)
			],
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 1,
			"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
			"_selector": {"m_FileID":0, "m_PathID":0}
		}),
	)?;
	if behavior.semantic_facts.len() != 1
		|| !matches!(
			&behavior.semantic_facts[0].semantic,
			BattlePrefabSemantic::StaticBuffAbility(_)
		) {
		return Err(BlessingTemplateDecodeError::new(
			"static blessing",
			format!(
				"expected one static-buff fact, found {:?}",
				behavior
					.semantic_facts
					.iter()
					.map(|fact| fact.kind())
					.collect::<Vec<_>>()
			),
		));
	}
	Ok(())
}

fn expected_custom_buff(key: &str) -> Value {
	let base: Value = json!({
		"attributes": {
			"abnormalAntis": [],
			"abnormalComboImmunes": [],
			"abnormalCombos": [],
			"abnormalFlags": [],
			"abnormalImmunes": [],
			"attributeModifiers": []
		},
		"audioSignal": "",
		"blackboard": [],
		"buffKey": key,
		"clearAllStackCntWhenTimeUp": 0,
		"disableOverride": 0,
		"durationKey": "duration",
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
		"lifeTimeType": 2
	});
	merge_configuration_parts(vec![
		base,
		json!({
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
			"templateKey": key,
			"triggerCnt": 0,
			"triggerInterval": -1.0,
			"triggerLifeType": 0,
			"waitFirstTriggerInterval": 1
		}),
	])
}

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	expected: Value,
) -> Result<(), BlessingTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		BlessingTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(BlessingTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	let actual: Value = Value::Object(actual);
	if let Some(detail) = first_value_mismatch(&expected, &actual, "$") {
		return Err(BlessingTemplateDecodeError::new(component, detail));
	}
	Ok(())
}

fn merge_configuration_parts(parts: Vec<Value>) -> Value {
	let mut merged: Map<String, Value> = Map::new();
	for part in parts {
		let fields: Map<String, Value> = part
			.as_object()
			.expect("internal expected configuration part must be an object")
			.clone();
		for (key, value) in fields {
			assert!(
				merged.insert(key.clone(), value).is_none(),
				"duplicate internal expected configuration field {key:?}"
			);
		}
	}
	Value::Object(merged)
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
	layout: &BlessingLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, BlessingTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		BattlePrefabSemanticIssue::UnknownScriptId {
			source: layout.bridge.source.clone(),
			script_id: BRIDGE_SCRIPT_ID,
		},
		custom_buff_issue(layout.static_blessing, "$._buffs[0]", RETALIATION_BUFF_KEY),
		custom_buff_issue(layout.static_blessing, "$._buffs[1]", REVIVAL_BUFF_KEY),
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
		return Err(BlessingTemplateDecodeError::new(
			"semantic issues",
			format!("missing {missing:?}; unexpected {unexpected:?}"),
		));
	}
	Ok(actual)
}

fn custom_buff_issue(
	behavior: &BattlePrefabMonoBehaviour,
	field_path: &str,
	key: &str,
) -> BattlePrefabSemanticIssue {
	BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
		source: behavior.source.clone(),
		field_path: field_path.to_owned(),
		buff_key: Some(key.to_owned()),
		template_key: Some(key.to_owned()),
	}
}

fn issue_sort_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
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
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
		PotentialRank,
	};
	use crate::simulation::TickRate;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab, BattlePrefabGraph, BattlePrefabRootSection,
		},
		combat::{EffectTarget, ExpectedEffect, ReplayRepeat, ReplayTrigger, StatusKind},
		mechanic_semantics::{SemanticEvent, SemanticValueExpression},
	};

	use super::{
		contains_blessing_identity, first_value_mismatch, try_decode_blessing_template,
		BlessingEffectTarget, COLD_DURATION_KEY,
	};

	#[test]
	fn exact_value_comparison_reports_drift_path() {
		let expected = serde_json::json!({"a":{"b":1}});
		let actual = serde_json::json!({"a":{"b":2}});
		assert_eq!(
			first_value_mismatch(&expected, &actual, "$"),
			Some("$.a.b expected 1, found 2".to_owned())
		);
	}

	#[test]
	fn unrelated_empty_graph_is_not_applicable() {
		let graph = BattlePrefabGraph {
			mechanics_revision: "a".repeat(64),
			logical_path: "fixture".to_owned(),
			asset_path: "fixture".to_owned(),
			section: BattlePrefabRootSection::Talents,
			prefab_name: "unrelated".to_owned(),
			root_game_object_indices: Vec::new(),
			game_objects: Vec::new(),
			mono_behaviours: Vec::new(),
			issues: Vec::new(),
		};
		assert!(!contains_blessing_identity(&graph));
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_blessing_decodes_and_lowers_generic_runtime_rules() {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&gamedata_path).expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		let build: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id("char_1046_sbell2".to_owned()),
			2,
			90,
			200,
			2,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		);
		let resolved = snapshot
			.operators()
			.resolve(&build)
			.expect("resolve reviewed local build");
		let candidate = resolved
			.selected_talent_candidates()
			.find(|candidate| candidate.prefab_key() == Some("2"))
			.expect("active blessing talent");
		let graph: BattlePrefabGraph = interpret_battle_prefab(
			&prefabs,
			resolved.operator_id(),
			BattlePrefabRootSection::Talents,
			"2",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed blessing graph");
		let decoded = try_decode_blessing_template(
			&graph,
			candidate,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect("reviewed template must not drift")
		.expect("reviewed template identity must match");
		assert_eq!(decoded.consumed_issues.len(), 3);
		assert_eq!(decoded.consumed_semantic_fact_sources.len(), 1);
		assert_eq!(decoded.semantics.retaliation.trigger, SemanticEvent::Damage);
		assert_eq!(
			decoded.semantics.retaliation.target,
			BlessingEffectTarget::DamageSource
		);
		assert_eq!(
			decoded.semantics.retaliation.duration_seconds,
			SemanticValueExpression::Blackboard(COLD_DURATION_KEY.to_owned())
		);
		assert_eq!(
			decoded.semantics.revival.trigger,
			SemanticEvent::BeforeLethal
		);
		let lowered = decoded
			.lower_program()
			.lower(
				candidate.blackboard(),
				TickRate::new(NonZeroU32::new(30).expect("nonzero")),
			)
			.expect("verified blessing must lower");
		assert_eq!(lowered.on_deploy.len(), 2);
		assert_eq!(lowered.reached_mechanics.len(), 2);
		let ExpectedEffect::RegisterListener(retaliation) = &lowered.on_deploy[0].effect else {
			panic!("retaliation must register a listener");
		};
		assert_eq!(retaliation.trigger, ReplayTrigger::Damage);
		assert_eq!(retaliation.repeat, ReplayRepeat::Unlimited);
		assert_eq!(
			retaliation.effects[0].target,
			EffectTarget::TriggerEffectSource
		);
		assert!(matches!(
			&retaliation.effects[0].effect,
			ExpectedEffect::ApplyStatus(status) if status.kind == StatusKind::Cold
		));
		let ExpectedEffect::RegisterListener(revival) = &lowered.on_deploy[1].effect else {
			panic!("revival must register a listener");
		};
		assert_eq!(revival.trigger, ReplayTrigger::BeforeLethal);
		assert_eq!(revival.repeat, ReplayRepeat::Once);
		assert_eq!(revival.effects.len(), 3);
		assert!(matches!(
			&revival.effects[0].effect,
			ExpectedEffect::RestoreHpToCurrentMaxRatio { .. }
		));
		assert_eq!(revival.effects[2].target, EffectTarget::EnemiesInRange);
	}
}
