use std::{
	collections::{BTreeMap, BTreeSet},
	fmt,
};

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::data::{manifest::VerifiedEntry, ArkDataError};

const ENEMY_DATABASE_PATH: &str = "enemy_database.json";
const ENEMY_HANDBOOK_PATH: &str = "enemy_handbook_table.json";

/// One damage family the handbook attributes to an enemy record.
///
/// The handbook aggregates every source a record can deal damage through, so a record with a skill
/// can legitimately declare more than one kind. Consumers that need a single normal-attack damage
/// type must reject anything but an exact one-kind set themselves.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EnemyDamageKind {
	Physical,
	Arts,
	NoDamage,
	Healing,
}

impl EnemyDamageKind {
	fn parse(raw: &str) -> Option<Self> {
		match raw {
			"PHYSIC" => Some(Self::Physical),
			"MAGIC" => Some(Self::Arts),
			"NO_DAMAGE" => Some(Self::NoDamage),
			"HEAL" => Some(Self::Healing),
			_ => None,
		}
	}

	#[must_use]
	pub const fn stable_id(self) -> &'static str {
		match self {
			Self::Physical => "PHYSIC",
			Self::Arts => "MAGIC",
			Self::NoDamage => "NO_DAMAGE",
			Self::Healing => "HEAL",
		}
	}
}

/// The revision-pinned handbook row for one enemy record.
///
/// The handbook is the only pinned table that describes what a record *does* rather than what its
/// panel is. Both fields are retained verbatim: an unrecognised damage family stays readable as an
/// opaque string so drift invalidates only the records whose evidence changed, and the declared
/// abilities are counted so a consumer can tell a record with no described behaviour from one whose
/// behaviour is described but not modelled.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EnemyHandbookFacts {
	damage_kinds: BTreeSet<EnemyDamageKind>,
	unreadable_damage_families: BTreeSet<String>,
	declared_ability_count: usize,
}

impl EnemyHandbookFacts {
	/// Damage families the handbook attributes to this record, in stable order.
	#[must_use]
	pub const fn damage_kinds(&self) -> &BTreeSet<EnemyDamageKind> {
		&self.damage_kinds
	}

	/// Declared damage families this revision of the data decoder cannot name.
	#[must_use]
	pub const fn unreadable_damage_families(&self) -> &BTreeSet<String> {
		&self.unreadable_damage_families
	}

	/// How many distinct behaviours the handbook describes for this record.
	#[must_use]
	pub const fn declared_ability_count(&self) -> usize {
		self.declared_ability_count
	}
}

#[derive(Debug, Deserialize)]
struct RawEnemyHandbook {
	#[serde(rename = "enemyData", default)]
	enemy_data: BTreeMap<String, RawEnemyHandbookRecord>,
}

#[derive(Debug, Deserialize)]
struct RawEnemyHandbookRecord {
	#[serde(rename = "damageType", default)]
	damage_type: Option<Vec<String>>,
	#[serde(rename = "abilityList", default)]
	ability_list: Option<Vec<Value>>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemyKey(String);

impl EnemyKey {
	pub fn new(value: String) -> Result<Self, ArkDataError> {
		if value.is_empty() {
			return Err(ArkDataError::InvalidEnemyIdentity {
				enemy_id: value,
				reason: "identity must not be empty".to_owned(),
			});
		}
		Ok(Self(value))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl fmt::Display for EnemyKey {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

#[derive(Clone, Debug, Default, PartialEq, PartialOrd)]
pub struct EnemyStats {
	max_hp: Option<i32>,
	attack: Option<i32>,
	defense: Option<i32>,
	block_count: Option<i32>,
	magic_resistance: Option<f64>,
	move_speed: Option<f64>,
	attack_speed: Option<f64>,
	base_attack_time: Option<f64>,
	mass_level: Option<i32>,
	life_point_reduce: Option<i32>,
	range_radius: Option<f64>,
	not_count_in_total: Option<bool>,
}

impl EnemyStats {
	pub fn max_hp(&self) -> Option<i32> {
		self.max_hp
	}

	pub fn attack(&self) -> Option<i32> {
		self.attack
	}

	pub fn defense(&self) -> Option<i32> {
		self.defense
	}

	pub fn block_count(&self) -> Option<i32> {
		self.block_count
	}

	pub fn magic_resistance(&self) -> Option<f64> {
		self.magic_resistance
	}

	pub fn move_speed(&self) -> Option<f64> {
		self.move_speed
	}

	pub fn attack_speed(&self) -> Option<f64> {
		self.attack_speed
	}

	pub fn base_attack_time(&self) -> Option<f64> {
		self.base_attack_time
	}

	pub fn mass_level(&self) -> Option<i32> {
		self.mass_level
	}

	pub fn life_point_reduce(&self) -> Option<i32> {
		self.life_point_reduce
	}

	pub fn range_radius(&self) -> Option<f64> {
		self.range_radius
	}

	pub fn not_count_in_total(&self) -> Option<bool> {
		self.not_count_in_total
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct EnemySkill {
	prefab_key: String,
	priority: i32,
	cooldown: f64,
	initial_cooldown: f64,
	sp_cost: i32,
	blackboard: Vec<EnemyBlackboardEntry>,
}

impl EnemySkill {
	pub fn prefab_key(&self) -> &str {
		&self.prefab_key
	}

	pub fn priority(&self) -> i32 {
		self.priority
	}

	pub fn cooldown_seconds(&self) -> f64 {
		self.cooldown
	}

	pub fn initial_cooldown_seconds(&self) -> f64 {
		self.initial_cooldown
	}

	pub fn sp_cost(&self) -> i32 {
		self.sp_cost
	}

	pub fn blackboard(&self) -> &[EnemyBlackboardEntry] {
		&self.blackboard
	}
}

/// One typed parameter retained from an enemy skill or talent blackboard.
#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct EnemyBlackboardEntry {
	key: String,
	value: f64,
	value_string: Option<String>,
}

impl EnemyBlackboardEntry {
	pub fn key(&self) -> &str {
		&self.key
	}

	pub fn value(&self) -> f64 {
		self.value
	}

	pub fn value_string(&self) -> Option<&str> {
		self.value_string.as_deref()
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledEnemy {
	key: EnemyKey,
	database_level: Option<u32>,
	prefab_key: Option<String>,
	stats: EnemyStats,
	skills: Vec<EnemySkill>,
	_name: Option<String>,
	_description: Option<String>,
	apply_way: Option<RawEnumValue>,
	_motion: Option<RawEnumValue>,
	_level_type: Option<RawEnumValue>,
	_enemy_tags: Option<Vec<String>>,
	_talent_blackboard: Option<Value>,
	_sp_data: Option<Value>,
	handbook: Option<EnemyHandbookFacts>,
}

impl CompiledEnemy {
	pub fn key(&self) -> &EnemyKey {
		&self.key
	}

	pub fn database_level(&self) -> Option<u32> {
		self.database_level
	}

	pub fn prefab_key(&self) -> Option<&str> {
		self.prefab_key.as_deref()
	}

	pub fn stats(&self) -> &EnemyStats {
		&self.stats
	}

	pub fn skills(&self) -> &[EnemySkill] {
		&self.skills
	}

	/// Table attack delivery class after stage overrides.
	///
	/// `None` when the value is undefined or numerically encoded: a caller that needs the class must
	/// fail closed rather than guess a name for a code.
	pub fn apply_way_name(&self) -> Option<&str> {
		match &self.apply_way {
			Some(RawEnumValue::Name(name)) => Some(name),
			Some(RawEnumValue::Code(_)) | None => None,
		}
	}

	/// Raw talent blackboard retained from the revision-pinned enemy record.
	pub fn talent_blackboard(&self) -> Option<&Value> {
		self._talent_blackboard.as_ref()
	}

	/// Raw SP metadata retained from the revision-pinned enemy record.
	pub fn sp_data(&self) -> Option<&Value> {
		self._sp_data.as_ref()
	}

	/// The pinned handbook row for this record.
	///
	/// `None` means the handbook has no row for the key, which is not the same as a row declaring
	/// no damage or no ability. Callers must distinguish the two rather than defaulting either.
	pub const fn handbook(&self) -> Option<&EnemyHandbookFacts> {
		self.handbook.as_ref()
	}
}

#[derive(Debug)]
pub(in crate::data) struct EnemyCatalog {
	levels: BTreeMap<EnemyKey, BTreeMap<u32, RawEnemyPatch>>,
	handbook: BTreeMap<EnemyKey, EnemyHandbookFacts>,
}

impl EnemyCatalog {
	pub(in crate::data) fn from_entries(
		entries: &BTreeMap<String, VerifiedEntry>,
	) -> Result<Self, ArkDataError> {
		let entry: &VerifiedEntry =
			entries
				.get(ENEMY_DATABASE_PATH)
				.ok_or(ArkDataError::MissingRequiredRoot {
					destination_path: ENEMY_DATABASE_PATH,
				})?;
		let bytes: &[u8] =
			entry
				.retained_bytes
				.as_deref()
				.ok_or_else(|| ArkDataError::InvalidManifest {
					field: "entries",
					reason: format!(
						"verified root {ENEMY_DATABASE_PATH:?} did not retain its bytes"
					),
				})?;
		let document: RawEnemyDatabase =
			serde_json::from_slice(bytes).map_err(|source| ArkDataError::RootParse {
				destination_path: entry.destination_path.clone(),
				source,
			})?;
		let handbook_entry: &VerifiedEntry =
			entries
				.get(ENEMY_HANDBOOK_PATH)
				.ok_or(ArkDataError::MissingRequiredRoot {
					destination_path: ENEMY_HANDBOOK_PATH,
				})?;
		let handbook_bytes: &[u8] = handbook_entry.retained_bytes.as_deref().ok_or_else(|| {
			ArkDataError::InvalidManifest {
				field: "entries",
				reason: format!("verified root {ENEMY_HANDBOOK_PATH:?} did not retain its bytes"),
			}
		})?;
		let handbook: RawEnemyHandbook =
			serde_json::from_slice(handbook_bytes).map_err(|source| ArkDataError::RootParse {
				destination_path: handbook_entry.destination_path.clone(),
				source,
			})?;
		Self::from_documents(document, handbook)
	}

	fn from_documents(
		document: RawEnemyDatabase,
		handbook: RawEnemyHandbook,
	) -> Result<Self, ArkDataError> {
		let mut rows: BTreeMap<EnemyKey, EnemyHandbookFacts> = BTreeMap::new();
		for (raw_key, record) in handbook.enemy_data {
			let key: EnemyKey = EnemyKey::new(raw_key)?;
			let mut facts: EnemyHandbookFacts = EnemyHandbookFacts {
				damage_kinds: BTreeSet::new(),
				unreadable_damage_families: BTreeSet::new(),
				declared_ability_count: record.ability_list.map_or(0, |abilities| abilities.len()),
			};
			for raw_kind in record.damage_type.unwrap_or_default() {
				// Retain an unnameable family instead of failing the whole snapshot: drift must
				// invalidate only the records whose evidence changed.
				match EnemyDamageKind::parse(&raw_kind) {
					Some(kind) => {
						facts.damage_kinds.insert(kind);
					}
					None => {
						facts.unreadable_damage_families.insert(raw_kind);
					}
				}
			}
			rows.insert(key, facts);
		}
		Self::from_document(document, rows)
	}

	fn from_document(
		document: RawEnemyDatabase,
		handbook: BTreeMap<EnemyKey, EnemyHandbookFacts>,
	) -> Result<Self, ArkDataError> {
		let mut levels: BTreeMap<EnemyKey, BTreeMap<u32, RawEnemyPatch>> = BTreeMap::new();
		for raw_entry in document.enemies {
			let key: EnemyKey = EnemyKey::new(raw_entry.key)?;
			if levels.contains_key(&key) {
				return Err(ArkDataError::DuplicateEnemyId { enemy_id: key.0 });
			}
			let mut enemy_levels: BTreeMap<u32, RawEnemyPatch> = BTreeMap::new();
			for raw_level in raw_entry.levels {
				let level: u32 = checked_u32(
					raw_level.level,
					&key,
					"level",
					"database level must be a nonnegative u32",
				)?;
				if enemy_levels.insert(level, raw_level.enemy_data).is_some() {
					return Err(ArkDataError::DuplicateEnemyLevel {
						enemy_id: key.0,
						level,
					});
				}
			}
			levels.insert(key, enemy_levels);
		}
		Ok(Self { levels, handbook })
	}

	pub(in crate::data) fn compile_level(
		&self,
		bytes: &[u8],
		destination_path: &str,
	) -> Result<BTreeMap<EnemyKey, CompiledEnemy>, ArkDataError> {
		let document: RawLevelEnemies =
			serde_json::from_slice(bytes).map_err(|source| ArkDataError::LevelParse {
				destination_path: destination_path.to_owned(),
				source,
			})?;
		let mut prepared_refs: Vec<PreparedEnemyRef> =
			Vec::with_capacity(document.enemy_db_refs.len());
		let mut seen_refs: BTreeMap<EnemyKey, u32> = BTreeMap::new();
		for raw in document.enemy_db_refs {
			let key: EnemyKey = EnemyKey::new(raw.id)?;
			let level: u32 = checked_u32(
				raw.level,
				&key,
				"enemyDbRefs[].level",
				"stage enemy level must be a nonnegative u32",
			)?;
			if seen_refs.insert(key.clone(), level).is_some() {
				return Err(ArkDataError::DuplicateEnemyReference {
					enemy_id: key.0,
					level,
				});
			}
			prepared_refs.push(PreparedEnemyRef {
				use_db: raw.use_db,
				key,
				level,
				overwritten_data: raw.overwritten_data,
			});
		}

		let mut compiled: BTreeMap<EnemyKey, CompiledEnemy> = BTreeMap::new();
		for reference in prepared_refs {
			let enemy: CompiledEnemy = if reference.use_db {
				self.compile_database_enemy(
					reference.key.clone(),
					reference.level,
					reference.overwritten_data.as_ref(),
				)?
			} else {
				let patch: &RawEnemyPatch =
					reference.overwritten_data.as_ref().ok_or_else(|| {
						ArkDataError::MissingLocalEnemyData {
							enemy_id: reference.key.0.clone(),
						}
					})?;
				let mut state: EnemyState = EnemyState::default();
				state.apply_patch(patch, &reference.key, "stage-local enemy data")?;
				state.finish(reference.key.clone(), None)
			};
			compiled.insert(reference.key, enemy);
		}

		for (key, enemy) in &mut compiled {
			enemy.handbook = self.handbook.get(key).cloned();
		}

		for raw_local in document.enemies {
			let key: EnemyKey = EnemyKey::new(raw_local.key.clone())?;
			if compiled.contains_key(&key) {
				return Err(ArkDataError::DuplicateEnemyId { enemy_id: key.0 });
			}
			let mut enemy: CompiledEnemy = compile_top_level_enemy(raw_local, key.clone())?;
			enemy.handbook = self.handbook.get(&key).cloned();
			if compiled.insert(key.clone(), enemy).is_some() {
				return Err(ArkDataError::DuplicateEnemyId { enemy_id: key.0 });
			}
		}
		Ok(compiled)
	}

	fn compile_database_enemy(
		&self,
		key: EnemyKey,
		requested_level: u32,
		overwrite: Option<&RawEnemyPatch>,
	) -> Result<CompiledEnemy, ArkDataError> {
		let levels: &BTreeMap<u32, RawEnemyPatch> =
			self.levels
				.get(&key)
				.ok_or_else(|| ArkDataError::MissingEnemyId {
					enemy_id: key.0.clone(),
				})?;
		if !levels.contains_key(&requested_level) {
			return Err(ArkDataError::MissingEnemyLevel {
				enemy_id: key.0.clone(),
				requested_level,
				available_levels: levels.keys().copied().collect(),
			});
		}

		let mut state: EnemyState = EnemyState::default();
		for (level, patch) in levels.range(..=requested_level) {
			state.apply_patch(patch, &key, &format!("database level {level}"))?;
		}
		if let Some(overwrite) = overwrite {
			state.apply_patch(overwrite, &key, "stage override")?;
		}
		Ok(state.finish(key, Some(requested_level)))
	}
}

#[derive(Debug)]
struct PreparedEnemyRef {
	use_db: bool,
	key: EnemyKey,
	level: u32,
	overwritten_data: Option<RawEnemyPatch>,
}

#[derive(Debug, Deserialize)]
struct RawEnemyDatabase {
	#[serde(default, deserialize_with = "null_default")]
	enemies: Vec<RawEnemyEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEnemyEntry {
	#[serde(rename = "Key")]
	key: String,
	#[serde(rename = "Value", default, deserialize_with = "null_default")]
	levels: Vec<RawEnemyLevel>,
}

#[derive(Debug, Deserialize)]
struct RawEnemyLevel {
	level: i64,
	#[serde(rename = "enemyData")]
	enemy_data: RawEnemyPatch,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RawEnemyPatch {
	#[serde(default)]
	name: Option<RawDefined<String>>,
	#[serde(default)]
	description: Option<RawDefined<String>>,
	#[serde(rename = "prefabKey", default)]
	prefab_key: Option<RawDefined<String>>,
	#[serde(default)]
	attributes: RawEnemyAttributes,
	#[serde(rename = "applyWay", default)]
	apply_way: Option<RawDefined<RawEnumValue>>,
	#[serde(default)]
	motion: Option<RawDefined<RawEnumValue>>,
	#[serde(rename = "enemyTags", default)]
	enemy_tags: Option<RawDefined<Vec<String>>>,
	#[serde(rename = "lifePointReduce", default)]
	life_point_reduce: Option<RawDefined<i64>>,
	#[serde(rename = "levelType", default)]
	level_type: Option<RawDefined<RawEnumValue>>,
	#[serde(rename = "rangeRadius", default)]
	range_radius: Option<RawDefined<f64>>,
	#[serde(rename = "notCountInTotal", default)]
	not_count_in_total: Option<RawDefined<bool>>,
	#[serde(default)]
	skills: Option<Vec<RawEnemySkill>>,
	#[serde(rename = "talentBlackboard", default)]
	talent_blackboard: Option<Value>,
	#[serde(rename = "spData", default)]
	sp_data: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RawEnemyAttributes {
	#[serde(rename = "maxHp", default)]
	max_hp: Option<RawDefined<i64>>,
	#[serde(rename = "atk", default)]
	attack: Option<RawDefined<i64>>,
	#[serde(rename = "def", default)]
	defense: Option<RawDefined<i64>>,
	#[serde(rename = "blockCnt", default)]
	block_count: Option<RawDefined<i64>>,
	#[serde(rename = "magicResistance", default)]
	magic_resistance: Option<RawDefined<f64>>,
	#[serde(rename = "moveSpeed", default)]
	move_speed: Option<RawDefined<f64>>,
	#[serde(rename = "attackSpeed", default)]
	attack_speed: Option<RawDefined<f64>>,
	#[serde(rename = "baseAttackTime", default)]
	base_attack_time: Option<RawDefined<f64>>,
	#[serde(rename = "massLevel", default)]
	mass_level: Option<RawDefined<i64>>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawDefined<T> {
	m_defined: bool,
	m_value: Option<T>,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Deserialize)]
#[serde(untagged)]
enum RawEnumValue {
	Name(String),
	Code(i64),
}

#[derive(Clone, Debug, Deserialize)]
struct RawEnemySkill {
	#[serde(rename = "prefabKey")]
	prefab_key: String,
	priority: i64,
	cooldown: f64,
	#[serde(rename = "initCooldown")]
	init_cooldown: f64,
	#[serde(rename = "spCost")]
	sp_cost: i64,
	#[serde(default, deserialize_with = "null_default")]
	blackboard: Vec<RawEnemyBlackboardEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawEnemyBlackboardEntry {
	#[serde(default)]
	key: Option<String>,
	value: f64,
	#[serde(rename = "valueStr", default)]
	value_string: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLevelEnemies {
	#[serde(default, deserialize_with = "null_default")]
	enemies: Vec<RawLocalEnemy>,
	#[serde(rename = "enemyDbRefs", default, deserialize_with = "null_default")]
	enemy_db_refs: Vec<RawEnemyDbRef>,
}

#[derive(Debug, Deserialize)]
struct RawEnemyDbRef {
	#[serde(rename = "useDb")]
	use_db: bool,
	id: String,
	level: i64,
	#[serde(rename = "overwrittenData", default)]
	overwritten_data: Option<RawEnemyPatch>,
}

#[derive(Debug, Deserialize)]
struct RawLocalEnemy {
	key: String,
	#[serde(default)]
	name: Option<String>,
	#[serde(default)]
	description: Option<String>,
	#[serde(rename = "prefabKey", default)]
	prefab_key: Option<String>,
	#[serde(default)]
	attributes: RawLocalAttributes,
	#[serde(rename = "applyWay", default)]
	apply_way: Option<RawEnumValue>,
	#[serde(default)]
	motion: Option<RawEnumValue>,
	#[serde(rename = "enemyTags", default)]
	enemy_tags: Option<Vec<String>>,
	#[serde(rename = "lifePointReduce", default)]
	life_point_reduce: Option<i64>,
	#[serde(rename = "levelType", default)]
	level_type: Option<RawEnumValue>,
	#[serde(rename = "rangeRadius", default)]
	range_radius: Option<f64>,
	#[serde(rename = "notCountInTotal", default)]
	not_count_in_total: Option<bool>,
	#[serde(default)]
	skills: Option<Vec<RawEnemySkill>>,
	#[serde(rename = "talentBlackboard", default)]
	talent_blackboard: Option<Value>,
	#[serde(rename = "spData", default)]
	sp_data: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLocalAttributes {
	#[serde(rename = "maxHp", default)]
	max_hp: Option<i64>,
	#[serde(rename = "atk", default)]
	attack: Option<i64>,
	#[serde(rename = "def", default)]
	defense: Option<i64>,
	#[serde(rename = "blockCnt", default)]
	block_count: Option<i64>,
	#[serde(rename = "magicResistance", default)]
	magic_resistance: Option<f64>,
	#[serde(rename = "moveSpeed", default)]
	move_speed: Option<f64>,
	#[serde(rename = "attackSpeed", default)]
	attack_speed: Option<f64>,
	#[serde(rename = "baseAttackTime", default)]
	base_attack_time: Option<f64>,
	#[serde(rename = "massLevel", default)]
	mass_level: Option<i64>,
}

#[derive(Debug, Default)]
struct EnemyState {
	prefab_key: Option<String>,
	stats: EnemyStats,
	skills: Option<Vec<EnemySkill>>,
	name: Option<String>,
	description: Option<String>,
	apply_way: Option<RawEnumValue>,
	motion: Option<RawEnumValue>,
	level_type: Option<RawEnumValue>,
	enemy_tags: Option<Vec<String>>,
	talent_blackboard: Option<Value>,
	sp_data: Option<Value>,
}

impl EnemyState {
	fn apply_patch(
		&mut self,
		patch: &RawEnemyPatch,
		enemy: &EnemyKey,
		context: &str,
	) -> Result<(), ArkDataError> {
		apply_nullable(patch.name.as_ref(), &mut self.name);
		apply_nullable(patch.description.as_ref(), &mut self.description);
		apply_nullable(patch.prefab_key.as_ref(), &mut self.prefab_key);
		apply_nullable(patch.apply_way.as_ref(), &mut self.apply_way);
		apply_nullable(patch.motion.as_ref(), &mut self.motion);
		apply_nullable(patch.level_type.as_ref(), &mut self.level_type);
		apply_nullable(patch.enemy_tags.as_ref(), &mut self.enemy_tags);

		apply_i32(
			patch.attributes.max_hp.as_ref(),
			&mut self.stats.max_hp,
			enemy,
			"attributes.maxHp",
			context,
		)?;
		apply_i32(
			patch.attributes.attack.as_ref(),
			&mut self.stats.attack,
			enemy,
			"attributes.atk",
			context,
		)?;
		apply_i32(
			patch.attributes.defense.as_ref(),
			&mut self.stats.defense,
			enemy,
			"attributes.def",
			context,
		)?;
		apply_i32(
			patch.attributes.block_count.as_ref(),
			&mut self.stats.block_count,
			enemy,
			"attributes.blockCnt",
			context,
		)?;
		apply_f64(
			patch.attributes.magic_resistance.as_ref(),
			&mut self.stats.magic_resistance,
			enemy,
			"attributes.magicResistance",
			context,
		)?;
		apply_f64(
			patch.attributes.move_speed.as_ref(),
			&mut self.stats.move_speed,
			enemy,
			"attributes.moveSpeed",
			context,
		)?;
		apply_f64(
			patch.attributes.attack_speed.as_ref(),
			&mut self.stats.attack_speed,
			enemy,
			"attributes.attackSpeed",
			context,
		)?;
		apply_f64(
			patch.attributes.base_attack_time.as_ref(),
			&mut self.stats.base_attack_time,
			enemy,
			"attributes.baseAttackTime",
			context,
		)?;
		apply_i32(
			patch.attributes.mass_level.as_ref(),
			&mut self.stats.mass_level,
			enemy,
			"attributes.massLevel",
			context,
		)?;
		apply_i32(
			patch.life_point_reduce.as_ref(),
			&mut self.stats.life_point_reduce,
			enemy,
			"lifePointReduce",
			context,
		)?;
		apply_f64(
			patch.range_radius.as_ref(),
			&mut self.stats.range_radius,
			enemy,
			"rangeRadius",
			context,
		)?;
		apply_bool(
			patch.not_count_in_total.as_ref(),
			&mut self.stats.not_count_in_total,
		);

		if let Some(raw_skills) = &patch.skills {
			self.skills = Some(compile_skills(raw_skills, enemy, context)?);
		}
		if let Some(talent_blackboard) = &patch.talent_blackboard {
			self.talent_blackboard = Some(talent_blackboard.clone());
		}
		if let Some(sp_data) = &patch.sp_data {
			self.sp_data = Some(sp_data.clone());
		}
		Ok(())
	}

	fn finish(self, key: EnemyKey, database_level: Option<u32>) -> CompiledEnemy {
		CompiledEnemy {
			key,
			database_level,
			prefab_key: self.prefab_key,
			stats: self.stats,
			skills: self.skills.unwrap_or_default(),
			_name: self.name,
			_description: self.description,
			apply_way: self.apply_way,
			_motion: self.motion,
			_level_type: self.level_type,
			_enemy_tags: self.enemy_tags,
			_talent_blackboard: self.talent_blackboard,
			_sp_data: self.sp_data,
			handbook: None,
		}
	}
}

fn apply_nullable<T: Clone>(wrapper: Option<&RawDefined<T>>, target: &mut Option<T>) {
	if let Some(wrapper) = wrapper.filter(|wrapper: &&RawDefined<T>| wrapper.m_defined) {
		*target = wrapper.m_value.clone();
	}
}

fn apply_i32(
	wrapper: Option<&RawDefined<i64>>,
	target: &mut Option<i32>,
	enemy: &EnemyKey,
	field: &'static str,
	context: &str,
) -> Result<(), ArkDataError> {
	let Some(wrapper) = wrapper.filter(|wrapper: &&RawDefined<i64>| wrapper.m_defined) else {
		return Ok(());
	};
	let value: i64 = wrapper
		.m_value
		.ok_or_else(|| invalid_field(enemy, field, context, "defined value must not be null"))?;
	*target = Some(i32::try_from(value).map_err(|_| {
		invalid_field(
			enemy,
			field,
			context,
			&format!("value {value} is outside i32 range"),
		)
	})?);
	Ok(())
}

fn apply_f64(
	wrapper: Option<&RawDefined<f64>>,
	target: &mut Option<f64>,
	enemy: &EnemyKey,
	field: &'static str,
	context: &str,
) -> Result<(), ArkDataError> {
	let Some(wrapper) = wrapper.filter(|wrapper: &&RawDefined<f64>| wrapper.m_defined) else {
		return Ok(());
	};
	let value: f64 = wrapper
		.m_value
		.ok_or_else(|| invalid_field(enemy, field, context, "defined value must not be null"))?;
	if !value.is_finite() {
		return Err(invalid_field(enemy, field, context, "value must be finite"));
	}
	*target = Some(value);
	Ok(())
}

fn apply_bool(wrapper: Option<&RawDefined<bool>>, target: &mut Option<bool>) {
	if let Some(wrapper) = wrapper.filter(|wrapper: &&RawDefined<bool>| wrapper.m_defined) {
		*target = wrapper.m_value;
	}
}

fn compile_skills(
	raw_skills: &[RawEnemySkill],
	enemy: &EnemyKey,
	context: &str,
) -> Result<Vec<EnemySkill>, ArkDataError> {
	raw_skills
		.iter()
		.enumerate()
		.map(|(index, raw): (usize, &RawEnemySkill)| {
			if raw.prefab_key.is_empty() {
				return Err(invalid_field(
					enemy,
					"skills[].prefabKey",
					context,
					&format!("skill {index} identity must not be empty"),
				));
			}
			if !raw.cooldown.is_finite() || !raw.init_cooldown.is_finite() {
				return Err(invalid_field(
					enemy,
					"skills[]",
					context,
					&format!("skill {index} cooldown values must be finite"),
				));
			}
			let priority: i32 = i32::try_from(raw.priority).map_err(|_| {
				invalid_field(
					enemy,
					"skills[].priority",
					context,
					&format!("skill {index} value {} is outside i32 range", raw.priority),
				)
			})?;
			let sp_cost: i32 = i32::try_from(raw.sp_cost).map_err(|_| {
				invalid_field(
					enemy,
					"skills[].spCost",
					context,
					&format!("skill {index} value {} is outside i32 range", raw.sp_cost),
				)
			})?;
			let blackboard: Vec<EnemyBlackboardEntry> = raw
				.blackboard
				.iter()
				.enumerate()
				.map(|(entry_index, entry)| {
					let key: &str = entry
						.key
						.as_deref()
						.filter(|key: &&str| !key.trim().is_empty())
						.ok_or_else(|| {
							invalid_field(
								enemy,
								"skills[].blackboard[].key",
								context,
								&format!(
									"skill {index} blackboard entry {entry_index} key must not be null or blank"
								),
							)
						})?;
					if !entry.value.is_finite() {
						return Err(invalid_field(
							enemy,
							"skills[].blackboard[].value",
							context,
							&format!("skill {index} blackboard entry {entry_index} must be finite"),
						));
					}
					Ok(EnemyBlackboardEntry {
						key: key.to_owned(),
						value: entry.value,
						value_string: entry.value_string.clone(),
					})
				})
				.collect::<Result<_, ArkDataError>>()?;
			Ok(EnemySkill {
				prefab_key: raw.prefab_key.clone(),
				priority,
				cooldown: raw.cooldown,
				initial_cooldown: raw.init_cooldown,
				sp_cost,
				blackboard,
			})
		})
		.collect()
}

fn invalid_field(
	enemy: &EnemyKey,
	field: &'static str,
	context: &str,
	reason: &str,
) -> ArkDataError {
	ArkDataError::InvalidEnemyField {
		enemy_id: enemy.0.clone(),
		field,
		reason: format!("{context}: {reason}"),
	}
}

fn compile_top_level_enemy(
	raw: RawLocalEnemy,
	key: EnemyKey,
) -> Result<CompiledEnemy, ArkDataError> {
	if raw.prefab_key.as_deref() == Some("") {
		return Err(invalid_field(
			&key,
			"prefabKey",
			"top-level local enemy",
			"identity must not be empty",
		));
	}
	let stats: EnemyStats = EnemyStats {
		max_hp: local_i32(raw.attributes.max_hp, &key, "attributes.maxHp")?,
		attack: local_i32(raw.attributes.attack, &key, "attributes.atk")?,
		defense: local_i32(raw.attributes.defense, &key, "attributes.def")?,
		block_count: local_i32(raw.attributes.block_count, &key, "attributes.blockCnt")?,
		magic_resistance: local_f64(
			raw.attributes.magic_resistance,
			&key,
			"attributes.magicResistance",
		)?,
		move_speed: local_f64(raw.attributes.move_speed, &key, "attributes.moveSpeed")?,
		attack_speed: local_f64(raw.attributes.attack_speed, &key, "attributes.attackSpeed")?,
		base_attack_time: local_f64(
			raw.attributes.base_attack_time,
			&key,
			"attributes.baseAttackTime",
		)?,
		mass_level: local_i32(raw.attributes.mass_level, &key, "attributes.massLevel")?,
		life_point_reduce: local_i32(raw.life_point_reduce, &key, "lifePointReduce")?,
		range_radius: local_f64(raw.range_radius, &key, "rangeRadius")?,
		not_count_in_total: raw.not_count_in_total,
	};
	let skills: Vec<EnemySkill> = match raw.skills {
		Some(raw_skills) => compile_skills(&raw_skills, &key, "top-level local enemy")?,
		None => Vec::new(),
	};
	Ok(CompiledEnemy {
		key,
		database_level: None,
		prefab_key: raw.prefab_key,
		stats,
		skills,
		_name: raw.name,
		_description: raw.description,
		apply_way: raw.apply_way,
		_motion: raw.motion,
		_level_type: raw.level_type,
		_enemy_tags: raw.enemy_tags,
		_talent_blackboard: raw.talent_blackboard,
		_sp_data: raw.sp_data,
		handbook: None,
	})
}

fn local_i32(
	value: Option<i64>,
	enemy: &EnemyKey,
	field: &'static str,
) -> Result<Option<i32>, ArkDataError> {
	value
		.map(|value: i64| {
			i32::try_from(value).map_err(|_| {
				invalid_field(
					enemy,
					field,
					"top-level local enemy",
					&format!("value {value} is outside i32 range"),
				)
			})
		})
		.transpose()
}

fn local_f64(
	value: Option<f64>,
	enemy: &EnemyKey,
	field: &'static str,
) -> Result<Option<f64>, ArkDataError> {
	match value {
		Some(value) if !value.is_finite() => Err(invalid_field(
			enemy,
			field,
			"top-level local enemy",
			"value must be finite",
		)),
		value => Ok(value),
	}
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de> + Default,
{
	Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn checked_u32(
	value: i64,
	enemy: &EnemyKey,
	field: &'static str,
	reason: &'static str,
) -> Result<u32, ArkDataError> {
	u32::try_from(value).map_err(|_| ArkDataError::InvalidEnemyField {
		enemy_id: enemy.0.clone(),
		field,
		reason: format!("{reason}; found {value}"),
	})
}
