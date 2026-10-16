//! Deterministic MAA job file materialization for one concrete replacement plan.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::data::{GridPosition, MapDimensions};

use super::codec::validate_maa_job_executable;
use super::model::{
	MaaActionConditions, MaaJob, MaaJobAction, MaaJobDifficulty, MaaJobDifficultyMask,
	MaaJobDirection, MaaJobDocument, MaaJobOperator, MaaJobRequirements, MaaSkillUsage,
};
use super::seed::MaaJobSeed;

/// Source-guide metadata retained by an exported independent strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaaJobExportMetadata<'a> {
	pub minimum_required: &'a str,
	pub title: &'a str,
	pub details: Option<&'a str>,
}

/// One exact replacement build within the fields expressible by the MAA job file format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaaExactRequirements {
	pub elite: u8,
	pub level: u16,
	pub potential: u8,
	pub skill_level: u8,
	pub module: Option<i8>,
	pub module_level: u8,
}

/// One exact operator and selected skill replacing the source slot.
///
/// The compiler emits `skill_usage = 1`: the generated explicit `Skill` action pins the first
/// activation, then the final `SkillDaemon` activates the skill whenever it becomes ready again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaaReplacementOperator {
	pub name: String,
	pub skill: u8,
	pub requirements: MaaExactRequirements,
}

/// One globally ordered action performed by a replacement operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaaReplacementAction<'a> {
	pub operator_name: &'a str,
	pub kind: MaaReplacementActionKind,
	/// Tick selected for this action by the compiler caller.
	pub chosen_tick: u64,
	/// Earliest tick at which this action is executable after all preceding exported actions.
	pub earliest_legal_tick: u64,
}

/// Replacement action subset supported by the deterministic exporter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaaReplacementActionKind {
	Deploy {
		/// Bottom-origin stage cell, validated and converted to MAA coordinates during export.
		cell: GridPosition,
		/// Direction serialized directly in canonical MAA spelling.
		direction: MaaJobDirection,
	},
	Skill,
}

/// Complete input needed to compile one independent, deterministic MAA job file.
#[derive(Clone, Copy, Debug)]
pub struct MaaJobCompileRequest<'a> {
	pub seed: &'a MaaJobSeed,
	pub metadata: MaaJobExportMetadata<'a>,
	/// Name of the already-selected source operator in `seed`, not a pre-expansion group alias.
	pub source_slot_name: &'a str,
	/// Exact replacement builds, emitted at the source operator's position in this order.
	pub replacements: &'a [MaaReplacementOperator],
	/// Globally ordered replacement actions.
	///
	/// Actions must be chronological, and each member's deployment must precede its own skill.
	/// Deployments must follow `replacements` order; skills may precede later deployments of
	/// other members.
	pub replacement_actions: &'a [MaaReplacementAction<'a>],
	/// Exact MAA-expressible builds for every selected operator except the replaced source slot.
	pub retained_exact_requirements: &'a BTreeMap<String, MaaExactRequirements>,
	pub map_dimensions: MapDimensions,
}

/// Compiles one concrete selection into an independently executable MAA job file.
///
/// Group alternatives are already resolved in `request.seed` and are emitted as fixed operators.
/// The only supported timing representation is an ordered action sequence that MAA executes at
/// each action's earliest legal readiness. The compiler rejects inputs that would require a
/// stopwatch, an absolute time condition, or a later manually chosen action. Because MAA actions
/// do not encode operator death, a retained deployment remains occupied until an explicit
/// preceding `Retreat` names its operator or location.
pub fn compile_maa_job(request: MaaJobCompileRequest<'_>) -> Result<MaaJob, MaaJobCompileError> {
	validate_request(request)?;

	let source_deployment: SourceDeployment =
		locate_source_deployment(request.seed, request.source_slot_name)?;
	validate_replacement_prefix_occupancy(
		request.seed,
		source_deployment.action_index,
		request.replacement_actions,
		request.map_dimensions,
	)?;
	let skill_daemon_index: usize = locate_skill_daemon(request.seed)?;
	if skill_daemon_index <= source_deployment.action_index {
		return Err(MaaJobCompileError::SkillDaemonBeforeSourceDeployment {
			source_action_index: source_deployment.action_index,
			skill_daemon_index,
		});
	}
	if skill_daemon_index + 1 != request.seed.actions().len() {
		return Err(MaaJobCompileError::SkillDaemonNotLast { skill_daemon_index });
	}
	validate_source_action_tail(
		request.seed,
		request.source_slot_name,
		source_deployment.action_index,
		skill_daemon_index,
		source_deployment.original_location,
	)?;

	let opers: Vec<MaaJobOperator> = export_operators(
		request.seed,
		request.source_slot_name,
		request.replacements,
		request.retained_exact_requirements,
	)?;
	let mut actions: Vec<MaaJobAction> =
		Vec::with_capacity(request.seed.actions().len() - 1 + request.replacement_actions.len());
	for (action_index, action) in request.seed.actions().iter().enumerate() {
		if action_index == source_deployment.action_index {
			let source_conditions: &MaaActionConditions = action.conditions();
			let mut first_deployment: bool = true;
			for planned in request.replacement_actions {
				match planned.kind {
					MaaReplacementActionKind::Deploy { cell, direction } => {
						actions.push(MaaJobAction::Deploy {
							name: planned.operator_name.to_owned(),
							location: maa_location(cell, request.map_dimensions)?,
							direction,
							timeout: None,
							conditions: if first_deployment {
								first_deployment = false;
								export_conditions(source_conditions)
							} else {
								MaaActionConditions::default()
							},
						});
					}
					MaaReplacementActionKind::Skill => {
						actions.push(MaaJobAction::Skill {
							name: Some(planned.operator_name.to_owned()),
							location: None,
							timeout: None,
							conditions: MaaActionConditions::default(),
						});
					}
				}
			}
			continue;
		}
		actions.push(export_action(action, action_index)?);
	}

	let job = MaaJob {
		minimum_required: request.metadata.minimum_required.to_owned(),
		stage_name: request.seed.stage_name().to_owned(),
		actions,
		doc: MaaJobDocument {
			title: request.metadata.title.to_owned(),
			title_color: None,
			details: request.metadata.details.map(str::to_owned),
			details_color: None,
		},
		groups: Vec::new(),
		opers,
		difficulty: difficulty_mask(request.seed.difficulty()),
	};
	validate_maa_job_executable(&job).map_err(|error| MaaJobCompileError::GeneratedJobInvalid {
		message: error.to_string(),
	})?;
	Ok(job)
}

#[derive(Clone, Copy, Debug)]
struct SourceDeployment {
	action_index: usize,
	original_location: [i32; 2],
}

fn validate_replacement_prefix_occupancy(
	seed: &MaaJobSeed,
	source_deployment_index: usize,
	replacement_actions: &[MaaReplacementAction<'_>],
	map_dimensions: MapDimensions,
) -> Result<(), MaaJobCompileError> {
	// MAA actions do not encode operator death. Occupancy is therefore released only by an
	// explicit Retreat before the replaced deployment; unknown deaths remain occupied.
	let mut occupied: BTreeMap<[i32; 2], &str> = BTreeMap::new();
	for action in &seed.actions()[..source_deployment_index] {
		match action {
			MaaJobAction::Deploy { name, location, .. } => {
				occupied.entry(*location).or_insert(name);
			}
			MaaJobAction::Retreat {
				name: Some(name), ..
			} => occupied.retain(|_, occupant| *occupant != name),
			MaaJobAction::Retreat {
				name: None,
				location: Some(location),
				..
			} => {
				occupied.remove(location);
			}
			_ => {}
		}
	}

	for (action_index, action) in replacement_actions.iter().enumerate() {
		let MaaReplacementActionKind::Deploy { cell, .. } = action.kind else {
			continue;
		};
		let location: [i32; 2] = maa_location(cell, map_dimensions)?;
		if let Some(retained_name) = occupied.get(&location) {
			return Err(MaaJobCompileError::ReplacementCellOccupiedByRetained {
				action_index,
				replacement_name: action.operator_name.to_owned(),
				retained_name: (*retained_name).to_owned(),
				location,
			});
		}
	}
	Ok(())
}

fn validate_request(request: MaaJobCompileRequest<'_>) -> Result<(), MaaJobCompileError> {
	if request.metadata.minimum_required.trim().is_empty() {
		return Err(MaaJobCompileError::BlankMinimumRequired);
	}
	if request.metadata.title.trim().is_empty() {
		return Err(MaaJobCompileError::BlankTitle);
	}
	if request.source_slot_name.trim().is_empty() {
		return Err(MaaJobCompileError::BlankSourceSlotName);
	}
	if request.map_dimensions.width == 0 || request.map_dimensions.height == 0 {
		return Err(MaaJobCompileError::EmptyMap);
	}
	if request.replacements.is_empty() {
		return Err(MaaJobCompileError::EmptyReplacements);
	}
	let mut replacement_names: BTreeSet<&str> = BTreeSet::new();
	for replacement in request.replacements {
		if replacement.name.trim().is_empty() {
			return Err(MaaJobCompileError::BlankReplacementName);
		}
		if !replacement_names.insert(replacement.name.as_str()) {
			return Err(MaaJobCompileError::DuplicateReplacementName {
				name: replacement.name.clone(),
			});
		}
		if !(1..=3).contains(&replacement.skill) {
			return Err(MaaJobCompileError::InvalidSkillSlot {
				value: replacement.skill,
			});
		}
		validate_exact_requirements(&replacement.requirements)?;
	}
	let mut deployment_counts: BTreeMap<&str, usize> = request
		.replacements
		.iter()
		.map(|replacement| (replacement.name.as_str(), 0))
		.collect();
	let mut skill_counts: BTreeMap<&str, usize> = deployment_counts.clone();
	let mut occupied_cells: BTreeMap<(u32, u32), &str> = BTreeMap::new();
	let mut previous_tick: Option<(usize, u64)> = None;
	let mut deployment_index: usize = 0;
	for (action_index, action) in request.replacement_actions.iter().enumerate() {
		if !replacement_names.contains(action.operator_name) {
			return Err(MaaJobCompileError::UnknownReplacementActionOperator {
				action_index,
				name: action.operator_name.to_owned(),
			});
		}
		if action.chosen_tick != action.earliest_legal_tick {
			return Err(MaaJobCompileError::ReplacementActionIsNotEarliest {
				action_index,
				name: action.operator_name.to_owned(),
				action_type: replacement_action_type(action.kind),
				chosen_tick: action.chosen_tick,
				earliest_legal_tick: action.earliest_legal_tick,
			});
		}
		if let Some((previous_action_index, previous_action_tick)) =
			previous_tick.filter(|(_, tick)| *tick > action.chosen_tick)
		{
			return Err(MaaJobCompileError::ReplacementActionsOutOfOrder {
				previous_action_index,
				previous_action_tick,
				action_index,
				action_tick: action.chosen_tick,
			});
		}
		previous_tick = Some((action_index, action.chosen_tick));
		match action.kind {
			MaaReplacementActionKind::Deploy { cell, .. } => {
				if let Some(expected) = request.replacements.get(deployment_index) {
					if action.operator_name != expected.name {
						return Err(MaaJobCompileError::ReplacementDeploymentOrderMismatch {
							action_index,
							expected_name: expected.name.clone(),
							actual_name: action.operator_name.to_owned(),
						});
					}
				}
				deployment_index += 1;
				maa_location(cell, request.map_dimensions)?;
				if let Some(first_name) =
					occupied_cells.insert((cell.row, cell.col), action.operator_name)
				{
					return Err(MaaJobCompileError::DuplicateReplacementCell {
						row: cell.row,
						col: cell.col,
						first_name: first_name.to_owned(),
						second_name: action.operator_name.to_owned(),
					});
				}
				*deployment_counts
					.get_mut(action.operator_name)
					.expect("replacement action operator was validated") += 1;
			}
			MaaReplacementActionKind::Skill => {
				if deployment_counts[action.operator_name] == 0 {
					return Err(MaaJobCompileError::ReplacementSkillBeforeDeployment {
						action_index,
						name: action.operator_name.to_owned(),
					});
				}
				*skill_counts
					.get_mut(action.operator_name)
					.expect("replacement action operator was validated") += 1;
			}
		}
	}
	for replacement in request.replacements {
		match deployment_counts[replacement.name.as_str()] {
			0 => {
				return Err(MaaJobCompileError::MissingReplacementDeployment {
					name: replacement.name.clone(),
				});
			}
			1 => {}
			count => {
				return Err(MaaJobCompileError::DuplicateReplacementDeployment {
					name: replacement.name.clone(),
					count,
				});
			}
		}
		match skill_counts[replacement.name.as_str()] {
			0 => {
				return Err(MaaJobCompileError::MissingReplacementSkill {
					name: replacement.name.clone(),
				});
			}
			1 => {}
			count => {
				return Err(MaaJobCompileError::DuplicateReplacementSkill {
					name: replacement.name.clone(),
					count,
				});
			}
		}
	}
	for requirements in request.retained_exact_requirements.values() {
		validate_exact_requirements(requirements)?;
	}
	Ok(())
}

fn validate_exact_requirements(
	requirements: &MaaExactRequirements,
) -> Result<(), MaaJobCompileError> {
	if requirements.elite > 2 {
		return Err(MaaJobCompileError::InvalidElite {
			value: requirements.elite,
		});
	}
	if !(1..=90).contains(&requirements.level) {
		return Err(MaaJobCompileError::InvalidLevel {
			value: requirements.level,
		});
	}
	if !(1..=6).contains(&requirements.potential) {
		return Err(MaaJobCompileError::InvalidPotential {
			value: requirements.potential,
		});
	}
	if !(1..=10).contains(&requirements.skill_level) {
		return Err(MaaJobCompileError::InvalidSkillLevel {
			value: requirements.skill_level,
		});
	}
	match requirements.module {
		Some(value) if !(1..=3).contains(&value) => {
			Err(MaaJobCompileError::InvalidModule { value })
		}
		Some(_) if !(1..=3).contains(&requirements.module_level) => {
			Err(MaaJobCompileError::InvalidModuleLevel {
				value: requirements.module_level,
				has_module: true,
			})
		}
		None if requirements.module_level != 0 => Err(MaaJobCompileError::InvalidModuleLevel {
			value: requirements.module_level,
			has_module: false,
		}),
		Some(_) | None => Ok(()),
	}
}

fn locate_source_deployment(
	seed: &MaaJobSeed,
	source_slot_name: &str,
) -> Result<SourceDeployment, MaaJobCompileError> {
	let deployments: Vec<(usize, [i32; 2])> = seed
		.actions()
		.iter()
		.enumerate()
		.filter_map(|(index, action)| match action {
			MaaJobAction::Deploy { name, location, .. } if name == source_slot_name => {
				Some((index, *location))
			}
			_ => None,
		})
		.collect();
	let [(action_index, original_location)] = deployments.as_slice() else {
		return if deployments.is_empty() {
			Err(MaaJobCompileError::MissingSourceDeployment {
				name: source_slot_name.to_owned(),
			})
		} else {
			Err(MaaJobCompileError::DuplicateSourceDeployment {
				name: source_slot_name.to_owned(),
			})
		};
	};
	Ok(SourceDeployment {
		action_index: *action_index,
		original_location: *original_location,
	})
}

fn locate_skill_daemon(seed: &MaaJobSeed) -> Result<usize, MaaJobCompileError> {
	let indices: Vec<usize> = seed
		.actions()
		.iter()
		.enumerate()
		.filter_map(|(index, action)| {
			matches!(action, MaaJobAction::SkillDaemon { .. }).then_some(index)
		})
		.collect();
	let [index] = indices.as_slice() else {
		return if indices.is_empty() {
			Err(MaaJobCompileError::MissingSkillDaemon)
		} else {
			Err(MaaJobCompileError::DuplicateSkillDaemon)
		};
	};
	Ok(*index)
}

fn validate_source_action_tail(
	seed: &MaaJobSeed,
	source_slot_name: &str,
	source_deployment_index: usize,
	skill_daemon_index: usize,
	source_location: [i32; 2],
) -> Result<(), MaaJobCompileError> {
	for (action_index, action) in seed.actions().iter().enumerate() {
		if matches!(action, MaaJobAction::ResetStopwatch { .. }) {
			return Err(MaaJobCompileError::ResetStopwatchUnsupported { action_index });
		}
		validate_action_conditions(action, action_index, source_deployment_index)?;
		if action_index > source_deployment_index
			&& action_index < skill_daemon_index
			&& !matches!(action, MaaJobAction::SpeedUp { .. })
		{
			return Err(MaaJobCompileError::BlockingActionBeforeSkillDaemon {
				action_index,
				action_type: action_type(action),
			});
		}
		if action_index != source_deployment_index
			&& action_targets_source(action, source_slot_name, source_location)
		{
			return Err(MaaJobCompileError::AdditionalSourceAction {
				action_index,
				action_type: action_type(action),
			});
		}
	}
	Ok(())
}

fn validate_action_conditions(
	action: &MaaJobAction,
	action_index: usize,
	source_deployment_index: usize,
) -> Result<(), MaaJobCompileError> {
	let conditions: &MaaActionConditions = action.conditions();
	if conditions.time_elapsed().is_some() {
		return Err(MaaJobCompileError::TimeElapsedUnsupported { action_index });
	}
	if action
		.timeout()
		.is_some_and(|timeout| timeout.is_branching())
	{
		return Err(MaaJobCompileError::TimeoutUnsupported { action_index });
	}

	match action {
		MaaJobAction::Deploy { .. } => {
			if action_index == source_deployment_index {
				if let Some(field) = first_timing_or_guard_condition(conditions) {
					return Err(MaaJobCompileError::SourceDeploymentConditionUnsupported {
						action_index,
						field,
					});
				}
			} else {
				validate_retained_deployment_conditions(conditions, action_index)?;
			}
		}
		_ => {
			if let Some(field) = first_timing_or_guard_condition(conditions) {
				return Err(MaaJobCompileError::NonDeploymentConditionUnsupported {
					action_index,
					action_type: action_type(action),
					field,
				});
			}
		}
	}
	Ok(())
}

fn validate_retained_deployment_conditions(
	conditions: &MaaActionConditions,
	action_index: usize,
) -> Result<(), MaaJobCompileError> {
	for (field, value) in [
		("cost_changes", conditions.cost_changes()),
		("kills", conditions.kills()),
		("cooling", conditions.cooling()),
		("pre_delay", conditions.pre_delay()),
	] {
		if value.is_some() {
			return Err(MaaJobCompileError::RetainedDeploymentConditionUnsupported {
				action_index,
				field,
			});
		}
	}

	for (field, value) in [
		("costs", conditions.costs()),
		("post_delay", conditions.post_delay()),
	] {
		if let Some(value) = value.filter(|value| *value < 0) {
			return Err(
				MaaJobCompileError::RetainedDeploymentConditionValueUnsupported {
					action_index,
					field,
					value,
				},
			);
		}
	}
	Ok(())
}

fn first_timing_or_guard_condition(conditions: &MaaActionConditions) -> Option<&'static str> {
	[
		("costs", conditions.costs().is_some()),
		("cost_changes", conditions.cost_changes().is_some()),
		("kills", conditions.kills().is_some()),
		("cooling", conditions.cooling().is_some()),
		("pre_delay", conditions.pre_delay().is_some()),
		("post_delay", conditions.post_delay().is_some()),
	]
	.into_iter()
	.find_map(|(field, present)| present.then_some(field))
}

fn action_targets_source(
	action: &MaaJobAction,
	source_slot_name: &str,
	source_location: [i32; 2],
) -> bool {
	match action {
		MaaJobAction::Deploy { name, .. } | MaaJobAction::SkillUsage { name, .. } => {
			name == source_slot_name
		}
		MaaJobAction::Skill { name, location, .. }
		| MaaJobAction::Retreat { name, location, .. }
		| MaaJobAction::BulletTime { name, location, .. } => {
			name.as_deref() == Some(source_slot_name) || *location == Some(source_location)
		}
		MaaJobAction::SpeedUp { .. }
		| MaaJobAction::Output { .. }
		| MaaJobAction::SkillDaemon { .. }
		| MaaJobAction::MoveCamera { .. }
		| MaaJobAction::ResetStopwatch { .. } => false,
	}
}

fn export_operators(
	seed: &MaaJobSeed,
	source_slot_name: &str,
	replacements: &[MaaReplacementOperator],
	retained_exact_requirements: &BTreeMap<String, MaaExactRequirements>,
) -> Result<Vec<MaaJobOperator>, MaaJobCompileError> {
	let source_count: usize = seed
		.operators()
		.iter()
		.filter(|operator| operator.name() == source_slot_name)
		.count();
	if source_count == 0 {
		return Err(MaaJobCompileError::MissingSourceOperator {
			name: source_slot_name.to_owned(),
		});
	}
	if source_count > 1 {
		return Err(MaaJobCompileError::DuplicateSourceOperator {
			name: source_slot_name.to_owned(),
		});
	}
	for replacement in replacements {
		if replacement.name != source_slot_name
			&& seed
				.operators()
				.iter()
				.any(|operator| operator.name() == replacement.name)
		{
			return Err(MaaJobCompileError::ReplacementAlreadySelected {
				name: replacement.name.clone(),
			});
		}
	}

	let retained_names: BTreeSet<&str> = seed
		.operators()
		.iter()
		.filter_map(|operator| (operator.name() != source_slot_name).then_some(operator.name()))
		.collect();
	for retained_name in &retained_names {
		if !retained_exact_requirements.contains_key(*retained_name) {
			return Err(MaaJobCompileError::MissingRetainedExactRequirements {
				name: (*retained_name).to_owned(),
			});
		}
	}
	for supplied_name in retained_exact_requirements.keys() {
		if !retained_names.contains(supplied_name.as_str()) {
			return Err(MaaJobCompileError::UnexpectedRetainedExactRequirements {
				name: supplied_name.clone(),
			});
		}
	}

	let mut exported: Vec<MaaJobOperator> =
		Vec::with_capacity(seed.operators().len() - 1 + replacements.len());
	for operator in seed.operators() {
		if operator.name() == source_slot_name {
			exported.extend(replacements.iter().map(export_replacement_operator));
		} else {
			let requirements: &MaaExactRequirements = retained_exact_requirements
				.get(operator.name())
				.expect("retained name set was validated");
			exported.push(export_retained_operator(operator, requirements)?);
		}
	}
	Ok(exported)
}

fn export_replacement_operator(replacement: &MaaReplacementOperator) -> MaaJobOperator {
	let requirements: &MaaExactRequirements = &replacement.requirements;
	MaaJobOperator {
		name: replacement.name.clone(),
		skill: Some(replacement.skill),
		skill_usage: Some(MaaSkillUsage::WhenReady),
		skill_times: None,
		requirements: Some(export_exact_requirements(requirements)),
	}
}

fn export_retained_operator(
	operator: &MaaJobOperator,
	requirements: &MaaExactRequirements,
) -> Result<MaaJobOperator, MaaJobCompileError> {
	if let Some(skill_usage) = operator
		.skill_usage()
		.filter(|skill_usage| *skill_usage != 0)
	{
		return Err(MaaJobCompileError::RetainedDaemonSkillUsageUnsupported {
			name: operator.name().to_owned(),
			skill_usage,
		});
	}
	let skill: u8 = operator
		.skill()
		.ok_or_else(|| MaaJobCompileError::MissingRetainedSkill {
			name: operator.name().to_owned(),
		})?;
	Ok(MaaJobOperator {
		name: operator.name().to_owned(),
		skill: Some(skill),
		skill_usage: operator.skill_usage,
		skill_times: operator.skill_times(),
		requirements: Some(export_exact_requirements(requirements)),
	})
}

fn export_exact_requirements(requirements: &MaaExactRequirements) -> MaaJobRequirements {
	MaaJobRequirements {
		elite: Some(requirements.elite),
		level: Some(requirements.level),
		module: requirements.module,
		module_level: requirements.module.map(|_| requirements.module_level),
		potential: Some(requirements.potential),
		skill_level: Some(requirements.skill_level),
	}
}

fn export_action(
	action: &MaaJobAction,
	action_index: usize,
) -> Result<MaaJobAction, MaaJobCompileError> {
	let output: MaaJobAction = match action {
		MaaJobAction::Deploy {
			name,
			location,
			direction,
			conditions,
			..
		} => MaaJobAction::Deploy {
			name: name.clone(),
			location: *location,
			direction: *direction,
			timeout: None,
			conditions: export_conditions(conditions),
		},
		MaaJobAction::Skill {
			name,
			location,
			conditions,
			..
		} => MaaJobAction::Skill {
			name: name.clone(),
			location: *location,
			timeout: None,
			conditions: export_conditions(conditions),
		},
		MaaJobAction::Retreat {
			name,
			location,
			conditions,
		} => MaaJobAction::Retreat {
			name: name.clone(),
			location: *location,
			conditions: export_conditions(conditions),
		},
		MaaJobAction::BulletTime {
			name,
			location,
			conditions,
		} => MaaJobAction::BulletTime {
			name: name.clone(),
			location: *location,
			conditions: export_conditions(conditions),
		},
		MaaJobAction::SkillUsage {
			name,
			skill_usage,
			skill_times,
			conditions,
		} => MaaJobAction::SkillUsage {
			name: name.clone(),
			skill_usage: *skill_usage,
			skill_times: *skill_times,
			conditions: export_conditions(conditions),
		},
		MaaJobAction::SpeedUp { conditions } => MaaJobAction::SpeedUp {
			conditions: export_conditions(conditions),
		},
		MaaJobAction::Output { conditions } => MaaJobAction::Output {
			conditions: export_conditions(conditions),
		},
		MaaJobAction::SkillDaemon { conditions } => MaaJobAction::SkillDaemon {
			conditions: export_conditions(conditions),
		},
		MaaJobAction::MoveCamera {
			distance,
			conditions,
		} => MaaJobAction::MoveCamera {
			distance: *distance,
			conditions: export_conditions(conditions),
		},
		MaaJobAction::ResetStopwatch { .. } => {
			return Err(MaaJobCompileError::ResetStopwatchUnsupported { action_index });
		}
	};
	Ok(output)
}

fn export_conditions(conditions: &MaaActionConditions) -> MaaActionConditions {
	MaaActionConditions {
		doc: conditions.doc().map(str::to_owned),
		doc_color: conditions.doc_color().map(str::to_owned),
		costs: conditions.costs(),
		cost_changes: conditions.cost_changes(),
		kills: conditions.kills(),
		cooling: conditions.cooling(),
		pre_delay: conditions.pre_delay(),
		post_delay: conditions.post_delay(),
		time_elapsed: conditions.time_elapsed(),
	}
}

fn maa_location(
	cell: GridPosition,
	dimensions: MapDimensions,
) -> Result<[i32; 2], MaaJobCompileError> {
	if cell.col >= dimensions.width || cell.row >= dimensions.height {
		return Err(MaaJobCompileError::ReplacementCellOutOfBounds {
			row: cell.row,
			col: cell.col,
			map_width: dimensions.width,
			map_height: dimensions.height,
		});
	}
	let y_from_top: u32 = dimensions.height - 1 - cell.row;
	let x: i32 = i32::try_from(cell.col).map_err(|_| MaaJobCompileError::CoordinateOverflow)?;
	let y: i32 = i32::try_from(y_from_top).map_err(|_| MaaJobCompileError::CoordinateOverflow)?;
	Ok([x, y])
}

fn difficulty_mask(difficulty: MaaJobDifficulty) -> MaaJobDifficultyMask {
	match difficulty {
		MaaJobDifficulty::Unspecified => MaaJobDifficultyMask::UNSPECIFIED,
		MaaJobDifficulty::Normal => MaaJobDifficultyMask::NORMAL,
		MaaJobDifficulty::Challenge => MaaJobDifficultyMask::CHALLENGE,
	}
}

fn action_type(action: &MaaJobAction) -> &'static str {
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

const fn replacement_action_type(kind: MaaReplacementActionKind) -> &'static str {
	match kind {
		MaaReplacementActionKind::Deploy { .. } => "Deploy",
		MaaReplacementActionKind::Skill => "Skill",
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaaJobCompileError {
	BlankMinimumRequired,
	BlankTitle,
	BlankSourceSlotName,
	EmptyReplacements,
	BlankReplacementName,
	DuplicateReplacementName {
		name: String,
	},
	InvalidSkillSlot {
		value: u8,
	},
	InvalidElite {
		value: u8,
	},
	InvalidLevel {
		value: u16,
	},
	InvalidPotential {
		value: u8,
	},
	InvalidSkillLevel {
		value: u8,
	},
	InvalidModule {
		value: i8,
	},
	InvalidModuleLevel {
		value: u8,
		has_module: bool,
	},
	UnknownReplacementActionOperator {
		action_index: usize,
		name: String,
	},
	ReplacementActionIsNotEarliest {
		action_index: usize,
		name: String,
		action_type: &'static str,
		chosen_tick: u64,
		earliest_legal_tick: u64,
	},
	ReplacementActionsOutOfOrder {
		previous_action_index: usize,
		previous_action_tick: u64,
		action_index: usize,
		action_tick: u64,
	},
	ReplacementSkillBeforeDeployment {
		action_index: usize,
		name: String,
	},
	ReplacementDeploymentOrderMismatch {
		action_index: usize,
		expected_name: String,
		actual_name: String,
	},
	MissingReplacementDeployment {
		name: String,
	},
	DuplicateReplacementDeployment {
		name: String,
		count: usize,
	},
	MissingReplacementSkill {
		name: String,
	},
	DuplicateReplacementSkill {
		name: String,
		count: usize,
	},
	DuplicateReplacementCell {
		row: u32,
		col: u32,
		first_name: String,
		second_name: String,
	},
	ReplacementCellOccupiedByRetained {
		action_index: usize,
		replacement_name: String,
		retained_name: String,
		location: [i32; 2],
	},
	EmptyMap,
	ReplacementCellOutOfBounds {
		row: u32,
		col: u32,
		map_width: u32,
		map_height: u32,
	},
	CoordinateOverflow,
	MissingSourceOperator {
		name: String,
	},
	DuplicateSourceOperator {
		name: String,
	},
	ReplacementAlreadySelected {
		name: String,
	},
	MissingRetainedExactRequirements {
		name: String,
	},
	UnexpectedRetainedExactRequirements {
		name: String,
	},
	MissingRetainedSkill {
		name: String,
	},
	RetainedDaemonSkillUsageUnsupported {
		name: String,
		skill_usage: u8,
	},
	MissingSourceDeployment {
		name: String,
	},
	DuplicateSourceDeployment {
		name: String,
	},
	MissingSkillDaemon,
	DuplicateSkillDaemon,
	SkillDaemonBeforeSourceDeployment {
		source_action_index: usize,
		skill_daemon_index: usize,
	},
	SkillDaemonNotLast {
		skill_daemon_index: usize,
	},
	ResetStopwatchUnsupported {
		action_index: usize,
	},
	TimeElapsedUnsupported {
		action_index: usize,
	},
	TimeoutUnsupported {
		action_index: usize,
	},
	SourceDeploymentConditionUnsupported {
		action_index: usize,
		field: &'static str,
	},
	RetainedDeploymentConditionUnsupported {
		action_index: usize,
		field: &'static str,
	},
	RetainedDeploymentConditionValueUnsupported {
		action_index: usize,
		field: &'static str,
		value: i32,
	},
	NonDeploymentConditionUnsupported {
		action_index: usize,
		action_type: &'static str,
		field: &'static str,
	},
	BlockingActionBeforeSkillDaemon {
		action_index: usize,
		action_type: &'static str,
	},
	AdditionalSourceAction {
		action_index: usize,
		action_type: &'static str,
	},
	GeneratedJobInvalid {
		message: String,
	},
}

impl Display for MaaJobCompileError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::BlankMinimumRequired => formatter.write_str("minimum_required must be non-blank"),
			Self::BlankTitle => formatter.write_str("document title must be non-blank"),
			Self::BlankSourceSlotName => formatter.write_str("source slot name must be non-blank"),
			Self::EmptyReplacements => {
				formatter.write_str("at least one replacement operator is required")
			}
			Self::BlankReplacementName => {
				formatter.write_str("replacement operator name must be non-blank")
			}
			Self::DuplicateReplacementName { name } => {
				write!(
					formatter,
					"replacement operator {name:?} occurs more than once"
				)
			}
			Self::InvalidSkillSlot { value } => {
				write!(formatter, "replacement skill slot {value} is outside 1..=3")
			}
			Self::InvalidElite { value } => {
				write!(
					formatter,
					"replacement elite phase {value} is outside 0..=2"
				)
			}
			Self::InvalidLevel { value } => {
				write!(formatter, "replacement level {value} is outside 1..=90")
			}
			Self::InvalidPotential { value } => {
				write!(formatter, "replacement potential {value} is outside 1..=6")
			}
			Self::InvalidSkillLevel { value } => {
				write!(
					formatter,
					"replacement skill level {value} is outside 1..=10"
				)
			}
			Self::InvalidModule { value } => {
				write!(formatter, "replacement module {value} is outside 1..=3")
			}
			Self::InvalidModuleLevel { value, has_module } => write!(
				formatter,
				"replacement module level {value} is invalid when has_module={has_module}"
			),
			Self::UnknownReplacementActionOperator { action_index, name } => write!(
				formatter,
				"replacement action {action_index} targets unknown operator {name:?}"
			),
			Self::ReplacementActionIsNotEarliest {
				action_index,
				name,
				action_type,
				chosen_tick,
				earliest_legal_tick,
			} => write!(
				formatter,
				"replacement {action_type} action {action_index} for {name:?} chooses tick \
				 {chosen_tick}, not earliest legal tick {earliest_legal_tick}"
			),
			Self::ReplacementActionsOutOfOrder {
				previous_action_index,
				previous_action_tick,
				action_index,
				action_tick,
			} => write!(
				formatter,
				"replacement action {action_index} at tick {action_tick} precedes action \
				 {previous_action_index} at tick {previous_action_tick}"
			),
			Self::ReplacementSkillBeforeDeployment { action_index, name } => write!(
				formatter,
				"replacement Skill action {action_index} for {name:?} precedes its Deploy action"
			),
			Self::ReplacementDeploymentOrderMismatch {
				action_index,
				expected_name,
				actual_name,
			} => write!(
				formatter,
				"replacement Deploy action {action_index} targets {actual_name:?}; expected \
				 {expected_name:?} from replacement roster order"
			),
			Self::MissingReplacementDeployment { name } => {
				write!(
					formatter,
					"replacement operator {name:?} has no Deploy action"
				)
			}
			Self::DuplicateReplacementDeployment { name, count } => write!(
				formatter,
				"replacement operator {name:?} has {count} Deploy actions; exactly one is required"
			),
			Self::MissingReplacementSkill { name } => {
				write!(
					formatter,
					"replacement operator {name:?} has no Skill action"
				)
			}
			Self::DuplicateReplacementSkill { name, count } => write!(
				formatter,
				"replacement operator {name:?} has {count} Skill actions; exactly one is required"
			),
			Self::DuplicateReplacementCell {
				row,
				col,
				first_name,
				second_name,
			} => write!(
				formatter,
				"replacement operators {first_name:?} and {second_name:?} both deploy at \
				 ({row}, {col})"
			),
			Self::ReplacementCellOccupiedByRetained {
				action_index,
				replacement_name,
				retained_name,
				location: [x, y],
			} => write!(
				formatter,
				"replacement Deploy action {action_index} for {replacement_name:?} targets [{x}, \
				 {y}], which remains occupied by retained operator {retained_name:?}; only an \
				 explicit preceding Retreat releases inferred occupancy"
			),
			Self::EmptyMap => formatter.write_str("map dimensions must be positive"),
			Self::ReplacementCellOutOfBounds {
				row,
				col,
				map_width,
				map_height,
			} => write!(
				formatter,
				"replacement cell ({row}, {col}) is outside {map_width}x{map_height} map"
			),
			Self::CoordinateOverflow => {
				formatter.write_str("replacement cell does not fit MAA coordinates")
			}
			Self::MissingSourceOperator { name } => {
				write!(
					formatter,
					"source operator {name:?} is absent from selected seed"
				)
			}
			Self::DuplicateSourceOperator { name } => write!(
				formatter,
				"source operator {name:?} occurs more than once in selected seed"
			),
			Self::ReplacementAlreadySelected { name } => write!(
				formatter,
				"replacement operator {name:?} is already selected in another slot"
			),
			Self::MissingRetainedExactRequirements { name } => write!(
				formatter,
				"retained operator {name:?} has no exact MAA requirements"
			),
			Self::UnexpectedRetainedExactRequirements { name } => write!(
				formatter,
				"exact requirements were supplied for unselected retained operator {name:?}"
			),
			Self::MissingRetainedSkill { name } => write!(
				formatter,
				"retained operator {name:?} has no selected skill"
			),
			Self::RetainedDaemonSkillUsageUnsupported { name, skill_usage } => write!(
				formatter,
				"retained operator {name:?} has unsupported daemon skill_usage {skill_usage}"
			),
			Self::MissingSourceDeployment { name } => {
				write!(
					formatter,
					"source operator {name:?} has no deployment action"
				)
			}
			Self::DuplicateSourceDeployment { name } => write!(
				formatter,
				"source operator {name:?} has more than one deployment action"
			),
			Self::MissingSkillDaemon => formatter.write_str("guide has no SkillDaemon action"),
			Self::DuplicateSkillDaemon => {
				formatter.write_str("guide has more than one SkillDaemon action")
			}
			Self::SkillDaemonBeforeSourceDeployment {
				source_action_index,
				skill_daemon_index,
			} => write!(
				formatter,
				"SkillDaemon action {skill_daemon_index} does not follow source deployment \
				 {source_action_index}"
			),
			Self::SkillDaemonNotLast { skill_daemon_index } => write!(
				formatter,
				"SkillDaemon action {skill_daemon_index} must be the final source action"
			),
			Self::ResetStopwatchUnsupported { action_index } => write!(
				formatter,
				"ResetStopwatch action {action_index} is unsupported by earliest-ready export"
			),
			Self::TimeElapsedUnsupported { action_index } => write!(
				formatter,
				"time_elapsed on action {action_index} is unsupported by earliest-ready export"
			),
			Self::TimeoutUnsupported { action_index } => write!(
				formatter,
				"branching timeout on action {action_index} is unsupported by earliest-ready export"
			),
			Self::SourceDeploymentConditionUnsupported {
				action_index,
				field,
			} => write!(
				formatter,
				"{field} on source Deploy action {action_index} is unsupported because the \
				 replacement timing claim does not model it"
			),
			Self::RetainedDeploymentConditionUnsupported {
				action_index,
				field,
			} => write!(
				formatter,
				"{field} on retained Deploy action {action_index} is unsupported by the guide \
				 scheduler"
			),
			Self::RetainedDeploymentConditionValueUnsupported {
				action_index,
				field,
				value,
			} => write!(
				formatter,
				"{field}={value} on retained Deploy action {action_index} is unsupported by the \
				 guide scheduler"
			),
			Self::NonDeploymentConditionUnsupported {
				action_index,
				action_type,
				field,
			} => write!(
				formatter,
				"{field} on {action_type} action {action_index} is unsupported by the export \
				 scheduler"
			),
			Self::BlockingActionBeforeSkillDaemon {
				action_index,
				action_type,
			} => write!(
				formatter,
				"{action_type} action {action_index} may delay the explicit skill activation"
			),
			Self::AdditionalSourceAction {
				action_index,
				action_type,
			} => write!(
				formatter,
				"{action_type} action {action_index} targets the replaced source slot"
			),
			Self::GeneratedJobInvalid { message } => {
				write!(formatter, "generated MAA job file is invalid: {message}")
			}
		}
	}
}

impl Error for MaaJobCompileError {}
