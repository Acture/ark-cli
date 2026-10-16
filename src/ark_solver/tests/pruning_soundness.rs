use std::collections::BTreeSet;
use std::num::{NonZeroU128, NonZeroU64};

use ark_solver::data::GridPosition;
use ark_solver::planning::{
	HardPruner, PruneDecision, Pruner, ReplacementCandidateBounds, SlotRequirementEnvelope,
	TickWindow,
};
use ark_solver::simulation::SimTick;

fn tick(value: u64) -> SimTick {
	SimTick::new(value)
}

fn cell(index: usize) -> GridPosition {
	GridPosition {
		row: 0,
		col: u32::try_from(index).expect("small-domain cell index fits u32"),
	}
}

fn cells_from_mask(mask: u8) -> BTreeSet<GridPosition> {
	(0..2)
		.filter(|index| mask & (1 << index) != 0)
		.map(cell)
		.collect()
}

fn attack_ticks(mask: u8) -> Vec<u64> {
	(0..=2).filter(|tick| mask & (1 << tick) != 0).collect()
}

fn exact_attack_schedule_is_legal(
	attack_ticks: &[u64],
	deployment_tick: u64,
	minimum_interval: u64,
) -> bool {
	attack_ticks.iter().all(|tick| *tick >= deployment_tick)
		&& attack_ticks
			.windows(2)
			.all(|pair| pair[1] - pair[0] >= minimum_interval)
}

#[test]
fn every_exact_success_in_small_domain_is_kept() {
	let duty = TickWindow::new(tick(0), tick(2)).unwrap();
	let mut exact_successes: usize = 0;

	for dp_at_zero in 0..=2 {
		for dp_at_one in 0..=2 {
			for dp_at_two in 0..=2 {
				let dp_by_tick = [dp_at_zero, dp_at_one, dp_at_two];
				let optimistic_dp = *dp_by_tick.iter().max().unwrap();
				for deployment_tick in 0..=2_u64 {
					for minimum_cost in 0..=1_u32 {
						for actual_cost in minimum_cost..=2 {
							let deployment_index =
								usize::try_from(deployment_tick).expect("small tick fits usize");
							if dp_by_tick[deployment_index] < actual_cost {
								continue;
							}
							for engagement_mask in 1..4_u8 {
								let engagement = cells_from_mask(engagement_mask);
								for targetable_mask in 0..4_u8 {
									let targetable = cells_from_mask(targetable_mask);
									for actual_target_index in 0..2 {
										let actual_target = cell(actual_target_index);
										if !engagement.contains(&actual_target)
											|| !targetable.contains(&actual_target)
										{
											continue;
										}
										for required_damage in 1..=3_u128 {
											for damage_upper_bound in 0..=2_u128 {
												for actual_damage_per_attack in
													0..=damage_upper_bound
												{
													for minimum_interval in 1..=2_u64 {
														for attack_mask in 0..8_u8 {
															let attacks = attack_ticks(attack_mask);
															if !exact_attack_schedule_is_legal(
																&attacks,
																deployment_tick,
																minimum_interval,
															) {
																continue;
															}
															let exact_damage =
																u128::try_from(attacks.len())
																	.unwrap()
																	* actual_damage_per_attack;
															if exact_damage < required_damage {
																continue;
															}

															exact_successes += 1;
															let envelope =
																SlotRequirementEnvelope::new(
																	Some(engagement.clone()),
																	duty.clone(),
																	Some(optimistic_dp),
																	NonZeroU128::new(
																		required_damage,
																	),
																)
																.unwrap();
															let candidate =
																ReplacementCandidateBounds::new(
																	"small-domain".to_owned(),
																	Some(minimum_cost),
																	Some(targetable.clone()),
																	Some(damage_upper_bound),
																	NonZeroU64::new(
																		minimum_interval,
																	),
																)
																.unwrap();

															assert_eq!(
																HardPruner.evaluate(&envelope, &candidate),
																PruneDecision::Keep,
																"exact success was pruned: deployment_tick={deployment_tick}, \
																actual_cost={actual_cost}, target={actual_target:?}, \
																attacks={attacks:?}, exact_damage={exact_damage}"
															);
														}
													}
												}
											}
										}
									}
								}
							}
						}
					}
				}
			}
		}
	}

	assert!(
		exact_successes > 0,
		"small-domain oracle must exercise successes"
	);
}
