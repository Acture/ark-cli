//! Fail-closed decoders for reviewed, reusable battle-prefab mechanic templates.
//!
//! A decoder recognizes serialized mechanic identities and graph structure, never an operator ID.
//! Once any template-specific identity is present, partial or drifted data is an error rather than
//! a guessed mechanic.

#[path = "mechanic_templates_shu_skill.rs"]
pub mod shu_skill;

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{ArkDataSnapshot, OperatorEffectCandidate, RangeOffset};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_handlers::{STATIC_BUFF_ABILITY_OLD_TYPE_HASH, STATIC_BUFF_ABILITY_SCRIPT_ID},
	battle_prefab_interpreter::{
		BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemantic, BattlePrefabSemanticIssue,
	},
	combat::{ExpectedValue, MechanicBehavior, MechanicId, ReplayRepeat},
	mechanic_semantics::{
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticActorFilter,
		SemanticBuffModifier, SemanticCellSet, SemanticEffect, SemanticEvent, SemanticLifetime,
		SemanticPeriodicEffect, SemanticProvenance, SemanticResidentBuff, SemanticSourceLocation,
		SemanticTileField, SemanticTrigger, SemanticTriggerFilter, SemanticValueExpression,
	},
	normal_attack::NormalAttackTargetFilter,
};

const BRIDGE_SCRIPT_ID: [u8; 16] = [
	0x7f, 0xfd, 0xc8, 0x07, 0x91, 0x6c, 0xa1, 0xea, 0x67, 0x41, 0xf4, 0x11, 0x75, 0xed, 0xd3, 0x5f,
];
const BRIDGE_OLD_TYPE_HASH: [u8; 16] = [
	0x59, 0xd9, 0x43, 0x18, 0x68, 0x25, 0x4c, 0x1f, 0xea, 0x2a, 0x27, 0x5c, 0x39, 0x37, 0x95, 0x98,
];
const DO_FARM_SCRIPT_ID: [u8; 16] = [
	0xb8, 0x75, 0x1d, 0x4f, 0x8b, 0x9b, 0xff, 0x1f, 0xaf, 0x0c, 0x1b, 0x27, 0x89, 0xd5, 0xe1, 0x1e,
];
const DO_FARM_OLD_TYPE_HASH: [u8; 16] = [
	0x89, 0xfa, 0x26, 0x54, 0x8d, 0x93, 0x68, 0xbd, 0x2b, 0x9e, 0x4f, 0xd2, 0x3a, 0x9c, 0xc8, 0x7d,
];
const OCCUPANT_LISTENER_SCRIPT_ID: [u8; 16] = [
	0x36, 0xfd, 0xc2, 0x1c, 0xe7, 0x18, 0x1c, 0x05, 0x06, 0x9f, 0xd7, 0x2a, 0x90, 0x90, 0x59, 0x0b,
];
const OCCUPANT_LISTENER_OLD_TYPE_HASH: [u8; 16] = [
	0xc3, 0x8c, 0x89, 0x68, 0xd2, 0xeb, 0x7f, 0x1c, 0xb5, 0xde, 0x47, 0x57, 0x7d, 0x80, 0xb5, 0xfe,
];
const ALLY_VALIDATOR_SCRIPT_ID: [u8; 16] = [
	0xe2, 0xdb, 0xc5, 0xc0, 0x77, 0x9c, 0xd9, 0x19, 0x18, 0xdc, 0xd7, 0x47, 0x6e, 0xda, 0xfc, 0xb4,
];
const ALLY_VALIDATOR_OLD_TYPE_HASH: [u8; 16] = [
	0x5a, 0x12, 0x7d, 0x18, 0x03, 0xc6, 0x98, 0x22, 0x55, 0x4e, 0x5c, 0x11, 0xd1, 0x35, 0x25, 0xaa,
];
const RANGE_PROVIDER_SCRIPT_ID: [u8; 16] = [
	0xbe, 0xe2, 0x61, 0x68, 0xad, 0x7b, 0x24, 0xa7, 0x3e, 0x02, 0xe4, 0x55, 0x14, 0xd0, 0xd5, 0xac,
];
const RANGE_PROVIDER_OLD_TYPE_HASH: [u8; 16] = [
	0xa5, 0x32, 0x35, 0x7a, 0x45, 0xdd, 0x6d, 0x81, 0xf3, 0x2c, 0x3a, 0x82, 0xd4, 0xfc, 0x57, 0xb0,
];
const TILE_SELECTOR_SCRIPT_ID: [u8; 16] = [
	0xee, 0xe4, 0x3b, 0xc9, 0xe3, 0x12, 0x56, 0xc7, 0x75, 0x76, 0x23, 0xed, 0x1e, 0xc0, 0xba, 0x1a,
];
const TILE_SELECTOR_OLD_TYPE_HASH: [u8; 16] = [
	0xa6, 0xae, 0x99, 0x94, 0xc4, 0x74, 0x13, 0x8e, 0xe8, 0x23, 0x6b, 0x8c, 0xa6, 0x9d, 0xa0, 0x71,
];

const OWNER_BUFF_KEY: &str = "shu_t_1";
const OCCUPANT_BUFF_KEY: &str = "shu_t_1[char]";
const TILE_MARK_BUFF_KEY: &str = "shu_tile_buff[mark]";
const RANGE_ID: &str = "x-5";
const HEAL_BLACKBOARD_KEY: &str = "hp_recovery_per_sec";
const SHELTER_BLACKBOARD_KEY: &str = "damage_resistance";

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedMechanicTemplate {
	pub program: MechanicSemanticProgram,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_semantic_fact_sources: Vec<BattlePrefabObjectSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MechanicTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl MechanicTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for MechanicTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"sowed-field template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for MechanicTemplateDecodeError {}

/// Decodes the reviewed sowed-field template without consulting operator identity.
///
/// Absence of all template-specific buff identities means the graph is not applicable. Once any
/// identity appears, every script schema, graph edge, configuration, range, blackboard input, and
/// fail-closed issue must match exactly.
pub fn try_decode_sowed_field_template(
	graph: &BattlePrefabGraph,
	candidate: &OperatorEffectCandidate,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedMechanicTemplate>, MechanicTemplateDecodeError> {
	if !contains_sowed_field_identity(graph) {
		return Ok(None);
	}
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(MechanicTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, but the verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.section != BattlePrefabRootSection::Talents {
		return Err(MechanicTemplateDecodeError::new(
			"root section",
			format!("expected Talents, found {:?}", graph.section),
		));
	}
	validate_candidate(candidate, graph)?;

	let layout: SowedFieldLayout<'_> = validate_graph_layout(graph)?;
	validate_bridge(layout.trigger_bridge, "trigger bridge")?;
	validate_bridge(layout.owner_bridge, "owner bridge")?;
	validate_do_farm(layout.do_farm)?;
	validate_occupant_listener(layout.occupant_listener)?;
	validate_ally_validator(layout.ally_validator)?;
	validate_range_provider(layout.range_provider)?;
	validate_tile_selector(layout.tile_selector)?;
	validate_static_owner_buff(layout.static_owner_buff)?;
	validate_reference_edge(
		layout.do_farm,
		"$._selector",
		layout.tile_selector,
		"DoFarm selector",
	)?;
	validate_reference_edge(
		layout.occupant_listener,
		"$._targetValidator",
		layout.ally_validator,
		"occupant listener validator",
	)?;

	let range = snapshot.resolve_range(RANGE_ID).map_err(|error| {
		MechanicTemplateDecodeError::new(
			"range table",
			format!("cannot resolve {RANGE_ID:?}: {error}"),
		)
	})?;
	let expected_offsets: Vec<RangeOffset> = vec![
		RangeOffset::new(1, 0),
		RangeOffset::new(0, -1),
		RangeOffset::new(0, 0),
		RangeOffset::new(0, 1),
		RangeOffset::new(-1, 0),
	];
	if range.source_direction() != 1 || range.offsets() != expected_offsets {
		return Err(MechanicTemplateDecodeError::new(
			"range table",
			format!(
				"{RANGE_ID:?} must be the reviewed center-plus-four-orthogonal cross; direction={}, offsets={:?}",
				range.source_direction(),
				range.offsets()
			),
		));
	}

	let consumed_issues: Vec<BattlePrefabSemanticIssue> =
		validate_and_collect_issues(graph, &layout)?;
	let provenance: SemanticProvenance = SemanticProvenance::BattlePrefab {
		mechanics_revision: graph.mechanics_revision.clone(),
		gamedata_revision: gamedata_revision.to_owned(),
		script_id: DO_FARM_SCRIPT_ID,
		old_type_hash: DO_FARM_OLD_TYPE_HASH,
		source: SemanticSourceLocation {
			logical_path: layout.do_farm.source.logical_path.clone(),
			cab_name: layout.do_farm.source.cab_name.clone(),
			path_id: layout.do_farm.source.path_id,
			field_paths: vec![
				"$._metadata.namedAsAlias".to_owned(),
				"$._selector".to_owned(),
				qualified_field(layout.range_provider, "$._rangeId"),
				qualified_field(layout.tile_selector, "$._options"),
				qualified_field(layout.occupant_listener, "$._buffs[0]"),
				qualified_field(layout.occupant_listener, "$._buffsWhenCasted[0]"),
				qualified_field(layout.ally_validator, "$._targetOptions"),
				qualified_field(layout.static_owner_buff, "$._buffs[0]"),
			],
		},
	};
	let mechanic: MechanicId = MechanicId::new(format!(
		"{}/template/{OWNER_BUFF_KEY}/sowed-field",
		graph.logical_path
	));
	let program: MechanicSemanticProgram = MechanicSemanticProgram {
		rules: vec![MechanicSemanticRule {
			mechanic: mechanic.clone(),
			provenance,
			trigger: SemanticTrigger {
				event: SemanticEvent::Heal,
				filter: SemanticTriggerFilter::EffectSourceOwner,
				repeat: ReplayRepeat::Unlimited,
			},
			lifetime: SemanticLifetime::OwnerDeployment,
			action: SemanticAction::AddTileField(SemanticTileField {
				key: mechanic,
				cells: SemanticCellSet::TriggerActorRelative(range.offsets().to_vec()),
				actor_filter: SemanticActorFilter::AllyOfOwner,
				target_filter: NormalAttackTargetFilter::GroundAndAir,
				max_layers_per_cell: SemanticValueExpression::Literal(ExpectedValue::from_whole(1)),
				lifetime: SemanticLifetime::Persistent,
				on_enter_once: Vec::new(),
				resident_buffs: vec![SemanticResidentBuff {
					key_suffix: OCCUPANT_BUFF_KEY.to_owned(),
					max_stacks: SemanticValueExpression::Literal(ExpectedValue::from_whole(1)),
					modifiers_per_stack: vec![SemanticBuffModifier::Shelter {
						incoming_damage_multiplier: SemanticValueExpression::OneMinus(Box::new(
							SemanticValueExpression::Blackboard(SHELTER_BLACKBOARD_KEY.to_owned()),
						)),
					}],
				}],
				layer_threshold_reactions: Vec::new(),
				periodic: Some(SemanticPeriodicEffect {
					initial_delay_millis: 1_000,
					interval_millis: 1_000,
					effects_per_layer: vec![SemanticEffect::Healing {
						amount: SemanticValueExpression::Blackboard(HEAL_BLACKBOARD_KEY.to_owned()),
					}],
				}),
				remove_on_first_matching_leave: false,
			}),
			behavior: MechanicBehavior::ShelterExposureOrDirectHit,
		}],
	};

	Ok(Some(DecodedMechanicTemplate {
		program,
		consumed_issues,
		consumed_semantic_fact_sources: vec![layout.static_owner_buff.source.clone()],
	}))
}

#[derive(Clone, Copy)]
struct SowedFieldLayout<'a> {
	trigger_bridge: &'a BattlePrefabMonoBehaviour,
	owner_bridge: &'a BattlePrefabMonoBehaviour,
	do_farm: &'a BattlePrefabMonoBehaviour,
	occupant_listener: &'a BattlePrefabMonoBehaviour,
	ally_validator: &'a BattlePrefabMonoBehaviour,
	range_provider: &'a BattlePrefabMonoBehaviour,
	tile_selector: &'a BattlePrefabMonoBehaviour,
	static_owner_buff: &'a BattlePrefabMonoBehaviour,
}

fn contains_sowed_field_identity(graph: &BattlePrefabGraph) -> bool {
	graph.mono_behaviours.iter().any(|behaviour| {
		behaviour.buffs.iter().any(|buff| {
			matches!(
				buff.buff_key.as_deref(),
				Some(OWNER_BUFF_KEY | OCCUPANT_BUFF_KEY)
			) || matches!(
				buff.template_key.as_deref(),
				Some(OWNER_BUFF_KEY | OCCUPANT_BUFF_KEY)
			)
		}) || value_contains_string(&behaviour.raw_fields, TILE_MARK_BUFF_KEY)
	})
}

fn value_contains_string(value: &Value, expected: &str) -> bool {
	match value {
		Value::String(actual) => actual == expected,
		Value::Array(values) => values
			.iter()
			.any(|value| value_contains_string(value, expected)),
		Value::Object(fields) => fields
			.values()
			.any(|value| value_contains_string(value, expected)),
		_ => false,
	}
}

fn validate_candidate(
	candidate: &OperatorEffectCandidate,
	graph: &BattlePrefabGraph,
) -> Result<(), MechanicTemplateDecodeError> {
	if candidate.prefab_key() != Some(graph.prefab_name.as_str()) {
		return Err(MechanicTemplateDecodeError::new(
			"candidate",
			format!(
				"prefab key {:?} does not select interpreted root {:?}",
				candidate.prefab_key(),
				graph.prefab_name
			),
		));
	}
	if candidate.range_id().is_some() || candidate.token_key().is_some() || candidate.hidden() {
		return Err(MechanicTemplateDecodeError::new(
			"candidate",
			format!(
				"expected no candidate range/token override and a visible talent, found range={:?}, token={:?}, hidden={}",
				candidate.range_id(),
				candidate.token_key(),
				candidate.hidden()
			),
		));
	}
	let mut values: BTreeMap<&str, f64> = BTreeMap::new();
	for entry in candidate.blackboard() {
		if entry.value_string().is_some() {
			return Err(MechanicTemplateDecodeError::new(
				"candidate blackboard",
				format!("entry {:?} unexpectedly has a string value", entry.key()),
			));
		}
		if values.insert(entry.key(), entry.value()).is_some() {
			return Err(MechanicTemplateDecodeError::new(
				"candidate blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: Vec<&str> = vec![SHELTER_BLACKBOARD_KEY, HEAL_BLACKBOARD_KEY];
	if values.keys().copied().collect::<Vec<_>>() != expected_keys {
		return Err(MechanicTemplateDecodeError::new(
			"candidate blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				values.keys().collect::<Vec<_>>()
			),
		));
	}
	let heal: f64 = values[HEAL_BLACKBOARD_KEY];
	let shelter: f64 = values[SHELTER_BLACKBOARD_KEY];
	if !heal.is_finite() || heal < 0.0 {
		return Err(MechanicTemplateDecodeError::new(
			"candidate blackboard",
			format!("{HEAL_BLACKBOARD_KEY:?} must be finite and nonnegative, found {heal}"),
		));
	}
	if !shelter.is_finite() || !(0.0..=1.0).contains(&shelter) {
		return Err(MechanicTemplateDecodeError::new(
			"candidate blackboard",
			format!("{SHELTER_BLACKBOARD_KEY:?} must be in [0, 1], found {shelter}"),
		));
	}
	Ok(())
}

fn validate_graph_layout(
	graph: &BattlePrefabGraph,
) -> Result<SowedFieldLayout<'_>, MechanicTemplateDecodeError> {
	if graph.root_game_object_indices.len() != 2 {
		return Err(MechanicTemplateDecodeError::new(
			"graph roots",
			format!(
				"expected exactly two parallel talent roots, found {}",
				graph.root_game_object_indices.len()
			),
		));
	}
	let mut trigger_root: Option<usize> = None;
	let mut owner_root: Option<usize> = None;
	for &root_index in &graph.root_game_object_indices {
		let root = graph.game_objects.get(root_index).ok_or_else(|| {
			MechanicTemplateDecodeError::new(
				"graph roots",
				format!("root index {root_index} is outside the GameObject table"),
			)
		})?;
		if root.name != graph.prefab_name {
			return Err(MechanicTemplateDecodeError::new(
				"graph roots",
				format!(
					"root {:?} is named {:?}, expected prefab name {:?}",
					root.source, root.name, graph.prefab_name
				),
			));
		}
		let pairs: Vec<([u8; 16], [u8; 16])> = root
			.mono_behaviour_indices
			.iter()
			.map(|&index| behavior_pair(graph, index, "root component"))
			.collect::<Result<_, _>>()?;
		if pairs.contains(&(DO_FARM_SCRIPT_ID, DO_FARM_OLD_TYPE_HASH)) {
			if trigger_root.replace(root_index).is_some() {
				return Err(MechanicTemplateDecodeError::new(
					"graph roots",
					"multiple roots contain the DoFarm behavior",
				));
			}
		} else if pairs.contains(&(
			STATIC_BUFF_ABILITY_SCRIPT_ID,
			STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
		)) {
			if owner_root.replace(root_index).is_some() {
				return Err(MechanicTemplateDecodeError::new(
					"graph roots",
					"multiple roots contain the static owner buff",
				));
			}
		} else {
			return Err(MechanicTemplateDecodeError::new(
				"graph roots",
				format!("root {:?} matches neither reviewed role", root.source),
			));
		}
	}
	let trigger_root_index: usize = trigger_root.ok_or_else(|| {
		MechanicTemplateDecodeError::new("graph roots", "missing DoFarm trigger root")
	})?;
	let owner_root_index: usize = owner_root.ok_or_else(|| {
		MechanicTemplateDecodeError::new("graph roots", "missing static owner-buff root")
	})?;
	let trigger = &graph.game_objects[trigger_root_index];
	let owner = &graph.game_objects[owner_root_index];
	if trigger.child_indices.len() != 1 || !owner.child_indices.is_empty() {
		return Err(MechanicTemplateDecodeError::new(
			"graph hierarchy",
			format!(
				"trigger root must have one selector child and owner root none; found {} and {}",
				trigger.child_indices.len(),
				owner.child_indices.len()
			),
		));
	}
	let selector = graph
		.game_objects
		.get(trigger.child_indices[0])
		.ok_or_else(|| {
			MechanicTemplateDecodeError::new(
				"graph hierarchy",
				"selector child index is outside the GameObject table",
			)
		})?;
	if selector.name != "Selector" || !selector.child_indices.is_empty() {
		return Err(MechanicTemplateDecodeError::new(
			"graph hierarchy",
			format!(
				"expected a leaf child named \"Selector\", found {:?} with {} children",
				selector.name,
				selector.child_indices.len()
			),
		));
	}

	let trigger_bridge = exact_owned_behavior(
		graph,
		trigger_root_index,
		BRIDGE_SCRIPT_ID,
		BRIDGE_OLD_TYPE_HASH,
		"trigger bridge",
	)?;
	let owner_bridge = exact_owned_behavior(
		graph,
		owner_root_index,
		BRIDGE_SCRIPT_ID,
		BRIDGE_OLD_TYPE_HASH,
		"owner bridge",
	)?;
	let do_farm = exact_owned_behavior(
		graph,
		trigger_root_index,
		DO_FARM_SCRIPT_ID,
		DO_FARM_OLD_TYPE_HASH,
		"DoFarm",
	)?;
	let occupant_listener = exact_owned_behavior(
		graph,
		trigger_root_index,
		OCCUPANT_LISTENER_SCRIPT_ID,
		OCCUPANT_LISTENER_OLD_TYPE_HASH,
		"occupant listener",
	)?;
	let ally_validator = exact_owned_behavior(
		graph,
		trigger_root_index,
		ALLY_VALIDATOR_SCRIPT_ID,
		ALLY_VALIDATOR_OLD_TYPE_HASH,
		"ally validator",
	)?;
	let range_provider = exact_owned_behavior(
		graph,
		trigger.child_indices[0],
		RANGE_PROVIDER_SCRIPT_ID,
		RANGE_PROVIDER_OLD_TYPE_HASH,
		"range provider",
	)?;
	let tile_selector = exact_owned_behavior(
		graph,
		trigger.child_indices[0],
		TILE_SELECTOR_SCRIPT_ID,
		TILE_SELECTOR_OLD_TYPE_HASH,
		"tile selector",
	)?;
	let static_owner_buff = exact_owned_behavior(
		graph,
		owner_root_index,
		STATIC_BUFF_ABILITY_SCRIPT_ID,
		STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
		"static owner buff",
	)?;

	let expected_trigger_components: usize = 4;
	let expected_owner_components: usize = 2;
	let expected_selector_components: usize = 2;
	if trigger.mono_behaviour_indices.len() != expected_trigger_components
		|| owner.mono_behaviour_indices.len() != expected_owner_components
		|| selector.mono_behaviour_indices.len() != expected_selector_components
	{
		return Err(MechanicTemplateDecodeError::new(
			"graph components",
			format!(
				"expected trigger/owner/selector behavior counts 4/2/2, found {}/{}/{}",
				trigger.mono_behaviour_indices.len(),
				owner.mono_behaviour_indices.len(),
				selector.mono_behaviour_indices.len()
			),
		));
	}
	if graph.mono_behaviours.len() != 8 || graph.game_objects.len() != 3 {
		return Err(MechanicTemplateDecodeError::new(
			"graph shape",
			format!(
				"expected 3 GameObjects and 8 behaviors, found {} and {}",
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	Ok(SowedFieldLayout {
		trigger_bridge,
		owner_bridge,
		do_farm,
		occupant_listener,
		ally_validator,
		range_provider,
		tile_selector,
		static_owner_buff,
	})
}

fn behavior_pair(
	graph: &BattlePrefabGraph,
	index: usize,
	component: &'static str,
) -> Result<([u8; 16], [u8; 16]), MechanicTemplateDecodeError> {
	let behavior = graph.mono_behaviours.get(index).ok_or_else(|| {
		MechanicTemplateDecodeError::new(
			component,
			format!("behavior index {index} is outside the behavior table"),
		)
	})?;
	match (behavior.script_id, behavior.old_type_hash) {
		(Some(script_id), Some(old_type_hash)) => Ok((script_id, old_type_hash)),
		actual => Err(MechanicTemplateDecodeError::new(
			component,
			format!("missing script/schema identity: {actual:?}"),
		)),
	}
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_id: [u8; 16],
	old_type_hash: [u8; 16],
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, MechanicTemplateDecodeError> {
	let owner = graph.game_objects.get(owner_index).ok_or_else(|| {
		MechanicTemplateDecodeError::new(component, "owner GameObject index is invalid")
	})?;
	let matches: Vec<&BattlePrefabMonoBehaviour> = owner
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index| graph.mono_behaviours.get(index))
		.filter(|behavior| {
			behavior.script_id == Some(script_id) && behavior.old_type_hash == Some(old_type_hash)
		})
		.collect();
	if matches.len() != 1 {
		return Err(MechanicTemplateDecodeError::new(
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
	if behavior.game_object_index != Some(owner_index) {
		return Err(MechanicTemplateDecodeError::new(
			component,
			format!(
				"behavior owner is {:?}, expected GameObject index {owner_index}",
				behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

fn validate_bridge(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), MechanicTemplateDecodeError> {
	validate_configuration(
		behavior,
		component,
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

fn validate_do_farm(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), MechanicTemplateDecodeError> {
	let effect_settings: Value = json!([{
		"audioSignal": "",
		"castedTimeThreshold": 1,
		"effectKey": "shu_talent_buff_01",
		"startEffectKey": ""
	}]);
	let expected: Value = merge_configuration_parts(vec![
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_activeBuffs": [],
			"_affectedBySlowDown": 0,
			"_allowNoTarget": 0,
			"_alwaysIncludeInputTargetRootTile": 0,
			"_alwaysIncludeTarget": 1,
			"_animKey": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_audioSignal": "",
			"_checkFaceToFirstCastTargetDownAttack": 0,
			"_cooldown": 0.0,
			"_cooldownKey": "duration",
			"_dontCreateNewEffectWhenCastedTimesSame": 0,
			"_downAnimKey": "",
			"_downEndAnimKey": "",
			"_effectSettings": effect_settings,
			"_endAnimKey": "",
			"_escapeTime": 0.0,
			"_extraStartEffects": []
		}),
		json!({
			"_faceToDefault": 0,
			"_faceToFirstCastTarget": 0,
			"_faceToFront": 0,
			"_faceToTarget": 1,
			"_faceToTile": 0,
			"_finishStartEffectsOnCastOnTile": 0,
			"_finishStartEffectsOnDetached": 0,
			"_finishTileStartEffectWhenCast": 0,
			"_fixCastDirectly": 0,
			"_fixCastToTarget": 0,
			"_forceAsDmgOrHealAbility": 0,
			"_forceFaceToDirection": 0,
			"_ignoreIfOwnerDead": 0,
			"_ignorePalsyInterrupt": 0,
			"_interruptAbilityOnDetach": 1,
			"_interuptIfTargetDead": 0,
			"_maxAnimScale": -1.0,
			"_metadata": {"blackboardPrefix": "", "namedAsAlias": "DoFarm"},
			"_onlyPlayEndAnimWhenIsLastAbility": 0,
			"_playAudioWhenEffectChanged": 0
		}),
		json!({
			"_preDelay": 0.0,
			"_resetCdStrategy": 1,
			"_selectTargetSource": 2,
			"_selectTargetTiming": 0,
			"_startEffect": "",
			"_tileEffect": "",
			"_tileHoldEffect": 0,
			"_timeMode": 2,
			"_upAnimKey": "",
			"_upEndAnimKey": "",
			"_useDynamicAttackType": 0,
			"_useOwnerRootTileAsTargetSource": 0,
			"_waitForAttackEvent": 0
		}),
	]);
	validate_configuration(behavior, "DoFarm", &["_selector"], expected)
}

fn validate_occupant_listener(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), MechanicTemplateDecodeError> {
	let buff: Value = expected_buff_configuration(OCCUPANT_BUFF_KEY, true, true, 0);
	validate_configuration(
		behavior,
		"occupant listener",
		&["_targetValidator"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_buffs": [buff.clone()],
			"_buffsWhenCasted": [buff],
			"_clearBuffWhenAbilityDetached": 0,
			"_clearBuffWhenTileListenerRemoved": 0,
			"_useBlackList": 1,
			"_whiteListBuffKeys": [TILE_MARK_BUFF_KEY]
		}),
	)
}

fn validate_ally_validator(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), MechanicTemplateDecodeError> {
	validate_configuration(
		behavior,
		"ally validator",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_targetOptions": {
				"abnormalCombo": 0,
				"abnormalFlag": 0,
				"checkUnitType": 0,
				"containAbnormalFlag": 0,
				"containSomeAbnormalFlags": 0,
				"enableAdvancedOptions": 1,
				"excludeAbnormalFlag": 0,
				"excludeSomeAbnormalFlags": 0,
				"ignoreAllyTargetFree": 0,
				"ignoreHealFree": 0,
				"ignoreTargetFree": 0,
				"ignoreTargetSide": 0,
				"onlyIgnoreSomeOfTargetFreeCase": 0,
				"professionMask": 767,
				"purposeMask": 0,
				"targetCategory": 1,
				"targetMotion": 3,
				"targetSide": 1,
				"unitTypeMask": 0
			}
		}),
	)
}

fn validate_range_provider(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), MechanicTemplateDecodeError> {
	validate_configuration(
		behavior,
		"range provider",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_extendable": 0,
			"_rangeId": RANGE_ID
		}),
	)
}

fn validate_tile_selector(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), MechanicTemplateDecodeError> {
	let options: Value = json!({
		"advancedBuildableMask": 1,
		"advancedBuildableMaskExcept": 0,
		"allowAllAdvancedBuildableMask": 1,
		"allowNoneBuildableType": 0,
		"allowNonePassableMask": 0,
		"allowedTileBlackboardKey": "",
		"buildableType": 3,
		"checkBuildableOrPassable": 1,
		"checkExtraBuildableCheckers": 0,
		"checkHeightType": 0,
		"checkTileHidden": 0,
		"checkTileMode": 0,
		"checkTileMoveCost": 0,
		"checkTileTypes": 0,
		"exceptTileTypes": 0,
		"excludeTileBlackboardKey": "",
		"heightType": 0,
		"modeIndex": 0,
		"moveCostCompareType": 0,
		"moveCostThreshold": 0,
		"passableMask": 1,
		"tileTypesMask": 1
	});
	let expected: Value = merge_configuration_parts(vec![
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_alwaysRandomInTheEnd": 1,
			"_alwaysSort": 0,
			"_blackboardKey": "",
			"_blackboardKeys": [],
			"_checkUnitType": 0,
			"_chrBuffKey": "",
			"_distCompareType": 0,
			"_distValue": 0,
			"_enemyBuffKey": "",
			"_enemyKeys": [],
			"_excludeAllCharacter": 0,
			"_excludeCamouflagCharacter": 0,
			"_excludeOwnerRootTile": 0,
			"_extraFilterTypeList": [],
			"_filterBlackboardKey": "",
			"_filterType": 0,
			"_fixedDistance": 0,
			"_ignoreBlackList": 0
		}),
		json!({
			"_ignoreHitRange": 0,
			"_limitTargetNum": 0,
			"_maxNum": 1,
			"_needProfessionMask": 0,
			"_options": options,
			"_randomBeforeShrink": 0,
			"_targetCategory": 1,
			"_targetMotion": 1,
			"_targetSide": 2,
			"_tileOrTokenIdListPreferred": [],
			"_tileSortType": 0,
			"_unitTypeMask": 0
		}),
	]);
	validate_configuration(behavior, "tile selector", &[], expected)
}

fn validate_static_owner_buff(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), MechanicTemplateDecodeError> {
	validate_configuration(
		behavior,
		"static owner buff",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [expected_buff_configuration(OWNER_BUFF_KEY, false, false, 2)],
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 1,
			"_metadata": {"blackboardPrefix": "", "namedAsAlias": ""},
			"_selector": {"m_FileID": 0, "m_PathID": 0}
		}),
	)?;
	if behavior.semantic_facts.len() != 1
		|| !matches!(
			behavior.semantic_facts[0].semantic,
			BattlePrefabSemantic::StaticBuffAbility(_)
		) {
		return Err(MechanicTemplateDecodeError::new(
			"static owner buff",
			format!(
				"expected exactly one typed static-buff fact, found {:?}",
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

fn expected_buff_configuration(
	key: &str,
	disable_override: bool,
	independent_source: bool,
	lifetime_type: i64,
) -> Value {
	json!({
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
		"disableOverride": i32::from(disable_override),
		"durationKey": "duration",
		"enableInitDirectionFromSource": 0,
		"firstTriggerInterval": -1.0,
		"independentCharacterSource": i32::from(independent_source),
		"isDamageMissable": 0,
		"isDurableBuff": 0,
		"isFreezable": 0,
		"isLevitatable": 0,
		"isSilenceable": 0,
		"isStunnable": 0,
		"lifeTime": 0.0,
		"lifeTimeType": lifetime_type,
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
	})
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

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	dynamic_reference_fields: &[&str],
	expected: Value,
) -> Result<(), MechanicTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		MechanicTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(MechanicTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	for &field in dynamic_reference_fields {
		if actual.remove(field).is_none() {
			return Err(MechanicTemplateDecodeError::new(
				component,
				format!("missing graph-reference field {field:?}"),
			));
		}
	}
	let actual: Value = Value::Object(actual);
	if let Some(detail) = first_value_mismatch(&expected, &actual, "$") {
		return Err(MechanicTemplateDecodeError::new(component, detail));
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

fn validate_reference_edge(
	source: &BattlePrefabMonoBehaviour,
	field_path: &str,
	target: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), MechanicTemplateDecodeError> {
	if source.references.len() != 1
		|| source.references[0].field_path != field_path
		|| source.references[0].target != target.source
	{
		return Err(MechanicTemplateDecodeError::new(
			component,
			format!(
				"expected sole edge {field_path} -> {:?}, found {:?}",
				target.source, source.references
			),
		));
	}
	Ok(())
}

fn validate_and_collect_issues(
	graph: &BattlePrefabGraph,
	layout: &SowedFieldLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, MechanicTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.trigger_bridge, BRIDGE_SCRIPT_ID),
		unknown_issue(layout.owner_bridge, BRIDGE_SCRIPT_ID),
		unknown_issue(layout.do_farm, DO_FARM_SCRIPT_ID),
		unknown_issue(layout.occupant_listener, OCCUPANT_LISTENER_SCRIPT_ID),
		unknown_issue(layout.ally_validator, ALLY_VALIDATOR_SCRIPT_ID),
		unknown_issue(layout.range_provider, RANGE_PROVIDER_SCRIPT_ID),
		unknown_issue(layout.tile_selector, TILE_SELECTOR_SCRIPT_ID),
		custom_buff_issue(layout.static_owner_buff, "$._buffs[0]", OWNER_BUFF_KEY),
		custom_buff_issue(layout.occupant_listener, "$._buffs[0]", OCCUPANT_BUFF_KEY),
		custom_buff_issue(
			layout.occupant_listener,
			"$._buffsWhenCasted[0]",
			OCCUPANT_BUFF_KEY,
		),
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
		return Err(MechanicTemplateDecodeError::new(
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
	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
		PotentialRank,
	};

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab, BattlePrefabGraph, BattlePrefabRootSection,
		},
		combat::ReplayRepeat,
		mechanic_semantics::{
			SemanticAction, SemanticCellSet, SemanticEvent, SemanticValueExpression,
		},
	};

	use super::{
		contains_sowed_field_identity, first_value_mismatch, try_decode_sowed_field_template,
		HEAL_BLACKBOARD_KEY, OCCUPANT_BUFF_KEY, SHELTER_BLACKBOARD_KEY,
	};

	#[test]
	fn exact_value_comparison_reports_the_first_drifted_field() {
		let expected = serde_json::json!({"a": {"b": 1}, "c": []});
		let actual = serde_json::json!({"a": {"b": 2}, "c": []});
		assert_eq!(
			first_value_mismatch(&expected, &actual, "$"),
			Some("$.a.b expected 1, found 2".to_owned())
		);
	}

	#[test]
	fn empty_graph_has_no_sowed_field_identity() {
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
		assert!(!contains_sowed_field_identity(&graph));
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_sowed_field_decodes_when_snapshots_are_present() {
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
			OperatorIdentity::Id("char_2025_shu".to_owned()),
			2,
			90,
			200,
			1,
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
			.find(|candidate| candidate.prefab_key() == Some("1"))
			.expect("active sowed-field talent");
		let graph: BattlePrefabGraph = interpret_battle_prefab(
			&prefabs,
			resolved.operator_id(),
			BattlePrefabRootSection::Talents,
			"1",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed sowed-field graph");
		let decoded = try_decode_sowed_field_template(
			&graph,
			candidate,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect("reviewed template must not drift")
		.expect("reviewed template identity must match");
		assert_eq!(decoded.consumed_issues.len(), 10);
		assert_eq!(decoded.consumed_semantic_fact_sources.len(), 1);
		assert_eq!(decoded.program.rules.len(), 1);
		let rule = &decoded.program.rules[0];
		assert_eq!(rule.trigger.event, SemanticEvent::Heal);
		assert_eq!(rule.trigger.repeat, ReplayRepeat::Unlimited);
		let SemanticAction::AddTileField(field) = &rule.action else {
			panic!("sowed-field template must add a tile field");
		};
		assert!(matches!(
			&field.cells,
			SemanticCellSet::TriggerActorRelative(offsets) if offsets.len() == 5
		));
		assert_eq!(field.resident_buffs[0].key_suffix, OCCUPANT_BUFF_KEY);
		let periodic = field.periodic.as_ref().expect("periodic healing");
		let crate::mechanics::mechanic_semantics::SemanticEffect::Healing { amount } =
			&periodic.effects_per_layer[0]
		else {
			panic!("expected periodic healing");
		};
		assert_eq!(
			amount,
			&SemanticValueExpression::Blackboard(HEAL_BLACKBOARD_KEY.to_owned())
		);
		let shelter = &field.resident_buffs[0].modifiers_per_stack[0];
		assert!(format!("{shelter:?}").contains(SHELTER_BLACKBOARD_KEY));
	}
}
