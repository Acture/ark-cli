//! Strict MAA state profile; unsupported executable fields are rejected.
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::runtime::Difficulty;

pub(crate) const PROFILE: &str = "original-state-v4";
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
/// A deployed name that is no squad member, named as in the original table: a card the level
/// deals (a device such as 干扰地雷), else a summon from a card an operator brings. Device names
/// repeat across the table, so only the level's own cards are candidates for them.
/// The card the level deals under an original name: an operator at the level's fixed training,
/// or a device.
fn level_card(
	identities: &Value,
	level_cards: &BTreeSet<String>,
	name: &str,
) -> Result<Option<String>> {
	let matches: Vec<&String> = level_cards
		.iter()
		.filter(|id| identities[id.as_str()]["name"].as_str() == Some(name))
		.collect();
	ensure!(matches.len() <= 1, "Ambiguous level card name {name}");
	Ok(matches.first().map(|id| (*id).clone()))
}
/// A unit no squad brings: the level's card of that name, else the one original summon token.
pub(crate) fn token_by_name(
	identities: &Value,
	level_cards: &BTreeSet<String>,
	name: &str,
) -> Result<Option<String>> {
	if let Some(id) = level_card(identities, level_cards, name)? {
		return Ok(Some(id));
	}
	let matches: Vec<&String> = identities
		.as_object()
		.context("Character identities not object")?
		.iter()
		.filter(|(id, identity)| {
			id.starts_with("token_") && identity["name"].as_str() == Some(name)
		})
		.map(|(id, _)| id)
		.collect();
	ensure!(matches.len() <= 1, "Ambiguous original token name {name}");
	Ok(matches.first().map(|id| (*id).clone()))
}
/// What the original level brings besides the squad, at the difficulty that runs; the challenge
/// mode adds its hard predefines.
#[derive(Clone, Debug, Default)]
pub(crate) struct LevelUnits {
	/// The cards the level deals, such as a device or an operator at fixed training, hidden ones
	/// included.
	pub cards: BTreeSet<String>,
	/// The units the level places before the battle, by tile (column, row).
	pub placed: BTreeMap<(i64, i64), String>,
}

pub(crate) fn level_units(level: &Value, difficulty: Difficulty) -> LevelUnits {
	let mut sets: Vec<&Value> = vec![&level["predefines"]];
	if difficulty == Difficulty::Challenge {
		sets.push(&level["hardPredefines"]);
	}
	let mut units: LevelUnits = LevelUnits::default();
	// The level's positions number rows from the opposite edge of its map matrix, whose row order
	// MAA's locations and the original deploys use.
	let height: i64 = level["mapData"]["map"].as_array().map_or(0, Vec::len) as i64;
	for set in sets {
		for group in ["characterCards", "tokenCards"] {
			for card in set[group].as_array().into_iter().flatten() {
				if let Some(key) = card["inst"]["characterKey"].as_str() {
					units.cards.insert(key.to_owned());
				}
			}
		}
		for group in ["characterInsts", "tokenInsts"] {
			for unit in set[group].as_array().into_iter().flatten() {
				let position: &Value = &unit["position"];
				if let (Some(key), Some(col), Some(row)) = (
					unit["inst"]["characterKey"].as_str(),
					position["col"].as_i64(),
					position["row"].as_i64(),
				) {
					units.placed.insert((col, height - 1 - row), key.to_owned());
				}
			}
		}
	}
	units
}
/// Whether an action sets one of MAA's conditions, whose absent values are 0 kills, costs and
/// cost changes and -1 cooling.
pub(crate) fn has_conditions(action: &Value) -> bool {
	["kills", "costs", "cost_changes", "cooling"]
		.iter()
		.any(|key| {
			action
				.get(*key)
				.and_then(Value::as_i64)
				.is_some_and(|value| match *key {
					"cooling" => value >= 0,
					"cost_changes" => value != 0,
					_ => value > 0,
				})
		})
}

/// Ticks an MAA wall-clock delay spans at the game speed it ran at: 30 ticks a second at speed 1.
fn delay_ticks(milliseconds: i64, speed: i64) -> i64 {
	(milliseconds * 30 * speed + 500) / 1000
}
/// The wall-clock milliseconds of ticks at a game speed, for MAA.
pub(crate) fn delay_milliseconds(ticks: i64, speed: i64) -> i64 {
	(ticks * 1000 + 15 * speed) / (30 * speed)
}
pub(crate) fn delay(action: &Value, key: &str, speed: i64) -> Result<i64> {
	action.get(key).map_or(Ok(0), |value| {
		Ok(delay_ticks(integer(value, 0, 86_400_000, key)?, speed))
	})
}
/// MAA's timeout abandons an action not done in time: -1 waits without limit, 0 checks once.
pub(crate) fn deadline(action: &Value, speed: i64) -> Result<Option<i64>> {
	let Some(value) = action.get("timeout") else {
		return Ok(None);
	};
	let milliseconds: i64 = integer(value, -1, 86_400_000, "timeout")?;
	Ok((milliseconds >= 0).then(|| delay_ticks(milliseconds, speed)))
}
/// The difficulty's bit in MAA's job mask.
fn maa_bit(difficulty: Difficulty) -> i64 {
	match difficulty {
		Difficulty::Normal => 1,
		Difficulty::Challenge => 2,
	}
}
/// The difficulty a job runs at. MAA's mask selects normal 1, challenge 2, or either 3 (0 leaves
/// it unspecified); a job that allows normal runs it unless the challenge is requested.
pub(crate) fn difficulty(job: &Value, requested: Option<Difficulty>) -> Result<Difficulty> {
	let mask: i64 = integer(
		job.get("difficulty").unwrap_or(&json!(0)),
		0,
		3,
		"difficulty",
	)?;
	let allows = |difficulty: Difficulty| -> bool { mask == 0 || mask & maa_bit(difficulty) != 0 };
	let chosen: Difficulty = requested.unwrap_or(if allows(Difficulty::Normal) {
		Difficulty::Normal
	} else {
		Difficulty::Challenge
	});
	ensure!(
		allows(chosen),
		"The job does not select the {} difficulty",
		chosen.name()
	);
	Ok(chosen)
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
/// Translate the job for its squad; `level` is what the level brings at the difficulty that
/// runs ([`level_units`]). MAA's wall-clock delays and timeouts become ticks at the speed the job
/// set: a battle starts at speed 1 and each SpeedUp toggles it.
pub(crate) fn translate(
	job: &Value,
	squad: &Value,
	identities: &Value,
	modules: &Value,
	level: &LevelUnits,
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
		3,
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
	let mut bind =
		|seen: &mut BTreeSet<String>, label: &str, id: String, op: &Value| -> Result<()> {
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
		bind(&mut seen, operator_name(op)?, id, op)?;
	}
	for group in job
		.get("groups")
		.unwrap_or(&empty)
		.as_array()
		.context("groups must be array")?
	{
		fields(group, &["name", "opers"], &["name", "opers"], "group")?;
		// As MAA and the generated squad choose: the first member in the group's order that the
		// squad holds at its requirements and no earlier operator or group took.
		let mut eligible: Option<(String, &Value)> = None;
		for op in group["opers"].as_array().context("Group opers not array")? {
			if let Some(id) = select(op)?.filter(|id| !seen.contains(id)) {
				eligible = Some((id, op));
				break;
			}
		}
		let (id, op) = eligible.context("Group has no eligible operator left in the squad")?;
		bind(
			&mut seen,
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
	// An operator the job deploys without declaring it comes from the squad, as MAA takes it
	// from the player's own squad, unless the level deals that card or it names a summon.
	let mut declared: BTreeSet<String> = ["opers", "groups"]
		.iter()
		.flat_map(|key| {
			job.get(*key)
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
		})
		.filter_map(|entry| entry["name"].as_str().map(str::to_owned))
		.collect();
	let mut undeclared: Vec<String> = Vec::new();
	for action in actions {
		let deploy: bool = matches!(
			action
				.get("type")
				.and_then(Value::as_str)
				.unwrap_or("Deploy"),
			"Deploy" | "部署"
		);
		let Some(name) = action["name"]
			.as_str()
			.filter(|name| deploy && !declared.contains(*name))
		else {
			continue;
		};
		if token_by_name(identities, &level.cards, name)?.is_some() {
			continue;
		}
		let op: Value = json!({ "name": name });
		if let Some(id) = select(&op)?.filter(|id| !seen.contains(id)) {
			bind(&mut seen, name, id, &op)?;
			declared.insert(name.to_owned());
			undeclared.push(name.to_owned());
		}
	}
	let mut commands: Vec<Value> = Vec::new();
	let mut speed: i64 = 1;
	// Post delays owed to the next step, in ticks.
	let mut owed: i64 = 0;
	// Who stands on each tile, for actions that name only a tile: the level's own units, then
	// the job's deploys and retreats so far.
	let mut positions: BTreeMap<(i64, i64), String> = level.placed.clone();
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
				"rear_delay",
				"timeout",
				"distance",
				"keep_kills",
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
			// Bullet time and camera moves serve MAA's screen recognition; the step keeps only
			// their conditions and delays.
			"BulletTime" | "子弹时间" => "bullet_time",
			"MoveCamera" | "移动镜头" => "camera",
			name => bail!("Unsupported MAA action {name}"),
		};
		let pre: i64 = delay(action, "pre_delay", speed)?;
		if kind == "output" {
			fields(
				action,
				&[
					"type",
					"doc",
					"doc_color",
					"pre_delay",
					"post_delay",
					"rear_delay",
					"kills",
					"costs",
					"cost_changes",
					"cooling",
				],
				&[],
				"Output action",
			)?;
			// MAA waits for an Output's conditions like any action's; one without conditions
			// only carries its delays.
			if !has_conditions(action) {
				owed +=
					pre + delay(action, "post_delay", speed)? + delay(action, "rear_delay", speed)?;
				continue;
			}
		}
		ensure!(
			kind == "skill_usage"
				|| (action.get("skill_usage").is_none() && action.get("skill_times").is_none()),
			"Skill policy on non-SkillUsage action"
		);
		ensure!(
			kind == "camera"
				|| (action.get("distance").is_none() && action.get("keep_kills").is_none()),
			"Camera fields on a non-MoveCamera action"
		);
		let mut cmd = json!({"kind":kind,"kills":integer(action.get("kills").unwrap_or(&json!(0)),0,99999,"kills")?,"costs":integer(action.get("costs").unwrap_or(&json!(0)),0,99999,"costs")?,"cost_changes":integer(action.get("cost_changes").unwrap_or(&json!(0)),-99999,99999,"cost_changes")?,"cooling":integer(action.get("cooling").unwrap_or(&json!(-1)),-1,99,"cooling")?});
		cmd["action_index"] = json!(index);
		cmd["wait_before"] = json!(std::mem::take(&mut owed));
		cmd["wait_after"] = json!(pre);
		if let Some(ticks) = deadline(action, speed)? {
			ensure!(kind != "skill_daemon", "SkillDaemon takes no timeout");
			cmd["deadline"] = json!(ticks);
		}
		if kind == "speed_up" {
			speed = 3 - speed;
		}
		owed = delay(action, "post_delay", speed)? + delay(action, "rear_delay", speed)?;
		if matches!(kind, "speed_up" | "skill_daemon" | "camera" | "output") {
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
			if kind == "camera" {
				ensure!(
					action["distance"].as_array().is_some_and(
						|distance| distance.len() == 2 && distance.iter().all(Value::is_number)
					),
					"MoveCamera needs a distance [x, y]"
				);
				ensure!(
					action.get("keep_kills").is_none_or(Value::is_boolean),
					"keep_kills must be a boolean"
				);
				cmd["kind"] = json!("wait");
			}
			if kind == "output" {
				cmd["kind"] = json!("wait");
			}
			commands.push(cmd);
			continue;
		}
		if kind == "bullet_time" {
			ensure!(
				action.get("direction").is_none(),
				"Direction on a BulletTime action"
			);
			ensure!(
				action.get("name").is_some() || action.get("location").is_some(),
				"BulletTime needs a name or a location"
			);
			cmd["kind"] = json!("wait");
			commands.push(cmd);
			continue;
		}
		let location: Option<(i64, i64)> = action.get("location").map(grid).transpose()?;
		let (label, target): (String, String) = match action.get("name") {
			Some(name) => {
				let name: &str = name.as_str().context("Action name invalid")?;
				let target: String = match bindings.get(name) {
					Some(id) => id.clone(),
					None if kind != "skill_usage" => token_by_name(identities, &level.cards, name)?
						.with_context(|| {
							format!("Action operator/group/token unresolved: {name}")
						})?,
					None => bail!("SkillUsage targets an undeclared operator {name}"),
				};
				(name.to_owned(), target)
			}
			// A Skill or Retreat may name only its tile, as MAA allows for the level's devices and
			// same-named summons: the unit there is the level's own or the job's latest deploy.
			None => {
				ensure!(
					matches!(kind, "skill" | "withdraw"),
					"Only Skill and Retreat may name just a location"
				);
				let (col, row): (i64, i64) =
					location.context("A Skill or Retreat needs a name or a location")?;
				let target: String = positions
					.get(&(col, row))
					.with_context(|| format!("No unit stands at [{col}, {row}] for this action"))?
					.clone();
				(format!("[{col}, {row}]"), target)
			}
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
			let (col, row) = location.context("Deploy location missing")?;
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
				// MAA chooses no direction for a device or summon; the original deploy still
				// takes one, and the original levels place their own devices facing up.
				"None" | "无" if !target.starts_with("char_") => "UP",
				"None" | "无" => bail!("Operator {label} needs a deploy direction"),
				_ => bail!("Unsupported direction"),
			});
			positions.insert((col, row), target);
		} else {
			ensure!(
				action.get("direction").is_none(),
				"Direction on a non-Deploy action"
			);
			// A located Skill/Retreat must find the named unit on that tile; the probe checks it.
			if let Some((col, row)) = location {
				ensure!(kind != "skill_usage", "SkillUsage takes no location");
				cmd["col"] = json!(col);
				cmd["row"] = json!(row);
				cmd["located"] = json!(1);
			}
			if kind == "withdraw" {
				match location {
					Some(tile) => {
						positions.remove(&tile);
					}
					None => positions.retain(|_, unit| *unit != target),
				}
			}
		}
		commands.push(cmd);
	}
	ensure!(!commands.is_empty(), "No executable MAA actions");
	let names: BTreeMap<String, String> = by_name
		.iter()
		.flat_map(|(name, ids)| ids.iter().map(move |id| (id.clone(), name.clone())))
		.filter(|(id, _)| roster.contains_key(id.as_str()))
		.collect();
	Ok(
		json!({"policies":policies,"commands":commands,"bindings":bindings,"names":names,"undeclared":undeclared}),
	)
}

/// The job as the original engine established it: actions up to the last completed command,
/// each operator's requirements set to the explicit training that ran, and the difficulty it
/// ran at. MAA can execute it.
pub(crate) fn concrete_job(
	job: &Value,
	plan: &Value,
	squad: &Value,
	modules: &Value,
	difficulty: Difficulty,
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
	concrete["difficulty"] = json!(maa_bit(difficulty));
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
pub(crate) const CONDITION_WAIT: i64 = 6;
pub(crate) const CONDITION_HOLD: i64 = 7;
pub(crate) const CONDITION_DEADLINE: i64 = 8;

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
	if let Some(ticks) = cmd.get("deadline").and_then(Value::as_i64) {
		list.push((CONDITION_DEADLINE, ticks));
	}
	// The previous action's post delay, then MAA's conditions, then this action's pre delay.
	if value("wait_before") > 0 {
		list.push((CONDITION_WAIT, value("wait_before")));
	}
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
	if value("wait_after") > 0 {
		list.push((CONDITION_WAIT, value("wait_after")));
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
		policies.len() <= 13 && (1..=128).contains(&commands.len()),
		"Probe capacity exceeded"
	);
	let identifier = Regex::new(r"^(?:char|token|trap)_[A-Za-z0-9_]{1,75}$")?;
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
			"wait" => 8,
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
		let plan = translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default(),
		)
		.unwrap();
		assert_eq!(compile(&plan, 0).unwrap(),"ARKP1\nS|2|3|char_fixture_a\nT|0|3|4|3|0|0|0|2|3|-5|4|2|char_fixture_a\nT|5|0|0|0|0|1|0|0|char_fixture_a\nT|4|0|0|0|0|0|0|0|_\n");
	}
	#[test]
	fn unsupported_executable_fields_never_disappear() {
		for (key, value) in [
			("pre_delay", json!(-1)),
			("post_delay", json!(true)),
			("timeout", json!(-2)),
			("distance", json!([0, 1])),
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
				translate(
					&job,
					&squad,
					&identities,
					&json!({}),
					&LevelUnits::default()
				)
				.is_err(),
				"{key}"
			);
		}
		for kind in ["ResetStopwatch", "Click", "Swipe", "SetUnitLocation"] {
			let (mut job, squad, identities) = fixture();
			job["actions"][0]["type"] = json!(kind);
			assert!(
				translate(
					&job,
					&squad,
					&identities,
					&json!({}),
					&LevelUnits::default()
				)
				.is_err(),
				"{kind}"
			);
		}
	}
	#[test]
	fn explicit_squad_training_and_group_ambiguity_are_enforced() {
		let (mut job, squad, identities) = fixture();
		job["opers"][0]["requirements"] = json!({"elite":1,"level":61});
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_err());
		job["opers"][0]["requirements"]["level"] = json!(60);
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_ok());
		job["opers"][0]["skill"] = json!(2);
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_err());
		let (mut job, mut squad, identities) = fixture();
		job["opers"] = json!([]);
		job["groups"] = json!([{"name":"甲","opers":[{"name":"甲"},{"name":"乙"}]}]);
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_ok());
		// Both members in the squad: the group takes the first in its order, and a second group
		// over the same members takes the one left.
		let mut second = squad["operators"][0].clone();
		second["character_id"] = json!("char_fixture_b");
		squad["operators"].as_array_mut().unwrap().push(second);
		job["groups"] = json!([
			{"name":"前","opers":[{"name":"甲"},{"name":"乙"}]},
			{"name":"后","opers":[{"name":"甲"},{"name":"乙"}]}
		]);
		job["actions"] = json!([
			{"type":"Deploy","name":"前","location":[4,3],"direction":"Left"},
			{"type":"Deploy","name":"后","location":[3,3],"direction":"Left"}
		]);
		let plan: Value = translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default(),
		)
		.unwrap();
		assert_eq!(plan["bindings"]["前"], "char_fixture_a");
		assert_eq!(plan["bindings"]["后"], "char_fixture_b");
		job["groups"]
			.as_array_mut()
			.unwrap()
			.push(json!({"name":"多","opers":[{"name":"甲"}]}));
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_err());
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
			&translate(
				&job,
				&squad,
				&identities,
				&json!({}),
				&LevelUnits::default(),
			)
			.unwrap(),
			0,
		)
		.unwrap();
		assert!(native.contains("T|0|1|2|0|0|0|0|0|token_fixture_s"));
		assert!(native.contains("T|2|3|4|0|0|0|1|0|char_fixture_a"));
		job["actions"] = json!([{"type":"Skill","location":[4,3]}]);
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_err());
		job["actions"] = json!([{"type":"Deploy","name":"未知","location":[1,1],"direction":"Up"}]);
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_err());
	}
	#[test]
	fn module_type_requirements_match_the_equipped_original_module() {
		let (mut job, mut squad, identities) = fixture();
		let modules = json!({"equipDict":{"uniequip_002_a":{"typeName2":"X"},"uniequip_003_a":{"typeName2":"Y"}}});
		job["opers"][0]["requirements"] = json!({"module":2});
		assert!(
			translate(&job, &squad, &identities, &modules, &LevelUnits::default()).is_err(),
			"no module equipped"
		);
		squad["operators"][0]["modules"] = json!([{"key":"uniequip_002_a","level":3}]);
		assert!(
			translate(&job, &squad, &identities, &modules, &LevelUnits::default()).is_err(),
			"X is not γ"
		);
		squad["operators"][0]["modules"] = json!([{"key":"uniequip_003_a","level":1}]);
		assert!(translate(&job, &squad, &identities, &modules, &LevelUnits::default()).is_ok());
		job["opers"][0]["requirements"] = json!({"module":0});
		assert!(
			translate(&job, &squad, &identities, &modules, &LevelUnits::default()).is_err(),
			"job forbids modules"
		);
	}
	#[test]
	fn difficulty_runs_normal_unless_the_job_is_challenge_only_or_asks_for_it() {
		let job = |mask: i64| json!({"difficulty": mask});
		assert_eq!(difficulty(&json!({}), None).unwrap(), Difficulty::Normal);
		assert_eq!(difficulty(&job(3), None).unwrap(), Difficulty::Normal);
		assert_eq!(difficulty(&job(2), None).unwrap(), Difficulty::Challenge);
		assert_eq!(
			difficulty(&job(3), Some(Difficulty::Challenge)).unwrap(),
			Difficulty::Challenge
		);
		assert_eq!(
			difficulty(&job(0), Some(Difficulty::Challenge)).unwrap(),
			Difficulty::Challenge
		);
		assert!(difficulty(&job(1), Some(Difficulty::Challenge)).is_err());
		assert!(difficulty(&job(2), Some(Difficulty::Normal)).is_err());
		assert!(difficulty(&job(4), None).is_err());
	}
	#[test]
	fn level_cards_resolve_devices_the_level_deals_before_summons() {
		let level = json!({
			"predefines": {"tokenCards": [{"inst": {"characterKey": "trap_fixture_mine"}}]},
			"hardPredefines": {"tokenCards": [{"inst": {"characterKey": "trap_fixture_wall"}}]}
		});
		let normal: LevelUnits = level_units(&level, Difficulty::Normal);
		assert_eq!(
			normal.cards,
			BTreeSet::from(["trap_fixture_mine".to_owned()])
		);
		let challenge: LevelUnits = level_units(&level, Difficulty::Challenge);
		assert!(
			challenge.cards.contains("trap_fixture_mine")
				&& challenge.cards.contains("trap_fixture_wall")
		);
		let (mut job, squad, mut identities) = fixture();
		// The same device name elsewhere in the table never competes with the level's own card.
		identities["trap_fixture_mine"] = json!({"name":"地雷"});
		identities["trap_fixture_other_mine"] = json!({"name":"地雷"});
		// A device takes no direction in MAA; the original deploy faces it up.
		job["actions"] =
			json!([{"type":"Deploy","name":"地雷","location":[2,1],"direction":"None"}]);
		let plan: Value = translate(&job, &squad, &identities, &json!({}), &normal).unwrap();
		assert!(compile(&plan, 0)
			.unwrap()
			.contains("T|0|1|2|0|0|0|0|0|trap_fixture_mine"));
		assert!(translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default()
		)
		.is_err());
		job["actions"] = json!([{"type":"Deploy","name":"甲","location":[2,1],"direction":"None"}]);
		assert!(translate(&job, &squad, &identities, &json!({}), &normal).is_err());
	}
	#[test]
	fn concrete_jobs_record_the_difficulty_that_ran() {
		let (job, squad, identities) = fixture();
		let plan: Value = translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default(),
		)
		.unwrap();
		let concrete: Value = concrete_job(
			&job,
			&plan,
			&squad,
			&json!({}),
			Difficulty::Challenge,
			3,
			"",
		)
		.unwrap();
		assert_eq!(concrete["difficulty"], 2);
	}
	#[test]
	fn delays_timeouts_aids_and_tiles_follow_maa_in_ticks() {
		let (mut job, squad, mut identities) = fixture();
		identities["trap_fixture_device"] = json!({"name":"装置"});
		job["actions"] = json!([
			{"type":"Deploy","name":"甲","location":[4,3],"direction":"Left","post_delay":1000},
			{"type":"SpeedUp","pre_delay":500,"post_delay":500},
			{"type":"BulletTime","name":"甲","costs":10},
			{"type":"Skill","location":[4,3],"timeout":0},
			{"type":"Output","doc":"note","post_delay":250},
			{"type":"Output","doc":"wait","kills":2},
			{"type":"MoveCamera","distance":[0,-0.7],"pre_delay":2000},
			{"type":"Skill","location":[2,1]},
			{"type":"Retreat","location":[4,3]}
		]);
		let level: LevelUnits = LevelUnits {
			placed: BTreeMap::from([((2, 1), "trap_fixture_device".to_owned())]),
			..LevelUnits::default()
		};
		let native: String = compile(
			&translate(&job, &squad, &identities, &json!({}), &level).unwrap(),
			0,
		)
		.unwrap();
		let steps: Vec<&str> = native
			.lines()
			.filter(|line| line.starts_with("T|"))
			.collect();
		// A battle starts at speed 1: 1000 ms is 30 ticks; after SpeedUp, 500 ms is 30 ticks.
		assert_eq!(steps[0], "T|0|3|4|3|0|0|0|0|char_fixture_a");
		assert_eq!(steps[1], "T|3|0|0|0|0|0|0|2|6|30|6|15|_");
		assert_eq!(steps[2], "T|8|0|0|0|0|0|0|2|6|30|2|10|_");
		assert_eq!(steps[3], "T|2|3|4|0|0|0|1|1|8|0|char_fixture_a");
		// An Output with a condition waits for it, as MAA does.
		assert_eq!(steps[4], "T|8|0|0|0|0|0|0|2|6|15|1|2|_");
		assert_eq!(steps[5], "T|8|0|0|0|0|0|0|1|6|120|_");
		assert_eq!(steps[6], "T|2|1|2|0|0|0|1|0|trap_fixture_device");
		assert_eq!(steps[7], "T|1|3|4|0|0|0|1|0|char_fixture_a");
		// A tile with no unit, before any deploy or after its retreat, names nobody.
		job["actions"] = json!([{"type":"Retreat","location":[4,3]}]);
		assert!(translate(&job, &squad, &identities, &json!({}), &level).is_err());
		job["actions"] = json!([{"type":"MoveCamera"}]);
		assert!(translate(&job, &squad, &identities, &json!({}), &level).is_err());
		job["actions"] = json!([{"type":"BulletTime","costs":3}]);
		assert!(translate(&job, &squad, &identities, &json!({}), &level).is_err());
	}
	#[test]
	fn undeclared_operators_bind_and_placed_units_take_deploy_rows() {
		let (mut job, mut squad, identities) = fixture();
		job["opers"] = json!([]);
		job["actions"] = json!([{"type":"Deploy","name":"乙","location":[1,1],"direction":"Up"}]);
		let mut second = squad["operators"][0].clone();
		second["character_id"] = json!("char_fixture_b");
		squad["operators"] = json!([second]);
		let plan: Value = translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&LevelUnits::default(),
		)
		.unwrap();
		assert_eq!(plan["bindings"]["乙"], "char_fixture_b");
		assert_eq!(plan["undeclared"], json!(["乙"]));
		// An undeclared name is the level's card, hidden until the battle reveals it, or a summon
		// before it is a squad operator. A declared operator stays the squad's, which a paradox
		// simulation's original conversion puts in place of the level's card.
		let mut identities = identities;
		identities["token_fixture_summon"] = json!({"name":"召唤"});
		identities["char_fixture_summon"] = json!({"name":"召唤"});
		job["opers"] = json!([{"name":"乙","skill":1}]);
		job["actions"] = json!([{"type":"Deploy","name":"甲","location":[1,1],"direction":"Up"},
			{"type":"Deploy","name":"乙","location":[2,1],"direction":"Up"},
			{"type":"Deploy","name":"召唤","location":[3,1],"direction":"Up"}]);
		let dealt: Value = json!({"predefines":{"characterCards":[
			{"hidden":true,"inst":{"characterKey":"char_fixture_a"}},
			{"hidden":false,"inst":{"characterKey":"char_fixture_b"}}]}});
		let plan: Value = translate(
			&job,
			&squad,
			&identities,
			&json!({}),
			&level_units(&dealt, Difficulty::Normal),
		)
		.unwrap();
		assert_eq!(plan["bindings"]["乙"], "char_fixture_b");
		assert_eq!(plan["undeclared"], json!([]));
		let native: String = compile(&plan, 0).unwrap();
		assert!(native.contains("|char_fixture_a\n"), "{native}");
		assert!(native.contains("|token_fixture_summon\n"), "{native}");
		// A three-row level placing a device on its position row 0: deploys count that row 2.
		let level: Value = json!({"mapData":{"map":[[0,0],[0,0],[0,0]]},
			"predefines":{"tokenInsts":[{"position":{"row":0,"col":1},
				"inst":{"characterKey":"trap_fixture_device"}}]}});
		assert_eq!(
			level_units(&level, Difficulty::Normal).placed,
			BTreeMap::from([((1, 2), "trap_fixture_device".to_owned())])
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
