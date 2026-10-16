//! Optional exact bindings from opaque operator prefab data to reusable replay mechanics.
//!
//! This is not an operator admission catalog. Unbound operators still use the generic base-attack,
//! skill, and effect-candidate paths. A binding only enables mechanics whose source semantics have
//! been reviewed; binding drift falls back to a partial replay instead of rejecting the operator.

use std::{
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::{
	BlackboardEntry, OperatorBuild, OperatorEffectCandidate, OperatorModule, OperatorPosition,
	PotentialRank, ResolvedOperatorBuild, SkillActivationType, SkillDurationType, SkillSpType,
};

pub const EXUSIAI_OPERATOR_ID: &str = "char_103_angel";
pub const CUORA_OPERATOR_ID: &str = "char_150_snakek";

const EXUSIAI_SKILL_ID: &str = "skchr_angel_2";

/// Exact non-negative ratio retained without floating-point interpretation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExactRatio {
	pub numerator: u32,
	pub denominator: u32,
}

impl ExactRatio {
	#[must_use]
	pub const fn new(numerator: u32, denominator: u32) -> Self {
		assert!(denominator != 0, "ratio denominator must be non-zero");
		Self {
			numerator,
			denominator,
		}
	}
}

/// One permanent self modifier reviewed from an active talent candidate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PermanentModifier {
	AttackSpeedBonus(i32),
	AttackMultiplier(ExactRatio),
	DefenseMultiplier(ExactRatio),
	MaxHpMultiplier(ExactRatio),
}

/// The target contract of Exusiai's second talent.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TalentTarget {
	SelfActor,
	OneRandomOtherDeployedOperator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExusiaiMechanicOverride {
	pub first_talent_prefab_key: &'static str,
	pub first_talent_modifier: PermanentModifier,
	pub second_talent_prefab_key: &'static str,
	pub second_talent_modifiers: [PermanentModifier; 2],
	pub second_talent_targets: [TalentTarget; 2],
}

/// One reviewed talent whose whole effect is a permanent modifier on its own operator.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SelfPermanentTalent {
	/// Position of the talent among the operator's active effect candidates.
	pub effect_index: usize,
	pub prefab_key: &'static str,
	pub modifier: PermanentModifier,
}

/// A reviewed operator whose every active effect candidate is a permanent self modifier.
///
/// The entries must cover every active candidate: a matched override accounts for all of them, so
/// a partial list would silently hide the ones it left out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelfPermanentTalentsOverride {
	pub talents: &'static [SelfPermanentTalent],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorMechanicOverride {
	Exusiai(ExusiaiMechanicOverride),
	SelfPermanentTalents(SelfPermanentTalentsOverride),
}

/// Optional result of matching a reviewed prefab binding.
///
/// `Absent` means no special binding exists and the generic paths remain authoritative. `Drift`
/// means a known binding no longer matches the snapshot; callers should omit only that mechanism.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorMechanicOverrideEntry {
	Matched(OperatorMechanicOverride),
	Drift(OperatorMechanicBindingError),
	Absent,
}

impl OperatorMechanicOverrideEntry {
	#[must_use]
	pub const fn as_matched(&self) -> Option<&OperatorMechanicOverride> {
		match self {
			Self::Matched(reviewed) => Some(reviewed),
			Self::Drift(_) | Self::Absent => None,
		}
	}

	#[must_use]
	pub const fn as_drift(&self) -> Option<&OperatorMechanicBindingError> {
		match self {
			Self::Drift(drift) => Some(drift),
			Self::Matched(_) | Self::Absent => None,
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorMechanicBindingError {
	operator_id: String,
	code: &'static str,
	detail: String,
}

impl OperatorMechanicBindingError {
	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub const fn code(&self) -> &'static str {
		self.code
	}

	#[must_use]
	pub fn detail(&self) -> &str {
		&self.detail
	}
}

impl Display for OperatorMechanicBindingError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		write!(
			formatter,
			"operator mechanic binding {} drifted ({}): {}",
			self.operator_id, self.code, self.detail
		)
	}
}

impl Error for OperatorMechanicBindingError {}

/// Matches one exact, optional mechanic override.
#[must_use]
pub fn review_operator_mechanic_override(
	build: &ResolvedOperatorBuild,
) -> OperatorMechanicOverrideEntry {
	let reviewed: Result<OperatorMechanicOverride, OperatorMechanicBindingError> =
		match build.operator_id() {
			EXUSIAI_OPERATOR_ID => review_exusiai(build).map(OperatorMechanicOverride::Exusiai),
			CUORA_OPERATOR_ID => {
				review_cuora(build).map(OperatorMechanicOverride::SelfPermanentTalents)
			}
			_ => return OperatorMechanicOverrideEntry::Absent,
		};
	match reviewed {
		Ok(reviewed) => OperatorMechanicOverrideEntry::Matched(reviewed),
		Err(drift) => OperatorMechanicOverrideEntry::Drift(drift),
	}
}

fn review_exusiai(
	resolved: &ResolvedOperatorBuild,
) -> Result<ExusiaiMechanicOverride, OperatorMechanicBindingError> {
	validate_exact_build(
		resolved,
		ExactBuildShape {
			elite: 2,
			level: 90,
			trust_percent: 200,
			skill: Some(2),
			skill_level: Some(10),
			position: OperatorPosition::Ranged,
			profession: "SNIPER",
			sub_profession_id: "fastshot",
		},
	)?;
	validate_skill(
		resolved,
		ExpectedSkillShape {
			skill_id: EXUSIAI_SKILL_ID,
			name: "扫射模式",
			prefab_id: Some(EXUSIAI_SKILL_ID),
			override_token_key: None,
			activation_type: SkillActivationType::Manual,
			duration_type: SkillDurationType::None,
			duration_seconds: 15.0,
			range_id: None,
			sp_type: SkillSpType::IncreaseWithTime,
			max_charges: 1,
			sp_cost: 35,
			initial_sp: 25,
			sp_increment: 1.0,
			blackboard: &[
				ExpectedBlackboardEntry::numeric("attack@atk_scale", 1.25),
				ExpectedBlackboardEntry::numeric("attack@times", 4.0),
			],
		},
	)?;
	validate_talent_shape(resolved, 2)?;
	validate_selected_talent(
		resolved,
		0,
		ExpectedTalentShape {
			name: "快速弹匣",
			description: "攻击速度+15<@ba.talpu>（+3）</>",
			prefab_key: "1",
			required_potential_rank: 2,
			blackboard: &[ExpectedBlackboardEntry::numeric("attack_speed", 15.0)],
		},
	)?;
	validate_selected_talent(
		resolved,
		1,
		ExpectedTalentShape {
			name: "天使的祝福",
			description: concat!(
				"攻击力+8%<@ba.talpu>（+2%）</>，生命上限+13%",
				"<@ba.talpu>（+3%）</>。置入战场后这个效果会同样赋予给一名随机友方单位"
			),
			prefab_key: "2",
			required_potential_rank: 5,
			blackboard: &[
				ExpectedBlackboardEntry::numeric("max_hp", 0.13),
				ExpectedBlackboardEntry::numeric("atk", 0.08),
			],
		},
	)?;
	Ok(exusiai_review())
}

/// 蛇屠箱 has one talent and no trait candidate; its whole reviewed effect is a flat defense bonus.
fn review_cuora(
	resolved: &ResolvedOperatorBuild,
) -> Result<SelfPermanentTalentsOverride, OperatorMechanicBindingError> {
	validate_exact_build(
		resolved,
		ExactBuildShape {
			elite: 2,
			level: 70,
			trust_percent: 200,
			// A talent binding does not depend on which skill is selected.
			skill: None,
			skill_level: None,
			position: OperatorPosition::Melee,
			profession: "TANK",
			sub_profession_id: "protector",
		},
	)?;
	validate_talent_shape(resolved, 1)?;
	validate_selected_talent(
		resolved,
		0,
		ExpectedTalentShape {
			name: "防御专精",
			description: "防御力+12%",
			prefab_key: "1",
			required_potential_rank: 0,
			blackboard: &[ExpectedBlackboardEntry::numeric("def", 0.12)],
		},
	)?;
	Ok(cuora_review())
}

/// def+12% is a multiplier of 1.12 on the base panel defense.
const CUORA_TALENTS: [SelfPermanentTalent; 1] = [SelfPermanentTalent {
	effect_index: 0,
	prefab_key: "1",
	modifier: PermanentModifier::DefenseMultiplier(ExactRatio::new(28, 25)),
}];

const fn cuora_review() -> SelfPermanentTalentsOverride {
	SelfPermanentTalentsOverride {
		talents: &CUORA_TALENTS,
	}
}

#[derive(Clone)]
struct ExactBuildShape {
	elite: u8,
	level: u32,
	trust_percent: u8,
	/// `None` leaves skill selection to the generic skill/prefab pipeline.
	skill: Option<u8>,
	/// `None` leaves skill level to the generic skill/prefab pipeline.
	skill_level: Option<u8>,
	position: OperatorPosition,
	profession: &'static str,
	sub_profession_id: &'static str,
}

fn validate_exact_build(
	resolved: &ResolvedOperatorBuild,
	expected: ExactBuildShape,
) -> Result<(), OperatorMechanicBindingError> {
	let build: &OperatorBuild = resolved.build();
	require_equal(resolved, "build.elite", build.elite(), expected.elite)?;
	require_equal(resolved, "build.level", build.level(), expected.level)?;
	require_equal(
		resolved,
		"build.trust_percent",
		build.trust_percent(),
		expected.trust_percent,
	)?;
	if let Some(skill) = expected.skill {
		require_equal(resolved, "build.skill", build.skill(), skill)?;
	}
	if let Some(skill_level) = expected.skill_level {
		require_equal(
			resolved,
			"build.skill_level",
			build.skill_level(),
			skill_level,
		)?;
	}
	require_equal(
		resolved,
		"build.potential",
		build.potential(),
		PotentialRank::P6,
	)?;
	require_equal(resolved, "build.module_level", build.module_level(), 0)?;
	if !matches!(build.module(), OperatorModule::None) {
		return Err(drift(
			resolved,
			"build.module",
			"None",
			format!("{:?}", build.module()),
		));
	}
	require_equal(
		resolved,
		"position",
		resolved.position(),
		&expected.position,
	)?;
	require_equal(
		resolved,
		"profession",
		resolved.profession(),
		expected.profession,
	)?;
	require_equal(
		resolved,
		"sub_profession_id",
		resolved.sub_profession_id(),
		expected.sub_profession_id,
	)
}

struct ExpectedSkillShape<'a> {
	skill_id: &'static str,
	name: &'static str,
	prefab_id: Option<&'static str>,
	override_token_key: Option<&'static str>,
	activation_type: SkillActivationType,
	duration_type: SkillDurationType,
	duration_seconds: f64,
	range_id: Option<&'static str>,
	sp_type: SkillSpType,
	max_charges: u32,
	sp_cost: u32,
	initial_sp: u32,
	sp_increment: f64,
	blackboard: &'a [ExpectedBlackboardEntry],
}

fn validate_skill(
	resolved: &ResolvedOperatorBuild,
	expected: ExpectedSkillShape<'_>,
) -> Result<(), OperatorMechanicBindingError> {
	let skill = resolved.selected_skill();
	require_equal(
		resolved,
		"resolved.skill_id",
		resolved.skill_id(),
		expected.skill_id,
	)?;
	require_equal(
		resolved,
		"selected_skill.skill_id",
		skill.skill_id(),
		expected.skill_id,
	)?;
	require_equal(resolved, "selected_skill.name", skill.name(), expected.name)?;
	require_equal(
		resolved,
		"selected_skill.prefab_id",
		skill.prefab_id(),
		expected.prefab_id,
	)?;
	require_equal(
		resolved,
		"selected_skill.override_prefab_key",
		skill.override_prefab_key(),
		None,
	)?;
	require_equal(
		resolved,
		"selected_skill.override_token_key",
		skill.override_token_key(),
		expected.override_token_key,
	)?;
	require_equal(
		resolved,
		"selected_skill.activation_type",
		skill.activation_type(),
		&expected.activation_type,
	)?;
	require_equal(
		resolved,
		"selected_skill.duration_type",
		skill.duration_type(),
		&expected.duration_type,
	)?;
	require_f64(
		resolved,
		"selected_skill.duration_seconds",
		skill.duration_seconds(),
		expected.duration_seconds,
	)?;
	require_equal(
		resolved,
		"selected_skill.range_id",
		skill.range_id(),
		expected.range_id,
	)?;
	let sp = skill.sp();
	require_equal(
		resolved,
		"selected_skill.sp.type",
		sp.sp_type(),
		&expected.sp_type,
	)?;
	require_equal(
		resolved,
		"selected_skill.sp.max_charges",
		sp.max_charges(),
		expected.max_charges,
	)?;
	require_equal(
		resolved,
		"selected_skill.sp.cost",
		sp.cost(),
		expected.sp_cost,
	)?;
	require_equal(
		resolved,
		"selected_skill.sp.initial",
		sp.initial(),
		expected.initial_sp,
	)?;
	require_f64(
		resolved,
		"selected_skill.sp.increment",
		sp.increment(),
		expected.sp_increment,
	)?;
	validate_blackboard(
		resolved,
		"selected_skill.blackboard",
		skill.blackboard(),
		expected.blackboard,
	)
}

fn validate_talent_shape(
	resolved: &ResolvedOperatorBuild,
	expected_group_count: usize,
) -> Result<(), OperatorMechanicBindingError> {
	validate_effect_group_counts(resolved, 0, expected_group_count)
}

fn validate_effect_group_counts(
	resolved: &ResolvedOperatorBuild,
	expected_trait_candidate_count: usize,
	expected_talent_group_count: usize,
) -> Result<(), OperatorMechanicBindingError> {
	require_equal(
		resolved,
		"trait_candidates.len",
		resolved.trait_candidates().len(),
		expected_trait_candidate_count,
	)?;
	require_equal(
		resolved,
		"talent_groups.len",
		resolved.talent_groups().len(),
		expected_talent_group_count,
	)
}

struct ExpectedTalentShape<'a> {
	name: &'static str,
	description: &'static str,
	prefab_key: &'static str,
	required_potential_rank: u8,
	blackboard: &'a [ExpectedBlackboardEntry],
}

fn validate_selected_talent(
	resolved: &ResolvedOperatorBuild,
	group_index: usize,
	expected: ExpectedTalentShape<'_>,
) -> Result<(), OperatorMechanicBindingError> {
	let group = resolved.talent_groups().get(group_index).ok_or_else(|| {
		drift(
			resolved,
			"talent_groups",
			format!("group {group_index}"),
			"missing",
		)
	})?;
	let candidate: &OperatorEffectCandidate =
		select_effect_candidate(group.candidates(), resolved.build()).ok_or_else(|| {
			drift(
				resolved,
				"talent candidate",
				format!("active group {group_index} candidate"),
				"none",
			)
		})?;
	let prefix: String = format!("talent_groups[{group_index}].selected");
	require_equal(
		resolved,
		&format!("{prefix}.unlock_phase"),
		candidate.unlock_phase(),
		"PHASE_2",
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.unlock_level"),
		candidate.unlock_level(),
		1,
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.required_potential_rank"),
		candidate.required_potential_rank(),
		expected.required_potential_rank,
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.prefab_key"),
		candidate.prefab_key(),
		Some(expected.prefab_key),
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.name"),
		candidate.name(),
		Some(expected.name),
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.description"),
		candidate.description(),
		Some(expected.description),
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.range_id"),
		candidate.range_id(),
		None,
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.token_key"),
		candidate.token_key(),
		None,
	)?;
	require_equal(
		resolved,
		&format!("{prefix}.hidden"),
		candidate.hidden(),
		false,
	)?;
	validate_blackboard(
		resolved,
		&format!("{prefix}.blackboard"),
		candidate.blackboard(),
		expected.blackboard,
	)
}

#[derive(Clone, Copy)]
struct ExpectedBlackboardEntry {
	key: &'static str,
	value: f64,
	value_string: Option<&'static str>,
}

impl ExpectedBlackboardEntry {
	const fn numeric(key: &'static str, value: f64) -> Self {
		Self {
			key,
			value,
			value_string: None,
		}
	}
}

fn validate_blackboard(
	resolved: &ResolvedOperatorBuild,
	field: &str,
	actual: &[BlackboardEntry],
	expected: &[ExpectedBlackboardEntry],
) -> Result<(), OperatorMechanicBindingError> {
	let observed = actual
		.iter()
		.map(|entry| (entry.key(), entry.value(), entry.value_string()));
	if blackboard_matches(observed, expected) {
		return Ok(());
	}
	Err(drift(
		resolved,
		field,
		describe_expected_blackboard(expected),
		describe_blackboard(actual),
	))
}

fn blackboard_matches<'a, I>(actual: I, expected: &[ExpectedBlackboardEntry]) -> bool
where
	I: ExactSizeIterator<Item = (&'a str, f64, Option<&'a str>)>,
{
	if actual.len() != expected.len() {
		return false;
	}
	actual.zip(expected).all(|(actual, expected)| {
		actual.0 == expected.key
			&& actual.1.to_bits() == expected.value.to_bits()
			&& actual.2 == expected.value_string
	})
}

fn describe_blackboard(entries: &[BlackboardEntry]) -> String {
	format!(
		"{:?}",
		entries
			.iter()
			.map(|entry| (entry.key(), entry.value(), entry.value_string()))
			.collect::<Vec<_>>()
	)
}

fn describe_expected_blackboard(entries: &[ExpectedBlackboardEntry]) -> String {
	format!(
		"{:?}",
		entries
			.iter()
			.map(|entry| (entry.key, entry.value, entry.value_string))
			.collect::<Vec<_>>()
	)
}

fn select_effect_candidate<'a>(
	candidates: &'a [OperatorEffectCandidate],
	build: &OperatorBuild,
) -> Option<&'a OperatorEffectCandidate> {
	candidates
		.iter()
		.filter(|candidate| candidate_is_active(candidate, build))
		.max_by_key(|candidate| {
			(
				parse_phase(candidate.unlock_phase()).unwrap_or(0),
				candidate.unlock_level(),
				candidate.required_potential_rank(),
			)
		})
}

fn candidate_is_active(candidate: &OperatorEffectCandidate, build: &OperatorBuild) -> bool {
	let Some(phase) = parse_phase(candidate.unlock_phase()) else {
		return false;
	};
	let level_unlocked: bool = build.elite() > phase
		|| (build.elite() == phase && build.level() >= candidate.unlock_level());
	let applied_potential_rank: u8 = build.potential().number().saturating_sub(1);
	level_unlocked && applied_potential_rank >= candidate.required_potential_rank()
}

fn parse_phase(value: &str) -> Option<u8> {
	value.strip_prefix("PHASE_")?.parse().ok()
}

fn require_equal<T>(
	resolved: &ResolvedOperatorBuild,
	field: &str,
	actual: T,
	expected: T,
) -> Result<(), OperatorMechanicBindingError>
where
	T: PartialEq + std::fmt::Debug,
{
	if actual == expected {
		Ok(())
	} else {
		Err(drift(
			resolved,
			field,
			format!("{expected:?}"),
			format!("{actual:?}"),
		))
	}
}

fn require_f64(
	resolved: &ResolvedOperatorBuild,
	field: &str,
	actual: f64,
	expected: f64,
) -> Result<(), OperatorMechanicBindingError> {
	if actual.to_bits() == expected.to_bits() {
		Ok(())
	} else {
		Err(drift(resolved, field, expected, actual))
	}
}

fn drift(
	resolved: &ResolvedOperatorBuild,
	field: &str,
	expected: impl Display,
	actual: impl Display,
) -> OperatorMechanicBindingError {
	unsupported(
		resolved,
		"reviewed_shape_drift",
		format!("{field} expected {expected}, found {actual}"),
	)
}

fn unsupported(
	resolved: &ResolvedOperatorBuild,
	code: &'static str,
	detail: impl Into<String>,
) -> OperatorMechanicBindingError {
	OperatorMechanicBindingError {
		operator_id: resolved.operator_id().to_owned(),
		code,
		detail: detail.into(),
	}
}

fn exusiai_review() -> ExusiaiMechanicOverride {
	ExusiaiMechanicOverride {
		first_talent_prefab_key: "1",
		first_talent_modifier: PermanentModifier::AttackSpeedBonus(15),
		second_talent_prefab_key: "2",
		second_talent_modifiers: [
			PermanentModifier::AttackMultiplier(ExactRatio::new(27, 25)),
			PermanentModifier::MaxHpMultiplier(ExactRatio::new(113, 100)),
		],
		second_talent_targets: [
			TalentTarget::SelfActor,
			TalentTarget::OneRandomOtherDeployedOperator,
		],
	}
}

#[cfg(test)]
mod tests {
	use super::{
		blackboard_matches, exusiai_review, ExpectedBlackboardEntry, PermanentModifier,
		TalentTarget,
	};

	#[test]
	fn pure_blackboard_shape_match_is_exact_and_ordered() {
		let expected = [
			ExpectedBlackboardEntry::numeric("atk", 0.08),
			ExpectedBlackboardEntry::numeric("max_hp", 0.13),
		];
		assert!(blackboard_matches(
			[("atk", 0.08, None), ("max_hp", 0.13, None)].into_iter(),
			&expected,
		));
		assert!(!blackboard_matches(
			[("max_hp", 0.13, None), ("atk", 0.08, None)].into_iter(),
			&expected,
		));
		assert!(!blackboard_matches(
			[("atk", 0.080_001, None), ("max_hp", 0.13, None)].into_iter(),
			&expected,
		));
	}

	#[test]
	fn exusiai_review_retains_reusable_talent_components() {
		let review = exusiai_review();
		assert_eq!(
			review.first_talent_modifier,
			PermanentModifier::AttackSpeedBonus(15)
		);
		assert_eq!(
			review.second_talent_targets,
			[
				TalentTarget::SelfActor,
				TalentTarget::OneRandomOtherDeployedOperator,
			]
		);
	}
}
