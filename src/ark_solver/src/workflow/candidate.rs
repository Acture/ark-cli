//! Candidate-plan materialization at the planning-to-replay seam.

use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::{
	ArkDataSnapshot, BattlePrefabSnapshot, GridPosition, OperatorBuild, SkillActivationType,
};
use crate::simulation::{SimTick, TickRate};

use super::strategy_binding::{
	prepare_replay_guide, replay_stage_difficulty, ExpectedReplayPrepareError,
};
use crate::maa_job::{MaaJobAction, MaaJobSeed};
use crate::planning::TickSpan;
use crate::{
	mechanics::domain::{
		Facing, MaterializedGuideAction, MaterializedGuidePolicy, OperatorInstanceId,
	},
	replay::{
		materialize_expected_replay_input, ExactReplayBuildBinding, ExpectedReplayAdapterError,
		ExpectedReplayCompileInput, ExpectedReplayInput, PreparedReplayGuide,
	},
};

/// Explicit deployment anchor used by an authoritative candidate replay.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CandidateReplayPlacement {
	pub cell: GridPosition,
	pub facing: Facing,
	pub requested_at: SimTick,
}

/// One exact candidate build, activation schedule, and deployment selected by the planner.
#[derive(Clone, Copy, Debug)]
pub struct CandidateReplayMemberPlan<'a> {
	/// Stable identity used for deterministic replay bindings and diagnostics.
	pub candidate_key: &'a str,
	pub build: &'a OperatorBuild,
	pub skill_active_spans: &'a [TickSpan],
	pub placement: CandidateReplayPlacement,
}

/// Ordered members replacing one source-slot deployment.
///
/// Slice order is deployment order. Deployment request ticks must be nondecreasing. Each member's
/// first manual skill activation may occur before a later member is deployed.
#[derive(Clone, Copy, Debug)]
pub struct CandidateReplayPlan<'a> {
	pub members: &'a [CandidateReplayMemberPlan<'a>],
}

/// Materializes an ordered replacement plan for complete expected replay.
///
/// The source binding and deployment are replaced by a chronological stream containing every
/// member deployment and first manual skill activation. Equal-tick deployments precede skills;
/// member order breaks remaining ties. Retained bindings and actions keep their relative order.
/// The persistent daemon activates manual replacements whenever their skills become ready again.
/// Base-only, automatic, and passive members do not gain manual daemon activations. To keep
/// ordering honest, the source guide must contain no `Deploy`, `Skill`, or `Retreat` command after
/// the source deployment.
///
/// `SpeedUp` and the final `SkillDaemon` may remain after deployment because the executor consumes
/// them directly.
pub fn materialize_candidate_expected_replay_input(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	seed: &MaaJobSeed,
	exact_bindings: &[ExactReplayBuildBinding],
	source_slot_name: &str,
	plan: CandidateReplayPlan<'_>,
	tick_rate: TickRate,
) -> Result<ExpectedReplayInput, CandidateExpectedReplayError> {
	let validated_members: Vec<ValidatedCandidateReplayMember<'_>> =
		validate_candidate_replay_plan(snapshot, plan)?;
	validate_source_action_order(seed, source_slot_name)?;
	let source_binding_index: usize =
		unique_source_binding_index(exact_bindings, source_slot_name)?;
	let original_guide: PreparedReplayGuide = prepare_replay_guide(snapshot, seed, exact_bindings)
		.map_err(CandidateExpectedReplayError::Prepare)?;
	let prepared: PreparedCandidateReplay = prepare_candidate_replay(
		exact_bindings,
		source_slot_name,
		source_binding_index,
		&validated_members,
		original_guide.actions,
		original_guide.policy,
	)?;
	materialize_expected_replay_input(
		snapshot,
		prefabs,
		ExpectedReplayCompileInput {
			stage_name: seed.stage_name(),
			difficulty: replay_stage_difficulty(seed.difficulty())
				.map_err(CandidateExpectedReplayError::Prepare)?,
			bindings: &prepared.bindings,
			guide: &prepared.guide,
			tick_rate,
		},
	)
	.map_err(CandidateExpectedReplayError::Adapter)
}

/// Materializes the original guide with one source-slot operator removed.
///
/// The stage, enemy schedule, retained builds, and retained guide order are unchanged. The source
/// operator and its named actions are removed from the seed before materialization, so every
/// roster-dependent retained effect is compiled again instead of surviving as stale state. This is
/// the retained-only counterfactual used to derive full-battle damage gaps.
pub fn materialize_source_removed_expected_replay_input(
	snapshot: &ArkDataSnapshot,
	prefabs: &BattlePrefabSnapshot,
	seed: &MaaJobSeed,
	exact_bindings: &[ExactReplayBuildBinding],
	source_slot_name: &str,
	tick_rate: TickRate,
) -> Result<ExpectedReplayInput, CandidateExpectedReplayError> {
	let _source_index: usize = unique_source_binding_index(exact_bindings, source_slot_name)?;
	let source_seed_operator_count: usize = seed
		.operators()
		.iter()
		.filter(|operator| operator.name() == source_slot_name)
		.count();
	match source_seed_operator_count {
		0 => {
			return Err(CandidateExpectedReplayError::MissingSourceSeedOperator {
				source_name: source_slot_name.to_owned(),
			});
		}
		1 => {}
		_ => {
			return Err(CandidateExpectedReplayError::DuplicateSourceSeedOperator {
				source_name: source_slot_name.to_owned(),
			});
		}
	}
	let retained_seed: MaaJobSeed = seed.clone_without_named_operator(source_slot_name);
	let retained_bindings: Vec<ExactReplayBuildBinding> = exact_bindings
		.iter()
		.filter(|binding| binding.source_name != source_slot_name)
		.cloned()
		.collect();
	let retained_guide: PreparedReplayGuide =
		prepare_replay_guide(snapshot, &retained_seed, &retained_bindings)
			.map_err(CandidateExpectedReplayError::Prepare)?;
	materialize_expected_replay_input(
		snapshot,
		prefabs,
		ExpectedReplayCompileInput {
			stage_name: retained_seed.stage_name(),
			difficulty: replay_stage_difficulty(retained_seed.difficulty())
				.map_err(CandidateExpectedReplayError::Prepare)?,
			bindings: &retained_bindings,
			guide: &retained_guide,
			tick_rate,
		},
	)
	.map_err(CandidateExpectedReplayError::Adapter)
}

#[derive(Debug)]
struct PreparedCandidateReplay {
	bindings: Vec<ExactReplayBuildBinding>,
	guide: PreparedReplayGuide,
}

#[derive(Clone, Copy, Debug)]
struct ValidatedCandidateReplayMember<'a> {
	plan: CandidateReplayMemberPlan<'a>,
	manual_skill_schedule: bool,
}

fn validate_candidate_replay_plan<'a>(
	snapshot: &ArkDataSnapshot,
	plan: CandidateReplayPlan<'a>,
) -> Result<Vec<ValidatedCandidateReplayMember<'a>>, CandidateExpectedReplayError> {
	validate_candidate_replay_plan_shape(plan)?;

	let mut operator_ids: BTreeSet<String> = BTreeSet::new();
	let mut validated: Vec<ValidatedCandidateReplayMember<'a>> =
		Vec::with_capacity(plan.members.len());
	for (member_index, member) in plan.members.iter().copied().enumerate() {
		let resolved = snapshot
			.operators()
			.resolve(member.build)
			.map_err(|error| {
				CandidateExpectedReplayError::Adapter(ExpectedReplayAdapterError::ArkData(error))
			})?;
		if !operator_ids.insert(resolved.operator_id().to_owned()) {
			return Err(CandidateExpectedReplayError::DuplicateCandidateOperator {
				operator_id: resolved.operator_id().to_owned(),
			});
		}
		let manual_skill_schedule: bool = validate_member_skill_schedule(
			member_index,
			resolved.selected_skill().activation_type(),
			member.skill_active_spans,
		)?;
		validated.push(ValidatedCandidateReplayMember {
			plan: member,
			manual_skill_schedule,
		});
	}
	validate_manual_skill_daemon_timing(&validated)?;
	Ok(validated)
}

fn validate_candidate_replay_plan_shape(
	plan: CandidateReplayPlan<'_>,
) -> Result<(), CandidateExpectedReplayError> {
	if plan.members.is_empty() {
		return Err(CandidateExpectedReplayError::EmptyCandidatePlan);
	}

	let mut candidate_keys: BTreeSet<&str> = BTreeSet::new();
	let mut deployment_cells: BTreeSet<GridPosition> = BTreeSet::new();
	let mut previous_deploy_tick: Option<u64> = None;
	for (member_index, member) in plan.members.iter().copied().enumerate() {
		if member.candidate_key.trim().is_empty() {
			return Err(CandidateExpectedReplayError::BlankCandidateKey { member_index });
		}
		if !candidate_keys.insert(member.candidate_key) {
			return Err(CandidateExpectedReplayError::DuplicateCandidateKey {
				candidate_key: member.candidate_key.to_owned(),
			});
		}
		if !deployment_cells.insert(member.placement.cell) {
			return Err(CandidateExpectedReplayError::DuplicateCandidateCell {
				cell: member.placement.cell,
			});
		}
		let deploy_tick: u64 = member.placement.requested_at.get();
		if let Some(previous_tick) = previous_deploy_tick {
			if deploy_tick < previous_tick {
				return Err(CandidateExpectedReplayError::DeploymentTicksOutOfOrder {
					previous_member_index: member_index - 1,
					previous_tick,
					member_index,
					tick: deploy_tick,
				});
			}
		}
		validate_active_spans(member_index, member.skill_active_spans)?;
		if let Some(first_span) = member.skill_active_spans.first() {
			if first_span.start < deploy_tick {
				return Err(CandidateExpectedReplayError::SkillBeforeDeployment {
					member_index,
					skill_tick: first_span.start,
					deploy_tick,
				});
			}
		}
		previous_deploy_tick = Some(deploy_tick);
	}
	Ok(())
}

fn validate_manual_skill_daemon_timing(
	members: &[ValidatedCandidateReplayMember<'_>],
) -> Result<(), CandidateExpectedReplayError> {
	let replacement_stream_end_tick: u64 = members
		.iter()
		.flat_map(|member| {
			std::iter::once(member.plan.placement.requested_at.get()).chain(
				member
					.manual_skill_schedule
					.then(|| member.plan.skill_active_spans.first())
					.flatten()
					.map(|span| span.start),
			)
		})
		.max()
		.expect("validated candidate plan is non-empty");
	for (member_index, member) in members.iter().enumerate() {
		if !member.manual_skill_schedule {
			continue;
		}
		if let Some(second_span) = member.plan.skill_active_spans.get(1) {
			if second_span.start < replacement_stream_end_tick {
				return Err(CandidateExpectedReplayError::RecurringSkillBeforeDaemon {
					member_index,
					skill_tick: second_span.start,
					daemon_tick: replacement_stream_end_tick,
				});
			}
		}
	}
	Ok(())
}

fn validate_member_skill_schedule(
	member_index: usize,
	activation_type: &SkillActivationType,
	active_spans: &[TickSpan],
) -> Result<bool, CandidateExpectedReplayError> {
	match activation_type {
		SkillActivationType::Manual => Ok(!active_spans.is_empty()),
		SkillActivationType::Auto | SkillActivationType::Passive => Ok(false),
		SkillActivationType::Unknown(value) => {
			Err(CandidateExpectedReplayError::UnsupportedActivationType {
				member_index,
				value: value.clone(),
			})
		}
	}
}

fn prepare_candidate_replay(
	exact_bindings: &[ExactReplayBuildBinding],
	source_slot_name: &str,
	source_binding_index: usize,
	members: &[ValidatedCandidateReplayMember<'_>],
	original_actions: Vec<MaterializedGuideAction>,
	original_policy: MaterializedGuidePolicy,
) -> Result<PreparedCandidateReplay, CandidateExpectedReplayError> {
	let source_operator: OperatorInstanceId = operator_id(source_binding_index)?;
	let source_deployment_indices: Vec<usize> = original_actions
		.iter()
		.enumerate()
		.filter_map(|(action_index, action)| {
			matches!(
				action,
				MaterializedGuideAction::Deploy { operator, .. }
					if *operator == source_operator
			)
			.then_some(action_index)
		})
		.collect();
	let [source_deployment_index] = source_deployment_indices.as_slice() else {
		return if source_deployment_indices.is_empty() {
			Err(CandidateExpectedReplayError::MissingSourceDeployment {
				source_name: source_slot_name.to_owned(),
			})
		} else {
			Err(CandidateExpectedReplayError::DuplicateSourceDeployment {
				source_name: source_slot_name.to_owned(),
			})
		};
	};

	let mut bindings: Vec<ExactReplayBuildBinding> = Vec::with_capacity(
		exact_bindings
			.len()
			.checked_sub(1)
			.and_then(|count| count.checked_add(members.len()))
			.ok_or(CandidateExpectedReplayError::OperatorCountOverflow)?,
	);
	for (binding_index, binding) in exact_bindings.iter().enumerate() {
		if binding_index == source_binding_index {
			for member in members {
				bindings.push(ExactReplayBuildBinding {
					source_name: member.plan.candidate_key.to_owned(),
					build: member.plan.build.clone(),
					skill_usage: u8::from(member.manual_skill_schedule),
					skill_times: None,
				});
			}
		} else {
			bindings.push(binding.clone());
		}
	}
	let mut operator_ids: BTreeMap<String, OperatorInstanceId> = BTreeMap::new();
	for (binding_index, binding) in bindings.iter().enumerate() {
		let operator: OperatorInstanceId = operator_id(binding_index)?;
		if operator_ids
			.insert(binding.source_name.clone(), operator)
			.is_some()
		{
			return Err(CandidateExpectedReplayError::CandidateBindingNameConflict {
				name: binding.source_name.clone(),
			});
		}
	}

	let replacement_actions: Vec<MaterializedGuideAction> =
		candidate_replacement_actions(members, &operator_ids);
	let final_replacement_action_at: SimTick = replacement_actions
		.last()
		.expect("validated candidate plan produces at least one deployment")
		.requested_at();

	let mut actions: Vec<MaterializedGuideAction> = Vec::with_capacity(
		original_actions
			.len()
			.checked_sub(1)
			.and_then(|count| count.checked_add(members.len()))
			.and_then(|count| {
				count.checked_add(
					members
						.iter()
						.filter(|member| member.manual_skill_schedule)
						.count(),
				)
			})
			.ok_or(CandidateExpectedReplayError::ActionCountOverflow)?,
	);
	for (action_index, action) in original_actions.into_iter().enumerate() {
		if action_index == *source_deployment_index {
			actions.extend(replacement_actions.iter().cloned());
			continue;
		}
		actions.push(remap_retained_action(
			action,
			exact_bindings,
			source_binding_index,
			&operator_ids,
			(action_index > *source_deployment_index).then_some(final_replacement_action_at),
		)?);
	}
	validate_final_skill_daemon(&actions)?;

	let mut skill_usage: BTreeMap<OperatorInstanceId, u8> = BTreeMap::new();
	for (binding_index, binding) in exact_bindings.iter().enumerate() {
		if binding_index == source_binding_index {
			continue;
		}
		let old_operator: OperatorInstanceId = operator_id(binding_index)?;
		let new_operator: OperatorInstanceId = operator_ids[&binding.source_name];
		let usage: u8 = original_policy.skill_usage(old_operator).ok_or_else(|| {
			CandidateExpectedReplayError::MissingRetainedSkillPolicy {
				source_name: binding.source_name.clone(),
			}
		})?;
		skill_usage.insert(new_operator, usage);
	}
	for member in members {
		skill_usage.insert(
			operator_ids[member.plan.candidate_key],
			u8::from(member.manual_skill_schedule),
		);
	}

	Ok(PreparedCandidateReplay {
		bindings,
		guide: PreparedReplayGuide {
			actions,
			policy: MaterializedGuidePolicy::new(skill_usage),
			operator_ids,
		},
	})
}

fn candidate_replacement_actions(
	members: &[ValidatedCandidateReplayMember<'_>],
	operator_ids: &BTreeMap<String, OperatorInstanceId>,
) -> Vec<MaterializedGuideAction> {
	let mut ordered_actions: Vec<(u64, u8, usize, MaterializedGuideAction)> =
		Vec::with_capacity(members.len().saturating_mul(2));
	for (member_index, member) in members.iter().enumerate() {
		let operator: OperatorInstanceId = operator_ids[member.plan.candidate_key];
		ordered_actions.push((
			member.plan.placement.requested_at.get(),
			0,
			member_index,
			MaterializedGuideAction::Deploy {
				requested_at: member.plan.placement.requested_at,
				operator,
				cell: member.plan.placement.cell,
				facing: member.plan.placement.facing,
			},
		));
		if member.manual_skill_schedule {
			let first_span: &TickSpan = member
				.plan
				.skill_active_spans
				.first()
				.expect("manual schedule has at least one active span");
			ordered_actions.push((
				first_span.start,
				1,
				member_index,
				MaterializedGuideAction::ActivateSkill {
					requested_at: SimTick::new(first_span.start),
					operator,
				},
			));
		}
	}
	ordered_actions.sort_by_key(|(tick, kind, member_index, _)| (*tick, *kind, *member_index));
	ordered_actions
		.into_iter()
		.map(|(_, _, _, action)| action)
		.collect()
}

fn operator_id(binding_index: usize) -> Result<OperatorInstanceId, CandidateExpectedReplayError> {
	u32::try_from(binding_index)
		.map(OperatorInstanceId::new)
		.map_err(|_| CandidateExpectedReplayError::OperatorCountOverflow)
}

fn remap_retained_action(
	action: MaterializedGuideAction,
	old_bindings: &[ExactReplayBuildBinding],
	source_binding_index: usize,
	new_operator_ids: &BTreeMap<String, OperatorInstanceId>,
	minimum_requested_at: Option<SimTick>,
) -> Result<MaterializedGuideAction, CandidateExpectedReplayError> {
	let remap =
		|operator: OperatorInstanceId| -> Result<OperatorInstanceId, CandidateExpectedReplayError> {
			let binding_index: usize = usize::try_from(operator.get())
				.map_err(|_| CandidateExpectedReplayError::OperatorCountOverflow)?;
			if binding_index == source_binding_index {
				return Err(CandidateExpectedReplayError::UnexpectedSourceCommand);
			}
			let binding: &ExactReplayBuildBinding = old_bindings
				.get(binding_index)
				.ok_or(CandidateExpectedReplayError::UnknownRetainedOperator { operator })?;
			new_operator_ids
				.get(&binding.source_name)
				.copied()
				.ok_or_else(|| CandidateExpectedReplayError::MissingRetainedBinding {
					source_name: binding.source_name.clone(),
				})
		};
	let clamp = |requested_at: SimTick| {
		minimum_requested_at.map_or(requested_at, |minimum| requested_at.max(minimum))
	};
	match action {
		MaterializedGuideAction::Deploy {
			requested_at,
			operator,
			cell,
			facing,
		} => Ok(MaterializedGuideAction::Deploy {
			requested_at: clamp(requested_at),
			operator: remap(operator)?,
			cell,
			facing,
		}),
		MaterializedGuideAction::ActivateSkill {
			requested_at,
			operator,
		} => Ok(MaterializedGuideAction::ActivateSkill {
			requested_at: clamp(requested_at),
			operator: remap(operator)?,
		}),
		MaterializedGuideAction::Retreat {
			requested_at,
			operator,
		} => Ok(MaterializedGuideAction::Retreat {
			requested_at: clamp(requested_at),
			operator: remap(operator)?,
		}),
		MaterializedGuideAction::SpeedUp { requested_at } => Ok(MaterializedGuideAction::SpeedUp {
			requested_at: clamp(requested_at),
		}),
		MaterializedGuideAction::SkillDaemon { requested_at } => {
			Ok(MaterializedGuideAction::SkillDaemon {
				requested_at: clamp(requested_at),
			})
		}
	}
}

fn unique_source_binding_index(
	exact_bindings: &[ExactReplayBuildBinding],
	source_slot_name: &str,
) -> Result<usize, CandidateExpectedReplayError> {
	let matching_indices: Vec<usize> = exact_bindings
		.iter()
		.enumerate()
		.filter_map(|(index, binding)| (binding.source_name == source_slot_name).then_some(index))
		.collect();
	let [binding_index] = matching_indices.as_slice() else {
		return if matching_indices.is_empty() {
			Err(CandidateExpectedReplayError::MissingSourceBinding {
				source_name: source_slot_name.to_owned(),
			})
		} else {
			Err(CandidateExpectedReplayError::DuplicateSourceBinding {
				source_name: source_slot_name.to_owned(),
			})
		};
	};
	Ok(*binding_index)
}

fn validate_final_skill_daemon(
	actions: &[MaterializedGuideAction],
) -> Result<(), CandidateExpectedReplayError> {
	let daemon_indices: Vec<usize> = actions
		.iter()
		.enumerate()
		.filter_map(|(index, action)| {
			matches!(action, MaterializedGuideAction::SkillDaemon { .. }).then_some(index)
		})
		.collect();
	let [daemon_index] = daemon_indices.as_slice() else {
		return if daemon_indices.is_empty() {
			Err(CandidateExpectedReplayError::MissingSkillDaemon)
		} else {
			Err(CandidateExpectedReplayError::DuplicateSkillDaemon)
		};
	};
	if *daemon_index + 1 != actions.len() {
		return Err(CandidateExpectedReplayError::SkillDaemonNotLast {
			action_index: *daemon_index,
		});
	}
	Ok(())
}

fn validate_active_spans(
	member_index: usize,
	active_spans: &[TickSpan],
) -> Result<(), CandidateExpectedReplayError> {
	for (index, span) in active_spans.iter().enumerate() {
		if span.start >= span.end {
			return Err(CandidateExpectedReplayError::InvalidActiveSpan {
				member_index,
				index,
				start: span.start,
				end: span.end,
			});
		}
		if let Some(previous) = index
			.checked_sub(1)
			.and_then(|value| active_spans.get(value))
		{
			if previous.end > span.start {
				return Err(CandidateExpectedReplayError::OverlappingActiveSpans {
					member_index,
					previous_index: index - 1,
					previous_end: previous.end,
					index,
					start: span.start,
				});
			}
		}
	}
	Ok(())
}

fn validate_source_action_order(
	seed: &MaaJobSeed,
	source_slot_name: &str,
) -> Result<(), CandidateExpectedReplayError> {
	let deployment_indices: Vec<usize> = seed
		.actions()
		.iter()
		.enumerate()
		.filter_map(|(index, action)| match action {
			MaaJobAction::Deploy { name, .. } if name == source_slot_name => Some(index),
			_ => None,
		})
		.collect();
	let [deployment_index] = deployment_indices.as_slice() else {
		return if deployment_indices.is_empty() {
			Err(CandidateExpectedReplayError::MissingSourceDeployment {
				source_name: source_slot_name.to_owned(),
			})
		} else {
			Err(CandidateExpectedReplayError::DuplicateSourceDeployment {
				source_name: source_slot_name.to_owned(),
			})
		};
	};
	for (action_index, action) in seed.actions().iter().enumerate() {
		if action_index < *deployment_index && targets_source_command(action, source_slot_name) {
			return Err(
				CandidateExpectedReplayError::SourceCommandBeforeDeployment {
					source_name: source_slot_name.to_owned(),
					action_index,
					kind: command_kind(action).expect("source command has a command kind"),
				},
			);
		}
		if action_index > *deployment_index {
			if let Some(kind) = command_kind(action) {
				return Err(CandidateExpectedReplayError::CommandAfterSourceDeployment {
					source_name: source_slot_name.to_owned(),
					action_index,
					kind,
				});
			}
		}
	}
	Ok(())
}

fn targets_source_command(action: &MaaJobAction, source_slot_name: &str) -> bool {
	match action {
		MaaJobAction::Skill {
			name: Some(name), ..
		}
		| MaaJobAction::Retreat {
			name: Some(name), ..
		} => name == source_slot_name,
		_ => false,
	}
}

fn command_kind(action: &MaaJobAction) -> Option<&'static str> {
	match action {
		MaaJobAction::Deploy { .. } => Some("Deploy"),
		MaaJobAction::Skill { .. } => Some("Skill"),
		MaaJobAction::Retreat { .. } => Some("Retreat"),
		_ => None,
	}
}

/// Failure to build one source-slot candidate replay without changing guide semantics.
#[derive(Debug)]
pub enum CandidateExpectedReplayError {
	Adapter(ExpectedReplayAdapterError),
	Prepare(ExpectedReplayPrepareError),
	EmptyCandidatePlan,
	BlankCandidateKey {
		member_index: usize,
	},
	DuplicateCandidateKey {
		candidate_key: String,
	},
	DuplicateCandidateOperator {
		operator_id: String,
	},
	DuplicateCandidateCell {
		cell: GridPosition,
	},
	DeploymentTicksOutOfOrder {
		previous_member_index: usize,
		previous_tick: u64,
		member_index: usize,
		tick: u64,
	},
	CandidateBindingNameConflict {
		name: String,
	},
	MissingSourceBinding {
		source_name: String,
	},
	DuplicateSourceBinding {
		source_name: String,
	},
	MissingSourceDeployment {
		source_name: String,
	},
	DuplicateSourceDeployment {
		source_name: String,
	},
	SourceCommandBeforeDeployment {
		source_name: String,
		action_index: usize,
		kind: &'static str,
	},
	CommandAfterSourceDeployment {
		source_name: String,
		action_index: usize,
		kind: &'static str,
	},
	InvalidActiveSpan {
		member_index: usize,
		index: usize,
		start: u64,
		end: u64,
	},
	OverlappingActiveSpans {
		member_index: usize,
		previous_index: usize,
		previous_end: u64,
		index: usize,
		start: u64,
	},
	SkillBeforeDeployment {
		member_index: usize,
		skill_tick: u64,
		deploy_tick: u64,
	},
	RecurringSkillBeforeDaemon {
		member_index: usize,
		skill_tick: u64,
		daemon_tick: u64,
	},
	MissingSkillDaemon,
	DuplicateSkillDaemon,
	SkillDaemonNotLast {
		action_index: usize,
	},
	UnsupportedActivationType {
		member_index: usize,
		value: String,
	},
	MissingRetainedSkillPolicy {
		source_name: String,
	},
	MissingRetainedBinding {
		source_name: String,
	},
	UnknownRetainedOperator {
		operator: OperatorInstanceId,
	},
	UnexpectedSourceCommand,
	MissingSourceSeedOperator {
		source_name: String,
	},
	DuplicateSourceSeedOperator {
		source_name: String,
	},
	OperatorCountOverflow,
	ActionCountOverflow,
}

impl Display for CandidateExpectedReplayError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Adapter(error) => Display::fmt(error, formatter),
			Self::Prepare(error) => Display::fmt(error, formatter),
			Self::EmptyCandidatePlan => {
				formatter.write_str("candidate replay plan must contain at least one member")
			}
			Self::BlankCandidateKey { member_index } => {
				write!(
					formatter,
					"candidate replay member {member_index} has a blank key"
				)
			}
			Self::DuplicateCandidateKey { candidate_key } => {
				write!(
					formatter,
					"candidate replay plan repeats key {candidate_key:?}"
				)
			}
			Self::DuplicateCandidateOperator { operator_id } => write!(
				formatter,
				"candidate replay plan repeats operator {operator_id:?}"
			),
			Self::DuplicateCandidateCell { cell } => write!(
				formatter,
				"candidate replay plan repeats deployment cell ({}, {})",
				cell.row, cell.col
			),
			Self::DeploymentTicksOutOfOrder {
				previous_member_index,
				previous_tick,
				member_index,
				tick,
			} => write!(
				formatter,
				"candidate replay deployment ticks are out of order: member \
				 {previous_member_index} requests tick {previous_tick}, member {member_index} \
				 requests tick {tick}"
			),
			Self::CandidateBindingNameConflict { name } => write!(
				formatter,
				"candidate key {name:?} conflicts with another replay binding"
			),
			Self::MissingSourceBinding { source_name } => {
				write!(
					formatter,
					"source slot {source_name:?} has no exact build binding"
				)
			}
			Self::DuplicateSourceBinding { source_name } => write!(
				formatter,
				"source slot {source_name:?} has more than one exact build binding"
			),
			Self::MissingSourceDeployment { source_name } => {
				write!(
					formatter,
					"source slot {source_name:?} has no Deploy action"
				)
			}
			Self::DuplicateSourceDeployment { source_name } => {
				write!(
					formatter,
					"source slot {source_name:?} has more than one Deploy action"
				)
			}
			Self::SourceCommandBeforeDeployment {
				source_name,
				action_index,
				kind,
			} => write!(
				formatter,
				"source slot {source_name:?} has {kind} action {action_index} before deployment"
			),
			Self::CommandAfterSourceDeployment {
				source_name,
				action_index,
				kind,
			} => write!(
				formatter,
				"cannot append candidate skill schedule: source slot {source_name:?} is followed by \
				 {kind} command {action_index}"
			),
			Self::InvalidActiveSpan {
				member_index,
				index,
				start,
				end,
			} => write!(
				formatter,
				"candidate replay member {member_index} active span {index} is not a non-empty \
				 interval: [{start}, {end})"
			),
			Self::OverlappingActiveSpans {
				member_index,
				previous_index,
				previous_end,
				index,
				start,
			} => write!(
				formatter,
				"candidate replay member {member_index} active spans overlap or are out of order: \
				 span {previous_index} ends at {previous_end}, span {index} starts at {start}"
			),
			Self::SkillBeforeDeployment {
				member_index,
				skill_tick,
				deploy_tick,
			} => write!(
				formatter,
				"candidate replay member {member_index} skill starts at tick {skill_tick} before \
				 deployment tick {deploy_tick}"
			),
			Self::RecurringSkillBeforeDaemon {
				member_index,
				skill_tick,
				daemon_tick,
			} => write!(
				formatter,
				"candidate replay member {member_index} recurring skill starts at tick \
				 {skill_tick} before the replacement stream can start SkillDaemon at tick \
				 {daemon_tick}"
			),
			Self::MissingSkillDaemon => {
				formatter.write_str("candidate replay requires one final SkillDaemon action")
			}
			Self::DuplicateSkillDaemon => {
				formatter.write_str("candidate replay has more than one SkillDaemon action")
			}
			Self::SkillDaemonNotLast { action_index } => write!(
				formatter,
				"candidate replay SkillDaemon action {action_index} must be final"
			),
			Self::UnsupportedActivationType {
				member_index,
				value,
			} => {
				write!(
					formatter,
					"candidate replay member {member_index} skill has unknown activation type \
					 {value:?}"
				)
			}
			Self::MissingRetainedSkillPolicy { source_name } => write!(
				formatter,
				"retained replay binding {source_name:?} has no skill policy"
			),
			Self::MissingRetainedBinding { source_name } => write!(
				formatter,
				"retained replay binding {source_name:?} disappeared during remapping"
			),
			Self::UnknownRetainedOperator { operator } => write!(
				formatter,
				"retained replay action references unknown operator instance {}",
				operator.get()
			),
			Self::UnexpectedSourceCommand => {
				formatter.write_str("source replay operator has a command outside its deployment")
			}
			Self::MissingSourceSeedOperator { source_name } => write!(
				formatter,
				"source slot {source_name:?} has no materialized seed operator"
			),
			Self::DuplicateSourceSeedOperator { source_name } => write!(
				formatter,
				"source slot {source_name:?} has more than one materialized seed operator"
			),
			Self::OperatorCountOverflow => {
				formatter.write_str("candidate operator index exceeds u32")
			}
			Self::ActionCountOverflow => {
				formatter.write_str("candidate replay action count overflowed usize")
			}
		}
	}
}

impl Error for CandidateExpectedReplayError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::Adapter(error) => Some(error),
			Self::Prepare(error) => Some(error),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::{collections::BTreeMap, num::NonZeroU32};

	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, GridPosition, OperatorBuild, OperatorIdentity,
		OperatorModule, PotentialRank, SkillActivationType,
	};
	use crate::simulation::{SimTick, TickRate};

	use super::{
		candidate_replacement_actions, materialize_candidate_expected_replay_input,
		prepare_candidate_replay, validate_candidate_replay_plan,
		validate_candidate_replay_plan_shape, validate_manual_skill_daemon_timing,
		validate_member_skill_schedule, validate_source_action_order, CandidateExpectedReplayError,
		CandidateReplayMemberPlan, CandidateReplayPlacement, CandidateReplayPlan,
		MaterializedGuideAction, OperatorInstanceId, ValidatedCandidateReplayMember,
	};
	use crate::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSeed};
	use crate::mechanics::domain::{Facing, MaterializedGuidePolicy};
	use crate::planning::TickSpan;
	use crate::replay::{
		execute_expected_replay, ExactReplayBuildBinding, ExpectedReplayAdapterError,
	};
	use crate::verification_artifact::ArtifactFacing;

	fn binding(name: &str, operator_id: &str, skill_usage: u8) -> ExactReplayBuildBinding {
		ExactReplayBuildBinding {
			source_name: name.to_owned(),
			build: build(operator_id),
			skill_usage,
			skill_times: None,
		}
	}

	fn build(operator_id: &str) -> OperatorBuild {
		OperatorBuild::new(
			OperatorIdentity::Id(operator_id.to_owned()),
			2,
			90,
			200,
			2,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		)
	}

	fn bd7_seed() -> MaaJobSeed {
		parse_maa_job(include_str!(
			"../../tests/fixtures/maa_job_bd7_sanitized.json"
		))
		.unwrap()
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.unwrap()
		.next()
		.unwrap()
		.materialize()
	}

	fn local_battle_snapshots() -> (ArkDataSnapshot, BattlePrefabSnapshot) {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot = ArkDataSnapshot::load(&gamedata_path)
			.expect("ignored real-data test requires a verified local gamedata snapshot");
		let prefabs = BattlePrefabSnapshot::load(&prefab_path)
			.expect("ignored real-data test requires a verified local battle-prefab snapshot");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("local snapshots must target the same gamedata revision");
		(snapshot, prefabs)
	}

	fn seed_with_trailing_deploy() -> MaaJobSeed {
		parse_maa_job(
			r#"{
				"minimum_required": "v6.0.0",
				"stage_name": "test_stage",
				"actions": [
					{
						"type": "Deploy",
						"name": "source",
						"location": [0, 0],
						"direction": "Right"
					},
					{
						"type": "Deploy",
						"name": "retained",
						"location": [1, 0],
						"direction": "Left"
					}
				],
				"opers": [
					{"name": "source", "skill": 1, "skill_usage": 0},
					{"name": "retained", "skill": 1, "skill_usage": 0}
				],
				"difficulty": 3
			}"#,
		)
		.unwrap()
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.unwrap()
		.next()
		.unwrap()
		.materialize()
	}

	fn member<'a>(
		candidate_key: &'a str,
		build: &'a OperatorBuild,
		skill_active_spans: &'a [TickSpan],
		cell: GridPosition,
		facing: Facing,
		requested_at: u64,
	) -> CandidateReplayMemberPlan<'a> {
		CandidateReplayMemberPlan {
			candidate_key,
			build,
			skill_active_spans,
			placement: CandidateReplayPlacement {
				cell,
				facing,
				requested_at: SimTick::new(requested_at),
			},
		}
	}

	fn policy(entries: &[(u32, u8)]) -> MaterializedGuidePolicy {
		MaterializedGuidePolicy::new(
			entries
				.iter()
				.map(|(operator, usage)| (OperatorInstanceId::new(*operator), *usage))
				.collect(),
		)
	}

	#[test]
	fn replaces_one_source_with_ordered_members_and_preserves_retained_state() {
		let bindings = vec![
			binding("retained-before", "char_retained_before", 0),
			binding("source", "char_source", 1),
			binding("retained-after", "char_retained_after", 1),
		];
		let candidate_a = build("char_candidate_a");
		let candidate_b = build("char_candidate_b");
		let spans_a = [
			TickSpan { start: 15, end: 30 },
			TickSpan { start: 40, end: 50 },
		];
		let spans_b = [TickSpan { start: 20, end: 30 }];
		let members = [
			ValidatedCandidateReplayMember {
				plan: member(
					"candidate:a",
					&candidate_a,
					&spans_a,
					GridPosition { row: 0, col: 0 },
					Facing::Right,
					10,
				),
				manual_skill_schedule: true,
			},
			ValidatedCandidateReplayMember {
				plan: member(
					"candidate:b",
					&candidate_b,
					&spans_b,
					GridPosition { row: 2, col: 0 },
					Facing::Left,
					20,
				),
				manual_skill_schedule: true,
			},
		];
		let original_actions = vec![
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(0),
				operator: OperatorInstanceId::new(0),
				cell: GridPosition { row: 0, col: 0 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(5),
				operator: OperatorInstanceId::new(2),
				cell: GridPosition { row: 1, col: 0 },
				facing: Facing::Left,
			},
			MaterializedGuideAction::Retreat {
				requested_at: SimTick::new(6),
				operator: OperatorInstanceId::new(0),
			},
			MaterializedGuideAction::Deploy {
				requested_at: SimTick::new(7),
				operator: OperatorInstanceId::new(1),
				cell: GridPosition { row: 3, col: 0 },
				facing: Facing::Right,
			},
			MaterializedGuideAction::SpeedUp {
				requested_at: SimTick::new(0),
			},
			MaterializedGuideAction::SkillDaemon {
				requested_at: SimTick::new(0),
			},
		];

		let prepared = prepare_candidate_replay(
			&bindings,
			"source",
			1,
			&members,
			original_actions,
			policy(&[(0, 0), (1, 1), (2, 1)]),
		)
		.unwrap();

		assert_eq!(prepared.bindings[0], bindings[0]);
		assert_eq!(prepared.bindings[1].source_name, "candidate:a");
		assert_eq!(prepared.bindings[1].build, candidate_a);
		assert_eq!(prepared.bindings[1].skill_usage, 1);
		assert_eq!(prepared.bindings[1].skill_times, None);
		assert_eq!(prepared.bindings[2].source_name, "candidate:b");
		assert_eq!(prepared.bindings[2].build, candidate_b);
		assert_eq!(prepared.bindings[3], bindings[2]);
		assert_eq!(
			prepared.guide.operator_ids,
			BTreeMap::from([
				("candidate:a".to_owned(), OperatorInstanceId::new(1)),
				("candidate:b".to_owned(), OperatorInstanceId::new(2)),
				("retained-after".to_owned(), OperatorInstanceId::new(3)),
				("retained-before".to_owned(), OperatorInstanceId::new(0)),
			])
		);
		assert_eq!(
			prepared.guide.policy.entries(),
			&BTreeMap::from([
				(OperatorInstanceId::new(0), 0),
				(OperatorInstanceId::new(1), 1),
				(OperatorInstanceId::new(2), 1),
				(OperatorInstanceId::new(3), 1),
			])
		);
		assert_eq!(
			prepared.guide.actions,
			vec![
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(0),
					operator: OperatorInstanceId::new(0),
					cell: GridPosition { row: 0, col: 0 },
					facing: Facing::Right,
				},
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(5),
					operator: OperatorInstanceId::new(3),
					cell: GridPosition { row: 1, col: 0 },
					facing: Facing::Left,
				},
				MaterializedGuideAction::Retreat {
					requested_at: SimTick::new(6),
					operator: OperatorInstanceId::new(0),
				},
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(10),
					operator: OperatorInstanceId::new(1),
					cell: GridPosition { row: 0, col: 0 },
					facing: Facing::Right,
				},
				MaterializedGuideAction::ActivateSkill {
					requested_at: SimTick::new(15),
					operator: OperatorInstanceId::new(1),
				},
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(20),
					operator: OperatorInstanceId::new(2),
					cell: GridPosition { row: 2, col: 0 },
					facing: Facing::Left,
				},
				MaterializedGuideAction::ActivateSkill {
					requested_at: SimTick::new(20),
					operator: OperatorInstanceId::new(2),
				},
				MaterializedGuideAction::SpeedUp {
					requested_at: SimTick::new(20),
				},
				MaterializedGuideAction::SkillDaemon {
					requested_at: SimTick::new(20),
				},
			]
		);
	}

	#[test]
	fn base_only_candidate_does_not_enable_manual_skill_daemon() {
		let bindings = vec![binding("source", "char_source", 1)];
		let candidate = build("char_103_angel");
		let members = [ValidatedCandidateReplayMember {
			plan: member(
				"candidate",
				&candidate,
				&[],
				GridPosition { row: 0, col: 0 },
				Facing::Right,
				0,
			),
			manual_skill_schedule: false,
		}];

		let prepared = prepare_candidate_replay(
			&bindings,
			"source",
			0,
			&members,
			vec![
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(0),
					operator: OperatorInstanceId::new(0),
					cell: GridPosition { row: 1, col: 0 },
					facing: Facing::Right,
				},
				MaterializedGuideAction::SkillDaemon {
					requested_at: SimTick::new(0),
				},
			],
			policy(&[(0, 1)]),
		)
		.unwrap();

		assert_eq!(prepared.bindings[0].skill_usage, 0);
		assert_eq!(prepared.bindings[0].skill_times, None);
		assert!(!prepared
			.guide
			.actions
			.iter()
			.any(|action| matches!(action, MaterializedGuideAction::ActivateSkill { .. })));
	}

	#[test]
	fn orders_deployments_and_first_manual_skills_chronologically() {
		let candidate_a = build("char_candidate_a");
		let candidate_b = build("char_candidate_b");
		let spans_a = [TickSpan { start: 15, end: 30 }];
		let spans_b = [TickSpan { start: 20, end: 30 }];
		let members = [
			ValidatedCandidateReplayMember {
				plan: member(
					"candidate:a",
					&candidate_a,
					&spans_a,
					GridPosition { row: 0, col: 0 },
					Facing::Right,
					10,
				),
				manual_skill_schedule: true,
			},
			ValidatedCandidateReplayMember {
				plan: member(
					"candidate:b",
					&candidate_b,
					&spans_b,
					GridPosition { row: 1, col: 0 },
					Facing::Left,
					20,
				),
				manual_skill_schedule: true,
			},
		];
		let operator_ids = BTreeMap::from([
			("candidate:a".to_owned(), OperatorInstanceId::new(1)),
			("candidate:b".to_owned(), OperatorInstanceId::new(2)),
		]);

		assert_eq!(
			candidate_replacement_actions(&members, &operator_ids),
			vec![
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(10),
					operator: OperatorInstanceId::new(1),
					cell: GridPosition { row: 0, col: 0 },
					facing: Facing::Right,
				},
				MaterializedGuideAction::ActivateSkill {
					requested_at: SimTick::new(15),
					operator: OperatorInstanceId::new(1),
				},
				MaterializedGuideAction::Deploy {
					requested_at: SimTick::new(20),
					operator: OperatorInstanceId::new(2),
					cell: GridPosition { row: 1, col: 0 },
					facing: Facing::Left,
				},
				MaterializedGuideAction::ActivateSkill {
					requested_at: SimTick::new(20),
					operator: OperatorInstanceId::new(2),
				},
			]
		);
	}

	#[test]
	fn rejects_empty_candidate_plan() {
		let error =
			validate_candidate_replay_plan_shape(CandidateReplayPlan { members: &[] }).unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::EmptyCandidatePlan
		));
	}

	#[test]
	fn rejects_duplicate_candidate_keys() {
		let candidate_a = build("char_candidate_a");
		let candidate_b = build("char_candidate_b");
		let members = [
			member(
				"duplicate",
				&candidate_a,
				&[],
				GridPosition { row: 0, col: 0 },
				Facing::Right,
				0,
			),
			member(
				"duplicate",
				&candidate_b,
				&[],
				GridPosition { row: 1, col: 0 },
				Facing::Right,
				1,
			),
		];

		let error = validate_candidate_replay_plan_shape(CandidateReplayPlan { members: &members })
			.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::DuplicateCandidateKey {
				ref candidate_key
			} if candidate_key == "duplicate"
		));
	}

	#[test]
	fn rejects_duplicate_candidate_cells() {
		let candidate_a = build("char_candidate_a");
		let candidate_b = build("char_candidate_b");
		let duplicate_cell = GridPosition { row: 0, col: 0 };
		let members = [
			member("a", &candidate_a, &[], duplicate_cell, Facing::Right, 0),
			member("b", &candidate_b, &[], duplicate_cell, Facing::Right, 1),
		];

		let error = validate_candidate_replay_plan_shape(CandidateReplayPlan { members: &members })
			.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::DuplicateCandidateCell { cell }
				if cell == duplicate_cell
		));
	}

	#[test]
	fn rejects_deployment_ticks_out_of_order() {
		let candidate_a = build("char_candidate_a");
		let candidate_b = build("char_candidate_b");
		let members = [
			member(
				"a",
				&candidate_a,
				&[],
				GridPosition { row: 0, col: 0 },
				Facing::Right,
				20,
			),
			member(
				"b",
				&candidate_b,
				&[],
				GridPosition { row: 1, col: 0 },
				Facing::Right,
				10,
			),
		];

		let error = validate_candidate_replay_plan_shape(CandidateReplayPlan { members: &members })
			.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::DeploymentTicksOutOfOrder {
				previous_member_index: 0,
				previous_tick: 20,
				member_index: 1,
				tick: 10,
			}
		));
	}

	#[test]
	fn rejects_overlapping_candidate_skill_spans() {
		let candidate = build("char_candidate");
		let spans = [
			TickSpan { start: 10, end: 20 },
			TickSpan { start: 19, end: 30 },
		];
		let members = [member(
			"candidate",
			&candidate,
			&spans,
			GridPosition { row: 0, col: 0 },
			Facing::Right,
			0,
		)];

		let error = validate_candidate_replay_plan_shape(CandidateReplayPlan { members: &members })
			.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::OverlappingActiveSpans {
				member_index: 0,
				previous_index: 0,
				previous_end: 20,
				index: 1,
				start: 19,
			}
		));
	}

	#[test]
	fn rejects_skill_start_before_its_deployment() {
		let candidate = build("char_candidate");
		let spans = [TickSpan { start: 9, end: 20 }];
		let members = [member(
			"candidate",
			&candidate,
			&spans,
			GridPosition { row: 0, col: 0 },
			Facing::Right,
			10,
		)];

		let error = validate_candidate_replay_plan_shape(CandidateReplayPlan { members: &members })
			.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::SkillBeforeDeployment {
				member_index: 0,
				skill_tick: 9,
				deploy_tick: 10,
			}
		));
	}

	#[test]
	fn manual_skill_start_need_not_wait_for_the_final_deployment() {
		let spans = [TickSpan { start: 19, end: 30 }];

		let manual_skill_schedule =
			validate_member_skill_schedule(0, &SkillActivationType::Manual, &spans).unwrap();

		assert!(manual_skill_schedule);
	}

	#[test]
	fn rejects_recurring_skill_before_the_mixed_stream_can_start_the_daemon() {
		let first = build("char_first");
		let second = build("char_second");
		let first_spans = [
			TickSpan { start: 20, end: 25 },
			TickSpan { start: 30, end: 35 },
		];
		let members = [
			member(
				"first",
				&first,
				&first_spans,
				GridPosition { row: 0, col: 0 },
				Facing::Right,
				10,
			),
			member(
				"second",
				&second,
				&[],
				GridPosition { row: 0, col: 1 },
				Facing::Right,
				40,
			),
		];

		let validated = [
			ValidatedCandidateReplayMember {
				plan: members[0],
				manual_skill_schedule: true,
			},
			ValidatedCandidateReplayMember {
				plan: members[1],
				manual_skill_schedule: false,
			},
		];
		let error = validate_manual_skill_daemon_timing(&validated).unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::RecurringSkillBeforeDaemon {
				member_index: 0,
				skill_tick: 30,
				daemon_tick: 40,
			}
		));
	}

	#[test]
	fn automatic_skill_spans_do_not_extend_or_recur_before_the_manual_daemon() {
		let manual = build("char_manual");
		let automatic = build("char_automatic");
		let manual_spans = [
			TickSpan { start: 20, end: 25 },
			TickSpan { start: 30, end: 35 },
		];
		let automatic_spans = [
			TickSpan { start: 40, end: 45 },
			TickSpan { start: 46, end: 50 },
		];
		let members = [
			member(
				"manual",
				&manual,
				&manual_spans,
				GridPosition { row: 0, col: 0 },
				Facing::Right,
				10,
			),
			member(
				"automatic",
				&automatic,
				&automatic_spans,
				GridPosition { row: 0, col: 1 },
				Facing::Right,
				25,
			),
		];
		let validated = [
			ValidatedCandidateReplayMember {
				plan: members[0],
				manual_skill_schedule: true,
			},
			ValidatedCandidateReplayMember {
				plan: members[1],
				manual_skill_schedule: false,
			},
		];

		validate_manual_skill_daemon_timing(&validated).unwrap();
	}

	#[test]
	fn rejects_commands_after_source_deploy_instead_of_reordering_them() {
		let seed = seed_with_trailing_deploy();

		let error = validate_source_action_order(&seed, "source").unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::CommandAfterSourceDeployment {
				ref source_name,
				action_index: 1,
				kind: "Deploy",
			} if source_name == "source"
		));
	}

	#[test]
	#[ignore = "requires configured gamedata snapshot"]
	fn rejects_duplicate_resolved_candidate_operators() {
		let snapshot_path = crate::test_support::configured_gamedata_dir();
		let snapshot = ArkDataSnapshot::load(&snapshot_path).unwrap();
		let candidate_a = build("char_103_angel");
		let candidate_b = build("char_103_angel");
		let members = [
			member(
				"a",
				&candidate_a,
				&[],
				GridPosition { row: 3, col: 2 },
				Facing::Right,
				0,
			),
			member(
				"b",
				&candidate_b,
				&[],
				GridPosition { row: 5, col: 3 },
				Facing::Right,
				0,
			),
		];

		let error =
			validate_candidate_replay_plan(&snapshot, CandidateReplayPlan { members: &members })
				.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::DuplicateCandidateOperator {
				ref operator_id
			} if operator_id == "char_103_angel"
		));
	}

	#[test]
	#[ignore = "requires local verified gamedata and battle-prefab snapshots"]
	fn materializes_the_selected_candidate_anchor_instead_of_the_source_anchor() {
		let (snapshot, prefabs) = local_battle_snapshots();
		let seed = bd7_seed();
		let bindings = vec![
			binding("黍", "char_2025_shu", 0),
			binding("圣聆初雪", "char_1046_sbell2", 0),
		];
		let placement = CandidateReplayPlacement {
			cell: GridPosition { row: 3, col: 2 },
			facing: Facing::Left,
			requested_at: SimTick::new(660),
		};
		let candidate = build("char_103_angel");
		let spans = [TickSpan {
			start: 960,
			end: 1_405,
		}];
		let members = [CandidateReplayMemberPlan {
			candidate_key: "candidate:angel",
			build: &candidate,
			skill_active_spans: &spans,
			placement,
		}];

		let input = materialize_candidate_expected_replay_input(
			&snapshot,
			&prefabs,
			&seed,
			&bindings,
			"圣聆初雪",
			CandidateReplayPlan { members: &members },
			TickRate::new(NonZeroU32::new(30).unwrap()),
		)
		.unwrap();

		assert!(input.guide.actions.iter().any(|action| {
			matches!(
				action,
				MaterializedGuideAction::Deploy {
					requested_at,
					operator,
					cell,
					facing,
				} if *requested_at == placement.requested_at
					&& *operator == OperatorInstanceId::new(1)
					&& *cell == placement.cell
					&& *facing == placement.facing
			)
		}));
		assert!(!input.guide.actions.iter().any(|action| {
			matches!(
				action,
				MaterializedGuideAction::Deploy {
					operator,
					cell: GridPosition { row: 5, col: 3 },
					..
				} if *operator == OperatorInstanceId::new(1)
			)
		}));
		let output = execute_expected_replay(&input, [0_u8; 32], "candidate-placement");
		let deploy = output
			.report
			.actions
			.iter()
			.find(|action| {
				action.kind == "deploy" && action.operator_id.as_deref() == Some("char_103_angel")
			})
			.expect("candidate deployment must be emitted in replay");
		assert_eq!(deploy.row, Some(3));
		assert_eq!(deploy.column, Some(2));
		assert_eq!(deploy.facing, ArtifactFacing::Left);
		assert_eq!(deploy.requested_tick, 660);
	}

	#[test]
	#[ignore = "requires local verified gamedata and battle-prefab snapshots"]
	fn rejects_a_candidate_anchor_on_an_incompatible_stage_cell() {
		let (snapshot, prefabs) = local_battle_snapshots();
		let candidate = build("char_103_angel");
		let spans = [TickSpan {
			start: 960,
			end: 1_405,
		}];
		let members = [member(
			"candidate:angel",
			&candidate,
			&spans,
			GridPosition { row: 4, col: 3 },
			Facing::Right,
			660,
		)];
		let error = materialize_candidate_expected_replay_input(
			&snapshot,
			&prefabs,
			&bd7_seed(),
			&[
				binding("黍", "char_2025_shu", 0),
				binding("圣聆初雪", "char_1046_sbell2", 0),
			],
			"圣聆初雪",
			CandidateReplayPlan { members: &members },
			TickRate::new(NonZeroU32::new(30).unwrap()),
		)
		.unwrap_err();

		assert!(matches!(
			error,
			CandidateExpectedReplayError::Adapter(
				ExpectedReplayAdapterError::InvalidDeploymentCell {
					ref operator,
					cell: GridPosition { row: 4, col: 3 },
				}
			) if operator == "char_103_angel"
		));
	}
}
