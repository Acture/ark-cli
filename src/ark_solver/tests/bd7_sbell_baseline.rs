use ark_solver::data::{
	ArkDataSnapshot, OperatorBuild, OperatorIdentity, OperatorModule, PotentialRank,
	ResolvedOperatorBuild,
};
use ark_solver::mechanics::{
	compile::try_decode_phalanx_trait,
	domain::{ExpectedValue, PhalanxNormalAttackRule},
};

mod support;
use support::configured_gamedata_dir;

const SBELL_OPERATOR_ID: &str = "char_1046_sbell2";

fn exact_no_module_build(name: &str, skill: u8) -> OperatorBuild {
	OperatorBuild::new(
		OperatorIdentity::Name(name.to_owned()),
		2,
		90,
		200,
		skill,
		10,
		PotentialRank::P6,
		OperatorModule::None,
		0,
	)
}
#[test]
#[ignore = "requires configured gamedata snapshot"]
fn gamedata_phalanx_decoder_retains_the_selected_trait_semantics() {
	let snapshot_path = configured_gamedata_dir();
	let snapshot: ArkDataSnapshot = ArkDataSnapshot::load(&snapshot_path).unwrap();
	let resolved: ResolvedOperatorBuild = snapshot
		.operators()
		.resolve(&exact_no_module_build("圣聆初雪", 2))
		.expect("canonical no-module 圣聆初雪 build should resolve");

	assert_eq!(resolved.operator_id(), SBELL_OPERATOR_ID);
	assert_eq!(
		resolved.stable_id(),
		"67472377b1ac84a45d38b8277b79e3a298985675:char_1046_sbell2:e2:l90:t200:p6:s2:sl10:m-none:ml0"
	);
	let decoded = try_decode_phalanx_trait(&resolved)
		.expect("pinned phalanx trait must keep its reviewed shape")
		.expect("圣聆初雪 must use the phalanx archetype");
	assert_eq!(
		decoded.semantics.outside_skill.normal_attack,
		PhalanxNormalAttackRule::SuppressedByArchetype
	);
	assert_eq!(
		decoded.semantics.outside_skill.defense_multiplier,
		ExpectedValue::from_whole(3)
	);
	assert_eq!(
		decoded
			.semantics
			.outside_skill
			.arts_resistance_additive_ratio,
		ExpectedValue::from_ratio(1, 5).expect("valid ratio")
	);
}
