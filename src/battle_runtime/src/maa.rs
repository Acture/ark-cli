//! Strict MAA state profile; unsupported executable fields are rejected.
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const PROFILE: &str = "original-state-v3";
pub(crate) fn integer(value: &Value, low: i64, high: i64, label: &str) -> Result<i64> {
	let value = value
		.as_i64()
		.with_context(|| format!("Invalid integer {label}"))?;
	ensure!((low..=high).contains(&value), "Out of range {label}");
	Ok(value)
}
pub(crate) fn fields(
	value: &Value,
	allowed: &[&str],
	required: &[&str],
	label: &str,
) -> Result<()> {
	let object = value
		.as_object()
		.with_context(|| format!("Expected object {label}"))?;
	ensure!(
		object.keys().all(|k| allowed.contains(&k.as_str()))
			&& required.iter().all(|k| object.contains_key(*k)),
		"Unsupported or missing fields in {label}"
	);
	Ok(())
}
pub(crate) fn operators_by_name(identities: &Value) -> Result<BTreeMap<String, Vec<String>>> {
	let mut result: BTreeMap<String, Vec<String>> = BTreeMap::new();
	for (id, record) in identities
		.as_object()
		.context("Character identities not object")?
	{
		if id.starts_with("char_") {
			if let Some(name) = record["name"].as_str() {
				result
					.entry(name.to_owned())
					.or_default()
					.push(id.to_owned());
			}
		}
	}
	Ok(result)
}
/// Summons and devices deployed from cards an operator brings, named as in the original table.
fn token_by_name(identities: &Value, name: &str) -> Result<Option<String>> {
	let matches: Vec<&String> = identities
		.as_object()
		.context("Character identities not object")?
		.iter()
		.filter(|(id, record)| id.starts_with("token_") && record["name"].as_str() == Some(name))
		.map(|(id, _)| id)
		.collect();
	ensure!(matches.len() <= 1, "Ambiguous original token name {name}");
	Ok(matches.first().map(|id| (*id).clone()))
}
fn grid(location: &Value) -> Result<(i64, i64)> {
	let location = location.as_array().context("Location must be [x,y]")?;
	ensure!(location.len() == 2, "Expected [x,y]");
	Ok((
		integer(&location[0], 0, 63, "column")?,
		integer(&location[1], 0, 63, "row")?,
	))
}
pub(crate) fn operator_name(operator: &Value) -> Result<&str> {
	fields(
		operator,
		&[
			"name",
			"role",
			"skill",
			"skill_usage",
			"skill_times",
			"requirements",
		],
		&["name"],
		"MAA operator",
	)?;
	let name = operator["name"]
		.as_str()
		.context("Operator name not string")?;
	ensure!(!name.is_empty(), "Empty operator name");
	Ok(name)
}
pub(crate) fn requirements(operator: &Value) -> Result<BTreeMap<String, i64>> {
	let default = json!({});
	let value = operator.get("requirements").unwrap_or(&default);
	fields(
		value,
		&[
			"elite",
			"level",
			"skill_level",
			"module",
			"module_level",
			"potential",
			"potentiality",
		],
		&[],
		"operator requirements",
	)?;
	let mut result = BTreeMap::from([(
		"skill".to_owned(),
		integer(
			operator.get("skill").unwrap_or(&json!(0)),
			0,
			3,
			"MAA skill",
		)?,
	)]);
	for (key, high) in [
		("elite", 2),
		("level", 1000),
		("skill_level", 10),
		("potential", 6),
		("potentiality", 6),
		("module", 5),
		("module_level", 3),
	] {
		if let Some(value) = value.get(key) {
			let v = integer(value, -1, high, key)?;
			if v >= 0 {
				result.insert(
					if key == "potentiality" {
						"potential"
					} else {
						key
					}
					.to_owned(),
					v,
				);
			}
		}
	}
	Ok(result)
}
fn policy(value: &Value) -> Result<(i64, i64)> {
	Ok((
		integer(
			value.get("skill_usage").unwrap_or(&json!(0)),
			0,
			2,
			"skill_usage",
		)?,
		integer(
			value.get("skill_times").unwrap_or(&json!(1)),
			1,
			99,
			"skill_times",
		)?,
	))
}
fn satisfies(actual: &Value, wanted: &BTreeMap<String, i64>, modules: &Value) -> bool {
	let skill = wanted["skill"];
	// A skill number cannot apply to an operator without skills (original skill index -1).
	let has_skills: bool = actual["skill_index"].as_i64() != Some(-1);
	if skill > 0 && has_skills && actual["skill_index"].as_i64() != Some(skill - 1) {
		return false;
	}
	let equipped: Vec<&Value> = actual["modules"].as_array().into_iter().flatten().collect();
	match wanted.get("module") {
		Some(0) if !equipped.is_empty() => return false,
		// A module type requirement names the original module's type; its level is a minimum.
		Some(&number) if number > 0 => {
			let letter = crate::squad::module_type_letter(number);
			let matching = equipped.iter().any(|module| {
				let key = module["key"].as_str().unwrap_or_default();
				letter.is_some()
					&& modules["equipDict"][key]["typeName2"].as_str() == letter
					&& module["level"].as_i64() >= wanted.get("module_level").copied()
			});
			if !matching {
				return false;
			}
		}
		_ => {}
	}
	[
		("elite", "phase", 0),
		("level", "level", 0),
		("potential", "potential_rank", 1),
		("skill_level", "skill_level", 0),
	]
	.iter()
	.all(|(key, field, offset)| {
		actual[*field]
			.as_i64()
			.is_some_and(|v| v + offset >= *wanted.get(*key).unwrap_or(&0))
	})
}
pub(crate) fn translate(
	job: &Value,
	squad: &Value,
	identities: &Value,
	modules: &Value,
) -> Result<Value> {
	crate::squad::compile(squad)?;
	fields(
		job,
		&[
			"version",
			"minimum_required",
			"stage_name",
			"actions",
			"doc",
			"opers",
			"groups",
			"difficulty",
		],
		&["minimum_required", "stage_name", "actions"],
		"MAA job",
	)?;
	ensure!(
		job["minimum_required"].as_str().is_some_and(|v| Regex::new(
			r"^v?\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$"
		)
		.unwrap()
		.is_match(v)),
		"Invalid minimum_required"
	);
	integer(
		job.get("difficulty").unwrap_or(&json!(0)),
		0,
		1,
		"difficulty",
	)?;
	let by_name = operators_by_name(identities)?;
	let roster: BTreeMap<&str, &Value> = squad["operators"]
		.as_array()
		.unwrap()
		.iter()
		.map(|op| (op["character_id"].as_str().unwrap(), op))
		.collect();
	let mut bindings: BTreeMap<String, String> = BTreeMap::new();
	let mut seen: BTreeSet<String> = BTreeSet::new();
	let mut policies: Vec<Value> = Vec::new();
	let select = |op: &Value| -> Result<Option<String>> {
		let name = operator_name(op)?;
		policy(op)?;
		let wanted = requirements(op)?;
		let matches: Vec<&String> = by_name
			.get(name)
			.into_iter()
			.flatten()
			.filter(|id| roster.contains_key(id.as_str()))
			.collect();
		ensure!(
			matches.len() <= 1,
			"Ambiguous original operator name {name}"
		);
		Ok(matches
			.first()
			.filter(|id| satisfies(roster[id.as_str()], &wanted, modules))
			.map(|id| (*id).clone()))
	};
	let mut bind = |label: &str, id: String, op: &Value| -> Result<()> {
		ensure!(
			!label.is_empty() && !bindings.contains_key(label) && seen.insert(id.clone()),
			"Duplicate operator binding {label}"
		);
		let (usage, times) = policy(op)?;
		if usage > 0 {
			policies.push(json!({"character_id":id,"skill_usage":usage,"skill_times":times}));
		}
		bindings.insert(label.to_owned(), id);
		Ok(())
	};
	let empty = json!([]);
	for op in job
		.get("opers")
		.unwrap_or(&empty)
		.as_array()
		.context("opers must be array")?
	{
		let id = select(op)?.context("Operator missing or below requirements")?;
		bind(operator_name(op)?, id, op)?;
	}
	for group in job
		.get("groups")
		.unwrap_or(&empty)
		.as_array()
		.context("groups must be array")?
	{
		fields(group, &["name", "opers"], &["name", "opers"], "group")?;
		let mut eligible: Vec<(String, &Value)> = Vec::new();
		for op in group["opers"].as_array().context("Group opers not array")? {
			if let Some(id) = select(op)? {
				eligible.push((id, op));
			}
		}
		ensure!(
			eligible.len() == 1,
			"Group needs exactly one eligible explicit operator"
		);
		let (id, op) = eligible.pop().unwrap();
		bind(
			group["name"].as_str().context("Group name invalid")?,
			id,
			op,
		)?;
	}
	let actions = job["actions"].as_array().context("MAA actions not array")?;
	ensure!(
		(1..=128).contains(&actions.len()),
		"Expected 1..128 MAA actions"
	);
	let mut commands: Vec<Value> = Vec::new();
	for (index, action) in actions.iter().enumerate() {
		fields(
			action,
			&[
				"type",
				"name",
				"location",
				"direction",
				"kills",
				"costs",
				"cost_changes",
				"cooling",
				"skill_usage",
				"skill_times",
				"pre_delay",
				"post_delay",
				"timeout",
				"doc",
				"doc_color",
			],
			&[],
			"MAA action",
		)?;
		let kind = match action
			.get("type")
			.unwrap_or(&json!("Deploy"))
			.as_str()
			.context("Action type invalid")?
		{
			"Deploy" | "部署" => "spawn",
			"Retreat" | "撤退" => "withdraw",
			"Skill" | "技能" => "skill",
			"SpeedUp" | "二倍速" => "speed_up",
			"SkillDaemon" | "摆完挂机" => "skill_daemon",
			"SkillUsage" | "技能用法" => "skill_usage",
			"Output" | "打印" => "output",
			name => bail!("Unsupported MAA action {name}"),
		};
		for key in ["pre_delay", "post_delay"] {
			if let Some(v) = action.get(key) {
				ensure!(
					integer(v, 0, i32::MAX as i64, key)? == 0,
					"Wall-clock delay unsupported"
				);
			}
		}
		if let Some(timeout) = action.get("timeout") {
			ensure!(
				timeout.as_i64() == Some(-1),
				"Finite wall-clock timeout unsupported"
			);
		}
		if kind == "output" {
			fields(action, &["type", "doc", "doc_color"], &[], "Output action")?;
			continue;
		}
		ensure!(
			kind == "skill_usage"
				|| (action.get("skill_usage").is_none() && action.get("skill_times").is_none()),
			"Skill policy on non-SkillUsage action"
		);
		let mut cmd = json!({"kind":kind,"kills":integer(action.get("kills").unwrap_or(&json!(0)),0,99999,"kills")?,"costs":integer(action.get("costs").unwrap_or(&json!(0)),0,99999,"costs")?,"cost_changes":integer(action.get("cost_changes").unwrap_or(&json!(0)),-99999,99999,"cost_changes")?,"cooling":integer(action.get("cooling").unwrap_or(&json!(-1)),-1,99,"cooling")?});
		if matches!(kind, "speed_up" | "skill_daemon") {
			ensure!(
				["name", "location", "direction"]
					.iter()
					.all(|k| action.get(k).is_none()),
				"Unexpected control action target"
			);
			ensure!(
				kind != "skill_daemon" || index == actions.len() - 1,
				"SkillDaemon must be last"
			);
			cmd["action_index"] = json!(index);
			commands.push(cmd);
			continue;
		}
		let name = action["name"]
			.as_str()
			.context("Location-only Skill/Retreat unsupported; the action needs a name")?;
		let target: String = match bindings.get(name) {
			Some(id) => id.clone(),
			None if kind != "skill_usage" => token_by_name(identities, name)?
				.with_context(|| format!("Action operator/group/token unresolved: {name}"))?,
			None => bail!("SkillUsage targets an undeclared operator {name}"),
		};
		cmd["character_id"] = json!(target);
		if kind == "skill_usage" {
			ensure!(
				action.get("skill_usage").is_some(),
				"SkillUsage requires skill_usage"
			);
			let (usage, times) = policy(action)?;
			cmd["skill_usage"] = json!(usage);
			cmd["skill_times"] = json!(times);
		}
		if kind == "spawn" {
			let (col, row) = grid(action.get("location").context("Deploy location missing")?)?;
			cmd["col"] = json!(col);
			cmd["row"] = json!(row);
			cmd["direction"] = json!(match action["direction"]
				.as_str()
				.context("Deploy direction missing")?
			{
				"Up" | "上" => "UP",
				"Right" | "右" => "RIGHT",
				"Down" | "下" => "DOWN",
				"Left" | "左" => "LEFT",
				_ => bail!("Unsupported direction"),
			});
		} else {
			ensure!(
				action.get("direction").is_none(),
				"Direction on a non-Deploy action"
			);
			// A located Skill/Retreat must find the named unit on that tile; the probe checks it.
			if let Some(location) = action.get("location") {
				ensure!(kind != "skill_usage", "SkillUsage takes no location");
				let (col, row) = grid(location)?;
				cmd["col"] = json!(col);
				cmd["row"] = json!(row);
				cmd["located"] = json!(1);
			}
		}
		cmd["action_index"] = json!(index);
		commands.push(cmd);
	}
	ensure!(!commands.is_empty(), "No executable MAA actions");
	let names: BTreeMap<String, String> = by_name
		.iter()
		.flat_map(|(name, ids)| ids.iter().map(move |id| (id.clone(), name.clone())))
		.filter(|(id, _)| roster.contains_key(id.as_str()))
		.collect();
	Ok(json!({"policies":policies,"commands":commands,"bindings":bindings,"names":names}))
}

/// The job as the original engine established it: actions up to the last completed command,
/// and each operator's requirements set to the explicit training that ran. MAA can execute it.
pub(crate) fn concrete_job(
	job: &Value,
	plan: &Value,
	squad: &Value,
	modules: &Value,
	completed: usize,
	note: &str,
) -> Result<Value> {
	let commands = plan["commands"].as_array().context("Commands missing")?;
	let actions = job["actions"].as_array().context("MAA actions not array")?;
	let kept: usize = if completed >= commands.len() {
		actions.len()
	} else if completed == 0 {
		0
	} else {
		commands[completed - 1]["action_index"]
			.as_u64()
			.context("Command has no source action")? as usize
			+ 1
	};
	let training = |label: &str| -> Result<Value> {
		let id = plan["bindings"][label]
			.as_str()
			.with_context(|| format!("Unbound operator {label}"))?;
		let actual = squad["operators"]
			.as_array()
			.into_iter()
			.flatten()
			.find(|op| op["character_id"] == id)
			.with_context(|| format!("{id} is not in the squad"))?;
		let module: i64 = actual["modules"]
			.as_array()
			.and_then(|list| list.first())
			.and_then(|module| modules["equipDict"][module["key"].as_str()?]["typeName2"].as_str())
			.and_then(|letter| ["X", "Y", "A", "D", "B"].iter().position(|l| *l == letter))
			.map_or(0, |position| position as i64 + 1);
		Ok(
			json!({"elite":actual["phase"],"level":actual["level"],"skill_level":actual["skill_level"],
			"potentiality":actual["potential_rank"].as_i64().unwrap_or(0) + 1,"module":module}),
		)
	};
	let mut concrete: Value = job.clone();
	if let Some(operators) = concrete.get_mut("opers").and_then(Value::as_array_mut) {
		for operator in operators {
			let name: String = operator_name(operator)?.to_owned();
			operator["requirements"] = training(&name)?;
		}
	}
	if let Some(groups) = concrete.get_mut("groups").and_then(Value::as_array_mut) {
		for group in groups {
			let label: String = group["name"]
				.as_str()
				.context("Group name invalid")?
				.to_owned();
			let id: &str = plan["bindings"][&label].as_str().context("Unbound group")?;
			// The group member whose original name maps to the identity that ran.
			let mut chosen: Value = group["opers"]
				.as_array()
				.into_iter()
				.flatten()
				.find(|op| {
					plan["names"][id]
						.as_str()
						.is_some_and(|name| op["name"] == name)
				})
				.cloned()
				.with_context(|| format!("Group {label} has no executed member"))?;
			chosen["requirements"] = training(&label)?;
			group["opers"] = json!([chosen]);
		}
	}
	concrete["actions"] = json!(actions[..kept]);
	let title: &str = job["doc"]["title"].as_str().unwrap_or_default();
	concrete["doc"] =
		json!({"title": format!("{title} [ark-battle-runtime]").trim_start(), "details": note});
	Ok(concrete)
}

/// Battle program conditions, met in order by the probe.
pub(crate) const CONDITION_KILLS: i64 = 1;
pub(crate) const CONDITION_COST: i64 = 2;
pub(crate) const CONDITION_COST_CHANGE: i64 = 3;
pub(crate) const CONDITION_COOLING: i64 = 4;
pub(crate) const CONDITION_HOLD: i64 = 7;

/// A command's conditions in order: explicit program conditions, or an MAA action's in the order
/// MAA waits for them.
fn conditions(cmd: &Value) -> Result<Vec<(i64, i64)>> {
	if let Some(explicit) = cmd.get("conditions") {
		return explicit
			.as_array()
			.context("Conditions must be an array")?
			.iter()
			.map(|pair| {
				Ok((
					pair[0].as_i64().context("Condition kind")?,
					pair[1].as_i64().context("Condition value")?,
				))
			})
			.collect();
	}
	let value = |key: &str| -> i64 { cmd[key].as_i64().unwrap_or(0) };
	let mut list: Vec<(i64, i64)> = Vec::new();
	if value("cost_changes") != 0 {
		list.push((CONDITION_COST_CHANGE, value("cost_changes")));
	}
	if value("kills") > 0 {
		list.push((CONDITION_KILLS, value("kills")));
	}
	if value("costs") > 0 {
		list.push((CONDITION_COST, value("costs")));
	}
	if cmd["cooling"].as_i64().unwrap_or(-1) >= 0 {
		list.push((CONDITION_COOLING, value("cooling")));
	}
	Ok(list)
}

/// Compile the translated job into the probe's battle program: `S` lines set initial skill
/// policies, and each `T` line is one step, an operation behind conditions met in order. A
/// nonzero `delay` makes every unit operation stay ready that many ticks after its conditions are
/// met before it is issued (unless its step declares its own delay),
/// a declared latency such as a screen-recognition client's; skill policies cast a skill only
/// once it has stayed ready that long.
pub(crate) fn compile(plan: &Value, delay: u32) -> Result<String> {
	let policies = plan["policies"].as_array().context("Policies missing")?;
	let commands = plan["commands"].as_array().context("Commands missing")?;
	ensure!(
		policies.len() <= 12 && (1..=128).contains(&commands.len()),
		"Probe capacity exceeded"
	);
	let identifier = Regex::new(r"^(?:char|token)_[A-Za-z0-9_]{1,75}$")?;
	let mut result = String::from("ARKP1\n");
	if delay > 0 {
		// Skill policies take the same latency: a skill must stay ready that long.
		result.push_str(&format!("D|{delay}|_\n"));
	}
	for p in policies {
		let id = p["character_id"].as_str().context("Policy ID missing")?;
		ensure!(identifier.is_match(id), "Invalid policy ID");
		result.push_str(&format!(
			"S|{}|{}|{id}\n",
			p["skill_usage"], p["skill_times"]
		));
	}
	for cmd in commands {
		let id = cmd["character_id"].as_str().unwrap_or("_");
		ensure!(id == "_" || identifier.is_match(id), "Invalid command ID");
		let kind = match cmd["kind"].as_str().context("Command kind missing")? {
			"spawn" => 0,
			"withdraw" => 1,
			"skill" => 2,
			"speed_up" => 3,
			"skill_daemon" => 4,
			"skill_usage" => 5,
			"cast" => 6,
			"suspend" => 7,
			_ => bail!("Invalid compiled command kind"),
		};
		let direction = match cmd["direction"].as_str().unwrap_or("UP") {
			"UP" => 0,
			"RIGHT" => 1,
			"DOWN" => 2,
			"LEFT" => 3,
			_ => bail!("Invalid compiled direction"),
		};
		let mut conditions: Vec<(i64, i64)> = conditions(cmd)?;
		if delay > 0
			&& matches!(kind, 0 | 1 | 2 | 6 | 7)
			&& !conditions.iter().any(|(code, _)| *code == CONDITION_HOLD)
		{
			conditions.push((CONDITION_HOLD, i64::from(delay)));
		}
		ensure!(
			conditions.len() <= 8,
			"A step has more than eight conditions"
		);
		let encoded: String = conditions
			.iter()
			.map(|(kind, value)| format!("|{kind}|{value}"))
			.collect();
		result.push_str(&format!(
			"T|{kind}|{}|{}|{direction}|{}|{}|{}|{}{encoded}|{id}\n",
			cmd.get("row").unwrap_or(&json!(0)),
			cmd.get("col").unwrap_or(&json!(0)),
			cmd.get("skill_usage").unwrap_or(&json!(0)),
			cmd.get("skill_times").unwrap_or(&json!(0)),
			cmd.get("located").unwrap_or(&json!(0)),
			conditions.len()
		));
	}
	Ok(result)
}
pub(crate) fn command_count(native: &str) -> usize {
	native.lines().filter(|l| l.starts_with("T|")).count()
}

#[cfg(test)]
mod tests {
	use super::*;
	fn fixture() -> (Value, Value, Value) {
		(
			json!({"minimum_required":"v6.0.0","stage_name":"stage_fixture","opers":[{"name":"甲","skill":1,"skill_usage":2,"skill_times":3}],"actions":[{"type":"部署","name":"甲","location":[4,3],"direction":"左","cost_changes":-5,"cooling":2},{"type":"SkillUsage","name":"甲","skill_usage":0},{"type":"SkillDaemon"}]}),
			json!({"schema":"ark-battle-squad-v2","operators":[{"character_id":"char_fixture_a","level":60,"phase":2,"potential_rank":5,"favor_battle_phase":0,"skill_index":0,"skill_level":7}]}),
			json!({"char_fixture_a":{"name":"甲"},"char_fixture_b":{"name":"乙"}}),
		)
	}
	#[test]
	fn ordered_conditions_coordinates_and_skill_policies_keep_wire_semantics() {
		let (job, squad, identities) = fixture();
		let plan = translate(&job, &squad, &identities, &json!({})).unwrap();
		assert_eq!(compile(&plan, 0).unwrap(),"ARKP1\nS|2|3|char_fixture_a\nT|0|3|4|3|0|0|0|2|3|-5|4|2|char_fixture_a\nT|5|0|0|0|0|1|0|0|char_fixture_a\nT|4|0|0|0|0|0|0|0|_\n");
	}
	#[test]
	fn unsupported_executable_fields_never_disappear() {
		for (key, value) in [
			("pre_delay", json!(1)),
			("post_delay", json!(1)),
			("timeout", json!(0)),
			("elapsed_time", json!(1)),
			("skip_if_not_ready", json!(true)),
			("kills", json!(true)),
			("costs", json!(-1)),
			("cooling", json!(100)),
			("skill_usage", json!(1)),
		] {
			let (mut job, squad, identities) = fixture();
			job["actions"][0][key] = value;
			assert!(
				translate(&job, &squad, &identities, &json!({})).is_err(),
				"{key}"
			);
		}
		for kind in [
			"BulletTime",
			"MoveCamera",
			"ResetStopwatch",
			"Click",
			"SetUnitLocation",
		] {
			let (mut job, squad, identities) = fixture();
			job["actions"][0]["type"] = json!(kind);
			assert!(
				translate(&job, &squad, &identities, &json!({})).is_err(),
				"{kind}"
			);
		}
	}
	#[test]
	fn explicit_squad_training_and_group_ambiguity_are_enforced() {
		let (mut job, squad, identities) = fixture();
		job["opers"][0]["requirements"] = json!({"elite":1,"level":61});
		assert!(translate(&job, &squad, &identities, &json!({})).is_err());
		job["opers"][0]["requirements"]["level"] = json!(60);
		assert!(translate(&job, &squad, &identities, &json!({})).is_ok());
		job["opers"][0]["skill"] = json!(2);
		assert!(translate(&job, &squad, &identities, &json!({})).is_err());
		let (mut job, mut squad, identities) = fixture();
		job["opers"] = json!([]);
		job["groups"] = json!([{"name":"甲","opers":[{"name":"甲"},{"name":"乙"}]}]);
		assert!(translate(&job, &squad, &identities, &json!({})).is_ok());
		let mut second = squad["operators"][0].clone();
		second["character_id"] = json!("char_fixture_b");
		squad["operators"].as_array_mut().unwrap().push(second);
		assert!(translate(&job, &squad, &identities, &json!({})).is_err());
	}
	#[test]
	fn tokens_resolve_by_original_name_and_located_actions_keep_their_tile() {
		let (mut job, squad, mut identities) = fixture();
		identities["token_fixture_s"] = json!({"name":"召唤"});
		job["actions"] = json!([
			{"type":"Deploy","name":"召唤","location":[2,1],"direction":"Up"},
			{"type":"Skill","name":"甲","location":[4,3]},
			{"type":"Retreat","name":"召唤","location":[2,1]}
		]);
		let native = compile(
			&translate(&job, &squad, &identities, &json!({})).unwrap(),
			0,
		)
		.unwrap();
		assert!(native.contains("T|0|1|2|0|0|0|0|0|token_fixture_s"));
		assert!(native.contains("T|2|3|4|0|0|0|1|0|char_fixture_a"));
		job["actions"] = json!([{"type":"Skill","location":[4,3]}]);
		assert!(translate(&job, &squad, &identities, &json!({})).is_err());
		job["actions"] = json!([{"type":"Deploy","name":"未知","location":[1,1],"direction":"Up"}]);
		assert!(translate(&job, &squad, &identities, &json!({})).is_err());
	}
	#[test]
	fn module_type_requirements_match_the_equipped_original_module() {
		let (mut job, mut squad, identities) = fixture();
		let modules = json!({"equipDict":{"uniequip_002_a":{"typeName2":"X"},"uniequip_003_a":{"typeName2":"Y"}}});
		job["opers"][0]["requirements"] = json!({"module":2});
		assert!(
			translate(&job, &squad, &identities, &modules).is_err(),
			"no module equipped"
		);
		squad["operators"][0]["modules"] = json!([{"key":"uniequip_002_a","level":3}]);
		assert!(
			translate(&job, &squad, &identities, &modules).is_err(),
			"X is not γ"
		);
		squad["operators"][0]["modules"] = json!([{"key":"uniequip_003_a","level":1}]);
		assert!(translate(&job, &squad, &identities, &modules).is_ok());
		job["opers"][0]["requirements"] = json!({"module":0});
		assert!(
			translate(&job, &squad, &identities, &modules).is_err(),
			"job forbids modules"
		);
	}
	#[test]
	fn policies_are_not_ordered_commands() {
		assert_eq!(
			command_count("ARKP1\nS|1|1|char_x\nT|0|0|0|0|0|0|0|0|char_x\n"),
			1
		);
	}
	#[test]
	fn invalid_numbers_and_unknown_fields_reject() {
		assert!(integer(&json!(true), 0, 1, "test").is_err());
		assert!(fields(&json!({"unknown":1}), &[], &[], "test").is_err());
	}
}
