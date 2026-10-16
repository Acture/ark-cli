//! Repeatability of original-engine observations, without certifying solver replay outcomes.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::environment::{read_json, sha256_file, write_json};

const STATE_STAGES: &[&str] = &[
	"original_battle_seed",
	"original_battle_deterministic",
	"original_battle_physics_auto",
	"original_battle_contiguous_manual_ticks",
	"original_battle_fixed_frames",
	"original_battle_requested_ticks",
	"original_battle_enemy_count",
	"original_battle_character_count",
	"original_battle_spawned_enemies",
	"original_battle_killed_enemies",
	"original_battle_dp",
	"original_battle_is_finished",
	"original_battle_is_playing",
	"original_battle_game_result",
	"original_battle_life_point",
	"original_enemy_snapshot_index",
	"original_character_snapshot_index",
	"original_enemy_grid",
	"original_enemy_hp",
	"original_character_grid",
	"original_character_hp",
];

pub fn normalize(events: &[Value]) -> Result<Vec<Value>> {
	let mut identities: BTreeMap<&str, BTreeMap<i64, usize>> = BTreeMap::new();
	let mut trace: Vec<Value> = Vec::new();
	for event in events {
		let stage: &str = event["stage"]
			.as_str()
			.context("Observation has no stage")?;
		if matches!(
			stage,
			"original_enemy_instance_id" | "original_character_instance_id"
		) {
			let mapping: &mut BTreeMap<i64, usize> = identities.entry(stage).or_default();
			let next: usize = mapping.len() + 1;
			let id: i64 = event["value"]
				.as_i64()
				.context("Observation has no numeric unit identity")?;
			let normalized: usize = *mapping.entry(id).or_insert(next);
			trace.push(json!({"stage": stage, "value": normalized}));
		} else if stage == "original_event" {
			let mapping: &mut BTreeMap<i64, usize> = identities.entry(stage).or_default();
			let next: usize = mapping.len() + 1;
			let unit: usize = *mapping
				.entry(event["unit"].as_i64().unwrap_or(0))
				.or_insert(next);
			trace.push(
				json!({"stage": stage, "tick": event["tick"], "kind": event["kind"],
				"unit": unit, "value": event["value"]}),
			);
		} else if STATE_STAGES.contains(&stage)
			|| stage.starts_with("original_probe_")
			|| stage.starts_with("original_maa_")
		{
			let mut sample = serde_json::Map::new();
			for key in ["stage", "value", "path"] {
				if let Some(value) = event.get(key) {
					sample.insert(key.to_owned(), value.clone());
				}
			}
			trace.push(Value::Object(sample));
		}
	}
	Ok(trace)
}

pub fn compare_values(first: &Value, second: &Value) -> Result<Value> {
	ensure!(
		[first, second]
			.iter()
			.all(|run| run["battle_outcome"]["terminal_verified"] == true),
		"Both runs must have verified original terminal states"
	);
	let traces: Vec<Vec<Value>> = [first, second]
		.iter()
		.map(|run| {
			normalize(
				run["events"]
					.as_array()
					.context("Run has no observation events")?,
			)
		})
		.collect::<Result<_>>()?;
	for run in [first, second] {
		ensure!(
			run["battle_inputs"].is_object()
				&& run["effective_inputs"].is_array()
				&& run["execution_inputs"].is_array(),
			"Run is missing pinned inputs"
		);
		for key in ["actions_sha256", "squad_sha256", "seed", "max_ticks"] {
			ensure!(
				run["battle_inputs"].get(key).is_some(),
				"Run is missing pinned {key}"
			);
		}
	}
	let inputs_equal: bool = ["actions_sha256", "squad_sha256", "seed", "max_ticks"]
		.iter()
		.all(|key| first["battle_inputs"][key] == second["battle_inputs"][key]);
	let players_equal: bool = first["effective_inputs"] == second["effective_inputs"];
	let execution_equal: bool = first["execution_inputs"] == second["execution_inputs"];
	let states_equal: bool = traces[0] == traces[1];
	let outcomes_equal: bool = first["battle_outcome"] == second["battle_outcome"];
	let jobs_complete: bool = [first, second].iter().all(|run| {
		run["passed"] == true
			&& run
				.get("maa_job_complete")
				.is_none_or(|value| value == true)
	});
	let difference: Option<Value> = traces[0]
		.iter()
		.zip(&traces[1])
		.enumerate()
		.find(|(_, (left, right))| left != right)
		.map(|(index, (left, right))| json!({"index": index, "first": left, "second": right}));
	let digests: Vec<String> = traces
		.iter()
		.map(|trace| serde_json::to_vec(trace).map(|bytes| format!("{:x}", Sha256::digest(bytes))))
		.collect::<Result<_, _>>()?;
	Ok(
		json!({"schema": "ark-original-battle-repeatability-v3", "passed": jobs_complete && inputs_equal
		&& players_equal && execution_equal && states_equal && outcomes_equal, "jobs_complete": jobs_complete,
		"battle_inputs_equal": inputs_equal, "player_inputs_equal": players_equal,
		"execution_inputs_equal": execution_equal,
		"sampled_states_equal": states_equal, "terminal_results_equal": outcomes_equal,
		"normalized_trace_sha256": digests, "sampled_event_counts": [traces[0].len(), traces[1].len()],
		"first_difference": difference, "original_outcome": first["battle_outcome"],
		"verification_scope": "identical pinned inputs, sampled states, action effects and original terminal result"}),
	)
}

pub fn compare(first: &Path, second: &Path) -> Result<Value> {
	ensure!(
		first.canonicalize()? != second.canonicalize()?,
		"Compare two different owned Player runs"
	);
	let mut report: Value = compare_values(&read_json(first)?, &read_json(second)?)?;
	report["run_results"] = json!([{"path": first, "sha256": sha256_file(first)?},
		{"path": second, "sha256": sha256_file(second)?}]);
	write_json(&second.with_file_name("repeatability.json"), &report)?;
	Ok(report)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn run() -> Value {
		json!({"passed": true, "maa_job_complete": true, "battle_outcome": {"terminal_verified": true,
			"status": "original_win"}, "battle_inputs": {"actions_sha256": "actions", "squad_sha256": "squad",
			"seed": 1, "max_ticks": 12000}, "effective_inputs": [], "execution_inputs": [{"role":"scene/bundle/000","sha256":"original"}], "events": [
			{"stage": "original_enemy_instance_id", "value": 1001},
			{"stage": "original_enemy_hp", "path": "2700", "thread_id": 1}]})
	}

	#[test]
	fn process_identity_changes_preserve_trace_but_unit_replacement_does_not() {
		let first: Value = run();
		let mut second: Value = first.clone();
		second["events"][0]["value"] = json!(5001);
		second["events"][1]["thread_id"] = json!(20);
		assert_eq!(compare_values(&first, &second).unwrap()["passed"], true);
		let before = json!([{"stage": "original_enemy_instance_id", "value": 1},
			{"stage": "original_enemy_instance_id", "value": 1}]);
		let after = json!([{"stage": "original_enemy_instance_id", "value": 1},
			{"stage": "original_enemy_instance_id", "value": 2}]);
		assert_ne!(
			normalize(before.as_array().unwrap()).unwrap(),
			normalize(after.as_array().unwrap()).unwrap()
		);
	}

	#[test]
	fn matching_wins_do_not_pass_when_a_job_is_incomplete() {
		let mut value: Value = run();
		value["maa_job_complete"] = json!(false);
		assert_eq!(compare_values(&value, &value).unwrap()["passed"], false);
	}

	#[test]
	fn hp_changes_and_different_inputs_prevent_repeatability() {
		let first: Value = run();
		let mut second: Value = first.clone();
		second["events"][1]["path"] = json!("2600");
		assert_eq!(compare_values(&first, &second).unwrap()["passed"], false);
		second = first.clone();
		second["execution_inputs"][0]["sha256"] = json!("different-original-bundle");
		assert_eq!(compare_values(&first, &second).unwrap()["passed"], false);
		second = first.clone();
		second["battle_inputs"]["seed"] = json!(2);
		assert_eq!(compare_values(&first, &second).unwrap()["passed"], false);
	}

	#[test]
	fn missing_terminal_or_input_evidence_is_rejected() {
		let mut value: Value = run();
		value["battle_outcome"]["terminal_verified"] = json!(false);
		assert!(compare_values(&value, &value).is_err());
		value = run();
		value.as_object_mut().unwrap().remove("battle_inputs");
		assert!(compare_values(&value, &value).is_err());
	}
}
