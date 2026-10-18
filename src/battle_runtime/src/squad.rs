use crate::maa::{fields, integer};
use anyhow::{bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) fn compile(value: &Value) -> Result<String> {
	fields(
		value,
		&["schema", "operators"],
		&["schema", "operators"],
		"squad",
	)?;
	ensure!(
		value["schema"] == "ark-battle-squad-v2",
		"Wrong squad schema"
	);
	let operators = value["operators"]
		.as_array()
		.context("Squad operators must be an array")?;
	// Twelve squad members and a support operator, as the client deploys them; none when the
	// level deals its own operators.
	ensure!(operators.len() <= 13, "Expected at most 13 operators");
	let identifier = Regex::new(r"^char_[A-Za-z0-9_]{1,75}$")?;
	let optional = Regex::new(r"^[A-Za-z0-9_#-]{1,100}$")?;
	let module_id = Regex::new(r"^[A-Za-z0-9_]{1,100}$")?;
	let mut seen: BTreeSet<&str> = BTreeSet::new();
	let mut result = String::new();
	for (index, op) in operators.iter().enumerate() {
		fields(
			op,
			&[
				"character_id",
				"level",
				"phase",
				"potential_rank",
				"favor_battle_phase",
				"skill_index",
				"skill_level",
				"skin_id",
				"template_id",
				"modules",
			],
			&[
				"character_id",
				"level",
				"phase",
				"potential_rank",
				"favor_battle_phase",
				"skill_index",
				"skill_level",
			],
			"operator metadata",
		)?;
		let id = op["character_id"]
			.as_str()
			.context("Character ID missing")?;
		ensure!(
			identifier.is_match(id) && seen.insert(id),
			"Invalid or duplicate character identifier"
		);
		for (key, low, high) in [
			("level", 1, 1000),
			("phase", 0, 2),
			("potential_rank", 0, 5),
			("favor_battle_phase", 0, 100),
			("skill_index", -1, 2),
			("skill_level", 1, 10),
		] {
			integer(&op[key], low, high, key)?;
		}
		for key in ["skin_id", "template_id"] {
			ensure!(
				op[key].is_null() || op[key].as_str().is_some_and(|s| optional.is_match(s)),
				"Invalid {key}"
			);
		}
		let modules = op.get("modules").cloned().unwrap_or_else(|| json!([]));
		let list = modules.as_array().context("Modules must be an array")?;
		ensure!(list.len() <= 8, "Too many modules");
		for module in list {
			fields(module, &["key", "level"], &["key", "level"], "module")?;
			ensure!(
				module["key"]
					.as_str()
					.is_some_and(|s| module_id.is_match(s)),
				"Invalid module identifier"
			);
			integer(&module["level"], 1, 3, "module level")?;
		}
		let slot = json!({"inst":{"characterKey":id,"level":op["level"],"phase":op["phase"],"potentialRank":op["potential_rank"],"favorBattlePhase":op["favor_battle_phase"],"playerInstId":index+1},"skillIndex":op["skill_index"],"mainSkillLvl":op["skill_level"],"skinId":op["skin_id"],"tmplId":op["template_id"],"uniEquipIds":modules,"showSpIllust":false,"masterInfos":[]});
		result.push_str(&serde_json::to_string(&slot)?);
		result.push('\n');
	}
	Ok(result)
}

/// Which squad operators the original deck holds, each at its requested training, and which
/// cards the level dealt in their place, or `None` when the deck contradicts the squad. A level
/// that deals its own operators, such as a story stage, can leave the squad unused; a paradox
/// simulation puts the squad's operator in place of the level's card of it.
pub(crate) fn verify(value: &Value, converted: &Value) -> Option<Value> {
	let actual: &Vec<Value> = converted["characters"].as_array()?;
	let operators: &Vec<Value> = value["operators"].as_array()?;
	let ids: BTreeSet<&str> = actual.iter().filter_map(|e| e["id"].as_str()).collect();
	if ids.len() != actual.len() {
		return None;
	}
	let requested: BTreeSet<&str> = operators
		.iter()
		.filter_map(|op| op["character_id"].as_str())
		.collect();
	let mut level: Vec<&str> = Vec::new();
	for entry in actual {
		let id: &str = entry["id"].as_str()?;
		if !requested.contains(id) {
			if entry["isPredefined"] != true {
				return None;
			}
			level.push(id);
		}
	}
	let mut used: Vec<&str> = Vec::new();
	let mut unused: Vec<&str> = Vec::new();
	for op in operators {
		let Some(entry) = actual.iter().find(|e| e["id"] == op["character_id"]) else {
			unused.push(op["character_id"].as_str()?);
			continue;
		};
		used.push(op["character_id"].as_str()?);
		for (requested, original) in [
			("level", "level"),
			("potential_rank", "potentialRank"),
			("skill_index", "mainSkillIndex"),
			("favor_battle_phase", "favorBattlePhase"),
		] {
			if entry[original] != op[requested] {
				return None;
			}
		}
		if entry["evolvePhase"] != op["phase"]
			&& entry["evolvePhase"] != format!("PHASE_{}", op["phase"])
		{
			return None;
		}
		// Operators without skills (skill_index -1) have no main skill level to compare.
		if op["skill_index"] != -1 && entry["mainSkill"]["level"] != op["skill_level"] {
			return None;
		}
		fn modules(v: &Value) -> Option<Vec<(String, i64)>> {
			let Some(array) = v.as_array() else {
				return if v.is_null() { Some(vec![]) } else { None };
			};
			let mut entries: Vec<(String, i64)> = Vec::new();
			for v in array {
				entries.push((v["key"].as_str()?.to_owned(), v["level"].as_i64()?));
			}
			entries.sort();
			Some(entries)
		}
		if modules(&entry["uniEquipQueries"]) != modules(&op["modules"]) {
			return None;
		}
		if !op["skin_id"].is_null() && entry["skinData"]["skinId"] != op["skin_id"] {
			return None;
		}
		if !op["template_id"].is_null() && entry["tmplId"] != op["template_id"] {
			return None;
		}
	}
	Some(json!({"used":used,"unused":unused,"level_cards":level}))
}

/// Training applied to every generated operator. Each axis lowers independently; a job's own
/// requirements remain floors, so a generated squad never falls below what the job asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Training {
	/// Level at the highest elite phase; `None` for that phase's original maximum.
	pub level: Option<i64>,
	/// Specialization rank 0..=3 (skill levels 8..=10); needs elite phase 2.
	pub mastery: i64,
	/// Module level 0..=3; 0 equips none.
	pub module: i64,
	/// Trust percent from the original favor frames; `None` for the original maximum.
	pub trust: Option<i64>,
}

impl Default for Training {
	fn default() -> Self {
		Self {
			level: None,
			mastery: 3,
			module: 3,
			trust: None,
		}
	}
}

impl Training {
	pub fn validate(&self) -> Result<()> {
		ensure!(
			self.level.is_none_or(|level| (1..=90).contains(&level)),
			"level must be 1..90 or max"
		);
		ensure!((0..=3).contains(&self.mastery), "mastery must be 0..3");
		ensure!((0..=3).contains(&self.module), "module must be 0..3");
		ensure!(
			self.trust.is_none_or(|trust| (0..=200).contains(&trust)),
			"trust must be 0..200 percent or max"
		);
		Ok(())
	}
}

/// Training for a whole squad: one team default, optionally overridden per original identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SquadTraining {
	pub team: Training,
	pub operators: std::collections::BTreeMap<String, Training>,
}

impl SquadTraining {
	pub fn team(team: Training) -> Self {
		Self {
			team,
			operators: Default::default(),
		}
	}

	pub(crate) fn of(&self, id: &str) -> &Training {
		self.operators.get(id).unwrap_or(&self.team)
	}
}

/// Original tables a generated squad reads: operators, modules and trust frames.
pub(crate) struct Tables<'a> {
	pub characters: &'a Value,
	pub modules: &'a Value,
	pub favor: &'a Value,
}

/// The squad a job names at a training. An undeclared name of a card the level deals, from
/// `level_cards`, or of a summon stays out of it.
pub(crate) fn for_job(
	job: &Value,
	tables: &Tables<'_>,
	training: &SquadTraining,
	level_cards: &BTreeSet<String>,
) -> Result<Value> {
	training.team.validate()?;
	for operator in training.operators.values() {
		operator.validate()?;
	}
	let mut chosen: BTreeSet<String> = BTreeSet::new();
	let mut operators: Vec<Value> = Vec::new();
	let by_name = crate::maa::operators_by_name(tables.characters)?;
	let empty: Vec<Value> = Vec::new();
	let direct = job
		.get("opers")
		.map(|x| x.as_array().context("opers must be array"))
		.transpose()?
		.unwrap_or(&empty);
	let groups = job
		.get("groups")
		.map(|x| x.as_array().context("groups must be array"))
		.transpose()?
		.unwrap_or(&empty);
	for op in direct {
		let name = crate::maa::operator_name(op)?;
		let candidates = by_name.get(name).context("Unknown original operator")?;
		ensure!(candidates.len() == 1, "Ambiguous original operator {name}");
		let id = &candidates[0];
		ensure!(chosen.insert(id.clone()), "Duplicate operator {name}");
		operators.push(auto_entry(id, tables, op, training.of(id))?);
	}
	for group in groups {
		fields(
			group,
			&["name", "opers"],
			&["name", "opers"],
			"operator group",
		)?;
		let mut found: Option<Value> = None;
		for op in group["opers"]
			.as_array()
			.context("group opers must be array")?
		{
			let name = crate::maa::operator_name(op)?;
			if let Some(ids) = by_name.get(name) {
				ensure!(ids.len() == 1, "Ambiguous original operator {name}");
				let id = &ids[0];
				if chosen.insert(id.clone()) {
					found = Some(auto_entry(id, tables, op, training.of(id))?);
					break;
				}
			}
		}
		operators.push(found.context("No original operator available for group")?);
	}
	// MAA deploys an operator the job never declares from the player's own squad; such an
	// operator joins the squad at MAA's default skill, unless it is a card or summon no squad
	// brings.
	let declared: BTreeSet<&str> = direct
		.iter()
		.chain(groups)
		.filter_map(|entry| entry["name"].as_str())
		.collect();
	for action in job["actions"].as_array().into_iter().flatten() {
		let deploy: bool = matches!(
			action
				.get("type")
				.and_then(Value::as_str)
				.unwrap_or("Deploy"),
			"Deploy" | "部署"
		);
		let Some(name) = action["name"]
			.as_str()
			.filter(|name| deploy && !declared.contains(name))
		else {
			continue;
		};
		if crate::maa::token_by_name(tables.characters, level_cards, name)?.is_some() {
			continue;
		}
		if let Some([id]) = by_name.get(name).map(Vec::as_slice) {
			if chosen.insert(id.clone()) {
				let op: Value = json!({ "name": name });
				operators.push(auto_entry(id, tables, &op, training.of(id))?);
			}
		}
	}
	let result = json!({"schema":"ark-battle-squad-v2","operators":operators});
	compile(&result)?;
	Ok(result)
}

/// Trust as the original favor frame gives it: the favor point, which module unlocks compare,
/// and the battle phase (0..=100, 50 at 100% trust), which battle stats interpolate.
fn favor(favor: &Value, trust: Option<i64>) -> Result<(i64, i64)> {
	let frames: &Vec<Value> = favor["favorFrames"]
		.as_array()
		.context("Original favor frames missing")?;
	let frame: &Value = match trust {
		Some(percent) => frames
			.iter()
			.find(|frame| {
				frame["data"]["percent"]
					.as_i64()
					.is_some_and(|p| p >= percent)
			})
			.or(frames.last()),
		None => frames.last(),
	}
	.context("Original favor frames empty")?;
	let point: i64 = match trust {
		Some(_) => frame["data"]["favorPoint"].as_i64(),
		None => favor["maxFavor"].as_i64(),
	}
	.context("Original favor point missing")?;
	let phase: i64 = frame["data"]["battlePhase"]
		.as_i64()
		.context("Original favor frame has no battlePhase")?;
	Ok((point, phase))
}

/// MAA module numbers 1..=5 name the original module types χ, γ, α, Δ, β.
pub(crate) fn module_type_letter(number: i64) -> Option<&'static str> {
	["X", "Y", "A", "D", "B"]
		.get(usize::try_from(number - 1).ok()?)
		.copied()
}

/// The equipped module: the job's required type, otherwise the first advanced module.
fn module_entry(
	id: &str,
	modules: &Value,
	wanted_type: i64,
	phase: i64,
	level: i64,
	favor: i64,
	target: i64,
) -> Result<Option<Value>> {
	let mut advanced: Vec<&Value> = modules["charEquip"][id]
		.as_array()
		.into_iter()
		.flatten()
		.filter_map(|key| key.as_str().map(|key| &modules["equipDict"][key]))
		.filter(|equip| equip["type"] == "ADVANCED")
		.collect();
	advanced.sort_by_key(|equip| equip["charEquipOrder"].as_i64().unwrap_or(i64::MAX));
	let equip: Option<&Value> = match wanted_type {
		number if number > 0 => {
			let letter = module_type_letter(number).context("Unknown MAA module type")?;
			Some(
				advanced
					.into_iter()
					.find(|equip| equip["typeName2"] == letter)
					.with_context(|| format!("{id} has no {letter} module required by the job"))?,
			)
		}
		0 => None,
		_ => advanced.into_iter().next(),
	};
	let Some(equip) = equip else {
		return Ok(None);
	};
	let unlocked: bool = phase >= equip["unlockEvolvePhase"].as_i64().unwrap_or(2)
		&& level >= equip["unlockLevel"].as_i64().unwrap_or(0);
	// The highest module level the training and the original unlock trust allow.
	let module_level: Option<i64> = (1..=target).rev().find(|candidate| {
		equip["unlockFavors"][candidate.to_string()]
			.as_i64()
			.is_some_and(|needed| favor >= needed)
	});
	match (unlocked, module_level) {
		(true, Some(module_level)) => Ok(Some(
			json!({"key": equip["uniEquipId"], "level": module_level}),
		)),
		_ if wanted_type > 0 => {
			bail!("{id} cannot unlock the job's required module at this training")
		}
		_ => Ok(None),
	}
}

fn auto_entry(
	id: &str,
	tables: &Tables<'_>,
	operator: &Value,
	training: &Training,
) -> Result<Value> {
	let wanted = crate::maa::requirements(operator)?;
	let record: &Value = &tables.characters[id];
	let phases = record["phases"]
		.as_array()
		.context("Original operator phases absent")?;
	ensure!(!phases.is_empty(), "Original operator has no phase");
	let phase: i64 = phases.len() as i64 - 1;
	let maximum_level: i64 = phases[phases.len() - 1]["maxLevel"]
		.as_i64()
		.context("Original phase has no maxLevel")?;
	let level: i64 = training
		.level
		.unwrap_or(maximum_level)
		.clamp(*wanted.get("level").unwrap_or(&1), maximum_level);
	let skills = record["skills"].as_array().cloned().unwrap_or_default();
	let index = (wanted["skill"] - 1).max(0) as usize;
	// The original converter rejects any skill index for an operator without skills.
	let skill_index: i64 = if skills.is_empty() { -1 } else { index as i64 };
	ensure!(
		skills.is_empty() || index < skills.len(),
		"Original operator has no requested skill"
	);
	let masteries = if phase >= 2 {
		skills
			.get(index)
			.and_then(|s| s["levelUpCostCond"].as_array())
			.map_or(0, Vec::len) as i64
	} else {
		0
	};
	let skill_level: i64 = if skills.is_empty() {
		1
	} else {
		(7 + training.mastery)
			.max(*wanted.get("skill_level").unwrap_or(&0))
			.min(7 + masteries)
	};
	ensure!(
		*wanted.get("skill_level").unwrap_or(&0) <= skill_level,
		"Original operator cannot reach required skill level"
	);
	let (favor, battle_phase): (i64, i64) = favor(tables.favor, training.trust)?;
	let wanted_type: i64 = *wanted.get("module").unwrap_or(&-1);
	let module: Option<Value> = if training.module == 0 {
		ensure!(wanted_type <= 0, "{id}: the job requires a module");
		None
	} else {
		module_entry(
			id,
			tables.modules,
			wanted_type,
			phase,
			level,
			favor,
			training.module,
		)?
	};
	let mut entry = json!({"character_id":id,"level":level,"phase":phase,"potential_rank":record.get("maxPotentialLevel").cloned().unwrap_or(json!(0)),"favor_battle_phase":battle_phase,"skill_index":skill_index,"skill_level":skill_level});
	if let Some(module) = module {
		entry["modules"] = json!([module]);
	}
	Ok(entry)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn rejects_boolean_training_and_duplicate_ids() {
		let mut value: Value =
			serde_json::from_str(include_str!("../examples/squad-main-01-01.json")).unwrap();
		assert!(compile(&value).is_ok());
		value["operators"][0]["level"] = json!(true);
		assert!(compile(&value).is_err());
	}

	fn tables() -> (Value, Value, Value) {
		let skill = json!({"levelUpCostCond":[{},{},{}]});
		(
			json!({
				"char_fixture_a":{"name":"甲","maxPotentialLevel":5,
					"phases":[{"maxLevel":50},{"maxLevel":70},{"maxLevel":90}],"skills":[skill,skill]},
				"char_fixture_c":{"name":"丙","maxPotentialLevel":5,"phases":[{"maxLevel":40}],"skills":[]}
			}),
			json!({"charEquip":{"char_fixture_a":["uniequip_001_a","uniequip_002_a","uniequip_003_a"]},
				"equipDict":{
					"uniequip_001_a":{"uniEquipId":"uniequip_001_a","type":"INITIAL"},
					"uniequip_002_a":{"uniEquipId":"uniequip_002_a","type":"ADVANCED","typeName2":"X",
						"charEquipOrder":1,"unlockEvolvePhase":2,"unlockLevel":60,
						"unlockFavors":{"1":0,"2":2732,"3":10070}},
					"uniequip_003_a":{"uniEquipId":"uniequip_003_a","type":"ADVANCED","typeName2":"Y",
						"charEquipOrder":2,"unlockEvolvePhase":2,"unlockLevel":60,
						"unlockFavors":{"1":0,"2":2732,"3":10070}}}}),
			json!({"maxFavor":25570,"favorFrames":[
				{"level":0,"data":{"favorPoint":0,"percent":0,"battlePhase":0}},
				{"level":25,"data":{"favorPoint":2400,"percent":50,"battlePhase":25}},
				{"level":50,"data":{"favorPoint":9600,"percent":100,"battlePhase":50}},
				{"level":100,"data":{"favorPoint":25570,"percent":200,"battlePhase":100}}]}),
		)
	}

	fn generate(job: &Value, training: &Training) -> Result<Value> {
		generate_with(job, training, &BTreeSet::new())
	}

	fn generate_with(
		job: &Value,
		training: &Training,
		level_cards: &BTreeSet<String>,
	) -> Result<Value> {
		let (characters, modules, favor) = tables();
		for_job(
			job,
			&Tables {
				characters: &characters,
				modules: &modules,
				favor: &favor,
			},
			&SquadTraining::team(*training),
			level_cards,
		)
	}

	#[test]
	fn the_original_deck_shows_which_squad_operators_the_level_kept() {
		let squad = json!({"operators":[
			{"character_id":"char_fixture_a","level":90,"phase":2,"potential_rank":5,
				"favor_battle_phase":100,"skill_index":0,"skill_level":10},
			{"character_id":"char_fixture_b","level":90,"phase":2,"potential_rank":5,
				"favor_battle_phase":100,"skill_index":0,"skill_level":10}]});
		let card = |id: &str, level: i64, predefined: bool| -> Value {
			json!({"id":id,"level":level,"evolvePhase":"PHASE_2","potentialRank":5,
				"mainSkillIndex":0,"favorBattlePhase":100,"mainSkill":{"level":10},
				"isPredefined":predefined})
		};
		// A paradox simulation: the squad's operator in place of the level's card, a level card
		// beside it, and a squad operator the level left out.
		let deck = json!({"characters":[card("char_fixture_a", 90, false),
			card("char_fixture_level", 50, true)]});
		assert_eq!(
			verify(&squad, &deck),
			Some(
				json!({"used":["char_fixture_a"],"unused":["char_fixture_b"],
				"level_cards":["char_fixture_level"]})
			)
		);
		// A squad card at other training, or an extra card the level does not deal, contradicts.
		let deck = json!({"characters":[card("char_fixture_a", 80, false)]});
		assert_eq!(verify(&squad, &deck), None);
		let deck = json!({"characters":[card("char_fixture_other", 90, false)]});
		assert_eq!(verify(&squad, &deck), None);
	}

	#[test]
	fn undeclared_deploys_join_unless_the_level_deals_them() {
		let job = json!({"opers":[{"name":"甲"}],"actions":[
			{"type":"Deploy","name":"甲"},{"type":"Deploy","name":"丙"}]});
		// A declared operator always joins, as the paradox simulations' own operators do.
		let ids = |squad: &Value| -> Vec<String> {
			squad["operators"]
				.as_array()
				.unwrap()
				.iter()
				.map(|op| op["character_id"].as_str().unwrap().to_owned())
				.collect()
		};
		assert_eq!(
			ids(&generate(&job, &Training::default()).unwrap()),
			["char_fixture_a", "char_fixture_c"]
		);
		let dealt: BTreeSet<String> =
			BTreeSet::from(["char_fixture_a".to_owned(), "char_fixture_c".to_owned()]);
		assert_eq!(
			ids(&generate_with(&job, &Training::default(), &dealt).unwrap()),
			["char_fixture_a"]
		);
	}

	#[test]
	fn maximum_training_uses_original_caps_and_the_first_module() {
		let job = json!({"opers":[{"name":"甲","skill":2},{"name":"丙"}]});
		let squad = generate(&job, &Training::default()).unwrap();
		let first = &squad["operators"][0];
		assert_eq!(
			(first["phase"].clone(), first["level"].clone()),
			(json!(2), json!(90))
		);
		assert_eq!(
			(first["skill_index"].clone(), first["skill_level"].clone()),
			(json!(1), json!(10))
		);
		assert_eq!(first["favor_battle_phase"], 100);
		assert_eq!(
			first["modules"],
			json!([{"key":"uniequip_002_a","level":3}])
		);
		let skillless = &squad["operators"][1];
		assert_eq!(
			(skillless["skill_index"].clone(), skillless["level"].clone()),
			(json!(-1), json!(40))
		);
		assert!(skillless.get("modules").is_none());
	}

	#[test]
	fn each_axis_lowers_independently_and_job_requirements_stay_floors() {
		let job = json!({"opers":[{"name":"甲","skill":1,"requirements":{"level":60,"module":2}}]});
		let low = Training {
			level: Some(30),
			mastery: 1,
			module: 2,
			trust: Some(50),
		};
		let entry = generate(&job, &low).unwrap()["operators"][0].clone();
		assert_eq!(entry["level"], 60, "the job's level stays a floor");
		assert_eq!(entry["skill_level"], 8);
		assert_eq!(entry["favor_battle_phase"], 25);
		// Module level 2 needs 2732 trust points; 50% trust only unlocks level 1 of the Y module.
		assert_eq!(
			entry["modules"],
			json!([{"key":"uniequip_003_a","level":1}])
		);
		let none = Training {
			module: 0,
			..Training::default()
		};
		assert!(generate(&job, &none).is_err(), "the job requires a module");
	}
}
