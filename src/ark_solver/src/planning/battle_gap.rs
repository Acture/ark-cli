//! Pure planning DTOs for retained-plan full-battle damage gaps.

use serde::Serialize;

use crate::mechanics::stage::SpawnInstanceId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FullBattleEnemyResolution {
	Defeated,
	Leaked,
	Unresolved,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FullBattleEnemyDamageGap {
	pub enemy_instance_id: u32,
	/// Stable stage/planner identity for this replay enemy.
	pub planning_target: SpawnInstanceId,
	pub enemy_key: String,
	pub spawned_tick: Option<u64>,
	pub resolution: FullBattleEnemyResolution,
	/// Failure boundary at which the retained-only enemy reached its goal; absent when defeated.
	pub failure_deadline_tick: Option<u64>,
	/// HP that the retained-only plan still lacked when this enemy reached the goal.
	pub required_damage_micros: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FullBattleWaveTiming {
	pub wave_id: u32,
	pub started_tick: Option<u64>,
	pub cleared_tick: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FullBattleDamageGapLedger {
	pub terminal: String,
	pub wave_timings: Vec<FullBattleWaveTiming>,
	pub enemies: Vec<FullBattleEnemyDamageGap>,
}

impl FullBattleDamageGapLedger {
	#[must_use]
	pub fn unresolved_damage_micros(&self) -> u128 {
		self.enemies
			.iter()
			.map(|enemy| u128::from(enemy.required_damage_micros))
			.sum()
	}

	#[must_use]
	pub fn leaked_enemy_count(&self) -> usize {
		self.enemies
			.iter()
			.filter(|enemy| enemy.resolution == FullBattleEnemyResolution::Leaked)
			.count()
	}
}
