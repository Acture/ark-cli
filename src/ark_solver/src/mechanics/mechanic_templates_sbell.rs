//! Fail-closed decoder for the reusable layered-snowfield prefab template.
//!
//! The decoder keys on serialized script schemas, graph edges, and custom-buff identities. It does
//! not inspect operator identity. The typed result lowers into generic periodic layered-tile
//! primitives; no operator-specific runtime branch is involved.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{ArkDataSnapshot, OperatorEffectCandidate};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_handlers::{
		COMPOSITE_ABILITY_OLD_TYPE_HASH, COMPOSITE_ABILITY_SCRIPT_ID,
		STATIC_BUFF_ABILITY_OLD_TYPE_HASH, STATIC_BUFF_ABILITY_SCRIPT_ID,
		TILE_LISTENER_OLD_TYPE_HASH, TILE_LISTENER_SCRIPT_ID,
	},
	battle_prefab_interpreter::{
		BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemantic, BattlePrefabSemanticIssue,
	},
	combat::{DamageDelivery, DamageType, MechanicBehavior, MechanicId, ReplayRepeat, StatKind},
	mechanic_semantics::{
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticActorFilter,
		SemanticBuffModifier, SemanticCellSet, SemanticEffect, SemanticEvent, SemanticLifetime,
		SemanticProvenance, SemanticResidentBuff, SemanticSourceLocation, SemanticTileField,
		SemanticTrigger, SemanticTriggerFilter, SemanticValueExpression,
	},
	normal_attack::NormalAttackTargetFilter,
};

const BRIDGE_SCRIPT_ID: [u8; 16] = [
	0x7f, 0xfd, 0xc8, 0x07, 0x91, 0x6c, 0xa1, 0xea, 0x67, 0x41, 0xf4, 0x11, 0x75, 0xed, 0xd3, 0x5f,
];
const BRIDGE_OLD_TYPE_HASH: [u8; 16] = [
	0x59, 0xd9, 0x43, 0x18, 0x68, 0x25, 0x4c, 0x1f, 0xea, 0x2a, 0x27, 0x5c, 0x39, 0x37, 0x95, 0x98,
];
const SNOW_ACCUMULATION_SCRIPT_ID: [u8; 16] = [
	0x19, 0xc9, 0xfd, 0x6d, 0x29, 0x4b, 0x86, 0xf0, 0x74, 0x90, 0xa3, 0x2c, 0x18, 0xb2, 0xad, 0x44,
];
const SNOW_ACCUMULATION_OLD_TYPE_HASH: [u8; 16] = [
	0x26, 0xb5, 0xde, 0xd0, 0x16, 0x3f, 0x4b, 0xcc, 0x35, 0x20, 0x36, 0x8c, 0x13, 0x9e, 0x63, 0x6f,
];
const TARGET_VALIDATOR_SCRIPT_ID: [u8; 16] = [
	0xe2, 0xdb, 0xc5, 0xc0, 0x77, 0x9c, 0xd9, 0x19, 0x18, 0xdc, 0xd7, 0x47, 0x6e, 0xda, 0xfc, 0xb4,
];
const TARGET_VALIDATOR_OLD_TYPE_HASH: [u8; 16] = [
	0x5a, 0x12, 0x7d, 0x18, 0x03, 0xc6, 0x98, 0x22, 0x55, 0x4e, 0x5c, 0x11, 0xd1, 0x35, 0x25, 0xaa,
];
const CHARACTER_LISTENER_SCRIPT_ID: [u8; 16] = [
	0x36, 0xfd, 0xc2, 0x1c, 0xe7, 0x18, 0x1c, 0x05, 0x06, 0x9f, 0xd7, 0x2a, 0x90, 0x90, 0x59, 0x0b,
];
const CHARACTER_LISTENER_OLD_TYPE_HASH: [u8; 16] = [
	0xc3, 0x8c, 0x89, 0x68, 0xd2, 0xeb, 0x7f, 0x1c, 0xb5, 0xde, 0x47, 0x57, 0x7d, 0x80, 0xb5, 0xfe,
];

const ENTER_BUFF_KEY: &str = "sbell2_t1[enemy_enter]";
const LEAVE_BUFF_KEY: &str = "sbell2_t1[enemy_leave]";
const CHARACTER_BUFF_KEY: &str = "sbell2_t1[enemy_character]";
const SNOW_TRIGGER_BUFF_KEY: &str = "sbell2_t1[snow_trigger]";
const DEATH_CLEANUP_BUFF_KEY: &str = "sbell2_t1[dead_clean_buff]";

const INTERVAL_KEY: &str = "interval";
const MAX_LAYERS_KEY: &str = "max_cast_cnt";
const MOVEMENT_SPEED_KEY: &str = "move_speed";
const ATTACK_SCALE_KEY: &str = "talent_magic_scale";
const INITIAL_LAYERS_KEY: &str = "first_snow";

#[must_use]
pub(crate) fn snowfield_mechanic_id(logical_path: &str) -> MechanicId {
	MechanicId::new(format!(
		"{logical_path}/template/{SNOW_TRIGGER_BUFF_KEY}/layered-snowfield"
	))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnowfieldCellSelection {
	SourceCurrentAttackRangeGroundCells,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnowfieldRemovalRule {
	FirstGroundEnemyLeavesCell,
}

/// Exact, operator-independent meaning recovered from the reviewed snowfield graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnowfieldTemplateSemantics {
	pub provenance: SemanticProvenance,
	pub generation_interval_seconds: SemanticValueExpression,
	pub maximum_layers_per_cell: SemanticValueExpression,
	pub initial_layers_per_cell: SemanticValueExpression,
	pub cells: SnowfieldCellSelection,
	pub entry_damage_source_stat: StatKind,
	pub entry_damage_scale: SemanticValueExpression,
	pub entry_damage_type: DamageType,
	pub entry_damage_delivery: DamageDelivery,
	pub movement_speed_reduction_per_layer: SemanticValueExpression,
	pub removal: SnowfieldRemovalRule,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedSnowfieldTemplate {
	pub mechanic_id: MechanicId,
	pub semantics: SnowfieldTemplateSemantics,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_semantic_fact_sources: Vec<BattlePrefabObjectSource>,
}

impl DecodedSnowfieldTemplate {
	/// Lowers the verified graph into operator-independent periodic layered-tile rules.
	#[must_use]
	pub fn lower_program(&self) -> MechanicSemanticProgram {
		let tile_field: SemanticTileField = SemanticTileField {
			key: self.mechanic_id.clone(),
			cells: SemanticCellSet::SourceCurrentAttackRangeGroundCells,
			actor_filter: SemanticActorFilter::EnemyOfOwner,
			target_filter: NormalAttackTargetFilter::GroundOnly,
			max_layers_per_cell: self.semantics.maximum_layers_per_cell.clone(),
			lifetime: SemanticLifetime::OwnerDeployment,
			on_enter_once: vec![SemanticEffect::SourceStatScaledDamage {
				stat: self.semantics.entry_damage_source_stat,
				multiplier: self.semantics.entry_damage_scale.clone(),
				damage_type: self.semantics.entry_damage_type,
				delivery: self.semantics.entry_damage_delivery,
			}],
			resident_buffs: vec![SemanticResidentBuff {
				key_suffix: "movement-speed-reduction".to_owned(),
				max_stacks: self.semantics.maximum_layers_per_cell.clone(),
				modifiers_per_stack: vec![SemanticBuffModifier::MovementSpeedReduction {
					reduction: self.semantics.movement_speed_reduction_per_layer.clone(),
				}],
			}],
			layer_threshold_reactions: Vec::new(),
			periodic: None,
			remove_on_first_matching_leave: true,
		};
		let initial_rule_id: MechanicId =
			MechanicId::new(format!("{}/initial", self.mechanic_id.as_str()));
		let generation_rule_id: MechanicId =
			MechanicId::new(format!("{}/generation", self.mechanic_id.as_str()));
		MechanicSemanticProgram {
			rules: vec![
				MechanicSemanticRule {
					mechanic: initial_rule_id,
					provenance: self.semantics.provenance.clone(),
					trigger: SemanticTrigger {
						event: SemanticEvent::Deploy,
						filter: SemanticTriggerFilter::AffectedOwner,
						repeat: ReplayRepeat::Once,
					},
					lifetime: SemanticLifetime::OwnerDeployment,
					action: SemanticAction::Repeat {
						count: self.semantics.initial_layers_per_cell.clone(),
						action: Box::new(SemanticAction::AddTileField(tile_field.clone())),
					},
					behavior: MechanicBehavior::LayeredTileDamageAndSlow,
				},
				MechanicSemanticRule {
					mechanic: generation_rule_id,
					provenance: self.semantics.provenance.clone(),
					trigger: SemanticTrigger {
						event: SemanticEvent::Periodic {
							initial_delay_seconds: self
								.semantics
								.generation_interval_seconds
								.clone(),
							interval_seconds: self.semantics.generation_interval_seconds.clone(),
						},
						filter: SemanticTriggerFilter::AffectedOwner,
						repeat: ReplayRepeat::Unlimited,
					},
					lifetime: SemanticLifetime::OwnerDeployment,
					action: SemanticAction::AddTileField(tile_field),
					behavior: MechanicBehavior::LayeredTileDamageAndSlow,
				},
			],
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnowfieldTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl SnowfieldTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for SnowfieldTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"layered-snowfield template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for SnowfieldTemplateDecodeError {}

/// Recognizes and decodes the reviewed layered-snowfield talent without operator-ID branching.
pub fn try_decode_snowfield_template(
	graph: &BattlePrefabGraph,
	candidate: &OperatorEffectCandidate,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedSnowfieldTemplate>, SnowfieldTemplateDecodeError> {
	if !contains_snowfield_identity(graph) {
		return Ok(None);
	}
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(SnowfieldTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.section != BattlePrefabRootSection::Talents {
		return Err(SnowfieldTemplateDecodeError::new(
			"root section",
			format!("expected Talents, found {:?}", graph.section),
		));
	}
	validate_candidate(candidate, graph)?;
	let layout: SnowfieldLayout<'_> = validate_graph_layout(graph)?;
	validate_bridge(layout.bridge)?;
	validate_composite(layout.composite)?;
	validate_snow_accumulation(layout.accumulation)?;
	validate_tile_listener(layout.tile_listener)?;
	validate_target_validator(layout.tile_validator, 3, "tile listener validator")?;
	validate_character_listener(layout.character_listener)?;
	validate_target_validator(layout.character_validator, 1, "ground-character validator")?;
	validate_snow_trigger(layout.snow_trigger)?;

	validate_composite_edges(&layout)?;
	validate_reference_edge(
		layout.tile_listener,
		"$._targetValidator",
		layout.tile_validator,
		"tile listener validator edge",
	)?;
	validate_reference_edge(
		layout.character_listener,
		"$._targetValidator",
		layout.character_validator,
		"ground-character validator edge",
	)?;

	let consumed_issues: Vec<BattlePrefabSemanticIssue> =
		validate_and_collect_issues(graph, &layout)?;
	let provenance: SemanticProvenance = SemanticProvenance::BattlePrefab {
		mechanics_revision: graph.mechanics_revision.clone(),
		gamedata_revision: gamedata_revision.to_owned(),
		script_id: SNOW_ACCUMULATION_SCRIPT_ID,
		old_type_hash: SNOW_ACCUMULATION_OLD_TYPE_HASH,
		source: SemanticSourceLocation {
			logical_path: layout.accumulation.source.logical_path.clone(),
			cab_name: layout.accumulation.source.cab_name.clone(),
			path_id: layout.accumulation.source.path_id,
			field_paths: vec![
				"$._tileOptions".to_owned(),
				qualified_field(layout.composite, "$._abilities"),
				qualified_field(layout.tile_listener, "$._buffsWhenEnemyEnter[0]"),
				qualified_field(layout.tile_listener, "$._buffsWhenEnemyLeave[0]"),
				qualified_field(layout.character_listener, "$._buffs[0]"),
				qualified_field(layout.snow_trigger, "$._buffs"),
			],
		},
	};
	let semantics: SnowfieldTemplateSemantics = SnowfieldTemplateSemantics {
		provenance,
		generation_interval_seconds: SemanticValueExpression::Blackboard(INTERVAL_KEY.to_owned()),
		maximum_layers_per_cell: SemanticValueExpression::Blackboard(MAX_LAYERS_KEY.to_owned()),
		initial_layers_per_cell: SemanticValueExpression::Blackboard(INITIAL_LAYERS_KEY.to_owned()),
		cells: SnowfieldCellSelection::SourceCurrentAttackRangeGroundCells,
		entry_damage_source_stat: StatKind::Attack,
		entry_damage_scale: SemanticValueExpression::Blackboard(ATTACK_SCALE_KEY.to_owned()),
		entry_damage_type: DamageType::Arts,
		entry_damage_delivery: DamageDelivery::DirectHit {
			ignore_defense: false,
			ignore_shelter: false,
		},
		movement_speed_reduction_per_layer: SemanticValueExpression::NegatedBlackboard(
			MOVEMENT_SPEED_KEY.to_owned(),
		),
		removal: SnowfieldRemovalRule::FirstGroundEnemyLeavesCell,
	};
	Ok(Some(DecodedSnowfieldTemplate {
		mechanic_id: snowfield_mechanic_id(&graph.logical_path),
		semantics,
		consumed_issues,
		consumed_semantic_fact_sources: vec![
			layout.composite.source.clone(),
			layout.tile_listener.source.clone(),
			layout.snow_trigger.source.clone(),
		],
	}))
}

#[derive(Clone, Copy)]
struct SnowfieldLayout<'a> {
	bridge: &'a BattlePrefabMonoBehaviour,
	composite: &'a BattlePrefabMonoBehaviour,
	accumulation: &'a BattlePrefabMonoBehaviour,
	tile_listener: &'a BattlePrefabMonoBehaviour,
	tile_validator: &'a BattlePrefabMonoBehaviour,
	character_listener: &'a BattlePrefabMonoBehaviour,
	character_validator: &'a BattlePrefabMonoBehaviour,
	snow_trigger: &'a BattlePrefabMonoBehaviour,
}

fn contains_snowfield_identity(graph: &BattlePrefabGraph) -> bool {
	graph.mono_behaviours.iter().any(|behavior| {
		behavior.buffs.iter().any(|buff| {
			[buff.buff_key.as_deref(), buff.template_key.as_deref()]
				.into_iter()
				.flatten()
				.any(|key| {
					matches!(
						key,
						ENTER_BUFF_KEY
							| LEAVE_BUFF_KEY | CHARACTER_BUFF_KEY
							| SNOW_TRIGGER_BUFF_KEY
							| DEATH_CLEANUP_BUFF_KEY
					)
				})
		})
	})
}

fn validate_candidate(
	candidate: &OperatorEffectCandidate,
	graph: &BattlePrefabGraph,
) -> Result<(), SnowfieldTemplateDecodeError> {
	if candidate.prefab_key() != Some(graph.prefab_name.as_str()) {
		return Err(SnowfieldTemplateDecodeError::new(
			"candidate",
			format!(
				"prefab key {:?} does not select graph root {:?}",
				candidate.prefab_key(),
				graph.prefab_name
			),
		));
	}
	if candidate.range_id().is_some() || candidate.token_key().is_some() || candidate.hidden() {
		return Err(SnowfieldTemplateDecodeError::new(
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
			return Err(SnowfieldTemplateDecodeError::new(
				"candidate blackboard",
				format!("entry {:?} unexpectedly has a string value", entry.key()),
			));
		}
		if values.insert(entry.key(), entry.value()).is_some() {
			return Err(SnowfieldTemplateDecodeError::new(
				"candidate blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: Vec<&str> = vec![
		INITIAL_LAYERS_KEY,
		INTERVAL_KEY,
		MAX_LAYERS_KEY,
		MOVEMENT_SPEED_KEY,
		ATTACK_SCALE_KEY,
	];
	if values.keys().copied().collect::<Vec<_>>() != expected_keys {
		return Err(SnowfieldTemplateDecodeError::new(
			"candidate blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				values.keys().collect::<Vec<_>>()
			),
		));
	}
	let interval: f64 = values[INTERVAL_KEY];
	let max_layers: f64 = values[MAX_LAYERS_KEY];
	let movement_speed: f64 = values[MOVEMENT_SPEED_KEY];
	let attack_scale: f64 = values[ATTACK_SCALE_KEY];
	let initial_layers: f64 = values[INITIAL_LAYERS_KEY];
	if !interval.is_finite() || interval <= 0.0 {
		return Err(invalid_blackboard(
			INTERVAL_KEY,
			interval,
			"finite and positive",
		));
	}
	if !max_layers.is_finite()
		|| max_layers.fract() != 0.0
		|| !(1.0..=f64::from(u16::MAX)).contains(&max_layers)
	{
		return Err(invalid_blackboard(
			MAX_LAYERS_KEY,
			max_layers,
			"a positive u16 integer",
		));
	}
	if !movement_speed.is_finite() || !(-1.0..=0.0).contains(&movement_speed) {
		return Err(invalid_blackboard(
			MOVEMENT_SPEED_KEY,
			movement_speed,
			"in [-1, 0]",
		));
	}
	if !attack_scale.is_finite() || attack_scale < 0.0 {
		return Err(invalid_blackboard(
			ATTACK_SCALE_KEY,
			attack_scale,
			"finite and nonnegative",
		));
	}
	if !initial_layers.is_finite()
		|| initial_layers.fract() != 0.0
		|| !(0.0..=max_layers).contains(&initial_layers)
	{
		return Err(invalid_blackboard(
			INITIAL_LAYERS_KEY,
			initial_layers,
			"an integer between zero and max_cast_cnt",
		));
	}
	Ok(())
}

fn invalid_blackboard(key: &str, value: f64, expected: &str) -> SnowfieldTemplateDecodeError {
	SnowfieldTemplateDecodeError::new(
		"candidate blackboard",
		format!("{key:?} must be {expected}, found {value}"),
	)
}

fn validate_graph_layout(
	graph: &BattlePrefabGraph,
) -> Result<SnowfieldLayout<'_>, SnowfieldTemplateDecodeError> {
	if graph.root_game_object_indices.len() != 1 {
		return Err(SnowfieldTemplateDecodeError::new(
			"graph roots",
			format!(
				"expected one talent root, found {}",
				graph.root_game_object_indices.len()
			),
		));
	}
	let root_index: usize = graph.root_game_object_indices[0];
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		SnowfieldTemplateDecodeError::new(
			"graph roots",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	if root.name != graph.prefab_name || root.child_indices.len() != 2 {
		return Err(SnowfieldTemplateDecodeError::new(
			"graph hierarchy",
			format!(
				"root must be named {:?} with two children, found {:?} with {}",
				graph.prefab_name,
				root.name,
				root.child_indices.len()
			),
		));
	}
	let accumulation_index: usize = exact_child(graph, root, "SnowAccumulation")?;
	let trigger_index: usize = exact_child(graph, root, "SnowTrigger")?;
	let accumulation_object = &graph.game_objects[accumulation_index];
	let trigger_object = &graph.game_objects[trigger_index];
	if !accumulation_object.child_indices.is_empty() || !trigger_object.child_indices.is_empty() {
		return Err(SnowfieldTemplateDecodeError::new(
			"graph hierarchy",
			"SnowAccumulation and SnowTrigger must both be leaf GameObjects",
		));
	}

	let bridge = exact_owned_behavior(
		graph,
		root_index,
		BRIDGE_SCRIPT_ID,
		BRIDGE_OLD_TYPE_HASH,
		"talent bridge",
	)?;
	let composite = exact_owned_behavior(
		graph,
		root_index,
		COMPOSITE_ABILITY_SCRIPT_ID,
		COMPOSITE_ABILITY_OLD_TYPE_HASH,
		"composite",
	)?;
	let accumulation = exact_owned_behavior(
		graph,
		accumulation_index,
		SNOW_ACCUMULATION_SCRIPT_ID,
		SNOW_ACCUMULATION_OLD_TYPE_HASH,
		"snow accumulation",
	)?;
	let tile_listener = exact_owned_behavior(
		graph,
		accumulation_index,
		TILE_LISTENER_SCRIPT_ID,
		TILE_LISTENER_OLD_TYPE_HASH,
		"tile listener",
	)?;
	let character_listener = exact_owned_behavior(
		graph,
		accumulation_index,
		CHARACTER_LISTENER_SCRIPT_ID,
		CHARACTER_LISTENER_OLD_TYPE_HASH,
		"ground-character listener",
	)?;
	let validators: Vec<&BattlePrefabMonoBehaviour> = accumulation_object
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index| graph.mono_behaviours.get(index))
		.filter(|behavior| {
			behavior.script_id == Some(TARGET_VALIDATOR_SCRIPT_ID)
				&& behavior.old_type_hash == Some(TARGET_VALIDATOR_OLD_TYPE_HASH)
		})
		.collect();
	if validators.len() != 2 {
		return Err(SnowfieldTemplateDecodeError::new(
			"target validators",
			format!(
				"expected two validator instances, found {}",
				validators.len()
			),
		));
	}
	let tile_validator: &BattlePrefabMonoBehaviour = reference_target_behavior(
		graph,
		tile_listener,
		"$._targetValidator",
		"tile listener validator",
	)?;
	let character_validator: &BattlePrefabMonoBehaviour = reference_target_behavior(
		graph,
		character_listener,
		"$._targetValidator",
		"ground-character validator",
	)?;
	if tile_validator.source == character_validator.source
		|| !validators.contains(&tile_validator)
		|| !validators.contains(&character_validator)
	{
		return Err(SnowfieldTemplateDecodeError::new(
			"target validators",
			"listener edges must address the two distinct owned validator instances",
		));
	}
	let snow_trigger = exact_owned_behavior(
		graph,
		trigger_index,
		STATIC_BUFF_ABILITY_SCRIPT_ID,
		STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
		"snow trigger",
	)?;

	if graph.game_objects.len() != 3
		|| graph.mono_behaviours.len() != 8
		|| root.mono_behaviour_indices.len() != 2
		|| accumulation_object.mono_behaviour_indices.len() != 5
		|| trigger_object.mono_behaviour_indices.len() != 1
	{
		return Err(SnowfieldTemplateDecodeError::new(
			"graph shape",
			format!(
				"expected GameObjects/behaviors/root/accumulation/trigger counts 3/8/2/5/1, found {}/{}/{}/{}/{}",
				graph.game_objects.len(),
				graph.mono_behaviours.len(),
				root.mono_behaviour_indices.len(),
				accumulation_object.mono_behaviour_indices.len(),
				trigger_object.mono_behaviour_indices.len()
			),
		));
	}
	Ok(SnowfieldLayout {
		bridge,
		composite,
		accumulation,
		tile_listener,
		tile_validator,
		character_listener,
		character_validator,
		snow_trigger,
	})
}

fn exact_child(
	graph: &BattlePrefabGraph,
	parent: &crate::mechanics::battle_prefab_interpreter::BattlePrefabGameObject,
	name: &'static str,
) -> Result<usize, SnowfieldTemplateDecodeError> {
	let matches: Vec<usize> = parent
		.child_indices
		.iter()
		.copied()
		.filter(|&index| {
			graph
				.game_objects
				.get(index)
				.is_some_and(|child| child.name == name)
		})
		.collect();
	if matches.len() != 1 {
		return Err(SnowfieldTemplateDecodeError::new(
			"graph hierarchy",
			format!("expected one child named {name:?}, found {}", matches.len()),
		));
	}
	Ok(matches[0])
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_id: [u8; 16],
	old_type_hash: [u8; 16],
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, SnowfieldTemplateDecodeError> {
	let owner = graph.game_objects.get(owner_index).ok_or_else(|| {
		SnowfieldTemplateDecodeError::new(component, "owner GameObject index is invalid")
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
		return Err(SnowfieldTemplateDecodeError::new(
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
		return Err(SnowfieldTemplateDecodeError::new(
			component,
			format!(
				"behavior owner is {:?}, expected {owner_index}",
				behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

fn reference_target_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	source: &BattlePrefabMonoBehaviour,
	field_path: &str,
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, SnowfieldTemplateDecodeError> {
	let matches: Vec<usize> = source
		.references
		.iter()
		.filter(|reference| reference.field_path == field_path)
		.filter_map(|reference| {
			graph
				.mono_behaviours
				.iter()
				.position(|behavior| behavior.source == reference.target)
		})
		.collect();
	if matches.len() != 1 {
		return Err(SnowfieldTemplateDecodeError::new(
			component,
			format!(
				"expected one {field_path} behavior edge, found {}",
				matches.len()
			),
		));
	}
	Ok(&graph.mono_behaviours[matches[0]])
}

fn validate_bridge(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), SnowfieldTemplateDecodeError> {
	validate_configuration(
		behavior,
		"talent bridge",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_affectWhenNotRootTalent": 0,
			"_applyBlackboardBySkill": 1,
			"_applyStrBlackboardBySkill": 1,
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

fn validate_composite(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), SnowfieldTemplateDecodeError> {
	validate_configuration(
		behavior,
		"composite",
		&["_abilities"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_alsoCheckChildrenAffecting": 0,
			"_alsoInterruptSubAbilities": 0,
			"_attachPassiveBuffsOnDummy": 1,
			"_finishAllAbilitiesIfOneFinish": 0,
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 1,
			"_mainAbility": {"m_FileID": 0, "m_PathID": 0},
			"_metadata": {"blackboardPrefix": "", "namedAsAlias": ""},
			"_selector": {"m_FileID": 0, "m_PathID": 0},
			"_timeMode": 1,
			"_updateCooldownAfterSubAbilitiesAttached": 0,
			"_useEscaptime": 0
		}),
	)?;
	validate_single_fact(behavior, "composite", |semantic| {
		matches!(semantic, BattlePrefabSemantic::CompositeAbility(_))
	})
}

fn validate_snow_accumulation(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), SnowfieldTemplateDecodeError> {
	let effect_settings: Value = json!([
		{"audioSignal":"", "castedTimeThreshold":1, "effectKey":"sbell2_ground_01", "startEffectKey":""},
		{"audioSignal":"", "castedTimeThreshold":2, "effectKey":"sbell2_ground_02", "startEffectKey":""},
		{"audioSignal":"", "castedTimeThreshold":3, "effectKey":"sbell2_ground_02", "startEffectKey":""},
		{"audioSignal":"", "castedTimeThreshold":4, "effectKey":"sbell2_ground_02", "startEffectKey":""},
		{"audioSignal":"", "castedTimeThreshold":5, "effectKey":"sbell2_ground_03", "startEffectKey":""}
	]);
	let tile_options: Value = json!({
		"advancedBuildableMask": 1,
		"advancedBuildableMaskExcept": 0,
		"allowAllAdvancedBuildableMask": 1,
		"allowNoneBuildableType": 1,
		"allowNonePassableMask": 0,
		"allowedTileBlackboardKey": "",
		"buildableType": 0,
		"checkBuildableOrPassable": 0,
		"checkExtraBuildableCheckers": 0,
		"checkHeightType": 1,
		"checkTileHidden": 0,
		"checkTileMode": 0,
		"checkTileMoveCost": 0,
		"checkTileTypes": 1,
		"exceptTileTypes": 1,
		"excludeTileBlackboardKey": "",
		"heightType": 0,
		"modeIndex": -1,
		"moveCostCompareType": 0,
		"moveCostThreshold": 0,
		"passableMask": 1,
		"tileTypesMask": 2
	});
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
			"_audioSignal": "sbell2_snow_refresh_audio",
			"_checkFaceToFirstCastTargetDownAttack": 0,
			"_cooldown": 0.0,
			"_cooldownKey": "duration",
			"_dontCreateNewEffectWhenCastedTimesSame": 1,
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
			"_metadata": {"blackboardPrefix": "", "namedAsAlias": ""},
			"_onlyPlayEndAnimWhenIsLastAbility": 0,
			"_playAudioWhenEffectChanged": 1
		}),
		json!({
			"_preDelay": 0.0,
			"_resetCdStrategy": 1,
			"_selectTargetSource": 2,
			"_selectTargetTiming": 0,
			"_selector": {"m_FileID": 0, "m_PathID": 0},
			"_startEffect": "",
			"_tileEffect": "",
			"_tileHoldEffect": 0,
			"_tileOptions": tile_options,
			"_timeMode": 2,
			"_upAnimKey": "",
			"_upEndAnimKey": "",
			"_useDynamicAttackType": 0,
			"_useOwnerRootTileAsTargetSource": 0,
			"_waitForAttackEvent": 0
		}),
	]);
	validate_configuration(behavior, "snow accumulation", &[], expected)?;
	validate_no_facts(behavior, "snow accumulation")
}

fn validate_tile_listener(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), SnowfieldTemplateDecodeError> {
	let enter: Value = expected_custom_buff(
		ENTER_BUFF_KEY,
		vec![json!({"key":"interval", "value":0.5, "valueStr":""})],
		2,
		2,
		2,
	);
	let leave: Value = expected_custom_buff(
		LEAVE_BUFF_KEY,
		vec![json!({"key":"interval", "value":-1.0, "valueStr":""})],
		0,
		0,
		0,
	);
	validate_configuration(
		behavior,
		"tile listener",
		&["_targetValidator"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_buffsWhenCasted": [],
			"_buffsWhenEnemyEnter": [enter],
			"_buffsWhenEnemyLeave": [leave],
			"_clearBuffWhenAbilityDetached": 1,
			"_clearBuffWhenTileListenerRemoved": 1
		}),
	)?;
	validate_single_fact(behavior, "tile listener", |semantic| {
		matches!(semantic, BattlePrefabSemantic::TileListener(_))
	})
}

fn validate_target_validator(
	behavior: &BattlePrefabMonoBehaviour,
	target_motion: i64,
	component: &'static str,
) -> Result<(), SnowfieldTemplateDecodeError> {
	validate_configuration(
		behavior,
		component,
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
				"enableAdvancedOptions": 0,
				"excludeAbnormalFlag": 0,
				"excludeSomeAbnormalFlags": 0,
				"ignoreAllyTargetFree": 0,
				"ignoreHealFree": 0,
				"ignoreTargetFree": 0,
				"ignoreTargetSide": 0,
				"onlyIgnoreSomeOfTargetFreeCase": 0,
				"professionMask": 0,
				"purposeMask": 0,
				"targetCategory": 1,
				"targetMotion": target_motion,
				"targetSide": 2,
				"unitTypeMask": 0
			}
		}),
	)?;
	validate_no_facts(behavior, component)
}

fn validate_character_listener(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), SnowfieldTemplateDecodeError> {
	let character: Value = expected_custom_buff(
		CHARACTER_BUFF_KEY,
		vec![json!({"key":"interval", "value":0.5, "valueStr":""})],
		2,
		2,
		2,
	);
	validate_configuration(
		behavior,
		"ground-character listener",
		&["_targetValidator"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_buffs": [character],
			"_buffsWhenCasted": [],
			"_clearBuffWhenAbilityDetached": 1,
			"_clearBuffWhenTileListenerRemoved": 1,
			"_useBlackList": 1,
			"_whiteListBuffKeys": []
		}),
	)?;
	validate_no_facts(behavior, "ground-character listener")
}

fn validate_snow_trigger(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), SnowfieldTemplateDecodeError> {
	let snow_trigger: Value = expected_custom_buff(SNOW_TRIGGER_BUFF_KEY, Vec::new(), 2, 0, 2);
	let death_cleanup: Value = expected_custom_buff(DEATH_CLEANUP_BUFF_KEY, Vec::new(), 2, 0, 0);
	validate_configuration(
		behavior,
		"snow trigger",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [snow_trigger, death_cleanup],
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 1,
			"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
			"_selector": {"m_FileID":0, "m_PathID":0}
		}),
	)?;
	validate_single_fact(behavior, "snow trigger", |semantic| {
		matches!(semantic, BattlePrefabSemantic::StaticBuffAbility(_))
	})
}

fn expected_custom_buff(
	key: &str,
	blackboard: Vec<Value>,
	lifetime_type: i64,
	override_type: i64,
	trigger_life_type: i64,
) -> Value {
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
		"blackboard": blackboard,
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
		"lifeTimeType": lifetime_type
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
			"overrideType": override_type,
			"priority": -1,
			"priorityBBKeys": [],
			"refreshRemainingTimeWhenStackMax": 0,
			"statusResistable": 2,
			"stripBlackboardParamsWithBuffKey": 0,
			"takeSnapshotWhenExtend": 0,
			"templateKey": key,
			"triggerCnt": 0,
			"triggerInterval": -1.0,
			"triggerLifeType": trigger_life_type,
			"waitFirstTriggerInterval": 1
		}),
	])
}

fn validate_composite_edges(
	layout: &SnowfieldLayout<'_>,
) -> Result<(), SnowfieldTemplateDecodeError> {
	let expected: Vec<(&str, &BattlePrefabObjectSource)> = vec![
		("$._abilities[0]", &layout.accumulation.source),
		("$._abilities[1]", &layout.snow_trigger.source),
	];
	if layout.composite.references.len() != expected.len()
		|| expected.iter().any(|(field, target)| {
			!layout
				.composite
				.references
				.iter()
				.any(|reference| reference.field_path == *field && reference.target == **target)
		}) {
		return Err(SnowfieldTemplateDecodeError::new(
			"composite edges",
			format!(
				"expected _abilities[0/1] -> accumulation/trigger, found {:?}",
				layout.composite.references
			),
		));
	}
	Ok(())
}

fn validate_reference_edge(
	source: &BattlePrefabMonoBehaviour,
	field_path: &str,
	target: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), SnowfieldTemplateDecodeError> {
	if source.references.len() != 1
		|| source.references[0].field_path != field_path
		|| source.references[0].target != target.source
	{
		return Err(SnowfieldTemplateDecodeError::new(
			component,
			format!(
				"expected sole edge {field_path} -> {:?}, found {:?}",
				target.source, source.references
			),
		));
	}
	Ok(())
}

fn validate_single_fact(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	predicate: impl FnOnce(&BattlePrefabSemantic) -> bool,
) -> Result<(), SnowfieldTemplateDecodeError> {
	if behavior.semantic_facts.len() != 1 || !predicate(&behavior.semantic_facts[0].semantic) {
		return Err(SnowfieldTemplateDecodeError::new(
			component,
			format!(
				"expected one matching typed fact, found {:?}",
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

fn validate_no_facts(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), SnowfieldTemplateDecodeError> {
	if !behavior.semantic_facts.is_empty() {
		return Err(SnowfieldTemplateDecodeError::new(
			component,
			format!(
				"custom script unexpectedly emitted typed facts {:?}",
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

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	dynamic_reference_fields: &[&str],
	expected: Value,
) -> Result<(), SnowfieldTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		SnowfieldTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(SnowfieldTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	for &field in dynamic_reference_fields {
		if actual.remove(field).is_none() {
			return Err(SnowfieldTemplateDecodeError::new(
				component,
				format!("missing graph-reference field {field:?}"),
			));
		}
	}
	let actual: Value = Value::Object(actual);
	if let Some(detail) = first_value_mismatch(&expected, &actual, "$") {
		return Err(SnowfieldTemplateDecodeError::new(component, detail));
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
	layout: &SnowfieldLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, SnowfieldTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.bridge, BRIDGE_SCRIPT_ID),
		unknown_issue(layout.accumulation, SNOW_ACCUMULATION_SCRIPT_ID),
		unknown_issue(layout.tile_validator, TARGET_VALIDATOR_SCRIPT_ID),
		unknown_issue(layout.character_listener, CHARACTER_LISTENER_SCRIPT_ID),
		unknown_issue(layout.character_validator, TARGET_VALIDATOR_SCRIPT_ID),
		custom_buff_issue(
			layout.tile_listener,
			"$._buffsWhenEnemyEnter[0]",
			ENTER_BUFF_KEY,
		),
		custom_buff_issue(
			layout.tile_listener,
			"$._buffsWhenEnemyLeave[0]",
			LEAVE_BUFF_KEY,
		),
		custom_buff_issue(layout.character_listener, "$._buffs[0]", CHARACTER_BUFF_KEY),
		custom_buff_issue(layout.snow_trigger, "$._buffs[0]", SNOW_TRIGGER_BUFF_KEY),
		custom_buff_issue(layout.snow_trigger, "$._buffs[1]", DEATH_CLEANUP_BUFF_KEY),
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
		return Err(SnowfieldTemplateDecodeError::new(
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
		combat::{BuffModifier, ExpectedEffect, ReplayCellSet, ReplayTrigger, StatKind},
		mechanic_semantics::SemanticValueExpression,
		normal_attack::NormalAttackTargetFilter,
	};

	use super::{
		contains_snowfield_identity, first_value_mismatch, try_decode_snowfield_template,
		ATTACK_SCALE_KEY, INTERVAL_KEY,
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
		assert!(!contains_snowfield_identity(&graph));
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_snowfield_decodes_and_lowers_generic_runtime_rules() {
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
			.find(|candidate| candidate.prefab_key() == Some("1"))
			.expect("active snowfield talent");
		let graph: BattlePrefabGraph = interpret_battle_prefab(
			&prefabs,
			resolved.operator_id(),
			BattlePrefabRootSection::Talents,
			"1",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed snowfield graph");
		let decoded = try_decode_snowfield_template(
			&graph,
			candidate,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect("reviewed template must not drift")
		.expect("reviewed template identity must match");
		assert_eq!(decoded.consumed_issues.len(), 10);
		assert_eq!(decoded.consumed_semantic_fact_sources.len(), 3);
		assert!(decoded.mechanic_id.as_str().contains("layered-snowfield"));
		assert_eq!(
			decoded.semantics.generation_interval_seconds,
			SemanticValueExpression::Blackboard(INTERVAL_KEY.to_owned())
		);
		assert_eq!(
			decoded.semantics.entry_damage_scale,
			SemanticValueExpression::Blackboard(ATTACK_SCALE_KEY.to_owned())
		);
		let program = decoded.lower_program();
		assert_eq!(program.rules.len(), 2);
		let lowered = program
			.lower(
				candidate.blackboard(),
				TickRate::new(NonZeroU32::new(30).expect("nonzero tick rate")),
			)
			.expect("reviewed blackboard must lower");
		assert_eq!(lowered.on_deploy.len(), 2);
		let periodic = lowered
			.on_deploy
			.iter()
			.find_map(|effect| match &effect.effect {
				ExpectedEffect::RegisterListener(listener)
					if matches!(listener.trigger, ReplayTrigger::Periodic { .. }) =>
				{
					Some(listener)
				}
				_ => None,
			})
			.expect("snow generation must be periodic");
		assert!(matches!(
			periodic.trigger,
			ReplayTrigger::Periodic {
				initial_delay_ticks: 165,
				interval_ticks: 165,
			}
		));
		let ExpectedEffect::AddTileLayer(tile) = &periodic.effects[0].effect else {
			panic!("periodic rule must add one tile layer");
		};
		assert_eq!(
			tile.cells,
			ReplayCellSet::SourceCurrentAttackRangeGroundCells
		);
		assert_eq!(tile.target_filter, NormalAttackTargetFilter::GroundOnly);
		assert!(tile.remove_on_first_matching_leave);
		assert!(matches!(
			tile.on_enter_once.as_slice(),
			[crate::mechanics::combat::TaggedEffect {
				effect: ExpectedEffect::SourceStatScaledDamage {
					stat: StatKind::Attack,
					..
				},
				..
			}]
		));
		let ExpectedEffect::ApplyBuff(buff) = &tile.on_enter_per_layer[0].effect else {
			panic!("each layer must add a resident slow buff stack");
		};
		assert!(matches!(
			buff.modifiers_per_stack.as_slice(),
			[BuffModifier::MovementSpeedReduction { .. }]
		));
	}
}
