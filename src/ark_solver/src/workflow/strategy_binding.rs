//! Exact strategy binding from MAA job file selections into planning and replay inputs.

mod maa_planning;

#[cfg(test)]
#[path = "tests/guide_schedule.rs"]
mod guide_schedule_tests;

pub(crate) use maa_planning::guide_deployment_domain_from_maa_job;

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::{
	ArkDataError, ArkDataSnapshot, GridPosition, OperatorBuild, OperatorIdentity, OperatorModule,
	StageDifficulty, StageIdentity, StageRef,
};
use crate::simulation::SimTick;

use crate::maa_job::{MaaActionConditions, MaaJobAction, MaaJobDirection, MaaJobSeed};
use crate::mechanics::domain::{
	Facing, MaterializedGuideAction, MaterializedGuidePolicy, OperatorInstanceId,
};
use crate::replay::{ExactReplayBuildBinding, PreparedReplayGuide};

/// One independent Cartesian choice across every selected operator's legal max-level modules.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MaximalGuideBuildVariant {
	pub variant_id: String,
	pub bindings: Vec<ExactReplayBuildBinding>,
}

/// Enumerates E2 max, trust 200, P6, M3 variants for every legal real max-level module.
///
/// `OperatorCatalog::maximal_builds` deliberately emits `None` only when the operator has no
/// legal real module at the selected build. Module identities remain categorical.
pub fn enumerate_maximal_guide_build_variants(
	snapshot: &ArkDataSnapshot,
	seed: &MaaJobSeed,
) -> Result<Vec<MaximalGuideBuildVariant>, ExpectedReplayPrepareError> {
	enumerate_guide_build_variants(snapshot, seed, false)
}

/// Enumerates replay variants for every maximal real module plus the exact no-module build.
///
/// Reviewed replay contracts may deliberately pin `module=None`; standalone replay must include
/// that build even when the operator also owns a legal real module.
pub fn enumerate_replay_guide_build_variants(
	snapshot: &ArkDataSnapshot,
	seed: &MaaJobSeed,
) -> Result<Vec<MaximalGuideBuildVariant>, ExpectedReplayPrepareError> {
	enumerate_guide_build_variants(snapshot, seed, true)
}

fn enumerate_guide_build_variants(
	snapshot: &ArkDataSnapshot,
	seed: &MaaJobSeed,
	include_no_module: bool,
) -> Result<Vec<MaximalGuideBuildVariant>, ExpectedReplayPrepareError> {
	let mut source_names: BTreeSet<&str> = BTreeSet::new();
	let mut variants: Vec<Vec<ExactReplayBuildBinding>> = vec![Vec::new()];
	for operator in seed.operators() {
		if !source_names.insert(operator.name()) {
			return Err(ExpectedReplayPrepareError::DuplicateOperator {
				name: operator.name().to_owned(),
			});
		}
		let skill: u8 =
			operator
				.skill()
				.ok_or_else(|| ExpectedReplayPrepareError::MissingSkill {
					name: operator.name().to_owned(),
				})?;
		let mut builds: Vec<OperatorBuild> = snapshot
			.operators()
			.maximal_builds(operator.name(), skill)
			.map_err(ExpectedReplayPrepareError::ArkData)?;
		if builds.is_empty() {
			return Err(ExpectedReplayPrepareError::NoMaximalBuild {
				name: operator.name().to_owned(),
			});
		}
		if include_no_module {
			let template: &OperatorBuild = builds
				.first()
				.expect("nonempty maximal build set checked above");
			let no_module = OperatorBuild::new(
				template.identity().clone(),
				template.elite(),
				template.level(),
				template.trust_percent(),
				template.skill(),
				template.skill_level(),
				template.potential(),
				OperatorModule::None,
				0,
			);
			snapshot
				.operators()
				.resolve(&no_module)
				.map_err(ExpectedReplayPrepareError::ArkData)?;
			builds.push(no_module);
			builds.sort();
			builds.dedup();
		}
		let next_capacity: usize = variants
			.len()
			.checked_mul(builds.len())
			.ok_or(ExpectedReplayPrepareError::VariantCountOverflow)?;
		let mut next: Vec<Vec<ExactReplayBuildBinding>> = Vec::new();
		next.try_reserve_exact(next_capacity)
			.map_err(|_| ExpectedReplayPrepareError::VariantCountOverflow)?;
		for prefix in variants {
			for build in &builds {
				let mut bindings: Vec<ExactReplayBuildBinding> = prefix.clone();
				bindings.push(ExactReplayBuildBinding {
					source_name: operator.name().to_owned(),
					build: build.clone(),
					skill_usage: operator.skill_usage().unwrap_or(0),
					skill_times: operator.skill_times(),
				});
				next.push(bindings);
			}
		}
		variants = next;
	}
	let mut materialized: Vec<MaximalGuideBuildVariant> = variants
		.into_iter()
		.map(|bindings: Vec<ExactReplayBuildBinding>| {
			let variant_id: String = bindings
				.iter()
				.map(binding_variant_component)
				.collect::<Vec<String>>()
				.join("+");
			MaximalGuideBuildVariant {
				variant_id,
				bindings,
			}
		})
		.collect();
	materialized.sort();
	Ok(materialized)
}

/// Converts the complete source action list without assigning speculative execution times.
///
/// All actions are requested at tick zero. The replay executor preserves source order and records
/// actual DP waits. Guards and delays outside the current replay contract fail closed.
pub fn prepare_guide_actions(
	seed: &MaaJobSeed,
	bindings: &[ExactReplayBuildBinding],
	map_height: u32,
) -> Result<PreparedReplayGuide, ExpectedReplayPrepareError> {
	let _difficulty: StageDifficulty = replay_stage_difficulty(seed.difficulty())?;
	let mut operator_ids: BTreeMap<String, OperatorInstanceId> = BTreeMap::new();
	let mut skill_usage: BTreeMap<OperatorInstanceId, u8> = BTreeMap::new();
	for (index, binding) in bindings.iter().enumerate() {
		if !matches!(binding.skill_usage, 0 | 1) {
			let operator_identity: String = match binding.build.identity() {
				OperatorIdentity::Id(value) | OperatorIdentity::Name(value) => value.clone(),
			};
			return Err(ExpectedReplayPrepareError::UnsupportedSkillUsage {
				operator_identity,
				source_name: binding.source_name.clone(),
				skill_usage: binding.skill_usage,
				skill_times: binding.skill_times,
			});
		}
		let instance: OperatorInstanceId = OperatorInstanceId::new(
			u32::try_from(index).map_err(|_| ExpectedReplayPrepareError::OperatorCountOverflow)?,
		);
		if operator_ids
			.insert(binding.source_name.clone(), instance)
			.is_some()
		{
			return Err(ExpectedReplayPrepareError::DuplicateOperator {
				name: binding.source_name.clone(),
			});
		}
		skill_usage.insert(instance, binding.skill_usage);
	}
	let mut actions: Vec<MaterializedGuideAction> = Vec::with_capacity(seed.actions().len());
	for (action_index, action) in seed.actions().iter().enumerate() {
		if action
			.timeout()
			.is_some_and(|timeout| timeout.is_branching())
		{
			return Err(ExpectedReplayPrepareError::UnsupportedCondition {
				action_index,
				field: "timeout",
			});
		}
		validate_conditions(action_index, action.conditions())?;
		let requested_at: SimTick = SimTick::new(0);
		let materialized: MaterializedGuideAction = match action {
			MaaJobAction::Deploy {
				name,
				location,
				direction,
				..
			} => MaterializedGuideAction::Deploy {
				requested_at,
				operator: resolve_operator(&operator_ids, action_index, name)?,
				cell: maa_cell(*location, map_height, action_index)?,
				facing: maa_facing(*direction, action_index)?,
			},
			MaaJobAction::Skill {
				name: Some(name), ..
			} => MaterializedGuideAction::ActivateSkill {
				requested_at,
				operator: resolve_operator(&operator_ids, action_index, name)?,
			},
			MaaJobAction::Retreat {
				name: Some(name), ..
			} => MaterializedGuideAction::Retreat {
				requested_at,
				operator: resolve_operator(&operator_ids, action_index, name)?,
			},
			MaaJobAction::SpeedUp { .. } => MaterializedGuideAction::SpeedUp { requested_at },
			MaaJobAction::SkillDaemon { .. } => {
				MaterializedGuideAction::SkillDaemon { requested_at }
			}
			MaaJobAction::Skill { name: None, .. } | MaaJobAction::Retreat { name: None, .. } => {
				return Err(ExpectedReplayPrepareError::LocationOnlyAction { action_index });
			}
			MaaJobAction::BulletTime { .. }
			| MaaJobAction::SkillUsage { .. }
			| MaaJobAction::Output { .. }
			| MaaJobAction::MoveCamera { .. }
			| MaaJobAction::ResetStopwatch { .. } => {
				return Err(ExpectedReplayPrepareError::UnsupportedAction {
					action_index,
					kind: action_kind(action),
				});
			}
		};
		actions.push(materialized);
	}
	Ok(PreparedReplayGuide {
		actions,
		policy: MaterializedGuidePolicy::new(skill_usage),
		operator_ids,
	})
}

pub(super) fn prepare_replay_guide(
	snapshot: &ArkDataSnapshot,
	seed: &MaaJobSeed,
	bindings: &[ExactReplayBuildBinding],
) -> Result<PreparedReplayGuide, ExpectedReplayPrepareError> {
	let difficulty: StageDifficulty = replay_stage_difficulty(seed.difficulty())?;
	let stage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(seed.stage_name().to_owned()),
			difficulty,
		})
		.map_err(ExpectedReplayPrepareError::ArkData)?;
	prepare_guide_actions(seed, bindings, stage.map_dimensions().height)
}

pub(super) fn replay_stage_difficulty(
	difficulty: crate::maa_job::MaaJobDifficulty,
) -> Result<StageDifficulty, ExpectedReplayPrepareError> {
	match difficulty {
		crate::maa_job::MaaJobDifficulty::Normal => Ok(StageDifficulty::Normal),
		crate::maa_job::MaaJobDifficulty::Challenge => Ok(StageDifficulty::Raid),
		crate::maa_job::MaaJobDifficulty::Unspecified => {
			Err(ExpectedReplayPrepareError::UnspecifiedDifficulty)
		}
	}
}

fn binding_variant_component(binding: &ExactReplayBuildBinding) -> String {
	let operator_id: &str = match binding.build.identity() {
		OperatorIdentity::Id(value) | OperatorIdentity::Name(value) => value,
	};
	let module: &str = match binding.build.module() {
		OperatorModule::None => "none",
		OperatorModule::Id(value) => value,
	};
	format!("{operator_id}@{module}:{}", binding.build.module_level())
}

fn validate_conditions(
	action_index: usize,
	conditions: &MaaActionConditions,
) -> Result<(), ExpectedReplayPrepareError> {
	let unsupported: Option<&'static str> = [
		("costs", conditions.costs().is_some()),
		("cost_changes", conditions.cost_changes().is_some()),
		("kills", conditions.kills().is_some()),
		("cooling", conditions.cooling().is_some()),
		("pre_delay", conditions.pre_delay().is_some()),
		("post_delay", conditions.post_delay().is_some()),
		("time_elapsed", conditions.time_elapsed().is_some()),
	]
	.into_iter()
	.find_map(|(name, present): (&'static str, bool)| present.then_some(name));
	if let Some(field) = unsupported {
		return Err(ExpectedReplayPrepareError::UnsupportedCondition {
			action_index,
			field,
		});
	}
	Ok(())
}

fn resolve_operator(
	operators: &BTreeMap<String, OperatorInstanceId>,
	action_index: usize,
	name: &str,
) -> Result<OperatorInstanceId, ExpectedReplayPrepareError> {
	operators
		.get(name)
		.copied()
		.ok_or_else(|| ExpectedReplayPrepareError::UnknownActionOperator {
			action_index,
			name: name.to_owned(),
		})
}

fn maa_cell(
	location: [i32; 2],
	map_height: u32,
	action_index: usize,
) -> Result<GridPosition, ExpectedReplayPrepareError> {
	let [x, y]: [i32; 2] = location;
	let col: u32 = u32::try_from(x).map_err(|_| ExpectedReplayPrepareError::InvalidActionCell {
		action_index,
		location,
	})?;
	let top_row: u32 =
		u32::try_from(y).map_err(|_| ExpectedReplayPrepareError::InvalidActionCell {
			action_index,
			location,
		})?;
	let row: u32 = map_height.checked_sub(top_row.saturating_add(1)).ok_or(
		ExpectedReplayPrepareError::InvalidActionCell {
			action_index,
			location,
		},
	)?;
	Ok(GridPosition { row, col })
}

fn maa_facing(
	direction: MaaJobDirection,
	action_index: usize,
) -> Result<Facing, ExpectedReplayPrepareError> {
	match direction {
		MaaJobDirection::Left => Ok(Facing::Left),
		MaaJobDirection::Right => Ok(Facing::Right),
		MaaJobDirection::Up => Ok(Facing::Up),
		MaaJobDirection::Down => Ok(Facing::Down),
		MaaJobDirection::None => {
			Err(ExpectedReplayPrepareError::MissingDeployFacing { action_index })
		}
	}
}

fn action_kind(action: &MaaJobAction) -> &'static str {
	match action {
		MaaJobAction::Deploy { .. } => "Deploy",
		MaaJobAction::Skill { .. } => "Skill",
		MaaJobAction::Retreat { .. } => "Retreat",
		MaaJobAction::BulletTime { .. } => "BulletTime",
		MaaJobAction::SkillUsage { .. } => "SkillUsage",
		MaaJobAction::SpeedUp { .. } => "SpeedUp",
		MaaJobAction::Output { .. } => "Output",
		MaaJobAction::SkillDaemon { .. } => "SkillDaemon",
		MaaJobAction::MoveCamera { .. } => "MoveCamera",
		MaaJobAction::ResetStopwatch { .. } => "ResetStopwatch",
	}
}

#[derive(Debug)]
pub enum ExpectedReplayPrepareError {
	ArkData(ArkDataError),
	UnspecifiedDifficulty,
	DuplicateOperator {
		name: String,
	},
	MissingSkill {
		name: String,
	},
	NoMaximalBuild {
		name: String,
	},
	VariantCountOverflow,
	OperatorCountOverflow,
	UnknownActionOperator {
		action_index: usize,
		name: String,
	},
	InvalidActionCell {
		action_index: usize,
		location: [i32; 2],
	},
	MissingDeployFacing {
		action_index: usize,
	},
	LocationOnlyAction {
		action_index: usize,
	},
	UnsupportedAction {
		action_index: usize,
		kind: &'static str,
	},
	UnsupportedCondition {
		action_index: usize,
		field: &'static str,
	},
	UnsupportedSkillUsage {
		operator_identity: String,
		source_name: String,
		skill_usage: u8,
		skill_times: Option<u32>,
	},
}

impl Display for ExpectedReplayPrepareError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::ArkData(error) => Display::fmt(error, formatter),
			Self::UnspecifiedDifficulty => {
				formatter.write_str("guide difficulty must be selected before replay preparation")
			}
			Self::DuplicateOperator { name } => {
				write!(
					formatter,
					"guide materializes operator {name:?} more than once"
				)
			}
			Self::MissingSkill { name } => {
				write!(formatter, "guide operator {name:?} has no selected skill")
			}
			Self::NoMaximalBuild { name } => {
				write!(
					formatter,
					"guide operator {name:?} has no legal maximal build"
				)
			}
			Self::VariantCountOverflow => {
				formatter.write_str("maximal guide build variant count overflowed")
			}
			Self::OperatorCountOverflow => formatter.write_str("guide operator count exceeds u32"),
			Self::UnknownActionOperator { action_index, name } => write!(
				formatter,
				"action {action_index} references unknown materialized operator {name:?}"
			),
			Self::InvalidActionCell {
				action_index,
				location,
			} => write!(
				formatter,
				"action {action_index} has invalid MAA cell {location:?}"
			),
			Self::MissingDeployFacing { action_index } => {
				write!(formatter, "deploy action {action_index} has no facing")
			}
			Self::LocationOnlyAction { action_index } => write!(
				formatter,
				"action {action_index} selects its operator only by location"
			),
			Self::UnsupportedAction { action_index, kind } => {
				write!(
					formatter,
					"action {action_index} uses unsupported kind {kind}"
				)
			}
			Self::UnsupportedCondition {
				action_index,
				field,
			} => write!(
				formatter,
				"action {action_index} uses unsupported condition {field}"
			),
			Self::UnsupportedSkillUsage {
				operator_identity,
				source_name,
				skill_usage,
				skill_times,
			} => write!(
				formatter,
				"guide operator {source_name:?} ({operator_identity:?}) uses unsupported \
				 skill_usage {skill_usage} with skill_times {skill_times:?}; only 0 and 1 are \
				 supported"
			),
		}
	}
}

impl Error for ExpectedReplayPrepareError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::ArkData(error) => Some(error),
			_ => None,
		}
	}
}
