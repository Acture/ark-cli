use std::path::PathBuf;

use ark_solver::config::Config;
use ark_solver::data::{
	ArkDataSnapshot, OperatorBuild, OperatorIdentity, OperatorModule, PotentialRank,
};
use ark_solver::maa_job::{parse_maa_job, MaaJobDifficulty};
use ark_solver::mechanics::domain::{Facing, MaterializedGuideAction, OperatorInstanceId};
use ark_solver::replay::ExactReplayBuildBinding;
use ark_solver::workflow::{
	enumerate_replay_guide_build_variants, prepare_guide_actions, ExpectedReplayPrepareError,
};

#[test]
#[ignore = "requires configured gamedata snapshot"]
fn standalone_replay_variants_include_the_reviewed_no_module_build() {
	let snapshot_path: PathBuf = Config::load()
		.expect("load workspace rogue.toml for local-data test")
		.gamedata_dir;
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(&snapshot_path).unwrap();
	let guide = parse_maa_job(include_str!("fixtures/maa_job_bd7_sanitized.json")).unwrap();
	let seed = guide
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.unwrap()
		.next()
		.unwrap()
		.materialize();

	let variants = enumerate_replay_guide_build_variants(&snapshot, &seed).unwrap();

	assert!(variants.iter().any(|variant| {
		variant
			.bindings
			.iter()
			.all(|binding| matches!(binding.build.module(), OperatorModule::None))
	}));
}

fn binding(name: &str, skill: u8) -> ExactReplayBuildBinding {
	ExactReplayBuildBinding {
		source_name: name.to_owned(),
		build: OperatorBuild::new(
			OperatorIdentity::Name(name.to_owned()),
			2,
			90,
			200,
			skill,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		),
		skill_usage: 0,
		skill_times: None,
	}
}

#[test]
fn materializes_all_bd7_actions_at_zero_and_converts_top_origin_cells() {
	let guide = parse_maa_job(include_str!("fixtures/maa_job_bd7_sanitized.json")).unwrap();
	let seed = guide
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.unwrap()
		.next()
		.unwrap()
		.materialize();
	let bindings = vec![binding("黍", 1), binding("圣聆初雪", 2)];

	let prepared = prepare_guide_actions(&seed, &bindings, 8).unwrap();

	assert_eq!(prepared.actions.len(), 4);
	assert_eq!(
		prepared.actions[0],
		MaterializedGuideAction::Deploy {
			requested_at: ark_solver::simulation::SimTick::new(0),
			operator: OperatorInstanceId::new(0),
			cell: ark_solver::data::GridPosition { row: 4, col: 3 },
			facing: Facing::Right,
		}
	);
	assert_eq!(
		prepared.actions[1],
		MaterializedGuideAction::Deploy {
			requested_at: ark_solver::simulation::SimTick::new(0),
			operator: OperatorInstanceId::new(1),
			cell: ark_solver::data::GridPosition { row: 5, col: 3 },
			facing: Facing::Right,
		}
	);
	assert!(matches!(
		prepared.actions[2],
		MaterializedGuideAction::SpeedUp { .. }
	));
	assert!(matches!(
		prepared.actions[3],
		MaterializedGuideAction::SkillDaemon { .. }
	));
	assert_eq!(
		prepared.policy.skill_usage(OperatorInstanceId::new(0)),
		Some(0)
	);
	assert_eq!(
		prepared.policy.skill_usage(OperatorInstanceId::new(1)),
		Some(0)
	);
}

#[test]
fn retains_supported_daemon_skill_usage() {
	let guide = parse_maa_job(include_str!("fixtures/maa_job_bd7_sanitized.json")).unwrap();
	let seed = guide
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.unwrap()
		.next()
		.unwrap()
		.materialize();
	let mut daemon_binding: ExactReplayBuildBinding = binding("黍", 1);
	daemon_binding.skill_usage = 1;
	let bindings = vec![daemon_binding, binding("圣聆初雪", 2)];

	let prepared = prepare_guide_actions(&seed, &bindings, 8).unwrap();

	assert_eq!(
		prepared.policy.skill_usage(OperatorInstanceId::new(0)),
		Some(1)
	);
	assert_eq!(
		prepared.policy.skill_usage(OperatorInstanceId::new(1)),
		Some(0)
	);
}

#[test]
fn rejects_unrepresentable_skill_usage_instead_of_silently_enabling_daemon() {
	let guide = parse_maa_job(include_str!("fixtures/maa_job_bd7_sanitized.json")).unwrap();
	let seed = guide
		.executable_seed_selections(MaaJobDifficulty::Normal, 1)
		.unwrap()
		.next()
		.unwrap()
		.materialize();
	let mut unsupported: ExactReplayBuildBinding = binding("黍", 1);
	unsupported.skill_usage = 2;
	unsupported.skill_times = Some(3);

	let error = prepare_guide_actions(&seed, &[unsupported], 8).unwrap_err();

	match &error {
		ExpectedReplayPrepareError::UnsupportedSkillUsage {
			operator_identity,
			source_name,
			skill_usage,
			skill_times,
		} => {
			assert_eq!(operator_identity, "黍");
			assert_eq!(source_name, "黍");
			assert_eq!(*skill_usage, 2);
			assert_eq!(*skill_times, Some(3));
		}
		other => panic!("expected structured skill usage error, got {other:?}"),
	}
	assert_eq!(
		error.to_string(),
		"guide operator \"黍\" (\"黍\") uses unsupported skill_usage 2 with skill_times Some(3); \
		 only 0 and 1 are supported"
	);
}
