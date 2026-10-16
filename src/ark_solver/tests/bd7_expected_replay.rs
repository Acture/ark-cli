use std::collections::{BTreeMap, BTreeSet};

use ark_solver::data::{GridPosition, MapDimensions, RangeOffset};
use ark_solver::simulation::{SimTick, TraceEntry};
use ark_solver::{
	mechanics::domain::{
		ActorCombatFacts, ArkBattleFacts, AttackPattern, AttackProfile, BattleActorId, BattleGrid,
		BattlePath, BattlePathStep, BattlePathStepAction, BattleTimeAnchor, BattleWave,
		BlockTargetFilter, DamageDelivery, DamageType, EffectTarget, EnemyFacts, EnemyInstanceId,
		EnemySpawnFacts, ExpectedDeploymentRules, ExpectedDpRules, ExpectedEffect, ExpectedValue,
		Facing, Faction, GoalId, MaterializedGuide, MaterializedGuideAction,
		MaterializedGuidePolicy, MechanicBehavior, MechanicId, NormalAttackAction,
		NormalAttackTargetFilter, OperatorBuildFacts, OperatorClassificationFacts,
		OperatorInstanceId, PathId, PreplacedActorFacts, ReachedMechanic, RelativeBattleTime,
		ScheduledStageEffect, SummonCellOccupancy, SummonTemplate, SummonTemplateId, TaggedEffect,
		TargetClass, Targetability, WaveId, WaveStartWindow,
	},
	replay::{compile_battle, replay_expected, ExpectedEvent, ExpectedTerminalStatus},
};

fn value(whole: u32) -> ExpectedValue {
	ExpectedValue::from_whole(whole)
}

fn deployment_rules() -> ExpectedDeploymentRules {
	ExpectedDeploymentRules {
		simultaneous_limit: 8,
		second_deployment_cost_multiplier: ExpectedValue::from_ratio(3, 2)
			.expect("valid standard second-deployment multiplier"),
		later_deployment_cost_multiplier: value(2),
		retreat_refund_ratio: ExpectedValue::from_ratio(1, 2)
			.expect("valid standard retreat refund"),
	}
}

fn combat(hp: u32, attack: u32, defense: u32) -> ActorCombatFacts {
	ActorCombatFacts {
		max_hp: value(hp),
		attack: value(attack),
		defense: value(defense),
		target_class: TargetClass::Ground,
		arts_resistance: ExpectedValue::ZERO,
		minimum_damage_ratio: ExpectedValue::ZERO,
	}
}

fn inert_attack() -> AttackProfile {
	AttackProfile {
		first_delay_ticks: 100,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 0,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::None,
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	}
}

fn wave(id: u32, start: WaveStartWindow, spawns: Vec<EnemySpawnFacts>) -> BattleWave {
	BattleWave {
		id: WaveId::new(id),
		start,
		spawns,
	}
}

fn at_battle_start() -> WaveStartWindow {
	WaveStartWindow {
		earliest: RelativeBattleTime {
			anchor: BattleTimeAnchor::BattleStarted,
			offset_ticks: 0,
		},
		latest: None,
	}
}

fn grid(width: u32, height: u32) -> BattleGrid {
	let cells: BTreeSet<GridPosition> = (0..height)
		.flat_map(|row| (0..width).map(move |col| GridPosition { row, col }))
		.collect();
	BattleGrid {
		dimensions: MapDimensions { width, height },
		cells: BTreeMap::new(),
		deployable_cells: cells.clone(),
		ground_cells: cells,
	}
}

fn supported(id: &str, behavior: MechanicBehavior) -> ReachedMechanic {
	ReachedMechanic {
		id: MechanicId::new(id),
		origin: format!("act21mini_07/{id}"),
		behavior,
	}
}

#[test]
fn causal_fragments_and_timed_route_preserve_wait_disappear_reappear_and_goal() {
	let route = BattlePath {
		id: PathId::new(1),
		goal: GoalId::new(1),
		start: GridPosition { row: 0, col: 0 },
		steps: vec![
			BattlePathStep {
				duration_ticks: 2,
				affected_by_movement_speed: false,
				action: BattlePathStepAction::Wait,
			},
			BattlePathStep {
				duration_ticks: 1,
				affected_by_movement_speed: false,
				action: BattlePathStepAction::Disappear,
			},
			BattlePathStep {
				duration_ticks: 2,
				affected_by_movement_speed: false,
				action: BattlePathStepAction::Reappear {
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: 4,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Goal {
					goal: GoalId::new(1),
				},
			},
		],
	};
	let first_enemy = EnemyInstanceId::new(1);
	let second_enemy = EnemyInstanceId::new(2);
	let facts = ArkBattleFacts {
		battle_id: "act21mini_07-causal".to_owned(),
		end_at: Some(SimTick::new(20)),
		dp: ExpectedDpRules {
			initial: 0,
			maximum: 0,
			regen_amount: 0,
			regen_interval_ticks: 1,
		},
		deployment: deployment_rules(),
		grid: grid(3, 1),
		paths: vec![route],
		waves: vec![
			wave(
				1,
				at_battle_start(),
				vec![EnemySpawnFacts {
					id: first_enemy,
					enemy_key: "route_enemy".to_owned(),
					after_wave_start_ticks: 0,
					path: PathId::new(1),
					facing: Facing::Right,
				}],
			),
			wave(
				2,
				WaveStartWindow {
					earliest: RelativeBattleTime {
						anchor: BattleTimeAnchor::WaveCleared(WaveId::new(1)),
						offset_ticks: 2,
					},
					latest: Some(RelativeBattleTime {
						anchor: BattleTimeAnchor::WaveStarted(WaveId::new(1)),
						offset_ticks: 10,
					}),
				},
				vec![EnemySpawnFacts {
					id: second_enemy,
					enemy_key: "route_enemy".to_owned(),
					after_wave_start_ticks: 0,
					path: PathId::new(1),
					facing: Facing::Right,
				}],
			),
		],
		enemies: vec![EnemyFacts {
			enemy_key: "route_enemy".to_owned(),
			block_demand: 1,
			mass: 1,
			combat: combat(100, 0, 0),
			attack: inert_attack(),
			on_spawn: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		summons: Vec::new(),
		preplaced_actors: Vec::new(),
		stage_effects: vec![ScheduledStageEffect {
			at: SimTick::new(1),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("fragment_clear"),
				target: EffectTarget::AllEnemies,
				effect: ExpectedEffect::Death,
			},
		}],
		reached_mechanics: Vec::new(),
	};
	let battle =
		compile_battle(&facts, &MaterializedGuide::default()).expect("typed route is supported");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.status(),
		&ExpectedTerminalStatus::GoalLeak {
			enemies: vec![second_enemy]
		}
	);
	assert_eq!(
		assessment.enemies()[&second_enemy].spawned_at,
		Some(SimTick::new(3))
	);
	assert_eq!(
		assessment.enemies()[&second_enemy].deadline_at,
		Some(SimTick::new(12))
	);
	let movement_ticks: Vec<u64> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled }
				if matches!(
					scheduled.event(),
					ExpectedEvent::Move { enemy, .. } if *enemy == second_enemy
				) =>
			{
				Some(scheduled.at().get())
			}
			TraceEntry::Command { .. } | TraceEntry::Event { .. } | TraceEntry::Choice { .. } => {
				None
			}
		})
		.collect();
	assert_eq!(movement_ticks, vec![5, 6, 8, 12]);
}

#[test]
fn bd7_shelter_preplaced_actor_is_present_and_blocks_as_blocked_only_target() {
	let trap_template = SummonTemplateId::new(1);
	let enemy = EnemyInstanceId::new(1);
	let facts = ArkBattleFacts {
		battle_id: "act21mini_07-preplaced".to_owned(),
		end_at: Some(SimTick::new(5)),
		dp: ExpectedDpRules {
			initial: 0,
			maximum: 0,
			regen_amount: 0,
			regen_interval_ticks: 1,
		},
		deployment: deployment_rules(),
		grid: grid(3, 1),
		paths: vec![BattlePath {
			id: PathId::new(1),
			goal: GoalId::new(1),
			start: GridPosition { row: 0, col: 0 },
			steps: vec![
				BattlePathStep {
					duration_ticks: 1,
					affected_by_movement_speed: true,
					action: BattlePathStepAction::Move {
						cell: GridPosition { row: 0, col: 1 },
						facing: Facing::Right,
					},
				},
				BattlePathStep {
					duration_ticks: 1,
					affected_by_movement_speed: true,
					action: BattlePathStepAction::Goal {
						goal: GoalId::new(1),
					},
				},
			],
		}],
		waves: vec![wave(
			1,
			at_battle_start(),
			vec![EnemySpawnFacts {
				id: enemy,
				enemy_key: "attacker".to_owned(),
				after_wave_start_ticks: 0,
				path: PathId::new(1),
				facing: Facing::Right,
			}],
		)],
		enemies: vec![EnemyFacts {
			enemy_key: "attacker".to_owned(),
			block_demand: 1,
			mass: 1,
			combat: combat(100, 100, 0),
			attack: AttackProfile {
				first_delay_ticks: 1,
				interval_ticks: 1,
				range: vec![RangeOffset::new(0, 0)],
				target_count: 1,
				target_filter: NormalAttackTargetFilter::GroundAndAir,
				action: NormalAttackAction::Damage {
					damage_type: DamageType::Physical,
					delivery: DamageDelivery::Normal,
				},
				pattern: AttackPattern::Single,
				extra_effects: Vec::new(),
			},
			on_spawn: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		summons: vec![SummonTemplate {
			id: trap_template,
			key: "trap_183_sheltk".to_owned(),
			faction: Faction::Friendly,
			cell_occupancy: SummonCellOccupancy::Exclusive,
			block_count: 3,
			block_target_filter: BlockTargetFilter::GroundOnly,
			targetability: Targetability::BlockedOnly,
			combat: combat(5_000, 0, 200),
			attack: inert_attack(),
			lifetime_ticks: None,
			reached_mechanics: vec![supported("trap_183_sheltk", MechanicBehavior::Blocking)],
		}],
		preplaced_actors: vec![PreplacedActorFacts {
			instance: 0,
			template: trap_template,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		stage_effects: Vec::new(),
		reached_mechanics: vec![supported("trap_183_sheltk", MechanicBehavior::Blocking)],
	};
	let battle = compile_battle(&facts, &MaterializedGuide::default())
		.expect("reviewed preplaced shelter actor is supported");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.status(),
		&ExpectedTerminalStatus::Deadline {
			alive_enemies: vec![enemy]
		}
	);
	assert!(assessment.trace().entries().iter().any(|entry| matches!(
		entry,
		TraceEntry::Event { scheduled }
			if matches!(
				scheduled.event(),
				ExpectedEvent::SummonSpawn {
					source: BattleActorId::Stage,
					template,
					instance: 0,
					..
				} if *template == trap_template
			)
	)));
	assert!(assessment.trace().entries().iter().any(|entry| matches!(
		entry,
		TraceEntry::Event { scheduled }
			if matches!(
				scheduled.event(),
				ExpectedEvent::ResolveEffect { targets, .. }
					if targets == &vec![BattleActorId::Summon(0)]
			)
	)));
}

#[test]
fn cross_attack_uses_grid_shape_and_bd7_reviewed_mechanics_compile() {
	let operator = OperatorInstanceId::new(1);
	let spawn_cells = [
		GridPosition { row: 2, col: 3 },
		GridPosition { row: 2, col: 4 },
		GridPosition { row: 3, col: 3 },
	];
	let paths: Vec<BattlePath> = spawn_cells
		.iter()
		.enumerate()
		.map(|(index, cell)| BattlePath {
			id: PathId::new(index as u32),
			goal: GoalId::new(index as u32),
			start: *cell,
			steps: vec![
				BattlePathStep {
					duration_ticks: 10,
					affected_by_movement_speed: false,
					action: BattlePathStepAction::Wait,
				},
				BattlePathStep {
					duration_ticks: 1,
					affected_by_movement_speed: true,
					action: BattlePathStepAction::Goal {
						goal: GoalId::new(index as u32),
					},
				},
			],
		})
		.collect();
	let spawns: Vec<EnemySpawnFacts> = spawn_cells
		.iter()
		.enumerate()
		.map(|(index, _)| EnemySpawnFacts {
			id: EnemyInstanceId::new(index as u32),
			enemy_key: "cross_target".to_owned(),
			after_wave_start_ticks: 0,
			path: PathId::new(index as u32),
			facing: Facing::Right,
		})
		.collect();
	let reached = vec![
		supported("CrossAttack", MechanicBehavior::CrossAttack),
		supported("PowerHit", MechanicBehavior::PowerHit),
		supported("snow", MechanicBehavior::SnowColdFreeze),
		supported("cold", MechanicBehavior::SnowColdFreeze),
		supported("freeze", MechanicBehavior::SnowColdFreeze),
		supported("gravity", MechanicBehavior::Gravity),
		supported(
			"shelter_exposure_direct_hit",
			MechanicBehavior::ShelterExposureOrDirectHit,
		),
	];
	let facts = ArkBattleFacts {
		battle_id: "act21mini_07-cross".to_owned(),
		end_at: Some(SimTick::new(10)),
		dp: ExpectedDpRules {
			initial: 0,
			maximum: 0,
			regen_amount: 0,
			regen_interval_ticks: 1,
		},
		deployment: deployment_rules(),
		grid: grid(5, 5),
		paths,
		waves: vec![wave(1, at_battle_start(), spawns)],
		enemies: vec![EnemyFacts {
			enemy_key: "cross_target".to_owned(),
			block_demand: 1,
			mass: 1,
			combat: combat(10, 0, 0),
			attack: inert_attack(),
			on_spawn: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		summons: Vec::new(),
		preplaced_actors: Vec::new(),
		stage_effects: Vec::new(),
		reached_mechanics: reached,
	};
	let guide = MaterializedGuide {
		builds: vec![OperatorBuildFacts {
			id: operator,
			operator_key: "cross_attacker".to_owned(),
			classification: OperatorClassificationFacts::default(),
			on_battle_start: Vec::new(),
			deployment_cost: 0,
			redeployment_cooldown_ticks: 0,
			block_count: 1,
			block_target_filter: BlockTargetFilter::GroundOnly,
			combat: combat(100, 10, 0),
			attack: AttackProfile {
				first_delay_ticks: 1,
				interval_ticks: 10,
				range: vec![RangeOffset::new(0, 1)],
				target_count: 1,
				target_filter: NormalAttackTargetFilter::GroundAndAir,
				action: NormalAttackAction::Damage {
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				},
				pattern: AttackPattern::CrossAttack { arm_length: 1 },
				extra_effects: Vec::new(),
			},
			skill: None,
			on_deploy: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: vec![supported("CrossAttack", MechanicBehavior::CrossAttack)],
		}],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 2, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("reviewed BD-7 mechanics are supported");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	assert!(assessment
		.enemies()
		.values()
		.all(|enemy| enemy.death_at == Some(SimTick::new(1))));
}

#[test]
fn power_hit_uses_compiled_cadence_and_multiplier() {
	let operator = OperatorInstanceId::new(1);
	let enemy = EnemyInstanceId::new(1);
	let facts = ArkBattleFacts {
		battle_id: "act21mini_07-power-hit".to_owned(),
		end_at: Some(SimTick::new(10)),
		dp: ExpectedDpRules {
			initial: 0,
			maximum: 0,
			regen_amount: 0,
			regen_interval_ticks: 1,
		},
		deployment: deployment_rules(),
		grid: grid(3, 1),
		paths: vec![BattlePath {
			id: PathId::new(1),
			goal: GoalId::new(1),
			start: GridPosition { row: 0, col: 1 },
			steps: vec![
				BattlePathStep {
					duration_ticks: 9,
					affected_by_movement_speed: false,
					action: BattlePathStepAction::Wait,
				},
				BattlePathStep {
					duration_ticks: 1,
					affected_by_movement_speed: true,
					action: BattlePathStepAction::Goal {
						goal: GoalId::new(1),
					},
				},
			],
		}],
		waves: vec![wave(
			1,
			at_battle_start(),
			vec![EnemySpawnFacts {
				id: enemy,
				enemy_key: "power_target".to_owned(),
				after_wave_start_ticks: 0,
				path: PathId::new(1),
				facing: Facing::Right,
			}],
		)],
		enemies: vec![EnemyFacts {
			enemy_key: "power_target".to_owned(),
			block_demand: 1,
			mass: 1,
			combat: combat(15, 0, 0),
			attack: inert_attack(),
			on_spawn: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		summons: Vec::new(),
		preplaced_actors: Vec::new(),
		stage_effects: Vec::new(),
		reached_mechanics: vec![supported("PowerHit", MechanicBehavior::PowerHit)],
	};
	let guide = MaterializedGuide {
		builds: vec![OperatorBuildFacts {
			id: operator,
			operator_key: "power_attacker".to_owned(),
			classification: OperatorClassificationFacts::default(),
			on_battle_start: Vec::new(),
			deployment_cost: 0,
			redeployment_cooldown_ticks: 0,
			block_count: 1,
			block_target_filter: BlockTargetFilter::GroundOnly,
			combat: combat(100, 5, 0),
			attack: AttackProfile {
				first_delay_ticks: 1,
				interval_ticks: 1,
				range: vec![RangeOffset::new(0, 1)],
				target_count: 1,
				target_filter: NormalAttackTargetFilter::GroundAndAir,
				action: NormalAttackAction::Damage {
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				},
				pattern: AttackPattern::PowerHit {
					every: 2,
					damage_multiplier: value(2),
					shift: None,
				},
				extra_effects: Vec::new(),
			},
			skill: None,
			on_deploy: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: vec![supported("PowerHit", MechanicBehavior::PowerHit)],
		}],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 0 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("PowerHit facts are explicit");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	assert_eq!(assessment.enemies()[&enemy].death_at, Some(SimTick::new(2)));
	assert_eq!(
		assessment.enemies()[&enemy].expected_damage_taken,
		value(15)
	);
}
