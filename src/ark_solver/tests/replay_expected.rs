use std::collections::{BTreeMap, BTreeSet};

use ark_solver::data::{GridPosition, MapDimensions, RangeOffset};
use ark_solver::simulation::{SimTick, TraceEntry};
use ark_solver::{
	mechanics::domain::{
		ActorCombatFacts, ActorCondition, ArkBattleFacts, AttackPattern, AttackProfile,
		BattleActorId, BattleCompileIssue, BattleGrid, BattlePath, BattlePathStep,
		BattlePathStepAction, BattleTimeAnchor, BattleWave, BlockCountModifier, BlockTargetFilter,
		BlockTargetModifier, ConditionalStatModifier, DamageDelivery, DamageType, EffectTarget,
		EnemyAxisDirectHitFacts, EnemyFacts, EnemyInstanceId, EnemySpawnFacts,
		ExpectedDeploymentRules, ExpectedDpRules, ExpectedEffect, ExpectedValue, Facing, Faction,
		HitCountModifier, MaterializedGuide, MaterializedGuideAction, MaterializedGuidePolicy,
		MechanicBehavior, MechanicId, NormalAttackAction, NormalAttackTargetFilter,
		OperatorBuildFacts, OperatorClassificationFacts, OperatorInstanceId, PathId,
		PeriodicSpSource, PreplacedActorFacts, ReachedMechanic, RelativeBattleTime,
		ScheduledStageEffect, ShiftDirection, ShiftEffect, SkillActivationRule, SkillFacts,
		SkillManualDiscardActivationLimit, SkillManualDiscardPolicy, SpChargeRule, StatKind,
		StatModifier, StatModifierBucket, StatusEffect, StatusKind, StoredAttackHpRatioComparison,
		StoredAttackReplacementDelivery, StoredAttackReplacementFacts,
		StoredAttackReservationInvalidationAction, StoredAttackReservationInvalidationPolicy,
		StoredAttackTargetOrder, StoredAttackTargetRelationship, SummonCellOccupancy,
		SummonTemplate, SummonTemplateId, TaggedEffect, TargetClass, Targetability, WaveId,
		WaveStartWindow,
	},
	replay::{
		compile_battle, evaluate_defensive_probability, execute_expected_replay, replay_expected,
		replay_expected_to_resolution, ActionAuditStatus, AxisDirectHitPhase, BlockAuditEntry,
		BlockEndReason, DpAuditKind, ExpectedCommand, ExpectedEvent, ExpectedReplayInput,
		ExpectedReplayOutput, ExpectedReplayReportStatus, ExpectedTerminalStatus,
		ExpectedValueAssessment, SkillActivationOrigin, SkillEndReason,
		EXPECTED_REPLAY_REPORT_SCHEMA_VERSION,
	},
	ArtifactFacing, DeterministicDamageStatus, ExpectedStrategyStatus,
};

fn value(whole: u32) -> ExpectedValue {
	ExpectedValue::from_whole(whole)
}

fn status(
	kind: StatusKind,
	duration_ticks: u64,
	incoming_damage_multiplier: ExpectedValue,
) -> StatusEffect {
	StatusEffect {
		kind,
		duration_ticks,
		movement_interval_multiplier: ExpectedValue::ONE,
		attack_speed_delta: 0,
		incoming_damage_multiplier,
		arts_resistance_reduction: ExpectedValue::ZERO,
		arts_resistance_reduction_target: ark_solver::mechanics::domain::StatusFactionFilter::Any,
	}
}

fn deployment_rules(simultaneous_limit: u32) -> ExpectedDeploymentRules {
	ExpectedDeploymentRules {
		simultaneous_limit,
		second_deployment_cost_multiplier: ExpectedValue::from_ratio(3, 2)
			.expect("valid standard second-deployment multiplier"),
		later_deployment_cost_multiplier: value(2),
		retreat_refund_ratio: ExpectedValue::from_ratio(1, 2)
			.expect("valid standard retreat refund"),
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

fn combat(max_hp: u32, attack: u32) -> ActorCombatFacts {
	ActorCombatFacts {
		max_hp: value(max_hp),
		attack: value(attack),
		defense: ExpectedValue::ZERO,
		target_class: TargetClass::Ground,
		arts_resistance: ExpectedValue::ZERO,
		minimum_damage_ratio: ExpectedValue::ZERO,
	}
}

fn path(id: u32, move_at: u64, goal_at: u64) -> BattlePath {
	BattlePath {
		id: PathId::new(id),
		goal: ark_solver::mechanics::domain::GoalId::new(id),
		start: GridPosition { row: 0, col: 0 },
		steps: vec![
			BattlePathStep {
				duration_ticks: move_at,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Move {
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: goal_at,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Goal {
					goal: ark_solver::mechanics::domain::GoalId::new(id),
				},
			},
		],
	}
}

fn stationary_path(id: u32, start: GridPosition) -> BattlePath {
	BattlePath {
		id: PathId::new(id),
		goal: ark_solver::mechanics::domain::GoalId::new(id),
		start,
		steps: vec![
			BattlePathStep {
				duration_ticks: 100,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Wait,
			},
			BattlePathStep {
				duration_ticks: 1,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Goal {
					goal: ark_solver::mechanics::domain::GoalId::new(id),
				},
			},
		],
	}
}

fn one_wave(enemy: EnemyInstanceId, path: PathId) -> BattleWave {
	BattleWave {
		id: WaveId::new(1),
		start: WaveStartWindow {
			earliest: RelativeBattleTime {
				anchor: BattleTimeAnchor::BattleStarted,
				offset_ticks: 0,
			},
			latest: None,
		},
		spawns: vec![EnemySpawnFacts {
			id: enemy,
			enemy_key: "enemy".to_owned(),
			after_wave_start_ticks: 0,
			path,
			facing: Facing::Right,
		}],
	}
}

fn base_facts(enemy_hp: u32) -> ArkBattleFacts {
	let deploy_cell = GridPosition { row: 0, col: 1 };
	ArkBattleFacts {
		battle_id: "synthetic".to_owned(),
		end_at: Some(SimTick::new(30)),
		dp: ExpectedDpRules {
			initial: 0,
			maximum: 10,
			regen_amount: 1,
			regen_interval_ticks: 1,
		},
		deployment: deployment_rules(8),
		grid: BattleGrid {
			dimensions: MapDimensions {
				width: 4,
				height: 1,
			},
			cells: BTreeMap::new(),
			deployable_cells: BTreeSet::from([deploy_cell, GridPosition { row: 0, col: 2 }]),
			ground_cells: (0..4).map(|col| GridPosition { row: 0, col }).collect(),
		},
		paths: vec![path(1, 3, 20)],
		waves: vec![one_wave(EnemyInstanceId::new(1), PathId::new(1))],
		enemies: vec![EnemyFacts {
			enemy_key: "enemy".to_owned(),
			block_demand: 1,
			mass: 1,
			combat: combat(enemy_hp, 0),
			attack: inert_attack(),
			on_spawn: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		summons: Vec::new(),
		preplaced_actors: Vec::new(),
		stage_effects: Vec::new(),
		reached_mechanics: Vec::new(),
	}
}

fn operator_build(
	id: OperatorInstanceId,
	deployment_cost: u32,
	operator_key: &str,
) -> OperatorBuildFacts {
	OperatorBuildFacts {
		id,
		operator_key: operator_key.to_owned(),
		classification: OperatorClassificationFacts::default(),
		on_battle_start: Vec::new(),
		deployment_cost,
		redeployment_cooldown_ticks: 0,
		block_count: 1,
		block_target_filter: BlockTargetFilter::GroundOnly,
		combat: combat(100, 0),
		attack: inert_attack(),
		skill: None,
		on_deploy: Vec::new(),
		on_death: Vec::new(),
		reached_mechanics: Vec::new(),
	}
}

fn summon_template(id: SummonTemplateId, key: &str, faction: Faction) -> SummonTemplate {
	SummonTemplate {
		id,
		key: key.to_owned(),
		faction,
		cell_occupancy: SummonCellOccupancy::Exclusive,
		block_count: 1,
		block_target_filter: BlockTargetFilter::GroundOnly,
		targetability: Targetability::All,
		combat: combat(100, 0),
		attack: inert_attack(),
		lifetime_ticks: None,
		reached_mechanics: Vec::new(),
	}
}

fn stored_attack_replacement_skill(
	initial_sp: u32,
	max_charges: u16,
	range: Vec<RangeOffset>,
	hp_ratio_comparison: StoredAttackHpRatioComparison,
	heal_multiplier: ExpectedValue,
) -> SkillFacts {
	SkillFacts {
		id: MechanicId::new("stored_attack_replacement"),
		activation: SkillActivationRule::Auto,
		initial_sp,
		sp_cost: 1,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks: 100,
		},
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: Some(StoredAttackReplacementFacts {
			range,
			relationship: StoredAttackTargetRelationship::Ally,
			require_healable: true,
			target_filter: NormalAttackTargetFilter::GroundAndAir,
			include_source: true,
			hp_ratio_comparison,
			hp_ratio_threshold: ExpectedValue::from_ratio(1, 2).expect("valid threshold"),
			target_order: StoredAttackTargetOrder::LowestHpRatio,
			max_charges,
			delivery_delay_ticks: 0,
			delivery: StoredAttackReplacementDelivery::SourceStatScaledHealing {
				stat: StatKind::Attack,
				multiplier: heal_multiplier,
			},
			invalidation: StoredAttackReservationInvalidationPolicy {
				target_death: StoredAttackReservationInvalidationAction::ConsumeSpentSp,
				target_disappeared: StoredAttackReservationInvalidationAction::ConsumeSpentSp,
				target_out_of_range: StoredAttackReservationInvalidationAction::ConsumeSpentSp,
				target_generation_changed:
					StoredAttackReservationInvalidationAction::ConsumeSpentSp,
			},
		}),
		effects: Vec::new(),
	}
}

fn stored_attack_replacement_mut(skill: &mut SkillFacts) -> &mut StoredAttackReplacementFacts {
	skill
		.stored_attack_replacement
		.as_mut()
		.expect("test skill has stored-attack replacement facts")
}

fn self_damage(mechanic: &str, amount: u32) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Damage {
			amount: value(amount),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}
}

fn inert_replacement_source(
	id: OperatorInstanceId,
	attack: u32,
	first_delay_ticks: u64,
	interval_ticks: u64,
	skill: SkillFacts,
) -> OperatorBuildFacts {
	let mut build: OperatorBuildFacts = operator_build(id, 0, "stored_attack_source");
	build.combat.attack = value(attack);
	build.attack = AttackProfile {
		first_delay_ticks,
		interval_ticks,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 0,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::None,
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	build.skill = Some(skill);
	build
}

fn discardable_auto_skill(on_discard: Vec<TaggedEffect>) -> SkillFacts {
	SkillFacts {
		id: MechanicId::new("discardable_auto_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 1,
		sp_cost: 1,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks: 2,
		},
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: None,
		manual_discard: Some(SkillManualDiscardPolicy {
			activation_limit: SkillManualDiscardActivationLimit::Unlimited,
			allow_while_dozing: false,
			on_discard,
		}),
		stored_attack_replacement: None,
		effects: Vec::new(),
	}
}

#[test]
fn simultaneous_deployment_limit_blocks_an_additional_operator() {
	let first = OperatorInstanceId::new(1);
	let second = OperatorInstanceId::new(2);
	let mut facts = base_facts(100);
	facts.deployment.simultaneous_limit = 1;
	facts.dp = ExpectedDpRules {
		initial: 0,
		maximum: 0,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(first, 0, "first"),
			operator_build(second, 0, "second"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: first,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: second,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("deployment limit is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.action_audit()[0].executed_at,
		Some(SimTick::new(0))
	);
	assert_eq!(assessment.action_audit()[1].executed_at, None);
	assert_eq!(
		assessment.action_audit()[1].status,
		ActionAuditStatus::SkippedNotReady
	);
}

#[test]
fn redeployment_waits_for_cooldown_and_uses_progressive_costs_and_refunds() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(20));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 30,
		maximum: 30,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	facts.deployment.simultaneous_limit = 1;
	let mut build = operator_build(operator, 5, "redeployed");
	build.redeployment_cooldown_ticks = 4;
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(1),
				operator,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(6),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(6),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("standard redeployment is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment
			.action_audit()
			.iter()
			.map(|action| action.executed_at)
			.collect::<Vec<_>>(),
		vec![
			Some(SimTick::new(0)),
			Some(SimTick::new(1)),
			Some(SimTick::new(5)),
			Some(SimTick::new(6)),
			Some(SimTick::new(10)),
		]
	);
	assert_eq!(
		assessment
			.dp_audit()
			.entries
			.iter()
			.map(|entry| (entry.kind, entry.amount))
			.collect::<Vec<_>>(),
		vec![
			(DpAuditKind::Spent, 5),
			(DpAuditKind::Refunded, 2),
			(DpAuditKind::Spent, 7),
			(DpAuditKind::Refunded, 3),
			(DpAuditKind::Spent, 10),
		]
	);
	assert_eq!(assessment.dp_audit().final_amount, 13);
}

#[test]
fn operator_death_starts_cooldown_without_refunding_dp() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(10));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 20,
		maximum: 20,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("kill_operator"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Death,
		},
	}];
	let mut build = operator_build(operator, 5, "fragile");
	build.redeployment_cooldown_ticks = 4;
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("death redeployment is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.action_audit()[1].executed_at,
		Some(SimTick::new(5))
	);
	assert_eq!(
		assessment
			.dp_audit()
			.entries
			.iter()
			.map(|entry| (entry.kind, entry.amount))
			.collect::<Vec<_>>(),
		vec![(DpAuditKind::Spent, 5), (DpAuditKind::Spent, 7)]
	);
	assert_eq!(assessment.dp_audit().final_amount, 8);
	let operator_report = &assessment.operators()[&operator];
	assert_eq!(operator_report.deployed_at, Some(SimTick::new(5)));
	assert_eq!(operator_report.death_at, None);
	assert_eq!(operator_report.remaining_hp, value(100));
}

/// An automatic, instantaneous skill that grants `amount` DP each time its time-charged SP fills.
fn dp_grant_skill(sp_cost: u32, interval_ticks: u64, amount: u32) -> SkillFacts {
	SkillFacts {
		id: MechanicId::new("dp_grant_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks,
		},
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("dp_grant"),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::GainDp { amount },
		}],
	}
}

/// Every DP change as `(tick, kind, credited or spent amount, balance after)`.
fn dp_changes(assessment: &ExpectedValueAssessment) -> Vec<(u64, DpAuditKind, u32, u32)> {
	assessment
		.dp_audit()
		.entries
		.iter()
		.map(|entry| (entry.at.get(), entry.kind, entry.amount, entry.after))
		.collect()
}

#[test]
fn automatic_dp_grant_deploys_a_waiting_operator_in_the_same_tick() {
	let granter = OperatorInstanceId::new(1);
	let waiting = OperatorInstanceId::new(2);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(10));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 5,
		maximum: 99,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let mut granter_build = operator_build(granter, 5, "dp_granter");
	granter_build.skill = Some(dp_grant_skill(3, 2, 9));
	let guide = MaterializedGuide {
		builds: vec![granter_build, operator_build(waiting, 9, "dp_waiter")],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: granter,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: waiting,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("automatic DP grants are supported");

	// The job's manual skill-use setting leaves the granter's skill off, which must not matter to
	// an automatic skill.
	let assessment = replay_expected(
		&battle,
		MaterializedGuidePolicy::new(BTreeMap::from([(granter, 0), (waiting, 0)])),
	);

	// Natural regeneration is off, so only the grant can pay for the second deployment. It lands
	// when the third SP pulse fills the skill, and the waiting deployment takes it at once.
	assert_eq!(
		assessment.action_audit()[1].status,
		ActionAuditStatus::ExecutedAfterDpWait
	);
	assert_eq!(
		assessment.action_audit()[1].executed_at,
		Some(SimTick::new(6))
	);
	assert_eq!(
		dp_changes(&assessment),
		vec![
			(0, DpAuditKind::Spent, 5, 0),
			(6, DpAuditKind::Granted, 9, 9),
			(6, DpAuditKind::Spent, 9, 0),
		]
	);
	assert_eq!(
		assessment.operators()[&waiting].deployed_at,
		Some(SimTick::new(6))
	);
}

#[test]
fn automatic_dp_grant_repeats_after_each_recharge_and_stops_at_the_maximum() {
	let granter = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(25));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 5,
		maximum: 20,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let mut build = operator_build(granter, 5, "dp_granter");
	build.skill = Some(dp_grant_skill(3, 2, 9));
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: granter,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("automatic DP grants are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	// Each activation spends the full charge, and charging restarts when the instantaneous skill
	// ends, so the skill fires every `sp_cost` pulses. Grants past the maximum credit only what
	// fits, and a grant at the maximum credits nothing.
	assert_eq!(
		dp_changes(&assessment),
		vec![
			(0, DpAuditKind::Spent, 5, 0),
			(6, DpAuditKind::Granted, 9, 9),
			(12, DpAuditKind::Granted, 9, 18),
			(18, DpAuditKind::Granted, 2, 20),
			(24, DpAuditKind::Granted, 0, 20),
		]
	);
	assert_eq!(assessment.dp_audit().final_amount, 20);
}

#[test]
fn retired_deployment_grants_no_dp_and_redeployment_restarts_the_charge() {
	let granter = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(14));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 10,
		maximum: 99,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let mut build = operator_build(granter, 2, "dp_granter");
	build.skill = Some(dp_grant_skill(3, 2, 9));
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: granter,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(5),
				operator: granter,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(5),
				operator: granter,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("automatic DP grants are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	// The first episode reaches two of three SP and retreats one tick before its grant. Its pulses
	// from tick 6 on belong to the retired deployment, and any of them reaching the new episode
	// would fill it early. The redeployment instead recharges from its initial SP alone and grants
	// at tick 11, while cost, refund and redeployment cost stay the standard progression.
	assert_eq!(
		dp_changes(&assessment),
		vec![
			(0, DpAuditKind::Spent, 2, 8),
			(5, DpAuditKind::Refunded, 1, 9),
			(5, DpAuditKind::Spent, 3, 6),
			(11, DpAuditKind::Granted, 9, 15),
		]
	);
	assert_eq!(
		assessment.action_audit()[2].executed_at,
		Some(SimTick::new(5))
	);
}

#[test]
fn dead_granter_grants_no_dp() {
	let granter = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(14));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 10,
		maximum: 99,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(5),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("kill_granter"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Death,
		},
	}];
	let mut build = operator_build(granter, 2, "dp_granter");
	build.skill = Some(dp_grant_skill(3, 2, 9));
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: granter,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("automatic DP grants are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	// The granter dies one tick before its skill fills, so the charge it had built never pays out
	// and death refunds nothing.
	assert_eq!(
		assessment.operators()[&granter].death_at,
		Some(SimTick::new(5))
	);
	assert_eq!(dp_changes(&assessment), vec![(0, DpAuditKind::Spent, 2, 8)]);
}

#[test]
fn deployment_point_gain_is_a_positive_whole_grant_to_its_source() {
	let granter = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.paths = vec![path(1, 100, 100)];
	let gain = |mechanic: &str, target: EffectTarget, effect: ExpectedEffect| TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target,
		effect,
	};
	let mut skill = dp_grant_skill(3, 2, 9);
	skill.effects = vec![
		gain(
			"zero_grant",
			EffectTarget::SelfActor,
			ExpectedEffect::GainDp { amount: 0 },
		),
		gain(
			"team_grant",
			EffectTarget::AllAllies,
			ExpectedEffect::GainDp { amount: 9 },
		),
	];
	let mut build = operator_build(granter, 0, "dp_granter");
	build.skill = Some(skill);
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: Vec::new(),
		reached_mechanics: Vec::new(),
	};

	let error =
		compile_battle(&facts, &guide).expect_err("malformed DP grants must fail preflight");
	let details: Vec<(&str, &str)> = error
		.issues()
		.iter()
		.filter_map(|issue| match issue {
			BattleCompileIssue::InvalidReplayPrimitive { mechanic, detail } => {
				Some((mechanic.as_str(), detail.as_str()))
			}
			_ => None,
		})
		.collect();
	assert_eq!(
		details,
		vec![
			(
				"team_grant",
				"deployment-point gain credits the battle once and must target its source"
			),
			("zero_grant", "deployment-point gain must be positive"),
		]
	);

	// DP is whole, so a grant that only lands with some probability has no expected-value form,
	// even when its expectation happens to be a whole five points.
	let mut expected_skill = dp_grant_skill(3, 2, 9);
	expected_skill.effects = vec![gain(
		"coin_flip_grant",
		EffectTarget::SelfActor,
		ExpectedEffect::Expected {
			probability: ExpectedValue::from_ratio(1, 2).expect("valid probability"),
			effect: Box::new(ExpectedEffect::GainDp { amount: 10 }),
		},
	)];
	let mut expected_build = operator_build(granter, 0, "dp_granter");
	expected_build.skill = Some(expected_skill);
	let error = compile_battle(
		&facts,
		&MaterializedGuide {
			builds: vec![expected_build],
			actions: Vec::new(),
			reached_mechanics: Vec::new(),
		},
	)
	.expect_err("probabilistic DP grants must fail preflight");
	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::ExpectedDiscreteEffect { mechanic }
			if mechanic.as_str() == "coin_flip_grant"
	)));
}

#[test]
fn same_tick_retreat_discards_queued_on_deploy_effects() {
	let operator = OperatorInstanceId::new(1);
	let enemy = EnemyInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 0,
		maximum: 0,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let mut build = operator_build(operator, 0, "retreated_deployer");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("queued_on_deploy_damage"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Damage {
			amount: value(10),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(0),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("on-deploy damage is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment
			.action_audit()
			.iter()
			.map(|action| action.executed_at)
			.collect::<Vec<_>>(),
		vec![Some(SimTick::new(0)), Some(SimTick::new(0))]
	);
	assert_eq!(
		assessment.enemies()[&enemy].expected_damage_taken,
		ExpectedValue::ZERO,
		"an effect from a retired deployment must not reach another actor"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn zero_cooldown_redeployment_does_not_skip_on_death_effects() {
	let operator = OperatorInstanceId::new(1);
	let enemy = EnemyInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 0,
		maximum: 0,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("kill_operator"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Death,
		},
	}];
	let mut build = operator_build(operator, 0, "reviving");
	build.on_death = vec![TaggedEffect {
		mechanic: MechanicId::new("on_death_damage"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Damage {
			amount: value(10),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("zero-cooldown death is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&enemy].expected_damage_taken,
		value(10)
	);
	assert_eq!(
		assessment.action_audit()[1].executed_at,
		Some(SimTick::new(1))
	);
	assert_eq!(
		assessment.operators()[&operator].deployed_at,
		Some(SimTick::new(1))
	);
	assert_eq!(assessment.operators()[&operator].death_at, None);
}

#[test]
fn stale_attack_from_a_retired_episode_cannot_hit_after_redeployment() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(10);
	facts.end_at = Some(SimTick::new(20));
	facts.dp = ExpectedDpRules {
		initial: 0,
		maximum: 0,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let mut build = operator_build(operator, 0, "episode_attacker");
	build.redeployment_cooldown_ticks = 2;
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 10,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(1),
				operator,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("attack episodes are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(13))
	);
}

#[test]
fn redeployment_restarts_periodic_sp_and_retires_the_old_schedule() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(10);
	facts.end_at = Some(SimTick::new(12));
	facts.paths = vec![path(1, 100, 100)];
	facts.dp = ExpectedDpRules {
		initial: 0,
		maximum: 0,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	let mut build = operator_build(operator, 0, "episode_caster");
	build.redeployment_cooldown_ticks = 2;
	build.skill = Some(SkillFacts {
		id: MechanicId::new("episode_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost: 2,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks: 5,
		},
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: value(1),
			interval_ticks: 5,
		}],
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("episode_skill_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(10),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	});
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(1),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("SP episodes are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(8))
	);
}

#[test]
fn base_and_fractional_periodic_sp_sources_compose_at_millionth_scale() {
	let operator: OperatorInstanceId = OperatorInstanceId::new(1);
	let mut facts: ArkBattleFacts = base_facts(10);
	facts.end_at = Some(SimTick::new(12));
	facts.paths = vec![path(1, 100, 100)];
	let mut build: OperatorBuildFacts = operator_build(operator, 0, "periodic_caster");
	build.skill = Some(SkillFacts {
		id: MechanicId::new("periodic_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost: 13,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks: 1,
		},
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: ExpectedValue::from_scaled(300_000),
			interval_ticks: 1,
		}],
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("periodic_skill_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(10),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	});
	let guide: MaterializedGuide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("periodic SP sources are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(10))
	);
}

#[test]
fn natural_sp_recovery_does_not_charge_attack_recovery_skills() {
	let operator: OperatorInstanceId = OperatorInstanceId::new(1);
	let mut facts: ArkBattleFacts = base_facts(10);
	facts.end_at = Some(SimTick::new(12));
	facts.paths = vec![path(1, 100, 100)];
	let mut build: OperatorBuildFacts = operator_build(operator, 0, "attack_recovery_caster");
	build.skill = Some(SkillFacts {
		id: MechanicId::new("attack_recovery_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost: 1,
		charge: SpChargeRule::Attack { amount: 1 },
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: value(1),
			interval_ticks: 1,
		}],
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("attack_recovery_skill_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(10),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	});
	let guide: MaterializedGuide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("attack recovery facts are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].death_at,
		None
	);
}

#[test]
fn periodic_sp_pauses_during_skill_and_restarts_from_skill_end() {
	let operator: OperatorInstanceId = OperatorInstanceId::new(1);
	let mut facts: ArkBattleFacts = base_facts(20);
	facts.end_at = Some(SimTick::new(8));
	facts.paths = vec![path(1, 100, 100)];
	let mut build: OperatorBuildFacts = operator_build(operator, 0, "periodic_caster");
	build.skill = Some(SkillFacts {
		id: MechanicId::new("periodic_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost: 2,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks: 2,
		},
		periodic_sp_sources: vec![PeriodicSpSource {
			amount: value(1),
			interval_ticks: 2,
		}],
		active_duration_ticks: Some(3),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("periodic_skill_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(10),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	});
	let guide: MaterializedGuide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("periodic SP sources are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(7))
	);
}

#[test]
fn skill_button_discards_an_active_auto_skill_then_recharge_restarts_it() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.paths = vec![path(1, 100, 100)];
	let mut build = operator_build(operator, 0, "discardable_auto_caster");
	build.skill = Some(discardable_auto_skill(vec![TaggedEffect {
		mechanic: MechanicId::new("manual_discard_damage"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Damage {
			amount: value(10),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}]));
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("manual discard policy is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		value(10)
	);
	assert_eq!(
		assessment.action_audit()[1].executed_at,
		Some(SimTick::new(1))
	);
	let activation_ticks: Vec<SimTick> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled }
				if matches!(
					scheduled.event(),
					ExpectedEvent::SkillActivation { operator: activated, .. }
						if *activated == operator
				) =>
			{
				Some(scheduled.at())
			}
			_ => None,
		})
		.collect();
	assert_eq!(activation_ticks, vec![SimTick::new(0), SimTick::new(3)]);
	assert!(assessment.trace().entries().iter().any(|entry| matches!(
		entry,
		TraceEntry::Command {
			command: ExpectedCommand::DiscardSkill { operator: discarded, .. },
			..
		} if *discarded == operator
	)));
	assert!(assessment.trace().entries().iter().any(|entry| matches!(
		entry,
		TraceEntry::Event { scheduled }
			if matches!(
				scheduled.event(),
				ExpectedEvent::SkillEnded {
					operator: ended,
					reason: SkillEndReason::ManualDiscard,
					..
				} if *ended == operator
			)
	)));
	assert!(SkillManualDiscardActivationLimit::FirstNActivations(1).allows(1));
	assert!(!SkillManualDiscardActivationLimit::FirstNActivations(1).allows(2));
}

#[test]
fn skill_button_does_not_start_an_inactive_auto_skill() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(2));
	facts.paths = vec![path(1, 100, 100)];
	let mut build = operator_build(operator, 0, "inactive_auto_caster");
	let mut skill = discardable_auto_skill(Vec::new());
	skill.initial_sp = 0;
	skill.charge = SpChargeRule::None;
	build.skill = Some(skill);
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("inactive auto skill is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.action_audit()[1].executed_at, None);
	assert!(!assessment.trace().entries().iter().any(|entry| matches!(
		entry,
		TraceEntry::Command {
			command: ExpectedCommand::ActivateSkill { operator: activated, .. },
			..
		} if *activated == operator
	)));
}

#[test]
fn death_and_retreat_do_not_execute_manual_discard_effects() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(3));
	facts.paths = vec![path(1, 100, 100)];
	let mut build = operator_build(operator, 0, "retreating_auto_caster");
	build.skill = Some(discardable_auto_skill(vec![TaggedEffect {
		mechanic: MechanicId::new("manual_discard_only_damage"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Damage {
			amount: value(100),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}]));
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("manual discard policy is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		ExpectedValue::ZERO
	);
	assert!(!assessment.trace().entries().iter().any(|entry| matches!(
		entry,
		TraceEntry::Event { scheduled }
			if matches!(scheduled.event(), ExpectedEvent::SkillDiscarded { .. })
	)));

	let mut dying_build = operator_build(operator, 0, "dying_auto_caster");
	dying_build.skill = Some(discardable_auto_skill(vec![TaggedEffect {
		mechanic: MechanicId::new("manual_discard_only_damage"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Damage {
			amount: value(100),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}]));
	dying_build.on_deploy.push(TaggedEffect {
		mechanic: MechanicId::new("die_after_skill_start"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Death,
	});
	let death_guide = MaterializedGuide {
		builds: vec![dying_build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let death_battle =
		compile_battle(&facts, &death_guide).expect("manual discard policy is supported");
	let death_assessment = replay_expected(&death_battle, MaterializedGuidePolicy::default());
	assert_eq!(
		death_assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		ExpectedValue::ZERO
	);
	assert!(!death_assessment
		.trace()
		.entries()
		.iter()
		.any(|entry| matches!(
			entry,
			TraceEntry::Event { scheduled }
				if matches!(scheduled.event(), ExpectedEvent::SkillDiscarded { .. })
		)));
}

#[test]
fn expected_damage_is_fixed_point_and_ordered_deploy_waits_for_dp() {
	let operator = OperatorInstanceId::new(1);
	let chance = ExpectedValue::from_ratio(1, 2).expect("valid probability");
	let guide = MaterializedGuide {
		builds: vec![OperatorBuildFacts {
			id: operator,
			operator_key: "operator".to_owned(),
			classification: OperatorClassificationFacts::default(),
			on_battle_start: Vec::new(),
			deployment_cost: 2,
			redeployment_cooldown_ticks: 0,
			block_count: 1,
			block_target_filter: BlockTargetFilter::GroundOnly,
			combat: combat(100, 0),
			attack: AttackProfile {
				first_delay_ticks: 1,
				interval_ticks: 1,
				range: vec![RangeOffset::new(0, 0)],
				target_count: 1,
				target_filter: NormalAttackTargetFilter::GroundAndAir,
				action: NormalAttackAction::Damage {
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				},
				pattern: AttackPattern::Single,
				extra_effects: vec![TaggedEffect {
					mechanic: MechanicId::new("coin_flip_damage"),
					target: EffectTarget::CurrentTargets,
					effect: ExpectedEffect::Expected {
						probability: chance,
						effect: Box::new(ExpectedEffect::Damage {
							amount: value(100),
							damage_type: DamageType::True,
							delivery: DamageDelivery::Normal,
						}),
					},
				}],
			},
			skill: None,
			on_deploy: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::SpeedUp {
				requested_at: SimTick::new(0),
			},
			MaterializedGuideAction::SkillDaemon {
				requested_at: SimTick::new(0),
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&base_facts(100), &guide).expect("supported battle");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let probability = evaluate_defensive_probability(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	let enemy = &assessment.enemies()[&EnemyInstanceId::new(1)];
	assert_eq!(enemy.expected_damage_taken, value(100));
	assert_eq!(enemy.remaining_hp, ExpectedValue::ZERO);
	assert_eq!(
		assessment.action_audit()[0].status,
		ActionAuditStatus::ExecutedAfterDpWait
	);
	assert_eq!(
		assessment.action_audit()[0].executed_at,
		Some(SimTick::new(2))
	);
	assert_eq!(
		assessment.action_audit()[1].status,
		ActionAuditStatus::ExecutorOnly
	);
	assert_eq!(
		assessment.action_audit()[2].status,
		ActionAuditStatus::ExecutorOnly
	);
	let commands: Vec<_> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Command { command, .. } => Some(command),
			TraceEntry::Event { .. } | TraceEntry::Choice { .. } => None,
		})
		.collect();
	assert_eq!(
		commands.len(),
		1,
		"SpeedUp and SkillDaemon are executor-only"
	);
	assert!(assessment.issues().is_empty());
	assert!(probability.verified());
	assert_eq!(probability.success_probability(), Some(1.0));
	assert_eq!(probability.success_probability_millionths(), 1_000_000);
	assert_eq!(
		probability.peak_branches(),
		1,
		"outgoing damage RNG must stay on the deterministic expected-damage lane"
	);
}

#[test]
fn outgoing_expected_physical_damage_applies_defense_before_probability() {
	let operator = OperatorInstanceId::new(1);
	let probability = ExpectedValue::from_ratio(1, 2).expect("valid probability");
	let mut build = operator_build(operator, 0, "expected_physical");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("expected_physical"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Expected {
			probability,
			effect: Box::new(ExpectedEffect::Damage {
				amount: value(100),
				damage_type: DamageType::Physical,
				delivery: DamageDelivery::Normal,
			}),
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.enemies[0].combat.defense = value(80);
	let battle = compile_battle(&facts, &guide).expect("expected physical damage is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &assessment.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(enemy.expected_damage_taken, value(10));
	assert_eq!(enemy.remaining_hp, value(90));
	assert!(assessment.issues().is_empty());
}

#[test]
fn outgoing_expected_damage_caps_the_mean_effective_damage_at_remaining_hp() {
	let operator = OperatorInstanceId::new(1);
	let probability = ExpectedValue::from_ratio(1, 2).expect("valid probability");
	let mut build = operator_build(operator, 0, "expected_overkill");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("expected_overkill"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Expected {
			probability,
			effect: Box::new(ExpectedEffect::Damage {
				amount: value(200),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			}),
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&base_facts(100), &guide).expect("expected overkill is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &assessment.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	assert_eq!(enemy.expected_damage_taken, value(100));
	assert_eq!(enemy.remaining_hp, ExpectedValue::ZERO);
	assert!(assessment.issues().is_empty());
}

#[test]
fn timed_hit_count_modifier_applies_defense_per_hit_and_expires() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "multi_hit");
	build.combat.attack = value(100);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 2,
		range: vec![RangeOffset::new(0, -1)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::Physical,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("four_hits"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::HitCount(HitCountModifier {
			delta: 3,
			duration_ticks: Some(2),
		}),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(1_000);
	facts.end_at = Some(SimTick::new(4));
	facts.enemies[0].combat.defense = value(80);
	let battle = compile_battle(&facts, &guide).expect("multi-hit modifier is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &assessment.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(enemy.expected_damage_taken, value(100));
	assert_eq!(enemy.remaining_hp, value(900));
	assert!(assessment.issues().is_empty());
}

#[test]
fn full_replay_reports_probability_when_victory_requires_a_dodge() {
	let operator = OperatorInstanceId::new(1);
	let hit_probability = ExpectedValue::from_ratio(3, 4).expect("valid probability");
	let mut build: OperatorBuildFacts = operator_build(operator, 0, "dodge_blocker");
	build.attack = AttackProfile {
		first_delay_ticks: 6,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	build.combat.attack = value(100);
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(100);
	facts.end_at = None;
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 4,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::None,
		pattern: AttackPattern::Single,
		extra_effects: vec![TaggedEffect {
			mechanic: MechanicId::new("operator_dodge"),
			target: EffectTarget::CurrentTargets,
			effect: ExpectedEffect::Expected {
				probability: hit_probability,
				effect: Box::new(ExpectedEffect::Damage {
					amount: value(100),
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				}),
			},
		}],
	};
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide,
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [4; 32], "dodge-variant");
	let probability = output
		.report
		.defensive_probability
		.expect("supported replay has a probability audit");

	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedVictory,
		"the ordinary replay keeps deterministic expected incoming damage"
	);
	assert!(probability.verified);
	assert_eq!(probability.success_probability, Some(0.25));
	assert_eq!(probability.covered_probability_mass, Some(1.0));
	assert_eq!(probability.success_probability_millionths, 250_000);
	assert_eq!(probability.covered_probability_millionths, 1_000_000);
	assert_eq!(probability.peak_branches, 2);
	assert!(probability.issues.is_empty());
}

#[test]
fn defensive_probability_fails_closed_for_multiple_friendly_targets() {
	let first = OperatorInstanceId::new(1);
	let second = OperatorInstanceId::new(2);
	let chance = ExpectedValue::from_ratio(1, 2).expect("valid probability");
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(first, 0, "first"),
			operator_build(second, 0, "second"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: first,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: second,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(100);
	facts.stage_effects.push(ScheduledStageEffect {
		at: SimTick::new(0),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("shared_random_damage"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Expected {
				probability: chance,
				effect: Box::new(ExpectedEffect::Damage {
					amount: value(1),
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				}),
			},
		},
	});
	let battle = compile_battle(&facts, &guide).expect("damage wrapper is supported");

	let probability = evaluate_defensive_probability(&battle, MaterializedGuidePolicy::default());

	assert!(!probability.verified());
	assert_eq!(probability.covered_probability_millionths(), 0);
	assert!(probability
		.issues()
		.iter()
		.any(|issue| issue.contains("exactly one live friendly damage target")));
}

#[test]
fn no_hard_cap_allows_blocking_past_the_unblocked_horizon_to_end_in_victory() {
	let operator = OperatorInstanceId::new(1);
	let mut build: OperatorBuildFacts = operator_build(operator, 0, "blocker");
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 10,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(30);
	facts.end_at = None;
	let battle = compile_battle(&facts, &guide).expect("blocking is supported without a hard cap");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &assessment.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	assert_eq!(enemy.deadline_at, Some(SimTick::new(23)));
	assert_eq!(enemy.death_at, Some(SimTick::new(31)));
	assert_eq!(
		assessment.block_audit(),
		&[BlockAuditEntry {
			enemy: EnemyInstanceId::new(1),
			blocker: BattleActorId::Operator(operator),
			started_at: SimTick::new(3),
			ended_at: Some(SimTick::new(31)),
			end_reason: Some(BlockEndReason::EnemyDeath),
		}]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn no_hard_cap_resumes_movement_after_blocker_death_and_ends_in_a_leak() {
	let operator = OperatorInstanceId::new(1);
	let mut build: OperatorBuildFacts = operator_build(operator, 0, "temporary_blocker");
	build.combat.max_hp = value(10);
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(100);
	facts.end_at = None;
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 30,
		interval_ticks: 30,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::Physical,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("blocking is supported without a hard cap");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &assessment.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(
		assessment.status(),
		&ExpectedTerminalStatus::GoalLeak {
			enemies: vec![EnemyInstanceId::new(1)]
		}
	);
	assert_eq!(enemy.deadline_at, Some(SimTick::new(23)));
	assert_eq!(enemy.leaked_at, Some(SimTick::new(50)));
	assert_eq!(
		assessment.block_audit(),
		&[BlockAuditEntry {
			enemy: EnemyInstanceId::new(1),
			blocker: BattleActorId::Operator(operator),
			started_at: SimTick::new(3),
			ended_at: Some(SimTick::new(30)),
			end_reason: Some(BlockEndReason::BlockerDeath),
		}]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn counterfactual_replay_continues_later_waves_after_first_leak() {
	let first_enemy = EnemyInstanceId::new(1);
	let second_enemy = EnemyInstanceId::new(2);
	let mut facts: ArkBattleFacts = base_facts(100);
	facts.end_at = None;
	facts.waves = vec![
		one_wave(first_enemy, PathId::new(1)),
		BattleWave {
			id: WaveId::new(2),
			start: WaveStartWindow {
				earliest: RelativeBattleTime {
					anchor: BattleTimeAnchor::WaveCleared(WaveId::new(1)),
					offset_ticks: 7,
				},
				latest: None,
			},
			spawns: vec![EnemySpawnFacts {
				id: second_enemy,
				enemy_key: "enemy".to_owned(),
				after_wave_start_ticks: 0,
				path: PathId::new(1),
				facing: Facing::Right,
			}],
		},
	];
	let battle = compile_battle(&facts, &MaterializedGuide::default())
		.expect("two causally ordered waves are supported");

	let normal = replay_expected(&battle, MaterializedGuidePolicy::default());
	assert_eq!(
		normal.status(),
		&ExpectedTerminalStatus::GoalLeak {
			enemies: vec![first_enemy],
		}
	);
	assert_eq!(normal.enemies()[&second_enemy].spawned_at, None);

	let counterfactual = replay_expected_to_resolution(&battle, MaterializedGuidePolicy::default());
	assert_eq!(
		counterfactual.status(),
		&ExpectedTerminalStatus::GoalLeak {
			enemies: vec![first_enemy, second_enemy],
		}
	);
	assert!(counterfactual.enemies()[&second_enemy].spawned_at.is_some());
	assert!(counterfactual.enemies()[&second_enemy].leaked_at.is_some());
	let first_wave_cleared =
		counterfactual.anchors()[&BattleTimeAnchor::WaveCleared(WaveId::new(1))];
	let second_wave_started =
		counterfactual.anchors()[&BattleTimeAnchor::WaveStarted(WaveId::new(2))];
	assert_eq!(
		second_wave_started.get(),
		first_wave_cleared.get() + 7,
		"the next wave must preserve its configured delay after the causal wave-clear anchor"
	);
}

#[test]
fn no_hard_cap_proves_an_inert_all_blocked_stalemate_without_hitting_step_limit() {
	let operator = OperatorInstanceId::new(1);
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "inert_blocker")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = None;
	let battle = compile_battle(&facts, &guide).expect("inert blocking is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.status(),
		&ExpectedTerminalStatus::SimulationIncomplete
	);
	assert_eq!(
		assessment.block_audit(),
		&[BlockAuditEntry {
			enemy: EnemyInstanceId::new(1),
			blocker: BattleActorId::Operator(operator),
			started_at: SimTick::new(3),
			ended_at: None,
			end_reason: None,
		}]
	);
	assert!(assessment.trace().entries().len() < 20);
	assert!(assessment.issues().is_empty());
}

#[test]
fn ground_only_blocker_does_not_stop_an_air_enemy() {
	let operator = OperatorInstanceId::new(1);
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "ground_blocker")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(4),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = None;
	facts.enemies[0].combat.target_class = TargetClass::Air;
	let battle = compile_battle(&facts, &guide).expect("air movement is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.status(),
		&ExpectedTerminalStatus::GoalLeak {
			enemies: vec![EnemyInstanceId::new(1)]
		}
	);
	assert!(assessment.block_audit().is_empty());
	assert!(assessment.issues().is_empty());
}

#[test]
fn block_target_modifier_stops_then_releases_an_air_enemy() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "temporary_air_blocker");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("temporary_air_block"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::BlockTarget(BlockTargetModifier {
			filter: BlockTargetFilter::AirOnly,
			duration_ticks: Some(1),
		}),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(4),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(6));
	facts.enemies[0].combat.target_class = TargetClass::Air;
	let battle = compile_battle(&facts, &guide).expect("typed air blocking is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.block_audit(),
		&[BlockAuditEntry {
			enemy: EnemyInstanceId::new(1),
			blocker: BattleActorId::Operator(operator),
			started_at: SimTick::new(4),
			ended_at: Some(SimTick::new(5)),
			end_reason: Some(BlockEndReason::BlockTargetChange),
		}]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn on_deploy_block_filter_is_applied_before_initial_capture() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "air_only_on_deploy");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("air_only"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::BlockTarget(BlockTargetModifier {
			filter: BlockTargetFilter::AirOnly,
			duration_ticks: None,
		}),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(4),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	let mut air = facts.enemies[0].clone();
	air.enemy_key = "air".to_owned();
	air.combat.target_class = TargetClass::Air;
	facts.enemies.push(air);
	facts.waves[0].spawns.push(EnemySpawnFacts {
		id: EnemyInstanceId::new(2),
		enemy_key: "air".to_owned(),
		after_wave_start_ticks: 0,
		path: PathId::new(1),
		facing: Facing::Right,
	});
	let battle =
		compile_battle(&facts, &guide).expect("atomic block-filter deployment is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.block_audit().len(), 1);
	assert_eq!(assessment.block_audit()[0].enemy, EnemyInstanceId::new(2));
	assert_eq!(
		assessment.block_audit()[0].blocker,
		BattleActorId::Operator(operator)
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn freed_block_capacity_captures_the_next_enemy_on_the_same_cell() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "block_and_replace");
	build.combat.attack = value(100);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.waves[0].spawns.push(EnemySpawnFacts {
		id: EnemyInstanceId::new(2),
		enemy_key: "enemy".to_owned(),
		after_wave_start_ticks: 0,
		path: PathId::new(1),
		facing: Facing::Right,
	});
	let battle =
		compile_battle(&facts, &guide).expect("same-cell replacement blocking is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.block_audit(),
		&[
			BlockAuditEntry {
				enemy: EnemyInstanceId::new(1),
				blocker: BattleActorId::Operator(operator),
				started_at: SimTick::new(3),
				ended_at: Some(SimTick::new(4)),
				end_reason: Some(BlockEndReason::EnemyDeath),
			},
			BlockAuditEntry {
				enemy: EnemyInstanceId::new(2),
				blocker: BattleActorId::Operator(operator),
				started_at: SimTick::new(4),
				ended_at: Some(SimTick::new(5)),
				end_reason: Some(BlockEndReason::EnemyDeath),
			},
		]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn shifting_a_blocked_enemy_fills_the_freed_capacity() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "shift_and_replace");
	build.attack = AttackProfile {
		first_delay_ticks: 4,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::PowerHit {
			every: 1,
			damage_multiplier: ExpectedValue::ONE,
			shift: Some(ShiftEffect {
				direction: ShiftDirection::AwayFromGoal,
				cells: 1,
				force: 1,
			}),
		},
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.waves[0].spawns.push(EnemySpawnFacts {
		id: EnemyInstanceId::new(2),
		enemy_key: "enemy".to_owned(),
		after_wave_start_ticks: 0,
		path: PathId::new(1),
		facing: Facing::Right,
	});
	let battle = compile_battle(&facts, &guide).expect("shift replacement blocking is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.block_audit(),
		&[
			BlockAuditEntry {
				enemy: EnemyInstanceId::new(1),
				blocker: BattleActorId::Operator(operator),
				started_at: SimTick::new(3),
				ended_at: Some(SimTick::new(4)),
				end_reason: Some(BlockEndReason::Shift),
			},
			BlockAuditEntry {
				enemy: EnemyInstanceId::new(2),
				blocker: BattleActorId::Operator(operator),
				started_at: SimTick::new(4),
				ended_at: Some(SimTick::new(5)),
				end_reason: Some(BlockEndReason::BattleDeadline),
			},
		]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn blocked_enemy_attacks_its_blocker_before_other_targets_in_range() {
	let bystander = OperatorInstanceId::new(1);
	let blocker = OperatorInstanceId::new(2);
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(bystander, 0, "bystander"),
			operator_build(blocker, 0, "blocker"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: bystander,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: blocker,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.enemies[0].combat.attack = value(50);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 4,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 0), RangeOffset::new(0, 1)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("blocked targeting is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.operators()[&bystander].remaining_hp, value(100));
	assert_eq!(assessment.operators()[&blocker].remaining_hp, value(50));
	assert!(assessment.issues().is_empty());
}

#[test]
fn first_after_block_attack_rearms_for_each_block_session() {
	let first = OperatorInstanceId::new(1);
	let second = OperatorInstanceId::new(2);
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(first, 0, "first_blocker"),
			operator_build(second, 0, "second_blocker"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: first,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(5),
				operator: first,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(5),
				operator: second,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 4,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::FirstAttackAfterBlock {
			damage_multiplier: value(2),
			shelter_damage_multiplier: ExpectedValue::ONE,
		},
		extra_effects: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("first-after-block attack is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.operators()[&first].remaining_hp, value(80));
	assert_eq!(assessment.operators()[&second].remaining_hp, value(80));
	assert_eq!(assessment.block_audit().len(), 2);
	assert_eq!(
		assessment.block_audit()[0].end_reason,
		Some(BlockEndReason::Retreat)
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn first_after_block_attack_does_not_rearm_when_blocking_starts() {
	let bystander = OperatorInstanceId::new(1);
	let blocker = OperatorInstanceId::new(2);
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(bystander, 0, "entry_target"),
			operator_build(blocker, 0, "later_blocker"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: bystander,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: blocker,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 3,
		range: vec![
			RangeOffset::new(0, 0),
			RangeOffset::new(0, 1),
			RangeOffset::new(0, 2),
		],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::FirstAttackAfterBlock {
			damage_multiplier: value(2),
			shelter_damage_multiplier: ExpectedValue::ONE,
		},
		extra_effects: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("entry-armed attack is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.operators()[&bystander].remaining_hp, value(80));
	assert_eq!(
		assessment.operators()[&blocker].remaining_hp,
		value(90),
		"starting a block session must not arm another power hit"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn first_after_block_attack_multiplies_every_shelter_hit() {
	let shelter = SummonTemplateId::new(1);
	let mut template = summon_template(shelter, "shelter", Faction::Friendly);
	template.block_count = 1;
	template.targetability = Targetability::Shelter;
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(7));
	facts.summons = vec![template];
	facts.preplaced_actors = vec![PreplacedActorFacts {
		instance: 1,
		template: shelter,
		cell: GridPosition { row: 0, col: 1 },
		facing: Facing::Right,
	}];
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 4,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::FirstAttackAfterBlock {
			damage_multiplier: ExpectedValue::from_ratio(9, 5).expect("valid multiplier"),
			shelter_damage_multiplier: ExpectedValue::from_ratio(5, 2).expect("valid multiplier"),
		},
		extra_effects: Vec::new(),
	};
	let battle =
		compile_battle(&facts, &MaterializedGuide::default()).expect("shelter multiplier is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.block_audit(),
		&[BlockAuditEntry {
			enemy: EnemyInstanceId::new(1),
			blocker: BattleActorId::Summon(1),
			started_at: SimTick::new(3),
			ended_at: Some(SimTick::new(7)),
			end_reason: Some(BlockEndReason::BlockerDeath),
		}]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn axis_direct_hit_uses_one_direction_and_shelter_collision() {
	let replay = |exposed: bool| {
		let operator = OperatorInstanceId::new(1);
		let shelter = SummonTemplateId::new(1);
		let mut template = summon_template(shelter, "shelter", Faction::Friendly);
		template.block_count = 0;
		template.targetability = Targetability::Shelter;
		let mut build = operator_build(operator, 0, "axis_target");
		build.combat.attack = value(10);
		build.attack = AttackProfile {
			first_delay_ticks: 2,
			interval_ticks: 100,
			range: vec![RangeOffset::new(0, -2)],
			target_count: 1,
			target_filter: NormalAttackTargetFilter::GroundOnly,
			action: NormalAttackAction::Damage {
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
			pattern: AttackPattern::Single,
			extra_effects: Vec::new(),
		};
		if exposed {
			build.on_deploy = vec![TaggedEffect {
				mechanic: MechanicId::new("exposed_axis_target"),
				target: EffectTarget::SelfActor,
				effect: ExpectedEffect::ApplyStatus(status(
					StatusKind::Exposure,
					10,
					ExpectedValue::ONE,
				)),
			}];
		}
		let guide = MaterializedGuide {
			builds: vec![build],
			actions: vec![MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			}],
			reached_mechanics: Vec::new(),
		};
		let mut facts = base_facts(100);
		facts.end_at = Some(SimTick::new(3));
		facts.paths = vec![path(1, 100, 100)];
		facts.summons = vec![template];
		facts.preplaced_actors = vec![PreplacedActorFacts {
			instance: 1,
			template: shelter,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}];
		facts.enemies[0].combat.attack = value(10);
		facts.enemies[0].on_spawn = vec![TaggedEffect {
			mechanic: MechanicId::new("axis_direct_hit"),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
				initial_cooldown_ticks: 1,
				cooldown_ticks: 100,
				windup_ticks: 0,
				skill_duration_ticks: 0,
				projectiles: 3,
				damage_multiplier: ExpectedValue::ONE,
				damage_type: DamageType::True,
				delivery: DamageDelivery::DirectHit {
					ignore_defense: true,
					ignore_shelter: true,
				},
				status: Some(status(StatusKind::Stun, 3, ExpectedValue::ONE)),
				requires_unblocked: true,
				stealth_recovery_ticks: None,
			}),
		}];
		let battle = compile_battle(&facts, &guide).expect("axis direct hit is typed");
		replay_expected(&battle, MaterializedGuidePolicy::default())
	};

	let hidden = replay(false);
	let exposed = replay(true);
	let axis_events = |assessment: &ExpectedValueAssessment| {
		assessment
			.trace()
			.entries()
			.iter()
			.filter(|entry| {
				matches!(
					entry,
					TraceEntry::Event { scheduled }
						if matches!(
							scheduled.event(),
							ExpectedEvent::AxisDirectHit {
								phase: AxisDirectHitPhase::Ready,
								..
							}
						)
				)
			})
			.count()
	};

	assert_eq!(axis_events(&hidden), 3, "ready skill retries every tick");
	assert_eq!(
		axis_events(&exposed),
		1,
		"Exposure supplies one trigger direction"
	);
	assert_eq!(
		exposed.operators()[&OperatorInstanceId::new(1)].remaining_hp,
		value(100),
		"the shelter still catches the projectile"
	);
	assert_eq!(
		exposed.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(90),
		"the exposed operator was not hit or stunned behind the shelter"
	);
	assert!(hidden.issues().is_empty());
	assert!(exposed.issues().is_empty());
}

#[test]
fn axis_direct_hit_applies_three_projectiles_and_optional_stun() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "axis_target");
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 2,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, -2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(3));
	facts.paths = vec![path(1, 100, 100)];
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("axis_direct_hit"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
			initial_cooldown_ticks: 1,
			cooldown_ticks: 100,
			windup_ticks: 0,
			skill_duration_ticks: 0,
			projectiles: 3,
			damage_multiplier: ExpectedValue::ONE,
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
			status: Some(status(StatusKind::Stun, 3, ExpectedValue::ONE)),
			requires_unblocked: true,
			stealth_recovery_ticks: None,
		}),
	}];
	let battle = compile_battle(&facts, &guide).expect("axis projectiles are typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.operators()[&operator].remaining_hp, value(70));
	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(100),
		"Stun suppresses the operator attack scheduled for tick 2"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn axis_direct_hit_rechecks_collision_after_each_projectile() {
	let operator = OperatorInstanceId::new(1);
	let shelter = SummonTemplateId::new(1);
	let mut template = summon_template(shelter, "fragile_shelter", Faction::Friendly);
	template.block_count = 0;
	template.targetability = Targetability::Shelter;
	template.combat.max_hp = value(10);
	let mut build = operator_build(operator, 0, "exposed_rear_target");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("exposed_rear_target"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::ApplyStatus(status(StatusKind::Exposure, 10, ExpectedValue::ONE)),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(2));
	facts.paths = vec![path(1, 100, 100)];
	facts.summons = vec![template];
	facts.preplaced_actors = vec![PreplacedActorFacts {
		instance: 1,
		template: shelter,
		cell: GridPosition { row: 0, col: 1 },
		facing: Facing::Right,
	}];
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("rapid_axis_direct_hit"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
			initial_cooldown_ticks: 1,
			cooldown_ticks: 100,
			windup_ticks: 0,
			skill_duration_ticks: 0,
			projectiles: 3,
			damage_multiplier: ExpectedValue::ONE,
			damage_type: DamageType::True,
			delivery: DamageDelivery::DirectHit {
				ignore_defense: true,
				ignore_shelter: true,
			},
			status: None,
			requires_unblocked: true,
			stealth_recovery_ticks: None,
		}),
	}];
	let battle = compile_battle(&facts, &guide).expect("sequential projectiles are typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.operators()[&operator].remaining_hp,
		value(80),
		"arrow one destroys the shelter; arrows two and three reselect the rear target"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn axis_direct_hit_stays_ready_while_owner_is_blocked() {
	let blocker = OperatorInstanceId::new(1);
	let target = OperatorInstanceId::new(2);
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(blocker, 0, "blocker"),
			operator_build(target, 0, "axis_target"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: blocker,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: target,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(3));
	facts.paths = vec![path(1, 1, 100)];
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("axis_direct_hit"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
			initial_cooldown_ticks: 2,
			cooldown_ticks: 100,
			windup_ticks: 0,
			skill_duration_ticks: 0,
			projectiles: 3,
			damage_multiplier: ExpectedValue::ONE,
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
			status: None,
			requires_unblocked: true,
			stealth_recovery_ticks: None,
		}),
	}];
	let battle = compile_battle(&facts, &guide).expect("blocked readiness is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.operators()[&blocker].remaining_hp, value(100));
	assert_eq!(assessment.operators()[&target].remaining_hp, value(100));
	assert!(assessment.issues().is_empty());
}

#[test]
fn axis_direct_hit_locks_normal_attacks_and_movement_until_skill_end() {
	let blocker = OperatorInstanceId::new(1);
	let target = OperatorInstanceId::new(2);
	let guide = MaterializedGuide {
		builds: vec![
			operator_build(blocker, 0, "blocker"),
			operator_build(target, 0, "axis_target"),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: blocker,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: target,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(8));
	facts.paths = vec![path(1, 1, 100)];
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, 1)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("timed_axis_direct_hit"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
			initial_cooldown_ticks: 1,
			cooldown_ticks: 2,
			windup_ticks: 2,
			skill_duration_ticks: 4,
			projectiles: 1,
			damage_multiplier: ExpectedValue::ZERO,
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
			status: None,
			requires_unblocked: true,
			stealth_recovery_ticks: None,
		}),
	}];
	let battle = compile_battle(&facts, &guide).expect("axis lockout is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let ready_ticks: Vec<SimTick> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled }
				if matches!(
					scheduled.event(),
					ExpectedEvent::AxisDirectHit {
						phase: AxisDirectHitPhase::Ready,
						..
					}
				) =>
			{
				Some(scheduled.at())
			}
			TraceEntry::Event { .. } | TraceEntry::Command { .. } | TraceEntry::Choice { .. } => {
				None
			}
		})
		.collect();

	assert_eq!(assessment.operators()[&blocker].remaining_hp, value(90));
	assert_eq!(assessment.block_audit()[0].started_at, SimTick::new(6));
	assert_eq!(
		ready_ticks,
		vec![SimTick::new(1), SimTick::new(7), SimTick::new(8)],
		"the next cooldown starts at the tick-5 skill end, then retries while blocked"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn action_pause_does_not_restart_a_fixed_route_wait() {
	let target = OperatorInstanceId::new(1);
	let guide = MaterializedGuide {
		builds: vec![operator_build(target, 0, "axis_target")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: target,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(20));
	facts.paths = vec![BattlePath {
		id: PathId::new(1),
		goal: ark_solver::mechanics::domain::GoalId::new(1),
		start: GridPosition { row: 0, col: 0 },
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
					goal: ark_solver::mechanics::domain::GoalId::new(1),
				},
			},
		],
	}];
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("fixed_wait_axis_direct_hit"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
			initial_cooldown_ticks: 2,
			cooldown_ticks: 100,
			windup_ticks: 1,
			skill_duration_ticks: 4,
			projectiles: 1,
			damage_multiplier: ExpectedValue::ZERO,
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
			status: None,
			requires_unblocked: false,
			stealth_recovery_ticks: None,
		}),
	}];
	let battle = compile_battle(&facts, &guide).expect("fixed waits and action pauses are typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].leaked_at,
		Some(SimTick::new(11)),
		"the tick-2..6 skill must not restart the independent tick-10 route wait"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn axis_direct_hit_suppresses_conditional_stealth_while_active() {
	let sniper = OperatorInstanceId::new(1);
	let mut build = operator_build(sniper, 0, "stealth_sniper");
	build.combat.attack = value(10);
	build.block_count = 0;
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, -2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: sniper,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	facts.paths = vec![path(1, 100, 100)];
	facts.enemies[0].on_spawn = vec![
		TaggedEffect {
			mechanic: MechanicId::new("timed_axis_direct_hit"),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
				initial_cooldown_ticks: 1,
				cooldown_ticks: 100,
				windup_ticks: 2,
				skill_duration_ticks: 4,
				projectiles: 1,
				damage_multiplier: ExpectedValue::ZERO,
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
				status: None,
				requires_unblocked: true,
				stealth_recovery_ticks: Some(3),
			}),
		},
		TaggedEffect {
			mechanic: MechanicId::new("stealth_while_unblocked"),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::ApplyStatus(status(
				StatusKind::StealthWhileUnblocked,
				0,
				ExpectedValue::ONE,
			)),
		},
	];
	let battle = compile_battle(&facts, &guide).expect("skill-time stealth suppression is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		value(40),
		"the enemy is revealed from activation through tick 4 and hidden again at tick 5"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stun_pauses_attack_and_movement_then_reschedules_both() {
	let mut movement_facts = base_facts(100);
	movement_facts.end_at = None;
	movement_facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("stun_enemy"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::ApplyStatus(status(StatusKind::Stun, 5, ExpectedValue::ONE)),
		},
	}];
	let movement_battle = compile_battle(&movement_facts, &MaterializedGuide::default())
		.expect("Stun movement is typed");
	let movement = replay_expected(&movement_battle, MaterializedGuidePolicy::default());
	assert_eq!(
		movement.enemies()[&EnemyInstanceId::new(1)].leaked_at,
		Some(SimTick::new(29))
	);

	let operator = OperatorInstanceId::new(1);
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "stun_target")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut attack_facts = base_facts(100);
	attack_facts.end_at = Some(SimTick::new(9));
	attack_facts.paths = vec![path(1, 20, 20)];
	attack_facts.enemies[0].combat.attack = value(10);
	attack_facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 2,
		interval_ticks: 2,
		range: vec![RangeOffset::new(0, 2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	attack_facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("stun_enemy"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::ApplyStatus(status(StatusKind::Stun, 5, ExpectedValue::ONE)),
		},
	}];
	let attack_battle = compile_battle(&attack_facts, &guide).expect("Stun attack is typed");
	let attack = replay_expected(&attack_battle, MaterializedGuidePolicy::default());

	assert_eq!(attack.operators()[&operator].remaining_hp, value(90));
	assert!(movement.issues().is_empty());
	assert!(attack.issues().is_empty());
}

#[test]
fn stunned_blocker_releases_then_recaptures_waiting_enemy() {
	let operator = OperatorInstanceId::new(1);
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "stunned_blocker")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(7));
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(4),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("stun_blocker"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::ApplyStatus(status(StatusKind::Stun, 2, ExpectedValue::ONE)),
		},
	}];
	let battle = compile_battle(&facts, &guide).expect("Stun blocking is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.block_audit(),
		&[
			BlockAuditEntry {
				enemy: EnemyInstanceId::new(1),
				blocker: BattleActorId::Operator(operator),
				started_at: SimTick::new(3),
				ended_at: Some(SimTick::new(4)),
				end_reason: Some(BlockEndReason::BlockCountChange),
			},
			BlockAuditEntry {
				enemy: EnemyInstanceId::new(1),
				blocker: BattleActorId::Operator(operator),
				started_at: SimTick::new(6),
				ended_at: Some(SimTick::new(7)),
				end_reason: Some(BlockEndReason::BattleDeadline),
			},
		]
	);
	assert!(assessment.issues().is_empty());
}

const BLOCKER: OperatorInstanceId = OperatorInstanceId::new(1);

/// Raises the carrier's defense by half, in the panel bucket, while it blocks `minimum` enemies.
fn blocked_count_defense(minimum: u16) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new("blocked_count_defense"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::ConditionalStatModifier(ConditionalStatModifier {
			condition: ActorCondition::BlockedEnemiesAtLeast(minimum),
			modifier: StatModifier {
				stat: StatKind::Defense,
				bucket: StatModifierBucket::Panel,
				multiplier: ExpectedValue::from_ratio(3, 2).expect("valid multiplier"),
				duration_ticks: None,
			},
		}),
	}
}

/// A 200 physical stage hit on every ally: the blocker loses 200 minus its effective defense at
/// the moment the hit resolves.
fn defense_probe(at: u64) -> ScheduledStageEffect {
	ScheduledStageEffect {
		at: SimTick::new(at),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new(format!("defense_probe_{at}")),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Damage {
				amount: value(200),
				damage_type: DamageType::Physical,
				delivery: DamageDelivery::Normal,
			},
		},
	}
}

/// True stage damage to every enemy, which kills exactly the enemies it reaches the HP of.
fn enemy_damage(at: u64, amount: u32) -> ScheduledStageEffect {
	ScheduledStageEffect {
		at: SimTick::new(at),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new(format!("enemy_damage_{at}")),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(amount),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		},
	}
}

fn deploy_blocker(at: u64, col: u32) -> MaterializedGuideAction {
	MaterializedGuideAction::Deploy {
		requested_at: SimTick::new(at),
		operator: BLOCKER,
		cell: GridPosition { row: 0, col },
		facing: Facing::Right,
	}
}

/// A two-enemy blocker with 1000 HP and defense 100 deployed on `[0, 1]` at tick 0, and three
/// enemies on the one path to that cell. Enemy 1 (10 HP) is blocked at tick 3 and enemy 2 (30 HP) at
/// tick 6. Enemy 3 (1000 HP) reaches the cell at tick 7, when the blocker is full, and stays on it
/// unblocked until something frees capacity. No enemy attacks and none reaches its goal.
fn blocked_count_stage(
	on_deploy: Vec<TaggedEffect>,
	stage_effects: Vec<ScheduledStageEffect>,
	later_actions: Vec<MaterializedGuideAction>,
	end_at: u64,
) -> (ArkBattleFacts, MaterializedGuide) {
	let mut build = operator_build(BLOCKER, 0, "two_enemy_blocker");
	build.block_count = 2;
	build.combat = combat(1000, 0);
	build.combat.defense = value(100);
	build.on_deploy = on_deploy;
	let mut actions: Vec<MaterializedGuideAction> = vec![deploy_blocker(0, 1)];
	actions.extend(later_actions);
	let guide = MaterializedGuide {
		builds: vec![build],
		actions,
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(10);
	facts.end_at = Some(SimTick::new(end_at));
	facts.paths = vec![path(1, 3, 50)];
	for (key, max_hp) in [("sturdy", 30), ("anchor", 1000)] {
		let mut enemy = facts.enemies[0].clone();
		enemy.enemy_key = key.to_owned();
		enemy.combat.max_hp = value(max_hp);
		facts.enemies.push(enemy);
	}
	for (id, key, spawn_at) in [(2, "sturdy", 3), (3, "anchor", 4)] {
		facts.waves[0].spawns.push(EnemySpawnFacts {
			id: EnemyInstanceId::new(id),
			enemy_key: key.to_owned(),
			after_wave_start_ticks: spawn_at,
			path: PathId::new(1),
			facing: Facing::Right,
		});
	}
	facts.stage_effects = stage_effects;
	(facts, guide)
}

/// The blocker's HP once each listed tick has fully resolved, one replay per tick.
fn blocker_hp_after(
	on_deploy: &[TaggedEffect],
	stage_effects: &[ScheduledStageEffect],
	later_actions: &[MaterializedGuideAction],
	ticks: &[u64],
) -> Vec<(u64, ExpectedValue)> {
	ticks
		.iter()
		.map(|&tick| {
			let (facts, guide) = blocked_count_stage(
				on_deploy.to_vec(),
				stage_effects.to_vec(),
				later_actions.to_vec(),
				tick,
			);
			let battle = compile_battle(&facts, &guide).expect("blocked-count stage is supported");
			let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
			assert!(assessment.issues().is_empty(), "{:?}", assessment.issues());
			(tick, assessment.operators()[&BLOCKER].remaining_hp)
		})
		.collect()
}

fn block_entry(
	enemy: u32,
	started_at: u64,
	ended_at: u64,
	end_reason: BlockEndReason,
) -> BlockAuditEntry {
	BlockAuditEntry {
		enemy: EnemyInstanceId::new(enemy),
		blocker: BattleActorId::Operator(BLOCKER),
		started_at: SimTick::new(started_at),
		ended_at: Some(SimTick::new(ended_at)),
		end_reason: Some(end_reason),
	}
}

#[test]
fn blocked_enemy_count_gates_defense_from_the_block_that_reaches_it() {
	// Enemy 1 dies at tick 10, which frees a place that enemy 3 takes in the same tick, so the
	// blocker never holds fewer than two; enemy 2 dies at tick 13 and leaves enemy 3 alone.
	let stage_effects: Vec<ScheduledStageEffect> = vec![
		defense_probe(4),
		defense_probe(6),
		defense_probe(7),
		enemy_damage(10, 10),
		defense_probe(11),
		enemy_damage(13, 20),
		defense_probe(14),
	];
	let (facts, guide) = blocked_count_stage(
		vec![blocked_count_defense(2)],
		stage_effects.clone(),
		Vec::new(),
		15,
	);
	let battle = compile_battle(&facts, &guide).expect("blocked-count stage is supported");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	assert_eq!(
		assessment.block_audit(),
		&[
			block_entry(1, 3, 10, BlockEndReason::EnemyDeath),
			block_entry(2, 6, 13, BlockEndReason::EnemyDeath),
			block_entry(3, 10, 15, BlockEndReason::BattleDeadline),
		]
	);

	// One enemy leaves defense at 100, so a probe costs 100 HP. Enemy 2's step onto the cell at
	// tick 6 is a movement, which resolves after that tick's probe, so the probe still reads 100;
	// from tick 7 the blocker holds two and a probe costs only 200 - 150. The same-tick replacement
	// at tick 10 keeps the bonus, and losing enemy 2 at tick 13 removes it.
	assert_eq!(
		blocker_hp_after(
			&[blocked_count_defense(2)],
			&stage_effects,
			&[],
			&[4, 6, 7, 11, 14]
		),
		[
			(4, value(900)),
			(6, value(800)),
			(7, value(750)),
			(11, value(700)),
			(14, value(600)),
		]
	);
	// The same threshold at three is never met by a two-enemy blocker.
	assert_eq!(
		blocker_hp_after(&[blocked_count_defense(3)], &stage_effects, &[], &[14]),
		[(14, value(500))]
	);
}

#[test]
fn blocked_count_defense_sums_with_other_panel_defense() {
	let permanent_panel = TaggedEffect {
		mechanic: MechanicId::new("permanent_panel_defense"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StatModifier(StatModifier {
			stat: StatKind::Defense,
			bucket: StatModifierBucket::Panel,
			multiplier: ExpectedValue::from_ratio(6, 5).expect("valid multiplier"),
			duration_ticks: None,
		}),
	};
	// Defense 100 x (1 + 0.2) reads 120 against one enemy. Against two the ratios share their
	// bucket, 100 x (1 + 0.2 + 0.5) = 170, rather than compounding to 180.
	assert_eq!(
		blocker_hp_after(
			&[permanent_panel, blocked_count_defense(2)],
			&[defense_probe(4), defense_probe(7)],
			&[],
			&[4, 7]
		),
		[(4, value(920)), (7, value(890))]
	);
}

#[test]
fn a_released_block_ends_the_blocked_count_bonus_until_it_is_refilled() {
	// A stun empties the blocker's capacity at tick 8, releasing both enemies; when it expires at
	// tick 11 the blocker takes back the two lowest-numbered enemies still on its cell. The probe
	// listed after the stun resolves after the release in the same tick.
	let stun = ScheduledStageEffect {
		at: SimTick::new(8),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("stun_blocker"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::ApplyStatus(status(StatusKind::Stun, 3, ExpectedValue::ONE)),
		},
	};
	let stage_effects: Vec<ScheduledStageEffect> =
		vec![defense_probe(7), stun, defense_probe(8), defense_probe(12)];
	let (facts, guide) = blocked_count_stage(
		vec![blocked_count_defense(2)],
		stage_effects.clone(),
		Vec::new(),
		13,
	);
	let battle = compile_battle(&facts, &guide).expect("blocked-count stage is supported");
	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	assert_eq!(
		assessment.block_audit(),
		&[
			block_entry(1, 3, 8, BlockEndReason::BlockCountChange),
			block_entry(2, 6, 8, BlockEndReason::BlockCountChange),
			block_entry(1, 11, 13, BlockEndReason::BattleDeadline),
			block_entry(2, 11, 13, BlockEndReason::BattleDeadline),
		]
	);

	assert_eq!(
		blocker_hp_after(
			&[blocked_count_defense(2)],
			&stage_effects,
			&[],
			&[7, 8, 12]
		),
		[(7, value(950)), (8, value(850)), (12, value(800))]
	);
}

#[test]
fn an_enemy_that_fills_the_capacity_alone_counts_once() {
	// Enemy 1 demands both of the blocker's places, so enemies 2 and 3 reach the full cell at ticks
	// 6 and 7 and wait on it unblocked. The blocker holds one enemy until enemy 1 dies at 10 and
	// the other two take its places.
	let stage_effects: Vec<ScheduledStageEffect> =
		vec![defense_probe(7), enemy_damage(10, 10), defense_probe(11)];
	let replay = |end_at: u64| {
		let (mut facts, guide) = blocked_count_stage(
			vec![blocked_count_defense(2)],
			stage_effects.clone(),
			Vec::new(),
			end_at,
		);
		facts.enemies[0].block_demand = 2;
		let battle = compile_battle(&facts, &guide).expect("heavy-enemy stage is supported");
		let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
		assert!(assessment.issues().is_empty(), "{:?}", assessment.issues());
		assessment
	};

	assert_eq!(
		replay(12).block_audit(),
		&[
			block_entry(1, 3, 10, BlockEndReason::EnemyDeath),
			block_entry(2, 10, 12, BlockEndReason::BattleDeadline),
			block_entry(3, 10, 12, BlockEndReason::BattleDeadline),
		]
	);
	// A full capacity is not two held enemies: the probe at 7 reads defense 100, the one at 11,
	// with two enemies held, reads 150.
	assert_eq!(
		[7, 11].map(|tick| replay(tick).operators()[&BLOCKER].remaining_hp),
		[value(900), value(850)]
	);
}

#[test]
fn an_enemy_hidden_on_the_cell_is_neither_captured_nor_counted() {
	// Enemy 3 reaches the full blocker's cell at tick 7 and a Disappear step hides it there at 9.
	// When enemy 1 dies at 10 the freed place must stay empty, so the blocker holds only enemy 2
	// until enemy 3 reappears on the cell at 19 and is captured.
	let hidden_path = BattlePath {
		id: PathId::new(2),
		goal: ark_solver::mechanics::domain::GoalId::new(1),
		start: GridPosition { row: 0, col: 0 },
		steps: vec![
			BattlePathStep {
				duration_ticks: 3,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Move {
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: 2,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Disappear,
			},
			BattlePathStep {
				duration_ticks: 10,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Reappear {
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: 50,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Goal {
					goal: ark_solver::mechanics::domain::GoalId::new(1),
				},
			},
		],
	};
	let stage_effects: Vec<ScheduledStageEffect> = vec![
		defense_probe(7),
		enemy_damage(10, 10),
		defense_probe(11),
		defense_probe(20),
	];
	let replay = |end_at: u64| {
		let (mut facts, guide) = blocked_count_stage(
			vec![blocked_count_defense(2)],
			stage_effects.clone(),
			Vec::new(),
			end_at,
		);
		facts.paths.push(hidden_path.clone());
		facts.waves[0].spawns[2].path = PathId::new(2);
		let battle = compile_battle(&facts, &guide).expect("hidden-enemy stage is supported");
		let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
		assert!(assessment.issues().is_empty(), "{:?}", assessment.issues());
		assessment
	};

	assert_eq!(
		replay(21).block_audit(),
		&[
			block_entry(1, 3, 10, BlockEndReason::EnemyDeath),
			block_entry(2, 6, 21, BlockEndReason::BattleDeadline),
			block_entry(3, 19, 21, BlockEndReason::BattleDeadline),
		]
	);
	assert_eq!(
		[7, 11, 20].map(|tick| replay(tick).operators()[&BLOCKER].remaining_hp),
		[value(950), value(850), value(800)]
	);
}

#[test]
fn a_redeployed_blocker_counts_only_its_own_blocks() {
	// The first deployment holds enemies 1 and 2 when it retreats at tick 8, and both stay on
	// `[0, 1]`. A redeployment elsewhere blocks nothing, so the bonus does not survive the retreat;
	// one back on `[0, 1]` captures both on its deployment tick and earns it again. Either way the
	// redeployment starts from full HP.
	let retreat = MaterializedGuideAction::Retreat {
		requested_at: SimTick::new(8),
		operator: BLOCKER,
	};
	let redeploy_hp = |col: u32| -> Vec<(u64, ExpectedValue)> {
		blocker_hp_after(
			&[blocked_count_defense(2)],
			&[defense_probe(7), defense_probe(10)],
			&[retreat.clone(), deploy_blocker(9, col)],
			&[7, 10],
		)
	};

	assert_eq!(redeploy_hp(2), [(7, value(950)), (10, value(900))]);
	assert_eq!(redeploy_hp(1), [(7, value(950)), (10, value(950))]);
}

#[test]
fn conditional_stat_modifier_rejects_unrepresentable_gates() {
	let mut zero_threshold = blocked_count_defense(0);
	zero_threshold.mechanic = MechanicId::new("zero_threshold");
	let mut max_hp = blocked_count_defense(2);
	max_hp.mechanic = MechanicId::new("gated_max_hp");
	let ExpectedEffect::ConditionalStatModifier(conditional) = &mut max_hp.effect else {
		unreachable!("the helper builds a conditional stat modifier");
	};
	conditional.modifier.stat = StatKind::MaxHp;
	let (facts, guide) =
		blocked_count_stage(vec![zero_threshold, max_hp], Vec::new(), Vec::new(), 5);

	let error = compile_battle(&facts, &guide).expect_err("both gates fail preflight");
	let rejected: Vec<(&str, &str)> = error
		.issues()
		.iter()
		.filter_map(|issue| match issue {
			BattleCompileIssue::InvalidReplayPrimitive { mechanic, detail } => {
				Some((mechanic.as_str(), detail.as_str()))
			}
			_ => None,
		})
		.collect();
	assert_eq!(
		rejected,
		[
			(
				"gated_max_hp",
				"conditional stat modifier cannot gate maximum HP, whose change clamps current HP"
			),
			(
				"zero_threshold",
				"blocked-enemy count threshold must be positive"
			),
		]
	);
}

#[test]
fn max_hp_modifier_fills_full_actor_and_clamps_when_expired() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "max_hp_buff");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("double_max_hp"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StatModifier(StatModifier {
			bucket: StatModifierBucket::Scale,
			stat: StatKind::MaxHp,
			multiplier: value(2),
			duration_ticks: Some(2),
		}),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(2));
	facts.paths = vec![path(1, 100, 100)];
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("chip_buffed_hp"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Damage {
				amount: value(50),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		},
	}];
	let battle = compile_battle(&facts, &guide).expect("max-HP modifier is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.operators()[&operator].remaining_hp,
		value(100),
		"deployment fills to 200, damage leaves 150, expiration clamps to 100"
	);
	assert_eq!(assessment.operators()[&operator].max_hp, value(100));
	assert!(assessment.issues().is_empty());
}

#[test]
fn permanent_max_hp_modifier_updates_the_assessment_ceiling() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "permanent_max_hp_buff");
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("permanent_double_max_hp"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StatModifier(StatModifier {
			bucket: StatModifierBucket::Scale,
			stat: StatKind::MaxHp,
			multiplier: value(2),
			duration_ticks: None,
		}),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(1));
	facts.paths = vec![path(1, 100, 100)];
	let battle = compile_battle(&facts, &guide).expect("permanent max-HP modifier is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let report = &assessment.operators()[&operator];

	assert_eq!(report.max_hp, value(200));
	assert_eq!(report.remaining_hp, value(200));
	assert!(report.remaining_hp <= report.max_hp);
	assert!(assessment.issues().is_empty());
}

#[test]
fn damage_to_reactive_target_exposes_the_damage_source() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "exposure_source");
	build.combat.attack = value(1);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, -2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(2));
	facts.paths = vec![path(1, 100, 100)];
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 2,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("expose_damage_source"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::ApplyStatus(status(
			StatusKind::ExposeDamageSource {
				exposure_duration_ticks: 5,
				incoming_damage_multiplier: value(2),
			},
			0,
			ExpectedValue::ONE,
		)),
	}];
	let battle = compile_battle(&facts, &guide).expect("exposure reaction is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(assessment.operators()[&operator].remaining_hp, value(80));
	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(99)
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stealth_while_unblocked_becomes_targetable_after_blocking() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "stealth_revealer");
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, -1), RangeOffset::new(0, 0)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(4));
	facts.enemies[0].on_spawn = vec![TaggedEffect {
		mechanic: MechanicId::new("stealth_while_unblocked"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::ApplyStatus(status(
			StatusKind::StealthWhileUnblocked,
			0,
			ExpectedValue::ONE,
		)),
	}];
	let battle = compile_battle(&facts, &guide).expect("conditional stealth is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		value(10)
	);
	assert_eq!(assessment.block_audit()[0].started_at, SimTick::new(3));
	assert!(assessment.issues().is_empty());
}

#[test]
fn stealth_recovery_waits_after_unblock_and_reblocking_invalidates_the_old_timer() {
	let blocker = OperatorInstanceId::new(1);
	let sniper = OperatorInstanceId::new(2);
	let mut sniper_build = operator_build(sniper, 0, "recovery_sniper");
	sniper_build.block_count = 0;
	sniper_build.combat.attack = value(10);
	sniper_build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, -1), RangeOffset::new(0, -2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![operator_build(blocker, 0, "recovery_blocker"), sniper_build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: blocker,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: sniper,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(4),
				operator: blocker,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(5),
				operator: blocker,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(6),
				operator: blocker,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(9));
	facts.enemies[0].on_spawn = vec![
		TaggedEffect {
			mechanic: MechanicId::new("stealth_recovery_owner"),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::StartAxisDirectHit(EnemyAxisDirectHitFacts {
				initial_cooldown_ticks: 100,
				cooldown_ticks: 100,
				windup_ticks: 0,
				skill_duration_ticks: 0,
				projectiles: 1,
				damage_multiplier: ExpectedValue::ZERO,
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
				status: None,
				requires_unblocked: true,
				stealth_recovery_ticks: Some(3),
			}),
		},
		TaggedEffect {
			mechanic: MechanicId::new("stealth_while_unblocked"),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::ApplyStatus(status(
				StatusKind::StealthWhileUnblocked,
				0,
				ExpectedValue::ONE,
			)),
		},
	];
	let battle = compile_battle(&facts, &guide).expect("stealth recovery is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		value(50),
		"the stale tick-7 recovery must not hide the enemy before the reset tick-9 recovery"
	);
	assert_eq!(assessment.block_audit().len(), 2);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stored_attack_replacement_caps_charges_uses_dynamic_stat_and_excludes_devices() {
	let healer = OperatorInstanceId::new(1);
	let patients = [
		OperatorInstanceId::new(2),
		OperatorInstanceId::new(3),
		OperatorInstanceId::new(4),
	];
	let shelter = SummonTemplateId::new(1);
	let device = SummonTemplateId::new(2);
	let mut shelter_template = summon_template(shelter, "shelter", Faction::Friendly);
	shelter_template.block_count = 0;
	shelter_template.targetability = Targetability::Shelter;
	shelter_template.combat.max_hp = value(70);
	let mut device_template = summon_template(device, "device", Faction::Friendly);
	device_template.block_count = 0;
	device_template.targetability = Targetability::None;
	device_template.combat.max_hp = value(70);

	let mut healer_build = operator_build(healer, 0, "conditional_healer");
	healer_build.combat.attack = value(30);
	healer_build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, 6)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::Physical,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	healer_build.skill = Some(stored_attack_replacement_skill(
		99,
		3,
		(1..=5).map(|col| RangeOffset::new(0, col)).collect(),
		StoredAttackHpRatioComparison::LessThan,
		ExpectedValue::ONE,
	));
	healer_build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("dynamic_attack"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StatModifier(StatModifier {
			bucket: StatModifierBucket::Scale,
			stat: StatKind::Attack,
			multiplier: value(2),
			duration_ticks: None,
		}),
	}];
	let guide = MaterializedGuide {
		builds: std::iter::once(healer_build)
			.chain(
				patients
					.into_iter()
					.map(|patient| operator_build(patient, 0, "patient")),
			)
			.collect(),
		actions: [
			(healer, 0),
			(patients[0], 3),
			(patients[1], 4),
			(patients[2], 5),
		]
		.into_iter()
		.map(|(operator, col)| MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col },
			facing: Facing::Right,
		})
		.collect(),
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(4));
	facts.grid = BattleGrid {
		dimensions: MapDimensions {
			width: 7,
			height: 1,
		},
		cells: BTreeMap::new(),
		deployable_cells: (0..=5).map(|col| GridPosition { row: 0, col }).collect(),
		ground_cells: (0..7).map(|col| GridPosition { row: 0, col }).collect(),
	};
	facts.paths = vec![stationary_path(1, GridPosition { row: 0, col: 6 })];
	facts.summons = vec![shelter_template, device_template];
	facts.preplaced_actors = vec![
		PreplacedActorFacts {
			instance: 1,
			template: shelter,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		},
		PreplacedActorFacts {
			instance: 2,
			template: device,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		},
	];
	facts.enemies[0].combat.defense = value(10);
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(0),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("injure_allies"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Damage {
				amount: value(60),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		},
	}];
	let battle = compile_battle(&facts, &guide).expect("conditional healing is typed");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	for patient in patients {
		assert_eq!(
			assessment.operators()[&patient].remaining_hp,
			value(100),
			"all three stored charges must heal operators before the ordinary attack resumes"
		);
	}
	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		value(50),
		"the fourth tick restores physical damage using dynamically doubled ATK"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stored_attack_replacement_respects_hp_boundary_and_facing() {
	let replay = |damage: u32, comparison: StoredAttackHpRatioComparison| {
		let healer = OperatorInstanceId::new(1);
		let patient = OperatorInstanceId::new(2);
		let mut healer_build = operator_build(healer, 0, "left_facing_healer");
		healer_build.combat.attack = value(50);
		healer_build.attack = AttackProfile {
			first_delay_ticks: 1,
			interval_ticks: 100,
			range: vec![RangeOffset::new(0, -2)],
			target_count: 1,
			target_filter: NormalAttackTargetFilter::GroundOnly,
			action: NormalAttackAction::Damage {
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
			pattern: AttackPattern::Single,
			extra_effects: Vec::new(),
		};
		healer_build.skill = Some(stored_attack_replacement_skill(
			1,
			1,
			vec![RangeOffset::new(0, 1)],
			comparison,
			ExpectedValue::ONE,
		));
		let guide = MaterializedGuide {
			builds: vec![
				healer_build,
				operator_build(patient, 0, "threshold_patient"),
			],
			actions: vec![
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(0),
					operator: healer,
					cell: GridPosition { row: 0, col: 2 },
					facing: Facing::Left,
				},
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(0),
					operator: patient,
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
			],
			reached_mechanics: Vec::new(),
		};
		let mut facts = base_facts(100);
		facts.end_at = Some(SimTick::new(1));
		facts.grid = BattleGrid {
			dimensions: MapDimensions {
				width: 5,
				height: 1,
			},
			cells: BTreeMap::new(),
			deployable_cells: BTreeSet::from([
				GridPosition { row: 0, col: 1 },
				GridPosition { row: 0, col: 2 },
			]),
			ground_cells: (0..5).map(|col| GridPosition { row: 0, col }).collect(),
		};
		facts.paths = vec![stationary_path(1, GridPosition { row: 0, col: 4 })];
		facts.stage_effects = vec![ScheduledStageEffect {
			at: SimTick::new(0),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("threshold_damage"),
				target: EffectTarget::AllAllies,
				effect: ExpectedEffect::Damage {
					amount: value(damage),
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				},
			},
		}];
		let battle = compile_battle(&facts, &guide).expect("conditional threshold is typed");
		replay_expected(&battle, MaterializedGuidePolicy::default())
	};

	let exact_half = replay(50, StoredAttackHpRatioComparison::LessThan);
	assert_eq!(
		exact_half.operators()[&OperatorInstanceId::new(2)].remaining_hp,
		value(50),
		"exactly 50 percent does not satisfy a strict-below threshold"
	);
	assert_eq!(
		exact_half.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		value(50),
		"an ineligible heal preserves the ordinary attack"
	);

	let inclusive_half = replay(50, StoredAttackHpRatioComparison::LessThanOrEqual);
	assert_eq!(
		inclusive_half.operators()[&OperatorInstanceId::new(2)].remaining_hp,
		value(100),
		"the inclusive threshold accepts exactly 50 percent HP"
	);
	assert_eq!(
		inclusive_half.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		ExpectedValue::ZERO,
		"an inclusive qualifying replacement suppresses the ordinary attack"
	);

	let below_half = replay(51, StoredAttackHpRatioComparison::LessThan);
	assert_eq!(
		below_half.operators()[&OperatorInstanceId::new(2)].remaining_hp,
		value(99),
		"left-facing projection reaches the cell to the healer's left"
	);
	assert_eq!(
		below_half.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken,
		ExpectedValue::ZERO,
		"the qualifying heal replaces the ordinary attack"
	);
	assert!(exact_half.issues().is_empty());
	assert!(inclusive_half.issues().is_empty());
	assert!(below_half.issues().is_empty());
}

#[test]
fn stored_attack_replacement_does_not_spend_without_an_eligible_target() {
	let source = OperatorInstanceId::new(1);
	let target = OperatorInstanceId::new(2);
	let mut skill = stored_attack_replacement_skill(
		1,
		1,
		vec![RangeOffset::new(0, 1)],
		StoredAttackHpRatioComparison::LessThan,
		ExpectedValue::ONE,
	);
	stored_attack_replacement_mut(&mut skill).include_source = false;
	let source_build = inert_replacement_source(source, 50, 1, 2, skill);
	let guide = MaterializedGuide {
		builds: vec![source_build, operator_build(target, 0, "later_patient")],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 0 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: target,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(3));
	facts
		.grid
		.deployable_cells
		.insert(GridPosition { row: 0, col: 0 });
	facts.paths = vec![stationary_path(1, GridPosition { row: 0, col: 3 })];
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(2),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("late_injury"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Damage {
				amount: value(60),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		},
	}];
	let battle = compile_battle(&facts, &guide).expect("no-target reservation facts are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.operators()[&target].remaining_hp,
		value(90),
		"the tick-1 miss must preserve SP for the tick-3 replacement"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stored_attack_replacement_keeps_the_reserved_stable_lowest_ratio_target() {
	let source = OperatorInstanceId::new(1);
	let first = OperatorInstanceId::new(2);
	let second = OperatorInstanceId::new(3);
	let ordinary_healer = OperatorInstanceId::new(4);
	let mut skill = stored_attack_replacement_skill(
		1,
		1,
		vec![RangeOffset::new(0, 1), RangeOffset::new(0, 2)],
		StoredAttackHpRatioComparison::LessThan,
		ExpectedValue::ONE,
	);
	let replacement = stored_attack_replacement_mut(&mut skill);
	replacement.include_source = false;
	replacement.delivery_delay_ticks = 2;
	let source_build = inert_replacement_source(source, 50, 1, 100, skill);
	let mut first_build = operator_build(first, 0, "first_patient");
	first_build.on_deploy = vec![self_damage("injure_first", 80)];
	let mut second_build = operator_build(second, 0, "second_patient");
	second_build.on_deploy = vec![self_damage("injure_second", 70)];
	let mut ordinary_healer_build = operator_build(ordinary_healer, 0, "ordinary_healer");
	ordinary_healer_build.combat.attack = value(100);
	ordinary_healer_build.attack = AttackProfile {
		first_delay_ticks: 2,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 1), RangeOffset::new(0, 2)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::Heal,
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![
			source_build,
			first_build,
			second_build,
			ordinary_healer_build,
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 0 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: first,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: second,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: ordinary_healer,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Left,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(3));
	facts.grid = BattleGrid {
		dimensions: MapDimensions {
			width: 6,
			height: 1,
		},
		cells: BTreeMap::new(),
		deployable_cells: (0..=3).map(|col| GridPosition { row: 0, col }).collect(),
		ground_cells: (0..6).map(|col| GridPosition { row: 0, col }).collect(),
	};
	facts.paths = vec![stationary_path(1, GridPosition { row: 0, col: 5 })];
	let battle = compile_battle(&facts, &guide).expect("stable reservation facts are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let reserved_targets: Vec<_> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled } => match scheduled.event() {
				ExpectedEvent::StoredAttackReplacementDelivery {
					source: event_source,
					target,
					..
				} if *event_source == BattleActorId::Operator(source) => Some(*target),
				_ => None,
			},
			TraceEntry::Command { .. } | TraceEntry::Choice { .. } => None,
		})
		.collect();

	assert_eq!(reserved_targets.len(), 1);
	assert_eq!(reserved_targets[0].actor, BattleActorId::Operator(first));
	assert_eq!(reserved_targets[0].deployment_generation, Some(1));
	assert_eq!(
		assessment.operators()[&second].remaining_hp,
		value(30),
		"delivery must not retarget after another healer makes the reservation target healthier"
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stored_attack_replacement_target_death_refunds_once_without_stale_retargeting() {
	let source = OperatorInstanceId::new(1);
	let doomed = OperatorInstanceId::new(2);
	let next = OperatorInstanceId::new(3);
	let untouched = OperatorInstanceId::new(4);
	let mut skill = stored_attack_replacement_skill(
		1,
		1,
		(1..=3).map(|col| RangeOffset::new(0, col)).collect(),
		StoredAttackHpRatioComparison::LessThan,
		ExpectedValue::ONE,
	);
	let replacement = stored_attack_replacement_mut(&mut skill);
	replacement.include_source = false;
	replacement.delivery_delay_ticks = 4;
	replacement.invalidation.target_death =
		StoredAttackReservationInvalidationAction::RefundSpentSp;
	let source_build = inert_replacement_source(source, 50, 1, 2, skill);
	let mut doomed_build = operator_build(doomed, 0, "doomed_patient");
	doomed_build.on_deploy = vec![self_damage("injure_doomed", 90)];
	let mut next_build = operator_build(next, 0, "next_patient");
	next_build.on_deploy = vec![self_damage("injure_next", 20)];
	let mut untouched_build = operator_build(untouched, 0, "untouched_patient");
	untouched_build.on_deploy = vec![self_damage("injure_untouched", 20)];
	let guide = MaterializedGuide {
		builds: vec![source_build, doomed_build, next_build, untouched_build],
		actions: [source, doomed, next, untouched]
			.into_iter()
			.enumerate()
			.map(|(col, operator)| MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition {
					row: 0,
					col: col as u32,
				},
				facing: Facing::Right,
			})
			.collect(),
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(9));
	facts.grid = BattleGrid {
		dimensions: MapDimensions {
			width: 6,
			height: 1,
		},
		cells: BTreeMap::new(),
		deployable_cells: (0..=3).map(|col| GridPosition { row: 0, col }).collect(),
		ground_cells: (0..6).map(|col| GridPosition { row: 0, col }).collect(),
	};
	facts.paths = vec![stationary_path(1, GridPosition { row: 0, col: 5 })];
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(2),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("kill_reserved_target"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Damage {
				amount: value(40),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		},
	}];
	let battle = compile_battle(&facts, &guide).expect("death refund facts are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let replacement_heals: Vec<(SimTick, Vec<BattleActorId>)> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled } => match scheduled.event() {
				ExpectedEvent::ResolveEffect {
					source: effect_source,
					targets,
					effect,
					..
				} if *effect_source == BattleActorId::Operator(source)
					&& matches!(effect.effect, ExpectedEffect::Healing { .. }) =>
				{
					Some((scheduled.at(), targets.clone()))
				}
				_ => None,
			},
			TraceEntry::Command { .. } | TraceEntry::Choice { .. } => None,
		})
		.collect();

	assert_eq!(
		assessment.operators()[&doomed].death_at,
		Some(SimTick::new(2))
	);
	assert_eq!(assessment.operators()[&next].remaining_hp, value(90));
	assert_eq!(
		assessment.operators()[&untouched].remaining_hp,
		value(40),
		"a stale delivery must neither retarget nor refund a second time"
	);
	assert_eq!(
		replacement_heals,
		vec![(SimTick::new(7), vec![BattleActorId::Operator(next)])]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn stored_attack_replacement_redeployed_target_uses_generation_refund_policy() {
	let source = OperatorInstanceId::new(1);
	let target = OperatorInstanceId::new(2);
	let mut skill = stored_attack_replacement_skill(
		1,
		1,
		vec![RangeOffset::new(0, 1)],
		StoredAttackHpRatioComparison::LessThan,
		ExpectedValue::ONE,
	);
	let replacement = stored_attack_replacement_mut(&mut skill);
	replacement.include_source = false;
	replacement.delivery_delay_ticks = 4;
	replacement.invalidation.target_disappeared =
		StoredAttackReservationInvalidationAction::ConsumeSpentSp;
	replacement.invalidation.target_generation_changed =
		StoredAttackReservationInvalidationAction::RefundSpentSp;
	let source_build = inert_replacement_source(source, 50, 1, 4, skill);
	let mut target_build = operator_build(target, 0, "redeployed_patient");
	target_build.on_deploy = vec![self_damage("injure_each_deployment", 80)];
	let guide = MaterializedGuide {
		builds: vec![source_build, target_build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 0 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: target,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(2),
				operator: target,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(3),
				operator: target,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(9));
	facts
		.grid
		.deployable_cells
		.insert(GridPosition { row: 0, col: 0 });
	facts.paths = vec![stationary_path(1, GridPosition { row: 0, col: 3 })];
	let battle = compile_battle(&facts, &guide).expect("generation policy facts are supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());
	let generations: Vec<Option<u32>> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled } => match scheduled.event() {
				ExpectedEvent::StoredAttackReplacementDelivery {
					source: event_source,
					target: event_target,
					..
				} if *event_source == BattleActorId::Operator(source) => {
					Some(event_target.deployment_generation)
				}
				_ => None,
			},
			TraceEntry::Command { .. } | TraceEntry::Choice { .. } => None,
		})
		.collect();

	assert_eq!(generations, vec![Some(1), Some(2)]);
	assert_eq!(assessment.operators()[&target].remaining_hp, value(70));
	assert!(assessment.issues().is_empty());
}

#[test]
fn stored_attack_replacement_distinguishes_disappeared_from_out_of_range() {
	let replay = |first_action: BattlePathStepAction,
	              return_action: BattlePathStepAction,
	              disappeared: StoredAttackReservationInvalidationAction,
	              out_of_range: StoredAttackReservationInvalidationAction| {
		let source = OperatorInstanceId::new(1);
		let mut skill = stored_attack_replacement_skill(
			1,
			1,
			vec![RangeOffset::new(0, 1)],
			StoredAttackHpRatioComparison::LessThan,
			ExpectedValue::ONE,
		);
		let replacement = stored_attack_replacement_mut(&mut skill);
		replacement.relationship = StoredAttackTargetRelationship::Enemy;
		replacement.require_healable = false;
		replacement.include_source = false;
		replacement.delivery_delay_ticks = 2;
		replacement.invalidation.target_disappeared = disappeared;
		replacement.invalidation.target_out_of_range = out_of_range;
		let source_build = inert_replacement_source(source, 30, 1, 2, skill);
		let guide = MaterializedGuide {
			builds: vec![source_build],
			actions: vec![MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 0 },
				facing: Facing::Right,
			}],
			reached_mechanics: Vec::new(),
		};
		let mut facts = base_facts(100);
		facts.end_at = Some(SimTick::new(7));
		facts
			.grid
			.deployable_cells
			.insert(GridPosition { row: 0, col: 0 });
		facts.paths = vec![BattlePath {
			id: PathId::new(1),
			goal: ark_solver::mechanics::domain::GoalId::new(1),
			start: GridPosition { row: 0, col: 1 },
			steps: vec![
				BattlePathStep {
					duration_ticks: 2,
					affected_by_movement_speed: true,
					action: first_action,
				},
				BattlePathStep {
					duration_ticks: 2,
					affected_by_movement_speed: true,
					action: return_action,
				},
				BattlePathStep {
					duration_ticks: 100,
					affected_by_movement_speed: true,
					action: BattlePathStepAction::Wait,
				},
				BattlePathStep {
					duration_ticks: 1,
					affected_by_movement_speed: true,
					action: BattlePathStepAction::Goal {
						goal: ark_solver::mechanics::domain::GoalId::new(1),
					},
				},
			],
		}];
		facts.enemies[0].on_spawn = vec![self_damage("injure_enemy", 60)];
		let battle =
			compile_battle(&facts, &guide).expect("target invalidation facts are supported");
		replay_expected(&battle, MaterializedGuidePolicy::default())
	};

	let disappeared = replay(
		BattlePathStepAction::Disappear,
		BattlePathStepAction::Reappear {
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		},
		StoredAttackReservationInvalidationAction::RefundSpentSp,
		StoredAttackReservationInvalidationAction::ConsumeSpentSp,
	);
	let out_of_range = replay(
		BattlePathStepAction::Move {
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		},
		BattlePathStepAction::Move {
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		},
		StoredAttackReservationInvalidationAction::ConsumeSpentSp,
		StoredAttackReservationInvalidationAction::RefundSpentSp,
	);

	assert_eq!(
		disappeared.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(70),
		"an invisible enemy must use target_disappeared, not target_out_of_range"
	);
	assert_eq!(
		out_of_range.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(70),
		"a visible enemy outside the range must use target_out_of_range"
	);
	assert!(disappeared.issues().is_empty());
	assert!(out_of_range.issues().is_empty());
}

#[test]
fn unknown_stored_attack_invalidation_policy_fails_closed_at_compile_time() {
	let source = OperatorInstanceId::new(1);
	let mut skill = stored_attack_replacement_skill(
		1,
		1,
		vec![RangeOffset::new(0, 1)],
		StoredAttackHpRatioComparison::LessThan,
		ExpectedValue::ONE,
	);
	stored_attack_replacement_mut(&mut skill)
		.invalidation
		.target_out_of_range = StoredAttackReservationInvalidationAction::Unsupported;
	let guide = MaterializedGuide {
		builds: vec![inert_replacement_source(source, 30, 1, 2, skill)],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: source,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let report = compile_battle(&base_facts(100), &guide)
		.expect_err("unknown invalidation behavior must fail closed");

	assert!(report.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidCombatFacts { detail }
			if detail.contains("invalid stored-attack replacement skill")
	)));
}

#[test]
fn increasing_block_count_captures_an_enemy_already_on_the_cell() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "zero_block_then_gain_one");
	build.block_count = 0;
	build.on_deploy = vec![TaggedEffect {
		mechanic: MechanicId::new("gain_block"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Blocking(BlockCountModifier {
			delta: 1,
			duration_ticks: None,
		}),
	}];
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(4),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(5));
	let battle = compile_battle(&facts, &guide).expect("blocking modifier is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.block_audit(),
		&[BlockAuditEntry {
			enemy: EnemyInstanceId::new(1),
			blocker: BattleActorId::Operator(operator),
			started_at: SimTick::new(4),
			ended_at: Some(SimTick::new(5)),
			end_reason: Some(BlockEndReason::BattleDeadline),
		}]
	);
	assert!(assessment.issues().is_empty());
}

#[test]
fn compiler_aggregates_every_reached_unknown_mechanic() {
	let mut facts = base_facts(10);
	facts.reached_mechanics = vec![
		ReachedMechanic {
			id: MechanicId::new("unknown_a"),
			origin: "stage/rune/0".to_owned(),
			behavior: MechanicBehavior::Unsupported {
				reason: "no reviewed behavior".to_owned(),
			},
		},
		ReachedMechanic {
			id: MechanicId::new("unknown_b"),
			origin: "enemy/prefab".to_owned(),
			behavior: MechanicBehavior::Unsupported {
				reason: "no reviewed behavior".to_owned(),
			},
		},
		ReachedMechanic {
			id: MechanicId::new("unknown_a"),
			origin: "stage/tile/2".to_owned(),
			behavior: MechanicBehavior::Unsupported {
				reason: "tile behavior missing".to_owned(),
			},
		},
	];
	let report = compile_battle(&facts, &MaterializedGuide::default())
		.expect_err("unknown mechanics must fail before replay");
	let ids: Vec<&str> = report.unsupported_ids().map(MechanicId::as_str).collect();

	assert_eq!(ids, vec!["unknown_a", "unknown_b"]);
	assert_eq!(report.unsupported()[0].origins.len(), 2);
	assert!(report.issues().is_empty());
}

#[test]
fn skill_daemon_only_invokes_manual_usage_one_while_auto_uses_game_rule() {
	let manual = OperatorInstanceId::new(1);
	let automatic = OperatorInstanceId::new(2);
	let skill = |id: &str, activation| ark_solver::mechanics::domain::SkillFacts {
		id: MechanicId::new(id),
		activation,
		initial_sp: 1,
		sp_cost: 1,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new(id),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(100),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	};
	let build = |id, key: &str, skill| OperatorBuildFacts {
		id,
		operator_key: key.to_owned(),
		classification: OperatorClassificationFacts::default(),
		on_battle_start: Vec::new(),
		deployment_cost: 0,
		redeployment_cooldown_ticks: 0,
		block_count: 1,
		block_target_filter: BlockTargetFilter::GroundOnly,
		combat: combat(100, 0),
		attack: inert_attack(),
		skill: Some(skill),
		on_deploy: Vec::new(),
		on_death: Vec::new(),
		reached_mechanics: Vec::new(),
	};
	let guide = MaterializedGuide {
		builds: vec![
			build(
				manual,
				"manual",
				skill(
					"manual_skill",
					ark_solver::mechanics::domain::SkillActivationRule::Manual,
				),
			),
			build(
				automatic,
				"auto",
				skill(
					"auto_skill",
					ark_solver::mechanics::domain::SkillActivationRule::Auto,
				),
			),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(1),
				operator: manual,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(1),
				operator: automatic,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::SkillDaemon {
				requested_at: SimTick::new(1),
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(150);
	facts.end_at = Some(SimTick::new(10));
	facts.paths = vec![path(1, 20, 1)];
	let battle = compile_battle(&facts, &guide).expect("supported skills");

	let usage_zero = replay_expected(
		&battle,
		MaterializedGuidePolicy::new(BTreeMap::from([(manual, 0)])),
	);
	assert_eq!(
		usage_zero.status(),
		&ExpectedTerminalStatus::Deadline {
			alive_enemies: vec![EnemyInstanceId::new(1)]
		}
	);
	assert_eq!(
		usage_zero.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(50)
	);

	let usage_one = replay_expected(
		&battle,
		MaterializedGuidePolicy::new(BTreeMap::from([(manual, 1)])),
	);
	assert_eq!(usage_one.status(), &ExpectedTerminalStatus::Victory);
	let activation_origins: BTreeMap<OperatorInstanceId, SkillActivationOrigin> = usage_one
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| {
			let TraceEntry::Event { scheduled } = entry else {
				return None;
			};
			let ExpectedEvent::SkillActivation {
				operator, origin, ..
			} = scheduled.event()
			else {
				return None;
			};
			Some((*operator, *origin))
		})
		.collect();
	assert_eq!(
		activation_origins,
		BTreeMap::from([
			(manual, SkillActivationOrigin::SkillDaemon),
			(automatic, SkillActivationOrigin::Automatic),
		])
	);
}

#[test]
fn skill_daemon_repeats_manual_skills_and_survives_redeployment() {
	let operator = OperatorInstanceId::new(1);
	let mut build = operator_build(operator, 0, "daemon_caster");
	build.redeployment_cooldown_ticks = 2;
	build.skill = Some(SkillFacts {
		id: MechanicId::new("daemon_skill"),
		activation: SkillActivationRule::Manual,
		initial_sp: 0,
		sp_cost: 1,
		charge: SpChargeRule::Time {
			amount: 1,
			interval_ticks: 3,
		},
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(2),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("daemon_skill_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(10),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	});
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::SkillDaemon {
				requested_at: SimTick::new(0),
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(5),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(5),
				operator,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(20);
	facts.end_at = Some(SimTick::new(12));
	facts.paths = vec![path(1, 100, 100)];
	let battle = compile_battle(&facts, &guide).expect("daemon skill is supported");

	let assessment = replay_expected(
		&battle,
		MaterializedGuidePolicy::new(BTreeMap::from([(operator, 1)])),
	);

	assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	assert_eq!(
		assessment.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(10))
	);
	assert_eq!(
		assessment
			.trace()
			.entries()
			.iter()
			.filter(|entry| {
				matches!(
					entry,
					TraceEntry::Event { scheduled }
						if matches!(
							scheduled.event(),
							ExpectedEvent::SkillActivation {
								operator: activated,
								..
							} if *activated == operator
						)
				)
			})
			.count(),
		2,
		"the persistent daemon should activate once in each deployment episode"
	);
}

#[test]
fn timed_skill_recharges_only_after_its_active_duration_ends() {
	for (first_activation, second_activation) in [(960, 2_460), (1_146, 2_646)] {
		let operator = OperatorInstanceId::new(1);
		let mut build = operator_build(operator, 0, "timed_daemon_caster");
		build.skill = Some(SkillFacts {
			id: MechanicId::new("timed_daemon_skill"),
			activation: SkillActivationRule::Manual,
			initial_sp: 25,
			sp_cost: 35,
			charge: SpChargeRule::Time {
				amount: 1,
				interval_ticks: 30,
			},
			periodic_sp_sources: Vec::new(),
			active_duration_ticks: Some(450),
			manual_discard: None,
			stored_attack_replacement: None,
			effects: vec![TaggedEffect {
				mechanic: MechanicId::new("timed_daemon_damage"),
				target: EffectTarget::AllEnemies,
				effect: ExpectedEffect::Damage {
					amount: value(10),
					damage_type: DamageType::True,
					delivery: DamageDelivery::Normal,
				},
			}],
		});
		let guide = MaterializedGuide {
			builds: vec![build],
			actions: vec![
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(660),
					operator,
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
				MaterializedGuideAction::ActivateSkill {
					requested_at: SimTick::new(first_activation),
					operator,
				},
				MaterializedGuideAction::SkillDaemon {
					requested_at: SimTick::new(first_activation),
				},
			],
			reached_mechanics: Vec::new(),
		};
		let mut facts = base_facts(20);
		facts.end_at = Some(SimTick::new(second_activation + 1));
		facts.paths = vec![path(1, 10_000, 10_000)];
		let battle = compile_battle(&facts, &guide).expect("timed daemon skill is supported");

		let assessment = replay_expected(
			&battle,
			MaterializedGuidePolicy::new(BTreeMap::from([(operator, 1)])),
		);
		let activation_ticks: Vec<SimTick> = assessment
			.trace()
			.entries()
			.iter()
			.filter_map(|entry| match entry {
				TraceEntry::Event { scheduled }
					if matches!(
						scheduled.event(),
						ExpectedEvent::SkillActivation {
							operator: activated,
							..
						} if *activated == operator
					) =>
				{
					Some(scheduled.at())
				}
				TraceEntry::Event { .. }
				| TraceEntry::Command { .. }
				| TraceEntry::Choice { .. } => None,
			})
			.collect();

		assert_eq!(
			activation_ticks,
			vec![
				SimTick::new(first_activation),
				SimTick::new(second_activation)
			]
		);
		assert_eq!(assessment.status(), &ExpectedTerminalStatus::Victory);
	}
}

#[test]
fn final_enemy_on_death_effects_run_before_victory() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(10);
	facts.enemies[0].on_spawn.push(TaggedEffect {
		mechanic: MechanicId::new("self_destruct"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Death,
	});
	facts.enemies[0].on_death.push(TaggedEffect {
		mechanic: MechanicId::new("enemy_on_death"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::Damage {
			amount: value(10),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	});
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "target")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("enemy on-death effects are supported");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.status(), &ExpectedTerminalStatus::Victory);
	assert_eq!(
		replay.operators()[&operator].remaining_hp,
		value(90),
		"terminal victory must not skip the final enemy's same-tick on-death damage"
	);
}

#[test]
fn compiler_rejects_hostile_summons_and_hostile_preplaced_actors() {
	let hostile = SummonTemplateId::new(7);
	let mut facts = base_facts(10);
	facts.summons = vec![summon_template(hostile, "hostile_add", Faction::Hostile)];
	facts.preplaced_actors = vec![PreplacedActorFacts {
		instance: 9,
		template: hostile,
		cell: GridPosition { row: 0, col: 2 },
		facing: Facing::Left,
	}];

	let report = compile_battle(&facts, &MaterializedGuide::default())
		.expect_err("hostile summons need objective semantics");

	assert!(report
		.issues()
		.contains(&BattleCompileIssue::UnsupportedHostileSummon {
			template: hostile,
			key: "hostile_add".to_owned(),
		}));
	assert!(report
		.issues()
		.contains(&BattleCompileIssue::UnsupportedHostilePreplacedActor {
			instance: 9,
			template: hostile,
		}));
}

#[test]
fn terminal_clearance_with_a_required_action_waiting_for_dp_is_incomplete() {
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(100);
	facts.dp = ExpectedDpRules {
		initial: 0,
		maximum: 0,
		regen_amount: 0,
		regen_interval_ticks: 1,
	};
	facts.enemies[0].on_spawn.push(TaggedEffect {
		mechanic: MechanicId::new("self_destruct"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Death,
	});
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 1, "waiting_operator")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 1 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("self-destruct is supported");

	let assessment = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		assessment.status(),
		&ExpectedTerminalStatus::SimulationIncomplete
	);
	assert_eq!(
		assessment.action_audit()[0].status,
		ActionAuditStatus::WaitedForDp
	);
}

#[test]
fn simulation_horizon_is_reported_as_incomplete_not_combat_defeat() {
	let mut facts = base_facts(100);
	facts.end_at = Some(SimTick::new(10));
	facts.paths = vec![path(1, 100, 1)];
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide: MaterializedGuide::default(),
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [1; 32], "variant");

	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
	assert_eq!(
		output.artifact.battlefield_status,
		ExpectedStrategyStatus::Incomplete
	);
	assert!(output
		.report
		.terminal
		.as_deref()
		.is_some_and(|terminal| terminal.starts_with("Deadline")));
}

#[test]
fn unsupported_json_report_retains_full_materialized_action_fields() {
	let operator = OperatorInstanceId::new(3);
	let mut facts = base_facts(100);
	facts.reached_mechanics.push(ReachedMechanic {
		id: MechanicId::new("unsupported_stage_rule"),
		origin: "stage:test".to_owned(),
		behavior: MechanicBehavior::Unsupported {
			reason: "test-only unknown".to_owned(),
		},
	});
	facts.reached_mechanics.push(ReachedMechanic {
		id: MechanicId::new("unmodeled_stage_rule"),
		origin: "stage:test".to_owned(),
		behavior: MechanicBehavior::UnmodeledOmission {
			reason: "test-only omitted effect".to_owned(),
		},
	});
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 1, "char_test")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(4),
			operator,
			cell: GridPosition { row: 0, col: 2 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide,
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [2; 32], "unsupported-variant");
	let action = &output.report.actions[0];

	assert_eq!(
		output.report.schema_version,
		EXPECTED_REPLAY_REPORT_SCHEMA_VERSION
	);
	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::Unsupported
	);
	assert_eq!(action.operator_id.as_deref(), Some("char_test"));
	assert_eq!(action.row, Some(0));
	assert_eq!(action.column, Some(2));
	assert_eq!(action.facing, ArtifactFacing::Left);
	assert_eq!(action.requested_tick, 4);
	assert_eq!(action.executed_tick, None);
	assert_eq!(action.status, "not_executed_unsupported");
	assert_eq!(output.report.unmodeled_omissions.len(), 1);
	assert_eq!(
		output.report.unmodeled_omissions[0].mechanic_id,
		"unmodeled_stage_rule"
	);
}

#[test]
fn supported_json_report_retains_full_materialized_action_fields() {
	let operator = OperatorInstanceId::new(4);
	let mut facts = base_facts(100);
	facts
		.grid
		.deployable_cells
		.insert(GridPosition { row: 0, col: 0 });
	facts.enemies[0].on_spawn.push(TaggedEffect {
		mechanic: MechanicId::new("self_destruct"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Death,
	});
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "char_supported")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 0 },
			facing: Facing::Up,
		}],
		reached_mechanics: Vec::new(),
	};
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide,
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [3; 32], "supported-variant");
	let action = &output.report.actions[0];
	let report_json = serde_json::to_value(&output.report).expect("report is serializable");

	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedVictory
	);
	assert_eq!(action.operator_id.as_deref(), Some("char_supported"));
	assert_eq!(action.row, Some(0));
	assert_eq!(action.column, Some(0));
	assert_eq!(action.facing, ArtifactFacing::Up);
	assert_eq!(action.requested_tick, 0);
	assert_eq!(action.executed_tick, Some(0));
	assert_eq!(action.status, "executed");
	assert_eq!(report_json["blocks"][0]["enemy_instance_id"], 1);
	assert_eq!(report_json["blocks"][0]["blocker_kind"], "operator");
	assert_eq!(report_json["blocks"][0]["blocker_instance_id"], 4);
	assert_eq!(report_json["blocks"][0]["start_tick"], 0);
	assert_eq!(report_json["blocks"][0]["end_tick"], 0);
	assert_eq!(report_json["blocks"][0]["end_reason"], "enemy_death");
}

#[test]
fn conservative_beneficial_omission_preserves_modeled_victory() {
	let operator = OperatorInstanceId::new(5);
	let mut facts = base_facts(100);
	facts
		.grid
		.deployable_cells
		.insert(GridPosition { row: 0, col: 0 });
	facts.enemies[0].on_spawn.push(TaggedEffect {
		mechanic: MechanicId::new("self_destruct"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Death,
	});
	facts.reached_mechanics.push(ReachedMechanic {
		id: MechanicId::new("omitted_friendly_buff"),
		origin: "operator:test".to_owned(),
		behavior: MechanicBehavior::ConservativeBeneficialOmission {
			reason: "test-only friendly benefit".to_owned(),
		},
	});
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "char_supported")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 0 },
			facing: Facing::Up,
		}],
		reached_mechanics: Vec::new(),
	};
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide,
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [4; 32], "omission-variant");
	let probability = output
		.report
		.defensive_probability
		.as_ref()
		.expect("executed replay has a probability audit");

	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedVictory
	);
	assert_eq!(
		output.artifact.battlefield_status,
		ExpectedStrategyStatus::ExpectedVictory
	);
	assert_eq!(
		output.artifact.damage_status,
		DeterministicDamageStatus::Feasible
	);
	assert!(output.report.unsupported.is_empty());
	assert_eq!(output.report.conservative_omissions.len(), 1);
	assert!(output.report.unmodeled_omissions.is_empty());
	assert!(probability.verified);
}

#[test]
fn unmodeled_omission_allows_victory_execution_but_forces_incomplete_audit() {
	let operator = OperatorInstanceId::new(6);
	let mut facts = base_facts(100);
	facts
		.grid
		.deployable_cells
		.insert(GridPosition { row: 0, col: 0 });
	facts.enemies[0].on_spawn.push(TaggedEffect {
		mechanic: MechanicId::new("self_destruct"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Death,
	});
	facts.reached_mechanics.push(ReachedMechanic {
		id: MechanicId::new("unmodeled_stage_effect"),
		origin: "stage:test".to_owned(),
		behavior: MechanicBehavior::UnmodeledOmission {
			reason: "test-only omitted effect".to_owned(),
		},
	});
	let guide = MaterializedGuide {
		builds: vec![operator_build(operator, 0, "char_supported")],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 0 },
			facing: Facing::Up,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("unmodeled omission remains executable");
	assert!(
		evaluate_defensive_probability(&battle, MaterializedGuidePolicy::default()).verified(),
		"the omission, rather than replay evaluation, must invalidate verification"
	);
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide,
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [5; 32], "unmodeled-victory");
	let probability = output
		.report
		.defensive_probability
		.as_ref()
		.expect("executed replay has a probability audit");
	let report_json = serde_json::to_value(&output.report).expect("report is serializable");

	assert_eq!(EXPECTED_REPLAY_REPORT_SCHEMA_VERSION, 11);
	assert_eq!(output.report.schema_version, 11);
	assert_eq!(output.report.terminal.as_deref(), Some("Victory"));
	assert_eq!(output.report.actions[0].status, "executed");
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
	assert_eq!(
		output.artifact.battlefield_status,
		ExpectedStrategyStatus::Incomplete
	);
	assert!(output.report.unsupported.is_empty());
	assert!(output.report.conservative_omissions.is_empty());
	assert_eq!(output.report.unmodeled_omissions.len(), 1);
	assert_eq!(
		output.report.unmodeled_omissions[0].mechanic_id,
		"unmodeled_stage_effect"
	);
	assert_eq!(output.report.unmodeled_omissions[0].origins, ["stage:test"]);
	assert_eq!(
		output.report.unmodeled_omissions[0].reasons,
		["test-only omitted effect"]
	);
	assert!(!probability.verified);
	assert_eq!(report_json["schema_version"], 11);
	assert_eq!(
		report_json["unmodeled_omissions"][0]["mechanic_id"],
		"unmodeled_stage_effect"
	);
}

/// Replays `facts` with `guide`, where the single enemy is expected to reach its goal.
fn replay_goal_leak(
	facts: ArkBattleFacts,
	guide: MaterializedGuide,
	variant_id: &str,
) -> ExpectedReplayOutput {
	let battle = compile_battle(&facts, &guide).expect("the leaking battle remains executable");
	assert!(
		evaluate_defensive_probability(&battle, MaterializedGuidePolicy::default()).verified(),
		"replay evaluation itself must not withhold the verdict"
	);
	let input = ExpectedReplayInput {
		data_revision: "test-revision".to_owned(),
		mechanics_revision: "a".repeat(64),
		stage_id: "test-stage".to_owned(),
		enemy_lineage: BTreeMap::new(),
		resolved_builds: Vec::new(),
		facts,
		guide,
		policy: MaterializedGuidePolicy::default(),
	};

	let output = execute_expected_replay(&input, [6; 32], variant_id);

	assert!(output
		.report
		.terminal
		.as_deref()
		.is_some_and(|terminal| terminal.starts_with("GoalLeak")));
	assert!(output.report.enemies[0].leaked_tick.is_some());
	assert!(output.report.unsupported.is_empty());
	output
}

fn assert_defeat_withheld(output: &ExpectedReplayOutput) {
	assert_eq!(output.report.status, ExpectedReplayReportStatus::Incomplete);
	assert_eq!(
		output.artifact.battlefield_status,
		ExpectedStrategyStatus::Incomplete
	);
	assert_eq!(
		output.artifact.damage_status,
		DeterministicDamageStatus::Unsupported
	);
}

#[test]
fn goal_leak_without_omissions_certifies_expected_defeat() {
	let output = replay_goal_leak(
		base_facts(100),
		MaterializedGuide::default(),
		"complete-goal-leak",
	);

	assert!(output.report.conservative_omissions.is_empty());
	assert!(output.report.unmodeled_omissions.is_empty());
	assert_eq!(
		output.report.status,
		ExpectedReplayReportStatus::ExpectedDefeat
	);
	assert_eq!(
		output.artifact.battlefield_status,
		ExpectedStrategyStatus::ExpectedDefeat
	);
	assert_eq!(
		output.artifact.damage_status,
		DeterministicDamageStatus::Infeasible
	);
}

#[test]
fn unmodeled_omission_withholds_modeled_goal_leak_defeat() {
	let mut facts = base_facts(100);
	facts.enemies[0].reached_mechanics.push(ReachedMechanic {
		id: MechanicId::new("unmodeled_enemy_effect"),
		origin: "enemy:test".to_owned(),
		behavior: MechanicBehavior::UnmodeledOmission {
			reason: "test-only omitted enemy effect".to_owned(),
		},
	});

	let output = replay_goal_leak(facts, MaterializedGuide::default(), "unmodeled-goal-leak");
	let probability = output
		.report
		.defensive_probability
		.as_ref()
		.expect("executed replay has a probability audit");

	// Even an omitted enemy effect has no direction: it could hold the enemy back as easily as
	// hurry it along.
	assert_defeat_withheld(&output);
	assert!(output.report.conservative_omissions.is_empty());
	assert_eq!(output.report.unmodeled_omissions.len(), 1);
	assert_eq!(
		output.report.unmodeled_omissions[0].mechanic_id,
		"unmodeled_enemy_effect"
	);
	assert!(!probability.verified);
}

#[test]
fn conservative_beneficial_omission_withholds_modeled_goal_leak_defeat() {
	let operator = OperatorInstanceId::new(7);
	let mut build = operator_build(operator, 0, "char_supported");
	build.reached_mechanics.push(ReachedMechanic {
		id: MechanicId::new("omitted_friendly_buff"),
		origin: "operator:test".to_owned(),
		behavior: MechanicBehavior::ConservativeBeneficialOmission {
			reason: "test-only friendly benefit".to_owned(),
		},
	});
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: Vec::new(),
		reached_mechanics: Vec::new(),
	};

	let output = replay_goal_leak(base_facts(100), guide, "conservative-goal-leak");
	let probability = output
		.report
		.defensive_probability
		.as_ref()
		.expect("executed replay has a probability audit");

	// The omitted benefit only understates the strategy, which keeps a clear certifiable but
	// leaves open whether the modelled benefit would have stopped this leak.
	assert_defeat_withheld(&output);
	assert!(output.report.unmodeled_omissions.is_empty());
	assert_eq!(output.report.conservative_omissions.len(), 1);
	assert_eq!(
		output.report.conservative_omissions[0].mechanic_id,
		"omitted_friendly_buff"
	);
	assert!(
		probability.verified,
		"only certification, not the victory probability, is withheld"
	);
}
