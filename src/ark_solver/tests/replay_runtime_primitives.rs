use std::collections::{BTreeMap, BTreeSet};

use ark_solver::data::{
	Buildable, GridPosition, MapDimensions, Passability, RangeOffset, TileHeight,
};
use ark_solver::simulation::{ModelError, SimTick, SimulationIssue, TraceEntry};
use ark_solver::{
	mechanics::domain::{
		ActorCombatFacts, ArkBattleFacts, AttackCadenceModifier, AttackPattern, AttackProfile,
		AttackRangeModifier, AttackSpeedModifier, BattleCompileIssue, BattleGrid,
		BattleGridCellMetadata, BattlePath, BattlePathStep, BattlePathStepAction, BattleTimeAnchor,
		BattleWave, BlockCountModifier, BlockTargetFilter, BuffModifier, BuffRemovalSource,
		ConditionalAuraPeriodicEffect, ContributionMergePolicy, DamageDelivery, DamageType,
		EffectTarget, EnemyFacts, EnemyInstanceId, EnemySpawnFacts, ExpectedDeploymentRules,
		ExpectedDpRules, ExpectedEffect, ExpectedValue, Facing, GoalId, MaterializedGuide,
		MaterializedGuideAction, MaterializedGuidePolicy, MechanicId, NormalAttackAction,
		NormalAttackTargetFilter, OperatorBuildFacts, OperatorClassificationFacts,
		OperatorInstanceId, PathId, PeriodicTileEffect, PreplacedActorFacts, PulseSourceOrder,
		RelativeBattleTime, ReplayCellSet, ReplayListener, ReplayPredicate, ReplayRepeat,
		ReplayTrigger, ReplayTriggerKind, RuntimeBuff, RuntimeCachedTileLayerPropagation,
		RuntimeConditionalAura, RuntimeEffectLifetime, RuntimeSkillInactiveStance,
		RuntimeTileEffect, RuntimeTileFieldOverlay, RuntimeTileLayerMutationCause,
		RuntimeTileLayerPropagationAudit, RuntimeTileLayerThresholdReaction, ScheduledStageEffect,
		SkillActivationRule, SkillFacts, SpChargeRule, StatKind, StatModifier, StatModifierBucket,
		StatusEffect, SummonCellOccupancy, SummonEffect, SummonTemplate, SummonTemplateId,
		TaggedEffect, TargetCellActorSpawn, TargetCellSpawnCollision, TargetCellSpawnOwnership,
		TargetClass, TargetCountModifier, Targetability, TeamCompositionCondition,
		TileFieldOverlayLeaveScope, TileLayerThresholdReactionEffect, TileLayerThresholdTransition,
		TriggerActorFilter, WaveId, WaveStartWindow,
	},
	replay::{
		compile_battle, replay_expected, replay_expected_to_resolution, BlockEndReason,
		ExpectedEvent, ExpectedTerminalStatus,
	},
};

fn value(value: u32) -> ExpectedValue {
	ExpectedValue::from_whole(value)
}

fn combat(max_hp: u32) -> ActorCombatFacts {
	ActorCombatFacts {
		max_hp: value(max_hp),
		attack: ExpectedValue::ZERO,
		defense: ExpectedValue::ZERO,
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

fn path(steps: Vec<BattlePathStep>) -> BattlePath {
	BattlePath {
		id: PathId::new(1),
		goal: GoalId::new(1),
		start: GridPosition { row: 0, col: 0 },
		steps,
	}
}

fn base_facts(steps: Vec<BattlePathStep>, enemy_on_spawn: Vec<TaggedEffect>) -> ArkBattleFacts {
	ArkBattleFacts {
		battle_id: "runtime-primitives".to_owned(),
		end_at: Some(SimTick::new(8)),
		dp: ExpectedDpRules {
			initial: 10,
			maximum: 10,
			regen_amount: 0,
			regen_interval_ticks: 1,
		},
		deployment: ExpectedDeploymentRules {
			simultaneous_limit: 1,
			second_deployment_cost_multiplier: ExpectedValue::ONE,
			later_deployment_cost_multiplier: ExpectedValue::ONE,
			retreat_refund_ratio: ExpectedValue::ZERO,
		},
		grid: BattleGrid {
			dimensions: MapDimensions {
				width: 4,
				height: 1,
			},
			cells: BTreeMap::new(),
			deployable_cells: BTreeSet::from([GridPosition { row: 0, col: 3 }]),
			ground_cells: BTreeSet::from([
				GridPosition { row: 0, col: 0 },
				GridPosition { row: 0, col: 1 },
				GridPosition { row: 0, col: 2 },
				GridPosition { row: 0, col: 3 },
			]),
		},
		paths: vec![path(steps)],
		waves: vec![BattleWave {
			id: WaveId::new(1),
			start: WaveStartWindow {
				earliest: RelativeBattleTime {
					anchor: BattleTimeAnchor::BattleStarted,
					offset_ticks: 0,
				},
				latest: None,
			},
			spawns: vec![EnemySpawnFacts {
				id: EnemyInstanceId::new(1),
				enemy_key: "enemy".to_owned(),
				after_wave_start_ticks: 0,
				path: PathId::new(1),
				facing: Facing::Right,
			}],
		}],
		enemies: vec![EnemyFacts {
			enemy_key: "enemy".to_owned(),
			block_demand: 1,
			mass: 1,
			combat: combat(100),
			attack: inert_attack(),
			on_spawn: enemy_on_spawn,
			on_death: Vec::new(),
			reached_mechanics: Vec::new(),
		}],
		summons: Vec::new(),
		preplaced_actors: Vec::new(),
		stage_effects: Vec::new(),
		reached_mechanics: Vec::new(),
	}
}

fn operator_build(on_deploy: Vec<TaggedEffect>) -> OperatorBuildFacts {
	operator_build_for(OperatorInstanceId::new(1), on_deploy)
}

fn operator_build_for(id: OperatorInstanceId, on_deploy: Vec<TaggedEffect>) -> OperatorBuildFacts {
	OperatorBuildFacts {
		id,
		operator_key: format!("operator_{}", id.get()),
		classification: OperatorClassificationFacts::default(),
		on_battle_start: Vec::new(),
		deployment_cost: 0,
		redeployment_cooldown_ticks: 1,
		block_count: 0,
		block_target_filter: BlockTargetFilter::GroundOnly,
		combat: combat(100),
		attack: inert_attack(),
		skill: None,
		on_deploy,
		on_death: Vec::new(),
		reached_mechanics: Vec::new(),
	}
}

fn guide(on_deploy: Vec<TaggedEffect>, retreat_at: Option<u64>) -> MaterializedGuide {
	let operator = OperatorInstanceId::new(1);
	let mut actions = vec![MaterializedGuideAction::Deploy {
		requested_at: SimTick::new(0),
		operator,
		cell: GridPosition { row: 0, col: 3 },
		facing: Facing::Right,
	}];
	if let Some(at) = retreat_at {
		actions.push(MaterializedGuideAction::Retreat {
			requested_at: SimTick::new(at),
			operator,
		});
	}
	MaterializedGuide {
		builds: vec![operator_build(on_deploy)],
		actions,
		reached_mechanics: Vec::new(),
	}
}

fn damage(mechanic: &str, amount: u32) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::Damage {
			amount: value(amount),
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}
}

fn listener(
	key: &str,
	trigger: ReplayTrigger,
	repeat: ReplayRepeat,
	effects: Vec<TaggedEffect>,
) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(key),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::RegisterListener(ReplayListener {
			key: MechanicId::new(key),
			trigger,
			actor_filter: TriggerActorFilter::EnemyOfSource,
			predicate: ReplayPredicate::Always,
			repeat,
			lifetime: RuntimeEffectLifetime::SourceActor,
			effects,
		}),
	}
}

fn long_wait_path() -> Vec<BattlePathStep> {
	vec![
		BattlePathStep {
			duration_ticks: 100,
			affected_by_movement_speed: true,
			action: BattlePathStepAction::Wait,
		},
		BattlePathStep {
			duration_ticks: 1,
			affected_by_movement_speed: true,
			action: BattlePathStepAction::Goal {
				goal: GoalId::new(1),
			},
		},
	]
}

#[test]
fn cell_entry_and_exit_fire_exactly_once_per_boundary_crossing() {
	let watched = ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 1 }]));
	let on_deploy = vec![
		listener(
			"enter",
			ReplayTrigger::ActorEnteredCells {
				cells: watched.clone(),
			},
			ReplayRepeat::Unlimited,
			vec![damage("enter_damage", 10)],
		),
		listener(
			"leave",
			ReplayTrigger::ActorLeftCells { cells: watched },
			ReplayRepeat::Unlimited,
			vec![damage("leave_damage", 1)],
		),
	];
	let facts = base_facts(
		vec![
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
				action: BattlePathStepAction::Move {
					cell: GridPosition { row: 0, col: 2 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: 20,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Goal {
					goal: GoalId::new(1),
				},
			},
		],
		Vec::new(),
	);
	let battle = compile_battle(&facts, &guide(on_deploy, None)).expect("valid listeners");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(89)
	);
}

fn tile_effect() -> RuntimeTileEffect {
	RuntimeTileEffect {
		key: MechanicId::new("snow"),
		cells: ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 0 }])),
		actor_filter: TriggerActorFilter::EnemyOfSource,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		max_layers: 3,
		duration_ticks: None,
		lifetime: RuntimeEffectLifetime::SourceActor,
		apply_new_layer_to_occupants: false,
		on_enter_once: Vec::new(),
		on_enter_per_layer: Vec::new(),
		on_leave_per_layer: Vec::new(),
		on_remove_per_layer: Vec::new(),
		layer_threshold_reactions: Vec::new(),
		remove_on_first_matching_leave: false,
		periodic: Some(PeriodicTileEffect {
			initial_delay_ticks: 1,
			interval_ticks: 2,
			effects_per_layer: vec![damage("snow_tick", 3)],
		}),
	}
}

fn add_tile(tile: RuntimeTileEffect) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new("snow"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::AddTileLayer(tile),
	}
}

fn spawn_reaction(
	template: u32,
	threshold: u16,
	transition: TileLayerThresholdTransition,
	collision: TargetCellSpawnCollision,
	ownership: TargetCellSpawnOwnership,
) -> RuntimeTileLayerThresholdReaction {
	RuntimeTileLayerThresholdReaction {
		threshold,
		transition,
		effect: TileLayerThresholdReactionEffect::SpawnActor(TargetCellActorSpawn {
			template: SummonTemplateId::new(template),
			collision,
			ownership,
		}),
	}
}

fn tile_field_overlay(
	key: &str,
	observed_tile_key: &str,
	leave_scope: TileFieldOverlayLeaveScope,
	initial_delay_ticks: u64,
	interval_ticks: u64,
	periodic_effects: Vec<TaggedEffect>,
	on_leave_once: Vec<TaggedEffect>,
) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(key),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::InstallTileFieldOverlay(RuntimeTileFieldOverlay {
			key: MechanicId::new(key),
			observed_tile_key: MechanicId::new(observed_tile_key),
			observed_tile_max_layers: None,
			actor_filter: TriggerActorFilter::EnemyOfSource,
			target_filter: NormalAttackTargetFilter::GroundOnly,
			leave_scope,
			initial_delay_ticks,
			interval_ticks,
			lifetime: RuntimeEffectLifetime::SkillActivation,
			periodic_effects,
			on_leave_once,
			layer_threshold_reactions: Vec::new(),
			cached_propagation: None,
		}),
	}
}

fn walkable_grid(width: u32, height: u32) -> BTreeMap<GridPosition, BattleGridCellMetadata> {
	(0..height)
		.flat_map(|row| {
			(0..width).map(move |col| {
				let position: GridPosition = GridPosition { row, col };
				(
					position,
					BattleGridCellMetadata::new(
						position,
						"tile_road",
						TileHeight::Lowland,
						Buildable::All,
						Passability::All,
					),
				)
			})
		})
		.collect()
}

fn cached_propagation_overlay(
	maximum_outside_temporary_cells: u16,
	contribution_merge: ContributionMergePolicy,
) -> TaggedEffect {
	let mut overlay: TaggedEffect = tile_field_overlay(
		"cached_snow_overlay",
		"cached_snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		100,
		100,
		Vec::new(),
		Vec::new(),
	);
	let ExpectedEffect::InstallTileFieldOverlay(spec) = &mut overlay.effect else {
		panic!("overlay helper must install an overlay");
	};
	spec.cached_propagation = Some(RuntimeCachedTileLayerPropagation {
		maximum_outside_temporary_cells,
		source_order: PulseSourceOrder::Unknown,
		contribution_merge,
	});
	overlay
}

fn cached_propagation_tile(cells: ReplayCellSet) -> RuntimeTileEffect {
	RuntimeTileEffect {
		key: MechanicId::new("cached_snow"),
		cells,
		actor_filter: TriggerActorFilter::EnemyOfSource,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		max_layers: 1,
		duration_ticks: None,
		lifetime: RuntimeEffectLifetime::SourceActor,
		apply_new_layer_to_occupants: true,
		on_enter_once: Vec::new(),
		on_enter_per_layer: Vec::new(),
		on_leave_per_layer: Vec::new(),
		on_remove_per_layer: Vec::new(),
		layer_threshold_reactions: Vec::new(),
		remove_on_first_matching_leave: false,
		periodic: None,
	}
}

fn cached_propagation_scenario(
	dimensions: MapDimensions,
	deployed_at: GridPosition,
	initial_cells: BTreeSet<GridPosition>,
	maximum_outside_temporary_cells: u16,
	contribution_merge: ContributionMergePolicy,
) -> (ArkBattleFacts, MaterializedGuide) {
	let initial: TaggedEffect = add_tile(cached_propagation_tile(ReplayCellSet::Explicit(
		initial_cells,
	)));
	let pulse: TaggedEffect = add_tile(cached_propagation_tile(
		ReplayCellSet::SourceCurrentAttackRangeGroundCells,
	));
	let generator: TaggedEffect = listener(
		"cached_snow_generator",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 1,
			interval_ticks: 100,
		},
		ReplayRepeat::Once,
		vec![pulse],
	);
	let mut build: OperatorBuildFacts = operator_build(vec![
		initial,
		cached_propagation_overlay(maximum_outside_temporary_cells, contribution_merge),
		generator,
	]);
	build.attack.range = vec![RangeOffset::new(0, 0)];
	let guide: MaterializedGuide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: OperatorInstanceId::new(1),
			cell: deployed_at,
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(2));
	facts.grid.dimensions = dimensions;
	facts.grid.cells = walkable_grid(dimensions.width, dimensions.height);
	facts.grid.deployable_cells = BTreeSet::from([deployed_at]);
	facts.grid.ground_cells = facts.grid.cells.keys().copied().collect();
	(facts, guide)
}

#[test]
fn cached_propagation_uses_d_l_u_r_neighbors_without_same_pulse_recursion() {
	let center: GridPosition = GridPosition { row: 1, col: 1 };
	let (facts, guide) = cached_propagation_scenario(
		MapDimensions {
			width: 3,
			height: 3,
		},
		center,
		BTreeSet::from([center]),
		20,
		ContributionMergePolicy::Unknown,
	);
	let battle = compile_battle(&facts, &guide).expect("valid cached propagation battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let added_by_propagation: BTreeSet<GridPosition> = replay
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| {
			let TraceEntry::Event { scheduled } = entry else {
				return None;
			};
			let ExpectedEvent::TileLayerPropagationAudit {
				event:
					RuntimeTileLayerPropagationAudit::LayersAdded {
						cell,
						cause: RuntimeTileLayerMutationCause::CachedPropagation { .. },
						..
					},
				..
			} = scheduled.event()
			else {
				return None;
			};
			Some(*cell)
		})
		.collect();

	assert_eq!(
		added_by_propagation,
		BTreeSet::from([
			GridPosition { row: 0, col: 1 },
			GridPosition { row: 1, col: 0 },
			GridPosition { row: 1, col: 2 },
			GridPosition { row: 2, col: 1 },
		])
	);
}

#[test]
fn cached_propagation_uses_the_same_target_cell_threshold_reaction_path() {
	let center: GridPosition = GridPosition { row: 1, col: 1 };
	let (mut facts, mut guide) = cached_propagation_scenario(
		MapDimensions {
			width: 3,
			height: 3,
		},
		center,
		BTreeSet::from([center]),
		20,
		ContributionMergePolicy::Unknown,
	);
	let reaction: RuntimeTileLayerThresholdReaction = spawn_reaction(
		0,
		1,
		TileLayerThresholdTransition::ReachedFromBelow,
		TargetCellSpawnCollision::AllowCoexistence,
		TargetCellSpawnOwnership::Independent,
	);
	for effect in &mut guide.builds[0].on_deploy {
		if let ExpectedEffect::InstallTileFieldOverlay(overlay) = &mut effect.effect {
			overlay.observed_tile_max_layers = Some(1);
			overlay.layer_threshold_reactions = vec![reaction];
		}
	}
	facts.summons = vec![friendly_summon(0, None)];
	let battle = compile_battle(&facts, &guide).expect("valid cached target-cell reactions");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let spawned_cells: BTreeSet<GridPosition> = replay
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| {
			let TraceEntry::Event { scheduled } = entry else {
				return None;
			};
			let ExpectedEvent::TargetCellActorSpawn(event) = scheduled.event() else {
				return None;
			};
			Some(event.cell)
		})
		.collect();

	assert_eq!(
		spawned_cells,
		BTreeSet::from([
			GridPosition { row: 0, col: 1 },
			GridPosition { row: 1, col: 0 },
			GridPosition { row: 1, col: 2 },
			GridPosition { row: 2, col: 1 },
		])
	);
}

#[test]
fn cached_propagation_unknown_source_order_fails_closed() {
	let (facts, guide) = cached_propagation_scenario(
		MapDimensions {
			width: 5,
			height: 1,
		},
		GridPosition { row: 0, col: 1 },
		BTreeSet::from([
			GridPosition { row: 0, col: 1 },
			GridPosition { row: 0, col: 3 },
		]),
		1,
		ContributionMergePolicy::DeduplicateDestination,
	);
	let battle = compile_battle(&facts, &guide).expect("valid ambiguous propagation battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert!(replay.issues().iter().any(|issue| matches!(
		issue,
		SimulationIssue::Model(ModelError::Unsupported(detail))
			if detail.contains("SourceOrderChangesBudgetSelection")
	)));
}

#[test]
fn cached_propagation_unknown_multi_source_merge_fails_closed() {
	let (facts, guide) = cached_propagation_scenario(
		MapDimensions {
			width: 3,
			height: 1,
		},
		GridPosition { row: 0, col: 0 },
		BTreeSet::from([
			GridPosition { row: 0, col: 0 },
			GridPosition { row: 0, col: 2 },
		]),
		20,
		ContributionMergePolicy::Unknown,
	);
	let battle = compile_battle(&facts, &guide).expect("valid ambiguous merge battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert!(replay.issues().iter().any(|issue| matches!(
		issue,
		SimulationIssue::Model(ModelError::Unsupported(detail))
			if detail.contains("ContributionMergeUnknown")
	)));
}

#[test]
fn cached_propagation_skill_end_removes_temporary_but_not_permanent_cells() {
	let mut initial_tile: RuntimeTileEffect =
		cached_propagation_tile(ReplayCellSet::Explicit(BTreeSet::from([GridPosition {
			row: 0,
			col: 1,
		}])));
	initial_tile.on_remove_per_layer = vec![damage("snow_cleanup", 10)];
	let mut pulse_tile: RuntimeTileEffect = initial_tile.clone();
	pulse_tile.cells = ReplayCellSet::SourceCurrentAttackRangeGroundCells;
	let generator: TaggedEffect = listener(
		"cached_snow_generator",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 2,
			interval_ticks: 100,
		},
		ReplayRepeat::Once,
		vec![add_tile(pulse_tile)],
	);
	let build: OperatorBuildFacts = operator_build(vec![add_tile(initial_tile), generator]);
	let guide: MaterializedGuide = skill_overlay_guide(
		build,
		cached_propagation_overlay(1, ContributionMergePolicy::Unknown),
		2,
	);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(4));
	facts.grid.cells = walkable_grid(4, 1);
	facts.grid.ground_cells = facts.grid.cells.keys().copied().collect();
	let battle = compile_battle(&facts, &guide).expect("valid temporary cleanup battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(90),
		"only the temporary snow on the occupied cell should be removed at skill end"
	);
}

#[test]
fn cached_propagation_owner_death_removes_permanent_cells() {
	let mut initial_tile: RuntimeTileEffect =
		cached_propagation_tile(ReplayCellSet::Explicit(BTreeSet::from([GridPosition {
			row: 0,
			col: 0,
		}])));
	initial_tile.on_remove_per_layer = vec![damage("snow_owner_cleanup", 10)];
	let mut pulse_tile: RuntimeTileEffect = initial_tile.clone();
	pulse_tile.cells = ReplayCellSet::SourceCurrentAttackRangeGroundCells;
	let generator: TaggedEffect = listener(
		"cached_snow_generator",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 1,
			interval_ticks: 100,
		},
		ReplayRepeat::Once,
		vec![add_tile(pulse_tile)],
	);
	let on_deploy: Vec<TaggedEffect> = vec![
		add_tile(initial_tile),
		cached_propagation_overlay(1, ContributionMergePolicy::Unknown),
		generator,
	];
	let guide: MaterializedGuide = guide(on_deploy, None);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(3));
	facts.grid.cells = walkable_grid(4, 1);
	facts.grid.ground_cells = facts.grid.cells.keys().copied().collect();
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(2),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("kill_snow_owner"),
			target: EffectTarget::AllAllies,
			effect: ExpectedEffect::Death,
		},
	}];
	let battle = compile_battle(&facts, &guide).expect("valid owner cleanup battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(90),
		"owner death should remove the occupied permanent snow cell"
	);
}

fn with_overlay_skill(
	mut build: OperatorBuildFacts,
	overlay: TaggedEffect,
	active_duration_ticks: u64,
) -> OperatorBuildFacts {
	let operator: OperatorInstanceId = build.id;
	build.skill = Some(SkillFacts {
		id: MechanicId::new(format!("overlay_skill_{}", operator.get())),
		activation: SkillActivationRule::Manual,
		initial_sp: 1,
		sp_cost: 1,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(active_duration_ticks),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![overlay],
	});
	build
}

fn skill_overlay_guide(
	build: OperatorBuildFacts,
	overlay: TaggedEffect,
	active_duration_ticks: u64,
) -> MaterializedGuide {
	let build: OperatorBuildFacts = with_overlay_skill(build, overlay, active_duration_ticks);
	let operator: OperatorInstanceId = build.id;
	MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	}
}

fn skill_inactive_stance(
	key: &str,
	disable_normal_attacks: bool,
	modifiers: Vec<BuffModifier>,
) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(key),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::InstallSkillInactiveStance(RuntimeSkillInactiveStance {
			key: MechanicId::new(key),
			disable_normal_attacks,
			modifiers,
		}),
	}
}

fn with_empty_manual_skill(
	mut build: OperatorBuildFacts,
	active_duration_ticks: u64,
) -> OperatorBuildFacts {
	build.skill = Some(SkillFacts {
		id: MechanicId::new("manual_skill"),
		activation: SkillActivationRule::Manual,
		initial_sp: 1,
		sp_cost: 1,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(active_duration_ticks),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: Vec::new(),
	});
	build
}

fn one_skill_activation_guide(build: OperatorBuildFacts, activate_at: u64) -> MaterializedGuide {
	let operator: OperatorInstanceId = build.id;
	MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Left,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(activate_at),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	}
}

#[test]
fn skill_inactive_stance_suppresses_normal_attacks_only_outside_skill() {
	let stance: TaggedEffect = skill_inactive_stance("phalanx", true, Vec::new());
	let mut build: OperatorBuildFacts = operator_build(vec![stance]);
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let guide: MaterializedGuide = one_skill_activation_guide(with_empty_manual_skill(build, 2), 2);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(5));
	let battle = compile_battle(&facts, &guide).expect("valid skill-inactive attack stance");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(90)
	);
}

fn remaining_operator_hp_against_stance(
	damage_type: DamageType,
	base_arts_resistance: ExpectedValue,
) -> ExpectedValue {
	let stance: TaggedEffect = skill_inactive_stance(
		"phalanx",
		true,
		vec![
			BuffModifier::Stat(StatModifier {
				bucket: StatModifierBucket::Scale,
				stat: StatKind::Defense,
				multiplier: value(2),
				duration_ticks: None,
			}),
			BuffModifier::ArtsResistanceAddition {
				ratio: ExpectedValue::from_ratio(1, 5).expect("valid RES ratio"),
			},
		],
	);
	let mut build: OperatorBuildFacts = operator_build(vec![stance]);
	build.combat = combat(500);
	build.combat.defense = value(10);
	build.combat.arts_resistance = base_arts_resistance;
	let guide: MaterializedGuide = one_skill_activation_guide(with_empty_manual_skill(build, 2), 2);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(6));
	facts.enemies[0].combat.attack = value(100);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 2,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid skill-inactive defensive stance");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.operators()[&OperatorInstanceId::new(1)].remaining_hp
}

#[test]
fn skill_inactive_stance_toggles_defense_and_additive_arts_resistance() {
	assert_eq!(
		remaining_operator_hp_against_stance(DamageType::Physical, ExpectedValue::ZERO),
		value(250)
	);
	assert_eq!(
		remaining_operator_hp_against_stance(
			DamageType::Arts,
			ExpectedValue::from_ratio(1, 10).expect("valid base RES ratio"),
		),
		value(270)
	);
}

#[test]
fn skill_inactive_stance_rejects_invalid_contracts() {
	let invalid_empty = TaggedEffect {
		mechanic: MechanicId::new("invalid_empty_stance"),
		target: EffectTarget::AllEnemies,
		effect: ExpectedEffect::InstallSkillInactiveStance(RuntimeSkillInactiveStance {
			key: MechanicId::new(""),
			disable_normal_attacks: false,
			modifiers: Vec::new(),
		}),
	};
	let invalid_duration = skill_inactive_stance(
		"timed_stance",
		false,
		vec![BuffModifier::Stat(StatModifier {
			bucket: StatModifierBucket::Scale,
			stat: StatKind::Defense,
			multiplier: value(2),
			duration_ticks: Some(1),
		})],
	);
	let build: OperatorBuildFacts =
		with_empty_manual_skill(operator_build(vec![invalid_empty, invalid_duration]), 2);
	let facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	let guide: MaterializedGuide = one_skill_activation_guide(build, 2);
	let error = compile_battle(&facts, &guide)
		.expect_err("invalid skill-inactive stance contracts must fail preflight");
	let details: Vec<&str> = error
		.issues()
		.iter()
		.filter_map(|issue| match issue {
			BattleCompileIssue::InvalidReplayPrimitive { detail, .. } => Some(detail.as_str()),
			_ => None,
		})
		.collect();

	assert!(details.contains(&"skill-inactive stance must target its source actor"));
	assert!(details.contains(&"stance key must not be empty"));
	assert!(details.contains(&"stance must change at least one behavior"));
	assert!(details.contains(&"stance-owned modifiers must not declare independent durations"));
}

/// One enemy shooting the operator deployed at `[0, 3]` from its start cell at ticks 1, 3 and 5.
fn shooting_enemy_facts(pattern: AttackPattern) -> ArkBattleFacts {
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(6));
	facts.enemies[0].combat.attack = value(100);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 2,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern,
		extra_effects: Vec::new(),
	};
	facts
}

fn sturdy_operator_guide(retreat_at: Option<u64>) -> MaterializedGuide {
	let mut guide: MaterializedGuide = guide(Vec::new(), retreat_at);
	guide.builds[0].combat = combat(500);
	guide
}

fn remaining_operator_hp(facts: &ArkBattleFacts, guide: &MaterializedGuide) -> ExpectedValue {
	let battle = compile_battle(facts, guide).expect("valid projectile battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.operators()[&OperatorInstanceId::new(1)].remaining_hp
}

const TWO_TICK_PROJECTILE: AttackPattern = AttackPattern::Projectile { flight_ticks: 2 };

#[test]
fn projectile_normal_attack_resolves_after_its_flight() {
	// An instant hit lands at ticks 1, 3 and 5. The projectile fired at tick 5 would land at tick 7,
	// after the battle ends at tick 6.
	let guide: MaterializedGuide = sturdy_operator_guide(None);
	assert_eq!(
		remaining_operator_hp(&shooting_enemy_facts(AttackPattern::Single), &guide),
		value(200)
	);
	assert_eq!(
		remaining_operator_hp(&shooting_enemy_facts(TWO_TICK_PROJECTILE), &guide),
		value(300)
	);
}

#[test]
fn projectile_hit_is_void_once_its_target_has_retreated() {
	// The operator retreats at tick 2: the instant hit fired at tick 1 has already landed, while the
	// projectile fired then would land at tick 3 on an operator that is no longer deployed.
	let guide: MaterializedGuide = sturdy_operator_guide(Some(2));
	assert_eq!(
		remaining_operator_hp(&shooting_enemy_facts(AttackPattern::Single), &guide),
		value(400)
	);
	assert_eq!(
		remaining_operator_hp(&shooting_enemy_facts(TWO_TICK_PROJECTILE), &guide),
		value(500)
	);
}

#[test]
fn projectile_still_lands_after_its_attacker_dies() {
	// The operator kills the enemy at tick 2, after its projectile fired at tick 1 and before it
	// lands at tick 3; the enemy never fires again.
	let mut guide: MaterializedGuide = sturdy_operator_guide(None);
	guide.builds[0].combat.attack = value(100);
	guide.builds[0].attack = AttackProfile {
		first_delay_ticks: 2,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, -3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let mut facts: ArkBattleFacts = shooting_enemy_facts(TWO_TICK_PROJECTILE);
	// A spawn after the battle ends keeps it running: killing its only enemy would end the battle at
	// tick 2 as a victory, before the hit could land.
	facts.waves[0].spawns.push(EnemySpawnFacts {
		id: EnemyInstanceId::new(2),
		enemy_key: "enemy".to_owned(),
		after_wave_start_ticks: 50,
		path: PathId::new(1),
		facing: Facing::Right,
	});
	let battle = compile_battle(&facts, &guide).expect("valid projectile battle");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		ExpectedValue::ZERO
	);
	assert_eq!(
		replay.operators()[&OperatorInstanceId::new(1)].remaining_hp,
		value(400)
	);
}

/// The flight replay executes for every plain ranged enemy, standing in for a serialized speed it
/// never converts into ticks.
const DECLARED_ONE_TICK_FLIGHT: AttackPattern = AttackPattern::Projectile { flight_ticks: 1 };

#[test]
fn the_declared_one_tick_flight_alone_decides_whether_the_operator_lives() {
	// Why a Declared Approximation is disclosed beside every verdict rather than passed over. The
	// approximated value is not always in the noise: against an operator whose HP is exactly three
	// hits, the unexecuted flight is the whole difference between a dead operator and a live one,
	// and an operator's death decides blocking, leaks and therefore the verdict. Hits fire at ticks
	// 1, 3 and 5 and the battle ends at tick 6, so the declared one-tick flight lands all three
	// while any longer real flight lands only two. On a real stage the margins are usually orders
	// larger than this, which is what a neutrality bound would have to establish per battle.
	let mut guide: MaterializedGuide = sturdy_operator_guide(None);
	guide.builds[0].combat = combat(300);

	assert_eq!(
		remaining_operator_hp(&shooting_enemy_facts(DECLARED_ONE_TICK_FLIGHT), &guide),
		ExpectedValue::ZERO
	);
	for real_flight in [2_u64, 3] {
		assert_eq!(
			remaining_operator_hp(
				&shooting_enemy_facts(AttackPattern::Projectile {
					flight_ticks: real_flight
				}),
				&guide
			),
			value(100),
			"a {real_flight}-tick flight leaves the operator alive"
		);
	}
}

#[test]
fn projectile_attacks_must_be_one_plain_damage_hit_with_a_flight() {
	let guide: MaterializedGuide = sturdy_operator_guide(None);
	let details = |facts: ArkBattleFacts| -> Vec<String> {
		compile_battle(&facts, &guide)
			.expect_err("an invalid projectile attack must fail preflight")
			.issues()
			.iter()
			.filter_map(|issue| match issue {
				BattleCompileIssue::InvalidCombatFacts { detail } => Some(detail.clone()),
				_ => None,
			})
			.collect()
	};

	let zero_flight: ArkBattleFacts =
		shooting_enemy_facts(AttackPattern::Projectile { flight_ticks: 0 });
	assert!(details(zero_flight)
		.iter()
		.any(|detail| detail.contains("zero projectile flight")));
	let mut healing: ArkBattleFacts = shooting_enemy_facts(TWO_TICK_PROJECTILE);
	healing.enemies[0].attack.action = NormalAttackAction::Heal;
	assert!(details(healing)
		.iter()
		.any(|detail| detail.contains("not one plain damage hit")));
	let mut two_targets: ArkBattleFacts = shooting_enemy_facts(TWO_TICK_PROJECTILE);
	two_targets.enemies[0].attack.target_count = 2;
	assert!(details(two_targets)
		.iter()
		.any(|detail| detail.contains("not one plain damage hit")));
}

fn source_stat_damage(mechanic: &str, multiplier: ExpectedValue) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::SourceStatScaledDamage {
			stat: StatKind::Attack,
			multiplier,
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
	}
}

fn movement_reduction_buff(key: &str, reduction: ExpectedValue, max_stacks: u16) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(format!("apply_{key}")),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::ApplyBuff(RuntimeBuff {
			key: MechanicId::new(key),
			max_stacks,
			duration_ticks: None,
			lifetime: RuntimeEffectLifetime::SourceActor,
			modifiers_per_stack: vec![BuffModifier::MovementSpeedReduction { reduction }],
		}),
	}
}

fn remove_source_buff(key: &str) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(format!("remove_{key}")),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::RemoveBuff {
			key: MechanicId::new(key),
			source: BuffRemovalSource::EffectSource,
		},
	}
}

fn layered_ground_tile(
	cells: ReplayCellSet,
	max_layers: u16,
	reduction: ExpectedValue,
) -> RuntimeTileEffect {
	let slow_key = "snow_slow";
	RuntimeTileEffect {
		key: MechanicId::new("layered_snow"),
		cells,
		actor_filter: TriggerActorFilter::EnemyOfSource,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		max_layers,
		duration_ticks: None,
		lifetime: RuntimeEffectLifetime::SourceActor,
		apply_new_layer_to_occupants: true,
		on_enter_once: vec![source_stat_damage(
			"snow_entry_damage",
			ExpectedValue::from_ratio(1, 2).expect("valid damage multiplier"),
		)],
		on_enter_per_layer: vec![movement_reduction_buff(slow_key, reduction, max_layers)],
		on_leave_per_layer: vec![remove_source_buff(slow_key)],
		on_remove_per_layer: vec![remove_source_buff(slow_key)],
		layer_threshold_reactions: Vec::new(),
		remove_on_first_matching_leave: false,
		periodic: None,
	}
}

#[test]
fn periodic_tile_damage_scales_with_layers() {
	let tile = tile_effect();
	let facts = base_facts(long_wait_path(), Vec::new());
	let battle = compile_battle(
		&facts,
		&guide(vec![add_tile(tile.clone()), add_tile(tile)], None),
	)
	.expect("valid layered tile");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(76)
	);
}

#[test]
fn target_cell_spawn_distinguishes_threshold_crossing_from_saturated_attempt() {
	let target_cell: GridPosition = GridPosition { row: 0, col: 1 };
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.cells = ReplayCellSet::Explicit(BTreeSet::from([target_cell]));
	tile.max_layers = 2;
	tile.periodic = None;
	tile.layer_threshold_reactions = vec![
		spawn_reaction(
			1,
			2,
			TileLayerThresholdTransition::ReachedFromBelow,
			TargetCellSpawnCollision::AllowCoexistence,
			TargetCellSpawnOwnership::Independent,
		),
		spawn_reaction(
			2,
			2,
			TileLayerThresholdTransition::SaturatedAttempt,
			TargetCellSpawnCollision::AllowCoexistence,
			TargetCellSpawnOwnership::Independent,
		),
	];
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![friendly_summon(1, None), friendly_summon(2, None)];
	let battle = compile_battle(
		&facts,
		&guide(
			vec![
				add_tile(tile.clone()),
				add_tile(tile.clone()),
				add_tile(tile),
			],
			None,
		),
	)
	.expect("valid target-cell spawn reactions");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let spawns: Vec<(SummonTemplateId, GridPosition)> = replay
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| {
			let TraceEntry::Event { scheduled } = entry else {
				return None;
			};
			let ExpectedEvent::TargetCellActorSpawn(event) = scheduled.event() else {
				return None;
			};
			Some((event.template, event.cell))
		})
		.collect();

	assert_eq!(
		spawns,
		vec![
			(SummonTemplateId::new(1), target_cell),
			(SummonTemplateId::new(2), target_cell),
		]
	);
}

#[test]
fn target_cell_spawn_rejects_invalid_threshold_and_missing_template() {
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.max_layers = 2;
	tile.layer_threshold_reactions = vec![spawn_reaction(
		99,
		1,
		TileLayerThresholdTransition::SaturatedAttempt,
		TargetCellSpawnCollision::AllowCoexistence,
		TargetCellSpawnOwnership::Independent,
	)];
	let error = compile_battle(
		&base_facts(long_wait_path(), Vec::new()),
		&guide(vec![add_tile(tile)], None),
	)
	.expect_err("invalid target-cell reaction must fail preflight");
	let details: Vec<&str> = error
		.issues()
		.iter()
		.filter_map(|issue| match issue {
			BattleCompileIssue::InvalidReplayPrimitive { detail, .. } => Some(detail.as_str()),
			_ => None,
		})
		.collect();

	assert!(details.contains(&"saturated tile reaction threshold must equal max_layers"));
	assert!(details.contains(&"tile reaction summon template does not exist"));
}

#[test]
fn tile_field_overlay_periodic_effect_runs_once_across_five_observed_layers() {
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.max_layers = 5;
	tile.periodic = None;
	let on_deploy: Vec<TaggedEffect> = (0..5).map(|_| add_tile(tile.clone())).collect();
	let build: OperatorBuildFacts = operator_build(on_deploy);
	let overlay: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		1,
		1,
		vec![damage("skill_snow_tick", 3)],
		Vec::new(),
	);
	let guide: MaterializedGuide = skill_overlay_guide(build, overlay, 10);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(2));
	let battle = compile_battle(&facts, &guide).expect("valid skill tile-field overlay");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(97)
	);
}

#[test]
fn overlay_layer_reactions_exist_only_during_the_owning_skill_activation() {
	let target_cell: GridPosition = GridPosition { row: 0, col: 1 };
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.cells = ReplayCellSet::Explicit(BTreeSet::from([target_cell]));
	tile.max_layers = 2;
	tile.periodic = None;
	let layer_pulse: TaggedEffect = listener(
		"snow_layer_pulse",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 0,
			interval_ticks: 2,
		},
		ReplayRepeat::Unlimited,
		vec![add_tile(tile)],
	);
	let build: OperatorBuildFacts = operator_build(vec![layer_pulse]);
	let mut overlay: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		100,
		100,
		Vec::new(),
		Vec::new(),
	);
	let ExpectedEffect::InstallTileFieldOverlay(spec) = &mut overlay.effect else {
		panic!("overlay helper must install an overlay");
	};
	spec.observed_tile_max_layers = Some(2);
	spec.layer_threshold_reactions = vec![
		spawn_reaction(
			0,
			2,
			TileLayerThresholdTransition::ReachedFromBelow,
			TargetCellSpawnCollision::AllowCoexistence,
			TargetCellSpawnOwnership::SkillActivation,
		),
		spawn_reaction(
			1,
			2,
			TileLayerThresholdTransition::SaturatedAttempt,
			TargetCellSpawnCollision::AllowCoexistence,
			TargetCellSpawnOwnership::SkillActivation,
		),
	];
	let guide: MaterializedGuide = skill_overlay_guide(build, overlay, 2);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(5));
	facts.summons = vec![friendly_summon(0, None), friendly_summon(1, None)];
	let battle = compile_battle(&facts, &guide).expect("valid skill-scoped layer reactions");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let spawns: Vec<(SimTick, SummonTemplateId)> = replay
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| {
			let TraceEntry::Event { scheduled } = entry else {
				return None;
			};
			let ExpectedEvent::TargetCellActorSpawn(event) = scheduled.event() else {
				return None;
			};
			Some((scheduled.at(), event.template))
		})
		.collect();
	let skill_owned_death_at: Option<SimTick> = replay.trace().entries().iter().find_map(|entry| {
		let TraceEntry::Event { scheduled } = entry else {
			return None;
		};
		matches!(
			scheduled.event(),
			ExpectedEvent::Death {
				actor: ark_solver::mechanics::domain::BattleActorId::Summon(0),
				..
			}
		)
		.then_some(scheduled.at())
	});

	assert_eq!(spawns, vec![(SimTick::new(2), SummonTemplateId::new(0))]);
	assert_eq!(skill_owned_death_at, Some(SimTick::new(3)));
}

#[test]
fn tile_field_overlay_rejects_invalid_interval_lifetime_and_install_target() {
	let mut overlay: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		0,
		1,
		Vec::new(),
		Vec::new(),
	);
	overlay.target = EffectTarget::AllEnemies;
	let ExpectedEffect::InstallTileFieldOverlay(spec) = &mut overlay.effect else {
		panic!("overlay helper must install an overlay");
	};
	spec.interval_ticks = 0;
	spec.lifetime = RuntimeEffectLifetime::SourceActor;
	let facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	let error = compile_battle(&facts, &guide(vec![overlay], None))
		.expect_err("invalid overlay contract must fail preflight");
	let details: Vec<&str> = error
		.issues()
		.iter()
		.filter_map(|issue| match issue {
			BattleCompileIssue::InvalidReplayPrimitive { detail, .. } => Some(detail.as_str()),
			_ => None,
		})
		.collect();

	assert!(details.contains(&"tile-field overlay must target its source actor"));
	assert!(details.contains(&"tile-field overlay interval must be positive"));
	assert!(details.contains(&"tile-field overlay lifetime must be skill activation"));
}

#[test]
fn tile_field_overlay_reaction_declares_and_respects_the_observed_layer_cap() {
	let mut overlay: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		1,
		1,
		Vec::new(),
		Vec::new(),
	);
	let ExpectedEffect::InstallTileFieldOverlay(spec) = &mut overlay.effect else {
		panic!("overlay helper must install an overlay");
	};
	spec.layer_threshold_reactions = vec![spawn_reaction(
		0,
		1,
		TileLayerThresholdTransition::ReachedFromBelow,
		TargetCellSpawnCollision::AllowCoexistence,
		TargetCellSpawnOwnership::Independent,
	)];
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![friendly_summon(0, None)];
	let missing_cap = compile_battle(&facts, &guide(vec![overlay.clone()], None))
		.expect_err("overlay reaction without a tile cap must fail preflight");
	assert!(missing_cap.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidReplayPrimitive { detail, .. }
			if detail == "overlay tile reactions require the observed tile layer cap"
	)));

	let ExpectedEffect::InstallTileFieldOverlay(spec) = &mut overlay.effect else {
		panic!("overlay helper must install an overlay");
	};
	spec.observed_tile_max_layers = Some(2);
	spec.layer_threshold_reactions = vec![spawn_reaction(
		0,
		1,
		TileLayerThresholdTransition::SaturatedAttempt,
		TargetCellSpawnCollision::AllowCoexistence,
		TargetCellSpawnOwnership::Independent,
	)];
	let wrong_saturation = compile_battle(&facts, &guide(vec![overlay], None))
		.expect_err("saturated reaction below the tile cap must fail preflight");
	assert!(wrong_saturation.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidReplayPrimitive { detail, .. }
			if detail
				== "saturated overlay reaction threshold must equal the observed tile layer cap"
	)));
}

fn overlay_leave_remaining_hp(leave_scope: TileFieldOverlayLeaveScope) -> ExpectedValue {
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.cells = ReplayCellSet::Explicit(BTreeSet::from([
		GridPosition { row: 0, col: 0 },
		GridPosition { row: 0, col: 1 },
	]));
	tile.max_layers = 5;
	tile.periodic = None;
	tile.remove_on_first_matching_leave = true;
	let on_deploy: Vec<TaggedEffect> = (0..5).map(|_| add_tile(tile.clone())).collect();
	let build: OperatorBuildFacts = operator_build(on_deploy);
	let overlay: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		leave_scope,
		100,
		100,
		Vec::new(),
		vec![damage("skill_snow_leave", 7)],
	);
	let guide: MaterializedGuide = skill_overlay_guide(build, overlay, 10);
	let mut facts: ArkBattleFacts = base_facts(
		vec![
			BattlePathStep {
				duration_ticks: 2,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Move {
					cell: GridPosition { row: 0, col: 1 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: 1,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Move {
					cell: GridPosition { row: 0, col: 2 },
					facing: Facing::Right,
				},
			},
			BattlePathStep {
				duration_ticks: 20,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Goal {
					goal: GoalId::new(1),
				},
			},
		],
		Vec::new(),
	);
	facts.end_at = Some(SimTick::new(4));
	let battle = compile_battle(&facts, &guide).expect("valid overlay leave scope");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp
}

#[test]
fn observed_cell_leave_fires_when_moving_between_observed_cells() {
	assert_eq!(
		overlay_leave_remaining_hp(TileFieldOverlayLeaveScope::ObservedCell),
		value(86)
	);
}

#[test]
fn observed_field_leave_fires_only_at_the_outer_boundary() {
	assert_eq!(
		overlay_leave_remaining_hp(TileFieldOverlayLeaveScope::ObservedField),
		value(93)
	);
}

#[test]
fn skill_end_removes_tile_field_overlay_but_preserves_observed_tile() {
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.max_layers = 1;
	tile.periodic = Some(PeriodicTileEffect {
		initial_delay_ticks: 1,
		interval_ticks: 1,
		effects_per_layer: vec![damage("base_snow_tick", 1)],
	});
	let build: OperatorBuildFacts = operator_build(vec![add_tile(tile)]);
	let overlay: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		1,
		1,
		vec![damage("skill_snow_tick", 10)],
		Vec::new(),
	);
	let guide: MaterializedGuide = skill_overlay_guide(build, overlay, 2);
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(4));
	let battle = compile_battle(&facts, &guide).expect("valid skill cleanup overlay");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(86)
	);
}

#[test]
fn tile_field_overlays_only_observe_tiles_from_their_own_source() {
	let operator_one = OperatorInstanceId::new(1);
	let operator_two = OperatorInstanceId::new(2);
	let mut tile_one: RuntimeTileEffect = tile_effect();
	tile_one.periodic = None;
	let mut tile_two: RuntimeTileEffect = tile_one.clone();
	tile_two.cells = ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 1 }]));
	let overlay_one: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		1,
		1,
		vec![source_stat_damage("source_one_tick", ExpectedValue::ONE)],
		Vec::new(),
	);
	let overlay_two: TaggedEffect = tile_field_overlay(
		"skill_snow",
		"snow",
		TileFieldOverlayLeaveScope::ObservedCell,
		1,
		1,
		vec![source_stat_damage("source_two_tick", ExpectedValue::ONE)],
		Vec::new(),
	);
	let mut build_one: OperatorBuildFacts = with_overlay_skill(
		operator_build_for(operator_one, vec![add_tile(tile_one)]),
		overlay_one,
		10,
	);
	build_one.combat.attack = value(3);
	let mut build_two: OperatorBuildFacts = with_overlay_skill(
		operator_build_for(operator_two, vec![add_tile(tile_two)]),
		overlay_two,
		10,
	);
	build_two.combat.attack = value(50);
	let guide = MaterializedGuide {
		builds: vec![build_one, build_two],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: operator_one,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: operator_two,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator: operator_one,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator: operator_two,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(2));
	facts.deployment.simultaneous_limit = 2;
	facts
		.grid
		.deployable_cells
		.insert(GridPosition { row: 0, col: 2 });
	let battle = compile_battle(&facts, &guide).expect("valid independent source overlays");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(97)
	);
}

fn dynamic_ground_snow_scenario(
	target_class: TargetClass,
	ground_cells: BTreeSet<GridPosition>,
) -> (ArkBattleFacts, MaterializedGuide) {
	let tile = layered_ground_tile(
		ReplayCellSet::SourceCurrentAttackRangeGroundCells,
		3,
		ExpectedValue::from_ratio(1, 10).expect("valid movement reduction"),
	);
	let add_layers = vec![
		add_tile(tile.clone()),
		add_tile(tile.clone()),
		add_tile(tile),
	];
	let attack_boost = TaggedEffect {
		mechanic: MechanicId::new("snow_source_attack_boost"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StatModifier(StatModifier {
			bucket: StatModifierBucket::Scale,
			stat: StatKind::Attack,
			multiplier: value(2),
			duration_ticks: None,
		}),
	};
	let range_boost = TaggedEffect {
		mechanic: MechanicId::new("snow_source_range_boost"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::AttackRange(AttackRangeModifier {
			additional_offsets: vec![RangeOffset::new(0, 3)],
			duration_ticks: None,
		}),
	};
	let generator = listener(
		"snow_generator",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 1,
			interval_ticks: 100,
		},
		ReplayRepeat::Once,
		add_layers,
	);
	let mut source_build = operator_build(vec![attack_boost, range_boost, generator]);
	source_build.combat.attack = value(50);
	let mut facts = base_facts(
		vec![
			BattlePathStep {
				duration_ticks: 10,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Wait,
			},
			BattlePathStep {
				duration_ticks: 1,
				affected_by_movement_speed: false,
				action: BattlePathStepAction::Goal {
					goal: GoalId::new(1),
				},
			},
		],
		Vec::new(),
	);
	facts.end_at = Some(SimTick::new(20));
	facts.grid.ground_cells = ground_cells;
	facts.enemies[0].combat.target_class = target_class;
	let guide = MaterializedGuide {
		builds: vec![source_build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: OperatorInstanceId::new(1),
			cell: GridPosition { row: 0, col: 3 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	(facts, guide)
}

#[test]
fn dynamic_ground_snow_uses_current_range_and_stat_but_enters_once() {
	let (facts, guide) = dynamic_ground_snow_scenario(
		TargetClass::Ground,
		BTreeSet::from([GridPosition { row: 0, col: 0 }]),
	);
	let battle = compile_battle(&facts, &guide).expect("valid dynamic ground snow");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &replay.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(enemy.remaining_hp, value(50));
	assert_eq!(enemy.leaked_at, Some(SimTick::new(16)));

	let (highland_facts, highland_guide) =
		dynamic_ground_snow_scenario(TargetClass::Ground, BTreeSet::new());
	let highland_battle =
		compile_battle(&highland_facts, &highland_guide).expect("valid empty ground range");
	let highland_replay = replay_expected(&highland_battle, MaterializedGuidePolicy::default());
	let highland_enemy = &highland_replay.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(highland_enemy.remaining_hp, value(100));
	assert_eq!(highland_enemy.leaked_at, Some(SimTick::new(11)));
}

#[test]
fn ground_only_tile_ignores_airborne_occupants() {
	let (facts, guide) = dynamic_ground_snow_scenario(
		TargetClass::Air,
		BTreeSet::from([GridPosition { row: 0, col: 0 }]),
	);
	let battle = compile_battle(&facts, &guide).expect("valid airborne snow scenario");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let enemy = &replay.enemies()[&EnemyInstanceId::new(1)];

	assert_eq!(enemy.remaining_hp, value(100));
	assert_eq!(enemy.leaked_at, Some(SimTick::new(11)));
}

#[test]
fn first_matching_leave_clears_only_its_cell_and_reschedules_other_occupants() {
	let mut tile = layered_ground_tile(
		ReplayCellSet::Explicit(BTreeSet::from([
			GridPosition { row: 0, col: 0 },
			GridPosition { row: 0, col: 2 },
		])),
		1,
		ExpectedValue::from_ratio(1, 2).expect("valid movement reduction"),
	);
	tile.remove_on_first_matching_leave = true;
	let mut source_build = operator_build(vec![add_tile(tile)]);
	source_build.combat.attack = value(10);
	let mut facts = base_facts(
		vec![
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
				affected_by_movement_speed: false,
				action: BattlePathStepAction::Goal {
					goal: GoalId::new(1),
				},
			},
		],
		Vec::new(),
	);
	facts.end_at = Some(SimTick::new(20));
	facts.paths.push(BattlePath {
		id: PathId::new(2),
		goal: GoalId::new(1),
		start: GridPosition { row: 0, col: 0 },
		steps: vec![
			BattlePathStep {
				duration_ticks: 10,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Wait,
			},
			BattlePathStep {
				duration_ticks: 1,
				affected_by_movement_speed: false,
				action: BattlePathStepAction::Goal {
					goal: GoalId::new(1),
				},
			},
		],
	});
	facts.paths.push(BattlePath {
		id: PathId::new(3),
		goal: GoalId::new(1),
		start: GridPosition { row: 0, col: 3 },
		steps: vec![
			BattlePathStep {
				duration_ticks: 3,
				affected_by_movement_speed: true,
				action: BattlePathStepAction::Move {
					cell: GridPosition { row: 0, col: 2 },
					facing: Facing::Left,
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
	});
	facts.waves[0].spawns = vec![
		EnemySpawnFacts {
			id: EnemyInstanceId::new(1),
			enemy_key: "enemy".to_owned(),
			after_wave_start_ticks: 0,
			path: PathId::new(1),
			facing: Facing::Right,
		},
		EnemySpawnFacts {
			id: EnemyInstanceId::new(2),
			enemy_key: "enemy".to_owned(),
			after_wave_start_ticks: 0,
			path: PathId::new(2),
			facing: Facing::Right,
		},
		EnemySpawnFacts {
			id: EnemyInstanceId::new(3),
			enemy_key: "enemy".to_owned(),
			after_wave_start_ticks: 0,
			path: PathId::new(3),
			facing: Facing::Left,
		},
	];
	let guide = MaterializedGuide {
		builds: vec![source_build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator: OperatorInstanceId::new(1),
			cell: GridPosition { row: 0, col: 3 },
			facing: Facing::Right,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid first-leave field");
	let replay = replay_expected_to_resolution(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(95)
	);
	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].leaked_at,
		Some(SimTick::new(3))
	);
	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(2)].remaining_hp,
		value(95)
	);
	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(2)].leaked_at,
		Some(SimTick::new(13))
	);
	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(3)].remaining_hp,
		value(95)
	);
	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(3)].leaked_at,
		Some(SimTick::new(5))
	);
}

#[test]
fn movement_reduction_rejects_non_positive_speed_at_max_stacks() {
	let invalid = movement_reduction_buff(
		"invalid_slow",
		ExpectedValue::from_ratio(1, 2).expect("valid movement reduction"),
		2,
	);
	let error = compile_battle(
		&base_facts(long_wait_path(), Vec::new()),
		&guide(vec![invalid], None),
	)
	.expect_err("zero movement speed must fail preflight");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidReplayPrimitive { detail, .. }
			if detail.contains("movement-speed reduction")
	)));
}

fn target_relative_tile(
	key: &str,
	offsets: Vec<RangeOffset>,
	max_layers: u16,
	on_enter_per_layer: Vec<TaggedEffect>,
) -> RuntimeTileEffect {
	RuntimeTileEffect {
		key: MechanicId::new(key),
		cells: ReplayCellSet::EffectTargetRelative(offsets),
		actor_filter: TriggerActorFilter::AllyOfSource,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		max_layers,
		duration_ticks: None,
		lifetime: RuntimeEffectLifetime::SourceActor,
		apply_new_layer_to_occupants: true,
		on_enter_once: Vec::new(),
		on_enter_per_layer,
		on_leave_per_layer: Vec::new(),
		on_remove_per_layer: Vec::new(),
		layer_threshold_reactions: Vec::new(),
		remove_on_first_matching_leave: false,
		periodic: None,
	}
}

fn healing(mechanic: &str, amount: u32, target: EffectTarget) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target,
		effect: ExpectedEffect::Healing {
			amount: value(amount),
		},
	}
}

#[test]
fn heal_target_tiles_layer_independently_per_resolved_cell() {
	let source = OperatorInstanceId::new(1);
	let first_target = OperatorInstanceId::new(2);
	let second_target = OperatorInstanceId::new(3);
	let sow_tile = TaggedEffect {
		mechanic: MechanicId::new("sow_target_cell"),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::AddTileLayer(target_relative_tile(
			"sowed_cell",
			vec![RangeOffset::new(0, 0)],
			2,
			vec![damage("sowed_cell_entry", 1)],
		)),
	};
	let mut sow_on_heal = listener(
		"sow_on_heal",
		ReplayTrigger::Heal,
		ReplayRepeat::Unlimited,
		vec![sow_tile],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut sow_on_heal.effect {
		spec.actor_filter = TriggerActorFilter::EffectSource;
	}
	let source_build = operator_build_for(
		source,
		vec![
			sow_on_heal,
			healing("first_heal", 10, EffectTarget::AllAllies),
			healing("second_heal", 10, EffectTarget::AllAllies),
		],
	);
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.deployment.simultaneous_limit = 3;
	facts.grid.deployable_cells = BTreeSet::from([
		GridPosition { row: 0, col: 1 },
		GridPosition { row: 0, col: 2 },
		GridPosition { row: 0, col: 3 },
	]);
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("wound_targets"),
			target: EffectTarget::AllAllies,
			effect: damage("wound_targets", 50).effect,
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![
			source_build,
			operator_build_for(first_target, Vec::new()),
			operator_build_for(second_target, Vec::new()),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: first_target,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: second_target,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(2),
				operator: source,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid target-relative tile fields");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&first_target].remaining_hp, value(68));
	assert_eq!(replay.operators()[&second_target].remaining_hp, value(68));
	assert_eq!(replay.operators()[&source].remaining_hp, value(100));
}

#[test]
fn overlapping_target_relative_cells_are_applied_once_per_cell() {
	let source = OperatorInstanceId::new(1);
	let first_target = OperatorInstanceId::new(2);
	let second_target = OperatorInstanceId::new(3);
	let field = TaggedEffect {
		mechanic: MechanicId::new("overlapping_field"),
		target: EffectTarget::AllAllies,
		effect: ExpectedEffect::AddTileLayer(target_relative_tile(
			"overlapping_field",
			vec![RangeOffset::new(0, 0), RangeOffset::new(0, 1)],
			1,
			vec![damage("field_entry", 1)],
		)),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.deployment.simultaneous_limit = 3;
	facts.grid.deployable_cells = BTreeSet::from([
		GridPosition { row: 0, col: 1 },
		GridPosition { row: 0, col: 2 },
		GridPosition { row: 0, col: 3 },
	]);
	let guide = MaterializedGuide {
		builds: vec![
			operator_build_for(source, vec![field]),
			operator_build_for(first_target, Vec::new()),
			operator_build_for(second_target, Vec::new()),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: first_target,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: second_target,
				cell: GridPosition { row: 0, col: 2 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(1),
				operator: source,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid overlapping target-relative field");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&first_target].remaining_hp, value(99));
	assert_eq!(replay.operators()[&second_target].remaining_hp, value(99));
	assert_eq!(replay.operators()[&source].remaining_hp, value(99));
}

#[test]
fn before_lethal_once_guard_prevents_only_the_first_death() {
	let revive = TaggedEffect {
		mechanic: MechanicId::new("revive"),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::Healing { amount: value(100) },
	};
	let mut self_listener = listener(
		"before_lethal",
		ReplayTrigger::BeforeLethal,
		ReplayRepeat::Once,
		vec![revive],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut self_listener.effect {
		spec.actor_filter = TriggerActorFilter::SelfActor;
		spec.lifetime = RuntimeEffectLifetime::SourceActor;
	}
	let mut facts = base_facts(long_wait_path(), vec![self_listener]);
	facts.stage_effects = vec![
		ScheduledStageEffect {
			at: SimTick::new(2),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("lethal_one"),
				target: EffectTarget::AllEnemies,
				effect: damage("nested_one", 100).effect,
			},
		},
		ScheduledStageEffect {
			at: SimTick::new(4),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("lethal_two"),
				target: EffectTarget::AllEnemies,
				effect: damage("nested_two", 100).effect,
			},
		},
	];
	let battle = compile_battle(&facts, &MaterializedGuide::default()).expect("valid once guard");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(4))
	);
}

#[test]
fn source_retreat_cleans_up_tile_pulses() {
	let mut tile = tile_effect();
	tile.periodic = Some(PeriodicTileEffect {
		initial_delay_ticks: 1,
		interval_ticks: 1,
		effects_per_layer: vec![damage("farm_tick", 10)],
	});
	let facts = base_facts(long_wait_path(), Vec::new());
	let battle = compile_battle(&facts, &guide(vec![add_tile(tile)], Some(3)))
		.expect("valid source-owned tile");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(80)
	);
}

#[test]
fn same_tick_trigger_cycles_fail_compilation() {
	let on_deploy = vec![listener(
		"damage_loop",
		ReplayTrigger::Damage,
		ReplayRepeat::Unlimited,
		vec![damage("damage_again", 1)],
	)];
	let facts = base_facts(long_wait_path(), Vec::new());
	let error = compile_battle(&facts, &guide(on_deploy, None)).expect_err("cycle must fail");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::ReplayTriggerCycle { triggers }
			if triggers.contains(&ReplayTriggerKind::Damage)
	)));
}

#[test]
fn zero_delay_tile_install_participates_in_trigger_cycle_validation() {
	let mut tile = tile_effect();
	tile.periodic = Some(PeriodicTileEffect {
		initial_delay_ticks: 0,
		interval_ticks: 1,
		effects_per_layer: vec![damage("tile_damage", 1)],
	});
	let on_deploy = vec![listener(
		"damage_installs_tile",
		ReplayTrigger::Damage,
		ReplayRepeat::Unlimited,
		vec![add_tile(tile)],
	)];
	let facts = base_facts(long_wait_path(), Vec::new());
	let error = compile_battle(&facts, &guide(on_deploy, None))
		.expect_err("zero-delay tile pulse closes the damage cycle");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::ReplayTriggerCycle { triggers }
			if triggers.contains(&ReplayTriggerKind::Damage)
	)));
}

#[test]
fn zero_delay_listener_install_participates_in_trigger_cycle_validation() {
	let mut periodic_damage = damage("periodic_damage", 1);
	periodic_damage.target = EffectTarget::AllEnemies;
	let installs_periodic = listener(
		"periodic_damage",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 0,
			interval_ticks: 1,
		},
		ReplayRepeat::Unlimited,
		vec![periodic_damage],
	);
	let on_deploy = vec![listener(
		"damage_installs_listener",
		ReplayTrigger::Damage,
		ReplayRepeat::Unlimited,
		vec![installs_periodic],
	)];
	let facts = base_facts(long_wait_path(), Vec::new());
	let error = compile_battle(&facts, &guide(on_deploy, None))
		.expect_err("zero-delay listener pulse closes the damage cycle");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::ReplayTriggerCycle { triggers }
			if triggers.contains(&ReplayTriggerKind::Damage)
	)));
}

#[test]
fn deadline_kill_waits_for_enemy_death_effects_to_drain() {
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(8),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("deadline_kill"),
			target: EffectTarget::AllEnemies,
			effect: damage("deadline_kill", 100).effect,
		},
	}];
	let battle =
		compile_battle(&facts, &MaterializedGuide::default()).expect("deadline kill is valid");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.status(), &ExpectedTerminalStatus::Victory);
	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(8))
	);
}

#[test]
fn instantaneous_skill_cleans_up_skill_activation_lifetime() {
	let operator = OperatorInstanceId::new(1);
	let instant_buff = apply_buff_effect(
		"instant_hp",
		EffectTarget::SelfActor,
		RuntimeBuff {
			key: MechanicId::new("instant_hp"),
			max_stacks: 1,
			duration_ticks: None,
			lifetime: RuntimeEffectLifetime::SkillActivation,
			modifiers_per_stack: vec![BuffModifier::Stat(StatModifier {
				bucket: StatModifierBucket::Scale,
				stat: StatKind::MaxHp,
				multiplier: value(2),
				duration_ticks: None,
			})],
		},
	);
	let mut build = operator_build(Vec::new());
	build.skill = Some(SkillFacts {
		id: MechanicId::new("instant_skill"),
		activation: SkillActivationRule::Manual,
		initial_sp: 1,
		sp_cost: 1,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![instant_buff],
	});
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let facts = base_facts(long_wait_path(), Vec::new());
	let battle = compile_battle(&facts, &guide).expect("instant skill is valid");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&operator].max_hp, value(100));
}

fn friendly_summon(id: u32, lifetime_ticks: Option<u64>) -> SummonTemplate {
	SummonTemplate {
		id: SummonTemplateId::new(id),
		key: format!("summon_{id}"),
		faction: ark_solver::mechanics::domain::Faction::Friendly,
		cell_occupancy: SummonCellOccupancy::Exclusive,
		block_count: 1,
		block_target_filter: BlockTargetFilter::GroundOnly,
		targetability: Targetability::All,
		combat: combat(100),
		attack: inert_attack(),
		lifetime_ticks,
		reached_mechanics: Vec::new(),
	}
}

fn target_spawn_attack_runs(collision: TargetCellSpawnCollision) -> bool {
	let target_cell: GridPosition = GridPosition { row: 0, col: 1 };
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.cells = ReplayCellSet::Explicit(BTreeSet::from([target_cell]));
	tile.periodic = None;
	tile.max_layers = 1;
	tile.layer_threshold_reactions = vec![spawn_reaction(
		1,
		1,
		TileLayerThresholdTransition::ReachedFromBelow,
		collision,
		TargetCellSpawnOwnership::Independent,
	)];
	let mut occupant: SummonTemplate = friendly_summon(0, None);
	occupant.cell_occupancy = SummonCellOccupancy::Exclusive;
	let mut target: SummonTemplate = friendly_summon(1, None);
	target.attack.first_delay_ticks = 1;
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(2));
	facts.summons = vec![occupant, target];
	facts.preplaced_actors = vec![PreplacedActorFacts {
		instance: 0,
		template: SummonTemplateId::new(0),
		cell: target_cell,
		facing: Facing::Right,
	}];
	let battle = compile_battle(&facts, &guide(vec![add_tile(tile)], None))
		.expect("valid collision-policy scenario");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.trace().entries().iter().any(|entry| {
		matches!(
			entry,
			TraceEntry::Event { scheduled }
				if matches!(
					scheduled.event(),
					ExpectedEvent::NormalAttack {
						actor: ark_solver::mechanics::domain::BattleActorId::Summon(1),
						..
					}
				)
		)
	})
}

#[test]
fn target_cell_spawn_collision_policy_is_explicit() {
	assert!(!target_spawn_attack_runs(
		TargetCellSpawnCollision::SuppressIfFriendlyOccupied
	));
	assert!(target_spawn_attack_runs(
		TargetCellSpawnCollision::AllowCoexistence
	));
}

fn deployment_executes_with_preplaced_occupancy(occupancy: SummonCellOccupancy) -> bool {
	let deploy_cell: GridPosition = GridPosition { row: 0, col: 3 };
	let mut template: SummonTemplate = friendly_summon(0, None);
	template.cell_occupancy = occupancy;
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![template];
	facts.preplaced_actors = vec![PreplacedActorFacts {
		instance: 0,
		template: SummonTemplateId::new(0),
		cell: deploy_cell,
		facing: Facing::Right,
	}];
	let battle =
		compile_battle(&facts, &guide(Vec::new(), None)).expect("valid summon-occupancy scenario");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.action_audit()[0].executed_at.is_some()
}

#[test]
fn only_exclusive_friendly_summons_block_operator_deployment() {
	assert!(!deployment_executes_with_preplaced_occupancy(
		SummonCellOccupancy::Exclusive
	));
	assert!(deployment_executes_with_preplaced_occupancy(
		SummonCellOccupancy::NonExclusive
	));
}

fn summon_death_at_for_tile_ownership(
	ownership: TargetCellSpawnOwnership,
	duration_ticks: Option<u64>,
	retreat_at: Option<u64>,
) -> Option<SimTick> {
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.cells = ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 1 }]));
	tile.max_layers = 1;
	tile.duration_ticks = duration_ticks;
	tile.periodic = None;
	tile.layer_threshold_reactions = vec![spawn_reaction(
		0,
		1,
		TileLayerThresholdTransition::ReachedFromBelow,
		TargetCellSpawnCollision::AllowCoexistence,
		ownership,
	)];
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(3));
	facts.summons = vec![friendly_summon(0, None)];
	let battle = compile_battle(&facts, &guide(vec![add_tile(tile)], retreat_at))
		.expect("valid tile-spawn ownership scenario");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.trace().entries().iter().find_map(|entry| {
		let TraceEntry::Event { scheduled } = entry else {
			return None;
		};
		matches!(
			scheduled.event(),
			ExpectedEvent::Death {
				actor: ark_solver::mechanics::domain::BattleActorId::Summon(0),
				..
			}
		)
		.then_some(scheduled.at())
	})
}

fn summon_death_at_for_skill_ownership() -> Option<SimTick> {
	let operator: OperatorInstanceId = OperatorInstanceId::new(1);
	let mut tile: RuntimeTileEffect = tile_effect();
	tile.cells = ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 1 }]));
	tile.max_layers = 1;
	tile.periodic = None;
	tile.lifetime = RuntimeEffectLifetime::SkillActivation;
	tile.layer_threshold_reactions = vec![spawn_reaction(
		0,
		1,
		TileLayerThresholdTransition::ReachedFromBelow,
		TargetCellSpawnCollision::AllowCoexistence,
		TargetCellSpawnOwnership::SkillActivation,
	)];
	let mut build: OperatorBuildFacts = operator_build(Vec::new());
	build.skill = Some(SkillFacts {
		id: MechanicId::new("spawn_skill"),
		activation: SkillActivationRule::Manual,
		initial_sp: 1,
		sp_cost: 1,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(1),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![add_tile(tile)],
	});
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(1),
				operator,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(3));
	facts.summons = vec![friendly_summon(0, None)];
	let battle = compile_battle(&facts, &guide).expect("valid skill-owned spawn scenario");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	replay.trace().entries().iter().find_map(|entry| {
		let TraceEntry::Event { scheduled } = entry else {
			return None;
		};
		matches!(
			scheduled.event(),
			ExpectedEvent::Death {
				actor: ark_solver::mechanics::domain::BattleActorId::Summon(0),
				..
			}
		)
		.then_some(scheduled.at())
	})
}

#[test]
fn target_cell_spawn_ownership_uses_normal_death_cleanup() {
	assert_eq!(
		summon_death_at_for_tile_ownership(TargetCellSpawnOwnership::Independent, None, Some(1)),
		None
	);
	assert_eq!(
		summon_death_at_for_tile_ownership(TargetCellSpawnOwnership::SourceActor, None, Some(1)),
		Some(SimTick::new(1))
	);
	assert_eq!(
		summon_death_at_for_tile_ownership(
			TargetCellSpawnOwnership::TriggeringTileEffect,
			Some(1),
			None,
		),
		Some(SimTick::new(1))
	);
	assert_eq!(summon_death_at_for_skill_ownership(), Some(SimTick::new(2)));
}

fn apply_buff_effect(key: &str, target: EffectTarget, buff: RuntimeBuff) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(key),
		target,
		effect: ExpectedEffect::ApplyBuff(buff),
	}
}

#[test]
fn timed_summon_expires_and_releases_its_blocked_enemy() {
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![friendly_summon(1, Some(2))];
	facts.preplaced_actors = vec![PreplacedActorFacts {
		instance: 1,
		template: SummonTemplateId::new(1),
		cell: GridPosition { row: 0, col: 0 },
		facing: Facing::Right,
	}];
	let battle = compile_battle(&facts, &MaterializedGuide::default()).expect("valid summon");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert!(replay.block_audit().iter().any(|block| {
		block.started_at == SimTick::new(0)
			&& block.ended_at == Some(SimTick::new(2))
			&& block.end_reason == Some(BlockEndReason::BlockerDeath)
	}));
}

#[test]
fn explicit_death_is_not_a_cancelable_lethal_damage_event() {
	let revive = TaggedEffect {
		mechanic: MechanicId::new("revive"),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::Healing { amount: value(100) },
	};
	let mut guard = listener(
		"before_lethal",
		ReplayTrigger::BeforeLethal,
		ReplayRepeat::Once,
		vec![revive],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut guard.effect {
		spec.actor_filter = TriggerActorFilter::SelfActor;
	}
	let mut facts = base_facts(long_wait_path(), vec![guard]);
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(2),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("forced_death"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Death,
		},
	}];
	let battle =
		compile_battle(&facts, &MaterializedGuide::default()).expect("valid explicit death");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].death_at,
		Some(SimTick::new(2))
	);
}

#[test]
fn persistent_listener_keeps_working_after_its_operator_retires() {
	let mut persistent = listener(
		"persistent_damage",
		ReplayTrigger::Damage,
		ReplayRepeat::Once,
		vec![damage("follow_up", 10)],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut persistent.effect {
		spec.lifetime = RuntimeEffectLifetime::Persistent;
	}
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(2),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("stage_damage"),
			target: EffectTarget::AllEnemies,
			effect: damage("stage_hit", 1).effect,
		},
	}];
	let battle = compile_battle(&facts, &guide(vec![persistent], Some(1)))
		.expect("valid persistent listener");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(89)
	);
}

#[test]
fn persistent_listener_retains_source_faction_after_source_retires() {
	let source = OperatorInstanceId::new(1);
	let ally = OperatorInstanceId::new(2);
	let mut persistent = listener(
		"persistent_ally_guard",
		ReplayTrigger::Damage,
		ReplayRepeat::Once,
		vec![healing(
			"persistent_ally_heal",
			10,
			EffectTarget::CurrentTargets,
		)],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut persistent.effect {
		spec.actor_filter = TriggerActorFilter::AllyOfSource;
		spec.lifetime = RuntimeEffectLifetime::Persistent;
	}
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.deployment.simultaneous_limit = 2;
	facts.grid.deployable_cells = BTreeSet::from([
		GridPosition { row: 0, col: 1 },
		GridPosition { row: 0, col: 3 },
	]);
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(2),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("wound_remaining_ally"),
			target: EffectTarget::AllAllies,
			effect: damage("wound_remaining_ally", 20).effect,
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![
			operator_build_for(source, vec![persistent]),
			operator_build_for(ally, Vec::new()),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: ally,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator: source,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid persistent ally listener");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&ally].remaining_hp, value(90));
}

#[test]
fn persistent_tile_retains_source_faction_after_source_retires() {
	let source = OperatorInstanceId::new(1);
	let ally = OperatorInstanceId::new(2);
	let field = TaggedEffect {
		mechanic: MechanicId::new("persistent_field"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::AddTileLayer(RuntimeTileEffect {
			key: MechanicId::new("persistent_field"),
			cells: ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 1 }])),
			actor_filter: TriggerActorFilter::AllyOfSource,
			target_filter: NormalAttackTargetFilter::GroundAndAir,
			max_layers: 1,
			duration_ticks: None,
			lifetime: RuntimeEffectLifetime::Persistent,
			apply_new_layer_to_occupants: false,
			on_enter_once: Vec::new(),
			on_enter_per_layer: Vec::new(),
			on_leave_per_layer: Vec::new(),
			on_remove_per_layer: Vec::new(),
			layer_threshold_reactions: Vec::new(),
			remove_on_first_matching_leave: false,
			periodic: Some(PeriodicTileEffect {
				initial_delay_ticks: 2,
				interval_ticks: 100,
				effects_per_layer: vec![healing(
					"persistent_field_heal",
					10,
					EffectTarget::CurrentTargets,
				)],
			}),
		}),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.deployment.simultaneous_limit = 2;
	facts.grid.deployable_cells = BTreeSet::from([
		GridPosition { row: 0, col: 1 },
		GridPosition { row: 0, col: 3 },
	]);
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new("wound_ally"),
			target: EffectTarget::AllAllies,
			effect: damage("wound_ally", 20).effect,
		},
	}];
	let guide = MaterializedGuide {
		builds: vec![
			operator_build_for(source, vec![field]),
			operator_build_for(ally, Vec::new()),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: ally,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator: source,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid persistent ally field");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&ally].remaining_hp, value(90));
}

#[test]
fn shelter_buff_is_source_scoped_removed_and_bypassed_by_direct_hit() {
	let source = OperatorInstanceId::new(1);
	let ally = OperatorInstanceId::new(2);
	let shelter_key = MechanicId::new("sowed_shelter");
	let shelter = apply_buff_effect(
		"apply_sowed_shelter",
		EffectTarget::CurrentTargets,
		RuntimeBuff {
			key: shelter_key.clone(),
			max_stacks: 1,
			duration_ticks: None,
			lifetime: RuntimeEffectLifetime::SourceActor,
			modifiers_per_stack: vec![BuffModifier::Shelter {
				incoming_damage_multiplier: ExpectedValue::from_ratio(1, 2)
					.expect("valid shelter multiplier"),
			}],
		},
	);
	let remove_shelter = TaggedEffect {
		mechanic: MechanicId::new("remove_sowed_shelter"),
		target: EffectTarget::CurrentTargets,
		effect: ExpectedEffect::RemoveBuff {
			key: shelter_key,
			source: BuffRemovalSource::EffectSource,
		},
	};
	let field = TaggedEffect {
		mechanic: MechanicId::new("temporary_sowed_cell"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::AddTileLayer(RuntimeTileEffect {
			key: MechanicId::new("temporary_sowed_cell"),
			cells: ReplayCellSet::Explicit(BTreeSet::from([GridPosition { row: 0, col: 1 }])),
			actor_filter: TriggerActorFilter::AllyOfSource,
			target_filter: NormalAttackTargetFilter::GroundAndAir,
			max_layers: 1,
			duration_ticks: Some(2),
			lifetime: RuntimeEffectLifetime::SourceActor,
			apply_new_layer_to_occupants: true,
			on_enter_once: Vec::new(),
			on_enter_per_layer: vec![shelter],
			on_leave_per_layer: vec![remove_shelter.clone()],
			on_remove_per_layer: vec![remove_shelter],
			layer_threshold_reactions: Vec::new(),
			remove_on_first_matching_leave: false,
			periodic: None,
		}),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.deployment.simultaneous_limit = 2;
	facts.grid.deployable_cells = BTreeSet::from([
		GridPosition { row: 0, col: 1 },
		GridPosition { row: 0, col: 3 },
	]);
	facts.stage_effects = vec![
		ScheduledStageEffect {
			at: SimTick::new(1),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("sheltered_hit"),
				target: EffectTarget::AllAllies,
				effect: damage("sheltered_hit", 20).effect,
			},
		},
		ScheduledStageEffect {
			at: SimTick::new(1),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("ignore_shelter_hit"),
				target: EffectTarget::AllAllies,
				effect: ExpectedEffect::Damage {
					amount: value(20),
					damage_type: DamageType::True,
					delivery: DamageDelivery::DirectHit {
						ignore_defense: false,
						ignore_shelter: true,
					},
				},
			},
		},
		ScheduledStageEffect {
			at: SimTick::new(3),
			repeat_interval_ticks: None,
			repeat_until: None,
			effect: TaggedEffect {
				mechanic: MechanicId::new("post_shelter_hit"),
				target: EffectTarget::AllAllies,
				effect: damage("post_shelter_hit", 20).effect,
			},
		},
	];
	let guide = MaterializedGuide {
		builds: vec![
			operator_build_for(source, vec![field]),
			operator_build_for(ally, Vec::new()),
		],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: ally,
				cell: GridPosition { row: 0, col: 1 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: source,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid source-scoped shelter field");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&ally].remaining_hp, value(50));
}

#[test]
fn persistent_buff_is_reinstalled_for_a_new_deployment_generation() {
	let max_hp_buff = apply_buff_effect(
		"persistent_hp",
		EffectTarget::SelfActor,
		RuntimeBuff {
			key: MechanicId::new("persistent_hp"),
			max_stacks: 1,
			duration_ticks: None,
			lifetime: RuntimeEffectLifetime::Persistent,
			modifiers_per_stack: vec![BuffModifier::Stat(StatModifier {
				bucket: StatModifierBucket::Scale,
				stat: StatKind::MaxHp,
				multiplier: value(2),
				duration_ticks: None,
			})],
		},
	);
	let operator = OperatorInstanceId::new(1);
	let guide = MaterializedGuide {
		builds: vec![operator_build(vec![max_hp_buff])],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(1),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(3),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let facts = base_facts(long_wait_path(), Vec::new());
	let battle = compile_battle(&facts, &guide).expect("valid persistent buff");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&operator].max_hp, value(200));
	assert_eq!(replay.operators()[&operator].remaining_hp, value(200));
}

#[test]
fn once_listener_does_not_make_a_trigger_cycle() {
	let on_deploy = vec![listener(
		"damage_once",
		ReplayTrigger::Damage,
		ReplayRepeat::Once,
		vec![damage("one_follow_up", 1)],
	)];
	let facts = base_facts(long_wait_path(), Vec::new());

	compile_battle(&facts, &guide(on_deploy, None)).expect("once listener terminates");
}

#[test]
fn zero_delay_summon_attack_participates_in_trigger_cycle_validation() {
	let summon_id = SummonTemplateId::new(1);
	let mut summon = friendly_summon(1, None);
	summon.attack = AttackProfile {
		first_delay_ticks: 0,
		interval_ticks: 10,
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
	let summon_effect = TaggedEffect {
		mechanic: MechanicId::new("spawn_attacker"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Summon(SummonEffect {
			template: summon_id,
		}),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![summon];
	let error = compile_battle(
		&facts,
		&guide(
			vec![listener(
				"summon_loop",
				ReplayTrigger::Damage,
				ReplayRepeat::Unlimited,
				vec![summon_effect],
			)],
			None,
		),
	)
	.expect_err("summon-induced cycle must fail");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::ReplayTriggerCycle { triggers }
			if triggers.contains(&ReplayTriggerKind::Damage)
	)));
}

#[test]
fn summon_attack_extra_effects_are_validated() {
	let mut summon = friendly_summon(1, None);
	summon.attack.extra_effects = vec![listener(
		"invalid_periodic",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 0,
			interval_ticks: 0,
		},
		ReplayRepeat::Unlimited,
		Vec::new(),
	)];
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![summon];
	let error = compile_battle(&facts, &MaterializedGuide::default())
		.expect_err("invalid summon extra effect must fail");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidReplayPrimitive { detail, .. }
			if detail.contains("interval")
	)));
}

#[test]
fn recursive_zero_delay_summon_attacks_fail_compilation() {
	let summon_id = SummonTemplateId::new(1);
	let mut summon = friendly_summon(1, None);
	summon.attack.first_delay_ticks = 0;
	summon.attack.target_count = 1;
	summon.attack.extra_effects = vec![TaggedEffect {
		mechanic: MechanicId::new("recursive_summon"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::Summon(SummonEffect {
			template: summon_id,
		}),
	}];
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.summons = vec![summon];
	let error = compile_battle(&facts, &MaterializedGuide::default())
		.expect_err("recursive summon attack must fail");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidReplayPrimitive { detail, .. }
			if detail.contains("recursively spawn")
	)));
}

#[test]
fn blocking_buff_reconciles_waiting_enemies_immediately() {
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.paths[0].start = GridPosition { row: 0, col: 3 };
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: apply_buff_effect(
			"block_buff",
			EffectTarget::AllAllies,
			RuntimeBuff {
				key: MechanicId::new("block_buff"),
				max_stacks: 1,
				duration_ticks: None,
				lifetime: RuntimeEffectLifetime::Persistent,
				modifiers_per_stack: vec![BuffModifier::Blocking(BlockCountModifier {
					delta: 1,
					duration_ticks: None,
				})],
			},
		),
	}];
	let battle = compile_battle(&facts, &guide(Vec::new(), None)).expect("valid blocking buff");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert!(replay
		.block_audit()
		.iter()
		.any(|block| block.started_at == SimTick::new(1)));
}

#[test]
fn cadence_buff_reschedules_the_next_attack_immediately() {
	let mut build = operator_build(Vec::new());
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 100,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let operator = build.id;
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 3 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.stage_effects = vec![ScheduledStageEffect {
		at: SimTick::new(1),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: apply_buff_effect(
			"cadence_buff",
			EffectTarget::AllAllies,
			RuntimeBuff {
				key: MechanicId::new("cadence_buff"),
				max_stacks: 1,
				duration_ticks: None,
				lifetime: RuntimeEffectLifetime::Persistent,
				modifiers_per_stack: vec![BuffModifier::AttackCadence(AttackCadenceModifier {
					interval_multiplier: ExpectedValue::from_ratio(1, 100).expect("non-zero ratio"),
					duration_ticks: None,
				})],
			},
		),
	}];
	let battle = compile_battle(&facts, &guide).expect("valid cadence buff");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert!(replay.enemies()[&EnemyInstanceId::new(1)].expected_damage_taken > ExpectedValue::ZERO);
}

#[test]
fn death_trigger_preserves_the_killing_effect_source() {
	let reward = apply_buff_effect(
		"kill_reward",
		EffectTarget::SelfActor,
		RuntimeBuff {
			key: MechanicId::new("kill_reward"),
			max_stacks: 1,
			duration_ticks: None,
			lifetime: RuntimeEffectLifetime::SourceActor,
			modifiers_per_stack: vec![BuffModifier::Stat(StatModifier {
				bucket: StatModifierBucket::Scale,
				stat: StatKind::MaxHp,
				multiplier: value(2),
				duration_ticks: None,
			})],
		},
	);
	let mut kill_listener = listener(
		"on_kill",
		ReplayTrigger::Death,
		ReplayRepeat::Once,
		vec![reward],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut kill_listener.effect {
		spec.actor_filter = TriggerActorFilter::EffectSource;
	}
	let mut build = operator_build(vec![kill_listener]);
	build.combat.attack = value(100);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let operator = build.id;
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 3 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.waves[0].spawns.push(EnemySpawnFacts {
		id: EnemyInstanceId::new(2),
		enemy_key: "enemy".to_owned(),
		after_wave_start_ticks: 0,
		path: PathId::new(1),
		facing: Facing::Right,
	});
	let battle = compile_battle(&facts, &guide).expect("valid death trigger");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(replay.operators()[&operator].max_hp, value(200));
}

fn status_effect(mechanic: &str, target: EffectTarget, status: StatusEffect) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target,
		effect: ExpectedEffect::ApplyStatus(status),
	}
}

fn restore_effect(mechanic: &str, target: EffectTarget) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new(mechanic),
		target,
		effect: ExpectedEffect::RestoreHpToCurrentMaxRatio {
			ratio: ExpectedValue::from_ratio(1, 2).expect("valid restore ratio"),
		},
	}
}

fn stage_damage(
	at: u64,
	target: EffectTarget,
	amount: u32,
	damage_type: DamageType,
) -> ScheduledStageEffect {
	ScheduledStageEffect {
		at: SimTick::new(at),
		repeat_interval_ticks: None,
		repeat_until: None,
		effect: TaggedEffect {
			mechanic: MechanicId::new(format!("stage_damage_{at}")),
			target,
			effect: ExpectedEffect::Damage {
				amount: value(amount),
				damage_type,
				delivery: DamageDelivery::Normal,
			},
		},
	}
}

#[test]
fn blessing_runtime_rejects_trigger_source_outside_listener_effects() {
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.stage_effects = vec![stage_damage(
		1,
		EffectTarget::TriggerEffectSource,
		1,
		DamageType::True,
	)];
	let error = compile_battle(&facts, &MaterializedGuide::default())
		.expect_err("trigger source has no meaning at stage scope");

	assert!(error.issues().iter().any(|issue| matches!(
		issue,
		BattleCompileIssue::InvalidReplayPrimitive { detail, .. }
			if detail.contains("only valid on a direct listener effect")
	)));
}

#[test]
fn blessing_runtime_targets_the_triggering_damage_source() {
	let source_cold = status_effect(
		"cold_damage_source",
		EffectTarget::TriggerEffectSource,
		StatusEffect::cold(200),
	);
	let mut retaliation = listener(
		"damage_source_listener",
		ReplayTrigger::Damage,
		ReplayRepeat::Unlimited,
		vec![source_cold],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut retaliation.effect {
		spec.actor_filter = TriggerActorFilter::SelfActor;
	}
	let long_path = vec![
		BattlePathStep {
			duration_ticks: 500,
			affected_by_movement_speed: true,
			action: BattlePathStepAction::Wait,
		},
		BattlePathStep {
			duration_ticks: 1,
			affected_by_movement_speed: true,
			action: BattlePathStepAction::Goal {
				goal: GoalId::new(1),
			},
		},
	];
	let mut facts = base_facts(long_path, vec![retaliation]);
	facts.end_at = Some(SimTick::new(110));
	let mut build = operator_build(Vec::new());
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let operator = build.id;
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 3 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid trigger-source listener");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(90)
	);
}

#[test]
fn unlimited_target_count_hits_every_currently_matching_actor() {
	let all_targets = TaggedEffect {
		mechanic: MechanicId::new("all_targets"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::TargetCount(TargetCountModifier::Unlimited {
			duration_ticks: None,
		}),
	};
	let mut build = operator_build(vec![all_targets]);
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let operator = build.id;
	let guide = MaterializedGuide {
		builds: vec![build],
		actions: vec![MaterializedGuideAction::Deploy {
			requested_at: SimTick::new(0),
			operator,
			cell: GridPosition { row: 0, col: 3 },
			facing: Facing::Left,
		}],
		reached_mechanics: Vec::new(),
	};
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(3));
	facts.waves[0].spawns.push(EnemySpawnFacts {
		id: EnemyInstanceId::new(2),
		enemy_key: "enemy".to_owned(),
		after_wave_start_ticks: 0,
		path: PathId::new(1),
		facing: Facing::Right,
	});
	let battle = compile_battle(&facts, &guide).expect("valid unlimited target-count modifier");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	for enemy in [EnemyInstanceId::new(1), EnemyInstanceId::new(2)] {
		assert_eq!(replay.enemies()[&enemy].remaining_hp, value(90));
	}
}

#[test]
fn blessing_runtime_combines_cold_and_positive_aspd_additively() {
	let attack_speed = TaggedEffect {
		mechanic: MechanicId::new("positive_aspd"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::AttackSpeed(AttackSpeedModifier {
			delta: 15,
			duration_ticks: None,
		}),
	};
	let cold = status_effect("cold", EffectTarget::SelfActor, StatusEffect::cold(500));
	let long_path = vec![
		BattlePathStep {
			duration_ticks: 500,
			affected_by_movement_speed: true,
			action: BattlePathStepAction::Wait,
		},
		BattlePathStep {
			duration_ticks: 1,
			affected_by_movement_speed: true,
			action: BattlePathStepAction::Goal {
				goal: GoalId::new(1),
			},
		},
	];
	let mut facts = base_facts(long_path, vec![attack_speed, cold]);
	facts.end_at = Some(SimTick::new(120));
	facts.enemies[0].combat.attack = value(10);
	facts.enemies[0].attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 100,
		range: vec![RangeOffset::new(0, 3)],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide(Vec::new(), None)).expect("valid ASPD stack");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.operators()[&OperatorInstanceId::new(1)].remaining_hp,
		value(90),
		"+15 ASPD and Cold -30 must schedule at 100 / 85, not compose interval scales",
	);
}

/// Percentage buffs that share a bucket sum inside it; separate buckets multiply. Two panel buffs
/// therefore stack more cheaply than two standalone scales, which is the whole reason the bucket is
/// carried on the modifier rather than folded away at compile time.
#[test]
fn stat_modifiers_sum_within_a_bucket_and_multiply_across_buckets() {
	let modifier =
		|name: &str, bucket: StatModifierBucket, multiplier: ExpectedValue| TaggedEffect {
			mechanic: MechanicId::new(name),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::StatModifier(StatModifier {
				stat: StatKind::Defense,
				bucket,
				multiplier,
				duration_ticks: None,
			}),
		};
	// Base defense 100 against a 400 physical hit, so the surviving HP reads the effective defense
	// directly: 1000 - (400 - defense).
	let remaining_hp = |on_spawn: Vec<TaggedEffect>| -> ExpectedValue {
		let mut facts = base_facts(long_wait_path(), on_spawn);
		facts.enemies[0].combat.max_hp = value(1000);
		facts.enemies[0].combat.defense = value(100);
		facts.stage_effects = vec![stage_damage(
			2,
			EffectTarget::AllEnemies,
			400,
			DamageType::Physical,
		)];
		let battle = compile_battle(&facts, &MaterializedGuide::default())
			.expect("stacked stat modifiers are valid");
		replay_expected(&battle, MaterializedGuidePolicy::default()).enemies()
			[&EnemyInstanceId::new(1)]
			.remaining_hp
	};
	let low = ExpectedValue::from_ratio(12, 10).expect("valid multiplier");
	let high = ExpectedValue::from_ratio(13, 10).expect("valid multiplier");

	// Two panel buffs share one bucket: 1 + 0.2 + 0.3, so defense reaches 150 and not 156.
	assert_eq!(
		remaining_hp(vec![
			modifier("panel_low", StatModifierBucket::Panel, low),
			modifier("panel_high", StatModifierBucket::Panel, high),
		]),
		value(750),
		"two panel buffs on one stat must sum inside their bucket",
	);
	// Two standalone scales are two buckets: 1.2 x 1.3, so defense reaches 156.
	assert_eq!(
		remaining_hp(vec![
			modifier("scale_low", StatModifierBucket::Scale, low),
			modifier("scale_high", StatModifierBucket::Scale, high),
		]),
		value(756),
		"two standalone scales on one stat must multiply",
	);
	// A lone modifier states what it does by itself, so the bucket cannot change it.
	for bucket in [StatModifierBucket::Panel, StatModifierBucket::Scale] {
		assert_eq!(
			remaining_hp(vec![modifier("only", bucket, low)]),
			value(720),
			"a lone {bucket:?} modifier must behave as its own multiplier",
		);
	}
	// One of each is still two buckets, so they multiply exactly as two scales would.
	assert_eq!(
		remaining_hp(vec![
			modifier("panel_low", StatModifierBucket::Panel, low),
			modifier("scale_high", StatModifierBucket::Scale, high),
		]),
		value(756),
		"a panel buff and a standalone scale occupy different buckets",
	);
}

/// Neither defense nor resistance may take a hit below the reviewed floor, a fraction of the raw
/// damage: a target that outclasses the attack entirely still takes a sliver rather than nothing.
#[test]
fn mitigation_floors_both_damage_families_at_the_reviewed_fraction() {
	let remaining_hp = |resistance: ExpectedValue,
	                    defense: ExpectedValue,
	                    damage_type: DamageType|
	 -> ExpectedValue {
		let mut facts = base_facts(long_wait_path(), Vec::new());
		facts.enemies[0].combat.max_hp = value(100);
		facts.enemies[0].combat.arts_resistance = resistance;
		facts.enemies[0].combat.defense = defense;
		facts.enemies[0].combat.minimum_damage_ratio =
			ActorCombatFacts::REVIEWED_MINIMUM_DAMAGE_RATIO;
		facts.stage_effects = vec![stage_damage(1, EffectTarget::AllEnemies, 40, damage_type)];
		let battle = compile_battle(&facts, &MaterializedGuide::default())
			.expect("a floored damage profile is valid");
		replay_expected(&battle, MaterializedGuidePolicy::default()).enemies()
			[&EnemyInstanceId::new(1)]
			.remaining_hp
	};

	// Full resistance and defense above the attack each swallow the whole hit, and the floor keeps
	// five percent of 40 alive on both sides.
	assert_eq!(
		remaining_hp(ExpectedValue::ONE, ExpectedValue::ZERO, DamageType::Arts),
		value(98),
		"Arts against full resistance must still land the two-point floor",
	);
	assert_eq!(
		remaining_hp(ExpectedValue::ZERO, value(400), DamageType::Physical),
		value(98),
		"physical against overwhelming defense must still land the two-point floor",
	);
	// Just above the floor the ordinary term wins, so the floor is not clamping every hit.
	assert_eq!(
		remaining_hp(
			ExpectedValue::from_ratio(9, 10).expect("valid resistance"),
			ExpectedValue::ZERO,
			DamageType::Arts,
		),
		value(96),
		"a hit above the floor must keep its own value",
	);
	assert_eq!(
		remaining_hp(ExpectedValue::ZERO, value(36), DamageType::Physical),
		value(96),
		"a hit above the floor must keep its own value",
	);
}

#[test]
fn blessing_runtime_second_cold_becomes_frozen_with_res_reduction() {
	let cold = status_effect("cold_one", EffectTarget::SelfActor, StatusEffect::cold(20));
	let second_cold = status_effect("cold_two", EffectTarget::SelfActor, StatusEffect::cold(20));
	// A target that has resistance to lose shows the promoted Frozen actually spending its 15
	// points: 30% resistance becomes 15%, so a 100 Arts hit lands 85.
	let mut facts = base_facts(long_wait_path(), vec![cold.clone(), second_cold.clone()]);
	facts.enemies[0].combat.max_hp = value(110);
	facts.enemies[0].combat.arts_resistance =
		ExpectedValue::from_ratio(3, 10).expect("valid resistance");
	facts.stage_effects = vec![stage_damage(
		1,
		EffectTarget::AllEnemies,
		100,
		DamageType::Arts,
	)];
	let battle = compile_battle(&facts, &MaterializedGuide::default())
		.expect("valid two-layer Cold transition");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(25),
		"promoted Frozen must normalize cadence and spend its RES -15 against 30% resistance",
	);

	// A target with no resistance to lose takes the hit unamplified. Reducing resistance past zero
	// leaves it at zero; it does not push the multiplier above 1 and manufacture damage.
	let mut bare_facts = base_facts(long_wait_path(), vec![cold, second_cold]);
	bare_facts.enemies[0].combat.max_hp = value(110);
	bare_facts.stage_effects = vec![stage_damage(
		1,
		EffectTarget::AllEnemies,
		100,
		DamageType::Arts,
	)];
	let bare_battle = compile_battle(&bare_facts, &MaterializedGuide::default())
		.expect("valid two-layer Cold transition");
	let bare_replay = replay_expected(&bare_battle, MaterializedGuidePolicy::default());
	assert_eq!(
		bare_replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(10),
		"RES -15 against no resistance must land 100, not 115",
	);

	let friendly_frozen = status_effect(
		"friendly_frozen",
		EffectTarget::SelfActor,
		StatusEffect::frozen(20),
	);
	let mut friendly_facts = base_facts(long_wait_path(), Vec::new());
	friendly_facts.stage_effects = vec![stage_damage(
		1,
		EffectTarget::AllAllies,
		100,
		DamageType::Arts,
	)];
	let mut friendly_guide = guide(vec![friendly_frozen], None);
	friendly_guide.builds[0].combat.max_hp = value(110);
	let friendly_battle =
		compile_battle(&friendly_facts, &friendly_guide).expect("valid friendly Frozen profile");
	let friendly_replay = replay_expected(&friendly_battle, MaterializedGuidePolicy::default());
	assert_eq!(
		friendly_replay.operators()[&OperatorInstanceId::new(1)].remaining_hp,
		value(10),
		"Frozen RES -15 applies only to hostile actors",
	);
}

#[test]
fn blessing_runtime_restore_is_silent_and_uses_effective_max_hp() {
	let max_hp = TaggedEffect {
		mechanic: MechanicId::new("double_max_hp"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::StatModifier(StatModifier {
			bucket: StatModifierBucket::Scale,
			stat: StatKind::MaxHp,
			multiplier: value(2),
			duration_ticks: None,
		}),
	};
	let mut revival = listener(
		"revival",
		ReplayTrigger::BeforeLethal,
		ReplayRepeat::Once,
		vec![restore_effect("restore", EffectTarget::CurrentTargets)],
	);
	let mut heal_probe = listener(
		"heal_probe",
		ReplayTrigger::Heal,
		ReplayRepeat::Once,
		vec![damage("heal_probe_damage", 1)],
	);
	for effect in [&mut revival, &mut heal_probe] {
		if let ExpectedEffect::RegisterListener(spec) = &mut effect.effect {
			spec.actor_filter = TriggerActorFilter::SelfActor;
		}
	}
	let mut facts = base_facts(long_wait_path(), vec![max_hp, revival, heal_probe]);
	facts.stage_effects = vec![stage_damage(
		2,
		EffectTarget::AllEnemies,
		200,
		DamageType::True,
	)];
	let battle =
		compile_battle(&facts, &MaterializedGuide::default()).expect("valid silent restore");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(100)
	);
	assert_eq!(replay.enemies()[&EnemyInstanceId::new(1)].death_at, None);
}

#[test]
fn blessing_runtime_before_lethal_preempts_same_tick_damage() {
	let mut revival = listener(
		"priority_revival",
		ReplayTrigger::BeforeLethal,
		ReplayRepeat::Once,
		vec![restore_effect(
			"priority_restore",
			EffectTarget::CurrentTargets,
		)],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut revival.effect {
		spec.actor_filter = TriggerActorFilter::SelfActor;
	}
	let mut facts = base_facts(long_wait_path(), vec![revival]);
	facts.stage_effects = vec![
		stage_damage(2, EffectTarget::AllEnemies, 100, DamageType::True),
		stage_damage(2, EffectTarget::AllEnemies, 1, DamageType::True),
	];
	let battle = compile_battle(&facts, &MaterializedGuide::default()).expect("valid lethal order");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(49)
	);
}

#[test]
fn blessing_runtime_once_listener_reinstalls_per_deployment() {
	let mut revival = listener(
		"deployment_revival",
		ReplayTrigger::BeforeLethal,
		ReplayRepeat::Once,
		vec![restore_effect(
			"deployment_restore",
			EffectTarget::SelfActor,
		)],
	);
	if let ExpectedEffect::RegisterListener(spec) = &mut revival.effect {
		spec.actor_filter = TriggerActorFilter::SelfActor;
		spec.lifetime = RuntimeEffectLifetime::SourceActor;
	}
	let operator = OperatorInstanceId::new(1);
	let mut facts = base_facts(long_wait_path(), Vec::new());
	facts.stage_effects = vec![
		stage_damage(1, EffectTarget::AllAllies, 100, DamageType::True),
		stage_damage(4, EffectTarget::AllAllies, 100, DamageType::True),
		stage_damage(5, EffectTarget::AllAllies, 100, DamageType::True),
	];
	let guide = MaterializedGuide {
		builds: vec![operator_build(vec![revival])],
		actions: vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(2),
				operator,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(3),
				operator,
				cell: GridPosition { row: 0, col: 3 },
				facing: Facing::Right,
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&facts, &guide).expect("valid deployment-scoped revival");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.operators()[&operator].death_at,
		Some(SimTick::new(5))
	);
}

fn classified_operator(
	id: u32,
	profession_category_mask: u16,
	group_id: Option<&str>,
	on_deploy: Vec<TaggedEffect>,
) -> OperatorBuildFacts {
	let mut build: OperatorBuildFacts = operator_build_for(OperatorInstanceId::new(id), on_deploy);
	build.classification = OperatorClassificationFacts {
		profession_category_mask,
		group_id: group_id.map(str::to_owned),
	};
	build
}

fn attacking_operator(id: u32, profession_category_mask: u16) -> OperatorBuildFacts {
	let mut build: OperatorBuildFacts =
		classified_operator(id, profession_category_mask, None, Vec::new());
	build.combat.attack = value(10);
	build.attack = AttackProfile {
		first_delay_ticks: 1,
		interval_ticks: 1,
		range: vec![
			RangeOffset::new(0, 1),
			RangeOffset::new(0, 2),
			RangeOffset::new(0, 3),
		],
		target_count: 1,
		target_filter: NormalAttackTargetFilter::GroundAndAir,
		action: NormalAttackAction::Damage {
			damage_type: DamageType::True,
			delivery: DamageDelivery::Normal,
		},
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	};
	build
}

fn conditional_attack_aura(
	condition: TeamCompositionCondition,
	target_profession_mask: u16,
	periodic: Option<ConditionalAuraPeriodicEffect>,
) -> TaggedEffect {
	TaggedEffect {
		mechanic: MechanicId::new("conditional_aura"),
		target: EffectTarget::SelfActor,
		effect: ExpectedEffect::InstallConditionalAura(RuntimeConditionalAura {
			key: MechanicId::new("conditional_aura"),
			condition,
			target_profession_mask,
			includes_source: false,
			lifetime: RuntimeEffectLifetime::SourceActor,
			buff: RuntimeBuff {
				key: MechanicId::new("conditional_aura_buff"),
				max_stacks: 1,
				duration_ticks: None,
				lifetime: RuntimeEffectLifetime::SourceActor,
				modifiers_per_stack: vec![BuffModifier::Stat(StatModifier {
					bucket: StatModifierBucket::Scale,
					stat: StatKind::Attack,
					multiplier: value(2),
					duration_ticks: None,
				})],
			},
			periodic,
		}),
	}
}

fn persistent_conditional_attack_aura(
	condition: TeamCompositionCondition,
	target_profession_mask: u16,
	periodic: Option<ConditionalAuraPeriodicEffect>,
) -> TaggedEffect {
	let mut aura: TaggedEffect =
		conditional_attack_aura(condition, target_profession_mask, periodic);
	let ExpectedEffect::InstallConditionalAura(spec) = &mut aura.effect else {
		unreachable!("conditional aura helper must install an aura");
	};
	spec.lifetime = RuntimeEffectLifetime::Persistent;
	spec.buff.lifetime = RuntimeEffectLifetime::Persistent;
	aura
}

fn with_battle_start_effects(
	mut build: OperatorBuildFacts,
	effects: Vec<TaggedEffect>,
) -> OperatorBuildFacts {
	build.on_battle_start = effects;
	build
}

fn deploy(operator: u32, at: u64, col: u32) -> MaterializedGuideAction {
	MaterializedGuideAction::Deploy {
		requested_at: SimTick::new(at),
		operator: OperatorInstanceId::new(operator),
		cell: GridPosition { row: 0, col },
		facing: Facing::Left,
	}
}

fn conditional_aura_facts(end_at: u64) -> ArkBattleFacts {
	let mut facts: ArkBattleFacts = base_facts(long_wait_path(), Vec::new());
	facts.end_at = Some(SimTick::new(end_at));
	facts.deployment.simultaneous_limit = 4;
	facts.grid.deployable_cells = BTreeSet::from([
		GridPosition { row: 0, col: 1 },
		GridPosition { row: 0, col: 2 },
		GridPosition { row: 0, col: 3 },
	]);
	facts
}

#[test]
fn conditional_aura_activates_dynamically_inherits_future_targets_and_removes_on_loss() {
	let aura: TaggedEffect = conditional_attack_aura(
		TeamCompositionCondition::DeployedDistinctProfessions {
			minimum_count: 2,
			profession_mask: 0b0011,
		},
		0b0100,
		None,
	);
	let guide = MaterializedGuide {
		builds: vec![
			classified_operator(1, 0b0001, None, vec![aura]),
			classified_operator(2, 0b0010, None, Vec::new()),
			attacking_operator(3, 0b0100),
		],
		actions: vec![
			deploy(1, 0, 3),
			deploy(2, 1, 2),
			deploy(3, 2, 1),
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(4),
				operator: OperatorInstanceId::new(2),
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle =
		compile_battle(&conditional_aura_facts(4), &guide).expect("valid dynamic conditional aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(70),
		"the future target attacks once while buffed, then once after condition loss",
	);
}

#[test]
fn conditional_aura_fixed_group_uses_complete_guide_roster() {
	let aura: TaggedEffect = conditional_attack_aura(
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count: 2,
			group_id: "shu".to_owned(),
		},
		0b0100,
		None,
	);
	let guide = MaterializedGuide {
		builds: vec![
			classified_operator(1, 0b0001, Some("shu"), vec![aura]),
			classified_operator(2, 0b0010, Some("shu"), Vec::new()),
			attacking_operator(3, 0b0100),
		],
		actions: vec![deploy(1, 0, 3), deploy(3, 1, 1)],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&conditional_aura_facts(2), &guide)
		.expect("valid fixed-roster conditional aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(80),
		"an undeployed roster member must satisfy the fixed-group condition",
	);
}

#[test]
fn conditional_aura_periodic_sp_uses_per_target_cadence() {
	let aura: TaggedEffect = conditional_attack_aura(
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count: 1,
			group_id: "aura".to_owned(),
		},
		0b0010,
		Some(ConditionalAuraPeriodicEffect {
			initial_delay_ticks: 1,
			interval_ticks: 1,
			effects: vec![TaggedEffect {
				mechanic: MechanicId::new("conditional_aura_sp"),
				target: EffectTarget::CurrentTargets,
				effect: ExpectedEffect::GainSp { amount: 1 },
			}],
		}),
	);
	let mut caster: OperatorBuildFacts = classified_operator(2, 0b0010, None, Vec::new());
	caster.skill = Some(SkillFacts {
		id: MechanicId::new("aura_charged_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost: 2,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("aura_charged_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(30),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	});
	let guide = MaterializedGuide {
		builds: vec![
			classified_operator(1, 0b0001, Some("aura"), vec![aura]),
			caster,
		],
		actions: vec![deploy(1, 0, 3), deploy(2, 1, 2)],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&conditional_aura_facts(3), &guide)
		.expect("valid periodic conditional aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(70),
	);
}

#[test]
fn persistent_fixed_roster_aura_works_before_source_deploy_and_survives_source_death() {
	let aura: TaggedEffect = persistent_conditional_attack_aura(
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count: 1,
			group_id: "aura".to_owned(),
		},
		0b0010,
		None,
	);
	let source_death: TaggedEffect = listener(
		"persistent_source_death",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 1,
			interval_ticks: 100,
		},
		ReplayRepeat::Once,
		vec![TaggedEffect {
			mechanic: MechanicId::new("persistent_source_death"),
			target: EffectTarget::CurrentTargets,
			effect: ExpectedEffect::Death,
		}],
	);
	let source: OperatorBuildFacts = with_battle_start_effects(
		classified_operator(1, 0b0001, Some("aura"), vec![source_death]),
		vec![aura],
	);
	let guide = MaterializedGuide {
		builds: vec![source, attacking_operator(2, 0b0010)],
		actions: vec![deploy(2, 0, 1), deploy(1, 1, 3)],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&conditional_aura_facts(4), &guide)
		.expect("valid persistent fixed-roster aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(20),
		"target must stay buffed before source deployment and after source death",
	);
	assert_eq!(
		replay.operators()[&OperatorInstanceId::new(1)].death_at,
		Some(SimTick::new(2)),
	);
}

#[test]
fn persistent_fixed_roster_aura_survives_source_retreat() {
	let aura: TaggedEffect = persistent_conditional_attack_aura(
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count: 1,
			group_id: "aura".to_owned(),
		},
		0b0010,
		None,
	);
	let source: OperatorBuildFacts = with_battle_start_effects(
		classified_operator(1, 0b0001, Some("aura"), Vec::new()),
		vec![aura],
	);
	let guide = MaterializedGuide {
		builds: vec![source, attacking_operator(2, 0b0010)],
		actions: vec![
			deploy(2, 0, 1),
			deploy(1, 1, 3),
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(2),
				operator: OperatorInstanceId::new(1),
			},
		],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&conditional_aura_facts(4), &guide)
		.expect("valid persistent fixed-roster aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(20),
		"target must stay buffed after the source retreats",
	);
}

fn periodic_sp_aura(initial_delay_ticks: u64, interval_ticks: u64) -> TaggedEffect {
	persistent_conditional_attack_aura(
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count: 1,
			group_id: "aura".to_owned(),
		},
		0b0010,
		Some(ConditionalAuraPeriodicEffect {
			initial_delay_ticks,
			interval_ticks,
			effects: vec![TaggedEffect {
				mechanic: MechanicId::new("conditional_aura_sp"),
				target: EffectTarget::CurrentTargets,
				effect: ExpectedEffect::GainSp { amount: 1 },
			}],
		}),
	)
}

fn one_sp_damage_skill() -> SkillFacts {
	SkillFacts {
		id: MechanicId::new("aura_charged_skill"),
		activation: SkillActivationRule::Auto,
		initial_sp: 0,
		sp_cost: 1,
		charge: SpChargeRule::None,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: vec![TaggedEffect {
			mechanic: MechanicId::new("aura_charged_damage"),
			target: EffectTarget::AllEnemies,
			effect: ExpectedEffect::Damage {
				amount: value(30),
				damage_type: DamageType::True,
				delivery: DamageDelivery::Normal,
			},
		}],
	}
}

#[test]
fn conditional_aura_target_deployed_before_old_boundary_waits_full_interval() {
	let source: OperatorBuildFacts = with_battle_start_effects(
		classified_operator(1, 0b0001, Some("aura"), Vec::new()),
		vec![periodic_sp_aura(4, 4)],
	);
	let mut caster: OperatorBuildFacts = classified_operator(2, 0b0010, None, Vec::new());
	caster.skill = Some(one_sp_damage_skill());
	let guide = MaterializedGuide {
		builds: vec![source, caster],
		actions: vec![deploy(2, 3, 2)],
		reached_mechanics: Vec::new(),
	};
	let battle =
		compile_battle(&conditional_aura_facts(7), &guide).expect("valid per-target periodic aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let activation_ticks: Vec<SimTick> = replay
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled }
				if matches!(
					scheduled.event(),
					ExpectedEvent::SkillActivation { operator, .. }
						if *operator == OperatorInstanceId::new(2)
				) =>
			{
				Some(scheduled.at())
			}
			_ => None,
		})
		.collect();

	assert_eq!(activation_ticks, vec![SimTick::new(7)]);
}

#[test]
fn conditional_aura_redeployment_resets_target_cadence() {
	let source: OperatorBuildFacts = with_battle_start_effects(
		classified_operator(1, 0b0001, Some("aura"), Vec::new()),
		vec![periodic_sp_aura(2, 2)],
	);
	let mut caster: OperatorBuildFacts = classified_operator(2, 0b0010, None, Vec::new());
	caster.skill = Some(one_sp_damage_skill());
	let guide = MaterializedGuide {
		builds: vec![source, caster],
		actions: vec![
			deploy(2, 1, 2),
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(4),
				operator: OperatorInstanceId::new(2),
			},
			deploy(2, 5, 2),
		],
		reached_mechanics: Vec::new(),
	};
	let battle =
		compile_battle(&conditional_aura_facts(7), &guide).expect("valid per-target periodic aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());
	let activation_ticks: Vec<SimTick> = replay
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled }
				if matches!(
					scheduled.event(),
					ExpectedEvent::SkillActivation { operator, .. }
						if *operator == OperatorInstanceId::new(2)
				) =>
			{
				Some(scheduled.at())
			}
			_ => None,
		})
		.collect();

	assert_eq!(activation_ticks, vec![SimTick::new(3), SimTick::new(7)]);
}

#[test]
fn conditional_aura_source_death_removes_target_buff_and_stales_pulses() {
	let aura: TaggedEffect = conditional_attack_aura(
		TeamCompositionCondition::FixedRosterGroup {
			minimum_count: 1,
			group_id: "aura".to_owned(),
		},
		0b0010,
		Some(ConditionalAuraPeriodicEffect {
			initial_delay_ticks: 1,
			interval_ticks: 1,
			effects: vec![TaggedEffect {
				mechanic: MechanicId::new("harmless_sp"),
				target: EffectTarget::CurrentTargets,
				effect: ExpectedEffect::GainSp { amount: 1 },
			}],
		}),
	);
	let source_death: TaggedEffect = listener(
		"source_death",
		ReplayTrigger::Periodic {
			initial_delay_ticks: 3,
			interval_ticks: 100,
		},
		ReplayRepeat::Once,
		vec![TaggedEffect {
			mechanic: MechanicId::new("source_death"),
			target: EffectTarget::CurrentTargets,
			effect: ExpectedEffect::Death,
		}],
	);
	let guide = MaterializedGuide {
		builds: vec![
			classified_operator(1, 0b0001, Some("aura"), vec![aura, source_death]),
			attacking_operator(2, 0b0010),
		],
		actions: vec![deploy(1, 0, 3), deploy(2, 1, 2)],
		reached_mechanics: Vec::new(),
	};
	let battle = compile_battle(&conditional_aura_facts(4), &guide)
		.expect("valid source-owned conditional aura");
	let replay = replay_expected(&battle, MaterializedGuidePolicy::default());

	assert_eq!(
		replay.enemies()[&EnemyInstanceId::new(1)].remaining_hp,
		value(60),
		"tick 2 is buffed; ticks 3 and 4 are unbuffed after source death",
	);
	assert_eq!(
		replay.operators()[&OperatorInstanceId::new(1)].death_at,
		Some(SimTick::new(3)),
	);
}
