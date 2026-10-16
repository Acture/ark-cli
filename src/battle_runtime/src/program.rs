//! Battle programs: the probe's ordered steps as a document. A program names operators as MAA
//! jobs do (`opers`, `groups`, `stage_name`), and each step is an operation behind conditions met
//! in order. Programs express what MAA cannot (tick and wait conditions, a step delay after its
//! operation is ready, cast-only and suspend-only operations); one that MAA can express exports to
//! an MAA job.

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};

use crate::maa::{fields, integer};

pub(crate) const SCHEMA: &str = "ark-battle-program-v1";

/// Condition names and their probe codes. MAA waits for its four in this order.
const CONDITIONS: [(&str, i64); 6] = [
	("cost_change", 3),
	("kills", 1),
	("cost", 2),
	("cooling", 4),
	("tick", 5),
	("wait", 6),
];

/// Operations, the MAA action type each translates through, and the command kind it forces.
const OPERATIONS: [(&str, &str, Option<&str>); 8] = [
	("deploy", "Deploy", None),
	("retreat", "Retreat", None),
	("press", "Skill", None),
	("cast", "Skill", Some("cast")),
	("suspend", "Skill", Some("suspend")),
	("speed", "SpeedUp", None),
	("policy", "SkillUsage", None),
	("end", "SkillDaemon", None),
];

/// Step fields carried over to and from MAA actions unchanged.
const TARGET_FIELDS: [&str; 5] = [
	"name",
	"location",
	"direction",
	"skill_usage",
	"skill_times",
];

pub(crate) fn is_program(value: &Value) -> bool {
	value["schema"] == SCHEMA
}

fn steps(program: &Value) -> Result<&Vec<Value>> {
	let steps = program["steps"]
		.as_array()
		.context("Program steps must be an array")?;
	ensure!(
		(1..=128).contains(&steps.len()),
		"Expected 1..128 program steps"
	);
	Ok(steps)
}

/// A step's conditions as (name, value) pairs, validated.
fn step_conditions(step: &Value) -> Result<Vec<(&'static str, i64)>> {
	let empty: Vec<Value> = Vec::new();
	let when = step
		.get("when")
		.map(|value| value.as_array().context("Step `when` must be an array"))
		.transpose()?
		.unwrap_or(&empty);
	ensure!(when.len() <= 8, "A step has more than eight conditions");
	when.iter()
		.map(|condition| {
			let object = condition
				.as_object()
				.filter(|object| object.len() == 1)
				.context("Each condition is an object with one key")?;
			let (key, value) = object.iter().next().unwrap();
			let (name, _) = CONDITIONS
				.iter()
				.find(|(name, _)| name == key)
				.with_context(|| format!("Unknown condition {key}"))?;
			let value: i64 = match *name {
				"cost_change" => {
					let value = integer(value, -99999, 99999, key)?;
					ensure!(value != 0, "cost_change must be nonzero");
					value
				}
				"cooling" => integer(value, 0, 99, key)?,
				_ => integer(value, 0, 99999, key)?,
			};
			Ok((*name, value))
		})
		.collect()
}

/// Ticks a step's operation must stay ready, after its conditions, before it is issued.
fn step_delay(step: &Value) -> Result<i64> {
	step.get("delay")
		.map_or(Ok(0), |value| integer(value, 0, 900, "delay"))
}

fn operation(step: &Value) -> Result<&'static (&'static str, &'static str, Option<&'static str>)> {
	let name = step["do"].as_str().context("Step `do` missing")?;
	OPERATIONS
		.iter()
		.find(|(op, _, _)| *op == name)
		.with_context(|| format!("Unknown operation {name}"))
}

/// The MAA-shaped job a program translates through, so operator names, groups, summons and
/// locations resolve exactly as for MAA jobs.
pub(crate) fn as_job(program: &Value) -> Result<Value> {
	fields(
		program,
		&[
			"schema",
			"minimum_required",
			"stage_name",
			"opers",
			"groups",
			"steps",
			"doc",
		],
		&["schema", "stage_name", "steps"],
		"battle program",
	)?;
	let mut actions: Vec<Value> = Vec::new();
	for step in steps(program)? {
		fields(
			step,
			&[
				"when",
				"do",
				"name",
				"location",
				"direction",
				"skill_usage",
				"skill_times",
				"delay",
				"doc",
			],
			&["do"],
			"program step",
		)?;
		step_conditions(step)?;
		step_delay(step)?;
		let (_, maa_type, _) = operation(step)?;
		let mut action: Map<String, Value> = Map::new();
		action.insert("type".to_owned(), json!(maa_type));
		for key in TARGET_FIELDS {
			if let Some(value) = step.get(key) {
				action.insert(key.to_owned(), value.clone());
			}
		}
		actions.push(Value::Object(action));
	}
	let mut job: Map<String, Value> = Map::new();
	job.insert(
		"minimum_required".to_owned(),
		program
			.get("minimum_required")
			.cloned()
			.unwrap_or(json!("v4.0.0")),
	);
	job.insert("stage_name".to_owned(), program["stage_name"].clone());
	for key in ["opers", "groups", "doc"] {
		if let Some(value) = program.get(key) {
			job.insert(key.to_owned(), value.clone());
		}
	}
	job.insert("actions".to_owned(), json!(actions));
	Ok(Value::Object(job))
}

/// The program's explicit conditions and forced operation kinds onto its translated plan, whose
/// commands map one to one onto the steps.
pub(crate) fn apply(plan: &mut Value, program: &Value) -> Result<()> {
	let steps = steps(program)?;
	for command in plan["commands"]
		.as_array_mut()
		.context("Commands missing")?
	{
		let index = command["action_index"]
			.as_u64()
			.context("Command has no source step")? as usize;
		let step = steps.get(index).context("Command step out of range")?;
		let mut conditions: Vec<Value> = step_conditions(step)?
			.into_iter()
			.map(|(name, value)| {
				let code = CONDITIONS.iter().find(|(n, _)| *n == name).unwrap().1;
				json!([code, value])
			})
			.collect();
		let delay: i64 = step_delay(step)?;
		if delay > 0 {
			ensure!(
				conditions.len() < 8,
				"A step has more than eight conditions"
			);
			conditions.push(json!([crate::maa::CONDITION_HOLD, delay]));
		}
		command["conditions"] = json!(conditions);
		if let Some(kind) = operation(step)?.2 {
			command["kind"] = json!(kind);
		}
	}
	Ok(())
}

/// An MAA job as a program: each action one step, with MAA's conditions in MAA's order.
pub(crate) fn from_maa(job: &Value) -> Result<Value> {
	let mut steps: Vec<Value> = Vec::new();
	for action in job["actions"].as_array().context("MAA actions not array")? {
		let deploy: Value = json!("Deploy");
		let kind = action
			.get("type")
			.unwrap_or(&deploy)
			.as_str()
			.context("Action type invalid")?;
		let op: &str = match kind {
			"Deploy" | "部署" => "deploy",
			"Retreat" | "撤退" => "retreat",
			"Skill" | "技能" => "press",
			"SpeedUp" | "二倍速" => "speed",
			"SkillDaemon" | "摆完挂机" => "end",
			"SkillUsage" | "技能用法" => "policy",
			"Output" | "打印" => continue,
			name => bail!("Unsupported MAA action {name}"),
		};
		for key in ["pre_delay", "post_delay"] {
			if let Some(value) = action.get(key) {
				ensure!(
					integer(value, 0, i32::MAX as i64, key)? == 0,
					"Wall-clock delay unsupported"
				);
			}
		}
		let mut when: Vec<Value> = Vec::new();
		for (maa_key, name) in [
			("cost_changes", "cost_change"),
			("kills", "kills"),
			("costs", "cost"),
			("cooling", "cooling"),
		] {
			if let Some(value) = action.get(maa_key).and_then(Value::as_i64) {
				// MAA's absent values: 0 kills, costs and cost changes; -1 cooling.
				let present: bool = match name {
					"cooling" => value >= 0,
					"cost_change" => value != 0,
					_ => value > 0,
				};
				if present {
					when.push(json!({ name: value }));
				}
			}
		}
		let mut step: Map<String, Value> = Map::new();
		if !when.is_empty() {
			step.insert("when".to_owned(), json!(when));
		}
		step.insert("do".to_owned(), json!(op));
		for key in TARGET_FIELDS.iter().chain(["doc"].iter()) {
			if let Some(value) = action.get(*key) {
				step.insert((*key).to_owned(), value.clone());
			}
		}
		steps.push(Value::Object(step));
	}
	let mut program: Map<String, Value> = Map::new();
	program.insert("schema".to_owned(), json!(SCHEMA));
	for key in ["minimum_required", "stage_name", "opers", "groups", "doc"] {
		if let Some(value) = job.get(key) {
			program.insert(key.to_owned(), value.clone());
		}
	}
	program.insert("steps".to_owned(), json!(steps));
	Ok(Value::Object(program))
}

/// A program as an MAA job when MAA can express it, otherwise every reason it cannot.
pub(crate) fn to_maa(program: &Value) -> Result<std::result::Result<Value, Vec<String>>> {
	as_job(program)?;
	let mut reasons: Vec<String> = Vec::new();
	let mut actions: Vec<Value> = Vec::new();
	for (index, step) in steps(program)?.iter().enumerate() {
		let (op, maa_type, forced) = operation(step)?;
		if forced.is_some() {
			reasons.push(format!("step {index}: MAA has no {op}-only operation"));
		}
		if step_delay(step)? > 0 {
			reasons.push(format!("step {index}: MAA has no step delay"));
		}
		let conditions = step_conditions(step)?;
		let mut action: Map<String, Value> = Map::new();
		action.insert("type".to_owned(), json!(maa_type));
		let mut order: Vec<usize> = Vec::new();
		for (name, value) in &conditions {
			let maa_key = match *name {
				"cost_change" => "cost_changes",
				"kills" => "kills",
				"cost" => "costs",
				"cooling" => "cooling",
				other => {
					reasons.push(format!("step {index}: MAA has no {other} condition"));
					continue;
				}
			};
			if action.insert(maa_key.to_owned(), json!(value)).is_some() {
				reasons.push(format!("step {index}: MAA takes one {name} condition"));
			}
			order.push(CONDITIONS.iter().position(|(n, _)| n == name).unwrap());
		}
		if order.windows(2).any(|pair| pair[0] > pair[1]) {
			reasons.push(format!(
				"step {index}: MAA meets conditions in the order cost_change, kills, cost, cooling"
			));
		}
		for key in TARGET_FIELDS.iter().chain(["doc"].iter()) {
			if let Some(value) = step.get(*key) {
				action.insert((*key).to_owned(), value.clone());
			}
		}
		actions.push(Value::Object(action));
	}
	if !reasons.is_empty() {
		return Ok(Err(reasons));
	}
	let mut job: Map<String, Value> = Map::new();
	job.insert(
		"minimum_required".to_owned(),
		program
			.get("minimum_required")
			.cloned()
			.unwrap_or(json!("v4.0.0")),
	);
	for key in ["stage_name", "opers", "groups", "doc"] {
		if let Some(value) = program.get(key) {
			job.insert(key.to_owned(), value.clone());
		}
	}
	job.insert("actions".to_owned(), json!(actions));
	Ok(Ok(Value::Object(job)))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn job() -> Value {
		json!({"minimum_required":"v6.0.0","stage_name":"main_00-01","opers":[{"name":"甲","skill":1}],
			"actions":[
				{"type":"Deploy","name":"甲","location":[4,3],"direction":"Left","cost_changes":-5,"cooling":2},
				{"type":"Output","doc":"note"},
				{"type":"Skill","name":"甲","kills":3},
				{"type":"SkillDaemon"}]})
	}

	#[test]
	fn maa_jobs_round_trip_through_programs() {
		let program = from_maa(&job()).unwrap();
		assert_eq!(program["schema"], SCHEMA);
		assert_eq!(
			program["steps"][0]["when"],
			json!([{"cost_change":-5},{"cooling":2}])
		);
		assert_eq!(
			program["steps"][1],
			json!({"when":[{"kills":3}],"do":"press","name":"甲"})
		);
		let back = to_maa(&program).unwrap().unwrap();
		assert_eq!(back["actions"][0]["cost_changes"], -5);
		assert_eq!(back["actions"][1]["kills"], 3);
		assert_eq!(back["actions"].as_array().unwrap().len(), 3);
	}

	#[test]
	fn programs_beyond_maa_name_every_reason_and_compile_explicit_conditions() {
		let mut program = from_maa(&job()).unwrap();
		program["steps"][1] =
			json!({"when":[{"kills":3},{"wait":30}],"do":"cast","name":"甲","delay":12});
		program["steps"][2] = json!({"when":[{"kills":9},{"cost_change":4}],"do":"end"});
		let reasons = to_maa(&program).unwrap().unwrap_err();
		assert_eq!(reasons.len(), 4, "{reasons:?}");
		let job = as_job(&program).unwrap();
		assert_eq!(job["actions"][1]["type"], "Skill");
		let mut plan = json!({"commands":[{"action_index":0,"kind":"spawn"},{"action_index":1,"kind":"skill"}]});
		apply(&mut plan, &program).unwrap();
		assert_eq!(plan["commands"][0]["conditions"], json!([[3, -5], [4, 2]]));
		assert_eq!(
			plan["commands"][1]["conditions"],
			json!([[1, 3], [6, 30], [7, 12]])
		);
		assert_eq!(plan["commands"][1]["kind"], "cast");
		assert!(
			as_job(&json!({"schema":SCHEMA,"stage_name":"x","steps":[{"do":"jump"}]})).is_err()
		);
		assert!(as_job(
			&json!({"schema":SCHEMA,"stage_name":"x","steps":[{"do":"end","when":[{"wait":1,"tick":2}]}]})
		)
		.is_err());
	}
}
