//! Fail-closed decoder for the reusable conditional-heal normal-attack skill template.
//!
//! Recognition is based on the official serialized script/schema graph, not an operator or skill
//! ID. Values that legitimately vary by skill level (range, heal scale, SP, and charge count) come
//! from the revision-matched selected-skill metadata.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::data::{
	ArkDataSnapshot, RangeOffset, SelectedSkillMetadata, SkillActivationType, SkillDurationType,
	SkillSpType,
};
use serde_json::{json, Map, Value};

use crate::mechanics::{
	battle_prefab_interpreter::{
		BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemanticIssue,
	},
	combat::{
		ExpectedValue, MechanicId, StatKind, StoredAttackHpRatioComparison,
		StoredAttackReplacementDelivery, StoredAttackReplacementFacts,
		StoredAttackReservationInvalidationAction, StoredAttackReservationInvalidationPolicy,
		StoredAttackTargetOrder, StoredAttackTargetRelationship,
	},
	mechanic_semantics::{SemanticProvenance, SemanticSourceLocation},
	normal_attack::FixedDamage,
	normal_attack::NormalAttackTargetFilter,
};

const COMMON_LOGICAL_PATH: &str = "[uc]skills";
const COMMON_ASSET_DIRECTORY: &str = "dyn/battle/prefabs/[uc]skills";
const SKILL_TRIGGER_NAME: &str = "SkillTrigger";

const EFFECT_HOOK_SCRIPT: &str = "33f2cf8eeccb67cd86fa554b93e178ad";
const EFFECT_HOOK_SCHEMA: &str = "78a80a284e4b68170bc79447219d63c3";
const HEAL_ABILITY_SCRIPT: &str = "4950c56471686a5e08775585353b8d76";
const HEAL_ABILITY_SCHEMA: &str = "60271db1479982f758050a295953aa30";
const SKILL_CONTROLLER_SCRIPT: &str = "e7cf059fe521c4c5150614f9898eb5f0";
const SKILL_CONTROLLER_SCHEMA: &str = "1dc15e2450942db3bbaa3939311b1be1";
const EXTENDABLE_MARKER_SCRIPT: &str = "c385ccbc4766ba332e89aa830d922182";
const EXTENDABLE_MARKER_SCHEMA: &str = "be6ed90d722c123373912b37f0c813ef";
const TARGET_SELECTOR_SCRIPT: &str = "8bb1dcbeee8198241abbd824753dd32b";
const TARGET_SELECTOR_SCHEMA: &str = "a13432461393112b98ffee277e47b572";
const ATTACK_TRIGGER_SCRIPT: &str = "e79f9cdc1738eb7cf612b213533905cb";
const ATTACK_TRIGGER_SCHEMA: &str = "95d0b5928e12e465b31b080fb4682302";

const HEAL_SCALE_BLACKBOARD_KEY: &str = "heal_scale";
const CHARGE_COUNT_BLACKBOARD_KEY: &str = "ct";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuSkillTemplateEvidence {
	pub prefab_sources: Vec<SemanticProvenance>,
	pub selected_skill_id: String,
	pub range_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuSkillHpComparison {
	LessThanOrEqual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuSkillTargetSelection {
	LowestHpRatioAlly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuSkillTargetSelectionTiming {
	ReserveBeforeNormalAttack,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuSkillDelivery {
	ReplaceNextNormalAttack,
}

/// Serialized eligibility settings retained because replay's generic heal predicate is narrower.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShuSkillTargetEligibility {
	pub target_side: i64,
	pub target_category: i64,
	pub target_motion: i64,
	pub abnormal_combo: i64,
	pub abnormal_flag: i64,
	pub exclude_abnormal_flag: i64,
	pub exclude_some_abnormal_flags: bool,
	pub unit_type_mask: i64,
	pub profession_mask: i64,
	pub exclude_owner: bool,
	pub force_ignore_camouflage: bool,
	pub ignore_ally_target_free: bool,
	pub ignore_heal_free: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShuSkillChargeSemantics {
	pub max_charges: u16,
	pub sp_cost: u32,
	pub initial_sp: u32,
	pub sp_increment: ExpectedValue,
	pub recover_sp_if_reserved_target_dies: bool,
}

/// Exact behavior recovered from official prefab and selected-level gamedata evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuSkillTemplateSemantics {
	pub evidence: ShuSkillTemplateEvidence,
	pub range: Vec<RangeOffset>,
	pub range_source_direction: i32,
	pub target_hp_ratio: ExpectedValue,
	pub hp_comparison: ShuSkillHpComparison,
	pub target_selection: ShuSkillTargetSelection,
	pub target_selection_timing: ShuSkillTargetSelectionTiming,
	pub target_eligibility: ShuSkillTargetEligibility,
	pub delivery: ShuSkillDelivery,
	pub heal_multiplier: ExpectedValue,
	pub charges: ShuSkillChargeSemantics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShuSkillSemanticIrRequirement {
	RightFacingRangeProjection,
	SerializedTargetEligibility,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuSkillSemanticIrGap {
	pub requirements: Vec<ShuSkillSemanticIrRequirement>,
}

impl fmt::Display for ShuSkillSemanticIrGap {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		let phrases: Vec<&'static str> = self
			.requirements
			.iter()
			.map(
				|requirement: &ShuSkillSemanticIrRequirement| match requirement {
					ShuSkillSemanticIrRequirement::RightFacingRangeProjection => {
						"a non-standard source direction for projected range offsets"
					}
					ShuSkillSemanticIrRequirement::SerializedTargetEligibility => {
						"the serialized ally/heal-target eligibility flags"
					}
				},
			)
			.collect();
		write!(
			formatter,
			"conditional-heal attack still needs: {}",
			phrases.join("; ")
		)
	}
}

impl Error for ShuSkillSemanticIrGap {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedShuSkillTemplate {
	pub mechanic_id: MechanicId,
	pub semantics: ShuSkillTemplateSemantics,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
	pub consumed_semantic_fact_sources: Vec<BattlePrefabObjectSource>,
}

impl DecodedShuSkillTemplate {
	/// Lowers the reviewed conditional-heal template into the operator-independent stored-attack
	/// runtime primitive. Unknown range orientation or selector shapes remain explicit IR gaps.
	pub fn try_lower_stored_attack_replacement(
		&self,
	) -> Result<StoredAttackReplacementFacts, ShuSkillSemanticIrGap> {
		let mut requirements: Vec<ShuSkillSemanticIrRequirement> = Vec::new();
		if self.semantics.range_source_direction != 1 {
			requirements.push(ShuSkillSemanticIrRequirement::RightFacingRangeProjection);
		}
		if self.semantics.target_eligibility != reviewed_heal_target_eligibility() {
			requirements.push(ShuSkillSemanticIrRequirement::SerializedTargetEligibility);
		}
		if !requirements.is_empty() {
			return Err(ShuSkillSemanticIrGap { requirements });
		}

		let hp_ratio_comparison: StoredAttackHpRatioComparison = match self.semantics.hp_comparison
		{
			ShuSkillHpComparison::LessThanOrEqual => StoredAttackHpRatioComparison::LessThanOrEqual,
		};
		let target_order: StoredAttackTargetOrder = match self.semantics.target_selection {
			ShuSkillTargetSelection::LowestHpRatioAlly => StoredAttackTargetOrder::LowestHpRatio,
		};
		match self.semantics.target_selection_timing {
			ShuSkillTargetSelectionTiming::ReserveBeforeNormalAttack => {}
		}
		match self.semantics.delivery {
			ShuSkillDelivery::ReplaceNextNormalAttack => {}
		}
		let target_death: StoredAttackReservationInvalidationAction =
			if self.semantics.charges.recover_sp_if_reserved_target_dies {
				StoredAttackReservationInvalidationAction::RefundSpentSp
			} else {
				StoredAttackReservationInvalidationAction::ConsumeSpentSp
			};
		// The reviewed controller exposes a recovery branch only for target death. Every other
		// invalidation category is outside that predicate and therefore consumes the spent SP.
		Ok(StoredAttackReplacementFacts {
			range: self.semantics.range.clone(),
			relationship: StoredAttackTargetRelationship::Ally,
			require_healable: true,
			target_filter: NormalAttackTargetFilter::GroundAndAir,
			include_source: !self.semantics.target_eligibility.exclude_owner,
			hp_ratio_comparison,
			hp_ratio_threshold: self.semantics.target_hp_ratio,
			target_order,
			max_charges: self.semantics.charges.max_charges,
			delivery_delay_ticks: 0,
			delivery: StoredAttackReplacementDelivery::SourceStatScaledHealing {
				stat: StatKind::Attack,
				multiplier: self.semantics.heal_multiplier,
			},
			invalidation: StoredAttackReservationInvalidationPolicy {
				target_death,
				target_disappeared: StoredAttackReservationInvalidationAction::ConsumeSpentSp,
				target_out_of_range: StoredAttackReservationInvalidationAction::ConsumeSpentSp,
				target_generation_changed:
					StoredAttackReservationInvalidationAction::ConsumeSpentSp,
			},
		})
	}
}

fn reviewed_heal_target_eligibility() -> ShuSkillTargetEligibility {
	ShuSkillTargetEligibility {
		target_side: 1,
		target_category: 1,
		target_motion: 3,
		abnormal_combo: 2,
		abnormal_flag: 32,
		exclude_abnormal_flag: 45,
		exclude_some_abnormal_flags: false,
		unit_type_mask: 0,
		profession_mask: 0,
		exclude_owner: false,
		force_ignore_camouflage: false,
		ignore_ally_target_free: false,
		ignore_heal_free: false,
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShuSkillTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl ShuSkillTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for ShuSkillTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"conditional-heal attack template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for ShuSkillTemplateDecodeError {}

/// Decodes the official conditional-heal attack template without operator-ID branching.
pub fn try_decode_shu_skill_template(
	graph: &BattlePrefabGraph,
	selected_skill: &SelectedSkillMetadata,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedShuSkillTemplate>, ShuSkillTemplateDecodeError> {
	if !claims_conditional_heal_attack_template(graph) {
		return Ok(None);
	}
	validate_revisions(graph, snapshot, gamedata_revision)?;
	let selected: SelectedSkillEvidence = validate_selected_skill(selected_skill, graph, snapshot)?;
	let layout: ShuSkillLayout<'_> = validate_graph_layout(graph)?;
	let target_hp_ratio: ExpectedValue = validate_configurations(&layout)?;
	validate_edges(graph, &layout)?;
	validate_no_semantic_facts_or_buffs(graph)?;
	let consumed_issues: Vec<BattlePrefabSemanticIssue> = validate_exact_issues(graph, &layout)?;
	let evidence: ShuSkillTemplateEvidence = ShuSkillTemplateEvidence {
		prefab_sources: prefab_provenance(graph, &layout, gamedata_revision),
		selected_skill_id: selected_skill.skill_id().to_owned(),
		range_id: selected.range_id,
	};
	let semantics: ShuSkillTemplateSemantics = ShuSkillTemplateSemantics {
		evidence,
		range: selected.range,
		range_source_direction: selected.range_source_direction,
		target_hp_ratio,
		hp_comparison: ShuSkillHpComparison::LessThanOrEqual,
		target_selection: ShuSkillTargetSelection::LowestHpRatioAlly,
		target_selection_timing: ShuSkillTargetSelectionTiming::ReserveBeforeNormalAttack,
		target_eligibility: reviewed_heal_target_eligibility(),
		delivery: ShuSkillDelivery::ReplaceNextNormalAttack,
		heal_multiplier: selected.heal_multiplier,
		charges: ShuSkillChargeSemantics {
			max_charges: selected.max_charges,
			sp_cost: selected.sp_cost,
			initial_sp: selected.initial_sp,
			sp_increment: selected.sp_increment,
			recover_sp_if_reserved_target_dies: true,
		},
	};

	Ok(Some(DecodedShuSkillTemplate {
		mechanic_id: MechanicId::new(format!(
			"{}/template/conditional-heal-attack",
			selected_skill.skill_id()
		)),
		semantics,
		consumed_issues,
		consumed_semantic_fact_sources: Vec::new(),
	}))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedSkillEvidence {
	range_id: String,
	range: Vec<RangeOffset>,
	range_source_direction: i32,
	heal_multiplier: ExpectedValue,
	max_charges: u16,
	sp_cost: u32,
	initial_sp: u32,
	sp_increment: ExpectedValue,
}

#[derive(Clone, Copy)]
struct ShuSkillLayout<'a> {
	effect_hook: &'a BattlePrefabMonoBehaviour,
	heal_ability: &'a BattlePrefabMonoBehaviour,
	controller: &'a BattlePrefabMonoBehaviour,
	extendable_marker: &'a BattlePrefabMonoBehaviour,
	target_selector: &'a BattlePrefabMonoBehaviour,
	attack_trigger: &'a BattlePrefabMonoBehaviour,
}

pub(crate) fn claims_conditional_heal_attack_template(graph: &BattlePrefabGraph) -> bool {
	let script_id: [u8; 16] = parse_id(HEAL_ABILITY_SCRIPT);
	graph
		.mono_behaviours
		.iter()
		.any(|behavior: &BattlePrefabMonoBehaviour| behavior.script_id == Some(script_id))
}

fn validate_revisions(
	graph: &BattlePrefabGraph,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<(), ShuSkillTemplateDecodeError> {
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(ShuSkillTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.mechanics_revision.is_empty() {
		return Err(ShuSkillTemplateDecodeError::new(
			"mechanics revision",
			"official prefab graph has an empty mechanics revision",
		));
	}
	Ok(())
}

fn validate_selected_skill(
	selected_skill: &SelectedSkillMetadata,
	graph: &BattlePrefabGraph,
	snapshot: &ArkDataSnapshot,
) -> Result<SelectedSkillEvidence, ShuSkillTemplateDecodeError> {
	let prefab_id: &str = selected_skill.prefab_id().ok_or_else(|| {
		ShuSkillTemplateDecodeError::new("selected skill", "missing common skill prefab ID")
	})?;
	if prefab_id != graph.prefab_name {
		return Err(ShuSkillTemplateDecodeError::new(
			"selected skill",
			format!(
				"prefab ID {prefab_id:?} does not match graph root {:?}",
				graph.prefab_name
			),
		));
	}
	if selected_skill.override_prefab_key().is_some()
		|| selected_skill.override_token_key().is_some()
	{
		return Err(ShuSkillTemplateDecodeError::new(
			"selected skill",
			format!(
				"expected no prefab/token override, found prefab={:?}, token={:?}",
				selected_skill.override_prefab_key(),
				selected_skill.override_token_key()
			),
		));
	}
	if selected_skill.activation_type() != &SkillActivationType::Auto
		|| selected_skill.duration_type() != &SkillDurationType::None
		|| selected_skill.duration_seconds() != 0.0
	{
		return Err(ShuSkillTemplateDecodeError::new(
			"selected skill lifecycle",
			format!(
				"expected AUTO/NONE/0, found {:?}/{:?}/{}",
				selected_skill.activation_type(),
				selected_skill.duration_type(),
				selected_skill.duration_seconds()
			),
		));
	}
	let range_id: &str = selected_skill.range_id().ok_or_else(|| {
		ShuSkillTemplateDecodeError::new("selected skill range", "missing range ID")
	})?;
	let range = snapshot.resolve_range(range_id).map_err(|error| {
		ShuSkillTemplateDecodeError::new(
			"selected skill range",
			format!("cannot resolve {range_id:?}: {error}"),
		)
	})?;

	let sp = selected_skill.sp();
	if sp.sp_type() != &SkillSpType::IncreaseWithTime
		|| sp.max_charges() == 0
		|| sp.max_charges() > u32::from(u16::MAX)
		|| !sp.increment().is_finite()
		|| sp.increment() <= 0.0
		|| sp.initial() > sp.cost()
	{
		return Err(ShuSkillTemplateDecodeError::new(
			"selected skill SP",
			format!(
				"expected positive time recovery, 1..=u16::MAX charges, and initial<=cost; found {:?}, max={}, increment={}, initial={}, cost={}",
				sp.sp_type(),
				sp.max_charges(),
				sp.increment(),
				sp.initial(),
				sp.cost()
			),
		));
	}

	let mut blackboard: BTreeMap<&str, (f64, Option<&str>)> = BTreeMap::new();
	for entry in selected_skill.blackboard() {
		if blackboard
			.insert(entry.key(), (entry.value(), entry.value_string()))
			.is_some()
		{
			return Err(ShuSkillTemplateDecodeError::new(
				"selected skill blackboard",
				format!("duplicate key {:?}", entry.key()),
			));
		}
	}
	let expected_keys: Vec<&str> = vec![CHARGE_COUNT_BLACKBOARD_KEY, HEAL_SCALE_BLACKBOARD_KEY];
	if blackboard.keys().copied().collect::<Vec<_>>() != expected_keys {
		return Err(ShuSkillTemplateDecodeError::new(
			"selected skill blackboard",
			format!(
				"expected exactly {expected_keys:?}, found {:?}",
				blackboard.keys().collect::<Vec<_>>()
			),
		));
	}
	for key in [CHARGE_COUNT_BLACKBOARD_KEY, HEAL_SCALE_BLACKBOARD_KEY] {
		let (value, value_string): (f64, Option<&str>) = blackboard[key];
		if !value.is_finite() || value <= 0.0 || value_string.is_some() {
			return Err(ShuSkillTemplateDecodeError::new(
				"selected skill blackboard",
				format!(
					"entry {key:?} must be finite, positive, and string-free; found {value}, {value_string:?}"
				),
			));
		}
	}
	let charge_count: f64 = blackboard[CHARGE_COUNT_BLACKBOARD_KEY].0;
	if charge_count.fract() != 0.0 || charge_count as u32 != sp.max_charges() {
		return Err(ShuSkillTemplateDecodeError::new(
			"selected skill charges",
			format!(
				"blackboard charge count {charge_count} does not equal max charge count {}",
				sp.max_charges()
			),
		));
	}

	Ok(SelectedSkillEvidence {
		range_id: range_id.to_owned(),
		range: range.offsets().to_vec(),
		range_source_direction: range.source_direction(),
		heal_multiplier: expected_value_from_f64(
			blackboard[HEAL_SCALE_BLACKBOARD_KEY].0,
			"selected skill heal scale",
		)?,
		max_charges: sp.max_charges() as u16,
		sp_cost: sp.cost(),
		initial_sp: sp.initial(),
		sp_increment: expected_value_from_f64(sp.increment(), "selected skill SP increment")?,
	})
}

fn validate_graph_layout(
	graph: &BattlePrefabGraph,
) -> Result<ShuSkillLayout<'_>, ShuSkillTemplateDecodeError> {
	let expected_asset_path: String =
		format!("{COMMON_ASSET_DIRECTORY}/{}.prefab", graph.prefab_name);
	if graph.logical_path != COMMON_LOGICAL_PATH
		|| graph.asset_path != expected_asset_path
		|| graph.section != BattlePrefabRootSection::MainAsset
	{
		return Err(ShuSkillTemplateDecodeError::new(
			"common root identity",
			format!(
				"expected logical path {COMMON_LOGICAL_PATH:?}, asset {expected_asset_path:?}, MainAsset; found {:?}, {:?}, {:?}",
				graph.logical_path, graph.asset_path, graph.section
			),
		));
	}
	if graph.root_game_object_indices.len() != 1
		|| graph.game_objects.len() != 2
		|| graph.mono_behaviours.len() != 6
	{
		return Err(ShuSkillTemplateDecodeError::new(
			"common graph shape",
			format!(
				"expected one root, two GameObjects, and six behaviors; found roots={}, GameObjects={}, behaviors={}",
				graph.root_game_object_indices.len(),
				graph.game_objects.len(),
				graph.mono_behaviours.len()
			),
		));
	}
	let root_index: usize = graph.root_game_object_indices[0];
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		ShuSkillTemplateDecodeError::new(
			"common graph root",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	let trigger_index: usize =
		exact_child(graph, root_index, SKILL_TRIGGER_NAME, "skill-trigger child")?;
	let trigger = &graph.game_objects[trigger_index];
	if root.name != graph.prefab_name
		|| root.hierarchy_path != graph.prefab_name
		|| root.mono_behaviour_indices.len() != 3
		|| root.component_sources.len() != 4
		|| trigger.mono_behaviour_indices.len() != 3
		|| trigger.component_sources.len() != 4
		|| !trigger.child_indices.is_empty()
	{
		return Err(ShuSkillTemplateDecodeError::new(
			"common graph hierarchy",
			format!(
				"expected root {:?} and leaf SkillTrigger with three behaviors each; found root name={:?}, path={:?}, behavior counts={}/{}, component counts={}/{}, trigger children={}",
				graph.prefab_name,
				root.name,
				root.hierarchy_path,
				root.mono_behaviour_indices.len(),
				trigger.mono_behaviour_indices.len(),
				root.component_sources.len(),
				trigger.component_sources.len(),
				trigger.child_indices.len()
			),
		));
	}

	Ok(ShuSkillLayout {
		effect_hook: exact_owned_behavior(
			graph,
			root_index,
			EFFECT_HOOK_SCRIPT,
			EFFECT_HOOK_SCHEMA,
			"effect hook",
		)?,
		heal_ability: exact_owned_behavior(
			graph,
			root_index,
			HEAL_ABILITY_SCRIPT,
			HEAL_ABILITY_SCHEMA,
			"heal ability",
		)?,
		controller: exact_owned_behavior(
			graph,
			root_index,
			SKILL_CONTROLLER_SCRIPT,
			SKILL_CONTROLLER_SCHEMA,
			"skill controller",
		)?,
		extendable_marker: exact_owned_behavior(
			graph,
			trigger_index,
			EXTENDABLE_MARKER_SCRIPT,
			EXTENDABLE_MARKER_SCHEMA,
			"extendable marker",
		)?,
		target_selector: exact_owned_behavior(
			graph,
			trigger_index,
			TARGET_SELECTOR_SCRIPT,
			TARGET_SELECTOR_SCHEMA,
			"target selector",
		)?,
		attack_trigger: exact_owned_behavior(
			graph,
			trigger_index,
			ATTACK_TRIGGER_SCRIPT,
			ATTACK_TRIGGER_SCHEMA,
			"attack trigger",
		)?,
	})
}

fn exact_child(
	graph: &BattlePrefabGraph,
	owner_index: usize,
	name: &str,
	component: &'static str,
) -> Result<usize, ShuSkillTemplateDecodeError> {
	let owner = &graph.game_objects[owner_index];
	if owner.child_indices.len() != 1 {
		return Err(ShuSkillTemplateDecodeError::new(
			component,
			format!(
				"expected one child named {name:?}, found {} children",
				owner.child_indices.len()
			),
		));
	}
	let child_index: usize = owner.child_indices[0];
	let child = graph.game_objects.get(child_index).ok_or_else(|| {
		ShuSkillTemplateDecodeError::new(
			component,
			format!("child index {child_index} is outside the GameObject table"),
		)
	})?;
	if child.name != name || child.hierarchy_path != format!("{}/{name}", owner.hierarchy_path) {
		return Err(ShuSkillTemplateDecodeError::new(
			component,
			format!(
				"expected child {name:?} below {:?}, found name={:?}, path={:?}",
				owner.hierarchy_path, child.name, child.hierarchy_path
			),
		));
	}
	Ok(child_index)
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_hex: &str,
	schema_hex: &str,
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, ShuSkillTemplateDecodeError> {
	let script_id: [u8; 16] = parse_id(script_hex);
	let old_type_hash: [u8; 16] = parse_id(schema_hex);
	let owner = &graph.game_objects[owner_index];
	let matches: Vec<&BattlePrefabMonoBehaviour> = owner
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index: &usize| graph.mono_behaviours.get(index))
		.filter(|behavior: &&BattlePrefabMonoBehaviour| {
			behavior.script_id == Some(script_id) && behavior.old_type_hash == Some(old_type_hash)
		})
		.collect();
	if matches.len() != 1 {
		return Err(ShuSkillTemplateDecodeError::new(
			component,
			format!(
				"expected one behavior with script {script_hex} and schema {schema_hex}, found {}",
				matches.len()
			),
		));
	}
	let behavior: &BattlePrefabMonoBehaviour = matches[0];
	if behavior.game_object_index != Some(owner_index) {
		return Err(ShuSkillTemplateDecodeError::new(
			component,
			format!(
				"expected behavior owned by GameObject {owner_index}, found {:?}",
				behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

fn validate_configurations(
	layout: &ShuSkillLayout<'_>,
) -> Result<ExpectedValue, ShuSkillTemplateDecodeError> {
	let threshold: f64 = numeric_field(
		layout.target_selector,
		"_maxHpRatio",
		"target-selector threshold",
	)?;
	if !(0.0..=1.0).contains(&threshold) {
		return Err(ShuSkillTemplateDecodeError::new(
			"target-selector threshold",
			format!("$._maxHpRatio must be in [0, 1], found {threshold}"),
		));
	}
	let cast_effect: String = nonempty_nested_string(
		layout.effect_hook,
		&["_castEffects", "0", "_effects", "0"],
		"effect hook",
	)?;
	let hit_effect: String = nonempty_nested_string(
		layout.effect_hook,
		&["_hitEffects", "0", "_effects", "0"],
		"effect hook",
	)?;

	let mut effect_hook_configuration: Value = expected_configuration(
		r#"{
			"_attachEffects": [],
			"_castEffects": [{
				"_delayIfFirstAttack": 0.0,
				"_directionType": 0,
				"_effects": ["__CAST_EFFECT__"],
				"_endEvent": 3,
				"_hookEffectAfterFirstAttack": "",
				"_oneshot": 0,
				"_onlyOnce": 0,
				"_startEvent": 2,
				"_useFourDirectionalFace": 0
			}],
			"_castTargetEffects": [],
			"_convertCastEndToAttachFinish": 0,
			"_hitEffects": [{
				"_directionType": 0,
				"_effects": ["__HIT_EFFECT__"],
				"_endEvent": 3,
				"_oneshot": 1,
				"_onlyOnce": 0,
				"_playIfNotInputTarget": 0,
				"_playOnlyIfInputTarget": 0,
				"_useFourDirectionalFace": 0,
				"_useOwnerToTargetDirection": 0
			}],
			"_inputTargetEffects": [],
			"_preDelayFactor": 0.0,
			"_recalculatePlaybackSpeed": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}"#,
	);
	effect_hook_configuration["_castEffects"][0]["_effects"][0] = Value::String(cast_effect);
	effect_hook_configuration["_hitEffects"][0]["_effects"][0] = Value::String(hit_effect);
	validate_configuration(
		layout.effect_hook,
		"effect-hook configuration",
		effect_hook_configuration,
	)?;
	let mut heal_ability_configuration: Value = expected_configuration(
		r#"{
			"_activeBuffs": [],
			"_affectedBySlowDown": 0,
			"_allowNoTarget": 0,
			"_alwaysIncludeTarget": 1,
			"_animKey": "Attack_2",
			"_applyEPHeal": 0,
			"_applyHealScaleToEPHealScale": 0,
			"_attachPassiveBuffsOnDummy": 1,
			"_checkFaceToFirstCastTargetDownAttack": 0,
			"_cooldown": 0.0,
			"_cooldownKey": "duration",
			"_downAnimKey": "",
			"_downEndAnimKey": "",
			"_endAnimKey": "Idle",
			"_escapeTime": 0.0,
			"_faceToDefault": 0,
			"_faceToFirstCastTarget": 0,
			"_faceToFront": 0,
			"_faceToTarget": 1,
			"_forceAsDmgOrHealAbility": 0,
			"_forceFaceToDirection": 0,
			"_hasNoActionNode": 0,
			"_ignoreHealFree": 0,
			"_ignoreIfOwnerDead": 0,
			"_ignorePalsyInterrupt": 0,
			"_interruptAbilityOnDetach": 1,
			"_interuptIfTargetDead": 0,
			"_isCont": 0,
			"_isHpRatio": 0,
			"_maxAnimScale": 1.0,
			"_metadata": {"blackboardPrefix": "", "namedAsAlias": ""},
			"_onlyPlayEndAnimWhenIsLastAbility": 0,
			"_preDelay": 0.0,
			"_resetCdStrategy": 1,
			"_selectTargetSource": 2,
			"_selectTargetTiming": 0,
			"_selector": {"m_FileID": 0, "m_PathID": 0},
			"_timeMode": 0,
			"_upAnimKey": "",
			"_upEndAnimKey": "",
			"_waitForAttackEvent": 1,
			"m_Enabled": 1,
			"m_Name": ""
		}"#,
	);
	heal_ability_configuration["_selector"]["m_PathID"] =
		Value::from(layout.target_selector.source.path_id);
	validate_configuration(
		layout.heal_ability,
		"heal-ability configuration",
		heal_ability_configuration,
	)?;
	let mut controller_configuration: Value = expected_configuration(
		r#"{
			"_allowSpRecoveryWhenAffecting": 0,
			"_attachSignalIds": 0,
			"_attackBlackboardModeIndex": -1,
			"_availableValidator": {"m_FileID": 0, "m_PathID": 0},
			"_beginAnim": "",
			"_beginAnimDown": "",
			"_beginAnimValidator": {"m_FileID": 0, "m_PathID": 0},
			"_beginEffect": [],
			"_canCastWithNoSp": 0,
			"_canSilenced": 0,
			"_canUseInAbnormalState": 0,
			"_cancelIfSearchTargetFailed": 1,
			"_defaultModeIndex": -1,
			"_earlySkillFinishAtAttackFinished": 0,
			"_extraAbilities": [],
			"_extraAttackBlackboardModeIndices": [],
			"_extraRangeIdModeIndices": [],
			"_hidden": 0,
			"_hideHudSkillStackCount": 0,
			"_hideSkillStackCount": 0,
			"_limitGlobalTriggerTime": 0,
			"_maxTriggerTime": -1,
			"_playCharWordVoice": 1,
			"_playCharWordWithPassiveType": 0,
			"_playSkillBeginAnim": 0,
			"_rangeIdModeIndex": -1,
			"_rangeIdUsageType": 7,
			"_rangeToShow": {"m_FileID": 0, "m_PathID": 0},
			"_recoverSpIfTargetDead": 1,
			"_showProgressToNext": 0,
			"_showSpAsBulletMode": 0,
			"_showSpBarEvenHidden": 0,
			"_showSpRatioAsProgress": 0,
			"_trigger": {"m_FileID": 0, "m_PathID": 0},
			"_useEscapeTime": 0,
			"_useFaceVector": 0,
			"_writeDurationToAttackBlackboard": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}"#,
	);
	controller_configuration["_trigger"]["m_PathID"] =
		Value::from(layout.attack_trigger.source.path_id);
	validate_configuration(
		layout.controller,
		"skill-controller configuration",
		controller_configuration,
	)?;
	validate_configuration(
		layout.extendable_marker,
		"extendable-marker configuration",
		json!({"_extendable": 0, "m_Enabled": 1, "m_Name": ""}),
	)?;
	let mut target_selector_configuration: Value = expected_configuration(
		r#"{
			"_abilityInputTargetAsSortSource": 0,
			"_abnormalCombo": 2,
			"_abnormalFlag": 32,
			"_checkUnitType": 0,
			"_excludeAbnormalFlag": 45,
			"_excludeOwner": 0,
			"_excludeSomeAbnormalFlags": 0,
			"_filerMaxHp": 1,
			"_filterMinHp": 0,
			"_forceIgnoreCamouflage": 0,
			"_ignoreAllyTargetFree": 0,
			"_ignoreHealFree": 0,
			"_ignoreHitRange": 0,
			"_ignoreTargetFree": 0,
			"_limitTargetNum": 1,
			"_maxHpExcludeEqual": 0,
			"_maxHpRatio": 0.0,
			"_maxNum": 1,
			"_maxTargetKey": "max_target",
			"_minHpRatio": 0.0,
			"_needProfessionMask": 0,
			"_onlyIgnoreSomeOfTargetFreeCase": 0,
			"_postFilter": 3,
			"_professionMask": 0,
			"_sortByTauntAtLast": 0,
			"_targetCategory": 1,
			"_targetMotion": 3,
			"_targetSide": 1,
			"_unitTypeMask": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}"#,
	);
	target_selector_configuration["_maxHpRatio"] = json!(threshold);
	validate_configuration(
		layout.target_selector,
		"target-selector configuration",
		target_selector_configuration,
	)?;
	validate_configuration(
		layout.attack_trigger,
		"attack-trigger configuration",
		json!({
			"_keepTarget": 0,
			"_minTargetNum": 1,
			"_overrideSearchTargetTick": -1,
			"m_Enabled": 1,
			"m_Name": ""
		}),
	)?;

	expected_value_from_f64(threshold, "target-selector threshold")
}

fn expected_configuration(serialized: &str) -> Value {
	serde_json::from_str(serialized)
		.expect("internal official prefab configuration must be valid JSON")
}

fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	expected: Value,
) -> Result<(), ShuSkillTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		ShuSkillTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(ShuSkillTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	let actual: Value = Value::Object(actual);
	if let Some(detail) = first_value_mismatch(&expected, &actual, "$") {
		return Err(ShuSkillTemplateDecodeError::new(component, detail));
	}
	Ok(())
}

fn numeric_field(
	behavior: &BattlePrefabMonoBehaviour,
	field: &str,
	component: &'static str,
) -> Result<f64, ShuSkillTemplateDecodeError> {
	behavior
		.raw_fields
		.get(field)
		.and_then(Value::as_f64)
		.filter(|value: &f64| value.is_finite())
		.ok_or_else(|| {
			ShuSkillTemplateDecodeError::new(
				component,
				format!("$.{field} must be a finite number"),
			)
		})
}

fn nonempty_nested_string(
	behavior: &BattlePrefabMonoBehaviour,
	path: &[&str],
	component: &'static str,
) -> Result<String, ShuSkillTemplateDecodeError> {
	let mut value: &Value = &behavior.raw_fields;
	let mut rendered: String = "$".to_owned();
	for segment in path {
		let next: Option<&Value> = if let Ok(index) = segment.parse::<usize>() {
			rendered.push_str(&format!("[{index}]"));
			value.get(index)
		} else {
			rendered.push('.');
			rendered.push_str(segment);
			value.get(*segment)
		};
		value = next.ok_or_else(|| {
			ShuSkillTemplateDecodeError::new(component, format!("missing string {rendered}"))
		})?;
	}
	let text: &str = value
		.as_str()
		.filter(|text: &&str| !text.is_empty())
		.ok_or_else(|| {
			ShuSkillTemplateDecodeError::new(
				component,
				format!("{rendered} must be non-empty text"),
			)
		})?;
	Ok(text.to_owned())
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
				.find(|key: &&String| !expected.contains_key(*key))
				.map(|key: &String| format!("unexpected field {path}.{key}"))
		}
		(Value::Array(expected), Value::Array(actual)) => {
			if expected.len() != actual.len() {
				return Some(format!(
					"{path} expected {} entries, found {}",
					expected.len(),
					actual.len()
				));
			}
			expected.iter().zip(actual).enumerate().find_map(
				|(index, (expected, actual)): (usize, (&Value, &Value))| {
					first_value_mismatch(expected, actual, &format!("{path}[{index}]"))
				},
			)
		}
		_ if expected == actual => None,
		_ => Some(format!("{path} expected {expected}, found {actual}")),
	}
}

fn validate_edges(
	graph: &BattlePrefabGraph,
	layout: &ShuSkillLayout<'_>,
) -> Result<(), ShuSkillTemplateDecodeError> {
	validate_single_edge(
		graph,
		layout.heal_ability,
		"$._selector",
		layout.target_selector,
		"heal selector edge",
	)?;
	validate_single_edge(
		graph,
		layout.controller,
		"$._trigger",
		layout.attack_trigger,
		"skill trigger edge",
	)?;
	for (component, behavior) in [
		("effect hook", layout.effect_hook),
		("extendable marker", layout.extendable_marker),
		("target selector", layout.target_selector),
		("attack trigger", layout.attack_trigger),
	] {
		if !behavior.references.is_empty() || !behavior.referenced_mono_behaviour_indices.is_empty()
		{
			return Err(ShuSkillTemplateDecodeError::new(
				component,
				format!(
					"expected no non-null graph edges, found references={:?}, behavior indices={:?}",
					behavior.references, behavior.referenced_mono_behaviour_indices
				),
			));
		}
	}
	Ok(())
}

fn validate_single_edge(
	graph: &BattlePrefabGraph,
	owner: &BattlePrefabMonoBehaviour,
	field_path: &str,
	target: &BattlePrefabMonoBehaviour,
	component: &'static str,
) -> Result<(), ShuSkillTemplateDecodeError> {
	if owner.references.len() != 1
		|| owner.references[0].field_path != field_path
		|| owner.references[0].target != target.source
	{
		return Err(ShuSkillTemplateDecodeError::new(
			component,
			format!(
				"expected sole {field_path} edge to {:?}, found {:?}",
				target.source, owner.references
			),
		));
	}
	let target_index: usize = graph
		.mono_behaviours
		.iter()
		.position(|behavior: &BattlePrefabMonoBehaviour| behavior.source == target.source)
		.expect("validated target behavior is in the graph");
	if owner.referenced_mono_behaviour_indices != vec![target_index] {
		return Err(ShuSkillTemplateDecodeError::new(
			component,
			format!(
				"expected sole referenced behavior index {target_index}, found {:?}",
				owner.referenced_mono_behaviour_indices
			),
		));
	}
	Ok(())
}

fn validate_no_semantic_facts_or_buffs(
	graph: &BattlePrefabGraph,
) -> Result<(), ShuSkillTemplateDecodeError> {
	for behavior in &graph.mono_behaviours {
		if !behavior.semantic_facts.is_empty() || !behavior.buffs.is_empty() {
			return Err(ShuSkillTemplateDecodeError::new(
				"semantic fact consumption",
				format!(
					"unknown script {:?} unexpectedly emitted facts={:?} or buffs={:?}",
					behavior.source,
					behavior
						.semantic_facts
						.iter()
						.map(|fact| fact.kind())
						.collect::<Vec<_>>(),
					behavior.buffs
				),
			));
		}
	}
	Ok(())
}

fn validate_exact_issues(
	graph: &BattlePrefabGraph,
	layout: &ShuSkillLayout<'_>,
) -> Result<Vec<BattlePrefabSemanticIssue>, ShuSkillTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.effect_hook, EFFECT_HOOK_SCRIPT),
		unknown_issue(layout.heal_ability, HEAL_ABILITY_SCRIPT),
		unknown_issue(layout.controller, SKILL_CONTROLLER_SCRIPT),
		unknown_issue(layout.extendable_marker, EXTENDABLE_MARKER_SCRIPT),
		unknown_issue(layout.target_selector, TARGET_SELECTOR_SCRIPT),
		unknown_issue(layout.attack_trigger, ATTACK_TRIGGER_SCRIPT),
	];
	expected.sort_by_key(issue_sort_key);
	let mut actual: Vec<BattlePrefabSemanticIssue> = graph.issues.clone();
	actual.sort_by_key(issue_sort_key);
	if actual != expected {
		let missing: Vec<&BattlePrefabSemanticIssue> = expected
			.iter()
			.filter(|issue: &&BattlePrefabSemanticIssue| !actual.contains(*issue))
			.collect();
		let unexpected: Vec<&BattlePrefabSemanticIssue> = actual
			.iter()
			.filter(|issue: &&BattlePrefabSemanticIssue| !expected.contains(*issue))
			.collect();
		return Err(ShuSkillTemplateDecodeError::new(
			"semantic issues",
			format!("missing {missing:?}; unexpected {unexpected:?}"),
		));
	}
	Ok(actual)
}

fn unknown_issue(
	behavior: &BattlePrefabMonoBehaviour,
	script_hex: &str,
) -> BattlePrefabSemanticIssue {
	BattlePrefabSemanticIssue::UnknownScriptId {
		source: behavior.source.clone(),
		script_id: parse_id(script_hex),
	}
}

fn prefab_provenance(
	graph: &BattlePrefabGraph,
	layout: &ShuSkillLayout<'_>,
	gamedata_revision: &str,
) -> Vec<SemanticProvenance> {
	[
		(
			layout.effect_hook,
			EFFECT_HOOK_SCRIPT,
			EFFECT_HOOK_SCHEMA,
			vec!["$._castEffects".to_owned(), "$._hitEffects".to_owned()],
		),
		(
			layout.heal_ability,
			HEAL_ABILITY_SCRIPT,
			HEAL_ABILITY_SCHEMA,
			vec![
				"$._selector".to_owned(),
				"$._selectTargetTiming".to_owned(),
				"$._waitForAttackEvent".to_owned(),
			],
		),
		(
			layout.controller,
			SKILL_CONTROLLER_SCRIPT,
			SKILL_CONTROLLER_SCHEMA,
			vec![
				"$._trigger".to_owned(),
				"$._rangeIdUsageType".to_owned(),
				"$._recoverSpIfTargetDead".to_owned(),
			],
		),
		(
			layout.extendable_marker,
			EXTENDABLE_MARKER_SCRIPT,
			EXTENDABLE_MARKER_SCHEMA,
			vec!["$._extendable".to_owned()],
		),
		(
			layout.target_selector,
			TARGET_SELECTOR_SCRIPT,
			TARGET_SELECTOR_SCHEMA,
			vec![
				"$._maxHpRatio".to_owned(),
				"$._maxHpExcludeEqual".to_owned(),
				"$._postFilter".to_owned(),
				"$._targetSide".to_owned(),
				"$._targetCategory".to_owned(),
				"$._targetMotion".to_owned(),
			],
		),
		(
			layout.attack_trigger,
			ATTACK_TRIGGER_SCRIPT,
			ATTACK_TRIGGER_SCHEMA,
			vec!["$._minTargetNum".to_owned()],
		),
	]
	.into_iter()
	.map(|(behavior, script, schema, fields)| {
		provenance(graph, behavior, script, schema, gamedata_revision, fields)
	})
	.collect()
}

fn provenance(
	graph: &BattlePrefabGraph,
	behavior: &BattlePrefabMonoBehaviour,
	script_hex: &str,
	schema_hex: &str,
	gamedata_revision: &str,
	field_paths: Vec<String>,
) -> SemanticProvenance {
	SemanticProvenance::BattlePrefab {
		mechanics_revision: graph.mechanics_revision.clone(),
		gamedata_revision: gamedata_revision.to_owned(),
		script_id: parse_id(script_hex),
		old_type_hash: parse_id(schema_hex),
		source: SemanticSourceLocation {
			logical_path: behavior.source.logical_path.clone(),
			cab_name: behavior.source.cab_name.clone(),
			path_id: behavior.source.path_id,
			field_paths,
		},
	}
}

fn expected_value_from_f64(
	value: f64,
	component: &'static str,
) -> Result<ExpectedValue, ShuSkillTemplateDecodeError> {
	let fixed: FixedDamage = FixedDamage::try_from_f64(value).map_err(|error| {
		ShuSkillTemplateDecodeError::new(
			component,
			format!("value {value} cannot be represented exactly enough for replay: {error}"),
		)
	})?;
	Ok(ExpectedValue::from_scaled(fixed.scaled()))
}

fn issue_sort_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
}

fn parse_id(hex: &str) -> [u8; 16] {
	assert_eq!(
		hex.len(),
		32,
		"internal script/schema ID must have 32 hex digits"
	);
	let mut bytes: [u8; 16] = [0; 16];
	for (index, byte) in bytes.iter_mut().enumerate() {
		let start: usize = index * 2;
		*byte = u8::from_str_radix(&hex[start..start + 2], 16)
			.expect("internal script/schema ID must be hexadecimal");
	}
	bytes
}

#[cfg(test)]
mod tests {
	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
		PotentialRank,
	};
	use serde_json::json;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab_asset, BattlePrefabGraph, BattlePrefabRootSection,
			BattlePrefabSemanticIssue,
		},
		combat::{
			ExpectedValue, StatKind, StoredAttackHpRatioComparison,
			StoredAttackReplacementDelivery, StoredAttackReservationInvalidationAction,
			StoredAttackTargetOrder, StoredAttackTargetRelationship,
		},
		normal_attack::NormalAttackTargetFilter,
	};

	use super::{
		first_value_mismatch, parse_id, try_decode_shu_skill_template, ShuSkillDelivery,
		ShuSkillHpComparison, ShuSkillSemanticIrRequirement, ShuSkillTargetSelection,
		ShuSkillTargetSelectionTiming, COMMON_ASSET_DIRECTORY, TARGET_SELECTOR_SCRIPT,
	};

	#[test]
	fn exact_value_comparison_reports_drift_path() {
		let expected = json!({"a":{"b":1}});
		let actual = json!({"a":{"b":2}});
		assert_eq!(
			first_value_mismatch(&expected, &actual, "$"),
			Some("$.a.b expected 1, found 2".to_owned())
		);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_conditional_heal_attack_decodes_and_one_field_drift_fails_closed() {
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
		let selected_skill = resolved.selected_skill();
		let prefab_id: &str = selected_skill.prefab_id().expect("common skill prefab ID");
		let asset_path: String = format!("{COMMON_ASSET_DIRECTORY}/{prefab_id}.prefab");
		let graph: BattlePrefabGraph = interpret_battle_prefab_asset(
			&prefabs,
			"[uc]skills",
			&asset_path,
			BattlePrefabRootSection::MainAsset,
			prefab_id,
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret reviewed conditional-heal skill graph");
		let decoded = try_decode_shu_skill_template(
			&graph,
			selected_skill,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect("reviewed conditional-heal template must not drift")
		.expect("reviewed heal-ability script must claim the template");

		assert_eq!(decoded.consumed_issues.len(), 6);
		assert!(decoded
			.consumed_issues
			.iter()
			.all(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. })));
		assert!(decoded.consumed_semantic_fact_sources.is_empty());
		assert!(graph
			.mono_behaviours
			.iter()
			.all(|behavior| { behavior.semantic_facts.is_empty() && behavior.buffs.is_empty() }));

		let semantics = &decoded.semantics;
		assert_eq!(semantics.evidence.selected_skill_id, "skchr_shu_1");
		assert_eq!(semantics.evidence.range_id, "x-4");
		assert_eq!(semantics.evidence.prefab_sources.len(), 6);
		assert_eq!(
			semantics.range,
			snapshot.resolve_range("x-4").unwrap().offsets()
		);
		assert_eq!(semantics.range_source_direction, 1);
		assert_eq!(
			semantics.target_hp_ratio,
			ExpectedValue::from_ratio(1, 2).unwrap()
		);
		assert_eq!(
			semantics.hp_comparison,
			ShuSkillHpComparison::LessThanOrEqual
		);
		assert_eq!(
			semantics.target_selection,
			ShuSkillTargetSelection::LowestHpRatioAlly
		);
		assert_eq!(
			semantics.target_selection_timing,
			ShuSkillTargetSelectionTiming::ReserveBeforeNormalAttack
		);
		assert_eq!(
			semantics.delivery,
			ShuSkillDelivery::ReplaceNextNormalAttack
		);
		assert_eq!(
			semantics.heal_multiplier,
			ExpectedValue::from_ratio(9, 5).unwrap()
		);
		assert_eq!(semantics.charges.max_charges, 3);
		assert_eq!(semantics.charges.sp_cost, 4);
		assert_eq!(semantics.charges.initial_sp, 0);
		assert_eq!(semantics.charges.sp_increment, ExpectedValue::ONE);
		assert!(semantics.charges.recover_sp_if_reserved_target_dies);

		let replacement = decoded
			.try_lower_stored_attack_replacement()
			.expect("reviewed selector and reservation semantics now lower exactly");
		assert_eq!(replacement.range, semantics.range);
		assert_eq!(
			replacement.relationship,
			StoredAttackTargetRelationship::Ally
		);
		assert!(replacement.require_healable);
		assert_eq!(
			replacement.target_filter,
			NormalAttackTargetFilter::GroundAndAir
		);
		assert!(replacement.include_source);
		assert_eq!(
			replacement.hp_ratio_comparison,
			StoredAttackHpRatioComparison::LessThanOrEqual
		);
		assert_eq!(replacement.hp_ratio_threshold, semantics.target_hp_ratio);
		assert_eq!(
			replacement.target_order,
			StoredAttackTargetOrder::LowestHpRatio
		);
		assert_eq!(replacement.max_charges, 3);
		assert_eq!(replacement.delivery_delay_ticks, 0);
		assert_eq!(
			replacement.delivery,
			StoredAttackReplacementDelivery::SourceStatScaledHealing {
				stat: StatKind::Attack,
				multiplier: ExpectedValue::from_ratio(9, 5).unwrap(),
			}
		);
		assert_eq!(
			replacement.invalidation.target_death,
			StoredAttackReservationInvalidationAction::RefundSpentSp
		);
		assert_eq!(
			replacement.invalidation.target_disappeared,
			StoredAttackReservationInvalidationAction::ConsumeSpentSp
		);
		assert_eq!(
			replacement.invalidation.target_out_of_range,
			StoredAttackReservationInvalidationAction::ConsumeSpentSp
		);
		assert_eq!(
			replacement.invalidation.target_generation_changed,
			StoredAttackReservationInvalidationAction::ConsumeSpentSp
		);

		let mut unsupported = decoded.clone();
		unsupported.semantics.range_source_direction = 0;
		unsupported.semantics.target_eligibility.exclude_owner = true;
		let gap = unsupported
			.try_lower_stored_attack_replacement()
			.expect_err("unknown range and selector shapes must fail closed");
		assert_eq!(
			gap.requirements,
			vec![
				ShuSkillSemanticIrRequirement::RightFacingRangeProjection,
				ShuSkillSemanticIrRequirement::SerializedTargetEligibility,
			]
		);

		let mut drifted: BattlePrefabGraph = graph.clone();
		let target_selector = drifted
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.script_id == Some(parse_id(TARGET_SELECTOR_SCRIPT)))
			.expect("reviewed target selector");
		target_selector.raw_fields["_limitTargetNum"] = json!(0);
		let drift = try_decode_shu_skill_template(
			&drifted,
			selected_skill,
			&snapshot,
			snapshot.resolved_revision(),
		)
		.expect_err("one-field selector drift must fail closed");
		assert!(drift.to_string().contains("$._limitTargetNum"));
	}
}
