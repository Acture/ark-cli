use std::collections::BTreeMap;

use serde::{de::Error as SerdeError, Deserialize, Deserializer, Serialize};

use crate::data::{enemy::EnemyKey, ArkDataError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct MapDimensions {
	pub width: u32,
	pub height: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct GridPosition {
	pub row: u32,
	pub col: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, PartialOrd, Serialize)]
pub struct StageVector2 {
	pub x: f64,
	pub y: f64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FunctionDisableMask(u32);

impl FunctionDisableMask {
	pub const NONE: Self = Self(0);
	pub const CARD_LIST: Self = Self(1);
	pub const CHARACTER_MENU: Self = Self(1 << 1);
	pub const CHARACTER_INFO: Self = Self(1 << 2);
	pub const SYSTEM_MENU: Self = Self(1 << 3);
	pub const PAUSE_BUTTON: Self = Self(1 << 4);
	pub const SPEED_SWITCHER_BUTTON: Self = Self(1 << 5);
	pub const BATTLE_STATUS: Self = Self(1 << 6);
	pub const COST_PANEL: Self = Self(1 << 7);
	pub const SLOW_MOTION: Self = Self(1 << 8);
	pub const PAUSE_BUTTON_INTERACT: Self = Self(1 << 9);
	pub const SYSTEM_MENU_INTERACT: Self = Self(1 << 10);
	pub const SPEED_SWITCHER_BUTTON_INTERACT: Self = Self(1 << 11);
	pub const UNIT_HUD_SKILL_CAST_MASK: Self = Self(1 << 12);
	pub const WITHDRAWABLE_PANEL: Self = Self(1 << 13);
	pub const COST_PANEL_KEEP_CHARACTERLIMIT: Self = Self(1 << 14);
	pub const CHARACTER_LIMIT: Self = Self(1 << 15);
	pub const AUTOCHESS_SELL_OR_DESTORY: Self = Self(1 << 16);
	pub const CHARACTER_MENU_PANEL: Self = Self(1 << 17);
	pub const ALL: Self = Self((1 << 18) - 1);

	pub fn bits(self) -> u32 {
		self.0
	}

	pub fn is_empty(self) -> bool {
		self == Self::NONE
	}

	pub fn contains(self, other: Self) -> bool {
		self.0 & other.0 == other.0
	}

	fn try_from_raw(raw: RawEnumValue) -> Result<Self, String> {
		match raw {
			RawEnumValue::Name(value) => Self::from_name(&value)
				.ok_or_else(|| format!("unsupported function disable mask name {value:?}")),
			RawEnumValue::Code(value) => {
				let bits: u32 = u32::try_from(value)
					.map_err(|_| format!("expected a non-negative 32-bit mask, found {value}"))?;
				if bits & !Self::ALL.bits() != 0 {
					return Err(format!(
						"mask {bits} contains undefined bits outside {}",
						Self::ALL.bits()
					));
				}
				Ok(Self(bits))
			}
		}
	}

	fn from_name(value: &str) -> Option<Self> {
		match value {
			"NONE" => Some(Self::NONE),
			"CARD_LIST" => Some(Self::CARD_LIST),
			"CHARACTER_MENU" => Some(Self::CHARACTER_MENU),
			"CHARACTER_INFO" => Some(Self::CHARACTER_INFO),
			"SYSTEM_MENU" => Some(Self::SYSTEM_MENU),
			"PAUSE_BUTTON" => Some(Self::PAUSE_BUTTON),
			"SPEED_SWITCHER_BUTTON" => Some(Self::SPEED_SWITCHER_BUTTON),
			"BATTLE_STATUS" => Some(Self::BATTLE_STATUS),
			"COST_PANEL" => Some(Self::COST_PANEL),
			"SLOW_MOTION" => Some(Self::SLOW_MOTION),
			"PAUSE_BUTTON_INTERACT" => Some(Self::PAUSE_BUTTON_INTERACT),
			"SYSTEM_MENU_INTERACT" => Some(Self::SYSTEM_MENU_INTERACT),
			"SPEED_SWITCHER_BUTTON_INTERACT" => Some(Self::SPEED_SWITCHER_BUTTON_INTERACT),
			"UNIT_HUD_SKILL_CAST_MASK" => Some(Self::UNIT_HUD_SKILL_CAST_MASK),
			"WITHDRAWABLE_PANEL" => Some(Self::WITHDRAWABLE_PANEL),
			"COST_PANEL_KEEP_CHARACTERLIMIT" => Some(Self::COST_PANEL_KEEP_CHARACTERLIMIT),
			"CHARACTER_LIMIT" => Some(Self::CHARACTER_LIMIT),
			"AUTOCHESS_SELL_OR_DESTORY" => Some(Self::AUTOCHESS_SELL_OR_DESTORY),
			"CHARACTER_MENU_PANEL" => Some(Self::CHARACTER_MENU_PANEL),
			"ALL" => Some(Self::ALL),
			_ => None,
		}
	}
}

impl<'de> Deserialize<'de> for FunctionDisableMask {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let raw: RawEnumValue = RawEnumValue::deserialize(deserializer)?;
		Self::try_from_raw(raw).map_err(D::Error::custom)
	}
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct StageOptions {
	initial_cost: u32,
	max_cost: u32,
	cost_increase_time: f64,
	move_multiplier: f64,
	character_limit: u32,
	function_disable_mask: FunctionDisableMask,
}

impl StageOptions {
	pub fn initial_cost(&self) -> u32 {
		self.initial_cost
	}

	pub fn max_cost(&self) -> u32 {
		self.max_cost
	}

	pub fn cost_increase_time(&self) -> f64 {
		self.cost_increase_time
	}

	/// Stage-wide scale applied to every actor's move-speed attribute.
	///
	/// The client multiplies the attribute by this option before advancing an actor along its
	/// route, so it is part of the speed rather than a presentation detail.
	pub fn move_multiplier(&self) -> f64 {
		self.move_multiplier
	}

	pub fn character_limit(&self) -> u32 {
		self.character_limit
	}

	pub fn function_disable_mask(&self) -> FunctionDisableMask {
		self.function_disable_mask
	}
}

/// Source collection containing one predeployed stage actor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StageActorCollection {
	Character,
	Token,
}

/// One concrete actor placed by the level before guide actions execute.
///
/// This is deliberately distinct from token cards: a card is only reachable when a strategy
/// deploys it, while a predeployed actor already participates in blocking and combat.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PredeployedStageActor {
	hard: bool,
	collection: StageActorCollection,
	index: u32,
	character_key: String,
	position: GridPosition,
	direction: MapDirection,
	hidden: bool,
	alias: Option<String>,
	elite: u8,
	level: u32,
	potential_rank: u8,
	selected_skill: Option<u8>,
	skill_level: u8,
}

impl PredeployedStageActor {
	pub fn hard(&self) -> bool {
		self.hard
	}

	pub fn collection(&self) -> StageActorCollection {
		self.collection
	}

	pub fn index(&self) -> u32 {
		self.index
	}

	pub fn character_key(&self) -> &str {
		&self.character_key
	}

	pub fn position(&self) -> GridPosition {
		self.position
	}

	pub fn direction(&self) -> &MapDirection {
		&self.direction
	}

	pub fn hidden(&self) -> bool {
		self.hidden
	}

	pub fn alias(&self) -> Option<&str> {
		self.alias.as_deref()
	}

	pub fn elite(&self) -> u8 {
		self.elite
	}

	pub fn level(&self) -> u32 {
		self.level
	}

	pub fn potential_rank(&self) -> u8 {
		self.potential_rank
	}

	pub fn selected_skill(&self) -> Option<u8> {
		self.selected_skill
	}

	pub fn skill_level(&self) -> u8 {
		self.skill_level
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MapDirection {
	Up,
	Right,
	Down,
	Left,
	Invalid,
	Unknown(String),
}

impl MapDirection {
	pub fn stable_id(&self) -> &str {
		match self {
			Self::Up => "UP",
			Self::Right => "RIGHT",
			Self::Down => "DOWN",
			Self::Left => "LEFT",
			Self::Invalid => "INVALID",
			Self::Unknown(value) => value,
		}
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TileHeight {
	Lowland,
	Highland,
	Unknown(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Buildable {
	None,
	Melee,
	Ranged,
	All,
	Unknown(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Passability {
	None,
	WalkOnly,
	FlyOnly,
	All,
	Unknown(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockEdge {
	position: GridPosition,
	direction: MapDirection,
	block_mask: Passability,
}

impl BlockEdge {
	pub fn position(&self) -> GridPosition {
		self.position
	}

	pub fn direction(&self) -> &MapDirection {
		&self.direction
	}

	pub fn block_mask(&self) -> &Passability {
		&self.block_mask
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MotionMode {
	Walk,
	Fly,
	Unknown(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RouteCheckpointKind {
	Move,
	WaitForSeconds,
	WaitForPlayTime,
	WaitCurrentFragmentTime,
	WaitCurrentWaveTime,
	Disappear,
	AppearAtPosition,
	Alert,
	PatrolMove,
	WaitBossRushWave,
	MapOffsetMove,
	Unknown(String),
}

impl RouteCheckpointKind {
	fn has_position(&self) -> bool {
		matches!(
			self,
			Self::Move | Self::AppearAtPosition | Self::PatrolMove | Self::MapOffsetMove
		)
	}

	pub fn mechanic_id(&self) -> &str {
		match self {
			Self::Move => "MOVE",
			Self::WaitForSeconds => "WAIT_FOR_SECONDS",
			Self::WaitForPlayTime => "WAIT_FOR_PLAY_TIME",
			Self::WaitCurrentFragmentTime => "WAIT_CURRENT_FRAGMENT_TIME",
			Self::WaitCurrentWaveTime => "WAIT_CURRENT_WAVE_TIME",
			Self::Disappear => "DISAPPEAR",
			Self::AppearAtPosition => "APPEAR_AT_POS",
			Self::Alert => "ALERT",
			Self::PatrolMove => "PATROL_MOVE",
			Self::WaitBossRushWave => "WAIT_BOSSRUSH_WAVE",
			Self::MapOffsetMove => "MAP_OFFSET_MOVE",
			Self::Unknown(value) => value,
		}
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MapCell {
	position: GridPosition,
	tile_key: String,
	height: TileHeight,
	buildable: Buildable,
	passability: Passability,
}

impl MapCell {
	pub fn position(&self) -> GridPosition {
		self.position
	}

	pub fn tile_key(&self) -> &str {
		&self.tile_key
	}

	pub fn height(&self) -> &TileHeight {
		&self.height
	}

	pub fn buildable(&self) -> &Buildable {
		&self.buildable
	}

	pub fn passability(&self) -> &Passability {
		&self.passability
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct RouteCheckpoint {
	kind: RouteCheckpointKind,
	position: Option<GridPosition>,
	delay_seconds: f64,
	reach_offset: StageVector2,
	randomize_reach_offset: bool,
	reach_distance: f64,
}

impl RouteCheckpoint {
	pub fn kind(&self) -> &RouteCheckpointKind {
		&self.kind
	}

	pub fn position(&self) -> Option<GridPosition> {
		self.position
	}

	pub fn delay_seconds(&self) -> f64 {
		self.delay_seconds
	}

	pub fn reach_offset(&self) -> StageVector2 {
		self.reach_offset
	}

	pub fn randomize_reach_offset(&self) -> bool {
		self.randomize_reach_offset
	}

	pub fn reach_distance(&self) -> f64 {
		self.reach_distance
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct Route {
	motion: MotionMode,
	start: GridPosition,
	end: GridPosition,
	spawn_random_range: StageVector2,
	spawn_offset: StageVector2,
	checkpoints: Vec<RouteCheckpoint>,
	allow_diagonal_move: bool,
	visit_every_tile_center: bool,
	visit_every_node_center: bool,
	visit_every_checkpoint: bool,
}

impl Route {
	pub fn motion(&self) -> &MotionMode {
		&self.motion
	}

	pub fn start(&self) -> GridPosition {
		self.start
	}

	pub fn end(&self) -> GridPosition {
		self.end
	}

	pub fn spawn_random_range(&self) -> StageVector2 {
		self.spawn_random_range
	}

	pub fn spawn_offset(&self) -> StageVector2 {
		self.spawn_offset
	}

	pub fn checkpoints(&self) -> &[RouteCheckpoint] {
		&self.checkpoints
	}

	pub fn allow_diagonal_move(&self) -> bool {
		self.allow_diagonal_move
	}

	pub fn visit_every_tile_center(&self) -> bool {
		self.visit_every_tile_center
	}

	pub fn visit_every_node_center(&self) -> bool {
		self.visit_every_node_center
	}

	pub fn visit_every_checkpoint(&self) -> bool {
		self.visit_every_checkpoint
	}
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum WaveActionContainer {
	Wave { wave: u32, fragment: u32 },
	Branch { branch: String, phase: u32 },
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct WaveActionOrigin {
	pub container: WaveActionContainer,
	pub action: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum StageActionKind {
	Spawn,
	PreviewCursor,
	Story,
	Tutorial,
	PlayBgm,
	DisplayEnemyInfo,
	ActivatePredefined,
	PlayOpera,
	TriggerPredefined,
	BattleEvents,
	WithdrawPredefined,
	Dialog,
	ShowAllHiddenCards,
	Empty,
	EnumCount,
	Unknown(String),
}

impl StageActionKind {
	pub fn is_spawn(&self) -> bool {
		self == &Self::Spawn
	}

	pub fn stable_id(&self) -> &str {
		match self {
			Self::Spawn => "SPAWN",
			Self::PreviewCursor => "PREVIEW_CURSOR",
			Self::Story => "STORY",
			Self::Tutorial => "TUTORIAL",
			Self::PlayBgm => "PLAY_BGM",
			Self::DisplayEnemyInfo => "DISPLAY_ENEMY_INFO",
			Self::ActivatePredefined => "ACTIVATE_PREDEFINED",
			Self::PlayOpera => "PLAY_OPERA",
			Self::TriggerPredefined => "TRIGGER_PREDEFINED",
			Self::BattleEvents => "BATTLE_EVENTS",
			Self::WithdrawPredefined => "WITHDRAW_PREDEFINED",
			Self::Dialog => "DIALOG",
			Self::ShowAllHiddenCards => "SHOW_ALL_HIDDEN_CARDS",
			Self::Empty => "EMPTY",
			Self::EnumCount => "E_NUM",
			Self::Unknown(value) => value,
		}
	}
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum StageActionSchedulePeriod {
	Always,
	PerDay,
	Never,
	PerSettleDay,
	PerSeason,
}

impl StageActionSchedulePeriod {
	pub fn stable_id(self) -> &'static str {
		match self {
			Self::Always => "ALWAYS",
			Self::PerDay => "PER_DAY",
			Self::Never => "NEVER",
			Self::PerSettleDay => "PER_SETTLE_DAY",
			Self::PerSeason => "PER_SEASON",
		}
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct SpawnAction {
	route_index: u32,
	count: u32,
	pre_delay_seconds: f64,
	interval_seconds: f64,
}

impl SpawnAction {
	pub fn route_index(&self) -> u32 {
		self.route_index
	}

	pub fn count(&self) -> u32 {
		self.count
	}

	pub fn pre_delay_seconds(&self) -> f64 {
		self.pre_delay_seconds
	}

	pub fn interval_seconds(&self) -> f64 {
		self.interval_seconds
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct StageAction {
	origin: WaveActionOrigin,
	kind: StageActionKind,
	managed_by_scheduler: bool,
	key: String,
	spawn: Option<SpawnAction>,
	block_fragment: bool,
	auto_preview_route: bool,
	auto_display_enemy_info: bool,
	is_unharmful_and_always_count_as_killed: bool,
	hidden_group: Option<String>,
	random_spawn_group: Option<String>,
	random_spawn_pack: Option<String>,
	random_type: StageActionSchedulePeriod,
	refresh_type: StageActionSchedulePeriod,
	weight: i32,
	dont_block_wave: bool,
	force_block_wave_in_branch: bool,
}

impl StageAction {
	pub fn origin(&self) -> &WaveActionOrigin {
		&self.origin
	}

	pub fn kind(&self) -> &StageActionKind {
		&self.kind
	}

	pub fn managed_by_scheduler(&self) -> bool {
		self.managed_by_scheduler
	}

	pub fn key(&self) -> &str {
		&self.key
	}

	pub fn spawn(&self) -> Option<&SpawnAction> {
		self.spawn.as_ref()
	}

	pub fn block_fragment(&self) -> bool {
		self.block_fragment
	}

	pub fn auto_preview_route(&self) -> bool {
		self.auto_preview_route
	}

	pub fn auto_display_enemy_info(&self) -> bool {
		self.auto_display_enemy_info
	}

	pub fn is_unharmful_and_always_count_as_killed(&self) -> bool {
		self.is_unharmful_and_always_count_as_killed
	}

	pub fn hidden_group(&self) -> Option<&str> {
		self.hidden_group.as_deref()
	}

	pub fn random_spawn_group(&self) -> Option<&str> {
		self.random_spawn_group.as_deref()
	}

	pub fn random_spawn_pack(&self) -> Option<&str> {
		self.random_spawn_pack.as_deref()
	}

	pub fn random_type(&self) -> StageActionSchedulePeriod {
		self.random_type
	}

	pub fn refresh_type(&self) -> StageActionSchedulePeriod {
		self.refresh_type
	}

	pub fn weight(&self) -> i32 {
		self.weight
	}

	pub fn dont_block_wave(&self) -> bool {
		self.dont_block_wave
	}

	pub fn force_block_wave_in_branch(&self) -> bool {
		self.force_block_wave_in_branch
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct StageFragment {
	pre_delay_seconds: f64,
	actions: Vec<StageAction>,
}

impl StageFragment {
	pub fn pre_delay_seconds(&self) -> f64 {
		self.pre_delay_seconds
	}

	pub fn actions(&self) -> &[StageAction] {
		&self.actions
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct StageWave {
	pre_delay_seconds: f64,
	post_delay_seconds: f64,
	max_time_waiting_for_next_wave_seconds: f64,
	advanced_wave_tag: Option<String>,
	fragments: Vec<StageFragment>,
}

impl StageWave {
	pub fn pre_delay_seconds(&self) -> f64 {
		self.pre_delay_seconds
	}

	pub fn post_delay_seconds(&self) -> f64 {
		self.post_delay_seconds
	}

	pub fn max_time_waiting_for_next_wave_seconds(&self) -> f64 {
		self.max_time_waiting_for_next_wave_seconds
	}

	pub fn advanced_wave_tag(&self) -> Option<&str> {
		self.advanced_wave_tag.as_deref()
	}

	pub fn fragments(&self) -> &[StageFragment] {
		&self.fragments
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct StageBranchPhase {
	pre_delay_seconds: f64,
	actions: Vec<StageAction>,
}

impl StageBranchPhase {
	pub fn pre_delay_seconds(&self) -> f64 {
		self.pre_delay_seconds
	}

	pub fn actions(&self) -> &[StageAction] {
		&self.actions
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct StageBranch {
	phases: Vec<StageBranchPhase>,
}

impl StageBranch {
	pub fn phases(&self) -> &[StageBranchPhase] {
		&self.phases
	}
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub struct Spawn {
	origin: WaveActionOrigin,
	enemy: EnemyKey,
	action: SpawnAction,
}

impl Spawn {
	pub fn origin(&self) -> &WaveActionOrigin {
		&self.origin
	}

	pub fn enemy(&self) -> &EnemyKey {
		&self.enemy
	}

	pub fn action(&self) -> &SpawnAction {
		&self.action
	}

	pub fn route_index(&self) -> u32 {
		self.action.route_index()
	}

	pub fn count(&self) -> u32 {
		self.action.count()
	}

	pub fn action_pre_delay_seconds(&self) -> f64 {
		self.action.pre_delay_seconds()
	}

	pub fn interval_seconds(&self) -> f64 {
		self.action.interval_seconds()
	}
}

#[derive(Debug)]
pub(in crate::data) struct StagePayload {
	options: StageOptions,
	dimensions: MapDimensions,
	cells: Vec<MapCell>,
	block_edges: Vec<BlockEdge>,
	predeployed_actors: Vec<PredeployedStageActor>,
	routes: Vec<Route>,
	extra_routes: Vec<Route>,
	spawns: Vec<Spawn>,
	waves: Vec<StageWave>,
	branches: BTreeMap<String, StageBranch>,
}

struct CompiledSchedule {
	waves: Vec<StageWave>,
	branches: BTreeMap<String, StageBranch>,
	spawns: Vec<Spawn>,
}

impl StagePayload {
	pub(in crate::data) fn parse(bytes: &[u8]) -> Result<Self, PayloadError> {
		let raw: RawLevel =
			serde_json::from_slice(bytes).map_err(|source| PayloadError::Parse { source })?;
		let (dimensions, cells, block_edges): (MapDimensions, Vec<MapCell>, Vec<BlockEdge>) =
			compile_map(raw.map_data)?;
		let mut predeployed_actors: Vec<PredeployedStageActor> =
			compile_predeployed_actors(raw.predefines, dimensions, false)?;
		predeployed_actors.extend(compile_predeployed_actors(
			raw.hard_predefines,
			dimensions,
			true,
		)?);
		predeployed_actors.sort();
		let routes: Vec<Route> = compile_routes(raw.routes, dimensions, false)?;
		let extra_routes: Vec<Route> = compile_routes(raw.extra_routes, dimensions, true)?;
		let CompiledSchedule {
			waves,
			branches,
			spawns,
		} = compile_schedule(raw.waves, raw.branches, routes.len())?;

		Ok(Self {
			options: raw.options.into(),
			dimensions,
			cells,
			block_edges,
			predeployed_actors,
			routes,
			extra_routes,
			spawns,
			waves,
			branches,
		})
	}

	pub(in crate::data) fn options(&self) -> &StageOptions {
		&self.options
	}

	pub(in crate::data) fn map_dimensions(&self) -> MapDimensions {
		self.dimensions
	}

	pub(in crate::data) fn cells(&self) -> &[MapCell] {
		&self.cells
	}

	pub(in crate::data) fn cell(&self, position: GridPosition) -> Option<&MapCell> {
		if position.row >= self.dimensions.height || position.col >= self.dimensions.width {
			return None;
		}
		let index: usize =
			position.row as usize * self.dimensions.width as usize + position.col as usize;
		self.cells.get(index)
	}

	pub(in crate::data) fn block_edges(&self) -> &[BlockEdge] {
		&self.block_edges
	}

	pub(in crate::data) fn predeployed_actors(&self) -> &[PredeployedStageActor] {
		&self.predeployed_actors
	}

	pub(in crate::data) fn routes(&self) -> &[Route] {
		&self.routes
	}

	pub(in crate::data) fn extra_routes(&self) -> &[Route] {
		&self.extra_routes
	}

	pub(in crate::data) fn spawns(&self) -> &[Spawn] {
		&self.spawns
	}

	pub(in crate::data) fn waves(&self) -> &[StageWave] {
		&self.waves
	}

	pub(in crate::data) fn branches(&self) -> &BTreeMap<String, StageBranch> {
		&self.branches
	}

	pub(in crate::data) fn actions(&self) -> impl Iterator<Item = &StageAction> {
		let wave_actions = self
			.waves
			.iter()
			.flat_map(|wave: &StageWave| wave.fragments.iter())
			.flat_map(|fragment: &StageFragment| fragment.actions.iter());
		let branch_actions = self
			.branches
			.values()
			.flat_map(|branch: &StageBranch| branch.phases.iter())
			.flat_map(|phase: &StageBranchPhase| phase.actions.iter());
		wave_actions.chain(branch_actions)
	}
}

#[derive(Debug)]
pub(in crate::data) enum PayloadError {
	Parse {
		source: serde_json::Error,
	},
	InvalidMap {
		reason: String,
	},
	InvalidTilePaletteIndex {
		tile_index: i64,
		tile_count: u32,
		position: GridPosition,
	},
	InvalidRoutePosition {
		extra: bool,
		route: u32,
		row: i64,
		col: i64,
		reason: String,
	},
	InvalidRouteCheckpointPosition {
		extra: bool,
		route: u32,
		checkpoint: u32,
		row: i64,
		col: i64,
		reason: String,
	},
	InvalidRouteCheckpointField {
		extra: bool,
		route: u32,
		checkpoint: u32,
		field: &'static str,
		reason: String,
	},
	InvalidSpawnRouteIndex {
		route_index: i64,
		route_count: u32,
		origin: WaveActionOrigin,
	},
	InvalidActionField {
		origin: WaveActionOrigin,
		field: &'static str,
		reason: String,
	},
	InvalidCollectionLength {
		field: &'static str,
		length: usize,
	},
}

impl PayloadError {
	pub(in crate::data) fn into_ark_data_error(self, destination_path: &str) -> ArkDataError {
		let destination_path: String = destination_path.to_owned();
		match self {
			Self::Parse { source } => ArkDataError::LevelParse {
				destination_path,
				source,
			},
			Self::InvalidMap { reason } => ArkDataError::InvalidMap {
				destination_path,
				reason,
			},
			Self::InvalidTilePaletteIndex {
				tile_index,
				tile_count,
				position,
			} => ArkDataError::InvalidTilePaletteIndex {
				destination_path,
				tile_index,
				tile_count,
				position,
			},
			Self::InvalidRoutePosition {
				extra,
				route,
				row,
				col,
				reason,
			} => ArkDataError::InvalidRoutePosition {
				destination_path,
				extra,
				route,
				row,
				col,
				reason,
			},
			Self::InvalidRouteCheckpointPosition {
				extra,
				route,
				checkpoint,
				row,
				col,
				reason,
			} => ArkDataError::InvalidRouteCheckpointPosition {
				destination_path,
				extra,
				route,
				checkpoint,
				row,
				col,
				reason,
			},
			Self::InvalidRouteCheckpointField {
				extra,
				route,
				checkpoint,
				field,
				reason,
			} => ArkDataError::InvalidRouteCheckpointField {
				destination_path,
				extra,
				route,
				checkpoint,
				field,
				reason,
			},
			Self::InvalidSpawnRouteIndex {
				route_index,
				route_count,
				origin,
			} => ArkDataError::InvalidSpawnRouteIndex {
				destination_path,
				route_index,
				route_count,
				origin,
			},
			Self::InvalidActionField {
				origin,
				field,
				reason,
			} => ArkDataError::InvalidActionField {
				destination_path,
				origin,
				field,
				reason,
			},
			Self::InvalidCollectionLength { field, length } => {
				ArkDataError::InvalidLevelCollectionLength {
					destination_path,
					field,
					length,
				}
			}
		}
	}
}

#[derive(Debug, Deserialize)]
struct RawLevel {
	options: RawStageOptions,
	#[serde(rename = "mapData")]
	map_data: RawMapData,
	#[serde(default, deserialize_with = "null_default")]
	predefines: RawActorPredefines,
	#[serde(rename = "hardPredefines", default, deserialize_with = "null_default")]
	hard_predefines: RawActorPredefines,
	#[serde(default, deserialize_with = "null_default")]
	routes: Vec<RawRoute>,
	#[serde(rename = "extraRoutes", default, deserialize_with = "null_default")]
	extra_routes: Vec<RawRoute>,
	#[serde(default, deserialize_with = "null_default")]
	waves: Vec<RawWave>,
	#[serde(default, deserialize_with = "null_default")]
	branches: BTreeMap<String, RawBranch>,
}

#[derive(Debug, Default, Deserialize)]
struct RawActorPredefines {
	#[serde(rename = "characterInsts", default, deserialize_with = "null_default")]
	character_instances: Vec<RawPredeployedActor>,
	#[serde(rename = "tokenInsts", default, deserialize_with = "null_default")]
	token_instances: Vec<RawPredeployedActor>,
}

#[derive(Debug, Deserialize)]
struct RawPredeployedActor {
	position: RawPosition,
	direction: RawEnumValue,
	#[serde(default)]
	hidden: bool,
	#[serde(default)]
	alias: Option<String>,
	inst: RawPredeployedActorBuild,
	#[serde(rename = "skillIndex")]
	skill_index: i32,
	#[serde(rename = "mainSkillLvl")]
	skill_level: u8,
}

#[derive(Debug, Deserialize)]
struct RawPredeployedActorBuild {
	#[serde(rename = "characterKey")]
	character_key: String,
	level: u32,
	phase: RawEnumValue,
	#[serde(rename = "potentialRank")]
	potential_rank: u8,
}

#[derive(Debug, Deserialize)]
struct RawStageOptions {
	#[serde(
		rename = "initialCost",
		deserialize_with = "deserialize_non_negative_i32"
	)]
	initial_cost: u32,
	#[serde(rename = "maxCost", deserialize_with = "deserialize_non_negative_i32")]
	max_cost: u32,
	#[serde(
		rename = "costIncreaseTime",
		deserialize_with = "deserialize_positive_finite_f64"
	)]
	cost_increase_time: f64,
	#[serde(
		rename = "moveMultiplier",
		deserialize_with = "deserialize_positive_finite_f64"
	)]
	move_multiplier: f64,
	#[serde(
		rename = "characterLimit",
		deserialize_with = "deserialize_non_negative_i32"
	)]
	character_limit: u32,
	#[serde(rename = "functionDisableMask")]
	function_disable_mask: FunctionDisableMask,
}

impl From<RawStageOptions> for StageOptions {
	fn from(raw: RawStageOptions) -> Self {
		Self {
			initial_cost: raw.initial_cost,
			max_cost: raw.max_cost,
			cost_increase_time: raw.cost_increase_time,
			move_multiplier: raw.move_multiplier,
			character_limit: raw.character_limit,
			function_disable_mask: raw.function_disable_mask,
		}
	}
}

#[derive(Debug, Deserialize)]
struct RawMapData {
	map: Vec<Vec<i64>>,
	tiles: Vec<RawTile>,
	#[serde(rename = "blockEdges", default, deserialize_with = "null_default")]
	block_edges: Vec<RawBlockEdge>,
}

#[derive(Debug, Deserialize)]
struct RawBlockEdge {
	pos: RawPosition,
	direction: RawEnumValue,
	#[serde(rename = "blockMask")]
	block_mask: RawEnumValue,
}

#[derive(Debug, Deserialize)]
struct RawTile {
	#[serde(rename = "tileKey")]
	tile_key: String,
	#[serde(rename = "heightType")]
	height: RawEnumValue,
	#[serde(rename = "buildableType")]
	buildable: RawEnumValue,
	#[serde(rename = "passableMask")]
	passability: RawEnumValue,
}

#[derive(Debug, Deserialize)]
struct RawRoute {
	#[serde(rename = "motionMode")]
	motion: RawEnumValue,
	#[serde(rename = "startPosition")]
	start: RawPosition,
	#[serde(rename = "endPosition")]
	end: RawPosition,
	#[serde(rename = "spawnRandomRange", default)]
	spawn_random_range: Option<RawVector2>,
	#[serde(rename = "spawnOffset", default)]
	spawn_offset: Option<RawVector2>,
	#[serde(default, deserialize_with = "null_default")]
	checkpoints: Vec<RawRouteCheckpoint>,
	#[serde(rename = "allowDiagonalMove", default)]
	allow_diagonal_move: bool,
	#[serde(rename = "visitEveryTileCenter", default)]
	visit_every_tile_center: bool,
	#[serde(rename = "visitEveryNodeCenter", default)]
	visit_every_node_center: bool,
	#[serde(rename = "visitEveryCheckPoint", default)]
	visit_every_checkpoint: bool,
}

#[derive(Debug, Deserialize)]
struct RawRouteCheckpoint {
	#[serde(rename = "type")]
	kind: RawEnumValue,
	time: f64,
	#[serde(default)]
	position: Option<RawPosition>,
	#[serde(rename = "reachOffset", default)]
	reach_offset: Option<RawVector2>,
	#[serde(rename = "randomizeReachOffset", default)]
	randomize_reach_offset: bool,
	#[serde(rename = "reachDistance", default)]
	reach_distance: f64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct RawPosition {
	row: i64,
	col: i64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct RawVector2 {
	x: f64,
	y: f64,
}

#[derive(Debug, Deserialize)]
struct RawWave {
	#[serde(rename = "preDelay", default)]
	pre_delay: f64,
	#[serde(rename = "postDelay", default)]
	post_delay: f64,
	#[serde(rename = "maxTimeWaitingForNextWave", default)]
	max_time_waiting_for_next_wave: f64,
	#[serde(default, deserialize_with = "null_default")]
	fragments: Vec<RawFragment>,
	#[serde(rename = "advancedWaveTag", default)]
	advanced_wave_tag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFragment {
	#[serde(rename = "preDelay", default)]
	pre_delay: f64,
	#[serde(default, deserialize_with = "null_default")]
	actions: Vec<RawAction>,
}

#[derive(Debug, Deserialize)]
struct RawBranch {
	#[serde(default, deserialize_with = "null_default")]
	phases: Vec<RawPhase>,
}

#[derive(Debug, Deserialize)]
struct RawPhase {
	#[serde(rename = "preDelay", default)]
	pre_delay: f64,
	#[serde(default, deserialize_with = "null_default")]
	actions: Vec<RawAction>,
}

#[derive(Debug, Deserialize)]
struct RawAction {
	#[serde(rename = "actionType")]
	action_type: RawEnumValue,
	#[serde(rename = "managedByScheduler", default)]
	managed_by_scheduler: bool,
	#[serde(default)]
	key: Option<String>,
	#[serde(rename = "routeIndex", default)]
	route_index: Option<i64>,
	#[serde(default)]
	count: Option<i64>,
	#[serde(rename = "preDelay", default)]
	pre_delay: Option<f64>,
	#[serde(default)]
	interval: Option<f64>,
	#[serde(rename = "blockFragment", default)]
	block_fragment: bool,
	#[serde(rename = "autoPreviewRoute", default)]
	auto_preview_route: bool,
	#[serde(rename = "autoDisplayEnemyInfo", default)]
	auto_display_enemy_info: bool,
	#[serde(rename = "isUnharmfulAndAlwaysCountAsKilled", default)]
	is_unharmful_and_always_count_as_killed: bool,
	#[serde(rename = "randomSpawnGroupKey", default)]
	random_spawn_group: Option<String>,
	#[serde(rename = "randomSpawnGroupPackKey", default)]
	random_spawn_pack: Option<String>,
	#[serde(rename = "hiddenGroup", default)]
	hidden_group: Option<String>,
	#[serde(rename = "randomType", default = "raw_enum_zero")]
	random_type: RawEnumValue,
	#[serde(rename = "refreshType", default = "raw_enum_zero")]
	refresh_type: RawEnumValue,
	#[serde(default)]
	weight: i64,
	#[serde(rename = "dontBlockWave", default)]
	dont_block_wave: bool,
	#[serde(rename = "forceBlockWaveInBranch", default)]
	force_block_wave_in_branch: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum RawEnumValue {
	Name(String),
	Code(i64),
}

fn raw_enum_zero() -> RawEnumValue {
	RawEnumValue::Code(0)
}

fn deserialize_non_negative_i32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
	D: Deserializer<'de>,
{
	let value: i64 = i64::deserialize(deserializer)?;
	let value: i32 = i32::try_from(value)
		.map_err(|_| D::Error::custom(format!("expected a 32-bit integer, found {value}")))?;
	u32::try_from(value)
		.map_err(|_| D::Error::custom(format!("expected a non-negative integer, found {value}")))
}

fn deserialize_positive_finite_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
	D: Deserializer<'de>,
{
	let value: f64 = f64::deserialize(deserializer)?;
	let source_value: f32 = value as f32;
	if !value.is_finite() || value <= 0.0 || !source_value.is_finite() || source_value == 0.0 {
		return Err(D::Error::custom(format!(
			"expected a positive finite 32-bit source number, found {value}"
		)));
	}
	Ok(value)
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de> + Default,
{
	Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn compile_map(
	raw: RawMapData,
) -> Result<(MapDimensions, Vec<MapCell>, Vec<BlockEdge>), PayloadError> {
	let RawMapData {
		map,
		tiles,
		block_edges: raw_block_edges,
	} = raw;
	if map.is_empty() {
		return Err(PayloadError::InvalidMap {
			reason: "map matrix must contain at least one row".to_owned(),
		});
	}
	let width: usize = map[0].len();
	if width == 0 {
		return Err(PayloadError::InvalidMap {
			reason: "map matrix rows must not be empty".to_owned(),
		});
	}
	if map.iter().any(|row: &Vec<i64>| row.len() != width) {
		return Err(PayloadError::InvalidMap {
			reason: "map matrix must be rectangular".to_owned(),
		});
	}
	let width: u32 = u32::try_from(width).map_err(|_| PayloadError::InvalidMap {
		reason: "map width exceeds u32".to_owned(),
	})?;
	let height: u32 = u32::try_from(map.len()).map_err(|_| PayloadError::InvalidMap {
		reason: "map height exceeds u32".to_owned(),
	})?;
	let tile_count: u32 = u32::try_from(tiles.len()).map_err(|_| PayloadError::InvalidMap {
		reason: "tile palette length exceeds u32".to_owned(),
	})?;
	let dimensions: MapDimensions = MapDimensions { width, height };
	let mut cells: Vec<MapCell> = Vec::with_capacity(width as usize * height as usize);
	for logical_row in 0..height {
		let source_row: usize = (height - 1 - logical_row) as usize;
		for col in 0..width {
			let position: GridPosition = GridPosition {
				row: logical_row,
				col,
			};
			let tile_index: i64 = map[source_row][col as usize];
			let tile_index_usize: usize =
				usize::try_from(tile_index).map_err(|_| PayloadError::InvalidTilePaletteIndex {
					tile_index,
					tile_count,
					position,
				})?;
			let tile: &RawTile =
				tiles
					.get(tile_index_usize)
					.ok_or(PayloadError::InvalidTilePaletteIndex {
						tile_index,
						tile_count,
						position,
					})?;
			if tile.tile_key.is_empty() {
				return Err(PayloadError::InvalidMap {
					reason: format!("tile palette entry {tile_index} has an empty tileKey"),
				});
			}
			cells.push(MapCell {
				position,
				tile_key: tile.tile_key.clone(),
				height: TileHeight::from_raw(&tile.height),
				buildable: Buildable::from_raw(&tile.buildable),
				passability: Passability::from_raw(&tile.passability),
			});
		}
	}
	let block_edges: Vec<BlockEdge> = raw_block_edges
		.into_iter()
		.enumerate()
		.map(
			|(edge, raw): (usize, RawBlockEdge)| -> Result<BlockEdge, PayloadError> {
				let position: GridPosition =
					checked_position(raw.pos, dimensions).map_err(|reason| {
						PayloadError::InvalidMap {
							reason: format!(
								"blockEdges[{edge}] position ({}, {}) is invalid: {reason}",
								raw.pos.row, raw.pos.col
							),
						}
					})?;
				Ok(BlockEdge {
					position,
					direction: MapDirection::from_raw(&raw.direction),
					block_mask: Passability::from_raw(&raw.block_mask),
				})
			},
		)
		.collect::<Result<_, _>>()?;
	Ok((dimensions, cells, block_edges))
}

fn compile_routes(
	raw_routes: Vec<RawRoute>,
	dimensions: MapDimensions,
	extra: bool,
) -> Result<Vec<Route>, PayloadError> {
	if raw_routes.len() > u32::MAX as usize {
		return Err(PayloadError::InvalidCollectionLength {
			field: if extra { "extraRoutes" } else { "routes" },
			length: raw_routes.len(),
		});
	}
	raw_routes
		.into_iter()
		.enumerate()
		.map(|(route, raw): (usize, RawRoute)| compile_route(raw, dimensions, extra, route as u32))
		.collect()
}

fn compile_route(
	raw: RawRoute,
	dimensions: MapDimensions,
	extra: bool,
	route: u32,
) -> Result<Route, PayloadError> {
	let RawRoute {
		motion,
		start: raw_start,
		end: raw_end,
		spawn_random_range,
		spawn_offset,
		checkpoints: raw_checkpoints,
		allow_diagonal_move,
		visit_every_tile_center,
		visit_every_node_center,
		visit_every_checkpoint,
	} = raw;
	let start: GridPosition = checked_position(raw_start, dimensions).map_err(|reason| {
		PayloadError::InvalidRoutePosition {
			extra,
			route,
			row: raw_start.row,
			col: raw_start.col,
			reason: format!("invalid start position: {reason}"),
		}
	})?;
	let end: GridPosition = checked_position(raw_end, dimensions).map_err(|reason| {
		PayloadError::InvalidRoutePosition {
			extra,
			route,
			row: raw_end.row,
			col: raw_end.col,
			reason: format!("invalid end position: {reason}"),
		}
	})?;
	if raw_checkpoints.len() > u32::MAX as usize {
		return Err(PayloadError::InvalidCollectionLength {
			field: "route checkpoints",
			length: raw_checkpoints.len(),
		});
	}
	let checkpoints: Vec<RouteCheckpoint> = raw_checkpoints
		.into_iter()
		.enumerate()
		.map(|(checkpoint, raw): (usize, RawRouteCheckpoint)| {
			compile_checkpoint(raw, dimensions, extra, route, checkpoint as u32)
		})
		.collect::<Result<_, _>>()?;
	Ok(Route {
		motion: MotionMode::from_raw(&motion),
		start,
		end,
		spawn_random_range: StageVector2::from_raw(spawn_random_range.unwrap_or_default()),
		spawn_offset: StageVector2::from_raw(spawn_offset.unwrap_or_default()),
		checkpoints,
		allow_diagonal_move,
		visit_every_tile_center,
		visit_every_node_center,
		visit_every_checkpoint,
	})
}

fn compile_checkpoint(
	raw: RawRouteCheckpoint,
	dimensions: MapDimensions,
	extra: bool,
	route: u32,
	checkpoint: u32,
) -> Result<RouteCheckpoint, PayloadError> {
	if !raw.time.is_finite() {
		return Err(PayloadError::InvalidRouteCheckpointField {
			extra,
			route,
			checkpoint,
			field: "time",
			reason: "value must be finite".to_owned(),
		});
	}
	if !raw.reach_distance.is_finite() {
		return Err(PayloadError::InvalidRouteCheckpointField {
			extra,
			route,
			checkpoint,
			field: "reachDistance",
			reason: "value must be finite".to_owned(),
		});
	}
	let reach_offset: StageVector2 = StageVector2::from_raw(raw.reach_offset.unwrap_or_default());
	if !reach_offset.x.is_finite() || !reach_offset.y.is_finite() {
		return Err(PayloadError::InvalidRouteCheckpointField {
			extra,
			route,
			checkpoint,
			field: "reachOffset",
			reason: "x and y must be finite".to_owned(),
		});
	}
	let kind: RouteCheckpointKind = RouteCheckpointKind::from_raw(&raw.kind);
	let position: Option<GridPosition> = if kind.has_position() {
		let raw_position: RawPosition = raw.position.unwrap_or(RawPosition { row: -1, col: -1 });
		Some(
			checked_position(raw_position, dimensions).map_err(|reason| {
				PayloadError::InvalidRouteCheckpointPosition {
					extra,
					route,
					checkpoint,
					row: raw_position.row,
					col: raw_position.col,
					reason,
				}
			})?,
		)
	} else {
		None
	};
	Ok(RouteCheckpoint {
		kind,
		position,
		delay_seconds: raw.time,
		reach_offset,
		randomize_reach_offset: raw.randomize_reach_offset,
		reach_distance: raw.reach_distance,
	})
}

fn checked_position(raw: RawPosition, dimensions: MapDimensions) -> Result<GridPosition, String> {
	let row: u32 = u32::try_from(raw.row).map_err(|_| "row must be nonnegative".to_owned())?;
	let col: u32 = u32::try_from(raw.col).map_err(|_| "column must be nonnegative".to_owned())?;
	if row >= dimensions.height || col >= dimensions.width {
		return Err(format!(
			"position ({row}, {col}) is outside {}x{} map",
			dimensions.height, dimensions.width
		));
	}
	Ok(GridPosition { row, col })
}

fn compile_predeployed_actors(
	raw: RawActorPredefines,
	dimensions: MapDimensions,
	hard: bool,
) -> Result<Vec<PredeployedStageActor>, PayloadError> {
	let mut actors: Vec<PredeployedStageActor> = Vec::new();
	for (collection, entries) in [
		(StageActorCollection::Character, raw.character_instances),
		(StageActorCollection::Token, raw.token_instances),
	] {
		for (index, entry) in entries.into_iter().enumerate() {
			let index: u32 = checked_index("predeployed actors", index)?;
			let position: GridPosition =
				checked_position(entry.position, dimensions).map_err(|reason| {
					PayloadError::InvalidMap {
						reason: format!(
							"predeployed actor {collection:?} {index} has invalid position: {reason}"
						),
					}
				})?;
			let character_key: String = entry.inst.character_key;
			if character_key.trim().is_empty() {
				return Err(PayloadError::InvalidMap {
					reason: format!(
						"predeployed actor {collection:?} {index} has a blank character key"
					),
				});
			}
			if entry.inst.level == 0 {
				return Err(PayloadError::InvalidMap {
					reason: format!("predeployed actor {collection:?} {index} has level zero"),
				});
			}
			let elite: u8 =
				elite_from_raw(entry.inst.phase).ok_or_else(|| PayloadError::InvalidMap {
					reason: format!(
						"predeployed actor {collection:?} {index} has an unsupported phase"
					),
				})?;
			let potential_rank: u8 = entry
				.inst
				.potential_rank
				.checked_add(1)
				.filter(|rank| (1..=6).contains(rank))
				.ok_or_else(|| PayloadError::InvalidMap {
					reason: format!(
						"predeployed actor {collection:?} {index} has invalid potential index {}",
						entry.inst.potential_rank
					),
				})?;
			let selected_skill: Option<u8> = match entry.skill_index {
				-1 => None,
				value if value >= 0 => u8::try_from(value)
					.ok()
					.and_then(|value| value.checked_add(1)),
				_ => None,
			};
			if entry.skill_index < -1 || (entry.skill_index >= 0 && selected_skill.is_none()) {
				return Err(PayloadError::InvalidMap {
					reason: format!(
						"predeployed actor {collection:?} {index} has invalid skill index {}",
						entry.skill_index
					),
				});
			}
			actors.push(PredeployedStageActor {
				hard,
				collection,
				index,
				character_key,
				position,
				direction: MapDirection::from_raw(&entry.direction),
				hidden: entry.hidden,
				alias: entry.alias,
				elite,
				level: entry.inst.level,
				potential_rank,
				selected_skill,
				skill_level: entry.skill_level,
			});
		}
	}
	Ok(actors)
}

fn elite_from_raw(raw: RawEnumValue) -> Option<u8> {
	match raw {
		RawEnumValue::Name(value) => value
			.strip_prefix("PHASE_")
			.and_then(|value| value.parse::<u8>().ok()),
		RawEnumValue::Code(value) => u8::try_from(value).ok(),
	}
}

fn compile_schedule(
	waves: Vec<RawWave>,
	branches: BTreeMap<String, RawBranch>,
	route_count: usize,
) -> Result<CompiledSchedule, PayloadError> {
	let route_count: u32 =
		u32::try_from(route_count).map_err(|_| PayloadError::InvalidCollectionLength {
			field: "routes",
			length: route_count,
		})?;
	let mut spawns: Vec<Spawn> = Vec::new();
	let mut compiled_waves: Vec<StageWave> = Vec::with_capacity(waves.len());
	for (wave, raw_wave) in waves.into_iter().enumerate() {
		let wave: u32 = checked_index("waves", wave)?;
		let RawWave {
			pre_delay,
			post_delay,
			max_time_waiting_for_next_wave,
			fragments: raw_fragments,
			advanced_wave_tag,
		} = raw_wave;
		let mut fragments: Vec<StageFragment> = Vec::with_capacity(raw_fragments.len());
		for (fragment, raw_fragment) in raw_fragments.into_iter().enumerate() {
			let fragment: u32 = checked_index("wave fragments", fragment)?;
			let actions: Vec<StageAction> = compile_action_list(
				raw_fragment.actions,
				WaveActionContainer::Wave { wave, fragment },
				route_count,
				&mut spawns,
			)?;
			fragments.push(StageFragment {
				pre_delay_seconds: raw_fragment.pre_delay,
				actions,
			});
		}
		compiled_waves.push(StageWave {
			pre_delay_seconds: pre_delay,
			post_delay_seconds: post_delay,
			max_time_waiting_for_next_wave_seconds: max_time_waiting_for_next_wave,
			advanced_wave_tag,
			fragments,
		});
	}
	let mut compiled_branches: BTreeMap<String, StageBranch> = BTreeMap::new();
	for (branch, raw_branch) in branches {
		let mut phases: Vec<StageBranchPhase> = Vec::with_capacity(raw_branch.phases.len());
		for (phase, raw_phase) in raw_branch.phases.into_iter().enumerate() {
			let phase: u32 = checked_index("branch phases", phase)?;
			let actions: Vec<StageAction> = compile_action_list(
				raw_phase.actions,
				WaveActionContainer::Branch {
					branch: branch.clone(),
					phase,
				},
				route_count,
				&mut spawns,
			)?;
			phases.push(StageBranchPhase {
				pre_delay_seconds: raw_phase.pre_delay,
				actions,
			});
		}
		compiled_branches.insert(branch, StageBranch { phases });
	}
	Ok(CompiledSchedule {
		waves: compiled_waves,
		branches: compiled_branches,
		spawns,
	})
}

fn compile_action_list(
	raw_actions: Vec<RawAction>,
	container: WaveActionContainer,
	route_count: u32,
	spawns: &mut Vec<Spawn>,
) -> Result<Vec<StageAction>, PayloadError> {
	let mut actions: Vec<StageAction> = Vec::with_capacity(raw_actions.len());
	for (action, raw) in raw_actions.into_iter().enumerate() {
		let origin: WaveActionOrigin = WaveActionOrigin {
			container: container.clone(),
			action: checked_index("actions", action)?,
		};
		let kind: StageActionKind = StageActionKind::from_raw(&raw.action_type);
		let random_type: StageActionSchedulePeriod =
			StageActionSchedulePeriod::try_from_raw(&raw.random_type, &origin, "randomType")?;
		let refresh_type: StageActionSchedulePeriod =
			StageActionSchedulePeriod::try_from_raw(&raw.refresh_type, &origin, "refreshType")?;
		let weight: i32 =
			i32::try_from(raw.weight).map_err(|_| PayloadError::InvalidActionField {
				origin: origin.clone(),
				field: "weight",
				reason: format!("expected an i32, found {}", raw.weight),
			})?;
		let key: String = raw.key.unwrap_or_default();
		let spawn: Option<SpawnAction> = if kind.is_spawn() {
			let raw_route_index: i64 =
				required_action_field(raw.route_index, &origin, "routeIndex")?;
			let checked_route_index: u32 = u32::try_from(raw_route_index).map_err(|_| {
				PayloadError::InvalidSpawnRouteIndex {
					route_index: raw_route_index,
					route_count,
					origin: origin.clone(),
				}
			})?;
			if checked_route_index >= route_count {
				return Err(PayloadError::InvalidSpawnRouteIndex {
					route_index: raw_route_index,
					route_count,
					origin,
				});
			}
			let raw_count: i64 = required_action_field(raw.count, &origin, "count")?;
			let checked_count: u32 =
				u32::try_from(raw_count).map_err(|_| PayloadError::InvalidActionField {
					origin: origin.clone(),
					field: "count",
					reason: format!("expected a nonnegative u32, found {raw_count}"),
				})?;
			let checked_pre_delay: f64 =
				required_finite_action_field(raw.pre_delay, &origin, "preDelay")?;
			let checked_interval: f64 =
				required_finite_action_field(raw.interval, &origin, "interval")?;
			let spawn: SpawnAction = SpawnAction {
				route_index: checked_route_index,
				count: checked_count,
				pre_delay_seconds: checked_pre_delay,
				interval_seconds: checked_interval,
			};
			if !key.is_empty() {
				let enemy: EnemyKey =
					EnemyKey::new(key.clone()).map_err(|_| PayloadError::InvalidActionField {
						origin: origin.clone(),
						field: "key",
						reason: "direct SPAWN key must be a valid enemy identity".to_owned(),
					})?;
				spawns.push(Spawn {
					origin: origin.clone(),
					enemy,
					action: spawn.clone(),
				});
			}
			Some(spawn)
		} else {
			None
		};
		actions.push(StageAction {
			origin,
			kind,
			managed_by_scheduler: raw.managed_by_scheduler,
			key,
			spawn,
			block_fragment: raw.block_fragment,
			auto_preview_route: raw.auto_preview_route,
			auto_display_enemy_info: raw.auto_display_enemy_info,
			is_unharmful_and_always_count_as_killed: raw.is_unharmful_and_always_count_as_killed,
			random_spawn_group: raw.random_spawn_group,
			random_spawn_pack: raw.random_spawn_pack,
			hidden_group: raw.hidden_group,
			random_type,
			refresh_type,
			weight,
			dont_block_wave: raw.dont_block_wave,
			force_block_wave_in_branch: raw.force_block_wave_in_branch,
		});
	}
	Ok(actions)
}

fn required_action_field<T: Copy>(
	value: Option<T>,
	origin: &WaveActionOrigin,
	field: &'static str,
) -> Result<T, PayloadError> {
	value.ok_or_else(|| PayloadError::InvalidActionField {
		origin: origin.clone(),
		field,
		reason: "field is required for SPAWN".to_owned(),
	})
}

fn required_finite_action_field(
	value: Option<f64>,
	origin: &WaveActionOrigin,
	field: &'static str,
) -> Result<f64, PayloadError> {
	let value: f64 = required_action_field(value, origin, field)?;
	if !value.is_finite() {
		return Err(PayloadError::InvalidActionField {
			origin: origin.clone(),
			field,
			reason: "value must be finite".to_owned(),
		});
	}
	Ok(value)
}

fn checked_index(field: &'static str, index: usize) -> Result<u32, PayloadError> {
	u32::try_from(index).map_err(|_| PayloadError::InvalidCollectionLength {
		field,
		length: index.saturating_add(1),
	})
}

impl StageVector2 {
	fn from_raw(raw: RawVector2) -> Self {
		Self { x: raw.x, y: raw.y }
	}
}

impl MapDirection {
	fn from_raw(raw: &RawEnumValue) -> Self {
		match raw {
			RawEnumValue::Name(value) if value == "UP" => Self::Up,
			RawEnumValue::Name(value) if value == "RIGHT" => Self::Right,
			RawEnumValue::Name(value) if value == "DOWN" => Self::Down,
			RawEnumValue::Name(value) if value == "LEFT" => Self::Left,
			RawEnumValue::Name(value) if value == "E_NUM" || value == "INVALID" => Self::Invalid,
			RawEnumValue::Code(0) => Self::Up,
			RawEnumValue::Code(1) => Self::Right,
			RawEnumValue::Code(2) => Self::Down,
			RawEnumValue::Code(3) => Self::Left,
			RawEnumValue::Code(4) => Self::Invalid,
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		}
	}
}

impl TileHeight {
	fn from_raw(raw: &RawEnumValue) -> Self {
		match raw {
			RawEnumValue::Name(value) if value == "LOWLAND" => Self::Lowland,
			RawEnumValue::Name(value) if value == "HIGHLAND" => Self::Highland,
			RawEnumValue::Code(0) => Self::Lowland,
			RawEnumValue::Code(1) => Self::Highland,
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		}
	}
}

impl Buildable {
	fn from_raw(raw: &RawEnumValue) -> Self {
		match raw {
			RawEnumValue::Name(value) if value == "NONE" => Self::None,
			RawEnumValue::Name(value) if value == "MELEE" => Self::Melee,
			RawEnumValue::Name(value) if value == "RANGED" => Self::Ranged,
			RawEnumValue::Name(value) if value == "ALL" => Self::All,
			RawEnumValue::Code(0) => Self::None,
			RawEnumValue::Code(1) => Self::Melee,
			RawEnumValue::Code(2) => Self::Ranged,
			RawEnumValue::Code(3) => Self::All,
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		}
	}
}

impl Passability {
	fn from_raw(raw: &RawEnumValue) -> Self {
		match raw {
			RawEnumValue::Name(value) if value == "NONE" => Self::None,
			RawEnumValue::Name(value) if value == "WALK_ONLY" => Self::WalkOnly,
			RawEnumValue::Name(value) if value == "FLY_ONLY" => Self::FlyOnly,
			RawEnumValue::Name(value) if value == "ALL" => Self::All,
			RawEnumValue::Code(0) => Self::None,
			RawEnumValue::Code(1) => Self::WalkOnly,
			RawEnumValue::Code(2) => Self::FlyOnly,
			RawEnumValue::Code(3) => Self::All,
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		}
	}
}

impl MotionMode {
	fn from_raw(raw: &RawEnumValue) -> Self {
		match raw {
			RawEnumValue::Name(value) if value == "WALK" => Self::Walk,
			RawEnumValue::Name(value) if value == "FLY" => Self::Fly,
			RawEnumValue::Code(0) => Self::Walk,
			RawEnumValue::Code(1) => Self::Fly,
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		}
	}
}

impl RouteCheckpointKind {
	fn from_raw(raw: &RawEnumValue) -> Self {
		match raw {
			RawEnumValue::Name(value) if value == "MOVE" => Self::Move,
			RawEnumValue::Name(value) if value == "WAIT_FOR_SECONDS" => Self::WaitForSeconds,
			RawEnumValue::Name(value) if value == "WAIT_FOR_PLAY_TIME" => Self::WaitForPlayTime,
			RawEnumValue::Name(value) if value == "WAIT_CURRENT_FRAGMENT_TIME" => {
				Self::WaitCurrentFragmentTime
			}
			RawEnumValue::Name(value) if value == "WAIT_CURRENT_WAVE_TIME" => {
				Self::WaitCurrentWaveTime
			}
			RawEnumValue::Name(value) if value == "DISAPPEAR" => Self::Disappear,
			RawEnumValue::Name(value) if value == "APPEAR_AT_POS" => Self::AppearAtPosition,
			RawEnumValue::Name(value) if value == "ALERT" => Self::Alert,
			RawEnumValue::Name(value) if value == "PATROL_MOVE" => Self::PatrolMove,
			RawEnumValue::Name(value) if value == "WAIT_BOSSRUSH_WAVE" => Self::WaitBossRushWave,
			RawEnumValue::Name(value) if value == "MAP_OFFSET_MOVE" => Self::MapOffsetMove,
			RawEnumValue::Code(0) => Self::Move,
			RawEnumValue::Code(1) => Self::WaitForSeconds,
			RawEnumValue::Code(2) => Self::WaitForPlayTime,
			RawEnumValue::Code(3) => Self::WaitCurrentFragmentTime,
			RawEnumValue::Code(4) => Self::WaitCurrentWaveTime,
			RawEnumValue::Code(5) => Self::Disappear,
			RawEnumValue::Code(6) => Self::AppearAtPosition,
			RawEnumValue::Code(7) => Self::Alert,
			RawEnumValue::Code(8) => Self::PatrolMove,
			RawEnumValue::Code(9) => Self::WaitBossRushWave,
			RawEnumValue::Code(10) => Self::MapOffsetMove,
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		}
	}
}

impl StageActionKind {
	fn from_raw(raw: &RawEnumValue) -> Self {
		let kind: Option<Self> = match raw {
			RawEnumValue::Name(value) => Self::from_name(value),
			RawEnumValue::Code(value) => Self::from_code(*value),
		};
		kind.unwrap_or_else(|| match raw {
			RawEnumValue::Name(value) => Self::Unknown(format!("name:{value}")),
			RawEnumValue::Code(value) => Self::Unknown(format!("numeric:{value}")),
		})
	}

	fn from_name(value: &str) -> Option<Self> {
		match value {
			"SPAWN" => Some(Self::Spawn),
			"PREVIEW_CURSOR" => Some(Self::PreviewCursor),
			"STORY" => Some(Self::Story),
			"TUTORIAL" => Some(Self::Tutorial),
			"PLAY_BGM" => Some(Self::PlayBgm),
			"DISPLAY_ENEMY_INFO" => Some(Self::DisplayEnemyInfo),
			"ACTIVATE_PREDEFINED" => Some(Self::ActivatePredefined),
			"PLAY_OPERA" => Some(Self::PlayOpera),
			"TRIGGER_PREDEFINED" => Some(Self::TriggerPredefined),
			"BATTLE_EVENTS" => Some(Self::BattleEvents),
			"WITHDRAW_PREDEFINED" => Some(Self::WithdrawPredefined),
			"DIALOG" => Some(Self::Dialog),
			"SHOW_ALL_HIDDEN_CARDS" => Some(Self::ShowAllHiddenCards),
			"EMPTY" => Some(Self::Empty),
			"E_NUM" => Some(Self::EnumCount),
			_ => None,
		}
	}

	fn from_code(value: i64) -> Option<Self> {
		match value {
			0 => Some(Self::Spawn),
			1 => Some(Self::PreviewCursor),
			2 => Some(Self::Story),
			3 => Some(Self::Tutorial),
			4 => Some(Self::PlayBgm),
			5 => Some(Self::DisplayEnemyInfo),
			6 => Some(Self::ActivatePredefined),
			7 => Some(Self::PlayOpera),
			8 => Some(Self::TriggerPredefined),
			9 => Some(Self::BattleEvents),
			10 => Some(Self::WithdrawPredefined),
			11 => Some(Self::Dialog),
			12 => Some(Self::ShowAllHiddenCards),
			13 => Some(Self::Empty),
			14 => Some(Self::EnumCount),
			_ => None,
		}
	}
}

impl StageActionSchedulePeriod {
	fn try_from_raw(
		raw: &RawEnumValue,
		origin: &WaveActionOrigin,
		field: &'static str,
	) -> Result<Self, PayloadError> {
		let period: Option<Self> = match raw {
			RawEnumValue::Name(value) => match value.as_str() {
				"ALWAYS" => Some(Self::Always),
				"PER_DAY" => Some(Self::PerDay),
				"NEVER" => Some(Self::Never),
				"PER_SETTLE_DAY" => Some(Self::PerSettleDay),
				"PER_SEASON" => Some(Self::PerSeason),
				_ => None,
			},
			RawEnumValue::Code(value) => match value {
				0 => Some(Self::Always),
				1 => Some(Self::PerDay),
				2 => Some(Self::Never),
				3 => Some(Self::PerSettleDay),
				4 => Some(Self::PerSeason),
				_ => None,
			},
		};
		period.ok_or_else(|| PayloadError::InvalidActionField {
			origin: origin.clone(),
			field,
			reason: format!("unsupported scheduling enum {}", describe_raw_enum(raw)),
		})
	}
}

fn describe_raw_enum(raw: &RawEnumValue) -> String {
	match raw {
		RawEnumValue::Name(value) => format!("name {value:?}"),
		RawEnumValue::Code(value) => format!("numeric code {value}"),
	}
}
