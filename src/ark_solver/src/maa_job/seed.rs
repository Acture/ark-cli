use std::collections::BTreeMap;
use std::iter::FusedIterator;

use super::codec::{validate_maa_job, validate_maa_job_executable, MaaJobCodecError};
use super::model::{
	MaaJob, MaaJobAction, MaaJobDifficulty, MaaJobDirection, MaaJobGroup, MaaJobOperator,
};

impl MaaJob {
	/// Visits at most `budget` protocol-level group selections for one selected difficulty.
	///
	/// This validates structural invariants but deliberately preserves importable protocol shapes
	/// that replay cannot execute. Production replay should use
	/// [`Self::executable_seed_selections`].
	pub fn seed_selections(
		&self,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<MaaJobSeedSelectionIter<'_>, MaaJobCodecError> {
		MaaJobSeedSelectionIter::new(self, difficulty, budget)
	}

	/// Visits selections only when the job is supported by executable replay semantics.
	pub fn executable_seed_selections(
		&self,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<MaaJobSeedSelectionIter<'_>, MaaJobCodecError> {
		MaaJobSeedSelectionIter::new_executable(self, difficulty, budget)
	}

	/// Visits protocol-level source-lineage-bearing hints without materializing every seed.
	///
	/// Production planning should use [`Self::executable_guide_seeds`].
	pub fn guide_seeds<'a>(
		&'a self,
		source_lineage: MaaJobSourceLineage<'a>,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<MaaJobGuideSeedIter<'a>, MaaJobCodecError> {
		Ok(MaaJobGuideSeedIter {
			source_lineage,
			selections: self.seed_selections(difficulty, budget)?,
		})
	}

	/// Visits source-lineage-bearing hints only for an executable-profile job.
	pub fn executable_guide_seeds<'a>(
		&'a self,
		source_lineage: MaaJobSourceLineage<'a>,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<MaaJobGuideSeedIter<'a>, MaaJobCodecError> {
		Ok(MaaJobGuideSeedIter {
			source_lineage,
			selections: self.executable_seed_selections(difficulty, budget)?,
		})
	}
}

/// Progress for one budgeted, single-difficulty group traversal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaaJobExpansionReport {
	potential: Option<u128>,
	visited: usize,
	truncated: bool,
}

impl MaaJobExpansionReport {
	/// Exact source-order selection count, or `None` when the product exceeds `u128`.
	#[must_use]
	pub const fn potential(&self) -> Option<u128> {
		self.potential
	}

	#[must_use]
	pub const fn visited(&self) -> usize {
		self.visited
	}

	#[must_use]
	pub const fn truncated(&self) -> bool {
		self.truncated
	}
}

/// Lazy source-order selections for one explicit difficulty.
#[derive(Debug)]
pub struct MaaJobSeedSelectionIter<'a> {
	job: &'a MaaJob,
	difficulty: MaaJobDifficulty,
	selection_indices: Vec<usize>,
	potential: Option<u128>,
	budget: usize,
	visited: usize,
	source_exhausted: bool,
}

impl<'a> MaaJobSeedSelectionIter<'a> {
	fn new(
		job: &'a MaaJob,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<Self, MaaJobCodecError> {
		validate_maa_job(job)?;
		Self::new_validated(job, difficulty, budget)
	}

	fn new_executable(
		job: &'a MaaJob,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<Self, MaaJobCodecError> {
		validate_maa_job_executable(job)?;
		Self::new_validated(job, difficulty, budget)
	}

	fn new_validated(
		job: &'a MaaJob,
		difficulty: MaaJobDifficulty,
		budget: usize,
	) -> Result<Self, MaaJobCodecError> {
		if !job.difficulty.contains(difficulty) {
			return Err(MaaJobCodecError::DifficultyNotSelected {
				requested: difficulty,
			});
		}
		Ok(Self {
			job,
			difficulty,
			selection_indices: vec![0; job.groups.len()],
			potential: potential_selection_count(&job.groups),
			budget,
			visited: 0,
			source_exhausted: false,
		})
	}

	#[must_use]
	pub const fn report(&self) -> MaaJobExpansionReport {
		MaaJobExpansionReport {
			potential: self.potential,
			visited: self.visited,
			truncated: !self.source_exhausted && self.visited >= self.budget,
		}
	}

	fn advance_selection(&mut self) {
		for group_index in (0..self.selection_indices.len()).rev() {
			let operator_count: usize = self.job.groups[group_index].opers.len();
			let next_operator_index: usize = self.selection_indices[group_index] + 1;
			if next_operator_index < operator_count {
				self.selection_indices[group_index] = next_operator_index;
				return;
			}
			self.selection_indices[group_index] = 0;
		}
		self.source_exhausted = true;
	}

	fn remaining_len(&self) -> usize {
		if self.source_exhausted {
			return 0;
		}
		let budget_remaining: usize = self.budget.saturating_sub(self.visited);
		match self
			.potential
			.and_then(|potential| usize::try_from(potential).ok())
		{
			Some(potential) => budget_remaining.min(potential.saturating_sub(self.visited)),
			None => budget_remaining,
		}
	}
}

impl<'a> Iterator for MaaJobSeedSelectionIter<'a> {
	type Item = MaaJobSeedSelection<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		if self.source_exhausted || self.visited >= self.budget {
			return None;
		}
		let selection: MaaJobSeedSelection<'a> = MaaJobSeedSelection {
			job: self.job,
			difficulty: self.difficulty,
			selection_indices: self.selection_indices.clone(),
		};
		self.visited += 1;
		self.advance_selection();
		Some(selection)
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let remaining: usize = self.remaining_len();
		(remaining, Some(remaining))
	}
}

impl ExactSizeIterator for MaaJobSeedSelectionIter<'_> {}
impl FusedIterator for MaaJobSeedSelectionIter<'_> {}

/// One borrowed group selection from a source job.
#[derive(Clone, Debug)]
pub struct MaaJobSeedSelection<'a> {
	job: &'a MaaJob,
	difficulty: MaaJobDifficulty,
	selection_indices: Vec<usize>,
}

impl<'a> MaaJobSeedSelection<'a> {
	#[must_use]
	pub fn stage_name(&self) -> &str {
		self.job.stage_name()
	}

	#[must_use]
	pub const fn difficulty(&self) -> MaaJobDifficulty {
		self.difficulty
	}

	/// Selected group builds in canonical source group order.
	pub fn group_hints(
		&self,
	) -> impl ExactSizeIterator<Item = MaaJobBuildHint<'a>> + DoubleEndedIterator + '_ {
		self.job
			.groups
			.iter()
			.zip(self.selection_indices.iter().copied())
			.map(|(group, operator_index)| MaaJobBuildHint {
				group_name: Some(group.name()),
				slot_name: group.name(),
				operator: &group.opers[operator_index],
			})
	}

	/// Materializes only this selection into an owned replay seed.
	#[must_use]
	pub fn materialize(self) -> MaaJobSeed {
		let substitutions: BTreeMap<&str, &str> = self
			.group_hints()
			.map(|hint| (hint.slot_name(), hint.operator_name()))
			.collect();
		let actions: Vec<MaaJobAction> = self
			.job
			.actions
			.iter()
			.map(|action| substitute_action(action, &substitutions))
			.collect();
		let mut operators: Vec<MaaJobOperator> = self
			.group_hints()
			.map(|hint| hint.operator().clone())
			.collect();
		operators.extend(self.job.opers.iter().cloned());
		MaaJobSeed {
			stage_name: self.job.stage_name.clone(),
			difficulty: self.difficulty,
			operators,
			actions,
		}
	}

	#[must_use]
	pub fn into_guide_seed(self, source_lineage: MaaJobSourceLineage<'a>) -> MaaJobGuideSeed<'a> {
		MaaJobGuideSeed {
			source_lineage,
			selection: self,
		}
	}
}

/// One operator-name and build hint retained from an imported source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaaJobBuildHint<'a> {
	group_name: Option<&'a str>,
	slot_name: &'a str,
	operator: &'a MaaJobOperator,
}

impl<'a> MaaJobBuildHint<'a> {
	#[must_use]
	pub const fn group_name(&self) -> Option<&'a str> {
		self.group_name
	}

	#[must_use]
	pub const fn slot_name(&self) -> &'a str {
		self.slot_name
	}

	#[must_use]
	pub fn operator_name(&self) -> &'a str {
		self.operator.name()
	}

	#[must_use]
	pub const fn operator(&self) -> &'a MaaJobOperator {
		self.operator
	}
}

/// Stable external identity for one imported job; an upstream MAA job file identifier may be
/// retained as provenance without changing the executable wire protocol.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MaaJobSourceLineage<'a> {
	source_id: &'a str,
	content_sha256: &'a str,
}

impl<'a> MaaJobSourceLineage<'a> {
	pub fn new(source_id: &'a str, content_sha256: &'a str) -> Result<Self, MaaJobCodecError> {
		if source_id.trim().is_empty() {
			return Err(MaaJobCodecError::BlankSourceLineage);
		}
		if content_sha256.len() != 64
			|| !content_sha256
				.bytes()
				.all(|byte: u8| byte.is_ascii_hexdigit())
		{
			return Err(MaaJobCodecError::InvalidSourceDigest);
		}
		Ok(Self {
			source_id,
			content_sha256,
		})
	}

	#[must_use]
	pub const fn source_id(&self) -> &'a str {
		self.source_id
	}

	#[must_use]
	pub const fn content_sha256(&self) -> &'a str {
		self.content_sha256
	}
}

/// A source-provided deployment anchor and timing hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaaJobDeploymentHint<'a> {
	action_index: usize,
	build: MaaJobBuildHint<'a>,
	cell: [i32; 2],
	facing: MaaJobDirection,
	conditions: &'a super::model::MaaActionConditions,
}

impl<'a> MaaJobDeploymentHint<'a> {
	#[must_use]
	pub const fn action_index(&self) -> usize {
		self.action_index
	}

	#[must_use]
	pub const fn build(&self) -> MaaJobBuildHint<'a> {
		self.build
	}

	#[must_use]
	pub const fn cell(&self) -> [i32; 2] {
		self.cell
	}

	#[must_use]
	pub const fn facing(&self) -> MaaJobDirection {
		self.facing
	}

	#[must_use]
	pub const fn time_elapsed(&self) -> Option<i32> {
		self.conditions.time_elapsed()
	}

	#[must_use]
	pub const fn pre_delay(&self) -> Option<i32> {
		self.conditions.pre_delay()
	}

	#[must_use]
	pub const fn conditions(&self) -> &'a super::model::MaaActionConditions {
		self.conditions
	}
}

/// A source-lineage-bearing job selection used by workflow derivation and search ordering.
#[derive(Clone, Debug)]
pub struct MaaJobGuideSeed<'a> {
	source_lineage: MaaJobSourceLineage<'a>,
	selection: MaaJobSeedSelection<'a>,
}

impl<'a> MaaJobGuideSeed<'a> {
	#[must_use]
	pub const fn source_lineage(&self) -> MaaJobSourceLineage<'a> {
		self.source_lineage
	}

	#[must_use]
	pub const fn source_id(&self) -> &'a str {
		self.source_lineage.source_id()
	}

	#[must_use]
	pub const fn content_sha256(&self) -> &'a str {
		self.source_lineage.content_sha256()
	}

	#[must_use]
	pub fn stage_name(&self) -> &str {
		self.selection.stage_name()
	}

	#[must_use]
	pub const fn difficulty(&self) -> MaaJobDifficulty {
		self.selection.difficulty()
	}

	#[must_use]
	pub fn materialize(&self) -> MaaJobSeed {
		self.selection.clone().materialize()
	}

	pub fn build_hints(&self) -> impl DoubleEndedIterator<Item = MaaJobBuildHint<'a>> + '_ {
		self.selection
			.group_hints()
			.chain(
				self.selection
					.job
					.opers
					.iter()
					.map(|operator| MaaJobBuildHint {
						group_name: None,
						slot_name: operator.name(),
						operator,
					}),
			)
	}

	pub fn deployment_hints(&self) -> impl Iterator<Item = MaaJobDeploymentHint<'a>> + '_ {
		self.selection
			.job
			.actions
			.iter()
			.enumerate()
			.filter_map(|(action_index, action)| {
				let MaaJobAction::Deploy {
					name,
					location,
					direction,
					conditions,
					..
				} = action
				else {
					return None;
				};
				self.build_hint_for_slot(name)
					.map(|build| MaaJobDeploymentHint {
						action_index,
						build,
						cell: *location,
						facing: *direction,
						conditions,
					})
			})
	}

	fn build_hint_for_slot(&self, slot_name: &str) -> Option<MaaJobBuildHint<'a>> {
		self.build_hints()
			.find(|build| build.slot_name() == slot_name)
	}
}

#[derive(Debug)]
pub struct MaaJobGuideSeedIter<'a> {
	source_lineage: MaaJobSourceLineage<'a>,
	selections: MaaJobSeedSelectionIter<'a>,
}

impl MaaJobGuideSeedIter<'_> {
	#[must_use]
	pub const fn report(&self) -> MaaJobExpansionReport {
		self.selections.report()
	}
}

impl<'a> Iterator for MaaJobGuideSeedIter<'a> {
	type Item = MaaJobGuideSeed<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		self.selections.next().map(|selection| MaaJobGuideSeed {
			source_lineage: self.source_lineage,
			selection,
		})
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		self.selections.size_hint()
	}
}

impl ExactSizeIterator for MaaJobGuideSeedIter<'_> {}
impl FusedIterator for MaaJobGuideSeedIter<'_> {}

/// One independent normalized seed selected from a job.
#[derive(Clone, Debug, PartialEq)]
pub struct MaaJobSeed {
	stage_name: String,
	difficulty: MaaJobDifficulty,
	operators: Vec<MaaJobOperator>,
	actions: Vec<MaaJobAction>,
}

impl MaaJobSeed {
	#[must_use]
	pub fn stage_name(&self) -> &str {
		&self.stage_name
	}

	#[must_use]
	pub const fn difficulty(&self) -> MaaJobDifficulty {
		self.difficulty
	}

	#[must_use]
	pub fn operators(&self) -> &[MaaJobOperator] {
		&self.operators
	}

	#[must_use]
	pub fn actions(&self) -> &[MaaJobAction] {
		&self.actions
	}

	pub(crate) fn clone_without_named_operator(&self, source_name: &str) -> Self {
		debug_assert_eq!(
			self.operators
				.iter()
				.filter(|operator| operator.name() == source_name)
				.count(),
			1,
			"caller must prove one source operator"
		);
		Self {
			stage_name: self.stage_name.clone(),
			difficulty: self.difficulty,
			operators: self
				.operators
				.iter()
				.filter(|operator| operator.name() != source_name)
				.cloned()
				.collect(),
			actions: self
				.actions
				.iter()
				.filter(|action| action.target_name() != Some(source_name))
				.cloned()
				.collect(),
		}
	}
}

fn potential_selection_count(groups: &[MaaJobGroup]) -> Option<u128> {
	groups.iter().try_fold(1_u128, |count, group| {
		count.checked_mul(group.opers.len() as u128)
	})
}

fn substitute_action(action: &MaaJobAction, substitutions: &BTreeMap<&str, &str>) -> MaaJobAction {
	match action {
		MaaJobAction::Deploy {
			name,
			location,
			direction,
			timeout,
			conditions,
		} => MaaJobAction::Deploy {
			name: substitute_name(name, substitutions),
			location: *location,
			direction: *direction,
			timeout: *timeout,
			conditions: conditions.clone(),
		},
		MaaJobAction::Skill {
			name,
			location,
			timeout,
			conditions,
		} => MaaJobAction::Skill {
			name: substitute_optional_name(name, substitutions),
			location: *location,
			timeout: *timeout,
			conditions: conditions.clone(),
		},
		MaaJobAction::Retreat {
			name,
			location,
			conditions,
		} => MaaJobAction::Retreat {
			name: substitute_optional_name(name, substitutions),
			location: *location,
			conditions: conditions.clone(),
		},
		MaaJobAction::BulletTime {
			name,
			location,
			conditions,
		} => MaaJobAction::BulletTime {
			name: substitute_optional_name(name, substitutions),
			location: *location,
			conditions: conditions.clone(),
		},
		MaaJobAction::SkillUsage {
			name,
			skill_usage,
			skill_times,
			conditions,
		} => MaaJobAction::SkillUsage {
			name: substitute_name(name, substitutions),
			skill_usage: *skill_usage,
			skill_times: *skill_times,
			conditions: conditions.clone(),
		},
		MaaJobAction::SpeedUp { conditions } => MaaJobAction::SpeedUp {
			conditions: conditions.clone(),
		},
		MaaJobAction::Output { conditions } => MaaJobAction::Output {
			conditions: conditions.clone(),
		},
		MaaJobAction::SkillDaemon { conditions } => MaaJobAction::SkillDaemon {
			conditions: conditions.clone(),
		},
		MaaJobAction::MoveCamera {
			distance,
			conditions,
		} => MaaJobAction::MoveCamera {
			distance: *distance,
			conditions: conditions.clone(),
		},
		MaaJobAction::ResetStopwatch { conditions } => MaaJobAction::ResetStopwatch {
			conditions: conditions.clone(),
		},
	}
}

fn substitute_name(name: &str, substitutions: &BTreeMap<&str, &str>) -> String {
	substitutions.get(name).copied().unwrap_or(name).to_owned()
}

fn substitute_optional_name(
	name: &Option<String>,
	substitutions: &BTreeMap<&str, &str>,
) -> Option<String> {
	name.as_deref()
		.map(|name| substitute_name(name, substitutions))
}
