use std::{fs, path::PathBuf};

use ark_solver::config::Config;
use ark_solver::data::{
	ArkDataError, ArkDataSnapshot, BasePanelBuild, OperatorBuild, OperatorIdentity, OperatorModule,
	OperatorPosition, PotentialRank, ResolvedOperatorBuild, SkillActivationType, SkillDurationType,
	SkillSpType,
};
use common::{Fixture, REVISION};
use serde_json::{json, Value};

mod common;

fn base_stats(max_hp: u32, attack: u32, defense: u32, cost: u32) -> Value {
	json!({
		"maxHp": max_hp,
		"atk": attack,
		"def": defense,
		"magicResistance": 15.0,
		"cost": cost,
		"blockCnt": 1,
		"attackSpeed": 100.0,
		"baseAttackTime": 1.25,
		"respawnTime": 70
	})
}

fn stat_delta(max_hp: u32, attack: u32, defense: u32) -> Value {
	json!({
		"maxHp": max_hp,
		"atk": attack,
		"def": defense,
		"magicResistance": 0.0,
		"cost": 0,
		"blockCnt": 0,
		"attackSpeed": 0.0,
		"baseAttackTime": 0.0,
		"respawnTime": 0
	})
}

fn skill_level(level: u8) -> Value {
	json!({
		"name": "Synthetic Skill",
		"description": format!("level {level}"),
		"skillType": "AUTO",
		"durationType": "NONE",
		"duration": 12.5,
		"prefabId": "skill_test_prefab",
		"rangeId": "skill-range",
		"blackboard": [
			{"key": "atk_scale", "value": f64::from(level) / 10.0, "valueStr": null},
			{"key": "token_key", "value": 0.0, "valueStr": "token_test"}
		],
		"spData": {
			"spType": "INCREASE_WITH_TIME",
			"maxChargeTime": 1,
			"spCost": 40 - u32::from(level),
			"initSp": u32::from(level),
			"increment": 1.0
		}
	})
}

fn character(name: &str, position: &str) -> Value {
	json!({
		"name": name,
		"position": position,
		"profession": "CASTER",
		"subProfessionId": "corecaster",
		"phases": [
			{
				"rangeId": "test-range",
				"maxLevel": 30,
				"attributesKeyFrames": [
					{"level": 1, "data": base_stats(100, 20, 10, 8)},
					{"level": 30, "data": base_stats(200, 40, 20, 8)}
				]
			},
			{
				"rangeId": "test-range",
				"maxLevel": 50,
				"attributesKeyFrames": [
					{"level": 1, "data": base_stats(250, 50, 25, 10)},
					{"level": 50, "data": base_stats(500, 100, 50, 10)}
				]
			},
			{
				"rangeId": "test-range",
				"maxLevel": 90,
				"attributesKeyFrames": [
					{"level": 1, "data": base_stats(500, 100, 50, 12)},
					{"level": 90, "data": base_stats(700, 140, 70, 12)}
				]
			}
		],
		"skills": [
			{
				"skillId": "skill_test_1",
				"unlockCond": {"phase": "PHASE_0", "level": 1},
				"levelUpCostCond": []
			},
			{
				"skillId": "skill_test_2",
				"overridePrefabKey": "skill_test_override_prefab",
				"overrideTokenKey": "token_10058_sbell2_icetgt",
				"unlockCond": {"phase": "PHASE_1", "level": 1},
				"levelUpCostCond": [
					{"unlockCond": {"phase": "PHASE_1", "level": 30}},
					{"unlockCond": {"phase": "PHASE_1", "level": 40}},
					{"unlockCond": {"phase": "PHASE_1", "level": 50}}
				]
			}
		],
		"allSkillLvlup": [
			{"unlockCond": {"phase": "PHASE_0", "level": 1}},
			{"unlockCond": {"phase": "PHASE_0", "level": 1}},
			{"unlockCond": {"phase": "PHASE_0", "level": 1}},
			{"unlockCond": {"phase": "PHASE_1", "level": 1}},
			{"unlockCond": {"phase": "PHASE_1", "level": 1}},
			{"unlockCond": {"phase": "PHASE_1", "level": 1}}
		],
		"potentialRanks": [
			{
				"type": "BUFF",
				"description": "cost",
				"buff": {"attributes": {"attributeModifiers": [{
					"attributeType": "COST",
					"formulaItem": "ADDITION",
					"value": -1.0,
					"loadFromBlackboard": false,
					"fetchBaseValueFromSourceEntity": false
				}]}}
			},
			{"type": "CUSTOM", "description": "talent alpha"},
			{
				"type": "BUFF",
				"description": "attack",
				"buff": {"attributes": {"attributeModifiers": [{
					"attributeType": "ATK",
					"formulaItem": "ADDITION",
					"value": 7.0,
					"loadFromBlackboard": false,
					"fetchBaseValueFromSourceEntity": false
				}]}}
			},
			{"type": "CUSTOM", "description": "talent beta"},
			{
				"type": "BUFF",
				"description": "defense",
				"buff": {"attributes": {"attributeModifiers": [{
					"attributeType": "DEF",
					"formulaItem": "ADDITION",
					"value": 3.0,
					"loadFromBlackboard": false,
					"fetchBaseValueFromSourceEntity": false
				}]}}
			}
		],
		"favorKeyFrames": [
			{"level": 0, "data": stat_delta(0, 0, 0)},
			{"level": 50, "data": stat_delta(20, 20, 10)}
		],
		"trait": {"candidates": [{
			"unlockCondition": {"phase": "PHASE_0", "level": 1},
			"requiredPotentialRank": 0,
			"blackboard": [{"key": "dynamic_def_scale", "value": 2.0, "valueStr": null}],
			"overrideDescripton": "dynamic trait",
			"prefabKey": "trait_prefab",
			"rangeId": null
		}]},
		"talents": [{"candidates": [{
			"unlockCondition": {"phase": "PHASE_2", "level": 1},
			"requiredPotentialRank": 0,
			"prefabKey": "talent_prefab",
			"name": "Dynamic Talent",
			"description": "replay only",
			"rangeId": "skill-range",
			"blackboard": [{"key": "dynamic_atk_scale", "value": 1.5, "valueStr": null}],
			"tokenKey": "token_test",
			"isHideTalent": false
		}]}]
	})
}

fn classified_character(name: &str, profession: &str, group_id: Option<&str>) -> Value {
	let mut value: Value = character(name, "MELEE");
	value["profession"] = json!(profession);
	if let Some(group_id) = group_id {
		value["groupId"] = json!(group_id);
	}
	value
}

fn operator_fixture() -> Fixture {
	let mut fixture: Fixture = Fixture::valid();
	fixture.set_verified_entry_json(
		"character_table.json",
		&json!({
			"char_test_unique": character("Unique Operator", "MELEE"),
			"char_test_duplicate_a": character("Duplicate", "MELEE"),
			"char_test_duplicate_b": character("Duplicate", "MELEE"),
			"char_test_ranged": character("Ranged Operator", "RANGED"),
			"char_test_all": character("All Operator", "ALL"),
			"char_test_none": character("None Operator", "NONE"),
			"char_test_future": character("Future Operator", "FUTURE_POSITION"),
			"token_test_summon": classified_character("Test Summon", "TOKEN", None),
			"trap_test_crate": classified_character("Test Crate", "TRAP", None)
		}),
	);
	fixture.set_verified_entry_json(
		"char_patch_table.json",
		&json!({
			"patchChars": {
				"char_test_patch": character("Patch Operator", "RANGED")
			},
			"infos": {
				"char_test_unique": {
					"tmplIds": ["char_test_unique", "char_test_patch"],
					"default": "char_test_unique"
				}
			}
		}),
	);
	fixture.set_verified_entry_json(
		"skill_table.json",
		&json!({
			"skill_test_1": {
				"levels": (1..=7).map(skill_level).collect::<Vec<_>>()
			},
			"skill_test_2": {
				"levels": (1..=10).map(skill_level).collect::<Vec<_>>()
			}
		}),
	);
	fixture.set_verified_entry_json(
		"range_table.json",
		&json!({
			"test-range": {"id": "test-range", "direction": 1, "grids": []},
			"skill-range": {"id": "skill-range", "direction": 1, "grids": []}
		}),
	);
	fixture.set_verified_entry_json(
		"favor_table.json",
		&json!({
			"maxFavor": 25570,
			"favorFrames": [
				{"level": 0, "data": {"favorPoint": 0, "percent": 0, "battlePhase": 0}},
				{"level": 10070, "data": {"favorPoint": 10070, "percent": 100, "battlePhase": 50}},
				{"level": 25570, "data": {"favorPoint": 25570, "percent": 200, "battlePhase": 100}}
			]
		}),
	);
	fixture.set_verified_entry_json(
		"uniequip_table.json",
		&json!({
			"charEquip": {
				"char_test_unique": ["module_initial", "module_real", "module_special"],
				"char_test_ranged": ["module_ranged_initial"],
				"char_test_patch": ["module_patch"]
			},
			"equipDict": {
				"module_initial": {
					"uniEquipId": "module_initial",
					"charId": "char_test_unique",
					"tmplId": null,
					"type": "INITIAL",
					"isSpecialEquip": false,
					"unlockEvolvePhase": "PHASE_0",
					"unlockLevel": 0
				},
				"module_real": {
					"uniEquipId": "module_real",
					"charId": "char_test_unique",
					"tmplId": null,
					"type": "ADVANCED",
					"isSpecialEquip": false,
					"unlockEvolvePhase": "PHASE_2",
					"unlockLevel": 60
				},
				"module_special": {
					"uniEquipId": "module_special",
					"charId": "char_test_unique",
					"tmplId": null,
					"type": "ADVANCED",
					"isSpecialEquip": true,
					"unlockEvolvePhase": "PHASE_2",
					"unlockLevel": 60
				},
				"module_ranged_initial": {
					"uniEquipId": "module_ranged_initial",
					"charId": "char_test_ranged",
					"tmplId": null,
					"type": "INITIAL",
					"isSpecialEquip": false,
					"unlockEvolvePhase": "PHASE_0",
					"unlockLevel": 0
				},
				"module_patch": {
					"uniEquipId": "module_patch",
					"charId": "char_test_unique",
					"tmplId": "char_test_patch",
					"type": "ADVANCED",
					"isSpecialEquip": false,
					"unlockEvolvePhase": "PHASE_2",
					"unlockLevel": 60
				}
			}
		}),
	);
	fixture.set_verified_entry_json(
		"battle_equip_table.json",
		&json!({
			"module_real": {
				"phases": [
					{
						"equipLevel": 1,
						"attributeBlackboard": [
							{"key": "atk", "value": 10.0, "valueStr": null}
						],
						"parts": [],
						"tokenAttributeBlackboard": {}
					},
					{
						"equipLevel": 2,
						"attributeBlackboard": [
							{"key": "atk", "value": 20.0, "valueStr": null}
						],
						"parts": [],
						"tokenAttributeBlackboard": {}
					},
					{
						"equipLevel": 3,
						"attributeBlackboard": [
							{"key": "atk", "value": 30.0, "valueStr": null},
							{"key": "def", "value": 5.0, "valueStr": null}
						],
						"parts": [{
							"resKey": "module_part",
							"target": "TALENT",
							"isToken": false,
							"validInGameTag": null,
							"validInMapTag": null,
							"addOrOverrideTalentDataBundle": {"candidates": [{
								"displayRangeId": false,
								"upgradeDescription": "dynamic upgrade",
								"talentIndex": 0,
								"validModeIndices": null,
								"unlockCondition": {"phase": "PHASE_2", "level": 60},
								"requiredPotentialRank": 0,
								"prefabKey": "talent_prefab",
								"name": "Dynamic module talent",
								"description": null,
								"rangeId": null,
								"blackboard": [
									{"key": "damage_scale", "value": 1.5, "valueStr": null}
								],
								"tokenKey": null,
								"isHideTalent": false
							}]},
							"overrideTraitDataBundle": {"candidates": null}
						}],
						"tokenAttributeBlackboard": {
							"token_test": [
								{"key": "max_hp", "value": 100.0, "valueStr": null}
							]
						}
					}
				]
			},
			"module_special": {
				"phases": [{
					"equipLevel": 1,
					"attributeBlackboard": [],
					"parts": [],
					"tokenAttributeBlackboard": {}
				}]
			},
			"module_patch": {
				"phases": [{
					"equipLevel": 1,
					"attributeBlackboard": [],
					"parts": [],
					"tokenAttributeBlackboard": {}
				}]
			}
		}),
	);
	fixture
}

fn supported_build(identity: OperatorIdentity) -> OperatorBuild {
	OperatorBuild::new(
		identity,
		1,
		50,
		0,
		2,
		7,
		PotentialRank::P1,
		OperatorModule::None,
		0,
	)
}

fn fixture_entry_json(fixture: &Fixture, destination_path: &str) -> Value {
	serde_json::from_slice(
		&fs::read(fixture.entry_path(destination_path)).expect("read fixture entry"),
	)
	.expect("parse fixture entry")
}

#[test]
fn resolves_an_interior_level_for_an_exact_trust_and_potential_rank() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		1,
		25,
		0,
		2,
		7,
		PotentialRank::P1,
		OperatorModule::None,
		0,
	);
	let resolved = snapshot.operators().resolve(&build).unwrap();

	let expected_attack: f64 = 50.0 + (100.0 - 50.0) * 24.0 / 49.0;
	assert!((resolved.base_stats().attack() - expected_attack).abs() < 1e-9);
	assert_eq!(resolved.build().trust_percent(), 0);
	assert_eq!(resolved.build().potential(), PotentialRank::P1);
	assert_eq!(
		resolved.stable_id(),
		format!("{REVISION}:char_test_unique:e1:l25:t0:p1:s2:sl7:m-none:ml0")
	);
}

#[test]
fn applies_capped_trust_and_cumulative_buff_potentials_while_retaining_custom_ranks() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		1,
		50,
		200,
		2,
		10,
		PotentialRank::P6,
		OperatorModule::None,
		0,
	);
	let resolved = snapshot.operators().resolve(&build).unwrap();

	assert_eq!(resolved.base_stats().max_hp(), 520.0);
	assert_eq!(resolved.base_stats().attack(), 127.0);
	assert_eq!(resolved.base_stats().defense(), 63.0);
	assert_eq!(resolved.deployment_cost(), 9);
	assert_eq!(resolved.trust_battle_phase(), 50);
	assert_eq!(
		resolved
			.custom_potential_effects()
			.iter()
			.map(|effect| (effect.rank(), effect.description()))
			.collect::<Vec<_>>(),
		vec![
			(PotentialRank::P3, "talent alpha"),
			(PotentialRank::P5, "talent beta")
		]
	);
}

#[test]
fn exposes_the_exact_selected_skill_level_metadata() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		1,
		50,
		0,
		2,
		10,
		PotentialRank::P1,
		OperatorModule::None,
		0,
	);
	let resolved = snapshot.operators().resolve(&build).unwrap();
	let skill = resolved.selected_skill();

	assert_eq!(skill.skill_id(), "skill_test_2");
	assert_eq!(
		skill.override_prefab_key(),
		Some("skill_test_override_prefab")
	);
	assert_eq!(
		skill.override_token_key(),
		Some("token_10058_sbell2_icetgt")
	);
	assert_eq!(skill.name(), "Synthetic Skill");
	assert_eq!(skill.description(), Some("level 10"));
	assert_eq!(skill.activation_type(), &SkillActivationType::Auto);
	assert_eq!(skill.duration_type(), &SkillDurationType::None);
	assert_eq!(skill.duration_seconds(), 12.5);
	assert_eq!(skill.range_id(), Some("skill-range"));
	assert_eq!(skill.sp().sp_type(), &SkillSpType::IncreaseWithTime);
	assert_eq!(skill.sp().cost(), 30);
	assert_eq!(skill.sp().initial(), 10);
	assert_eq!(skill.sp().max_charges(), 1);
	assert_eq!(skill.sp().increment(), 1.0);
	assert_eq!(skill.blackboard()[0].key(), "atk_scale");
	assert_eq!(skill.blackboard()[0].value(), 1.0);
	assert_eq!(skill.blackboard()[1].value_string(), Some("token_test"));
}

#[test]
fn rejects_common_skill_levels_above_the_operator_progression() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		0,
		30,
		0,
		1,
		7,
		PotentialRank::P1,
		OperatorModule::None,
		0,
	);

	assert!(matches!(
		snapshot.operators().resolve(&build),
		Err(ArkDataError::InvalidOperatorBuild {
			field: "skill_level",
			..
		})
	));
}

#[test]
fn preserves_dynamic_trait_and_talent_metadata_without_applying_it_to_the_panel() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		2,
		90,
		200,
		2,
		10,
		PotentialRank::P6,
		OperatorModule::None,
		0,
	);
	let resolved = snapshot.operators().resolve(&build).unwrap();

	assert_eq!(resolved.base_stats().attack(), 167.0);
	assert_eq!(resolved.trait_candidates().len(), 1);
	assert_eq!(
		resolved.trait_candidates()[0].blackboard()[0].key(),
		"dynamic_def_scale"
	);
	assert_eq!(resolved.talent_groups().len(), 1);
	assert_eq!(
		resolved.talent_groups()[0].candidates()[0].range_id(),
		Some("skill-range")
	);
	assert_eq!(
		resolved.talent_groups()[0].candidates()[0].blackboard()[0].key(),
		"dynamic_atk_scale"
	);
}

#[test]
fn selects_active_trait_and_talent_candidates_by_progression_and_potential() {
	let mut fixture: Fixture = operator_fixture();
	let mut characters: Value = fixture_entry_json(&fixture, "character_table.json");
	characters["char_test_unique"]["trait"]["candidates"] = json!([
		{
			"unlockCondition": {"phase": "PHASE_0", "level": 1},
			"requiredPotentialRank": 0,
			"blackboard": [{"key": "trait_scale", "value": 1.0, "valueStr": null}],
			"overrideDescripton": "base trait",
			"prefabKey": "trait_prefab",
			"rangeId": null
		},
		{
			"unlockCondition": {"phase": "PHASE_1", "level": 40},
			"requiredPotentialRank": 0,
			"blackboard": [{"key": "trait_scale", "value": 2.0, "valueStr": null}],
			"overrideDescripton": "elite trait",
			"prefabKey": "trait_prefab",
			"rangeId": null
		}
	]);
	characters["char_test_unique"]["talents"] = json!([
		{"candidates": [
			{
				"unlockCondition": {"phase": "PHASE_0", "level": 1},
				"requiredPotentialRank": 0,
				"prefabKey": "talent_prefab",
				"name": "Base Talent",
				"description": "base",
				"rangeId": null,
				"blackboard": [{"key": "talent_scale", "value": 1.0, "valueStr": null}],
				"tokenKey": null,
				"isHideTalent": false
			},
			{
				"unlockCondition": {"phase": "PHASE_1", "level": 40},
				"requiredPotentialRank": 0,
				"prefabKey": "talent_prefab",
				"name": "Elite Talent",
				"description": "elite",
				"rangeId": null,
				"blackboard": [{"key": "talent_scale", "value": 2.0, "valueStr": null}],
				"tokenKey": null,
				"isHideTalent": false
			},
			{
				"unlockCondition": {"phase": "PHASE_1", "level": 40},
				"requiredPotentialRank": 2,
				"prefabKey": "talent_prefab",
				"name": "Potential Talent",
				"description": "potential",
				"rangeId": null,
				"blackboard": [{"key": "talent_scale", "value": 3.0, "valueStr": null}],
				"tokenKey": null,
				"isHideTalent": false
			},
			{
				"unlockCondition": {"phase": "PHASE_1", "level": 40},
				"requiredPotentialRank": 3,
				"prefabKey": "talent_prefab",
				"name": "Later Talent",
				"description": "later",
				"rangeId": null,
				"blackboard": [{"key": "talent_scale", "value": 4.0, "valueStr": null}],
				"tokenKey": null,
				"isHideTalent": false
			}
		]},
		{"candidates": [{
			"unlockCondition": {"phase": "PHASE_2", "level": 1},
			"requiredPotentialRank": 0,
			"prefabKey": "locked_talent_prefab",
			"name": "Locked Talent",
			"description": "locked",
			"rangeId": null,
			"blackboard": [],
			"tokenKey": null,
			"isHideTalent": false
		}]}
	]);
	fixture.set_verified_entry_json("character_table.json", &characters);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();

	let below_level = snapshot
		.operators()
		.resolve(&OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			30,
			0,
			2,
			7,
			PotentialRank::P3,
			OperatorModule::None,
			0,
		))
		.unwrap();
	assert_eq!(
		below_level.selected_trait_candidate().unwrap().blackboard()[0].value(),
		1.0
	);

	let below_potential = snapshot
		.operators()
		.resolve(&OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			0,
			2,
			7,
			PotentialRank::P2,
			OperatorModule::None,
			0,
		))
		.unwrap();
	assert_eq!(
		below_potential
			.selected_trait_candidate()
			.unwrap()
			.blackboard()[0]
			.value(),
		2.0
	);
	assert_eq!(
		below_potential
			.selected_talent_candidates()
			.map(|candidate| candidate.name())
			.collect::<Vec<_>>(),
		vec![Some("Elite Talent")]
	);

	let potential_unlocked = snapshot
		.operators()
		.resolve(&OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			0,
			2,
			7,
			PotentialRank::P3,
			OperatorModule::None,
			0,
		))
		.unwrap();
	assert_eq!(
		potential_unlocked
			.selected_trait_candidate()
			.unwrap()
			.blackboard()[0]
			.value(),
		2.0
	);
	assert_eq!(
		potential_unlocked
			.selected_talent_candidates()
			.map(|candidate| candidate.name())
			.collect::<Vec<_>>(),
		vec![Some("Potential Talent")]
	);
	let direct_custom_effect = potential_unlocked
		.custom_potential_effects()
		.iter()
		.find(|effect| effect.rank() == PotentialRank::P3)
		.unwrap();
	assert!(potential_unlocked.custom_potential_effect_is_absorbed(direct_custom_effect));

	let later_potential = snapshot
		.operators()
		.resolve(&OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			0,
			2,
			7,
			PotentialRank::P5,
			OperatorModule::None,
			0,
		))
		.unwrap();
	assert_eq!(
		later_potential
			.selected_talent_candidates()
			.map(|candidate| candidate.name())
			.collect::<Vec<_>>(),
		vec![Some("Later Talent")]
	);
	let superseded_custom_effect = later_potential
		.custom_potential_effects()
		.iter()
		.find(|effect| effect.rank() == PotentialRank::P3)
		.unwrap();
	assert!(later_potential.custom_potential_effect_is_absorbed(superseded_custom_effect));
	let unmatched_custom_effect = later_potential
		.custom_potential_effects()
		.iter()
		.find(|effect| effect.rank() == PotentialRank::P5)
		.unwrap();
	assert!(!later_potential.custom_potential_effect_is_absorbed(unmatched_custom_effect));
}

#[test]
fn enumerates_and_resolves_only_real_ordinary_modules_for_the_exact_form() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let legal = snapshot
		.operators()
		.legal_modules("char_test_unique", 2, 90)
		.unwrap();

	assert_eq!(legal.len(), 1);
	assert_eq!(legal[0].module_id(), "module_real");
	assert_eq!(legal[0].max_level(), 3);

	let maximal = snapshot
		.operators()
		.maximal_builds("char_test_unique", 2)
		.unwrap();
	assert_eq!(maximal.len(), 1);
	assert_eq!(maximal[0].elite(), 2);
	assert_eq!(maximal[0].level(), 90);
	assert_eq!(maximal[0].trust_percent(), 200);
	assert_eq!(maximal[0].potential(), PotentialRank::P6);
	assert_eq!(maximal[0].skill(), 2);
	assert_eq!(maximal[0].skill_level(), 10);
	assert_eq!(
		maximal[0].module(),
		&OperatorModule::Id("module_real".to_owned())
	);
	assert_eq!(maximal[0].module_level(), 3);

	let resolved = snapshot.operators().resolve(&maximal[0]).unwrap();
	assert_eq!(resolved.base_stats().max_hp(), 720.0);
	assert_eq!(resolved.base_stats().attack(), 197.0);
	assert_eq!(resolved.base_stats().defense(), 88.0);
	assert_eq!(resolved.deployment_cost(), 11);
	let module = resolved.selected_module().unwrap();
	assert_eq!(module.module_id(), "module_real");
	assert_eq!(module.level(), 3);
	assert_eq!(module.parts()[0].resource_key(), Some("module_part"));
	assert_eq!(
		module.parts()[0].talent_candidates()[0].blackboard()[0].key(),
		"damage_scale"
	);
	assert_eq!(
		module.token_attribute_blackboard()["token_test"][0].key(),
		"max_hp"
	);

	let no_module_maximal = snapshot
		.operators()
		.maximal_builds("char_test_ranged", 2)
		.unwrap();
	assert_eq!(no_module_maximal.len(), 1);
	assert_eq!(no_module_maximal[0].module(), &OperatorModule::None);
	assert_eq!(no_module_maximal[0].module_level(), 0);
}

#[test]
fn operator_ids_skip_token_and_trap_records() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();

	let operator_ids: Vec<&str> = snapshot.operators().operator_ids().collect();

	assert!(operator_ids.contains(&"char_test_unique"));
	assert!(!operator_ids.contains(&"token_test_summon"));
	assert!(!operator_ids.contains(&"trap_test_crate"));
}

#[test]
fn keeps_patch_forms_and_their_modules_as_exact_distinct_identities() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();

	assert!(snapshot
		.operators()
		.operator_ids()
		.any(|operator_id: &str| operator_id == "char_test_patch"));
	let patch_modules = snapshot
		.operators()
		.legal_modules("char_test_patch", 2, 90)
		.unwrap();
	assert_eq!(patch_modules.len(), 1);
	assert_eq!(patch_modules[0].module_id(), "module_patch");
	let base_modules = snapshot
		.operators()
		.legal_modules("char_test_unique", 2, 90)
		.unwrap();
	assert_eq!(base_modules.len(), 1);
	assert_eq!(base_modules[0].module_id(), "module_real");

	let patch = snapshot
		.operators()
		.maximal_builds("Patch Operator", 2)
		.unwrap();
	assert_eq!(
		patch[0].identity(),
		&OperatorIdentity::Id("char_test_patch".to_owned())
	);
	assert_eq!(
		patch[0].module(),
		&OperatorModule::Id("module_patch".to_owned())
	);
}

#[test]
fn resolves_highest_phase_base_panel_without_requiring_a_skill() {
	let mut fixture: Fixture = operator_fixture();
	let mut skillless: Value = character("Unique Operator", "MELEE");
	skillless["skills"] = json!([]);
	fixture.set_verified_entry_json(
		"character_table.json",
		&json!({"char_test_unique": skillless}),
	);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let build: BasePanelBuild = snapshot
		.operators()
		.highest_phase_base_panel("char_test_unique")
		.unwrap();
	let resolved = snapshot.operators().resolve_base_panel(&build).unwrap();

	assert_eq!(build.elite(), 2);
	assert_eq!(build.level(), 90);
	assert_eq!(resolved.operator_id(), "char_test_unique");
	assert_eq!(resolved.range_id(), "test-range");
	assert_eq!(resolved.profession(), "CASTER");
	assert_eq!(resolved.sub_profession_id(), "corecaster");
	assert_eq!(resolved.base_stats().attack(), 140.0);
	assert_eq!(resolved.deployment_cost(), 12);
	assert_eq!(
		resolved.stable_id(),
		format!("{REVISION}:char_test_unique:base-panel:e2:l90")
	);
}

#[test]
fn resolves_exact_keyframe_build_by_unique_name_and_id() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();

	for identity in [
		OperatorIdentity::Name("Unique Operator".to_owned()),
		OperatorIdentity::Id("char_test_unique".to_owned()),
	] {
		let resolved = snapshot
			.operators()
			.resolve(&supported_build(identity))
			.unwrap();

		assert_eq!(resolved.operator_id(), "char_test_unique");
		assert_eq!(resolved.name(), "Unique Operator");
		assert_eq!(resolved.resolved_revision(), REVISION);
		assert_eq!(resolved.profession(), "CASTER");
		assert_eq!(resolved.sub_profession_id(), "corecaster");
		assert_eq!(resolved.skill_id(), "skill_test_2");
		assert_eq!(resolved.range_id(), "test-range");
		assert_eq!(resolved.base_stats().max_hp(), 500.0);
		assert_eq!(resolved.static_stats(), resolved.base_stats());
		assert_eq!(resolved.base_stats().attack(), 100.0);
		assert_eq!(resolved.base_stats().defense(), 50.0);
		assert_eq!(resolved.base_stats().magic_resistance(), 15.0);
		assert_eq!(resolved.deployment_cost(), 10);
		assert_eq!(resolved.base_stats().block_count(), 1);
		assert_eq!(resolved.base_stats().base_attack_time_seconds(), 1.25);
		assert_eq!(resolved.base_stats().attack_speed(), 100.0);
		assert_eq!(
			resolved.stable_id(),
			format!("{REVISION}:char_test_unique:e1:l50:t0:p1:s2:sl7:m-none:ml0")
		);
	}
}

#[test]
fn preserves_typed_operator_positions_including_unknown_source_values() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let cases = [
		("char_test_unique", OperatorPosition::Melee),
		("char_test_ranged", OperatorPosition::Ranged),
		("char_test_all", OperatorPosition::All),
		("char_test_none", OperatorPosition::None),
		(
			"char_test_future",
			OperatorPosition::Unknown("FUTURE_POSITION".to_owned()),
		),
	];

	for (operator_id, expected) in cases {
		let resolved = snapshot
			.operators()
			.resolve(&supported_build(OperatorIdentity::Id(
				operator_id.to_owned(),
			)))
			.unwrap();

		assert_eq!(resolved.position(), &expected);
	}
}

#[test]
fn preserves_optional_group_and_maps_official_profession_category_masks() {
	let mut fixture: Fixture = operator_fixture();
	let cases: [(&str, &str, u16); 8] = [
		("char_test_warrior", "WARRIOR", 1),
		("char_test_sniper", "SNIPER", 2),
		("char_test_tank", "TANK", 4),
		("char_test_medic", "MEDIC", 8),
		("char_test_support", "SUPPORT", 16),
		("char_test_caster", "CASTER", 32),
		("char_test_special", "SPECIAL", 64),
		("char_test_pioneer", "PIONEER", 512),
	];
	let mut character_table: Value = fixture_entry_json(&fixture, "character_table.json");
	let character_records: &mut serde_json::Map<String, Value> = character_table
		.as_object_mut()
		.expect("character table fixture must be an object");
	for (operator_id, profession, _) in cases {
		character_records.insert(
			operator_id.to_owned(),
			classified_character(profession, profession, Some("group_test")),
		);
	}
	character_records.insert(
		"char_test_unknown_profession".to_owned(),
		classified_character("Unknown Profession", "FUTURE_PROFESSION", None),
	);
	fixture.set_verified_entry_json("character_table.json", &character_table);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();

	for (operator_id, _, expected_mask) in cases {
		let resolved: ResolvedOperatorBuild = snapshot
			.operators()
			.resolve(&supported_build(OperatorIdentity::Id(
				operator_id.to_owned(),
			)))
			.unwrap();

		assert_eq!(resolved.group_id(), Some("group_test"));
		assert_eq!(resolved.profession_category_mask(), Some(expected_mask));
	}

	let unknown: ResolvedOperatorBuild = snapshot
		.operators()
		.resolve(&supported_build(OperatorIdentity::Id(
			"char_test_unknown_profession".to_owned(),
		)))
		.unwrap();
	assert_eq!(unknown.group_id(), None);
	assert_eq!(unknown.profession_category_mask(), None);
}

#[test]
fn rejects_ambiguous_names_instead_of_picking_an_operator() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();

	match snapshot
		.operators()
		.resolve(&supported_build(OperatorIdentity::Name(
			"Duplicate".to_owned(),
		)))
		.unwrap_err()
	{
		ArkDataError::AmbiguousOperatorName {
			name,
			candidate_ids,
		} => {
			assert_eq!(name, "Duplicate");
			assert_eq!(
				candidate_ids,
				vec![
					"char_test_duplicate_a".to_owned(),
					"char_test_duplicate_b".to_owned()
				]
			);
		}
		other => panic!("expected AmbiguousOperatorName, got {other:?}"),
	}
}

#[test]
fn fails_closed_for_unknown_potential_formulas_and_module_stat_keys() {
	let mut potential_fixture: Fixture = operator_fixture();
	let mut characters: Value = fixture_entry_json(&potential_fixture, "character_table.json");
	characters["char_test_unique"]["potentialRanks"][0]["buff"]["attributes"]
		["attributeModifiers"][0]["formulaItem"] = json!("MULTIPLICATION");
	potential_fixture.set_verified_entry_json("character_table.json", &characters);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(potential_fixture.path()).unwrap();
	let potential_build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		1,
		50,
		0,
		2,
		7,
		PotentialRank::P2,
		OperatorModule::None,
		0,
	);
	assert!(matches!(
		snapshot.operators().resolve(&potential_build),
		Err(ArkDataError::UnsupportedOperatorProgression { .. })
	));

	let mut module_fixture: Fixture = operator_fixture();
	let mut modules: Value = fixture_entry_json(&module_fixture, "battle_equip_table.json");
	modules["module_real"]["phases"][0]["attributeBlackboard"][0]["key"] =
		json!("unknown_static_stat");
	module_fixture.set_verified_entry_json("battle_equip_table.json", &modules);
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(module_fixture.path()).unwrap();
	let module_build: OperatorBuild = OperatorBuild::new(
		OperatorIdentity::Id("char_test_unique".to_owned()),
		2,
		90,
		0,
		2,
		7,
		PotentialRank::P1,
		OperatorModule::Id("module_real".to_owned()),
		1,
	);
	assert!(matches!(
		snapshot.operators().resolve(&module_build),
		Err(ArkDataError::UnsupportedOperatorProgression { .. })
	));
}

#[test]
fn validates_every_exact_build_dimension() {
	let fixture: Fixture = operator_fixture();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	let invalid = [
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			3,
			1,
			0,
			1,
			1,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			2,
			91,
			0,
			1,
			1,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			201,
			1,
			1,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			0,
			0,
			1,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			0,
			30,
			0,
			2,
			7,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			29,
			0,
			2,
			8,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			0,
			2,
			11,
			PotentialRank::P1,
			OperatorModule::None,
			0,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			1,
			50,
			0,
			2,
			7,
			PotentialRank::P1,
			OperatorModule::None,
			1,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			2,
			90,
			0,
			2,
			7,
			PotentialRank::P1,
			OperatorModule::Id("module_initial".to_owned()),
			1,
		),
		OperatorBuild::new(
			OperatorIdentity::Id("char_test_unique".to_owned()),
			2,
			90,
			0,
			2,
			7,
			PotentialRank::P1,
			OperatorModule::Id("module_real".to_owned()),
			4,
		),
	];

	for build in invalid {
		assert!(matches!(
			snapshot.operators().resolve(&build),
			Err(ArkDataError::InvalidOperatorBuild { .. })
		));
	}
}

#[test]
#[ignore = "requires configured gamedata snapshot"]
fn resolves_a_pinned_real_maximal_operator_module_vector_when_available() {
	let root: PathBuf = Config::load()
		.expect("load workspace rogue.toml for local-data test")
		.gamedata_dir;
	let mut fixture: Fixture = Fixture::valid();
	for destination_path in [
		"character_table.json",
		"char_patch_table.json",
		"skill_table.json",
		"range_table.json",
		"favor_table.json",
		"uniequip_table.json",
		"battle_equip_table.json",
	] {
		let bytes: Vec<u8> =
			fs::read(root.join(destination_path)).expect("read pinned operator root");
		fixture.set_verified_entry_bytes(destination_path, &bytes);
	}
	fixture.manifest_mut()["resolved_revision"] = json!("67472377b1ac84a45d38b8277b79e3a298985675");
	fixture.write_manifest();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(fixture.path()).unwrap();
	assert_eq!(
		snapshot.resolved_revision(),
		"67472377b1ac84a45d38b8277b79e3a298985675"
	);
	let builds = snapshot
		.operators()
		.maximal_builds("char_1046_sbell2", 2)
		.unwrap();
	assert_eq!(builds.len(), 1);
	assert_eq!(
		builds[0].module(),
		&OperatorModule::Id("uniequip_002_sbell2".to_owned())
	);
	assert_eq!(builds[0].module_level(), 3);

	let resolved = snapshot.operators().resolve(&builds[0]).unwrap();
	assert_eq!(resolved.name(), "圣聆初雪");
	assert_eq!(resolved.profession(), "CASTER");
	assert_eq!(resolved.sub_profession_id(), "phalanx");
	assert_eq!(resolved.base_stats().max_hp(), 2058.0);
	assert_eq!(resolved.base_stats().attack(), 1025.0);
	assert_eq!(resolved.base_stats().defense(), 333.0);
	assert_eq!(resolved.base_stats().magic_resistance(), 15.0);
	assert_eq!(resolved.deployment_cost(), 22);
	assert_eq!(resolved.selected_skill().skill_id(), "skchr_sbell2_2");
	assert_eq!(
		resolved.selected_skill().override_token_key(),
		Some("token_10058_sbell2_icetgt")
	);
	assert_eq!(
		resolved.stable_id(),
		"67472377b1ac84a45d38b8277b79e3a298985675:char_1046_sbell2:e2:l90:t200:p6:s2:sl10:m-uniequip_002_sbell2:ml3"
	);
}
