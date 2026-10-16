use std::collections::BTreeSet;
use std::num::{NonZeroU128, NonZeroU64};

use crate::data::GridPosition;
use crate::simulation::SimTick;

use super::envelope::{ReplacementCandidateBounds, SlotRequirementEnvelope, TickWindow};

/// Evaluates whether certified necessary conditions prove a candidate impossible.
pub trait Pruner {
	/// Keeps the candidate unless the returned public proof establishes impossibility.
	fn evaluate(
		&self,
		envelope: &SlotRequirementEnvelope,
		candidate: &ReplacementCandidateBounds,
	) -> PruneDecision;
}

/// Stateless hard pruner using reachability, cost, then optimistic damage in stable order.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HardPruner;

impl Pruner for HardPruner {
	fn evaluate(
		&self,
		envelope: &SlotRequirementEnvelope,
		candidate: &ReplacementCandidateBounds,
	) -> PruneDecision {
		if let (Some(engagement), Some(targetable)) = (
			envelope.engagement_cells_upper_bound(),
			candidate.targetable_cells_upper_bound(),
		) {
			if engagement.is_disjoint(targetable) {
				return PruneDecision::Prune {
					proof: PruneProof::Reachability {
						engagement_cells_upper_bound: engagement.clone(),
						targetable_cells_upper_bound: targetable.clone(),
					},
				};
			}
		}

		if let (Some(minimum_cost), Some(optimistic_dp)) = (
			candidate.minimum_deployment_cost(),
			envelope.optimistic_spendable_dp_upper_bound_through_duty_end(),
		) {
			if minimum_cost > optimistic_dp {
				return PruneDecision::Prune {
					proof: PruneProof::DeploymentCost {
						minimum_candidate_cost: minimum_cost,
						optimistic_spendable_dp_upper_bound_through_duty_end: optimistic_dp,
						duty_end: envelope.duty().end(),
					},
				};
			}
		}

		let (Some(required_damage), Some(damage_per_attack), Some(minimum_interval)) = (
			envelope.required_damage_lower_bound(),
			candidate.optimistic_damage_per_attack_upper_bound(),
			candidate.minimum_attack_interval_ticks(),
		) else {
			return PruneDecision::Keep;
		};
		let Some(elapsed_ticks) = envelope
			.duty()
			.end()
			.get()
			.checked_sub(envelope.duty().start().get())
		else {
			return PruneDecision::Keep;
		};
		let elapsed_ticks = u128::from(elapsed_ticks);
		let minimum_interval_ticks = u128::from(minimum_interval.get());
		let Some(attack_count) = elapsed_ticks
			.checked_div(minimum_interval_ticks)
			.and_then(|count| count.checked_add(1))
		else {
			return PruneDecision::Keep;
		};
		let Some(optimistic_damage) = attack_count.checked_mul(damage_per_attack) else {
			return PruneDecision::Keep;
		};
		if optimistic_damage < required_damage.get() {
			return PruneDecision::Prune {
				proof: PruneProof::OptimisticDamage {
					required_damage_lower_bound: required_damage,
					optimistic_damage_upper_bound: optimistic_damage,
					optimistic_attack_count: attack_count,
					duty: envelope.duty().clone(),
					damage_per_attack_upper_bound: damage_per_attack,
					minimum_attack_interval_ticks: minimum_interval,
				},
			};
		}

		PruneDecision::Keep
	}
}

/// Public result of hard-pruning evaluation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PruneDecision {
	/// No certified rule proved impossibility.
	Keep,
	/// A certified necessary condition failed.
	Prune { proof: PruneProof },
}

/// Exact certified values establishing why a candidate cannot satisfy the slot.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PruneProof {
	/// The complete engagement and targetable upper bounds are disjoint.
	Reachability {
		engagement_cells_upper_bound: BTreeSet<GridPosition>,
		targetable_cells_upper_bound: BTreeSet<GridPosition>,
	},
	/// Even the candidate's minimum cost exceeds all DP spendable through the deadline.
	DeploymentCost {
		minimum_candidate_cost: u32,
		optimistic_spendable_dp_upper_bound_through_duty_end: u32,
		duty_end: SimTick,
	},
	/// Even immediate attacks throughout the full inclusive duty window cannot meet the residual.
	OptimisticDamage {
		required_damage_lower_bound: NonZeroU128,
		optimistic_damage_upper_bound: u128,
		optimistic_attack_count: u128,
		duty: TickWindow,
		damage_per_attack_upper_bound: u128,
		minimum_attack_interval_ticks: NonZeroU64,
	},
}
