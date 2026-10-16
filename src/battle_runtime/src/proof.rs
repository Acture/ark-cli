//! Re-read original unit observations without rounding serialized HP values.
use anyhow::{ensure, Context, Result};
use ark_unity::io;
use serde_json::{json, Value};
use std::{
	cmp::Ordering,
	collections::{BTreeMap, BTreeSet},
	path::Path,
};

#[derive(Eq, PartialEq)]
struct Decimal {
	negative: bool,
	whole: String,
	fraction: String,
}
impl Decimal {
	fn parse(value: &str) -> Result<Self> {
		ensure!(value.len() <= 128, "HP decimal exceeds bound");
		let negative = value.starts_with('-');
		let digits = value.strip_prefix('-').unwrap_or(value);
		let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
		ensure!(
			!whole.is_empty()
				&& whole
					.bytes()
					.chain(fraction.bytes())
					.all(|b| b.is_ascii_digit()),
			"Invalid original HP decimal"
		);
		let whole = whole.trim_start_matches('0');
		let fraction = fraction.trim_end_matches('0');
		Ok(Self {
			negative: negative && !(whole.is_empty() && fraction.is_empty()),
			whole: whole.to_owned(),
			fraction: fraction.to_owned(),
		})
	}
}
impl Ord for Decimal {
	fn cmp(&self, other: &Self) -> Ordering {
		if self.negative != other.negative {
			return other.negative.cmp(&self.negative);
		}
		let length = self.fraction.len().max(other.fraction.len());
		let order = self
			.whole
			.len()
			.cmp(&other.whole.len())
			.then(self.whole.cmp(&other.whole))
			.then_with(|| {
				self.fraction
					.bytes()
					.chain(std::iter::repeat(b'0'))
					.take(length)
					.cmp(
						other
							.fraction
							.bytes()
							.chain(std::iter::repeat(b'0'))
							.take(length),
					)
			});
		if self.negative {
			order.reverse()
		} else {
			order
		}
	}
}
impl PartialOrd for Decimal {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

pub(crate) fn summarize(path: &Path) -> Result<Value> {
	let result: Value = io::read_json(path)?;
	let outcome = crate::runtime::outcome(&result)?;
	ensure!(
		outcome["terminal_verified"] == true,
		"Original battle has no verified natural terminal state"
	);
	let events = result["events"].as_array().context("Missing events")?;
	let mut units: BTreeMap<(String, i64), Vec<Value>> = BTreeMap::new();
	let mut frame: i64 = 0;
	let mut kind: Option<&str> = None;
	let mut instance: Option<i64> = None;
	let mut grid: Option<&str> = None;
	for event in events {
		let stage = event["stage"].as_str().context("Event stage missing")?;
		match stage {
			"original_battle_fixed_frames" => {
				frame = event["value"].as_i64().context("Frame missing")?
			}
			"original_enemy_snapshot_index" | "original_character_snapshot_index" => {
				kind = Some(if stage.starts_with("original_enemy") {
					"enemy"
				} else {
					"character"
				});
				instance = None;
				grid = None;
			}
			_ => {
				if let Some(k) = kind {
					if stage == format!("original_{k}_instance_id") {
						let id = event["value"].as_i64().context("Instance missing")?;
						instance = Some(if id >= 1 << 31 { id - (1 << 32) } else { id });
					} else if stage == format!("original_{k}_grid") {
						grid = event["path"].as_str();
					} else if stage == format!("original_{k}_hp") {
						let hp = event["path"].as_str().context("HP missing")?;
						Decimal::parse(hp)?;
						units
							.entry((
								k.to_owned(),
								instance.context("HP has no stable unit identity")?,
							))
							.or_default()
							.push(
								json!({"fixed_frame":frame,"grid":grid.context("HP has no grid")?,"hp":hp}),
							);
						kind = None;
					}
				}
			}
		}
	}
	let mut tracks: Vec<Value> = Vec::new();
	for ((kind, id), samples) in units {
		let parsed: Vec<Decimal> = samples
			.iter()
			.map(|s| Decimal::parse(s["hp"].as_str().unwrap()))
			.collect::<Result<_>>()?;
		let minimum = parsed
			.iter()
			.enumerate()
			.min_by(|(_, a), (_, b)| a.cmp(b))
			.unwrap()
			.0;
		let grids: BTreeSet<&str> = samples.iter().filter_map(|s| s["grid"].as_str()).collect();
		tracks.push(json!({"kind":kind,"instance_id":id,"first_hp":samples[0]["hp"],"minimum_hp":samples[minimum]["hp"],"last_hp":samples.last().unwrap()["hp"],"position_changed":grids.len()>1,"hp_decreased":parsed[minimum]<parsed[0],"samples":samples}));
	}
	let max = |stage: &str| {
		events
			.iter()
			.filter(|e| e["stage"] == stage)
			.filter_map(|e| e["value"].as_u64())
			.max()
			.unwrap_or(0)
	};
	let observed =
		|kind: &str, key: &str| tracks.iter().any(|t| t["kind"] == kind && t[key] == true);
	Ok(
		json!({"schema":"ark-original-battle-proof-v3","run_result":path,"run_result_sha256":io::sha256_file(path)?,"battle_outcome":outcome,"fixed_frames":max("original_battle_fixed_frames"),"maximum_deployed":max("original_battle_character_count"),"spawned_enemies":max("original_battle_spawned_enemies"),"killed_enemies":max("original_battle_killed_enemies"),"enemy_movement_observed":observed("enemy","position_changed"),"enemy_hp_loss_observed":observed("enemy","hp_decreased"),"character_hp_loss_observed":observed("character","hp_decreased"),"verified_action_count":result["verified_action_count"],"completed_action_count":result["completed_action_count"],"remaining_action_count":result["remaining_action_count"],"runtime_diagnostics":result["runtime_diagnostics"],"verification_scope":"original engine natural terminal state and observed actions; not an Ark Solver verdict","units":tracks}),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn decimal_order_keeps_all_original_digits() {
		assert!(
			Decimal::parse("100000000000000.0000000000000001").unwrap()
				> Decimal::parse("100000000000000.0000000000000000").unwrap()
		);
		assert!(Decimal::parse("-0.5").unwrap() < Decimal::parse("0").unwrap());
		assert!(Decimal::parse("001.500").unwrap() == Decimal::parse("1.5").unwrap());
	}
}
