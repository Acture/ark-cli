//! Translation from verified Ark data and one exact MAA job file seed into replay facts.

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::{
	ArkDataError, ArkDataSnapshot, BasePanelBuild, BattlePrefabSnapshot, BlackboardEntry,
	Buildable, CompiledEnemy, CompiledStage, EnemyKey, FunctionDisableMask, GridPosition,
	MapDirection, MechanicKind, MechanicOrigin, MechanicReference, MotionMode, OperatorBuild,
	OperatorEffectCandidate, OperatorIdentity, OperatorModulePartTarget, OperatorPosition,
	PredefinedCollection, PredeployedStageActor, ResolvedOperatorBuild, SkillActivationType,
	SkillSpType, StageAction, StageActionKind, StageActionSchedulePeriod, StageDifficulty,
	StageIdentity, StageRef, StageVector2, TileHeight, WaveActionContainer,
};
use crate::simulation::TickRate;

use super::engine::rounded_up_ticks;
use crate::mechanics::{
	compile::{
		built_in_battle_prefab_handler_registry, claims_charge_cost_skill_template,
		claims_conditional_heal_attack_template, claims_sbell_skill_template,
		compile_base_panel_normal_attack_facts, compile_expected_damage_skill,
		decode_plain_enemy_prefab, interpret_battle_prefab, interpret_battle_prefab_asset,
		review_operator_mechanic_override, review_undescribed_enemy, try_decode_blessing_template,
		try_decode_block_toggle_template, try_decode_charge_cost_skill_template,
		try_decode_phalanx_trait, try_decode_sbell_skill_template, try_decode_shu_roster_template,
		try_decode_shu_skill_template, try_decode_snowfield_template,
		try_decode_sowed_field_template, BattlePrefabInterpretError,
	},
	domain::{
		ActorCombatFacts, ApproximationTrigger, ArkBattleFacts, AttackCadenceModifier,
		AttackPattern, AttackProfile, AttackRangeModifier, AttackSpeedModifier,
		BasePanelNormalAttackFacts, BattleGrid, BattleGridCellMetadata, BattlePath, BattlePathStep,
		BattlePathStepAction, BattleTimeAnchor, BattleWave, ChargeCostSkillActiveSpan,
		DamageDelivery, DamageType, DecodedChargeCostSkillTemplate, DecodedPlainEnemyPrefab,
		DecodedShuRosterTemplate, EffectTarget, EnemyFacts, EnemyInstanceId, EnemySpawnFacts,
		ExactRatio, ExpectedAttackPhase, ExpectedDamageDelivery, ExpectedDamageSkillEntry,
		ExpectedDamageSkillProfile, ExpectedDamageType, ExpectedDeploymentRules, ExpectedDpRules,
		ExpectedEffect, ExpectedSkillDuration, ExpectedTargetFilter, ExpectedValue, Facing,
		Faction, FixedDamage, GoalId, HitCountModifier, LoweredMechanicSemanticProgram,
		MaterializedGuide, MaterializedGuideAction, MaterializedGuidePolicy, MechanicBehavior,
		MechanicId, MechanicSemanticError, MechanicSemanticProgram, NormalAttackAction,
		NormalAttackCatalogEntry, NormalAttackDamageType, NormalAttackTargetFilter,
		OperatorBuildFacts, OperatorClassificationFacts, OperatorInstanceId,
		OperatorMechanicOverride, OperatorMechanicOverrideEntry, PathId, PermanentModifier,
		PlainEnemyAttackForm, PlainEnemyPrefabDecode, PlainEnemyPrefabDecodeError,
		PlainEnemyTableFacts, PreplacedActorFacts, ReachedMechanic, RelativeBattleTime,
		ReviewedEnemyDamageType, ShuRosterCondition, ShuRosterConditionalBranch,
		SkillActivationRule, SkillDamageMechanicTemplate, SkillFacts, SkillManualDiscardPolicy,
		SpChargeRule, StatKind, StatModifier, StatModifierBucket, SummonCellOccupancy,
		SummonTemplate, SummonTemplateCatalog, SummonTemplateCatalogError, SummonTemplateId,
		TaggedEffect, TargetClass, TargetCountModifier, Targetability, UndescribedEnemy, WaveId,
		WaveStartWindow,
	},
	prefab::{
		BattlePrefabBuffConfigurationSet, BattlePrefabGraph, BattlePrefabHandlerRegistry,
		BattlePrefabObjectSource, BattlePrefabRootSection, BattlePrefabSemantic,
		BattlePrefabSemanticFact, BattlePrefabSemanticIssue, BattlePrefabStaticBuffAbility,
	},
	stage::{
		compile_route_timeline, ticks_from_seconds_ceil, RouteTimeline, RouteTimelineAction,
		SpawnCohortId, SpawnInstanceId, StageCompileError,
	},
};

/// One source operator bound to one exact, revision-pinned replay build.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExactReplayBuildBinding {
	pub source_name: String,
	pub build: OperatorBuild,
	pub skill_usage: u8,
	pub skill_times: Option<u32>,
}

/// Executor-neutral materialized guide produced by an outer workflow adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedReplayGuide {
	pub actions: Vec<MaterializedGuideAction>,
	pub policy: MaterializedGuidePolicy,
	pub operator_ids: BTreeMap<String, OperatorInstanceId>,
}

/// Complete lower-layer compile contract for one exact replay variant.
#[derive(Clone, Copy, Debug)]
pub struct ExpectedReplayCompileInput<'a> {
	pub stage_name: &'a str,
	pub difficulty: StageDifficulty,
	pub bindings: &'a [ExactReplayBuildBinding],
	pub guide: &'a PreparedReplayGuide,
	pub tick_rate: TickRate,
}

// Explicit standard-rule assumptions for this adapter revision. The local stage and operator
// snapshots expose the simultaneous limit and respawn time, but not these DP formulas.
const STANDARD_SECOND_DEPLOYMENT_COST_MULTIPLIER: ExpectedValue =
	ExpectedValue::from_scaled(ExpectedValue::SCALE * 3 / 2);
const STANDARD_LATER_DEPLOYMENT_COST_MULTIPLIER: ExpectedValue =
	ExpectedValue::from_scaled(ExpectedValue::SCALE * 2);
const STANDARD_RETREAT_REFUND_RATIO: ExpectedValue =
	ExpectedValue::from_scaled(ExpectedValue::SCALE / 2);

/// Complete preflight input for one exact guide/build variant.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedReplayInput {
	pub data_revision: String,
	/// Identity of the verified battle-prefab snapshot: common bundles, charpacks and, when pinned,
	/// enemy packs. Mechanics with `verified-prefab:` origins were interpreted from it; those with
	/// `reviewed-gamedata:` origins are separately guarded by exact gamedata-shape checks and do
	/// not claim to have been interpreted from this prefab snapshot.
	pub mechanics_revision: String,
	pub stage_id: String,
	/// Stable bridge from replay enemy IDs to the planner's spawn-action identity.
	pub enemy_lineage: BTreeMap<EnemyInstanceId, SpawnInstanceId>,
	pub resolved_builds: Vec<ResolvedOperatorBuild>,
	pub facts: ArkBattleFacts,
	pub guide: MaterializedGuide,
	pub policy: MaterializedGuidePolicy,
}

/// Materializes one normalized exact guide without pretending unsupported mechanics are absent.
///
/// The resulting facts can always be passed to `compile_battle`. Every reached operator, enemy,
/// or preplaced-actor behavior that is not yet mapped is retained as `Unsupported`, producing one
/// complete fail-closed support report before simulation.
pub fn materialize_expected_replay_input(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	input: ExpectedReplayCompileInput<'_>,
) -> Result<ExpectedReplayInput, ExpectedReplayAdapterError> {
	prefabs
		.ensure_compatible_with(snapshot)
		.map_err(ExpectedReplayAdapterError::ArkData)?;
	if input.difficulty != StageDifficulty::Normal {
		return Err(ExpectedReplayAdapterError::UnsupportedStageShape {
			detail: "expected-replay adapter v0 supports Normal stages only".to_owned(),
		});
	}
	let stage: CompiledStage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(input.stage_name.to_owned()),
			difficulty: input.difficulty,
		})
		.map_err(ExpectedReplayAdapterError::ArkData)?;
	if !stage.branches().is_empty() || stage.waves().len() != 1 {
		return Err(ExpectedReplayAdapterError::UnsupportedStageShape {
			detail: format!(
				"expected one branch-free stage wave, found {} waves and {} branches",
				stage.waves().len(),
				stage.branches().len()
			),
		});
	}
	validate_prepared_bindings(input.guide, input.bindings)?;
	let (paths, waves, enemy_classes, active_route_indices, enemy_lineage, mut path_approximations) =
		compile_stage_schedule(&stage, input.tick_rate)?;
	let enemies: Vec<EnemyFacts> =
		compile_enemies(&stage, prefabs, &enemy_classes, input.tick_rate)?;
	let mut summon_catalog: SummonTemplateCatalog = SummonTemplateCatalog::new();
	let preplaced_actors: Vec<PreplacedActorFacts> =
		compile_preplaced(snapshot, &stage, input.tick_rate, &mut summon_catalog)?;
	let (builds, resolved_builds): (Vec<OperatorBuildFacts>, Vec<ResolvedOperatorBuild>) =
		compile_operator_builds(
			snapshot,
			prefabs,
			&stage,
			input.bindings,
			input.guide,
			input.tick_rate,
			&mut summon_catalog,
		)?;
	let guide: MaterializedGuide = MaterializedGuide {
		builds,
		actions: input.guide.actions.clone(),
		reached_mechanics: Vec::new(),
	};
	let mut reached_mechanics: Vec<ReachedMechanic> =
		compile_stage_mechanics(&stage, &active_route_indices, &enemy_classes);
	reached_mechanics.append(&mut path_approximations);
	let facts: ArkBattleFacts = ArkBattleFacts {
		battle_id: format!(
			"{}:{}:{}:{}",
			stage.stage_id(),
			stage.level_id(),
			snapshot.resolved_revision(),
			prefabs.mechanics_revision()
		),
		end_at: None,
		dp: compile_dp_rules(&stage, input.tick_rate)?,
		deployment: compile_deployment_rules(&stage),
		grid: compile_grid(&stage)?,
		paths,
		waves,
		enemies,
		summons: summon_catalog.into_templates(),
		preplaced_actors,
		stage_effects: Vec::new(),
		reached_mechanics,
	};
	Ok(ExpectedReplayInput {
		data_revision: snapshot.resolved_revision().to_owned(),
		mechanics_revision: prefabs.mechanics_revision().to_owned(),
		stage_id: stage.stage_id().to_owned(),
		enemy_lineage,
		resolved_builds,
		facts,
		guide,
		policy: input.guide.policy.clone(),
	})
}

fn validate_prepared_bindings(
	prepared: &PreparedReplayGuide,
	bindings: &[ExactReplayBuildBinding],
) -> Result<(), ExpectedReplayAdapterError> {
	let expected: BTreeSet<&str> = prepared.operator_ids.keys().map(String::as_str).collect();
	let actual: BTreeSet<&str> = bindings
		.iter()
		.map(|binding| binding.source_name.as_str())
		.collect();
	if expected != actual || expected.len() != bindings.len() {
		return Err(ExpectedReplayAdapterError::BuildBindingMismatch {
			expected: expected.into_iter().map(str::to_owned).collect(),
			actual: actual.into_iter().map(str::to_owned).collect(),
		});
	}
	Ok(())
}

#[allow(clippy::type_complexity)]
fn compile_stage_schedule(
	stage: &CompiledStage,
	tick_rate: TickRate,
) -> Result<
	(
		Vec<BattlePath>,
		Vec<BattleWave>,
		BTreeMap<String, TargetClass>,
		BTreeSet<u32>,
		BTreeMap<EnemyInstanceId, SpawnInstanceId>,
		Vec<ReachedMechanic>,
	),
	ExpectedReplayAdapterError,
> {
	let stage_wave = &stage.waves()[0];
	let mut paths: Vec<BattlePath> = Vec::new();
	let mut path_approximations: Vec<ReachedMechanic> = Vec::new();
	let mut waves: Vec<BattleWave> = Vec::new();
	let mut enemy_classes: BTreeMap<String, TargetClass> = BTreeMap::new();
	let mut active_route_indices: BTreeSet<u32> = BTreeSet::new();
	let mut enemy_lineage: BTreeMap<EnemyInstanceId, SpawnInstanceId> = BTreeMap::new();
	let stage_pre_delay_ticks: u64 =
		ticks_from_seconds_ceil(stage_wave.pre_delay_seconds(), tick_rate)
			.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
	for (fragment_index, fragment) in stage_wave.fragments().iter().enumerate() {
		let wave_id: WaveId = WaveId::new(
			u32::try_from(fragment_index)
				.map_err(|_| ExpectedReplayAdapterError::IdentifierOverflow)?,
		);
		let pre_delay_ticks: u64 = ticks_from_seconds_ceil(fragment.pre_delay_seconds(), tick_rate)
			.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
		let start: WaveStartWindow = if fragment_index == 0 {
			WaveStartWindow {
				earliest: RelativeBattleTime {
					anchor: BattleTimeAnchor::BattleStarted,
					offset_ticks: stage_pre_delay_ticks
						.checked_add(pre_delay_ticks)
						.ok_or(ExpectedReplayAdapterError::TickOverflow)?,
				},
				latest: None,
			}
		} else {
			WaveStartWindow {
				earliest: RelativeBattleTime {
					anchor: BattleTimeAnchor::WaveCleared(WaveId::new(
						u32::try_from(fragment_index - 1)
							.map_err(|_| ExpectedReplayAdapterError::IdentifierOverflow)?,
					)),
					offset_ticks: pre_delay_ticks,
				},
				latest: None,
			}
		};
		let mut spawns: Vec<EnemySpawnFacts> = Vec::new();
		for action in fragment.actions() {
			if !stage.action_is_active(action) {
				continue;
			}
			let Some(spawn) = action.spawn() else {
				if action_is_executor_inert(action.kind()) {
					validate_wave_action_schedule(action)?;
					continue;
				}
				return Err(ExpectedReplayAdapterError::UnsupportedStageAction {
					origin: format!("{:?}", action.origin()),
					kind: action.kind().stable_id().to_owned(),
				});
			};
			validate_spawn_action(action)?;
			let enemy_key: EnemyKey = EnemyKey::new(action.key().to_owned())
				.map_err(ExpectedReplayAdapterError::ArkData)?;
			let enemy: &CompiledEnemy = stage.enemy(&enemy_key).ok_or_else(|| {
				ExpectedReplayAdapterError::MissingEnemy {
					enemy_key: action.key().to_owned(),
				}
			})?;
			let move_speed: f64 =
				required_enemy_f64(action.key(), "move_speed", enemy.stats().move_speed())?;
			let route = stage
				.routes()
				.get(spawn.route_index() as usize)
				.ok_or_else(|| ExpectedReplayAdapterError::InvalidRouteIndex {
					route_index: spawn.route_index(),
				})?;
			let target_class: TargetClass = match route.motion() {
				MotionMode::Walk => TargetClass::Ground,
				MotionMode::Fly => TargetClass::Air,
				MotionMode::Unknown(value) => {
					return Err(ExpectedReplayAdapterError::UnsupportedMotion {
						value: value.clone(),
					});
				}
			};
			if enemy_classes
				.insert(action.key().to_owned(), target_class)
				.is_some_and(|previous| previous != target_class)
			{
				return Err(ExpectedReplayAdapterError::InconsistentEnemyMotion {
					enemy_key: action.key().to_owned(),
				});
			}
			let timeline: RouteTimeline =
				compile_route_timeline(stage, spawn.route_index(), move_speed, tick_rate)
					.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
			active_route_indices.insert(spawn.route_index());
			let path_index: u32 = u32::try_from(paths.len())
				.map_err(|_| ExpectedReplayAdapterError::IdentifierOverflow)?;
			let path_id: PathId = PathId::new(path_index);
			let goal_id: GoalId = GoalId::new(path_index);
			let initial_facing: Facing = initial_route_facing(&timeline, spawn.route_index())?;
			if let Some(approximation) = spawn_scatter_approximation(
				stage,
				route.spawn_random_range(),
				spawn.route_index(),
				path_id,
			) {
				path_approximations.push(approximation);
			}
			let cohort: SpawnCohortId = SpawnCohortId::new(stage_spawn_cohort_id(action))
				.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
			paths.push(BattlePath {
				id: path_id,
				goal: goal_id,
				start: timeline.start,
				steps: timeline
					.steps
					.into_iter()
					.map(|step| BattlePathStep {
						duration_ticks: step.duration_ticks,
						affected_by_movement_speed: step.affected_by_movement_speed,
						action: match step.action {
							RouteTimelineAction::Move { cell, facing } => {
								BattlePathStepAction::Move { cell, facing }
							}
							RouteTimelineAction::Wait => BattlePathStepAction::Wait,
							RouteTimelineAction::Disappear => BattlePathStepAction::Disappear,
							RouteTimelineAction::Reappear { cell, facing } => {
								BattlePathStepAction::Reappear { cell, facing }
							}
							RouteTimelineAction::Goal => {
								BattlePathStepAction::Goal { goal: goal_id }
							}
						},
					})
					.collect(),
			});
			for ordinal in 0..spawn.count() {
				let spawn_seconds: f64 =
					spawn.pre_delay_seconds() + spawn.interval_seconds() * f64::from(ordinal);
				let spawn_ticks: u64 = ticks_from_seconds_ceil(spawn_seconds, tick_rate)
					.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
				let instance_index: u32 = u32::try_from(
					waves
						.iter()
						.map(|wave: &BattleWave| wave.spawns.len())
						.sum::<usize>()
						.checked_add(spawns.len())
						.ok_or(ExpectedReplayAdapterError::IdentifierOverflow)?,
				)
				.map_err(|_| ExpectedReplayAdapterError::IdentifierOverflow)?;
				let enemy_instance_id: EnemyInstanceId = EnemyInstanceId::new(instance_index);
				let previous: Option<SpawnInstanceId> = enemy_lineage.insert(
					enemy_instance_id,
					SpawnInstanceId {
						cohort: cohort.clone(),
						ordinal,
					},
				);
				debug_assert!(previous.is_none(), "replay enemy IDs are globally unique");
				spawns.push(EnemySpawnFacts {
					id: enemy_instance_id,
					enemy_key: action.key().to_owned(),
					after_wave_start_ticks: spawn_ticks,
					path: path_id,
					facing: initial_facing,
				});
			}
		}
		waves.push(BattleWave {
			id: wave_id,
			start,
			spawns,
		});
	}
	Ok((
		paths,
		waves,
		enemy_classes,
		active_route_indices,
		enemy_lineage,
		path_approximations,
	))
}

/// Wave-action kinds that only drive presentation and never the battle.
///
/// These narrate, frame the camera, or open the enemy panel. None of them creates, removes, or
/// retimes a battle actor, so skipping one leaves every sibling action's
/// `preDelay + ordinal * interval` schedule untouched. Every other kind stays fail-closed: anything
/// that activates, triggers, or withdraws a predefined actor, fires a battle event, or reveals
/// hidden cards can change the battle and must be modelled before it may be admitted. `TUTORIAL`,
/// `EMPTY`, `E_NUM` and unrecognised ids stay closed for the same reason: no reviewed evidence says
/// what they do. An inert kind is skipped only after `validate_wave_action_schedule` proves it also
/// schedules inertly.
const EXECUTOR_INERT_ACTION_IDS: [&str; 6] = [
	"DIALOG",
	"DISPLAY_ENEMY_INFO",
	"PLAY_BGM",
	"PLAY_OPERA",
	"PREVIEW_CURSOR",
	"STORY",
];

fn action_is_executor_inert(kind: &StageActionKind) -> bool {
	action_id_is_executor_inert(kind.stable_id())
}

fn action_id_is_executor_inert(stable_id: &str) -> bool {
	EXECUTOR_INERT_ACTION_IDS.contains(&stable_id)
}

fn stage_spawn_cohort_id(action: &StageAction) -> String {
	match &action.origin().container {
		WaveActionContainer::Wave { wave, fragment } => {
			format!(
				"wave:{wave}/fragment:{fragment}/action:{}",
				action.origin().action
			)
		}
		WaveActionContainer::Branch { branch, phase } => {
			format!(
				"branch:{branch}/phase:{phase}/action:{}",
				action.origin().action
			)
		}
	}
}

fn initial_route_facing(
	timeline: &RouteTimeline,
	route_index: u32,
) -> Result<Facing, ExpectedReplayAdapterError> {
	timeline
		.initial_facing()
		.ok_or(ExpectedReplayAdapterError::MissingRouteFacing { route_index })
}

fn validate_spawn_action(action: &StageAction) -> Result<(), ExpectedReplayAdapterError> {
	validate_wave_action_schedule(action)?;
	if action.random_spawn_group().is_some()
		|| action.random_spawn_pack().is_some()
		|| action.is_unharmful_and_always_count_as_killed()
	{
		return Err(ExpectedReplayAdapterError::UnsupportedSpawnSchedule {
			origin: format!("{:?}", action.origin()),
		});
	}
	Ok(())
}

/// Rejects the gating fields any wave action may carry, whether or not it spawns.
///
/// A skipped presentation action is only inert if it also schedules like one. `block_fragment` and
/// `dont_block_wave` change when a wave or fragment is considered finished, and a non-`Always`
/// schedule period makes the action conditional; nothing downstream reads any of them, so an action
/// that declares one must fail closed rather than be dropped.
fn validate_wave_action_schedule(action: &StageAction) -> Result<(), ExpectedReplayAdapterError> {
	if action.block_fragment()
		|| action.dont_block_wave()
		|| action.force_block_wave_in_branch()
		|| action.random_type() != StageActionSchedulePeriod::Always
		|| action.refresh_type() != StageActionSchedulePeriod::Always
	{
		return Err(ExpectedReplayAdapterError::UnsupportedSpawnSchedule {
			origin: format!("{:?}", action.origin()),
		});
	}
	Ok(())
}

fn compile_grid(stage: &CompiledStage) -> Result<BattleGrid, ExpectedReplayAdapterError> {
	let mut cells: BTreeMap<GridPosition, BattleGridCellMetadata> = BTreeMap::new();
	let mut deployable_cells: BTreeSet<GridPosition> = BTreeSet::new();
	let mut ground_cells: BTreeSet<GridPosition> = BTreeSet::new();
	for cell in stage.cells() {
		let metadata: BattleGridCellMetadata = BattleGridCellMetadata::from(cell);
		match cell.height() {
			TileHeight::Lowland | TileHeight::Highland => {}
			TileHeight::Unknown(value) => {
				return Err(ExpectedReplayAdapterError::UnsupportedTileHeight {
					value: value.clone(),
				});
			}
		}
		if metadata.is_walkable_lowland() {
			ground_cells.insert(cell.position());
		}
		cells.insert(cell.position(), metadata);
		match cell.buildable() {
			Buildable::Melee | Buildable::Ranged | Buildable::All => {
				deployable_cells.insert(cell.position());
			}
			Buildable::None => {}
			Buildable::Unknown(value) => {
				return Err(ExpectedReplayAdapterError::UnsupportedBuildable {
					value: value.clone(),
				});
			}
		}
	}
	Ok(BattleGrid {
		dimensions: stage.map_dimensions(),
		cells,
		deployable_cells,
		ground_cells,
	})
}

fn compile_dp_rules(
	stage: &CompiledStage,
	tick_rate: TickRate,
) -> Result<ExpectedDpRules, ExpectedReplayAdapterError> {
	let regen_interval_ticks: u64 =
		ticks_from_seconds_ceil(stage.options().cost_increase_time(), tick_rate)
			.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
	if regen_interval_ticks == 0 {
		return Err(ExpectedReplayAdapterError::InvalidDpRules);
	}
	Ok(ExpectedDpRules {
		initial: stage.options().initial_cost(),
		maximum: stage.options().max_cost(),
		regen_amount: 1,
		regen_interval_ticks,
	})
}

fn compile_deployment_rules(stage: &CompiledStage) -> ExpectedDeploymentRules {
	ExpectedDeploymentRules {
		simultaneous_limit: stage.options().character_limit(),
		second_deployment_cost_multiplier: STANDARD_SECOND_DEPLOYMENT_COST_MULTIPLIER,
		later_deployment_cost_multiplier: STANDARD_LATER_DEPLOYMENT_COST_MULTIPLIER,
		retreat_refund_ratio: STANDARD_RETREAT_REFUND_RATIO,
	}
}

fn compile_enemies(
	stage: &CompiledStage,
	prefabs: &BattlePrefabSnapshot,
	enemy_classes: &BTreeMap<String, TargetClass>,
	tick_rate: TickRate,
) -> Result<Vec<EnemyFacts>, ExpectedReplayAdapterError> {
	let registry: BattlePrefabHandlerRegistry = built_in_battle_prefab_handler_registry();
	let mut compiled: Vec<EnemyFacts> = Vec::with_capacity(enemy_classes.len());
	for (enemy_key, target_class) in enemy_classes {
		let key: EnemyKey =
			EnemyKey::new(enemy_key.clone()).map_err(ExpectedReplayAdapterError::ArkData)?;
		let enemy: &CompiledEnemy =
			stage
				.enemy(&key)
				.ok_or_else(|| ExpectedReplayAdapterError::MissingEnemy {
					enemy_key: enemy_key.clone(),
				})?;
		let stats = enemy.stats();
		let attack_interval: u64 = attack_interval_ticks(
			required_enemy_f64(enemy_key, "base_attack_time", stats.base_attack_time())?,
			required_enemy_f64(enemy_key, "attack_speed", stats.attack_speed())?,
			tick_rate,
		)?;
		let mut attack: AttackProfile = AttackProfile {
			first_delay_ticks: attack_interval,
			interval_ticks: attack_interval,
			range: enemy_range_offsets(stats.range_radius())?,
			target_count: 1,
			target_filter: NormalAttackTargetFilter::GroundOnly,
			action: NormalAttackAction::Damage {
				damage_type: DamageType::Physical,
				delivery: DamageDelivery::Normal,
			},
			pattern: AttackPattern::Single,
			extra_effects: Vec::new(),
		};
		let block_demand: u16 = enemy_block_demand(enemy_key, stats.block_count())?;
		let mut reached_mechanics: Vec<ReachedMechanic> =
			unestablished_ranged_reach(enemy_key, enemy.apply_way_name(), stats.range_radius())
				.into_iter()
				.collect();
		compile_enemy_behavior(
			EnemyRecordFacts {
				enemy_key,
				target_class: *target_class,
			},
			|| review_undescribed_enemy(enemy).map_err(|described| described.reason().to_owned()),
			|undescribed: &UndescribedEnemy| {
				// The table gate has already required the record to own its prefab key.
				let facts: PlainEnemyTableFacts = PlainEnemyTableFacts {
					prefab_key: enemy_key.clone(),
					apply_way: enemy.apply_way_name().map(str::to_owned),
					range_radius: stats.range_radius(),
					block_demand,
					damage: undescribed.normal_attack_damage_type,
				};
				decode_plain_enemy_prefab(prefabs, &registry, &facts)
			},
			EnemyCompilation {
				attack: &mut attack,
				reached_mechanics: &mut reached_mechanics,
			},
		)?;
		compiled.push(EnemyFacts {
			enemy_key: enemy_key.clone(),
			block_demand,
			mass: i16::try_from(required_enemy_i32(
				enemy_key,
				"mass_level",
				stats.mass_level(),
			)?)
			.map_err(|_| ExpectedReplayAdapterError::InvalidEnemyStat {
				enemy_key: enemy_key.clone(),
				field: "mass_level",
			})?,
			combat: ActorCombatFacts {
				max_hp: expected_i32(
					enemy_key,
					"max_hp",
					required_enemy_i32(enemy_key, "max_hp", stats.max_hp())?,
				)?,
				attack: expected_i32(
					enemy_key,
					"attack",
					required_enemy_i32(enemy_key, "attack", stats.attack())?,
				)?,
				defense: expected_i32(
					enemy_key,
					"defense",
					required_enemy_i32(enemy_key, "defense", stats.defense())?,
				)?,
				target_class: *target_class,
				arts_resistance: expected_ratio_percent(
					enemy_key,
					required_enemy_f64(enemy_key, "magic_resistance", stats.magic_resistance())?,
				)?,
				minimum_damage_ratio: ActorCombatFacts::REVIEWED_MINIMUM_DAMAGE_RATIO,
			},
			attack,
			on_spawn: Vec::new(),
			on_death: Vec::new(),
			reached_mechanics,
		});
	}
	compiled.sort_by(|left, right| left.enemy_key.cmp(&right.enemy_key));
	Ok(compiled)
}

/// What one enemy record's own tables say, before any prefab or catalog is consulted.
#[derive(Clone, Copy)]
struct EnemyRecordFacts<'a> {
	enemy_key: &'a str,
	target_class: TargetClass,
}

struct EnemyCompilation<'a> {
	attack: &'a mut AttackProfile,
	reached_mechanics: &'a mut Vec<ReachedMechanic>,
}

/// Ticks from a plain ranged enemy's attack until its projectile lands.
///
/// A declared approximation: the decoded projectile's serialized speed is not executed, so every
/// plain projectile lands on the tick after it fires.
const PLAIN_ENEMY_PROJECTILE_FLIGHT_TICKS: u64 = 1;

/// Compiles one enemy record's behaviour from its pinned data alone.
///
/// Every record passes the table gate first; only then is its pinned battle prefab consulted. With
/// no enemy pack pinned the unread prefab stays an omission, while a pinned prefab either decodes as
/// a plain form or is Unsupported. A record whose tables describe behaviour the templates do not
/// decode is Unsupported: enemy semantics come from decoding the pinned data, never from a reviewed
/// conclusion written into this crate.
fn compile_enemy_behavior(
	record: EnemyRecordFacts<'_>,
	undescribed: impl FnOnce() -> Result<UndescribedEnemy, String>,
	plain: impl FnOnce(&UndescribedEnemy) -> Result<PlainEnemyPrefabDecode, PlainEnemyPrefabDecodeError>,
	output: EnemyCompilation<'_>,
) -> Result<(), ExpectedReplayAdapterError> {
	let EnemyRecordFacts {
		enemy_key,
		target_class,
	} = record;
	let undescribed: UndescribedEnemy = match undescribed() {
		Ok(undescribed) => undescribed,
		Err(described) => {
			output.reached_mechanics.push(unsupported_mechanic(
				format!("{enemy_key}/prefab"),
				format!("gamedata:enemy:{enemy_key}"),
				described,
			));
			return Ok(());
		}
	};
	match plain(&undescribed) {
		Ok(PlainEnemyPrefabDecode::NoEnemyPacks) => compile_undescribed_enemy(
			enemy_key,
			undescribed,
			output.attack,
			output.reached_mechanics,
		),
		Ok(PlainEnemyPrefabDecode::Decoded(decoded)) => {
			compile_plain_enemy(enemy_key, target_class, &decoded, output);
		}
		Err(error) => output.reached_mechanics.push(unsupported_mechanic(
			format!("{enemy_key}/prefab"),
			format!("verified-prefab:enemy:{enemy_key}"),
			error.to_string(),
		)),
	}
	Ok(())
}

/// Compiles one record whose pinned battle prefab decoded as a plain form.
///
/// The base [`AttackProfile`] already carries the record's own cadence and range. The decoded form
/// adds the damage family and, for a ranged record, a projectile that lands one tick after it fires.
fn compile_plain_enemy(
	enemy_key: &str,
	target_class: TargetClass,
	decoded: &DecodedPlainEnemyPrefab,
	output: EnemyCompilation<'_>,
) {
	let mechanic_id: String = format!("{enemy_key}/prefab");
	let origin: String = plain_enemy_origin(enemy_key, decoded);
	if target_class != TargetClass::Ground {
		output.reached_mechanics.push(unsupported_mechanic(
			mechanic_id,
			origin,
			"the plain enemy prefab template reviews only enemies on ground routes",
		));
		return;
	}
	output.attack.action = NormalAttackAction::Damage {
		damage_type: replay_enemy_damage_type(decoded.damage),
		delivery: DamageDelivery::Normal,
	};
	if let PlainEnemyAttackForm::RangedProjectile { speed, .. } = decoded.form {
		output.attack.pattern = AttackPattern::Projectile {
			flight_ticks: PLAIN_ENEMY_PROJECTILE_FLIGHT_TICKS,
		};
		output.reached_mechanics.push(approximated_mechanic(
			format!("{mechanic_id}/projectile-flight"),
			origin.clone(),
			format!(
				"the projectile's serialized speed {speed} is decoded but not executed; every hit \
				 lands {PLAIN_ENEMY_PROJECTILE_FLIGHT_TICKS} tick after it fires, and nothing \
				 converts that speed into the flight it describes, so whether replay voids fewer \
				 or more hits than the pinned data would is unestablished"
			),
			ApproximationTrigger::EnemyNormalAttack {
				enemy_key: enemy_key.to_owned(),
			},
		));
	}
	output.reached_mechanics.push(supported_mechanic(
		mechanic_id,
		origin,
		MechanicBehavior::Damage,
	));
}

/// Exact location of the decoded enemy prefab, and of its projectile for a ranged record.
fn plain_enemy_origin(enemy_key: &str, decoded: &DecodedPlainEnemyPrefab) -> String {
	let enemy: String = format!(
		"verified-prefab:enemy:{enemy_key}/bundle:{}/cab:{}/path:{}",
		decoded.location.logical_path,
		decoded.location.root.cab_name,
		decoded.location.root.path_id
	);
	match &decoded.form {
		PlainEnemyAttackForm::Melee => enemy,
		PlainEnemyAttackForm::RangedProjectile {
			projectile_key,
			projectile,
			..
		} => format!(
			"{enemy}/projectile:{projectile_key}/cab:{}/path:{}",
			projectile.root.cab_name, projectile.root.path_id
		),
	}
}

fn replay_enemy_damage_type(damage: ReviewedEnemyDamageType) -> DamageType {
	match damage {
		ReviewedEnemyDamageType::Physical => DamageType::Physical,
		ReviewedEnemyDamageType::Arts => DamageType::Arts,
	}
}

/// Compiles one record whose tables describe nothing beyond its base panel, when no enemy pack is
/// pinned.
///
/// The caller has already shaped the base [`AttackProfile`] from the record's own stats, so the only
/// fact this adds is the damage family the handbook names. No prefab evidence exists to read: the
/// record is admitted because the pinned tables *describe* no behaviour, which is weaker than
/// proving it has none. The unread prefab is therefore recorded as an omission, so the base panel
/// still executes while nothing this replay produces can be certified as a victory or a defeat.
fn compile_undescribed_enemy(
	enemy_key: &str,
	undescribed: UndescribedEnemy,
	attack: &mut AttackProfile,
	reached_mechanics: &mut Vec<ReachedMechanic>,
) {
	attack.action = NormalAttackAction::Damage {
		damage_type: replay_enemy_damage_type(undescribed.normal_attack_damage_type),
		delivery: DamageDelivery::Normal,
	};
	reached_mechanics.push(unmodeled_mechanic(
		format!("{enemy_key}/prefab"),
		format!("undescribed-gamedata:enemy:{enemy_key}"),
		"the record's battle prefab was not read; only its base panel and the handbook damage \
		 family are modelled",
	));
}

fn enemy_block_demand(
	enemy_key: &str,
	block_count: Option<i32>,
) -> Result<u16, ExpectedReplayAdapterError> {
	u16::try_from(block_count.unwrap_or(1)).map_err(|_| {
		ExpectedReplayAdapterError::InvalidEnemyStat {
			enemy_key: enemy_key.to_owned(),
			field: "block_count",
		}
	})
}

fn compile_preplaced(
	snapshot: &ArkDataSnapshot,
	stage: &CompiledStage,
	tick_rate: TickRate,
	catalog: &mut SummonTemplateCatalog,
) -> Result<Vec<PreplacedActorFacts>, ExpectedReplayAdapterError> {
	let actors: Vec<&PredeployedStageActor> = stage
		.predeployed_actors()
		.iter()
		.filter(|actor| !actor.hard())
		.collect();
	for actor in &actors {
		let key: &str = actor.character_key();
		if catalog.id_by_key(key).is_some() {
			continue;
		}
		let resolved = snapshot
			.operators()
			.resolve_base_panel(&BasePanelBuild::new(
				OperatorIdentity::Id(key.to_owned()),
				actor.elite(),
				actor.level(),
			))
			.map_err(ExpectedReplayAdapterError::ArkData)?;
		let stats = resolved.base_stats();
		let known_shelter: bool = key == "trap_183_sheltk";
		let reviewed_shelter: bool = known_shelter
			&& stats.max_hp() == 5_000.0
			&& stats.attack() == 0.0
			&& stats.defense() == 200.0
			&& stats.magic_resistance() == 0.0
			&& stats.block_count() == 3;
		let block_count: u16 = u16::try_from(stats.block_count()).map_err(|_| {
			ExpectedReplayAdapterError::InvalidOperatorStat {
				operator: key.to_owned(),
				field: "block_count",
			}
		})?;
		let combat: ActorCombatFacts = actor_combat_facts(key, stats, TargetClass::Ground)?;
		catalog
			.get_or_insert_with(key, |template| SummonTemplate {
				id: template,
				key: key.to_owned(),
				faction: Faction::Friendly,
				cell_occupancy: SummonCellOccupancy::Exclusive,
				block_count,
				block_target_filter: crate::mechanics::domain::BlockTargetFilter::GroundOnly,
				targetability: if reviewed_shelter {
					Targetability::Shelter
				} else {
					Targetability::None
				},
				combat,
				attack: inert_attack(tick_rate),
				lifetime_ticks: None,
				reached_mechanics: vec![if reviewed_shelter {
					supported_mechanic(
						format!("{key}/preplaced_behavior"),
						format!("preplaced:{key}"),
						MechanicBehavior::ShelterExposureOrDirectHit,
					)
				} else {
					unsupported_mechanic(
						format!("{key}/preplaced_behavior"),
						format!("preplaced:{key}"),
						if known_shelter {
							"shelter base-panel shape drifted from the reviewed BD-7 device"
						} else {
							"preplaced actor behavior is not registered"
						},
					)
				}],
			})
			.map_err(ExpectedReplayAdapterError::SummonTemplateCatalog)?;
	}
	let preplaced: Vec<PreplacedActorFacts> = actors
		.into_iter()
		.enumerate()
		.map(|(index, actor)| {
			Ok(PreplacedActorFacts {
				instance: u32::try_from(index)
					.map_err(|_| ExpectedReplayAdapterError::IdentifierOverflow)?,
				template: catalog.id_by_key(actor.character_key()).ok_or_else(|| {
					ExpectedReplayAdapterError::UnsupportedPreplacedActor {
						key: actor.character_key().to_owned(),
						detail: "template disappeared".to_owned(),
					}
				})?,
				cell: actor.position(),
				facing: map_facing(actor.direction())?,
			})
		})
		.collect::<Result<_, ExpectedReplayAdapterError>>()?;
	Ok(preplaced)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReviewedRosterFacts {
	distinct_profession_count: usize,
	maximum_same_profession_count: usize,
	group_counts: BTreeMap<String, usize>,
}

fn reviewed_roster_facts(
	snapshot: &ArkDataSnapshot,
	bindings: &[ExactReplayBuildBinding],
) -> Result<ReviewedRosterFacts, ExpectedReplayAdapterError> {
	let mut operator_ids: BTreeSet<String> = BTreeSet::new();
	let mut profession_counts: BTreeMap<String, usize> = BTreeMap::new();
	let mut group_counts: BTreeMap<String, usize> = BTreeMap::new();
	for binding in bindings {
		let resolved: ResolvedOperatorBuild = snapshot
			.operators()
			.resolve(&binding.build)
			.map_err(ExpectedReplayAdapterError::ArkData)?;
		if !operator_ids.insert(resolved.operator_id().to_owned()) {
			return Err(ExpectedReplayAdapterError::DuplicateOperatorIdentity {
				operator_id: resolved.operator_id().to_owned(),
			});
		}
		let count: &mut usize = profession_counts
			.entry(resolved.profession().to_owned())
			.or_default();
		*count += 1;
		if let Some(group_id) = resolved.group_id() {
			*group_counts.entry(group_id.to_owned()).or_default() += 1;
		}
	}
	Ok(ReviewedRosterFacts {
		distinct_profession_count: profession_counts.len(),
		maximum_same_profession_count: profession_counts.values().copied().max().unwrap_or(0),
		group_counts,
	})
}

fn roster_condition_is_proven_false(
	condition: &ShuRosterCondition,
	roster: &ReviewedRosterFacts,
) -> bool {
	match condition {
		ShuRosterCondition::DeployedDistinctProfessions { minimum_count, .. } => {
			roster.distinct_profession_count < usize::from(*minimum_count)
		}
		ShuRosterCondition::DeployedSameProfession { minimum_count, .. } => {
			roster.maximum_same_profession_count < usize::from(*minimum_count)
		}
		ShuRosterCondition::FixedRosterSerializedTag {
			minimum_count,
			filter_tag,
			..
		} => roster.group_counts.get(filter_tag).copied().unwrap_or(0) < usize::from(*minimum_count),
	}
}

fn lower_shu_roster_template(
	decoded: &DecodedShuRosterTemplate,
	blackboard: &[BlackboardEntry],
	tick_rate: TickRate,
	actor_templates: &BTreeMap<String, SummonTemplateId>,
	roster: &ReviewedRosterFacts,
) -> Result<LoweredMechanicSemanticProgram, MechanicSemanticError> {
	let mut lowered: LoweredMechanicSemanticProgram = decoded
		.lower_program()
		.lower_with_actor_templates(blackboard, tick_rate, actor_templates)?;
	let branches: [&ShuRosterConditionalBranch; 3] = [
		&decoded.semantics.distinct_professions,
		&decoded.semantics.same_profession,
		&decoded.semantics.roster_tag,
	];
	let inactive: BTreeSet<MechanicId> = branches
		.into_iter()
		.filter(|branch| roster_condition_is_proven_false(&branch.condition, roster))
		.map(|branch| branch.mechanic_id.clone())
		.collect();
	lowered
		.on_battle_start
		.retain(|effect| !inactive.contains(&effect.mechanic));
	lowered
		.on_deploy
		.retain(|effect| !inactive.contains(&effect.mechanic));
	for mechanic in &mut lowered.reached_mechanics {
		if inactive.contains(&mechanic.id) {
			mechanic.behavior = MechanicBehavior::ProvenInactive;
		}
	}
	Ok(lowered)
}

fn compile_operator_normal_attack(
	resolved: &ResolvedOperatorBuild,
	range: Vec<crate::data::RangeOffset>,
	tick_rate: TickRate,
	reviewed_no_attack_override: bool,
) -> Result<(AttackProfile, Option<ReachedMechanic>), ExpectedReplayAdapterError> {
	let stats = resolved.static_stats();
	let facts = BasePanelNormalAttackFacts::new(
		resolved.operator_id().to_owned(),
		resolved.profession().to_owned(),
		resolved.sub_profession_id().to_owned(),
		stats.attack(),
		stats.base_attack_time_seconds(),
		stats.attack_speed(),
	);
	match compile_base_panel_normal_attack_facts(&facts, tick_rate) {
		NormalAttackCatalogEntry::Supported(entry) => {
			let profile = entry.profile();
			let [packet] = profile.packets() else {
				return Err(ExpectedReplayAdapterError::InvalidOperatorStat {
					operator: resolved.operator_id().to_owned(),
					field: "normal_attack_packets",
				});
			};
			let damage_type: DamageType = match packet.damage_type() {
				NormalAttackDamageType::Physical => DamageType::Physical,
				NormalAttackDamageType::Arts => DamageType::Arts,
				NormalAttackDamageType::True => DamageType::True,
			};
			let target_count: u16 = u16::try_from(profile.target_count().get()).map_err(|_| {
				ExpectedReplayAdapterError::InvalidOperatorStat {
					operator: resolved.operator_id().to_owned(),
					field: "normal_attack_target_count",
				}
			})?;
			Ok((
				AttackProfile {
					first_delay_ticks: profile.first_attack_delay_ticks(),
					interval_ticks: profile.attack_interval_ticks().get(),
					range,
					target_count,
					target_filter: profile.target_filter(),
					action: NormalAttackAction::Damage {
						damage_type,
						delivery: DamageDelivery::Normal,
					},
					pattern: AttackPattern::Single,
					extra_effects: Vec::new(),
				},
				None,
			))
		}
		NormalAttackCatalogEntry::Unsupported(unsupported) => {
			let interval_ticks: u64 = attack_interval_ticks(
				stats.base_attack_time_seconds(),
				stats.attack_speed(),
				tick_rate,
			)?;
			let target_filter: NormalAttackTargetFilter = match resolved.position() {
				OperatorPosition::Ranged | OperatorPosition::All => {
					NormalAttackTargetFilter::GroundAndAir
				}
				OperatorPosition::Melee | OperatorPosition::None | OperatorPosition::Unknown(_) => {
					NormalAttackTargetFilter::GroundOnly
				}
			};
			let omission: Option<ReachedMechanic> = (!reviewed_no_attack_override).then(|| {
				unmodeled_mechanic(
					format!(
						"{}/normal_attack/{}",
						resolved.operator_id(),
						resolved.sub_profession_id()
					),
					format!("operator:{}", resolved.operator_id()),
					format!(
						"normal attack is omitted from this partial replay: {}",
						unsupported.reason()
					),
				)
			});
			Ok((
				AttackProfile {
					first_delay_ticks: interval_ticks,
					interval_ticks,
					range,
					target_count: 0,
					target_filter,
					action: NormalAttackAction::None,
					pattern: AttackPattern::Single,
					extra_effects: Vec::new(),
				},
				omission,
			))
		}
	}
}

fn compile_operator_builds(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	stage: &CompiledStage,
	bindings: &[ExactReplayBuildBinding],
	prepared: &PreparedReplayGuide,
	tick_rate: TickRate,
	summon_catalog: &mut SummonTemplateCatalog,
) -> Result<(Vec<OperatorBuildFacts>, Vec<ResolvedOperatorBuild>), ExpectedReplayAdapterError> {
	let roster_facts: ReviewedRosterFacts = reviewed_roster_facts(snapshot, bindings)?;
	let prefab_handlers: BattlePrefabHandlerRegistry = built_in_battle_prefab_handler_registry();
	let mut deploy_cells: BTreeMap<OperatorInstanceId, Vec<GridPosition>> = BTreeMap::new();
	for action in &prepared.actions {
		if let crate::mechanics::domain::MaterializedGuideAction::Deploy {
			operator, cell, ..
		} = action
		{
			deploy_cells.entry(*operator).or_default().push(*cell);
		}
	}
	let mut facts: Vec<OperatorBuildFacts> = Vec::with_capacity(bindings.len());
	let mut resolved_builds: Vec<ResolvedOperatorBuild> = Vec::with_capacity(bindings.len());
	let mut ordered_bindings: Vec<(&ExactReplayBuildBinding, OperatorInstanceId)> = bindings
		.iter()
		.map(|binding| {
			prepared
				.operator_ids
				.get(&binding.source_name)
				.copied()
				.map(|instance| (binding, instance))
				.ok_or_else(|| ExpectedReplayAdapterError::UnknownBuildBinding {
					name: binding.source_name.clone(),
				})
		})
		.collect::<Result<_, ExpectedReplayAdapterError>>()?;
	ordered_bindings.sort_by_key(|(_, instance)| *instance);
	for (binding, instance) in ordered_bindings {
		let resolved: ResolvedOperatorBuild = snapshot
			.operators()
			.resolve(&binding.build)
			.map_err(ExpectedReplayAdapterError::ArkData)?;
		let cells: &[GridPosition] = deploy_cells.get(&instance).ok_or_else(|| {
			ExpectedReplayAdapterError::MissingDeployment {
				name: binding.source_name.clone(),
			}
		})?;
		for cell in cells {
			validate_deployment_cell(stage, &resolved, *cell)?;
		}
		let stats = resolved.static_stats();
		let redeployment_cooldown_ticks: u64 =
			ticks_from_seconds_ceil(f64::from(stats.respawn_time_seconds()), tick_rate)
				.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
		let range = snapshot
			.resolve_range(resolved.range_id())
			.map_err(ExpectedReplayAdapterError::ArkData)?
			.offsets()
			.to_vec();
		let combat: ActorCombatFacts =
			actor_combat_facts(resolved.operator_id(), stats, TargetClass::Ground)?;
		let phalanx_decode = try_decode_phalanx_trait(&resolved);
		let phalanx_trait_decoded: bool = matches!(&phalanx_decode, Ok(Some(_)));
		let phalanx_trait_accounted: bool = !matches!(&phalanx_decode, Ok(None));
		let mechanic_override: OperatorMechanicOverrideEntry =
			review_operator_mechanic_override(&resolved);
		let manual_override_active: bool = matches!(
			&mechanic_override,
			OperatorMechanicOverrideEntry::Matched(_)
		);
		let (mut attack, base_attack_omission): (AttackProfile, Option<ReachedMechanic>) =
			compile_operator_normal_attack(&resolved, range, tick_rate, phalanx_trait_decoded)?;
		let mut skill: SkillFacts = compile_skill(&resolved, tick_rate)?;
		let selected_skill_reached: bool = selected_skill_behavior_is_reached(
			prepared,
			instance,
			resolved.selected_skill().activation_type(),
		);
		let compiled_skill: ExpectedDamageSkillEntry =
			compile_expected_damage_skill(&resolved, tick_rate);
		let active_range: Option<Vec<crate::data::RangeOffset>> = compiled_skill
			.as_supported()
			.map(|profile| {
				snapshot
					.resolve_range(profile.range().range_id())
					.map(|range| range.offsets().to_vec())
			})
			.transpose()
			.map_err(ExpectedReplayAdapterError::ArkData)?;
		let mut reached_mechanics: Vec<ReachedMechanic> =
			base_attack_omission.into_iter().collect();
		let mut on_battle_start: Vec<TaggedEffect> = Vec::new();
		let mut on_deploy: Vec<TaggedEffect> = Vec::new();
		match phalanx_decode {
			Ok(Some(decoded)) => match decoded.lower_program().lower_with_actor_templates(
				&[],
				tick_rate,
				summon_catalog.bindings(),
			) {
				Ok(lowered) => {
					on_battle_start.extend(lowered.on_battle_start);
					on_deploy.extend(lowered.on_deploy);
					reached_mechanics.extend(lowered.reached_mechanics);
				}
				Err(error) => reached_mechanics.push(unsupported_mechanic(
					format!("{}/trait/phalanx/lowering", resolved.operator_id()),
					decoded.provenance.origin(),
					format!("verified phalanx semantics could not lower: {error}"),
				)),
			},
			Ok(None) => {}
			Err(error) => reached_mechanics.push(unsupported_mechanic(
				format!("{}/trait/phalanx/decoding", resolved.operator_id()),
				format!("verified-gamedata:operator:{}", resolved.operator_id()),
				format!("phalanx trait shape drifted: {error}"),
			)),
		}
		let active_candidates: Vec<&OperatorEffectCandidate> =
			active_operator_candidates(&resolved);
		let OperatorPrefabCompileResult {
			reached_mechanics: prefab_mechanics,
			selected_skill_template_claimed,
			selected_skill_mode_root_accounted: _,
			mut accounted_effect_candidates,
			unclaimed_effect_roots,
		}: OperatorPrefabCompileResult =
			compile_operator_prefab_inventory(OperatorPrefabCompileContext {
				snapshot,
				prefabs,
				resolved: &resolved,
				active_candidates: &active_candidates,
				roster_facts: &roster_facts,
				selected_skill_reached,
				tick_rate,
				registry: &prefab_handlers,
				summon_catalog,
				attack: &mut attack,
				on_battle_start: &mut on_battle_start,
				on_deploy: &mut on_deploy,
				skill: &mut skill,
				allow_effect_lowering: !manual_override_active,
			});
		if phalanx_trait_accounted {
			if let Some(selected_trait) = resolved.selected_trait_candidate() {
				if let Some(active_index) = active_candidates
					.iter()
					.position(|candidate| std::ptr::eq(*candidate, selected_trait))
				{
					accounted_effect_candidates.insert(active_index);
				}
			}
		}
		reached_mechanics.extend(prefab_mechanics);
		let override_matched: bool = match mechanic_override {
			OperatorMechanicOverrideEntry::Matched(reviewed) => {
				compile_operator_mechanic_override(
					resolved.operator_id(),
					reviewed,
					OperatorMechanicOverrideCompileContext {
						on_deploy: &mut on_deploy,
						reached_mechanics: &mut reached_mechanics,
					},
				);
				true
			}
			OperatorMechanicOverrideEntry::Drift(drift) => {
				reached_mechanics.push(unsupported_mechanic(
					format!("{}/mechanic_binding", resolved.operator_id()),
					format!("operator:{}", resolved.operator_id()),
					drift.detail(),
				));
				false
			}
			OperatorMechanicOverrideEntry::Absent => false,
		};
		if !selected_skill_template_claimed {
			reached_mechanics.extend(register_selected_skill_behavior(
				resolved.operator_id(),
				resolved.skill_id(),
				&compiled_skill,
				SelectedSkillBehaviorContext {
					skill: &mut skill,
					attack: &attack,
					base_attack: combat.attack,
					active_range: active_range.as_deref(),
					behavior_reached: selected_skill_reached,
				},
			));
		}
		if !override_matched {
			for (index, candidate) in active_candidates.into_iter().enumerate() {
				if accounted_effect_candidates.contains(&index) {
					continue;
				}
				let mechanic_id: String = format!(
					"{}/effect/{index}/{}",
					resolved.operator_id(),
					candidate.prefab_key().unwrap_or("no-prefab")
				);
				let origin: String = format!("operator:{}", resolved.operator_id());
				reached_mechanics.push(unmodeled_mechanic(
					mechanic_id,
					origin,
					unmodeled_operator_effect_reason(
						candidate,
						unclaimed_effect_roots.contains(&index),
					),
				));
			}
		}
		for custom in resolved.custom_potential_effects() {
			if !resolved.custom_potential_effect_is_absorbed(custom) {
				reached_mechanics.push(unsupported_mechanic(
					format!(
						"{}/potential/p{}",
						resolved.operator_id(),
						custom.rank().number()
					),
					format!("operator:{}", resolved.operator_id()),
					format!(
						"CUSTOM potential metadata has no matching potential-gated trait/talent \
						 candidate: {}",
						custom.description()
					),
				));
			}
		}
		if let Some(module) = resolved.selected_module() {
			for (part_index, part) in module.parts().iter().enumerate() {
				if part.target() != &OperatorModulePartTarget::Display {
					reached_mechanics.push(unsupported_mechanic(
						format!(
							"{}/module/{}/part/{part_index}/{:?}",
							resolved.operator_id(),
							module.module_id(),
							part.target()
						),
						format!(
							"operator:{}/module:{}",
							resolved.operator_id(),
							module.module_id()
						),
						"dynamic module part behavior is omitted from this partial replay",
					));
				}
			}
		}
		facts.push(OperatorBuildFacts {
			id: instance,
			operator_key: resolved.operator_id().to_owned(),
			classification: OperatorClassificationFacts {
				profession_category_mask: resolved.profession_category_mask().ok_or_else(|| {
					ExpectedReplayAdapterError::InvalidOperatorStat {
						operator: resolved.operator_id().to_owned(),
						field: "profession",
					}
				})?,
				group_id: resolved.group_id().map(str::to_owned),
			},
			deployment_cost: resolved.deployment_cost(),
			redeployment_cooldown_ticks,
			block_count: u16::try_from(stats.block_count()).map_err(|_| {
				ExpectedReplayAdapterError::InvalidOperatorStat {
					operator: resolved.operator_id().to_owned(),
					field: "block_count",
				}
			})?,
			block_target_filter: crate::mechanics::domain::BlockTargetFilter::GroundOnly,
			combat,
			attack,
			skill: Some(skill),
			on_battle_start,
			on_deploy,
			on_death: Vec::new(),
			reached_mechanics,
		});
		resolved_builds.push(resolved);
	}
	facts.sort_by_key(|build| build.id);
	resolved_builds.sort_by(|left, right| left.operator_id().cmp(right.operator_id()));
	Ok((facts, resolved_builds))
}

struct OperatorMechanicOverrideCompileContext<'a> {
	on_deploy: &'a mut Vec<TaggedEffect>,
	reached_mechanics: &'a mut Vec<ReachedMechanic>,
}

fn compile_operator_mechanic_override(
	operator_id: &str,
	reviewed: OperatorMechanicOverride,
	context: OperatorMechanicOverrideCompileContext<'_>,
) {
	let OperatorMechanicOverrideCompileContext {
		on_deploy,
		reached_mechanics,
	} = context;
	let origin: String = format!("operator:{operator_id}");
	match reviewed {
		OperatorMechanicOverride::SelfPermanentTalents(review) => {
			for talent in review.talents {
				let mechanic: MechanicId = MechanicId::new(format!(
					"{operator_id}/effect/{}/{}",
					talent.effect_index, talent.prefab_key
				));
				// Every reviewed permanent talent here is worded as a panel percentage — 蛇屠箱's
				// 防御力+12%, 能天使's 攻击力+8% — so they share their stat's bucket.
				let effect: ExpectedEffect = match talent.modifier {
					PermanentModifier::AttackSpeedBonus(delta) => {
						ExpectedEffect::AttackSpeed(AttackSpeedModifier {
							delta: i16::try_from(delta)
								.expect("reviewed permanent ASPD delta fits i16"),
							duration_ticks: None,
						})
					}
					PermanentModifier::AttackMultiplier(ratio) => {
						ExpectedEffect::StatModifier(StatModifier {
							bucket: StatModifierBucket::Panel,
							stat: StatKind::Attack,
							multiplier: expected_ratio(ratio),
							duration_ticks: None,
						})
					}
					PermanentModifier::DefenseMultiplier(ratio) => {
						ExpectedEffect::StatModifier(StatModifier {
							bucket: StatModifierBucket::Panel,
							stat: StatKind::Defense,
							multiplier: expected_ratio(ratio),
							duration_ticks: None,
						})
					}
					PermanentModifier::MaxHpMultiplier(ratio) => {
						ExpectedEffect::StatModifier(StatModifier {
							bucket: StatModifierBucket::Panel,
							stat: StatKind::MaxHp,
							multiplier: expected_ratio(ratio),
							duration_ticks: None,
						})
					}
				};
				let behavior: MechanicBehavior = match talent.modifier {
					PermanentModifier::AttackSpeedBonus(_) => MechanicBehavior::AttackCadence,
					PermanentModifier::AttackMultiplier(_)
					| PermanentModifier::DefenseMultiplier(_)
					| PermanentModifier::MaxHpMultiplier(_) => MechanicBehavior::StatModifier,
				};
				on_deploy.push(TaggedEffect {
					mechanic: mechanic.clone(),
					target: EffectTarget::SelfActor,
					effect,
				});
				reached_mechanics.push(supported_mechanic(
					mechanic.as_str().to_owned(),
					origin.clone(),
					behavior,
				));
			}
		}
		OperatorMechanicOverride::Exusiai(review) => {
			let first_mechanic: MechanicId = MechanicId::new(format!(
				"{operator_id}/effect/0/{}",
				review.first_talent_prefab_key
			));
			let PermanentModifier::AttackSpeedBonus(attack_speed_delta) =
				review.first_talent_modifier
			else {
				unreachable!("reviewed Exusiai cadence shape is fixed");
			};
			on_deploy.push(TaggedEffect {
				mechanic: first_mechanic.clone(),
				target: EffectTarget::SelfActor,
				effect: ExpectedEffect::AttackSpeed(AttackSpeedModifier {
					delta: i16::try_from(attack_speed_delta)
						.expect("reviewed Exusiai ASPD delta fits i16"),
					duration_ticks: None,
				}),
			});
			reached_mechanics.push(supported_mechanic(
				first_mechanic.as_str().to_owned(),
				origin.clone(),
				MechanicBehavior::AttackCadence,
			));

			let second_mechanic: MechanicId = MechanicId::new(format!(
				"{operator_id}/effect/1/{}",
				review.second_talent_prefab_key
			));
			for modifier in review.second_talent_modifiers {
				let (stat, multiplier): (StatKind, ExpectedValue) = match modifier {
					PermanentModifier::AttackMultiplier(ratio) => {
						(StatKind::Attack, expected_ratio(ratio))
					}
					PermanentModifier::MaxHpMultiplier(ratio) => {
						(StatKind::MaxHp, expected_ratio(ratio))
					}
					PermanentModifier::DefenseMultiplier(ratio) => {
						(StatKind::Defense, expected_ratio(ratio))
					}
					PermanentModifier::AttackSpeedBonus(_) => {
						unreachable!("reviewed Exusiai second talent has no cadence modifier");
					}
				};
				on_deploy.push(TaggedEffect {
					mechanic: second_mechanic.clone(),
					target: EffectTarget::SelfActor,
					effect: ExpectedEffect::StatModifier(StatModifier {
						bucket: StatModifierBucket::Panel,
						stat,
						multiplier,
						duration_ticks: None,
					}),
				});
			}
			reached_mechanics.push(supported_mechanic(
				second_mechanic.as_str().to_owned(),
				origin.clone(),
				MechanicBehavior::StatModifier,
			));
			reached_mechanics.push(supported_mechanic(
				format!("{operator_id}/effect/1/random_ally"),
				origin.clone(),
				MechanicBehavior::ConservativeBeneficialOmission {
					reason: "random ally receives only a beneficial ATK/MaxHP buff".to_owned(),
				},
			));
		}
	}
}

fn expected_ratio(ratio: ExactRatio) -> ExpectedValue {
	ExpectedValue::from_ratio(u64::from(ratio.numerator), u64::from(ratio.denominator))
		.expect("reviewed BD-7 ratio is valid")
}

fn compile_skill(
	resolved: &ResolvedOperatorBuild,
	tick_rate: TickRate,
) -> Result<SkillFacts, ExpectedReplayAdapterError> {
	let skill = resolved.selected_skill();
	let activation: SkillActivationRule = match skill.activation_type() {
		SkillActivationType::Auto => SkillActivationRule::Auto,
		SkillActivationType::Manual => SkillActivationRule::Manual,
		SkillActivationType::Passive => SkillActivationRule::Passive,
		SkillActivationType::Unknown(value) => {
			return Err(ExpectedReplayAdapterError::UnsupportedSkillMetadata {
				operator: resolved.operator_id().to_owned(),
				detail: format!("unknown activation type {value:?}"),
			});
		}
	};
	let sp = skill.sp();
	let amount: u32 = exact_nonnegative_u32(sp.increment()).ok_or_else(|| {
		ExpectedReplayAdapterError::UnsupportedSkillMetadata {
			operator: resolved.operator_id().to_owned(),
			detail: format!("non-integral SP increment {}", sp.increment()),
		}
	})?;
	let charge: SpChargeRule = match sp.sp_type() {
		SkillSpType::IncreaseWithTime => SpChargeRule::Time {
			amount,
			interval_ticks: ticks_from_seconds_ceil(1.0, tick_rate)
				.map_err(ExpectedReplayAdapterError::StageSpaceTime)?,
		},
		SkillSpType::IncreaseWhenAttack => SpChargeRule::Attack { amount },
		SkillSpType::IncreaseWhenTakenDamage => SpChargeRule::Hit { amount },
		SkillSpType::UnknownName(value) => {
			return Err(ExpectedReplayAdapterError::UnsupportedSkillMetadata {
				operator: resolved.operator_id().to_owned(),
				detail: format!("unknown SP type {value:?}"),
			});
		}
		SkillSpType::UnknownCode(value) => {
			return Err(ExpectedReplayAdapterError::UnsupportedSkillMetadata {
				operator: resolved.operator_id().to_owned(),
				detail: format!("unknown SP type code {value}"),
			});
		}
	};
	Ok(SkillFacts {
		id: MechanicId::new(format!(
			"{}/skill/{}",
			resolved.operator_id(),
			skill.skill_id()
		)),
		activation,
		initial_sp: sp.initial(),
		sp_cost: sp.cost(),
		charge,
		periodic_sp_sources: Vec::new(),
		active_duration_ticks: Some(0),
		manual_discard: None,
		stored_attack_replacement: None,
		effects: Vec::new(),
	})
}

struct SelectedSkillBehaviorContext<'a> {
	skill: &'a mut SkillFacts,
	attack: &'a AttackProfile,
	base_attack: ExpectedValue,
	active_range: Option<&'a [crate::data::RangeOffset]>,
	behavior_reached: bool,
}

fn register_selected_skill_behavior(
	operator_id: &str,
	skill_id: &str,
	compiled: &ExpectedDamageSkillEntry,
	context: SelectedSkillBehaviorContext<'_>,
) -> Option<ReachedMechanic> {
	let SelectedSkillBehaviorContext {
		skill,
		attack,
		base_attack,
		active_range,
		behavior_reached,
	} = context;
	let result: Result<(Vec<TaggedEffect>, Option<u64>), String> = match compiled {
		ExpectedDamageSkillEntry::Supported(profile) => active_range
			.ok_or_else(|| "reviewed skill range was not resolved".to_owned())
			.and_then(|active_range| {
				compile_reviewed_modified_normal_effects(profile, attack, base_attack, active_range)
			})
			.and_then(|effects| {
				reviewed_active_duration_ticks(profile).map(|duration| (effects, duration))
			}),
		ExpectedDamageSkillEntry::Unsupported(unsupported) => Err(unsupported
			.reasons()
			.iter()
			.map(ToString::to_string)
			.collect::<Vec<_>>()
			.join("; ")),
	};
	match result {
		Ok((effects, active_duration_ticks)) => {
			skill.effects = effects;
			skill.active_duration_ticks = active_duration_ticks;
			None
		}
		Err(_) if !behavior_reached => None,
		Err(reason) => Some(unmodeled_mechanic(
			format!("{operator_id}/skill/{skill_id}"),
			format!("operator:{operator_id}"),
			format!("selected skill behavior is omitted from this partial replay: {reason}"),
		)),
	}
}

fn reviewed_active_duration_ticks(
	profile: &ExpectedDamageSkillProfile,
) -> Result<Option<u64>, String> {
	match profile.duration() {
		ExpectedSkillDuration::Timed { ticks } => Ok(Some(ticks.get())),
		ExpectedSkillDuration::Indefinite => Ok(None),
		ExpectedSkillDuration::Instant => {
			Err("instant skills cannot be represented by timed modifiers".to_owned())
		}
		ExpectedSkillDuration::Ammo { .. } => {
			Err("ammo skills require attack-counted modifier expiry".to_owned())
		}
	}
}

fn compile_reviewed_modified_normal_effects(
	profile: &ExpectedDamageSkillProfile,
	attack: &AttackProfile,
	base_attack: ExpectedValue,
	active_range: &[crate::data::RangeOffset],
) -> Result<Vec<TaggedEffect>, String> {
	if profile.template() != SkillDamageMechanicTemplate::ModifiedNormal {
		return Err(format!(
			"{:?} cannot be represented by normal-attack modifiers",
			profile.template()
		));
	}
	let duration_ticks: Option<u64> = match profile.duration() {
		ExpectedSkillDuration::Timed { ticks } => Some(ticks.get()),
		ExpectedSkillDuration::Indefinite => None,
		ExpectedSkillDuration::Instant => {
			return Err("instant skills cannot be represented by timed modifiers".to_owned());
		}
		ExpectedSkillDuration::Ammo { .. } => {
			return Err("ammo skills require attack-counted modifier expiry".to_owned());
		}
	};
	validate_base_attack_phase(profile.base(), attack, base_attack)?;
	validate_active_attack_phase(profile.active(), attack)?;

	// The active attack is built from two lanes that resolve in different multiplier buckets: the
	// panel percentage the skill grants sums with every other panel percentage on the operator,
	// while its damage scale stands alone. Emitting their product as one factor would let a panel
	// talent stack multiplicatively against the skill, so each lane becomes its own modifier and
	// the pair must reproduce the fused attack exactly before either is trusted.
	let active_raw_attack: ExpectedValue = deterministic_raw_attack(profile.active(), "active")?;
	let panel_multiplier: ExpectedValue =
		exact_lane_multiplier(1.0 + profile.additive_attack_ratio())
			.ok_or_else(|| "active panel percentage is not exactly representable".to_owned())?;
	let attack_scale: ExpectedValue = exact_lane_multiplier(profile.primary_attack_scale())
		.ok_or_else(|| "active attack scale is not exactly representable".to_owned())?;
	if base_attack
		.checked_mul(panel_multiplier)
		.and_then(|panel| panel.checked_mul(attack_scale))
		!= Some(active_raw_attack)
	{
		return Err(
			"active attack is not exactly representable by the replay stat multipliers".to_owned(),
		);
	}
	// Attack speed is additive: every source's points land in one sum and the cadence follows from
	// that sum, so lowering a skill's points as a cadence multiplier would compound them against
	// every other source instead of joining them. The base-attack-time change the skill makes is a
	// genuinely separate factor and stays a cadence multiplier.
	let attack_speed_delta: i16 = whole_attack_speed_delta(profile.attack_speed_bonus())
		.ok_or_else(|| "active attack-speed bonus is not whole replay points".to_owned())?;
	let cadence_multiplier: ExpectedValue = exact_interval_multiplier(
		attack.interval_ticks,
		profile.active_interval_before_attack_speed_ticks().get(),
	)
	.ok_or_else(|| {
		"active cadence is not exactly representable by the replay cadence multiplier".to_owned()
	})?;
	if replayed_attack_interval(
		attack.interval_ticks,
		cadence_multiplier,
		attack_speed_delta,
	) != Some(profile.active().attack_interval_ticks().get())
	{
		return Err(
			"active cadence is not exactly representable by the replay cadence lanes".to_owned(),
		);
	}
	let active_target_count: u16 = u16::try_from(profile.active().maximum_primary_targets().get())
		.map_err(|_| "active target count exceeds replay capacity".to_owned())?;
	let target_count_delta: i16 =
		i16::try_from(i32::from(active_target_count) - i32::from(attack.target_count))
			.map_err(|_| "active target-count delta exceeds replay capacity".to_owned())?;
	let hit_count_delta: i16 =
		i16::try_from(i64::from(profile.active().primary_hit_count().get()) - 1_i64)
			.map_err(|_| "active hit-count delta exceeds replay capacity".to_owned())?;
	let additional_range: Vec<crate::data::RangeOffset> =
		compile_additional_range(profile, &attack.range, active_range)?;

	let mechanic: MechanicId = MechanicId::new(format!(
		"{}/skill/{}",
		profile.operator_id(),
		profile.skill_id()
	));
	let mut effects: Vec<TaggedEffect> = Vec::new();
	for (bucket, multiplier) in [
		(StatModifierBucket::Panel, panel_multiplier),
		(StatModifierBucket::Scale, attack_scale),
	] {
		if multiplier != ExpectedValue::ONE {
			effects.push(TaggedEffect {
				mechanic: mechanic.clone(),
				target: EffectTarget::SelfActor,
				effect: ExpectedEffect::StatModifier(StatModifier {
					bucket,
					stat: StatKind::Attack,
					multiplier,
					duration_ticks,
				}),
			});
		}
	}
	if cadence_multiplier != ExpectedValue::ONE {
		effects.push(TaggedEffect {
			mechanic: mechanic.clone(),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::AttackCadence(AttackCadenceModifier {
				interval_multiplier: cadence_multiplier,
				duration_ticks,
			}),
		});
	}
	if attack_speed_delta != 0 {
		effects.push(TaggedEffect {
			mechanic: mechanic.clone(),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::AttackSpeed(AttackSpeedModifier {
				delta: attack_speed_delta,
				duration_ticks,
			}),
		});
	}
	if !additional_range.is_empty() {
		effects.push(TaggedEffect {
			mechanic: mechanic.clone(),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::AttackRange(AttackRangeModifier {
				additional_offsets: additional_range,
				duration_ticks,
			}),
		});
	}
	if target_count_delta != 0 {
		effects.push(TaggedEffect {
			mechanic: mechanic.clone(),
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::TargetCount(TargetCountModifier::Delta {
				delta: target_count_delta,
				duration_ticks,
			}),
		});
	}
	if hit_count_delta != 0 {
		effects.push(TaggedEffect {
			mechanic,
			target: EffectTarget::SelfActor,
			effect: ExpectedEffect::HitCount(HitCountModifier {
				delta: hit_count_delta,
				duration_ticks,
			}),
		});
	}
	Ok(effects)
}

fn validate_base_attack_phase(
	phase: &ExpectedAttackPhase,
	attack: &AttackProfile,
	base_attack: ExpectedValue,
) -> Result<(), String> {
	validate_common_modified_normal_phase(phase, attack, "base")?;
	if phase.primary_hit_count().get() != 1 {
		return Err("base attack has multiple primary hits".to_owned());
	}
	if phase.maximum_primary_targets().get() != u32::from(attack.target_count) {
		return Err("base target count does not match replay attack facts".to_owned());
	}
	if deterministic_raw_attack(phase, "base")? != base_attack {
		return Err("base raw attack does not match replay combat facts".to_owned());
	}
	Ok(())
}

fn validate_active_attack_phase(
	phase: &ExpectedAttackPhase,
	attack: &AttackProfile,
) -> Result<(), String> {
	validate_common_modified_normal_phase(phase, attack, "active")
}

fn validate_common_modified_normal_phase(
	phase: &ExpectedAttackPhase,
	attack: &AttackProfile,
	phase_name: &str,
) -> Result<(), String> {
	if attack.pattern != AttackPattern::Single || !attack.extra_effects.is_empty() {
		return Err("base replay attack is not a plain single-target attack".to_owned());
	}
	let NormalAttackAction::Damage {
		damage_type,
		delivery: DamageDelivery::Normal,
	} = attack.action
	else {
		return Err("base replay attack is not normal damage".to_owned());
	};
	if damage_type != replay_damage_type(phase.damage_type()) {
		return Err(format!(
			"{phase_name} damage type does not match replay attack facts"
		));
	}
	if attack.target_filter != replay_target_filter(phase.target_filter()) {
		return Err(format!(
			"{phase_name} target filter does not match replay attack facts"
		));
	}
	if phase.delivery() != &ExpectedDamageDelivery::Direct {
		return Err(format!(
			"{phase_name} splash or chain delivery is not representable"
		));
	}
	let adjustment = phase.defense_adjustment();
	if adjustment.physical_defense_reduction_ratio() != 0.0
		|| adjustment.physical_defense_reduction_fixed() != 0.0
		|| adjustment.arts_resistance_reduction_ratio() != 0.0
		|| adjustment.arts_resistance_reduction_fixed() != 0.0
	{
		return Err(format!(
			"{phase_name} defense adjustment is not represented by replay modifiers"
		));
	}
	Ok(())
}

fn deterministic_raw_attack(
	phase: &ExpectedAttackPhase,
	phase_name: &str,
) -> Result<ExpectedValue, String> {
	let outcomes = phase.primary_hit_outcomes().outcomes();
	let [outcome] = outcomes else {
		return Err(format!(
			"{phase_name} probabilistic damage requires outcome-by-outcome mitigation"
		));
	};
	if outcome.probability() != 1.0 {
		return Err(format!(
			"{phase_name} probabilistic damage requires outcome-by-outcome mitigation"
		));
	}
	let fixed: FixedDamage = FixedDamage::try_from_f64(outcome.raw_damage())
		.map_err(|error| format!("{phase_name} raw attack is invalid: {error}"))?;
	Ok(ExpectedValue::from_scaled(fixed.scaled()))
}

fn replay_damage_type(damage_type: ExpectedDamageType) -> DamageType {
	match damage_type {
		ExpectedDamageType::Physical => DamageType::Physical,
		ExpectedDamageType::Arts => DamageType::Arts,
		ExpectedDamageType::True => DamageType::True,
	}
}

fn replay_target_filter(target_filter: ExpectedTargetFilter) -> NormalAttackTargetFilter {
	match target_filter {
		ExpectedTargetFilter::GroundAndAir => NormalAttackTargetFilter::GroundAndAir,
		ExpectedTargetFilter::GroundOnly => NormalAttackTargetFilter::GroundOnly,
		ExpectedTargetFilter::AirOnly => NormalAttackTargetFilter::AirOnly,
	}
}

/// A skill lane's factor as an expected value. Rounding to the millionth-scaled lane is checked by
/// the caller, which requires the lanes to reproduce the fused attack exactly before trusting them.
fn exact_lane_multiplier(value: f64) -> Option<ExpectedValue> {
	if !value.is_finite() || value < 0.0 {
		return None;
	}
	FixedDamage::try_from_f64(value)
		.ok()
		.map(|fixed: FixedDamage| ExpectedValue::from_scaled(fixed.scaled()))
}

/// Attack-speed points as the replay's whole-point delta, or `None` when the source is not whole.
fn whole_attack_speed_delta(bonus: f64) -> Option<i16> {
	(bonus.is_finite() && bonus.fract() == 0.0).then_some(())?;
	i16::try_from(bonus as i64).ok()
}

/// The interval the engine will actually run, given the two cadence lanes. Mirrors
/// `effective_attack_interval` so the lowering can refuse anything the engine would round away.
fn replayed_attack_interval(
	base_ticks: u64,
	cadence_multiplier: ExpectedValue,
	attack_speed_delta: i16,
) -> Option<u64> {
	let denominator: i64 = 100_i64.checked_add(i64::from(attack_speed_delta))?;
	let speed_multiplier: ExpectedValue =
		ExpectedValue::from_ratio(100, u64::try_from(denominator).ok()?)?;
	let interval: ExpectedValue =
		ExpectedValue::from_scaled(base_ticks.checked_mul(ExpectedValue::SCALE)?);
	Some(rounded_up_ticks(
		interval
			.checked_mul(cadence_multiplier)?
			.checked_mul(speed_multiplier)?,
	))
}

fn exact_interval_multiplier(base_ticks: u64, active_ticks: u64) -> Option<ExpectedValue> {
	if base_ticks == 0 {
		return None;
	}
	let base_scaled: u64 = base_ticks.checked_mul(ExpectedValue::SCALE)?;
	let numerator: u128 = u128::from(active_ticks).checked_mul(u128::from(ExpectedValue::SCALE))?;
	// The candidate must land just under the target so that rounding the partial tick up reaches
	// it. Rounding the candidate up instead overshoots into the next tick.
	let candidate: u128 = numerator / u128::from(base_ticks);
	let multiplier: ExpectedValue = ExpectedValue::from_scaled(u64::try_from(candidate).ok()?);
	let effective: ExpectedValue =
		ExpectedValue::from_scaled(base_scaled).checked_mul(multiplier)?;
	(rounded_up_ticks(effective) == active_ticks).then_some(multiplier)
}

fn compile_additional_range(
	profile: &ExpectedDamageSkillProfile,
	base_range: &[crate::data::RangeOffset],
	active_range: &[crate::data::RangeOffset],
) -> Result<Vec<crate::data::RangeOffset>, String> {
	let base: BTreeSet<crate::data::RangeOffset> = base_range.iter().copied().collect();
	let mut active: BTreeSet<crate::data::RangeOffset> = active_range.iter().copied().collect();
	for offset in active_range {
		for extension in 1..=profile.range().forward_extension_tiles() {
			let extension: i32 = i32::try_from(extension)
				.map_err(|_| "forward range extension exceeds i32".to_owned())?;
			let col: i32 = offset
				.col()
				.checked_add(extension)
				.ok_or_else(|| "forward range extension overflowed".to_owned())?;
			active.insert(crate::data::RangeOffset::new(offset.row(), col));
		}
	}
	if !base.is_subset(&active) {
		return Err("skill range removes base offsets, which replay cannot model".to_owned());
	}
	Ok(active.difference(&base).copied().collect())
}

fn active_operator_candidates(resolved: &ResolvedOperatorBuild) -> Vec<&OperatorEffectCandidate> {
	let mut active: Vec<&OperatorEffectCandidate> = Vec::new();
	if let Some(candidate) = select_effect_candidate(resolved.trait_candidates(), resolved.build())
	{
		active.push(candidate);
	}
	for group in resolved.talent_groups() {
		if let Some(candidate) = select_effect_candidate(group.candidates(), resolved.build()) {
			active.push(candidate);
		}
	}
	active
}

struct OperatorPrefabCompileContext<'a> {
	snapshot: &'a ArkDataSnapshot,
	prefabs: &'a BattlePrefabSnapshot,
	resolved: &'a ResolvedOperatorBuild,
	active_candidates: &'a [&'a OperatorEffectCandidate],
	roster_facts: &'a ReviewedRosterFacts,
	selected_skill_reached: bool,
	tick_rate: TickRate,
	registry: &'a BattlePrefabHandlerRegistry,
	summon_catalog: &'a mut SummonTemplateCatalog,
	attack: &'a mut AttackProfile,
	on_battle_start: &'a mut Vec<TaggedEffect>,
	on_deploy: &'a mut Vec<TaggedEffect>,
	skill: &'a mut SkillFacts,
	allow_effect_lowering: bool,
}

struct SelectedSkillTemplateCompileContext<'a> {
	snapshot: &'a ArkDataSnapshot,
	prefabs: &'a BattlePrefabSnapshot,
	resolved: &'a ResolvedOperatorBuild,
	registry: &'a BattlePrefabHandlerRegistry,
	tick_rate: TickRate,
	summon_catalog: &'a SummonTemplateCatalog,
	attack: &'a mut AttackProfile,
	on_battle_start: &'a mut Vec<TaggedEffect>,
	on_deploy: &'a mut Vec<TaggedEffect>,
	skill: &'a mut SkillFacts,
}

#[derive(Debug, Default)]
struct OperatorPrefabCompileResult {
	reached_mechanics: Vec<ReachedMechanic>,
	selected_skill_template_claimed: bool,
	/// Whether the claimed template read the character's own skill mode root. A single-root
	/// template that did not leaves that root to the prefab inventory, so evidence it holds stays
	/// visible.
	selected_skill_mode_root_accounted: bool,
	accounted_effect_candidates: BTreeSet<usize>,
	/// Effect candidates whose talent root was read but claimed by no reviewed template. They stay
	/// the effect's omission, which then names the unclaimed root.
	unclaimed_effect_roots: BTreeSet<usize>,
}

/// Binds the reviewed `charge_cost` owner to the selected skill. The skill keeps the SP lifecycle
/// `compile_skill` built from the same level; the template supplies what one activation does and
/// how long the skill then stays active, so a timed skill recharges only after it ends. Anything
/// else the skill carries stays one partial-replay omission under the skill's own ID. A claimed root
/// that fails to decode is Unsupported rather than an omission, because the root is this
/// template's and has drifted. The character's skill mode root is left to the prefab inventory.
fn compile_charge_cost_skill_template(
	common_graph: &BattlePrefabGraph,
	snapshot: &ArkDataSnapshot,
	resolved: &ResolvedOperatorBuild,
	tick_rate: TickRate,
	skill: &mut SkillFacts,
) -> OperatorPrefabCompileResult {
	let origin: String = format!(
		"verified-prefab:{}:{}",
		common_graph.logical_path, common_graph.prefab_name
	);
	let mut result: OperatorPrefabCompileResult = OperatorPrefabCompileResult {
		reached_mechanics: Vec::new(),
		selected_skill_template_claimed: true,
		selected_skill_mode_root_accounted: false,
		accounted_effect_candidates: BTreeSet::new(),
		unclaimed_effect_roots: BTreeSet::new(),
	};
	let drifted = |detail: String| {
		unsupported_mechanic(
			format!("{}/template/charge-cost/decoding", skill.id.as_str()),
			origin.clone(),
			detail,
		)
	};
	let decoded: DecodedChargeCostSkillTemplate = match try_decode_charge_cost_skill_template(
		common_graph,
		resolved.selected_skill(),
		snapshot,
		snapshot.resolved_revision(),
	) {
		Ok(Some(decoded)) => decoded,
		Ok(None) => {
			result.reached_mechanics.push(drifted(
				"recognized common skill identity was not claimed by its charge-cost decoder"
					.to_owned(),
			));
			return result;
		}
		Err(error) => {
			result.reached_mechanics.push(drifted(format!(
				"verified charge-cost skill template drifted: {error}"
			)));
			return result;
		}
	};
	let active_duration_ticks: u64 = match decoded.active_span {
		ChargeCostSkillActiveSpan::Instantaneous => 0,
		ChargeCostSkillActiveSpan::Timed { seconds } => {
			match ticks_from_seconds_ceil(seconds, tick_rate) {
				Ok(ticks) if ticks > 0 => ticks,
				_ => {
					result.reached_mechanics.push(drifted(format!(
						"active span of {seconds} seconds has no positive tick length"
					)));
					return result;
				}
			}
		}
	};
	skill.effects = decoded.activation_effects();
	skill.active_duration_ticks = Some(active_duration_ticks);
	let lifecycle: &str = if skill.activation == SkillActivationRule::Auto {
		"auto_sp"
	} else {
		"manual_sp"
	};
	result.reached_mechanics.push(supported_mechanic(
		format!("{}/{lifecycle}", skill.id.as_str()),
		format!(
			"gamedata:operator:{}/skill:{}",
			resolved.operator_id(),
			resolved.skill_id()
		),
		MechanicBehavior::SpAndSkillActivation,
	));
	result.reached_mechanics.push(supported_mechanic(
		decoded.mechanic_id.as_str().to_owned(),
		origin,
		MechanicBehavior::DpGain,
	));
	if !decoded.remainder.is_empty() {
		result.reached_mechanics.push(unmodeled_mechanic(
			skill.id.as_str().to_owned(),
			format!("operator:{}", resolved.operator_id()),
			format!(
				"selected skill behavior beyond its deployment-point grant is omitted from this \
				 partial replay: {}",
				decoded.remainder.join("; ")
			),
		));
	}
	result
}

fn compile_selected_skill_template(
	context: SelectedSkillTemplateCompileContext<'_>,
) -> OperatorPrefabCompileResult {
	let SelectedSkillTemplateCompileContext {
		snapshot,
		prefabs,
		resolved,
		registry,
		tick_rate,
		summon_catalog,
		attack,
		on_battle_start,
		on_deploy,
		skill,
	} = context;
	const COMMON_SKILLS_LOGICAL_PATH: &str = "[uc]skills";
	const COMMON_SKILLS_ASSET_DIRECTORY: &str = "dyn/battle/prefabs/[uc]skills";

	let selected_skill = resolved.selected_skill();
	let Some(prefab_id) = selected_skill.prefab_id() else {
		return OperatorPrefabCompileResult::default();
	};
	let common_asset_path: String = format!("{COMMON_SKILLS_ASSET_DIRECTORY}/{prefab_id}.prefab");
	let common_graph: BattlePrefabGraph = match interpret_battle_prefab_asset(
		prefabs,
		COMMON_SKILLS_LOGICAL_PATH,
		&common_asset_path,
		BattlePrefabRootSection::MainAsset,
		prefab_id,
		registry,
	) {
		Ok(graph) => graph,
		Err(_) => return OperatorPrefabCompileResult::default(),
	};
	if claims_conditional_heal_attack_template(&common_graph) {
		let origin: String = format!(
			"verified-prefab:{}:{}",
			common_graph.logical_path, common_graph.prefab_name
		);
		let mut result: OperatorPrefabCompileResult = OperatorPrefabCompileResult {
			reached_mechanics: Vec::new(),
			selected_skill_template_claimed: true,
			selected_skill_mode_root_accounted: true,
			accounted_effect_candidates: BTreeSet::new(),
			unclaimed_effect_roots: BTreeSet::new(),
		};
		let decoded = match try_decode_shu_skill_template(
			&common_graph,
			selected_skill,
			snapshot,
			snapshot.resolved_revision(),
		) {
			Ok(Some(decoded)) => decoded,
			Ok(None) => {
				result.reached_mechanics.push(unsupported_mechanic(
					format!("{}/template/conditional-heal/decoding", skill.id.as_str()),
					origin,
					"recognized common skill identity was not claimed by its conditional-heal decoder",
				));
				return result;
			}
			Err(error) => {
				result.reached_mechanics.push(unsupported_mechanic(
					format!("{}/template/conditional-heal/decoding", skill.id.as_str()),
					origin,
					format!("verified conditional-heal skill template drifted: {error}"),
				));
				return result;
			}
		};
		match decoded.try_lower_stored_attack_replacement() {
			Ok(replacement) => {
				skill.stored_attack_replacement = Some(replacement);
				skill.effects.clear();
				result.reached_mechanics.push(supported_mechanic(
					format!("{}/auto_sp", skill.id.as_str()),
					format!(
						"gamedata:operator:{}/skill:{}",
						resolved.operator_id(),
						resolved.skill_id()
					),
					MechanicBehavior::SpAndSkillActivation,
				));
				result.reached_mechanics.push(supported_mechanic(
					decoded.mechanic_id.as_str().to_owned(),
					origin,
					MechanicBehavior::Healing,
				));
			}
			Err(gap) => result.reached_mechanics.push(unmodeled_mechanic(
				format!("{}/lowering", decoded.mechanic_id.as_str()),
				origin,
				gap.to_string(),
			)),
		}
		return result;
	}
	if claims_charge_cost_skill_template(&common_graph) {
		return compile_charge_cost_skill_template(
			&common_graph,
			snapshot,
			resolved,
			tick_rate,
			skill,
		);
	}
	if !claims_sbell_skill_template(&common_graph) {
		return OperatorPrefabCompileResult::default();
	}

	let mut result: OperatorPrefabCompileResult = OperatorPrefabCompileResult {
		reached_mechanics: Vec::new(),
		selected_skill_template_claimed: true,
		selected_skill_mode_root_accounted: true,
		accounted_effect_candidates: BTreeSet::new(),
		unclaimed_effect_roots: BTreeSet::new(),
	};
	let operator_id: &str = resolved.operator_id();
	let mode_prefab_name: String = format!("S{}", resolved.build().skill());
	let mode_graph: BattlePrefabGraph = match interpret_battle_prefab(
		prefabs,
		operator_id,
		BattlePrefabRootSection::Modes,
		&mode_prefab_name,
		registry,
	) {
		Ok(graph) => graph,
		Err(error) => {
			result.reached_mechanics.push(unsupported_mechanic(
				format!("{}/template/dual-root/interpretation", skill.id.as_str()),
				format!(
					"verified-prefab:{}:{}+operator:{operator_id}/Modes/{mode_prefab_name}",
					common_graph.logical_path, common_graph.prefab_name
				),
				format!("recognized common skill template could not load its mode root: {error}"),
			));
			return result;
		}
	};
	let origin: String = format!(
		"verified-prefab:{}:{}+{}:{}",
		common_graph.logical_path,
		common_graph.prefab_name,
		mode_graph.logical_path,
		mode_graph.prefab_name,
	);
	let decoded = match try_decode_sbell_skill_template(
		&common_graph,
		&mode_graph,
		selected_skill,
		snapshot,
		snapshot.resolved_revision(),
	) {
		Ok(Some(decoded)) => decoded,
		Ok(None) => {
			result.reached_mechanics.push(unsupported_mechanic(
				format!("{}/template/dual-root/decoding", skill.id.as_str()),
				origin,
				"recognized common skill identity was not claimed by its dual-root decoder",
			));
			return result;
		}
		Err(error) => {
			result.reached_mechanics.push(unsupported_mechanic(
				format!("{}/template/dual-root/decoding", skill.id.as_str()),
				origin,
				format!("verified dual-root skill template drifted: {error}"),
			));
			return result;
		}
	};
	let skill_mechanic: MechanicId = skill.id.clone();
	attack.target_count = decoded.semantics.attack.minimum_targets_to_cast;
	attack.action = NormalAttackAction::Damage {
		damage_type: decoded.semantics.attack.damage_type,
		delivery: DamageDelivery::Normal,
	};
	skill.active_duration_ticks = None;
	skill.manual_discard = Some(SkillManualDiscardPolicy {
		activation_limit: decoded.semantics.lifecycle.controller.manual_discard_limit,
		allow_while_dozing: decoded
			.semantics
			.lifecycle
			.controller
			.allow_manual_discard_while_dozing,
		// The controller policy is executable, but its custom switch-back buff is still omitted.
		on_discard: Vec::new(),
	});
	skill.effects.clear();
	result.reached_mechanics.push(supported_mechanic(
		format!("{}/auto_sp", skill_mechanic.as_str()),
		format!(
			"gamedata:operator:{operator_id}/skill:{}",
			resolved.skill_id()
		),
		MechanicBehavior::SpAndSkillActivation,
	));
	let semantic_lowering = decoded.lower_program();
	match semantic_lowering.program.lower_with_actor_templates(
		selected_skill.blackboard(),
		tick_rate,
		summon_catalog.bindings(),
	) {
		Ok(lowered) => {
			on_battle_start.extend(lowered.on_battle_start);
			on_deploy.extend(lowered.on_deploy);
			result.reached_mechanics.extend(lowered.reached_mechanics);
		}
		Err(error) => {
			result.reached_mechanics.push(unsupported_mechanic(
				format!("{}/snow-overlay/lowering", skill_mechanic.as_str()),
				origin.clone(),
				format!("verified snow-overlay semantics could not lower: {error}"),
			));
		}
	}
	if !semantic_lowering.remaining_gap.requirements.is_empty() {
		result.reached_mechanics.push(unmodeled_mechanic(
			format!("{}/remaining-mode-mechanics", skill_mechanic.as_str()),
			origin,
			semantic_lowering.remaining_gap.to_string(),
		));
	}
	result
}

fn compile_operator_prefab_inventory(
	context: OperatorPrefabCompileContext<'_>,
) -> OperatorPrefabCompileResult {
	let OperatorPrefabCompileContext {
		snapshot,
		prefabs,
		resolved,
		active_candidates,
		roster_facts,
		selected_skill_reached,
		tick_rate,
		registry,
		summon_catalog,
		attack,
		on_battle_start,
		on_deploy,
		skill,
		allow_effect_lowering,
	} = context;
	#[derive(Debug)]
	struct RootRequest<'a> {
		root_id: String,
		section: BattlePrefabRootSection,
		prefab_names: Vec<String>,
		candidate_index: Option<usize>,
		candidate: Option<&'a OperatorEffectCandidate>,
	}

	let operator_id: &str = resolved.operator_id();
	let mut result: OperatorPrefabCompileResult = if selected_skill_reached {
		compile_selected_skill_template(SelectedSkillTemplateCompileContext {
			snapshot,
			prefabs,
			resolved,
			registry,
			tick_rate,
			summon_catalog,
			attack,
			on_battle_start,
			on_deploy,
			skill,
		})
	} else {
		OperatorPrefabCompileResult::default()
	};
	let mut roots: Vec<RootRequest<'_>> = active_candidates
		.iter()
		.enumerate()
		.filter_map(|(index, candidate)| {
			candidate.prefab_key().map(|prefab_key| RootRequest {
				root_id: format!("effect/{index}/{prefab_key}"),
				section: BattlePrefabRootSection::Talents,
				prefab_names: vec![prefab_key.to_owned()],
				candidate_index: Some(index),
				candidate: Some(*candidate),
			})
		})
		.collect();
	if selected_skill_reached && !result.selected_skill_mode_root_accounted {
		let skill_roots: Vec<String> = resolved
			.selected_skill()
			.override_prefab_key()
			.map(|prefab_key| vec![prefab_key.to_owned()])
			.unwrap_or_else(|| vec![format!("S{}", resolved.build().skill())]);
		roots.push(RootRequest {
			root_id: format!("skill/{}", resolved.skill_id()),
			section: BattlePrefabRootSection::Modes,
			prefab_names: skill_roots,
			candidate_index: None,
			candidate: None,
		});
	}

	let mechanics: &mut Vec<ReachedMechanic> = &mut result.reached_mechanics;
	for root in roots {
		let mut graph_result: Option<(String, BattlePrefabGraph)> = None;
		let mut last_not_found: Option<BattlePrefabInterpretError> = None;
		for prefab_name in &root.prefab_names {
			match interpret_battle_prefab(prefabs, operator_id, root.section, prefab_name, registry)
			{
				Ok(graph) => {
					graph_result = Some((prefab_name.clone(), graph));
					break;
				}
				Err(error @ BattlePrefabInterpretError::PrefabNotFound { .. }) => {
					last_not_found = Some(error);
				}
				Err(error) => {
					last_not_found = Some(error);
					break;
				}
			}
		}
		let (prefab_name, graph): (String, BattlePrefabGraph) = match graph_result {
			Some(result) => result,
			None => {
				if matches!(
					&last_not_found,
					Some(
						BattlePrefabInterpretError::BundleNotFound { .. }
							| BattlePrefabInterpretError::PrefabNotFound { .. }
					)
				) {
					continue;
				}
				let error: String = last_not_found.map_or_else(
					|| "prefab root request contained no candidate names".to_owned(),
					|error| error.to_string(),
				);
				let origin: String = format!(
					"verified-prefab:operator:{operator_id}/prefab-candidates:{}",
					root.prefab_names.join(",")
				);
				mechanics.push(unsupported_mechanic(
					format!("{operator_id}/prefab/{}/interpretation", root.root_id),
					origin,
					format!("verified battle-prefab interpretation failed: {error}"),
				));
				if let Some(candidate_index) = root.candidate_index {
					result.accounted_effect_candidates.insert(candidate_index);
				}
				continue;
			}
		};
		let origin: String = format!("verified-prefab:operator:{operator_id}/prefab:{prefab_name}");
		let TalentRootClaim {
			claimed: template_claimed,
			consumed_issues,
			consumed_fact_sources,
		}: TalentRootClaim = match root.candidate {
			Some(candidate) => claim_talent_root(TalentRootClaimContext {
				graph: &graph,
				candidate,
				snapshot,
				resolved,
				roster_facts,
				tick_rate,
				summon_catalog,
				root_id: &root.root_id,
				origin: &origin,
				on_battle_start,
				on_deploy,
				mechanics,
			}),
			None => TalentRootClaim::default(),
		};
		if template_claimed {
			let candidate_index: usize = root
				.candidate_index
				.expect("only operator-effect roots can claim an effect template");
			result.accounted_effect_candidates.insert(candidate_index);
		} else if let Some(candidate_index) = root.candidate_index {
			// The talent's base behaviour is known from gamedata, so a root no reviewed template
			// claims is what the effect's omission leaves out, not evidence that refuses the replay.
			result.unclaimed_effect_roots.insert(candidate_index);
			continue;
		}
		for (index, issue) in graph.issues.iter().enumerate() {
			if consumed_issues.contains(issue) {
				continue;
			}
			mechanics.push(unsupported_mechanic(
				format!("{operator_id}/prefab/{}/issue/{index}", root.root_id),
				origin.clone(),
				prefab_semantic_issue_reason(issue),
			));
		}
		let mut fact_index: usize = 0;
		for behavior in &graph.mono_behaviours {
			for fact in &behavior.semantic_facts {
				let mechanic_id: String =
					format!("{operator_id}/prefab/{}/fact/{fact_index}", root.root_id);
				fact_index += 1;
				if consumed_fact_sources.contains(&fact.provenance.source) {
					continue;
				}
				if let Some(mechanic) = emit_prefab_semantic_fact(
					fact,
					&mechanic_id,
					&origin,
					&behavior.source,
					allow_effect_lowering,
					on_deploy,
				) {
					mechanics.push(mechanic);
				}
			}
		}
	}
	result
}

/// What the reviewed talent templates made of one verified talent root. A template that recognizes
/// the root claims it even when it then rejects it as drift, so only a root no template recognizes
/// is left unclaimed.
#[derive(Debug, Default)]
struct TalentRootClaim {
	claimed: bool,
	consumed_issues: Vec<BattlePrefabSemanticIssue>,
	consumed_fact_sources: Vec<BattlePrefabObjectSource>,
}

struct TalentRootClaimContext<'a> {
	graph: &'a BattlePrefabGraph,
	candidate: &'a OperatorEffectCandidate,
	snapshot: &'a ArkDataSnapshot,
	resolved: &'a ResolvedOperatorBuild,
	roster_facts: &'a ReviewedRosterFacts,
	tick_rate: TickRate,
	summon_catalog: &'a SummonTemplateCatalog,
	root_id: &'a str,
	origin: &'a str,
	on_battle_start: &'a mut Vec<TaggedEffect>,
	on_deploy: &'a mut Vec<TaggedEffect>,
	mechanics: &'a mut Vec<ReachedMechanic>,
}

/// Offers one talent root to each reviewed talent template in turn. The first that recognizes it
/// claims it: a successful decode lowers its effects and names the evidence it consumed, and a
/// drifted one is reported Unsupported.
fn claim_talent_root(context: TalentRootClaimContext<'_>) -> TalentRootClaim {
	let TalentRootClaimContext {
		graph,
		candidate,
		snapshot,
		resolved,
		roster_facts,
		tick_rate,
		summon_catalog,
		root_id,
		origin,
		on_battle_start,
		on_deploy,
		mechanics,
	} = context;
	let operator_id: &str = resolved.operator_id();
	let mut consumed_issues: Vec<BattlePrefabSemanticIssue> = Vec::new();
	let mut consumed_fact_sources: Vec<BattlePrefabObjectSource> = Vec::new();
	let mut template_claimed: bool = false;
	match try_decode_sowed_field_template(graph, candidate, snapshot, snapshot.resolved_revision())
	{
		Ok(Some(decoded)) => {
			template_claimed = true;
			match decoded.program.lower_with_actor_templates(
				candidate.blackboard(),
				tick_rate,
				summon_catalog.bindings(),
			) {
				Ok(lowered) => {
					on_battle_start.extend(lowered.on_battle_start);
					on_deploy.extend(lowered.on_deploy);
					mechanics.extend(lowered.reached_mechanics);
					consumed_issues = decoded.consumed_issues;
					consumed_fact_sources = decoded.consumed_semantic_fact_sources;
				}
				Err(error) => mechanics.push(unsupported_mechanic(
					format!("{operator_id}/prefab/{}/template/lowering", root_id),
					origin.to_owned(),
					format!("verified mechanic template could not lower to replay: {error}"),
				)),
			}
		}
		Ok(None) => {}
		Err(error) => {
			template_claimed = true;
			mechanics.push(unsupported_mechanic(
				format!("{operator_id}/prefab/{}/template/decoding", root_id),
				origin.to_owned(),
				format!("verified mechanic template drifted: {error}"),
			));
		}
	}
	if !template_claimed {
		match try_decode_snowfield_template(
			graph,
			candidate,
			snapshot,
			snapshot.resolved_revision(),
		) {
			Ok(Some(decoded)) => {
				template_claimed = true;
				let program: MechanicSemanticProgram = decoded.lower_program();
				match program.lower_with_actor_templates(
					candidate.blackboard(),
					tick_rate,
					summon_catalog.bindings(),
				) {
					Ok(lowered) => {
						on_battle_start.extend(lowered.on_battle_start);
						on_deploy.extend(lowered.on_deploy);
						mechanics.extend(lowered.reached_mechanics);
						consumed_issues = decoded.consumed_issues;
						consumed_fact_sources = decoded.consumed_semantic_fact_sources;
					}
					Err(error) => mechanics.push(unsupported_mechanic(
						format!("{operator_id}/prefab/{}/template/lowering", root_id),
						origin.to_owned(),
						format!("verified snowfield template could not lower to replay: {error}"),
					)),
				}
			}
			Ok(None) => {}
			Err(error) => {
				template_claimed = true;
				mechanics.push(unsupported_mechanic(
					format!(
						"{operator_id}/prefab/{}/template/snowfield_decoding",
						root_id
					),
					origin.to_owned(),
					format!("verified snowfield template drifted: {error}"),
				));
			}
		}
	}
	if !template_claimed {
		match try_decode_blessing_template(graph, candidate, snapshot, snapshot.resolved_revision())
		{
			Ok(Some(decoded)) => {
				template_claimed = true;
				let program: MechanicSemanticProgram = decoded.lower_program();
				match program.lower_with_actor_templates(
					candidate.blackboard(),
					tick_rate,
					summon_catalog.bindings(),
				) {
					Ok(lowered) => {
						on_battle_start.extend(lowered.on_battle_start);
						on_deploy.extend(lowered.on_deploy);
						mechanics.extend(lowered.reached_mechanics);
						consumed_issues = decoded.consumed_issues;
						consumed_fact_sources = decoded.consumed_semantic_fact_sources;
					}
					Err(error) => mechanics.push(unsupported_mechanic(
						format!("{operator_id}/prefab/{}/template/lowering", root_id),
						origin.to_owned(),
						format!("verified blessing template could not lower to replay: {error}"),
					)),
				}
			}
			Ok(None) => {}
			Err(error) => {
				template_claimed = true;
				mechanics.push(unsupported_mechanic(
					format!(
						"{operator_id}/prefab/{}/template/blessing_decoding",
						root_id
					),
					origin.to_owned(),
					format!("verified blessing template drifted: {error}"),
				));
			}
		}
	}
	if !template_claimed {
		match try_decode_shu_roster_template(
			graph,
			candidate,
			snapshot,
			snapshot.resolved_revision(),
		) {
			Ok(Some(decoded)) => {
				template_claimed = true;
				let lowering: Result<LoweredMechanicSemanticProgram, MechanicSemanticError> =
					lower_shu_roster_template(
						&decoded,
						candidate.blackboard(),
						tick_rate,
						summon_catalog.bindings(),
						roster_facts,
					);
				consumed_issues = decoded.consumed_issues;
				consumed_fact_sources = decoded.consumed_semantic_fact_sources;
				match lowering {
					Ok(lowered) => {
						on_battle_start.extend(lowered.on_battle_start);
						on_deploy.extend(lowered.on_deploy);
						mechanics.extend(lowered.reached_mechanics);
					}
					Err(error) => mechanics.push(unsupported_mechanic(
						format!("{operator_id}/prefab/{}/template/roster_lowering", root_id),
						origin.to_owned(),
						format!(
							"verified roster-threshold template could not lower to replay: {error}"
						),
					)),
				}
			}
			Ok(None) => {}
			Err(error) => {
				template_claimed = true;
				mechanics.push(unsupported_mechanic(
					format!("{operator_id}/prefab/{}/template/roster_decoding", root_id),
					origin.to_owned(),
					format!("verified roster-threshold template drifted: {error}"),
				));
			}
		}
	}
	if !template_claimed {
		match try_decode_block_toggle_template(
			graph,
			candidate,
			resolved.static_stats().block_count(),
			snapshot,
			snapshot.resolved_revision(),
		) {
			Ok(Some(decoded)) => {
				template_claimed = true;
				match decoded.lower_program().lower_with_actor_templates(
					candidate.blackboard(),
					tick_rate,
					summon_catalog.bindings(),
				) {
					Ok(lowered) => {
						on_battle_start.extend(lowered.on_battle_start);
						on_deploy.extend(lowered.on_deploy);
						mechanics.extend(lowered.reached_mechanics);
						consumed_issues = decoded.consumed_issues;
					}
					Err(error) => mechanics.push(unsupported_mechanic(
						format!("{operator_id}/prefab/{}/template/lowering", root_id),
						origin.to_owned(),
						format!(
								"verified block-count toggle template could not lower to replay: {error}"
							),
					)),
				}
			}
			Ok(None) => {}
			Err(error) => {
				template_claimed = true;
				mechanics.push(unsupported_mechanic(
					format!(
						"{operator_id}/prefab/{}/template/block_toggle_decoding",
						root_id
					),
					origin.to_owned(),
					format!("verified block-count toggle template drifted: {error}"),
				));
			}
		}
	}
	TalentRootClaim {
		claimed: template_claimed,
		consumed_issues,
		consumed_fact_sources,
	}
}

fn emit_prefab_semantic_fact(
	fact: &BattlePrefabSemanticFact,
	mechanic_id: &str,
	origin: &str,
	source: &BattlePrefabObjectSource,
	allow_effect_lowering: bool,
	on_deploy: &mut Vec<TaggedEffect>,
) -> Option<ReachedMechanic> {
	match lower_prefab_semantic_fact(fact, mechanic_id) {
		Ok(Some(lowered)) if allow_effect_lowering => {
			on_deploy.extend(lowered.effects);
			Some(supported_mechanic(
				mechanic_id.to_owned(),
				origin.to_owned(),
				lowered.behavior,
			))
		}
		Ok(Some(_)) => Some(unsupported_mechanic(
			mechanic_id.to_owned(),
			origin.to_owned(),
			"generic prefab lowering is suppressed because a reviewed operator override is active; cross-authority equivalence is not proven",
		)),
		Ok(None) => None,
		Err(reason) => Some(unsupported_mechanic(
			mechanic_id.to_owned(),
			origin.to_owned(),
			format!(
				"verified prefab fact {:?} from {} has no safe replay lowering: {reason}",
				fact.kind(),
				prefab_source_label(source),
			),
		)),
	}
}

#[derive(Debug)]
struct LoweredPrefabFact {
	effects: Vec<TaggedEffect>,
	behavior: MechanicBehavior,
}

/// Lowers only script configurations whose exact built-in handler contract maps directly to an
/// existing replay primitive. Recognizing a script ID is not sufficient: dynamic selectors,
/// blackboard-backed values, custom buffs, and timing semantics remain fail-closed.
fn lower_prefab_semantic_fact(
	fact: &BattlePrefabSemanticFact,
	mechanic_id: &str,
) -> Result<Option<LoweredPrefabFact>, String> {
	match &fact.semantic {
		BattlePrefabSemantic::CompositeAbility(_) => Ok(None),
		BattlePrefabSemantic::StaticBuffAbility(ability) => {
			lower_static_buff_fact(ability, mechanic_id).map(Some)
		}
		BattlePrefabSemantic::AttackAbility(_) => Err(
			"attack selector, attack-type, and cast-timing semantics are not lowered yet".to_owned(),
		),
		BattlePrefabSemantic::TileListener(_) => Err(
			"tile target validation, cell selection, and detach/removal semantics are not lowered yet"
				.to_owned(),
		),
	}
}

fn lower_static_buff_fact(
	ability: &BattlePrefabStaticBuffAbility,
	mechanic_id: &str,
) -> Result<LoweredPrefabFact, String> {
	if ability.target_selector.file_id != 0 || ability.target_selector.path_id != 0 {
		return Err("only the static-ability null selector (owner actor) is supported".to_owned());
	}
	let configuration = &ability.ability_configuration;
	if !configuration.enabled
		|| !configuration.metadata_named_as_alias.is_empty()
		|| !configuration.metadata_blackboard_prefix.is_empty()
		|| configuration.interrupt_on_detach
		|| configuration.attach_passive_buffs_on_dummy
		|| configuration.ignore_if_owner_dead
		|| configuration.force_as_damage_or_heal
	{
		return Err("static ability configuration is not the supported neutral default".to_owned());
	}
	if ability.buffs.is_empty() {
		return Err("static ability has no buff configuration".to_owned());
	}
	let mechanic: MechanicId = MechanicId::new(mechanic_id.to_owned());
	let mut effects: Vec<TaggedEffect> = Vec::new();
	for (buff_index, buff) in ability.buffs.iter().enumerate() {
		let buff_path: String = format!("buffs[{buff_index}]");
		let attributes = &buff.attribute_configuration;
		if !attributes.abnormal_antis.is_empty()
			|| !attributes.abnormal_combo_immunes.is_empty()
			|| !attributes.abnormal_combos.is_empty()
			|| !attributes.abnormal_flags.is_empty()
			|| !attributes.abnormal_immunes.is_empty()
		{
			return Err(format!(
				"{buff_path}.attribute_configuration contains abnormal-status semantics"
			));
		}
		validate_neutral_buff_configuration(buff, &buff_path)?;
		if buff.attribute_modifiers.is_empty() {
			return Err(format!(
				"buffs[{buff_index}] has no generic attribute modifiers"
			));
		}
		for (modifier_index, modifier) in buff.attribute_modifiers.iter().enumerate() {
			let path: String = format!("buffs[{buff_index}].attribute_modifiers[{modifier_index}]");
			if modifier.load_from_blackboard || modifier.fetch_base_value_from_source_entity {
				return Err(format!("{path} uses dynamic value resolution"));
			}
			// `formulaItem` names the lane the gamedata modifier resolves in: ADDITION(0) adds a
			// flat amount to the stat, MULTIPLIER(1) adds `base * value`. Only MULTIPLIER is read,
			// and its contribution is a share of the base value — a panel percentage, which sums
			// with every other panel percentage on the stat rather than multiplying against them.
			if modifier.formula_item != 1 {
				return Err(format!("{path} is not a MULTIPLIER modifier"));
			}
			let stat: StatKind = match modifier.attribute_type {
				0 => StatKind::MaxHp,
				1 => StatKind::Attack,
				2 => StatKind::Defense,
				value => {
					return Err(format!("{path} attribute type {value} has no replay stat"));
				}
			};
			let multiplier_value: f64 = 1.0 + modifier.value;
			if !multiplier_value.is_finite() || multiplier_value < 0.0 {
				return Err(format!("{path}.value produces an invalid multiplier"));
			}
			let fixed: FixedDamage = FixedDamage::try_from(multiplier_value)
				.map_err(|_| format!("{path}.value exceeds replay precision or range"))?;
			effects.push(TaggedEffect {
				mechanic: mechanic.clone(),
				target: EffectTarget::SelfActor,
				effect: ExpectedEffect::StatModifier(StatModifier {
					bucket: StatModifierBucket::Panel,
					stat,
					multiplier: ExpectedValue::from_scaled(fixed.scaled()),
					duration_ticks: None,
				}),
			});
		}
	}
	Ok(LoweredPrefabFact {
		effects,
		behavior: MechanicBehavior::StatModifier,
	})
}

fn validate_neutral_buff_configuration(
	buff: &BattlePrefabBuffConfigurationSet,
	path: &str,
) -> Result<(), String> {
	let configuration = &buff.configuration;
	if !configuration.audio_signal.is_empty()
		|| !configuration.blackboard.is_empty()
		|| !configuration.buff_key.is_empty()
		|| configuration.clear_all_stack_count_when_time_up
		|| configuration.disable_override
		|| configuration.duration_key != "duration"
		|| configuration.enable_initial_direction_from_source
		|| configuration.independent_character_source
		|| configuration.is_damage_missable
		|| configuration.is_durable
		|| configuration.is_freezable
		|| configuration.is_levitatable
		|| configuration.is_silenceable
		|| configuration.is_stunnable
		|| configuration.load_from_database
		|| configuration.max_stack_count != 1
		|| configuration.max_valid_stack_count != -1
		|| configuration.on_event_priority != 0
		|| !configuration.override_effect_key.is_empty()
		|| !configuration.override_key.is_empty()
		|| configuration.override_on_event_priority != 0
		|| configuration.override_type != 0
		|| configuration.priority != -1
		|| !configuration.priority_blackboard_keys.is_empty()
		|| configuration.refresh_remaining_time_when_stack_max
		|| configuration.status_resistable != 2
		|| configuration.strip_blackboard_params_with_buff_key
		|| configuration.take_snapshot_when_extend
		|| !matches!(configuration.template_key.as_str(), "" | "empty")
	{
		return Err(format!(
			"{path}.configuration is not the supported neutral default"
		));
	}
	let timing = &buff.listener_timing;
	if timing.life_time != 0.0
		|| timing.life_time_type != 2
		|| timing.first_trigger_interval != -1.0
		|| timing.trigger_interval != -1.0
		|| timing.trigger_count != 0
		|| timing.trigger_life_type != 0
		|| !timing.wait_first_trigger_interval
	{
		return Err(format!(
			"{path}.listener_timing is not permanent and passive"
		));
	}
	Ok(())
}

fn prefab_semantic_issue_reason(issue: &BattlePrefabSemanticIssue) -> String {
	match issue {
		BattlePrefabSemanticIssue::MissingScriptId { source } => format!(
			"MonoBehaviour {} has no serialized script ID",
			prefab_source_label(source)
		),
		BattlePrefabSemanticIssue::UnknownScriptId { source, script_id } => format!(
			"MonoBehaviour {} uses unregistered script ID {}",
			prefab_source_label(source),
			script_id_hex(script_id)
		),
		BattlePrefabSemanticIssue::ScriptSchemaHashMismatch {
			source,
			script_id,
			actual_old_type_hash,
			expected_old_type_hashes,
		} => format!(
			"MonoBehaviour {} script {} has old-type hash {}, expected one of {:?}",
			prefab_source_label(source),
			script_id_hex(script_id),
			actual_old_type_hash
				.as_ref()
				.map(script_id_hex)
				.unwrap_or_else(|| "missing".to_owned()),
			expected_old_type_hashes
				.iter()
				.map(script_id_hex)
				.collect::<Vec<_>>()
		),
		BattlePrefabSemanticIssue::UnavailableExternalReference {
			source,
			field_path,
			file_id,
			path_id,
			target_cab,
		} => format!(
			"MonoBehaviour {} field {field_path} references {target_cab} (fileID {file_id}, pathID \
			 {path_id}), which is outside the verified snapshot",
			prefab_source_label(source)
		),
		BattlePrefabSemanticIssue::ScriptHandlerFailed {
			source,
			script_id,
			old_type_hash,
			reason,
		} => format!(
			"MonoBehaviour {} script {} schema {} could not be interpreted: {reason}",
			prefab_source_label(source),
			script_id_hex(script_id),
			script_id_hex(old_type_hash)
		),
		BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
			source,
			field_path,
			buff_key,
			template_key,
		} => format!(
			"MonoBehaviour {} field {field_path} has custom buff semantics absent from TypeTree \
			 (buff_key={buff_key:?}, template_key={template_key:?})",
			prefab_source_label(source)
		),
	}
}

fn prefab_source_label(source: &BattlePrefabObjectSource) -> String {
	format!("{}:{}", source.cab_name, source.path_id)
}

fn script_id_hex(script_id: &[u8; 16]) -> String {
	script_id
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect::<Vec<_>>()
		.join("")
}

fn selected_skill_behavior_is_reached(
	prepared: &PreparedReplayGuide,
	operator: OperatorInstanceId,
	activation: &SkillActivationType,
) -> bool {
	match activation {
		SkillActivationType::Auto | SkillActivationType::Passive => true,
		SkillActivationType::Manual => {
			let explicit_activation: bool = prepared.actions.iter().any(|action| {
				matches!(
					action,
					crate::mechanics::domain::MaterializedGuideAction::ActivateSkill {
						operator: action_operator,
						..
					} if *action_operator == operator
				)
			});
			let daemon_enabled: bool = prepared.actions.iter().any(|action| {
				matches!(
					action,
					crate::mechanics::domain::MaterializedGuideAction::SkillDaemon { .. }
				)
			});
			explicit_activation
				|| (daemon_enabled && prepared.policy.skill_usage(operator) == Some(1))
		}
		SkillActivationType::Unknown(_) => true,
	}
}

fn unmodeled_operator_effect_reason(
	candidate: &OperatorEffectCandidate,
	unclaimed_prefab_root: bool,
) -> String {
	let blackboard_keys: Vec<&str> = candidate
		.blackboard()
		.iter()
		.map(crate::data::BlackboardEntry::key)
		.collect();
	let prefab_root: &str = if unclaimed_prefab_root {
		"; its verified prefab root is read but no reviewed mechanic template claims it"
	} else {
		""
	};
	format!(
		"active trait/talent effect is omitted from this partial replay \
		 (prefab={:?}, blackboard_keys={blackboard_keys:?}){prefab_root}",
		candidate.prefab_key()
	)
}

fn select_effect_candidate<'a>(
	candidates: &'a [OperatorEffectCandidate],
	build: &OperatorBuild,
) -> Option<&'a OperatorEffectCandidate> {
	candidates
		.iter()
		.filter(|candidate| candidate_is_active(candidate, build))
		.max_by_key(|candidate| {
			(
				parse_phase(candidate.unlock_phase()).unwrap_or(0),
				candidate.unlock_level(),
				candidate.required_potential_rank(),
			)
		})
}

fn candidate_is_active(candidate: &OperatorEffectCandidate, build: &OperatorBuild) -> bool {
	let Some(phase) = parse_phase(candidate.unlock_phase()) else {
		return false;
	};
	let level_unlocked: bool = build.elite() > phase
		|| (build.elite() == phase && build.level() >= candidate.unlock_level());
	let applied_potential: u8 = build.potential().number().saturating_sub(1);
	level_unlocked && applied_potential >= candidate.required_potential_rank()
}

fn parse_phase(value: &str) -> Option<u8> {
	value.strip_prefix("PHASE_")?.parse().ok()
}

fn validate_deployment_cell(
	stage: &CompiledStage,
	build: &ResolvedOperatorBuild,
	cell: GridPosition,
) -> Result<(), ExpectedReplayAdapterError> {
	let map_cell = stage
		.cell(cell)
		.ok_or(ExpectedReplayAdapterError::InvalidDeploymentCell {
			operator: build.operator_id().to_owned(),
			cell,
		})?;
	let allowed: bool = match (build.position(), map_cell.buildable()) {
		(OperatorPosition::Melee, Buildable::Melee | Buildable::All)
		| (OperatorPosition::Ranged, Buildable::Ranged | Buildable::All)
		| (OperatorPosition::All, Buildable::Melee | Buildable::Ranged | Buildable::All) => true,
		(
			OperatorPosition::Melee
			| OperatorPosition::Ranged
			| OperatorPosition::All
			| OperatorPosition::None,
			Buildable::None | Buildable::Melee | Buildable::Ranged | Buildable::All,
		) => false,
		(OperatorPosition::Unknown(_), _) | (_, Buildable::Unknown(_)) => false,
	};
	if !allowed {
		return Err(ExpectedReplayAdapterError::InvalidDeploymentCell {
			operator: build.operator_id().to_owned(),
			cell,
		});
	}
	Ok(())
}

fn compile_stage_mechanics(
	stage: &CompiledStage,
	active_route_indices: &BTreeSet<u32>,
	enemy_classes: &BTreeMap<String, TargetClass>,
) -> Vec<ReachedMechanic> {
	let supported_tiles: BTreeSet<&str> = [
		"tile_end",
		"tile_fence_bound",
		"tile_floor",
		"tile_forbidden",
		"tile_road",
		"tile_start",
		"tile_telin",
		"tile_telout",
		"tile_wall",
	]
	.into_iter()
	.collect();
	let active_enemy_keys: BTreeSet<&str> = enemy_classes.keys().map(String::as_str).collect();
	let mut unsupported: Vec<ReachedMechanic> = stage
		.mechanic_requirements()
		.references()
		.iter()
		.filter(|reference| {
			stage_requirement_is_reached(stage, reference, active_route_indices, &active_enemy_keys)
		})
		.filter_map(|reference| stage_requirement_unsupported(reference, &supported_tiles))
		.collect();

	let function_disable_mask: FunctionDisableMask = stage.options().function_disable_mask();
	if !function_disable_mask.is_empty() {
		unsupported.push(unsupported_mechanic(
			format!(
				"stage/function_disable_mask/{}",
				function_disable_mask.bits()
			),
			format!("stage:{}", stage.stage_id()),
			"non-empty function-disable masks are not registered by expected replay",
		));
	}
	for group in stage.active_hidden_groups() {
		if group != "normal" {
			unsupported.push(unsupported_mechanic(
				format!("hidden_group/{group}"),
				format!("stage:{}", stage.stage_id()),
				"active hidden group is not reviewed by Normal adapter v0",
			));
		}
	}
	unsupported
}

fn stage_requirement_is_reached(
	stage: &CompiledStage,
	reference: &MechanicReference,
	active_route_indices: &BTreeSet<u32>,
	active_enemy_keys: &BTreeSet<&str>,
) -> bool {
	match reference.origin() {
		MechanicOrigin::MapCell { .. }
		| MechanicOrigin::MapEffect { .. }
		| MechanicOrigin::Rune { .. }
		| MechanicOrigin::GlobalBuff { .. } => true,
		MechanicOrigin::RouteCheckpoint { extra, route, .. } => {
			!*extra && active_route_indices.contains(route)
		}
		MechanicOrigin::Enemy { enemy } | MechanicOrigin::EnemySkill { enemy, .. } => {
			active_enemy_keys.contains(enemy.as_str())
		}
		MechanicOrigin::Predefined {
			hard, collection, ..
		} => {
			!*hard
				&& matches!(
					collection,
					PredefinedCollection::CharacterInsts | PredefinedCollection::TokenInsts
				)
		}
		MechanicOrigin::WaveAction(origin) => stage
			.waves()
			.iter()
			.flat_map(|wave| wave.fragments())
			.flat_map(|fragment| fragment.actions())
			.any(|action| action.origin() == origin && stage.action_is_active(action)),
	}
}

fn stage_requirement_unsupported(
	reference: &MechanicReference,
	supported_tiles: &BTreeSet<&str>,
) -> Option<ReachedMechanic> {
	let kind: MechanicKind = reference.identity().kind();
	let id: &str = reference.identity().id();
	if stage_requirement_is_handled(kind, id, reference.origin(), supported_tiles) {
		return None;
	}
	Some(unsupported_mechanic(
		stage_requirement_id(reference),
		stage_requirement_origin(reference.origin()),
		stage_requirement_reason(kind),
	))
}

fn stage_requirement_is_handled(
	kind: MechanicKind,
	id: &str,
	origin: &MechanicOrigin,
	supported_tiles: &BTreeSet<&str>,
) -> bool {
	match (kind, origin) {
		(MechanicKind::Tile, _) => supported_tiles.contains(id),
		(MechanicKind::RouteCheckpoint, MechanicOrigin::RouteCheckpoint { .. }) => {
			matches!(id, "WAIT_FOR_SECONDS" | "DISAPPEAR" | "APPEAR_AT_POS")
		}
		(
			MechanicKind::Rune,
			MechanicOrigin::Rune {
				optional_group: None,
				..
			},
		) => id == "level_hidden_group_enable",
		(MechanicKind::WaveAction, MechanicOrigin::WaveAction(_)) => {
			// The schedule skips these actions entirely; see EXECUTOR_INERT_ACTION_IDS.
			action_id_is_executor_inert(id)
		}
		(MechanicKind::EnemyPrefab, MechanicOrigin::Enemy { .. })
		| (MechanicKind::EnemySkillPrefab, MechanicOrigin::EnemySkill { .. })
		| (
			MechanicKind::Token | MechanicKind::Predefined,
			MechanicOrigin::Predefined {
				collection: PredefinedCollection::CharacterInsts | PredefinedCollection::TokenInsts,
				..
			},
		) => {
			// Active enemy prefabs and skills are accepted here only because their owning EnemyFacts
			// carry their own exact inventory: a reviewed-gamedata catalog entry, a verified-prefab
			// decode from this snapshot, an undescribed-gamedata omission when no enemy pack is
			// pinned, or Unsupported. Preplaced actors likewise carry their own support inventory.
			true
		}
		_ => false,
	}
}

fn stage_requirement_id(reference: &MechanicReference) -> String {
	let kind: &str = match reference.identity().kind() {
		MechanicKind::Tile => "tile",
		MechanicKind::MapEffect => "map_effect",
		MechanicKind::RouteCheckpoint => "route_checkpoint",
		MechanicKind::Rune => "rune",
		MechanicKind::GlobalBuffPrefab => "global_buff",
		MechanicKind::EnemyPrefab => "enemy_prefab",
		MechanicKind::EnemySkillPrefab => "enemy_skill",
		MechanicKind::Token => "token",
		MechanicKind::Predefined => "predefined",
		MechanicKind::WaveAction => "wave_action",
	};
	match reference.origin() {
		MechanicOrigin::Enemy { enemy } | MechanicOrigin::EnemySkill { enemy, .. } => {
			format!(
				"stage/{kind}/{}/{}",
				enemy.as_str(),
				reference.identity().id()
			)
		}
		_ => format!("stage/{kind}/{}", reference.identity().id()),
	}
}

fn stage_requirement_origin(origin: &MechanicOrigin) -> String {
	match origin {
		MechanicOrigin::MapCell { position } => {
			format!("gamedata:cell:{},{}", position.row, position.col)
		}
		MechanicOrigin::MapEffect { position, index } => match position {
			Some(position) => {
				format!("cell:{},{}:effect:{index}", position.row, position.col)
			}
			None => format!("map:effect:{index}"),
		},
		MechanicOrigin::RouteCheckpoint {
			extra,
			route,
			checkpoint,
		} => format!(
			"{}:{route}:checkpoint:{checkpoint}",
			if *extra { "extra_route" } else { "route" }
		),
		MechanicOrigin::Rune {
			optional_group,
			index,
		} => optional_group.as_ref().map_or_else(
			|| format!("rune:{index}"),
			|group| format!("optional_rune:{group}:{index}"),
		),
		MechanicOrigin::GlobalBuff { index } => format!("global_buff:{index}"),
		MechanicOrigin::Enemy { enemy } => format!("enemy:{}", enemy.as_str()),
		MechanicOrigin::EnemySkill { enemy, index } => {
			format!("enemy:{}:skill:{index}", enemy.as_str())
		}
		MechanicOrigin::Predefined {
			hard,
			collection,
			index,
		} => format!(
			"{}:{collection:?}:{index}",
			if *hard {
				"hard_predefined"
			} else {
				"predefined"
			}
		),
		MechanicOrigin::WaveAction(origin) => {
			format!("wave_action:{:?}:{}", origin.container, origin.action)
		}
	}
}

fn stage_requirement_reason(kind: MechanicKind) -> &'static str {
	match kind {
		MechanicKind::Tile => "tile behavior is not registered",
		MechanicKind::MapEffect => "map effect behavior is not registered",
		MechanicKind::RouteCheckpoint => "route checkpoint behavior is not registered",
		MechanicKind::Rune => "active or unresolved optional rune behavior is not registered",
		MechanicKind::GlobalBuffPrefab => "global buff behavior is not registered",
		MechanicKind::EnemyPrefab => "enemy prefab behavior is not registered",
		MechanicKind::EnemySkillPrefab => "enemy skill behavior is not registered",
		MechanicKind::Token | MechanicKind::Predefined => {
			"predefined actor behavior is not registered"
		}
		MechanicKind::WaveAction => "wave action behavior is not registered",
	}
}

fn actor_combat_facts(
	operator: &str,
	stats: crate::data::OperatorBaseStats,
	target_class: TargetClass,
) -> Result<ActorCombatFacts, ExpectedReplayAdapterError> {
	Ok(ActorCombatFacts {
		max_hp: expected_f64(operator, "max_hp", stats.max_hp())?,
		attack: expected_f64(operator, "attack", stats.attack())?,
		defense: expected_f64(operator, "defense", stats.defense())?,
		target_class,
		arts_resistance: expected_ratio_percent(operator, stats.magic_resistance())?,
		minimum_damage_ratio: ActorCombatFacts::REVIEWED_MINIMUM_DAMAGE_RATIO,
	})
}

fn inert_attack(tick_rate: TickRate) -> AttackProfile {
	AttackProfile {
		first_delay_ticks: u64::from(tick_rate.ticks_per_second().get()),
		interval_ticks: u64::from(tick_rate.ticks_per_second().get()),
		range: vec![crate::data::RangeOffset::new(0, 0)],
		target_count: 0,
		target_filter: NormalAttackTargetFilter::GroundOnly,
		action: NormalAttackAction::None,
		pattern: AttackPattern::Single,
		extra_effects: Vec::new(),
	}
}

fn attack_interval_ticks(
	base_attack_time_seconds: f64,
	attack_speed: f64,
	tick_rate: TickRate,
) -> Result<u64, ExpectedReplayAdapterError> {
	if !base_attack_time_seconds.is_finite()
		|| base_attack_time_seconds <= 0.0
		|| !attack_speed.is_finite()
		|| attack_speed <= 0.0
	{
		return Err(ExpectedReplayAdapterError::InvalidAttackTiming);
	}
	let seconds: f64 = base_attack_time_seconds / (attack_speed / 100.0);
	let ticks: u64 = ticks_from_seconds_ceil(seconds, tick_rate)
		.map_err(ExpectedReplayAdapterError::StageSpaceTime)?;
	if ticks == 0 {
		return Err(ExpectedReplayAdapterError::InvalidAttackTiming);
	}
	Ok(ticks)
}

/// The enemy table's own name for a record that attacks from a distance.
const RANGED_APPLY_WAY: &str = "RANGED";

/// Whether an enemy record's tables establish a reach its normal attack can execute.
///
/// A record's level layer may set no radius at all, and a radius it does set may be a non-positive
/// code the pinned tables never explain. Those are different states and neither is a distance, so
/// for a record the tables call ranged, replay has no reach to execute. It executes the record's
/// own cell and records the reach it left out, which withholds both verdicts: the base attack is
/// independently known from the tables, and the value that is missing is a lookup the pinned tables
/// do not answer rather than evidence that is malformed. A melee record keeps that cell, which is
/// the reach it has.
fn unestablished_ranged_reach(
	enemy_key: &str,
	apply_way: Option<&str>,
	radius: Option<f64>,
) -> Option<ReachedMechanic> {
	if apply_way != Some(RANGED_APPLY_WAY) {
		return None;
	}
	if radius.is_some_and(|radius: f64| radius.is_finite() && radius > 0.0) {
		return None;
	}
	let stated: String = match radius {
		Some(radius) => format!("states an attack radius of {radius}"),
		None => "sets no attack radius in any of its level layers".to_owned(),
	};
	Some(unmodeled_mechanic(
		format!("{enemy_key}/normal-attack/range"),
		format!("gamedata:enemy:{enemy_key}"),
		format!(
			"the record's tables call it {RANGED_APPLY_WAY} but it {stated}, which is not a \
			 distance this replay can execute, so its normal attack reaches only its own cell; \
			 what a non-positive radius resolves to is not established by the pinned tables"
		),
	))
}

/// The cells an enemy record's normal attack reaches.
///
/// An absent radius and a non-positive one are kept apart rather than folded together: both leave
/// the record reaching only its own cell, which is a melee record's reach, but only one of them is
/// a value the tables actually state. A ranged record that lands here is reported by
/// [`unestablished_ranged_reach`] instead of executing this reach.
fn enemy_range_offsets(
	radius: Option<f64>,
) -> Result<Vec<crate::data::RangeOffset>, ExpectedReplayAdapterError> {
	let Some(radius) = radius else {
		return Ok(vec![crate::data::RangeOffset::new(0, 0)]);
	};
	if !radius.is_finite() || radius > f64::from(i16::MAX) {
		return Err(ExpectedReplayAdapterError::InvalidEnemyRange { radius });
	}
	if radius <= 0.0 {
		return Ok(vec![crate::data::RangeOffset::new(0, 0)]);
	}
	let bound: i32 = radius.ceil() as i32;
	let mut offsets: Vec<crate::data::RangeOffset> = Vec::new();
	for row in -bound..=bound {
		for col in -bound..=bound {
			if f64::from(row).hypot(f64::from(col)) <= radius {
				offsets.push(crate::data::RangeOffset::new(row, col));
			}
		}
	}
	if offsets.is_empty() {
		offsets.push(crate::data::RangeOffset::new(0, 0));
	}
	Ok(offsets)
}

fn map_facing(direction: &MapDirection) -> Result<Facing, ExpectedReplayAdapterError> {
	match direction {
		MapDirection::Up => Ok(Facing::Up),
		MapDirection::Right => Ok(Facing::Right),
		MapDirection::Down => Ok(Facing::Down),
		MapDirection::Left => Ok(Facing::Left),
		MapDirection::Invalid | MapDirection::Unknown(_) => {
			Err(ExpectedReplayAdapterError::InvalidMapDirection {
				value: direction.stable_id().to_owned(),
			})
		}
	}
}

fn unsupported_mechanic(id: String, origin: String, reason: impl Into<String>) -> ReachedMechanic {
	ReachedMechanic {
		id: MechanicId::new(id),
		origin,
		behavior: MechanicBehavior::Unsupported {
			reason: reason.into(),
		},
	}
}

fn unmodeled_mechanic(id: String, origin: String, reason: impl Into<String>) -> ReachedMechanic {
	ReachedMechanic {
		id: MechanicId::new(id),
		origin,
		behavior: MechanicBehavior::UnmodeledOmission {
			reason: reason.into(),
		},
	}
}

/// The declared approximation a path carries when its route scatters its spawn point.
///
/// `spawnRandomRange` displaces a spawn uniformly within a box around the route's start point. The
/// replay path model holds whole cells only, so it executes no displacement at all and every enemy
/// starts at the start cell.
///
/// The box belongs to the route, so every path built from that route shares one inventory entry;
/// each still carries its own trigger, and the entry is exercised once any of those paths is spawned
/// onto.
fn spawn_scatter_approximation(
	stage: &CompiledStage,
	scatter: StageVector2,
	route_index: u32,
	path: PathId,
) -> Option<ReachedMechanic> {
	if scatter.x == 0.0 && scatter.y == 0.0 {
		return None;
	}
	Some(approximated_mechanic(
		format!("stage/route/{route_index}/spawn-scatter"),
		format!("stage:{}/route:{route_index}", stage.stage_id()),
		format!(
			"the route declares a spawn scatter of half-widths ({}, {}) that replay does not \
			 execute: its path model holds whole cells only, so every spawn starts at the route's \
			 start cell. The box is symmetric about that cell, but arrival ticks are not linear in \
			 the spawn point, so a zero-mean displacement does not establish a zero-mean timeline",
			scatter.x, scatter.y
		),
		ApproximationTrigger::PathSpawn { path },
	))
}

/// One behavior replay executes in a declared simplified form rather than omitting.
fn approximated_mechanic(
	id: String,
	origin: String,
	reason: impl Into<String>,
	exercised_by: ApproximationTrigger,
) -> ReachedMechanic {
	ReachedMechanic {
		id: MechanicId::new(id),
		origin,
		behavior: MechanicBehavior::DeclaredApproximation {
			reason: reason.into(),
			exercised_by,
		},
	}
}

fn supported_mechanic(id: String, origin: String, behavior: MechanicBehavior) -> ReachedMechanic {
	ReachedMechanic {
		id: MechanicId::new(id),
		origin,
		behavior,
	}
}
fn expected_i32(
	enemy_key: &str,
	field: &'static str,
	value: i32,
) -> Result<ExpectedValue, ExpectedReplayAdapterError> {
	if value < 0 {
		return Err(ExpectedReplayAdapterError::InvalidEnemyStat {
			enemy_key: enemy_key.to_owned(),
			field,
		});
	}
	Ok(ExpectedValue::from_whole(value as u32))
}

fn expected_f64(
	operator: &str,
	field: &'static str,
	value: f64,
) -> Result<ExpectedValue, ExpectedReplayAdapterError> {
	let fixed: FixedDamage = FixedDamage::try_from(value).map_err(|_| {
		ExpectedReplayAdapterError::InvalidOperatorStat {
			operator: operator.to_owned(),
			field,
		}
	})?;
	Ok(ExpectedValue::from_scaled(fixed.scaled()))
}

fn expected_ratio_percent(
	actor: &str,
	value: f64,
) -> Result<ExpectedValue, ExpectedReplayAdapterError> {
	if !value.is_finite() || !(0.0..=100.0).contains(&value) {
		return Err(ExpectedReplayAdapterError::InvalidRatio {
			actor: actor.to_owned(),
			value,
		});
	}
	let fixed: FixedDamage = FixedDamage::try_from(value / 100.0).map_err(|_| {
		ExpectedReplayAdapterError::InvalidRatio {
			actor: actor.to_owned(),
			value,
		}
	})?;
	Ok(ExpectedValue::from_scaled(fixed.scaled()))
}

fn required_enemy_i32(
	enemy_key: &str,
	field: &'static str,
	value: Option<i32>,
) -> Result<i32, ExpectedReplayAdapterError> {
	value.ok_or_else(|| ExpectedReplayAdapterError::MissingEnemyStat {
		enemy_key: enemy_key.to_owned(),
		field,
	})
}

fn required_enemy_f64(
	enemy_key: &str,
	field: &'static str,
	value: Option<f64>,
) -> Result<f64, ExpectedReplayAdapterError> {
	let value: f64 = value.ok_or_else(|| ExpectedReplayAdapterError::MissingEnemyStat {
		enemy_key: enemy_key.to_owned(),
		field,
	})?;
	if !value.is_finite() {
		return Err(ExpectedReplayAdapterError::InvalidEnemyStat {
			enemy_key: enemy_key.to_owned(),
			field,
		});
	}
	Ok(value)
}

fn exact_nonnegative_u32(value: f64) -> Option<u32> {
	if !value.is_finite() || value < 0.0 || value > f64::from(u32::MAX) {
		return None;
	}
	let rounded: f64 = value.round();
	(value == rounded).then_some(rounded as u32)
}

#[derive(Debug)]
pub enum ExpectedReplayAdapterError {
	ArkData(ArkDataError),
	StageSpaceTime(StageCompileError),
	SummonTemplateCatalog(SummonTemplateCatalogError),
	UnsupportedStageShape {
		detail: String,
	},
	BuildBindingMismatch {
		expected: Vec<String>,
		actual: Vec<String>,
	},
	DuplicateOperatorIdentity {
		operator_id: String,
	},
	IdentifierOverflow,
	TickOverflow,
	UnsupportedStageAction {
		origin: String,
		kind: String,
	},
	UnsupportedSpawnSchedule {
		origin: String,
	},
	MissingEnemy {
		enemy_key: String,
	},
	MissingEnemyStat {
		enemy_key: String,
		field: &'static str,
	},
	InvalidEnemyStat {
		enemy_key: String,
		field: &'static str,
	},
	InvalidOperatorStat {
		operator: String,
		field: &'static str,
	},
	InvalidRatio {
		actor: String,
		value: f64,
	},
	InvalidAttackTiming,
	InvalidEnemyRange {
		radius: f64,
	},
	InvalidRouteIndex {
		route_index: u32,
	},
	MissingRouteFacing {
		route_index: u32,
	},
	UnsupportedMotion {
		value: String,
	},
	InconsistentEnemyMotion {
		enemy_key: String,
	},
	UnsupportedBuildable {
		value: String,
	},
	UnsupportedTileHeight {
		value: String,
	},
	InvalidDpRules,
	UnsupportedPreplacedActor {
		key: String,
		detail: String,
	},
	InvalidMapDirection {
		value: String,
	},
	UnknownBuildBinding {
		name: String,
	},
	MissingDeployment {
		name: String,
	},
	InvalidDeploymentCell {
		operator: String,
		cell: GridPosition,
	},
	UnsupportedSkillMetadata {
		operator: String,
		detail: String,
	},
}

impl Display for ExpectedReplayAdapterError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::ArkData(error) => Display::fmt(error, formatter),
			Self::StageSpaceTime(error) => Display::fmt(error, formatter),
			Self::SummonTemplateCatalog(error) => Display::fmt(error, formatter),
			Self::UnsupportedStageShape { detail } => formatter.write_str(detail),
			Self::BuildBindingMismatch { expected, actual } => write!(
				formatter,
				"exact build bindings mismatch: expected {expected:?}, found {actual:?}"
			),
			Self::DuplicateOperatorIdentity { operator_id } => write!(
				formatter,
				"exact replay roster contains duplicate operator {operator_id:?}"
			),
			Self::IdentifierOverflow => formatter.write_str("replay identifier overflowed"),
			Self::TickOverflow => formatter.write_str("replay tick arithmetic overflowed"),
			Self::UnsupportedStageAction { origin, kind } => {
				write!(formatter, "unsupported stage action {kind} at {origin}")
			}
			Self::UnsupportedSpawnSchedule { origin } => {
				write!(formatter, "unsupported spawn schedule at {origin}")
			}
			Self::MissingEnemy { enemy_key } => {
				write!(formatter, "active enemy {enemy_key:?} is not compiled")
			}
			Self::MissingEnemyStat { enemy_key, field } => {
				write!(formatter, "enemy {enemy_key:?} is missing {field}")
			}
			Self::InvalidEnemyStat { enemy_key, field } => {
				write!(formatter, "enemy {enemy_key:?} has invalid {field}")
			}
			Self::InvalidOperatorStat { operator, field } => {
				write!(formatter, "operator {operator:?} has invalid {field}")
			}
			Self::InvalidRatio { actor, value } => {
				write!(formatter, "actor {actor:?} has invalid percentage {value}")
			}
			Self::InvalidAttackTiming => formatter.write_str("attack timing must be positive"),
			Self::InvalidEnemyRange { radius } => {
				write!(formatter, "enemy attack range radius {radius} is invalid")
			}
			Self::InvalidRouteIndex { route_index } => {
				write!(
					formatter,
					"active spawn references invalid route {route_index}"
				)
			}
			Self::MissingRouteFacing { route_index } => {
				write!(
					formatter,
					"active route {route_index} has no movement from which to derive enemy facing"
				)
			}
			Self::UnsupportedMotion { value } => {
				write!(formatter, "unsupported route motion {value:?}")
			}
			Self::InconsistentEnemyMotion { enemy_key } => write!(
				formatter,
				"enemy {enemy_key:?} appears with inconsistent motion classes"
			),
			Self::UnsupportedBuildable { value } => {
				write!(formatter, "unsupported buildable category {value:?}")
			}
			Self::UnsupportedTileHeight { value } => {
				write!(formatter, "unsupported tile height category {value:?}")
			}
			Self::InvalidDpRules => formatter.write_str("stage has invalid passive DP rules"),
			Self::UnsupportedPreplacedActor { key, detail } => {
				write!(formatter, "preplaced actor {key:?}: {detail}")
			}
			Self::InvalidMapDirection { value } => {
				write!(formatter, "invalid map direction {value:?}")
			}
			Self::UnknownBuildBinding { name } => {
				write!(formatter, "unknown exact build binding {name:?}")
			}
			Self::MissingDeployment { name } => {
				write!(formatter, "operator {name:?} has no Deploy action")
			}
			Self::InvalidDeploymentCell { operator, cell } => {
				write!(formatter, "operator {operator:?} cannot deploy at {cell:?}")
			}
			Self::UnsupportedSkillMetadata { operator, detail } => {
				write!(formatter, "operator {operator:?} skill metadata: {detail}")
			}
		}
	}
}

impl Error for ExpectedReplayAdapterError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::ArkData(error) => Some(error),
			Self::StageSpaceTime(error) => Some(error),
			Self::SummonTemplateCatalog(error) => Some(error),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::{BTreeMap, BTreeSet};
	use std::num::NonZeroU32;

	use super::{action_is_executor_inert, EXECUTOR_INERT_ACTION_IDS};
	use crate::data::{
		GridPosition, MechanicKind, MechanicOrigin, OperatorPosition, RangeOffset,
		SkillActivationType, SkillDurationType, SkillSpType, StageActionKind,
	};
	use crate::simulation::{SimTick, TickRate};

	use super::{
		emit_prefab_semantic_fact, enemy_block_demand, initial_route_facing,
		lower_prefab_semantic_fact, lower_shu_roster_template, register_selected_skill_behavior,
		roster_condition_is_proven_false, selected_skill_behavior_is_reached,
		stage_requirement_is_handled, AttackPattern, AttackProfile, DamageDelivery, DamageType,
		ExpectedEffect, ExpectedReplayAdapterError, ExpectedValue, MechanicBehavior, MechanicId,
		NormalAttackAction, OperatorInstanceId, PreparedReplayGuide, ReviewedRosterFacts,
		RouteTimeline, RouteTimelineAction, SelectedSkillBehaviorContext, SkillFacts,
		StatModifierBucket,
	};
	use crate::mechanics::{
		compile::compile_expected_damage_skill_facts,
		domain::{
			DecodedShuRosterTemplate, GamedataSemanticSourceLocation, MaterializedGuideAction,
			MaterializedGuidePolicy, MechanicSemanticError, NormalAttackTargetFilter,
			SemanticProvenance, SemanticValueExpression, ShuRosterBranchCombination,
			ShuRosterBranchLifetime, ShuRosterCondition, ShuRosterConditionalBranch,
			ShuRosterEffect, ShuRosterModifierAggregation, ShuRosterStat, ShuRosterTargetFilter,
			ShuRosterTargetScope, ShuRosterTemplateSemantics, SkillDamageBlackboardFact,
			SkillDamageFacts,
		},
		prefab::{
			BattlePrefabAbilityConfiguration, BattlePrefabAttributeConfiguration,
			BattlePrefabAttributeModifier, BattlePrefabBuffConfiguration,
			BattlePrefabBuffConfigurationSet, BattlePrefabListenerTiming,
			BattlePrefabObjectReference, BattlePrefabObjectSource, BattlePrefabSemantic,
			BattlePrefabSemanticFact, BattlePrefabSemanticProvenance,
			BattlePrefabStaticBuffAbility,
		},
		stage::RouteTimelineStep,
	};

	fn tick_rate() -> TickRate {
		TickRate::new(NonZeroU32::new(30).expect("test tick rate is non-zero"))
	}

	#[test]
	fn roster_upper_bounds_only_prove_impossible_thresholds() {
		let roster: ReviewedRosterFacts = ReviewedRosterFacts {
			distinct_profession_count: 2,
			maximum_same_profession_count: 1,
			group_counts: BTreeMap::from([("sui".to_owned(), 2)]),
		};
		let distinct_three: ShuRosterCondition = ShuRosterCondition::DeployedDistinctProfessions {
			minimum_count: 3,
			serialized_profession_mask: 639,
		};
		let same_three: ShuRosterCondition = ShuRosterCondition::DeployedSameProfession {
			minimum_count: 3,
			serialized_profession_mask: 639,
		};
		let tag_four: ShuRosterCondition = ShuRosterCondition::FixedRosterSerializedTag {
			minimum_count: 4,
			filter_tag: "sui".to_owned(),
			serialized_category_mask: 9,
			includes_source: true,
		};
		let distinct_two: ShuRosterCondition = ShuRosterCondition::DeployedDistinctProfessions {
			minimum_count: 2,
			serialized_profession_mask: 639,
		};

		assert!(roster_condition_is_proven_false(&distinct_three, &roster));
		assert!(roster_condition_is_proven_false(&same_three, &roster));
		assert!(roster_condition_is_proven_false(&tag_four, &roster));
		assert!(!roster_condition_is_proven_false(&distinct_two, &roster));
	}

	fn shu_roster_fixture(max_hp_amount: SemanticValueExpression) -> DecodedShuRosterTemplate {
		let provenance: SemanticProvenance = SemanticProvenance::Gamedata {
			gamedata_revision: "fixture-gamedata".to_owned(),
			decoder_rule_revision: "fixture-decoder".to_owned(),
			source: GamedataSemanticSourceLocation {
				table_path: "fixture/table.json".to_owned(),
				record_key: "fixture".to_owned(),
				field_paths: vec!["$.fixture".to_owned()],
				resolved_projection_fields: Vec::new(),
			},
		};
		let target_scope: ShuRosterTargetScope = ShuRosterTargetScope {
			filter: ShuRosterTargetFilter::ValidatorProfessionMask(639),
			includes_source: true,
			includes_future_deployments: true,
		};
		DecodedShuRosterTemplate {
			semantics: ShuRosterTemplateSemantics {
				combination: ShuRosterBranchCombination::IndependentAndSimultaneous,
				distinct_professions: ShuRosterConditionalBranch {
					mechanic_id: MechanicId::new("fixture/shu/distinct"),
					provenance: provenance.clone(),
					condition: ShuRosterCondition::DeployedDistinctProfessions {
						minimum_count: 3,
						serialized_profession_mask: 639,
					},
					targets: target_scope,
					effects: vec![ShuRosterEffect::StatModifier {
						stat: ShuRosterStat::MaxHp,
						aggregation: ShuRosterModifierAggregation::MultiplicativeBonus,
						amount: max_hp_amount,
					}],
					lifetime: ShuRosterBranchLifetime::DynamicWhileConditionSatisfied {
						remove_on_condition_loss: true,
						remove_on_source_detach: true,
					},
				},
				same_profession: ShuRosterConditionalBranch {
					mechanic_id: MechanicId::new("fixture/shu/same"),
					provenance: provenance.clone(),
					condition: ShuRosterCondition::DeployedSameProfession {
						minimum_count: 3,
						serialized_profession_mask: 639,
					},
					targets: target_scope,
					effects: vec![ShuRosterEffect::StatModifier {
						stat: ShuRosterStat::AttackSpeed,
						aggregation: ShuRosterModifierAggregation::Additive,
						amount: SemanticValueExpression::Literal(ExpectedValue::from_whole(15)),
					}],
					lifetime: ShuRosterBranchLifetime::DynamicWhileConditionSatisfied {
						remove_on_condition_loss: true,
						remove_on_source_detach: true,
					},
				},
				roster_tag: ShuRosterConditionalBranch {
					mechanic_id: MechanicId::new("fixture/shu/roster"),
					provenance,
					condition: ShuRosterCondition::FixedRosterSerializedTag {
						minimum_count: 4,
						filter_tag: "sui".to_owned(),
						serialized_category_mask: 639,
						includes_source: true,
					},
					targets: ShuRosterTargetScope {
						filter: ShuRosterTargetFilter::DeckCategoryMask(639),
						..target_scope
					},
					effects: vec![
						ShuRosterEffect::StatModifier {
							stat: ShuRosterStat::Attack,
							aggregation: ShuRosterModifierAggregation::MultiplicativeBonus,
							amount: SemanticValueExpression::Literal(
								ExpectedValue::from_ratio(2, 25).expect("valid ratio"),
							),
						},
						ShuRosterEffect::PeriodicSkillPointGrant {
							interval_seconds: SemanticValueExpression::Literal(
								ExpectedValue::from_whole(4),
							),
							points: SemanticValueExpression::Literal(ExpectedValue::from_whole(1)),
						},
					],
					lifetime: ShuRosterBranchLifetime::FixedRosterForBattle,
				},
			},
			consumed_issues: Vec::new(),
			consumed_semantic_fact_sources: Vec::new(),
		}
	}

	#[test]
	fn shu_roster_adapter_lowers_active_auras_and_validates_before_inactive_pruning() {
		let decoded: DecodedShuRosterTemplate =
			shu_roster_fixture(SemanticValueExpression::Literal(
				ExpectedValue::from_ratio(13, 100).expect("valid ratio"),
			));
		let actor_templates = BTreeMap::new();
		let active = lower_shu_roster_template(
			&decoded,
			&[],
			tick_rate(),
			&actor_templates,
			&ReviewedRosterFacts {
				distinct_profession_count: 3,
				maximum_same_profession_count: 3,
				group_counts: BTreeMap::from([("sui".to_owned(), 4)]),
			},
		)
		.expect("active roster auras must lower");
		assert_eq!(active.on_battle_start.len(), 1);
		assert_eq!(active.on_deploy.len(), 2);
		assert_eq!(active.reached_mechanics.len(), 3);
		assert!(active
			.reached_mechanics
			.iter()
			.all(|mechanic| mechanic.behavior == MechanicBehavior::ConditionalAura));
		assert!(active.reached_mechanics.iter().all(|mechanic| !matches!(
			mechanic.behavior,
			MechanicBehavior::ConservativeBeneficialOmission { .. }
		)));

		let inactive_roster: ReviewedRosterFacts = ReviewedRosterFacts {
			distinct_profession_count: 2,
			maximum_same_profession_count: 1,
			group_counts: BTreeMap::from([("sui".to_owned(), 2)]),
		};
		let inactive = lower_shu_roster_template(
			&decoded,
			&[],
			tick_rate(),
			&actor_templates,
			&inactive_roster,
		)
		.expect("proven-inactive branches still validate and lower first");
		assert!(inactive.on_deploy.is_empty());
		assert!(inactive.on_battle_start.is_empty());
		assert!(inactive
			.reached_mechanics
			.iter()
			.all(|mechanic| mechanic.behavior == MechanicBehavior::ProvenInactive));

		let drifted: DecodedShuRosterTemplate =
			shu_roster_fixture(SemanticValueExpression::Blackboard("missing".to_owned()));
		assert!(matches!(
			lower_shu_roster_template(
				&drifted,
				&[],
				tick_rate(),
				&actor_templates,
				&inactive_roster,
			),
			Err(MechanicSemanticError::MissingBlackboardValue { ref key, .. })
				if key == "missing"
		));
	}

	fn permanent_static_buff_fact(load_from_blackboard: bool) -> BattlePrefabSemanticFact {
		BattlePrefabSemanticFact {
			provenance: BattlePrefabSemanticProvenance {
				source: BattlePrefabObjectSource {
					logical_path: "fixture".to_owned(),
					cab_name: "CAB-fixture".to_owned(),
					path_id: 1,
				},
				script_id: [1; 16],
				old_type_hash: [2; 16],
				mechanics_revision: "a".repeat(64),
			},
			semantic: BattlePrefabSemantic::StaticBuffAbility(BattlePrefabStaticBuffAbility {
				target_selector: BattlePrefabObjectReference {
					file_id: 0,
					path_id: 0,
				},
				ability_configuration: BattlePrefabAbilityConfiguration {
					enabled: true,
					metadata_named_as_alias: String::new(),
					metadata_blackboard_prefix: String::new(),
					interrupt_on_detach: false,
					attach_passive_buffs_on_dummy: false,
					ignore_if_owner_dead: false,
					force_as_damage_or_heal: false,
				},
				buffs: vec![BattlePrefabBuffConfigurationSet {
					attribute_modifiers: vec![BattlePrefabAttributeModifier {
						attribute_type: 1,
						formula_item: 1,
						value: 0.25,
						load_from_blackboard,
						fetch_base_value_from_source_entity: false,
					}],
					attribute_configuration: BattlePrefabAttributeConfiguration {
						abnormal_antis: Vec::new(),
						abnormal_combo_immunes: Vec::new(),
						abnormal_combos: Vec::new(),
						abnormal_flags: Vec::new(),
						abnormal_immunes: Vec::new(),
					},
					configuration: BattlePrefabBuffConfiguration {
						audio_signal: String::new(),
						blackboard: Vec::new(),
						buff_key: String::new(),
						clear_all_stack_count_when_time_up: false,
						disable_override: false,
						duration_key: "duration".to_owned(),
						enable_initial_direction_from_source: false,
						independent_character_source: false,
						is_damage_missable: false,
						is_durable: false,
						is_freezable: false,
						is_levitatable: false,
						is_silenceable: false,
						is_stunnable: false,
						load_from_database: false,
						max_stack_count: 1,
						max_valid_stack_count: -1,
						on_event_priority: 0,
						override_effect_key: String::new(),
						override_key: String::new(),
						override_on_event_priority: 0,
						override_type: 0,
						priority: -1,
						priority_blackboard_keys: Vec::new(),
						refresh_remaining_time_when_stack_max: false,
						status_resistable: 2,
						strip_blackboard_params_with_buff_key: false,
						take_snapshot_when_extend: false,
						template_key: "empty".to_owned(),
					},
					listener_timing: BattlePrefabListenerTiming {
						life_time: 0.0,
						life_time_type: 2,
						first_trigger_interval: -1.0,
						trigger_interval: -1.0,
						trigger_count: 0,
						trigger_life_type: 0,
						wait_first_trigger_interval: true,
					},
				}],
			}),
		}
	}

	#[test]
	fn constant_static_prefab_buff_lowers_to_on_deploy_stat_primitive() {
		let lowered =
			lower_prefab_semantic_fact(&permanent_static_buff_fact(false), "fixture/static")
				.expect("constant generic static buff is supported")
				.expect("static buff emits a primitive");
		assert_eq!(lowered.behavior, MechanicBehavior::StatModifier);
		assert_eq!(lowered.effects.len(), 1);
		assert_eq!(lowered.effects[0].target, super::EffectTarget::SelfActor);
		// A MULTIPLIER modifier contributes `base * value`, so `value: 0.25` is the panel wording
		// "攻击力+25%" and belongs in the stat's panel bucket rather than standing alone.
		assert!(matches!(
			&lowered.effects[0].effect,
			ExpectedEffect::StatModifier(modifier)
				if modifier.stat == super::StatKind::Attack
					&& modifier.bucket == super::StatModifierBucket::Panel
					&& modifier.multiplier == ExpectedValue::from_ratio(5, 4).unwrap()
					&& modifier.duration_ticks.is_none()
		));
	}

	#[test]
	fn blackboard_static_prefab_buff_remains_fail_closed() {
		let error = lower_prefab_semantic_fact(&permanent_static_buff_fact(true), "fixture/static")
			.expect_err("blackboard resolution must not be guessed");
		assert!(error.contains("dynamic value resolution"));
	}

	#[test]
	fn mixed_static_prefab_buff_semantics_remain_fail_closed() {
		let mut abnormal: BattlePrefabSemanticFact = permanent_static_buff_fact(false);
		let BattlePrefabSemantic::StaticBuffAbility(ability) = &mut abnormal.semantic else {
			unreachable!("fixture is a static buff")
		};
		ability.buffs[0].attribute_configuration.abnormal_flags = vec![11];
		let abnormal_error = lower_prefab_semantic_fact(&abnormal, "fixture/static")
			.expect_err("attribute modifier plus abnormal status must fail closed");
		assert!(abnormal_error.contains("abnormal-status semantics"));

		let mut stacked: BattlePrefabSemanticFact = permanent_static_buff_fact(false);
		let BattlePrefabSemantic::StaticBuffAbility(ability) = &mut stacked.semantic else {
			unreachable!("fixture is a static buff")
		};
		ability.buffs[0].configuration.max_stack_count = 2;
		let stack_error = lower_prefab_semantic_fact(&stacked, "fixture/static")
			.expect_err("attribute modifier plus stacking must fail closed");
		assert!(stack_error.contains("neutral default"));

		let mut detachable: BattlePrefabSemanticFact = permanent_static_buff_fact(false);
		let BattlePrefabSemantic::StaticBuffAbility(ability) = &mut detachable.semantic else {
			unreachable!("fixture is a static buff")
		};
		ability.ability_configuration.interrupt_on_detach = true;
		let detach_error = lower_prefab_semantic_fact(&detachable, "fixture/static")
			.expect_err("attribute modifier plus detach behavior must fail closed");
		assert!(detach_error.contains("neutral default"));
	}

	#[test]
	fn reviewed_override_gate_emits_no_duplicate_prefab_effect() {
		let mut on_deploy = Vec::new();
		let mechanic = emit_prefab_semantic_fact(
			&permanent_static_buff_fact(false),
			"fixture/static",
			"verified-prefab:fixture",
			&BattlePrefabObjectSource {
				logical_path: "fixture".to_owned(),
				cab_name: "CAB-fixture".to_owned(),
				path_id: 1,
			},
			false,
			&mut on_deploy,
		)
		.expect("suppressed fact remains an explicit mechanic inventory entry");
		assert!(on_deploy.is_empty());
		assert!(matches!(
			mechanic.behavior,
			MechanicBehavior::Unsupported { ref reason }
				if reason.contains("reviewed operator override is active")
		));
	}

	fn blackboard(key: &str, value: f64) -> SkillDamageBlackboardFact {
		SkillDamageBlackboardFact::new(key.to_owned(), value, None)
	}

	fn replay_skill(operator_id: &str, skill_id: &str) -> SkillFacts {
		SkillFacts {
			id: MechanicId::new(format!("{operator_id}/skill/{skill_id}")),
			activation: super::SkillActivationRule::Manual,
			initial_sp: 10,
			sp_cost: 30,
			charge: super::SpChargeRule::Time {
				amount: 1,
				interval_ticks: 30,
			},
			periodic_sp_sources: Vec::new(),
			active_duration_ticks: Some(0),
			manual_discard: None,
			stored_attack_replacement: None,
			effects: Vec::new(),
		}
	}

	fn replay_attack(
		interval_ticks: u64,
		damage_type: DamageType,
		range: Vec<RangeOffset>,
	) -> AttackProfile {
		AttackProfile {
			first_delay_ticks: interval_ticks,
			interval_ticks,
			range,
			target_count: 1,
			target_filter: NormalAttackTargetFilter::GroundAndAir,
			action: NormalAttackAction::Damage {
				damage_type,
				delivery: DamageDelivery::Normal,
			},
			pattern: AttackPattern::Single,
			extra_effects: Vec::new(),
		}
	}

	#[test]
	fn undefined_enemy_block_count_uses_game_default() {
		assert_eq!(enemy_block_demand("enemy", None).unwrap(), 1);
		assert_eq!(enemy_block_demand("enemy", Some(0)).unwrap(), 0);
		assert!(matches!(
			enemy_block_demand("enemy", Some(-1)),
			Err(ExpectedReplayAdapterError::InvalidEnemyStat {
				ref enemy_key,
				field: "block_count",
			}) if enemy_key == "enemy"
		));
	}

	#[test]
	fn stationary_route_facing_fails_closed_instead_of_defaulting_right() {
		let timeline = RouteTimeline {
			start: GridPosition { row: 1, col: 1 },
			goal: GridPosition { row: 1, col: 1 },
			steps: vec![RouteTimelineStep {
				duration_ticks: 0,
				affected_by_movement_speed: false,
				action: RouteTimelineAction::Goal,
			}],
		};

		assert!(matches!(
			initial_route_facing(&timeline, 7),
			Err(ExpectedReplayAdapterError::MissingRouteFacing { route_index: 7 })
		));
	}

	#[test]
	fn stage_requirement_inventory_only_accepts_explicitly_handled_behaviors() {
		let supported_tiles: BTreeSet<&str> = ["tile_floor"].into_iter().collect();
		let route_origin = MechanicOrigin::RouteCheckpoint {
			extra: false,
			route: 3,
			checkpoint: 4,
		};
		assert!(stage_requirement_is_handled(
			MechanicKind::Tile,
			"tile_floor",
			&MechanicOrigin::MapCell {
				position: GridPosition { row: 0, col: 0 },
			},
			&supported_tiles,
		));
		assert!(stage_requirement_is_handled(
			MechanicKind::RouteCheckpoint,
			"APPEAR_AT_POS",
			&route_origin,
			&supported_tiles,
		));
		assert!(!stage_requirement_is_handled(
			MechanicKind::RouteCheckpoint,
			"WAIT_CURRENT_WAVE_TIME",
			&route_origin,
			&supported_tiles,
		));
		assert!(stage_requirement_is_handled(
			MechanicKind::Rune,
			"level_hidden_group_enable",
			&MechanicOrigin::Rune {
				optional_group: None,
				index: 2,
			},
			&supported_tiles,
		));
		assert!(!stage_requirement_is_handled(
			MechanicKind::Rune,
			"level_hidden_group_enable",
			&MechanicOrigin::Rune {
				optional_group: Some("contract".to_owned()),
				index: 0,
			},
			&supported_tiles,
		));
		assert!(!stage_requirement_is_handled(
			MechanicKind::GlobalBuffPrefab,
			"unknown_buff",
			&MechanicOrigin::GlobalBuff { index: 0 },
			&supported_tiles,
		));
	}

	#[test]
	fn manual_skill_is_reached_only_by_an_explicit_action_or_enabled_daemon() {
		let operator: OperatorInstanceId = OperatorInstanceId::new(7);
		let prepared =
			|actions: Vec<MaterializedGuideAction>, skill_usage: u8| PreparedReplayGuide {
				actions,
				policy: MaterializedGuidePolicy::new(BTreeMap::from([(operator, skill_usage)])),
				operator_ids: BTreeMap::new(),
			};
		let daemon_disabled: PreparedReplayGuide = prepared(
			vec![MaterializedGuideAction::SkillDaemon {
				requested_at: SimTick::new(0),
			}],
			0,
		);
		let daemon_enabled: PreparedReplayGuide = prepared(
			vec![MaterializedGuideAction::SkillDaemon {
				requested_at: SimTick::new(0),
			}],
			1,
		);
		let explicit: PreparedReplayGuide = prepared(
			vec![MaterializedGuideAction::ActivateSkill {
				requested_at: SimTick::new(30),
				operator,
			}],
			0,
		);

		assert!(!selected_skill_behavior_is_reached(
			&daemon_disabled,
			operator,
			&SkillActivationType::Manual
		));
		assert!(selected_skill_behavior_is_reached(
			&daemon_enabled,
			operator,
			&SkillActivationType::Manual
		));
		assert!(selected_skill_behavior_is_reached(
			&explicit,
			operator,
			&SkillActivationType::Manual
		));
		assert!(selected_skill_behavior_is_reached(
			&daemon_disabled,
			operator,
			&SkillActivationType::Auto
		));
	}

	/// A cadence multiplier only means anything if the engine, rounding a partial tick up, lands
	/// back on the interval it was built for. A candidate rounded up instead of down overshoots
	/// into the next tick and fails every ratio that is not exact in millionths.
	#[test]
	fn a_cadence_multiplier_reproduces_the_interval_it_was_built_for() {
		for (base_ticks, active_ticks) in [(48_u64, 26_u64), (48, 47), (30, 43), (16, 5)] {
			let multiplier: ExpectedValue =
				super::exact_interval_multiplier(base_ticks, active_ticks).unwrap_or_else(|| {
					panic!("{base_ticks} -> {active_ticks} must be representable")
				});
			assert_eq!(
				super::replayed_attack_interval(base_ticks, multiplier, 0),
				Some(active_ticks),
				"{base_ticks} -> {active_ticks} must survive the engine's own arithmetic",
			);
		}
	}

	/// A skill's active attack is built from two lanes that resolve in different buckets. The panel
	/// percentage joins the operator's other panel percentages; the damage scale stands alone.
	/// Fusing them into one factor let a panel talent stack multiplicatively against the skill.
	#[test]
	fn a_skills_attack_lanes_reach_their_own_buckets() {
		let skill_effects = |operator_id: &str,
		                     skill_id: &str,
		                     board: Vec<SkillDamageBlackboardFact>|
		 -> Vec<super::TaggedEffect> {
			let facts = SkillDamageFacts::new(
				operator_id.to_owned(),
				skill_id.to_owned(),
				Some(skill_id.to_owned()),
				false,
				"CASTER".to_owned(),
				"corecaster".to_owned(),
				OperatorPosition::Ranged,
				600.0,
				1.6,
				100.0,
				"base-range".to_owned(),
				None,
				SkillActivationType::Manual,
				SkillDurationType::None,
				30.0,
				SkillSpType::IncreaseWithTime,
				30,
				10,
				board,
			);
			let compiled = compile_expected_damage_skill_facts(&facts, tick_rate());
			let mut skill = replay_skill(operator_id, skill_id);
			let attack = replay_attack(48, DamageType::Arts, vec![RangeOffset::new(0, 0)]);
			assert!(register_selected_skill_behavior(
				operator_id,
				skill_id,
				&compiled,
				SelectedSkillBehaviorContext {
					skill: &mut skill,
					attack: &attack,
					base_attack: ExpectedValue::from_whole(600),
					active_range: Some(&attack.range),
					behavior_reached: true,
				},
			)
			.is_none());
			skill.effects
		};
		let attack_modifiers =
			|effects: &[super::TaggedEffect]| -> Vec<(StatModifierBucket, ExpectedValue)> {
				effects
					.iter()
					.filter_map(|effect| match &effect.effect {
						ExpectedEffect::StatModifier(modifier)
							if modifier.stat == super::StatKind::Attack =>
						{
							Some((modifier.bucket, modifier.multiplier))
						}
						_ => None,
					})
					.collect()
			};

		// 空's S2 raises the panel: "攻击力+50%". It shares the operator's attack bucket.
		let panel_only = skill_effects(
			"char_190_clour",
			"skchr_clour_2",
			vec![blackboard("atk", 0.5), blackboard("attack@max_target", 2.0)],
		);
		assert_eq!(
			attack_modifiers(&panel_only),
			vec![(
				StatModifierBucket::Panel,
				ExpectedValue::from_ratio(3, 2).expect("valid multiplier")
			)],
			"a skill that only raises the panel emits one panel modifier and no scale",
		);

		// 能天使's S2 never touches the panel: it fires more shots at a damage scale of its own,
		// so it stays a bucket apart from her 攻击力+8% talent and multiplies against it.
		let scale_only = skill_effects(
			"char_103_angel",
			"skchr_angel_2",
			vec![
				blackboard("attack@atk_scale", 1.25),
				blackboard("attack@times", 3.0),
			],
		);
		assert_eq!(
			attack_modifiers(&scale_only),
			vec![(
				StatModifierBucket::Scale,
				ExpectedValue::from_ratio(5, 4).expect("valid multiplier")
			)],
			"a skill that only scales its damage emits one standalone scale and no panel modifier",
		);
	}

	#[test]
	fn reached_unmodeled_skill_is_a_partial_omission_not_a_compile_blocker() {
		let facts = SkillDamageFacts::new(
			"char_unknown".to_owned(),
			"skill_unknown".to_owned(),
			None,
			false,
			"CASTER".to_owned(),
			"corecaster".to_owned(),
			OperatorPosition::Ranged,
			600.0,
			1.6,
			100.0,
			"base-range".to_owned(),
			None,
			SkillActivationType::Manual,
			SkillDurationType::None,
			30.0,
			SkillSpType::IncreaseWithTime,
			30,
			10,
			Vec::new(),
		);
		let compiled = compile_expected_damage_skill_facts(&facts, tick_rate());
		let mut skill = replay_skill("char_unknown", "skill_unknown");
		let attack = replay_attack(
			48,
			DamageType::Arts,
			vec![RangeOffset::new(0, 0), RangeOffset::new(0, 1)],
		);

		assert!(register_selected_skill_behavior(
			"char_unknown",
			"skill_unknown",
			&compiled,
			SelectedSkillBehaviorContext {
				skill: &mut skill,
				attack: &attack,
				base_attack: ExpectedValue::from_whole(600),
				active_range: Some(&attack.range),
				behavior_reached: false,
			},
		)
		.is_none());
		let omission = register_selected_skill_behavior(
			"char_unknown",
			"skill_unknown",
			&compiled,
			SelectedSkillBehaviorContext {
				skill: &mut skill,
				attack: &attack,
				base_attack: ExpectedValue::from_whole(600),
				active_range: Some(&attack.range),
				behavior_reached: true,
			},
		)
		.expect("a reached unknown skill must remain auditable");
		assert!(matches!(
			omission.behavior,
			MechanicBehavior::UnmodeledOmission { .. }
		));
	}

	#[test]
	fn reviewed_single_hit_skill_registers_replay_effects_without_generic_unsupported_marker() {
		let facts = SkillDamageFacts::new(
			"char_002_amiya".to_owned(),
			"skcom_magic_rage[3]".to_owned(),
			Some("skcom_attack_speed_up".to_owned()),
			false,
			"CASTER".to_owned(),
			"corecaster".to_owned(),
			OperatorPosition::Ranged,
			600.0,
			1.6,
			100.0,
			"base-range".to_owned(),
			None,
			SkillActivationType::Manual,
			SkillDurationType::None,
			30.0,
			SkillSpType::IncreaseWithTime,
			30,
			10,
			vec![blackboard("attack_speed", 90.0)],
		);
		let compiled = compile_expected_damage_skill_facts(&facts, tick_rate());
		let mut skill = replay_skill("char_002_amiya", "skcom_magic_rage[3]");
		let attack = replay_attack(
			48,
			DamageType::Arts,
			vec![RangeOffset::new(0, 0), RangeOffset::new(0, 1)],
		);

		let unsupported = register_selected_skill_behavior(
			"char_002_amiya",
			"skcom_magic_rage[3]",
			&compiled,
			SelectedSkillBehaviorContext {
				skill: &mut skill,
				attack: &attack,
				base_attack: ExpectedValue::from_whole(600),
				active_range: Some(&attack.range),
				behavior_reached: true,
			},
		);

		assert!(unsupported.is_none());
		// 阿米娅's S1 grants +90 attack-speed points and changes nothing else about the cadence,
		// so it lowers to the additive lane alone: no cadence multiplier is emitted, and the
		// points join every other source's before the interval follows from their sum.
		assert!(matches!(
			skill.effects.as_slice(),
			[super::TaggedEffect {
				effect: ExpectedEffect::AttackSpeed(modifier),
				..
			}] if modifier.delta == 90 && modifier.duration_ticks == Some(900)
		));
		// The reviewed cadence is unchanged by the move: 1.6s at 190 points is 26 ticks at 30 Hz,
		// and the engine's own arithmetic reproduces it from the base interval and the delta.
		assert_eq!(
			super::replayed_attack_interval(48, ExpectedValue::ONE, 90),
			Some(26)
		);
	}

	#[test]
	fn reviewed_multi_hit_skill_registers_per_hit_replay_modifier() {
		let facts = SkillDamageFacts::new(
			"char_103_angel".to_owned(),
			"skchr_angel_2".to_owned(),
			Some("skchr_angel_2".to_owned()),
			false,
			"SNIPER".to_owned(),
			"fastshot".to_owned(),
			OperatorPosition::Ranged,
			600.0,
			1.0,
			100.0,
			"3-3".to_owned(),
			None,
			SkillActivationType::Manual,
			SkillDurationType::None,
			15.0,
			SkillSpType::IncreaseWithTime,
			35,
			25,
			vec![
				blackboard("attack@atk_scale", 1.25),
				blackboard("attack@times", 4.0),
			],
		);
		let compiled = compile_expected_damage_skill_facts(&facts, tick_rate());
		let mut skill = replay_skill("char_103_angel", "skchr_angel_2");
		let attack = replay_attack(
			30,
			DamageType::Physical,
			vec![RangeOffset::new(0, 0), RangeOffset::new(0, 1)],
		);

		let unsupported = register_selected_skill_behavior(
			"char_103_angel",
			"skchr_angel_2",
			&compiled,
			SelectedSkillBehaviorContext {
				skill: &mut skill,
				attack: &attack,
				base_attack: ExpectedValue::from_whole(600),
				active_range: Some(&attack.range),
				behavior_reached: true,
			},
		);
		assert!(unsupported.is_none());
		assert!(matches!(
			skill.effects.as_slice(),
			[
				super::TaggedEffect {
					effect: ExpectedEffect::StatModifier(attack),
					..
				},
				super::TaggedEffect {
					effect: ExpectedEffect::HitCount(hits),
					..
				}
			] if attack.multiplier == ExpectedValue::from_ratio(5, 4).unwrap()
				&& attack.duration_ticks == Some(450)
				&& hits.delta == 3
				&& hits.duration_ticks == Some(450)
		));
		assert_eq!(skill.active_duration_ticks, Some(450));
	}

	/// Pins both halves of the executor-inert gate: the admitted set, and everything still closed.
	#[test]
	fn only_presentation_wave_actions_are_executor_inert() {
		let every_kind: [StageActionKind; 16] = [
			StageActionKind::Spawn,
			StageActionKind::PreviewCursor,
			StageActionKind::Story,
			StageActionKind::Tutorial,
			StageActionKind::PlayBgm,
			StageActionKind::DisplayEnemyInfo,
			StageActionKind::ActivatePredefined,
			StageActionKind::PlayOpera,
			StageActionKind::TriggerPredefined,
			StageActionKind::BattleEvents,
			StageActionKind::WithdrawPredefined,
			StageActionKind::Dialog,
			StageActionKind::ShowAllHiddenCards,
			StageActionKind::Empty,
			StageActionKind::EnumCount,
			StageActionKind::Unknown("SOMETHING_NEW".to_owned()),
		];
		let inert: Vec<&str> = every_kind
			.iter()
			.filter(|kind| action_is_executor_inert(kind))
			.map(|kind| kind.stable_id())
			.collect();
		assert_eq!(
			inert,
			[
				"PREVIEW_CURSOR",
				"STORY",
				"PLAY_BGM",
				"DISPLAY_ENEMY_INFO",
				"PLAY_OPERA",
				"DIALOG",
			]
		);
		// Every id in the constant must name a real kind, so the two cannot drift apart.
		for id in EXECUTOR_INERT_ACTION_IDS {
			assert!(
				every_kind.iter().any(|kind| kind.stable_id() == id),
				"{id} is not a StageActionKind"
			);
		}
	}
}

#[cfg(test)]
mod enemy_reach_tests {
	use super::{enemy_range_offsets, unestablished_ranged_reach, MechanicBehavior};

	const ENEMY: &str = "enemy_test";

	fn omission_reason(radius: Option<f64>) -> String {
		let reached = unestablished_ranged_reach(ENEMY, Some("RANGED"), radius)
			.unwrap_or_else(|| panic!("a ranged record with radius {radius:?} must be reported"));
		assert_eq!(reached.id.as_str(), "enemy_test/normal-attack/range");
		let MechanicBehavior::UnmodeledOmission { reason } = reached.behavior else {
			panic!(
				"an unexecutable reach is an omission, found {:?}",
				reached.behavior
			);
		};
		reason
	}

	#[test]
	fn a_ranged_record_without_an_established_radius_omits_its_reach() {
		// -1.0 is written into the tables on purpose, and nothing in them says what it resolves to.
		// Replay may not read it as "no reach" just because it is not a positive distance.
		assert!(omission_reason(Some(-1.0)).contains("states an attack radius of -1"));
		// A record whose level layers never set the field is a different state, and says so.
		assert!(omission_reason(None).contains("sets no attack radius"));
	}

	#[test]
	fn an_established_radius_and_a_melee_record_both_stay_silent() {
		assert!(unestablished_ranged_reach(ENEMY, Some("RANGED"), Some(1.75)).is_none());
		// A melee record reaching only its own cell is the reach it has, not a gap.
		assert!(unestablished_ranged_reach(ENEMY, Some("MELEE"), Some(-1.0)).is_none());
		assert!(unestablished_ranged_reach(ENEMY, Some("MELEE"), None).is_none());
	}

	#[test]
	fn an_unexecutable_radius_still_compiles_to_the_records_own_cell() {
		// The omission withholds the verdict; the base attack keeps executing, from one cell.
		let own_cell: Vec<crate::data::RangeOffset> = vec![crate::data::RangeOffset::new(0, 0)];
		assert_eq!(enemy_range_offsets(None).expect("no radius"), own_cell);
		assert_eq!(
			enemy_range_offsets(Some(-1.0)).expect("coded radius"),
			own_cell
		);
		assert_eq!(
			enemy_range_offsets(Some(0.0)).expect("zero radius"),
			own_cell
		);
		assert!(enemy_range_offsets(Some(1.75)).expect("real radius").len() > 1);
	}
}

#[cfg(test)]
mod enemy_behavior_tests {

	use crate::data::{BattlePrefabEntryKind, RangeOffset};

	use super::{
		compile_enemy_behavior, ApproximationTrigger, AttackPattern, AttackProfile, DamageDelivery,
		DamageType, DecodedPlainEnemyPrefab, EnemyCompilation, EnemyRecordFacts, MechanicBehavior,
		NormalAttackAction, NormalAttackTargetFilter, PlainEnemyAttackForm, PlainEnemyPrefabDecode,
		PlainEnemyPrefabDecodeError, ReachedMechanic, ReviewedEnemyDamageType, TaggedEffect,
		TargetClass, UndescribedEnemy,
	};
	use crate::mechanics::{
		domain::ProjectileMovementSchema,
		prefab::{BattlePrefabContainerMatch, BattlePrefabObjectSource},
	};

	const ENEMY: &str = "enemy_test";

	struct Compiled {
		attack: AttackProfile,
		reached: Vec<ReachedMechanic>,
	}

	fn base_attack() -> AttackProfile {
		AttackProfile {
			first_delay_ticks: 42,
			interval_ticks: 42,
			range: vec![RangeOffset::new(0, 0)],
			target_count: 1,
			target_filter: NormalAttackTargetFilter::GroundOnly,
			action: NormalAttackAction::Damage {
				damage_type: DamageType::Physical,
				delivery: DamageDelivery::Normal,
			},
			pattern: AttackPattern::Single,
			extra_effects: Vec::new(),
		}
	}

	fn undescribed(damage: ReviewedEnemyDamageType) -> UndescribedEnemy {
		UndescribedEnemy {
			normal_attack_damage_type: damage,
		}
	}

	fn location(
		logical_path: &str,
		bundle_kind: BattlePrefabEntryKind,
		cab_name: &str,
		path_id: i64,
	) -> BattlePrefabContainerMatch {
		BattlePrefabContainerMatch {
			logical_path: logical_path.to_owned(),
			bundle_kind,
			container_file_id: 0,
			root: BattlePrefabObjectSource {
				logical_path: logical_path.to_owned(),
				cab_name: cab_name.to_owned(),
				path_id,
			},
		}
	}

	fn decoded(form: PlainEnemyAttackForm) -> PlainEnemyPrefabDecode {
		PlainEnemyPrefabDecode::Decoded(Box::new(DecodedPlainEnemyPrefab {
			location: location(
				"enm_pfb_0",
				BattlePrefabEntryKind::Enemypack,
				"CAB-ENEMY",
				7,
			),
			form,
			damage: ReviewedEnemyDamageType::Arts,
			provenance: Vec::new(),
			consumed_issues: Vec::new(),
		}))
	}

	fn ranged() -> PlainEnemyAttackForm {
		PlainEnemyAttackForm::RangedProjectile {
			projectile_key: "projectile_test".to_owned(),
			projectile: location(
				"[uc]projectiles",
				BattlePrefabEntryKind::Common,
				"CAB-PROJECTILE",
				9,
			),
			movement: ProjectileMovementSchema::Paracurve,
			speed: 5.0,
		}
	}

	/// Runs the seam; a `None` stage panics if the seam consults it.
	fn compile(
		target_class: TargetClass,
		gate: Option<Result<UndescribedEnemy, String>>,
		prefab: Option<Result<PlainEnemyPrefabDecode, PlainEnemyPrefabDecodeError>>,
	) -> Compiled {
		let mut attack: AttackProfile = base_attack();
		let _on_spawn: Vec<TaggedEffect> = Vec::new();
		let mut reached: Vec<ReachedMechanic> = Vec::new();
		compile_enemy_behavior(
			EnemyRecordFacts {
				enemy_key: ENEMY,
				target_class,
			},
			|| gate.expect("the table gate must not be consulted"),
			|_: &UndescribedEnemy| prefab.expect("the battle prefab must not be consulted"),
			EnemyCompilation {
				attack: &mut attack,
				reached_mechanics: &mut reached,
			},
		)
		.expect("enemy behaviour compiles");
		Compiled { attack, reached }
	}

	fn only(compiled: &Compiled) -> (&str, &str, &MechanicBehavior) {
		let [mechanic] = compiled.reached.as_slice() else {
			panic!(
				"expected one reached mechanic, found {:?}",
				compiled.reached
			);
		};
		(
			mechanic.id.as_str(),
			mechanic.origin.as_str(),
			&mechanic.behavior,
		)
	}

	fn arts_damage() -> NormalAttackAction {
		NormalAttackAction::Damage {
			damage_type: DamageType::Arts,
			delivery: DamageDelivery::Normal,
		}
	}

	#[test]
	fn described_tables_are_unsupported_before_the_prefab_is_read() {
		let compiled: Compiled = compile(
			TargetClass::Ground,
			Some(Err("record declares 1 skill(s)".to_owned())),
			None,
		);

		assert_eq!(
			only(&compiled),
			(
				"enemy_test/prefab",
				"gamedata:enemy:enemy_test",
				&MechanicBehavior::Unsupported {
					reason: "record declares 1 skill(s)".to_owned(),
				}
			)
		);
		assert_eq!(compiled.attack, base_attack());
	}

	#[test]
	fn without_enemy_packs_the_unread_prefab_stays_an_omission() {
		let compiled: Compiled = compile(
			TargetClass::Ground,
			Some(Ok(undescribed(ReviewedEnemyDamageType::Arts))),
			Some(Ok(PlainEnemyPrefabDecode::NoEnemyPacks)),
		);

		let (id, origin, behavior) = only(&compiled);
		assert_eq!(
			(id, origin),
			("enemy_test/prefab", "undescribed-gamedata:enemy:enemy_test")
		);
		assert!(matches!(
			behavior,
			MechanicBehavior::UnmodeledOmission { .. }
		));
		assert_eq!(compiled.attack.action, arts_damage());
		assert_eq!(compiled.attack.pattern, AttackPattern::Single);
	}

	#[test]
	fn a_pinned_prefab_that_does_not_decode_is_unsupported_and_never_an_omission() {
		let error: PlainEnemyPrefabDecodeError = PlainEnemyPrefabDecodeError::EnemyPrefabNotFound {
			asset_path: "dyn/battle/prefabs/enemies/enemy_test.prefab".to_owned(),
		};
		let compiled: Compiled = compile(
			TargetClass::Ground,
			Some(Ok(undescribed(ReviewedEnemyDamageType::Arts))),
			Some(Err(error.clone())),
		);

		assert_eq!(
			only(&compiled),
			(
				"enemy_test/prefab",
				"verified-prefab:enemy:enemy_test",
				&MechanicBehavior::Unsupported {
					reason: error.to_string(),
				}
			)
		);
		assert_eq!(compiled.attack, base_attack());
	}

	#[test]
	fn a_decoded_melee_prefab_is_supported_with_its_base_single_target_attack() {
		let compiled: Compiled = compile(
			TargetClass::Ground,
			Some(Ok(undescribed(ReviewedEnemyDamageType::Arts))),
			Some(Ok(decoded(PlainEnemyAttackForm::Melee))),
		);

		assert_eq!(
			only(&compiled),
			(
				"enemy_test/prefab",
				"verified-prefab:enemy:enemy_test/bundle:enm_pfb_0/cab:CAB-ENEMY/path:7",
				&MechanicBehavior::Damage
			)
		);
		let mut expected: AttackProfile = base_attack();
		expected.action = arts_damage();
		assert_eq!(compiled.attack, expected);
	}

	#[test]
	fn a_decoded_ranged_prefab_lands_its_projectile_one_tick_after_the_attack() {
		let compiled: Compiled = compile(
			TargetClass::Ground,
			Some(Ok(undescribed(ReviewedEnemyDamageType::Arts))),
			Some(Ok(decoded(ranged()))),
		);

		const PROJECTILE_ORIGIN: &str = concat!(
			"verified-prefab:enemy:enemy_test/bundle:enm_pfb_0/cab:CAB-ENEMY/path:7",
			"/projectile:projectile_test/cab:CAB-PROJECTILE/path:9"
		);
		// The executed damage is supported; the flight it lands after is the declared approximation,
		// inventoried beside it under its own stable ID.
		let [flight, damage] = compiled.reached.as_slice() else {
			panic!(
				"expected two reached mechanics, found {:?}",
				compiled.reached
			);
		};
		assert_eq!(damage.id.as_str(), "enemy_test/prefab");
		assert_eq!(damage.origin, PROJECTILE_ORIGIN);
		assert_eq!(damage.behavior, MechanicBehavior::Damage);
		assert_eq!(flight.id.as_str(), "enemy_test/prefab/projectile-flight");
		assert_eq!(flight.origin, PROJECTILE_ORIGIN);
		let MechanicBehavior::DeclaredApproximation {
			reason,
			exercised_by,
		} = &flight.behavior
		else {
			panic!("the flight must be a declared approximation, found {flight:?}");
		};
		assert!(
			reason.contains("serialized speed 5 is decoded but not executed"),
			"the reason must name the value replay declines to execute: {reason}"
		);
		assert_eq!(
			exercised_by,
			&ApproximationTrigger::EnemyNormalAttack {
				enemy_key: "enemy_test".to_owned()
			}
		);
		let mut expected: AttackProfile = base_attack();
		expected.action = arts_damage();
		expected.pattern = AttackPattern::Projectile { flight_ticks: 1 };
		assert_eq!(compiled.attack, expected);
	}

	#[test]
	fn a_decoded_prefab_off_a_ground_route_is_unsupported_and_leaves_the_attack_alone() {
		let compiled: Compiled = compile(
			TargetClass::Air,
			Some(Ok(undescribed(ReviewedEnemyDamageType::Arts))),
			Some(Ok(decoded(ranged()))),
		);

		let (id, _, behavior) = only(&compiled);
		assert_eq!(id, "enemy_test/prefab");
		assert!(matches!(
			behavior,
			MechanicBehavior::Unsupported { reason } if reason.contains("ground routes")
		));
		assert_eq!(compiled.attack, base_attack());
	}
}

#[cfg(test)]
mod talent_root_claim_tests {
	use std::num::NonZeroU32;

	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, ResolvedOperatorBuild,
	};
	use crate::simulation::TickRate;
	use serde_json::json;

	use super::{
		claim_talent_root, reviewed_roster_facts, ReviewedRosterFacts, TalentRootClaim,
		TalentRootClaimContext,
	};
	use crate::mechanics::{
		compile::{built_in_battle_prefab_handler_registry, interpret_battle_prefab},
		domain::{
			ExpectedEffect, MechanicBehavior, ReachedMechanic, SummonTemplateCatalog, TaggedEffect,
		},
		prefab::{BattlePrefabGraph, BattlePrefabRootSection},
	};

	struct LocalSnapshots {
		snapshot: ArkDataSnapshot,
		prefabs: BattlePrefabSnapshot,
	}

	fn local_snapshots() -> LocalSnapshots {
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&crate::test_support::configured_gamedata_dir())
				.expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&crate::test_support::configured_battle_prefab_dir())
				.expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		LocalSnapshots { snapshot, prefabs }
	}

	fn maximal(local: &LocalSnapshots, operator_id: &str) -> ResolvedOperatorBuild {
		let build: OperatorBuild = local
			.snapshot
			.operators()
			.maximal_builds(operator_id, 1)
			.expect("maximal builds resolve")
			.into_iter()
			.next()
			.expect("one maximal build");
		local
			.snapshot
			.operators()
			.resolve(&build)
			.expect("resolve maximal build")
	}

	fn talent_root(local: &LocalSnapshots, operator_id: &str) -> BattlePrefabGraph {
		interpret_battle_prefab(
			&local.prefabs,
			operator_id,
			BattlePrefabRootSection::Talents,
			"1",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret talent root 1")
	}

	/// The claim for talent root 1, with every effect and mechanic it emitted.
	fn claim(
		local: &LocalSnapshots,
		resolved: &ResolvedOperatorBuild,
		graph: &BattlePrefabGraph,
	) -> (TalentRootClaim, Vec<TaggedEffect>, Vec<ReachedMechanic>) {
		let roster_facts: ReviewedRosterFacts =
			reviewed_roster_facts(&local.snapshot, &[]).expect("empty roster facts");
		let candidate = resolved
			.selected_talent_candidates()
			.find(|candidate| candidate.prefab_key() == Some("1"))
			.expect("active talent 1");
		let mut on_battle_start: Vec<TaggedEffect> = Vec::new();
		let mut on_deploy: Vec<TaggedEffect> = Vec::new();
		let mut mechanics: Vec<ReachedMechanic> = Vec::new();
		let claim: TalentRootClaim = claim_talent_root(TalentRootClaimContext {
			graph,
			candidate,
			snapshot: &local.snapshot,
			resolved,
			roster_facts: &roster_facts,
			tick_rate: TickRate::new(NonZeroU32::new(30).expect("30 is nonzero")),
			summon_catalog: &SummonTemplateCatalog::new(),
			root_id: "effect/0/1",
			origin: "fixture-origin",
			on_battle_start: &mut on_battle_start,
			on_deploy: &mut on_deploy,
			mechanics: &mut mechanics,
		});
		assert!(on_battle_start.is_empty());
		(claim, on_deploy, mechanics)
	}

	/// A root a template recognizes stays claimed when it drifts, so its drift is Unsupported
	/// instead of falling back to the unclaimed root's omission.
	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn a_drifted_block_toggle_root_stays_claimed_and_unsupported() {
		let local: LocalSnapshots = local_snapshots();
		let courier: ResolvedOperatorBuild = maximal(&local, "char_198_blackd");
		let graph: BattlePrefabGraph = talent_root(&local, "char_198_blackd");

		let (intact, intact_effects, intact_mechanics) = claim(&local, &courier, &graph);
		assert!(intact.claimed);
		assert_eq!(intact.consumed_issues.len(), graph.issues.len());
		assert!(graph
			.issues
			.iter()
			.all(|issue| intact.consumed_issues.contains(issue)));
		assert!(matches!(
			intact_effects.as_slice(),
			[TaggedEffect {
				effect: ExpectedEffect::RegisterListener(_),
				..
			}]
		));
		assert_eq!(
			intact_mechanics
				.iter()
				.map(|mechanic| (mechanic.id.as_str(), &mechanic.behavior))
				.collect::<Vec<_>>(),
			[(
				"char_198_blackd/template/blackd_t_1/block-count-toggle",
				&MechanicBehavior::StatModifier
			)]
		);

		let mut drifted: BattlePrefabGraph = graph.clone();
		drifted
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.raw_fields.get("_isInverseToggle").is_some())
			.expect("buff toggle")
			.raw_fields["_isInverseToggle"] = json!(1);
		let (drift, drift_effects, drift_mechanics) = claim(&local, &courier, &drifted);
		assert!(drift.claimed);
		assert!(drift.consumed_issues.is_empty());
		assert!(drift_effects.is_empty());
		let [unsupported] = drift_mechanics.as_slice() else {
			panic!("{drift_mechanics:?}");
		};
		assert_eq!(
			unsupported.id.as_str(),
			"char_198_blackd/prefab/effect/0/1/template/block_toggle_decoding"
		);
		assert!(matches!(
			&unsupported.behavior,
			MechanicBehavior::Unsupported { reason } if reason.contains("$._isInverseToggle")
		));

		// A root no template recognizes is left unclaimed and emits nothing.
		let platinum: ResolvedOperatorBuild = maximal(&local, "char_204_platnm");
		let (unclaimed, unclaimed_effects, unclaimed_mechanics) =
			claim(&local, &platinum, &talent_root(&local, "char_204_platnm"));
		assert!(!unclaimed.claimed);
		assert!(unclaimed_effects.is_empty() && unclaimed_mechanics.is_empty());
	}
}
