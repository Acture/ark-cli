use std::{
	collections::{BTreeMap, BTreeSet},
	ops::ControlFlow,
};

use ark_solver::data::GridPosition;
use ark_solver::mechanics::{
	domain::FixedDamage,
	stage::{SpawnCohortId, SpawnInstanceId},
};
use ark_solver::planning::{
	solve_expected_damage_level, visit_expected_damage_level, ContributionCandidateKey,
	ContributionConstraintKey, ContributionDemand, ContributionOperatorIdentity,
	ExpectedDamageJointEvaluationFailure, ExpectedDamageJointEvaluator,
	ExpectedDamageJointEvidence, ExpectedDamageLevelSolveRequest, ExpectedDamageLevelSolveStatus,
	ExpectedDamageProposalDisposition, ExpectedDamageProposalRejection,
	IndependentContributionCandidate,
};

struct ExactSetJointEvaluator {
	accepted_sets: BTreeSet<Vec<String>>,
}

impl ExpectedDamageJointEvaluator for ExactSetJointEvaluator {
	fn evaluate(
		&self,
		selected_candidate_keys: &[ContributionCandidateKey],
	) -> Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure> {
		let selected: Vec<String> = selected_candidate_keys
			.iter()
			.map(|key| key.as_str().to_owned())
			.collect();
		let feasible: bool = self.accepted_sets.contains(&selected);
		Ok(ExpectedDamageJointEvidence {
			deterministic_expected_damage_feasible: feasible,
			minimum_cumulative_coverage_millionths: if feasible { 1_000_000 } else { 900_000 },
			total_due_remaining_hp_scaled: if feasible { 0 } else { 1_000_000 },
			clearance_slack_ticks: feasible.then_some(5),
			members: Vec::new(),
			targets: Vec::new(),
			cumulative_deadlines: Vec::new(),
		})
	}
}

struct FailingJointEvaluator;

impl ExpectedDamageJointEvaluator for FailingJointEvaluator {
	fn evaluate(
		&self,
		_selected_candidate_keys: &[ContributionCandidateKey],
	) -> Result<ExpectedDamageJointEvidence, ExpectedDamageJointEvaluationFailure> {
		Err(ExpectedDamageJointEvaluationFailure::new(
			"planning_timing_unknown",
			"later-wave start depends on predecessor clear",
		))
	}
}

#[test]
fn prefers_one_operator_over_a_cheaper_two_operator_plan() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> = vec![
		candidate("solo", "solo", 30, 0, &target, 10),
		candidate("pair_a", "pair_a", 1, 1, &target, 6),
		candidate("pair_b", "pair_b", 1, 2, &target, 4),
	];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::from([vec!["solo".to_owned()]]),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 3,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("one-operator proposal should solve");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached
	);
	assert_eq!(report.damage_feasible_proposal_indices, vec![0]);
	assert_eq!(report.replay_candidate_indices, vec![0]);
	let accepted = report
		.damage_feasible_proposals()
		.next()
		.expect("damage-feasible proposal");
	assert_eq!(accepted.additive.operator_count, 1);
	assert_eq!(
		accepted.additive.selected_candidates[0].candidate_key,
		"solo"
	);
	assert_eq!(
		accepted.disposition,
		ExpectedDamageProposalDisposition::DamageFeasibleCandidate
	);
}

#[test]
fn rejects_additive_false_positives_until_joint_validation_passes() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> = vec![
		candidate("bad_a", "operator_a", 1, 0, &target, 6),
		candidate("bad_b", "operator_b", 1, 1, &target, 4),
		candidate("good_a", "operator_a", 2, 2, &target, 5),
		candidate("good_b", "operator_b", 2, 3, &target, 5),
	];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::from([vec!["good_a".to_owned(), "good_b".to_owned()]]),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 8,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("jointly feasible configuration should be found");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::AdditiveSearchExhausted
	);
	assert!(report.proposals.len() > 1);
	assert!(matches!(
		report.proposals[0].disposition,
		ExpectedDamageProposalDisposition::Rejected {
			reason: ExpectedDamageProposalRejection::JointExpectedDamageInfeasible { .. }
		}
	));
	let accepted = report
		.damage_feasible_proposals()
		.next()
		.expect("damage-feasible proposal");
	let selected: Vec<&str> = accepted
		.additive
		.selected_candidates
		.iter()
		.map(|candidate| candidate.candidate_key.as_str())
		.collect();
	assert_eq!(selected, vec!["good_a", "good_b"]);
}

#[test]
fn retains_later_damage_feasible_candidates_for_battlefield_replay() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> = vec![
		candidate("first", "first", 1, 0, &target, 10),
		candidate("second", "second", 2, 1, &target, 10),
	];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::from([vec!["first".to_owned()], vec!["second".to_owned()]]),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 2,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("both candidates pass the damage gate");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached
	);
	assert_eq!(report.damage_feasible_proposal_indices, vec![0, 1]);
	assert_eq!(report.replay_candidate_indices, vec![0, 1]);
	assert_eq!(report.damage_feasible_proposals().count(), 2);
}

#[test]
fn proposal_visitor_can_cancel_after_observing_the_owned_first_audit() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> = vec![
		candidate("first", "first", 1, 0, &target, 10),
		candidate("second", "second", 2, 1, &target, 10),
	];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::from([vec!["first".to_owned()], vec!["second".to_owned()]]),
	};
	let mut observed_keys: Vec<String> = Vec::new();

	let report = visit_expected_damage_level(
		ExpectedDamageLevelSolveRequest {
			demands: &demands,
			candidates: &candidates,
			candidate_materialization_complete: true,
			planning_model_complete: true,
			candidate_exclusions: &[],
			additive_proposal_limit: 2,
			explore_cardinalities: false,
			joint_evaluator: &evaluator,
		},
		|proposal_index, proposal| {
			assert_eq!(proposal_index, 0);
			assert_eq!(
				proposal.disposition,
				ExpectedDamageProposalDisposition::DamageFeasibleCandidate
			);
			observed_keys.push(
				proposal.additive.selected_candidates[0]
					.candidate_key
					.clone(),
			);
			ControlFlow::Break(())
		},
	)
	.expect("the first fully audited proposal should be streamed");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::ProposalVisitorStopped
	);
	assert_eq!(observed_keys, vec!["first"]);
	assert_eq!(report.proposals.len(), 1);
	assert_eq!(report.damage_feasible_proposal_indices, vec![0]);
	assert_eq!(report.replay_candidate_indices, vec![0]);
}

#[test]
fn reports_additive_infeasibility_without_failing_the_cli_layer() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> =
		vec![candidate("weak", "weak", 1, 0, &target, 9)];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::new(),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 2,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("infeasibility is a normal solve outcome");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::AdditiveInfeasible
	);
	assert!(report.proposals.is_empty());
	assert!(report.damage_feasible_proposal_indices.is_empty());
	assert!(report.replay_candidate_indices.is_empty());
	assert_eq!(
		report.residual_demands[0].required_damage_scaled,
		10_000_000
	);
}

#[test]
fn incomplete_candidate_materialization_does_not_claim_additive_infeasibility() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> =
		vec![candidate("weak", "weak", 1, 0, &target, 9)];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::new(),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: false,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 2,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("an incomplete candidate set is a normal solve outcome");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::CandidateMaterializationIncomplete
	);
	assert!(report.proposals.is_empty());
	assert!(report.damage_feasible_proposal_indices.is_empty());
	assert!(report.replay_candidate_indices.is_empty());
}

#[test]
fn incomplete_planning_model_preserves_candidates_below_baseline_residual_gap() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> =
		vec![candidate("weak", "weak", 1, 0, &target, 9)];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::new(),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: false,
		candidate_exclusions: &[],
		additive_proposal_limit: 2,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("an incomplete planning model is a normal solve outcome");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::PlanningModelIncomplete
	);
	assert_eq!(report.proposals.len(), 1);
	assert_eq!(report.replay_candidate_indices, vec![0]);
	assert!(!report.proposals[0].additive.demands_enforced);
	assert_eq!(
		report.proposals[0].additive.constraints[0].remaining_demand_scaled,
		1_000_000
	);
	assert!(matches!(
		report.proposals[0].disposition,
		ExpectedDamageProposalDisposition::PlanningReplayCandidate { .. }
	));
}

#[test]
fn incomplete_unblocked_score_cannot_prune_a_full_replay_candidate() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> = vec![candidate(
		"blocked_route",
		"blocked_route",
		1,
		0,
		&target,
		10,
	)];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::new(),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: false,
		candidate_exclusions: &[],
		additive_proposal_limit: 1,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("incomplete planning must preserve replay candidates");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::AdditiveProposalLimitReached
	);
	assert!(report.damage_feasible_proposal_indices.is_empty());
	assert_eq!(report.replay_candidate_indices, vec![0]);
	assert_eq!(report.replay_candidates().count(), 1);
	assert!(matches!(
		report.proposals[0].disposition,
		ExpectedDamageProposalDisposition::PlanningReplayCandidate {
			incomplete_planning_result:
				ExpectedDamageProposalRejection::JointExpectedDamageInfeasible { .. }
		}
	));
}

#[test]
fn incomplete_joint_evaluation_failure_cannot_prune_a_replay_candidate() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> = vec![candidate(
		"causal_timing",
		"causal_timing",
		1,
		0,
		&target,
		10,
	)];

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: false,
		candidate_exclusions: &[],
		additive_proposal_limit: 1,
		explore_cardinalities: false,
		joint_evaluator: &FailingJointEvaluator,
	})
	.expect("incomplete joint evaluation must preserve replay candidates");

	assert!(report.damage_feasible_proposal_indices.is_empty());
	assert_eq!(report.replay_candidate_indices, vec![0]);
	assert!(matches!(
		report.proposals[0].disposition,
		ExpectedDamageProposalDisposition::PlanningReplayCandidate {
			incomplete_planning_result: ExpectedDamageProposalRejection::JointEvaluationFailed { .. }
		}
	));
}

#[test]
fn complete_joint_evaluation_failure_is_rejected() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> =
		vec![candidate("invalid", "invalid", 1, 0, &target, 10)];

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: true,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 1,
		explore_cardinalities: false,
		joint_evaluator: &FailingJointEvaluator,
	})
	.expect("complete planning evaluation failure is an audited rejection");

	assert!(report.replay_candidate_indices.is_empty());
	assert!(matches!(
		report.proposals[0].disposition,
		ExpectedDamageProposalDisposition::Rejected {
			reason: ExpectedDamageProposalRejection::JointEvaluationFailed { .. }
		}
	));
}

#[test]
fn incomplete_candidate_materialization_does_not_claim_search_exhaustion() {
	let target: SpawnInstanceId = target("enemy");
	let demands: Vec<ContributionDemand> = vec![demand(&target, 10)];
	let candidates: Vec<IndependentContributionCandidate> =
		vec![candidate("locally_bad", "locally_bad", 1, 0, &target, 10)];
	let evaluator = ExactSetJointEvaluator {
		accepted_sets: BTreeSet::new(),
	};

	let report = solve_expected_damage_level(ExpectedDamageLevelSolveRequest {
		demands: &demands,
		candidates: &candidates,
		candidate_materialization_complete: false,
		planning_model_complete: true,
		candidate_exclusions: &[],
		additive_proposal_limit: 2,
		explore_cardinalities: false,
		joint_evaluator: &evaluator,
	})
	.expect("joint rejection over an incomplete candidate set is not exhaustive");

	assert_eq!(
		report.status,
		ExpectedDamageLevelSolveStatus::CandidateMaterializationIncomplete
	);
	assert_eq!(report.proposals.len(), 1);
	assert!(matches!(
		report.proposals[0].disposition,
		ExpectedDamageProposalDisposition::Rejected { .. }
	));
	assert!(report.replay_candidate_indices.is_empty());
}

fn target(cohort: &str) -> SpawnInstanceId {
	SpawnInstanceId {
		cohort: SpawnCohortId::new(cohort.to_owned()).expect("valid cohort"),
		ordinal: 0,
	}
}

fn constraint(target: &SpawnInstanceId) -> ContributionConstraintKey {
	ContributionConstraintKey::space_time_hp_group(format!(
		"{}-{}",
		target.cohort.as_str(),
		target.ordinal
	))
	.expect("constraint key")
}

fn demand(target: &SpawnInstanceId, required_damage: u64) -> ContributionDemand {
	ContributionDemand {
		constraint: constraint(target),
		required: FixedDamage::from_whole(required_damage).expect("fixed damage"),
	}
}

fn candidate(
	key: &str,
	operator_identity: &str,
	deployment_cost: u32,
	col: u32,
	target: &SpawnInstanceId,
	damage: u64,
) -> IndependentContributionCandidate {
	IndependentContributionCandidate {
		key: ContributionCandidateKey::new(key).expect("candidate key"),
		operator_identity: ContributionOperatorIdentity::new(operator_identity)
			.expect("operator identity"),
		operator_name: operator_identity.to_owned(),
		deployment_cost,
		deployment_cell: GridPosition { row: 0, col },
		optimistic_damage_by_constraint: BTreeMap::from([(
			constraint(target),
			FixedDamage::from_whole(damage).expect("fixed damage"),
		)]),
		ranking_score: damage,
	}
}
