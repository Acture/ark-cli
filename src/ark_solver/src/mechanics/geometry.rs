use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::data::{AttackRange, GridPosition, MapDimensions, RangeOffset};
use serde::{Deserialize, Serialize};

/// Canonical deployment facing used by analysis queries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
	Up,
	Right,
	Down,
	Left,
}

/// Projects one source range into bounded stage cells.
pub fn project_attack_cells(
	anchor: GridPosition,
	facing: Facing,
	range: &AttackRange,
	dimensions: MapDimensions,
) -> Result<BTreeSet<GridPosition>, RangeProjectionError> {
	if range.source_direction() != 1 {
		return Err(RangeProjectionError::UnsupportedSourceDirection {
			range_id: range.id().to_owned(),
			source_direction: range.source_direction(),
		});
	}
	Ok(project_offsets(anchor, facing, range.offsets(), dimensions))
}

/// Rotates right-facing source offsets, translates them, and clips them to the map.
pub fn project_offsets(
	anchor: GridPosition,
	facing: Facing,
	offsets: &[RangeOffset],
	dimensions: MapDimensions,
) -> BTreeSet<GridPosition> {
	offsets
		.iter()
		.filter_map(|offset: &RangeOffset| {
			let (row_offset, col_offset): (i32, i32) =
				rotate_offset(offset.row(), offset.col(), facing);
			let row: i64 = i64::from(anchor.row) + i64::from(row_offset);
			let col: i64 = i64::from(anchor.col) + i64::from(col_offset);
			if row < 0
				|| col < 0 || row >= i64::from(dimensions.height)
				|| col >= i64::from(dimensions.width)
			{
				return None;
			}
			Some(GridPosition {
				row: row as u32,
				col: col as u32,
			})
		})
		.collect()
}

fn rotate_offset(row: i32, col: i32, facing: Facing) -> (i32, i32) {
	match facing {
		Facing::Right => (row, col),
		Facing::Down => (-col, row),
		Facing::Left => (-row, -col),
		Facing::Up => (col, -row),
	}
}

/// A source range cannot be projected without a declared normalization rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RangeProjectionError {
	UnsupportedSourceDirection {
		range_id: String,
		source_direction: i32,
	},
}

impl Display for RangeProjectionError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::UnsupportedSourceDirection {
				range_id,
				source_direction,
			} => write!(
				formatter,
				"range {range_id:?} uses unsupported source direction {source_direction}"
			),
		}
	}
}

impl Error for RangeProjectionError {}

#[cfg(test)]
mod tests {
	use crate::data::{GridPosition, MapDimensions, RangeOffset};

	use super::{project_offsets, Facing};

	#[test]
	fn rotates_right_facing_offsets_in_all_directions_and_clips() {
		let offsets: Vec<RangeOffset> = vec![
			RangeOffset::new(0, 0),
			RangeOffset::new(0, 1),
			RangeOffset::new(1, 0),
			RangeOffset::new(0, 5),
		];
		let dimensions: MapDimensions = MapDimensions {
			width: 3,
			height: 3,
		};
		let anchor: GridPosition = GridPosition { row: 1, col: 1 };

		assert_eq!(
			project_offsets(anchor, Facing::Right, &offsets, dimensions),
			[
				GridPosition { row: 1, col: 1 },
				GridPosition { row: 1, col: 2 },
				GridPosition { row: 2, col: 1 },
			]
			.into_iter()
			.collect()
		);
		assert_eq!(
			project_offsets(anchor, Facing::Down, &offsets, dimensions),
			[
				GridPosition { row: 0, col: 1 },
				GridPosition { row: 1, col: 1 },
				GridPosition { row: 1, col: 2 },
			]
			.into_iter()
			.collect()
		);
		assert_eq!(
			project_offsets(anchor, Facing::Left, &offsets, dimensions),
			[
				GridPosition { row: 0, col: 1 },
				GridPosition { row: 1, col: 0 },
				GridPosition { row: 1, col: 1 },
			]
			.into_iter()
			.collect()
		);
		assert_eq!(
			project_offsets(anchor, Facing::Up, &offsets, dimensions),
			[
				GridPosition { row: 1, col: 0 },
				GridPosition { row: 1, col: 1 },
				GridPosition { row: 2, col: 1 },
			]
			.into_iter()
			.collect()
		);
	}
}
