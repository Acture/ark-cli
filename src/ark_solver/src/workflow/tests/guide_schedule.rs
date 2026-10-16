use std::{collections::BTreeMap, num::NonZeroU32, path::PathBuf};

use crate::data::{ArkDataSnapshot, CompiledStage, StageDifficulty, StageIdentity, StageRef};

use super::guide_deployment_domain_from_maa_job;
use crate::maa_job::{
	parse_maa_job, MaaJob, MaaJobDifficulty, MaaJobGuideSeed, MaaJobSourceLineage,
};
use crate::planning::{
	prepare_guide_deployment_schedule_with_cost_overrides, CandidatePoolSpec, CatalogPreset,
	GuideDeploymentActionOverrides, GuideDeploymentDomain, PreparedGuideDeploymentSchedule,
	ResolvedCoverageCandidate,
};

const GUIDE: &str = include_str!("../../../tests/fixtures/maa_job_bd7_sanitized.json");

#[test]
#[ignore = "requires configured gamedata snapshot"]
fn bd7_unguarded_deploys_receive_ticks_from_action_order_and_stage_dp() {
	let snapshot_path: PathBuf = crate::test_support::configured_gamedata_dir();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(&snapshot_path).unwrap();
	let guide: MaaJob = parse_maa_job(GUIDE).unwrap();
	let digest: String = "ab".repeat(32);
	let lineage: MaaJobSourceLineage = MaaJobSourceLineage::new("fixture:bd7", &digest).unwrap();
	let seed: MaaJobGuideSeed<'_> = guide
		.executable_guide_seeds(lineage, MaaJobDifficulty::Normal, 10)
		.unwrap()
		.next()
		.unwrap();
	let stage: CompiledStage = snapshot
		.compile_stage(StageRef {
			identity: StageIdentity::StageId(seed.stage_name().to_owned()),
			difficulty: StageDifficulty::Normal,
		})
		.unwrap();
	let domain: GuideDeploymentDomain = guide_deployment_domain_from_maa_job(
		&seed,
		1,
		stage.map_dimensions(),
		CandidatePoolSpec::CatalogPreset {
			preset: CatalogPreset::HighestPhaseBasePanel,
		},
	)
	.unwrap();
	let ticks_per_second: NonZeroU32 = NonZeroU32::new(30).unwrap();
	let no_overrides = GuideDeploymentActionOverrides::default();

	let schedule: PreparedGuideDeploymentSchedule =
		prepare_guide_deployment_schedule_with_cost_overrides(
			&snapshot,
			ticks_per_second,
			&domain,
			&no_overrides,
		)
		.unwrap();

	assert_eq!(schedule.deployments.len(), 2);
	assert_eq!(schedule.deployments[0].action.operator_name, "黍");
	assert_eq!(schedule.deployments[0].resolved_build.deployment_cost(), 22);
	assert_eq!(schedule.deployments[0].deploy_tick(), 360);
	assert!(!schedule.deployments[0].action.is_source_slot);
	assert_eq!(schedule.deployments[1].action.operator_name, "圣聆初雪");
	assert_eq!(schedule.deployments[1].resolved_build.deployment_cost(), 24);
	assert_eq!(schedule.deployments[1].deploy_tick(), 1_080);
	assert!(schedule.deployments[1].action.is_source_slot);
	assert!(schedule.deployments[1]
		.retained_coverage_candidate
		.is_none());

	let exact_costs = GuideDeploymentActionOverrides {
		deployment_cost_by_action: BTreeMap::from([(0, 20), (1, 10)]),
		..GuideDeploymentActionOverrides::default()
	};
	let exact_cost_schedule: PreparedGuideDeploymentSchedule =
		prepare_guide_deployment_schedule_with_cost_overrides(
			&snapshot,
			ticks_per_second,
			&domain,
			&exact_costs,
		)
		.unwrap();
	let retained: &ResolvedCoverageCandidate = exact_cost_schedule.deployments[0]
		.retained_coverage_candidate
		.as_ref()
		.unwrap();
	assert_eq!(
		exact_cost_schedule.deployments[0]
			.resolved_build
			.deployment_cost(),
		22
	);
	assert_eq!(
		exact_cost_schedule.deployments[0].dp_schedule.action.cost,
		20
	);
	assert_eq!(retained.deployment_cost(), 20);

	let cheaper_replacement_costs = GuideDeploymentActionOverrides {
		deployment_cost_by_action: BTreeMap::from([(domain.source_action_index, 10)]),
		..GuideDeploymentActionOverrides::default()
	};
	let cheaper_replacement: PreparedGuideDeploymentSchedule =
		prepare_guide_deployment_schedule_with_cost_overrides(
			&snapshot,
			ticks_per_second,
			&domain,
			&cheaper_replacement_costs,
		)
		.unwrap();
	assert_eq!(cheaper_replacement.deployments[0].deploy_tick(), 360);
	assert_eq!(cheaper_replacement.deployments[1].deploy_tick(), 660);
}
