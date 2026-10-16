use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

use super::model::{
	MaaJob, MaaJobAction, MaaJobDifficulty, MaaJobGroup, MaaJobOperator, MaaJobRequirements,
};

/// Failure to decode, validate, or expand a supported MAA job file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaaJobCodecError {
	MalformedJson { message: String },
	SerializationFailed { message: String },
	InvalidJob { message: String },
	DifficultyNotSelected { requested: MaaJobDifficulty },
	BlankSourceLineage,
	InvalidSourceDigest,
	ExpansionOverflow,
}

impl Display for MaaJobCodecError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::MalformedJson { message } => {
				write!(formatter, "malformed MAA job file: {message}")
			}
			Self::SerializationFailed { message } => {
				write!(formatter, "failed to serialize MAA job file: {message}")
			}
			Self::InvalidJob { message } => write!(formatter, "invalid MAA job file: {message}"),
			Self::DifficultyNotSelected { requested } => {
				write!(formatter, "job does not select {requested} difficulty")
			}
			Self::BlankSourceLineage => {
				formatter.write_str("MAA job file source lineage must not be blank")
			}
			Self::InvalidSourceDigest => {
				formatter.write_str("MAA job file source digest must be 64 hexadecimal characters")
			}
			Self::ExpansionOverflow => formatter.write_str("job seed count overflowed usize"),
		}
	}
}

impl Error for MaaJobCodecError {}

impl MaaJob {
	/// Validates and serializes canonical pretty, newline-terminated JSON for durable outputs.
	pub fn to_deterministic_json(&self) -> Result<String, MaaJobCodecError> {
		validate_maa_job(self)?;
		let mut json: String = serde_json::to_string_pretty(self).map_err(|error| {
			MaaJobCodecError::SerializationFailed {
				message: error.to_string(),
			}
		})?;
		json.push('\n');
		Ok(json)
	}

	/// SHA-256 of [`Self::to_deterministic_json`], encoded as lowercase hexadecimal.
	pub fn deterministic_sha256_hex(&self) -> Result<String, MaaJobCodecError> {
		let json: String = self.to_deterministic_json()?;
		Ok(format!("{:x}", Sha256::digest(json.as_bytes())))
	}
}

/// Parses the official MAA job file wire shape and validates ark-solver's selected profile.
///
/// Deprecated `skip_if_not_ready` is accepted by the action deserializer only for `Skill` and is
/// immediately canonicalized to `timeout`. It is never retained or emitted.
pub fn parse_maa_job(json: &str) -> Result<MaaJob, MaaJobCodecError> {
	let job: MaaJob =
		serde_json::from_str(json).map_err(|error| MaaJobCodecError::MalformedJson {
			message: error.to_string(),
		})?;
	validate_maa_job(&job)?;
	Ok(job)
}

/// Validates protocol invariants that span multiple strongly typed fields.
pub fn validate_maa_job(job: &MaaJob) -> Result<(), MaaJobCodecError> {
	if job.stage_name.trim().is_empty() {
		return invalid("stage_name must not be blank");
	}
	if job.minimum_required.trim().is_empty() {
		return invalid("minimum_required must not be blank");
	}

	validate_slot_names(&job.groups, &job.opers)?;
	for operator in job
		.opers
		.iter()
		.chain(job.groups.iter().flat_map(|group| group.opers.iter()))
	{
		validate_operator(operator)?;
	}
	validate_action_references(job)?;
	validate_stopwatch_order(job)?;
	validate_condition_ranges(job)?;
	Ok(())
}

/// Validates the subset for which ark-solver can currently prove execution semantics.
///
/// Stopwatch epochs and branching timeouts remain parseable and round-trippable, but are rejected
/// here until replay can prove their alternate branches. Unlimited timeout is equivalent to the
/// default wait-until-ready behavior and is executable.
pub fn validate_maa_job_executable(job: &MaaJob) -> Result<(), MaaJobCodecError> {
	validate_maa_job(job)?;
	validate_executable_slot_overlaps(&job.groups, &job.opers)?;
	for (action_index, action) in job.actions.iter().enumerate() {
		if matches!(action, MaaJobAction::ResetStopwatch { .. }) {
			return invalid(format!(
				"ResetStopwatch action {action_index} is not supported by executable replay"
			));
		}
		if action.conditions().time_elapsed.is_some() {
			return invalid(format!(
				"time_elapsed on action {action_index} is not supported by executable replay"
			));
		}
		if action
			.timeout()
			.is_some_and(|timeout| timeout.is_branching())
		{
			return invalid(format!(
				"branching timeout on action {action_index} is not proved by executable replay"
			));
		}
	}
	Ok(())
}

fn validate_slot_names(
	groups: &[MaaJobGroup],
	fixed_operators: &[MaaJobOperator],
) -> Result<(), MaaJobCodecError> {
	let mut slot_names: BTreeSet<&str> = BTreeSet::new();
	let mut group_names: BTreeSet<&str> = BTreeSet::new();
	for group in groups {
		if group.name.trim().is_empty() {
			return invalid("operator group names must not be blank");
		}
		if group.opers.is_empty() {
			return invalid(format!("operator group {:?} is empty", group.name));
		}
		if !slot_names.insert(group.name.as_str()) {
			return invalid(format!(
				"operator or group name {:?} is duplicated",
				group.name
			));
		}
		group_names.insert(group.name.as_str());
	}
	for operator in fixed_operators {
		if !slot_names.insert(operator.name()) {
			return invalid(format!(
				"operator or group name {:?} is duplicated",
				operator.name()
			));
		}
	}

	for group in groups {
		let mut group_operator_names: BTreeSet<&str> = BTreeSet::new();
		for operator in &group.opers {
			if group_names.contains(operator.name()) {
				return invalid(format!(
					"group alternative {:?} conflicts with a group slot name",
					operator.name()
				));
			}
			if !group_operator_names.insert(operator.name()) {
				return invalid(format!(
					"operator {:?} is duplicated within group {:?}",
					operator.name(),
					group.name
				));
			}
		}
	}
	Ok(())
}

fn validate_executable_slot_overlaps(
	groups: &[MaaJobGroup],
	fixed_operators: &[MaaJobOperator],
) -> Result<(), MaaJobCodecError> {
	let mut operator_names: BTreeSet<&str> =
		fixed_operators.iter().map(MaaJobOperator::name).collect();
	for operator in groups.iter().flat_map(|group| group.opers.iter()) {
		if !operator_names.insert(operator.name()) {
			return invalid(format!(
				"operator {:?} occurs in more than one fixed or grouped slot; executable seed semantics would be ambiguous",
				operator.name()
			));
		}
	}
	Ok(())
}

fn validate_operator(operator: &MaaJobOperator) -> Result<(), MaaJobCodecError> {
	if operator.name.trim().is_empty() {
		return invalid("operator names must not be blank");
	}
	if operator.skill.is_some_and(|value| value > 3) {
		return invalid(format!(
			"operator {:?} has skill outside 0..=3",
			operator.name
		));
	}
	if let Some(requirements) = &operator.requirements {
		validate_requirements(operator.name(), requirements)?;
	}
	Ok(())
}

fn validate_requirements(
	operator_name: &str,
	requirements: &MaaJobRequirements,
) -> Result<(), MaaJobCodecError> {
	if requirements.elite.is_some_and(|value| value > 2) {
		return invalid(format!(
			"operator {operator_name:?} has elite outside 0..=2"
		));
	}
	if requirements.level.is_some_and(|value| value > 90) {
		return invalid(format!(
			"operator {operator_name:?} has level outside 0..=90"
		));
	}
	if requirements
		.module
		.is_some_and(|value| !(0..=3).contains(&value))
	{
		return invalid(format!(
			"operator {operator_name:?} has module outside 0..=3"
		));
	}
	if requirements.potential.is_some_and(|value| value > 6) {
		return invalid(format!(
			"operator {operator_name:?} has potential outside 0..=6"
		));
	}
	if requirements.skill_level.is_some_and(|value| value > 10) {
		return invalid(format!(
			"operator {operator_name:?} has skill_level outside 0..=10"
		));
	}
	match (requirements.module, requirements.module_level) {
		(_, Some(level)) if level > 3 => invalid(format!(
			"operator {operator_name:?} has module_level outside 0..=3"
		)),
		(None | Some(0), Some(level)) if level > 0 => invalid(format!(
			"operator {operator_name:?} has positive module_level without a selected module"
		)),
		_ => Ok(()),
	}
}

fn validate_action_references(job: &MaaJob) -> Result<(), MaaJobCodecError> {
	let slot_names: BTreeSet<&str> = job
		.groups
		.iter()
		.map(|group| group.name())
		.chain(job.opers.iter().map(MaaJobOperator::name))
		.collect();
	let group_operator_names: BTreeSet<&str> = job
		.groups
		.iter()
		.flat_map(|group| group.opers.iter())
		.map(MaaJobOperator::name)
		.collect();
	for (action_index, action) in job.actions.iter().enumerate() {
		let Some(name) = action.target_name() else {
			continue;
		};
		if slot_names.contains(name) {
			continue;
		}
		if group_operator_names.contains(name) {
			return invalid(format!(
				"action {action_index} references group alternative {name:?} directly"
			));
		}
		if matches!(action, MaaJobAction::Deploy { .. }) {
			return invalid(format!(
				"Deploy action {action_index} references unknown operator or group {name:?}"
			));
		}
	}
	Ok(())
}

fn validate_stopwatch_order(job: &MaaJob) -> Result<(), MaaJobCodecError> {
	let mut stopwatch_seen: bool = false;
	for (action_index, action) in job.actions.iter().enumerate() {
		if let Some(value) = action.conditions().time_elapsed {
			if value < 0 {
				return invalid(format!(
					"time_elapsed on action {action_index} must be non-negative"
				));
			}
			if !stopwatch_seen {
				return invalid(format!(
					"time_elapsed on action {action_index} requires a preceding ResetStopwatch"
				));
			}
		}
		if matches!(action, MaaJobAction::ResetStopwatch { .. }) {
			stopwatch_seen = true;
		}
	}
	Ok(())
}

fn validate_condition_ranges(job: &MaaJob) -> Result<(), MaaJobCodecError> {
	for (action_index, action) in job.actions.iter().enumerate() {
		let conditions = action.conditions();
		for (field, value) in [
			("costs", conditions.costs),
			("kills", conditions.kills),
			("pre_delay", conditions.pre_delay),
			("post_delay", conditions.post_delay),
			("time_elapsed", conditions.time_elapsed),
		] {
			if value.is_some_and(|value| value < 0) {
				return invalid(format!(
					"{field} on action {action_index} must be non-negative"
				));
			}
		}
		if conditions.cooling.is_some_and(|value| value < -1) {
			return invalid(format!(
				"cooling on action {action_index} must be -1 or non-negative"
			));
		}
		// cost_changes intentionally permits negative values: MAA uses the sign to describe
		// direction of the DP delta.
	}
	Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, MaaJobCodecError> {
	Err(MaaJobCodecError::InvalidJob {
		message: message.into(),
	})
}
