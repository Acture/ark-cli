use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::num::{NonZeroU128, NonZeroU64};

use crate::data::GridPosition;
use crate::simulation::SimTick;

/// Inclusive window containing every attack time that may count toward a slot requirement.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TickWindow {
	start: SimTick,
	end: SimTick,
}

impl TickWindow {
	/// Creates an inclusive window, rejecting an end before its start.
	pub fn new(start: SimTick, end: SimTick) -> Result<Self, EnvelopeError> {
		if start > end {
			return Err(EnvelopeError::StartAfterEnd { start, end });
		}
		Ok(Self { start, end })
	}

	#[must_use]
	/// Returns the inclusive start.
	pub const fn start(&self) -> SimTick {
		self.start
	}

	#[must_use]
	/// Returns the inclusive end.
	pub const fn end(&self) -> SimTick {
		self.end
	}
}

/// Certified necessary conditions for satisfying one replacement slot.
///
/// Every optional field is `None` when its bound is unknown. Unknown bounds never prove pruning.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SlotRequirementEnvelope {
	engagement_cells_upper_bound: Option<BTreeSet<GridPosition>>,
	duty: TickWindow,
	optimistic_spendable_dp_upper_bound_through_duty_end: Option<u32>,
	required_damage_lower_bound: Option<NonZeroU128>,
}

impl SlotRequirementEnvelope {
	/// Creates a certified slot envelope.
	///
	/// `engagement_cells_upper_bound` must cover every cell where required damage could count, and
	/// `duty` must cover every time such damage could occur. The spendable-DP value is an upper
	/// bound at any legal deployment time through the inclusive duty end, not an end-of-duty balance
	/// after other deployments. `required_damage_lower_bound` is the residual after subtracting
	/// optimistic maximum contributions from every other variable slot; it must never be copied from
	/// the reference operator's damage.
	pub fn new(
		engagement_cells_upper_bound: Option<BTreeSet<GridPosition>>,
		duty: TickWindow,
		optimistic_spendable_dp_upper_bound_through_duty_end: Option<u32>,
		required_damage_lower_bound: Option<NonZeroU128>,
	) -> Result<Self, EnvelopeError> {
		if engagement_cells_upper_bound
			.as_ref()
			.is_some_and(BTreeSet::is_empty)
		{
			return Err(EnvelopeError::EmptyEngagementCellsUpperBound);
		}
		Ok(Self {
			engagement_cells_upper_bound,
			duty,
			optimistic_spendable_dp_upper_bound_through_duty_end,
			required_damage_lower_bound,
		})
	}

	#[must_use]
	/// Returns the certified all-possibilities engagement-cell upper bound.
	pub const fn engagement_cells_upper_bound(&self) -> Option<&BTreeSet<GridPosition>> {
		self.engagement_cells_upper_bound.as_ref()
	}

	#[must_use]
	/// Returns the inclusive certified duty window.
	pub const fn duty(&self) -> &TickWindow {
		&self.duty
	}

	#[must_use]
	/// Returns the certified spendable-DP upper bound through the inclusive duty end.
	pub const fn optimistic_spendable_dp_upper_bound_through_duty_end(&self) -> Option<u32> {
		self.optimistic_spendable_dp_upper_bound_through_duty_end
	}

	#[must_use]
	/// Returns the certified residual-damage lower bound.
	pub const fn required_damage_lower_bound(&self) -> Option<NonZeroU128> {
		self.required_damage_lower_bound
	}
}

/// Certified optimistic bounds for one replacement candidate.
///
/// Every optional field is `None` when its bound is unknown. Unknown bounds never prove pruning.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplacementCandidateBounds {
	stable_id: String,
	minimum_deployment_cost: Option<u32>,
	targetable_cells_upper_bound: Option<BTreeSet<GridPosition>>,
	optimistic_damage_per_attack_upper_bound: Option<u128>,
	minimum_attack_interval_ticks: Option<NonZeroU64>,
}

impl ReplacementCandidateBounds {
	/// Creates certified candidate bounds under a stable, non-blank identity.
	///
	/// Cost is a lower bound after every discount. Targetable cells are an upper bound covering all
	/// legal deployments, facings, attack modes, skills, summons, and RNG outcomes. Damage is an
	/// aggregate upper bound covering every damage source, while interval is a positive lower bound
	/// after every acceleration. Known empty targetable cells and known zero damage are valid.
	pub fn new(
		stable_id: String,
		minimum_deployment_cost: Option<u32>,
		targetable_cells_upper_bound: Option<BTreeSet<GridPosition>>,
		optimistic_damage_per_attack_upper_bound: Option<u128>,
		minimum_attack_interval_ticks: Option<NonZeroU64>,
	) -> Result<Self, CandidateError> {
		if stable_id.trim().is_empty() {
			return Err(CandidateError::BlankStableId);
		}
		Ok(Self {
			stable_id,
			minimum_deployment_cost,
			targetable_cells_upper_bound,
			optimistic_damage_per_attack_upper_bound,
			minimum_attack_interval_ticks,
		})
	}

	#[must_use]
	/// Returns the stable candidate identity.
	pub fn stable_id(&self) -> &str {
		&self.stable_id
	}

	#[must_use]
	/// Returns the certified minimum deployment cost.
	pub const fn minimum_deployment_cost(&self) -> Option<u32> {
		self.minimum_deployment_cost
	}

	#[must_use]
	/// Returns the certified all-possibilities targetable-cell upper bound.
	pub const fn targetable_cells_upper_bound(&self) -> Option<&BTreeSet<GridPosition>> {
		self.targetable_cells_upper_bound.as_ref()
	}

	#[must_use]
	/// Returns the certified aggregate damage-per-attack upper bound.
	pub const fn optimistic_damage_per_attack_upper_bound(&self) -> Option<u128> {
		self.optimistic_damage_per_attack_upper_bound
	}

	#[must_use]
	/// Returns the certified post-acceleration attack-interval lower bound.
	pub const fn minimum_attack_interval_ticks(&self) -> Option<NonZeroU64> {
		self.minimum_attack_interval_ticks
	}
}

/// Validation failure for a slot requirement envelope.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EnvelopeError {
	/// The inclusive window starts after it ends.
	StartAfterEnd { start: SimTick, end: SimTick },
	/// A known engagement upper bound is empty, so no requirement exists to satisfy.
	EmptyEngagementCellsUpperBound,
}

impl Display for EnvelopeError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::StartAfterEnd { start, end } => write!(
				formatter,
				"duty start {} is after duty end {}",
				start.get(),
				end.get()
			),
			Self::EmptyEngagementCellsUpperBound => {
				formatter.write_str("known engagement-cell upper bound must not be empty")
			}
		}
	}
}

impl Error for EnvelopeError {}

/// Validation failure for candidate bounds.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CandidateError {
	/// The stable identity contains only whitespace.
	BlankStableId,
}

impl Display for CandidateError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::BlankStableId => formatter.write_str("candidate stable ID must not be blank"),
		}
	}
}

impl Error for CandidateError {}
