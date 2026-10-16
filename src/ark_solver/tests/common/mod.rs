// Each integration-test binary imports a different subset of this shared fixture API.
#![allow(dead_code)]

use std::{
	fs,
	path::{Path, PathBuf},
	sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
pub const REQUIRED_ROOTS: [&str; 12] = [
	"character_table.json",
	"char_patch_table.json",
	"skill_table.json",
	"range_table.json",
	"favor_table.json",
	"token_table.json",
	"uniequip_table.json",
	"battle_equip_table.json",
	"gamedata_const.json",
	"stage_table.json",
	"enemy_database.json",
	"enemy_handbook_table.json",
];
pub const LEVEL_PATH: &str = "levels/activities/test/level_test_01.json";
pub const ENEMY_KEY: &str = "enemy_test";

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
	root: PathBuf,
	manifest: Value,
}

impl Fixture {
	pub fn valid() -> Self {
		let id: u64 = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
		let root: PathBuf = std::env::temp_dir().join(format!(
			"rogue_ark_data_snapshot_{}_{}",
			std::process::id(),
			id
		));
		fs::create_dir_all(&root).expect("create fixture root");

		let mut destinations: Vec<&str> = REQUIRED_ROOTS.to_vec();
		destinations.push(LEVEL_PATH);
		let entries: Vec<Value> = destinations
			.into_iter()
			.map(|destination_path: &str| {
				let value: Value = default_entry_json(destination_path);
				let bytes: Vec<u8> =
					serde_json::to_vec(&value).expect("serialize default fixture entry");
				let path: PathBuf = root.join(destination_path);
				fs::create_dir_all(path.parent().expect("fixture file has parent"))
					.expect("create fixture parent");
				fs::write(&path, &bytes).expect("write fixture entry");
				json!({
					"destination_path": destination_path,
					"source_path": format!("zh_CN/gamedata/{destination_path}"),
					"byte_length": bytes.len(),
					"sha256": sha256_hex(&bytes),
				})
			})
			.collect();
		let manifest: Value = json!({
			"schema_version": 1,
			"repository": "Kengxxiao/ArknightsGameData",
			"resolved_revision": REVISION,
			"generated_at": "2026-07-22T00:00:00Z",
			"entries": entries,
		});
		let fixture: Self = Self { root, manifest };
		fixture.write_manifest();
		fixture
	}

	pub fn path(&self) -> &Path {
		&self.root
	}

	pub fn manifest_path(&self) -> PathBuf {
		self.root.join("manifest.json")
	}

	pub fn entry_path(&self, destination_path: &str) -> PathBuf {
		self.root.join(destination_path)
	}

	pub fn manifest_mut(&mut self) -> &mut Value {
		&mut self.manifest
	}

	pub fn entry_mut(&mut self, destination_path: &str) -> &mut Value {
		self.manifest["entries"]
			.as_array_mut()
			.expect("entries is an array")
			.iter_mut()
			.find(|entry: &&mut Value| entry["destination_path"] == destination_path)
			.expect("fixture entry exists")
	}

	pub fn remove_entry(&mut self, destination_path: &str) {
		self.manifest["entries"]
			.as_array_mut()
			.expect("entries is an array")
			.retain(|entry: &Value| entry["destination_path"] != destination_path);
		self.write_manifest();
	}

	pub fn set_verified_entry_bytes(&mut self, destination_path: &str, bytes: &[u8]) {
		fs::write(self.entry_path(destination_path), bytes).expect("write verified entry bytes");
		let entry: &mut Value = self.entry_mut(destination_path);
		entry["byte_length"] = json!(bytes.len());
		entry["sha256"] = json!(sha256_hex(bytes));
		self.write_manifest();
	}

	pub fn set_verified_entry_json(&mut self, destination_path: &str, value: &Value) {
		let bytes: Vec<u8> = serde_json::to_vec(value).expect("serialize verified entry JSON");
		self.set_verified_entry_bytes(destination_path, &bytes);
	}

	pub fn write_manifest(&self) {
		fs::write(
			self.manifest_path(),
			serde_json::to_vec_pretty(&self.manifest).expect("serialize fixture manifest"),
		)
		.expect("write fixture manifest");
	}
}

impl Drop for Fixture {
	fn drop(&mut self) {
		let _result: std::io::Result<()> = fs::remove_dir_all(&self.root);
	}
}

pub fn sha256_hex(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

fn default_entry_json(destination_path: &str) -> Value {
	match destination_path {
		LEVEL_PATH => minimal_level(),
		"enemy_database.json" => minimal_enemy_database(),
		"range_table.json" => minimal_range_table(),
		_ => json!({}),
	}
}

pub fn minimal_level() -> Value {
	json!({
		"options": {
			"initialCost": 0,
			"maxCost": 99,
			"costIncreaseTime": 1.0,
			"moveMultiplier": 0.5,
			"characterLimit": 8,
			"functionDisableMask": "NONE",
		},
		"levelId": "Activities/Test/Level_Test_01",
		"mapId": "map_test",
		"mapData": {
			"map": [[3, 4, 5], [0, 1, 2]],
			"tiles": [
				{
					"tileKey": "tile_floor_0",
					"heightType": 0,
					"buildableType": 3,
					"passableMask": 3,
					"blackboard": [],
					"effects": [],
				},
				{
					"tileKey": "tile_floor_1",
					"heightType": "LOWLAND",
					"buildableType": "MELEE",
					"passableMask": "WALK_ONLY",
					"blackboard": [],
					"effects": [],
				},
				{
					"tileKey": "tile_start",
					"heightType": "LOWLAND",
					"buildableType": "ALL",
					"passableMask": "ALL",
					"blackboard": [],
					"effects": [],
				},
				{
					"tileKey": "tile_goal",
					"heightType": 1,
					"buildableType": 0,
					"passableMask": 0,
					"blackboard": [],
					"effects": [],
				},
				{
					"tileKey": "tile_floor_4",
					"heightType": "HIGHLAND",
					"buildableType": "RANGED",
					"passableMask": "FLY_ONLY",
					"blackboard": [],
					"effects": [],
				},
				{
					"tileKey": "tile_floor_5",
					"heightType": 0,
					"buildableType": 1,
					"passableMask": 1,
					"blackboard": [],
					"effects": [],
				},
			],
			"blockEdges": [{
				"pos": {"row": 0, "col": 1},
				"direction": "RIGHT",
				"blockMask": "WALK_ONLY",
			}],
			"tags": [],
			"effects": [],
			"layerRects": [],
		},
		"runes": [],
		"optionalRunes": {},
		"globalBuffs": [],
		"routes": [{
			"motionMode": "WALK",
			"startPosition": {"row": 0, "col": 2},
			"endPosition": {"row": 1, "col": 0},
			"spawnRandomRange": {"x": 0.25, "y": 0.5},
			"spawnOffset": {"x": -0.5, "y": 0.75},
			"checkpoints": [
				{
					"type": "MOVE",
					"time": 0.25,
					"position": {"row": 0, "col": 1},
					"reachOffset": {"x": 0.125, "y": -0.25},
					"randomizeReachOffset": true,
					"reachDistance": 0.375,
				},
				{
					"type": "WAIT_FOR_SECONDS",
					"time": 1.5,
					"position": {"row": -1, "col": -1},
				},
				{"type": 5, "time": 0.0, "position": {"row": -1, "col": -1}},
				{
					"type": "APPEAR_AT_POS",
					"time": 0.0,
					"position": {"row": 0, "col": 0},
				},
				{
					"type": 8,
					"time": 0.5,
					"position": {"row": 1, "col": 1},
				},
				{
					"type": "MAP_OFFSET_MOVE",
					"time": 0.75,
					"position": {"row": 1, "col": 2},
				},
			],
			"allowDiagonalMove": true,
			"visitEveryTileCenter": true,
			"visitEveryNodeCenter": false,
			"visitEveryCheckPoint": true,
		}],
		"extraRoutes": [{
			"motionMode": 1,
			"startPosition": {"row": 1, "col": 2},
			"endPosition": {"row": 0, "col": 0},
			"checkpoints": [
				{"type": 1, "time": 2.0, "position": {"row": -1, "col": -1}},
			],
		}],
		"enemies": [],
		"enemyDbRefs": [{
			"useDb": true,
			"id": ENEMY_KEY,
			"level": 0,
			"overwrittenData": null,
		}],
		"waves": [{
			"preDelay": 1.5,
			"postDelay": 2.5,
			"maxTimeWaitingForNextWave": 9.0,
			"advancedWaveTag": "synthetic-advance",
			"fragments": [{
				"preDelay": 0.75,
				"actions": [{
					"actionType": "SPAWN",
					"managedByScheduler": true,
					"key": ENEMY_KEY,
					"count": 2,
					"preDelay": 0.5,
					"interval": 1.25,
					"routeIndex": 0,
					"blockFragment": true,
					"autoPreviewRoute": true,
					"autoDisplayEnemyInfo": true,
					"isUnharmfulAndAlwaysCountAsKilled": true,
					"randomType": "PER_DAY",
					"refreshType": 4,
					"weight": 7,
					"dontBlockWave": true,
					"forceBlockWaveInBranch": true,
				}],
			}],
		}],
		"branches": {
			"zeta": {"phases": [{"preDelay": 6.0, "actions": [{
				"actionType": 0,
				"managedByScheduler": false,
				"key": ENEMY_KEY,
				"count": 3,
				"preDelay": 3.0,
				"interval": 0.75,
				"routeIndex": 0,
			}]}]},
			"alpha": {"phases": [{"preDelay": 4.0, "actions": [{
				"actionType": "SPAWN",
				"managedByScheduler": false,
				"key": ENEMY_KEY,
				"count": 1,
				"preDelay": 2.0,
				"interval": 0.5,
				"routeIndex": 0,
			}]}]},
		},
		"predefines": empty_predefines(),
		"hardPredefines": empty_predefines(),
	})
}

pub fn minimal_enemy_database() -> Value {
	json!({
		"enemies": [{
			"Key": ENEMY_KEY,
			"Value": [{
				"level": 0,
				"enemyData": {
					"name": defined(json!("Synthetic Enemy")),
					"description": defined(json!("fixture enemy")),
					"prefabKey": defined(json!("enemy_prefab_test")),
					"attributes": {
						"maxHp": defined(json!(100)),
						"atk": defined(json!(20)),
						"def": defined(json!(5)),
						"magicResistance": defined(json!(10.0)),
						"cost": defined(json!(0)),
						"blockCnt": defined(json!(1)),
						"moveSpeed": defined(json!(1.0)),
						"attackSpeed": defined(json!(100.0)),
						"baseAttackTime": defined(json!(1.5)),
						"respawnTime": defined(json!(0)),
						"hpRecoveryPerSec": defined(json!(0.0)),
						"spRecoveryPerSec": defined(json!(0.0)),
						"maxDeployCount": defined(json!(1)),
						"massLevel": defined(json!(1)),
						"baseForceLevel": defined(json!(0)),
						"tauntLevel": defined(json!(0)),
					},
					"applyWay": defined(json!("MELEE")),
					"motion": defined(json!("WALK")),
					"enemyTags": defined(json!([])),
					"lifePointReduce": defined(json!(1)),
					"levelType": defined(json!("NORMAL")),
					"rangeRadius": defined(json!(0.5)),
					"numOfExtraDrops": defined(json!(0)),
					"viewRadius": defined(json!(0.0)),
					"notCountInTotal": defined(json!(false)),
					"skills": [{
						"prefabKey": "enemy_skill_test",
						"priority": 0,
						"cooldown": 5.0,
						"initCooldown": 0.0,
						"spCost": 0,
						"blackboard": [],
					}],
					"talentBlackboard": [],
					"spData": null,
				},
			}],
		}],
	})
}

pub fn minimal_range_table() -> Value {
	json!({
		"range_test": {
			"id": "range_test",
			"direction": 0,
			"grids": [{"row": 0, "col": 0}, {"row": -1, "col": 2}],
		},
	})
}

fn empty_predefines() -> Value {
	json!({
		"characterInsts": [],
		"tokenInsts": [],
		"characterCards": [],
		"tokenCards": [],
	})
}

pub fn defined(value: Value) -> Value {
	json!({"m_defined": true, "m_value": value})
}

pub fn undefined(value: Value) -> Value {
	json!({"m_defined": false, "m_value": value})
}
