//! Per-tick battle events the probe observed on the original state, summarized for a run.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Original hit points are fixed point; the probe records one raw value with its text.
fn fixed_scale(events: &[Value], texts: &[&str]) -> Option<f64> {
	let raw: f64 = events
		.iter()
		.find(|event| event["kind"] == "fixed_sample")?["value"]
		.as_f64()?;
	let text: f64 = texts.first()?.parse().ok()?;
	(text > 0.0).then_some(raw / text)
}

/// Leaks, life points lost, kills and skill activity from a run's observer log. A leak is an
/// enemy reaching an exit at the cost of life points. The original takes the life point when the
/// enemy arrives and removes it some ticks later with its hit points left, so each life point lost
/// is answered by the next enemy leaving with hit points; one unanswered is a leak the battle
/// ended before removing. An enemy leaving with hit points that answers no life point lost, such
/// as a unit that passes through, is an exit without life loss, not a leak.
pub(crate) fn summarize(observer: &Path) -> Result<Value> {
	let text: String =
		fs::read_to_string(observer).with_context(|| format!("Read {}", observer.display()))?;
	let mut events: Vec<Value> = Vec::new();
	let mut texts: Vec<String> = Vec::new();
	for line in text.lines() {
		let Ok(event) = serde_json::from_str::<Value>(line) else {
			continue;
		};
		match event["stage"].as_str() {
			Some("original_event") => events.push(event),
			Some("original_event_fixed_text") => {
				texts.push(event["path"].as_str().unwrap_or_default().to_owned());
			}
			_ => {}
		}
	}
	let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
	let scale: Option<f64> = fixed_scale(&events, &texts);
	let points = |raw: &Value| -> Value {
		match (raw.as_f64(), scale) {
			(Some(raw), Some(scale)) => json!((raw / scale * 100.0).round() / 100.0),
			_ => Value::Null,
		}
	};
	let mut leaks: Vec<Value> = Vec::new();
	let mut life: Vec<Value> = Vec::new();
	let mut skills: usize = 0;
	let mut entered: Vec<(i64, Value)> = Vec::new();
	// Ticks of life points lost, and enemies leaving with hit points.
	let mut lost: Vec<Value> = Vec::new();
	let mut left: Vec<Value> = Vec::new();
	let mut previous_life: Option<i64> = None;
	for event in &events {
		let tick: &Value = &event["tick"];
		match event["kind"].as_str().unwrap_or_default() {
			"enemy_enter" => {
				entered.push((event["unit"].as_i64().unwrap_or(0), event["value"].clone()))
			}
			"enemy_leave" if event["value"].as_i64().unwrap_or(0) > 0 => {
				let unit: i64 = event["unit"].as_i64().unwrap_or(0);
				let maximum: Value = entered
					.iter()
					.find(|(id, _)| *id == unit)
					.map_or(Value::Null, |(_, raw)| points(raw));
				left.push(json!({"tick": tick, "hp": points(&event["value"]), "max_hp": maximum}));
			}
			"life" => {
				let value: i64 = event["value"].as_i64().unwrap_or(0);
				if previous_life.is_none_or(|previous| value < previous) {
					lost.push(tick.clone());
				}
				previous_life = Some(value);
				life.push(json!({"tick": tick, "life_point": event["value"]}));
			}
			"skill_start" => skills += 1,
			_ => {}
		}
	}
	// Each life point lost is answered by the first enemy leaving with hit points at or after it.
	let mut answered: Vec<bool> = vec![false; left.len()];
	for tick in lost {
		let at: i64 = tick.as_i64().unwrap_or(0);
		match (0..left.len())
			.find(|&index| !answered[index] && left[index]["tick"].as_i64().unwrap_or(0) >= at)
		{
			Some(index) => {
				answered[index] = true;
				leaks.push(json!({"tick": tick, "hp": left[index]["hp"],
					"max_hp": left[index]["max_hp"]}));
			}
			None => leaks.push(
				json!({"tick": tick, "hp": Value::Null, "max_hp": Value::Null,
				"ended_before_leaving": true}),
			),
		}
	}
	let exits: Vec<Value> = left
		.into_iter()
		.zip(answered)
		.filter_map(|(exit, answered)| (!answered).then_some(exit))
		.collect();
	let enemies: usize = entered.len();
	let kills: Value = events
		.iter()
		.rev()
		.find(|event| event["kind"] == "kills")
		.map_or(json!(0), |event| event["value"].clone());
	Ok(json!({
		"leaks": leaks,
		"leak_count": leaks.len(),
		"exits_without_life_loss": exits,
		"perfect": leaks.is_empty() && life.is_empty(),
		"life_changes": life,
		"enemies": enemies,
		"kills": kills,
		"skill_starts": skills,
		"provenance": "per-tick original state observed by the probe; hit points from the original fixed point",
	}))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn leaks_carry_scaled_hit_points_and_kills_are_not_leaks() {
		let directory = std::env::temp_dir().join(format!("ark-events-{}", std::process::id()));
		fs::create_dir_all(&directory).unwrap();
		let path = directory.join("observer.jsonl");
		let scale: i64 = 1 << 32;
		let lines: Vec<String> = vec![
			json!({"stage":"original_event","tick":10,"kind":"enemy_enter","unit":1,"value":7000 * scale}).to_string(),
			json!({"stage":"original_event","tick":10,"kind":"fixed_sample","unit":1,"value":7000 * scale}).to_string(),
			json!({"stage":"original_event_fixed_text","path":"7000"}).to_string(),
			json!({"stage":"original_event","tick":11,"kind":"enemy_enter","unit":2,"value":100 * scale}).to_string(),
			json!({"stage":"original_event","tick":50,"kind":"enemy_leave","unit":2,"value":0}).to_string(),
			json!({"stage":"original_event","tick":50,"kind":"kills","unit":0,"value":1}).to_string(),
			json!({"stage":"original_event","tick":90,"kind":"enemy_leave","unit":1,"value":1750 * scale}).to_string(),
			json!({"stage":"original_event","tick":90,"kind":"life","unit":0,"value":2}).to_string(),
		];
		fs::write(&path, lines.join("\n")).unwrap();
		let summary: Value = summarize(&path).unwrap();
		fs::remove_dir_all(&directory).unwrap();
		assert_eq!(summary["leak_count"], 1);
		assert_eq!(summary["leaks"][0]["hp"], 1750.0);
		assert_eq!(summary["leaks"][0]["max_hp"], 7000.0);
		assert_eq!(summary["perfect"], false);
		assert_eq!(summary["kills"], 1);
	}

	#[test]
	fn a_life_point_lost_as_the_battle_ends_is_a_leak() {
		let directory =
			std::env::temp_dir().join(format!("ark-events-final-{}", std::process::id()));
		fs::create_dir_all(&directory).unwrap();
		let path = directory.join("observer.jsonl");
		let event = |tick: i64, kind: &str, unit: i64, value: i64| -> String {
			json!({"stage":"original_event","tick":tick,"kind":kind,"unit":unit,"value":value})
				.to_string()
		};
		// An exit that costs no life is no leak; the first leak's enemy leaves 29 ticks after its
		// life point; the second ends the battle before its enemy leaves.
		let lines: Vec<String> = vec![
			event(10, "enemy_enter", 1, 100),
			event(10, "enemy_enter", 2, 100),
			event(10, "enemy_enter", 3, 100),
			event(40, "enemy_leave", 1, 100),
			event(100, "life", 0, 1),
			event(129, "enemy_leave", 2, 50),
			event(300, "life", 0, 0),
		];
		fs::write(&path, lines.join("\n")).unwrap();
		let summary: Value = summarize(&path).unwrap();
		fs::remove_dir_all(&directory).unwrap();
		assert_eq!(summary["leak_count"], 2);
		assert_eq!(
			summary["leaks"][0],
			json!({"tick":100,"hp":Value::Null,"max_hp":Value::Null})
		);
		assert_eq!(summary["leaks"][1]["tick"], 300);
		assert_eq!(summary["leaks"][1]["ended_before_leaving"], true);
		assert_eq!(summary["exits_without_life_loss"][0]["tick"], 40);
		assert_eq!(summary["perfect"], false);
	}
}
