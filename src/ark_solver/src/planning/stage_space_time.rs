use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::data::{
	CompiledStage, EnemyKey, GridPosition, MechanicReference, MotionMode, StageAction,
	StageActionSchedulePeriod, WaveActionContainer,
};
use crate::mechanics::stage::{
	compile_route_timeline, ticks_from_seconds_ceil as canonical_ticks_from_seconds_ceil,
	RouteTimeline, RouteTimelineAction, SpawnCohortId, SpawnInstanceId, StageCompileError,
};
use crate::simulation::TickRate;
use serde::{Deserialize, Serialize};

use super::query::{StageTimeAnchor, StageTimeBinding};

const DISTANCE_SCALE: f64 = 1_000_000.0;

/// One half-open interval during which an enemy occupies a targetable map cell.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CellOccupancy {
	pub cell: GridPosition,
	pub enter_tick: u64,
	pub exit_tick: u64,
	pub remaining_route_distance_microunits: u64,
}

impl CellOccupancy {
	pub const fn contains(&self, tick: u64) -> bool {
		self.enter_tick <= tick && tick < self.exit_tick
	}
}

/// A concrete enemy on its deterministic unblocked route.
#[derive(Clone, Debug, PartialEq)]
pub struct EnemyTrajectory {
	pub instance_id: SpawnInstanceId,
	pub enemy_id: String,
	pub route_index: u32,
	pub spawn_tick: u64,
	pub goal_deadline_tick: u64,
	pub max_hp: u64,
	pub defense: u64,
	pub magic_resistance: f64,
	pub target_class: TrajectoryClass,
	pub occupancy: Vec<CellOccupancy>,
}

impl EnemyTrajectory {
	pub fn occupancy_at(&self, tick: u64) -> Option<&CellOccupancy> {
		self.occupancy
			.iter()
			.find(|interval: &&CellOccupancy| interval.contains(tick))
	}
}

/// One source spawn cohort and its concrete instances.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnCohort {
	pub id: SpawnCohortId,
	pub enemy_id: String,
	pub route_index: u32,
	pub first_spawn_tick: u64,
	pub count: u32,
}

/// Route-motion class used by normal-attack target filters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryClass {
	Ground,
	Air,
}

/// One spawn cohort before its event-relative schedule anchor is bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativeSpawnCohort {
	pub id: SpawnCohortId,
	pub enemy_id: String,
	pub route_index: u32,
	pub anchor: StageTimeAnchor,
	pub first_spawn_offset_ticks: u64,
	pub count: u32,
}

/// One route trajectory whose ticks are relative to a stage event.
#[derive(Clone, Debug, PartialEq)]
pub struct RelativeEnemyTrajectory {
	pub instance_id: SpawnInstanceId,
	pub enemy_id: String,
	pub route_index: u32,
	pub anchor: StageTimeAnchor,
	pub spawn_offset_ticks: u64,
	pub goal_deadline_offset_ticks: u64,
	pub max_hp: u64,
	pub defense: u64,
	pub magic_resistance: f64,
	pub target_class: TrajectoryClass,
	pub occupancy: Vec<CellOccupancy>,
}

/// Deterministic geometry and action-local timing without invented wave start times.
#[derive(Clone, Debug, PartialEq)]
pub struct StageSpaceTimeTemplate {
	pub stage_id: String,
	pub code: Option<String>,
	pub level_id: String,
	pub ticks_per_second: u32,
	pub cohorts: Vec<RelativeSpawnCohort>,
	pub enemies: Vec<RelativeEnemyTrajectory>,
	pub mechanic_requirements: Vec<MechanicReference>,
}

impl StageSpaceTimeTemplate {
	/// Returns the stage anchors represented by deterministic spawn cohorts.
	///
	/// The returned anchors remain event-relative. In particular, a later wave fragment is not
	/// assigned the absolute start tick observed in one retained replay.
	#[must_use]
	pub fn spawn_anchors(&self) -> BTreeSet<StageTimeAnchor> {
		self.cohorts
			.iter()
			.map(|cohort: &RelativeSpawnCohort| cohort.anchor.clone())
			.collect()
	}

	/// Optimistic offset at which every enemy under `anchor` could have spawned.
	///
	/// This is a causal lower bound for clearing a fragment when every spawned enemy is defeated
	/// immediately. It deliberately does not use retained replay start or clear ticks.
	#[must_use]
	pub fn optimistic_clear_offset_ticks(&self, anchor: &StageTimeAnchor) -> Option<u64> {
		self.enemies
			.iter()
			.filter(|enemy: &&RelativeEnemyTrajectory| &enemy.anchor == anchor)
			.map(|enemy: &RelativeEnemyTrajectory| enemy.spawn_offset_ticks)
			.max()
	}

	#[must_use]
	pub fn enemy(&self, id: &SpawnInstanceId) -> Option<&RelativeEnemyTrajectory> {
		self.enemies
			.iter()
			.find(|enemy: &&RelativeEnemyTrajectory| &enemy.instance_id == id)
	}
}

impl RelativeEnemyTrajectory {
	#[must_use]
	pub fn occupancy_at_offset(&self, offset_tick: u64) -> Option<&CellOccupancy> {
		self.occupancy
			.iter()
			.find(|interval: &&CellOccupancy| interval.contains(offset_tick))
	}
}

/// Complete deterministic, unblocked stage projection.
#[derive(Clone, Debug, PartialEq)]
pub struct StageSpaceTimeModel {
	pub stage_id: String,
	pub code: Option<String>,
	pub level_id: String,
	pub ticks_per_second: u32,
	pub cohorts: Vec<SpawnCohort>,
	pub enemies: Vec<EnemyTrajectory>,
	pub bound_anchors: Vec<StageTimeBinding>,
	pub mechanic_requirements: Vec<MechanicReference>,
}

impl StageSpaceTimeModel {
	pub fn cohort(&self, id: &SpawnCohortId) -> Option<&SpawnCohort> {
		self.cohorts
			.iter()
			.find(|cohort: &&SpawnCohort| &cohort.id == id)
	}

	pub fn target_enemies<'a>(
		&'a self,
		cohorts: &'a BTreeSet<String>,
	) -> impl Iterator<Item = &'a EnemyTrajectory> + 'a {
		self.enemies
			.iter()
			.filter(|enemy: &&EnemyTrajectory| cohorts.contains(enemy.instance_id.cohort.as_str()))
	}
}

/// Compiles spawn-local timing and unblocked routes without assuming absolute wave starts.
pub fn compile_stage_space_time_template(
	stage: &CompiledStage,
	tick_rate: TickRate,
) -> Result<StageSpaceTimeTemplate, StageSpaceTimeError> {
	compile_stage_space_time_template_inner(stage, tick_rate, None)
}

/// Compiles only cohorts under anchors activated by the derived planning context.
pub fn compile_stage_space_time_template_for_bindings(
	stage: &CompiledStage,
	tick_rate: TickRate,
	bindings: &[StageTimeBinding],
) -> Result<StageSpaceTimeTemplate, StageSpaceTimeError> {
	let active_anchors: BTreeSet<StageTimeAnchor> = bindings
		.iter()
		.map(|binding| binding.anchor.clone())
		.collect();
	compile_stage_space_time_template_inner(stage, tick_rate, Some(&active_anchors))
}

fn compile_stage_space_time_template_inner(
	stage: &CompiledStage,
	tick_rate: TickRate,
	active_anchors: Option<&BTreeSet<StageTimeAnchor>>,
) -> Result<StageSpaceTimeTemplate, StageSpaceTimeError> {
	let mut cohorts: Vec<RelativeSpawnCohort> = Vec::new();
	let mut enemies: Vec<RelativeEnemyTrajectory> = Vec::new();
	for wave in stage.waves() {
		for fragment in wave.fragments() {
			for action in fragment.actions() {
				if !stage.action_is_active(action) {
					continue;
				}
				if active_anchors
					.is_some_and(|anchors| !anchors.contains(&stage_time_anchor(action)))
				{
					continue;
				}
				compile_spawn_action(stage, action, tick_rate, &mut cohorts, &mut enemies)?;
			}
		}
	}
	for branch in stage.branches().values() {
		for phase in branch.phases() {
			for action in phase.actions() {
				if !stage.action_is_active(action) {
					continue;
				}
				if active_anchors
					.is_some_and(|anchors| !anchors.contains(&stage_time_anchor(action)))
				{
					continue;
				}
				compile_spawn_action(stage, action, tick_rate, &mut cohorts, &mut enemies)?;
			}
		}
	}
	cohorts.sort_by(|left, right| left.id.cmp(&right.id));
	enemies.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
	Ok(StageSpaceTimeTemplate {
		stage_id: stage.stage_id().to_owned(),
		code: stage.code().map(str::to_owned),
		level_id: stage.level_id().to_owned(),
		ticks_per_second: tick_rate.ticks_per_second().get(),
		cohorts,
		enemies,
		mechanic_requirements: stage.mechanic_requirements().references().to_vec(),
	})
}

/// Binds exactly the active event anchors derived by the workflow.
///
/// Cohorts under unbound anchors are omitted. Callers must bind every active fragment or branch
/// that can affect the derived planning horizon; this explicit contract is preferable to treating
/// conditional branch data as if every branch were active.
pub fn bind_stage_space_time(
	template: &StageSpaceTimeTemplate,
	bindings: &[StageTimeBinding],
) -> Result<StageSpaceTimeModel, StageSpaceTimeError> {
	let mut starts: BTreeMap<&StageTimeAnchor, u64> = BTreeMap::new();
	for binding in bindings {
		if starts.insert(&binding.anchor, binding.tick).is_some() {
			return Err(StageSpaceTimeError::DuplicateScheduleBinding {
				anchor: binding.anchor.clone(),
			});
		}
	}

	let mut cohorts: Vec<SpawnCohort> = Vec::new();
	for cohort in &template.cohorts {
		let Some(anchor_tick) = starts.get(&cohort.anchor).copied() else {
			continue;
		};
		cohorts.push(SpawnCohort {
			id: cohort.id.clone(),
			enemy_id: cohort.enemy_id.clone(),
			route_index: cohort.route_index,
			first_spawn_tick: anchor_tick
				.checked_add(cohort.first_spawn_offset_ticks)
				.ok_or(StageSpaceTimeError::TickOverflow)?,
			count: cohort.count,
		});
	}

	let mut enemies: Vec<EnemyTrajectory> = Vec::new();
	for enemy in &template.enemies {
		let Some(anchor_tick) = starts.get(&enemy.anchor).copied() else {
			continue;
		};
		let occupancy: Vec<CellOccupancy> = enemy
			.occupancy
			.iter()
			.map(|interval: &CellOccupancy| {
				Ok(CellOccupancy {
					cell: interval.cell,
					enter_tick: anchor_tick
						.checked_add(interval.enter_tick)
						.ok_or(StageSpaceTimeError::TickOverflow)?,
					exit_tick: anchor_tick
						.checked_add(interval.exit_tick)
						.ok_or(StageSpaceTimeError::TickOverflow)?,
					remaining_route_distance_microunits: interval
						.remaining_route_distance_microunits,
				})
			})
			.collect::<Result<_, StageSpaceTimeError>>()?;
		enemies.push(EnemyTrajectory {
			instance_id: enemy.instance_id.clone(),
			enemy_id: enemy.enemy_id.clone(),
			route_index: enemy.route_index,
			spawn_tick: anchor_tick
				.checked_add(enemy.spawn_offset_ticks)
				.ok_or(StageSpaceTimeError::TickOverflow)?,
			goal_deadline_tick: anchor_tick
				.checked_add(enemy.goal_deadline_offset_ticks)
				.ok_or(StageSpaceTimeError::TickOverflow)?,
			max_hp: enemy.max_hp,
			defense: enemy.defense,
			magic_resistance: enemy.magic_resistance,
			target_class: enemy.target_class,
			occupancy,
		});
	}

	Ok(StageSpaceTimeModel {
		stage_id: template.stage_id.clone(),
		code: template.code.clone(),
		level_id: template.level_id.clone(),
		ticks_per_second: template.ticks_per_second,
		cohorts,
		enemies,
		bound_anchors: bindings.to_vec(),
		mechanic_requirements: template.mechanic_requirements.clone(),
	})
}

fn compile_spawn_action(
	stage: &CompiledStage,
	action: &StageAction,
	tick_rate: TickRate,
	cohorts: &mut Vec<RelativeSpawnCohort>,
	enemies: &mut Vec<RelativeEnemyTrajectory>,
) -> Result<(), StageSpaceTimeError> {
	let Some(spawn) = action.spawn() else {
		return Ok(());
	};
	let cohort_id: SpawnCohortId = SpawnCohortId::new(cohort_id(action))?;
	if !spawn_schedule_is_deterministic(
		action.managed_by_scheduler(),
		action.random_spawn_group(),
		action.random_spawn_pack(),
		action.random_type(),
		action.refresh_type(),
	) {
		return Err(StageSpaceTimeError::UnsupportedSpawnSchedule {
			cohort_id: cohort_id.as_str().to_owned(),
		});
	}
	let enemy_key: EnemyKey =
		EnemyKey::new(action.key().to_owned()).map_err(|_| StageSpaceTimeError::BlankEnemyId {
			cohort_id: cohort_id.as_str().to_owned(),
		})?;
	let enemy = stage
		.enemy(&enemy_key)
		.ok_or_else(|| StageSpaceTimeError::MissingEnemy {
			cohort_id: cohort_id.as_str().to_owned(),
			enemy_id: action.key().to_owned(),
		})?;
	let stats = enemy.stats();
	let max_hp: u64 = positive_enemy_u64(action.key(), "max_hp", stats.max_hp())?;
	let defense: u64 = nonnegative_enemy_u64(action.key(), "defense", stats.defense())?;
	let magic_resistance: f64 =
		required_enemy_f64(action.key(), "magic_resistance", stats.magic_resistance())?;
	if !(0.0..=100.0).contains(&magic_resistance) {
		return Err(StageSpaceTimeError::InvalidEnemyStat {
			enemy_id: action.key().to_owned(),
			field: "magic_resistance",
			value: magic_resistance,
		});
	}
	let move_speed: f64 = required_enemy_f64(action.key(), "move_speed", stats.move_speed())?;
	let route_timeline: RouteTimeline =
		compile_route_timeline(stage, spawn.route_index(), move_speed, tick_rate)?;
	let route = stage
		.routes()
		.get(spawn.route_index() as usize)
		.expect("canonical route compilation validated the route index");
	let target_class: TrajectoryClass = match route.motion() {
		MotionMode::Walk => TrajectoryClass::Ground,
		MotionMode::Fly => TrajectoryClass::Air,
		MotionMode::Unknown(value) => {
			return Err(StageCompileError::UnsupportedMotionMode {
				mode: value.clone(),
			}
			.into());
		}
	};
	let anchor: StageTimeAnchor = stage_time_anchor(action);
	let first_spawn_offset_ticks: u64 =
		ticks_from_seconds_ceil(spawn.pre_delay_seconds(), tick_rate)?;
	cohorts.push(RelativeSpawnCohort {
		id: cohort_id.clone(),
		enemy_id: action.key().to_owned(),
		route_index: spawn.route_index(),
		anchor: anchor.clone(),
		first_spawn_offset_ticks,
		count: spawn.count(),
	});
	for ordinal in 0..spawn.count() {
		let spawn_seconds: f64 =
			spawn.pre_delay_seconds() + spawn.interval_seconds() * f64::from(ordinal);
		let spawn_offset_ticks: u64 = ticks_from_seconds_ceil(spawn_seconds, tick_rate)?;
		let (occupancy, goal_deadline_offset_ticks): (Vec<CellOccupancy>, u64) =
			compile_route_occupancy(&route_timeline, spawn_offset_ticks)?;
		enemies.push(RelativeEnemyTrajectory {
			instance_id: SpawnInstanceId {
				cohort: cohort_id.clone(),
				ordinal,
			},
			enemy_id: action.key().to_owned(),
			route_index: spawn.route_index(),
			anchor: anchor.clone(),
			spawn_offset_ticks,
			goal_deadline_offset_ticks,
			max_hp,
			defense,
			magic_resistance,
			target_class,
			occupancy,
		});
	}
	Ok(())
}

fn spawn_schedule_is_deterministic(
	_managed_by_scheduler: bool,
	random_spawn_group: Option<&str>,
	random_spawn_pack: Option<&str>,
	random_type: StageActionSchedulePeriod,
	refresh_type: StageActionSchedulePeriod,
) -> bool {
	// Scheduler ownership does not introduce randomness once the containing fragment/phase has an
	// explicit workflow-derived anchor. The action-local schedule remains preDelay + ordinal *
	// interval.
	random_spawn_group.is_none()
		&& random_spawn_pack.is_none()
		&& random_type == StageActionSchedulePeriod::Always
		&& refresh_type == StageActionSchedulePeriod::Always
}

fn stage_time_anchor(action: &StageAction) -> StageTimeAnchor {
	match &action.origin().container {
		WaveActionContainer::Wave { wave, fragment } => StageTimeAnchor::WaveFragmentStart {
			wave: *wave,
			fragment: *fragment,
		},
		WaveActionContainer::Branch { branch, phase } => StageTimeAnchor::BranchPhaseStart {
			branch: branch.clone(),
			phase: *phase,
		},
	}
}

fn cohort_id(action: &StageAction) -> String {
	match &action.origin().container {
		WaveActionContainer::Wave { wave, fragment } => {
			format!(
				"wave:{wave}/fragment:{fragment}/action:{}",
				action.origin().action
			)
		}
		WaveActionContainer::Branch { branch, phase } => {
			format!(
				"branch:{branch}/phase:{phase}/action:{}",
				action.origin().action
			)
		}
	}
}

fn positive_enemy_u64(
	enemy_id: &str,
	field: &'static str,
	value: Option<i32>,
) -> Result<u64, StageSpaceTimeError> {
	let value: i32 = value.ok_or_else(|| StageSpaceTimeError::MissingEnemyStat {
		enemy_id: enemy_id.to_owned(),
		field,
	})?;
	if value <= 0 {
		return Err(StageSpaceTimeError::InvalidEnemyStat {
			enemy_id: enemy_id.to_owned(),
			field,
			value: f64::from(value),
		});
	}
	Ok(value as u64)
}

fn nonnegative_enemy_u64(
	enemy_id: &str,
	field: &'static str,
	value: Option<i32>,
) -> Result<u64, StageSpaceTimeError> {
	let value: i32 = value.ok_or_else(|| StageSpaceTimeError::MissingEnemyStat {
		enemy_id: enemy_id.to_owned(),
		field,
	})?;
	if value < 0 {
		return Err(StageSpaceTimeError::InvalidEnemyStat {
			enemy_id: enemy_id.to_owned(),
			field,
			value: f64::from(value),
		});
	}
	Ok(value as u64)
}

fn required_enemy_f64(
	enemy_id: &str,
	field: &'static str,
	value: Option<f64>,
) -> Result<f64, StageSpaceTimeError> {
	let value: f64 = value.ok_or_else(|| StageSpaceTimeError::MissingEnemyStat {
		enemy_id: enemy_id.to_owned(),
		field,
	})?;
	if !value.is_finite() {
		return Err(StageSpaceTimeError::InvalidEnemyStat {
			enemy_id: enemy_id.to_owned(),
			field,
			value,
		});
	}
	Ok(value)
}

#[derive(Clone, Debug)]
struct TimelineOccupancyState {
	cell: Option<GridPosition>,
	duration_ticks: u64,
	distance_to_next: f64,
}

/// Projects canonical route events into optimistic targetable-cell occupancy.
///
/// A movement step's duration belongs to its source cell. Waits extend the current visible or
/// hidden state, reappearance starts a new visible state, and the goal cell is never exposed.
fn compile_route_occupancy(
	timeline: &RouteTimeline,
	spawn_tick: u64,
) -> Result<(Vec<CellOccupancy>, u64), StageSpaceTimeError> {
	let states: Vec<TimelineOccupancyState> = timeline_occupancy_states(timeline)?;
	let mut remaining_distances: Vec<u64> = vec![0; states.len()];
	let mut remaining: f64 = 0.0;
	for index in (0..states.len()).rev() {
		remaining += states[index].distance_to_next;
		remaining_distances[index] = checked_distance(remaining)?;
	}

	let mut occupancy: Vec<CellOccupancy> = Vec::with_capacity(states.len());
	let mut current_tick: u64 = spawn_tick;
	for (index, state) in states.iter().enumerate() {
		let exit_tick: u64 = current_tick
			.checked_add(state.duration_ticks)
			.ok_or(StageSpaceTimeError::TickOverflow)?;
		if let Some(cell) = state.cell.filter(|_| exit_tick > current_tick) {
			occupancy.push(CellOccupancy {
				cell,
				enter_tick: current_tick,
				exit_tick,
				remaining_route_distance_microunits: remaining_distances[index],
			});
		}
		current_tick = exit_tick;
	}
	Ok((occupancy, current_tick))
}

fn timeline_occupancy_states(
	timeline: &RouteTimeline,
) -> Result<Vec<TimelineOccupancyState>, StageSpaceTimeError> {
	let mut states: Vec<TimelineOccupancyState> = vec![TimelineOccupancyState {
		cell: Some(timeline.start),
		duration_ticks: 0,
		distance_to_next: 0.0,
	}];
	for step in &timeline.steps {
		{
			let current: &mut TimelineOccupancyState = states
				.last_mut()
				.expect("a canonical route timeline always has a current state");
			current.duration_ticks = current
				.duration_ticks
				.checked_add(step.duration_ticks)
				.ok_or(StageSpaceTimeError::TickOverflow)?;
			if step.affected_by_movement_speed {
				let from: GridPosition = current
					.cell
					.ok_or(StageSpaceTimeError::InvalidCanonicalRouteTimeline)?;
				let destination: GridPosition = match &step.action {
					RouteTimelineAction::Move { cell, .. } => *cell,
					RouteTimelineAction::Goal => timeline.goal,
					RouteTimelineAction::Wait
					| RouteTimelineAction::Disappear
					| RouteTimelineAction::Reappear { .. } => {
						return Err(StageSpaceTimeError::InvalidCanonicalRouteTimeline);
					}
				};
				let row_delta: f64 = f64::from(destination.row.abs_diff(from.row));
				let col_delta: f64 = f64::from(destination.col.abs_diff(from.col));
				current.distance_to_next = row_delta.hypot(col_delta);
			}
		}

		match &step.action {
			RouteTimelineAction::Move { cell, .. } => states.push(TimelineOccupancyState {
				cell: Some(*cell),
				duration_ticks: 0,
				distance_to_next: 0.0,
			}),
			RouteTimelineAction::Wait => {}
			RouteTimelineAction::Disappear => states.push(TimelineOccupancyState {
				cell: None,
				duration_ticks: 0,
				distance_to_next: 0.0,
			}),
			RouteTimelineAction::Reappear { cell, .. } => states.push(TimelineOccupancyState {
				cell: Some(*cell),
				duration_ticks: 0,
				distance_to_next: 0.0,
			}),
			RouteTimelineAction::Goal => {}
		}
	}
	Ok(states)
}

/// Planning-local error adapter for the canonical tick conversion.
pub(crate) fn ticks_from_seconds_ceil(
	seconds: f64,
	tick_rate: TickRate,
) -> Result<u64, StageSpaceTimeError> {
	canonical_ticks_from_seconds_ceil(seconds, tick_rate).map_err(StageSpaceTimeError::from)
}

fn checked_distance(distance: f64) -> Result<u64, StageSpaceTimeError> {
	let scaled: f64 = distance * DISTANCE_SCALE;
	if !scaled.is_finite() || scaled < 0.0 || scaled > u64::MAX as f64 {
		return Err(StageSpaceTimeError::DistanceOverflow);
	}
	Ok(scaled.round() as u64)
}

#[derive(Clone, Debug, PartialEq)]
pub enum StageSpaceTimeError {
	StageCompile(StageCompileError),
	BlankEnemyId {
		cohort_id: String,
	},
	MissingEnemy {
		cohort_id: String,
		enemy_id: String,
	},
	MissingEnemyStat {
		enemy_id: String,
		field: &'static str,
	},
	InvalidEnemyStat {
		enemy_id: String,
		field: &'static str,
		value: f64,
	},
	UnsupportedSpawnSchedule {
		cohort_id: String,
	},
	DuplicateScheduleBinding {
		anchor: StageTimeAnchor,
	},
	InvalidCanonicalRouteTimeline,
	TickOverflow,
	DistanceOverflow,
}

impl From<StageCompileError> for StageSpaceTimeError {
	fn from(error: StageCompileError) -> Self {
		Self::StageCompile(error)
	}
}

impl Display for StageSpaceTimeError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::StageCompile(error) => Display::fmt(error, formatter),
			Self::BlankEnemyId { cohort_id } => {
				write!(formatter, "spawn cohort {cohort_id} has a blank enemy ID")
			}
			Self::MissingEnemy {
				cohort_id,
				enemy_id,
			} => write!(
				formatter,
				"spawn cohort {cohort_id} references missing enemy {enemy_id}"
			),
			Self::MissingEnemyStat { enemy_id, field } => {
				write!(
					formatter,
					"enemy {enemy_id} is missing required stat {field}"
				)
			}
			Self::InvalidEnemyStat {
				enemy_id,
				field,
				value,
			} => write!(
				formatter,
				"enemy {enemy_id} has invalid {field} value {value}"
			),
			Self::UnsupportedSpawnSchedule { cohort_id } => write!(
				formatter,
				"spawn cohort {cohort_id} uses randomized or non-ALWAYS timing"
			),
			Self::DuplicateScheduleBinding { anchor } => {
				write!(formatter, "duplicate schedule binding for {anchor:?}")
			}
			Self::InvalidCanonicalRouteTimeline => {
				formatter.write_str("canonical route timeline contains an invalid movement event")
			}
			Self::TickOverflow => formatter.write_str("stage tick calculation overflowed"),
			Self::DistanceOverflow => formatter.write_str("route distance calculation overflowed"),
		}
	}
}

impl Error for StageSpaceTimeError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::StageCompile(error) => Some(error),
			Self::BlankEnemyId { .. }
			| Self::MissingEnemy { .. }
			| Self::MissingEnemyStat { .. }
			| Self::InvalidEnemyStat { .. }
			| Self::UnsupportedSpawnSchedule { .. }
			| Self::DuplicateScheduleBinding { .. }
			| Self::InvalidCanonicalRouteTimeline
			| Self::TickOverflow
			| Self::DistanceOverflow => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::num::NonZeroU32;

	use crate::data::{GridPosition, StageActionSchedulePeriod};
	use crate::mechanics::domain::Facing;
	use crate::mechanics::stage::{RouteTimeline, RouteTimelineAction, RouteTimelineStep};
	use crate::simulation::TickRate;

	use super::super::query::{StageTimeAnchor, StageTimeBinding};
	use super::{
		bind_stage_space_time, compile_route_occupancy, spawn_schedule_is_deterministic,
		ticks_from_seconds_ceil, CellOccupancy, RelativeEnemyTrajectory, RelativeSpawnCohort,
		SpawnCohortId, SpawnInstanceId, StageSpaceTimeError, StageSpaceTimeTemplate,
		TrajectoryClass,
	};

	#[test]
	fn rounds_stage_seconds_up_to_the_next_tick() {
		let rate: TickRate = TickRate::new(NonZeroU32::new(30).expect("30 is positive"));

		assert_eq!(ticks_from_seconds_ceil(0.0, rate), Ok(0));
		assert_eq!(ticks_from_seconds_ceil(0.01, rate), Ok(1));
		assert_eq!(ticks_from_seconds_ceil(0.1, rate), Ok(3));
	}

	#[test]
	fn scheduler_managed_always_schedule_is_deterministic() {
		assert!(spawn_schedule_is_deterministic(
			true,
			None,
			None,
			StageActionSchedulePeriod::Always,
			StageActionSchedulePeriod::Always,
		));
	}

	#[test]
	fn random_or_periodic_spawn_schedules_still_fail_closed() {
		assert!(!spawn_schedule_is_deterministic(
			true,
			Some("group"),
			None,
			StageActionSchedulePeriod::Always,
			StageActionSchedulePeriod::Always,
		));
		assert!(!spawn_schedule_is_deterministic(
			false,
			None,
			Some("pack"),
			StageActionSchedulePeriod::Always,
			StageActionSchedulePeriod::Always,
		));
		assert!(!spawn_schedule_is_deterministic(
			false,
			None,
			None,
			StageActionSchedulePeriod::PerDay,
			StageActionSchedulePeriod::Always,
		));
	}

	#[test]
	fn canonical_timeline_projects_visibility_waits_and_remaining_distance() {
		let timeline: RouteTimeline = RouteTimeline {
			start: GridPosition { row: 0, col: 0 },
			goal: GridPosition { row: 2, col: 3 },
			steps: vec![
				RouteTimelineStep {
					duration_ticks: 10,
					affected_by_movement_speed: true,
					action: RouteTimelineAction::Move {
						cell: GridPosition { row: 0, col: 1 },
						facing: Facing::Right,
					},
				},
				RouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Disappear,
				},
				RouteTimelineStep {
					duration_ticks: 20,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Wait,
				},
				RouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Reappear {
						cell: GridPosition { row: 2, col: 2 },
						facing: Facing::Right,
					},
				},
				RouteTimelineStep {
					duration_ticks: 10,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Wait,
				},
				RouteTimelineStep {
					duration_ticks: 10,
					affected_by_movement_speed: true,
					action: RouteTimelineAction::Move {
						cell: GridPosition { row: 2, col: 3 },
						facing: Facing::Right,
					},
				},
				RouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Goal,
				},
			],
		};

		let (occupancy, deadline) = compile_route_occupancy(&timeline, 0).unwrap();

		assert_eq!(deadline, 50);
		assert_eq!(
			occupancy,
			vec![
				CellOccupancy {
					cell: GridPosition { row: 0, col: 0 },
					enter_tick: 0,
					exit_tick: 10,
					remaining_route_distance_microunits: 2_000_000,
				},
				CellOccupancy {
					cell: GridPosition { row: 2, col: 2 },
					enter_tick: 30,
					exit_tick: 50,
					remaining_route_distance_microunits: 1_000_000,
				},
			]
		);
	}

	#[test]
	fn binding_shifts_relative_ticks_and_omits_inactive_anchors() {
		let active_anchor: StageTimeAnchor = StageTimeAnchor::WaveFragmentStart {
			wave: 1,
			fragment: 2,
		};
		let inactive_anchor: StageTimeAnchor = StageTimeAnchor::BranchPhaseStart {
			branch: "alternate".to_owned(),
			phase: 0,
		};
		let active_id: SpawnCohortId =
			SpawnCohortId::new("wave:1/fragment:2/action:3".to_owned()).unwrap();
		let inactive_id: SpawnCohortId =
			SpawnCohortId::new("branch:alternate/phase:0/action:0".to_owned()).unwrap();
		let template: StageSpaceTimeTemplate = StageSpaceTimeTemplate {
			stage_id: "stage".to_owned(),
			code: Some("TEST-1".to_owned()),
			level_id: "level".to_owned(),
			ticks_per_second: 30,
			cohorts: vec![
				RelativeSpawnCohort {
					id: active_id.clone(),
					enemy_id: "enemy_a".to_owned(),
					route_index: 0,
					anchor: active_anchor.clone(),
					first_spawn_offset_ticks: 5,
					count: 1,
				},
				RelativeSpawnCohort {
					id: inactive_id,
					enemy_id: "enemy_b".to_owned(),
					route_index: 1,
					anchor: inactive_anchor,
					first_spawn_offset_ticks: 0,
					count: 1,
				},
			],
			enemies: vec![RelativeEnemyTrajectory {
				instance_id: SpawnInstanceId {
					cohort: active_id,
					ordinal: 0,
				},
				enemy_id: "enemy_a".to_owned(),
				route_index: 0,
				anchor: active_anchor.clone(),
				spawn_offset_ticks: 5,
				goal_deadline_offset_ticks: 25,
				max_hp: 100,
				defense: 10,
				magic_resistance: 20.0,
				target_class: TrajectoryClass::Ground,
				occupancy: vec![CellOccupancy {
					cell: GridPosition { row: 1, col: 2 },
					enter_tick: 5,
					exit_tick: 15,
					remaining_route_distance_microunits: 3_000_000,
				}],
			}],
			mechanic_requirements: Vec::new(),
		};
		assert_eq!(template.spawn_anchors().len(), 2);
		assert_eq!(
			template.optimistic_clear_offset_ticks(&active_anchor),
			Some(5)
		);
		assert_eq!(
			template.enemies[0]
				.occupancy_at_offset(10)
				.map(|occupancy| occupancy.cell),
			Some(GridPosition { row: 1, col: 2 })
		);
		assert_eq!(
			template.enemy(&template.enemies[0].instance_id),
			Some(&template.enemies[0])
		);

		let model = bind_stage_space_time(
			&template,
			&[StageTimeBinding {
				anchor: active_anchor,
				tick: 100,
			}],
		)
		.unwrap();

		assert_eq!(model.cohorts.len(), 1);
		assert_eq!(model.cohorts[0].first_spawn_tick, 105);
		assert_eq!(model.enemies.len(), 1);
		assert_eq!(model.enemies[0].spawn_tick, 105);
		assert_eq!(model.enemies[0].goal_deadline_tick, 125);
		assert_eq!(model.enemies[0].occupancy[0].enter_tick, 105);
		assert_eq!(model.enemies[0].occupancy[0].exit_tick, 115);
	}

	#[test]
	fn duplicate_anchor_bindings_fail_closed() {
		let template: StageSpaceTimeTemplate = StageSpaceTimeTemplate {
			stage_id: "stage".to_owned(),
			code: None,
			level_id: "level".to_owned(),
			ticks_per_second: 30,
			cohorts: Vec::new(),
			enemies: Vec::new(),
			mechanic_requirements: Vec::new(),
		};
		let anchor: StageTimeAnchor = StageTimeAnchor::BattleStart;
		let error = bind_stage_space_time(
			&template,
			&[
				StageTimeBinding {
					anchor: anchor.clone(),
					tick: 0,
				},
				StageTimeBinding {
					anchor: anchor.clone(),
					tick: 1,
				},
			],
		)
		.unwrap_err();

		assert_eq!(
			error,
			StageSpaceTimeError::DuplicateScheduleBinding { anchor }
		);
	}
}
