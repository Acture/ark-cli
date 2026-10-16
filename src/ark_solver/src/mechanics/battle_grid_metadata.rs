//! Lossless battle-grid cell metadata and conservative, data-backed classifications.

use crate::data::{Buildable, GridPosition, MapCell, Passability, TileHeight};

const HOLE_TILE_KEY: &str = "tile_hole";
const STANDARD_PROTECTION_GOAL_TILE_KEY: &str = "tile_end";
const ALLY_PROTECTION_GOAL_TILE_KEY: &str = "tile_allygoal";

/// A positively identified friendly protection goal.
///
/// An unrecognized tile key is deliberately left unclassified rather than treated as either
/// variant.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FriendlyProtectionGoal {
	Standard,
	Ally,
}

/// Lossless metadata for one cell in a battle grid.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BattleGridCellMetadata {
	position: GridPosition,
	tile_key: String,
	height: TileHeight,
	buildable: Buildable,
	passability: Passability,
}

impl BattleGridCellMetadata {
	#[must_use]
	pub fn new(
		position: GridPosition,
		tile_key: impl Into<String>,
		height: TileHeight,
		buildable: Buildable,
		passability: Passability,
	) -> Self {
		Self {
			position,
			tile_key: tile_key.into(),
			height,
			buildable,
			passability,
		}
	}

	#[must_use]
	pub const fn position(&self) -> GridPosition {
		self.position
	}

	#[must_use]
	pub fn tile_key(&self) -> &str {
		&self.tile_key
	}

	#[must_use]
	pub const fn height(&self) -> &TileHeight {
		&self.height
	}

	#[must_use]
	pub const fn buildable(&self) -> &Buildable {
		&self.buildable
	}

	#[must_use]
	pub const fn passability(&self) -> &Passability {
		&self.passability
	}

	/// Returns true only for the exact, reviewed hole tile identity.
	#[must_use]
	pub fn is_hole(&self) -> bool {
		self.tile_key == HOLE_TILE_KEY
	}

	/// Returns true for non-hole lowland that explicitly permits walking.
	///
	/// Fly-only, impassable, and unknown height or passability values remain excluded.
	#[must_use]
	pub fn is_walkable_lowland(&self) -> bool {
		!self.is_hole()
			&& matches!(&self.height, TileHeight::Lowland)
			&& matches!(&self.passability, Passability::WalkOnly | Passability::All)
	}

	/// Classifies only exact, known friendly protection-goal tile identities.
	///
	/// `None` means unclassified, not that the tile is proven to be an ordinary non-goal tile.
	#[must_use]
	pub fn friendly_protection_goal(&self) -> Option<FriendlyProtectionGoal> {
		match self.tile_key.as_str() {
			STANDARD_PROTECTION_GOAL_TILE_KEY => Some(FriendlyProtectionGoal::Standard),
			ALLY_PROTECTION_GOAL_TILE_KEY => Some(FriendlyProtectionGoal::Ally),
			_ => None,
		}
	}
}

impl From<&MapCell> for BattleGridCellMetadata {
	fn from(cell: &MapCell) -> Self {
		Self::new(
			cell.position(),
			cell.tile_key(),
			cell.height().clone(),
			cell.buildable().clone(),
			cell.passability().clone(),
		)
	}
}

#[cfg(test)]
mod tests {
	use crate::data::{Buildable, GridPosition, Passability, TileHeight};

	use super::{BattleGridCellMetadata, FriendlyProtectionGoal};

	fn cell(
		tile_key: &str,
		height: TileHeight,
		buildable: Buildable,
		passability: Passability,
	) -> BattleGridCellMetadata {
		BattleGridCellMetadata::new(
			GridPosition { row: 2, col: 3 },
			tile_key,
			height,
			buildable,
			passability,
		)
	}

	#[test]
	fn walkable_lowland_requires_explicit_walk_passability() {
		let walk_only: BattleGridCellMetadata = cell(
			"tile_road",
			TileHeight::Lowland,
			Buildable::Melee,
			Passability::WalkOnly,
		);
		let all: BattleGridCellMetadata = cell(
			"tile_road",
			TileHeight::Lowland,
			Buildable::All,
			Passability::All,
		);
		let fly_only: BattleGridCellMetadata = cell(
			"tile_road",
			TileHeight::Lowland,
			Buildable::None,
			Passability::FlyOnly,
		);

		assert!(walk_only.is_walkable_lowland());
		assert!(all.is_walkable_lowland());
		assert!(!fly_only.is_walkable_lowland());
	}

	#[test]
	fn hole_is_excluded_even_when_lowland_and_fully_passable() {
		let hole: BattleGridCellMetadata = cell(
			"tile_hole",
			TileHeight::Lowland,
			Buildable::All,
			Passability::All,
		);

		assert!(hole.is_hole());
		assert!(!hole.is_walkable_lowland());
	}

	#[test]
	fn friendly_protection_goals_keep_standard_and_ally_tiles_distinct() {
		let standard: BattleGridCellMetadata = cell(
			"tile_end",
			TileHeight::Lowland,
			Buildable::None,
			Passability::All,
		);
		let ally: BattleGridCellMetadata = cell(
			"tile_allygoal",
			TileHeight::Lowland,
			Buildable::None,
			Passability::All,
		);
		let unknown: BattleGridCellMetadata = cell(
			"tile_future_goal",
			TileHeight::Lowland,
			Buildable::None,
			Passability::All,
		);

		assert_eq!(
			standard.friendly_protection_goal(),
			Some(FriendlyProtectionGoal::Standard)
		);
		assert_eq!(
			ally.friendly_protection_goal(),
			Some(FriendlyProtectionGoal::Ally)
		);
		assert_eq!(unknown.friendly_protection_goal(), None);
		assert_eq!(unknown.tile_key(), "tile_future_goal");
	}

	#[test]
	fn unknown_grid_values_are_preserved_and_not_treated_as_walkable() {
		let metadata: BattleGridCellMetadata = cell(
			"tile_future",
			TileHeight::Unknown("VOIDISH".to_owned()),
			Buildable::Unknown("SPECIAL".to_owned()),
			Passability::Unknown("PHASED".to_owned()),
		);

		assert_eq!(
			metadata.height(),
			&TileHeight::Unknown("VOIDISH".to_owned())
		);
		assert_eq!(
			metadata.buildable(),
			&Buildable::Unknown("SPECIAL".to_owned())
		);
		assert_eq!(
			metadata.passability(),
			&Passability::Unknown("PHASED".to_owned())
		);
		assert!(!metadata.is_walkable_lowland());
		assert_eq!(metadata.friendly_protection_goal(), None);
	}
}
