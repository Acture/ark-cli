//! Materialization of MAA-ordered guide deployments from a roster or catalog build domain.

use std::{
	collections::BTreeMap,
	error::Error,
	fmt::{Display, Formatter},
	num::NonZeroU32,
};

use crate::data::{
	ArkDataError, ArkDataSnapshot, CompiledStage, GridPosition, ResolvedBasePanelBuild,
	StageDifficulty, StageIdentity, StageRef,
};
use crate::simulation::TickRate;

use super::{
	coverage_prepare::{deployment_allowed, resolve_build_pool},
	coverage_search::{CoverageSearchError, ResolvedCoverageCandidate, ResolvedCoverageDeployment},
	deployment_dp::{
		schedule_ordered_deployments, DeploymentDpRules, OrderedDeploymentDpAction,
		OrderedDeploymentDpScheduleError, PlannedDeploymentDpGrant, ScheduledDeploymentDpAction,
	},
	query::DeploymentAnchor,
};
use crate::mechanics::{
	compile::compile_base_panel_normal_attack,
	domain::{project_attack_cells, NormalAttackCatalogEntry, RangeProjectionError},
};

/// Executor-independent guards and delays needed to schedule one guide deployment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GuideSchedulingConditions {
	pub minimum_dp: Option<i32>,
	pub cost_changes: Option<i32>,
	pub kills: Option<i32>,
	pub cooling: Option<i32>,
	pub pre_delay_millis: Option<i32>,
	pub post_delay_millis: Option<i32>,
	pub time_elapsed_millis: Option<i32>,
}

/// One normalized deployment action at the planning boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuideDeploymentActionSpec {
	pub action_index: usize,
	pub is_source_slot: bool,
	pub slot_name: String,
	pub operator_name: String,
	pub skill: Option<u8>,
	pub skill_usage: Option<u8>,
	pub skill_times: Option<u32>,
	pub deployment_anchor: DeploymentAnchor,
	pub conditions: GuideSchedulingConditions,
}

/// Ordered, normalized guide deployments plus the exact planning build domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuideDeploymentDomain {
	pub source_id: String,
	pub content_sha256: String,
	pub stage_name: String,
	pub difficulty: StageDifficulty,
	pub source_action_index: usize,
	pub selected_slots: BTreeMap<String, String>,
	pub build_domain: super::query::CandidatePoolSpec,
	pub actions: Vec<GuideDeploymentActionSpec>,
}

/// One guide deployment after build resolution and source-ordered DP scheduling.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedGuideDeployment {
	pub action: GuideDeploymentActionSpec,
	pub resolved_build: ResolvedBasePanelBuild,
	/// The source slot only needs its build cost for the retained-plan DP prefix.
	pub retained_coverage_candidate: Option<ResolvedCoverageCandidate>,
	pub dp_schedule: ScheduledDeploymentDpAction,
}

impl PreparedGuideDeployment {
	#[must_use]
	pub const fn deploy_tick(&self) -> u64 {
		self.dp_schedule.deploy_tick
	}

	#[must_use]
	pub fn as_retained_coverage_deployment(&self) -> Option<ResolvedCoverageDeployment<'_>> {
		self.retained_coverage_candidate
			.as_ref()
			.map(|candidate| ResolvedCoverageDeployment::new(candidate, self.deploy_tick()))
	}
}

/// Complete deployment prefix; the source slot remains present for DP accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedGuideDeploymentSchedule {
	pub data_revision: String,
	pub stage_id: String,
	pub level_id: String,
	pub ticks_per_second: u32,
	pub dp_rules: DeploymentDpRules,
	pub deployments: Vec<PreparedGuideDeployment>,
}

/// Exact per-action facts the planning boundary cannot recover from the build domain alone.
///
/// Deployment costs come from exact operator progression; DP grants come from the automatic
/// skills retained operators bring, decoded outside planning from the battle-prefab snapshot.
/// Both are keyed by guide action index; an absent action simply has neither.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GuideDeploymentActionOverrides {
	pub deployment_cost_by_action: BTreeMap<usize, u32>,
	pub dp_grants_by_action: BTreeMap<usize, Vec<PlannedDeploymentDpGrant>>,
}

/// Resolves a guide schedule with exact per-action deployment costs and DP grants.
///
/// Base-panel builds remain sufficient for retained damage compilation, while exact operator
/// progression may override DP costs for any guide action.
pub fn prepare_guide_deployment_schedule_with_cost_overrides(
	snapshot: &ArkDataSnapshot,
	ticks_per_second: NonZeroU32,
	domain: &GuideDeploymentDomain,
	overrides: &GuideDeploymentActionOverrides,
) -> Result<PreparedGuideDeploymentSchedule, GuideScheduleError> {
	prepare_guide_deployment_schedule_with_cost_overrides_inner(
		snapshot,
		ticks_per_second,
		domain,
		overrides,
		false,
	)
}

/// Resolves an exported schedule whose reviewed replacement block may interleave manual skills.
///
/// The caller must independently replay the complete action sequence and verify that every actual
/// deployment tick equals this DP-only schedule. That equality is what proves intervening actions
/// did not delay a later deployment.
pub(crate) fn prepare_interleaved_export_deployment_schedule_with_cost_overrides(
	snapshot: &ArkDataSnapshot,
	ticks_per_second: NonZeroU32,
	domain: &GuideDeploymentDomain,
	overrides: &GuideDeploymentActionOverrides,
) -> Result<PreparedGuideDeploymentSchedule, GuideScheduleError> {
	prepare_guide_deployment_schedule_with_cost_overrides_inner(
		snapshot,
		ticks_per_second,
		domain,
		overrides,
		true,
	)
}

fn prepare_guide_deployment_schedule_with_cost_overrides_inner(
	snapshot: &ArkDataSnapshot,
	ticks_per_second: NonZeroU32,
	domain: &GuideDeploymentDomain,
	overrides: &GuideDeploymentActionOverrides,
	allow_intervening_actions: bool,
) -> Result<PreparedGuideDeploymentSchedule, GuideScheduleError> {
	validate_action_order(&domain.actions, allow_intervening_actions)?;
	let tick_rate: TickRate = TickRate::new(ticks_per_second);
	let stage: CompiledStage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(domain.stage_name.clone()),
			difficulty: domain.difficulty,
		})
		.map_err(GuideScheduleError::ArkData)?;
	validate_domain_stage(domain, &stage)?;
	let builds: Vec<ResolvedBasePanelBuild> = resolve_build_pool(snapshot, &domain.build_domain)
		.map_err(GuideScheduleError::Preparation)?;

	let mut action_builds: Vec<ResolvedBasePanelBuild> = Vec::with_capacity(domain.actions.len());
	let mut retained_candidates: Vec<Option<ResolvedCoverageCandidate>> =
		Vec::with_capacity(domain.actions.len());
	let mut dp_actions: Vec<OrderedDeploymentDpAction> = Vec::with_capacity(domain.actions.len());
	for action in &domain.actions {
		let build: &ResolvedBasePanelBuild = unique_build_for_action(action, &builds)?;
		let deployment_cost: u32 = overrides
			.deployment_cost_by_action
			.get(&action.action_index)
			.copied()
			.unwrap_or_else(|| build.deployment_cost());
		let retained_candidate: Option<ResolvedCoverageCandidate> = if action.is_source_slot {
			None
		} else {
			Some(
				compile_action_candidate(snapshot, &stage, tick_rate, action, build)?
					.with_deployment_cost(deployment_cost),
			)
		};
		let completion_delay_ticks: u64 = completion_delay_ticks(action, tick_rate)?;
		let minimum_dp: u64 =
			non_negative_condition(action, "costs", action.conditions.minimum_dp.unwrap_or(0))?;
		validate_supported_conditions(action)?;
		let grants: Vec<PlannedDeploymentDpGrant> = overrides
			.dp_grants_by_action
			.get(&action.action_index)
			.cloned()
			.unwrap_or_default();
		dp_actions.push(
			OrderedDeploymentDpAction::new(
				format!("action:{}:{}", action.action_index, build.stable_id()),
				u64::from(deployment_cost),
				None,
				completion_delay_ticks,
			)
			.with_minimum_dp_before_deployment(minimum_dp)
			.with_grants(grants),
		);
		action_builds.push(build.clone());
		retained_candidates.push(retained_candidate);
	}

	let rules: DeploymentDpRules = stage_dp_rules(&stage, tick_rate)?;
	let scheduled: Vec<ScheduledDeploymentDpAction> =
		schedule_ordered_deployments(rules, &dp_actions).map_err(GuideScheduleError::DpSchedule)?;
	let deployments: Vec<PreparedGuideDeployment> = domain
		.actions
		.iter()
		.cloned()
		.zip(action_builds)
		.zip(retained_candidates)
		.zip(scheduled)
		.map(
			|(((action, resolved_build), retained_coverage_candidate), dp_schedule)| {
				PreparedGuideDeployment {
					action,
					resolved_build,
					retained_coverage_candidate,
					dp_schedule,
				}
			},
		)
		.collect();

	Ok(PreparedGuideDeploymentSchedule {
		data_revision: snapshot.resolved_revision().to_owned(),
		stage_id: stage.stage_id().to_owned(),
		level_id: stage.level_id().to_owned(),
		ticks_per_second: ticks_per_second.get(),
		dp_rules: rules,
		deployments,
	})
}

fn validate_domain_stage(
	domain: &GuideDeploymentDomain,
	stage: &CompiledStage,
) -> Result<(), GuideScheduleError> {
	let stage_name_matches: bool = domain.stage_name == stage.stage_id()
		|| stage.code().is_some_and(|code| domain.stage_name == code)
		|| domain.stage_name == stage.level_id();
	if !stage_name_matches {
		return Err(GuideScheduleError::GuideDomainMismatch {
			field: "stage_name",
			expected: format!(
				"one of {:?}, {:?}, {:?}",
				stage.stage_id(),
				stage.code(),
				stage.level_id()
			),
			found: domain.stage_name.clone(),
		});
	}
	let difficulty_matches: bool = domain.difficulty == stage.difficulty();
	if !difficulty_matches {
		return Err(GuideScheduleError::GuideDomainMismatch {
			field: "difficulty",
			expected: format!("{:?}", stage.difficulty()),
			found: format!("{:?}", domain.difficulty),
		});
	}
	Ok(())
}

fn validate_action_order(
	actions: &[GuideDeploymentActionSpec],
	allow_intervening_actions: bool,
) -> Result<(), GuideScheduleError> {
	if actions.is_empty() {
		return Err(GuideScheduleError::NoDeploymentActions);
	}
	let source_slots: usize = actions
		.iter()
		.filter(|action| action.is_source_slot)
		.count();
	if source_slots != 1 {
		return Err(GuideScheduleError::InvalidSourceSlotCount {
			found: source_slots,
		});
	}
	if actions[0].action_index != 0 {
		return Err(
			GuideScheduleError::UnsupportedActionsBeforeFirstDeployment {
				first_deployment_action_index: actions[0].action_index,
			},
		);
	}
	for pair in actions.windows(2) {
		if pair[0].action_index >= pair[1].action_index {
			return Err(GuideScheduleError::ActionsOutOfOrder {
				previous: pair[0].action_index,
				next: pair[1].action_index,
			});
		}
		if !allow_intervening_actions
			&& pair[0]
				.action_index
				.checked_add(1)
				.is_none_or(|expected| pair[1].action_index != expected)
		{
			return Err(GuideScheduleError::UnsupportedInterveningActions {
				previous_deployment_action_index: pair[0].action_index,
				next_deployment_action_index: pair[1].action_index,
			});
		}
	}
	Ok(())
}

fn unique_build_for_action<'a>(
	action: &GuideDeploymentActionSpec,
	builds: &'a [ResolvedBasePanelBuild],
) -> Result<&'a ResolvedBasePanelBuild, GuideScheduleError> {
	let matches: Vec<&ResolvedBasePanelBuild> = builds
		.iter()
		.filter(|build| build.name() == action.operator_name)
		.collect();
	match matches.as_slice() {
		[] => Err(GuideScheduleError::MissingOperatorBuild {
			action_index: action.action_index,
			operator_name: action.operator_name.clone(),
		}),
		[build] => Ok(*build),
		_ => Err(GuideScheduleError::AmbiguousOperatorBuild {
			action_index: action.action_index,
			operator_name: action.operator_name.clone(),
			stable_build_ids: matches
				.into_iter()
				.map(|build| build.stable_id().to_owned())
				.collect(),
		}),
	}
}

fn compile_action_candidate(
	snapshot: &ArkDataSnapshot,
	stage: &CompiledStage,
	tick_rate: TickRate,
	action: &GuideDeploymentActionSpec,
	build: &ResolvedBasePanelBuild,
) -> Result<ResolvedCoverageCandidate, GuideScheduleError> {
	let normal_attack = match compile_base_panel_normal_attack(build, tick_rate) {
		NormalAttackCatalogEntry::Supported(profile) => profile.into_profile(),
		NormalAttackCatalogEntry::Unsupported(unsupported) => {
			return Err(GuideScheduleError::UnsupportedNormalAttack {
				action_index: action.action_index,
				message: unsupported.reason().to_string(),
			});
		}
	};
	let range = snapshot
		.resolve_range(build.range_id())
		.map_err(GuideScheduleError::ArkData)?;
	let position: GridPosition = action.deployment_anchor.cell;
	let cell = stage
		.cell(position)
		.ok_or(GuideScheduleError::MissingStageCell {
			action_index: action.action_index,
			cell: action.deployment_anchor.cell,
		})?;
	if !deployment_allowed(build.position(), cell.buildable()) {
		return Err(GuideScheduleError::IncompatibleDeployment {
			action_index: action.action_index,
			operator_name: action.operator_name.clone(),
			cell: action.deployment_anchor.cell,
		});
	}
	let attack_cells = project_attack_cells(
		position,
		action.deployment_anchor.facing,
		range,
		stage.map_dimensions(),
	)
	.map_err(|source| GuideScheduleError::RangeProjection {
		action_index: action.action_index,
		source,
	})?;
	if attack_cells.is_empty() {
		return Err(GuideScheduleError::EmptyProjectedRange {
			action_index: action.action_index,
		});
	}
	ResolvedCoverageCandidate::new(
		build.stable_id().to_owned(),
		build.operator_id().to_owned(),
		build.name().to_owned(),
		build.deployment_cost(),
		position,
		action.deployment_anchor.facing,
		attack_cells,
		normal_attack,
	)
	.map_err(GuideScheduleError::Candidate)
}

fn validate_supported_conditions(
	action: &GuideDeploymentActionSpec,
) -> Result<(), GuideScheduleError> {
	let unsupported: Option<(&'static str, i32)> = [
		("cost_changes", action.conditions.cost_changes),
		("kills", action.conditions.kills),
		("pre_delay", action.conditions.pre_delay_millis),
		("time_elapsed", action.conditions.time_elapsed_millis),
	]
	.into_iter()
	.find_map(|(field, value)| {
		value
			.filter(|value| *value != 0)
			.map(|value| (field, value))
	});
	if let Some((field, value)) = unsupported {
		return Err(GuideScheduleError::UnsupportedActionCondition {
			action_index: action.action_index,
			field,
			value,
		});
	}
	if let Some(value) = action.conditions.cooling.filter(|value| *value != -1) {
		return Err(GuideScheduleError::UnsupportedActionCondition {
			action_index: action.action_index,
			field: "cooling",
			value,
		});
	}
	Ok(())
}

fn completion_delay_ticks(
	action: &GuideDeploymentActionSpec,
	tick_rate: TickRate,
) -> Result<u64, GuideScheduleError> {
	let delay: i32 = action.conditions.post_delay_millis.unwrap_or(0);
	let milliseconds: u64 = non_negative_condition(action, "post_delay", delay)?;
	milliseconds_to_ticks(action.action_index, "post_delay", milliseconds, tick_rate)
}

fn non_negative_condition(
	action: &GuideDeploymentActionSpec,
	field: &'static str,
	value: i32,
) -> Result<u64, GuideScheduleError> {
	u64::try_from(value).map_err(|_| GuideScheduleError::NegativeActionCondition {
		action_index: action.action_index,
		field,
		value,
	})
}

fn milliseconds_to_ticks(
	action_index: usize,
	field: &'static str,
	milliseconds: u64,
	tick_rate: TickRate,
) -> Result<u64, GuideScheduleError> {
	let numerator: u128 = u128::from(milliseconds) * u128::from(tick_rate.ticks_per_second().get());
	u64::try_from(numerator.div_ceil(1_000)).map_err(|_| GuideScheduleError::TickOverflow {
		action_index,
		field,
	})
}

fn stage_dp_rules(
	stage: &CompiledStage,
	tick_rate: TickRate,
) -> Result<DeploymentDpRules, GuideScheduleError> {
	let seconds: f64 = stage.options().cost_increase_time();
	if !seconds.is_finite() || seconds <= 0.0 {
		return Err(GuideScheduleError::InvalidNaturalDpInterval { seconds });
	}
	let ticks: f64 = (seconds * f64::from(tick_rate.ticks_per_second().get())).ceil();
	if ticks > u64::MAX as f64 {
		return Err(GuideScheduleError::NaturalDpIntervalOverflow { seconds });
	}
	Ok(DeploymentDpRules::new(
		u64::from(stage.options().initial_cost()),
		u64::from(stage.options().max_cost()),
		ticks as u64,
		1,
	))
}

#[derive(Debug)]
pub enum GuideScheduleError {
	Preparation(super::coverage_prepare::CoveragePreparationError),
	ArkData(ArkDataError),
	GuideDomainMismatch {
		field: &'static str,
		expected: String,
		found: String,
	},
	NoDeploymentActions,
	InvalidSourceSlotCount {
		found: usize,
	},
	UnsupportedActionsBeforeFirstDeployment {
		first_deployment_action_index: usize,
	},
	UnsupportedInterveningActions {
		previous_deployment_action_index: usize,
		next_deployment_action_index: usize,
	},
	ActionsOutOfOrder {
		previous: usize,
		next: usize,
	},
	MissingOperatorBuild {
		action_index: usize,
		operator_name: String,
	},
	AmbiguousOperatorBuild {
		action_index: usize,
		operator_name: String,
		stable_build_ids: Vec<String>,
	},
	MissingStageCell {
		action_index: usize,
		cell: GridPosition,
	},
	IncompatibleDeployment {
		action_index: usize,
		operator_name: String,
		cell: GridPosition,
	},
	UnsupportedNormalAttack {
		action_index: usize,
		message: String,
	},
	RangeProjection {
		action_index: usize,
		source: RangeProjectionError,
	},
	EmptyProjectedRange {
		action_index: usize,
	},
	NegativeActionCondition {
		action_index: usize,
		field: &'static str,
		value: i32,
	},
	UnsupportedActionCondition {
		action_index: usize,
		field: &'static str,
		value: i32,
	},
	TickOverflow {
		action_index: usize,
		field: &'static str,
	},
	InvalidNaturalDpInterval {
		seconds: f64,
	},
	NaturalDpIntervalOverflow {
		seconds: f64,
	},
	DpSchedule(OrderedDeploymentDpScheduleError),
	Candidate(CoverageSearchError),
}

impl Display for GuideScheduleError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Preparation(source) => {
				write!(formatter, "could not resolve build domain: {source}")
			}
			Self::ArkData(source) => {
				write!(formatter, "Arknights data resolution failed: {source}")
			}
			Self::GuideDomainMismatch {
				field,
				expected,
				found,
			} => write!(
				formatter,
				"guide domain {field} mismatch: expected {expected}, found {found}"
			),
			Self::NoDeploymentActions => formatter.write_str("guide has no deployment actions"),
			Self::InvalidSourceSlotCount { found } => {
				write!(
					formatter,
					"guide schedule expected one source slot, found {found}"
				)
			}
			Self::UnsupportedActionsBeforeFirstDeployment {
				first_deployment_action_index,
			} => write!(
				formatter,
				"guide has unsupported actions before its first deployment at action \
				 {first_deployment_action_index}"
			),
			Self::UnsupportedInterveningActions {
				previous_deployment_action_index,
				next_deployment_action_index,
			} => write!(
				formatter,
				"guide has unsupported actions between deployments \
				 {previous_deployment_action_index} and {next_deployment_action_index}"
			),
			Self::ActionsOutOfOrder { previous, next } => write!(
				formatter,
				"guide deployment actions are not in source order: {previous} before {next}"
			),
			Self::MissingOperatorBuild {
				action_index,
				operator_name,
			} => write!(
				formatter,
				"guide action {action_index} has no build for operator {operator_name:?} in the \
				 supplied domain"
			),
			Self::AmbiguousOperatorBuild {
				action_index,
				operator_name,
				stable_build_ids,
			} => write!(
				formatter,
				"guide action {action_index} operator {operator_name:?} has multiple builds in the \
				 supplied domain: {stable_build_ids:?}"
			),
			Self::MissingStageCell { action_index, cell } => write!(
				formatter,
				"guide action {action_index} cell ({}, {}) is outside the stage",
				cell.row, cell.col
			),
			Self::IncompatibleDeployment {
				action_index,
				operator_name,
				cell,
			} => write!(
				formatter,
				"guide action {action_index} cannot deploy {operator_name:?} at ({}, {})",
				cell.row, cell.col
			),
			Self::UnsupportedNormalAttack {
				action_index,
				message,
			} => write!(
				formatter,
				"guide action {action_index} has unsupported base-panel attack: {message}"
			),
			Self::RangeProjection {
				action_index,
				source,
			} => write!(
				formatter,
				"guide action {action_index} range projection failed: {source}"
			),
			Self::EmptyProjectedRange { action_index } => {
				write!(
					formatter,
					"guide action {action_index} projects an empty range"
				)
			}
			Self::NegativeActionCondition {
				action_index,
				field,
				value,
			} => write!(
				formatter,
				"guide action {action_index} has negative {field} value {value}"
			),
			Self::UnsupportedActionCondition {
				action_index,
				field,
				value,
			} => write!(
				formatter,
				"guide action {action_index} uses unsupported {field} condition {value}"
			),
			Self::TickOverflow {
				action_index,
				field,
			} => write!(
				formatter,
				"guide action {action_index} {field} conversion overflowed ticks"
			),
			Self::InvalidNaturalDpInterval { seconds } => write!(
				formatter,
				"stage natural DP interval must be finite and positive, found {seconds}"
			),
			Self::NaturalDpIntervalOverflow { seconds } => write!(
				formatter,
				"stage natural DP interval {seconds} seconds overflows ticks"
			),
			Self::DpSchedule(source) => write!(formatter, "guide DP scheduling failed: {source}"),
			Self::Candidate(source) => write!(formatter, "guide candidate is invalid: {source}"),
		}
	}
}

impl Error for GuideScheduleError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::Preparation(source) => Some(source),
			Self::ArkData(source) => Some(source),
			Self::RangeProjection { source, .. } => Some(source),
			Self::DpSchedule(source) => Some(source),
			Self::Candidate(source) => Some(source),
			_ => None,
		}
	}
}
