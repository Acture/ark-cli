use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer};

use crate::data::{
	enemy::EnemyKey,
	stage::StageDifficulty,
	stage_payload::{
		GridPosition, RouteCheckpointKind, StageAction, StagePayload, WaveActionOrigin,
	},
	ArkDataError,
};

#[derive(Debug, Deserialize)]
struct RawMechanicsLevel {
	#[serde(rename = "mapData")]
	map_data: RawMechanicsMapData,
	#[serde(default, deserialize_with = "null_default")]
	runes: Vec<RawRune>,
	#[serde(rename = "optionalRunes", default, deserialize_with = "null_default")]
	optional_runes: BTreeMap<String, Vec<RawRune>>,
	#[serde(rename = "globalBuffs", default, deserialize_with = "null_default")]
	global_buffs: Vec<RawGlobalBuff>,
	#[serde(default, deserialize_with = "null_default")]
	predefines: RawPredefines,
	#[serde(rename = "hardPredefines", default, deserialize_with = "null_default")]
	hard_predefines: RawPredefines,
}

#[derive(Debug, Deserialize)]
struct RawMechanicsMapData {
	map: Vec<Vec<i64>>,
	tiles: Vec<RawMechanicsTile>,
	#[serde(default, deserialize_with = "null_default")]
	effects: Vec<RawMapEffect>,
}

#[derive(Debug, Deserialize)]
struct RawMechanicsTile {
	#[serde(default, deserialize_with = "null_default")]
	effects: Vec<RawMapEffect>,
}

#[derive(Debug, Deserialize)]
struct RawMapEffect {
	key: String,
}

#[derive(Debug, Deserialize)]
struct RawRune {
	#[serde(rename = "difficultyMask")]
	difficulty_mask: RawMechanicValue,
	key: String,
	#[serde(default, deserialize_with = "null_default")]
	blackboard: Vec<RawRuneBlackboardEntry>,
}

#[derive(Debug, Deserialize)]
struct RawRuneBlackboardEntry {
	key: String,
	#[serde(rename = "valueStr", default)]
	value_string: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawGlobalBuff {
	#[serde(rename = "prefabKey")]
	prefab_key: String,
}

#[derive(Debug, Default, Deserialize)]
struct RawPredefines {
	#[serde(rename = "characterInsts", default, deserialize_with = "null_default")]
	character_insts: Vec<RawPredefinedEntry>,
	#[serde(rename = "tokenInsts", default, deserialize_with = "null_default")]
	token_insts: Vec<RawPredefinedEntry>,
	#[serde(rename = "characterCards", default, deserialize_with = "null_default")]
	character_cards: Vec<RawPredefinedEntry>,
	#[serde(rename = "tokenCards", default, deserialize_with = "null_default")]
	token_cards: Vec<RawPredefinedEntry>,
}

#[derive(Debug, Deserialize)]
struct RawPredefinedEntry {
	inst: RawPredefinedInst,
}

#[derive(Debug, Deserialize)]
struct RawPredefinedInst {
	#[serde(rename = "characterKey")]
	character_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawMechanicValue {
	Name(String),
	Code(i64),
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de> + Default,
{
	Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MechanicKind {
	Tile,
	MapEffect,
	RouteCheckpoint,
	Rune,
	GlobalBuffPrefab,
	EnemyPrefab,
	EnemySkillPrefab,
	Token,
	Predefined,
	WaveAction,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicIdentity {
	kind: MechanicKind,
	id: String,
}

impl MechanicIdentity {
	pub fn new(kind: MechanicKind, id: impl Into<String>) -> Result<Self, ArkDataError> {
		let id: String = id.into();
		if id.is_empty() {
			return Err(ArkDataError::InvalidMechanicIdentity {
				id,
				reason: "identity must not be empty".to_owned(),
			});
		}
		Ok(Self { kind, id })
	}

	pub fn kind(&self) -> MechanicKind {
		self.kind
	}

	pub fn id(&self) -> &str {
		&self.id
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PredefinedCollection {
	CharacterInsts,
	TokenInsts,
	CharacterCards,
	TokenCards,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MechanicOrigin {
	MapCell {
		position: GridPosition,
	},
	MapEffect {
		position: Option<GridPosition>,
		index: u32,
	},
	RouteCheckpoint {
		extra: bool,
		route: u32,
		checkpoint: u32,
	},
	Rune {
		optional_group: Option<String>,
		index: u32,
	},
	GlobalBuff {
		index: u32,
	},
	Enemy {
		enemy: EnemyKey,
	},
	EnemySkill {
		enemy: EnemyKey,
		index: u32,
	},
	Predefined {
		hard: bool,
		collection: PredefinedCollection,
		index: u32,
	},
	WaveAction(WaveActionOrigin),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicReference {
	identity: MechanicIdentity,
	origin: MechanicOrigin,
}

impl MechanicReference {
	pub fn identity(&self) -> &MechanicIdentity {
		&self.identity
	}

	pub fn origin(&self) -> &MechanicOrigin {
		&self.origin
	}
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicRequirements {
	references: Vec<MechanicReference>,
}

impl MechanicRequirements {
	pub fn references(&self) -> &[MechanicReference] {
		&self.references
	}

	pub fn support_report(&self, supported: &BTreeSet<MechanicIdentity>) -> MechanicSupportReport {
		let unsupported: Vec<MechanicReference> = self
			.references
			.iter()
			.filter(|reference: &&MechanicReference| !supported.contains(reference.identity()))
			.cloned()
			.collect();
		MechanicSupportReport { unsupported }
	}
}

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MechanicSupportReport {
	unsupported: Vec<MechanicReference>,
}

impl MechanicSupportReport {
	pub fn is_complete(&self) -> bool {
		self.unsupported.is_empty()
	}

	pub fn unsupported(&self) -> &[MechanicReference] {
		&self.unsupported
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) enum EncodedMechanic {
	Name(String),
	Code(i64),
}

impl From<RawMechanicValue> for EncodedMechanic {
	fn from(value: RawMechanicValue) -> Self {
		match value {
			RawMechanicValue::Name(value) => Self::Name(value),
			RawMechanicValue::Code(value) => Self::Code(value),
		}
	}
}

impl From<&RawMechanicValue> for EncodedMechanic {
	fn from(value: &RawMechanicValue) -> Self {
		match value {
			RawMechanicValue::Name(value) => Self::Name(value.clone()),
			RawMechanicValue::Code(value) => Self::Code(*value),
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct MapCellMechanics {
	pub(in crate::data) position: GridPosition,
	pub(in crate::data) tile_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct MapEffectMechanics {
	pub(in crate::data) position: Option<GridPosition>,
	pub(in crate::data) index: u32,
	pub(in crate::data) key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct RuneMechanics {
	pub(in crate::data) optional_group: Option<String>,
	pub(in crate::data) index: u32,
	pub(in crate::data) difficulty_mask: EncodedMechanic,
	pub(in crate::data) key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct GlobalBuffMechanics {
	pub(in crate::data) index: u32,
	pub(in crate::data) prefab_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct IndexedMechanic {
	pub(in crate::data) index: u32,
	pub(in crate::data) id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct EnemyMechanics {
	pub(in crate::data) enemy: EnemyKey,
	pub(in crate::data) prefab_key: Option<String>,
	pub(in crate::data) skill_prefabs: Vec<IndexedMechanic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::data) struct PredefinedMechanics {
	pub(in crate::data) hard: bool,
	pub(in crate::data) collection: PredefinedCollection,
	pub(in crate::data) index: u32,
	pub(in crate::data) character_key: String,
}

pub(in crate::data) struct CompiledStageMechanics {
	pub(in crate::data) requirements: MechanicRequirements,
	pub(in crate::data) active_hidden_groups: BTreeSet<String>,
}

#[derive(Debug)]
pub(in crate::data) struct MechanicRequirementsBuilder {
	difficulty: StageDifficulty,
	references: Vec<MechanicReference>,
}

impl MechanicRequirementsBuilder {
	pub(in crate::data) fn new(difficulty: StageDifficulty) -> Self {
		Self {
			difficulty,
			references: Vec::new(),
		}
	}

	pub(in crate::data) fn add_map_cell(
		&mut self,
		input: MapCellMechanics,
	) -> Result<(), ArkDataError> {
		self.push(
			MechanicKind::Tile,
			input.tile_key,
			MechanicOrigin::MapCell {
				position: input.position,
			},
		)
	}

	pub(in crate::data) fn add_map_effect(
		&mut self,
		input: MapEffectMechanics,
	) -> Result<(), ArkDataError> {
		self.push(
			MechanicKind::MapEffect,
			input.key,
			MechanicOrigin::MapEffect {
				position: input.position,
				index: input.index,
			},
		)
	}

	pub(in crate::data) fn add_rune(&mut self, input: RuneMechanics) -> Result<(), ArkDataError> {
		let difficulty_bits: u32 = rune_difficulty_bits(&input.difficulty_mask)?;
		let identity: MechanicIdentity = MechanicIdentity::new(MechanicKind::Rune, input.key)?;
		if difficulty_bits & selected_difficulty_bit(self.difficulty) == 0 {
			return Ok(());
		}
		self.references.push(MechanicReference {
			identity,
			origin: MechanicOrigin::Rune {
				optional_group: input.optional_group,
				index: input.index,
			},
		});
		Ok(())
	}

	pub(in crate::data) fn add_global_buff(
		&mut self,
		input: GlobalBuffMechanics,
	) -> Result<(), ArkDataError> {
		self.push(
			MechanicKind::GlobalBuffPrefab,
			input.prefab_key,
			MechanicOrigin::GlobalBuff { index: input.index },
		)
	}

	pub(in crate::data) fn add_enemy(&mut self, input: EnemyMechanics) -> Result<(), ArkDataError> {
		if let Some(prefab_key) = input.prefab_key {
			self.push(
				MechanicKind::EnemyPrefab,
				prefab_key,
				MechanicOrigin::Enemy {
					enemy: input.enemy.clone(),
				},
			)?;
		}
		for skill in input.skill_prefabs {
			self.push(
				MechanicKind::EnemySkillPrefab,
				skill.id,
				MechanicOrigin::EnemySkill {
					enemy: input.enemy.clone(),
					index: skill.index,
				},
			)?;
		}
		Ok(())
	}

	pub(in crate::data) fn add_predefined(
		&mut self,
		input: PredefinedMechanics,
	) -> Result<(), ArkDataError> {
		let kind: MechanicKind = match input.collection {
			PredefinedCollection::CharacterInsts | PredefinedCollection::CharacterCards => {
				MechanicKind::Predefined
			}
			PredefinedCollection::TokenInsts | PredefinedCollection::TokenCards => {
				MechanicKind::Token
			}
		};
		self.push(
			kind,
			input.character_key,
			MechanicOrigin::Predefined {
				hard: input.hard,
				collection: input.collection,
				index: input.index,
			},
		)
	}

	pub(in crate::data) fn add_stage_action(
		&mut self,
		action: &StageAction,
	) -> Result<(), ArkDataError> {
		if !action.kind().is_spawn() {
			return self.push(
				MechanicKind::WaveAction,
				action.kind().stable_id(),
				MechanicOrigin::WaveAction(action.origin().clone()),
			);
		}
		if !action.key().is_empty() {
			return Ok(());
		}
		self.add_random_spawn(
			action.origin().clone(),
			action.random_spawn_group().map(str::to_owned),
			action.random_spawn_pack().map(str::to_owned),
			action.hidden_group().map(str::to_owned),
		)
	}

	fn add_random_spawn(
		&mut self,
		origin: WaveActionOrigin,
		random_spawn_group_key: Option<String>,
		random_spawn_group_pack_key: Option<String>,
		hidden_group: Option<String>,
	) -> Result<(), ArkDataError> {
		let random_ids: [(Option<String>, &str); 3] = [
			(random_spawn_group_key, "SPAWN_RANDOM_GROUP:"),
			(random_spawn_group_pack_key, "SPAWN_RANDOM_PACK:"),
			(hidden_group, "SPAWN_HIDDEN_GROUP:"),
		];
		let mut found_random_identity: bool = false;
		for (value, prefix) in random_ids {
			let Some(value): Option<String> = value.filter(|value: &String| !value.is_empty())
			else {
				continue;
			};
			found_random_identity = true;
			self.push(
				MechanicKind::WaveAction,
				format!("{prefix}{value}"),
				MechanicOrigin::WaveAction(origin.clone()),
			)?;
		}
		if !found_random_identity {
			return Err(ArkDataError::InvalidRandomSpawn {
				origin,
				reason: "empty-key SPAWN requires random group, pack, or hidden-group metadata"
					.to_owned(),
			});
		}
		Ok(())
	}

	pub(in crate::data) fn finish(mut self) -> MechanicRequirements {
		self.references.sort();
		self.references.dedup();
		MechanicRequirements {
			references: self.references,
		}
	}

	fn push(
		&mut self,
		kind: MechanicKind,
		id: impl Into<String>,
		origin: MechanicOrigin,
	) -> Result<(), ArkDataError> {
		let identity: MechanicIdentity = MechanicIdentity::new(kind, id)?;
		self.references.push(MechanicReference { identity, origin });
		Ok(())
	}
}

pub(in crate::data) fn compile_requirements(
	bytes: &[u8],
	destination_path: &str,
	difficulty: StageDifficulty,
	payload: &StagePayload,
	enemies: impl IntoIterator<Item = EnemyMechanics>,
) -> Result<CompiledStageMechanics, ArkDataError> {
	let raw: RawMechanicsLevel =
		serde_json::from_slice(bytes).map_err(|source| ArkDataError::LevelParse {
			destination_path: destination_path.to_owned(),
			source,
		})?;
	let RawMechanicsLevel {
		map_data,
		runes,
		optional_runes,
		global_buffs,
		predefines,
		hard_predefines,
	} = raw;
	let mut builder: MechanicRequirementsBuilder = MechanicRequirementsBuilder::new(difficulty);
	let active_hidden_groups: BTreeSet<String> = compile_active_hidden_groups(&runes, difficulty)?;

	add_map_requirements(&mut builder, &map_data, payload, destination_path)?;
	add_route_requirements(&mut builder, payload, destination_path)?;
	add_rune_requirements(&mut builder, runes, None, "runes", destination_path)?;
	for (group, runes) in optional_runes {
		add_rune_requirements(
			&mut builder,
			runes,
			Some(group),
			"optionalRunes",
			destination_path,
		)?;
	}
	for (index, buff) in global_buffs.into_iter().enumerate() {
		builder.add_global_buff(GlobalBuffMechanics {
			index: checked_mechanic_index(destination_path, "globalBuffs", index)?,
			prefab_key: buff.prefab_key,
		})?;
	}
	for enemy in enemies {
		builder.add_enemy(enemy)?;
	}
	add_predefined_requirements(&mut builder, predefines, false, destination_path)?;
	add_predefined_requirements(&mut builder, hard_predefines, true, destination_path)?;
	for action in payload.actions() {
		builder.add_stage_action(action)?;
	}

	Ok(CompiledStageMechanics {
		requirements: builder.finish(),
		active_hidden_groups,
	})
}

fn compile_active_hidden_groups(
	runes: &[RawRune],
	difficulty: StageDifficulty,
) -> Result<BTreeSet<String>, ArkDataError> {
	let mut groups: BTreeSet<String> = BTreeSet::new();
	for rune in runes {
		let difficulty_bits: u32 =
			rune_difficulty_bits(&EncodedMechanic::from(&rune.difficulty_mask))?;
		if rune.key != "level_hidden_group_enable"
			|| difficulty_bits & selected_difficulty_bit(difficulty) == 0
		{
			continue;
		}
		for entry in &rune.blackboard {
			if entry.key != "key" {
				continue;
			}
			let group: &str = entry
				.value_string
				.as_deref()
				.filter(|value| !value.trim().is_empty())
				.ok_or_else(|| ArkDataError::InvalidMechanicIdentity {
					id: rune.key.clone(),
					reason: "hidden-group rune has no nonblank key value".to_owned(),
				})?;
			groups.insert(group.to_owned());
		}
	}
	Ok(groups)
}

fn add_map_requirements(
	builder: &mut MechanicRequirementsBuilder,
	raw: &RawMechanicsMapData,
	payload: &StagePayload,
	destination_path: &str,
) -> Result<(), ArkDataError> {
	let dimensions = payload.map_dimensions();
	let width: usize = dimensions.width as usize;
	let height: usize = dimensions.height as usize;
	if raw.map.len() != height || raw.map.iter().any(|row: &Vec<i64>| row.len() != width) {
		return Err(ArkDataError::InvalidMap {
			destination_path: destination_path.to_owned(),
			reason: "mechanic map matrix does not match the compiled map dimensions".to_owned(),
		});
	}
	let expected_cells: usize =
		width
			.checked_mul(height)
			.ok_or_else(|| ArkDataError::InvalidMap {
				destination_path: destination_path.to_owned(),
				reason: "compiled map cell count overflows usize".to_owned(),
			})?;
	if payload.cells().len() != expected_cells {
		return Err(ArkDataError::InvalidMap {
			destination_path: destination_path.to_owned(),
			reason: "compiled map cell count does not match its dimensions".to_owned(),
		});
	}

	for cell in payload.cells() {
		let position: GridPosition = cell.position();
		builder.add_map_cell(MapCellMechanics {
			position,
			tile_key: cell.tile_key().to_owned(),
		})?;
		let logical_row: usize = position.row as usize;
		let col: usize = position.col as usize;
		let source_row: usize =
			height
				.checked_sub(logical_row + 1)
				.ok_or_else(|| ArkDataError::InvalidMap {
					destination_path: destination_path.to_owned(),
					reason: format!("compiled map cell {position:?} is outside its dimensions"),
				})?;
		let tile_index: i64 = *raw
			.map
			.get(source_row)
			.and_then(|row: &Vec<i64>| row.get(col))
			.ok_or_else(|| ArkDataError::InvalidMap {
				destination_path: destination_path.to_owned(),
				reason: format!("compiled map cell {position:?} is missing from the raw matrix"),
			})?;
		let palette_index: usize =
			usize::try_from(tile_index).map_err(|_| ArkDataError::InvalidMap {
				destination_path: destination_path.to_owned(),
				reason: format!("map cell {position:?} has negative palette index {tile_index}"),
			})?;
		let tile: &RawMechanicsTile =
			raw.tiles
				.get(palette_index)
				.ok_or_else(|| ArkDataError::InvalidMap {
					destination_path: destination_path.to_owned(),
					reason: format!(
						"map cell {position:?} uses palette index {tile_index}, but palette has {} entries",
						raw.tiles.len()
					),
				})?;
		for (index, effect) in tile.effects.iter().enumerate() {
			builder.add_map_effect(MapEffectMechanics {
				position: Some(position),
				index: checked_mechanic_index(destination_path, "tile effects", index)?,
				key: effect.key.clone(),
			})?;
		}
	}
	for (index, effect) in raw.effects.iter().enumerate() {
		builder.add_map_effect(MapEffectMechanics {
			position: None,
			index: checked_mechanic_index(destination_path, "map effects", index)?,
			key: effect.key.clone(),
		})?;
	}
	Ok(())
}

fn add_route_requirements(
	builder: &mut MechanicRequirementsBuilder,
	payload: &StagePayload,
	destination_path: &str,
) -> Result<(), ArkDataError> {
	for (extra, routes) in [(false, payload.routes()), (true, payload.extra_routes())] {
		for (route, route_value) in routes.iter().enumerate() {
			let route: u32 = checked_mechanic_index(
				destination_path,
				if extra { "extraRoutes" } else { "routes" },
				route,
			)?;
			for (checkpoint, checkpoint_value) in route_value.checkpoints().iter().enumerate() {
				if matches!(checkpoint_value.kind(), RouteCheckpointKind::Move) {
					continue;
				}
				builder.push(
					MechanicKind::RouteCheckpoint,
					checkpoint_value.kind().mechanic_id(),
					MechanicOrigin::RouteCheckpoint {
						extra,
						route,
						checkpoint: checked_mechanic_index(
							destination_path,
							"route checkpoints",
							checkpoint,
						)?,
					},
				)?;
			}
		}
	}
	Ok(())
}

fn add_rune_requirements(
	builder: &mut MechanicRequirementsBuilder,
	runes: Vec<RawRune>,
	optional_group: Option<String>,
	field: &'static str,
	destination_path: &str,
) -> Result<(), ArkDataError> {
	for (index, rune) in runes.into_iter().enumerate() {
		builder.add_rune(RuneMechanics {
			optional_group: optional_group.clone(),
			index: checked_mechanic_index(destination_path, field, index)?,
			difficulty_mask: rune.difficulty_mask.into(),
			key: rune.key,
		})?;
	}
	Ok(())
}

fn add_predefined_requirements(
	builder: &mut MechanicRequirementsBuilder,
	raw: RawPredefines,
	hard: bool,
	destination_path: &str,
) -> Result<(), ArkDataError> {
	let RawPredefines {
		character_insts,
		token_insts,
		character_cards,
		token_cards,
	} = raw;
	for (collection, entries) in [
		(PredefinedCollection::CharacterInsts, character_insts),
		(PredefinedCollection::TokenInsts, token_insts),
		(PredefinedCollection::CharacterCards, character_cards),
		(PredefinedCollection::TokenCards, token_cards),
	] {
		let field: &'static str = predefined_collection_field(hard, collection);
		for (index, entry) in entries.into_iter().enumerate() {
			builder.add_predefined(PredefinedMechanics {
				hard,
				collection,
				index: checked_mechanic_index(destination_path, field, index)?,
				character_key: entry.inst.character_key,
			})?;
		}
	}
	Ok(())
}

fn predefined_collection_field(hard: bool, collection: PredefinedCollection) -> &'static str {
	match (hard, collection) {
		(false, PredefinedCollection::CharacterInsts) => "predefines.characterInsts",
		(false, PredefinedCollection::TokenInsts) => "predefines.tokenInsts",
		(false, PredefinedCollection::CharacterCards) => "predefines.characterCards",
		(false, PredefinedCollection::TokenCards) => "predefines.tokenCards",
		(true, PredefinedCollection::CharacterInsts) => "hardPredefines.characterInsts",
		(true, PredefinedCollection::TokenInsts) => "hardPredefines.tokenInsts",
		(true, PredefinedCollection::CharacterCards) => "hardPredefines.characterCards",
		(true, PredefinedCollection::TokenCards) => "hardPredefines.tokenCards",
	}
}

fn checked_mechanic_index(
	destination_path: &str,
	field: &'static str,
	index: usize,
) -> Result<u32, ArkDataError> {
	u32::try_from(index).map_err(|_| ArkDataError::InvalidLevelCollectionLength {
		destination_path: destination_path.to_owned(),
		field,
		length: index.saturating_add(1),
	})
}

fn selected_difficulty_bit(difficulty: StageDifficulty) -> u32 {
	match difficulty {
		StageDifficulty::Normal => 1,
		StageDifficulty::Raid => 2,
	}
}

fn rune_difficulty_bits(value: &EncodedMechanic) -> Result<u32, ArkDataError> {
	match value {
		EncodedMechanic::Name(name) => match name.as_str() {
			"NONE" => Ok(0),
			"NORMAL" => Ok(1),
			"FOUR_STAR" => Ok(2),
			"EASY" => Ok(4),
			"SIX_STAR" => Ok(8),
			"ALL" => Ok(15),
			_ => Err(ArkDataError::InvalidRuneDifficultyMask {
				mask: name.clone(),
				reason: "unknown named difficulty mask".to_owned(),
			}),
		},
		EncodedMechanic::Code(code) => {
			u32::try_from(*code).map_err(|_| ArkDataError::InvalidRuneDifficultyMask {
				mask: code.to_string(),
				reason: "numeric difficulty mask must fit a nonnegative u32".to_owned(),
			})
		}
	}
}
