use std::collections::BTreeMap;

use ark_solver::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSourceLineage};
use sha2::{Digest, Sha256};

mod support;

const GUIDE: &str = include_str!("fixtures/maa_job_bd7_sanitized.json");
const EXACT_GUIDE: &[u8] = include_bytes!("fixtures/maa_job_bd7_exact_sanitized.json");

#[test]
fn tracked_bd7_fixture_expands_each_difficulty_lazily() {
	let guide = parse_maa_job(GUIDE).expect("sanitized BD-7 guide should import");
	let digest: String = "ab".repeat(32);
	let lineage =
		MaaJobSourceLineage::new("fixture:bd7", &digest).expect("fixture lineage should be valid");
	let normal = guide
		.executable_guide_seeds(lineage, MaaJobDifficulty::Normal, 10)
		.expect("normal difficulty should be available")
		.collect::<Vec<_>>();
	let challenge = guide
		.executable_guide_seeds(lineage, MaaJobDifficulty::Challenge, 10)
		.expect("challenge difficulty should be available")
		.collect::<Vec<_>>();

	assert_eq!(normal.len(), 2);
	assert_eq!(challenge.len(), 2);
	assert_eq!(normal[0].stage_name(), "act21mini_07");
	assert_eq!(normal[0].build_hints().count(), 2);
	assert_eq!(normal[0].deployment_hints().count(), 2);
	assert_eq!(normal[0].difficulty(), MaaJobDifficulty::Normal);
	assert_eq!(challenge[0].difficulty(), MaaJobDifficulty::Challenge);
}

#[test]
fn exact_bd7_fixture_has_one_selection_and_one_fixed_source_deployment() {
	let guide = parse_maa_job(
		std::str::from_utf8(EXACT_GUIDE).expect("tracked exact guide should be UTF-8"),
	)
	.expect("sanitized exact BD-7 guide should import");
	let digest: String = Sha256::digest(EXACT_GUIDE)
		.iter()
		.map(|byte: &u8| format!("{byte:02x}"))
		.collect();
	let lineage = MaaJobSourceLineage::new("fixture:bd7-exact", &digest)
		.expect("fixture lineage should be valid");
	let mut seeds = guide
		.executable_guide_seeds(lineage, MaaJobDifficulty::Normal, 10)
		.expect("normal difficulty should be available");
	let seed = seeds
		.next()
		.expect("the exact fixture should materialize one selection");
	assert!(seeds.next().is_none());
	assert!(!seeds.report().truncated());
	assert_eq!(seed.source_id(), "fixture:bd7-exact");
	assert_eq!(seed.content_sha256(), digest);
	assert_eq!(seed.difficulty(), MaaJobDifficulty::Normal);
	let selected_slots: BTreeMap<String, String> = seed
		.build_hints()
		.map(|build| {
			(
				build.slot_name().to_owned(),
				build.operator_name().to_owned(),
			)
		})
		.collect();
	assert_eq!(
		selected_slots,
		BTreeMap::from([
			("奶盾".to_owned(), "黍".to_owned()),
			("圣聆初雪".to_owned(), "圣聆初雪".to_owned()),
		])
	);

	let deployments: Vec<(usize, Option<&str>, &str)> = seed
		.deployment_hints()
		.map(|deployment| {
			(
				deployment.action_index(),
				deployment.build().group_name(),
				deployment.build().operator_name(),
			)
		})
		.collect();
	assert_eq!(
		deployments,
		vec![(0, Some("奶盾"), "黍"), (1, None, "圣聆初雪")]
	);
}
