//! Operator-agnostic handlers for verified, reusable battle-prefab scripts.
//!
//! Every registration is keyed by an exact Unity `(script_id, old_type_hash)` pair. Handlers
//! retain direct TypeTree configuration and reject missing or malformed required fields; they do
//! not infer behavior from prefab names, operator data, skill data, or custom buff keys.

use serde_json::{Map, Value};

use crate::mechanics::battle_prefab_interpreter::{
	BattlePrefabAbilityConfiguration, BattlePrefabAttackAbility,
	BattlePrefabAttributeConfiguration, BattlePrefabAttributeModifier, BattlePrefabBlackboardEntry,
	BattlePrefabBuffConfiguration, BattlePrefabBuffConfigurationSet, BattlePrefabCompositeAbility,
	BattlePrefabGraph, BattlePrefabHandlerRegistry, BattlePrefabListenerTiming,
	BattlePrefabMonoBehaviour, BattlePrefabObjectReference, BattlePrefabSemantic,
	BattlePrefabStaticBuffAbility, BattlePrefabTileListener,
};

/// Verified script ID for the composite ability containing `_abilities` and `_mainAbility`.
pub const COMPOSITE_ABILITY_SCRIPT_ID: [u8; 16] = [
	0x6c, 0xea, 0x82, 0x1b, 0x94, 0x28, 0x0f, 0xf2, 0x95, 0xe6, 0xa3, 0x94, 0x21, 0x72, 0x8f, 0xa1,
];
pub const COMPOSITE_ABILITY_OLD_TYPE_HASH: [u8; 16] = [
	0x4f, 0x49, 0xa1, 0xe7, 0xeb, 0x11, 0x7c, 0x65, 0xdd, 0xa9, 0x61, 0x4d, 0x22, 0xef, 0xa2, 0x50,
];

/// Verified script ID for the attack ability containing `_atkScale` and `_damageType`.
pub const ATTACK_ABILITY_SCRIPT_ID: [u8; 16] = [
	0xc1, 0xb4, 0x4c, 0x3b, 0xb3, 0x49, 0x0f, 0x6d, 0x64, 0xea, 0x0c, 0xd1, 0xd6, 0x09, 0x44, 0x0c,
];
pub const ATTACK_ABILITY_OLD_TYPE_HASH: [u8; 16] = [
	0x62, 0x0b, 0x15, 0x91, 0x0b, 0x23, 0xf3, 0xf2, 0xfc, 0x51, 0x51, 0x05, 0x14, 0x8b, 0xa9, 0xe2,
];

/// Verified script ID for the static ability containing the direct `_buffs` list.
pub const STATIC_BUFF_ABILITY_SCRIPT_ID: [u8; 16] = [
	0x8b, 0x09, 0xc9, 0xf0, 0xba, 0xf5, 0xb0, 0xc8, 0x16, 0x94, 0xb5, 0xe5, 0x8b, 0x8b, 0x3e, 0x93,
];
pub const STATIC_BUFF_ABILITY_OLD_TYPE_HASH: [u8; 16] = [
	0x2c, 0xe7, 0xae, 0xc3, 0x9c, 0x64, 0xd9, 0x33, 0xe2, 0xf8, 0xe3, 0xf7, 0x26, 0xb3, 0xe4, 0x88,
];

/// Verified script ID for the listener applying buff configurations on tile events.
pub const TILE_LISTENER_SCRIPT_ID: [u8; 16] = [
	0x8c, 0x0c, 0xa4, 0x6a, 0xae, 0x34, 0x55, 0x2d, 0xb8, 0x72, 0xc2, 0xef, 0xc3, 0xb4, 0x2b, 0x26,
];
pub const TILE_LISTENER_OLD_TYPE_HASH: [u8; 16] = [
	0x62, 0x2b, 0x58, 0x5c, 0x88, 0xf9, 0x22, 0xbd, 0x03, 0xa7, 0xb1, 0xca, 0xa9, 0x6d, 0x85, 0xa4,
];

/// Builds the reviewed, operator-agnostic script registry.
pub fn built_in_battle_prefab_handler_registry() -> BattlePrefabHandlerRegistry {
	let mut registry: BattlePrefabHandlerRegistry = BattlePrefabHandlerRegistry::new();
	registry
		.register(
			COMPOSITE_ABILITY_SCRIPT_ID,
			COMPOSITE_ABILITY_OLD_TYPE_HASH,
			handle_composite_ability,
		)
		.expect("verified built-in battle-prefab script IDs are unique");
	registry
		.register(
			ATTACK_ABILITY_SCRIPT_ID,
			ATTACK_ABILITY_OLD_TYPE_HASH,
			handle_attack_ability,
		)
		.expect("verified built-in battle-prefab script IDs are unique");
	registry
		.register(
			STATIC_BUFF_ABILITY_SCRIPT_ID,
			STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
			handle_static_buff_ability,
		)
		.expect("verified built-in battle-prefab script IDs are unique");
	registry
		.register(
			TILE_LISTENER_SCRIPT_ID,
			TILE_LISTENER_OLD_TYPE_HASH,
			handle_tile_listener,
		)
		.expect("verified built-in battle-prefab script IDs are unique");
	registry
}

fn handle_composite_ability(
	graph: &BattlePrefabGraph,
	behaviour_index: usize,
) -> Result<Vec<BattlePrefabSemantic>, String> {
	let behaviour: &BattlePrefabMonoBehaviour = behaviour(graph, behaviour_index)?;
	let fields: &Map<String, Value> = raw_fields(behaviour)?;
	let target_selector: BattlePrefabObjectReference =
		required_reference(fields, "_selector", "$")?;
	let child_abilities: Vec<BattlePrefabObjectReference> =
		required_reference_array(fields, "_abilities", "$")?;
	let main_ability: BattlePrefabObjectReference =
		required_reference(fields, "_mainAbility", "$")?;
	Ok(vec![BattlePrefabSemantic::CompositeAbility(
		BattlePrefabCompositeAbility {
			target_selector,
			child_abilities,
			main_ability,
		},
	)])
}

fn handle_attack_ability(
	graph: &BattlePrefabGraph,
	behaviour_index: usize,
) -> Result<Vec<BattlePrefabSemantic>, String> {
	let behaviour: &BattlePrefabMonoBehaviour = behaviour(graph, behaviour_index)?;
	let fields: &Map<String, Value> = raw_fields(behaviour)?;
	let target_selector: BattlePrefabObjectReference =
		required_reference(fields, "_selector", "$")?;
	let attack_scale: f64 = required_number(fields, "_atkScale", "$")?;
	let attack_scale_key: &str = required_string(fields, "_atkScaleKey", "$")?;
	let attack_type: i64 = required_integer(fields, "_attackType", "$")?;
	let damage_type: i64 = required_integer(fields, "_damageType", "$")?;
	let element_damage_type: i64 = required_integer(fields, "_elementDamageType", "$")?;
	let extra_damage_type: i64 = required_integer(fields, "_extraDamageType", "$")?;
	let select_target_source: i64 = required_integer(fields, "_selectTargetSource", "$")?;
	let select_target_timing: i64 = required_integer(fields, "_selectTargetTiming", "$")?;
	Ok(vec![BattlePrefabSemantic::AttackAbility(
		BattlePrefabAttackAbility {
			target_selector,
			attack_scale,
			attack_scale_key: attack_scale_key.to_string(),
			attack_type,
			damage_type,
			element_damage_type,
			extra_damage_type,
			select_target_source,
			select_target_timing,
		},
	)])
}

fn handle_static_buff_ability(
	graph: &BattlePrefabGraph,
	behaviour_index: usize,
) -> Result<Vec<BattlePrefabSemantic>, String> {
	let behaviour: &BattlePrefabMonoBehaviour = behaviour(graph, behaviour_index)?;
	let fields: &Map<String, Value> = raw_fields(behaviour)?;
	reject_unexpected_fields(
		fields,
		&[
			"m_GameObject",
			"m_Enabled",
			"m_Script",
			"m_Name",
			"_selector",
			"_metadata",
			"_interruptAbilityOnDetach",
			"_attachPassiveBuffsOnDummy",
			"_ignoreIfOwnerDead",
			"_forceAsDmgOrHealAbility",
			"_buffs",
		],
		"$",
	)?;
	let target_selector: BattlePrefabObjectReference =
		required_reference(fields, "_selector", "$")?;
	let metadata: &Map<String, Value> = required_object(fields, "_metadata", "$")?;
	reject_unexpected_fields(
		metadata,
		&["namedAsAlias", "blackboardPrefix"],
		"$._metadata",
	)?;
	let enabled: bool = required_binary_flag(fields, "m_Enabled", "$")?;
	let metadata_named_as_alias: &str = required_string(metadata, "namedAsAlias", "$._metadata")?;
	let metadata_blackboard_prefix: &str =
		required_string(metadata, "blackboardPrefix", "$._metadata")?;
	let interrupt_on_detach: bool = required_binary_flag(fields, "_interruptAbilityOnDetach", "$")?;
	let attach_passive_buffs_on_dummy: bool =
		required_binary_flag(fields, "_attachPassiveBuffsOnDummy", "$")?;
	let ignore_if_owner_dead: bool = required_binary_flag(fields, "_ignoreIfOwnerDead", "$")?;
	let force_as_damage_or_heal: bool =
		required_binary_flag(fields, "_forceAsDmgOrHealAbility", "$")?;
	let buffs: Vec<BattlePrefabBuffConfigurationSet> =
		required_buff_configurations(fields, "_buffs", "$")?;
	Ok(vec![BattlePrefabSemantic::StaticBuffAbility(
		BattlePrefabStaticBuffAbility {
			target_selector,
			ability_configuration: BattlePrefabAbilityConfiguration {
				enabled,
				metadata_named_as_alias: metadata_named_as_alias.to_string(),
				metadata_blackboard_prefix: metadata_blackboard_prefix.to_string(),
				interrupt_on_detach,
				attach_passive_buffs_on_dummy,
				ignore_if_owner_dead,
				force_as_damage_or_heal,
			},
			buffs,
		},
	)])
}

fn handle_tile_listener(
	graph: &BattlePrefabGraph,
	behaviour_index: usize,
) -> Result<Vec<BattlePrefabSemantic>, String> {
	let behaviour: &BattlePrefabMonoBehaviour = behaviour(graph, behaviour_index)?;
	let fields: &Map<String, Value> = raw_fields(behaviour)?;
	let target_validator: BattlePrefabObjectReference =
		required_reference(fields, "_targetValidator", "$")?;
	let buffs_when_casted: Vec<BattlePrefabBuffConfigurationSet> =
		required_buff_configurations(fields, "_buffsWhenCasted", "$")?;
	let buffs_when_enemy_enter: Vec<BattlePrefabBuffConfigurationSet> =
		required_buff_configurations(fields, "_buffsWhenEnemyEnter", "$")?;
	let buffs_when_enemy_leave: Vec<BattlePrefabBuffConfigurationSet> =
		required_buff_configurations(fields, "_buffsWhenEnemyLeave", "$")?;
	let clear_when_ability_detached: bool =
		required_binary_flag(fields, "_clearBuffWhenAbilityDetached", "$")?;
	let clear_when_listener_removed: bool =
		required_binary_flag(fields, "_clearBuffWhenTileListenerRemoved", "$")?;
	Ok(vec![BattlePrefabSemantic::TileListener(
		BattlePrefabTileListener {
			target_validator,
			buffs_when_casted,
			buffs_when_enemy_enter,
			buffs_when_enemy_leave,
			clear_when_ability_detached,
			clear_when_listener_removed,
		},
	)])
}

fn behaviour(
	graph: &BattlePrefabGraph,
	behaviour_index: usize,
) -> Result<&BattlePrefabMonoBehaviour, String> {
	graph
		.mono_behaviours
		.get(behaviour_index)
		.ok_or_else(|| format!("behaviour index {behaviour_index} is outside the complete graph"))
}

fn raw_fields(behaviour: &BattlePrefabMonoBehaviour) -> Result<&Map<String, Value>, String> {
	behaviour
		.raw_fields
		.as_object()
		.ok_or_else(|| "raw TypeTree fields must be an object".to_string())
}

fn required_field<'a>(
	object: &'a Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<&'a Value, String> {
	object
		.get(field)
		.ok_or_else(|| format!("missing required field {parent_path}.{field}"))
}

fn reject_unexpected_fields(
	object: &Map<String, Value>,
	allowed_fields: &[&str],
	path: &str,
) -> Result<(), String> {
	for field in object.keys() {
		if !allowed_fields.contains(&field.as_str()) {
			return Err(format!("unexpected field {path}.{field}"));
		}
	}
	Ok(())
}

fn required_object<'a>(
	object: &'a Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<&'a Map<String, Value>, String> {
	let path: String = format!("{parent_path}.{field}");
	required_field(object, field, parent_path)?
		.as_object()
		.ok_or_else(|| format!("required field {path} must be an object"))
}

fn required_array<'a>(
	object: &'a Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<&'a [Value], String> {
	let path: String = format!("{parent_path}.{field}");
	required_field(object, field, parent_path)?
		.as_array()
		.map(Vec::as_slice)
		.ok_or_else(|| format!("required field {path} must be an array"))
}

fn required_string<'a>(
	object: &'a Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<&'a str, String> {
	let path: String = format!("{parent_path}.{field}");
	required_field(object, field, parent_path)?
		.as_str()
		.ok_or_else(|| format!("required field {path} must be a string"))
}

fn required_number(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<f64, String> {
	let path: String = format!("{parent_path}.{field}");
	required_field(object, field, parent_path)?
		.as_f64()
		.ok_or_else(|| format!("required field {path} must be a finite JSON number"))
}

fn required_integer(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<i64, String> {
	let path: String = format!("{parent_path}.{field}");
	required_field(object, field, parent_path)?
		.as_i64()
		.ok_or_else(|| format!("required field {path} must be an i64"))
}

fn required_binary_flag(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<bool, String> {
	let path: String = format!("{parent_path}.{field}");
	match required_integer(object, field, parent_path)? {
		0 => Ok(false),
		1 => Ok(true),
		value => Err(format!(
			"required field {path} must be the integer 0 or 1, found {value}"
		)),
	}
}

fn required_reference(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<BattlePrefabObjectReference, String> {
	let path: String = format!("{parent_path}.{field}");
	let reference: &Map<String, Value> = required_object(object, field, parent_path)?;
	normalize_reference(reference, &path)
}

fn required_reference_array(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<Vec<BattlePrefabObjectReference>, String> {
	let path: String = format!("{parent_path}.{field}");
	required_array(object, field, parent_path)?
		.iter()
		.enumerate()
		.map(|(index, value)| {
			let item_path: String = format!("{path}[{index}]");
			let reference: &Map<String, Value> = value
				.as_object()
				.ok_or_else(|| format!("required field {item_path} must be an object"))?;
			normalize_reference(reference, &item_path)
		})
		.collect()
}

fn required_integer_array(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<Vec<i64>, String> {
	let path: String = format!("{parent_path}.{field}");
	required_array(object, field, parent_path)?
		.iter()
		.enumerate()
		.map(|(index, value)| {
			value
				.as_i64()
				.ok_or_else(|| format!("required field {path}[{index}] must be an i64"))
		})
		.collect()
}

fn required_string_array(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<Vec<String>, String> {
	let path: String = format!("{parent_path}.{field}");
	required_array(object, field, parent_path)?
		.iter()
		.enumerate()
		.map(|(index, value)| {
			value
				.as_str()
				.map(str::to_string)
				.ok_or_else(|| format!("required field {path}[{index}] must be a string"))
		})
		.collect()
}

fn required_blackboard_entries(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<Vec<BattlePrefabBlackboardEntry>, String> {
	let path: String = format!("{parent_path}.{field}");
	required_array(object, field, parent_path)?
		.iter()
		.enumerate()
		.map(|(index, value)| {
			let entry_path: String = format!("{path}[{index}]");
			let entry: &Map<String, Value> = value.as_object().ok_or_else(|| {
				format!("required field {entry_path} must be a blackboard object")
			})?;
			reject_unexpected_fields(entry, &["key", "value", "valueStr"], &entry_path)?;
			Ok(BattlePrefabBlackboardEntry {
				key: required_string(entry, "key", &entry_path)?.to_string(),
				value: required_number(entry, "value", &entry_path)?,
				value_string: required_string(entry, "valueStr", &entry_path)?.to_string(),
			})
		})
		.collect()
}

fn normalize_reference(
	reference: &Map<String, Value>,
	path: &str,
) -> Result<BattlePrefabObjectReference, String> {
	reject_unexpected_fields(reference, &["m_FileID", "m_PathID"], path)?;
	let file_id_value: i64 = required_integer(reference, "m_FileID", path)?;
	let file_id: i32 = i32::try_from(file_id_value)
		.map_err(|_| format!("required field {path}.m_FileID must fit in an i32"))?;
	let path_id: i64 = required_integer(reference, "m_PathID", path)?;
	Ok(BattlePrefabObjectReference { file_id, path_id })
}

fn required_buff_configurations(
	object: &Map<String, Value>,
	field: &str,
	parent_path: &str,
) -> Result<Vec<BattlePrefabBuffConfigurationSet>, String> {
	let path: String = format!("{parent_path}.{field}");
	required_array(object, field, parent_path)?
		.iter()
		.enumerate()
		.map(|(index, value)| normalize_buff_configuration(value, &format!("{path}[{index}]")))
		.collect()
}

fn normalize_buff_configuration(
	value: &Value,
	path: &str,
) -> Result<BattlePrefabBuffConfigurationSet, String> {
	let buff: &Map<String, Value> = value
		.as_object()
		.ok_or_else(|| format!("required field {path} must be an object"))?;
	reject_unexpected_fields(
		buff,
		&[
			"attributes",
			"audioSignal",
			"blackboard",
			"buffKey",
			"clearAllStackCntWhenTimeUp",
			"disableOverride",
			"durationKey",
			"enableInitDirectionFromSource",
			"firstTriggerInterval",
			"independentCharacterSource",
			"isDamageMissable",
			"isDurableBuff",
			"isFreezable",
			"isLevitatable",
			"isSilenceable",
			"isStunnable",
			"lifeTime",
			"lifeTimeType",
			"loadFromDB",
			"maxStackCnt",
			"maxValidStackCnt",
			"onEventPriority",
			"overrideEffectKey",
			"overrideKey",
			"overrideOnEventPriority",
			"overrideType",
			"priority",
			"priorityBBKeys",
			"refreshRemainingTimeWhenStackMax",
			"statusResistable",
			"stripBlackboardParamsWithBuffKey",
			"takeSnapshotWhenExtend",
			"templateKey",
			"triggerCnt",
			"triggerInterval",
			"triggerLifeType",
			"waitFirstTriggerInterval",
		],
		path,
	)?;
	let attributes: &Map<String, Value> = required_object(buff, "attributes", path)?;
	reject_unexpected_fields(
		attributes,
		&[
			"abnormalAntis",
			"abnormalComboImmunes",
			"abnormalCombos",
			"abnormalFlags",
			"abnormalImmunes",
			"attributeModifiers",
		],
		&format!("{path}.attributes"),
	)?;
	let attribute_modifiers: Vec<BattlePrefabAttributeModifier> = required_array(
		attributes,
		"attributeModifiers",
		&format!("{path}.attributes"),
	)?
	.iter()
	.enumerate()
	.map(|(index, modifier)| {
		normalize_attribute_modifier(
			modifier,
			&format!("{path}.attributes.attributeModifiers[{index}]"),
		)
	})
	.collect::<Result<_, _>>()?;
	let abnormal_antis: Vec<i64> =
		required_integer_array(attributes, "abnormalAntis", &format!("{path}.attributes"))?;
	let abnormal_combo_immunes: Vec<i64> = required_integer_array(
		attributes,
		"abnormalComboImmunes",
		&format!("{path}.attributes"),
	)?;
	let abnormal_combos: Vec<i64> =
		required_integer_array(attributes, "abnormalCombos", &format!("{path}.attributes"))?;
	let abnormal_flags: Vec<i64> =
		required_integer_array(attributes, "abnormalFlags", &format!("{path}.attributes"))?;
	let abnormal_immunes: Vec<i64> =
		required_integer_array(attributes, "abnormalImmunes", &format!("{path}.attributes"))?;
	let audio_signal: &str = required_string(buff, "audioSignal", path)?;
	let blackboard: Vec<BattlePrefabBlackboardEntry> =
		required_blackboard_entries(buff, "blackboard", path)?;
	let buff_key: &str = required_string(buff, "buffKey", path)?;
	let clear_all_stack_count_when_time_up: bool =
		required_binary_flag(buff, "clearAllStackCntWhenTimeUp", path)?;
	let disable_override: bool = required_binary_flag(buff, "disableOverride", path)?;
	let duration_key: &str = required_string(buff, "durationKey", path)?;
	let enable_initial_direction_from_source: bool =
		required_binary_flag(buff, "enableInitDirectionFromSource", path)?;
	let independent_character_source: bool =
		required_binary_flag(buff, "independentCharacterSource", path)?;
	let is_damage_missable: bool = required_binary_flag(buff, "isDamageMissable", path)?;
	let is_durable: bool = required_binary_flag(buff, "isDurableBuff", path)?;
	let is_freezable: bool = required_binary_flag(buff, "isFreezable", path)?;
	let is_levitatable: bool = required_binary_flag(buff, "isLevitatable", path)?;
	let is_silenceable: bool = required_binary_flag(buff, "isSilenceable", path)?;
	let is_stunnable: bool = required_binary_flag(buff, "isStunnable", path)?;
	let load_from_database: bool = required_binary_flag(buff, "loadFromDB", path)?;
	let max_stack_count: i64 = required_integer(buff, "maxStackCnt", path)?;
	let max_valid_stack_count: i64 = required_integer(buff, "maxValidStackCnt", path)?;
	let on_event_priority: i64 = required_integer(buff, "onEventPriority", path)?;
	let override_effect_key: &str = required_string(buff, "overrideEffectKey", path)?;
	let override_key: &str = required_string(buff, "overrideKey", path)?;
	let override_on_event_priority: i64 = required_integer(buff, "overrideOnEventPriority", path)?;
	let override_type: i64 = required_integer(buff, "overrideType", path)?;
	let priority: i64 = required_integer(buff, "priority", path)?;
	let priority_blackboard_keys: Vec<String> =
		required_string_array(buff, "priorityBBKeys", path)?;
	let refresh_remaining_time_when_stack_max: bool =
		required_binary_flag(buff, "refreshRemainingTimeWhenStackMax", path)?;
	let status_resistable: i64 = required_integer(buff, "statusResistable", path)?;
	let strip_blackboard_params_with_buff_key: bool =
		required_binary_flag(buff, "stripBlackboardParamsWithBuffKey", path)?;
	let take_snapshot_when_extend: bool =
		required_binary_flag(buff, "takeSnapshotWhenExtend", path)?;
	let template_key: &str = required_string(buff, "templateKey", path)?;
	let life_time: f64 = required_number(buff, "lifeTime", path)?;
	let life_time_type: i64 = required_integer(buff, "lifeTimeType", path)?;
	let first_trigger_interval: f64 = required_number(buff, "firstTriggerInterval", path)?;
	let trigger_interval: f64 = required_number(buff, "triggerInterval", path)?;
	let trigger_count: i64 = required_integer(buff, "triggerCnt", path)?;
	let trigger_life_type: i64 = required_integer(buff, "triggerLifeType", path)?;
	let wait_first_trigger_interval: bool =
		required_binary_flag(buff, "waitFirstTriggerInterval", path)?;
	Ok(BattlePrefabBuffConfigurationSet {
		attribute_modifiers,
		attribute_configuration: BattlePrefabAttributeConfiguration {
			abnormal_antis,
			abnormal_combo_immunes,
			abnormal_combos,
			abnormal_flags,
			abnormal_immunes,
		},
		configuration: BattlePrefabBuffConfiguration {
			audio_signal: audio_signal.to_string(),
			blackboard,
			buff_key: buff_key.to_string(),
			clear_all_stack_count_when_time_up,
			disable_override,
			duration_key: duration_key.to_string(),
			enable_initial_direction_from_source,
			independent_character_source,
			is_damage_missable,
			is_durable,
			is_freezable,
			is_levitatable,
			is_silenceable,
			is_stunnable,
			load_from_database,
			max_stack_count,
			max_valid_stack_count,
			on_event_priority,
			override_effect_key: override_effect_key.to_string(),
			override_key: override_key.to_string(),
			override_on_event_priority,
			override_type,
			priority,
			priority_blackboard_keys,
			refresh_remaining_time_when_stack_max,
			status_resistable,
			strip_blackboard_params_with_buff_key,
			take_snapshot_when_extend,
			template_key: template_key.to_string(),
		},
		listener_timing: BattlePrefabListenerTiming {
			life_time,
			life_time_type,
			first_trigger_interval,
			trigger_interval,
			trigger_count,
			trigger_life_type,
			wait_first_trigger_interval,
		},
	})
}

fn normalize_attribute_modifier(
	value: &Value,
	path: &str,
) -> Result<BattlePrefabAttributeModifier, String> {
	let modifier: &Map<String, Value> = value
		.as_object()
		.ok_or_else(|| format!("required field {path} must be an object"))?;
	reject_unexpected_fields(
		modifier,
		&[
			"attributeType",
			"formulaItem",
			"value",
			"loadFromBlackboard",
			"fetchBaseValueFromSourceEntity",
		],
		path,
	)?;
	let attribute_type: i64 = required_integer(modifier, "attributeType", path)?;
	let formula_item: i64 = required_integer(modifier, "formulaItem", path)?;
	let modifier_value: f64 = required_number(modifier, "value", path)?;
	let load_from_blackboard: bool = required_binary_flag(modifier, "loadFromBlackboard", path)?;
	let fetch_base_value_from_source_entity: bool =
		required_binary_flag(modifier, "fetchBaseValueFromSourceEntity", path)?;
	Ok(BattlePrefabAttributeModifier {
		attribute_type,
		formula_item,
		value: modifier_value,
		load_from_blackboard,
		fetch_base_value_from_source_entity,
	})
}

#[cfg(test)]
mod tests {
	use serde_json::{json, Value};

	use crate::mechanics::battle_prefab_interpreter::{
		BattlePrefabAttackAbility, BattlePrefabGameObject, BattlePrefabGraph,
		BattlePrefabMonoBehaviour, BattlePrefabObjectReference, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemantic,
	};

	use super::{
		built_in_battle_prefab_handler_registry, handle_attack_ability, handle_composite_ability,
		handle_static_buff_ability, handle_tile_listener, ATTACK_ABILITY_OLD_TYPE_HASH,
		ATTACK_ABILITY_SCRIPT_ID, COMPOSITE_ABILITY_OLD_TYPE_HASH, COMPOSITE_ABILITY_SCRIPT_ID,
		STATIC_BUFF_ABILITY_OLD_TYPE_HASH, STATIC_BUFF_ABILITY_SCRIPT_ID,
		TILE_LISTENER_OLD_TYPE_HASH, TILE_LISTENER_SCRIPT_ID,
	};

	fn graph(script_id: [u8; 16], raw_fields: Value) -> BattlePrefabGraph {
		let old_type_hash: [u8; 16] = match script_id {
			COMPOSITE_ABILITY_SCRIPT_ID => COMPOSITE_ABILITY_OLD_TYPE_HASH,
			ATTACK_ABILITY_SCRIPT_ID => ATTACK_ABILITY_OLD_TYPE_HASH,
			STATIC_BUFF_ABILITY_SCRIPT_ID => STATIC_BUFF_ABILITY_OLD_TYPE_HASH,
			TILE_LISTENER_SCRIPT_ID => TILE_LISTENER_OLD_TYPE_HASH,
			_ => panic!("test graph requires a verified script schema"),
		};
		let source: BattlePrefabObjectSource = BattlePrefabObjectSource {
			logical_path: "fixture".to_string(),
			cab_name: "CAB-fixture".to_string(),
			path_id: 1,
		};
		BattlePrefabGraph {
			mechanics_revision: "a".repeat(64),
			logical_path: "fixture".to_string(),
			asset_path: "fixture.prefab".to_string(),
			section: BattlePrefabRootSection::MainAsset,
			prefab_name: "fixture".to_string(),
			root_game_object_indices: vec![0],
			game_objects: vec![BattlePrefabGameObject {
				source: source.clone(),
				name: "fixture".to_string(),
				hierarchy_path: "fixture".to_string(),
				component_sources: vec![source.clone()],
				mono_behaviour_indices: vec![0],
				child_indices: Vec::new(),
			}],
			mono_behaviours: vec![BattlePrefabMonoBehaviour {
				game_object_index: Some(0),
				source,
				script_id: Some(script_id),
				old_type_hash: Some(old_type_hash),
				script_class: None,
				raw_fields,
				buffs: Vec::new(),
				semantic_facts: Vec::new(),
				references: Vec::new(),
				unavailable_references: Vec::new(),
				referenced_mono_behaviour_indices: Vec::new(),
			}],
			issues: Vec::new(),
		}
	}

	fn reference(path_id: i64) -> Value {
		json!({"m_FileID": 0, "m_PathID": path_id})
	}

	fn buff(attribute_modifiers: Value) -> Value {
		json!({
			"attributes": {
				"abnormalAntis": [],
				"abnormalComboImmunes": [],
				"abnormalCombos": [],
				"abnormalFlags": [],
				"abnormalImmunes": [],
				"attributeModifiers": attribute_modifiers,
			},
			"audioSignal": "",
			"blackboard": [],
			"buffKey": "must_not_drive_handler_semantics",
			"clearAllStackCntWhenTimeUp": 0,
			"disableOverride": 0,
			"durationKey": "duration",
			"enableInitDirectionFromSource": 0,
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
			"templateKey": "must_not_drive_handler_semantics",
			"lifeTime": 2.5,
			"lifeTimeType": 2,
			"firstTriggerInterval": 0.25,
			"triggerInterval": 1.5,
			"triggerCnt": 3,
			"triggerLifeType": 1,
			"waitFirstTriggerInterval": 1,
		})
	}

	fn static_ability(buffs: Value) -> Value {
		json!({
			"m_Enabled": 1,
			"_selector": reference(0),
			"_metadata": {
				"namedAsAlias": "",
				"blackboardPrefix": "",
			},
			"_interruptAbilityOnDetach": 0,
			"_attachPassiveBuffsOnDummy": 0,
			"_ignoreIfOwnerDead": 0,
			"_forceAsDmgOrHealAbility": 0,
			"_buffs": buffs,
		})
	}

	#[test]
	fn registry_contains_every_verified_script_id() {
		let registry = built_in_battle_prefab_handler_registry();
		assert!(registry.contains(
			&COMPOSITE_ABILITY_SCRIPT_ID,
			&COMPOSITE_ABILITY_OLD_TYPE_HASH
		));
		assert!(registry.contains(&ATTACK_ABILITY_SCRIPT_ID, &ATTACK_ABILITY_OLD_TYPE_HASH));
		assert!(registry.contains(
			&STATIC_BUFF_ABILITY_SCRIPT_ID,
			&STATIC_BUFF_ABILITY_OLD_TYPE_HASH
		));
		assert!(registry.contains(&TILE_LISTENER_SCRIPT_ID, &TILE_LISTENER_OLD_TYPE_HASH));
		assert!(!registry.contains(&COMPOSITE_ABILITY_SCRIPT_ID, &[0xff; 16]));
	}

	#[test]
	fn composite_handler_emits_only_direct_references() {
		let semantics: Vec<BattlePrefabSemantic> = handle_composite_ability(
			&graph(
				COMPOSITE_ABILITY_SCRIPT_ID,
				json!({
				"_selector": reference(4),
				"_abilities": [reference(10), reference(11)],
				"_mainAbility": reference(10),
				}),
			),
			0,
		)
		.expect("valid composite ability");
		assert_eq!(
			semantics,
			vec![BattlePrefabSemantic::CompositeAbility(
				crate::mechanics::battle_prefab_interpreter::BattlePrefabCompositeAbility {
					target_selector: BattlePrefabObjectReference {
						file_id: 0,
						path_id: 4,
					},
					child_abilities: vec![
						BattlePrefabObjectReference {
							file_id: 0,
							path_id: 10,
						},
						BattlePrefabObjectReference {
							file_id: 0,
							path_id: 11,
						},
					],
					main_ability: BattlePrefabObjectReference {
						file_id: 0,
						path_id: 10,
					},
				},
			)],
		);
	}

	#[test]
	fn attack_handler_emits_scale_damage_types_and_target_configuration() {
		let semantics: Vec<BattlePrefabSemantic> = handle_attack_ability(
			&graph(
				ATTACK_ABILITY_SCRIPT_ID,
				json!({
				"_selector": reference(22),
				"_atkScale": 1.25,
				"_atkScaleKey": "atk_scale",
				"_attackType": 1,
				"_damageType": 2,
				"_elementDamageType": 3,
				"_extraDamageType": 4,
				"_selectTargetSource": 2,
				"_selectTargetTiming": 0,
				}),
			),
			0,
		)
		.expect("valid attack ability");
		let BattlePrefabSemantic::AttackAbility(BattlePrefabAttackAbility {
			target_selector,
			attack_scale,
			attack_scale_key,
			damage_type,
			..
		}) = &semantics[0]
		else {
			panic!("expected typed attack semantics");
		};
		assert_eq!(*attack_scale, 1.25);
		assert_eq!(attack_scale_key, "atk_scale");
		assert_eq!(*damage_type, 2);
		assert_eq!(
			*target_selector,
			BattlePrefabObjectReference {
				file_id: 0,
				path_id: 22,
			}
		);
	}

	#[test]
	fn static_buff_handler_retains_complete_configuration_without_key_dispatch() {
		let semantics: Vec<BattlePrefabSemantic> = handle_static_buff_ability(
			&graph(
				STATIC_BUFF_ABILITY_SCRIPT_ID,
				static_ability(json!([buff(json!([{
				"attributeType": 1,
				"formulaItem": 2,
				"value": 0.75,
				"loadFromBlackboard": 1,
				"fetchBaseValueFromSourceEntity": 0,
				}]))])),
			),
			0,
		)
		.expect("valid static buff ability");
		let BattlePrefabSemantic::StaticBuffAbility(ability) = &semantics[0] else {
			panic!("expected typed static-buff semantics");
		};
		let modifier = &ability.buffs[0].attribute_modifiers[0];
		assert_eq!(modifier.attribute_type, 1);
		assert_eq!(modifier.formula_item, 2);
		assert_eq!(modifier.value, 0.75);
		assert!(modifier.load_from_blackboard);
		assert!(!modifier.fetch_base_value_from_source_entity);
		assert_eq!(ability.buffs[0].listener_timing.trigger_interval, 1.5);
		assert_eq!(
			ability.buffs[0].configuration.buff_key,
			"must_not_drive_handler_semantics"
		);
		assert!(!ability.ability_configuration.interrupt_on_detach);
	}

	#[test]
	fn tile_listener_handler_separates_event_buff_lists() {
		let semantics: Vec<BattlePrefabSemantic> = handle_tile_listener(
			&graph(
				TILE_LISTENER_SCRIPT_ID,
				json!({
				"_targetValidator": reference(31),
				"_buffsWhenCasted": [],
				"_buffsWhenEnemyEnter": [buff(json!([]))],
				"_buffsWhenEnemyLeave": [buff(json!([]))],
				"_clearBuffWhenAbilityDetached": 1,
				"_clearBuffWhenTileListenerRemoved": 0,
				}),
			),
			0,
		)
		.expect("valid tile listener");
		let BattlePrefabSemantic::TileListener(listener) = &semantics[0] else {
			panic!("expected typed tile-listener semantics");
		};
		assert!(listener.buffs_when_casted.is_empty());
		assert_eq!(
			listener.buffs_when_enemy_enter[0]
				.listener_timing
				.first_trigger_interval,
			0.25
		);
		assert!(listener.clear_when_ability_detached);
		assert!(!listener.clear_when_listener_removed);
	}

	#[test]
	fn handlers_reject_missing_or_invalid_required_configuration() {
		let composite_error = handle_composite_ability(
			&graph(
				COMPOSITE_ABILITY_SCRIPT_ID,
				json!({
				"_selector": reference(0),
				"_abilities": [{"m_FileID": 0}],
				"_mainAbility": reference(0),
				}),
			),
			0,
		)
		.expect_err("invalid child reference must fail closed");
		assert!(composite_error.contains("m_PathID"));

		let attack_error = handle_attack_ability(
			&graph(
				ATTACK_ABILITY_SCRIPT_ID,
				json!({
				"_selector": reference(0),
				"_atkScale": 1.0,
				"_atkScaleKey": "atk_scale",
				"_attackType": 1,
				"_elementDamageType": 0,
				"_extraDamageType": 0,
				"_selectTargetSource": 2,
				"_selectTargetTiming": 0,
				}),
			),
			0,
		)
		.expect_err("missing damage type must fail closed");
		assert!(attack_error.contains("_damageType"));

		let static_buff_error = handle_static_buff_ability(
			&graph(
				STATIC_BUFF_ABILITY_SCRIPT_ID,
				static_ability(json!([buff(json!([{
				"attributeType": 1,
				"formulaItem": 1,
				"value": 1.0,
				"loadFromBlackboard": 2,
				"fetchBaseValueFromSourceEntity": 0,
				}]))])),
			),
			0,
		)
		.expect_err("non-binary modifier flag must fail closed");
		assert!(static_buff_error.contains("loadFromBlackboard"));

		let tile_error = handle_tile_listener(
			&graph(
				TILE_LISTENER_SCRIPT_ID,
				json!({
				"_targetValidator": reference(0),
				"_buffsWhenCasted": [],
				"_buffsWhenEnemyEnter": [],
				"_clearBuffWhenAbilityDetached": 0,
				"_clearBuffWhenTileListenerRemoved": 0,
				}),
			),
			0,
		)
		.expect_err("missing event list must fail closed");
		assert!(tile_error.contains("_buffsWhenEnemyLeave"));
	}

	#[test]
	fn static_buff_handler_rejects_unrecognized_ability_fields() {
		let mut fields: Value = static_ability(json!([]));
		fields
			.as_object_mut()
			.expect("fixture is an object")
			.insert("_removeBuffWhenAbilityDetached".to_owned(), json!(1));
		let error: String =
			handle_static_buff_ability(&graph(STATIC_BUFF_ABILITY_SCRIPT_ID, fields), 0)
				.expect_err("unrecognized detach semantics must fail closed");
		assert!(error.contains("_removeBuffWhenAbilityDetached"));
	}
}
