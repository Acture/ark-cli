use std::collections::BTreeSet;
use std::num::{NonZeroU128, NonZeroU64};

use ark_solver::data::GridPosition;
use ark_solver::planning::{
	CandidateError, EnvelopeError, HardPruner, PruneDecision, PruneProof, Pruner,
	ReplacementCandidateBounds, SlotRequirementEnvelope, TickWindow,
};
use ark_solver::simulation::SimTick;

fn tick(value: u64) -> SimTick {
	SimTick::new(value)
}

fn cell(row: u32, col: u32) -> GridPosition {
	GridPosition { row, col }
}

fn cells(values: &[GridPosition]) -> BTreeSet<GridPosition> {
	values.iter().copied().collect()
}

fn nonzero_u64(value: u64) -> NonZeroU64 {
	NonZeroU64::new(value).expect("test intervals are non-zero")
}

fn nonzero_u128(value: u128) -> NonZeroU128 {
	NonZeroU128::new(value).expect("test requirements are non-zero")
}

fn make_envelope(
	engagement: Option<BTreeSet<GridPosition>>,
	duty: TickWindow,
	dp: Option<u32>,
	required_damage: Option<NonZeroU128>,
) -> SlotRequirementEnvelope {
	SlotRequirementEnvelope::new(engagement, duty, dp, required_damage)
		.expect("test envelope is valid")
}

fn make_candidate(
	cost: Option<u32>,
	targetable: Option<BTreeSet<GridPosition>>,
	damage: Option<u128>,
	interval: Option<NonZeroU64>,
) -> ReplacementCandidateBounds {
	ReplacementCandidateBounds::new("candidate".to_owned(), cost, targetable, damage, interval)
		.expect("test candidate is valid")
}

#[test]
fn disjoint_spatial_upper_bounds_produce_reachability_proof() {
	let engagement = cells(&[cell(0, 0)]);
	let targetable = cells(&[cell(1, 1)]);
	let envelope = make_envelope(
		Some(engagement.clone()),
		TickWindow::new(tick(0), tick(10)).unwrap(),
		None,
		None,
	);
	let candidate = make_candidate(None, Some(targetable.clone()), None, None);

	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::Reachability {
				engagement_cells_upper_bound: engagement,
				targetable_cells_upper_bound: targetable,
			},
		}
	);
}

#[test]
fn cost_above_optimistic_dp_produces_deployment_cost_proof() {
	let duty = TickWindow::new(tick(2), tick(9)).unwrap();
	let envelope = make_envelope(None, duty.clone(), Some(8), None);
	let candidate = make_candidate(Some(9), None, None, None);

	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::DeploymentCost {
				minimum_candidate_cost: 9,
				optimistic_spendable_dp_upper_bound_through_duty_end: 8,
				duty_end: tick(9),
			},
		}
	);
}

#[test]
fn optimistic_damage_one_below_requirement_produces_damage_proof() {
	let duty = TickWindow::new(tick(4), tick(10)).unwrap();
	let required = nonzero_u128(28);
	let interval = nonzero_u64(3);
	let envelope = make_envelope(None, duty.clone(), None, Some(required));
	let candidate = make_candidate(None, None, Some(9), Some(interval));

	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::OptimisticDamage {
				required_damage_lower_bound: required,
				optimistic_damage_upper_bound: 27,
				optimistic_attack_count: 3,
				duty,
				damage_per_attack_upper_bound: 9,
				minimum_attack_interval_ticks: interval,
			},
		}
	);
}

#[test]
fn damage_equal_to_requirement_keeps_candidate() {
	let duty = TickWindow::new(tick(4), tick(10)).unwrap();
	let envelope = make_envelope(None, duty, None, Some(nonzero_u128(27)));
	let candidate = make_candidate(None, None, Some(9), Some(nonzero_u64(3)));

	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Keep
	);
}

#[test]
fn strong_candidate_is_kept() {
	let shared = cell(1, 2);
	let duty = TickWindow::new(tick(0), tick(4)).unwrap();
	let envelope = make_envelope(
		Some(cells(&[shared])),
		duty,
		Some(10),
		Some(nonzero_u128(29)),
	);
	let candidate = make_candidate(
		Some(10),
		Some(cells(&[shared, cell(2, 2)])),
		Some(10),
		Some(nonzero_u64(2)),
	);

	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Keep
	);
}

#[test]
fn evaluation_order_is_reachability_then_cost_then_damage() {
	let duty = TickWindow::new(tick(0), tick(0)).unwrap();
	let engagement = cells(&[cell(0, 0)]);
	let targetable = cells(&[cell(1, 1)]);
	let envelope = make_envelope(
		Some(engagement.clone()),
		duty.clone(),
		Some(0),
		Some(nonzero_u128(2)),
	);
	let candidate = make_candidate(
		Some(1),
		Some(targetable.clone()),
		Some(1),
		Some(nonzero_u64(1)),
	);
	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::Reachability {
				engagement_cells_upper_bound: engagement,
				targetable_cells_upper_bound: targetable,
			},
		}
	);

	let envelope = make_envelope(None, duty, Some(0), Some(nonzero_u128(2)));
	let candidate = make_candidate(Some(1), None, Some(1), Some(nonzero_u64(1)));
	assert!(matches!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::DeploymentCost { .. }
		}
	));
}

#[test]
fn invalid_inputs_are_rejected_while_known_zero_candidate_bounds_are_valid() {
	assert_eq!(
		TickWindow::new(tick(3), tick(2)),
		Err(EnvelopeError::StartAfterEnd {
			start: tick(3),
			end: tick(2),
		})
	);
	let duty = TickWindow::new(tick(0), tick(1)).unwrap();
	assert_eq!(
		SlotRequirementEnvelope::new(Some(BTreeSet::new()), duty.clone(), None, None),
		Err(EnvelopeError::EmptyEngagementCellsUpperBound)
	);
	assert_eq!(
		ReplacementCandidateBounds::new(" \t".to_owned(), None, None, None, None),
		Err(CandidateError::BlankStableId)
	);
	assert!(ReplacementCandidateBounds::new(
		"zero-bounds".to_owned(),
		None,
		Some(BTreeSet::new()),
		Some(0),
		None,
	)
	.is_ok());
}

#[test]
fn getters_expose_every_validated_input() {
	let engagement = cells(&[cell(2, 3)]);
	let targetable = cells(&[cell(2, 3), cell(3, 3)]);
	let duty = TickWindow::new(tick(7), tick(11)).unwrap();
	let required = nonzero_u128(99);
	let envelope = make_envelope(
		Some(engagement.clone()),
		duty.clone(),
		Some(42),
		Some(required),
	);
	assert_eq!(duty.start(), tick(7));
	assert_eq!(duty.end(), tick(11));
	assert_eq!(envelope.engagement_cells_upper_bound(), Some(&engagement));
	assert_eq!(envelope.duty(), &duty);
	assert_eq!(
		envelope.optimistic_spendable_dp_upper_bound_through_duty_end(),
		Some(42)
	);
	assert_eq!(envelope.required_damage_lower_bound(), Some(required));

	let interval = nonzero_u64(6);
	let candidate = ReplacementCandidateBounds::new(
		"stable-id".to_owned(),
		Some(12),
		Some(targetable.clone()),
		Some(500),
		Some(interval),
	)
	.unwrap();
	assert_eq!(candidate.stable_id(), "stable-id");
	assert_eq!(candidate.minimum_deployment_cost(), Some(12));
	assert_eq!(candidate.targetable_cells_upper_bound(), Some(&targetable));
	assert_eq!(
		candidate.optimistic_damage_per_attack_upper_bound(),
		Some(500)
	);
	assert_eq!(candidate.minimum_attack_interval_ticks(), Some(interval));
}

#[test]
fn unknown_fields_never_prove_impossibility() {
	let duty = TickWindow::new(tick(0), tick(2)).unwrap();
	let known_engagement = cells(&[cell(0, 0)]);
	let known_targetable = cells(&[cell(1, 1)]);
	let cases = [
		(
			make_envelope(None, duty.clone(), None, None),
			make_candidate(None, Some(known_targetable.clone()), None, None),
		),
		(
			make_envelope(Some(known_engagement), duty.clone(), None, None),
			make_candidate(None, None, None, None),
		),
		(
			make_envelope(None, duty.clone(), None, None),
			make_candidate(Some(u32::MAX), None, None, None),
		),
		(
			make_envelope(None, duty.clone(), Some(0), None),
			make_candidate(None, None, None, None),
		),
		(
			make_envelope(None, duty.clone(), None, None),
			make_candidate(None, None, Some(0), Some(nonzero_u64(1))),
		),
		(
			make_envelope(None, duty.clone(), None, Some(nonzero_u128(1))),
			make_candidate(None, None, None, Some(nonzero_u64(1))),
		),
		(
			make_envelope(None, duty, None, Some(nonzero_u128(1))),
			make_candidate(None, None, Some(0), None),
		),
	];

	for (envelope, candidate) in cases {
		assert_eq!(
			HardPruner.evaluate(&envelope, &candidate),
			PruneDecision::Keep
		);
	}
}

#[test]
fn boundary_cases_are_optimistic_and_strict() {
	let shared = cell(0, 0);
	let duty = TickWindow::new(tick(5), tick(5)).unwrap();
	let envelope = make_envelope(
		Some(cells(&[shared])),
		duty.clone(),
		Some(7),
		Some(nonzero_u128(2)),
	);
	let candidate = make_candidate(
		Some(7),
		Some(cells(&[shared])),
		Some(1),
		Some(nonzero_u64(u64::MAX)),
	);
	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::OptimisticDamage {
				required_damage_lower_bound: nonzero_u128(2),
				optimistic_damage_upper_bound: 1,
				optimistic_attack_count: 1,
				duty,
				damage_per_attack_upper_bound: 1,
				minimum_attack_interval_ticks: nonzero_u64(u64::MAX),
			},
		}
	);
}

#[test]
fn arithmetic_near_integer_limits_is_checked() {
	let duty = TickWindow::new(tick(0), tick(u64::MAX)).unwrap();
	let attacks = u128::from(u64::MAX) + 1;
	let required = nonzero_u128(attacks + 1);
	let envelope = make_envelope(None, duty.clone(), None, Some(required));
	let candidate = make_candidate(None, None, Some(1), Some(nonzero_u64(1)));
	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Prune {
			proof: PruneProof::OptimisticDamage {
				required_damage_lower_bound: required,
				optimistic_damage_upper_bound: attacks,
				optimistic_attack_count: attacks,
				duty: duty.clone(),
				damage_per_attack_upper_bound: 1,
				minimum_attack_interval_ticks: nonzero_u64(1),
			},
		}
	);

	let envelope = make_envelope(None, duty, None, Some(nonzero_u128(u128::MAX)));
	let candidate = make_candidate(None, None, Some(u128::MAX), Some(nonzero_u64(1)));
	assert_eq!(
		HardPruner.evaluate(&envelope, &candidate),
		PruneDecision::Keep
	);
}
