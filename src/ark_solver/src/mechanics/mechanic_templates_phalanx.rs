//! Gamedata-backed decoder for the generic phalanx archetype trait.
//!
//! This decoder does not claim prefab-script evidence. `subProfessionId` selects the reviewed
//! archetype rule, while the active trait candidate supplies its exact numerical parameters.

use std::{error::Error, fmt};

use crate::data::{OperatorEffectCandidate, ResolvedOperatorBuild};

use crate::mechanics::{
	combat::{
		ExpectedValue, MechanicBehavior, MechanicId, ReplayRepeat, StatKind, StatModifierBucket,
	},
	mechanic_semantics::{
		GamedataSemanticSourceLocation, MechanicSemanticProgram, MechanicSemanticRule,
		SemanticAction, SemanticBuffModifier, SemanticEvent, SemanticLifetime, SemanticProvenance,
		SemanticSkillInactiveStance, SemanticTrigger, SemanticTriggerFilter,
		SemanticValueExpression,
	},
	normal_attack::FixedDamage,
};

const PHALANX_SUB_PROFESSION_ID: &str = "phalanx";
const PHALANX_RULE_REVISION: &str = "ark-solver/phalanx-archetype/v1";
const CHARACTER_TABLE_PATH: &str = "character_table.json";
const DEF_BLACKBOARD_KEY: &str = "def";
const ARTS_RESISTANCE_BLACKBOARD_KEY: &str = "magic_resistance";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PhalanxNormalAttackRule {
	SuppressedByArchetype,
	UnmodifiedByArchetype,
}

/// Complete contribution of the archetype trait in one mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhalanxModeSemantics {
	pub normal_attack: PhalanxNormalAttackRule,
	pub defense_multiplier: ExpectedValue,
	/// Additive RES points represented as a ratio (`20` points becomes `0.20`).
	pub arts_resistance_additive_ratio: ExpectedValue,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhalanxModeTransition {
	pub event: SemanticEvent,
	pub mode: PhalanxModeSemantics,
}

/// Operator-independent phalanx behavior parameterized by the selected official trait candidate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhalanxTraitSemantics {
	pub outside_skill: PhalanxModeSemantics,
	pub transitions: [PhalanxModeTransition; 2],
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DecodedPhalanxTrait {
	pub mechanic_id: MechanicId,
	pub provenance: SemanticProvenance,
	pub semantics: PhalanxTraitSemantics,
}

impl DecodedPhalanxTrait {
	/// Lowers the exact archetype contribution through the canonical semantic-program path.
	#[must_use]
	pub fn lower_program(&self) -> MechanicSemanticProgram {
		MechanicSemanticProgram {
			rules: vec![MechanicSemanticRule {
				mechanic: self.mechanic_id.clone(),
				provenance: self.provenance.clone(),
				trigger: SemanticTrigger {
					event: SemanticEvent::Deploy,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				},
				lifetime: SemanticLifetime::OwnerDeployment,
				action: SemanticAction::InstallSkillInactiveStance(SemanticSkillInactiveStance {
					key: self.mechanic_id.clone(),
					disable_normal_attacks: matches!(
						self.semantics.outside_skill.normal_attack,
						PhalanxNormalAttackRule::SuppressedByArchetype
					),
					modifiers: vec![
						SemanticBuffModifier::StatMultiplier {
							bucket: StatModifierBucket::Panel,
							stat: StatKind::Defense,
							multiplier: SemanticValueExpression::Literal(
								self.semantics.outside_skill.defense_multiplier,
							),
						},
						SemanticBuffModifier::ArtsResistanceAddition {
							ratio: SemanticValueExpression::Literal(
								self.semantics.outside_skill.arts_resistance_additive_ratio,
							),
						},
					],
				}),
				behavior: MechanicBehavior::StatModifier,
			}],
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhalanxTraitDecodeError {
	field: &'static str,
	expected: String,
	actual: String,
}

impl PhalanxTraitDecodeError {
	fn drift(field: &'static str, expected: impl Into<String>, actual: impl Into<String>) -> Self {
		Self {
			field,
			expected: expected.into(),
			actual: actual.into(),
		}
	}

	#[must_use]
	pub const fn field(&self) -> &'static str {
		self.field
	}
}

impl fmt::Display for PhalanxTraitDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"phalanx trait shape drift at {}: expected {}, found {}",
			self.field, self.expected, self.actual
		)
	}
}

impl Error for PhalanxTraitDecodeError {}

/// Recognizes the official phalanx archetype without branching on operator identity.
///
/// A non-phalanx build returns `Ok(None)`. Once `subProfessionId == phalanx`, every selected-trait
/// structural mismatch is an error rather than an approximation.
pub fn try_decode_phalanx_trait(
	build: &ResolvedOperatorBuild,
) -> Result<Option<DecodedPhalanxTrait>, PhalanxTraitDecodeError> {
	if build.sub_profession_id() != PHALANX_SUB_PROFESSION_ID {
		return Ok(None);
	}
	let candidate: &OperatorEffectCandidate =
		build.selected_trait_candidate().ok_or_else(|| {
			PhalanxTraitDecodeError::drift(
				"selected_trait_candidate",
				"one active candidate",
				"none",
			)
		})?;
	let blackboard: Vec<(&str, f64, Option<&str>)> = candidate
		.blackboard()
		.iter()
		.map(|entry| (entry.key(), entry.value(), entry.value_string()))
		.collect();
	let semantics: PhalanxTraitSemantics = decode_shape(
		build.sub_profession_id(),
		Some(SelectedTraitShape {
			prefab_key: candidate.prefab_key(),
			range_id: candidate.range_id(),
			token_key: candidate.token_key(),
			hidden: candidate.hidden(),
			blackboard: &blackboard,
		}),
	)?
	.expect("phalanx identity was checked before exact shape decoding");
	let selected_index: usize = build
		.trait_candidates()
		.iter()
		.position(|observed| std::ptr::eq(observed, candidate))
		.expect("selected trait candidate comes from the resolved candidate slice");
	Ok(Some(DecodedPhalanxTrait {
		mechanic_id: MechanicId::new(format!("{}/trait/phalanx", build.operator_id())),
		provenance: gamedata_provenance(
			build.resolved_revision(),
			build.operator_id(),
			selected_index,
		),
		semantics,
	}))
}

#[derive(Clone, Copy)]
struct SelectedTraitShape<'a> {
	prefab_key: Option<&'a str>,
	range_id: Option<&'a str>,
	token_key: Option<&'a str>,
	hidden: bool,
	blackboard: &'a [(&'a str, f64, Option<&'a str>)],
}

fn decode_shape(
	sub_profession_id: &str,
	candidate: Option<SelectedTraitShape<'_>>,
) -> Result<Option<PhalanxTraitSemantics>, PhalanxTraitDecodeError> {
	if sub_profession_id != PHALANX_SUB_PROFESSION_ID {
		return Ok(None);
	}
	let candidate: SelectedTraitShape<'_> = candidate.ok_or_else(|| {
		PhalanxTraitDecodeError::drift("selected_trait_candidate", "one active candidate", "none")
	})?;
	require_none("selected_trait.prefab_key", candidate.prefab_key)?;
	require_none("selected_trait.range_id", candidate.range_id)?;
	require_none("selected_trait.token_key", candidate.token_key)?;
	if candidate.hidden {
		return Err(PhalanxTraitDecodeError::drift(
			"selected_trait.hidden",
			"false",
			"true",
		));
	}
	let expected_keys: [&str; 2] = [DEF_BLACKBOARD_KEY, ARTS_RESISTANCE_BLACKBOARD_KEY];
	if candidate.blackboard.len() != expected_keys.len()
		|| candidate
			.blackboard
			.iter()
			.zip(expected_keys)
			.any(|((key, _, value_string), expected)| *key != expected || value_string.is_some())
	{
		return Err(PhalanxTraitDecodeError::drift(
			"selected_trait.blackboard",
			"[(def, numeric), (magic_resistance, numeric)]",
			format!("{:?}", candidate.blackboard),
		));
	}
	let defense_bonus: ExpectedValue =
		expected_nonnegative("selected_trait.blackboard.def", candidate.blackboard[0].1)?;
	let defense_multiplier: ExpectedValue = ExpectedValue::from_scaled(
		ExpectedValue::ONE
			.scaled()
			.checked_add(defense_bonus.scaled())
			.ok_or_else(|| {
				PhalanxTraitDecodeError::drift(
					"selected_trait.blackboard.def",
					"finite representable non-negative number",
					candidate.blackboard[0].1.to_string(),
				)
			})?,
	);
	let arts_resistance_points: f64 = candidate.blackboard[1].1;
	let arts_resistance_additive_ratio: ExpectedValue = expected_nonnegative(
		"selected_trait.blackboard.magic_resistance",
		arts_resistance_points / 100.0,
	)?;
	let outside_skill = PhalanxModeSemantics {
		normal_attack: PhalanxNormalAttackRule::SuppressedByArchetype,
		defense_multiplier,
		arts_resistance_additive_ratio,
	};
	let during_skill = PhalanxModeSemantics {
		normal_attack: PhalanxNormalAttackRule::UnmodifiedByArchetype,
		defense_multiplier: ExpectedValue::ONE,
		arts_resistance_additive_ratio: ExpectedValue::ZERO,
	};
	Ok(Some(PhalanxTraitSemantics {
		outside_skill,
		transitions: [
			PhalanxModeTransition {
				event: SemanticEvent::SkillStarted,
				mode: during_skill,
			},
			PhalanxModeTransition {
				event: SemanticEvent::SkillEnded,
				mode: outside_skill,
			},
		],
	}))
}

fn require_none(field: &'static str, actual: Option<&str>) -> Result<(), PhalanxTraitDecodeError> {
	if actual.is_none() {
		return Ok(());
	}
	Err(PhalanxTraitDecodeError::drift(
		field,
		"none",
		format!("{:?}", actual),
	))
}

fn expected_nonnegative(
	field: &'static str,
	value: f64,
) -> Result<ExpectedValue, PhalanxTraitDecodeError> {
	if !value.is_finite() || value < 0.0 {
		return Err(PhalanxTraitDecodeError::drift(
			field,
			"finite representable non-negative number",
			value.to_string(),
		));
	}
	let fixed: FixedDamage = FixedDamage::try_from_f64(value).map_err(|_| {
		PhalanxTraitDecodeError::drift(
			field,
			"finite representable non-negative number",
			value.to_string(),
		)
	})?;
	Ok(ExpectedValue::from_scaled(fixed.scaled()))
}

fn gamedata_provenance(
	gamedata_revision: &str,
	operator_id: &str,
	selected_candidate_index: usize,
) -> SemanticProvenance {
	let trait_prefix: String =
		format!("$.{operator_id}.trait.candidates[{selected_candidate_index}]");
	SemanticProvenance::Gamedata {
		gamedata_revision: gamedata_revision.to_owned(),
		decoder_rule_revision: PHALANX_RULE_REVISION.to_owned(),
		source: GamedataSemanticSourceLocation {
			table_path: CHARACTER_TABLE_PATH.to_owned(),
			record_key: operator_id.to_owned(),
			field_paths: vec![
				format!("$.{operator_id}.subProfessionId"),
				format!("{trait_prefix}.prefabKey"),
				format!("{trait_prefix}.rangeId"),
				format!("{trait_prefix}.blackboard"),
			],
			resolved_projection_fields: vec![
				"selected_trait_candidate.token_key".to_owned(),
				"selected_trait_candidate.hidden".to_owned(),
			],
		},
	}
}

#[cfg(test)]
mod tests {
	use std::num::NonZeroU32;

	use crate::simulation::TickRate;

	use super::{decode_shape, gamedata_provenance, PhalanxNormalAttackRule, SelectedTraitShape};
	use crate::mechanics::{
		combat::{BuffModifier, ExpectedEffect, ExpectedValue, ReplayTrigger, StatKind},
		mechanic_semantics::{SemanticEvent, SemanticProvenance},
	};

	const VALID_BLACKBOARD: [(&str, f64, Option<&str>); 2] =
		[("def", 2.0, None), ("magic_resistance", 20.0, None)];

	fn valid_shape() -> SelectedTraitShape<'static> {
		SelectedTraitShape {
			prefab_key: None,
			range_id: None,
			token_key: None,
			hidden: false,
			blackboard: &VALID_BLACKBOARD,
		}
	}

	#[test]
	fn exact_shape_decodes_defensive_and_skill_modes() {
		let semantics = decode_shape("phalanx", Some(valid_shape()))
			.expect("reviewed shape")
			.expect("phalanx identity");

		assert_eq!(
			semantics.outside_skill.normal_attack,
			PhalanxNormalAttackRule::SuppressedByArchetype
		);
		assert_eq!(
			semantics.outside_skill.defense_multiplier,
			ExpectedValue::from_scaled(3_000_000)
		);
		assert_eq!(
			semantics.outside_skill.arts_resistance_additive_ratio,
			ExpectedValue::from_scaled(200_000)
		);
		assert_eq!(semantics.transitions[0].event, SemanticEvent::SkillStarted);
		assert_eq!(
			semantics.transitions[0].mode.normal_attack,
			PhalanxNormalAttackRule::UnmodifiedByArchetype
		);
		assert_eq!(
			semantics.transitions[0].mode.defense_multiplier,
			ExpectedValue::ONE
		);
		assert_eq!(
			semantics.transitions[0].mode.arts_resistance_additive_ratio,
			ExpectedValue::ZERO
		);
		assert_eq!(semantics.transitions[1].event, SemanticEvent::SkillEnded);
		assert_eq!(semantics.transitions[1].mode, semantics.outside_skill);
	}

	#[test]
	fn non_phalanx_archetype_is_not_claimed() {
		assert!(decode_shape("fastshot", Some(valid_shape()))
			.expect("unrelated archetype is not drift")
			.is_none());
	}

	#[test]
	fn structural_identity_drift_is_rejected() {
		let mut prefab = valid_shape();
		prefab.prefab_key = Some("trait_prefab");
		assert_eq!(
			decode_shape("phalanx", Some(prefab))
				.expect_err("prefab-bearing candidate is another template")
				.field(),
			"selected_trait.prefab_key"
		);

		let mut ranged = valid_shape();
		ranged.range_id = Some("x-1");
		assert_eq!(
			decode_shape("phalanx", Some(ranged))
				.expect_err("range-bearing candidate is another template")
				.field(),
			"selected_trait.range_id"
		);

		let mut token = valid_shape();
		token.token_key = Some("token");
		assert_eq!(
			decode_shape("phalanx", Some(token))
				.expect_err("token-bearing candidate is another template")
				.field(),
			"selected_trait.token_key"
		);

		let mut hidden = valid_shape();
		hidden.hidden = true;
		assert_eq!(
			decode_shape("phalanx", Some(hidden))
				.expect_err("hidden candidate is another template")
				.field(),
			"selected_trait.hidden"
		);
	}

	#[test]
	fn blackboard_shape_and_numeric_drift_are_rejected() {
		let reordered = [("magic_resistance", 20.0, None), ("def", 2.0, None)];
		let mut reordered_shape = valid_shape();
		reordered_shape.blackboard = &reordered;
		assert_eq!(
			decode_shape("phalanx", Some(reordered_shape))
				.expect_err("blackboard order is part of the exact snapshot shape")
				.field(),
			"selected_trait.blackboard"
		);

		let extra = [
			("def", 2.0, None),
			("magic_resistance", 20.0, None),
			("atk", 1.0, None),
		];
		let mut extra_shape = valid_shape();
		extra_shape.blackboard = &extra;
		assert_eq!(
			decode_shape("phalanx", Some(extra_shape))
				.expect_err("extra values change the candidate meaning")
				.field(),
			"selected_trait.blackboard"
		);

		let non_finite = [("def", f64::NAN, None), ("magic_resistance", 20.0, None)];
		let mut non_finite_shape = valid_shape();
		non_finite_shape.blackboard = &non_finite;
		assert_eq!(
			decode_shape("phalanx", Some(non_finite_shape))
				.expect_err("non-finite parameter is invalid")
				.field(),
			"selected_trait.blackboard.def"
		);
	}

	#[test]
	fn gamedata_provenance_never_claims_prefab_identity() {
		let provenance = gamedata_provenance("a".repeat(40).as_str(), "char_test", 2);

		let SemanticProvenance::Gamedata {
			gamedata_revision,
			decoder_rule_revision,
			source,
		} = &provenance
		else {
			panic!("phalanx provenance must remain gamedata-backed");
		};
		assert_eq!(gamedata_revision, &"a".repeat(40));
		assert_eq!(decoder_rule_revision, "ark-solver/phalanx-archetype/v1");
		assert_eq!(
			provenance.origin(),
			"verified-gamedata:character_table.json:char_test"
		);
		assert!(source
			.field_paths
			.iter()
			.any(|path| path.ends_with("trait.candidates[2].blackboard")));
		assert!(source
			.field_paths
			.iter()
			.all(|path| !path.ends_with("tokenKey") && !path.ends_with("hidden")));
		assert_eq!(provenance.field_paths(), source.field_paths);
	}

	#[test]
	fn exact_shape_lowers_through_semantic_program_to_runtime_stance() {
		let semantics = decode_shape("phalanx", Some(valid_shape()))
			.expect("reviewed shape")
			.expect("phalanx identity");
		let decoded = super::DecodedPhalanxTrait {
			mechanic_id: crate::mechanics::combat::MechanicId::new("test/phalanx"),
			provenance: gamedata_provenance("revision", "char_test", 0),
			semantics,
		};

		let lowered = decoded
			.lower_program()
			.lower(&[], TickRate::new(NonZeroU32::new(30).expect("nonzero")))
			.expect("valid phalanx semantic program");
		assert_eq!(lowered.on_deploy.len(), 1);
		assert_eq!(lowered.reached_mechanics.len(), 1);
		assert_eq!(
			lowered.reached_mechanics[0].origin,
			"verified-gamedata:character_table.json:char_test"
		);
		let ExpectedEffect::RegisterListener(listener) = &lowered.on_deploy[0].effect else {
			panic!("deploy-triggered semantic rule must lower to a listener");
		};
		assert_eq!(listener.trigger, ReplayTrigger::Deploy);
		let ExpectedEffect::InstallSkillInactiveStance(stance) = &listener.effects[0].effect else {
			panic!("phalanx rule must install one skill-inactive stance");
		};

		assert_eq!(stance.key.as_str(), "test/phalanx");
		assert!(stance.disable_normal_attacks);
		assert_eq!(stance.modifiers.len(), 2);
		assert!(matches!(
			&stance.modifiers[0],
			BuffModifier::Stat(modifier)
				if modifier.stat == StatKind::Defense
					&& modifier.multiplier == ExpectedValue::from_scaled(3_000_000)
					&& modifier.duration_ticks.is_none()
		));
		assert_eq!(
			stance.modifiers[1],
			BuffModifier::ArtsResistanceAddition {
				ratio: ExpectedValue::from_scaled(200_000),
			}
		);
	}
}
