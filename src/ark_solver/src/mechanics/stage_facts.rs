//! Canonical stage identities and route events shared by planning and replay.

use std::{
	collections::{BTreeMap, BTreeSet, VecDeque},
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::{
	CompiledStage, GridPosition, MapDirection, MotionMode, Passability, Route, RouteCheckpointKind,
	StageVector2,
};
use crate::simulation::TickRate;
use serde::{Deserialize, Serialize};

use crate::mechanics::geometry::Facing;

/// Stable stage-derived identity for a source spawn cohort.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SpawnCohortId(String);

impl SpawnCohortId {
	pub fn new(value: String) -> Result<Self, StageCompileError> {
		if value.trim().is_empty() {
			return Err(StageCompileError::BlankCohortId);
		}
		Ok(Self(value))
	}

	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

/// Stable identity of one concrete enemy produced by a spawn action.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SpawnInstanceId {
	pub cohort: SpawnCohortId,
	pub ordinal: u32,
}

/// Exact route events retained before runtime actor IDs are assigned.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteTimeline {
	pub start: GridPosition,
	pub goal: GridPosition,
	pub steps: Vec<RouteTimelineStep>,
}

impl RouteTimeline {
	#[must_use]
	pub fn initial_facing(&self) -> Option<Facing> {
		self.steps
			.iter()
			.find_map(|step: &RouteTimelineStep| match &step.action {
				RouteTimelineAction::Move { facing, .. }
				| RouteTimelineAction::Reappear { facing, .. } => Some(*facing),
				RouteTimelineAction::Wait
				| RouteTimelineAction::Disappear
				| RouteTimelineAction::Goal => None,
			})
	}
}

/// One causal route event and the time elapsed since the preceding event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RouteTimelineStep {
	pub duration_ticks: u64,
	pub affected_by_movement_speed: bool,
	pub action: RouteTimelineAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteTimelineAction {
	Move { cell: GridPosition, facing: Facing },
	Wait,
	Disappear,
	Reappear { cell: GridPosition, facing: Facing },
	Goal,
}

/// Compiles a stage route while retaining waits and zero-duration visibility transitions.
///
/// The final movement leg is represented by [`RouteTimelineAction::Goal`], so the goal cell is
/// never exposed as an ordinary targetable move.
///
/// `move_speed_attribute` is the actor's raw move-speed attribute. The stage's own
/// `options.moveMultiplier` scales it here, in the one place both replay and planning reach, so the
/// two cannot drift apart. Waits keep their wall-clock duration: the scale reaches movement legs
/// only.
pub fn compile_route_timeline(
	stage: &CompiledStage,
	route_index: u32,
	move_speed_attribute: f64,
	tick_rate: TickRate,
) -> Result<RouteTimeline, StageCompileError> {
	if !move_speed_attribute.is_finite() || move_speed_attribute <= 0.0 {
		return Err(StageCompileError::InvalidMoveSpeed {
			value: move_speed_attribute,
		});
	}
	let move_speed_tiles_per_second: f64 = move_speed_attribute * stage.options().move_multiplier();
	if !move_speed_tiles_per_second.is_finite() || move_speed_tiles_per_second <= 0.0 {
		return Err(StageCompileError::InvalidMoveSpeed {
			value: move_speed_tiles_per_second,
		});
	}
	let cohort_id: String = format!("route:{route_index}");
	let route: &Route =
		stage
			.routes()
			.get(route_index as usize)
			.ok_or(StageCompileError::InvalidRouteIndex {
				cohort_id: cohort_id.clone(),
				route_index,
				route_count: stage.routes().len(),
			})?;
	validate_route_projection(route, &cohort_id, route_index)?;
	let leg_compiler: RouteTimelineLegCompiler<'_> = RouteTimelineLegCompiler {
		stage,
		route,
		move_speed_tiles_per_second,
		tick_rate,
	};

	let mut draft: Vec<DraftRouteTimelineStep> = Vec::new();
	let mut current: GridPosition = route.start();
	let mut targetable: bool = true;
	for checkpoint in route.checkpoints() {
		match checkpoint.kind() {
			RouteCheckpointKind::Move => {
				if !targetable {
					return Err(StageCompileError::InvalidRouteVisibilityTransition {
						kind: "MOVE".to_owned(),
						targetable,
					});
				}
				require_zero_checkpoint_delay(checkpoint.kind(), checkpoint.delay_seconds())?;
				let destination: GridPosition =
					checkpoint
						.position()
						.ok_or(StageCompileError::MissingCheckpointPosition {
							kind: "MOVE".to_owned(),
						})?;
				leg_compiler.append(current, destination, false, &mut draft)?;
				current = destination;
			}
			RouteCheckpointKind::WaitForSeconds => {
				let wait_ticks: u64 =
					ticks_from_seconds_ceil(checkpoint.delay_seconds(), tick_rate)?;
				draft.push(DraftRouteTimelineStep {
					duration_ticks: wait_ticks,
					affected_by_movement_speed: false,
					action: DraftRouteTimelineAction::Wait,
				});
			}
			RouteCheckpointKind::Disappear => {
				require_zero_checkpoint_delay(checkpoint.kind(), checkpoint.delay_seconds())?;
				if !targetable {
					return Err(StageCompileError::InvalidRouteVisibilityTransition {
						kind: checkpoint.kind().mechanic_id().to_owned(),
						targetable,
					});
				}
				draft.push(DraftRouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: DraftRouteTimelineAction::Disappear,
				});
				targetable = false;
			}
			RouteCheckpointKind::AppearAtPosition => {
				require_zero_checkpoint_delay(checkpoint.kind(), checkpoint.delay_seconds())?;
				if targetable {
					return Err(StageCompileError::InvalidRouteVisibilityTransition {
						kind: checkpoint.kind().mechanic_id().to_owned(),
						targetable,
					});
				}
				let destination: GridPosition =
					checkpoint
						.position()
						.ok_or(StageCompileError::MissingCheckpointPosition {
							kind: checkpoint.kind().mechanic_id().to_owned(),
						})?;
				draft.push(DraftRouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: DraftRouteTimelineAction::Reappear { cell: destination },
				});
				current = destination;
				targetable = true;
			}
			unsupported => {
				return Err(StageCompileError::UnsupportedRouteCheckpoint {
					kind: unsupported.mechanic_id().to_owned(),
				});
			}
		}
	}
	if !targetable {
		return Err(StageCompileError::InvalidRouteVisibilityTransition {
			kind: "ROUTE_END".to_owned(),
			targetable,
		});
	}
	leg_compiler.append(current, route.end(), true, &mut draft)?;

	Ok(RouteTimeline {
		start: route.start(),
		goal: route.end(),
		steps: finalize_route_timeline(draft)?,
	})
}

/// Converts non-negative seconds into the first simulation tick at or after that duration.
pub fn ticks_from_seconds_ceil(
	seconds: f64,
	tick_rate: TickRate,
) -> Result<u64, StageCompileError> {
	if !seconds.is_finite() || seconds < 0.0 {
		return Err(StageCompileError::InvalidSeconds { value: seconds });
	}
	let ticks: f64 = seconds * f64::from(tick_rate.ticks_per_second().get());
	if !ticks.is_finite() || ticks > u64::MAX as f64 {
		return Err(StageCompileError::TickOverflow);
	}
	Ok(ticks.ceil() as u64)
}

fn validate_route_projection(
	route: &Route,
	cohort_id: &str,
	route_index: u32,
) -> Result<(), StageCompileError> {
	if !spawn_random_range_is_zero_mean_within_one_cell(route.spawn_random_range())
		|| route.spawn_offset().x != 0.0
		|| route.spawn_offset().y != 0.0
		|| route.checkpoints().iter().any(|checkpoint| {
			checkpoint.randomize_reach_offset()
				|| checkpoint.reach_offset().x != 0.0
				|| checkpoint.reach_offset().y != 0.0
				|| checkpoint.reach_distance() != 0.0
		}) {
		return Err(StageCompileError::UnsupportedContinuousRouteGeometry {
			cohort_id: cohort_id.to_owned(),
			route_index,
		});
	}
	if !route_traversal_flags_are_supported(
		route.visit_every_tile_center(),
		route.visit_every_node_center(),
		route.visit_every_checkpoint(),
	) {
		return Err(StageCompileError::UnsupportedRouteTraversalFlags {
			cohort_id: cohort_id.to_owned(),
			route_index,
			visit_every_tile_center: route.visit_every_tile_center(),
			visit_every_node_center: route.visit_every_node_center(),
			visit_every_checkpoint: route.visit_every_checkpoint(),
		});
	}
	Ok(())
}

/// Half a tile: the largest scatter whose whole box still lies inside the route's start cell.
const MAX_SPAWN_SCATTER_HALF_WIDTH: f64 = 0.5;

/// Whether a spawn scatter box is declared game RNG this replay may take at its expectation.
///
/// `spawnRandomRange` displaces a spawn uniformly within a box of the given half-widths around the
/// route's start point. That is symmetric about the start, so its expected displacement is zero and
/// an expected-value replay reproduces it by spawning at the start cell — the same substitution the
/// replay already makes for damage RNG.
///
/// The argument only licenses that substitution while the box stays inside one cell: a wider box
/// would let a real spawn begin in a different cell, which is a different route, not a sub-tile
/// offset this projection can average away. A negative or non-finite half-width is not a box the
/// argument covers at all, and `spawnOffset` is a deterministic displacement rather than RNG; all
/// of those stay fail-closed.
fn spawn_random_range_is_zero_mean_within_one_cell(range: StageVector2) -> bool {
	[range.x, range.y].into_iter().all(|half_width: f64| {
		half_width.is_finite() && (0.0..MAX_SPAWN_SCATTER_HALF_WIDTH).contains(&half_width)
	})
}

fn route_traversal_flags_are_supported(
	visit_every_tile_center: bool,
	visit_every_node_center: bool,
	visit_every_checkpoint: bool,
) -> bool {
	matches!(
		(
			visit_every_tile_center,
			visit_every_node_center,
			visit_every_checkpoint
		),
		(true, false, true) | (false, false, false)
	)
}

#[derive(Clone, Debug)]
struct DraftRouteTimelineStep {
	duration_ticks: u64,
	affected_by_movement_speed: bool,
	action: DraftRouteTimelineAction,
}

#[derive(Clone, Debug)]
enum DraftRouteTimelineAction {
	Move { cell: GridPosition, facing: Facing },
	Wait,
	Disappear,
	Reappear { cell: GridPosition },
	Goal { approach_facing: Option<Facing> },
}

struct RouteTimelineLegCompiler<'a> {
	stage: &'a CompiledStage,
	route: &'a Route,
	move_speed_tiles_per_second: f64,
	tick_rate: TickRate,
}

impl RouteTimelineLegCompiler<'_> {
	fn append(
		&self,
		start: GridPosition,
		end: GridPosition,
		is_goal: bool,
		draft: &mut Vec<DraftRouteTimelineStep>,
	) -> Result<(), StageCompileError> {
		let path: Vec<GridPosition> = route_leg_cells(self.stage, self.route, start, end)?;
		if path.len() == 1 {
			if is_goal {
				draft.push(DraftRouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: DraftRouteTimelineAction::Goal {
						approach_facing: None,
					},
				});
			}
			return Ok(());
		}
		for (edge_index, edge) in path.windows(2).enumerate() {
			let from: GridPosition = edge[0];
			let to: GridPosition = edge[1];
			let row_delta: f64 = f64::from(to.row.abs_diff(from.row));
			let col_delta: f64 = f64::from(to.col.abs_diff(from.col));
			let distance: f64 = row_delta.hypot(col_delta);
			let duration_ticks: u64 = ticks_from_seconds_ceil(
				distance / self.move_speed_tiles_per_second,
				self.tick_rate,
			)?;
			let facing: Facing = movement_facing(from, to)?;
			let final_edge: bool = edge_index + 2 == path.len();
			let action: DraftRouteTimelineAction = if is_goal && final_edge {
				DraftRouteTimelineAction::Goal {
					approach_facing: Some(facing),
				}
			} else {
				DraftRouteTimelineAction::Move { cell: to, facing }
			};
			draft.push(DraftRouteTimelineStep {
				duration_ticks,
				affected_by_movement_speed: true,
				action,
			});
		}
		Ok(())
	}
}

fn finalize_route_timeline(
	draft: Vec<DraftRouteTimelineStep>,
) -> Result<Vec<RouteTimelineStep>, StageCompileError> {
	let mut steps: Vec<RouteTimelineStep> = Vec::with_capacity(draft.len());
	for (index, step) in draft.iter().enumerate() {
		let action: RouteTimelineAction = match &step.action {
			DraftRouteTimelineAction::Move { cell, facing } => RouteTimelineAction::Move {
				cell: *cell,
				facing: *facing,
			},
			DraftRouteTimelineAction::Wait => RouteTimelineAction::Wait,
			DraftRouteTimelineAction::Disappear => RouteTimelineAction::Disappear,
			DraftRouteTimelineAction::Reappear { cell } => {
				let facing: Facing = timeline_facing_after_reappearance(&draft, index)?;
				RouteTimelineAction::Reappear {
					cell: *cell,
					facing,
				}
			}
			DraftRouteTimelineAction::Goal { .. } => RouteTimelineAction::Goal,
		};
		steps.push(RouteTimelineStep {
			duration_ticks: step.duration_ticks,
			affected_by_movement_speed: step.affected_by_movement_speed,
			action,
		});
	}
	Ok(steps)
}

fn timeline_facing_after_reappearance(
	draft: &[DraftRouteTimelineStep],
	reappearance_index: usize,
) -> Result<Facing, StageCompileError> {
	let next: Option<Facing> = draft[reappearance_index + 1..]
		.iter()
		.find_map(draft_movement_facing);
	if let Some(facing) = next {
		return Ok(facing);
	}
	draft[..reappearance_index]
		.iter()
		.rev()
		.find_map(draft_movement_facing)
		.ok_or(StageCompileError::MissingRouteFacing)
}

fn draft_movement_facing(step: &DraftRouteTimelineStep) -> Option<Facing> {
	match &step.action {
		DraftRouteTimelineAction::Move { facing, .. } => Some(*facing),
		DraftRouteTimelineAction::Goal {
			approach_facing: Some(facing),
		} => Some(*facing),
		DraftRouteTimelineAction::Wait
		| DraftRouteTimelineAction::Disappear
		| DraftRouteTimelineAction::Reappear { .. }
		| DraftRouteTimelineAction::Goal {
			approach_facing: None,
		} => None,
	}
}

fn movement_facing(from: GridPosition, to: GridPosition) -> Result<Facing, StageCompileError> {
	if to.col > from.col {
		return Ok(Facing::Right);
	}
	if to.col < from.col {
		return Ok(Facing::Left);
	}
	if to.row > from.row {
		return Ok(Facing::Up);
	}
	if to.row < from.row {
		return Ok(Facing::Down);
	}
	Err(StageCompileError::MissingRouteFacing)
}

fn require_zero_checkpoint_delay(
	kind: &RouteCheckpointKind,
	delay_seconds: f64,
) -> Result<(), StageCompileError> {
	if !delay_seconds.is_finite() || delay_seconds != 0.0 {
		return Err(StageCompileError::UnsupportedCheckpointDelay {
			kind: kind.mechanic_id().to_owned(),
			value: delay_seconds,
		});
	}
	Ok(())
}

fn route_leg_cells(
	stage: &CompiledStage,
	route: &Route,
	start: GridPosition,
	end: GridPosition,
) -> Result<Vec<GridPosition>, StageCompileError> {
	match route.motion() {
		MotionMode::Walk => shortest_walk_path(stage, start, end, route.allow_diagonal_move()),
		MotionMode::Fly => Ok(straight_flight_path(start, end)),
		MotionMode::Unknown(value) => Err(StageCompileError::UnsupportedMotionMode {
			mode: value.clone(),
		}),
	}
}

fn shortest_walk_path(
	stage: &CompiledStage,
	start: GridPosition,
	end: GridPosition,
	allow_diagonal_move: bool,
) -> Result<Vec<GridPosition>, StageCompileError> {
	let mut queue: VecDeque<GridPosition> = VecDeque::from([start]);
	let mut predecessor: BTreeMap<GridPosition, GridPosition> = BTreeMap::new();
	let mut visited: BTreeSet<GridPosition> = BTreeSet::from([start]);
	while let Some(current) = queue.pop_front() {
		if current == end {
			break;
		}
		for next in walk_neighbors(current, stage.map_dimensions(), allow_diagonal_move) {
			if visited.contains(&next)
				|| !walkable(stage, next, start, end)
				|| walk_transition_blocked(stage, current, next)
			{
				continue;
			}
			visited.insert(next);
			predecessor.insert(next, current);
			queue.push_back(next);
		}
	}
	if !visited.contains(&end) {
		return Err(StageCompileError::NoWalkPath { start, end });
	}
	let mut reversed: Vec<GridPosition> = vec![end];
	let mut cursor: GridPosition = end;
	while cursor != start {
		cursor = *predecessor
			.get(&cursor)
			.expect("visited non-start cells have predecessors");
		reversed.push(cursor);
	}
	reversed.reverse();
	Ok(reversed)
}

fn walk_neighbors(
	position: GridPosition,
	dimensions: crate::data::MapDimensions,
	allow_diagonal_move: bool,
) -> impl Iterator<Item = GridPosition> {
	let row: i64 = i64::from(position.row);
	let col: i64 = i64::from(position.col);
	[
		(1_i64, 0_i64),
		(0, 1),
		(-1, 0),
		(0, -1),
		(1, 1),
		(-1, 1),
		(-1, -1),
		(1, -1),
	]
	.into_iter()
	.filter_map(move |(row_delta, col_delta): (i64, i64)| {
		if !allow_diagonal_move && row_delta != 0 && col_delta != 0 {
			return None;
		}
		let next_row: i64 = row + row_delta;
		let next_col: i64 = col + col_delta;
		if next_row < 0
			|| next_col < 0
			|| next_row >= i64::from(dimensions.height)
			|| next_col >= i64::from(dimensions.width)
		{
			return None;
		}
		Some(GridPosition {
			row: next_row as u32,
			col: next_col as u32,
		})
	})
}

fn walk_transition_blocked(stage: &CompiledStage, from: GridPosition, to: GridPosition) -> bool {
	let row_delta: i64 = i64::from(to.row) - i64::from(from.row);
	let col_delta: i64 = i64::from(to.col) - i64::from(from.col);
	let crossed_directions: Vec<MapDirection> = [
		(row_delta > 0).then_some(MapDirection::Up),
		(row_delta < 0).then_some(MapDirection::Down),
		(col_delta > 0).then_some(MapDirection::Right),
		(col_delta < 0).then_some(MapDirection::Left),
	]
	.into_iter()
	.flatten()
	.collect();
	crossed_directions.into_iter().any(|direction| {
		let reverse: MapDirection = match direction {
			MapDirection::Up => MapDirection::Down,
			MapDirection::Right => MapDirection::Left,
			MapDirection::Down => MapDirection::Up,
			MapDirection::Left => MapDirection::Right,
			MapDirection::Invalid | MapDirection::Unknown(_) => {
				unreachable!("crossed directions are canonical")
			}
		};
		stage.block_edges().iter().any(|edge| {
			matches!(edge.block_mask(), Passability::WalkOnly | Passability::All)
				&& ((edge.position() == from && edge.direction() == &direction)
					|| (edge.position() == to && edge.direction() == &reverse))
		})
	})
}

fn walkable(
	stage: &CompiledStage,
	position: GridPosition,
	start: GridPosition,
	end: GridPosition,
) -> bool {
	if position == start || position == end {
		return true;
	}
	stage
		.cell(position)
		.is_some_and(|cell| matches!(cell.passability(), Passability::WalkOnly | Passability::All))
}

fn straight_flight_path(start: GridPosition, end: GridPosition) -> Vec<GridPosition> {
	let row_delta: i64 = i64::from(end.row) - i64::from(start.row);
	let col_delta: i64 = i64::from(end.col) - i64::from(start.col);
	let steps: i64 = row_delta.abs().max(col_delta.abs());
	if steps == 0 {
		return vec![start];
	}
	let mut path: Vec<GridPosition> = Vec::with_capacity(steps as usize + 1);
	for step in 0..=steps {
		let progress: f64 = step as f64 / steps as f64;
		let row: i64 = (i64::from(start.row) as f64 + row_delta as f64 * progress).round() as i64;
		let col: i64 = (i64::from(start.col) as f64 + col_delta as f64 * progress).round() as i64;
		let cell: GridPosition = GridPosition {
			row: row as u32,
			col: col as u32,
		};
		if path.last() != Some(&cell) {
			path.push(cell);
		}
	}
	path
}

#[derive(Clone, Debug, PartialEq)]
pub enum StageCompileError {
	BlankCohortId,
	InvalidRouteIndex {
		cohort_id: String,
		route_index: u32,
		route_count: usize,
	},
	UnsupportedContinuousRouteGeometry {
		cohort_id: String,
		route_index: u32,
	},
	UnsupportedRouteTraversalFlags {
		cohort_id: String,
		route_index: u32,
		visit_every_tile_center: bool,
		visit_every_node_center: bool,
		visit_every_checkpoint: bool,
	},
	InvalidMoveSpeed {
		value: f64,
	},
	InvalidSeconds {
		value: f64,
	},
	UnsupportedCheckpointDelay {
		kind: String,
		value: f64,
	},
	InvalidRouteVisibilityTransition {
		kind: String,
		targetable: bool,
	},
	MissingCheckpointPosition {
		kind: String,
	},
	MissingRouteFacing,
	UnsupportedRouteCheckpoint {
		kind: String,
	},
	UnsupportedMotionMode {
		mode: String,
	},
	NoWalkPath {
		start: GridPosition,
		end: GridPosition,
	},
	TickOverflow,
}

impl Display for StageCompileError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::BlankCohortId => formatter.write_str("spawn cohort ID must not be blank"),
			Self::InvalidRouteIndex {
				cohort_id,
				route_index,
				route_count,
			} => write!(
				formatter,
				"spawn cohort {cohort_id} route index {route_index} is outside {route_count} routes"
			),
			Self::UnsupportedContinuousRouteGeometry {
				cohort_id,
				route_index,
			} => write!(
				formatter,
				"spawn cohort {cohort_id} route {route_index} uses unsupported continuous offsets"
			),
			Self::UnsupportedRouteTraversalFlags {
				cohort_id,
				route_index,
				visit_every_tile_center,
				visit_every_node_center,
				visit_every_checkpoint,
			} => write!(
				formatter,
				"spawn cohort {cohort_id} route {route_index} traversal flags are unsupported: tile_center={visit_every_tile_center}, node_center={visit_every_node_center}, checkpoint={visit_every_checkpoint}"
			),
			Self::InvalidMoveSpeed { value } => write!(
				formatter,
				"enemy move speed must be finite and positive, found {value}"
			),
			Self::InvalidSeconds { value } => {
				write!(formatter, "seconds must be finite and nonnegative, found {value}")
			}
			Self::UnsupportedCheckpointDelay { kind, value } => write!(
				formatter,
				"route checkpoint {kind} requires zero delay, found {value}"
			),
			Self::InvalidRouteVisibilityTransition { kind, targetable } => write!(
				formatter,
				"route checkpoint {kind} is invalid while targetable={targetable}"
			),
			Self::MissingCheckpointPosition { kind } => {
				write!(formatter, "route checkpoint {kind} requires a position")
			}
			Self::MissingRouteFacing => {
				formatter.write_str("route visibility transition has no movement-facing context")
			}
			Self::UnsupportedRouteCheckpoint { kind } => {
				write!(formatter, "unsupported route checkpoint {kind}")
			}
			Self::UnsupportedMotionMode { mode } => {
				write!(formatter, "unsupported route motion mode {mode}")
			}
			Self::NoWalkPath { start, end } => write!(
				formatter,
				"no walkable path from ({}, {}) to ({}, {})",
				start.row, start.col, end.row, end.col
			),
			Self::TickOverflow => formatter.write_str("stage tick calculation overflowed"),
		}
	}
}

impl Error for StageCompileError {}

#[cfg(test)]
mod tests {
	use std::num::NonZeroU32;

	use super::{spawn_random_range_is_zero_mean_within_one_cell, StageVector2};
	use crate::data::GridPosition;
	use crate::simulation::TickRate;

	use super::{
		finalize_route_timeline, movement_facing, ticks_from_seconds_ceil,
		DraftRouteTimelineAction, DraftRouteTimelineStep, RouteTimelineAction, RouteTimelineStep,
		SpawnCohortId, SpawnInstanceId,
	};
	use crate::mechanics::geometry::Facing;

	#[test]
	fn rejects_blank_cohort_ids() {
		assert!(SpawnCohortId::new("  ".to_owned()).is_err());
		let id: SpawnCohortId = SpawnCohortId::new("wave:0/fragment:0/action:0".to_owned())
			.expect("non-blank cohort ID");
		assert_eq!(id.as_str(), "wave:0/fragment:0/action:0");
		assert_eq!(
			SpawnInstanceId {
				cohort: id,
				ordinal: 3,
			}
			.ordinal,
			3
		);
	}

	#[test]
	fn rounds_stage_seconds_up_to_the_next_tick() {
		let rate: TickRate = TickRate::new(NonZeroU32::new(30).expect("30 is positive"));
		assert_eq!(ticks_from_seconds_ceil(0.0, rate), Ok(0));
		assert_eq!(ticks_from_seconds_ceil(0.01, rate), Ok(1));
		assert_eq!(ticks_from_seconds_ceil(0.1, rate), Ok(3));
	}

	#[test]
	fn typed_timeline_preserves_visibility_timing_and_facing() {
		let first: GridPosition = GridPosition { row: 1, col: 1 };
		let second: GridPosition = GridPosition { row: 1, col: 2 };
		let draft: Vec<DraftRouteTimelineStep> = vec![
			DraftRouteTimelineStep {
				duration_ticks: 30,
				affected_by_movement_speed: true,
				action: DraftRouteTimelineAction::Move {
					cell: first,
					facing: Facing::Right,
				},
			},
			DraftRouteTimelineStep {
				duration_ticks: 15,
				affected_by_movement_speed: false,
				action: DraftRouteTimelineAction::Wait,
			},
			DraftRouteTimelineStep {
				duration_ticks: 0,
				affected_by_movement_speed: false,
				action: DraftRouteTimelineAction::Disappear,
			},
			DraftRouteTimelineStep {
				duration_ticks: 0,
				affected_by_movement_speed: false,
				action: DraftRouteTimelineAction::Reappear { cell: second },
			},
			DraftRouteTimelineStep {
				duration_ticks: 30,
				affected_by_movement_speed: true,
				action: DraftRouteTimelineAction::Goal {
					approach_facing: Some(Facing::Right),
				},
			},
		];
		assert_eq!(
			finalize_route_timeline(draft).expect("valid timeline"),
			vec![
				RouteTimelineStep {
					duration_ticks: 30,
					affected_by_movement_speed: true,
					action: RouteTimelineAction::Move {
						cell: first,
						facing: Facing::Right,
					},
				},
				RouteTimelineStep {
					duration_ticks: 15,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Wait,
				},
				RouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Disappear,
				},
				RouteTimelineStep {
					duration_ticks: 0,
					affected_by_movement_speed: false,
					action: RouteTimelineAction::Reappear {
						cell: second,
						facing: Facing::Right,
					},
				},
				RouteTimelineStep {
					duration_ticks: 30,
					affected_by_movement_speed: true,
					action: RouteTimelineAction::Goal,
				},
			]
		);
	}

	#[test]
	fn diagonal_route_facing_uses_horizontal_component() {
		assert_eq!(
			movement_facing(
				GridPosition { row: 0, col: 0 },
				GridPosition { row: 1, col: 1 },
			),
			Ok(Facing::Right)
		);
		assert_eq!(
			movement_facing(
				GridPosition { row: 1, col: 1 },
				GridPosition { row: 0, col: 0 },
			),
			Ok(Facing::Left)
		);
	}

	#[test]
	fn spawn_scatter_is_accepted_only_inside_one_cell() {
		for accepted in [(0.0, 0.0), (0.2, 0.2), (0.0, 0.2), (0.49, 0.49)] {
			assert!(
				spawn_random_range_is_zero_mean_within_one_cell(StageVector2 {
					x: accepted.0,
					y: accepted.1,
				}),
				"{accepted:?} stays inside the start cell"
			);
		}
		for rejected in [
			(-0.1, 0.0),
			(0.0, -0.1),
			(0.5, 0.0),
			(0.0, 3.0),
			(f64::NAN, 0.0),
			(f64::INFINITY, 0.0),
		] {
			assert!(
				!spawn_random_range_is_zero_mean_within_one_cell(StageVector2 {
					x: rejected.0,
					y: rejected.1,
				}),
				"{rejected:?} is not a zero-mean box inside one cell"
			);
		}
	}
}
