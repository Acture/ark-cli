//! Fail-closed decoder for the reusable roster-threshold aura template.
//!
//! Recognition is based on serialized script schemas, graph structure, and buff identities, never
//! operator identity. The three conditions are independent and may all be active simultaneously.
//! Decoded branches lower to generic conditional-aura semantics.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{ArkDataSnapshot, OperatorEffectCandidate};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_interpreter::{
		BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemanticIssue,
	},
	combat::{MechanicBehavior, MechanicId, ReplayRepeat, StatKind, StatModifierBucket},
	mechanic_semantics::{
		MechanicSemanticProgram, MechanicSemanticRule, SemanticAction, SemanticBuffModifier,
		SemanticConditionalAura, SemanticConditionalAuraPeriodicEffect, SemanticEffect,
		SemanticEvent, SemanticLifetime, SemanticProvenance, SemanticSourceLocation,
		SemanticTeamCompositionCondition, SemanticTrigger, SemanticTriggerFilter,
		SemanticValueExpression,
	},
};

const BRIDGE_SCRIPT_ID: [u8; 16] = [
	0x7f, 0xfd, 0xc8, 0x07, 0x91, 0x6c, 0xa1, 0xea, 0x67, 0x41, 0xf4, 0x11, 0x75, 0xed, 0xd3, 0x5f,
];
const BRIDGE_OLD_TYPE_HASH: [u8; 16] = [
	0x59, 0xd9, 0x43, 0x18, 0x68, 0x25, 0x4c, 0x1f, 0xea, 0x2a, 0x27, 0x5c, 0x39, 0x37, 0x95, 0x98,
];
const TOGGLE_SCRIPT_ID: [u8; 16] = [
	0xea, 0x5e, 0x6b, 0x42, 0x9b, 0x9e, 0xf1, 0x7c, 0xa7, 0x06, 0xca, 0xb6, 0x44, 0xba, 0xaa, 0x41,
];
const TOGGLE_OLD_TYPE_HASH: [u8; 16] = [
	0x2f, 0x24, 0x70, 0x09, 0x06, 0x17, 0x2e, 0x94, 0x69, 0x31, 0x24, 0x0a, 0x02, 0x20, 0x27, 0x79,
];
const PROFESSION_CHECKER_SCRIPT_ID: [u8; 16] = [
	0xf5, 0x67, 0x18, 0x4d, 0xb5, 0xf6, 0x81, 0x67, 0x48, 0xd6, 0x68, 0x7b, 0x16, 0x73, 0x4f, 0x3b,
];
const PROFESSION_CHECKER_OLD_TYPE_HASH: [u8; 16] = [
	0xe4, 0x87, 0xa4, 0x50, 0xdb, 0xd7, 0xb2, 0xf8, 0x3c, 0x2e, 0x8a, 0xee, 0x66, 0x7a, 0xb5, 0x8f,
];
const AURA_SCRIPT_ID: [u8; 16] = [
	0x67, 0x85, 0x41, 0xda, 0x6d, 0x91, 0xbf, 0xaa, 0x67, 0xa3, 0xd7, 0xa2, 0x3e, 0x4d, 0x2e, 0xc4,
];
const AURA_OLD_TYPE_HASH: [u8; 16] = [
	0x76, 0x2c, 0x83, 0x2f, 0x2c, 0xc1, 0xa3, 0xa8, 0xcd, 0x0a, 0x48, 0x06, 0x72, 0x38, 0xbf, 0x19,
];
const TARGET_VALIDATOR_SCRIPT_ID: [u8; 16] = [
	0xe2, 0xdb, 0xc5, 0xc0, 0x77, 0x9c, 0xd9, 0x19, 0x18, 0xdc, 0xd7, 0x47, 0x6e, 0xda, 0xfc, 0xb4,
];
const TARGET_VALIDATOR_OLD_TYPE_HASH: [u8; 16] = [
	0x5a, 0x12, 0x7d, 0x18, 0x03, 0xc6, 0x98, 0x22, 0x55, 0x4e, 0x5c, 0x11, 0xd1, 0x35, 0x25, 0xaa,
];
const DECK_MATCHER_SCRIPT_ID: [u8; 16] = [
	0xd8, 0xc6, 0x3f, 0xbf, 0x2e, 0x63, 0xe7, 0x91, 0x72, 0xb8, 0x6d, 0x84, 0xf7, 0xb0, 0x56, 0xb3,
];
const DECK_MATCHER_OLD_TYPE_HASH: [u8; 16] = [
	0xb6, 0xe4, 0xa4, 0x2c, 0x09, 0x56, 0xb6, 0xc4, 0xd3, 0x67, 0xae, 0x7a, 0x25, 0x68, 0x6c, 0xe6,
];
const PASSIVE_HOST_SCRIPT_ID: [u8; 16] = [
	0x2a, 0x10, 0x9a, 0x16, 0x9b, 0xf4, 0x09, 0x3b, 0x1c, 0xa2, 0x03, 0xec, 0xf3, 0x52, 0x28, 0x7e,
];
const PASSIVE_HOST_OLD_TYPE_HASH: [u8; 16] = [
	0x70, 0xea, 0x7c, 0xf3, 0x31, 0x86, 0x10, 0x0f, 0x44, 0x2b, 0x3b, 0x23, 0x05, 0xa0, 0x8f, 0x10,
];

const DIFFERENT_PROFESSION_BUFF_KEY: &str = "shu_t_2[different_profession]";
const SAME_PROFESSION_BUFF_KEY: &str = "shu_t_2[same_profession]";
const ROSTER_TAG_BUFF_KEY: &str = "shu_t_2[sui_count]";
const EMPTY_BUFF_TEMPLATE_KEY: &str = "empty";
const PERIODIC_SP_TEMPLATE_KEY: &str = "modify_sp[trigger]";
const SERIALIZED_ROSTER_TAG: &str = "sui";

const MAX_HP_BLACKBOARD_KEY: &str = "max_hp";
const ATTACK_SPEED_BLACKBOARD_KEY: &str = "attack_speed";
const ATTACK_BLACKBOARD_KEY: &str = "atk";
const INTERVAL_BLACKBOARD_KEY: &str = "interval";
const SKILL_POINT_BLACKBOARD_KEY: &str = "sp";

const PROFESSION_MASK: u16 = 639;
const REQUIRED_PROFESSION_COUNT: u8 = 3;
const REQUIRED_ROSTER_TAG_COUNT: u8 = 4;

/// The serialized template evaluates every branch independently; the branches are not an else-if.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuRosterBranchCombination {
	IndependentAndSimultaneous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShuRosterCondition {
	DeployedDistinctProfessions {
		minimum_count: u8,
		serialized_profession_mask: u16,
	},
	DeployedSameProfession {
		minimum_count: u8,
		serialized_profession_mask: u16,
	},
	FixedRosterSerializedTag {
		minimum_count: u8,
		filter_tag: String,
		serialized_category_mask: u16,
		includes_source: bool,
	},
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuRosterTargetFilter {
	ValidatorProfessionMask(u16),
	DeckCategoryMask(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShuRosterTargetScope {
	pub filter: ShuRosterTargetFilter,
	pub includes_source: bool,
	pub includes_future_deployments: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuRosterStat {
	MaxHp,
	Attack,
	AttackSpeed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuRosterModifierAggregation {
	MultiplicativeBonus,
	Additive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShuRosterEffect {
	StatModifier {
		stat: ShuRosterStat,
		aggregation: ShuRosterModifierAggregation,
		amount: SemanticValueExpression,
	},
	PeriodicSkillPointGrant {
		interval_seconds: SemanticValueExpression,
		points: SemanticValueExpression,
	},
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuRosterBranchLifetime {
	DynamicWhileConditionSatisfied {
		remove_on_condition_loss: bool,
		remove_on_source_detach: bool,
	},
	FixedRosterForBattle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuRosterConditionalBranch {
	pub mechanic_id: MechanicId,
	pub provenance: SemanticProvenance,
	pub condition: ShuRosterCondition,
	pub targets: ShuRosterTargetScope,
	pub effects: Vec<ShuRosterEffect>,
	pub lifetime: ShuRosterBranchLifetime,
}

/// Exact, operator-independent meaning recovered from the reviewed three-root talent graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuRosterTemplateSemantics {
	pub combination: ShuRosterBranchCombination,
	pub distinct_professions: ShuRosterConditionalBranch,
	pub same_profession: ShuRosterConditionalBranch,
	pub roster_tag: ShuRosterConditionalBranch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedShuRosterTemplate {
	pub semantics: ShuRosterTemplateSemantics,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_semantic_fact_sources: Vec<BattlePrefabObjectSource>,
}

impl DecodedShuRosterTemplate {
	#[must_use]
	pub fn lower_program(&self) -> MechanicSemanticProgram {
		let branches: [&ShuRosterConditionalBranch; 3] = [
			&self.semantics.distinct_professions,
			&self.semantics.same_profession,
			&self.semantics.roster_tag,
		];
		MechanicSemanticProgram {
			rules: branches.into_iter().map(lower_branch).collect(),
		}
	}
}

fn lower_branch(branch: &ShuRosterConditionalBranch) -> MechanicSemanticRule {
	debug_assert!(branch.targets.includes_future_deployments);
	debug_assert!(matches!(
		branch.lifetime,
		ShuRosterBranchLifetime::DynamicWhileConditionSatisfied {
			remove_on_condition_loss: true,
			remove_on_source_detach: true,
		} | ShuRosterBranchLifetime::FixedRosterForBattle
	));
	let (event, lifetime): (SemanticEvent, SemanticLifetime) = match branch.lifetime {
		ShuRosterBranchLifetime::DynamicWhileConditionSatisfied { .. } => {
			(SemanticEvent::Deploy, SemanticLifetime::OwnerDeployment)
		}
		ShuRosterBranchLifetime::FixedRosterForBattle => {
			(SemanticEvent::BattleStarted, SemanticLifetime::Persistent)
		}
	};
	let mut modifiers: Vec<SemanticBuffModifier> = Vec::new();
	let mut periodic: Option<SemanticConditionalAuraPeriodicEffect> = None;
	for effect in &branch.effects {
		match effect {
			ShuRosterEffect::StatModifier {
				stat,
				aggregation,
				amount,
			} => modifiers.push(lower_modifier(*stat, *aggregation, amount)),
			ShuRosterEffect::PeriodicSkillPointGrant {
				interval_seconds,
				points,
			} => {
				debug_assert!(periodic.is_none());
				periodic = Some(SemanticConditionalAuraPeriodicEffect {
					initial_delay_seconds: interval_seconds.clone(),
					interval_seconds: interval_seconds.clone(),
					effects: vec![SemanticEffect::GainSp {
						amount: points.clone(),
					}],
				});
			}
		}
	}
	MechanicSemanticRule {
		mechanic: branch.mechanic_id.clone(),
		provenance: branch.provenance.clone(),
		trigger: SemanticTrigger {
			event,
			filter: SemanticTriggerFilter::AffectedOwner,
			repeat: ReplayRepeat::Once,
		},
		lifetime,
		action: SemanticAction::InstallConditionalAura(SemanticConditionalAura {
			key: branch.mechanic_id.clone(),
			condition: lower_condition(&branch.condition),
			target_profession_mask: lower_target_profession_mask(branch.targets.filter),
			includes_source: branch.targets.includes_source,
			lifetime,
			modifiers,
			periodic,
		}),
		behavior: MechanicBehavior::ConditionalAura,
	}
}

fn lower_condition(condition: &ShuRosterCondition) -> SemanticTeamCompositionCondition {
	match condition {
		ShuRosterCondition::DeployedDistinctProfessions {
			minimum_count,
			serialized_profession_mask,
		} => SemanticTeamCompositionCondition::DeployedDistinctProfessions {
			minimum_count: u16::from(*minimum_count),
			profession_mask: *serialized_profession_mask,
		},
		ShuRosterCondition::DeployedSameProfession {
			minimum_count,
			serialized_profession_mask,
		} => SemanticTeamCompositionCondition::DeployedSameProfession {
			minimum_count: u16::from(*minimum_count),
			profession_mask: *serialized_profession_mask,
		},
		ShuRosterCondition::FixedRosterSerializedTag {
			minimum_count,
			filter_tag,
			includes_source,
			..
		} => {
			debug_assert!(*includes_source);
			SemanticTeamCompositionCondition::FixedRosterGroup {
				minimum_count: u16::from(*minimum_count),
				group_id: filter_tag.clone(),
			}
		}
	}
}

const fn lower_target_profession_mask(filter: ShuRosterTargetFilter) -> u16 {
	match filter {
		ShuRosterTargetFilter::ValidatorProfessionMask(mask)
		| ShuRosterTargetFilter::DeckCategoryMask(mask) => mask,
	}
}

fn lower_modifier(
	stat: ShuRosterStat,
	aggregation: ShuRosterModifierAggregation,
	amount: &SemanticValueExpression,
) -> SemanticBuffModifier {
	match (stat, aggregation) {
		(ShuRosterStat::MaxHp, ShuRosterModifierAggregation::MultiplicativeBonus) => {
			SemanticBuffModifier::StatMultiplier {
				bucket: StatModifierBucket::Panel,
				stat: StatKind::MaxHp,
				multiplier: SemanticValueExpression::OnePlus(Box::new(amount.clone())),
			}
		}
		(ShuRosterStat::Attack, ShuRosterModifierAggregation::MultiplicativeBonus) => {
			SemanticBuffModifier::StatMultiplier {
				bucket: StatModifierBucket::Panel,
				stat: StatKind::Attack,
				multiplier: SemanticValueExpression::OnePlus(Box::new(amount.clone())),
			}
		}
		(ShuRosterStat::AttackSpeed, ShuRosterModifierAggregation::Additive) => {
			SemanticBuffModifier::AttackSpeedAddition {
				points: amount.clone(),
			}
		}
		_ => unreachable!("verified Shu roster decoder emitted an unsupported modifier pairing"),
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuRosterTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl ShuRosterTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for ShuRosterTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"roster-threshold aura template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for ShuRosterTemplateDecodeError {}

/// Recognizes and decodes the reviewed roster-threshold aura without operator-ID branching.
pub fn try_decode_shu_roster_template(
	graph: &BattlePrefabGraph,
	candidate: &OperatorEffectCandidate,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedShuRosterTemplate>, ShuRosterTemplateDecodeError> {
	if !contains_shu_roster_identity(graph) {
		return Ok(None);
	}
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(ShuRosterTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.section != BattlePrefabRootSection::Talents {
		return Err(ShuRosterTemplateDecodeError::new(
			"root section",
			format!("expected Talents, found {:?}", graph.section),
		));
	}
	validate_candidate(candidate, graph)?;
	let layout: ShuRosterLayout<'_> = validate_graph_layout(graph)?;

	validate_conditional_branch(&layout.distinct, ProfessionCheckerMode::Distinct)?;
	validate_conditional_branch(&layout.same, ProfessionCheckerMode::Same)?;
	validate_deck_matcher(layout.deck_matcher)?;
	validate_passive_host(layout.passive_host)?;
	let consumed_issues: Vec<BattlePrefabSemanticIssue> =
		validate_and_collect_issues(graph, &layout)?;

	let semantics: ShuRosterTemplateSemantics = ShuRosterTemplateSemantics {
		combination: ShuRosterBranchCombination::IndependentAndSimultaneous,
		distinct_professions: build_profession_semantics(
			graph,
			gamedata_revision,
			&layout.distinct,
			ProfessionCheckerMode::Distinct,
		),
		same_profession: build_profession_semantics(
			graph,
			gamedata_revision,
			&layout.same,
			ProfessionCheckerMode::Same,
		),
		roster_tag: build_roster_tag_semantics(graph, gamedata_revision, &layout),
	};
	Ok(Some(DecodedShuRosterTemplate {
		semantics,
		consumed_issues,
		consumed_semantic_fact_sources: Vec::new(),
	}))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfessionCheckerMode {
	Distinct,
	Same,
}

#[derive(Clone, Copy)]
struct ConditionalBranchLayout<'a> {
	bridge: &'a BattlePrefabMonoBehaviour,
	toggle: &'a BattlePrefabMonoBehaviour,
	checker: &'a BattlePrefabMonoBehaviour,
	aura: &'a BattlePrefabMonoBehaviour,
	validator: &'a BattlePrefabMonoBehaviour,
}

#[derive(Clone, Copy)]
struct ShuRosterLayout<'a> {
	distinct: ConditionalBranchLayout<'a>,
	same: ConditionalBranchLayout<'a>,
	deck_matcher: &'a BattlePrefabMonoBehaviour,
	passive_host: &'a BattlePrefabMonoBehaviour,
}

/// The passive host is a generic ability other talent roots carry on their own, as 白金's does, so
/// it is validated as part of the layout but never claims a root by itself.
fn contains_shu_roster_identity(graph: &BattlePrefabGraph) -> bool {
	graph.mono_behaviours.iter().any(|behavior| {
		behavior.buffs.iter().any(|buff| {
			[buff.buff_key.as_deref(), buff.template_key.as_deref()]
				.into_iter()
				.flatten()
				.any(|key| {
					matches!(
						key,
						DIFFERENT_PROFESSION_BUFF_KEY
							| SAME_PROFESSION_BUFF_KEY
							| ROSTER_TAG_BUFF_KEY
					)
				})
		}) || matches!(
			behavior.script_id,
			Some(
				TOGGLE_SCRIPT_ID
					| PROFESSION_CHECKER_SCRIPT_ID
					| AURA_SCRIPT_ID
					| DECK_MATCHER_SCRIPT_ID
			)
		)
	})
}

fn validate_candidate(
	candidate: &OperatorEffectCandidate,
	graph: &BattlePrefabGraph,
) -> Result<(), ShuRosterTemplateDecodeError> {
	if graph.prefab_name != "2" || candidate.prefab_key() != Some(graph.prefab_name.as_str()) {
		return Err(ShuRosterTemplateDecodeError::new(
			"candidate",
			format!(
				"expected candidate prefab key \"2\" selecting graph root \"2\", found candidate={:?}, graph={:?}",
				candidate.prefab_key(),
				graph.prefab_name
			),
		));
	}
	if candidate.range_id().is_some() || candidate.token_key().is_some() || candidate.hidden() {
		return Err(ShuRosterTemplateDecodeError::new(
			"candidate",
			format!(
				"expected no range/token override and a visible talent, found range={:?}, token={:?}, hidden={}",
				candidate.range_id(),
				candidate.token_key(),
				candidate.hidden()
			),
		));
	}
	let mut values: BTreeMap<&str, f64> = BTreeMap::new();
	for entry in candidate.blackboard() {
		if entry.value_string().is_some() {
			return Err(ShuRosterTemplateDecodeError::new(
				"candidate blackboard",
				format!("entry {:?} unexpectedly has a string value", entry.key()),
			));
		}
		if values.insert(entry.key(), entry.value()).is_some() {
			return Err(ShuRosterTemplateDecodeError::new(
				"candidate blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: Vec<&str> = vec![
		ATTACK_BLACKBOARD_KEY,
		ATTACK_SPEED_BLACKBOARD_KEY,
		INTERVAL_BLACKBOARD_KEY,
		MAX_HP_BLACKBOARD_KEY,
		SKILL_POINT_BLACKBOARD_KEY,
	];
	if values.keys().copied().collect::<Vec<_>>() != expected_keys {
		return Err(ShuRosterTemplateDecodeError::new(
			"candidate blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				values.keys().collect::<Vec<_>>()
			),
		));
	}
	validate_nonnegative_blackboard(ATTACK_BLACKBOARD_KEY, values[ATTACK_BLACKBOARD_KEY])?;
	validate_nonnegative_blackboard(
		ATTACK_SPEED_BLACKBOARD_KEY,
		values[ATTACK_SPEED_BLACKBOARD_KEY],
	)?;
	validate_positive_blackboard(INTERVAL_BLACKBOARD_KEY, values[INTERVAL_BLACKBOARD_KEY])?;
	validate_nonnegative_blackboard(MAX_HP_BLACKBOARD_KEY, values[MAX_HP_BLACKBOARD_KEY])?;
	validate_positive_blackboard(
		SKILL_POINT_BLACKBOARD_KEY,
		values[SKILL_POINT_BLACKBOARD_KEY],
	)?;
	Ok(())
}

fn validate_nonnegative_blackboard(
	key: &str,
	value: f64,
) -> Result<(), ShuRosterTemplateDecodeError> {
	if !value.is_finite() || value < 0.0 {
		return Err(ShuRosterTemplateDecodeError::new(
			"candidate blackboard",
			format!("{key:?} must be finite and nonnegative, found {value}"),
		));
	}
	Ok(())
}

fn validate_positive_blackboard(key: &str, value: f64) -> Result<(), ShuRosterTemplateDecodeError> {
	if !value.is_finite() || value <= 0.0 {
		return Err(ShuRosterTemplateDecodeError::new(
			"candidate blackboard",
			format!("{key:?} must be finite and positive, found {value}"),
		));
	}
	Ok(())
}

fn validate_graph_layout(
	graph: &BattlePrefabGraph,
) -> Result<ShuRosterLayout<'_>, ShuRosterTemplateDecodeError> {
	if graph.root_game_object_indices.len() != 3 {
		return Err(ShuRosterTemplateDecodeError::new(
			"graph roots",
			format!(
				"expected exactly three sibling roots, found {}",
				graph.root_game_object_indices.len()
			),
		));
	}
	if graph.game_objects.len() != 5 || graph.mono_behaviours.len() != 12 {
		return Err(ShuRosterTemplateDecodeError::new(
			"graph shape",
			format!(
				"expected five GameObjects and twelve MonoBehaviours, found {} and {}",
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	let mut conditional: Vec<ConditionalBranchLayout<'_>> = Vec::new();
	let mut deck: Vec<(
		usize,
		&BattlePrefabMonoBehaviour,
		&BattlePrefabMonoBehaviour,
	)> = Vec::new();
	for &root_index in &graph.root_game_object_indices {
		let root = graph.game_objects.get(root_index).ok_or_else(|| {
			ShuRosterTemplateDecodeError::new(
				"graph roots",
				format!("root index {root_index} is outside the GameObject table"),
			)
		})?;
		if root.name != graph.prefab_name {
			return Err(ShuRosterTemplateDecodeError::new(
				"graph roots",
				format!(
					"every sibling root must be named {:?}, found {:?}",
					graph.prefab_name, root.name
				),
			));
		}
		let expected_prefix: String = format!("Others/Direction/Talents/{}#", graph.prefab_name);
		if !root.hierarchy_path.starts_with(&expected_prefix) {
			return Err(ShuRosterTemplateDecodeError::new(
				"graph roots",
				format!(
					"root hierarchy path {:?} is not a Talents sibling path with prefix {expected_prefix:?}",
					root.hierarchy_path
				),
			));
		}
		if root.child_indices.is_empty() {
			let deck_matcher = exact_owned_behavior(
				graph,
				root_index,
				DECK_MATCHER_SCRIPT_ID,
				DECK_MATCHER_OLD_TYPE_HASH,
				"deck matcher",
			)?;
			let passive_host = exact_owned_behavior(
				graph,
				root_index,
				PASSIVE_HOST_SCRIPT_ID,
				PASSIVE_HOST_OLD_TYPE_HASH,
				"passive host",
			)?;
			if root.mono_behaviour_indices.len() != 2 || root.component_sources.len() != 3 {
				return Err(ShuRosterTemplateDecodeError::new(
					"deck root",
					format!(
						"expected two behaviors plus one Transform, found {} behaviors and {} total components",
						root.mono_behaviour_indices.len(),
						root.component_sources.len()
					),
				));
			}
			deck.push((root_index, deck_matcher, passive_host));
			continue;
		}
		if root.child_indices.len() != 1 {
			return Err(ShuRosterTemplateDecodeError::new(
				"conditional root",
				format!(
					"expected one Aura child, found {} children",
					root.child_indices.len()
				),
			));
		}
		let aura_index: usize = root.child_indices[0];
		let aura_object = graph.game_objects.get(aura_index).ok_or_else(|| {
			ShuRosterTemplateDecodeError::new(
				"conditional root",
				format!("Aura child index {aura_index} is outside the GameObject table"),
			)
		})?;
		if aura_object.name != "Aura"
			|| !aura_object.child_indices.is_empty()
			|| aura_object.hierarchy_path != format!("{}/Aura", root.hierarchy_path)
		{
			return Err(ShuRosterTemplateDecodeError::new(
				"conditional root",
				format!(
					"expected one leaf Aura child below {:?}, found name={:?}, path={:?}, children={}",
					root.hierarchy_path,
					aura_object.name,
					aura_object.hierarchy_path,
					aura_object.child_indices.len()
				),
			));
		}
		if root.mono_behaviour_indices.len() != 3
			|| root.component_sources.len() != 4
			|| aura_object.mono_behaviour_indices.len() != 2
			|| aura_object.component_sources.len() != 3
		{
			return Err(ShuRosterTemplateDecodeError::new(
				"conditional root",
				format!(
					"expected root/Aura behavior counts 3/2 and component counts 4/3, found {}/{}/{}/{}",
					root.mono_behaviour_indices.len(),
					aura_object.mono_behaviour_indices.len(),
					root.component_sources.len(),
					aura_object.component_sources.len()
				),
			));
		}
		conditional.push(ConditionalBranchLayout {
			bridge: exact_owned_behavior(
				graph,
				root_index,
				BRIDGE_SCRIPT_ID,
				BRIDGE_OLD_TYPE_HASH,
				"talent bridge",
			)?,
			toggle: exact_owned_behavior(
				graph,
				root_index,
				TOGGLE_SCRIPT_ID,
				TOGGLE_OLD_TYPE_HASH,
				"conditional toggle",
			)?,
			checker: exact_owned_behavior(
				graph,
				root_index,
				PROFESSION_CHECKER_SCRIPT_ID,
				PROFESSION_CHECKER_OLD_TYPE_HASH,
				"profession checker",
			)?,
			aura: exact_owned_behavior(
				graph,
				aura_index,
				AURA_SCRIPT_ID,
				AURA_OLD_TYPE_HASH,
				"global aura",
			)?,
			validator: exact_owned_behavior(
				graph,
				aura_index,
				TARGET_VALIDATOR_SCRIPT_ID,
				TARGET_VALIDATOR_OLD_TYPE_HASH,
				"aura target validator",
			)?,
		});
	}
	if conditional.len() != 2 || deck.len() != 1 {
		return Err(ShuRosterTemplateDecodeError::new(
			"graph roots",
			format!(
				"expected two conditional roots and one deck root, found {} and {}",
				conditional.len(),
				deck.len()
			),
		));
	}
	let distinct_matches: Vec<ConditionalBranchLayout<'_>> = conditional
		.iter()
		.copied()
		.filter(|branch| checker_mode(branch.checker) == Some(ProfessionCheckerMode::Distinct))
		.collect();
	let same_matches: Vec<ConditionalBranchLayout<'_>> = conditional
		.iter()
		.copied()
		.filter(|branch| checker_mode(branch.checker) == Some(ProfessionCheckerMode::Same))
		.collect();
	if distinct_matches.len() != 1 || same_matches.len() != 1 {
		return Err(ShuRosterTemplateDecodeError::new(
			"profession branches",
			format!(
				"expected one distinct and one same-profession checker, found {} and {}",
				distinct_matches.len(),
				same_matches.len()
			),
		));
	}
	let (_deck_root_index, deck_matcher, passive_host) = deck[0];
	Ok(ShuRosterLayout {
		distinct: distinct_matches[0],
		same: same_matches[0],
		deck_matcher,
		passive_host,
	})
}

fn checker_mode(behavior: &BattlePrefabMonoBehaviour) -> Option<ProfessionCheckerMode> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object()?;
	match (
		fields.get("_checkMaxDifferentProfessionCount")?.as_i64()?,
		fields.get("_checkMaxSameProfessionCount")?.as_i64()?,
	) {
		(1, 0) => Some(ProfessionCheckerMode::Distinct),
		(0, 1) => Some(ProfessionCheckerMode::Same),
		_ => None,
	}
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_id: [u8; 16],
	old_type_hash: [u8; 16],
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, ShuRosterTemplateDecodeError> {
	let owner = graph.game_objects.get(owner_index).ok_or_else(|| {
		ShuRosterTemplateDecodeError::new(component, "owner GameObject index is invalid")
	})?;
	let matches: Vec<&BattlePrefabMonoBehaviour> = owner
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index| graph.mono_behaviours.get(index))
		.filter(|behavior| {
			behavior.script_id == Some(script_id) && behavior.old_type_hash == Some(old_type_hash)
		})
		.collect();
	if matches.len() != 1 {
		return Err(ShuRosterTemplateDecodeError::new(
			component,
			format!(
				"expected one behavior with script {} and schema {}, found {}",
				hex_id(script_id),
				hex_id(old_type_hash),
				matches.len()
			),
		));
	}
	let behavior: &BattlePrefabMonoBehaviour = matches[0];
	if behavior.game_object_index != Some(owner_index) {
		return Err(ShuRosterTemplateDecodeError::new(
			component,
			format!(
				"behavior owner is {:?}, expected {owner_index}",
				behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

fn validate_conditional_branch(
	layout: &ConditionalBranchLayout<'_>,
	mode: ProfessionCheckerMode,
) -> Result<(), ShuRosterTemplateDecodeError> {
	validate_bridge(layout.bridge)?;
	validate_toggle(layout.toggle)?;
	validate_checker(layout.checker, mode)?;
	validate_aura(layout.aura, mode)?;
	validate_target_validator(layout.validator)?;
	validate_toggle_edges(layout)?;
	validate_reference_edge(
		layout.aura,
		"$._targetValidator",
		layout.validator,
		"aura validator edge",
	)?;
	for (behavior, component) in [
		(layout.bridge, "talent bridge"),
		(layout.toggle, "conditional toggle"),
		(layout.checker, "profession checker"),
		(layout.aura, "global aura"),
		(layout.validator, "aura target validator"),
	] {
		validate_no_semantic_facts(behavior, component)?;
	}
	Ok(())
}

fn validate_bridge(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), ShuRosterTemplateDecodeError> {
	validate_configuration(
		behavior,
		"talent bridge",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_affectWhenNotRootTalent": 0,
			"_applyBlackboardBySkill": 0,
			"_applyStrBlackboardBySkill": 0,
			"_applyTalentRange": 0,
			"_applyTalentScale": 0,
			"_attachInDummy": 0,
			"_attackBlackboardModeIndex": -1,
			"_checkValidBySkillIndex": [],
			"_defaultModeIndex": -1,
			"_extraAttackBlackboardModeIndices": [],
			"_fromRawDataToSkill": 0,
			"_influenceSkillBlackboard": 0,
			"_overrideDefaultRangeId": 0,
			"_overwriteParentRangeId": 0,
			"_overwriteTalentKey": "",
			"_scaleCertainKeyFlag": 0,
			"_scaleCertainKeyList": [],
			"_scaleCertainMode": 0,
			"_scaleModeIndices": [],
			"_writeRangeIdToProjectileBlackboard": 0
		}),
	)
}

fn validate_toggle(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), ShuRosterTemplateDecodeError> {
	validate_configuration(
		behavior,
		"conditional toggle",
		&["_abilities", "_checker"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [],
			"_buffsWhenToggleOff": [],
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 1,
			"_isInverseToggle": 0,
			"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
			"_selector": {"m_FileID":0, "m_PathID":0},
			"_setToggleFalseOnDetached": 0,
			"_unmanagedBuffsWhenToggleOn": []
		}),
	)
}

fn validate_checker(
	behavior: &BattlePrefabMonoBehaviour,
	mode: ProfessionCheckerMode,
) -> Result<(), ShuRosterTemplateDecodeError> {
	let (different, same): (i64, i64) = match mode {
		ProfessionCheckerMode::Distinct => (1, 0),
		ProfessionCheckerMode::Same => (0, 1),
	};
	validate_configuration(
		behavior,
		"profession checker",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_checkMaxDifferentProfessionCount": different,
			"_checkMaxSameProfessionCount": same,
			"_forceToggleFlag": 0,
			"_minCount": REQUIRED_PROFESSION_COUNT,
			"_professionMask": PROFESSION_MASK
		}),
	)
}

fn validate_aura(
	behavior: &BattlePrefabMonoBehaviour,
	mode: ProfessionCheckerMode,
) -> Result<(), ShuRosterTemplateDecodeError> {
	let (buff_key, attribute_type, formula_item): (&str, i64, i64) = match mode {
		ProfessionCheckerMode::Distinct => (DIFFERENT_PROFESSION_BUFF_KEY, 0, 1),
		ProfessionCheckerMode::Same => (SAME_PROFESSION_BUFF_KEY, 7, 0),
	};
	validate_configuration(
		behavior,
		"global aura",
		&["_targetValidator"],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_buffs": [expected_buff(
				buff_key,
				EMPTY_BUFF_TEMPLATE_KEY,
				attribute_type,
				formula_item,
				2,
				0,
			)],
			"_clearBuffsWhenDisappear": 0,
			"_effects": [],
			"_excludeReborningEntity": 0,
			"_forceAsDmgOrHealAbility": 0,
			"_forceRemoveAllBuffsByKey": 0,
			"_forceTick": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 1,
			"_interval": 0.0,
			"_listenUnitRebornEvent": 0,
			"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
			"_onlyDetactTargetWhenStarted": 0,
			"_onlyDetectCurrentMapLayer": 0,
			"_passiveBuffs": [],
			"_removeBuffIncludeReborning": 0,
			"_removeBuffWhenAbilityDetached": 1,
			"_selector": {"m_FileID":0, "m_PathID":0},
			"_selfOption": 0,
			"_tryRemoveBuffFirstWhenReborn": 0,
			"_useDurableRallyPointLikeSwitch": 0
		}),
	)
}

fn validate_target_validator(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), ShuRosterTemplateDecodeError> {
	validate_configuration(
		behavior,
		"aura target validator",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_targetOptions": {
				"abnormalCombo": 0,
				"abnormalFlag": 0,
				"checkUnitType": 0,
				"containAbnormalFlag": 0,
				"containSomeAbnormalFlags": 0,
				"enableAdvancedOptions": 1,
				"excludeAbnormalFlag": 0,
				"excludeSomeAbnormalFlags": 0,
				"ignoreAllyTargetFree": 0,
				"ignoreHealFree": 1,
				"ignoreTargetFree": 1,
				"ignoreTargetSide": 0,
				"onlyIgnoreSomeOfTargetFreeCase": 0,
				"professionMask": PROFESSION_MASK,
				"purposeMask": 0,
				"targetCategory": 1,
				"targetMotion": 3,
				"targetSide": 1,
				"unitTypeMask": 0
			}
		}),
	)
}

fn expected_buff(
	buff_key: &str,
	template_key: &str,
	attribute_type: i64,
	formula_item: i64,
	lifetime_type: i64,
	trigger_life_type: i64,
) -> Value {
	json!({
		"attributes": {
			"abnormalAntis": [],
			"abnormalComboImmunes": [],
			"abnormalCombos": [],
			"abnormalFlags": [],
			"abnormalImmunes": [],
			"attributeModifiers": [{
				"attributeType": attribute_type,
				"fetchBaseValueFromSourceEntity": 0,
				"formulaItem": formula_item,
				"loadFromBlackboard": 1,
				"value": 0.0
			}]
		},
		"audioSignal": "",
		"blackboard": [],
		"buffKey": buff_key,
		"clearAllStackCntWhenTimeUp": 0,
		"disableOverride": 0,
		"durationKey": "duration",
		"enableInitDirectionFromSource": 0,
		"firstTriggerInterval": -1.0,
		"independentCharacterSource": 0,
		"isDamageMissable": 0,
		"isDurableBuff": 0,
		"isFreezable": 0,
		"isLevitatable": 0,
		"isSilenceable": 0,
		"isStunnable": 0,
		"lifeTime": 0.0,
		"lifeTimeType": lifetime_type,
		"loadFromDB": 0,
		"maxStackCnt": 1,
		"maxValidStackCnt": -1,
		"onEventPriority": 0,
		"overrideEffectKey": "",
		"overrideKey": "",
		"overrideOnEventPriority": 0,
		"overrideType": 0,
		"priority": -1,
		"priorityBBKeys": [],
		"refreshRemainingTimeWhenStackMax": 0,
		"statusResistable": 2,
		"stripBlackboardParamsWithBuffKey": 0,
		"takeSnapshotWhenExtend": 0,
		"templateKey": template_key,
		"triggerCnt": 0,
		"triggerInterval": -1.0,
		"triggerLifeType": trigger_life_type,
		"waitFirstTriggerInterval": 1
	})
}

fn validate_toggle_edges(
	layout: &ConditionalBranchLayout<'_>,
) -> Result<(), ShuRosterTemplateDecodeError> {
	let expected: Vec<(&str, &BattlePrefabObjectSource)> = vec![
		("$._abilities[0]", &layout.aura.source),
		("$._checker", &layout.checker.source),
	];
	if layout.toggle.references.len() != expected.len()
		|| expected.iter().any(|(field, target)| {
			!layout
				.toggle
				.references
				.iter()
				.any(|reference| reference.field_path == *field && reference.target == **target)
		}) {
		return Err(ShuRosterTemplateDecodeError::new(
			"conditional toggle edges",
			format!(
				"expected _checker -> checker and _abilities[0] -> aura, found {:?}",
				layout.toggle.references
			),
		));
	}
	Ok(())
}

fn validate_reference_edge(
	source: &BattlePrefabMonoBehaviour,
	field_path: &str,
	target: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), ShuRosterTemplateDecodeError> {
	if source.references.len() != 1
		|| source.references[0].field_path != field_path
		|| source.references[0].target != target.source
	{
		return Err(ShuRosterTemplateDecodeError::new(
			component,
			format!(
				"expected sole edge {field_path} -> {:?}, found {:?}",
				target.source, source.references
			),
		));
	}
	Ok(())
}

fn validate_deck_matcher(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), ShuRosterTemplateDecodeError> {
	validate_configuration(
		behavior,
		"deck matcher",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_checkValidBySkillIndex": [],
			"_deckBuffs": [{
				"affectInHand": 0,
				"affectOutOfHand": 0,
				"buff": expected_buff(
					ROSTER_TAG_BUFF_KEY,
					PERIODIC_SP_TEMPLATE_KEY,
					1,
					1,
					2,
					2,
				),
				"cardAnimWhenDeckbuffAdd": "",
				"cardEffectType": 0,
				"ignoreSpecialBuild": 0,
				"lifeType": 0,
				"showToastWhenAffect": 0,
				"wontSpawnWhenRallyPointSwitch": 0
			}],
			"_options": {
				"minCntToTrig": REQUIRED_ROSTER_TAG_COUNT,
				"selector": {
					"categoryMask": PROFESSION_MASK,
					"enableOverride": 0,
					"excludeHiddenByCardState": 0,
					"excludeMe": 0,
					"excludeNotInHand": 0,
					"excludeNotShowInCardList": 0,
					"filterTag": SERIALIZED_ROSTER_TAG,
					"mapTags": [],
					"onlySelectMe": 0,
					"subprofessionTag": ""
				},
				"selectorWhenGetBuff": {
					"categoryMask": PROFESSION_MASK,
					"enableOverride": 0,
					"excludeHiddenByCardState": 0,
					"excludeMe": 0,
					"excludeNotInHand": 0,
					"excludeNotShowInCardList": 0,
					"filterTag": "",
					"mapTags": [],
					"onlySelectMe": 0,
					"subprofessionTag": ""
				},
				"useOtherSelectorWhenGetBuff": 1
			},
			"_overwriteParentRangeId": 0,
			"_overwriteTalentKey": ""
		}),
	)?;
	validate_no_semantic_facts(behavior, "deck matcher")?;
	if !behavior.references.is_empty() {
		return Err(ShuRosterTemplateDecodeError::new(
			"deck matcher",
			format!("expected no graph edges, found {:?}", behavior.references),
		));
	}
	Ok(())
}

fn validate_passive_host(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<(), ShuRosterTemplateDecodeError> {
	validate_configuration(
		behavior,
		"passive host",
		&[],
		json!({
			"m_Enabled": 1,
			"m_Name": "",
			"_attachPassiveBuffsOnDummy": 1,
			"_forceAsDmgOrHealAbility": 0,
			"_ignoreIfOwnerDead": 0,
			"_interruptAbilityOnDetach": 0,
			"_metadata": {"blackboardPrefix":"", "namedAsAlias":""},
			"_selector": {"m_FileID":0, "m_PathID":0}
		}),
	)?;
	validate_no_semantic_facts(behavior, "passive host")?;
	if !behavior.references.is_empty() {
		return Err(ShuRosterTemplateDecodeError::new(
			"passive host",
			format!("expected no graph edges, found {:?}", behavior.references),
		));
	}
	Ok(())
}

fn validate_no_semantic_facts(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), ShuRosterTemplateDecodeError> {
	if !behavior.semantic_facts.is_empty() {
		return Err(ShuRosterTemplateDecodeError::new(
			component,
			format!(
				"reviewed unknown script unexpectedly emitted typed facts {:?}",
				behavior
					.semantic_facts
					.iter()
					.map(|fact| fact.kind())
					.collect::<Vec<_>>()
			),
		));
	}
	Ok(())
}

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	dynamic_reference_fields: &[&str],
	expected: Value,
) -> Result<(), ShuRosterTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		ShuRosterTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(ShuRosterTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	for &field in dynamic_reference_fields {
		if actual.remove(field).is_none() {
			return Err(ShuRosterTemplateDecodeError::new(
				component,
				format!("missing graph-reference field {field:?}"),
			));
		}
	}
	let actual: Value = Value::Object(actual);
	if let Some(detail) = first_value_mismatch(&expected, &actual, "$") {
		return Err(ShuRosterTemplateDecodeError::new(component, detail));
	}
	Ok(())
}

fn first_value_mismatch(expected: &Value, actual: &Value, path: &str) -> Option<String> {
	match (expected, actual) {
		(Value::Object(expected), Value::Object(actual)) => {
			for (key, expected_value) in expected {
				let field_path: String = format!("{path}.{key}");
				let Some(actual_value) = actual.get(key) else {
					return Some(format!("missing field {field_path}"));
				};
				if let Some(detail) =
					first_value_mismatch(expected_value, actual_value, &field_path)
				{
					return Some(detail);
				}
			}
			actual
				.keys()
				.find(|key| !expected.contains_key(*key))
				.map(|key| format!("unexpected field {path}.{key}"))
		}
		(Value::Array(expected), Value::Array(actual)) => {
			if expected.len() != actual.len() {
				return Some(format!(
					"{path} expected {} entries, found {}",
					expected.len(),
					actual.len()
				));
			}
			expected
				.iter()
				.zip(actual)
				.enumerate()
				.find_map(|(index, (expected, actual))| {
					first_value_mismatch(expected, actual, &format!("{path}[{index}]"))
				})
		}
		_ if expected == actual => None,
		_ => Some(format!("{path} expected {expected}, found {actual}")),
	}
}

fn build_profession_semantics(
	graph: &BattlePrefabGraph,
	gamedata_revision: &str,
	layout: &ConditionalBranchLayout<'_>,
	mode: ProfessionCheckerMode,
) -> ShuRosterConditionalBranch {
	let (buff_key, condition, effect): (&str, ShuRosterCondition, ShuRosterEffect) = match mode {
		ProfessionCheckerMode::Distinct => (
			DIFFERENT_PROFESSION_BUFF_KEY,
			ShuRosterCondition::DeployedDistinctProfessions {
				minimum_count: REQUIRED_PROFESSION_COUNT,
				serialized_profession_mask: PROFESSION_MASK,
			},
			ShuRosterEffect::StatModifier {
				stat: ShuRosterStat::MaxHp,
				aggregation: ShuRosterModifierAggregation::MultiplicativeBonus,
				amount: SemanticValueExpression::Blackboard(MAX_HP_BLACKBOARD_KEY.to_owned()),
			},
		),
		ProfessionCheckerMode::Same => (
			SAME_PROFESSION_BUFF_KEY,
			ShuRosterCondition::DeployedSameProfession {
				minimum_count: REQUIRED_PROFESSION_COUNT,
				serialized_profession_mask: PROFESSION_MASK,
			},
			ShuRosterEffect::StatModifier {
				stat: ShuRosterStat::AttackSpeed,
				aggregation: ShuRosterModifierAggregation::Additive,
				amount: SemanticValueExpression::Blackboard(ATTACK_SPEED_BLACKBOARD_KEY.to_owned()),
			},
		),
	};
	ShuRosterConditionalBranch {
		mechanic_id: MechanicId::new(format!(
			"{}/template/{buff_key}/roster-threshold-aura",
			graph.logical_path
		)),
		provenance: SemanticProvenance::BattlePrefab {
			mechanics_revision: graph.mechanics_revision.clone(),
			gamedata_revision: gamedata_revision.to_owned(),
			script_id: PROFESSION_CHECKER_SCRIPT_ID,
			old_type_hash: PROFESSION_CHECKER_OLD_TYPE_HASH,
			source: SemanticSourceLocation {
				logical_path: layout.checker.source.logical_path.clone(),
				cab_name: layout.checker.source.cab_name.clone(),
				path_id: layout.checker.source.path_id,
				field_paths: vec![
					"$._checkMaxDifferentProfessionCount".to_owned(),
					"$._checkMaxSameProfessionCount".to_owned(),
					"$._minCount".to_owned(),
					"$._professionMask".to_owned(),
					qualified_field(layout.toggle, "$._checker"),
					qualified_field(layout.toggle, "$._abilities[0]"),
					qualified_field(layout.aura, "$._buffs[0]"),
					qualified_field(layout.aura, "$._removeBuffWhenAbilityDetached"),
					qualified_field(layout.aura, "$._targetValidator"),
					qualified_field(layout.validator, "$._targetOptions"),
				],
			},
		},
		condition,
		targets: ShuRosterTargetScope {
			filter: ShuRosterTargetFilter::ValidatorProfessionMask(PROFESSION_MASK),
			includes_source: true,
			includes_future_deployments: true,
		},
		effects: vec![effect],
		lifetime: ShuRosterBranchLifetime::DynamicWhileConditionSatisfied {
			remove_on_condition_loss: true,
			remove_on_source_detach: true,
		},
	}
}

fn build_roster_tag_semantics(
	graph: &BattlePrefabGraph,
	gamedata_revision: &str,
	layout: &ShuRosterLayout<'_>,
) -> ShuRosterConditionalBranch {
	ShuRosterConditionalBranch {
		mechanic_id: MechanicId::new(format!(
			"{}/template/{ROSTER_TAG_BUFF_KEY}/fixed-roster-tag",
			graph.logical_path
		)),
		provenance: SemanticProvenance::BattlePrefab {
			mechanics_revision: graph.mechanics_revision.clone(),
			gamedata_revision: gamedata_revision.to_owned(),
			script_id: DECK_MATCHER_SCRIPT_ID,
			old_type_hash: DECK_MATCHER_OLD_TYPE_HASH,
			source: SemanticSourceLocation {
				logical_path: layout.deck_matcher.source.logical_path.clone(),
				cab_name: layout.deck_matcher.source.cab_name.clone(),
				path_id: layout.deck_matcher.source.path_id,
				field_paths: vec![
					"$._options.minCntToTrig".to_owned(),
					"$._options.selector".to_owned(),
					"$._options.useOtherSelectorWhenGetBuff".to_owned(),
					"$._options.selectorWhenGetBuff".to_owned(),
					"$._deckBuffs[0]".to_owned(),
					qualified_field(layout.passive_host, "$._interruptAbilityOnDetach"),
				],
			},
		},
		condition: ShuRosterCondition::FixedRosterSerializedTag {
			minimum_count: REQUIRED_ROSTER_TAG_COUNT,
			filter_tag: SERIALIZED_ROSTER_TAG.to_owned(),
			serialized_category_mask: PROFESSION_MASK,
			includes_source: true,
		},
		targets: ShuRosterTargetScope {
			filter: ShuRosterTargetFilter::DeckCategoryMask(PROFESSION_MASK),
			includes_source: true,
			includes_future_deployments: true,
		},
		effects: vec![
			ShuRosterEffect::StatModifier {
				stat: ShuRosterStat::Attack,
				aggregation: ShuRosterModifierAggregation::MultiplicativeBonus,
				amount: SemanticValueExpression::Blackboard(ATTACK_BLACKBOARD_KEY.to_owned()),
			},
			ShuRosterEffect::PeriodicSkillPointGrant {
				interval_seconds: SemanticValueExpression::Blackboard(
					INTERVAL_BLACKBOARD_KEY.to_owned(),
				),
				points: SemanticValueExpression::Blackboard(SKILL_POINT_BLACKBOARD_KEY.to_owned()),
			},
		],
		lifetime: ShuRosterBranchLifetime::FixedRosterForBattle,
	}
}

fn validate_and_collect_issues(
	graph: &BattlePrefabGraph,
	layout: &ShuRosterLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, ShuRosterTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.distinct.bridge, BRIDGE_SCRIPT_ID),
		unknown_issue(layout.distinct.toggle, TOGGLE_SCRIPT_ID),
		unknown_issue(layout.distinct.checker, PROFESSION_CHECKER_SCRIPT_ID),
		unknown_issue(layout.distinct.aura, AURA_SCRIPT_ID),
		unknown_issue(layout.distinct.validator, TARGET_VALIDATOR_SCRIPT_ID),
		unknown_issue(layout.same.bridge, BRIDGE_SCRIPT_ID),
		unknown_issue(layout.same.toggle, TOGGLE_SCRIPT_ID),
		unknown_issue(layout.same.checker, PROFESSION_CHECKER_SCRIPT_ID),
		unknown_issue(layout.same.aura, AURA_SCRIPT_ID),
		unknown_issue(layout.same.validator, TARGET_VALIDATOR_SCRIPT_ID),
		unknown_issue(layout.deck_matcher, DECK_MATCHER_SCRIPT_ID),
		unknown_issue(layout.passive_host, PASSIVE_HOST_SCRIPT_ID),
	];
	expected.sort_by_key(issue_sort_key);
	let mut actual: Vec<BattlePrefabSemanticIssue> = graph.issues.clone();
	actual.sort_by_key(issue_sort_key);
	if actual != expected {
		let missing: Vec<&BattlePrefabSemanticIssue> = expected
			.iter()
			.filter(|issue| !actual.contains(issue))
			.collect();
		let unexpected: Vec<&BattlePrefabSemanticIssue> = actual
			.iter()
			.filter(|issue| !expected.contains(issue))
			.collect();
		return Err(ShuRosterTemplateDecodeError::new(
			"semantic issues",
			format!("missing {missing:?}; unexpected {unexpected:?}"),
		));
	}
	Ok(actual)
}

fn unknown_issue(
	behavior: &BattlePrefabMonoBehaviour,
	script_id: [u8; 16],
) -> BattlePrefabSemanticIssue {
	BattlePrefabSemanticIssue::UnknownScriptId {
		source: behavior.source.clone(),
		script_id,
	}
}

fn issue_sort_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
}

fn qualified_field(behavior: &BattlePrefabMonoBehaviour, field: &str) -> String {
	format!(
		"{}:{}{field}",
		behavior.source.cab_name, behavior.source.path_id
	)
}

fn hex_id(bytes: [u8; 16]) -> String {
	bytes.iter().fold(String::new(), |mut hex, byte| {
		use std::fmt::Write as _;
		write!(hex, "{byte:02x}").expect("writing to String cannot fail");
		hex
	})
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
		PotentialRank,
	};
	use serde_json::json;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab, BattlePrefabGraph, BattlePrefabRootSection,
			BattlePrefabSemanticIssue,
		},
		combat::{MechanicBehavior, ReplayRepeat, StatKind, StatModifierBucket},
		mechanic_semantics::{
			SemanticAction, SemanticBuffModifier, SemanticEffect, SemanticEvent, SemanticLifetime,
			SemanticTeamCompositionCondition, SemanticTrigger, SemanticTriggerFilter,
			SemanticValueExpression,
		},
	};

	use super::{
		first_value_mismatch, try_decode_shu_roster_template, ShuRosterBranchCombination,
		ShuRosterCondition, ShuRosterEffect, ShuRosterModifierAggregation, ShuRosterStat,
		ATTACK_BLACKBOARD_KEY, ATTACK_SPEED_BLACKBOARD_KEY, INTERVAL_BLACKBOARD_KEY,
		MAX_HP_BLACKBOARD_KEY, SERIALIZED_ROSTER_TAG, SKILL_POINT_BLACKBOARD_KEY,
	};

	#[test]
	fn exact_value_comparison_reports_drift_path() {
		let expected = serde_json::json!({"a":{"b":1}});
		let actual = serde_json::json!({"a":{"b":2}});
		assert_eq!(
			first_value_mismatch(&expected, &actual, "$"),
			Some("$.a.b expected 1, found 2".to_owned())
		);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_shu_roster_decodes_three_independent_branches() {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&gamedata_path).expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		let build: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id("char_2025_shu".to_owned()),
			2,
			90,
			200,
			1,
			10,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		);
		let resolved = snapshot
			.operators()
			.resolve(&build)
			.expect("resolve reviewed local build");
		let candidate = resolved
			.selected_talent_candidates()
			.find(|candidate| candidate.prefab_key() == Some("2"))
			.expect("active roster-threshold talent");
		let graph: BattlePrefabGraph = interpret_battle_prefab(
			&prefabs,
			resolved.operator_id(),
			BattlePrefabRootSection::Talents,
			"2",
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed roster-threshold graph");
		assert_eq!(graph.root_game_object_indices.len(), 3);
		assert_eq!(graph.mono_behaviours.len(), 12);
		let decoded = try_decode_shu_roster_template(
			&graph,
			candidate,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect("reviewed template must not drift")
		.expect("reviewed template identity must match");
		assert_eq!(
			decoded.semantics.combination,
			ShuRosterBranchCombination::IndependentAndSimultaneous
		);
		assert_eq!(decoded.consumed_issues.len(), 12);
		assert!(decoded
			.consumed_issues
			.iter()
			.all(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. })));
		let sources: BTreeSet<_> = decoded
			.consumed_issues
			.iter()
			.map(|issue| match issue {
				BattlePrefabSemanticIssue::UnknownScriptId { source, .. } => source,
				_ => unreachable!("asserted exact issue variants"),
			})
			.collect();
		assert_eq!(sources.len(), 12);
		assert!(sources
			.iter()
			.all(|source| source.cab_name == "CAB-3dec954f81087c2cb8ba218cfa37c0f7"));
		assert!(decoded.consumed_semantic_fact_sources.is_empty());

		let distinct = &decoded.semantics.distinct_professions;
		assert_eq!(
			distinct.condition,
			ShuRosterCondition::DeployedDistinctProfessions {
				minimum_count: 3,
				serialized_profession_mask: 639,
			}
		);
		assert_eq!(
			distinct.effects,
			vec![ShuRosterEffect::StatModifier {
				stat: ShuRosterStat::MaxHp,
				aggregation: ShuRosterModifierAggregation::MultiplicativeBonus,
				amount: SemanticValueExpression::Blackboard(MAX_HP_BLACKBOARD_KEY.to_owned()),
			}]
		);
		let same = &decoded.semantics.same_profession;
		assert_eq!(
			same.effects,
			vec![ShuRosterEffect::StatModifier {
				stat: ShuRosterStat::AttackSpeed,
				aggregation: ShuRosterModifierAggregation::Additive,
				amount: SemanticValueExpression::Blackboard(ATTACK_SPEED_BLACKBOARD_KEY.to_owned(),),
			}]
		);
		let roster_tag = &decoded.semantics.roster_tag;
		assert_eq!(
			roster_tag.condition,
			ShuRosterCondition::FixedRosterSerializedTag {
				minimum_count: 4,
				filter_tag: SERIALIZED_ROSTER_TAG.to_owned(),
				serialized_category_mask: 639,
				includes_source: true,
			}
		);
		assert_eq!(roster_tag.effects.len(), 2);
		assert_eq!(
			roster_tag.effects[0],
			ShuRosterEffect::StatModifier {
				stat: ShuRosterStat::Attack,
				aggregation: ShuRosterModifierAggregation::MultiplicativeBonus,
				amount: SemanticValueExpression::Blackboard(ATTACK_BLACKBOARD_KEY.to_owned()),
			}
		);
		assert_eq!(
			roster_tag.effects[1],
			ShuRosterEffect::PeriodicSkillPointGrant {
				interval_seconds: SemanticValueExpression::Blackboard(
					INTERVAL_BLACKBOARD_KEY.to_owned(),
				),
				points: SemanticValueExpression::Blackboard(SKILL_POINT_BLACKBOARD_KEY.to_owned(),),
			}
		);
		assert_eq!(
			distinct.mechanic_id.as_str(),
			"char_2025_shu/template/shu_t_2[different_profession]/roster-threshold-aura"
		);
		assert_eq!(
			same.mechanic_id.as_str(),
			"char_2025_shu/template/shu_t_2[same_profession]/roster-threshold-aura"
		);
		assert_eq!(
			roster_tag.mechanic_id.as_str(),
			"char_2025_shu/template/shu_t_2[sui_count]/fixed-roster-tag"
		);
		let program = decoded.lower_program();
		assert_eq!(program.rules.len(), 3);
		for rule in &program.rules[..2] {
			assert_eq!(
				rule.trigger,
				SemanticTrigger {
					event: SemanticEvent::Deploy,
					filter: SemanticTriggerFilter::AffectedOwner,
					repeat: ReplayRepeat::Once,
				}
			);
			assert_eq!(rule.lifetime, SemanticLifetime::OwnerDeployment);
			assert_eq!(rule.behavior, MechanicBehavior::ConditionalAura);
		}
		assert_eq!(
			program.rules[2].trigger,
			SemanticTrigger {
				event: SemanticEvent::BattleStarted,
				filter: SemanticTriggerFilter::AffectedOwner,
				repeat: ReplayRepeat::Once,
			}
		);
		assert_eq!(program.rules[2].lifetime, SemanticLifetime::Persistent);
		assert_eq!(program.rules[2].behavior, MechanicBehavior::ConditionalAura);
		let SemanticAction::InstallConditionalAura(distinct_aura) = &program.rules[0].action else {
			panic!("distinct-profession branch must install a conditional aura");
		};
		assert_eq!(
			distinct_aura.condition,
			SemanticTeamCompositionCondition::DeployedDistinctProfessions {
				minimum_count: 3,
				profession_mask: 639,
			}
		);
		assert_eq!(distinct_aura.target_profession_mask, 639);
		assert!(distinct_aura.includes_source);
		assert_eq!(distinct_aura.lifetime, SemanticLifetime::OwnerDeployment);
		assert_eq!(
			distinct_aura.modifiers,
			vec![SemanticBuffModifier::StatMultiplier {
				bucket: StatModifierBucket::Panel,
				stat: StatKind::MaxHp,
				multiplier: SemanticValueExpression::OnePlus(Box::new(
					SemanticValueExpression::Blackboard(MAX_HP_BLACKBOARD_KEY.to_owned()),
				)),
			}]
		);
		assert!(distinct_aura.periodic.is_none());
		let SemanticAction::InstallConditionalAura(same_aura) = &program.rules[1].action else {
			panic!("same-profession branch must install a conditional aura");
		};
		assert_eq!(
			same_aura.condition,
			SemanticTeamCompositionCondition::DeployedSameProfession {
				minimum_count: 3,
				profession_mask: 639,
			}
		);
		assert_eq!(
			same_aura.modifiers,
			vec![SemanticBuffModifier::AttackSpeedAddition {
				points: SemanticValueExpression::Blackboard(ATTACK_SPEED_BLACKBOARD_KEY.to_owned(),),
			}]
		);
		assert_eq!(same_aura.lifetime, SemanticLifetime::OwnerDeployment);
		assert!(same_aura.periodic.is_none());
		let SemanticAction::InstallConditionalAura(roster_aura) = &program.rules[2].action else {
			panic!("fixed-roster branch must install a conditional aura");
		};
		assert_eq!(
			roster_aura.condition,
			SemanticTeamCompositionCondition::FixedRosterGroup {
				minimum_count: 4,
				group_id: SERIALIZED_ROSTER_TAG.to_owned(),
			}
		);
		assert_eq!(roster_aura.target_profession_mask, 639);
		assert_eq!(roster_aura.lifetime, SemanticLifetime::Persistent);
		assert_eq!(
			roster_aura.modifiers,
			vec![SemanticBuffModifier::StatMultiplier {
				bucket: StatModifierBucket::Panel,
				stat: StatKind::Attack,
				multiplier: SemanticValueExpression::OnePlus(Box::new(
					SemanticValueExpression::Blackboard(ATTACK_BLACKBOARD_KEY.to_owned()),
				)),
			}]
		);
		let periodic = roster_aura.periodic.as_ref().expect("periodic SP grant");
		assert_eq!(
			periodic.initial_delay_seconds,
			SemanticValueExpression::Blackboard(INTERVAL_BLACKBOARD_KEY.to_owned())
		);
		assert_eq!(periodic.initial_delay_seconds, periodic.interval_seconds);
		assert_eq!(
			periodic.effects,
			vec![SemanticEffect::GainSp {
				amount: SemanticValueExpression::Blackboard(SKILL_POINT_BLACKBOARD_KEY.to_owned()),
			}]
		);

		let mut drifted: BattlePrefabGraph = graph.clone();
		let checker = drifted
			.mono_behaviours
			.iter_mut()
			.find(|behavior| {
				behavior.raw_fields.get("_checkMaxDifferentProfessionCount") == Some(&json!(1))
			})
			.expect("distinct checker");
		checker.raw_fields["_minCount"] = json!(4);
		let drift = try_decode_shu_roster_template(
			&drifted,
			candidate,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect_err("configuration drift must fail closed");
		assert!(drift.to_string().contains("$._minCount"));
	}
}
