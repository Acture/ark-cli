//! Generic, fail-closed interpretation of verified Unity battle-prefab graphs.
//!
//! This module deliberately knows nothing about operator IDs or custom buff keys. It reconstructs
//! Unity's `GameObject`/`Transform` hierarchy, exposes raw `MonoBehaviour` fields, and dispatches
//! behavior interpretation only by the exact serialized script-ID/schema-hash pair.
//!
//! A `MonoBehaviour` field reference into a serialized file the snapshot does not pin is retained as
//! a semantic issue rather than followed; every other unresolved reference aborts traversal.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{
	BattlePrefabEntryKind, BattlePrefabFile, BattlePrefabObjectHandle, BattlePrefabScriptClass,
	BattlePrefabSnapshot,
};
use serde_json::{Map, Value};

/// Stable location of one object in a verified battle-prefab snapshot.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BattlePrefabObjectSource {
	pub logical_path: String,
	pub cab_name: String,
	pub path_id: i64,
}

/// One resolved PPtr found inside a MonoBehaviour's TypeTree fields.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BattlePrefabResolvedReference {
	pub field_path: String,
	pub target: BattlePrefabObjectSource,
}

/// One `MonoBehaviour` field PPtr into a serialized file the verified snapshot does not pin.
///
/// The reference is retained but never visited: nothing behind it is evidence.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BattlePrefabUnavailableReference {
	pub field_path: String,
	pub file_id: i32,
	pub path_id: i64,
	pub target_cab: String,
}

/// One AssetBundle container entry whose path matched exactly, with the bundle that declares it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BattlePrefabContainerMatch {
	pub logical_path: String,
	pub bundle_kind: BattlePrefabEntryKind,
	/// The container asset's own PPtr file ID; nonzero means its root lives in another CAB.
	pub container_file_id: i32,
	pub root: BattlePrefabObjectSource,
}

/// One buff-shaped object retained from raw TypeTree fields.
#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabBuffSpec {
	/// JSON-path-like location of the object containing `buffKey` or `templateKey`.
	pub field_path: String,
	pub buff_key: Option<String>,
	pub template_key: Option<String>,
	/// Unmodified `attributes.attributeModifiers` entries, in source order.
	pub attribute_modifiers: Vec<Value>,
}

/// One normalized Unity object reference retained by a typed semantic configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BattlePrefabObjectReference {
	pub file_id: i32,
	pub path_id: i64,
}

/// Exact source identity attached to every interpreted semantic fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BattlePrefabSemanticProvenance {
	pub source: BattlePrefabObjectSource,
	pub script_id: [u8; 16],
	pub old_type_hash: [u8; 16],
	pub mechanics_revision: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabCompositeAbility {
	pub target_selector: BattlePrefabObjectReference,
	pub child_abilities: Vec<BattlePrefabObjectReference>,
	pub main_ability: BattlePrefabObjectReference,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabAttackAbility {
	pub target_selector: BattlePrefabObjectReference,
	pub attack_scale: f64,
	pub attack_scale_key: String,
	pub attack_type: i64,
	pub damage_type: i64,
	pub element_damage_type: i64,
	pub extra_damage_type: i64,
	pub select_target_source: i64,
	pub select_target_timing: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabAbilityConfiguration {
	pub enabled: bool,
	pub metadata_named_as_alias: String,
	pub metadata_blackboard_prefix: String,
	pub interrupt_on_detach: bool,
	pub attach_passive_buffs_on_dummy: bool,
	pub ignore_if_owner_dead: bool,
	pub force_as_damage_or_heal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabAttributeModifier {
	pub attribute_type: i64,
	pub formula_item: i64,
	pub value: f64,
	pub load_from_blackboard: bool,
	pub fetch_base_value_from_source_entity: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BattlePrefabAttributeConfiguration {
	pub abnormal_antis: Vec<i64>,
	pub abnormal_combo_immunes: Vec<i64>,
	pub abnormal_combos: Vec<i64>,
	pub abnormal_flags: Vec<i64>,
	pub abnormal_immunes: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabBlackboardEntry {
	pub key: String,
	pub value: f64,
	pub value_string: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabBuffConfiguration {
	pub audio_signal: String,
	pub blackboard: Vec<BattlePrefabBlackboardEntry>,
	pub buff_key: String,
	pub clear_all_stack_count_when_time_up: bool,
	pub disable_override: bool,
	pub duration_key: String,
	pub enable_initial_direction_from_source: bool,
	pub independent_character_source: bool,
	pub is_damage_missable: bool,
	pub is_durable: bool,
	pub is_freezable: bool,
	pub is_levitatable: bool,
	pub is_silenceable: bool,
	pub is_stunnable: bool,
	pub load_from_database: bool,
	pub max_stack_count: i64,
	pub max_valid_stack_count: i64,
	pub on_event_priority: i64,
	pub override_effect_key: String,
	pub override_key: String,
	pub override_on_event_priority: i64,
	pub override_type: i64,
	pub priority: i64,
	pub priority_blackboard_keys: Vec<String>,
	pub refresh_remaining_time_when_stack_max: bool,
	pub status_resistable: i64,
	pub strip_blackboard_params_with_buff_key: bool,
	pub take_snapshot_when_extend: bool,
	pub template_key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabListenerTiming {
	pub life_time: f64,
	pub life_time_type: i64,
	pub first_trigger_interval: f64,
	pub trigger_interval: f64,
	pub trigger_count: i64,
	pub trigger_life_type: i64,
	pub wait_first_trigger_interval: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabBuffConfigurationSet {
	pub attribute_modifiers: Vec<BattlePrefabAttributeModifier>,
	pub attribute_configuration: BattlePrefabAttributeConfiguration,
	pub configuration: BattlePrefabBuffConfiguration,
	pub listener_timing: BattlePrefabListenerTiming,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabStaticBuffAbility {
	pub target_selector: BattlePrefabObjectReference,
	pub ability_configuration: BattlePrefabAbilityConfiguration,
	pub buffs: Vec<BattlePrefabBuffConfigurationSet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabTileListener {
	pub target_validator: BattlePrefabObjectReference,
	pub buffs_when_casted: Vec<BattlePrefabBuffConfigurationSet>,
	pub buffs_when_enemy_enter: Vec<BattlePrefabBuffConfigurationSet>,
	pub buffs_when_enemy_leave: Vec<BattlePrefabBuffConfigurationSet>,
	pub clear_when_ability_detached: bool,
	pub clear_when_listener_removed: bool,
}

/// Operator-agnostic semantics emitted from a verified script schema.
#[derive(Clone, Debug, PartialEq)]
pub enum BattlePrefabSemantic {
	CompositeAbility(BattlePrefabCompositeAbility),
	AttackAbility(BattlePrefabAttackAbility),
	StaticBuffAbility(BattlePrefabStaticBuffAbility),
	TileListener(BattlePrefabTileListener),
}

impl BattlePrefabSemantic {
	pub const fn stable_kind(&self) -> &'static str {
		match self {
			Self::CompositeAbility(_) => "composite_ability",
			Self::AttackAbility(_) => "attack_ability",
			Self::StaticBuffAbility(_) => "static_buff_ability",
			Self::TileListener(_) => "tile_listener",
		}
	}
}

/// A handler's typed result plus the exact snapshot schema that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabSemanticFact {
	pub provenance: BattlePrefabSemanticProvenance,
	pub semantic: BattlePrefabSemantic,
}

impl BattlePrefabSemanticFact {
	pub const fn kind(&self) -> &'static str {
		self.semantic.stable_kind()
	}
}

/// One MonoBehaviour found while walking a prefab hierarchy or its raw PPtr references.
#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabMonoBehaviour {
	/// Owning GameObject when the behaviour is an in-hierarchy component.
	///
	/// Referenced behaviours outside the traversed hierarchy deliberately retain an unknown owner.
	pub game_object_index: Option<usize>,
	pub source: BattlePrefabObjectSource,
	pub script_id: Option<[u8; 16]>,
	pub old_type_hash: Option<[u8; 16]>,
	pub script_class: Option<BattlePrefabScriptClass>,
	pub raw_fields: Value,
	pub buffs: Vec<BattlePrefabBuffSpec>,
	pub semantic_facts: Vec<BattlePrefabSemanticFact>,
	/// Every direct, non-null PPtr in `raw_fields`, excluding Unity ownership/script metadata.
	pub references: Vec<BattlePrefabResolvedReference>,
	/// Direct PPtrs in `raw_fields` whose external CAB is absent from the snapshot; never visited.
	pub unavailable_references: Vec<BattlePrefabUnavailableReference>,
	/// Unique graph indices for the MonoBehaviour targets in `references`.
	pub referenced_mono_behaviour_indices: Vec<usize>,
}

/// One GameObject in preorder. Child and behavior indices address vectors on the owning graph.
#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabGameObject {
	pub source: BattlePrefabObjectSource,
	pub name: String,
	pub hierarchy_path: String,
	pub component_sources: Vec<BattlePrefabObjectSource>,
	pub mono_behaviour_indices: Vec<usize>,
	pub child_indices: Vec<usize>,
}

/// Fully traversed prefab plus every fail-closed semantic issue found during interpretation.
#[derive(Clone, Debug, PartialEq)]
pub struct BattlePrefabGraph {
	pub mechanics_revision: String,
	pub logical_path: String,
	pub asset_path: String,
	pub section: BattlePrefabRootSection,
	pub prefab_name: String,
	/// Every GameObject contributing to one logical prefab key. Unity charpacks legitimately use
	/// repeated sibling names for parallel pieces of the same talent (for example Shu's three `2`
	/// objects), so this is a forest rather than an arbitrarily selected root.
	pub root_game_object_indices: Vec<usize>,
	pub game_objects: Vec<BattlePrefabGameObject>,
	pub mono_behaviours: Vec<BattlePrefabMonoBehaviour>,
	pub issues: Vec<BattlePrefabSemanticIssue>,
}

/// Exact hierarchy section beneath the charpack's AssetBundle-container main asset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BattlePrefabRootSection {
	MainAsset,
	Talents,
	Modes,
}

impl BattlePrefabRootSection {
	/// Every hierarchy path beneath the main asset that may hold the section's container. Later
	/// charpacks keep `Talents` under `Others/Direction`, so talents turn with the operator's
	/// facing; earlier ones keep it directly under the root. A prefab holding more than one of
	/// these containers is ambiguous rather than resolved by preference.
	fn container_layouts(self) -> &'static [&'static [&'static str]] {
		match self {
			Self::MainAsset => &[&[]],
			Self::Talents => &[&["Others", "Direction", "Talents"], &["Talents"]],
			Self::Modes => &[&["Modes"]],
		}
	}
}

impl BattlePrefabGraph {
	pub fn is_semantically_complete(&self) -> bool {
		self.issues.is_empty()
	}
}

/// Semantic omissions are data, not traversal errors: callers can report all of them together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BattlePrefabSemanticIssue {
	MissingScriptId {
		source: BattlePrefabObjectSource,
	},
	UnknownScriptId {
		source: BattlePrefabObjectSource,
		script_id: [u8; 16],
	},
	ScriptSchemaHashMismatch {
		source: BattlePrefabObjectSource,
		script_id: [u8; 16],
		actual_old_type_hash: Option<[u8; 16]>,
		expected_old_type_hashes: Vec<[u8; 16]>,
	},
	ScriptHandlerFailed {
		source: BattlePrefabObjectSource,
		script_id: [u8; 16],
		old_type_hash: [u8; 16],
		reason: String,
	},
	CustomBuffSemanticsMissing {
		source: BattlePrefabObjectSource,
		field_path: String,
		buff_key: Option<String>,
		template_key: Option<String>,
	},
	/// A `MonoBehaviour` field points into a serialized file the verified snapshot does not pin.
	UnavailableExternalReference {
		source: BattlePrefabObjectSource,
		field_path: String,
		file_id: i32,
		path_id: i64,
		target_cab: String,
	},
}

/// A pure script handler. The graph and all references are complete before this function runs.
pub type BattlePrefabScriptHandler =
	fn(&BattlePrefabGraph, usize) -> Result<Vec<BattlePrefabSemantic>, String>;

/// Exact script-ID/old-type-hash dispatch table.
#[derive(Clone, Default)]
pub struct BattlePrefabHandlerRegistry {
	handlers: BTreeMap<([u8; 16], [u8; 16]), BattlePrefabScriptHandler>,
}

impl BattlePrefabHandlerRegistry {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn register(
		&mut self,
		script_id: [u8; 16],
		old_type_hash: [u8; 16],
		handler: BattlePrefabScriptHandler,
	) -> Result<(), BattlePrefabInterpretError> {
		if self
			.handlers
			.insert((script_id, old_type_hash), handler)
			.is_some()
		{
			return Err(BattlePrefabInterpretError::DuplicateScriptHandler {
				script_id,
				old_type_hash,
			});
		}
		Ok(())
	}

	pub fn contains(&self, script_id: &[u8; 16], old_type_hash: &[u8; 16]) -> bool {
		self.handlers.contains_key(&(*script_id, *old_type_hash))
	}

	fn handler(
		&self,
		script_id: &[u8; 16],
		old_type_hash: &[u8; 16],
	) -> Option<BattlePrefabScriptHandler> {
		self.handlers.get(&(*script_id, *old_type_hash)).copied()
	}

	fn contains_script_id(&self, script_id: &[u8; 16]) -> bool {
		self.handlers
			.keys()
			.any(|(candidate, _)| candidate == script_id)
	}

	fn old_type_hashes(&self, script_id: &[u8; 16]) -> Vec<[u8; 16]> {
		self.handlers
			.keys()
			.filter_map(|(candidate, old_type_hash)| {
				(candidate == script_id).then_some(*old_type_hash)
			})
			.collect()
	}
}

impl fmt::Debug for BattlePrefabHandlerRegistry {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("BattlePrefabHandlerRegistry")
			.field("script_schemas", &self.handlers.keys())
			.finish()
	}
}

/// Structural or reference error that prevents a complete hierarchy traversal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BattlePrefabInterpretError {
	BundleNotFound {
		logical_path: String,
	},
	PrefabNotFound {
		logical_path: String,
		prefab_name: String,
	},
	AmbiguousSectionContainer {
		logical_path: String,
		section: BattlePrefabRootSection,
		containers: Vec<String>,
	},
	AssetContainerNotFound {
		logical_path: String,
		asset_path: String,
	},
	AmbiguousAssetContainer {
		logical_path: String,
		asset_path: String,
		matches: Vec<BattlePrefabObjectSource>,
	},
	DuplicateScriptHandler {
		script_id: [u8; 16],
		old_type_hash: [u8; 16],
	},
	MissingField {
		source: BattlePrefabObjectSource,
		field: String,
	},
	InvalidField {
		source: BattlePrefabObjectSource,
		field: String,
		expected: &'static str,
	},
	ObjectResolution {
		source_cab: String,
		file_id: i32,
		path_id: i64,
		reason: String,
	},
	ExpectedGameObject {
		source: BattlePrefabObjectSource,
		unity_type: String,
	},
	ExpectedTransform {
		source: BattlePrefabObjectSource,
		unity_type: String,
	},
	TransformComponentCount {
		game_object: BattlePrefabObjectSource,
		count: usize,
	},
	TransformGameObjectMismatch {
		transform: BattlePrefabObjectSource,
		expected: Box<BattlePrefabObjectSource>,
		actual: Box<BattlePrefabObjectSource>,
	},
	HierarchyCycleOrDuplicate {
		source: BattlePrefabObjectSource,
	},
	MonoBehaviourOwnerConflict {
		behaviour: Box<BattlePrefabObjectSource>,
		first_game_object: Box<BattlePrefabObjectSource>,
		second_game_object: Box<BattlePrefabObjectSource>,
	},
}

impl fmt::Display for BattlePrefabInterpretError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::BundleNotFound { logical_path } => {
				write!(
					formatter,
					"battle-prefab bundle {logical_path:?} was not found"
				)
			}
			Self::PrefabNotFound {
				logical_path,
				prefab_name,
			} => write!(
				formatter,
				"bundle {logical_path:?} has no GameObject named exactly {prefab_name:?}"
			),
			Self::AmbiguousSectionContainer {
				logical_path,
				section,
				containers,
			} => write!(
				formatter,
				"bundle {logical_path:?} holds a {section:?} container at each of {containers:?}"
			),
			Self::AssetContainerNotFound {
				logical_path,
				asset_path,
			} => write!(
				formatter,
				"bundle {logical_path:?} has no AssetBundle container entry {asset_path:?}"
			),
			Self::AmbiguousAssetContainer {
				logical_path,
				asset_path,
				matches,
			} => write!(
				formatter,
				"bundle {logical_path:?} has {} AssetBundle container entries {asset_path:?}",
				matches.len()
			),
			Self::DuplicateScriptHandler {
				script_id,
				old_type_hash,
			} => write!(
				formatter,
				"script handler ({}, {}) was registered more than once",
				hex_script_id(script_id),
				hex_script_id(old_type_hash)
			),
			Self::MissingField { source, field } => write!(
				formatter,
				"{} pathID {} is missing field {field:?}",
				source.cab_name, source.path_id
			),
			Self::InvalidField {
				source,
				field,
				expected,
			} => write!(
				formatter,
				"{} pathID {} field {field:?} must be {expected}",
				source.cab_name, source.path_id
			),
			Self::ObjectResolution {
				source_cab,
				file_id,
				path_id,
				reason,
			} => write!(
				formatter,
				"could not resolve {source_cab} fileID {file_id} pathID {path_id}: {reason}"
			),
			Self::ExpectedGameObject { source, unity_type } => write!(
				formatter,
				"{} pathID {} is {unity_type}, not GameObject",
				source.cab_name, source.path_id
			),
			Self::ExpectedTransform { source, unity_type } => write!(
				formatter,
				"{} pathID {} is {unity_type}, not Transform",
				source.cab_name, source.path_id
			),
			Self::TransformComponentCount { game_object, count } => write!(
				formatter,
				"{} pathID {} has {count} Transform components; expected exactly one",
				game_object.cab_name, game_object.path_id
			),
			Self::TransformGameObjectMismatch {
				transform,
				expected,
				actual,
			} => write!(
				formatter,
				"{} pathID {} points to GameObject {} pathID {}, expected {} pathID {}",
				transform.cab_name,
				transform.path_id,
				actual.cab_name,
				actual.path_id,
				expected.cab_name,
				expected.path_id
			),
			Self::HierarchyCycleOrDuplicate { source } => write!(
				formatter,
				"GameObject {} pathID {} occurs more than once in the Transform hierarchy",
				source.cab_name, source.path_id
			),
			Self::MonoBehaviourOwnerConflict {
				behaviour,
				first_game_object,
				second_game_object,
			} => write!(
				formatter,
				"MonoBehaviour {} pathID {} is a component of both {} pathID {} and {} pathID {}",
				behaviour.cab_name,
				behaviour.path_id,
				first_game_object.cab_name,
				first_game_object.path_id,
				second_game_object.cab_name,
				second_game_object.path_id
			),
		}
	}
}

impl Error for BattlePrefabInterpretError {}

/// Locates every exact root for one character prefab, reconstructs their hierarchies, and
/// dispatches all scripts.
pub fn interpret_battle_prefab(
	snapshot: &BattlePrefabSnapshot,
	logical_path: &str,
	section: BattlePrefabRootSection,
	prefab_name: &str,
	registry: &BattlePrefabHandlerRegistry,
) -> Result<BattlePrefabGraph, BattlePrefabInterpretError> {
	let asset_key: &str = logical_path.rsplit('/').next().unwrap_or(logical_path);
	let asset_path: String = format!("dyn/battle/prefabs/characters/{asset_key}.prefab");
	interpret_battle_prefab_asset(
		snapshot,
		logical_path,
		&asset_path,
		section,
		prefab_name,
		registry,
	)
}

/// Locates roots through an exact AssetBundle container path, reconstructs their hierarchies, and
/// dispatches all scripts. This is the generic entry point for character and common prefab bundles.
pub fn interpret_battle_prefab_asset(
	snapshot: &BattlePrefabSnapshot,
	logical_path: &str,
	asset_path: &str,
	section: BattlePrefabRootSection,
	prefab_name: &str,
	registry: &BattlePrefabHandlerRegistry,
) -> Result<BattlePrefabGraph, BattlePrefabInterpretError> {
	let bundle = snapshot.bundle(logical_path).ok_or_else(|| {
		BattlePrefabInterpretError::BundleNotFound {
			logical_path: logical_path.to_string(),
		}
	})?;
	let asset_root: BattlePrefabObjectHandle<'_> = resolve_container_asset(
		snapshot,
		logical_path,
		asset_path,
		bundle.serialized_files(),
	)?;
	let (container_segments, roots): (&[&str], Vec<BattlePrefabObjectHandle<'_>>) =
		resolve_section_roots(snapshot, asset_root, section, prefab_name)?;
	let mut reader = GraphReader {
		snapshot,
		graph: BattlePrefabGraph {
			mechanics_revision: snapshot.mechanics_revision().to_string(),
			logical_path: bundle.logical_path().to_string(),
			asset_path: asset_path.to_owned(),
			section,
			prefab_name: prefab_name.to_string(),
			root_game_object_indices: Vec::with_capacity(roots.len()),
			game_objects: Vec::new(),
			mono_behaviours: Vec::new(),
			issues: Vec::new(),
		},
		visited_game_objects: BTreeMap::new(),
		visited_mono_behaviours: BTreeMap::new(),
	};
	for (index, root) in roots.into_iter().enumerate() {
		let hierarchy_path: String = match container_segments {
			[] => prefab_name.to_string(),
			segments => format!("{}/{prefab_name}#{index}", segments.join("/")),
		};
		let root_index: usize = reader.visit_game_object(root, hierarchy_path)?;
		reader.graph.root_game_object_indices.push(root_index);
	}
	let mut graph: BattlePrefabGraph = reader.graph;
	dispatch_semantic_handlers(&mut graph, registry);
	Ok(graph)
}

fn dispatch_semantic_handlers(
	graph: &mut BattlePrefabGraph,
	registry: &BattlePrefabHandlerRegistry,
) {
	let mut interpreted: Vec<Option<Vec<BattlePrefabSemantic>>> =
		vec![None; graph.mono_behaviours.len()];
	let mut issues: Vec<BattlePrefabSemanticIssue> = Vec::new();
	for (behaviour_index, behaviour) in graph.mono_behaviours.iter().enumerate() {
		let Some(script_id) = behaviour.script_id else {
			issues.push(BattlePrefabSemanticIssue::MissingScriptId {
				source: behaviour.source.clone(),
			});
			continue;
		};
		if !registry.contains_script_id(&script_id) {
			issues.push(BattlePrefabSemanticIssue::UnknownScriptId {
				source: behaviour.source.clone(),
				script_id,
			});
			continue;
		}
		let Some(old_type_hash) = behaviour.old_type_hash else {
			issues.push(BattlePrefabSemanticIssue::ScriptSchemaHashMismatch {
				source: behaviour.source.clone(),
				script_id,
				actual_old_type_hash: None,
				expected_old_type_hashes: registry.old_type_hashes(&script_id),
			});
			continue;
		};
		let Some(handler) = registry.handler(&script_id, &old_type_hash) else {
			issues.push(BattlePrefabSemanticIssue::ScriptSchemaHashMismatch {
				source: behaviour.source.clone(),
				script_id,
				actual_old_type_hash: Some(old_type_hash),
				expected_old_type_hashes: registry.old_type_hashes(&script_id),
			});
			continue;
		};
		match handler(graph, behaviour_index) {
			Ok(semantics) => interpreted[behaviour_index] = Some(semantics),
			Err(reason) => issues.push(BattlePrefabSemanticIssue::ScriptHandlerFailed {
				source: behaviour.source.clone(),
				script_id,
				old_type_hash,
				reason,
			}),
		}
	}
	for (behaviour_index, semantics) in interpreted.into_iter().enumerate() {
		let Some(semantics) = semantics else {
			continue;
		};
		let behaviour: &mut BattlePrefabMonoBehaviour = &mut graph.mono_behaviours[behaviour_index];
		let script_id: [u8; 16] = behaviour
			.script_id
			.expect("dispatched behaviour has a script ID");
		let old_type_hash: [u8; 16] = behaviour
			.old_type_hash
			.expect("dispatched behaviour has an old-type hash");
		let provenance: BattlePrefabSemanticProvenance = BattlePrefabSemanticProvenance {
			source: behaviour.source.clone(),
			script_id,
			old_type_hash,
			mechanics_revision: graph.mechanics_revision.clone(),
		};
		behaviour.semantic_facts = semantics
			.into_iter()
			.map(|semantic| BattlePrefabSemanticFact {
				provenance: provenance.clone(),
				semantic,
			})
			.collect();
	}
	graph.issues.extend(issues);
}

/// Scans every pinned bundle for AssetBundle container entries equal to `asset_path` bytewise.
///
/// Every match is reported with the manifest kind of the bundle that declares it; callers decide
/// whether zero matches, several, or a match outside the kind they expect is acceptable.
pub fn find_battle_prefab_containers(
	snapshot: &BattlePrefabSnapshot,
	asset_path: &str,
) -> Result<Vec<BattlePrefabContainerMatch>, BattlePrefabInterpretError> {
	let mut found: Vec<BattlePrefabContainerMatch> = Vec::new();
	for entry in snapshot.entries() {
		let bundle = snapshot.bundle(&entry.logical_path).ok_or_else(|| {
			BattlePrefabInterpretError::BundleNotFound {
				logical_path: entry.logical_path.clone(),
			}
		})?;
		let matches: Vec<(BattlePrefabObjectHandle<'_>, i32)> = container_asset_matches(
			snapshot,
			bundle.logical_path(),
			asset_path,
			bundle.serialized_files(),
		)?;
		for (root, container_file_id) in matches {
			found.push(BattlePrefabContainerMatch {
				logical_path: bundle.logical_path().to_string(),
				bundle_kind: entry.bundle_kind,
				container_file_id,
				root: object_source(root),
			});
		}
	}
	Ok(found)
}

fn resolve_container_asset<'a>(
	snapshot: &'a BattlePrefabSnapshot,
	logical_path: &str,
	asset_path: &str,
	files: &'a [BattlePrefabFile],
) -> Result<BattlePrefabObjectHandle<'a>, BattlePrefabInterpretError> {
	let matches: Vec<(BattlePrefabObjectHandle<'a>, i32)> =
		container_asset_matches(snapshot, logical_path, asset_path, files)?;
	match matches.as_slice() {
		[] => Err(BattlePrefabInterpretError::AssetContainerNotFound {
			logical_path: logical_path.to_string(),
			asset_path: asset_path.to_string(),
		}),
		[(root, _)] => {
			if !is_game_object(root.object().class_id, &root.object().unity_type) {
				return Err(BattlePrefabInterpretError::ExpectedGameObject {
					source: object_source(*root),
					unity_type: root.object().unity_type.clone(),
				});
			}
			Ok(*root)
		}
		_ => Err(BattlePrefabInterpretError::AmbiguousAssetContainer {
			logical_path: logical_path.to_string(),
			asset_path: asset_path.to_string(),
			matches: matches
				.into_iter()
				.map(|(handle, _)| object_source(handle))
				.collect(),
		}),
	}
}

/// Every container entry in `files` whose path equals `asset_path`, with its asset PPtr file ID.
fn container_asset_matches<'a>(
	snapshot: &'a BattlePrefabSnapshot,
	logical_path: &str,
	asset_path: &str,
	files: &'a [BattlePrefabFile],
) -> Result<Vec<(BattlePrefabObjectHandle<'a>, i32)>, BattlePrefabInterpretError> {
	let mut matches: Vec<(BattlePrefabObjectHandle<'a>, i32)> = Vec::new();
	for file in files {
		for object in &file.objects {
			if object.class_id != 142 && object.unity_type != "AssetBundle" {
				continue;
			}
			let source: BattlePrefabObjectSource = BattlePrefabObjectSource {
				logical_path: logical_path.to_string(),
				cab_name: file.cab_name.clone(),
				path_id: object.path_id,
			};
			let container: &[Value] = object
				.fields
				.get("m_Container")
				.and_then(Value::as_array)
				.map(Vec::as_slice)
				.ok_or_else(|| BattlePrefabInterpretError::InvalidField {
					source: source.clone(),
					field: "m_Container".to_string(),
					expected: "an array",
				})?;
			for (index, entry) in container.iter().enumerate() {
				let Some(pair) = entry.as_array() else {
					return Err(BattlePrefabInterpretError::InvalidField {
						source: source.clone(),
						field: format!("m_Container[{index}]"),
						expected: "a [path, asset] pair",
					});
				};
				if pair.len() != 2 || pair[0].as_str() != Some(asset_path) {
					continue;
				}
				let reference_value: &Value = pair[1].get("asset").ok_or_else(|| {
					BattlePrefabInterpretError::MissingField {
						source: source.clone(),
						field: format!("m_Container[{index}][1].asset"),
					}
				})?;
				let reference: UnityObjectReference = parse_reference(
					reference_value,
					&source,
					&format!("m_Container[{index}][1].asset"),
				)?;
				let root: BattlePrefabObjectHandle<'a> = resolve_object(
					snapshot,
					&file.cab_name,
					reference.file_id,
					reference.path_id,
				)?;
				matches.push((root, reference.file_id));
			}
		}
	}
	Ok(matches)
}

/// Resolves every root named `prefab_name` in the section's one container, together with the
/// hierarchy path of that container.
fn resolve_section_roots<'a>(
	snapshot: &'a BattlePrefabSnapshot,
	asset_root: BattlePrefabObjectHandle<'a>,
	section: BattlePrefabRootSection,
	prefab_name: &str,
) -> Result<(&'static [&'static str], Vec<BattlePrefabObjectHandle<'a>>), BattlePrefabInterpretError>
{
	let logical_path: String = asset_root.bundle().logical_path().to_string();
	if section == BattlePrefabRootSection::MainAsset {
		if required_string(asset_root, "m_Name")? == prefab_name {
			return Ok((&[], vec![asset_root]));
		}
		return Err(BattlePrefabInterpretError::PrefabNotFound {
			logical_path,
			prefab_name: prefab_name.to_string(),
		});
	}
	let mut containers: Vec<(&'static [&'static str], BattlePrefabObjectHandle<'a>)> = Vec::new();
	let mut missing: Vec<String> = Vec::new();
	for &segments in section.container_layouts() {
		match resolve_section_container(snapshot, asset_root, segments)? {
			Ok(container) => containers.push((segments, container)),
			Err(missing_path) => missing.push(format!("{missing_path}/{prefab_name}")),
		}
	}
	let (segments, container): (&'static [&'static str], BattlePrefabObjectHandle<'a>) =
		match containers.as_slice() {
			[] => {
				return Err(BattlePrefabInterpretError::PrefabNotFound {
					logical_path,
					prefab_name: missing.join(" | "),
				});
			}
			[container] => *container,
			_ => {
				return Err(BattlePrefabInterpretError::AmbiguousSectionContainer {
					logical_path,
					section,
					containers: containers
						.iter()
						.map(|(segments, _)| segments.join("/"))
						.collect(),
				});
			}
		};
	let mut roots: Vec<BattlePrefabObjectHandle<'a>> = Vec::new();
	for child in direct_game_object_children(snapshot, container)? {
		if required_string(child, "m_Name")? == prefab_name {
			roots.push(child);
		}
	}
	if roots.is_empty() {
		return Err(BattlePrefabInterpretError::PrefabNotFound {
			logical_path,
			prefab_name: format!("{}/{prefab_name}", segments.join("/")),
		});
	}
	Ok((segments, roots))
}

/// Walks one container layout, one uniquely named child per segment. A segment with no child of
/// that name, or with several, leaves the layout absent and names the path walked so far.
fn resolve_section_container<'a>(
	snapshot: &'a BattlePrefabSnapshot,
	asset_root: BattlePrefabObjectHandle<'a>,
	segments: &[&str],
) -> Result<Result<BattlePrefabObjectHandle<'a>, String>, BattlePrefabInterpretError> {
	let mut container: BattlePrefabObjectHandle<'a> = asset_root;
	for (segment_index, segment) in segments.iter().enumerate() {
		let mut matches: Vec<BattlePrefabObjectHandle<'a>> = Vec::new();
		for child in direct_game_object_children(snapshot, container)? {
			if required_string(child, "m_Name")? == *segment {
				matches.push(child);
			}
		}
		let [only] = matches.as_slice() else {
			return Ok(Err(segments[..=segment_index].join("/")));
		};
		container = *only;
	}
	Ok(Ok(container))
}

fn direct_game_object_children<'a>(
	snapshot: &'a BattlePrefabSnapshot,
	game_object: BattlePrefabObjectHandle<'a>,
) -> Result<Vec<BattlePrefabObjectHandle<'a>>, BattlePrefabInterpretError> {
	let source: BattlePrefabObjectSource = object_source(game_object);
	let component_values: &[Value] = required_array(game_object, "m_Component")?;
	let mut transforms: Vec<BattlePrefabObjectHandle<'a>> = Vec::new();
	for (index, component) in component_values.iter().enumerate() {
		let reference_value: &Value = component.get("component").unwrap_or(component);
		let reference: UnityObjectReference =
			parse_reference(reference_value, &source, &format!("m_Component[{index}]"))?;
		let handle: BattlePrefabObjectHandle<'a> = resolve_object(
			snapshot,
			&source.cab_name,
			reference.file_id,
			reference.path_id,
		)?;
		if is_transform(handle.object().class_id, &handle.object().unity_type) {
			transforms.push(handle);
		}
	}
	if transforms.len() != 1 {
		return Err(BattlePrefabInterpretError::TransformComponentCount {
			game_object: source,
			count: transforms.len(),
		});
	}
	let transform: BattlePrefabObjectHandle<'a> = transforms[0];
	let mut children: Vec<BattlePrefabObjectHandle<'a>> = Vec::new();
	for (index, child) in required_array(transform, "m_Children")?.iter().enumerate() {
		let transform_source: BattlePrefabObjectSource = object_source(transform);
		let reference: UnityObjectReference =
			parse_reference(child, &transform_source, &format!("m_Children[{index}]"))?;
		let child_transform: BattlePrefabObjectHandle<'a> = resolve_object(
			snapshot,
			&transform_source.cab_name,
			reference.file_id,
			reference.path_id,
		)?;
		if !is_transform(
			child_transform.object().class_id,
			&child_transform.object().unity_type,
		) {
			return Err(BattlePrefabInterpretError::ExpectedTransform {
				source: object_source(child_transform),
				unity_type: child_transform.object().unity_type.clone(),
			});
		}
		let child_source: BattlePrefabObjectSource = object_source(child_transform);
		let game_object_reference: UnityObjectReference = parse_reference(
			required_field(child_transform, "m_GameObject")?,
			&child_source,
			"m_GameObject",
		)?;
		children.push(resolve_object(
			snapshot,
			&child_source.cab_name,
			game_object_reference.file_id,
			game_object_reference.path_id,
		)?);
	}
	Ok(children)
}

struct GraphReader<'a> {
	snapshot: &'a BattlePrefabSnapshot,
	graph: BattlePrefabGraph,
	visited_game_objects: BTreeMap<(String, i64), usize>,
	visited_mono_behaviours: BTreeMap<(String, i64), usize>,
}

impl<'snapshot> GraphReader<'snapshot> {
	fn visit_game_object(
		&mut self,
		handle: BattlePrefabObjectHandle<'snapshot>,
		hierarchy_path: String,
	) -> Result<usize, BattlePrefabInterpretError> {
		let source: BattlePrefabObjectSource = object_source(handle);
		if !is_game_object(handle.object().class_id, &handle.object().unity_type) {
			return Err(BattlePrefabInterpretError::ExpectedGameObject {
				source,
				unity_type: handle.object().unity_type.clone(),
			});
		}
		let visit_key: (String, i64) = (source.cab_name.to_ascii_lowercase(), source.path_id);
		if self
			.visited_game_objects
			.insert(visit_key, self.graph.game_objects.len())
			.is_some()
		{
			return Err(BattlePrefabInterpretError::HierarchyCycleOrDuplicate { source });
		}

		let name: String = required_string(handle, "m_Name")?.to_string();
		let component_values: &[Value] = required_array(handle, "m_Component")?;
		let game_object_index: usize = self.graph.game_objects.len();
		self.graph.game_objects.push(BattlePrefabGameObject {
			source: source.clone(),
			name,
			hierarchy_path,
			component_sources: Vec::with_capacity(component_values.len()),
			mono_behaviour_indices: Vec::new(),
			child_indices: Vec::new(),
		});

		let mut transform_handles: Vec<BattlePrefabObjectHandle<'_>> = Vec::new();
		for (component_index, component_value) in component_values.iter().enumerate() {
			let field_path: String = format!("m_Component[{component_index}]");
			let reference_value: &Value =
				component_value.get("component").unwrap_or(component_value);
			let reference: UnityObjectReference =
				parse_reference(reference_value, &source, &field_path)?;
			let component = resolve_object(
				self.snapshot,
				&source.cab_name,
				reference.file_id,
				reference.path_id,
			)?;
			let component_source: BattlePrefabObjectSource = object_source(component);
			self.graph.game_objects[game_object_index]
				.component_sources
				.push(component_source.clone());
			if is_transform(component.object().class_id, &component.object().unity_type) {
				transform_handles.push(component);
			} else if is_mono_behaviour(component.object().class_id, &component.object().unity_type)
			{
				let behaviour_index: usize =
					self.visit_mono_behaviour(component, Some(game_object_index))?;
				self.graph.game_objects[game_object_index]
					.mono_behaviour_indices
					.push(behaviour_index);
			}
		}
		if transform_handles.len() != 1 {
			return Err(BattlePrefabInterpretError::TransformComponentCount {
				game_object: source,
				count: transform_handles.len(),
			});
		}
		let transform = transform_handles[0];
		self.validate_transform_owner(
			transform,
			&self.graph.game_objects[game_object_index].source,
		)?;
		let child_values: &[Value] = required_array(transform, "m_Children")?;
		for (child_index, child_value) in child_values.iter().enumerate() {
			let child_reference: UnityObjectReference = parse_reference(
				child_value,
				&object_source(transform),
				&format!("m_Children[{child_index}]"),
			)?;
			let child_transform = resolve_object(
				self.snapshot,
				&transform.file().cab_name,
				child_reference.file_id,
				child_reference.path_id,
			)?;
			if !is_transform(
				child_transform.object().class_id,
				&child_transform.object().unity_type,
			) {
				return Err(BattlePrefabInterpretError::ExpectedTransform {
					source: object_source(child_transform),
					unity_type: child_transform.object().unity_type.clone(),
				});
			}
			let child_game_object = self.transform_game_object(child_transform)?;
			let child_name: &str = required_string(child_game_object, "m_Name")?;
			let child_hierarchy_path: String = format!(
				"{}/{}",
				self.graph.game_objects[game_object_index].hierarchy_path, child_name
			);
			let child_game_object_index: usize =
				self.visit_game_object(child_game_object, child_hierarchy_path)?;
			self.graph.game_objects[game_object_index]
				.child_indices
				.push(child_game_object_index);
		}
		Ok(game_object_index)
	}

	fn visit_mono_behaviour(
		&mut self,
		handle: BattlePrefabObjectHandle<'snapshot>,
		game_object_index: Option<usize>,
	) -> Result<usize, BattlePrefabInterpretError> {
		let source: BattlePrefabObjectSource = object_source(handle);
		let visit_key: (String, i64) = (source.cab_name.to_ascii_lowercase(), source.path_id);
		if let Some(index) = self.visited_mono_behaviours.get(&visit_key).copied() {
			self.merge_mono_behaviour_owner(index, game_object_index)?;
			return Ok(index);
		}
		let script_id: Option<[u8; 16]> = handle.serialized_type().script_id().copied();
		let old_type_hash: Option<[u8; 16]> = handle.serialized_type().old_type_hash().copied();
		let script_class: Option<BattlePrefabScriptClass> = handle
			.object()
			.script_class
			.clone()
			.or_else(|| handle.serialized_type().script_class.clone());
		let buffs: Vec<BattlePrefabBuffSpec> = collect_buff_specs(&handle.object().fields);
		for buff in &buffs {
			if buff.attribute_modifiers.is_empty()
				&& (has_nonempty_key(&buff.buff_key) || has_nonempty_key(&buff.template_key))
			{
				self.graph
					.issues
					.push(BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
						source: source.clone(),
						field_path: buff.field_path.clone(),
						buff_key: buff.buff_key.clone(),
						template_key: buff.template_key.clone(),
					});
			}
		}
		let behaviour = BattlePrefabMonoBehaviour {
			game_object_index,
			source: source.clone(),
			script_id,
			old_type_hash,
			script_class,
			raw_fields: handle.object().fields.clone(),
			buffs,
			semantic_facts: Vec::new(),
			references: Vec::new(),
			unavailable_references: Vec::new(),
			referenced_mono_behaviour_indices: Vec::new(),
		};
		let behaviour_index: usize = self.graph.mono_behaviours.len();
		self.visited_mono_behaviours
			.insert(visit_key, behaviour_index);
		self.graph.mono_behaviours.push(behaviour);

		let mut raw_references: Vec<(String, UnityObjectReference)> = Vec::new();
		collect_object_references(
			&handle.object().fields,
			"$",
			None,
			&source,
			&mut raw_references,
		)?;
		for (field_path, reference) in raw_references {
			if reference.path_id == 0 {
				continue;
			}
			if let Some(target_cab) =
				unavailable_external_cab(self.snapshot, handle.file(), reference.file_id)
			{
				self.graph
					.issues
					.push(BattlePrefabSemanticIssue::UnavailableExternalReference {
						source: source.clone(),
						field_path: field_path.clone(),
						file_id: reference.file_id,
						path_id: reference.path_id,
						target_cab: target_cab.clone(),
					});
				self.graph.mono_behaviours[behaviour_index]
					.unavailable_references
					.push(BattlePrefabUnavailableReference {
						field_path,
						file_id: reference.file_id,
						path_id: reference.path_id,
						target_cab,
					});
				continue;
			}
			let target = resolve_object(
				self.snapshot,
				&handle.file().cab_name,
				reference.file_id,
				reference.path_id,
			)?;
			self.graph.mono_behaviours[behaviour_index].references.push(
				BattlePrefabResolvedReference {
					field_path,
					target: object_source(target),
				},
			);
			if is_mono_behaviour(target.object().class_id, &target.object().unity_type) {
				let referenced_index: usize = self.visit_mono_behaviour(target, None)?;
				let referenced_indices: &mut Vec<usize> = &mut self.graph.mono_behaviours
					[behaviour_index]
					.referenced_mono_behaviour_indices;
				if !referenced_indices.contains(&referenced_index) {
					referenced_indices.push(referenced_index);
				}
			}
		}
		Ok(behaviour_index)
	}

	fn merge_mono_behaviour_owner(
		&mut self,
		behaviour_index: usize,
		game_object_index: Option<usize>,
	) -> Result<(), BattlePrefabInterpretError> {
		let Some(game_object_index) = game_object_index else {
			return Ok(());
		};
		let existing_owner: Option<usize> =
			self.graph.mono_behaviours[behaviour_index].game_object_index;
		match existing_owner {
			None => {
				self.graph.mono_behaviours[behaviour_index].game_object_index =
					Some(game_object_index);
				Ok(())
			}
			Some(existing_owner) if existing_owner == game_object_index => Ok(()),
			Some(existing_owner) => Err(BattlePrefabInterpretError::MonoBehaviourOwnerConflict {
				behaviour: Box::new(self.graph.mono_behaviours[behaviour_index].source.clone()),
				first_game_object: Box::new(self.graph.game_objects[existing_owner].source.clone()),
				second_game_object: Box::new(
					self.graph.game_objects[game_object_index].source.clone(),
				),
			}),
		}
	}

	fn validate_transform_owner(
		&self,
		transform: BattlePrefabObjectHandle<'snapshot>,
		expected: &BattlePrefabObjectSource,
	) -> Result<(), BattlePrefabInterpretError> {
		let actual: BattlePrefabObjectSource =
			object_source(self.transform_game_object(transform)?);
		if same_object(expected, &actual) {
			return Ok(());
		}
		Err(BattlePrefabInterpretError::TransformGameObjectMismatch {
			transform: object_source(transform),
			expected: Box::new(expected.clone()),
			actual: Box::new(actual),
		})
	}

	fn transform_game_object(
		&self,
		transform: BattlePrefabObjectHandle<'snapshot>,
	) -> Result<BattlePrefabObjectHandle<'snapshot>, BattlePrefabInterpretError> {
		let source: BattlePrefabObjectSource = object_source(transform);
		let value: &Value = required_field(transform, "m_GameObject")?;
		let reference: UnityObjectReference = parse_reference(value, &source, "m_GameObject")?;
		resolve_object(
			self.snapshot,
			&transform.file().cab_name,
			reference.file_id,
			reference.path_id,
		)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UnityObjectReference {
	file_id: i32,
	path_id: i64,
}

fn collect_object_references(
	value: &Value,
	path: &str,
	field_name: Option<&str>,
	source: &BattlePrefabObjectSource,
	output: &mut Vec<(String, UnityObjectReference)>,
) -> Result<(), BattlePrefabInterpretError> {
	match value {
		Value::Object(object)
			if object.contains_key("m_FileID") && object.contains_key("m_PathID") =>
		{
			if !matches!(field_name, Some("m_GameObject" | "m_Script")) {
				output.push((path.to_owned(), parse_reference(value, source, path)?));
			}
		}
		Value::Object(object) => {
			for (field, child) in object {
				collect_object_references(
					child,
					&format!("{path}.{}", escape_path_field(field)),
					Some(field),
					source,
					output,
				)?;
			}
		}
		Value::Array(values) => {
			for (index, child) in values.iter().enumerate() {
				collect_object_references(
					child,
					&format!("{path}[{index}]"),
					field_name,
					source,
					output,
				)?;
			}
		}
		_ => {}
	}
	Ok(())
}

fn parse_reference(
	value: &Value,
	source: &BattlePrefabObjectSource,
	field: &str,
) -> Result<UnityObjectReference, BattlePrefabInterpretError> {
	let Some(object) = value.as_object() else {
		return Err(BattlePrefabInterpretError::InvalidField {
			source: source.clone(),
			field: field.to_string(),
			expected: "a PPtr object",
		});
	};
	let file_id: i32 = object
		.get("m_FileID")
		.and_then(Value::as_i64)
		.and_then(|value| i32::try_from(value).ok())
		.ok_or_else(|| BattlePrefabInterpretError::InvalidField {
			source: source.clone(),
			field: format!("{field}.m_FileID"),
			expected: "an i32",
		})?;
	let path_id: i64 = object
		.get("m_PathID")
		.and_then(Value::as_i64)
		.ok_or_else(|| BattlePrefabInterpretError::InvalidField {
			source: source.clone(),
			field: format!("{field}.m_PathID"),
			expected: "an i64",
		})?;
	Ok(UnityObjectReference { file_id, path_id })
}

/// The absent CAB a `MonoBehaviour` field PPtr points into, when that is the only thing wrong with it.
///
/// A fileID missing from the external table, an external without a CAB name, and a pinned CAB
/// lacking the object are malformed evidence, so they return `None` and fail in `resolve_object`.
fn unavailable_external_cab(
	snapshot: &BattlePrefabSnapshot,
	source: &BattlePrefabFile,
	file_id: i32,
) -> Option<String> {
	if file_id == 0 {
		return None;
	}
	let target_cab: &str = source.external(file_id)?.target_cab_name()?;
	snapshot
		.file(target_cab)
		.is_none()
		.then(|| target_cab.to_owned())
}

fn resolve_object<'a>(
	snapshot: &'a BattlePrefabSnapshot,
	source_cab: &str,
	file_id: i32,
	path_id: i64,
) -> Result<BattlePrefabObjectHandle<'a>, BattlePrefabInterpretError> {
	snapshot
		.object(source_cab, file_id, path_id)
		.map_err(|error| BattlePrefabInterpretError::ObjectResolution {
			source_cab: source_cab.to_string(),
			file_id,
			path_id,
			reason: error.to_string(),
		})
}

fn required_field<'a>(
	handle: BattlePrefabObjectHandle<'a>,
	field: &str,
) -> Result<&'a Value, BattlePrefabInterpretError> {
	handle
		.object()
		.fields
		.get(field)
		.ok_or_else(|| BattlePrefabInterpretError::MissingField {
			source: object_source(handle),
			field: field.to_string(),
		})
}

fn required_array<'a>(
	handle: BattlePrefabObjectHandle<'a>,
	field: &str,
) -> Result<&'a [Value], BattlePrefabInterpretError> {
	required_field(handle, field)?
		.as_array()
		.map(Vec::as_slice)
		.ok_or_else(|| BattlePrefabInterpretError::InvalidField {
			source: object_source(handle),
			field: field.to_string(),
			expected: "an array",
		})
}

fn required_string<'a>(
	handle: BattlePrefabObjectHandle<'a>,
	field: &str,
) -> Result<&'a str, BattlePrefabInterpretError> {
	required_field(handle, field)?.as_str().ok_or_else(|| {
		BattlePrefabInterpretError::InvalidField {
			source: object_source(handle),
			field: field.to_string(),
			expected: "a string",
		}
	})
}

fn object_source(handle: BattlePrefabObjectHandle<'_>) -> BattlePrefabObjectSource {
	BattlePrefabObjectSource {
		logical_path: handle.bundle().logical_path().to_string(),
		cab_name: handle.file().cab_name.clone(),
		path_id: handle.object().path_id,
	}
}

fn same_object(left: &BattlePrefabObjectSource, right: &BattlePrefabObjectSource) -> bool {
	left.cab_name.eq_ignore_ascii_case(&right.cab_name) && left.path_id == right.path_id
}

fn is_game_object(class_id: i32, unity_type: &str) -> bool {
	class_id == 1 || unity_type == "GameObject"
}

fn is_transform(class_id: i32, unity_type: &str) -> bool {
	class_id == 4 || class_id == 224 || unity_type == "Transform" || unity_type == "RectTransform"
}

fn is_mono_behaviour(class_id: i32, unity_type: &str) -> bool {
	class_id == 114 || unity_type == "MonoBehaviour"
}

fn collect_buff_specs(fields: &Value) -> Vec<BattlePrefabBuffSpec> {
	let mut buffs: Vec<BattlePrefabBuffSpec> = Vec::new();
	visit_buff_values(fields, "$", &mut buffs);
	buffs
}

fn visit_buff_values(value: &Value, path: &str, buffs: &mut Vec<BattlePrefabBuffSpec>) {
	match value {
		Value::Object(object) => {
			let buff_key: Option<String> = string_field(object, "buffKey");
			let template_key: Option<String> = string_field(object, "templateKey");
			if buff_key.is_some() || template_key.is_some() {
				buffs.push(BattlePrefabBuffSpec {
					field_path: path.to_string(),
					buff_key,
					template_key,
					attribute_modifiers: direct_attribute_modifiers(object),
				});
			}
			for (field, child) in object {
				visit_buff_values(
					child,
					&format!("{path}.{}", escape_path_field(field)),
					buffs,
				);
			}
		}
		Value::Array(values) => {
			for (index, child) in values.iter().enumerate() {
				visit_buff_values(child, &format!("{path}[{index}]"), buffs);
			}
		}
		_ => {}
	}
}

fn string_field(object: &Map<String, Value>, field: &str) -> Option<String> {
	object
		.get(field)
		.and_then(Value::as_str)
		.map(str::to_string)
}

fn direct_attribute_modifiers(object: &Map<String, Value>) -> Vec<Value> {
	object
		.get("attributes")
		.and_then(Value::as_object)
		.and_then(|attributes| attributes.get("attributeModifiers"))
		.and_then(Value::as_array)
		.cloned()
		.unwrap_or_default()
}

fn has_nonempty_key(value: &Option<String>) -> bool {
	value.as_ref().is_some_and(|value| !value.is_empty())
}

fn escape_path_field(field: &str) -> String {
	if field
		.chars()
		.all(|character| character == '_' || character.is_ascii_alphanumeric())
	{
		return field.to_string();
	}
	format!("[{field:?}]")
}

fn hex_script_id(script_id: &[u8; 16]) -> String {
	let mut output: String = String::with_capacity(32);
	for byte in script_id {
		use fmt::Write;
		write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
	}
	output
}

#[cfg(test)]
mod tests {
	use std::{
		fs,
		path::{Path, PathBuf},
		sync::atomic::{AtomicU64, Ordering},
	};

	use crate::data::{
		compute_mechanics_revision, BattlePrefabEntryKind, BattlePrefabManifestEntry,
		BattlePrefabSnapshot, BattlePrefabSource, BattlePrefabTooling,
	};
	use serde_json::{json, Value};
	use sha2::{Digest, Sha256};

	use super::{
		find_battle_prefab_containers, interpret_battle_prefab, interpret_battle_prefab_asset,
		required_string, resolve_container_asset, resolve_section_roots, BattlePrefabAttackAbility,
		BattlePrefabContainerMatch, BattlePrefabGraph, BattlePrefabHandlerRegistry,
		BattlePrefabInterpretError, BattlePrefabMonoBehaviour, BattlePrefabObjectReference,
		BattlePrefabObjectSource, BattlePrefabRootSection, BattlePrefabSemantic,
		BattlePrefabSemanticFact, BattlePrefabSemanticIssue, BattlePrefabUnavailableReference,
	};

	const SCRIPT_ID: [u8; 16] = [
		0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
		0xff,
	];
	const SCRIPT_ID_HEX: &str = "00112233445566778899aabbccddeeff";
	const OLD_TYPE_HASH: [u8; 16] = [
		0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
		0x00,
	];
	const OLD_TYPE_HASH_HEX: &str = "ffeeddccbbaa99887766554433221100";
	const SOURCE_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
	static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

	struct Fixture {
		root: PathBuf,
	}

	impl Fixture {
		fn new(custom_empty_buff: bool) -> Self {
			Self::from_export(rich_export(custom_empty_buff))
		}

		fn from_export(export: Value) -> Self {
			Self::from_exports(vec![(
				"charpack/test",
				BattlePrefabEntryKind::Charpack,
				export,
			)])
		}

		/// Pins every export under its logical path and bundle kind, in manifest order.
		fn from_exports(exports: Vec<(&str, BattlePrefabEntryKind, Value)>) -> Self {
			let id: u64 = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
			let root: PathBuf = std::env::temp_dir().join(format!(
				"ark_solver_prefab_interpreter_{}_{}",
				std::process::id(),
				id
			));
			fs::create_dir_all(root.join("exports")).expect("create fixture directory");
			let mut entries: Vec<BattlePrefabManifestEntry> = Vec::with_capacity(exports.len());
			for (index, (logical_path, bundle_kind, export)) in exports.into_iter().enumerate() {
				let export_path: String = format!("exports/{index}.json");
				let source_apk_path: String = export["source_bundle"]
					.as_str()
					.expect("fixture export names its source bundle")
					.to_string();
				let export_bytes: Vec<u8> = serde_json::to_vec(&export).expect("serialize export");
				fs::write(root.join(&export_path), &export_bytes).expect("write export");
				entries.push(BattlePrefabManifestEntry {
					logical_path: logical_path.to_string(),
					bundle_kind,
					source_apk_path,
					source_sha256: SOURCE_SHA256.to_string(),
					source_byte_length: 4096,
					export_path,
					export_sha256: sha256_hex(&export_bytes),
					export_byte_length: export_bytes.len() as u64,
				});
			}
			let source = BattlePrefabSource {
				apk_sha256: "a".repeat(64),
				apk_byte_length: 8192,
			};
			let tooling = BattlePrefabTooling {
				ark_ingest_version: "0.1.0".to_string(),
				exporter_version: "1".to_string(),
				python_version: "3.13.5".to_string(),
				unitypy_version: "1.23.0".to_string(),
				ark_unpacker_version: "fixture".to_string(),
			};
			let gamedata_revision: String = "c".repeat(40);
			let mechanics_revision: String =
				compute_mechanics_revision(&gamedata_revision, &source, &tooling, &entries);
			let manifest: Value = json!({
				"schema_version": 2,
				"mechanics_revision": mechanics_revision,
				"gamedata_revision": gamedata_revision,
				"generated_at": "2026-08-04T00:00:00Z",
				"source": source,
				"tooling": tooling,
				"entries": entries,
			});
			fs::write(
				root.join("manifest.json"),
				serde_json::to_vec(&manifest).expect("serialize manifest"),
			)
			.expect("write manifest");
			Self { root }
		}

		fn path(&self) -> &Path {
			&self.root
		}
	}

	impl Drop for Fixture {
		fn drop(&mut self) {
			let _result: std::io::Result<()> = fs::remove_dir_all(&self.root);
		}
	}

	fn rich_export(custom_empty_buff: bool) -> Value {
		let child_fields: Value = if custom_empty_buff {
			json!({
				"m_GameObject": pptr(2),
				"_buffs": [{
					"attributes": {"attributeModifiers": []},
					"buffKey": "fixture_custom_buff",
					"templateKey": "fixture_custom_buff",
				}],
			})
		} else {
			json!({"m_GameObject": pptr(2), "value": 2})
		};
		json!({
			"schema_version": 1,
			"source_bundle": "assets/AB/Android/charpack/test.ab",
			"source_sha256": SOURCE_SHA256,
			"bundle": {
				"signature": "UnityFS",
				"format_version": 7,
				"player_version": "2022.3.22f1",
				"engine_version": "2022.3.22f1",
			},
			"serialized_files": [{
				"cab_name": "CAB-TEST",
				"unity_version": "2022.3.22f1",
				"target_platform": 13,
				"enable_type_tree": true,
				"externals": [],
				"types": [
					type_document(0, 1, "GameObject", None),
					type_document(1, 4, "Transform", None),
					type_document(2, 114, "FixtureAbility", Some(SCRIPT_ID_HEX)),
					type_document(3, 142, "AssetBundle", None),
				],
				"objects": [
					object_document(1, 1, 0, "GameObject", json!({
						"m_Component": [
							{"component": pptr(10)},
							{"component": pptr(100)},
						],
						"m_Name": "Root",
					})),
					object_document(2, 1, 0, "GameObject", json!({
						"m_Component": [
							{"component": pptr(20)},
							{"component": pptr(200)},
						],
						"m_Name": "Child",
					})),
					object_document(10, 4, 1, "Transform", json!({
						"m_GameObject": pptr(1),
						"m_Children": [pptr(20)],
					})),
					object_document(20, 4, 1, "Transform", json!({
						"m_GameObject": pptr(2),
						"m_Children": [],
					})),
					object_document(100, 114, 2, "MonoBehaviour", json!({
						"m_GameObject": pptr(1),
						"value": 1,
					})),
					object_document(200, 114, 2, "MonoBehaviour", child_fields),
					object_document(3000, 142, 3, "AssetBundle", json!({
						"m_Container": [[
							"dyn/battle/prefabs/characters/test.prefab",
							{"asset": pptr(1)},
						]],
					})),
				],
			}],
		})
	}

	fn referenced_mono_behaviour_export() -> Value {
		json!({
			"schema_version": 1,
			"source_bundle": "assets/AB/Android/charpack/test.ab",
			"source_sha256": SOURCE_SHA256,
			"bundle": {
				"signature": "UnityFS",
				"format_version": 7,
				"player_version": "2022.3.22f1",
				"engine_version": "2022.3.22f1",
			},
			"serialized_files": [
				{
					"cab_name": "CAB-TEST",
					"unity_version": "2022.3.22f1",
					"target_platform": 13,
					"enable_type_tree": true,
					"externals": [external_document(1, "CAB-Z-DETACHED")],
					"types": [
						type_document(0, 1, "GameObject", None),
						type_document(1, 4, "Transform", None),
						type_document(2, 114, "FixtureAbility", Some(SCRIPT_ID_HEX)),
						type_document(3, 142, "AssetBundle", None),
					],
					"objects": [
						object_document(1, 1, 0, "GameObject", json!({
							"m_Component": [
								{"component": pptr(10)},
								{"component": pptr(100)},
							],
							"m_Name": "Root",
						})),
						object_document(10, 4, 1, "Transform", json!({
							"m_GameObject": pptr(1),
							"m_Children": [],
						})),
						object_document(100, 114, 2, "MonoBehaviour", json!({
							"m_GameObject": external_pptr(99, 900),
							"m_Script": external_pptr(99, 901),
							"attack": external_pptr(1, 300),
							"attackAlias": external_pptr(1, 300),
							"value": 1,
						})),
						object_document(3000, 142, 3, "AssetBundle", json!({
							"m_Container": [[
								"dyn/battle/prefabs/characters/test.prefab",
								{"asset": pptr(1)},
							]],
						})),
					],
				},
				{
					"cab_name": "CAB-Z-DETACHED",
					"unity_version": "2022.3.22f1",
					"target_platform": 13,
					"enable_type_tree": true,
					"externals": [external_document(1, "CAB-TEST")],
					"types": [
						type_document(0, 114, "DetachedAttack", Some(SCRIPT_ID_HEX)),
						type_document(1, 28, "Texture2D", None),
					],
					"objects": [
						object_document(300, 114, 0, "MonoBehaviour", json!({
							"m_GameObject": pptr(999),
							"rootCycle": external_pptr(1, 100),
							"texture": pptr(400),
							"value": 2,
						})),
						object_document(400, 28, 1, "Texture2D", json!({
							"nestedDanglingReference": pptr(9999),
						})),
					],
				},
			],
		})
	}

	/// Compact copy of the real Shu talent root shape — one container main asset, two sibling
	/// talent objects keyed `1`, three keyed `2`, plus an unrelated global name collision that must
	/// never participate in the logical root forest — with its `Talents` container at `container`
	/// beneath the prefab root, and an empty second container at `empty_container` when given.
	fn talent_roots_export(container: &[&str], empty_container: Option<&[&str]>) -> Value {
		let mut objects: Vec<Value> = vec![
			object_document(
				3000,
				142,
				3,
				"AssetBundle",
				json!({
					"m_Container": [[
						"dyn/battle/prefabs/characters/test.prefab",
						{"asset": pptr(1)},
					]],
				}),
			),
			object_document(
				1,
				1,
				0,
				"GameObject",
				json!({
					"m_Component": [{"component": pptr(10)}],
					"m_Name": "test",
				}),
			),
		];
		let mut root_children: Vec<Value> = Vec::new();
		for (chain_offset, segments, leaves) in [
			(
				1000,
				Some(container),
				vec![pptr(31), pptr(41), pptr(51), pptr(61), pptr(71)],
			),
			(2000, empty_container, Vec::new()),
		] {
			let Some(segments) = segments else {
				continue;
			};
			root_children.push(pptr(chain_offset + 100));
			for (index, segment) in segments.iter().enumerate() {
				let game_object_id: i64 = chain_offset + index as i64;
				let transform_id: i64 = game_object_id + 100;
				let children: Vec<Value> = if index + 1 == segments.len() {
					leaves.clone()
				} else {
					vec![pptr(transform_id + 1)]
				};
				objects.push(object_document(
					game_object_id,
					1,
					0,
					"GameObject",
					json!({
						"m_Component": [{"component": pptr(transform_id)}],
						"m_Name": segment,
					}),
				));
				objects.push(object_document(
					transform_id,
					4,
					1,
					"Transform",
					json!({
						"m_GameObject": pptr(game_object_id),
						"m_Children": children,
					}),
				));
			}
		}
		objects.push(object_document(
			10,
			4,
			1,
			"Transform",
			json!({
				"m_GameObject": pptr(1),
				"m_Children": root_children,
			}),
		));
		for (game_object_id, transform_id, behavior_id, name, value) in [
			(4, 31, 401, "1", 101),
			(3, 41, 301, "1", 102),
			(5, 51, 501, "2", 201),
			(6, 61, 601, "2", 202),
			(7, 71, 701, "2", 203),
			(8, 81, 801, "1", 999),
		] {
			objects.push(object_document(
				game_object_id,
				1,
				0,
				"GameObject",
				json!({
					"m_Component": [
						{"component": pptr(transform_id)},
						{"component": pptr(behavior_id)},
					],
					"m_Name": name,
				}),
			));
			objects.push(object_document(
				transform_id,
				4,
				1,
				"Transform",
				json!({
					"m_GameObject": pptr(game_object_id),
					"m_Children": [],
				}),
			));
			objects.push(object_document(
				behavior_id,
				114,
				2,
				"MonoBehaviour",
				json!({
					"m_GameObject": pptr(game_object_id),
					"value": value,
				}),
			));
		}
		objects.sort_by_key(|object| object["path_id"].as_i64().expect("fixture path id"));
		json!({
			"schema_version": 1,
			"source_bundle": "assets/AB/Android/charpack/test.ab",
			"source_sha256": SOURCE_SHA256,
			"bundle": {
				"signature": "UnityFS",
				"format_version": 7,
				"player_version": "2022.3.22f1",
				"engine_version": "2022.3.22f1",
			},
			"serialized_files": [{
				"cab_name": "CAB-TEST",
				"unity_version": "2022.3.22f1",
				"target_platform": 13,
				"enable_type_tree": true,
				"externals": [],
				"types": [
					type_document(0, 1, "GameObject", None),
					type_document(1, 4, "Transform", None),
					type_document(2, 114, "FixtureAbility", Some(SCRIPT_ID_HEX)),
					type_document(3, 142, "AssetBundle", None),
				],
				"objects": objects,
			}],
		})
	}

	fn type_document(
		type_index: i32,
		class_id: i32,
		class_name: &str,
		script_id: Option<&str>,
	) -> Value {
		let script_class: Value = if class_id == 114 {
			json!({
				"class_name": class_name,
				"namespace": "Torappu.Battle",
				"assembly_name": "Assembly-CSharp.dll",
			})
		} else {
			Value::Null
		};
		json!({
			"type_index": type_index,
			"class_id": class_id,
			"is_stripped_type": false,
			"script_type_index": if class_id == 114 { 0 } else { -1 },
			"script_id": script_id,
			"old_type_hash": script_id.map(|_| OLD_TYPE_HASH_HEX),
			"type_dependencies": [],
			"script_class": script_class,
			"schema": [],
		})
	}

	fn object_document(
		path_id: i64,
		class_id: i32,
		type_index: i32,
		unity_type: &str,
		fields: Value,
	) -> Value {
		let script_class: Value = if class_id == 114 {
			json!({
				"class_name": "FixtureAbility",
				"namespace": "Torappu.Battle",
				"assembly_name": "Assembly-CSharp.dll",
			})
		} else {
			Value::Null
		};
		json!({
			"path_id": path_id,
			"class_id": class_id,
			"type_index": type_index,
			"unity_type": unity_type,
			"byte_start": 128,
			"byte_size": 64,
			"script_class": script_class,
			"fields": fields,
		})
	}

	fn pptr(path_id: i64) -> Value {
		json!({"m_FileID": 0, "m_PathID": path_id})
	}

	fn external_pptr(file_id: i32, path_id: i64) -> Value {
		json!({"m_FileID": file_id, "m_PathID": path_id})
	}

	fn external_document(file_id: i32, cab_name: &str) -> Value {
		json!({
			"file_id": file_id,
			"guid": "dddddddddddddddddddddddddddddddd",
			"type": 0,
			"path": format!("archive:/fixture/{cab_name}"),
			"name": cab_name,
		})
	}

	const ENEMY_PACK: &str = "enm_pfb_0";
	const ENEMY_PREFAB_NAME: &str = "enemy_test";
	const ENEMY_PREFAB_ASSET_PATH: &str = "dyn/battle/prefabs/enemies/enemy_test.prefab";

	/// One enemy-pack prefab whose renderer-style behaviour points `skeleton` wherever a test needs.
	///
	/// fileID 2 names an art CAB that is never pinned; fileID 3 names this pack's own CAB.
	fn enemy_pack_export(skeleton: Value) -> Value {
		json!({
			"schema_version": 1,
			"source_bundle": "assets/AB/Android/battle/enm_pfb_0.ab",
			"source_sha256": SOURCE_SHA256,
			"bundle": {
				"signature": "UnityFS",
				"format_version": 7,
				"player_version": "2022.3.22f1",
				"engine_version": "2022.3.22f1",
			},
			"serialized_files": [{
				"cab_name": "CAB-ENEMY",
				"unity_version": "2022.3.22f1",
				"target_platform": 13,
				"enable_type_tree": true,
				"externals": [
					external_document(2, "CAB-ART-ABSENT"),
					external_document(3, "CAB-ENEMY"),
				],
				"types": [
					type_document(0, 1, "GameObject", None),
					type_document(1, 4, "Transform", None),
					type_document(2, 114, "FixtureAbility", Some(SCRIPT_ID_HEX)),
					type_document(3, 142, "AssetBundle", None),
				],
				"objects": [
					object_document(1, 1, 0, "GameObject", json!({
						"m_Component": [
							{"component": pptr(10)},
							{"component": pptr(100)},
						],
						"m_Name": ENEMY_PREFAB_NAME,
					})),
					object_document(10, 4, 1, "Transform", json!({
						"m_GameObject": pptr(1),
						"m_Children": [],
					})),
					object_document(100, 114, 2, "MonoBehaviour", json!({
						"m_GameObject": pptr(1),
						"m_Script": external_pptr(99, 901),
						"skeletonDataAsset": skeleton,
						"value": 1,
					})),
					object_document(3000, 142, 3, "AssetBundle", json!({
						"m_Container": [[ENEMY_PREFAB_ASSET_PATH, {"asset": pptr(1)}]],
					})),
				],
			}],
		})
	}

	fn interpret_enemy_pack(
		export: Value,
	) -> Result<BattlePrefabGraph, BattlePrefabInterpretError> {
		let fixture: Fixture =
			Fixture::from_exports(vec![(ENEMY_PACK, BattlePrefabEntryKind::Enemypack, export)]);
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");
		interpret_battle_prefab_asset(
			&snapshot,
			ENEMY_PACK,
			ENEMY_PREFAB_ASSET_PATH,
			BattlePrefabRootSection::MainAsset,
			ENEMY_PREFAB_NAME,
			&registry,
		)
	}

	fn sha256_hex(bytes: &[u8]) -> String {
		format!("{:x}", Sha256::digest(bytes))
	}

	fn fixture_handler(
		graph: &BattlePrefabGraph,
		behaviour_index: usize,
	) -> Result<Vec<BattlePrefabSemantic>, String> {
		let behaviour: &BattlePrefabMonoBehaviour = graph
			.mono_behaviours
			.get(behaviour_index)
			.ok_or_else(|| "fixture behaviour index is invalid".to_string())?;
		let value: f64 = behaviour.raw_fields["value"].as_f64().unwrap_or(0.0);
		Ok(vec![BattlePrefabSemantic::AttackAbility(
			BattlePrefabAttackAbility {
				target_selector: BattlePrefabObjectReference {
					file_id: 0,
					path_id: behaviour.source.path_id,
				},
				attack_scale: value,
				attack_scale_key: String::new(),
				attack_type: behaviour.references.len() as i64,
				damage_type: 0,
				element_damage_type: 0,
				extra_damage_type: 0,
				select_target_source: graph.mono_behaviours.len() as i64,
				select_target_timing: behaviour.referenced_mono_behaviour_indices.len() as i64,
			},
		)])
	}

	fn fixture_value(fact: &BattlePrefabSemanticFact) -> f64 {
		let BattlePrefabSemantic::AttackAbility(attack) = &fact.semantic else {
			panic!("fixture handler must emit typed attack semantics");
		};
		attack.attack_scale
	}

	#[test]
	fn walks_component_and_transform_pptr_graph() {
		let fixture: Fixture = Fixture::new(false);
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");

		let graph = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::MainAsset,
			"Root",
			&registry,
		)
		.expect("interpret fixture hierarchy");

		assert_eq!(graph.game_objects.len(), 2);
		assert_eq!(graph.mono_behaviours.len(), 2);
		assert_eq!(graph.game_objects[0].hierarchy_path, "Root");
		assert_eq!(graph.game_objects[0].child_indices, vec![1]);
		assert_eq!(graph.game_objects[1].hierarchy_path, "Root/Child");
		assert_eq!(graph.mono_behaviours[1].source.path_id, 200);
		assert_eq!(graph.mono_behaviours[1].game_object_index, Some(1));
		assert_eq!(graph.mono_behaviours[1].script_id, Some(SCRIPT_ID));
		assert_eq!(graph.mono_behaviours[1].old_type_hash, Some(OLD_TYPE_HASH));
		let fact: &BattlePrefabSemanticFact = &graph.mono_behaviours[1].semantic_facts[0];
		assert_eq!(fact.kind(), "attack_ability");
		assert_eq!(fact.provenance.source.path_id, 200);
		assert_eq!(fact.provenance.script_id, SCRIPT_ID);
		assert_eq!(fact.provenance.old_type_hash, OLD_TYPE_HASH);
		assert_eq!(fact.provenance.mechanics_revision, graph.mechanics_revision);
		assert!(graph.is_semantically_complete());
	}

	#[test]
	fn follows_off_hierarchy_mono_behaviour_references_once_across_cabs() {
		let fixture: Fixture = Fixture::from_export(referenced_mono_behaviour_export());
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");

		let graph = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::MainAsset,
			"Root",
			&registry,
		)
		.expect("follow detached behaviour references");

		assert_eq!(graph.game_objects.len(), 1);
		assert_eq!(graph.mono_behaviours.len(), 2);
		let root: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[0];
		let detached: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[1];
		assert_eq!(root.game_object_index, Some(0));
		assert_eq!(detached.game_object_index, None);
		assert_eq!(detached.source.cab_name, "CAB-Z-DETACHED");
		assert_eq!(detached.source.path_id, 300);
		assert_eq!(
			graph
				.mono_behaviours
				.iter()
				.filter(|behaviour| behaviour.source == detached.source)
				.count(),
			1
		);
		assert_eq!(root.referenced_mono_behaviour_indices, vec![1]);
		assert_eq!(detached.referenced_mono_behaviour_indices, vec![0]);

		let detached_reference_paths: Vec<&str> = root
			.references
			.iter()
			.filter(|reference| reference.target == detached.source)
			.map(|reference| reference.field_path.as_str())
			.collect();
		assert_eq!(detached_reference_paths, vec!["$.attack", "$.attackAlias"]);
		assert!(root.references.iter().all(|reference| {
			reference.field_path != "$.m_GameObject" && reference.field_path != "$.m_Script"
		}));
		assert!(detached
			.references
			.iter()
			.any(|reference| reference.target.path_id == 400));
		let BattlePrefabSemantic::AttackAbility(root_semantics) = &root.semantic_facts[0].semantic
		else {
			panic!("fixture handler must emit attack semantics");
		};
		assert_eq!(root_semantics.select_target_source, 2);
		assert_eq!(root_semantics.select_target_timing, 1);
		assert_eq!(root_semantics.attack_type, root.references.len() as i64);
		assert!(graph.is_semantically_complete());
	}

	#[test]
	fn a_behaviour_field_into_an_absent_cab_is_a_retained_semantic_issue() {
		let graph: BattlePrefabGraph =
			interpret_enemy_pack(enemy_pack_export(external_pptr(2, 77)))
				.expect("an absent art CAB must not abort traversal");

		let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[0];
		assert_eq!(
			behaviour.unavailable_references,
			vec![BattlePrefabUnavailableReference {
				field_path: "$.skeletonDataAsset".to_string(),
				file_id: 2,
				path_id: 77,
				target_cab: "CAB-ART-ABSENT".to_string(),
			}]
		);
		assert!(behaviour
			.references
			.iter()
			.all(|reference| reference.field_path != "$.skeletonDataAsset"));
		assert_eq!(
			graph.issues,
			vec![BattlePrefabSemanticIssue::UnavailableExternalReference {
				source: behaviour.source.clone(),
				field_path: "$.skeletonDataAsset".to_string(),
				file_id: 2,
				path_id: 77,
				target_cab: "CAB-ART-ABSENT".to_string(),
			}]
		);
		assert!(!graph.is_semantically_complete());
	}

	#[test]
	fn malformed_references_and_structural_edges_stay_hard_resolution_errors() {
		// A fileID the external table does not declare.
		assert!(matches!(
			interpret_enemy_pack(enemy_pack_export(external_pptr(5, 77))),
			Err(BattlePrefabInterpretError::ObjectResolution { file_id: 5, .. })
		));
		// A declared CAB that is pinned but lacks the object.
		assert!(matches!(
			interpret_enemy_pack(enemy_pack_export(external_pptr(3, 999))),
			Err(BattlePrefabInterpretError::ObjectResolution { file_id: 3, .. })
		));
		// An absent CAB on a component edge rather than inside a behaviour's fields.
		let mut component_edge: Value = enemy_pack_export(pptr(0));
		component_edge["serialized_files"][0]["objects"][0]["fields"]["m_Component"]
			.as_array_mut()
			.expect("component array")
			.push(json!({"component": external_pptr(2, 55)}));
		assert!(matches!(
			interpret_enemy_pack(component_edge),
			Err(BattlePrefabInterpretError::ObjectResolution { file_id: 2, .. })
		));
	}

	#[test]
	fn container_matches_report_every_bundle_and_kind_bytewise() {
		let mut enemy_pack: Value = enemy_pack_export(pptr(0));
		enemy_pack["serialized_files"][0]["objects"][3]["fields"]["m_Container"]
			.as_array_mut()
			.expect("container array")
			.push(json!([ENEMY_PREFAB_ASSET_PATH, {"asset": pptr(1)}]));
		let fixture: Fixture = Fixture::from_exports(vec![
			(
				"charpack/test",
				BattlePrefabEntryKind::Charpack,
				rich_export(false),
			),
			(ENEMY_PACK, BattlePrefabEntryKind::Enemypack, enemy_pack),
		]);
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");

		let enemy_root: BattlePrefabContainerMatch = BattlePrefabContainerMatch {
			logical_path: ENEMY_PACK.to_string(),
			bundle_kind: BattlePrefabEntryKind::Enemypack,
			container_file_id: 0,
			root: BattlePrefabObjectSource {
				logical_path: ENEMY_PACK.to_string(),
				cab_name: "CAB-ENEMY".to_string(),
				path_id: 1,
			},
		};
		assert_eq!(
			find_battle_prefab_containers(&snapshot, ENEMY_PREFAB_ASSET_PATH)
				.expect("scan enemy containers"),
			vec![enemy_root.clone(), enemy_root]
		);
		let characters: Vec<BattlePrefabContainerMatch> =
			find_battle_prefab_containers(&snapshot, "dyn/battle/prefabs/characters/test.prefab")
				.expect("scan character containers");
		assert_eq!(characters.len(), 1);
		assert_eq!(characters[0].bundle_kind, BattlePrefabEntryKind::Charpack);
		assert!(find_battle_prefab_containers(
			&snapshot,
			&ENEMY_PREFAB_ASSET_PATH.to_ascii_uppercase()
		)
		.expect("scan is bytewise")
		.is_empty());
	}

	#[test]
	fn asset_container_and_section_identity_aggregate_repeated_shu_style_talent_roots() {
		let fixture: Fixture =
			Fixture::from_export(talent_roots_export(&NESTED_TALENTS_CONTAINER, None));
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");

		let first = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::Talents,
			"1",
			&registry,
		)
		.expect("interpret all talent-key 1 roots");
		assert_eq!(first.root_game_object_indices.len(), 2);
		assert_eq!(first.game_objects.len(), 2);
		assert_eq!(first.mono_behaviours.len(), 2);
		assert_eq!(
			first
				.mono_behaviours
				.iter()
				.map(|behavior| fixture_value(&behavior.semantic_facts[0]))
				.collect::<Vec<_>>(),
			vec![101.0, 102.0],
		);
		assert!(first
			.mono_behaviours
			.iter()
			.all(|behavior| fixture_value(&behavior.semantic_facts[0]) != 999.0));

		let second = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::Talents,
			"2",
			&registry,
		)
		.expect("interpret all talent-key 2 roots");
		assert_eq!(second.root_game_object_indices.len(), 3);
		assert_eq!(second.game_objects.len(), 3);
		assert_eq!(second.mono_behaviours.len(), 3);
		assert_eq!(
			root_hierarchy_paths(&first),
			[
				"Others/Direction/Talents/1#0",
				"Others/Direction/Talents/1#1"
			]
		);
	}

	const NESTED_TALENTS_CONTAINER: [&str; 3] = ["Others", "Direction", "Talents"];
	const ROOT_LEVEL_TALENTS_CONTAINER: [&str; 1] = ["Talents"];

	fn root_hierarchy_paths(graph: &BattlePrefabGraph) -> Vec<&str> {
		graph
			.root_game_object_indices
			.iter()
			.map(|&index| graph.game_objects[index].hierarchy_path.as_str())
			.collect()
	}

	fn fixture_registry() -> BattlePrefabHandlerRegistry {
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");
		registry
	}

	#[test]
	fn a_talents_container_directly_under_the_root_holds_the_same_roots() {
		let fixture: Fixture =
			Fixture::from_export(talent_roots_export(&ROOT_LEVEL_TALENTS_CONTAINER, None));
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");

		let graph: BattlePrefabGraph = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::Talents,
			"1",
			&fixture_registry(),
		)
		.expect("interpret root-level talent-key 1 roots");
		assert_eq!(
			graph
				.mono_behaviours
				.iter()
				.map(|behavior| fixture_value(&behavior.semantic_facts[0]))
				.collect::<Vec<_>>(),
			vec![101.0, 102.0],
		);
		assert_eq!(root_hierarchy_paths(&graph), ["Talents/1#0", "Talents/1#1"]);
	}

	#[test]
	fn a_prefab_holding_both_talents_containers_is_ambiguous() {
		let fixture: Fixture = Fixture::from_export(talent_roots_export(
			&NESTED_TALENTS_CONTAINER,
			Some(&ROOT_LEVEL_TALENTS_CONTAINER),
		));
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");

		// The root-level container holds no roots at all; that it exists is already ambiguous.
		assert_eq!(
			interpret_battle_prefab(
				&snapshot,
				"charpack/test",
				BattlePrefabRootSection::Talents,
				"1",
				&fixture_registry(),
			)
			.expect_err("two talent containers must not be resolved by preference"),
			BattlePrefabInterpretError::AmbiguousSectionContainer {
				logical_path: "charpack/test".to_owned(),
				section: BattlePrefabRootSection::Talents,
				containers: vec!["Others/Direction/Talents".to_owned(), "Talents".to_owned()],
			}
		);
	}

	#[test]
	fn a_prefab_without_a_talents_container_names_every_layout_it_tried() {
		let fixture: Fixture = Fixture::from_export(talent_roots_export(&["Modes"], None));
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");

		assert_eq!(
			interpret_battle_prefab(
				&snapshot,
				"charpack/test",
				BattlePrefabRootSection::Talents,
				"1",
				&fixture_registry(),
			)
			.expect_err("no talent container exists"),
			BattlePrefabInterpretError::PrefabNotFound {
				logical_path: "charpack/test".to_owned(),
				prefab_name: "Others/1 | Talents/1".to_owned(),
			}
		);
	}

	#[test]
	#[ignore = "requires configured battle-prefab snapshot"]
	fn local_verified_shu_root_forest_matches_container_hierarchy() {
		let prefab_path: PathBuf = crate::test_support::configured_battle_prefab_dir();
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		let bundle = snapshot.bundle("char_2025_shu").expect("Shu bundle");
		let asset_path: &str = "dyn/battle/prefabs/characters/char_2025_shu.prefab";
		let asset_root = resolve_container_asset(
			&snapshot,
			"char_2025_shu",
			asset_path,
			bundle.serialized_files(),
		)
		.expect("resolve exact Shu AssetBundle container root");
		assert_eq!(
			required_string(asset_root, "m_Name").unwrap(),
			"char_2025_shu"
		);
		assert_eq!(
			resolve_section_roots(&snapshot, asset_root, BattlePrefabRootSection::Talents, "1",)
				.expect("resolve Shu sow roots")
				.1
				.len(),
			2
		);
		assert_eq!(
			resolve_section_roots(&snapshot, asset_root, BattlePrefabRootSection::Talents, "2",)
				.expect("resolve Shu composition roots")
				.1
				.len(),
			3
		);
	}

	/// Charpacks authored before talents moved under `Others/Direction` keep them directly under
	/// the prefab root, and 杜宾 authors his talent inside each skill mode instead.
	#[test]
	#[ignore = "requires configured battle-prefab snapshot"]
	fn local_verified_charpacks_hold_their_talents_in_either_layout() {
		let prefab_path: PathBuf = crate::test_support::configured_battle_prefab_dir();
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		let registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		let talent_root_paths =
			|operator: &str| -> Result<Vec<String>, BattlePrefabInterpretError> {
				interpret_battle_prefab(
					&snapshot,
					operator,
					BattlePrefabRootSection::Talents,
					"1",
					&registry,
				)
				.map(|graph| {
					root_hierarchy_paths(&graph)
						.into_iter()
						.map(str::to_owned)
						.collect()
				})
			};
		for operator in ["char_198_blackd", "char_150_snakek", "char_204_platnm"] {
			assert_eq!(
				talent_root_paths(operator),
				Ok(vec!["Talents/1#0".to_owned()])
			);
		}
		for operator in ["char_2025_shu", "char_1046_sbell2"] {
			assert!(talent_root_paths(operator)
				.expect("nested talent roots resolve")
				.iter()
				.all(|path| path.starts_with("Others/Direction/Talents/1#")));
		}
		assert_eq!(
			talent_root_paths("char_130_doberm"),
			Err(BattlePrefabInterpretError::PrefabNotFound {
				logical_path: "char_130_doberm".to_owned(),
				prefab_name: "Others/Direction/Talents/1 | Talents/1".to_owned(),
			})
		);
	}

	#[test]
	fn unknown_script_id_is_a_fail_closed_issue() {
		let fixture: Fixture = Fixture::new(false);
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");

		let graph = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::MainAsset,
			"Root",
			&BattlePrefabHandlerRegistry::new(),
		)
		.expect("structural traversal still succeeds");

		assert!(!graph.is_semantically_complete());
		assert_eq!(
			graph
				.issues
				.iter()
				.filter(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. }))
				.count(),
			2
		);
	}

	#[test]
	fn known_script_with_changed_schema_hash_is_a_distinct_fail_closed_issue() {
		let mut export: Value = rich_export(false);
		export["serialized_files"][0]["types"][2]["old_type_hash"] =
			json!("00000000000000000000000000000000");
		let fixture: Fixture = Fixture::from_export(export);
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");

		let graph = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::MainAsset,
			"Root",
			&registry,
		)
		.expect("structural traversal still succeeds");

		assert!(!graph.is_semantically_complete());
		assert!(graph
			.mono_behaviours
			.iter()
			.all(|behaviour| behaviour.semantic_facts.is_empty()));
		assert_eq!(
			graph
				.issues
				.iter()
				.filter(|issue| matches!(
					issue,
					BattlePrefabSemanticIssue::ScriptSchemaHashMismatch {
						actual_old_type_hash: Some(actual_old_type_hash),
						expected_old_type_hashes,
						..
					} if *actual_old_type_hash == [0; 16]
						&& expected_old_type_hashes == &vec![OLD_TYPE_HASH]
				))
				.count(),
			2
		);
	}

	#[test]
	fn empty_attribute_modifiers_mark_custom_buff_semantics_missing() {
		let fixture: Fixture = Fixture::new(true);
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(fixture.path()).expect("load fixture");
		let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
		registry
			.register(SCRIPT_ID, OLD_TYPE_HASH, fixture_handler)
			.expect("register fixture handler");

		let graph = interpret_battle_prefab(
			&snapshot,
			"charpack/test",
			BattlePrefabRootSection::MainAsset,
			"Root",
			&registry,
		)
		.expect("interpret fixture hierarchy");

		assert_eq!(graph.mono_behaviours[1].buffs.len(), 1);
		assert!(graph.mono_behaviours[1].buffs[0]
			.attribute_modifiers
			.is_empty());
		assert!(matches!(
			graph.issues.as_slice(),
			[BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
				buff_key: Some(buff_key),
				template_key: Some(template_key),
				..
			}] if buff_key == "fixture_custom_buff" && template_key == "fixture_custom_buff"
		));
	}

	#[test]
	#[ignore = "requires configured battle-prefab snapshot"]
	fn local_verified_common_skill_uses_explicit_asset_path() {
		let prefab_path: PathBuf = crate::test_support::configured_battle_prefab_dir();
		let snapshot: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		let graph: BattlePrefabGraph = interpret_battle_prefab_asset(
			&snapshot,
			"[uc]skills",
			"dyn/battle/prefabs/[uc]skills/skchr_sbell2_2.prefab",
			BattlePrefabRootSection::MainAsset,
			"skchr_sbell2_2",
			&BattlePrefabHandlerRegistry::new(),
		)
		.expect("interpret reviewed common skill graph through its exact container path");

		assert_eq!(graph.logical_path, "[uc]skills");
		assert_eq!(
			graph.asset_path,
			"dyn/battle/prefabs/[uc]skills/skchr_sbell2_2.prefab"
		);
		assert_eq!(graph.root_game_object_indices.len(), 1);
		assert_eq!(graph.game_objects.len(), 1);
		assert_eq!(graph.mono_behaviours.len(), 3);
		assert_eq!(
			graph
				.issues
				.iter()
				.filter(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. }))
				.count(),
			3
		);
		assert_eq!(
			graph
				.issues
				.iter()
				.filter(|issue| matches!(
					issue,
					BattlePrefabSemanticIssue::CustomBuffSemanticsMissing { .. }
				))
				.count(),
			10
		);
	}
}
