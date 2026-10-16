use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::data::GridPosition;

use super::{
	full_battle_planning::{HpGroupId, SpaceTimeHpGroup, WaveLocalHpGroup, WaveLocalHpGroupId},
	query::{StageTimeAnchor, TickSpan},
	stage_space_time::{
		EnemyTrajectory, RelativeEnemyTrajectory, StageSpaceTimeModel, StageSpaceTimeTemplate,
		TrajectoryClass,
	},
};
use crate::mechanics::{
	domain::{
		ArtsResistance, DamageAccounting, EnemyDamageState, EnemyDefenses, Facing, FixedDamage,
		FixedDamageError, NormalAttackApplicationError, NormalAttackProfile,
		NormalAttackTargetPriority, TargetClass,
	},
	stage::SpawnInstanceId,
};

/// How an active attack profile interacts with the operator's normal attack timer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActiveAttackCadence {
	/// Skill-modified normal attacks use the same timer as attacks outside the skill window.
	Shared,
	/// Activation and periodic packets run from each activation boundary independently.
	///
	/// This must only be selected from reviewed skill semantics; a zero first-attack delay alone
	/// does not establish that a mechanic is activation-triggered.
	ActivationTriggered,
}

/// A reviewed build and spatial placement ready for normal-attack coverage scoring.
///
/// ArkData resolution, deployment legality, and range projection intentionally happen before
/// this pure search core. One value represents a stable `(build, cell, facing)` choice; search
/// chooses its deployment tick.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCoverageCandidate {
	stable_build_id: String,
	operator_id: String,
	operator_name: String,
	deployment_cost: u32,
	deployment_cell: GridPosition,
	facing: Facing,
	attack_cells: BTreeSet<GridPosition>,
	normal_attack: NormalAttackProfile,
	active_attack: Option<NormalAttackProfile>,
	active_attack_cells: Option<BTreeSet<GridPosition>>,
	active_attack_cadence: ActiveAttackCadence,
}

impl ResolvedCoverageCandidate {
	#[allow(
		clippy::too_many_arguments,
		reason = "candidate resolution is an atomic reviewed build-and-placement record"
	)]
	pub fn new(
		stable_build_id: String,
		operator_id: String,
		operator_name: String,
		deployment_cost: u32,
		deployment_cell: GridPosition,
		facing: Facing,
		attack_cells: BTreeSet<GridPosition>,
		normal_attack: NormalAttackProfile,
	) -> Result<Self, CoverageSearchError> {
		if stable_build_id.trim().is_empty() {
			return Err(CoverageSearchError::BlankStableBuildId);
		}
		if operator_id.trim().is_empty() {
			return Err(CoverageSearchError::BlankOperatorId);
		}
		if operator_name.trim().is_empty() {
			return Err(CoverageSearchError::BlankOperatorName);
		}
		if attack_cells.is_empty() {
			return Err(CoverageSearchError::EmptyAttackRange {
				stable_build_id,
				deployment_cell,
				facing,
			});
		}
		if normal_attack.target_priority() != NormalAttackTargetPriority::Standard {
			return Err(CoverageSearchError::UnsupportedTargetPriority);
		}
		Ok(Self {
			stable_build_id,
			operator_id,
			operator_name,
			deployment_cost,
			deployment_cell,
			facing,
			attack_cells,
			normal_attack,
			active_attack: None,
			active_attack_cells: None,
			active_attack_cadence: ActiveAttackCadence::Shared,
		})
	}

	/// Adds a skill-modified normal attack profile on the operator's shared attack timer.
	#[must_use]
	pub fn with_active_attack(mut self, active_attack: NormalAttackProfile) -> Self {
		self.active_attack = Some(active_attack);
		self.active_attack_cadence = ActiveAttackCadence::Shared;
		self
	}

	/// Adds a reviewed activation-triggered or independently periodic attack profile.
	///
	/// Unlike [`Self::with_active_attack`], this profile may fire from an activation boundary
	/// without waiting for the normal attack timer.
	#[must_use]
	pub fn with_activation_triggered_active_attack(
		mut self,
		active_attack: NormalAttackProfile,
	) -> Self {
		self.active_attack = Some(active_attack);
		self.active_attack_cadence = ActiveAttackCadence::ActivationTriggered;
		self
	}

	/// Overrides spatial coverage only while the active attack profile is running.
	#[must_use]
	pub fn with_active_attack_cells(mut self, active_attack_cells: BTreeSet<GridPosition>) -> Self {
		self.active_attack_cells = Some(active_attack_cells);
		self
	}

	/// Overrides only the deployment cost recorded for scheduling and audit.
	///
	/// Attack stats and range remain those of the resolved build used to construct this candidate.
	#[must_use]
	pub const fn with_deployment_cost(mut self, deployment_cost: u32) -> Self {
		self.deployment_cost = deployment_cost;
		self
	}

	#[must_use]
	pub fn stable_build_id(&self) -> &str {
		&self.stable_build_id
	}

	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub fn operator_name(&self) -> &str {
		&self.operator_name
	}

	#[must_use]
	pub const fn deployment_cost(&self) -> u32 {
		self.deployment_cost
	}

	#[must_use]
	pub const fn deployment_cell(&self) -> GridPosition {
		self.deployment_cell
	}

	#[must_use]
	pub const fn facing(&self) -> Facing {
		self.facing
	}

	#[must_use]
	pub fn attack_cells(&self) -> &BTreeSet<GridPosition> {
		&self.attack_cells
	}

	#[must_use]
	pub fn active_attack_cells(&self) -> &BTreeSet<GridPosition> {
		self.active_attack_cells
			.as_ref()
			.unwrap_or(&self.attack_cells)
	}
}

/// Stable identity of one evaluated solution.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoverageSolutionKey {
	pub stable_build_id: String,
	pub deployment_cell: GridPosition,
	pub facing: Facing,
	pub deploy_tick: u64,
}

/// Exact HP coverage ratio. Comparisons use cross multiplication, never floating point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageRatio {
	pub credited: FixedDamage,
	pub required: FixedDamage,
}

impl CoverageRatio {
	#[must_use]
	pub fn millionths(self) -> u64 {
		if self.required.is_zero() {
			return 1_000_000;
		}
		let scaled: u128 = u128::from(self.credited.scaled()) * u128::from(1_000_000_u64);
		u64::try_from(scaled / u128::from(self.required.scaled()))
			.expect("a capped coverage ratio cannot exceed one million")
	}

	#[must_use]
	pub const fn is_complete(self) -> bool {
		self.credited.scaled() == self.required.scaled()
	}

	fn compare(self, other: Self) -> Ordering {
		let left: u128 = u128::from(self.credited.scaled()) * u128::from(other.required.scaled());
		let right: u128 = u128::from(other.credited.scaled()) * u128::from(self.required.scaled());
		left.cmp(&right)
	}
}

/// Per-enemy damage audit retained for duty and non-duty targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnemyCoverageScore {
	pub instance_id: SpawnInstanceId,
	pub is_duty_target: bool,
	pub deadline_tick: Option<u64>,
	pub max_hp: FixedDamage,
	pub remaining_hp: FixedDamage,
	pub damage: DamageAccounting,
	pub kill_tick: Option<u64>,
	pub damage_events: Vec<EnemyDamageEvent>,
}

/// Damage attributed to one locked normal attack at a precise half-open scoring tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnemyDamageEvent {
	pub tick: u64,
	pub raw: FixedDamage,
	pub credited: FixedDamage,
	pub overkill: FixedDamage,
	pub remaining_hp: FixedDamage,
}

/// Cumulative requirement for all duty enemies due no later than one deadline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CumulativeDeadlineCoverage {
	pub deadline_tick: u64,
	pub required_hp: FixedDamage,
	pub credited_damage: FixedDamage,
	pub remaining_hp: FixedDamage,
	pub coverage: CoverageRatio,
}

/// One fully resolved member of a joint coverage configuration.
///
/// Candidate resolution owns build review, legal placement, and projected range. This value adds
/// the chosen deployment tick; its [`CoverageSolutionKey`] is the stable member key used to merge
/// simultaneous attacks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCoverageDeployment<'a> {
	candidate: &'a ResolvedCoverageCandidate,
	deploy_tick: u64,
	attack_origin_tick: Option<u64>,
	active_spans: Vec<TickSpan>,
}

impl<'a> ResolvedCoverageDeployment<'a> {
	#[must_use]
	pub const fn new(candidate: &'a ResolvedCoverageCandidate, deploy_tick: u64) -> Self {
		Self {
			candidate,
			deploy_tick,
			attack_origin_tick: None,
			active_spans: Vec::new(),
		}
	}

	/// Restricts this member's attacks to one half-open active interval.
	///
	/// This is used by expected-damage planning to score a skill phase without pretending that
	/// its modified attack profile remains active for the whole deployment.
	#[cfg(test)]
	#[must_use]
	pub fn with_active_span(mut self, active_span: TickSpan) -> Self {
		self.active_spans = vec![active_span];
		self
	}

	/// Restricts attacks to multiple skill activation intervals.
	///
	/// Modified normal attacks preserve one timer across these boundaries. Explicit
	/// activation-triggered profiles and legacy skill-only profiles restart at each interval.
	#[must_use]
	pub fn with_active_spans(mut self, active_spans: Vec<TickSpan>) -> Self {
		self.active_spans = active_spans;
		self
	}

	#[cfg(test)]
	#[must_use]
	pub const fn deploy_tick(&self) -> u64 {
		self.deploy_tick
	}

	#[cfg(test)]
	#[must_use]
	pub fn active_spans(&self) -> &[TickSpan] {
		&self.active_spans
	}

	#[must_use]
	pub const fn attack_origin_tick(&self) -> u64 {
		match self.attack_origin_tick {
			Some(tick) => tick,
			None => self.deploy_tick,
		}
	}

	#[must_use]
	pub fn key(&self) -> CoverageSolutionKey {
		CoverageSolutionKey {
			stable_build_id: self.candidate.stable_build_id.clone(),
			deployment_cell: self.candidate.deployment_cell,
			facing: self.candidate.facing,
			deploy_tick: self.deploy_tick,
		}
	}
}

/// Complete input for scoring one already chosen multi-operator deployment configuration.
#[derive(Clone, Copy, Debug)]
pub struct JointCoverageScoreRequest<'a> {
	pub model: &'a StageSpaceTimeModel,
	pub deployments: &'a [ResolvedCoverageDeployment<'a>],
	pub target_cohort_ids: &'a BTreeSet<String>,
	pub duty_window: TickSpan,
}

/// Static input prepared once when scoring many schedules for one resolved candidate.
#[derive(Clone, Copy, Debug)]
pub struct OptimisticHpGroupCapacityPrepareRequest<'candidate, 'context> {
	pub model: &'context StageSpaceTimeModel,
	pub candidate: &'candidate ResolvedCoverageCandidate,
	pub hp_groups: &'context [SpaceTimeHpGroup],
	pub duty_window: TickSpan,
}

/// Optimistic total damage output keyed by cumulative unblocked goal-deadline group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimisticHpGroupScore {
	pub capacity_by_group: BTreeMap<HpGroupId, FixedDamage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparedTickCapacitySegment {
	span: TickSpan,
	capacity: FixedDamage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedTickCapacity {
	segments: Vec<PreparedTickCapacitySegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedOptimisticHpGroupCapacity {
	group_id: HpGroupId,
	deadline_tick: u64,
	normal: PreparedTickCapacity,
	active: Option<PreparedTickCapacity>,
}

/// Cached piecewise HP-group capacities for many legal schedules of one candidate.
///
/// [`Self::score`] preserves half-open duty windows, active-span validation, and attack cadence
/// while avoiding repeated spatial and defense calculations for every schedule.
#[derive(Clone, Debug)]
pub struct PreparedOptimisticHpGroupCapacityScorer<'candidate> {
	candidate: &'candidate ResolvedCoverageCandidate,
	duty_window: TickSpan,
	groups: Vec<PreparedOptimisticHpGroupCapacity>,
}

/// Input for a causal, wave-local hard-pruning capacity bound.
///
/// Deployment and activation ticks are intentionally absent from the semantics: later wave
/// starts depend on predecessor clearance, and blocking can leave their latest ticks unbounded.
/// Each deployment contributes only its reviewed static range, target filter, and damage types.
#[derive(Clone, Copy, Debug)]
pub struct OptimisticWaveLocalReachabilityScoreRequest<'a> {
	pub template: &'a StageSpaceTimeTemplate,
	pub deployments: &'a [ResolvedCoverageDeployment<'a>],
	pub hp_groups: &'a [WaveLocalHpGroup],
}

/// Full-HP reachability capacity keyed by an event-relative wave-local requirement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimisticWaveLocalReachabilityScore {
	pub capacity_by_group: BTreeMap<WaveLocalHpGroupId, FixedDamage>,
}

/// Damage from one member's locked normal attack, attributed to its selected enemy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributedEnemyDamageEvent {
	pub attack_index: u64,
	pub instance_id: SpawnInstanceId,
	pub tick: u64,
	pub raw: FixedDamage,
	pub credited: FixedDamage,
	pub overkill: FixedDamage,
	pub remaining_hp: FixedDamage,
}

/// Per-member audit from a joint deployment simulation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JointMemberCoverageScore {
	pub key: CoverageSolutionKey,
	pub operator_id: String,
	pub operator_name: String,
	pub deployment_cost: u32,
	pub attack_origin_tick: u64,
	pub active_spans: Vec<TickSpan>,
	pub attacks_with_targets: u64,
	pub damage: DamageAccounting,
	pub damage_events: Vec<AttributedEnemyDamageEvent>,
}

/// Shared-enemy coverage result for one multi-operator deployment configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JointCoverageScore {
	pub member_scores: Vec<JointMemberCoverageScore>,
	pub enemy_scores: Vec<EnemyCoverageScore>,
	pub cumulative_deadlines: Vec<CumulativeDeadlineCoverage>,
	pub minimum_cumulative_coverage: CoverageRatio,
	pub total_due_remaining_hp: FixedDamage,
	pub clearance_slack_ticks: Option<u64>,
	/// This only passes the declared deterministic expected-damage/base-panel attack model.
	pub passed_declared_expected_damage_model: bool,
}

impl JointCoverageScore {
	#[must_use]
	pub const fn is_feasible(&self) -> bool {
		self.passed_declared_expected_damage_model
	}
}

#[derive(Clone, Debug)]
struct SimulatedEnemy {
	model_index: usize,
	damage: EnemyDamageState<SpawnInstanceId>,
	kill_tick: Option<u64>,
	damage_events: Vec<EnemyDamageEvent>,
}

#[derive(Clone, Debug)]
struct AppliedEnemyDamage {
	instance_id: SpawnInstanceId,
	event: EnemyDamageEvent,
}

#[derive(Clone, Debug)]
struct AppliedNormalAttack {
	accounting: DamageAccounting,
	enemies: Vec<AppliedEnemyDamage>,
}

fn initialize_enemies(
	model: &StageSpaceTimeModel,
) -> Result<Vec<SimulatedEnemy>, CoverageSearchError> {
	let mut enemies: Vec<SimulatedEnemy> = model
		.enemies
		.iter()
		.enumerate()
		.map(|(model_index, enemy)| {
			let maximum_hp: FixedDamage =
				FixedDamage::from_whole(enemy.max_hp).map_err(CoverageSearchError::Arithmetic)?;
			let physical_defense: FixedDamage =
				FixedDamage::from_whole(enemy.defense).map_err(CoverageSearchError::Arithmetic)?;
			let arts_resistance: ArtsResistance =
				ArtsResistance::try_from_percent_f64(enemy.magic_resistance).map_err(|_| {
					CoverageSearchError::InvalidArtsResistance {
						instance_id: enemy.instance_id.clone(),
						resistance: enemy.magic_resistance,
					}
				})?;
			Ok(SimulatedEnemy {
				model_index,
				damage: EnemyDamageState::new(
					enemy.instance_id.clone(),
					target_class(enemy.target_class),
					maximum_hp,
					EnemyDefenses::new(physical_defense, arts_resistance),
				),
				kill_tick: None,
				damage_events: Vec::new(),
			})
		})
		.collect::<Result<_, CoverageSearchError>>()?;
	enemies.sort_by(|left: &SimulatedEnemy, right: &SimulatedEnemy| {
		left.damage.id().cmp(right.damage.id())
	});
	Ok(enemies)
}

fn apply_normal_attack(
	model: &StageSpaceTimeModel,
	profile: &NormalAttackProfile,
	attack_cells: &BTreeSet<GridPosition>,
	enemies: &mut [SimulatedEnemy],
	attack_tick: u64,
) -> Result<AppliedNormalAttack, CoverageSearchError> {
	let mut eligible: Vec<(u64, SpawnInstanceId, usize)> = enemies
		.iter()
		.enumerate()
		.filter_map(|(state_index, state): (usize, &SimulatedEnemy)| {
			if state.damage.remaining_hp().is_zero() {
				return None;
			}
			let trajectory = &model.enemies[state.model_index];
			if attack_tick >= trajectory.goal_deadline_tick
				|| !profile.target_filter().allows(state.damage.target_class())
			{
				return None;
			}
			let occupancy = trajectory.occupancy_at(attack_tick)?;
			attack_cells.contains(&occupancy.cell).then(|| {
				(
					occupancy.remaining_route_distance_microunits,
					state.damage.id().clone(),
					state_index,
				)
			})
		})
		.collect();
	eligible.sort_by(
		|(left_distance, left_id, _), (right_distance, right_id, _)| {
			left_distance
				.cmp(right_distance)
				.then_with(|| left_id.cmp(right_id))
		},
	);
	eligible.truncate(profile.target_count().get() as usize);
	if eligible.is_empty() {
		return Ok(AppliedNormalAttack {
			accounting: DamageAccounting::ZERO,
			enemies: Vec::new(),
		});
	}

	let selected_indices: Vec<usize> = eligible
		.iter()
		.map(|(_, _, state_index)| *state_index)
		.collect();
	let mut selected: Vec<EnemyDamageState<SpawnInstanceId>> = selected_indices
		.iter()
		.map(|state_index: &usize| enemies[*state_index].damage.clone())
		.collect();
	let attack_report = profile
		.apply_locked_targets(&mut selected)
		.map_err(CoverageSearchError::AttackApplication)?;
	let mut applied_enemies: Vec<AppliedEnemyDamage> = Vec::with_capacity(selected.len());
	for (state_index, next) in selected_indices.into_iter().zip(selected) {
		let was_alive: bool = !enemies[state_index].damage.remaining_hp().is_zero();
		let was_killed: bool = was_alive && next.remaining_hp().is_zero();
		let previous_accounting: DamageAccounting = enemies[state_index].damage.accounting();
		let next_accounting: DamageAccounting = next.accounting();
		let event: EnemyDamageEvent = EnemyDamageEvent {
			tick: attack_tick,
			raw: next_accounting
				.raw()
				.checked_sub(previous_accounting.raw())
				.map_err(CoverageSearchError::Arithmetic)?,
			credited: next_accounting
				.credited()
				.checked_sub(previous_accounting.credited())
				.map_err(CoverageSearchError::Arithmetic)?,
			overkill: next_accounting
				.overkill()
				.checked_sub(previous_accounting.overkill())
				.map_err(CoverageSearchError::Arithmetic)?,
			remaining_hp: next.remaining_hp(),
		};
		enemies[state_index].damage = next;
		enemies[state_index].damage_events.push(event);
		if was_killed {
			enemies[state_index].kill_tick = Some(attack_tick);
		}
		applied_enemies.push(AppliedEnemyDamage {
			instance_id: enemies[state_index].damage.id().clone(),
			event,
		});
	}
	Ok(AppliedNormalAttack {
		accounting: attack_report.accounting(),
		enemies: applied_enemies,
	})
}

fn build_enemy_scores<Deadline>(
	model: &StageSpaceTimeModel,
	enemies: Vec<SimulatedEnemy>,
	target_cohort_ids: &BTreeSet<String>,
	mut deadline_for: Deadline,
) -> Vec<EnemyCoverageScore>
where
	Deadline: FnMut(&EnemyTrajectory) -> u64,
{
	let mut enemy_scores: Vec<EnemyCoverageScore> = Vec::with_capacity(enemies.len());
	for enemy in enemies {
		let trajectory: &EnemyTrajectory = &model.enemies[enemy.model_index];
		let is_duty_target: bool =
			target_cohort_ids.contains(trajectory.instance_id.cohort.as_str());
		let deadline_tick: Option<u64> = is_duty_target.then(|| deadline_for(trajectory));
		enemy_scores.push(EnemyCoverageScore {
			instance_id: trajectory.instance_id.clone(),
			is_duty_target,
			deadline_tick,
			max_hp: enemy.damage.max_hp(),
			remaining_hp: enemy.damage.remaining_hp(),
			damage: enemy.damage.accounting(),
			kill_tick: enemy.kill_tick,
			damage_events: enemy.damage_events,
		});
	}
	enemy_scores.sort_by(|left: &EnemyCoverageScore, right: &EnemyCoverageScore| {
		left.instance_id.cmp(&right.instance_id)
	});
	enemy_scores
}

#[derive(Clone, Debug)]
struct ResponsibilityCoverage {
	cumulative_deadlines: Vec<CumulativeDeadlineCoverage>,
	minimum_cumulative_coverage: CoverageRatio,
	total_due_remaining_hp: FixedDamage,
	clearance_slack_ticks: Option<u64>,
	passed: bool,
}

fn summarize_responsibility(
	enemy_scores: &[EnemyCoverageScore],
) -> Result<ResponsibilityCoverage, CoverageSearchError> {
	let cumulative_deadlines: Vec<CumulativeDeadlineCoverage> = cumulative_deadlines(enemy_scores)?;
	let minimum_cumulative_coverage: CoverageRatio = cumulative_deadlines
		.iter()
		.map(|deadline: &CumulativeDeadlineCoverage| deadline.coverage)
		.min_by(|left: &CoverageRatio, right: &CoverageRatio| left.compare(*right))
		.expect("validated requests include at least one target enemy");
	let total_due_remaining_hp: FixedDamage = cumulative_deadlines
		.last()
		.expect("validated requests include at least one target enemy")
		.remaining_hp;
	let passed: bool =
		minimum_cumulative_coverage.is_complete() && total_due_remaining_hp.is_zero();
	let clearance_slack_ticks: Option<u64> = passed.then(|| {
		enemy_scores
			.iter()
			.filter(|enemy: &&EnemyCoverageScore| enemy.is_duty_target)
			.map(|enemy: &EnemyCoverageScore| {
				let deadline: u64 = enemy
					.deadline_tick
					.expect("duty targets always have a deadline");
				let kill_tick: u64 = enemy
					.kill_tick
					.expect("passed duty targets are always killed");
				deadline.saturating_sub(kill_tick)
			})
			.min()
			.expect("validated requests include at least one target enemy")
	});
	Ok(ResponsibilityCoverage {
		cumulative_deadlines,
		minimum_cumulative_coverage,
		total_due_remaining_hp,
		clearance_slack_ticks,
		passed,
	})
}

#[derive(Clone, Debug)]
struct JointMemberSimulation<'a> {
	deployment: ResolvedCoverageDeployment<'a>,
	key: CoverageSolutionKey,
	attacks_with_targets: u64,
	damage: DamageAccounting,
	damage_events: Vec<AttributedEnemyDamageEvent>,
}

#[derive(Clone, Debug)]
struct ScheduledJointAttack {
	tick: u64,
	member_key: CoverageSolutionKey,
	attack_index: u64,
	member_index: usize,
	uses_active_profile: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScheduledMemberAttack {
	tick: u64,
	attack_index: u64,
	uses_active_profile: bool,
}

fn schedule_member_attacks(
	member_index: usize,
	member: &JointMemberSimulation<'_>,
	duty_window: TickSpan,
	scheduled_attacks: &mut Vec<ScheduledJointAttack>,
) -> Result<(), CoverageSearchError> {
	visit_deployment_attacks(&member.deployment, duty_window, |scheduled| {
		scheduled_attacks.push(ScheduledJointAttack {
			tick: scheduled.tick,
			member_key: member.key.clone(),
			attack_index: scheduled.attack_index,
			member_index,
			uses_active_profile: scheduled.uses_active_profile,
		});
	})
}

fn visit_deployment_attacks(
	deployment: &ResolvedCoverageDeployment<'_>,
	duty_window: TickSpan,
	mut visitor: impl FnMut(ScheduledMemberAttack),
) -> Result<(), CoverageSearchError> {
	let candidate: &ResolvedCoverageCandidate = deployment.candidate;
	match candidate.active_attack.as_ref() {
		Some(active_attack) if candidate.active_attack_cadence == ActiveAttackCadence::Shared => {
			schedule_shared_cadence_attacks(deployment, active_attack, duty_window, &mut visitor)
		}
		Some(active_attack) => {
			let mut attack_index: u64 = 0;
			schedule_unrestricted_attacks(
				deployment,
				&candidate.normal_attack,
				duty_window,
				&deployment.active_spans,
				false,
				&mut attack_index,
				&mut visitor,
			)?;
			schedule_restarted_span_attacks(
				active_attack,
				duty_window,
				&deployment.active_spans,
				true,
				&mut attack_index,
				&mut visitor,
			)
		}
		None if deployment.active_spans.is_empty() => {
			let mut attack_index: u64 = 0;
			schedule_unrestricted_attacks(
				deployment,
				&candidate.normal_attack,
				duty_window,
				&[],
				false,
				&mut attack_index,
				&mut visitor,
			)
		}
		None => {
			let mut attack_index: u64 = 0;
			schedule_restarted_span_attacks(
				&candidate.normal_attack,
				duty_window,
				&deployment.active_spans,
				false,
				&mut attack_index,
				&mut visitor,
			)
		}
	}
}

/// Schedules modified normal attacks on one timer that survives skill transitions.
///
/// An attack arms the timer using the interval of the profile that produced that attack. Merely
/// entering or leaving an active span never creates another attack or resets the timer.
fn schedule_shared_cadence_attacks(
	deployment: &ResolvedCoverageDeployment<'_>,
	active_attack: &NormalAttackProfile,
	duty_window: TickSpan,
	visitor: &mut impl FnMut(ScheduledMemberAttack),
) -> Result<(), CoverageSearchError> {
	let normal_attack: &NormalAttackProfile = &deployment.candidate.normal_attack;
	let Some(mut tick) = normal_attack.checked_attack_tick(deployment.attack_origin_tick(), 0)
	else {
		return Ok(());
	};
	let mut attack_index: u64 = 0;
	while tick < duty_window.end {
		let uses_active_profile: bool = deployment
			.active_spans
			.iter()
			.any(|span| span.contains(tick));
		if tick >= duty_window.start {
			visitor(ScheduledMemberAttack {
				tick,
				attack_index,
				uses_active_profile,
			});
		}
		attack_index = attack_index
			.checked_add(1)
			.ok_or(CoverageSearchError::ConfigurationCountOverflow)?;
		let interval_ticks: u64 = if uses_active_profile {
			active_attack.attack_interval_ticks().get()
		} else {
			normal_attack.attack_interval_ticks().get()
		};
		let Some(next_tick) = tick.checked_add(interval_ticks) else {
			break;
		};
		tick = next_tick;
	}
	Ok(())
}

fn schedule_unrestricted_attacks(
	deployment: &ResolvedCoverageDeployment<'_>,
	profile: &NormalAttackProfile,
	duty_window: TickSpan,
	suppressed_spans: &[TickSpan],
	uses_active_profile: bool,
	attack_index: &mut u64,
	visitor: &mut impl FnMut(ScheduledMemberAttack),
) -> Result<(), CoverageSearchError> {
	let mut phase_attack_index: u64 = 0;
	while let Some(tick) =
		profile.checked_attack_tick(deployment.attack_origin_tick(), phase_attack_index)
	{
		if tick >= duty_window.end {
			break;
		}
		let suppressed: bool = suppressed_spans.iter().any(|span| span.contains(tick));
		if tick >= duty_window.start && !suppressed {
			visitor(ScheduledMemberAttack {
				tick,
				attack_index: *attack_index,
				uses_active_profile,
			});
		}
		phase_attack_index = phase_attack_index
			.checked_add(1)
			.ok_or(CoverageSearchError::ConfigurationCountOverflow)?;
		*attack_index = attack_index
			.checked_add(1)
			.ok_or(CoverageSearchError::ConfigurationCountOverflow)?;
	}
	Ok(())
}

fn schedule_restarted_span_attacks(
	profile: &NormalAttackProfile,
	duty_window: TickSpan,
	active_spans: &[TickSpan],
	uses_active_profile: bool,
	attack_index: &mut u64,
	visitor: &mut impl FnMut(ScheduledMemberAttack),
) -> Result<(), CoverageSearchError> {
	for active_span in active_spans {
		let mut phase_attack_index: u64 = 0;
		while let Some(tick) = profile.checked_attack_tick(active_span.start, phase_attack_index) {
			if tick >= duty_window.end || tick >= active_span.end {
				break;
			}
			if tick >= duty_window.start {
				visitor(ScheduledMemberAttack {
					tick,
					attack_index: *attack_index,
					uses_active_profile,
				});
			}
			phase_attack_index = phase_attack_index
				.checked_add(1)
				.ok_or(CoverageSearchError::ConfigurationCountOverflow)?;
			*attack_index = attack_index
				.checked_add(1)
				.ok_or(CoverageSearchError::ConfigurationCountOverflow)?;
		}
	}
	Ok(())
}

fn validate_joint_request(
	request: &JointCoverageScoreRequest<'_>,
) -> Result<(), CoverageSearchError> {
	if request.deployments.is_empty() {
		return Err(CoverageSearchError::EmptyDeploymentSet);
	}
	if request.target_cohort_ids.is_empty() {
		return Err(CoverageSearchError::EmptyTargetCohorts);
	}
	if request.duty_window.start >= request.duty_window.end {
		return Err(CoverageSearchError::EmptyTickSpan {
			kind: "duty",
			start: request.duty_window.start,
			end: request.duty_window.end,
		});
	}

	let available_cohorts: BTreeSet<&str> = request
		.model
		.enemies
		.iter()
		.map(|enemy| enemy.instance_id.cohort.as_str())
		.collect();
	for cohort_id in request.target_cohort_ids {
		if !available_cohorts.contains(cohort_id.as_str()) {
			return Err(CoverageSearchError::UnknownTargetCohort {
				cohort_id: cohort_id.clone(),
			});
		}
	}

	let mut operator_ids: BTreeSet<&str> = BTreeSet::new();
	let mut deployment_cells: BTreeSet<GridPosition> = BTreeSet::new();
	let mut member_keys: BTreeSet<CoverageSolutionKey> = BTreeSet::new();
	for deployment in request.deployments {
		for active_span in &deployment.active_spans {
			if active_span.start >= active_span.end {
				return Err(CoverageSearchError::EmptyTickSpan {
					kind: "deployment activity",
					start: active_span.start,
					end: active_span.end,
				});
			}
			if active_span.start < deployment.deploy_tick {
				return Err(CoverageSearchError::ActivityBeforeDeployment {
					stable_build_id: deployment.candidate.stable_build_id.clone(),
					deploy_tick: deployment.deploy_tick,
					activity_start: active_span.start,
				});
			}
		}
		for pair in deployment.active_spans.windows(2) {
			let [left, right] = pair else {
				unreachable!("slice windows of two always contain two values");
			};
			if left.end > right.start {
				return Err(CoverageSearchError::OverlappingOrUnsortedActivitySpans {
					stable_build_id: deployment.candidate.stable_build_id.clone(),
					left: *left,
					right: *right,
				});
			}
		}
		let operator_id: &str = deployment.candidate.operator_id();
		if !operator_ids.insert(operator_id) {
			return Err(CoverageSearchError::DuplicateOperatorDeployment {
				operator_id: operator_id.to_owned(),
			});
		}
		let deployment_cell: GridPosition = deployment.candidate.deployment_cell();
		if !deployment_cells.insert(deployment_cell) {
			return Err(CoverageSearchError::DuplicateDeploymentCell { deployment_cell });
		}
		let key: CoverageSolutionKey = deployment.key();
		if !member_keys.insert(key.clone()) {
			return Err(CoverageSearchError::DuplicateDeploymentMember { key });
		}
	}

	let mut enemy_ids: BTreeSet<&SpawnInstanceId> = BTreeSet::new();
	for enemy in &request.model.enemies {
		if !enemy_ids.insert(&enemy.instance_id) {
			return Err(CoverageSearchError::DuplicateEnemyInstance {
				instance_id: enemy.instance_id.clone(),
			});
		}
	}
	Ok(())
}

fn validate_optimistic_hp_group_context(
	model: &StageSpaceTimeModel,
	hp_groups: &[SpaceTimeHpGroup],
	duty_window: TickSpan,
) -> Result<(), CoverageSearchError> {
	if hp_groups.is_empty() {
		return Err(CoverageSearchError::EmptyHpGroups);
	}
	if duty_window.start >= duty_window.end {
		return Err(CoverageSearchError::EmptyTickSpan {
			kind: "duty",
			start: duty_window.start,
			end: duty_window.end,
		});
	}

	let mut enemy_ids: BTreeSet<&SpawnInstanceId> = BTreeSet::new();
	for enemy in &model.enemies {
		if !enemy_ids.insert(&enemy.instance_id) {
			return Err(CoverageSearchError::DuplicateEnemyInstance {
				instance_id: enemy.instance_id.clone(),
			});
		}
	}

	let mut group_ids: BTreeSet<HpGroupId> = BTreeSet::new();
	for group in hp_groups {
		if !group_ids.insert(group.id) {
			return Err(CoverageSearchError::DuplicateHpGroup { group_id: group.id });
		}
		if group.id.deadline_tick() != group.deadline_tick {
			return Err(CoverageSearchError::HpGroupDeadlineMismatch {
				group_id: group.id,
				deadline_tick: group.deadline_tick,
			});
		}
		if group.deadline_tick > duty_window.end {
			return Err(CoverageSearchError::HpGroupDeadlineOutsideDuty {
				group_id: group.id,
				deadline_tick: group.deadline_tick,
				duty_window,
			});
		}
		if group.members.is_empty() {
			return Err(CoverageSearchError::EmptyHpGroup { group_id: group.id });
		}
		for member in &group.members {
			if !enemy_ids.contains(member) {
				return Err(CoverageSearchError::UnknownHpGroupMember {
					group_id: group.id,
					instance_id: member.clone(),
				});
			}
		}
	}
	Ok(())
}

fn validate_optimistic_deployments(
	deployments: &[ResolvedCoverageDeployment<'_>],
) -> Result<(), CoverageSearchError> {
	for deployment in deployments {
		for active_span in &deployment.active_spans {
			if active_span.start >= active_span.end {
				return Err(CoverageSearchError::EmptyTickSpan {
					kind: "deployment activity",
					start: active_span.start,
					end: active_span.end,
				});
			}
			if active_span.start < deployment.deploy_tick {
				return Err(CoverageSearchError::ActivityBeforeDeployment {
					stable_build_id: deployment.candidate.stable_build_id.clone(),
					deploy_tick: deployment.deploy_tick,
					activity_start: active_span.start,
				});
			}
		}
		for pair in deployment.active_spans.windows(2) {
			let [left, right] = pair else {
				unreachable!("slice windows of two always contain two values");
			};
			if left.end > right.start {
				return Err(CoverageSearchError::OverlappingOrUnsortedActivitySpans {
					stable_build_id: deployment.candidate.stable_build_id.clone(),
					left: *left,
					right: *right,
				});
			}
		}
	}
	Ok(())
}

fn validate_wave_local_reachability_request(
	request: &OptimisticWaveLocalReachabilityScoreRequest<'_>,
) -> Result<(), CoverageSearchError> {
	if request.deployments.is_empty() {
		return Err(CoverageSearchError::EmptyDeploymentSet);
	}
	if request.hp_groups.is_empty() {
		return Err(CoverageSearchError::EmptyHpGroups);
	}

	let mut enemy_ids: BTreeSet<&SpawnInstanceId> = BTreeSet::new();
	for enemy in &request.template.enemies {
		if !enemy_ids.insert(&enemy.instance_id) {
			return Err(CoverageSearchError::DuplicateEnemyInstance {
				instance_id: enemy.instance_id.clone(),
			});
		}
	}

	let mut group_ids: BTreeSet<&WaveLocalHpGroupId> = BTreeSet::new();
	for group in request.hp_groups {
		if !group_ids.insert(&group.id) {
			return Err(CoverageSearchError::DuplicateWaveLocalHpGroup {
				group_id: group.id.clone(),
			});
		}
		if group.members.is_empty() {
			return Err(CoverageSearchError::EmptyWaveLocalHpGroup {
				group_id: group.id.clone(),
			});
		}
		for member in &group.members {
			let enemy: &RelativeEnemyTrajectory =
				request.template.enemy(member).ok_or_else(|| {
					CoverageSearchError::UnknownWaveLocalHpGroupMember {
						group_id: group.id.clone(),
						instance_id: member.clone(),
					}
				})?;
			if enemy.anchor != group.id.anchor
				|| enemy.goal_deadline_offset_ticks > group.id.goal_deadline_offset_ticks
			{
				return Err(CoverageSearchError::InvalidWaveLocalHpGroupMember {
					group_id: group.id.clone(),
					instance_id: member.clone(),
					enemy_anchor: enemy.anchor.clone(),
					enemy_goal_deadline_offset_ticks: enemy.goal_deadline_offset_ticks,
				});
			}
		}
	}
	Ok(())
}

fn trajectory_defenses(trajectory: &EnemyTrajectory) -> Result<EnemyDefenses, CoverageSearchError> {
	let physical_defense: FixedDamage =
		FixedDamage::from_whole(trajectory.defense).map_err(CoverageSearchError::Arithmetic)?;
	let arts_resistance: ArtsResistance =
		ArtsResistance::try_from_percent_f64(trajectory.magic_resistance).map_err(|_| {
			CoverageSearchError::InvalidArtsResistance {
				instance_id: trajectory.instance_id.clone(),
				resistance: trajectory.magic_resistance,
			}
		})?;
	Ok(EnemyDefenses::new(physical_defense, arts_resistance))
}

fn relative_trajectory_defenses(
	trajectory: &RelativeEnemyTrajectory,
) -> Result<EnemyDefenses, CoverageSearchError> {
	let physical_defense: FixedDamage =
		FixedDamage::from_whole(trajectory.defense).map_err(CoverageSearchError::Arithmetic)?;
	let arts_resistance: ArtsResistance =
		ArtsResistance::try_from_percent_f64(trajectory.magic_resistance).map_err(|_| {
			CoverageSearchError::InvalidArtsResistance {
				instance_id: trajectory.instance_id.clone(),
				resistance: trajectory.magic_resistance,
			}
		})?;
	Ok(EnemyDefenses::new(physical_defense, arts_resistance))
}

#[derive(Clone, Copy, Debug)]
struct OptimisticEnemy<'a> {
	trajectory: &'a EnemyTrajectory,
	defenses: EnemyDefenses,
}

fn optimistic_damage_against(
	profile: &NormalAttackProfile,
	defenses: EnemyDefenses,
) -> Result<FixedDamage, CoverageSearchError> {
	profile
		.packets()
		.iter()
		.try_fold(FixedDamage::ZERO, |total, packet| {
			let damage: FixedDamage = packet
				.effective_damage(defenses)
				.map_err(CoverageSearchError::Arithmetic)?;
			total
				.checked_add(damage)
				.map_err(CoverageSearchError::Arithmetic)
		})
}

fn relative_route_ever_intersects(
	trajectory: &RelativeEnemyTrajectory,
	attack_cells: &BTreeSet<GridPosition>,
) -> bool {
	trajectory
		.occupancy
		.iter()
		.any(|occupancy| attack_cells.contains(&occupancy.cell))
}

fn optimistic_profile_reaches_relative_enemy(
	profile: &NormalAttackProfile,
	attack_cells: &BTreeSet<GridPosition>,
	enemy: &RelativeEnemyTrajectory,
) -> Result<bool, CoverageSearchError> {
	if !profile
		.target_filter()
		.allows(target_class(enemy.target_class))
		|| !relative_route_ever_intersects(enemy, attack_cells)
	{
		return Ok(false);
	}
	Ok(!optimistic_damage_against(profile, relative_trajectory_defenses(enemy)?)?.is_zero())
}

fn optimistic_candidate_reaches_relative_enemy(
	candidate: &ResolvedCoverageCandidate,
	enemy: &RelativeEnemyTrajectory,
) -> Result<bool, CoverageSearchError> {
	if optimistic_profile_reaches_relative_enemy(
		&candidate.normal_attack,
		&candidate.attack_cells,
		enemy,
	)? {
		return Ok(true);
	}
	let Some(active_attack) = candidate.active_attack.as_ref() else {
		return Ok(false);
	};
	optimistic_profile_reaches_relative_enemy(active_attack, candidate.active_attack_cells(), enemy)
}

fn optimistic_damage_by_enemy<'a>(
	profile: &NormalAttackProfile,
	enemies_by_id: &BTreeMap<&'a SpawnInstanceId, OptimisticEnemy<'a>>,
) -> Result<BTreeMap<&'a SpawnInstanceId, FixedDamage>, CoverageSearchError> {
	enemies_by_id
		.iter()
		.filter_map(|(instance_id, enemy)| {
			(profile
				.target_filter()
				.allows(target_class(enemy.trajectory.target_class)))
			.then_some((*instance_id, enemy.defenses))
		})
		.map(|(instance_id, defenses)| {
			optimistic_damage_against(profile, defenses).map(|damage| (instance_id, damage))
		})
		.collect()
}

fn optimistic_profile_capacity_by_group(
	profile: &NormalAttackProfile,
	attack_cells: &BTreeSet<GridPosition>,
	hp_groups: &[SpaceTimeHpGroup],
	enemies_by_id: &BTreeMap<&SpawnInstanceId, OptimisticEnemy<'_>>,
) -> Result<Vec<PreparedTickCapacity>, CoverageSearchError> {
	let damage_by_enemy: BTreeMap<&SpawnInstanceId, FixedDamage> =
		optimistic_damage_by_enemy(profile, enemies_by_id)?;
	let maximum_targets: usize = usize::try_from(profile.target_count().get())
		.expect("u32 target count fits supported usize");
	hp_groups
		.iter()
		.map(|group| {
			let mut boundaries: BTreeSet<u64> = BTreeSet::from([group.deadline_tick]);
			for member_id in &group.members {
				let Some(enemy) = enemies_by_id.get(member_id) else {
					continue;
				};
				if !damage_by_enemy.contains_key(member_id) {
					continue;
				}
				boundaries.insert(enemy.trajectory.goal_deadline_tick.min(group.deadline_tick));
				for occupancy in &enemy.trajectory.occupancy {
					if attack_cells.contains(&occupancy.cell)
						&& occupancy.enter_tick < group.deadline_tick
					{
						boundaries.insert(occupancy.enter_tick);
						boundaries.insert(occupancy.exit_tick.min(group.deadline_tick));
					}
				}
			}
			let boundary_ticks: Vec<u64> = boundaries.into_iter().collect();
			let mut segments: Vec<PreparedTickCapacitySegment> = Vec::new();
			for pair in boundary_ticks.windows(2) {
				let [start, end] = pair else {
					unreachable!("slice windows of two always contain two values");
				};
				if start >= end {
					continue;
				}
				let mut damage_by_target: Vec<FixedDamage> = group
					.members
					.iter()
					.filter_map(|member_id| {
						let enemy: &OptimisticEnemy<'_> = enemies_by_id.get(member_id)?;
						if *start >= enemy.trajectory.goal_deadline_tick {
							return None;
						}
						let occupancy = enemy.trajectory.occupancy_at(*start)?;
						attack_cells
							.contains(&occupancy.cell)
							.then(|| damage_by_enemy.get(member_id).copied())
							.flatten()
					})
					.collect();
				damage_by_target.sort_unstable_by(|left, right| right.cmp(left));
				damage_by_target.truncate(maximum_targets);
				let capacity: FixedDamage =
					damage_by_target
						.into_iter()
						.try_fold(FixedDamage::ZERO, |total, damage| {
							total
								.checked_add(damage)
								.map_err(CoverageSearchError::Arithmetic)
						})?;
				if !capacity.is_zero() {
					segments.push(PreparedTickCapacitySegment {
						span: TickSpan {
							start: *start,
							end: *end,
						},
						capacity,
					});
				}
			}
			Ok(PreparedTickCapacity { segments })
		})
		.collect()
}

impl PreparedTickCapacity {
	fn capacity_at(&self, tick: u64) -> FixedDamage {
		let index: usize = self
			.segments
			.partition_point(|segment| segment.span.start <= tick);
		index
			.checked_sub(1)
			.and_then(|index| self.segments.get(index))
			.filter(|segment| segment.span.contains(tick))
			.map_or(FixedDamage::ZERO, |segment| segment.capacity)
	}
}

fn optimistic_enemies_by_id(
	model: &StageSpaceTimeModel,
) -> Result<BTreeMap<&SpawnInstanceId, OptimisticEnemy<'_>>, CoverageSearchError> {
	model
		.enemies
		.iter()
		.map(|enemy| {
			Ok((
				&enemy.instance_id,
				OptimisticEnemy {
					trajectory: enemy,
					defenses: trajectory_defenses(enemy)?,
				},
			))
		})
		.collect()
}

/// Prepares the spatial and defense-dependent part of one candidate's optimistic score.
pub fn prepare_optimistic_hp_group_capacity<'candidate>(
	request: OptimisticHpGroupCapacityPrepareRequest<'candidate, '_>,
) -> Result<PreparedOptimisticHpGroupCapacityScorer<'candidate>, CoverageSearchError> {
	validate_optimistic_hp_group_context(request.model, request.hp_groups, request.duty_window)?;
	let enemies_by_id: BTreeMap<&SpawnInstanceId, OptimisticEnemy<'_>> =
		optimistic_enemies_by_id(request.model)?;
	let normal_by_group: Vec<PreparedTickCapacity> = optimistic_profile_capacity_by_group(
		&request.candidate.normal_attack,
		&request.candidate.attack_cells,
		request.hp_groups,
		&enemies_by_id,
	)?;
	let active_by_group: Option<Vec<PreparedTickCapacity>> = request
		.candidate
		.active_attack
		.as_ref()
		.map(|profile| {
			optimistic_profile_capacity_by_group(
				profile,
				request.candidate.active_attack_cells(),
				request.hp_groups,
				&enemies_by_id,
			)
		})
		.transpose()?;
	let groups: Vec<PreparedOptimisticHpGroupCapacity> = request
		.hp_groups
		.iter()
		.enumerate()
		.map(|(index, group)| PreparedOptimisticHpGroupCapacity {
			group_id: group.id,
			deadline_tick: group.deadline_tick,
			normal: normal_by_group[index].clone(),
			active: active_by_group
				.as_ref()
				.map(|capacities| capacities[index].clone()),
		})
		.collect();
	Ok(PreparedOptimisticHpGroupCapacityScorer {
		candidate: request.candidate,
		duty_window: request.duty_window,
		groups,
	})
}

impl PreparedOptimisticHpGroupCapacityScorer<'_> {
	/// Scores one deployment and its reviewed active spans against the prepared HP groups.
	pub fn score(
		&self,
		deploy_tick: u64,
		active_spans: &[TickSpan],
	) -> Result<OptimisticHpGroupScore, CoverageSearchError> {
		let deployment: ResolvedCoverageDeployment<'_> =
			ResolvedCoverageDeployment::new(self.candidate, deploy_tick)
				.with_active_spans(active_spans.to_vec());
		validate_optimistic_deployments(std::slice::from_ref(&deployment))?;

		let mut scheduled_attacks: Vec<ScheduledMemberAttack> = Vec::new();
		visit_deployment_attacks(&deployment, self.duty_window, |scheduled| {
			scheduled_attacks.push(scheduled);
		})?;
		let capacity_by_group: BTreeMap<HpGroupId, FixedDamage> =
			score_prepared_optimistic_attacks(&self.groups, &scheduled_attacks)?;
		Ok(OptimisticHpGroupScore { capacity_by_group })
	}
}

fn score_prepared_optimistic_attacks(
	groups: &[PreparedOptimisticHpGroupCapacity],
	attacks: &[ScheduledMemberAttack],
) -> Result<BTreeMap<HpGroupId, FixedDamage>, CoverageSearchError> {
	groups
		.iter()
		.map(|group| {
			let capacity: FixedDamage = attacks
				.iter()
				.filter(|attack| attack.tick < group.deadline_tick)
				.try_fold(FixedDamage::ZERO, |total, attack| {
					let profile_capacity: &PreparedTickCapacity = if attack.uses_active_profile {
						group
							.active
							.as_ref()
							.expect("scheduled active attacks require an active profile")
					} else {
						&group.normal
					};
					total
						.checked_add(profile_capacity.capacity_at(attack.tick))
						.map_err(CoverageSearchError::Arithmetic)
				})?;
			Ok((group.group_id, capacity))
		})
		.collect()
}

/// Computes a retained-start-invariant spatial reachability upper bound.
///
/// A wave-local enemy contributes its full HP when any deployment has at least one reviewed
/// normal or active attack profile that can intersect its relative route, target its motion
/// class, and deal positive damage through its defenses. HP is credited only once per group even
/// when several deployments can reach the same enemy. Attack counts, deployment ticks, skill
/// spans, target contention, and blocking duration are deliberately ignored; each omission can
/// only increase this hard-pruning capacity.
pub fn score_optimistic_wave_local_reachability_capacity(
	request: OptimisticWaveLocalReachabilityScoreRequest<'_>,
) -> Result<OptimisticWaveLocalReachabilityScore, CoverageSearchError> {
	validate_wave_local_reachability_request(&request)?;

	let mut capacity_by_group: BTreeMap<WaveLocalHpGroupId, FixedDamage> = BTreeMap::new();
	for group in request.hp_groups {
		let mut capacity: FixedDamage = FixedDamage::ZERO;
		for member in &group.members {
			let enemy: &RelativeEnemyTrajectory = request
				.template
				.enemy(member)
				.expect("wave-local request validation resolved every member");
			let mut reachable: bool = false;
			for deployment in request.deployments {
				if optimistic_candidate_reaches_relative_enemy(deployment.candidate, enemy)? {
					reachable = true;
					break;
				}
			}
			if reachable {
				let maximum_hp: FixedDamage = FixedDamage::from_whole(enemy.max_hp)
					.map_err(CoverageSearchError::Arithmetic)?;
				capacity = capacity
					.checked_add(maximum_hp)
					.map_err(CoverageSearchError::Arithmetic)?;
			}
		}
		capacity_by_group.insert(group.id.clone(), capacity);
	}
	Ok(OptimisticWaveLocalReachabilityScore { capacity_by_group })
}

/// Scores a fixed multi-operator configuration against one shared enemy HP and kill state.
///
/// Every member keeps its own normal-attack range, target filter, target count, and standard
/// target ordering. Attacks are merged by `(tick, stable member key, attack index)`, independent
/// of input order. The duty window bounds which attacks are enumerated; it never replaces an
/// enemy's goal deadline. Responsibility deadlines are therefore unified at the enemy goal and
/// are never shortened by one member's range exit.
pub fn score_joint_deployments(
	request: JointCoverageScoreRequest<'_>,
) -> Result<JointCoverageScore, CoverageSearchError> {
	validate_joint_request(&request)?;

	let mut members: Vec<JointMemberSimulation<'_>> = request
		.deployments
		.iter()
		.cloned()
		.map(|deployment| {
			let key: CoverageSolutionKey = deployment.key();
			JointMemberSimulation {
				deployment,
				key,
				attacks_with_targets: 0,
				damage: DamageAccounting::ZERO,
				damage_events: Vec::new(),
			}
		})
		.collect();
	members.sort_by(|left, right| left.key.cmp(&right.key));

	let mut scheduled_attacks: Vec<ScheduledJointAttack> = Vec::new();
	for (member_index, member) in members.iter().enumerate() {
		schedule_member_attacks(
			member_index,
			member,
			request.duty_window,
			&mut scheduled_attacks,
		)?;
	}
	scheduled_attacks.sort_by(|left, right| {
		left.tick
			.cmp(&right.tick)
			.then_with(|| left.member_key.cmp(&right.member_key))
			.then_with(|| left.attack_index.cmp(&right.attack_index))
	});

	let mut enemies: Vec<SimulatedEnemy> = initialize_enemies(request.model)?;
	for scheduled in scheduled_attacks {
		let candidate: &ResolvedCoverageCandidate =
			members[scheduled.member_index].deployment.candidate;
		let profile: &NormalAttackProfile = if scheduled.uses_active_profile {
			candidate
				.active_attack
				.as_ref()
				.expect("scheduled active attacks require an active profile")
		} else {
			&candidate.normal_attack
		};
		let attack_cells: &BTreeSet<GridPosition> = if scheduled.uses_active_profile {
			candidate.active_attack_cells()
		} else {
			&candidate.attack_cells
		};
		let applied: AppliedNormalAttack = apply_normal_attack(
			request.model,
			profile,
			attack_cells,
			&mut enemies,
			scheduled.tick,
		)?;
		if applied.enemies.is_empty() {
			continue;
		}

		let member: &mut JointMemberSimulation<'_> = &mut members[scheduled.member_index];
		member.attacks_with_targets = member
			.attacks_with_targets
			.checked_add(1)
			.ok_or(CoverageSearchError::ConfigurationCountOverflow)?;
		member.damage = member
			.damage
			.checked_add(applied.accounting)
			.map_err(CoverageSearchError::Arithmetic)?;
		member
			.damage_events
			.extend(
				applied
					.enemies
					.into_iter()
					.map(|applied_enemy| AttributedEnemyDamageEvent {
						attack_index: scheduled.attack_index,
						instance_id: applied_enemy.instance_id,
						tick: applied_enemy.event.tick,
						raw: applied_enemy.event.raw,
						credited: applied_enemy.event.credited,
						overkill: applied_enemy.event.overkill,
						remaining_hp: applied_enemy.event.remaining_hp,
					}),
			);
	}

	let enemy_scores: Vec<EnemyCoverageScore> = build_enemy_scores(
		request.model,
		enemies,
		request.target_cohort_ids,
		|trajectory| trajectory.goal_deadline_tick,
	);
	let responsibility: ResponsibilityCoverage = summarize_responsibility(&enemy_scores)?;
	let member_scores: Vec<JointMemberCoverageScore> = members
		.into_iter()
		.map(|member| JointMemberCoverageScore {
			key: member.key,
			operator_id: member.deployment.candidate.operator_id.clone(),
			operator_name: member.deployment.candidate.operator_name.clone(),
			deployment_cost: member.deployment.candidate.deployment_cost,
			attack_origin_tick: member.deployment.attack_origin_tick(),
			active_spans: member.deployment.active_spans,
			attacks_with_targets: member.attacks_with_targets,
			damage: member.damage,
			damage_events: member.damage_events,
		})
		.collect();
	Ok(JointCoverageScore {
		member_scores,
		enemy_scores,
		cumulative_deadlines: responsibility.cumulative_deadlines,
		minimum_cumulative_coverage: responsibility.minimum_cumulative_coverage,
		total_due_remaining_hp: responsibility.total_due_remaining_hp,
		clearance_slack_ticks: responsibility.clearance_slack_ticks,
		passed_declared_expected_damage_model: responsibility.passed,
	})
}

fn cumulative_deadlines(
	enemy_scores: &[EnemyCoverageScore],
) -> Result<Vec<CumulativeDeadlineCoverage>, CoverageSearchError> {
	let deadlines: BTreeSet<u64> = enemy_scores
		.iter()
		.filter_map(|enemy: &EnemyCoverageScore| enemy.deadline_tick)
		.collect();
	let mut output: Vec<CumulativeDeadlineCoverage> = Vec::with_capacity(deadlines.len());
	for deadline_tick in deadlines {
		let mut required_hp: FixedDamage = FixedDamage::ZERO;
		let mut credited_damage: FixedDamage = FixedDamage::ZERO;
		let mut remaining_hp: FixedDamage = FixedDamage::ZERO;
		for enemy in enemy_scores.iter().filter(|enemy: &&EnemyCoverageScore| {
			enemy
				.deadline_tick
				.is_some_and(|deadline: u64| deadline <= deadline_tick)
		}) {
			let enemy_deadline: u64 = enemy
				.deadline_tick
				.expect("filtered duty enemies always have a deadline");
			let credited_before_deadline: FixedDamage = enemy
				.damage_events
				.iter()
				.filter(|event: &&EnemyDamageEvent| event.tick < enemy_deadline)
				.try_fold(
					FixedDamage::ZERO,
					|total: FixedDamage, event: &EnemyDamageEvent| {
						total.checked_add(event.credited)
					},
				)
				.map_err(CoverageSearchError::Arithmetic)?;
			let remaining_at_deadline: FixedDamage = enemy
				.max_hp
				.checked_sub(credited_before_deadline)
				.map_err(CoverageSearchError::Arithmetic)?;
			required_hp = required_hp
				.checked_add(enemy.max_hp)
				.map_err(CoverageSearchError::Arithmetic)?;
			credited_damage = credited_damage
				.checked_add(credited_before_deadline)
				.map_err(CoverageSearchError::Arithmetic)?;
			remaining_hp = remaining_hp
				.checked_add(remaining_at_deadline)
				.map_err(CoverageSearchError::Arithmetic)?;
		}
		output.push(CumulativeDeadlineCoverage {
			deadline_tick,
			required_hp,
			credited_damage,
			remaining_hp,
			coverage: CoverageRatio {
				credited: credited_damage,
				required: required_hp,
			},
		});
	}
	Ok(output)
}

fn target_class(class: TrajectoryClass) -> TargetClass {
	match class {
		TrajectoryClass::Ground => TargetClass::Ground,
		TrajectoryClass::Air => TargetClass::Air,
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum CoverageSearchError {
	BlankStableBuildId,
	BlankOperatorId,
	BlankOperatorName,
	EmptyAttackRange {
		stable_build_id: String,
		deployment_cell: GridPosition,
		facing: Facing,
	},
	UnsupportedTargetPriority,
	EmptyDeploymentSet,
	EmptyHpGroups,
	EmptyHpGroup {
		group_id: HpGroupId,
	},
	DuplicateHpGroup {
		group_id: HpGroupId,
	},
	HpGroupDeadlineMismatch {
		group_id: HpGroupId,
		deadline_tick: u64,
	},
	HpGroupDeadlineOutsideDuty {
		group_id: HpGroupId,
		deadline_tick: u64,
		duty_window: TickSpan,
	},
	UnknownHpGroupMember {
		group_id: HpGroupId,
		instance_id: SpawnInstanceId,
	},
	EmptyWaveLocalHpGroup {
		group_id: WaveLocalHpGroupId,
	},
	DuplicateWaveLocalHpGroup {
		group_id: WaveLocalHpGroupId,
	},
	UnknownWaveLocalHpGroupMember {
		group_id: WaveLocalHpGroupId,
		instance_id: SpawnInstanceId,
	},
	InvalidWaveLocalHpGroupMember {
		group_id: WaveLocalHpGroupId,
		instance_id: SpawnInstanceId,
		enemy_anchor: StageTimeAnchor,
		enemy_goal_deadline_offset_ticks: u64,
	},
	EmptyTargetCohorts,
	UnknownTargetCohort {
		cohort_id: String,
	},
	DuplicateDeploymentMember {
		key: CoverageSolutionKey,
	},
	DuplicateDeploymentCell {
		deployment_cell: GridPosition,
	},
	DuplicateOperatorDeployment {
		operator_id: String,
	},
	DuplicateEnemyInstance {
		instance_id: SpawnInstanceId,
	},
	EmptyTickSpan {
		kind: &'static str,
		start: u64,
		end: u64,
	},
	ActivityBeforeDeployment {
		stable_build_id: String,
		deploy_tick: u64,
		activity_start: u64,
	},
	OverlappingOrUnsortedActivitySpans {
		stable_build_id: String,
		left: TickSpan,
		right: TickSpan,
	},
	ConfigurationCountOverflow,
	InvalidArtsResistance {
		instance_id: SpawnInstanceId,
		resistance: f64,
	},
	Arithmetic(FixedDamageError),
	AttackApplication(NormalAttackApplicationError),
}

impl Display for CoverageSearchError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::BlankStableBuildId => formatter.write_str("stable build ID must not be blank"),
			Self::BlankOperatorId => formatter.write_str("operator ID must not be blank"),
			Self::BlankOperatorName => formatter.write_str("operator name must not be blank"),
			Self::EmptyAttackRange {
				stable_build_id,
				deployment_cell,
				facing,
			} => write!(
				formatter,
				"candidate {stable_build_id} at ({}, {}) facing {facing:?} has an empty attack range",
				deployment_cell.row, deployment_cell.col
			),
			Self::UnsupportedTargetPriority => {
				formatter.write_str("only standard normal-attack target priority is supported")
			}
			Self::EmptyDeploymentSet => formatter.write_str("deployment set must not be empty"),
			Self::EmptyHpGroups => formatter.write_str("HP group set must not be empty"),
			Self::EmptyHpGroup { group_id } => {
				write!(
					formatter,
					"HP group {group_id} must contain at least one enemy"
				)
			}
			Self::DuplicateHpGroup { group_id } => {
				write!(formatter, "HP group {group_id} appears more than once")
			}
			Self::HpGroupDeadlineMismatch {
				group_id,
				deadline_tick,
			} => write!(
				formatter,
				"HP group {group_id} does not match its declared deadline tick {deadline_tick}"
			),
			Self::HpGroupDeadlineOutsideDuty {
				group_id,
				deadline_tick,
				duty_window,
			} => write!(
				formatter,
				"HP group {group_id} deadline {deadline_tick} is outside duty window [{}, {})",
				duty_window.start, duty_window.end
			),
			Self::UnknownHpGroupMember {
				group_id,
				instance_id,
			} => write!(
				formatter,
				"HP group {group_id} references missing enemy {instance_id:?}"
			),
			Self::EmptyWaveLocalHpGroup { group_id } => write!(
				formatter,
				"wave-local HP group {group_id:?} must contain at least one enemy"
			),
			Self::DuplicateWaveLocalHpGroup { group_id } => write!(
				formatter,
				"wave-local HP group {group_id:?} appears more than once"
			),
			Self::UnknownWaveLocalHpGroupMember {
				group_id,
				instance_id,
			} => write!(
				formatter,
				"wave-local HP group {group_id:?} references missing enemy {instance_id:?}"
			),
			Self::InvalidWaveLocalHpGroupMember {
				group_id,
				instance_id,
				enemy_anchor,
				enemy_goal_deadline_offset_ticks,
			} => write!(
				formatter,
				"wave-local HP group {group_id:?} cannot contain enemy {instance_id:?} under \
				 {enemy_anchor:?} with local deadline {enemy_goal_deadline_offset_ticks}"
			),
			Self::EmptyTargetCohorts => formatter.write_str("target cohort set must not be empty"),
			Self::UnknownTargetCohort { cohort_id } => {
				write!(
					formatter,
					"target cohort {cohort_id} is absent from the model"
				)
			}
			Self::DuplicateDeploymentMember { key } => write!(
				formatter,
				"duplicate deployment member {} at ({}, {}) facing {:?} on tick {}",
				key.stable_build_id,
				key.deployment_cell.row,
				key.deployment_cell.col,
				key.facing,
				key.deploy_tick
			),
			Self::DuplicateDeploymentCell { deployment_cell } => write!(
				formatter,
				"deployment cell ({}, {}) is occupied by more than one joint member",
				deployment_cell.row, deployment_cell.col
			),
			Self::DuplicateOperatorDeployment { operator_id } => write!(
				formatter,
				"operator {operator_id} appears more than once in the joint deployment"
			),
			Self::DuplicateEnemyInstance { instance_id } => {
				write!(formatter, "duplicate enemy instance {instance_id:?}")
			}
			Self::EmptyTickSpan { kind, start, end } => {
				write!(formatter, "{kind} tick span [{start}, {end}) is empty")
			}
			Self::ActivityBeforeDeployment {
				stable_build_id,
				deploy_tick,
				activity_start,
			} => write!(
				formatter,
				"candidate {stable_build_id} activity starts at tick {activity_start} before \
				 deployment tick {deploy_tick}"
			),
			Self::OverlappingOrUnsortedActivitySpans {
				stable_build_id,
				left,
				right,
			} => write!(
				formatter,
				"candidate {stable_build_id} has overlapping or unsorted activity spans \
				 [{}, {}) and [{}, {})",
				left.start, left.end, right.start, right.end
			),
			Self::ConfigurationCountOverflow => {
				formatter.write_str("coverage configuration count overflowed")
			}
			Self::InvalidArtsResistance {
				instance_id,
				resistance,
			} => write!(
				formatter,
				"enemy {instance_id:?} has invalid arts resistance {resistance}"
			),
			Self::Arithmetic(source) => write!(formatter, "coverage arithmetic failed: {source}"),
			Self::AttackApplication(source) => {
				write!(formatter, "normal attack application failed: {source}")
			}
		}
	}
}

impl Error for CoverageSearchError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::Arithmetic(source) => Some(source),
			Self::AttackApplication(source) => Some(source),
			_ => None,
		}
	}
}

#[cfg(test)]
mod joint_tests {
	use crate::data::GridPosition;

	use super::super::stage_space_time::CellOccupancy;
	use super::*;
	use crate::mechanics::domain::{
		LockedTargetDamagePacket, NormalAttackDamageType, NormalAttackTargetFilter,
	};
	use crate::mechanics::stage::SpawnCohortId;

	fn cell(row: u32, col: u32) -> GridPosition {
		GridPosition { row, col }
	}

	fn candidate(
		stable_build_id: &str,
		operator_id: &str,
		damage: u64,
		first_delay: u64,
		interval: u64,
		filter: NormalAttackTargetFilter,
		attack_cells: &[GridPosition],
	) -> ResolvedCoverageCandidate {
		let profile: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(damage).expect("test damage fits"),
			)],
			first_delay,
			interval,
			filter,
		)
		.expect("test profile is valid");
		let deployment_col: u32 = stable_build_id.bytes().map(u32::from).sum();
		ResolvedCoverageCandidate::new(
			stable_build_id.to_owned(),
			operator_id.to_owned(),
			stable_build_id.to_uppercase(),
			10,
			cell(9, deployment_col),
			Facing::Right,
			attack_cells.iter().copied().collect(),
			profile,
		)
		.expect("test candidate is valid")
	}

	fn enemy(
		ordinal: u32,
		max_hp: u64,
		class: TrajectoryClass,
		occupancy: Vec<CellOccupancy>,
		goal_deadline_tick: u64,
	) -> EnemyTrajectory {
		EnemyTrajectory {
			instance_id: SpawnInstanceId {
				cohort: SpawnCohortId::new("duty".to_owned()).expect("test cohort is valid"),
				ordinal,
			},
			enemy_id: format!("enemy-{ordinal}"),
			route_index: 0,
			spawn_tick: occupancy
				.first()
				.expect("test enemy has occupancy")
				.enter_tick,
			goal_deadline_tick,
			max_hp,
			defense: 0,
			magic_resistance: 0.0,
			target_class: class,
			occupancy,
		}
	}

	fn occupancy(
		cell: GridPosition,
		enter_tick: u64,
		exit_tick: u64,
		remaining_distance: u64,
	) -> CellOccupancy {
		CellOccupancy {
			cell,
			enter_tick,
			exit_tick,
			remaining_route_distance_microunits: remaining_distance,
		}
	}

	fn model(enemies: Vec<EnemyTrajectory>) -> StageSpaceTimeModel {
		StageSpaceTimeModel {
			stage_id: "joint-test".to_owned(),
			code: Some("JOINT".to_owned()),
			level_id: "joint-test-level".to_owned(),
			ticks_per_second: 30,
			cohorts: Vec::new(),
			enemies,
			bound_anchors: Vec::new(),
			mechanic_requirements: Vec::new(),
		}
	}

	fn target_cohorts() -> BTreeSet<String> {
		BTreeSet::from(["duty".to_owned()])
	}

	fn member<'a>(
		candidate: &'a ResolvedCoverageCandidate,
		deploy_tick: u64,
	) -> ResolvedCoverageDeployment<'a> {
		ResolvedCoverageDeployment::new(candidate, deploy_tick)
	}

	fn joint_score(
		model: &StageSpaceTimeModel,
		deployments: &[ResolvedCoverageDeployment<'_>],
		duty_window: TickSpan,
	) -> JointCoverageScore {
		score_joint_deployments(JointCoverageScoreRequest {
			model,
			deployments,
			target_cohort_ids: &target_cohorts(),
			duty_window,
		})
		.expect("joint score succeeds")
	}

	fn hp_group(
		stage: &StageSpaceTimeModel,
		deadline_tick: u64,
		member_ordinals: &[u32],
	) -> SpaceTimeHpGroup {
		let members: BTreeSet<SpawnInstanceId> = stage
			.enemies
			.iter()
			.filter(|enemy| member_ordinals.contains(&enemy.instance_id.ordinal))
			.map(|enemy| enemy.instance_id.clone())
			.collect();
		let required_whole: u64 = stage
			.enemies
			.iter()
			.filter(|enemy| members.contains(&enemy.instance_id))
			.map(|enemy| enemy.max_hp)
			.sum();
		SpaceTimeHpGroup {
			id: HpGroupId::new(deadline_tick),
			deadline_tick,
			members,
			required_hp: FixedDamage::from_whole(required_whole).expect("test HP fits"),
		}
	}

	fn optimistic_score(
		stage: &StageSpaceTimeModel,
		deployments: &[ResolvedCoverageDeployment<'_>],
		hp_groups: &[SpaceTimeHpGroup],
		duty_window: TickSpan,
	) -> OptimisticHpGroupScore {
		let mut capacity_by_group: BTreeMap<HpGroupId, FixedDamage> = hp_groups
			.iter()
			.map(|group| (group.id, FixedDamage::ZERO))
			.collect();
		for deployment in deployments {
			let prepared: PreparedOptimisticHpGroupCapacityScorer<'_> =
				prepare_optimistic_hp_group_capacity(OptimisticHpGroupCapacityPrepareRequest {
					model: stage,
					candidate: deployment.candidate,
					hp_groups,
					duty_window,
				})
				.expect("optimistic score preparation succeeds");
			let member_score: OptimisticHpGroupScore = prepared
				.score(deployment.deploy_tick(), deployment.active_spans())
				.expect("optimistic member score succeeds");
			for (group_id, capacity) in member_score.capacity_by_group {
				let total: &mut FixedDamage = capacity_by_group
					.get_mut(&group_id)
					.expect("prepared scorer preserves HP groups");
				*total = total
					.checked_add(capacity)
					.expect("test aggregate capacity fits");
			}
		}
		OptimisticHpGroupScore { capacity_by_group }
	}

	fn relative_enemy(
		fragment: u32,
		ordinal: u32,
		max_hp: u64,
		class: TrajectoryClass,
		occupancy: Vec<CellOccupancy>,
		goal_deadline_offset_ticks: u64,
	) -> RelativeEnemyTrajectory {
		RelativeEnemyTrajectory {
			instance_id: SpawnInstanceId {
				cohort: SpawnCohortId::new(format!("wave:0/fragment:{fragment}/action:0"))
					.expect("test cohort is valid"),
				ordinal,
			},
			enemy_id: format!("relative-enemy-{fragment}-{ordinal}"),
			route_index: 0,
			anchor: StageTimeAnchor::WaveFragmentStart { wave: 0, fragment },
			spawn_offset_ticks: occupancy
				.first()
				.expect("test enemy has occupancy")
				.enter_tick,
			goal_deadline_offset_ticks,
			max_hp,
			defense: 0,
			magic_resistance: 0.0,
			target_class: class,
			occupancy,
		}
	}

	fn relative_template(enemies: Vec<RelativeEnemyTrajectory>) -> StageSpaceTimeTemplate {
		StageSpaceTimeTemplate {
			stage_id: "relative-stage".to_owned(),
			code: None,
			level_id: "relative-level".to_owned(),
			ticks_per_second: 30,
			cohorts: Vec::new(),
			enemies,
			mechanic_requirements: Vec::new(),
		}
	}

	fn wave_local_group(
		template: &StageSpaceTimeTemplate,
		anchor: StageTimeAnchor,
		goal_deadline_offset_ticks: u64,
	) -> WaveLocalHpGroup {
		let members: BTreeSet<SpawnInstanceId> = template
			.enemies
			.iter()
			.filter(|enemy| {
				enemy.anchor == anchor
					&& enemy.goal_deadline_offset_ticks <= goal_deadline_offset_ticks
			})
			.map(|enemy| enemy.instance_id.clone())
			.collect();
		let required_hp: FixedDamage = template
			.enemies
			.iter()
			.filter(|enemy| members.contains(&enemy.instance_id))
			.try_fold(FixedDamage::ZERO, |total, enemy| {
				total.checked_add(FixedDamage::from_whole(enemy.max_hp).expect("test HP fits"))
			})
			.expect("test aggregate HP fits");
		WaveLocalHpGroup {
			id: WaveLocalHpGroupId {
				anchor,
				goal_deadline_offset_ticks,
			},
			members,
			required_hp,
		}
	}

	fn wave_local_reachability_score(
		template: &StageSpaceTimeTemplate,
		deployments: &[ResolvedCoverageDeployment<'_>],
		hp_groups: &[WaveLocalHpGroup],
	) -> OptimisticWaveLocalReachabilityScore {
		score_optimistic_wave_local_reachability_capacity(
			OptimisticWaveLocalReachabilityScoreRequest {
				template,
				deployments,
				hp_groups,
			},
		)
		.expect("wave-local reachability score succeeds")
	}

	#[test]
	fn wave_local_reachability_capacity_is_invariant_to_absolute_schedule_shifts() {
		let attack_cell: GridPosition = cell(0, 1);
		let outside_cell: GridPosition = cell(0, 2);
		let anchor: StageTimeAnchor = StageTimeAnchor::WaveFragmentStart {
			wave: 0,
			fragment: 1,
		};
		let template: StageSpaceTimeTemplate = relative_template(vec![
			relative_enemy(
				1,
				0,
				100,
				TrajectoryClass::Ground,
				vec![occupancy(attack_cell, 10, 20, 2)],
				30,
			),
			relative_enemy(
				1,
				1,
				200,
				TrajectoryClass::Ground,
				vec![occupancy(outside_cell, 5, 25, 1)],
				30,
			),
		]);
		let candidate: ResolvedCoverageCandidate = candidate(
			"relative-reach",
			"char-relative-reach",
			10,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let early: [ResolvedCoverageDeployment<'_>; 1] =
			[member(&candidate, 0).with_active_span(TickSpan { start: 10, end: 20 })];
		let shifted: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 10_000)
			.with_active_span(TickSpan {
				start: 10_010,
				end: 10_020,
			})];
		let groups: [WaveLocalHpGroup; 1] = [wave_local_group(&template, anchor, 30)];

		let early_score: OptimisticWaveLocalReachabilityScore =
			wave_local_reachability_score(&template, &early, &groups);
		let shifted_score: OptimisticWaveLocalReachabilityScore =
			wave_local_reachability_score(&template, &shifted, &groups);

		assert_eq!(early_score, shifted_score);
		assert_eq!(
			early_score.capacity_by_group[&groups[0].id],
			FixedDamage::from_whole(100).expect("test capacity fits")
		);
	}

	#[test]
	fn wave_local_reachability_saturates_each_reachable_enemy_at_full_hp_once() {
		let attack_cell: GridPosition = cell(0, 1);
		let anchor: StageTimeAnchor = StageTimeAnchor::WaveFragmentStart {
			wave: 0,
			fragment: 2,
		};
		let template: StageSpaceTimeTemplate = relative_template(vec![relative_enemy(
			2,
			0,
			250,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let first: ResolvedCoverageCandidate = candidate(
			"first-reach",
			"char-first-reach",
			1,
			0,
			100,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let second: ResolvedCoverageCandidate = candidate(
			"second-reach",
			"char-second-reach",
			999,
			999,
			999,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 2] =
			[member(&first, 0), member(&second, 9_999)];
		let groups: [WaveLocalHpGroup; 1] = [wave_local_group(&template, anchor, 10)];

		let score: OptimisticWaveLocalReachabilityScore =
			wave_local_reachability_score(&template, &deployments, &groups);

		assert_eq!(
			score.capacity_by_group[&groups[0].id],
			FixedDamage::from_whole(250).expect("test capacity fits")
		);
	}

	#[test]
	fn optimistic_attack_is_not_multiplied_by_every_group_member() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![
			enemy(
				0,
				1_000,
				TrajectoryClass::Ground,
				vec![occupancy(attack_cell, 0, 10, 2)],
				10,
			),
			enemy(
				1,
				1_000,
				TrajectoryClass::Ground,
				vec![occupancy(attack_cell, 0, 10, 1)],
				10,
			),
		]);
		let candidate: ResolvedCoverageCandidate = candidate(
			"single-target",
			"char-single-target",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)];
		let groups: [SpaceTimeHpGroup; 1] = [hp_group(&stage, 10, &[0, 1])];

		let score: OptimisticHpGroupScore = optimistic_score(
			&stage,
			&deployments,
			&groups,
			TickSpan { start: 0, end: 10 },
		);

		assert_eq!(
			score.capacity_by_group[&HpGroupId::new(10)],
			FixedDamage::from_whole(100).expect("fits")
		);
	}

	#[test]
	fn optimistic_attack_cannot_credit_a_future_route_intersection() {
		let attack_cell: GridPosition = cell(0, 1);
		let outside_cell: GridPosition = cell(0, 2);
		let mut front: EnemyTrajectory = enemy(
			0,
			1_000,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		);
		front.defense = 90;
		let rear: EnemyTrajectory = enemy(
			1,
			1_000,
			TrajectoryClass::Ground,
			vec![
				occupancy(outside_cell, 0, 5, 2),
				occupancy(attack_cell, 5, 10, 1),
			],
			10,
		);
		let stage: StageSpaceTimeModel = model(vec![front, rear]);
		let profile: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::Physical,
				FixedDamage::from_whole(100).expect("fits"),
			)],
			0,
			20,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("profile is valid");
		let candidate: ResolvedCoverageCandidate = ResolvedCoverageCandidate::new(
			"route-upper-bound".to_owned(),
			"char-route-upper-bound".to_owned(),
			"Route Upper Bound".to_owned(),
			10,
			cell(9, 9),
			Facing::Right,
			BTreeSet::from([attack_cell]),
			profile,
		)
		.expect("candidate is valid");
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)];
		let groups: [SpaceTimeHpGroup; 1] = [hp_group(&stage, 10, &[0, 1])];

		let exact: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 1 });
		let optimistic: OptimisticHpGroupScore = optimistic_score(
			&stage,
			&deployments,
			&groups,
			TickSpan { start: 0, end: 10 },
		);

		assert_eq!(
			exact.member_scores[0].damage.raw(),
			FixedDamage::from_whole(10).expect("fits")
		);
		assert_eq!(
			optimistic.capacity_by_group[&HpGroupId::new(10)],
			FixedDamage::from_whole(10).expect("fits")
		);
	}

	#[test]
	fn optimistic_multi_target_attack_uses_largest_defense_adjusted_capacities() {
		let attack_cell: GridPosition = cell(0, 1);
		let mut enemies: Vec<EnemyTrajectory> = (0..3)
			.map(|ordinal| {
				enemy(
					ordinal,
					1_000,
					TrajectoryClass::Ground,
					vec![occupancy(attack_cell, 0, 10, u64::from(ordinal))],
					10,
				)
			})
			.collect();
		enemies[0].defense = 0;
		enemies[1].defense = 40;
		enemies[2].defense = 90;
		let stage: StageSpaceTimeModel = model(enemies);
		let profile: NormalAttackProfile = NormalAttackProfile::new(
			2,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::Physical,
				FixedDamage::from_whole(100).expect("fits"),
			)],
			0,
			20,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("profile is valid");
		let candidate: ResolvedCoverageCandidate = ResolvedCoverageCandidate::new(
			"multi-target".to_owned(),
			"char-multi-target".to_owned(),
			"Multi Target".to_owned(),
			10,
			cell(9, 9),
			Facing::Right,
			BTreeSet::from([attack_cell]),
			profile,
		)
		.expect("candidate is valid");
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)];
		let groups: [SpaceTimeHpGroup; 1] = [hp_group(&stage, 10, &[0, 1, 2])];

		let score: OptimisticHpGroupScore = optimistic_score(
			&stage,
			&deployments,
			&groups,
			TickSpan { start: 0, end: 10 },
		);

		assert_eq!(
			score.capacity_by_group[&HpGroupId::new(10)],
			FixedDamage::from_whole(160).expect("fits")
		);
	}

	#[test]
	fn optimistic_capacity_respects_spatial_exclusion_and_group_deadlines() {
		let attack_cell: GridPosition = cell(0, 1);
		let outside_cell: GridPosition = cell(0, 2);
		let stage: StageSpaceTimeModel = model(vec![
			enemy(
				0,
				1_000,
				TrajectoryClass::Ground,
				vec![occupancy(attack_cell, 0, 6, 1)],
				6,
			),
			enemy(
				1,
				1_000,
				TrajectoryClass::Ground,
				vec![occupancy(outside_cell, 0, 6, 1)],
				6,
			),
		]);
		let candidate: ResolvedCoverageCandidate = candidate(
			"deadline",
			"char-deadline",
			10,
			0,
			2,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)];
		let groups: [SpaceTimeHpGroup; 2] =
			[hp_group(&stage, 3, &[0, 1]), hp_group(&stage, 5, &[0, 1])];

		let score: OptimisticHpGroupScore =
			optimistic_score(&stage, &deployments, &groups, TickSpan { start: 0, end: 6 });

		assert_eq!(
			score.capacity_by_group[&HpGroupId::new(3)],
			FixedDamage::from_whole(20).expect("fits")
		);
		assert_eq!(
			score.capacity_by_group[&HpGroupId::new(5)],
			FixedDamage::from_whole(30).expect("fits")
		);
	}

	#[test]
	fn optimistic_capacity_reuses_existing_active_span_cadence() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			1_000,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let candidate: ResolvedCoverageCandidate = candidate(
			"active-spans",
			"char-active-spans",
			10,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)
			.with_active_spans(vec![
				TickSpan { start: 2, end: 4 },
				TickSpan { start: 6, end: 7 },
			])];
		let groups: [SpaceTimeHpGroup; 1] = [hp_group(&stage, 10, &[0])];

		let score: OptimisticHpGroupScore = optimistic_score(
			&stage,
			&deployments,
			&groups,
			TickSpan { start: 0, end: 10 },
		);

		assert_eq!(
			score.capacity_by_group[&HpGroupId::new(10)],
			FixedDamage::from_whole(30).expect("fits")
		);
	}

	#[test]
	fn optimistic_cache_distinguishes_normal_and_active_profiles() {
		let normal_cell: GridPosition = cell(0, 1);
		let active_cell: GridPosition = cell(0, 2);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			1_000,
			TrajectoryClass::Ground,
			vec![occupancy(active_cell, 0, 4, 1)],
			4,
		)]);
		let active: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(20).expect("fits"),
			)],
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("active profile is valid");
		let candidate: ResolvedCoverageCandidate = candidate(
			"two-profile-cache",
			"char-two-profile-cache",
			10,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[normal_cell],
		)
		.with_active_attack(active)
		.with_active_attack_cells(BTreeSet::from([active_cell]));
		let deployments: [ResolvedCoverageDeployment<'_>; 1] =
			[member(&candidate, 0).with_active_span(TickSpan { start: 2, end: 4 })];
		let groups: [SpaceTimeHpGroup; 1] = [hp_group(&stage, 4, &[0])];

		let score: OptimisticHpGroupScore =
			optimistic_score(&stage, &deployments, &groups, TickSpan { start: 0, end: 4 });

		assert_eq!(
			score.capacity_by_group[&HpGroupId::new(4)],
			FixedDamage::from_whole(40).expect("fits")
		);
	}

	#[test]
	fn prepared_single_member_score_matches_general_scorer_across_active_cadences() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			1_000,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 12, 1)],
			12,
		)]);
		let active_profile: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(25).expect("fits"),
			)],
			0,
			2,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("active profile is valid");
		let shared: ResolvedCoverageCandidate = candidate(
			"prepared-shared",
			"char-prepared-shared",
			10,
			1,
			3,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		)
		.with_active_attack(active_profile.clone());
		let triggered: ResolvedCoverageCandidate = candidate(
			"prepared-triggered",
			"char-prepared-triggered",
			10,
			1,
			3,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		)
		.with_activation_triggered_active_attack(active_profile);
		let groups: [SpaceTimeHpGroup; 3] = [
			hp_group(&stage, 4, &[0]),
			hp_group(&stage, 7, &[0]),
			hp_group(&stage, 12, &[0]),
		];
		let duty_window: TickSpan = TickSpan { start: 0, end: 12 };
		let active_spans: Vec<TickSpan> = vec![
			TickSpan { start: 3, end: 6 },
			TickSpan { start: 8, end: 11 },
		];

		for (candidate, expected_whole) in
			[(&shared, [10_u64, 35, 70]), (&triggered, [35_u64, 60, 120])]
		{
			let prepared: PreparedOptimisticHpGroupCapacityScorer<'_> =
				prepare_optimistic_hp_group_capacity(OptimisticHpGroupCapacityPrepareRequest {
					model: &stage,
					candidate,
					hp_groups: &groups,
					duty_window,
				})
				.expect("prepared scorer succeeds");
			let deployment: [ResolvedCoverageDeployment<'_>; 1] =
				[member(candidate, 1).with_active_spans(active_spans.clone())];
			let general: OptimisticHpGroupScore =
				optimistic_score(&stage, &deployment, &groups, duty_window);
			let prepared_score: OptimisticHpGroupScore = prepared
				.score(1, &active_spans)
				.expect("prepared score succeeds");

			assert_eq!(prepared_score, general);
			for (group, expected_whole) in groups.iter().zip(expected_whole) {
				assert_eq!(
					prepared_score.capacity_by_group[&group.id],
					FixedDamage::from_whole(expected_whole).expect("expected capacity fits")
				);
			}
		}
	}

	#[test]
	fn exact_joint_damage_never_exceeds_optimistic_group_capacity() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			15,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 5, 1)],
			5,
		)]);
		let candidate: ResolvedCoverageCandidate = candidate(
			"upper-bound",
			"char-upper-bound",
			10,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)];
		let groups: [SpaceTimeHpGroup; 1] = [hp_group(&stage, 5, &[0])];
		let duty_window: TickSpan = TickSpan { start: 0, end: 5 };

		let exact: JointCoverageScore = joint_score(&stage, &deployments, duty_window);
		let optimistic: OptimisticHpGroupScore =
			optimistic_score(&stage, &deployments, &groups, duty_window);

		assert!(
			exact.member_scores[0].damage.raw() <= optimistic.capacity_by_group[&HpGroupId::new(5)]
		);
	}

	#[test]
	fn two_members_jointly_pass_when_neither_member_passes_alone() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			200,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let first: ResolvedCoverageCandidate = candidate(
			"a-build",
			"char-a",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let second: ResolvedCoverageCandidate = candidate(
			"b-build",
			"char-b",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let duty_window: TickSpan = TickSpan { start: 0, end: 1 };
		assert!(
			!joint_score(&stage, &[member(&first, 0)], duty_window)
				.passed_declared_expected_damage_model
		);
		assert!(
			!joint_score(&stage, &[member(&second, 0)], duty_window)
				.passed_declared_expected_damage_model
		);

		let deployments: [ResolvedCoverageDeployment<'_>; 2] =
			[member(&second, 0), member(&first, 0)];
		let score: JointCoverageScore = joint_score(&stage, &deployments, duty_window);
		assert!(score.passed_declared_expected_damage_model);
		assert_eq!(
			score.enemy_scores[0].damage.credited(),
			FixedDamage::from_whole(200).expect("fits")
		);
		assert!(score
			.member_scores
			.iter()
			.all(|member| member.damage.credited() == FixedDamage::from_whole(100).expect("fits")));
	}

	#[test]
	fn active_spans_limit_a_skill_profile_to_its_real_durations() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let skill: ResolvedCoverageCandidate = candidate(
			"skill-build@2",
			"char-skill",
			20,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&skill, 0)
			.with_active_spans(vec![
				TickSpan { start: 2, end: 4 },
				TickSpan { start: 6, end: 8 },
			])];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 10 });

		assert_eq!(score.member_scores[0].attacks_with_targets, 4);
		assert_eq!(
			score.member_scores[0]
				.damage_events
				.iter()
				.map(|event| event.tick)
				.collect::<Vec<_>>(),
			vec![2, 3, 6, 7]
		);
		assert_eq!(
			score.enemy_scores[0].remaining_hp,
			FixedDamage::from_whole(20).expect("fits")
		);
	}

	#[test]
	fn active_attack_replaces_normal_attacks_only_inside_skill_spans() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			200,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let active: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(20).expect("fits"),
			)],
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("active profile is valid");
		let candidate: ResolvedCoverageCandidate = candidate(
			"two-phase-build",
			"char-two-phase",
			10,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		)
		.with_active_attack(active);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&candidate, 0)
			.with_active_spans(vec![
				TickSpan { start: 2, end: 4 },
				TickSpan { start: 6, end: 8 },
			])];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 10 });

		assert_eq!(score.member_scores[0].attacks_with_targets, 10);
		assert_eq!(
			score.enemy_scores[0].remaining_hp,
			FixedDamage::from_whole(60).expect("fits")
		);
	}

	#[test]
	fn modified_normal_attack_keeps_one_timer_across_skill_boundaries() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			1_000,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 12, 1)],
			12,
		)]);
		let active: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(20).expect("fits"),
			)],
			0,
			2,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("active profile is valid");
		let candidate: ResolvedCoverageCandidate = candidate(
			"shared-cadence-build",
			"char-shared-cadence",
			10,
			0,
			3,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		)
		.with_active_attack(active);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] =
			[member(&candidate, 0).with_active_span(TickSpan { start: 4, end: 8 })];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 12 });
		let events: &Vec<AttributedEnemyDamageEvent> = &score.member_scores[0].damage_events;

		assert_eq!(
			events.iter().map(|event| event.tick).collect::<Vec<_>>(),
			vec![0, 3, 6, 8, 11]
		);
		assert_eq!(
			events
				.iter()
				.map(|event| event.raw.whole_floor())
				.collect::<Vec<_>>(),
			vec![10, 10, 20, 10, 10]
		);
	}

	#[test]
	fn reviewed_activation_triggered_attack_may_fire_at_activation_boundary() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			1_000,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 12, 1)],
			12,
		)]);
		let active: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(20).expect("fits"),
			)],
			0,
			2,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("active profile is valid");
		let candidate: ResolvedCoverageCandidate = candidate(
			"triggered-cadence-build",
			"char-triggered-cadence",
			10,
			0,
			3,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		)
		.with_activation_triggered_active_attack(active);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] =
			[member(&candidate, 0).with_active_span(TickSpan { start: 4, end: 8 })];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 12 });

		assert_eq!(
			score.member_scores[0]
				.damage_events
				.iter()
				.map(|event| event.tick)
				.collect::<Vec<_>>(),
			vec![0, 3, 4, 6, 9]
		);
	}

	#[test]
	fn active_attack_uses_its_own_projected_range() {
		let base_cell: GridPosition = cell(0, 1);
		let skill_cell: GridPosition = cell(0, 2);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![occupancy(skill_cell, 0, 10, 1)],
			10,
		)]);
		let active: NormalAttackProfile = NormalAttackProfile::new(
			1,
			vec![LockedTargetDamagePacket::new(
				NormalAttackDamageType::True,
				FixedDamage::from_whole(20).expect("fits"),
			)],
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
		)
		.expect("active profile is valid");
		let candidate: ResolvedCoverageCandidate = candidate(
			"range-override-build",
			"char-range-override",
			10,
			0,
			1,
			NormalAttackTargetFilter::GroundAndAir,
			&[base_cell],
		)
		.with_active_attack(active)
		.with_active_attack_cells(BTreeSet::from([skill_cell]));
		let deployments: [ResolvedCoverageDeployment<'_>; 1] =
			[member(&candidate, 0).with_active_span(TickSpan { start: 2, end: 4 })];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 10 });

		assert_eq!(score.member_scores[0].attacks_with_targets, 2);
		assert_eq!(
			score.enemy_scores[0].remaining_hp,
			FixedDamage::from_whole(60).expect("fits")
		);
	}

	#[test]
	fn shared_hp_prevents_double_damage_and_double_kill() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let first: ResolvedCoverageCandidate = candidate(
			"a-build",
			"char-a",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let second: ResolvedCoverageCandidate = candidate(
			"b-build",
			"char-b",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 2] =
			[member(&second, 0), member(&first, 0)];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 1 });
		let enemy: &EnemyCoverageScore = &score.enemy_scores[0];
		assert_eq!(
			enemy.damage.raw(),
			FixedDamage::from_whole(100).expect("fits")
		);
		assert_eq!(
			enemy.damage.credited(),
			FixedDamage::from_whole(100).expect("fits")
		);
		assert_eq!(enemy.kill_tick, Some(0));
		assert_eq!(enemy.damage_events.len(), 1);
		assert_eq!(
			score
				.member_scores
				.iter()
				.filter(|member| !member.damage.credited().is_zero())
				.count(),
			1
		);
	}

	#[test]
	fn same_tick_member_order_is_stable_across_input_order() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let first: ResolvedCoverageCandidate = candidate(
			"a-build",
			"char-a",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let second: ResolvedCoverageCandidate = candidate(
			"z-build",
			"char-z",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let forward: [ResolvedCoverageDeployment<'_>; 2] = [member(&first, 0), member(&second, 0)];
		let reversed: [ResolvedCoverageDeployment<'_>; 2] = [member(&second, 0), member(&first, 0)];
		let duty_window: TickSpan = TickSpan { start: 0, end: 1 };

		let forward_score: JointCoverageScore = joint_score(&stage, &forward, duty_window);
		let reversed_score: JointCoverageScore = joint_score(&stage, &reversed, duty_window);
		assert_eq!(forward_score, reversed_score);
		assert_eq!(
			forward_score.member_scores[0].key.stable_build_id,
			"a-build"
		);
		assert_eq!(
			forward_score.member_scores[0].damage.credited(),
			FixedDamage::from_whole(100).expect("fits")
		);
		assert_eq!(
			forward_score.member_scores[1].damage.credited(),
			FixedDamage::ZERO
		);
	}

	#[test]
	fn every_member_uses_its_own_range_and_target_filter() {
		let ground_cell: GridPosition = cell(0, 1);
		let air_cell: GridPosition = cell(0, 2);
		let stage: StageSpaceTimeModel = model(vec![
			enemy(
				0,
				100,
				TrajectoryClass::Ground,
				vec![occupancy(ground_cell, 0, 10, 2)],
				10,
			),
			enemy(
				1,
				100,
				TrajectoryClass::Air,
				vec![occupancy(air_cell, 0, 10, 1)],
				10,
			),
		]);
		let ground: ResolvedCoverageCandidate = candidate(
			"ground-build",
			"char-ground",
			100,
			0,
			10,
			NormalAttackTargetFilter::GroundOnly,
			&[ground_cell],
		);
		let air: ResolvedCoverageCandidate = candidate(
			"air-build",
			"char-air",
			100,
			0,
			10,
			NormalAttackTargetFilter::AirOnly,
			&[air_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 2] =
			[member(&ground, 0), member(&air, 0)];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 1 });
		assert!(score.is_feasible());
		let ground_member: &JointMemberCoverageScore = score
			.member_scores
			.iter()
			.find(|member| member.operator_id == "char-ground")
			.expect("ground member exists");
		let air_member: &JointMemberCoverageScore = score
			.member_scores
			.iter()
			.find(|member| member.operator_id == "char-air")
			.expect("air member exists");
		assert_eq!(ground_member.damage_events.len(), 1);
		assert_eq!(ground_member.damage_events[0].instance_id.ordinal, 0);
		assert_eq!(air_member.damage_events.len(), 1);
		assert_eq!(air_member.damage_events[0].instance_id.ordinal, 1);
	}

	#[test]
	fn joint_responsibility_uses_goal_deadline_beyond_enumeration_window() {
		let early_cell: GridPosition = cell(0, 1);
		let late_cell: GridPosition = cell(0, 2);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![
				occupancy(early_cell, 0, 3, 2),
				occupancy(late_cell, 3, 10, 1),
			],
			10,
		)]);
		let early: ResolvedCoverageCandidate = candidate(
			"early-build",
			"char-early",
			50,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[early_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 1] = [member(&early, 0)];

		let score: JointCoverageScore =
			joint_score(&stage, &deployments, TickSpan { start: 0, end: 8 });
		assert_eq!(score.enemy_scores[0].deadline_tick, Some(10));
		assert_eq!(score.cumulative_deadlines[0].deadline_tick, 10);
		assert_eq!(
			score.total_due_remaining_hp,
			FixedDamage::from_whole(50).expect("fits")
		);
	}

	#[test]
	fn joint_configuration_rejects_the_same_operator_twice() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let first_build: ResolvedCoverageCandidate = candidate(
			"first-build",
			"char-shared",
			50,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let second_build: ResolvedCoverageCandidate = candidate(
			"second-build",
			"char-shared",
			50,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let deployments: [ResolvedCoverageDeployment<'_>; 2] =
			[member(&first_build, 0), member(&second_build, 0)];
		let targets: BTreeSet<String> = target_cohorts();

		assert_eq!(
			score_joint_deployments(JointCoverageScoreRequest {
				model: &stage,
				deployments: &deployments,
				target_cohort_ids: &targets,
				duty_window: TickSpan { start: 0, end: 1 },
			}),
			Err(CoverageSearchError::DuplicateOperatorDeployment {
				operator_id: "char-shared".to_owned(),
			})
		);
	}

	#[test]
	fn joint_configuration_rejects_two_members_on_the_same_cell() {
		let attack_cell: GridPosition = cell(0, 1);
		let stage: StageSpaceTimeModel = model(vec![enemy(
			0,
			100,
			TrajectoryClass::Ground,
			vec![occupancy(attack_cell, 0, 10, 1)],
			10,
		)]);
		let first: ResolvedCoverageCandidate = candidate(
			"first-build",
			"char-first",
			50,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		let mut second: ResolvedCoverageCandidate = candidate(
			"second-build",
			"char-second",
			50,
			0,
			10,
			NormalAttackTargetFilter::GroundAndAir,
			&[attack_cell],
		);
		second.deployment_cell = first.deployment_cell;
		let deployments: [ResolvedCoverageDeployment<'_>; 2] =
			[member(&first, 0), member(&second, 0)];
		let targets: BTreeSet<String> = target_cohorts();

		assert_eq!(
			score_joint_deployments(JointCoverageScoreRequest {
				model: &stage,
				deployments: &deployments,
				target_cohort_ids: &targets,
				duty_window: TickSpan { start: 0, end: 1 },
			}),
			Err(CoverageSearchError::DuplicateDeploymentCell {
				deployment_cell: first.deployment_cell,
			})
		);
	}
}
