use crate::data::GridPosition;
use serde::{Deserialize, Serialize};

use crate::mechanics::domain::Facing;

/// Half-open tick interval `[start, end)`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct TickSpan {
	pub start: u64,
	pub end: u64,
}

impl TickSpan {
	#[must_use]
	pub const fn contains(self, tick: u64) -> bool {
		self.start <= tick && tick < self.end
	}
}

/// One exact base-panel phase keyframe.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BasePanelBuildSpec {
	pub operator_id: String,
	pub elite: u8,
	pub level: u32,
}

/// Exact deployment geometry of one prepared retained or replacement action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DeploymentAnchor {
	pub cell: GridPosition,
	pub facing: Facing,
}

/// Candidate builds available while materializing a guide deployment domain.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidatePoolSpec {
	Roster { builds: Vec<BasePanelBuildSpec> },
	CatalogPreset { preset: CatalogPreset },
}

/// Explicit catalog build policy. It is intentionally not called "maxed".
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogPreset {
	HighestPhaseBasePanel,
}

/// Event-relative stage time that may be bound to an absolute replay tick.
///
/// Wave and branch starts are strategy-dependent in stages whose scheduler waits for kills.
/// Keeping the anchor explicit prevents planning from inventing an absolute spawn time.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StageTimeAnchor {
	BattleStart,
	WaveStart { wave: u32 },
	WaveFragmentStart { wave: u32, fragment: u32 },
	BranchPhaseStart { branch: String, phase: u32 },
}

/// One absolute replay tick for an event-relative stage anchor.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct StageTimeBinding {
	pub anchor: StageTimeAnchor,
	pub tick: u64,
}

#[cfg(test)]
mod tests {
	use super::TickSpan;

	#[test]
	fn tick_span_contains_only_its_half_open_interval() {
		let span: TickSpan = TickSpan { start: 10, end: 20 };

		assert!(!span.contains(9));
		assert!(span.contains(10));
		assert!(span.contains(19));
		assert!(!span.contains(20));
	}
}
