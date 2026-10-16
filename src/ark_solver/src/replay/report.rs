//! Auditable JSON and canonical binary output for one expected replay preflight/run.

use std::collections::{BTreeMap, BTreeSet};

use crate::data::OperatorModule;
use crate::simulation::TraceEntry;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
	compile::ExpectedReplayInput,
	engine::{
		compile_battle, evaluate_defensive_probability, replay_expected, ActionAuditStatus,
		BlockAuditEntry, BlockEndReason, DefensiveProbabilityAssessment, ExpectedEvent,
		ExpectedTerminalStatus, ExpectedValueAssessment, GuideExecutorActionKind,
		SkillActivationOrigin,
	},
};
use crate::{
	mechanics::domain::{
		ApproximationTrigger, BattleActorId, BattleTimeAnchor, MaterializedGuideAction,
		MechanicBehavior, MechanicId, MechanicSupportReport, OperatorInstanceId, PathId,
		ReachedMechanic,
	},
	verification_artifact::{
		ArtifactAction, ArtifactEnemyAssessment, ArtifactExactBuild, ArtifactFacing,
		ArtifactOperatorAssessment, ArtifactUnsupportedMechanic, DeterministicDamageStatus,
		ExpectedStrategyStatus, MaterializedActionKind, StrategyArtifact,
		STRATEGY_ARTIFACT_SCHEMA_VERSION,
	},
};

pub const EXPECTED_REPLAY_REPORT_SCHEMA_VERSION: u32 = 11;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedReplayReportStatus {
	Unsupported,
	ExpectedVictory,
	ExpectedDefeat,
	Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayUnsupportedEntry {
	pub mechanic_id: String,
	pub origins: Vec<String>,
	pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayConservativeOmissionEntry {
	pub mechanic_id: String,
	pub origins: Vec<String>,
	pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayUnmodeledOmissionEntry {
	pub mechanic_id: String,
	pub origins: Vec<String>,
	pub reasons: Vec<String>,
}

/// One behavior replay executed in a declared simplified form rather than omitting.
///
/// This is disclosure, not a gate. Replay executes the effect at the value the model declares, so an
/// approximation is part of what an `Expected*` verdict is a verdict about, the way expected damage
/// and whole-cell positions are. `exercised` says whether this battle ran the simplification at all,
/// which is what a later neutrality or sensitivity argument has to start from: an unexercised entry
/// cannot have moved this outcome, and an exercised one is the one worth bounding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayDeclaredApproximationEntry {
	pub mechanic_id: String,
	pub origins: Vec<String>,
	pub reasons: Vec<String>,
	pub exercised: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayEnemyAudit {
	pub instance_id: u32,
	pub enemy_key: String,
	pub spawned_tick: Option<u64>,
	pub deadline_tick: Option<u64>,
	pub max_hp_micros: u64,
	pub remaining_hp_micros: u64,
	pub expected_damage_taken_micros: u64,
	pub death_tick: Option<u64>,
	pub leaked_tick: Option<u64>,
	pub deadline_margin_ticks: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayWaveAnchorAudit {
	pub wave_id: u32,
	pub started_tick: Option<u64>,
	pub cleared_tick: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayOperatorAudit {
	pub instance_id: u32,
	pub operator_id: String,
	pub max_hp_micros: u64,
	pub remaining_hp_micros: u64,
	/// Start tick of the operator's most recent deployment episode.
	pub deployed_tick: Option<u64>,
	/// Death tick of that deployment episode, or `None` if it did not end in death.
	pub death_tick: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayBlockerKind {
	Operator,
	Enemy,
	Summon,
	Stage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayBlockEndReason {
	EnemyDeath,
	BlockerDeath,
	Retreat,
	BlockCountChange,
	BlockTargetChange,
	Shift,
	Goal,
	BattleDeadline,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayBlockAudit {
	pub enemy_instance_id: u32,
	pub blocker_kind: ReplayBlockerKind,
	pub blocker_instance_id: Option<u32>,
	pub start_tick: u64,
	pub end_tick: Option<u64>,
	pub end_reason: Option<ReplayBlockEndReason>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayActionAudit {
	pub action_index: usize,
	pub kind: String,
	pub operator_id: Option<String>,
	pub row: Option<i32>,
	pub column: Option<i32>,
	pub facing: ArtifactFacing,
	pub requested_tick: u64,
	pub executed_tick: Option<u64>,
	pub status: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaySkillActivationSource {
	GuideAction,
	Automatic,
	SkillDaemon,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplaySkillActivationAudit {
	pub operator_instance_id: u32,
	pub operator_id: String,
	pub mechanic_id: String,
	pub tick: u64,
	pub source: ReplaySkillActivationSource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayDpEntryAudit {
	pub tick: u64,
	pub kind: String,
	pub before: u32,
	pub amount: u32,
	pub after: u32,
	pub action_index: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplayDpAudit {
	pub initial: u32,
	pub final_amount: u32,
	pub entries: Vec<ReplayDpEntryAudit>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReplayDefensiveProbabilityAudit {
	pub verified: bool,
	pub success_probability: Option<f64>,
	pub covered_probability_mass: Option<f64>,
	pub success_probability_millionths: u32,
	pub covered_probability_millionths: u32,
	pub processed_steps: usize,
	pub peak_branches: usize,
	pub issues: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExpectedReplayAuditReport {
	pub schema_version: u32,
	pub data_revision: String,
	pub mechanics_revision: String,
	pub stage_id: String,
	pub guide_variant_id: String,
	pub status: ExpectedReplayReportStatus,
	pub path_count: usize,
	pub wave_count: usize,
	pub enemy_spawn_count: usize,
	pub enemy_definition_count: usize,
	pub preplaced_actor_count: usize,
	pub exact_build_count: usize,
	pub exact_builds: Vec<ArtifactExactBuild>,
	pub unsupported: Vec<ReplayUnsupportedEntry>,
	pub conservative_omissions: Vec<ReplayConservativeOmissionEntry>,
	pub unmodeled_omissions: Vec<ReplayUnmodeledOmissionEntry>,
	pub declared_approximations: Vec<ReplayDeclaredApproximationEntry>,
	pub compile_issues: Vec<String>,
	pub terminal: Option<String>,
	pub wave_anchors: Vec<ReplayWaveAnchorAudit>,
	pub enemies: Vec<ReplayEnemyAudit>,
	pub operators: Vec<ReplayOperatorAudit>,
	pub blocks: Vec<ReplayBlockAudit>,
	pub actions: Vec<ReplayActionAudit>,
	pub skill_activations: Vec<ReplaySkillActivationAudit>,
	pub dp: Option<ReplayDpAudit>,
	pub defensive_probability: Option<ReplayDefensiveProbabilityAudit>,
	pub simulation_issues: Vec<String>,
	pub trace_sha256: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedReplayOutput {
	pub artifact: StrategyArtifact,
	pub report: ExpectedReplayAuditReport,
}

/// Runs preflight and only enters replay when every reached mechanic is supported.
#[must_use]
pub fn execute_expected_replay(
	input: &ExpectedReplayInput,
	guide_sha256: [u8; 32],
	guide_variant_id: &str,
) -> ExpectedReplayOutput {
	let counts: (usize, usize, usize, usize, usize, usize) = (
		input.facts.paths.len(),
		input.facts.waves.len(),
		input.facts.waves.iter().map(|wave| wave.spawns.len()).sum(),
		input.facts.enemies.len(),
		input.facts.preplaced_actors.len(),
		input.resolved_builds.len(),
	);
	match compile_battle(&input.facts, &input.guide) {
		Ok(battle) => {
			let assessment: ExpectedValueAssessment =
				replay_expected(&battle, input.policy.clone());
			let defensive_probability: DefensiveProbabilityAssessment =
				evaluate_defensive_probability(&battle, input.policy.clone());
			output_from_assessment(
				input,
				guide_sha256,
				guide_variant_id,
				counts,
				&assessment,
				&defensive_probability,
			)
		}
		Err(support) => {
			output_from_support(input, guide_sha256, guide_variant_id, counts, &support)
		}
	}
}

fn output_from_support(
	input: &ExpectedReplayInput,
	guide_sha256: [u8; 32],
	guide_variant_id: &str,
	counts: (usize, usize, usize, usize, usize, usize),
	support: &MechanicSupportReport,
) -> ExpectedReplayOutput {
	let trace_sha256: [u8; 32] = Sha256::digest(format!("{support:?}")).into();
	let conservative_omissions: Vec<ReplayConservativeOmissionEntry> =
		collect_conservative_omissions(input);
	let unmodeled_omissions: Vec<ReplayUnmodeledOmissionEntry> = collect_unmodeled_omissions(input);
	let declared_approximations: Vec<ReplayDeclaredApproximationEntry> =
		collect_declared_approximations(input, None);
	let unsupported: Vec<ReplayUnsupportedEntry> = support
		.unsupported()
		.iter()
		.map(|entry| ReplayUnsupportedEntry {
			mechanic_id: entry.id.as_str().to_owned(),
			origins: entry.origins.clone(),
			reasons: entry.reasons.clone(),
		})
		.collect();
	let artifact: StrategyArtifact = base_artifact(
		input,
		guide_sha256,
		guide_variant_id,
		ExpectedStrategyStatus::Unsupported,
		trace_sha256,
		None,
		support
			.unsupported()
			.iter()
			.flat_map(|entry| {
				entry
					.origins
					.iter()
					.map(|origin| ArtifactUnsupportedMechanic {
						mechanic_id: entry.id.as_str().to_owned(),
						origin: origin.clone(),
					})
			})
			.collect(),
	);
	ExpectedReplayOutput {
		artifact,
		report: ExpectedReplayAuditReport {
			schema_version: EXPECTED_REPLAY_REPORT_SCHEMA_VERSION,
			data_revision: input.data_revision.clone(),
			mechanics_revision: input.mechanics_revision.clone(),
			stage_id: input.stage_id.clone(),
			guide_variant_id: guide_variant_id.to_owned(),
			status: ExpectedReplayReportStatus::Unsupported,
			path_count: counts.0,
			wave_count: counts.1,
			enemy_spawn_count: counts.2,
			enemy_definition_count: counts.3,
			preplaced_actor_count: counts.4,
			exact_build_count: counts.5,
			exact_builds: exact_build_records(input),
			unsupported,
			conservative_omissions,
			unmodeled_omissions,
			declared_approximations,
			compile_issues: support
				.issues()
				.iter()
				.map(|issue| format!("{issue:?}"))
				.collect(),
			terminal: None,
			wave_anchors: Vec::new(),
			enemies: Vec::new(),
			operators: Vec::new(),
			blocks: Vec::new(),
			actions: unsupported_action_audits(&input.guide),
			skill_activations: Vec::new(),
			dp: None,
			defensive_probability: None,
			simulation_issues: Vec::new(),
			trace_sha256: hex_digest(trace_sha256),
		},
	}
}

/// What one finished battle observed, for deciding which declared approximations it ran.
///
/// Every test fails closed: an actor that fired a normal attack counts as having run its attack
/// approximation even if that attack found no target, and a path counts as spawned onto as soon as
/// any enemy bound to it entered the battle.
struct ExercisedApproximations {
	attacking_enemy_keys: BTreeSet<String>,
	attacking_operator_ids: BTreeSet<String>,
	spawned_paths: BTreeSet<PathId>,
}

impl ExercisedApproximations {
	fn observe(input: &ExpectedReplayInput, assessment: &ExpectedValueAssessment) -> Self {
		let mut attacking_enemy_keys: BTreeSet<String> = BTreeSet::new();
		let mut attacking_operator_ids: BTreeSet<String> = BTreeSet::new();
		for entry in assessment.trace().entries() {
			let TraceEntry::Event { scheduled } = entry else {
				continue;
			};
			let ExpectedEvent::NormalAttack { actor, .. } = scheduled.event() else {
				continue;
			};
			match actor {
				BattleActorId::Enemy(enemy) => {
					if let Some(state) = assessment.enemies().get(enemy) {
						attacking_enemy_keys.insert(state.enemy_key.clone());
					}
				}
				BattleActorId::Operator(operator) => {
					if let Some(state) = assessment.operators().get(operator) {
						attacking_operator_ids.insert(state.operator_key.clone());
					}
				}
				BattleActorId::Summon(_) | BattleActorId::Stage => {}
			}
		}
		let spawned_paths: BTreeSet<PathId> = input
			.facts
			.waves
			.iter()
			.flat_map(|wave| wave.spawns.iter())
			.filter(|spawn| {
				assessment
					.enemies()
					.get(&spawn.id)
					.is_some_and(|state| state.spawned_at.is_some())
			})
			.map(|spawn| spawn.path)
			.collect();
		Self {
			attacking_enemy_keys,
			attacking_operator_ids,
			spawned_paths,
		}
	}

	fn ran(&self, trigger: &ApproximationTrigger) -> bool {
		match trigger {
			ApproximationTrigger::EnemyNormalAttack { enemy_key } => {
				self.attacking_enemy_keys.contains(enemy_key)
			}
			ApproximationTrigger::OperatorNormalAttack { operator_id } => {
				self.attacking_operator_ids.contains(operator_id)
			}
			ApproximationTrigger::PathSpawn { path } => self.spawned_paths.contains(path),
		}
	}
}

/// Every mechanic this materialized build reached, across all five inventories.
fn reached_mechanics(input: &ExpectedReplayInput) -> impl Iterator<Item = &ReachedMechanic> {
	input
		.facts
		.reached_mechanics
		.iter()
		.chain(input.guide.reached_mechanics.iter())
		.chain(
			input
				.guide
				.builds
				.iter()
				.flat_map(|build| build.reached_mechanics.iter()),
		)
		.chain(
			input
				.facts
				.enemies
				.iter()
				.flat_map(|enemy| enemy.reached_mechanics.iter()),
		)
		.chain(
			input
				.facts
				.summons
				.iter()
				.flat_map(|summon| summon.reached_mechanics.iter()),
		)
}

/// Aggregates one behavior's reached mechanics into stable-ID entries with origins and reasons.
fn aggregate_reached<'a>(
	input: &'a ExpectedReplayInput,
	reason_of: impl Fn(&'a MechanicBehavior) -> Option<&'a String>,
) -> Vec<(String, Vec<String>, Vec<String>)> {
	let mut aggregate: BTreeMap<MechanicId, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
	for mechanic in reached_mechanics(input) {
		let Some(reason) = reason_of(&mechanic.behavior) else {
			continue;
		};
		let entry = aggregate.entry(mechanic.id.clone()).or_default();
		entry.0.insert(mechanic.origin.clone());
		entry.1.insert(reason.clone());
	}
	aggregate
		.into_iter()
		.map(|(id, (origins, reasons))| {
			(
				id.as_str().to_owned(),
				origins.into_iter().collect(),
				reasons.into_iter().collect(),
			)
		})
		.collect()
}

fn collect_conservative_omissions(
	input: &ExpectedReplayInput,
) -> Vec<ReplayConservativeOmissionEntry> {
	aggregate_reached(input, |behavior| match behavior {
		MechanicBehavior::ConservativeBeneficialOmission { reason } => Some(reason),
		_ => None,
	})
	.into_iter()
	.map(
		|(mechanic_id, origins, reasons)| ReplayConservativeOmissionEntry {
			mechanic_id,
			origins,
			reasons,
		},
	)
	.collect()
}

fn collect_unmodeled_omissions(input: &ExpectedReplayInput) -> Vec<ReplayUnmodeledOmissionEntry> {
	aggregate_reached(input, |behavior| match behavior {
		MechanicBehavior::UnmodeledOmission { reason } => Some(reason),
		_ => None,
	})
	.into_iter()
	.map(
		|(mechanic_id, origins, reasons)| ReplayUnmodeledOmissionEntry {
			mechanic_id,
			origins,
			reasons,
		},
	)
	.collect()
}

/// The declared approximations this build reached, each marked with whether the battle ran it.
///
/// `exercised` is what the battle observed, so it is only knowable from an assessment. A preflight
/// that never ran one fails closed and reports every reached approximation as exercised.
fn collect_declared_approximations(
	input: &ExpectedReplayInput,
	exercised: Option<&ExercisedApproximations>,
) -> Vec<ReplayDeclaredApproximationEntry> {
	let mut triggers: BTreeMap<MechanicId, BTreeSet<&ApproximationTrigger>> = BTreeMap::new();
	for mechanic in reached_mechanics(input) {
		if let MechanicBehavior::DeclaredApproximation { exercised_by, .. } = &mechanic.behavior {
			triggers
				.entry(mechanic.id.clone())
				.or_default()
				.insert(exercised_by);
		}
	}
	aggregate_reached(input, |behavior| match behavior {
		MechanicBehavior::DeclaredApproximation { reason, .. } => Some(reason),
		_ => None,
	})
	.into_iter()
	.map(|(mechanic_id, origins, reasons)| {
		let exercised: bool = exercised.is_none_or(|observed| {
			triggers
				.get(&MechanicId::new(mechanic_id.clone()))
				.is_some_and(|triggers| triggers.iter().any(|trigger| observed.ran(trigger)))
		});
		ReplayDeclaredApproximationEntry {
			mechanic_id,
			origins,
			reasons,
			exercised,
		}
	})
	.collect()
}

fn output_from_assessment(
	input: &ExpectedReplayInput,
	guide_sha256: [u8; 32],
	guide_variant_id: &str,
	counts: (usize, usize, usize, usize, usize, usize),
	assessment: &ExpectedValueAssessment,
	defensive_probability: &DefensiveProbabilityAssessment,
) -> ExpectedReplayOutput {
	let trace_sha256: [u8; 32] = Sha256::digest(format!("{:?}", assessment.trace())).into();
	let conservative_omissions: Vec<ReplayConservativeOmissionEntry> =
		collect_conservative_omissions(input);
	let unmodeled_omissions: Vec<ReplayUnmodeledOmissionEntry> = collect_unmodeled_omissions(input);
	let declared_approximations: Vec<ReplayDeclaredApproximationEntry> =
		collect_declared_approximations(
			input,
			Some(&ExercisedApproximations::observe(input, assessment)),
		);
	let departed: DeclaredDepartures =
		DeclaredDepartures::of(&conservative_omissions, &unmodeled_omissions);
	let has_issues: bool = !assessment.issues().is_empty();
	let has_pending_required_actions: bool = assessment.action_audit().iter().any(|action| {
		matches!(
			action.status,
			ActionAuditStatus::WaitedForDp | ActionAuditStatus::SkippedNotReady
		)
	});
	let (report_status, artifact_status): (ExpectedReplayReportStatus, ExpectedStrategyStatus) =
		classify_replay_status(
			assessment.status(),
			has_issues,
			has_pending_required_actions,
			departed,
		);
	let enemy_audits: Vec<ReplayEnemyAudit> = assessment
		.enemies()
		.values()
		.map(|enemy| ReplayEnemyAudit {
			instance_id: enemy.id.get(),
			enemy_key: enemy.enemy_key.clone(),
			spawned_tick: enemy.spawned_at.map(|tick| tick.get()),
			deadline_tick: enemy.deadline_at.map(|tick| tick.get()),
			max_hp_micros: enemy.max_hp.scaled(),
			remaining_hp_micros: enemy.remaining_hp.scaled(),
			expected_damage_taken_micros: enemy.expected_damage_taken.scaled(),
			death_tick: enemy.death_at.map(|tick| tick.get()),
			leaked_tick: enemy.leaked_at.map(|tick| tick.get()),
			deadline_margin_ticks: deadline_margin(
				enemy.deadline_at.map(|tick| tick.get()),
				enemy.death_at.or(enemy.leaked_at).map(|tick| tick.get()),
			),
		})
		.collect();
	let wave_anchor_audits: Vec<ReplayWaveAnchorAudit> = input
		.facts
		.waves
		.iter()
		.map(|wave| ReplayWaveAnchorAudit {
			wave_id: wave.id.get(),
			started_tick: assessment
				.anchors()
				.get(&BattleTimeAnchor::WaveStarted(wave.id))
				.map(|tick| tick.get()),
			cleared_tick: assessment
				.anchors()
				.get(&BattleTimeAnchor::WaveCleared(wave.id))
				.map(|tick| tick.get()),
		})
		.collect();
	let operator_audits: Vec<ReplayOperatorAudit> = assessment
		.operators()
		.values()
		.map(|operator| ReplayOperatorAudit {
			instance_id: operator.id.get(),
			operator_id: operator.operator_key.clone(),
			max_hp_micros: operator.max_hp.scaled(),
			remaining_hp_micros: operator.remaining_hp.scaled(),
			deployed_tick: operator.deployed_at.map(|tick| tick.get()),
			death_tick: operator.death_at.map(|tick| tick.get()),
		})
		.collect();
	let block_audits: Vec<ReplayBlockAudit> = assessment
		.block_audit()
		.iter()
		.map(replay_block_audit)
		.collect();
	let build_keys: BTreeMap<OperatorInstanceId, &str> = guide_build_keys(&input.guide);
	let action_audits: Vec<ReplayActionAudit> = assessment
		.action_audit()
		.iter()
		.map(|action| {
			let materialized: &MaterializedGuideAction = &input.guide.actions[action.action_index];
			let identity: ArtifactAction = artifact_action(
				action.action_index,
				materialized,
				&build_keys,
				action.executed_at.map(|tick| tick.get()),
			);
			ReplayActionAudit {
				action_index: action.action_index,
				kind: action_kind_name(action.kind).to_owned(),
				operator_id: identity.operator_id,
				row: identity.row,
				column: identity.column,
				facing: identity.facing,
				requested_tick: action.requested_at.get(),
				executed_tick: action.executed_at.map(|tick| tick.get()),
				status: action_status_name(action.status).to_owned(),
			}
		})
		.collect();
	let skill_activation_audits: Vec<ReplaySkillActivationAudit> =
		replay_skill_activation_audits(assessment, &build_keys);
	let dp = assessment.dp_audit();
	let dp_audit: ReplayDpAudit = ReplayDpAudit {
		initial: dp.initial,
		final_amount: dp.final_amount,
		entries: dp
			.entries
			.iter()
			.map(|entry| ReplayDpEntryAudit {
				tick: entry.at.get(),
				kind: format!("{:?}", entry.kind).to_ascii_lowercase(),
				before: entry.before,
				amount: entry.amount,
				after: entry.after,
				action_index: entry.action_index,
			})
			.collect(),
	};
	let artifact: StrategyArtifact = base_artifact(
		input,
		guide_sha256,
		guide_variant_id,
		artifact_status,
		trace_sha256,
		Some(assessment),
		Vec::new(),
	);
	ExpectedReplayOutput {
		artifact,
		report: ExpectedReplayAuditReport {
			schema_version: EXPECTED_REPLAY_REPORT_SCHEMA_VERSION,
			data_revision: input.data_revision.clone(),
			mechanics_revision: input.mechanics_revision.clone(),
			stage_id: input.stage_id.clone(),
			guide_variant_id: guide_variant_id.to_owned(),
			status: report_status,
			path_count: counts.0,
			wave_count: counts.1,
			enemy_spawn_count: counts.2,
			enemy_definition_count: counts.3,
			preplaced_actor_count: counts.4,
			exact_build_count: counts.5,
			exact_builds: exact_build_records(input),
			unsupported: Vec::new(),
			conservative_omissions,
			unmodeled_omissions,
			declared_approximations,
			compile_issues: Vec::new(),
			terminal: Some(format!("{:?}", assessment.status())),
			wave_anchors: wave_anchor_audits,
			enemies: enemy_audits,
			operators: operator_audits,
			blocks: block_audits,
			actions: action_audits,
			skill_activations: skill_activation_audits,
			dp: Some(dp_audit),
			defensive_probability: Some(ReplayDefensiveProbabilityAudit {
				verified: defensive_probability.verified() && departed.permit_victory(),
				success_probability: defensive_probability.success_probability(),
				covered_probability_mass: defensive_probability.covered_probability_mass(),
				success_probability_millionths: defensive_probability
					.success_probability_millionths(),
				covered_probability_millionths: defensive_probability
					.covered_probability_millionths(),
				processed_steps: defensive_probability.processed_steps(),
				peak_branches: defensive_probability.peak_branches(),
				issues: defensive_probability.issues().to_vec(),
			}),
			simulation_issues: assessment
				.issues()
				.iter()
				.map(|issue| format!("{issue:?}"))
				.collect(),
			trace_sha256: hex_digest(trace_sha256),
		},
	}
}

/// What a replay deliberately left out, ranked by which verdicts survive.
///
/// Only omissions rank here. An omission is a hole inside the model: replay reached the effect and
/// did not execute it, so the trace does not faithfully represent even its own model. A Declared
/// Approximation is not a hole — replay executes the effect, at a value the model declares — so it
/// is disclosed beside the verdict rather than withholding it. See [`ReplayDeclaredApproximationEntry`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeclaredDepartures {
	/// Either verdict can be certified.
	None,
	/// Only Conservative Beneficial Omissions: a clear can be certified, a leak cannot.
	BeneficialOnly,
	/// At least one Partial Replay Omission: neither verdict can be certified.
	Unmodeled,
}

impl DeclaredDepartures {
	fn of(
		conservative: &[ReplayConservativeOmissionEntry],
		unmodeled: &[ReplayUnmodeledOmissionEntry],
	) -> Self {
		if !unmodeled.is_empty() {
			Self::Unmodeled
		} else if !conservative.is_empty() {
			Self::BeneficialOnly
		} else {
			Self::None
		}
	}

	/// Whether a modelled clear still certifies ExpectedVictory.
	fn permit_victory(self) -> bool {
		matches!(self, Self::None | Self::BeneficialOnly)
	}

	/// Whether a modelled goal leak still certifies ExpectedDefeat.
	fn permit_defeat(self) -> bool {
		matches!(self, Self::None)
	}
}

fn classify_replay_status(
	terminal: &ExpectedTerminalStatus,
	has_issues: bool,
	has_pending_required_actions: bool,
	departed: DeclaredDepartures,
) -> (ExpectedReplayReportStatus, ExpectedStrategyStatus) {
	if has_issues {
		return (
			ExpectedReplayReportStatus::Incomplete,
			ExpectedStrategyStatus::Incomplete,
		);
	}
	match terminal {
		ExpectedTerminalStatus::GoalLeak { .. } if departed.permit_defeat() => (
			ExpectedReplayReportStatus::ExpectedDefeat,
			ExpectedStrategyStatus::ExpectedDefeat,
		),
		ExpectedTerminalStatus::Victory
			if !has_pending_required_actions && departed.permit_victory() =>
		{
			(
				ExpectedReplayReportStatus::ExpectedVictory,
				ExpectedStrategyStatus::ExpectedVictory,
			)
		}
		ExpectedTerminalStatus::GoalLeak { .. }
		| ExpectedTerminalStatus::Victory
		| ExpectedTerminalStatus::Deadline { .. }
		| ExpectedTerminalStatus::SimulationIncomplete => (
			ExpectedReplayReportStatus::Incomplete,
			ExpectedStrategyStatus::Incomplete,
		),
	}
}

fn replay_skill_activation_audits(
	assessment: &ExpectedValueAssessment,
	build_keys: &BTreeMap<OperatorInstanceId, &str>,
) -> Vec<ReplaySkillActivationAudit> {
	let mut activations: Vec<ReplaySkillActivationAudit> = assessment
		.trace()
		.entries()
		.iter()
		.filter_map(|entry| match entry {
			TraceEntry::Event { scheduled } => {
				let ExpectedEvent::SkillActivation {
					operator,
					mechanic,
					origin,
					..
				} = scheduled.event()
				else {
					return None;
				};
				let tick: u64 = scheduled.at().get();
				Some(ReplaySkillActivationAudit {
					operator_instance_id: operator.get(),
					operator_id: build_keys
						.get(operator)
						.copied()
						.unwrap_or("<unknown>")
						.to_owned(),
					mechanic_id: mechanic.as_str().to_owned(),
					tick,
					source: match origin {
						SkillActivationOrigin::GuideAction => {
							ReplaySkillActivationSource::GuideAction
						}
						SkillActivationOrigin::Automatic => ReplaySkillActivationSource::Automatic,
						SkillActivationOrigin::SkillDaemon => {
							ReplaySkillActivationSource::SkillDaemon
						}
					},
				})
			}
			TraceEntry::Command { .. } | TraceEntry::Choice { .. } => None,
		})
		.collect();
	activations.sort_by(|left, right| {
		(
			left.tick,
			&left.operator_id,
			&left.mechanic_id,
			left.operator_instance_id,
			left.source,
		)
			.cmp(&(
				right.tick,
				&right.operator_id,
				&right.mechanic_id,
				right.operator_instance_id,
				right.source,
			))
	});
	activations
}

fn replay_block_audit(entry: &BlockAuditEntry) -> ReplayBlockAudit {
	let (blocker_kind, blocker_instance_id): (ReplayBlockerKind, Option<u32>) = match entry.blocker
	{
		BattleActorId::Operator(operator) => (ReplayBlockerKind::Operator, Some(operator.get())),
		BattleActorId::Enemy(enemy) => (ReplayBlockerKind::Enemy, Some(enemy.get())),
		BattleActorId::Summon(instance) => (ReplayBlockerKind::Summon, Some(instance)),
		BattleActorId::Stage => (ReplayBlockerKind::Stage, None),
	};
	ReplayBlockAudit {
		enemy_instance_id: entry.enemy.get(),
		blocker_kind,
		blocker_instance_id,
		start_tick: entry.started_at.get(),
		end_tick: entry.ended_at.map(|tick| tick.get()),
		end_reason: entry.end_reason.map(|reason| match reason {
			BlockEndReason::EnemyDeath => ReplayBlockEndReason::EnemyDeath,
			BlockEndReason::BlockerDeath => ReplayBlockEndReason::BlockerDeath,
			BlockEndReason::Retreat => ReplayBlockEndReason::Retreat,
			BlockEndReason::BlockCountChange => ReplayBlockEndReason::BlockCountChange,
			BlockEndReason::BlockTargetChange => ReplayBlockEndReason::BlockTargetChange,
			BlockEndReason::Shift => ReplayBlockEndReason::Shift,
			BlockEndReason::Goal => ReplayBlockEndReason::Goal,
			BlockEndReason::BattleDeadline => ReplayBlockEndReason::BattleDeadline,
		}),
	}
}

#[allow(clippy::too_many_arguments)]
fn base_artifact(
	input: &ExpectedReplayInput,
	guide_sha256: [u8; 32],
	guide_variant_id: &str,
	battlefield_status: ExpectedStrategyStatus,
	trace_sha256: [u8; 32],
	assessment: Option<&ExpectedValueAssessment>,
	unsupported: Vec<ArtifactUnsupportedMechanic>,
) -> StrategyArtifact {
	let build_keys: BTreeMap<OperatorInstanceId, &str> = guide_build_keys(&input.guide);
	let executed_ticks: BTreeMap<usize, u64> = assessment
		.into_iter()
		.flat_map(ExpectedValueAssessment::action_audit)
		.filter_map(|action| {
			action
				.executed_at
				.map(|tick| (action.action_index, tick.get()))
		})
		.collect();
	let actions: Vec<ArtifactAction> = input
		.guide
		.actions
		.iter()
		.enumerate()
		.map(|(index, action)| {
			artifact_action(
				index,
				action,
				&build_keys,
				executed_ticks.get(&index).copied(),
			)
		})
		.collect();
	let builds: Vec<ArtifactExactBuild> = exact_build_records(input);
	let enemies: Vec<ArtifactEnemyAssessment> = assessment
		.into_iter()
		.flat_map(|value| value.enemies().values())
		.map(|enemy| {
			let deadline: Option<u64> = enemy.deadline_at.map(|tick| tick.get());
			let resolution: Option<u64> = enemy.death_at.or(enemy.leaked_at).map(|tick| tick.get());
			ArtifactEnemyAssessment {
				enemy_instance_id: format!("{}:{}", enemy.enemy_key, enemy.id.get()),
				spawned_tick: enemy.spawned_at.map(|tick| tick.get()),
				deadline_tick: deadline,
				max_hp_micros: saturating_i64(enemy.max_hp.scaled()),
				remaining_hp_micros: saturating_i64(enemy.remaining_hp.scaled()),
				credited_damage_micros: saturating_i64(enemy.expected_damage_taken.scaled()),
				death_tick: enemy.death_at.map(|tick| tick.get()),
				leaked_tick: enemy.leaked_at.map(|tick| tick.get()),
				deadline_margin_ticks: deadline_margin(deadline, resolution).unwrap_or(0),
			}
		})
		.collect();
	let operators: Vec<ArtifactOperatorAssessment> = assessment
		.into_iter()
		.flat_map(|value| value.operators().values())
		.map(|operator| ArtifactOperatorAssessment {
			operator_id: operator.operator_key.clone(),
			max_hp_micros: saturating_i64(operator.max_hp.scaled()),
			remaining_hp_micros: saturating_i64(operator.remaining_hp.scaled()),
			deployed_tick: operator.deployed_at.map(|tick| tick.get()),
			death_tick: operator.death_at.map(|tick| tick.get()),
		})
		.collect();
	let minimum_deadline_margin_ticks: i64 = enemies
		.iter()
		.map(|enemy| enemy.deadline_margin_ticks)
		.min()
		.unwrap_or(0);
	let minimum_operator_hp_micros: i64 = operators
		.iter()
		.filter(|operator| operator.deployed_tick.is_some())
		.map(|operator| operator.remaining_hp_micros)
		.min()
		.unwrap_or(0);
	StrategyArtifact {
		schema_version: STRATEGY_ARTIFACT_SCHEMA_VERSION,
		data_revision: input.data_revision.clone(),
		mechanics_revision: input.mechanics_revision.clone(),
		guide_sha256,
		guide_variant_id: guide_variant_id.to_owned(),
		damage_status: match battlefield_status {
			ExpectedStrategyStatus::ExpectedVictory => DeterministicDamageStatus::Feasible,
			ExpectedStrategyStatus::ExpectedDefeat => DeterministicDamageStatus::Infeasible,
			ExpectedStrategyStatus::Unsupported | ExpectedStrategyStatus::Incomplete => {
				DeterministicDamageStatus::Unsupported
			}
		},
		battlefield_status,
		minimum_deadline_margin_ticks,
		minimum_operator_hp_micros,
		actions,
		builds,
		enemies,
		operators,
		unsupported,
		trace_sha256,
	}
}

fn artifact_action(
	index: usize,
	action: &MaterializedGuideAction,
	build_keys: &BTreeMap<OperatorInstanceId, &str>,
	executed_tick: Option<u64>,
) -> ArtifactAction {
	let (kind, operator, cell, facing): (
		MaterializedActionKind,
		Option<OperatorInstanceId>,
		Option<crate::data::GridPosition>,
		ArtifactFacing,
	) = match action {
		MaterializedGuideAction::Deploy {
			operator,
			cell,
			facing,
			..
		} => (
			MaterializedActionKind::Deploy,
			Some(*operator),
			Some(*cell),
			artifact_facing(*facing),
		),
		MaterializedGuideAction::ActivateSkill { operator, .. } => (
			MaterializedActionKind::Skill,
			Some(*operator),
			None,
			ArtifactFacing::None,
		),
		MaterializedGuideAction::Retreat { operator, .. } => (
			MaterializedActionKind::Retreat,
			Some(*operator),
			None,
			ArtifactFacing::None,
		),
		MaterializedGuideAction::SpeedUp { .. } => (
			MaterializedActionKind::SpeedUp,
			None,
			None,
			ArtifactFacing::None,
		),
		MaterializedGuideAction::SkillDaemon { .. } => (
			MaterializedActionKind::SkillDaemon,
			None,
			None,
			ArtifactFacing::None,
		),
	};
	ArtifactAction {
		action_index: u32::try_from(index).unwrap_or(u32::MAX),
		kind,
		operator_id: operator
			.and_then(|id| build_keys.get(&id).copied())
			.map(str::to_owned),
		row: cell.and_then(|value| i32::try_from(value.row).ok()),
		column: cell.and_then(|value| i32::try_from(value.col).ok()),
		facing,
		scheduled_tick: executed_tick.unwrap_or_else(|| action.requested_at().get()),
	}
}

fn guide_build_keys(
	guide: &crate::mechanics::domain::MaterializedGuide,
) -> BTreeMap<OperatorInstanceId, &str> {
	guide
		.builds
		.iter()
		.map(|build| (build.id, build.operator_key.as_str()))
		.collect()
}

fn exact_build_records(input: &ExpectedReplayInput) -> Vec<ArtifactExactBuild> {
	input
		.resolved_builds
		.iter()
		.map(|resolved| {
			let build = resolved.build();
			let (module_id, module_level): (Option<String>, u8) = match build.module() {
				OperatorModule::None => (None, 0),
				OperatorModule::Id(id) => (Some(id.clone()), build.module_level()),
			};
			ArtifactExactBuild {
				operator_id: resolved.operator_id().to_owned(),
				elite: build.elite(),
				level: build.level(),
				trust_percent: u16::from(build.trust_percent()),
				potential_rank: build.potential().number(),
				skill_slot: build.skill(),
				skill_level: build.skill_level(),
				module_id,
				module_level,
			}
		})
		.collect()
}

fn artifact_facing(value: crate::mechanics::domain::Facing) -> ArtifactFacing {
	match value {
		crate::mechanics::domain::Facing::Left => ArtifactFacing::Left,
		crate::mechanics::domain::Facing::Right => ArtifactFacing::Right,
		crate::mechanics::domain::Facing::Up => ArtifactFacing::Up,
		crate::mechanics::domain::Facing::Down => ArtifactFacing::Down,
	}
}

fn deadline_margin(deadline: Option<u64>, resolution: Option<u64>) -> Option<i64> {
	let deadline: i128 = i128::from(deadline?);
	let resolution: i128 = i128::from(resolution?);
	i64::try_from(deadline - resolution).ok()
}

fn saturating_i64(value: u64) -> i64 {
	i64::try_from(value).unwrap_or(i64::MAX)
}

fn action_kind_name(value: GuideExecutorActionKind) -> &'static str {
	match value {
		GuideExecutorActionKind::Deploy => "deploy",
		GuideExecutorActionKind::ActivateSkill => "activate_skill",
		GuideExecutorActionKind::Retreat => "retreat",
		GuideExecutorActionKind::SpeedUp => "speed_up",
		GuideExecutorActionKind::SkillDaemon => "skill_daemon",
	}
}

fn materialized_action_kind_name(value: &MaterializedGuideAction) -> &'static str {
	match value {
		MaterializedGuideAction::Deploy { .. } => "deploy",
		MaterializedGuideAction::ActivateSkill { .. } => "activate_skill",
		MaterializedGuideAction::Retreat { .. } => "retreat",
		MaterializedGuideAction::SpeedUp { .. } => "speed_up",
		MaterializedGuideAction::SkillDaemon { .. } => "skill_daemon",
	}
}

fn unsupported_action_audits(
	guide: &crate::mechanics::domain::MaterializedGuide,
) -> Vec<ReplayActionAudit> {
	let build_keys: BTreeMap<OperatorInstanceId, &str> = guide_build_keys(guide);
	guide
		.actions
		.iter()
		.enumerate()
		.map(|(action_index, action)| {
			let identity: ArtifactAction = artifact_action(action_index, action, &build_keys, None);
			ReplayActionAudit {
				action_index,
				kind: materialized_action_kind_name(action).to_owned(),
				operator_id: identity.operator_id,
				row: identity.row,
				column: identity.column,
				facing: identity.facing,
				requested_tick: action.requested_at().get(),
				executed_tick: None,
				status: "not_executed_unsupported".to_owned(),
			}
		})
		.collect()
}

fn action_status_name(value: ActionAuditStatus) -> &'static str {
	match value {
		ActionAuditStatus::Executed => "executed",
		ActionAuditStatus::ExecutedAfterDpWait => "executed_after_dp_wait",
		ActionAuditStatus::WaitedForDp => "waited_for_dp",
		ActionAuditStatus::SkippedNotReady => "skipped_not_ready",
		ActionAuditStatus::ExecutorOnly => "executor_only",
	}
}

fn hex_digest(digest: [u8; 32]) -> String {
	digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
	use crate::simulation::SimTick;

	use super::{
		classify_replay_status, unsupported_action_audits, ArtifactFacing, DeclaredDepartures,
		ExpectedReplayReportStatus, ExpectedStrategyStatus, MaterializedGuideAction,
		ReplayConservativeOmissionEntry, ReplayUnmodeledOmissionEntry,
	};
	use crate::{mechanics::domain::MaterializedGuide, replay::ExpectedTerminalStatus};

	type Classified = (ExpectedReplayReportStatus, ExpectedStrategyStatus);

	const DEFEAT: Classified = (
		ExpectedReplayReportStatus::ExpectedDefeat,
		ExpectedStrategyStatus::ExpectedDefeat,
	);
	const VICTORY: Classified = (
		ExpectedReplayReportStatus::ExpectedVictory,
		ExpectedStrategyStatus::ExpectedVictory,
	);
	const INCOMPLETE: Classified = (
		ExpectedReplayReportStatus::Incomplete,
		ExpectedStrategyStatus::Incomplete,
	);

	fn goal_leak() -> ExpectedTerminalStatus {
		ExpectedTerminalStatus::GoalLeak {
			enemies: Vec::new(),
		}
	}

	#[test]
	fn simulation_issues_override_an_observed_goal_leak() {
		assert_eq!(
			classify_replay_status(&goal_leak(), true, false, DeclaredDepartures::None),
			INCOMPLETE
		);
	}

	#[test]
	fn a_goal_leak_certifies_defeat_only_when_nothing_is_omitted() {
		assert_eq!(
			classify_replay_status(&goal_leak(), false, false, DeclaredDepartures::None),
			DEFEAT
		);
		// An omitted benefit might have stopped the leak, and an unmodeled effect could move the
		// battle either way.
		assert_eq!(
			classify_replay_status(
				&goal_leak(),
				false,
				false,
				DeclaredDepartures::BeneficialOnly
			),
			INCOMPLETE
		);
		assert_eq!(
			classify_replay_status(&goal_leak(), false, false, DeclaredDepartures::Unmodeled),
			INCOMPLETE
		);
	}

	#[test]
	fn a_clear_certifies_victory_despite_beneficial_omissions_only() {
		let victory = ExpectedTerminalStatus::Victory;
		assert_eq!(
			classify_replay_status(&victory, false, false, DeclaredDepartures::None),
			VICTORY
		);
		assert_eq!(
			classify_replay_status(&victory, false, false, DeclaredDepartures::BeneficialOnly),
			VICTORY
		);
		assert_eq!(
			classify_replay_status(&victory, false, false, DeclaredDepartures::Unmodeled),
			INCOMPLETE
		);
		assert_eq!(
			classify_replay_status(&victory, false, true, DeclaredDepartures::None),
			INCOMPLETE
		);
	}

	#[test]
	fn a_pending_required_action_does_not_withhold_a_modelled_leak() {
		// Waiting for DP is modelled rather than omitted, so a leak while an action still waits is a
		// modelled defeat.
		assert_eq!(
			classify_replay_status(&goal_leak(), false, true, DeclaredDepartures::None),
			DEFEAT
		);
	}

	#[test]
	fn an_unfinished_battle_is_incomplete_whatever_is_omitted() {
		let unfinished: [ExpectedTerminalStatus; 2] = [
			ExpectedTerminalStatus::Deadline {
				alive_enemies: Vec::new(),
			},
			ExpectedTerminalStatus::SimulationIncomplete,
		];
		for terminal in &unfinished {
			for omitted in [
				DeclaredDepartures::None,
				DeclaredDepartures::BeneficialOnly,
				DeclaredDepartures::Unmodeled,
			] {
				assert_eq!(
					classify_replay_status(terminal, false, false, omitted),
					INCOMPLETE,
					"{terminal:?} with {omitted:?}"
				);
			}
		}
	}

	#[test]
	fn an_unmodeled_omission_outranks_beneficial_ones() {
		let conservative: [ReplayConservativeOmissionEntry; 1] =
			[ReplayConservativeOmissionEntry {
				mechanic_id: "friendly_buff".to_owned(),
				origins: vec!["operator:test".to_owned()],
				reasons: vec!["beneficial".to_owned()],
			}];
		let unmodeled: [ReplayUnmodeledOmissionEntry; 1] = [ReplayUnmodeledOmissionEntry {
			mechanic_id: "unreviewed_talent".to_owned(),
			origins: vec!["operator:test".to_owned()],
			reasons: vec!["unreviewed".to_owned()],
		}];

		assert_eq!(DeclaredDepartures::of(&[], &[]), DeclaredDepartures::None);
		assert_eq!(
			DeclaredDepartures::of(&conservative, &[]),
			DeclaredDepartures::BeneficialOnly
		);
		assert_eq!(
			DeclaredDepartures::of(&[], &unmodeled),
			DeclaredDepartures::Unmodeled
		);
		assert_eq!(
			DeclaredDepartures::of(&conservative, &unmodeled),
			DeclaredDepartures::Unmodeled
		);
	}

	#[test]
	fn a_declared_approximation_discloses_without_withholding_a_verdict() {
		// An approximation is not a hole in the model: replay executes the effect, at the value the
		// model declares, the way it uses expected damage and whole-cell positions. Those
		// simplifications are what an Expected* verdict is a verdict about, so ranking any of them
		// as a veto would leave no battle certifiable at all. It is disclosed beside the verdict
		// instead, and only a bound showing it exceeds a margin the verdict depends on is grounds
		// to withhold.
		assert_eq!(
			classify_replay_status(
				&ExpectedTerminalStatus::Victory,
				false,
				false,
				DeclaredDepartures::None
			),
			VICTORY
		);
		assert_eq!(
			classify_replay_status(&goal_leak(), false, false, DeclaredDepartures::None),
			DEFEAT
		);
	}

	#[test]
	fn unsupported_preflight_retains_every_requested_action() {
		let guide = MaterializedGuide {
			builds: Vec::new(),
			actions: vec![
				MaterializedGuideAction::SpeedUp {
					requested_at: SimTick::new(3),
				},
				MaterializedGuideAction::SkillDaemon {
					requested_at: SimTick::new(5),
				},
			],
			reached_mechanics: Vec::new(),
		};

		let audits = unsupported_action_audits(&guide);

		assert_eq!(audits.len(), 2);
		assert_eq!(audits[0].action_index, 0);
		assert_eq!(audits[0].kind, "speed_up");
		assert_eq!(audits[0].operator_id, None);
		assert_eq!(audits[0].row, None);
		assert_eq!(audits[0].column, None);
		assert_eq!(audits[0].facing, ArtifactFacing::None);
		assert_eq!(audits[0].requested_tick, 3);
		assert_eq!(audits[0].executed_tick, None);
		assert_eq!(audits[0].status, "not_executed_unsupported");
		assert_eq!(audits[1].action_index, 1);
		assert_eq!(audits[1].kind, "skill_daemon");
	}
}
