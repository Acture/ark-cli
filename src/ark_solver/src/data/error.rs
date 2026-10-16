use std::{io, path::PathBuf};

use thiserror::Error;

use crate::data::{
	operator::OperatorIdentity,
	stage::{StageDifficulty, StageIdentity},
	stage_payload::{GridPosition, WaveActionOrigin},
};

#[derive(Debug, Error)]
pub enum ArkDataError {
	#[error("invalid battle-prefab manifest field {field}: {reason}")]
	InvalidBattlePrefabManifest { field: &'static str, reason: String },
	#[error(
		"battle-prefab mechanics revision is {actual}, but canonical manifest identity requires {expected}"
	)]
	MechanicsRevisionMismatch { expected: String, actual: String },
	#[error(
		"battle-prefab snapshot targets gamedata revision {snapshot_gamedata_revision}, but loaded gamedata is {gamedata_revision}"
	)]
	BattlePrefabGamedataMismatch {
		snapshot_gamedata_revision: String,
		gamedata_revision: String,
	},
	#[error("failed to parse battle-prefab entry {destination_path}: {source}")]
	BattlePrefabEntryParse {
		destination_path: String,
		#[source]
		source: serde_json::Error,
	},
	#[error("invalid battle-prefab entry {destination_path}: {reason}")]
	InvalidBattlePrefabEntry {
		destination_path: String,
		reason: String,
	},
	#[error("duplicate battle-prefab bundle keys {first:?} and {second:?}")]
	DuplicateBattlePrefabBundle { first: String, second: String },
	#[error("duplicate serialized-file CAB identities {first:?} and {second:?}")]
	DuplicateBattlePrefabCab { first: String, second: String },
	#[error(
		"could not resolve object from source CAB {source_cab:?}, fileID {file_id}, pathID {path_id}: {reason}"
	)]
	BattlePrefabObjectResolution {
		source_cab: String,
		file_id: i32,
		path_id: i64,
		reason: String,
	},
	#[error("snapshot root is not a directory: {path}", path = .path.display())]
	InvalidSnapshotRoot { path: PathBuf },
	#[error("failed to read manifest {path}: {source}", path = .path.display())]
	ManifestRead {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("manifest is not a regular file: {path}", path = .path.display())]
	ManifestNotRegular { path: PathBuf },
	#[error("failed to parse manifest {path}: {source}", path = .path.display())]
	ManifestParse {
		path: PathBuf,
		#[source]
		source: serde_json::Error,
	},
	#[error("invalid manifest field {field}: {reason}")]
	InvalidManifest { field: &'static str, reason: String },
	#[error("unsafe {field} path {path:?}: {reason}")]
	UnsafePath {
		field: &'static str,
		path: String,
		reason: &'static str,
	},
	#[error("duplicate destination paths {first:?} and {second:?}")]
	DuplicateDestination { first: String, second: String },
	#[error("required snapshot root is missing: {destination_path}")]
	MissingRequiredRoot { destination_path: &'static str },
	#[error("snapshot entry is not a regular file: {destination_path}")]
	EntryNotRegular { destination_path: String },
	#[error("failed to read snapshot entry {destination_path}: {source}")]
	EntryRead {
		destination_path: String,
		#[source]
		source: io::Error,
	},
	#[error("snapshot entry {destination_path} has byte length {actual}, expected {expected}")]
	ByteLengthMismatch {
		destination_path: String,
		expected: u64,
		actual: u64,
	},
	#[error("snapshot entry {destination_path} has SHA-256 {actual}, expected {expected}")]
	Sha256Mismatch {
		destination_path: String,
		expected: String,
		actual: String,
	},
	#[error("failed to parse verified root {destination_path}: {source}")]
	RootParse {
		destination_path: String,
		#[source]
		source: serde_json::Error,
	},
	#[error("invalid operator table {destination_path}: {reason}")]
	InvalidOperatorTable {
		destination_path: &'static str,
		reason: String,
	},
	#[error("operator was not found for identity {identity:?}")]
	OperatorNotFound { identity: OperatorIdentity },
	#[error("operator name {name:?} is ambiguous across IDs {candidate_ids:?}")]
	AmbiguousOperatorName {
		name: String,
		candidate_ids: Vec<String>,
	},
	#[error("operator {operator_id:?} field {field} is invalid: {reason}")]
	InvalidOperatorRecord {
		operator_id: String,
		field: &'static str,
		reason: String,
	},
	#[error("operator {operator_id:?} build field {field} is invalid: {reason}")]
	InvalidOperatorBuild {
		operator_id: String,
		field: &'static str,
		reason: String,
	},
	#[error("operator {operator_id:?} progression {field} is unsupported: {reason}")]
	UnsupportedOperatorProgression {
		operator_id: String,
		field: &'static str,
		reason: String,
	},
	#[error("stage was not found for identity {identity:?}")]
	StageNotFound { identity: StageIdentity },
	#[error("stage code {code:?} is ambiguous across canonical stages {candidate_stage_ids:?}")]
	AmbiguousStageCode {
		code: String,
		candidate_stage_ids: Vec<String>,
	},
	#[error("stage {stage_id:?} is unavailable for {requested_difficulty:?}")]
	StageDifficultyConflict {
		stage_id: String,
		requested_difficulty: StageDifficulty,
	},
	#[error("stage {stage_id:?} has no raid variant")]
	RaidUnavailable { stage_id: String },
	#[error("stage {stage_id:?} has invalid hard-stage link {hard_stage_id:?}")]
	InvalidHardStageLink {
		stage_id: String,
		hard_stage_id: String,
	},
	#[error("stage {stage_id:?} has no level ID")]
	MissingStageLevelId { stage_id: String },
	#[error(
		"stage {stage_id:?} level {level_id:?} is missing verified manifest entry {destination_path:?}"
	)]
	MissingLevelManifestEntry {
		stage_id: String,
		level_id: String,
		destination_path: String,
	},
	#[error("duplicate normalized stage IDs {first_stage_id:?} and {second_stage_id:?}")]
	DuplicateStageId {
		first_stage_id: String,
		second_stage_id: String,
	},
	#[error("stage map key {map_key:?} does not match record ID {stage_id:?}")]
	StageMapKeyMismatch { map_key: String, stage_id: String },
	#[error("range map key {map_key:?} does not match record ID {range_id:?}")]
	RangeMapKeyMismatch { map_key: String, range_id: String },
	#[error("range ID is invalid: {reason}")]
	InvalidRangeId { range_id: String, reason: String },
	#[error("duplicate normalized range IDs {first_range_id:?} and {second_range_id:?}")]
	DuplicateRangeId {
		first_range_id: String,
		second_range_id: String,
	},
	#[error("enemy identity {enemy_id:?} is invalid: {reason}")]
	InvalidEnemyIdentity { enemy_id: String, reason: String },
	#[error("duplicate enemy identity {enemy_id:?}")]
	DuplicateEnemyId { enemy_id: String },
	#[error("duplicate database level {level} for enemy {enemy_id:?}")]
	DuplicateEnemyLevel { enemy_id: String, level: u32 },
	#[error("duplicate stage enemy reference {enemy_id:?} at level {level}")]
	DuplicateEnemyReference { enemy_id: String, level: u32 },
	#[error("enemy database does not contain identity {enemy_id:?}")]
	MissingEnemyId { enemy_id: String },
	#[error(
		"enemy {enemy_id:?} has no exact level {requested_level}; available levels: {available_levels:?}"
	)]
	MissingEnemyLevel {
		enemy_id: String,
		requested_level: u32,
		available_levels: Vec<u32>,
	},
	#[error("local enemy {enemy_id:?} has no overwritten data")]
	MissingLocalEnemyData { enemy_id: String },
	#[error("enemy {enemy_id:?} field {field} is invalid: {reason}")]
	InvalidEnemyField {
		enemy_id: String,
		field: &'static str,
		reason: String,
	},
	#[error("map in {destination_path} is invalid: {reason}")]
	InvalidMap {
		destination_path: String,
		reason: String,
	},
	#[error(
		"map cell {position:?} in {destination_path} uses palette index {tile_index}, but palette has {tile_count} entries"
	)]
	InvalidTilePaletteIndex {
		destination_path: String,
		tile_index: i64,
		tile_count: u32,
		position: GridPosition,
	},
	#[error(
		"route {route} (extra={extra}) position ({row}, {col}) in {destination_path} is invalid: {reason}"
	)]
	InvalidRoutePosition {
		destination_path: String,
		extra: bool,
		route: u32,
		row: i64,
		col: i64,
		reason: String,
	},
	#[error(
		"route {route} (extra={extra}) checkpoint {checkpoint} position ({row}, {col}) in {destination_path} is invalid: {reason}"
	)]
	InvalidRouteCheckpointPosition {
		destination_path: String,
		extra: bool,
		route: u32,
		checkpoint: u32,
		row: i64,
		col: i64,
		reason: String,
	},
	#[error(
		"route {route} (extra={extra}) checkpoint {checkpoint} field {field} in {destination_path} is invalid: {reason}"
	)]
	InvalidRouteCheckpointField {
		destination_path: String,
		extra: bool,
		route: u32,
		checkpoint: u32,
		field: &'static str,
		reason: String,
	},
	#[error(
		"SPAWN at {origin:?} in {destination_path} uses primary route index {route_index}, but route count is {route_count}"
	)]
	InvalidSpawnRouteIndex {
		destination_path: String,
		route_index: i64,
		route_count: u32,
		origin: WaveActionOrigin,
	},
	#[error("action at {origin:?} field {field} in {destination_path} is invalid: {reason}")]
	InvalidActionField {
		destination_path: String,
		origin: WaveActionOrigin,
		field: &'static str,
		reason: String,
	},
	#[error("level collection {field} in {destination_path} has unsupported length {length}")]
	InvalidLevelCollectionLength {
		destination_path: String,
		field: &'static str,
		length: usize,
	},
	#[error("SPAWN at {origin:?} references uncompiled enemy {key:?}")]
	SpawnEnemyNotCompiled {
		key: String,
		origin: WaveActionOrigin,
	},
	#[error("mechanic identity {id:?} is invalid: {reason}")]
	InvalidMechanicIdentity { id: String, reason: String },
	#[error("rune difficulty mask {mask:?} is invalid: {reason}")]
	InvalidRuneDifficultyMask { mask: String, reason: String },
	#[error("random SPAWN at {origin:?} is invalid: {reason}")]
	InvalidRandomSpawn {
		origin: WaveActionOrigin,
		reason: String,
	},
	#[error("failed to parse verified level {destination_path}: {source}")]
	LevelParse {
		destination_path: String,
		#[source]
		source: serde_json::Error,
	},
}
