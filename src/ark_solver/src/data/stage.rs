use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::data::{
	enemy::{CompiledEnemy, EnemyCatalog, EnemyKey},
	manifest::VerifiedEntry,
	mechanics::{
		compile_requirements, CompiledStageMechanics, EnemyMechanics, IndexedMechanic,
		MechanicRequirements,
	},
	stage_payload::{
		BlockEdge, GridPosition, MapCell, MapDimensions, PredeployedStageActor, Route, Spawn,
		StageAction, StageBranch, StageOptions, StagePayload, StageWave,
	},
	ArkDataError,
};

const STAGE_TABLE_PATH: &str = "stage_table.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageIdentity {
	Code(String),
	StageId(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageDifficulty {
	Normal,
	Raid,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageDifficultySelection {
	Normal,
	Raid,
	Both,
}

impl StageDifficultySelection {
	pub fn expand(self, identity: StageIdentity) -> Vec<StageRef> {
		match self {
			Self::Normal => vec![StageRef {
				identity,
				difficulty: StageDifficulty::Normal,
			}],
			Self::Raid => vec![StageRef {
				identity,
				difficulty: StageDifficulty::Raid,
			}],
			Self::Both => vec![
				StageRef {
					identity: identity.clone(),
					difficulty: StageDifficulty::Normal,
				},
				StageRef {
					identity,
					difficulty: StageDifficulty::Raid,
				},
			],
		}
	}
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StageRef {
	pub identity: StageIdentity,
	pub difficulty: StageDifficulty,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableStageLevel {
	pub stage_id: String,
	pub difficulty: StageDifficulty,
	pub level_id: String,
}

/// Resolve the normal level and any valid Raid level directly from a stage-table document.
///
/// This uses the same canonical code-family, difficulty, and hard-stage validation as
/// [`ArkDataSnapshot`](crate::data::ArkDataSnapshot) without requiring a completed snapshot.
pub fn resolve_available_stage_levels(
	stage_table_bytes: &[u8],
	identity: StageIdentity,
) -> Result<Vec<AvailableStageLevel>, ArkDataError> {
	let document: StageTableDocument =
		serde_json::from_slice(stage_table_bytes).map_err(|source| ArkDataError::RootParse {
			destination_path: STAGE_TABLE_PATH.to_owned(),
			source,
		})?;
	let catalog: StageCatalog = StageCatalog::from_document(document)?;
	let stage: &StageRecord = catalog.resolve_available_identity(&identity)?;
	let normal_record: &StageRecord = catalog.resolve_difficulty(stage, StageDifficulty::Normal)?;
	let normal: ResolvedStage = ResolvedStage::from(normal_record);
	let mut levels: Vec<AvailableStageLevel> =
		vec![available_stage_level(normal, StageDifficulty::Normal)?];
	match catalog.resolve_difficulty(stage, StageDifficulty::Raid) {
		Ok(raid) => levels.push(available_stage_level(
			ResolvedStage::from(raid),
			StageDifficulty::Raid,
		)?),
		Err(ArkDataError::RaidUnavailable { .. }) => {}
		Err(error) => return Err(error),
	}
	Ok(levels)
}

fn available_stage_level(
	resolved: ResolvedStage,
	difficulty: StageDifficulty,
) -> Result<AvailableStageLevel, ArkDataError> {
	let level_id: String = resolved
		.level_id
		.filter(|level_id| !level_id.is_empty())
		.ok_or_else(|| ArkDataError::MissingStageLevelId {
			stage_id: resolved.stage_id.clone(),
		})?;
	Ok(AvailableStageLevel {
		stage_id: resolved.stage_id,
		difficulty,
		level_id,
	})
}

#[derive(Debug)]
pub struct CompiledStage {
	stage_id: String,
	code: Option<String>,
	difficulty: StageDifficulty,
	level_id: String,
	payload: StagePayload,
	enemies: BTreeMap<EnemyKey, CompiledEnemy>,
	mechanic_requirements: MechanicRequirements,
	active_hidden_groups: BTreeSet<String>,
}

impl CompiledStage {
	pub fn stage_id(&self) -> &str {
		&self.stage_id
	}

	pub fn code(&self) -> Option<&str> {
		self.code.as_deref()
	}

	pub fn difficulty(&self) -> StageDifficulty {
		self.difficulty
	}

	pub fn level_id(&self) -> &str {
		&self.level_id
	}

	pub fn options(&self) -> &StageOptions {
		self.payload.options()
	}

	pub fn map_dimensions(&self) -> MapDimensions {
		self.payload.map_dimensions()
	}

	pub fn cells(&self) -> &[MapCell] {
		self.payload.cells()
	}

	pub fn cell(&self, position: GridPosition) -> Option<&MapCell> {
		self.payload.cell(position)
	}

	pub fn block_edges(&self) -> &[BlockEdge] {
		self.payload.block_edges()
	}

	pub fn predeployed_actors(&self) -> &[PredeployedStageActor] {
		self.payload.predeployed_actors()
	}

	pub fn routes(&self) -> &[Route] {
		self.payload.routes()
	}

	pub fn extra_routes(&self) -> &[Route] {
		self.payload.extra_routes()
	}

	pub fn spawns(&self) -> &[Spawn] {
		self.payload.spawns()
	}

	pub fn waves(&self) -> &[StageWave] {
		self.payload.waves()
	}

	pub fn branches(&self) -> &BTreeMap<String, StageBranch> {
		self.payload.branches()
	}

	pub fn enemies(&self) -> impl ExactSizeIterator<Item = (&EnemyKey, &CompiledEnemy)> + '_ {
		self.enemies.iter()
	}

	pub fn enemy(&self, key: &EnemyKey) -> Option<&CompiledEnemy> {
		self.enemies.get(key)
	}

	pub fn mechanic_requirements(&self) -> &MechanicRequirements {
		&self.mechanic_requirements
	}

	pub fn active_hidden_groups(&self) -> &BTreeSet<String> {
		&self.active_hidden_groups
	}

	pub fn action_is_active(&self, action: &StageAction) -> bool {
		action
			.hidden_group()
			.is_none_or(|group| self.active_hidden_groups.contains(group))
	}

	pub(in crate::data) fn parse(
		stage: ResolvedStage,
		difficulty: StageDifficulty,
		level_id: String,
		destination_path: &str,
		bytes: &[u8],
		enemy_catalog: &EnemyCatalog,
	) -> Result<Self, ArkDataError> {
		let payload: StagePayload = StagePayload::parse(bytes)
			.map_err(|error| error.into_ark_data_error(destination_path))?;
		let enemies: BTreeMap<EnemyKey, CompiledEnemy> =
			enemy_catalog.compile_level(bytes, destination_path)?;
		for spawn in payload.spawns() {
			if !enemies.contains_key(spawn.enemy()) {
				return Err(ArkDataError::SpawnEnemyNotCompiled {
					key: spawn.enemy().as_str().to_owned(),
					origin: spawn.origin().clone(),
				});
			}
		}
		let enemy_mechanics: Vec<EnemyMechanics> = enemies
			.iter()
			.map(
				|(key, enemy): (&EnemyKey, &CompiledEnemy)| -> Result<EnemyMechanics, ArkDataError> {
					let skill_prefabs: Vec<IndexedMechanic> = enemy
						.skills()
						.iter()
						.enumerate()
						.map(|(index, skill)| {
							Ok(IndexedMechanic {
								index: u32::try_from(index).map_err(|_| {
									ArkDataError::InvalidLevelCollectionLength {
										destination_path: destination_path.to_owned(),
										field: "enemy skills",
										length: index.saturating_add(1),
									}
								})?,
								id: skill.prefab_key().to_owned(),
							})
						})
						.collect::<Result<_, ArkDataError>>()?;
					Ok(EnemyMechanics {
						enemy: key.clone(),
						prefab_key: enemy.prefab_key().map(str::to_owned),
						skill_prefabs,
					})
				},
			)
			.collect::<Result<_, ArkDataError>>()?;
		let CompiledStageMechanics {
			requirements: mechanic_requirements,
			active_hidden_groups,
		} = compile_requirements(
			bytes,
			destination_path,
			difficulty,
			&payload,
			enemy_mechanics,
		)?;

		Ok(Self {
			stage_id: stage.stage_id,
			code: stage.code,
			difficulty,
			level_id,
			payload,
			enemies,
			mechanic_requirements,
			active_hidden_groups,
		})
	}
}

#[derive(Debug)]
pub(in crate::data) struct StageCatalog {
	stages: BTreeMap<String, StageRecord>,
	diff_groups: Vec<DiffGroup>,
}

impl StageCatalog {
	pub(in crate::data) fn parse(
		entries: &BTreeMap<String, VerifiedEntry>,
	) -> Result<Self, ArkDataError> {
		let entry: &VerifiedEntry =
			entries
				.get(STAGE_TABLE_PATH)
				.ok_or(ArkDataError::MissingRequiredRoot {
					destination_path: STAGE_TABLE_PATH,
				})?;
		let bytes: &[u8] =
			entry
				.retained_bytes
				.as_deref()
				.ok_or_else(|| ArkDataError::InvalidManifest {
					field: "entries",
					reason: format!("verified root {STAGE_TABLE_PATH:?} did not retain its bytes"),
				})?;
		let document: StageTableDocument =
			serde_json::from_slice(bytes).map_err(|source| ArkDataError::RootParse {
				destination_path: entry.destination_path.clone(),
				source,
			})?;

		Self::from_document(document)
	}

	pub(in crate::data) fn resolve(
		&self,
		stage_ref: &StageRef,
	) -> Result<ResolvedStage, ArkDataError> {
		let stage: &StageRecord = match &stage_ref.identity {
			StageIdentity::StageId(stage_id) => {
				self.stages.get(&normalize(stage_id)).ok_or_else(|| {
					ArkDataError::StageNotFound {
						identity: stage_ref.identity.clone(),
					}
				})?
			}
			StageIdentity::Code(code) => self.resolve_code(code, &stage_ref.identity)?,
		};
		let resolved: &StageRecord = self.resolve_difficulty(stage, stage_ref.difficulty)?;

		Ok(ResolvedStage {
			stage_id: resolved.stage_id.clone(),
			code: resolved.code.clone(),
			level_id: resolved.level_id.clone(),
		})
	}

	fn resolve_available_identity(
		&self,
		identity: &StageIdentity,
	) -> Result<&StageRecord, ArkDataError> {
		match identity {
			StageIdentity::StageId(stage_id) => {
				self.stages
					.get(&normalize(stage_id))
					.ok_or_else(|| ArkDataError::StageNotFound {
						identity: identity.clone(),
					})
			}
			StageIdentity::Code(code) => self.resolve_code_impl(code, identity, false),
		}
	}

	fn from_document(document: StageTableDocument) -> Result<Self, ArkDataError> {
		let mut stages: BTreeMap<String, StageRecord> = BTreeMap::new();
		for (map_key, raw_stage) in document.stages {
			let normalized_map_key: String = normalize(&map_key);
			let normalized_stage_id: String = normalize(&raw_stage.stage_id);
			if normalized_map_key != normalized_stage_id {
				return Err(ArkDataError::StageMapKeyMismatch {
					map_key,
					stage_id: raw_stage.stage_id,
				});
			}

			let stage: StageRecord = StageRecord::from(raw_stage);
			if let Some(first) = stages.get(&normalized_stage_id) {
				return Err(ArkDataError::DuplicateStageId {
					first_stage_id: first.stage_id.clone(),
					second_stage_id: stage.stage_id,
				});
			}
			stages.insert(normalized_stage_id, stage);
		}

		let diff_groups: Vec<DiffGroup> = document
			.diff_group_table
			.into_values()
			.map(DiffGroup::from)
			.collect();
		Ok(Self {
			stages,
			diff_groups,
		})
	}

	fn resolve_code(
		&self,
		code: &str,
		identity: &StageIdentity,
	) -> Result<&StageRecord, ArkDataError> {
		self.resolve_code_impl(code, identity, true)
	}

	fn resolve_code_impl(
		&self,
		code: &str,
		identity: &StageIdentity,
		include_unavailable: bool,
	) -> Result<&StageRecord, ArkDataError> {
		let normalized_code: String = normalize(code);
		let mut candidates: BTreeMap<String, String> = BTreeMap::new();
		for (normalized_stage_id, stage) in &self.stages {
			if stage.code.as_ref().map(|value: &String| normalize(value))
				!= Some(normalized_code.clone())
			{
				continue;
			}

			match stage.difficulty {
				RecordDifficulty::FourStar => {
					for (source_id, source) in &self.stages {
						if source.difficulty == RecordDifficulty::Normal
							&& source
								.hard_stage_id
								.as_ref()
								.is_some_and(|hard_id: &String| {
									normalize(hard_id) == *normalized_stage_id
								}) {
							self.add_canonical_families(source_id, &mut candidates);
						}
					}
				}
				RecordDifficulty::Normal => {
					self.add_canonical_families(normalized_stage_id, &mut candidates);
				}
				RecordDifficulty::Unavailable => {
					if include_unavailable {
						self.add_direct_candidate(normalized_stage_id, stage, &mut candidates);
					}
				}
			}
		}

		if candidates.is_empty() {
			return Err(ArkDataError::StageNotFound {
				identity: identity.clone(),
			});
		}
		if candidates.len() > 1 {
			let mut candidate_stage_ids: Vec<String> = candidates.into_values().collect();
			candidate_stage_ids.sort();
			return Err(ArkDataError::AmbiguousStageCode {
				code: code.to_string(),
				candidate_stage_ids,
			});
		}

		let normalized_stage_id: String = candidates
			.into_keys()
			.next()
			.expect("nonempty candidate map checked above");
		self.stages
			.get(&normalized_stage_id)
			.ok_or_else(|| ArkDataError::StageNotFound {
				identity: identity.clone(),
			})
	}

	fn add_direct_candidate(
		&self,
		normalized_stage_id: &str,
		stage: &StageRecord,
		candidates: &mut BTreeMap<String, String>,
	) {
		candidates
			.entry(normalized_stage_id.to_string())
			.or_insert_with(|| stage.stage_id.clone());
	}

	fn add_canonical_families(
		&self,
		normalized_stage_id: &str,
		candidates: &mut BTreeMap<String, String>,
	) {
		let mut matched_diff_group: bool = false;
		for group in &self.diff_groups {
			if group.contains(normalized_stage_id) {
				if let Some(normal_id) = group
					.normal_id
					.as_deref()
					.filter(|id: &&str| !id.is_empty())
				{
					matched_diff_group = true;
					let normalized_normal_id: String = normalize(normal_id);
					let original_normal_id: String = self
						.stages
						.get(&normalized_normal_id)
						.map(|stage: &StageRecord| stage.stage_id.clone())
						.unwrap_or_else(|| normal_id.to_string());
					candidates
						.entry(normalized_normal_id)
						.or_insert(original_normal_id);
				}
			}
		}

		if !matched_diff_group {
			let stage_id: String = self
				.stages
				.get(normalized_stage_id)
				.map(|stage: &StageRecord| stage.stage_id.clone())
				.unwrap_or_else(|| normalized_stage_id.to_string());
			candidates
				.entry(normalized_stage_id.to_string())
				.or_insert(stage_id);
		}
	}

	fn resolve_difficulty<'a>(
		&'a self,
		stage: &'a StageRecord,
		difficulty: StageDifficulty,
	) -> Result<&'a StageRecord, ArkDataError> {
		match (stage.difficulty, difficulty) {
			(RecordDifficulty::Normal, StageDifficulty::Normal)
			| (RecordDifficulty::FourStar, StageDifficulty::Raid) => Ok(stage),
			(RecordDifficulty::FourStar, StageDifficulty::Normal)
			| (RecordDifficulty::Unavailable, _) => Err(ArkDataError::StageDifficultyConflict {
				stage_id: stage.stage_id.clone(),
				requested_difficulty: difficulty,
			}),
			(RecordDifficulty::Normal, StageDifficulty::Raid) => {
				let hard_stage_id: &str = stage
					.hard_stage_id
					.as_deref()
					.filter(|hard_stage_id: &&str| !hard_stage_id.is_empty())
					.ok_or_else(|| ArkDataError::RaidUnavailable {
						stage_id: stage.stage_id.clone(),
					})?;
				let hard_stage: &StageRecord = self
					.stages
					.get(&normalize(hard_stage_id))
					.ok_or_else(|| ArkDataError::InvalidHardStageLink {
						stage_id: stage.stage_id.clone(),
						hard_stage_id: hard_stage_id.to_string(),
					})?;
				if hard_stage.difficulty != RecordDifficulty::FourStar {
					return Err(ArkDataError::InvalidHardStageLink {
						stage_id: stage.stage_id.clone(),
						hard_stage_id: hard_stage_id.to_string(),
					});
				}
				Ok(hard_stage)
			}
		}
	}
}

#[derive(Debug)]
pub(in crate::data) struct ResolvedStage {
	pub(in crate::data) stage_id: String,
	pub(in crate::data) code: Option<String>,
	pub(in crate::data) level_id: Option<String>,
}

impl From<&StageRecord> for ResolvedStage {
	fn from(stage: &StageRecord) -> Self {
		Self {
			stage_id: stage.stage_id.clone(),
			code: stage.code.clone(),
			level_id: stage.level_id.clone(),
		}
	}
}

#[derive(Debug)]
struct StageRecord {
	stage_id: String,
	code: Option<String>,
	level_id: Option<String>,
	difficulty: RecordDifficulty,
	hard_stage_id: Option<String>,
	_diff_group: Option<String>,
}

impl From<RawStageRecord> for StageRecord {
	fn from(raw: RawStageRecord) -> Self {
		Self {
			stage_id: raw.stage_id,
			code: raw.code,
			level_id: raw.level_id,
			difficulty: RecordDifficulty::from(raw.difficulty.as_str()),
			hard_stage_id: raw.hard_stage_id,
			_diff_group: raw.diff_group,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordDifficulty {
	Normal,
	FourStar,
	Unavailable,
}

impl From<&str> for RecordDifficulty {
	fn from(value: &str) -> Self {
		match value {
			"NORMAL" => Self::Normal,
			"FOUR_STAR" => Self::FourStar,
			_ => Self::Unavailable,
		}
	}
}

#[derive(Debug)]
struct DiffGroup {
	easy_id: Option<String>,
	normal_id: Option<String>,
	tough_id: Option<String>,
}

impl DiffGroup {
	fn contains(&self, normalized_stage_id: &str) -> bool {
		[&self.easy_id, &self.normal_id, &self.tough_id]
			.into_iter()
			.flatten()
			.any(|stage_id: &String| normalize(stage_id) == normalized_stage_id)
	}
}

impl From<RawDiffGroup> for DiffGroup {
	fn from(raw: RawDiffGroup) -> Self {
		Self {
			easy_id: raw.easy_id,
			normal_id: raw.normal_id,
			tough_id: raw.tough_id,
		}
	}
}

#[derive(Debug, Deserialize)]
struct StageTableDocument {
	#[serde(default)]
	stages: BTreeMap<String, RawStageRecord>,
	#[serde(rename = "diffGroupTable", default)]
	diff_group_table: BTreeMap<String, RawDiffGroup>,
	#[serde(rename = "runeStageGroups", default)]
	_rune_stage_groups: Value,
}

#[derive(Debug, Deserialize)]
struct RawStageRecord {
	#[serde(rename = "stageId")]
	stage_id: String,
	#[serde(default)]
	code: Option<String>,
	#[serde(rename = "levelId", default)]
	level_id: Option<String>,
	difficulty: String,
	#[serde(rename = "hardStagedId", default)]
	hard_stage_id: Option<String>,
	#[serde(rename = "diffGroup", default)]
	diff_group: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDiffGroup {
	#[serde(rename = "easyId", default)]
	easy_id: Option<String>,
	#[serde(rename = "normalId", default)]
	normal_id: Option<String>,
	#[serde(rename = "toughId", default)]
	tough_id: Option<String>,
}

fn normalize(value: &str) -> String {
	value.to_ascii_lowercase()
}
