//! Retained-only replay projected into the planner's full-battle damage-gap ledger.

use std::{
	error::Error,
	fmt::{Display, Formatter},
};

use crate::mechanics::domain::{BattleTimeAnchor, EnemyInstanceId, MechanicSupportReport};
use crate::mechanics::stage::SpawnInstanceId;
use crate::planning::{
	FullBattleDamageGapLedger, FullBattleEnemyDamageGap, FullBattleEnemyResolution,
	FullBattleWaveTiming,
};
use crate::replay::{
	compile_battle, replay_expected_to_resolution, ExpectedReplayInput, ExpectedTerminalStatus,
};

/// Runs a retained-only input until every enemy is defeated or leaked.
///
/// The input is expected to have already removed the source slot. Blocking, deaths, retargeting,
/// and causally started later waves therefore come from the same authoritative event model used by
/// final validation.
pub fn derive_full_battle_damage_gap_ledger(
	input: &ExpectedReplayInput,
) -> Result<FullBattleDamageGapLedger, FullBattleGapError> {
	let battle =
		compile_battle(&input.facts, &input.guide).map_err(FullBattleGapError::Unsupported)?;
	let assessment = replay_expected_to_resolution(&battle, input.policy.clone());
	let enemies: Vec<FullBattleEnemyDamageGap> = assessment
		.enemies()
		.values()
		.map(
			|enemy| -> Result<FullBattleEnemyDamageGap, FullBattleGapError> {
				let planning_target: SpawnInstanceId = input
					.enemy_lineage
					.get(&enemy.id)
					.cloned()
					.ok_or(FullBattleGapError::MissingEnemyLineage { enemy: enemy.id })?;
				let (resolution, failure_deadline_tick, required_damage_micros) =
					if enemy.death_at.is_some() {
						(FullBattleEnemyResolution::Defeated, None, 0)
					} else if let Some(tick) = enemy.leaked_at {
						(
							FullBattleEnemyResolution::Leaked,
							Some(tick.get()),
							enemy.remaining_hp.scaled(),
						)
					} else {
						(
							FullBattleEnemyResolution::Unresolved,
							None,
							enemy.remaining_hp.scaled(),
						)
					};
				Ok(FullBattleEnemyDamageGap {
					enemy_instance_id: enemy.id.get(),
					planning_target,
					enemy_key: enemy.enemy_key.clone(),
					spawned_tick: enemy.spawned_at.map(|tick| tick.get()),
					resolution,
					failure_deadline_tick,
					required_damage_micros,
				})
			},
		)
		.collect::<Result<_, _>>()?;
	if enemies
		.iter()
		.any(|enemy| enemy.resolution == FullBattleEnemyResolution::Unresolved)
	{
		return Err(FullBattleGapError::Incomplete {
			terminal: format!("{:?}", assessment.status()),
		});
	}
	let wave_timings: Vec<FullBattleWaveTiming> = input
		.facts
		.waves
		.iter()
		.map(|wave| FullBattleWaveTiming {
			wave_id: wave.id.get(),
			started_tick: assessment
				.anchors()
				.get(&BattleTimeAnchor::WaveStarted(wave.id))
				.map(|tick| tick.get()),
			cleared_tick: assessment
				.anchors()
				.get(&BattleTimeAnchor::WaveCleared(wave.id))
				.map(|tick| tick.get()),
		})
		.collect();
	match assessment.status() {
		ExpectedTerminalStatus::Victory | ExpectedTerminalStatus::GoalLeak { .. } => {
			Ok(FullBattleDamageGapLedger {
				terminal: format!("{:?}", assessment.status()),
				wave_timings,
				enemies,
			})
		}
		ExpectedTerminalStatus::Deadline { .. } | ExpectedTerminalStatus::SimulationIncomplete => {
			Err(FullBattleGapError::Incomplete {
				terminal: format!("{:?}", assessment.status()),
			})
		}
	}
}

#[derive(Clone, Debug)]
pub enum FullBattleGapError {
	Unsupported(MechanicSupportReport),
	MissingEnemyLineage { enemy: EnemyInstanceId },
	Incomplete { terminal: String },
}

impl Display for FullBattleGapError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Unsupported(report) => {
				write!(formatter, "retained-only replay is unsupported: {report:?}")
			}
			Self::MissingEnemyLineage { enemy } => {
				write!(
					formatter,
					"retained-only replay enemy {} has no planner lineage",
					enemy.get()
				)
			}
			Self::Incomplete { terminal } => {
				write!(
					formatter,
					"retained-only replay did not resolve every enemy: {terminal}"
				)
			}
		}
	}
}

impl Error for FullBattleGapError {}
