//! Fail-closed full-battle planning inputs over deterministic unblocked trajectories.

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::CompiledStage;
use crate::simulation::TickRate;

use super::{
	battle_gap::{
		FullBattleDamageGapLedger, FullBattleEnemyDamageGap, FullBattleEnemyResolution,
		FullBattleWaveTiming,
	},
	query::{StageTimeAnchor, StageTimeBinding, TickSpan},
	stage_space_time::{
		bind_stage_space_time, compile_stage_space_time_template, ticks_from_seconds_ceil,
		EnemyTrajectory, RelativeEnemyTrajectory, StageSpaceTimeError, StageSpaceTimeModel,
		StageSpaceTimeTemplate,
	},
};
use crate::mechanics::{
	domain::{
		BattleTimeAnchor, FixedDamage, FixedDamageError, RelativeBattleTime, WaveId,
		WaveStartWindow,
	},
	stage::{SpawnCohortId, SpawnInstanceId},
};

/// Stable identity of one cumulative unblocked goal-deadline HP cut.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HpGroupId(u64);

impl HpGroupId {
	#[must_use]
	pub const fn new(deadline_tick: u64) -> Self {
		Self(deadline_tick)
	}

	#[must_use]
	pub const fn deadline_tick(self) -> u64 {
		self.0
	}
}

impl Display for HpGroupId {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		write!(formatter, "goal_deadline_prefix:{}", self.0)
	}
}

/// Cumulative full enemy HP due no later than one unblocked goal deadline.
///
/// Members include every stage enemy whose unblocked route reaches its goal at or before
/// `deadline_tick`. Retained and replacement output must be compared against the same total-HP
/// prefix so target-order changes cannot invalidate a residual-only cut.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpaceTimeHpGroup {
	pub id: HpGroupId,
	pub deadline_tick: u64,
	pub members: BTreeSet<SpawnInstanceId>,
	pub required_hp: FixedDamage,
}

/// Stable identity of one cumulative HP cut within a causal stage anchor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WaveLocalHpGroupId {
	pub anchor: StageTimeAnchor,
	pub goal_deadline_offset_ticks: u64,
}

/// Cumulative full enemy HP due at one event-relative route deadline.
///
/// Members never cross anchors. Moving one wave in absolute battle time therefore cannot change
/// this requirement or its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaveLocalHpGroup {
	pub id: WaveLocalHpGroupId,
	pub members: BTreeSet<SpawnInstanceId>,
	pub required_hp: FixedDamage,
}

/// Inclusive bounds on one causal event tick.
///
/// `latest == None` means the event may be delayed without a finite proven bound, for example by
/// blocking an enemy from the preceding fragment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventTickBounds {
	pub earliest: u64,
	pub latest: Option<u64>,
}

/// Absolute timing observed in the retained-only replay.
///
/// This is audit evidence only. It is never used as a planning constraint for a later wave.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedWaveTimingWitness {
	pub started_tick: u64,
	pub cleared_tick: Option<u64>,
}

/// Event-relative start rule and optimistic absolute bounds for one active wave fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CausalWaveTiming {
	pub stage_anchor: StageTimeAnchor,
	pub replay_wave_id: u32,
	pub start: WaveStartWindow,
	pub start_bounds: EventTickBounds,
	pub retained_witness: RetainedWaveTimingWitness,
}

/// Full-stage unblocked geometry paired with cumulative full-HP deadline groups.
#[derive(Clone, Debug, PartialEq)]
pub struct FullBattlePlanningContext {
	/// Event-relative geometry used by causal hard pruning.
	pub template: StageSpaceTimeTemplate,
	/// Causal fragment starts. Retained absolute ticks are stored only in each witness.
	pub causal_waves: Vec<CausalWaveTiming>,
	/// Full-HP cuts local to each causal fragment.
	pub wave_local_hp_groups: Vec<WaveLocalHpGroup>,
	/// Retained-witness absolute projection retained for exact compatibility paths.
	///
	/// It must not be used to prove a later-wave proposal impossible.
	pub model: StageSpaceTimeModel,
	/// Retained-witness horizon; causal later waves may have no finite latest tick.
	pub duty_window: TickSpan,
	/// Every cohort in the complete active stage.
	pub target_cohort_ids: BTreeSet<String>,
	/// Full-HP cumulative cuts ordered by unblocked goal deadline.
	pub hp_groups: Vec<SpaceTimeHpGroup>,
}

/// Compiles the complete active stage and aligns it with one retained-only replay ledger.
///
/// The currently supported replay adapter maps its causal wave IDs to fragments `0..n` under
/// stage wave `0`. Any other active anchor shape fails closed instead of silently dropping
/// enemies from planning.
pub fn prepare_full_battle_unblocked_planning_context(
	stage: &CompiledStage,
	tick_rate: TickRate,
	ledger: &FullBattleDamageGapLedger,
) -> Result<FullBattlePlanningContext, FullBattlePlanningError> {
	let template: StageSpaceTimeTemplate = compile_stage_space_time_template(stage, tick_rate)
		.map_err(FullBattlePlanningError::StageSpaceTime)?;
	let bindings: Vec<StageTimeBinding> = full_battle_bindings(ledger)?;
	validate_complete_anchor_binding(&template, &bindings)?;
	validate_template_counts(&template)?;
	let causal_waves: Vec<CausalWaveTiming> =
		compile_causal_wave_timings(stage, &template, ledger, tick_rate)?;
	let model: StageSpaceTimeModel = bind_stage_space_time(&template, &bindings)
		.map_err(FullBattlePlanningError::StageSpaceTime)?;
	if model.cohorts.len() != template.cohorts.len() {
		return Err(FullBattlePlanningError::BoundCohortCountMismatch {
			template: template.cohorts.len(),
			bound: model.cohorts.len(),
		});
	}
	if model.enemies.len() != template.enemies.len() {
		return Err(FullBattlePlanningError::BoundEnemyCountMismatch {
			template: template.enemies.len(),
			bound: model.enemies.len(),
		});
	}
	assemble_full_battle_context(model, template, causal_waves, ledger)
}

fn full_battle_bindings(
	ledger: &FullBattleDamageGapLedger,
) -> Result<Vec<StageTimeBinding>, FullBattlePlanningError> {
	if ledger.wave_timings.is_empty() {
		return Err(FullBattlePlanningError::EmptyWaveTimings);
	}
	let mut starts_by_wave: BTreeMap<u32, u64> = BTreeMap::new();
	for timing in &ledger.wave_timings {
		let started_tick: u64 =
			timing
				.started_tick
				.ok_or(FullBattlePlanningError::MissingWaveStart {
					wave_id: timing.wave_id,
				})?;
		if starts_by_wave
			.insert(timing.wave_id, started_tick)
			.is_some()
		{
			return Err(FullBattlePlanningError::DuplicateWaveTiming {
				wave_id: timing.wave_id,
			});
		}
	}
	Ok(starts_by_wave
		.into_iter()
		.map(|(wave_id, tick)| StageTimeBinding {
			anchor: StageTimeAnchor::WaveFragmentStart {
				wave: 0,
				fragment: wave_id,
			},
			tick,
		})
		.collect())
}

fn compile_causal_wave_timings(
	stage: &CompiledStage,
	template: &StageSpaceTimeTemplate,
	ledger: &FullBattleDamageGapLedger,
	tick_rate: TickRate,
) -> Result<Vec<CausalWaveTiming>, FullBattlePlanningError> {
	if stage.waves().len() != 1 {
		return Err(FullBattlePlanningError::UnsupportedCausalStageWaveCount {
			found: stage.waves().len(),
		});
	}
	let stage_wave = &stage.waves()[0];
	let stage_pre_delay_ticks: u64 =
		ticks_from_seconds_ceil(stage_wave.pre_delay_seconds(), tick_rate)
			.map_err(FullBattlePlanningError::StageSpaceTime)?;
	let fragment_pre_delay_ticks: Vec<u64> = stage_wave
		.fragments()
		.iter()
		.map(|fragment| {
			ticks_from_seconds_ceil(fragment.pre_delay_seconds(), tick_rate)
				.map_err(FullBattlePlanningError::StageSpaceTime)
		})
		.collect::<Result<_, _>>()?;
	build_causal_wave_timings(
		stage_pre_delay_ticks,
		&fragment_pre_delay_ticks,
		template,
		&ledger.wave_timings,
	)
}

fn build_causal_wave_timings(
	stage_pre_delay_ticks: u64,
	fragment_pre_delay_ticks: &[u64],
	template: &StageSpaceTimeTemplate,
	retained_timings: &[FullBattleWaveTiming],
) -> Result<Vec<CausalWaveTiming>, FullBattlePlanningError> {
	let mut retained_by_wave: BTreeMap<u32, &FullBattleWaveTiming> = BTreeMap::new();
	for timing in retained_timings {
		if retained_by_wave.insert(timing.wave_id, timing).is_some() {
			return Err(FullBattlePlanningError::DuplicateWaveTiming {
				wave_id: timing.wave_id,
			});
		}
	}
	if retained_by_wave.len() != fragment_pre_delay_ticks.len() {
		return Err(FullBattlePlanningError::CausalWaveCountMismatch {
			fragments: fragment_pre_delay_ticks.len(),
			retained: retained_by_wave.len(),
		});
	}

	let mut timings: Vec<CausalWaveTiming> = Vec::with_capacity(fragment_pre_delay_ticks.len());
	for (fragment_index, clear_delay_ticks) in fragment_pre_delay_ticks.iter().copied().enumerate()
	{
		let replay_wave_id: u32 = u32::try_from(fragment_index)
			.map_err(|_| FullBattlePlanningError::CausalWaveIdentifierOverflow)?;
		let retained: &FullBattleWaveTiming = retained_by_wave
			.remove(&replay_wave_id)
			.ok_or(FullBattlePlanningError::MissingCausalWaveTiming { replay_wave_id })?;
		let retained_started_tick: u64 =
			retained
				.started_tick
				.ok_or(FullBattlePlanningError::MissingWaveStart {
					wave_id: replay_wave_id,
				})?;
		let stage_anchor: StageTimeAnchor = StageTimeAnchor::WaveFragmentStart {
			wave: 0,
			fragment: replay_wave_id,
		};
		let (start, start_bounds): (WaveStartWindow, EventTickBounds) = if fragment_index == 0 {
			let fixed_start_tick: u64 = stage_pre_delay_ticks
				.checked_add(clear_delay_ticks)
				.ok_or(FullBattlePlanningError::CausalTickOverflow { replay_wave_id })?;
			(
				WaveStartWindow {
					earliest: RelativeBattleTime {
						anchor: BattleTimeAnchor::BattleStarted,
						offset_ticks: fixed_start_tick,
					},
					latest: None,
				},
				EventTickBounds {
					earliest: fixed_start_tick,
					latest: Some(fixed_start_tick),
				},
			)
		} else {
			let predecessor: &CausalWaveTiming = timings
				.last()
				.expect("a later fragment always has one predecessor");
			let optimistic_clear_offset_ticks: u64 = template
				.optimistic_clear_offset_ticks(&predecessor.stage_anchor)
				.unwrap_or_default();
			let predecessor_earliest_clear_tick: u64 = predecessor
				.start_bounds
				.earliest
				.checked_add(optimistic_clear_offset_ticks)
				.ok_or(FullBattlePlanningError::CausalTickOverflow { replay_wave_id })?;
			let earliest: u64 = predecessor_earliest_clear_tick
				.checked_add(clear_delay_ticks)
				.ok_or(FullBattlePlanningError::CausalTickOverflow { replay_wave_id })?;
			(
				WaveStartWindow {
					earliest: RelativeBattleTime {
						anchor: BattleTimeAnchor::WaveCleared(WaveId::new(replay_wave_id - 1)),
						offset_ticks: clear_delay_ticks,
					},
					latest: None,
				},
				EventTickBounds {
					earliest,
					latest: None,
				},
			)
		};
		timings.push(CausalWaveTiming {
			stage_anchor,
			replay_wave_id,
			start,
			start_bounds,
			retained_witness: RetainedWaveTimingWitness {
				started_tick: retained_started_tick,
				cleared_tick: retained.cleared_tick,
			},
		});
	}
	if let Some(unexpected) = retained_by_wave.keys().next().copied() {
		return Err(FullBattlePlanningError::UnexpectedCausalWaveTiming {
			replay_wave_id: unexpected,
		});
	}
	Ok(timings)
}

fn validate_complete_anchor_binding(
	template: &StageSpaceTimeTemplate,
	bindings: &[StageTimeBinding],
) -> Result<(), FullBattlePlanningError> {
	let expected: BTreeSet<StageTimeAnchor> = template
		.cohorts
		.iter()
		.map(|cohort| cohort.anchor.clone())
		.chain(template.enemies.iter().map(|enemy| enemy.anchor.clone()))
		.collect();
	let found: BTreeSet<StageTimeAnchor> = bindings
		.iter()
		.map(|binding| binding.anchor.clone())
		.collect();
	if expected != found {
		return Err(FullBattlePlanningError::AnchorSetMismatch {
			missing: expected.difference(&found).cloned().collect(),
			unexpected: found.difference(&expected).cloned().collect(),
		});
	}
	Ok(())
}

fn validate_template_counts(
	template: &StageSpaceTimeTemplate,
) -> Result<(), FullBattlePlanningError> {
	let mut instance_counts: BTreeMap<&SpawnCohortId, usize> = BTreeMap::new();
	for enemy in &template.enemies {
		*instance_counts
			.entry(&enemy.instance_id.cohort)
			.or_default() += 1;
	}
	for cohort in &template.cohorts {
		let actual: usize = instance_counts.get(&cohort.id).copied().unwrap_or_default();
		let declared: usize = usize::try_from(cohort.count).unwrap_or(usize::MAX);
		if actual != declared {
			return Err(
				FullBattlePlanningError::TemplateCohortInstanceCountMismatch {
					cohort_id: cohort.id.as_str().to_owned(),
					declared,
					actual,
				},
			);
		}
	}
	Ok(())
}

fn assemble_full_battle_context(
	model: StageSpaceTimeModel,
	template: StageSpaceTimeTemplate,
	causal_waves: Vec<CausalWaveTiming>,
	ledger: &FullBattleDamageGapLedger,
) -> Result<FullBattlePlanningContext, FullBattlePlanningError> {
	if model.enemies.is_empty() {
		return Err(FullBattlePlanningError::EmptyStageModel);
	}
	if ledger.enemies.len() != model.enemies.len() {
		return Err(FullBattlePlanningError::LedgerEnemyCountMismatch {
			ledger: ledger.enemies.len(),
			model: model.enemies.len(),
		});
	}

	let mut model_enemies: BTreeMap<&SpawnInstanceId, &EnemyTrajectory> = BTreeMap::new();
	let mut instances_by_cohort: BTreeMap<&SpawnCohortId, usize> = BTreeMap::new();
	for enemy in &model.enemies {
		if model_enemies.insert(&enemy.instance_id, enemy).is_some() {
			return Err(FullBattlePlanningError::DuplicateModelTarget {
				target: enemy.instance_id.clone(),
			});
		}
		*instances_by_cohort
			.entry(&enemy.instance_id.cohort)
			.or_default() += 1;
		if enemy.occupancy.is_empty() {
			return Err(FullBattlePlanningError::EmptyEnemyOccupancy {
				target: enemy.instance_id.clone(),
			});
		}
	}

	let mut model_cohorts: BTreeSet<&SpawnCohortId> = BTreeSet::new();
	for cohort in &model.cohorts {
		if !model_cohorts.insert(&cohort.id) {
			return Err(FullBattlePlanningError::DuplicateModelCohort {
				cohort_id: cohort.id.as_str().to_owned(),
			});
		}
		let actual: usize = instances_by_cohort
			.get(&cohort.id)
			.copied()
			.unwrap_or_default();
		let declared: usize = usize::try_from(cohort.count).unwrap_or(usize::MAX);
		if actual != declared {
			return Err(FullBattlePlanningError::ModelCohortInstanceCountMismatch {
				cohort_id: cohort.id.as_str().to_owned(),
				declared,
				actual,
			});
		}
	}

	let ledger_cohorts: BTreeSet<&SpawnCohortId> = ledger
		.enemies
		.iter()
		.map(|enemy| &enemy.planning_target.cohort)
		.collect();
	if ledger_cohorts.len() != model_cohorts.len() {
		return Err(FullBattlePlanningError::LedgerCohortCountMismatch {
			ledger: ledger_cohorts.len(),
			model: model_cohorts.len(),
		});
	}
	for cohort_id in &ledger_cohorts {
		if !model_cohorts.contains(cohort_id) {
			return Err(FullBattlePlanningError::MissingPlanningCohort {
				cohort_id: cohort_id.as_str().to_owned(),
			});
		}
	}

	let mut seen_ledger_targets: BTreeSet<&SpawnInstanceId> = BTreeSet::new();
	for enemy in &ledger.enemies {
		validate_ledger_enemy(enemy, &model_enemies, &mut seen_ledger_targets)?;
	}

	let hp_groups: Vec<SpaceTimeHpGroup> = build_space_time_hp_groups(&model, ledger)?;
	let wave_local_hp_groups: Vec<WaveLocalHpGroup> =
		build_wave_local_hp_groups(&template, ledger)?;
	let target_cohort_ids: BTreeSet<String> = model
		.cohorts
		.iter()
		.map(|cohort| cohort.id.as_str().to_owned())
		.collect();
	let maximum_goal_deadline: u64 = model
		.enemies
		.iter()
		.map(|enemy| enemy.goal_deadline_tick)
		.max()
		.expect("nonempty model checked above");
	if maximum_goal_deadline == 0 {
		return Err(FullBattlePlanningError::EmptyDutyWindow);
	}
	let maximum_failure_deadline: u64 = ledger
		.enemies
		.iter()
		.filter(|enemy| enemy.required_damage_micros > 0)
		.filter_map(|enemy| enemy.failure_deadline_tick)
		.max()
		.expect("positive-gap validation guarantees a replay failure deadline");
	let planning_deadline: u64 = maximum_goal_deadline.max(maximum_failure_deadline);

	Ok(FullBattlePlanningContext {
		template,
		causal_waves,
		wave_local_hp_groups,
		model,
		duty_window: TickSpan {
			start: 0,
			end: planning_deadline,
		},
		target_cohort_ids,
		hp_groups,
	})
}

fn build_wave_local_hp_groups(
	template: &StageSpaceTimeTemplate,
	ledger: &FullBattleDamageGapLedger,
) -> Result<Vec<WaveLocalHpGroup>, FullBattlePlanningError> {
	let mut seen_targets: BTreeSet<&SpawnInstanceId> = BTreeSet::new();
	let mut has_positive_gap: bool = false;
	for enemy in &ledger.enemies {
		if !seen_targets.insert(&enemy.planning_target) {
			return Err(FullBattlePlanningError::DuplicateLedgerTarget {
				target: enemy.planning_target.clone(),
			});
		}
		has_positive_gap |= validated_positive_gap_deadline(enemy)?.is_some();
	}
	if !has_positive_gap {
		return Err(FullBattlePlanningError::EmptyHpGroups);
	}

	let mut enemies_by_anchor_and_deadline: BTreeMap<
		StageTimeAnchor,
		BTreeMap<u64, Vec<&RelativeEnemyTrajectory>>,
	> = BTreeMap::new();
	for enemy in &template.enemies {
		enemies_by_anchor_and_deadline
			.entry(enemy.anchor.clone())
			.or_default()
			.entry(enemy.goal_deadline_offset_ticks)
			.or_default()
			.push(enemy);
	}

	let mut groups: Vec<WaveLocalHpGroup> = Vec::new();
	for (anchor, enemies_by_deadline) in enemies_by_anchor_and_deadline {
		let mut members: BTreeSet<SpawnInstanceId> = BTreeSet::new();
		let mut required_hp: FixedDamage = FixedDamage::ZERO;
		for (goal_deadline_offset_ticks, mut due_enemies) in enemies_by_deadline {
			due_enemies.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
			for enemy in due_enemies {
				let maximum_hp: FixedDamage =
					FixedDamage::from_whole(enemy.max_hp).map_err(|source| {
						FullBattlePlanningError::WaveLocalHpGroupRequiredHpOverflow {
							anchor: anchor.clone(),
							goal_deadline_offset_ticks,
							source,
						}
					})?;
				required_hp = required_hp.checked_add(maximum_hp).map_err(|source| {
					FullBattlePlanningError::WaveLocalHpGroupRequiredHpOverflow {
						anchor: anchor.clone(),
						goal_deadline_offset_ticks,
						source,
					}
				})?;
				members.insert(enemy.instance_id.clone());
			}
			groups.push(WaveLocalHpGroup {
				id: WaveLocalHpGroupId {
					anchor: anchor.clone(),
					goal_deadline_offset_ticks,
				},
				members: members.clone(),
				required_hp,
			});
		}
	}
	Ok(groups)
}

fn build_space_time_hp_groups(
	model: &StageSpaceTimeModel,
	ledger: &FullBattleDamageGapLedger,
) -> Result<Vec<SpaceTimeHpGroup>, FullBattlePlanningError> {
	let mut seen_targets: BTreeSet<&SpawnInstanceId> = BTreeSet::new();
	let mut has_positive_gap: bool = false;
	for enemy in &ledger.enemies {
		if !seen_targets.insert(&enemy.planning_target) {
			return Err(FullBattlePlanningError::DuplicateLedgerTarget {
				target: enemy.planning_target.clone(),
			});
		}
		has_positive_gap |= validated_positive_gap_deadline(enemy)?.is_some();
	}
	if !has_positive_gap {
		return Err(FullBattlePlanningError::EmptyHpGroups);
	}

	let mut enemies_by_deadline: BTreeMap<u64, Vec<&EnemyTrajectory>> = BTreeMap::new();
	for enemy in &model.enemies {
		enemies_by_deadline
			.entry(enemy.goal_deadline_tick)
			.or_default()
			.push(enemy);
	}

	let mut members: BTreeSet<SpawnInstanceId> = BTreeSet::new();
	let mut required_hp: FixedDamage = FixedDamage::ZERO;
	let mut groups: Vec<SpaceTimeHpGroup> = Vec::with_capacity(enemies_by_deadline.len());
	for (deadline_tick, mut due_enemies) in enemies_by_deadline {
		due_enemies.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
		for enemy in due_enemies {
			let maximum_hp: FixedDamage =
				FixedDamage::from_whole(enemy.max_hp).map_err(|source| {
					FullBattlePlanningError::HpGroupRequiredHpOverflow {
						deadline_tick,
						source,
					}
				})?;
			required_hp = required_hp.checked_add(maximum_hp).map_err(|source| {
				FullBattlePlanningError::HpGroupRequiredHpOverflow {
					deadline_tick,
					source,
				}
			})?;
			members.insert(enemy.instance_id.clone());
		}
		groups.push(SpaceTimeHpGroup {
			id: HpGroupId::new(deadline_tick),
			deadline_tick,
			members: members.clone(),
			required_hp,
		});
	}
	Ok(groups)
}

fn validated_positive_gap_deadline(
	enemy: &FullBattleEnemyDamageGap,
) -> Result<Option<u64>, FullBattlePlanningError> {
	match enemy.resolution {
		FullBattleEnemyResolution::Defeated if enemy.required_damage_micros > 0 => {
			Err(FullBattlePlanningError::PositiveGapWithoutLeak {
				target: enemy.planning_target.clone(),
				resolution: enemy.resolution,
			})
		}
		FullBattleEnemyResolution::Defeated if enemy.failure_deadline_tick.is_some() => {
			Err(FullBattlePlanningError::FailureDeadlineWithoutLeak {
				target: enemy.planning_target.clone(),
				resolution: enemy.resolution,
			})
		}
		FullBattleEnemyResolution::Defeated => Ok(None),
		FullBattleEnemyResolution::Leaked if enemy.required_damage_micros == 0 => {
			Err(FullBattlePlanningError::ZeroGapWithLeak {
				target: enemy.planning_target.clone(),
			})
		}
		FullBattleEnemyResolution::Leaked => {
			enemy.failure_deadline_tick.map(Some).ok_or_else(|| {
				FullBattlePlanningError::PositiveGapWithoutFailureDeadline {
					target: enemy.planning_target.clone(),
				}
			})
		}
		FullBattleEnemyResolution::Unresolved => {
			Err(FullBattlePlanningError::UnresolvedLedgerEnemy {
				target: enemy.planning_target.clone(),
			})
		}
	}
}

fn validate_ledger_enemy<'model, 'ledger>(
	enemy: &'ledger FullBattleEnemyDamageGap,
	model_enemies: &BTreeMap<&'model SpawnInstanceId, &'model EnemyTrajectory>,
	seen_ledger_targets: &mut BTreeSet<&'ledger SpawnInstanceId>,
) -> Result<(), FullBattlePlanningError> {
	if !seen_ledger_targets.insert(&enemy.planning_target) {
		return Err(FullBattlePlanningError::DuplicateLedgerTarget {
			target: enemy.planning_target.clone(),
		});
	}
	let trajectory: &EnemyTrajectory = model_enemies.get(&enemy.planning_target).copied().ok_or(
		FullBattlePlanningError::MissingPlanningTarget {
			target: enemy.planning_target.clone(),
		},
	)?;
	let replay_spawn_tick: u64 =
		enemy
			.spawned_tick
			.ok_or_else(|| FullBattlePlanningError::MissingReplaySpawnTick {
				target: enemy.planning_target.clone(),
			})?;
	if replay_spawn_tick != trajectory.spawn_tick {
		return Err(FullBattlePlanningError::SpawnTickMismatch {
			target: enemy.planning_target.clone(),
			replay: replay_spawn_tick,
			unblocked: trajectory.spawn_tick,
		});
	}
	if enemy.enemy_key != trajectory.enemy_id {
		return Err(FullBattlePlanningError::EnemyIdentityMismatch {
			target: enemy.planning_target.clone(),
			replay: enemy.enemy_key.clone(),
			unblocked: trajectory.enemy_id.clone(),
		});
	}
	let Some(failure_deadline) = validated_positive_gap_deadline(enemy)? else {
		return Ok(());
	};
	if failure_deadline < replay_spawn_tick {
		return Err(FullBattlePlanningError::FailureDeadlineBeforeSpawn {
			target: enemy.planning_target.clone(),
			spawn_tick: replay_spawn_tick,
			failure_deadline,
		});
	}
	Ok(())
}

#[derive(Debug)]
pub enum FullBattlePlanningError {
	StageSpaceTime(StageSpaceTimeError),
	EmptyWaveTimings,
	MissingWaveStart {
		wave_id: u32,
	},
	DuplicateWaveTiming {
		wave_id: u32,
	},
	UnsupportedCausalStageWaveCount {
		found: usize,
	},
	CausalWaveCountMismatch {
		fragments: usize,
		retained: usize,
	},
	CausalWaveIdentifierOverflow,
	MissingCausalWaveTiming {
		replay_wave_id: u32,
	},
	UnexpectedCausalWaveTiming {
		replay_wave_id: u32,
	},
	CausalTickOverflow {
		replay_wave_id: u32,
	},
	AnchorSetMismatch {
		missing: Vec<StageTimeAnchor>,
		unexpected: Vec<StageTimeAnchor>,
	},
	TemplateCohortInstanceCountMismatch {
		cohort_id: String,
		declared: usize,
		actual: usize,
	},
	BoundCohortCountMismatch {
		template: usize,
		bound: usize,
	},
	BoundEnemyCountMismatch {
		template: usize,
		bound: usize,
	},
	EmptyStageModel,
	LedgerEnemyCountMismatch {
		ledger: usize,
		model: usize,
	},
	DuplicateModelTarget {
		target: SpawnInstanceId,
	},
	DuplicateModelCohort {
		cohort_id: String,
	},
	EmptyEnemyOccupancy {
		target: SpawnInstanceId,
	},
	ModelCohortInstanceCountMismatch {
		cohort_id: String,
		declared: usize,
		actual: usize,
	},
	LedgerCohortCountMismatch {
		ledger: usize,
		model: usize,
	},
	MissingPlanningCohort {
		cohort_id: String,
	},
	DuplicateLedgerTarget {
		target: SpawnInstanceId,
	},
	MissingPlanningTarget {
		target: SpawnInstanceId,
	},
	MissingReplaySpawnTick {
		target: SpawnInstanceId,
	},
	SpawnTickMismatch {
		target: SpawnInstanceId,
		replay: u64,
		unblocked: u64,
	},
	EnemyIdentityMismatch {
		target: SpawnInstanceId,
		replay: String,
		unblocked: String,
	},
	PositiveGapWithoutLeak {
		target: SpawnInstanceId,
		resolution: FullBattleEnemyResolution,
	},
	PositiveGapWithoutFailureDeadline {
		target: SpawnInstanceId,
	},
	ZeroGapWithLeak {
		target: SpawnInstanceId,
	},
	FailureDeadlineWithoutLeak {
		target: SpawnInstanceId,
		resolution: FullBattleEnemyResolution,
	},
	UnresolvedLedgerEnemy {
		target: SpawnInstanceId,
	},
	FailureDeadlineBeforeSpawn {
		target: SpawnInstanceId,
		spawn_tick: u64,
		failure_deadline: u64,
	},
	EmptyHpGroups,
	HpGroupRequiredHpOverflow {
		deadline_tick: u64,
		source: FixedDamageError,
	},
	WaveLocalHpGroupRequiredHpOverflow {
		anchor: StageTimeAnchor,
		goal_deadline_offset_ticks: u64,
		source: FixedDamageError,
	},
	EmptyDutyWindow,
}

impl Display for FullBattlePlanningError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::StageSpaceTime(source) => {
				write!(
					formatter,
					"full stage space-time compilation failed: {source}"
				)
			}
			Self::EmptyWaveTimings => {
				formatter.write_str("full-battle replay ledger has no wave timings")
			}
			Self::MissingWaveStart { wave_id } => {
				write!(formatter, "replay wave {wave_id} has no started tick")
			}
			Self::DuplicateWaveTiming { wave_id } => {
				write!(formatter, "replay ledger contains duplicate wave {wave_id}")
			}
			Self::UnsupportedCausalStageWaveCount { found } => write!(
				formatter,
				"causal full-battle planning supports one stage wave, found {found}"
			),
			Self::CausalWaveCountMismatch {
				fragments,
				retained,
			} => write!(
				formatter,
				"stage has {fragments} causal fragments but replay retained {retained} wave timings"
			),
			Self::CausalWaveIdentifierOverflow => {
				formatter.write_str("causal fragment index does not fit a replay wave ID")
			}
			Self::MissingCausalWaveTiming { replay_wave_id } => write!(
				formatter,
				"causal fragment {replay_wave_id} has no retained timing witness"
			),
			Self::UnexpectedCausalWaveTiming { replay_wave_id } => write!(
				formatter,
				"retained timing witness contains unknown causal fragment {replay_wave_id}"
			),
			Self::CausalTickOverflow { replay_wave_id } => write!(
				formatter,
				"causal lower-bound tick overflowed at replay wave {replay_wave_id}"
			),
			Self::AnchorSetMismatch {
				missing,
				unexpected,
			} => write!(
				formatter,
				"full-stage anchor binding mismatch: missing {missing:?}, unexpected {unexpected:?}"
			),
			Self::TemplateCohortInstanceCountMismatch {
				cohort_id,
				declared,
				actual,
			} => write!(
				formatter,
				"template cohort {cohort_id} declares {declared} enemies but contains {actual}"
			),
			Self::BoundCohortCountMismatch { template, bound } => write!(
				formatter,
				"binding retained {bound} of {template} full-stage cohorts"
			),
			Self::BoundEnemyCountMismatch { template, bound } => write!(
				formatter,
				"binding retained {bound} of {template} full-stage enemies"
			),
			Self::EmptyStageModel => formatter.write_str("full-stage model contains no enemies"),
			Self::LedgerEnemyCountMismatch { ledger, model } => write!(
				formatter,
				"replay ledger contains {ledger} enemies but full-stage model contains {model}"
			),
			Self::DuplicateModelTarget { target } => {
				write!(formatter, "full-stage model repeats target {target:?}")
			}
			Self::DuplicateModelCohort { cohort_id } => {
				write!(formatter, "full-stage model repeats cohort {cohort_id}")
			}
			Self::EmptyEnemyOccupancy { target } => {
				write!(
					formatter,
					"full-stage target {target:?} has no targetable occupancy"
				)
			}
			Self::ModelCohortInstanceCountMismatch {
				cohort_id,
				declared,
				actual,
			} => write!(
				formatter,
				"bound cohort {cohort_id} declares {declared} enemies but contains {actual}"
			),
			Self::LedgerCohortCountMismatch { ledger, model } => write!(
				formatter,
				"replay ledger contains {ledger} cohorts but full-stage model contains {model}"
			),
			Self::MissingPlanningCohort { cohort_id } => {
				write!(
					formatter,
					"replay ledger cohort {cohort_id} is absent from the model"
				)
			}
			Self::DuplicateLedgerTarget { target } => {
				write!(
					formatter,
					"replay ledger repeats planning target {target:?}"
				)
			}
			Self::MissingPlanningTarget { target } => {
				write!(
					formatter,
					"replay planning target {target:?} is absent from the model"
				)
			}
			Self::MissingReplaySpawnTick { target } => {
				write!(formatter, "replay target {target:?} has no spawn tick")
			}
			Self::SpawnTickMismatch {
				target,
				replay,
				unblocked,
			} => write!(
				formatter,
				"target {target:?} spawned at replay tick {replay}, not bound-model tick {unblocked}"
			),
			Self::EnemyIdentityMismatch {
				target,
				replay,
				unblocked,
			} => write!(
				formatter,
				"target {target:?} is replay enemy {replay}, not model enemy {unblocked}"
			),
			Self::PositiveGapWithoutLeak { target, resolution } => write!(
				formatter,
				"target {target:?} has a positive gap but resolution is {resolution:?}"
			),
			Self::PositiveGapWithoutFailureDeadline { target } => write!(
				formatter,
				"target {target:?} has a positive gap but no replay failure deadline"
			),
			Self::ZeroGapWithLeak { target } => write!(
				formatter,
				"target {target:?} leaked but has no retained-plan damage gap"
			),
			Self::FailureDeadlineWithoutLeak { target, resolution } => write!(
				formatter,
				"target {target:?} has a replay failure deadline but resolution is {resolution:?}"
			),
			Self::UnresolvedLedgerEnemy { target } => {
				write!(
					formatter,
					"target {target:?} is unresolved in the replay ledger"
				)
			}
			Self::FailureDeadlineBeforeSpawn {
				target,
				spawn_tick,
				failure_deadline,
			} => write!(
				formatter,
				"target {target:?} fails at tick {failure_deadline} before spawning at {spawn_tick}"
			),
			Self::EmptyHpGroups => formatter.write_str(
				"retained-only replay produced no positive damage gap requiring full-HP groups",
			),
			Self::HpGroupRequiredHpOverflow {
				deadline_tick,
				source,
			} => write!(
				formatter,
				"cumulative full HP through unblocked goal deadline {deadline_tick} overflowed: \
				 {source}"
			),
			Self::WaveLocalHpGroupRequiredHpOverflow {
				anchor,
				goal_deadline_offset_ticks,
				source,
			} => write!(
				formatter,
				"cumulative full HP under {anchor:?} through local deadline \
				 {goal_deadline_offset_ticks} overflowed: {source}"
			),
			Self::EmptyDutyWindow => {
				formatter.write_str("full-stage maximum goal deadline is not positive")
			}
		}
	}
}

impl Error for FullBattlePlanningError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::StageSpaceTime(source) => Some(source),
			Self::HpGroupRequiredHpOverflow { source, .. }
			| Self::WaveLocalHpGroupRequiredHpOverflow { source, .. } => Some(source),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use crate::data::GridPosition;

	use super::super::{
		battle_gap::{
			FullBattleDamageGapLedger, FullBattleEnemyDamageGap, FullBattleEnemyResolution,
			FullBattleWaveTiming,
		},
		query::{StageTimeAnchor, TickSpan},
		stage_space_time::{
			CellOccupancy, EnemyTrajectory, RelativeEnemyTrajectory, RelativeSpawnCohort,
			SpawnCohort, StageSpaceTimeModel, StageSpaceTimeTemplate, TrajectoryClass,
		},
	};
	use super::{
		assemble_full_battle_context, build_causal_wave_timings, build_space_time_hp_groups,
		build_wave_local_hp_groups, FullBattlePlanningError, HpGroupId,
	};
	use crate::mechanics::{
		domain::{BattleTimeAnchor, FixedDamage, WaveId},
		stage::{SpawnCohortId, SpawnInstanceId},
	};

	fn target(cohort: &SpawnCohortId) -> SpawnInstanceId {
		target_at(cohort, 0)
	}

	fn target_at(cohort: &SpawnCohortId, ordinal: u32) -> SpawnInstanceId {
		SpawnInstanceId {
			cohort: cohort.clone(),
			ordinal,
		}
	}

	fn model_and_ledger() -> (StageSpaceTimeModel, FullBattleDamageGapLedger) {
		let cohort: SpawnCohortId =
			SpawnCohortId::new("wave:0/fragment:0/action:0".to_owned()).unwrap();
		let instance: SpawnInstanceId = target(&cohort);
		(
			StageSpaceTimeModel {
				stage_id: "stage".to_owned(),
				code: None,
				level_id: "level".to_owned(),
				ticks_per_second: 30,
				cohorts: vec![SpawnCohort {
					id: cohort,
					enemy_id: "enemy".to_owned(),
					route_index: 0,
					first_spawn_tick: 10,
					count: 1,
				}],
				enemies: vec![EnemyTrajectory {
					instance_id: instance.clone(),
					enemy_id: "enemy".to_owned(),
					route_index: 0,
					spawn_tick: 10,
					goal_deadline_tick: 20,
					max_hp: 100,
					defense: 0,
					magic_resistance: 0.0,
					target_class: TrajectoryClass::Ground,
					occupancy: vec![CellOccupancy {
						cell: GridPosition { row: 0, col: 0 },
						enter_tick: 10,
						exit_tick: 20,
						remaining_route_distance_microunits: 1,
					}],
				}],
				bound_anchors: Vec::new(),
				mechanic_requirements: Vec::new(),
			},
			FullBattleDamageGapLedger {
				terminal: "GoalLeak".to_owned(),
				wave_timings: Vec::new(),
				enemies: vec![FullBattleEnemyDamageGap {
					enemy_instance_id: 0,
					planning_target: instance,
					enemy_key: "enemy".to_owned(),
					spawned_tick: Some(10),
					resolution: FullBattleEnemyResolution::Leaked,
					failure_deadline_tick: Some(25),
					required_damage_micros: 50,
				}],
			},
		)
	}

	fn template_from_model(model: &StageSpaceTimeModel) -> StageSpaceTimeTemplate {
		let anchor: StageTimeAnchor = StageTimeAnchor::WaveFragmentStart {
			wave: 0,
			fragment: 0,
		};
		StageSpaceTimeTemplate {
			stage_id: model.stage_id.clone(),
			code: model.code.clone(),
			level_id: model.level_id.clone(),
			ticks_per_second: model.ticks_per_second,
			cohorts: model
				.cohorts
				.iter()
				.map(|cohort| RelativeSpawnCohort {
					id: cohort.id.clone(),
					enemy_id: cohort.enemy_id.clone(),
					route_index: cohort.route_index,
					anchor: anchor.clone(),
					first_spawn_offset_ticks: cohort.first_spawn_tick,
					count: cohort.count,
				})
				.collect(),
			enemies: model
				.enemies
				.iter()
				.map(|enemy| RelativeEnemyTrajectory {
					instance_id: enemy.instance_id.clone(),
					enemy_id: enemy.enemy_id.clone(),
					route_index: enemy.route_index,
					anchor: anchor.clone(),
					spawn_offset_ticks: enemy.spawn_tick,
					goal_deadline_offset_ticks: enemy.goal_deadline_tick,
					max_hp: enemy.max_hp,
					defense: enemy.defense,
					magic_resistance: enemy.magic_resistance,
					target_class: enemy.target_class,
					occupancy: enemy.occupancy.clone(),
				})
				.collect(),
			mechanic_requirements: Vec::new(),
		}
	}

	fn three_fragment_template() -> StageSpaceTimeTemplate {
		let enemies: Vec<RelativeEnemyTrajectory> = [(0, 30, 100), (1, 50, 120), (2, 20, 90)]
			.into_iter()
			.map(
				|(fragment, spawn_offset_ticks, goal_deadline_offset_ticks)| {
					let cohort: SpawnCohortId =
						SpawnCohortId::new(format!("wave:0/fragment:{fragment}/action:0"))
							.expect("test cohort is valid");
					RelativeEnemyTrajectory {
						instance_id: target(&cohort),
						enemy_id: format!("enemy-{fragment}"),
						route_index: 0,
						anchor: StageTimeAnchor::WaveFragmentStart { wave: 0, fragment },
						spawn_offset_ticks,
						goal_deadline_offset_ticks,
						max_hp: 100,
						defense: 0,
						magic_resistance: 0.0,
						target_class: TrajectoryClass::Ground,
						occupancy: vec![CellOccupancy {
							cell: GridPosition { row: 0, col: 0 },
							enter_tick: spawn_offset_ticks,
							exit_tick: goal_deadline_offset_ticks,
							remaining_route_distance_microunits: 1,
						}],
					}
				},
			)
			.collect();
		StageSpaceTimeTemplate {
			stage_id: "causal-stage".to_owned(),
			code: None,
			level_id: "causal-level".to_owned(),
			ticks_per_second: 30,
			cohorts: Vec::new(),
			enemies,
			mechanic_requirements: Vec::new(),
		}
	}

	fn retained_wave_timings(shift: u64) -> Vec<FullBattleWaveTiming> {
		[(0, 60, 200), (1, 560, 1_000), (2, 2_200, 3_000)]
			.into_iter()
			.map(
				|(wave_id, started_tick, cleared_tick)| FullBattleWaveTiming {
					wave_id,
					started_tick: Some(started_tick + shift),
					cleared_tick: Some(cleared_tick + shift),
				},
			)
			.collect()
	}

	#[test]
	fn retained_witness_shift_does_not_bind_causal_wave_starts() {
		let template: StageSpaceTimeTemplate = three_fragment_template();
		let baseline =
			build_causal_wave_timings(60, &[0, 360, 1_200], &template, &retained_wave_timings(0))
				.expect("baseline causal timing is valid");
		let shifted = build_causal_wave_timings(
			60,
			&[0, 360, 1_200],
			&template,
			&retained_wave_timings(10_000),
		)
		.expect("shifted witness remains valid");

		assert_eq!(baseline.len(), 3);
		for (left, right) in baseline.iter().zip(&shifted) {
			assert_eq!(left.stage_anchor, right.stage_anchor);
			assert_eq!(left.start, right.start);
			assert_eq!(left.start_bounds, right.start_bounds);
			assert_ne!(left.retained_witness, right.retained_witness);
		}
		assert_eq!(baseline[0].start_bounds.latest, Some(60));
		assert_eq!(baseline[1].start_bounds.earliest, 450);
		assert_eq!(baseline[1].start_bounds.latest, None);
		assert_eq!(baseline[2].start_bounds.earliest, 1_700);
		assert_eq!(baseline[2].start_bounds.latest, None);
		assert_eq!(
			baseline[1].start.earliest.anchor,
			BattleTimeAnchor::WaveCleared(WaveId::new(0))
		);
		assert_eq!(baseline[1].start.earliest.offset_ticks, 360);
		assert_eq!(
			baseline[2].start.earliest.anchor,
			BattleTimeAnchor::WaveCleared(WaveId::new(1))
		);
		assert_eq!(baseline[2].start.earliest.offset_ticks, 1_200);
	}

	#[test]
	fn wave_local_hp_groups_never_accumulate_across_causal_anchors() {
		let template: StageSpaceTimeTemplate = three_fragment_template();
		let first_target: SpawnInstanceId = template.enemies[0].instance_id.clone();
		let ledger = FullBattleDamageGapLedger {
			terminal: "GoalLeak".to_owned(),
			wave_timings: retained_wave_timings(0),
			enemies: vec![FullBattleEnemyDamageGap {
				enemy_instance_id: 0,
				planning_target: first_target,
				enemy_key: "enemy-0".to_owned(),
				spawned_tick: Some(90),
				resolution: FullBattleEnemyResolution::Leaked,
				failure_deadline_tick: Some(160),
				required_damage_micros: 1,
			}],
		};

		let groups = build_wave_local_hp_groups(&template, &ledger)
			.expect("wave-local groups should compile");

		assert_eq!(groups.len(), 3);
		assert!(groups.iter().all(|group| group.members.len() == 1));
		assert!(groups.iter().all(|group| {
			group.members.iter().all(|member| {
				template
					.enemy(member)
					.is_some_and(|enemy| enemy.anchor == group.id.anchor)
			})
		}));
	}

	#[test]
	fn full_enemy_hp_defines_group_requirement_at_unblocked_goal_deadline() {
		let (model, ledger) = model_and_ledger();
		let target: SpawnInstanceId = ledger.enemies[0].planning_target.clone();
		let template: StageSpaceTimeTemplate = template_from_model(&model);
		let context = assemble_full_battle_context(model, template, Vec::new(), &ledger).unwrap();
		let group = context.hp_groups.first().unwrap();

		assert_eq!(context.duty_window, TickSpan { start: 0, end: 25 });
		assert_eq!(ledger.enemies[0].failure_deadline_tick, Some(25));
		assert_eq!(ledger.enemies[0].required_damage_micros, 50);
		assert_eq!(context.model.enemies[0].goal_deadline_tick, 20);
		assert_eq!(group.id, HpGroupId::new(20));
		assert_eq!(group.id.to_string(), "goal_deadline_prefix:20");
		assert_eq!(group.deadline_tick, 20);
		assert_eq!(group.members, BTreeSet::from([target]));
		assert_eq!(
			group.required_hp,
			FixedDamage::from_whole(100).expect("test HP fits")
		);
	}

	#[test]
	fn hp_groups_are_sorted_cumulative_full_hp_prefixes() {
		let (mut model, mut ledger) = model_and_ledger();
		let cohort: SpawnCohortId = ledger.enemies[0].planning_target.cohort.clone();
		let mut same_deadline: FullBattleEnemyDamageGap = ledger.enemies[0].clone();
		same_deadline.enemy_instance_id = 1;
		same_deadline.planning_target = target_at(&cohort, 1);
		same_deadline.required_damage_micros = 25;
		let mut later_deadline: FullBattleEnemyDamageGap = ledger.enemies[0].clone();
		later_deadline.enemy_instance_id = 2;
		later_deadline.planning_target = target_at(&cohort, 2);
		later_deadline.failure_deadline_tick = Some(30);
		later_deadline.required_damage_micros = 10;
		let mut defeated: FullBattleEnemyDamageGap = ledger.enemies[0].clone();
		defeated.enemy_instance_id = 3;
		defeated.planning_target = target_at(&cohort, 3);
		defeated.resolution = FullBattleEnemyResolution::Defeated;
		defeated.failure_deadline_tick = None;
		defeated.required_damage_micros = 0;
		let defeated_target: SpawnInstanceId = defeated.planning_target.clone();
		ledger
			.enemies
			.extend([same_deadline, later_deadline, defeated]);

		let mut same_goal: EnemyTrajectory = model.enemies[0].clone();
		same_goal.instance_id = target_at(&cohort, 1);
		same_goal.max_hp = 25;
		let mut later_goal: EnemyTrajectory = model.enemies[0].clone();
		later_goal.instance_id = target_at(&cohort, 2);
		later_goal.goal_deadline_tick = 30;
		later_goal.max_hp = 10;
		let mut defeated_before_later_goal: EnemyTrajectory = model.enemies[0].clone();
		defeated_before_later_goal.instance_id = defeated_target.clone();
		defeated_before_later_goal.goal_deadline_tick = 25;
		defeated_before_later_goal.max_hp = 40;
		model
			.enemies
			.extend([same_goal, later_goal, defeated_before_later_goal]);

		let groups = build_space_time_hp_groups(&model, &ledger).unwrap();

		assert_eq!(groups.len(), 3);
		assert_eq!(groups[0].deadline_tick, 20);
		assert_eq!(groups[0].members.len(), 2);
		assert_eq!(
			groups[0].required_hp,
			FixedDamage::from_whole(125).expect("test HP fits")
		);
		assert_eq!(groups[1].deadline_tick, 25);
		assert_eq!(groups[1].members.len(), 3);
		assert!(groups[0].members.is_subset(&groups[1].members));
		assert_eq!(
			groups[1].required_hp,
			FixedDamage::from_whole(165).expect("test HP fits")
		);
		assert!(groups[1].members.contains(&defeated_target));
		assert_eq!(groups[2].deadline_tick, 30);
		assert_eq!(groups[2].members.len(), 4);
		assert!(groups[1].members.is_subset(&groups[2].members));
		assert_eq!(
			groups[2].required_hp,
			FixedDamage::from_whole(175).expect("test HP fits")
		);
	}

	#[test]
	fn empty_positive_gap_set_fails_closed() {
		let (model, mut ledger) = model_and_ledger();
		ledger.enemies[0].resolution = FullBattleEnemyResolution::Defeated;
		ledger.enemies[0].failure_deadline_tick = None;
		ledger.enemies[0].required_damage_micros = 0;

		assert!(matches!(
			build_space_time_hp_groups(&model, &ledger),
			Err(FullBattlePlanningError::EmptyHpGroups)
		));
	}

	#[test]
	fn zero_gap_leak_fails_closed() {
		let (model, mut ledger) = model_and_ledger();
		ledger.enemies[0].required_damage_micros = 0;

		assert!(matches!(
			build_space_time_hp_groups(&model, &ledger),
			Err(FullBattlePlanningError::ZeroGapWithLeak { .. })
		));
	}

	#[test]
	fn cumulative_full_hp_overflow_fails_closed() {
		let (mut model, ledger) = model_and_ledger();
		model.enemies[0].max_hp = u64::MAX;

		assert!(matches!(
			build_space_time_hp_groups(&model, &ledger),
			Err(FullBattlePlanningError::HpGroupRequiredHpOverflow {
				deadline_tick: 20,
				..
			})
		));
	}

	#[test]
	fn missing_planning_target_fails_closed() {
		let (model, mut ledger) = model_and_ledger();
		ledger.enemies[0].planning_target.ordinal = 1;
		let template: StageSpaceTimeTemplate = template_from_model(&model);
		let error = assemble_full_battle_context(model, template, Vec::new(), &ledger).unwrap_err();

		assert!(matches!(
			error,
			FullBattlePlanningError::MissingPlanningTarget { .. }
		));
	}
}
