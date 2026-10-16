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

/// Leaks, life points lost, kills and skill activity from a run's observer log. An enemy that
/// leaves the field with hit points left reached an exit; one killed leaves at zero.
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
				leaks.push(json!({"tick": tick, "hp": points(&event["value"]), "max_hp": maximum}));
			}
			"life" => life.push(json!({"tick": tick, "life_point": event["value"]})),
			"skill_start" => skills += 1,
			_ => {}
		}
	}
	let enemies: usize = entered.len();
	let kills: Value = events
		.iter()
		.rev()
		.find(|event| event["kind"] == "kills")
		.map_or(json!(0), |event| event["value"].clone());
	Ok(json!({
		"leaks": leaks,
		"leak_count": leaks.len(),
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
}
