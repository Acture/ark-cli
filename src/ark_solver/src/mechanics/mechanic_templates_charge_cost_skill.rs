//! Fail-closed decoder for the reviewed owner of the `charge_cost` building block.
//!
//! The owner is a common `[uc]skills` root whose skill controller and buff ability match the reviewed
//! `skcom_charge_cost` evidence field for field. That ability attaches its buffs when the skill is
//! cast, so every `charge_cost` buff on it grants once per activation, decoded by
//! [`decode_charge_cost_buff`]. The controller carries no lifecycle of its own: the skill table
//! decides between an automatic or manual start and an instantaneous or timed active span.
//!
//! The ability may carry further buffs and the root further behaviours. The template executes none
//! of them; it names them as the skill's remainder, so a skill whose grant is its only behaviour is
//! complete while any other skill replays its grant as a partial replay. Owners whose attach timing
//! differs — other ability scripts, other ability or controller settings — are not this template.

use std::{collections::BTreeSet, error::Error, fmt};

use crate::data::{
	ArkDataSnapshot, BlackboardEntry, SelectedSkillMetadata, SkillActivationType,
	SkillDurationType, SkillSpMetadata, SkillSpType,
};
use crate::simulation::TickRate;
use serde_json::{Map, Value};

use crate::mechanics::{
	battle_prefab_interpreter::{
		BattlePrefabBuffSpec, BattlePrefabGraph, BattlePrefabMonoBehaviour,
		BattlePrefabRootSection, BattlePrefabSemanticIssue,
	},
	charge_cost_buff::{
		decode_charge_cost_buff, has_reviewed_charge_cost_shape, is_charge_cost_buff,
		serialized_buff, ChargeCostBuffDecodeError, ChargeCostGrant, CHARGE_COST_TEMPLATE,
	},
	combat::{EffectTarget, ExpectedEffect, MechanicId, TaggedEffect},
	mechanic_semantics::{SemanticProvenance, SemanticSourceLocation},
	stage_facts::ticks_from_seconds_ceil,
};

const COMMON_LOGICAL_PATH: &str = "[uc]skills";
const COMMON_ASSET_DIRECTORY: &str = "dyn/battle/prefabs/[uc]skills";

const SKILL_CONTROLLER_SCRIPT: &str = "c7fa404b0bed7ccf80fbfe40dfca12b2";
const SKILL_CONTROLLER_SCHEMA: &str = "316815aff00294374c2e09f40de1cca5";
const BUFF_ABILITY_SCRIPT: &str = "94d0ebfda9ef298a570ee810608eff8f";
const BUFF_ABILITY_SCHEMA: &str = "eb9a2d754589e3364f4b9b337fda93db";
const EFFECT_HOOK_SCRIPT: &str = "33f2cf8eeccb67cd86fa554b93e178ad";
const EFFECT_HOOK_SCHEMA: &str = "78a80a284e4b68170bc79447219d63c3";

/// Presentation start events observed on the reviewed owners' cast effect.
const REVIEWED_CAST_EFFECT_START_EVENTS: [i64; 2] = [2, 4];

/// How long the skill stays active after the cast that grants its deployment points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChargeCostSkillActiveSpan {
	Instantaneous,
	/// Seconds from the level's duration; SP does not recover while the skill is active.
	Timed {
		seconds: f64,
	},
}

/// Exact behavior recovered from the common prefab and the selected skill level.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedChargeCostSkillTemplate {
	pub mechanic_id: MechanicId,
	pub selected_skill_id: String,
	pub prefab_sources: Vec<SemanticProvenance>,
	/// One grant per `charge_cost` buff, each credited on every activation.
	pub grants: Vec<ChargeCostGrant>,
	pub active_span: ChargeCostSkillActiveSpan,
	/// Serialized evidence the template does not execute. Nonempty makes the replay partial.
	pub remainder: Vec<String>,
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
}

impl DecodedChargeCostSkillTemplate {
	/// The effects one activation resolves: one battle-wide DP credit per grant, named by its
	/// source.
	#[must_use]
	pub fn activation_effects(&self) -> Vec<TaggedEffect> {
		self.grants
			.iter()
			.map(|grant: &ChargeCostGrant| TaggedEffect {
				mechanic: self.mechanic_id.clone(),
				target: EffectTarget::SelfActor,
				effect: ExpectedEffect::GainDp {
					amount: grant.amount,
				},
			})
			.collect()
	}

	/// The cadence each grant repeats at, once the owner is deployed and left to its own SP.
	///
	/// This is the activation lifecycle alone. Whether the owner ever reaches it is the caller's:
	/// a manual skill has this cadence but only casts when something casts it.
	pub fn grant_cadences(
		&self,
		selected_skill: &SelectedSkillMetadata,
		tick_rate: TickRate,
	) -> Result<Vec<ChargeCostGrantCadence>, ChargeCostGrantCadenceError> {
		let sp: &SkillSpMetadata = selected_skill.sp();
		let sp_per_pulse: u32 = exact_whole_sp(sp.increment())
			.ok_or(ChargeCostGrantCadenceError::UnsupportedSpLifecycle)?;
		let active_span_ticks: u64 = match self.active_span {
			ChargeCostSkillActiveSpan::Instantaneous => 0,
			ChargeCostSkillActiveSpan::Timed { seconds } => {
				match ticks_from_seconds_ceil(seconds, tick_rate) {
					Ok(ticks) if ticks > 0 => ticks,
					_ => return Err(ChargeCostGrantCadenceError::UnsupportedActiveSpan),
				}
			}
		};
		let lifecycle = ChargeCostSkillSpLifecycle {
			initial_sp: sp.initial(),
			sp_cost: sp.cost(),
			sp_per_pulse,
			pulse_interval_ticks: ticks_from_seconds_ceil(1.0, tick_rate)
				.map_err(|_| ChargeCostGrantCadenceError::TickOverflow)?,
			active_span_ticks,
		};
		self.grants
			.iter()
			.map(|grant: &ChargeCostGrant| charge_cost_grant_cadence(lifecycle, grant.amount))
			.collect()
	}
}

/// Replay charges only whole SP per pulse; anything else has no cadence here.
fn exact_whole_sp(increment: f64) -> Option<u32> {
	(increment.is_finite()
		&& increment > 0.0
		&& increment.fract() == 0.0
		&& increment <= f64::from(u32::MAX))
	.then_some(increment as u32)
}

/// When one decoded grant credits deployment points, relative to its owner's deployment tick.
///
/// Replay charges the level's SP on a fixed pulse from the deployment tick, casts the moment the
/// SP cost is reached, and restarts charging only once the active span ends. The cadence states
/// that lifecycle without replaying it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChargeCostGrantCadence {
	/// Ticks after deployment at which the first grant resolves; zero means the owner's own
	/// deployment tick, after that deployment has already been paid for.
	pub first_offset_ticks: u64,
	/// Ticks between consecutive grants.
	pub period_ticks: u64,
	/// Whole deployment points credited by each grant.
	pub amount: u32,
}

/// The selected level's SP lifecycle, in the units replay charges it in.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChargeCostSkillSpLifecycle {
	pub initial_sp: u32,
	pub sp_cost: u32,
	/// SP credited by one pulse.
	pub sp_per_pulse: u32,
	pub pulse_interval_ticks: u64,
	/// Ticks the skill stays active after a cast; zero for an instantaneous skill.
	pub active_span_ticks: u64,
}

/// Why one automatic grant has no cadence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeCostGrantCadenceError {
	/// The level recovers no SP, costs no SP, or pulses on no time at all.
	UnsupportedSpLifecycle,
	/// The grant credits nothing.
	EmptyGrant,
	/// The active span has no finite tick length.
	UnsupportedActiveSpan,
	TickOverflow,
}

impl fmt::Display for ChargeCostGrantCadenceError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::UnsupportedSpLifecycle => formatter.write_str(
				"an automatic grant needs a positive SP cost recovered by positive SP on a \
				 positive tick interval",
			),
			Self::EmptyGrant => formatter.write_str("an automatic grant must credit whole DP"),
			Self::UnsupportedActiveSpan => {
				formatter.write_str("the active span has no finite tick length")
			}
			Self::TickOverflow => formatter.write_str("the grant cadence overflowed ticks"),
		}
	}
}

impl Error for ChargeCostGrantCadenceError {}

/// The cadence one grant of `amount` repeats at under `lifecycle`.
///
/// The first cast waits for the SP the level does not start with; every later cast waits for the
/// whole cost, because a cast spends exactly the cost from an SP pool capped at it, and because
/// the pulses resume only when the active span ends.
pub fn charge_cost_grant_cadence(
	lifecycle: ChargeCostSkillSpLifecycle,
	amount: u32,
) -> Result<ChargeCostGrantCadence, ChargeCostGrantCadenceError> {
	if lifecycle.sp_cost == 0 || lifecycle.sp_per_pulse == 0 || lifecycle.pulse_interval_ticks == 0
	{
		return Err(ChargeCostGrantCadenceError::UnsupportedSpLifecycle);
	}
	if amount == 0 {
		return Err(ChargeCostGrantCadenceError::EmptyGrant);
	}
	let pulses_to_sp = |sp: u32| -> Result<u64, ChargeCostGrantCadenceError> {
		u64::from(sp.div_ceil(lifecycle.sp_per_pulse))
			.checked_mul(lifecycle.pulse_interval_ticks)
			.ok_or(ChargeCostGrantCadenceError::TickOverflow)
	};
	let first_offset_ticks: u64 =
		pulses_to_sp(lifecycle.sp_cost.saturating_sub(lifecycle.initial_sp))?;
	let period_ticks: u64 = lifecycle
		.active_span_ticks
		.checked_add(pulses_to_sp(lifecycle.sp_cost)?)
		.ok_or(ChargeCostGrantCadenceError::TickOverflow)?;
	Ok(ChargeCostGrantCadence {
		first_offset_ticks,
		period_ticks,
		amount,
	})
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChargeCostSkillTemplateDecodeError {
	component: &'static str,
	detail: String,
}

impl ChargeCostSkillTemplateDecodeError {
	fn new(component: &'static str, detail: impl Into<String>) -> Self {
		Self {
			component,
			detail: detail.into(),
		}
	}
}

impl fmt::Display for ChargeCostSkillTemplateDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"instant deployment-point skill template {component} mismatch: {detail}",
			component = self.component,
			detail = self.detail
		)
	}
}

impl Error for ChargeCostSkillTemplateDecodeError {}

#[derive(Clone, Copy)]
struct ChargeCostOwnerLayout<'a> {
	controller: &'a BattlePrefabMonoBehaviour,
	buff_ability: &'a BattlePrefabMonoBehaviour,
	effect_hook: &'a BattlePrefabMonoBehaviour,
}

impl ChargeCostOwnerLayout<'_> {
	fn owns(&self, behavior: &BattlePrefabMonoBehaviour) -> bool {
		[self.controller, self.buff_ability, self.effect_hook]
			.iter()
			.any(|owner: &&BattlePrefabMonoBehaviour| owner.source == behavior.source)
	}
}

/// A buff ability on the root GameObject carrying a `charge_cost` buff of the reviewed shape
/// claims the root, so the ability that claims is the one the decoder reads. In the verified
/// common skills bundle that is `skcom_charge_cost`, Flametail's first skill and Courier's second;
/// the other skills whose buff ability grants deployment points carry an unreviewed variant of the
/// buff and stay unclaimed.
pub(crate) fn claims_charge_cost_skill_template(graph: &BattlePrefabGraph) -> bool {
	let script_id: [u8; 16] = parse_id(BUFF_ABILITY_SCRIPT);
	let [root_index] = graph.root_game_object_indices.as_slice() else {
		return false;
	};
	graph
		.mono_behaviours
		.iter()
		.filter(|behavior: &&BattlePrefabMonoBehaviour| {
			behavior.script_id == Some(script_id) && behavior.game_object_index == Some(*root_index)
		})
		.any(|behavior: &BattlePrefabMonoBehaviour| {
			behavior.buffs.iter().any(|spec: &BattlePrefabBuffSpec| {
				serialized_buff(behavior, spec)
					.is_some_and(|raw: &Value| has_reviewed_charge_cost_shape(spec, raw))
			})
		})
}

/// Decodes the reviewed `charge_cost` owner without operator-ID branching.
pub fn try_decode_charge_cost_skill_template(
	graph: &BattlePrefabGraph,
	selected_skill: &SelectedSkillMetadata,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<Option<DecodedChargeCostSkillTemplate>, ChargeCostSkillTemplateDecodeError> {
	if !claims_charge_cost_skill_template(graph) {
		return Ok(None);
	}
	validate_revisions(graph, snapshot, gamedata_revision)?;
	let active_span: ChargeCostSkillActiveSpan = validate_selected_skill(selected_skill, graph)?;
	let layout: ChargeCostOwnerLayout<'_> = validate_owner_layout(graph)?;
	validate_owner_configurations(&layout)?;
	validate_owner_edges_and_facts(&layout)?;
	let (grants, remainder_buffs): (Vec<ChargeCostGrant>, Vec<String>) =
		decode_ability_buffs(&layout, selected_skill.blackboard())?;
	let mut remainder: Vec<String> = remainder_buffs;
	remainder.extend(remainder_behaviours(graph, &layout));
	let unread_keys: Vec<&str> = selected_skill
		.blackboard()
		.iter()
		.map(BlackboardEntry::key)
		.filter(|key: &&str| !grants.iter().any(|grant| grant.amount_key == *key))
		.collect();
	if !unread_keys.is_empty() {
		if remainder.is_empty() {
			return Err(ChargeCostSkillTemplateDecodeError::new(
				"selected skill blackboard",
				format!("{unread_keys:?} belong to no grant and the skill has no other behavior"),
			));
		}
		remainder.push(format!("blackboard {unread_keys:?}"));
	}
	let consumed_issues: Vec<BattlePrefabSemanticIssue> = validate_issues(graph, &layout, &grants)?;
	Ok(Some(DecodedChargeCostSkillTemplate {
		mechanic_id: MechanicId::new(format!(
			"{}/template/charge-cost",
			selected_skill.skill_id()
		)),
		selected_skill_id: selected_skill.skill_id().to_owned(),
		prefab_sources: prefab_provenance(graph, &layout, gamedata_revision),
		grants,
		active_span,
		remainder,
		consumed_issues,
	}))
}

fn validate_revisions(
	graph: &BattlePrefabGraph,
	snapshot: &ArkDataSnapshot,
	gamedata_revision: &str,
) -> Result<(), ChargeCostSkillTemplateDecodeError> {
	if snapshot.resolved_revision() != gamedata_revision {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"gamedata revision",
			format!(
				"caller supplied {gamedata_revision:?}, verified snapshot is {:?}",
				snapshot.resolved_revision()
			),
		));
	}
	if graph.mechanics_revision.is_empty() {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"mechanics revision",
			"official prefab graph has an empty mechanics revision",
		));
	}
	Ok(())
}

/// Checks the level's lifecycle is one the replay executes: an automatic or manual start, a
/// rangeless cast, a single time-recovered charge, and an instantaneous or finite active span.
fn validate_selected_skill(
	selected_skill: &SelectedSkillMetadata,
	graph: &BattlePrefabGraph,
) -> Result<ChargeCostSkillActiveSpan, ChargeCostSkillTemplateDecodeError> {
	let prefab_id: &str = selected_skill.prefab_id().ok_or_else(|| {
		ChargeCostSkillTemplateDecodeError::new("selected skill", "missing common skill prefab ID")
	})?;
	if prefab_id != graph.prefab_name {
		return Err(ChargeCostSkillTemplateDecodeError::new(
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
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"selected skill",
			format!(
				"expected no prefab/token override, found prefab={:?}, token={:?}",
				selected_skill.override_prefab_key(),
				selected_skill.override_token_key()
			),
		));
	}
	let seconds: f64 = selected_skill.duration_seconds();
	if !matches!(
		selected_skill.activation_type(),
		SkillActivationType::Auto | SkillActivationType::Manual
	) || selected_skill.duration_type() != &SkillDurationType::None
		|| !seconds.is_finite()
		|| seconds < 0.0
	{
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"selected skill lifecycle",
			format!(
				"expected AUTO or MANUAL with a finite NONE duration, found {:?}/{:?}/{seconds}",
				selected_skill.activation_type(),
				selected_skill.duration_type()
			),
		));
	}
	if let Some(range_id) = selected_skill.range_id() {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"selected skill range",
			format!("expected no range, found {range_id:?}"),
		));
	}
	let sp = selected_skill.sp();
	if sp.sp_type() != &SkillSpType::IncreaseWithTime
		|| sp.max_charges() != 1
		|| !sp.increment().is_finite()
		|| sp.increment() <= 0.0
		|| sp.increment().fract() != 0.0
		|| sp.cost() == 0
		|| sp.initial() > sp.cost()
	{
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"selected skill SP",
			format!(
				"expected whole positive time recovery, one charge, and initial<=cost>0; found {:?}, max={}, increment={}, initial={}, cost={}",
				sp.sp_type(),
				sp.max_charges(),
				sp.increment(),
				sp.initial(),
				sp.cost()
			),
		));
	}
	Ok(if seconds == 0.0 {
		ChargeCostSkillActiveSpan::Instantaneous
	} else {
		ChargeCostSkillActiveSpan::Timed { seconds }
	})
}

fn validate_owner_layout(
	graph: &BattlePrefabGraph,
) -> Result<ChargeCostOwnerLayout<'_>, ChargeCostSkillTemplateDecodeError> {
	let expected_asset_path: String =
		format!("{COMMON_ASSET_DIRECTORY}/{}.prefab", graph.prefab_name);
	if graph.logical_path != COMMON_LOGICAL_PATH
		|| graph.asset_path != expected_asset_path
		|| graph.section != BattlePrefabRootSection::MainAsset
	{
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"common root identity",
			format!(
				"expected logical path {COMMON_LOGICAL_PATH:?}, asset {expected_asset_path:?}, MainAsset; found {:?}, {:?}, {:?}",
				graph.logical_path, graph.asset_path, graph.section
			),
		));
	}
	let [root_index] = graph.root_game_object_indices.as_slice() else {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"common graph shape",
			format!(
				"expected one root GameObject, found {}",
				graph.root_game_object_indices.len()
			),
		));
	};
	let root_index: usize = *root_index;
	let root = graph.game_objects.get(root_index).ok_or_else(|| {
		ChargeCostSkillTemplateDecodeError::new(
			"common graph root",
			format!("root index {root_index} is outside the GameObject table"),
		)
	})?;
	if root.name != graph.prefab_name || root.hierarchy_path != graph.prefab_name {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"common graph hierarchy",
			format!(
				"expected root {:?}, found name={:?}, path={:?}",
				graph.prefab_name, root.name, root.hierarchy_path
			),
		));
	}
	Ok(ChargeCostOwnerLayout {
		controller: exact_owned_behavior(
			graph,
			root_index,
			SKILL_CONTROLLER_SCRIPT,
			SKILL_CONTROLLER_SCHEMA,
			"skill controller",
		)?,
		buff_ability: exact_owned_behavior(
			graph,
			root_index,
			BUFF_ABILITY_SCRIPT,
			BUFF_ABILITY_SCHEMA,
			"buff ability",
		)?,
		effect_hook: exact_owned_behavior(
			graph,
			root_index,
			EFFECT_HOOK_SCRIPT,
			EFFECT_HOOK_SCHEMA,
			"effect hook",
		)?,
	})
}

fn exact_owned_behavior<'a>(
	graph: &'a BattlePrefabGraph,
	owner_index: usize,
	script_hex: &str,
	schema_hex: &str,
	component: &'static str,
) -> Result<&'a BattlePrefabMonoBehaviour, ChargeCostSkillTemplateDecodeError> {
	let script_id: [u8; 16] = parse_id(script_hex);
	let old_type_hash: [u8; 16] = parse_id(schema_hex);
	let matches: Vec<&BattlePrefabMonoBehaviour> = graph.game_objects[owner_index]
		.mono_behaviour_indices
		.iter()
		.filter_map(|&index: &usize| graph.mono_behaviours.get(index))
		.filter(|behavior: &&BattlePrefabMonoBehaviour| behavior.script_id == Some(script_id))
		.collect();
	let [behavior] = matches.as_slice() else {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			component,
			format!(
				"expected one root behavior with script {script_hex}, found {}",
				matches.len()
			),
		));
	};
	if behavior.old_type_hash != Some(old_type_hash)
		|| behavior.game_object_index != Some(owner_index)
	{
		return Err(ChargeCostSkillTemplateDecodeError::new(
			component,
			format!(
				"expected schema {schema_hex} owned by GameObject {owner_index}, found {:?} owned by {:?}",
				behavior.old_type_hash, behavior.game_object_index
			),
		));
	}
	Ok(behavior)
}

/// Pins every serialized field of the owner. The controller fixes the lifecycle the replay relies
/// on: unlimited repeat activations (`_maxTriggerTime: -1`), casting without a target, no casting
/// without SP and no SP recovery while the skill is active. The buff ability attaches its buffs to
/// its owner on the cast without a selector or pre-delay; only its buff list may differ, and each
/// entry is decoded or named on its own. Only the presentation cast effect may vary, within the
/// start events observed on reviewed owners.
fn validate_owner_configurations(
	layout: &ChargeCostOwnerLayout<'_>,
) -> Result<(), ChargeCostSkillTemplateDecodeError> {
	validate_configuration(
		layout.controller,
		"skill controller",
		&[],
		expected_configuration(
			r#"{
				"_allowNoTarget": 1,
				"_allowSpRecoveryWhenAffecting": 0,
				"_attachSignalIds": 0,
				"_attackBlackboardModeIndex": -1,
				"_availableValidator": {"m_FileID": 0, "m_PathID": 0},
				"_beginAnim": "",
				"_beginAnimDown": "",
				"_beginAnimValidator": {"m_FileID": 0, "m_PathID": 0},
				"_beginEffect": [],
				"_canCastDuringBorn": 1,
				"_canCastWithNoSp": 0,
				"_canSilenced": 0,
				"_canSwitchDuringBorn": 1,
				"_canUseInAbnormalState": 0,
				"_checkHasTargetBeforeDoCast": 0,
				"_defaultModeIndex": -1,
				"_earlySkillFinishAtAttackFinished": 0,
				"_extraAbilities": [],
				"_extraAttackBlackboardModeIndices": [],
				"_extraRangeIdModeIndices": [],
				"_forceToShowRange": 0,
				"_forceUseBaseShowRange": 0,
				"_hasUniStackColor": 0,
				"_hidden": 0,
				"_hideHudSkillStackCount": 0,
				"_hideRangeToShow": 0,
				"_hideSkillStackCount": 0,
				"_isRemoteControlled": 0,
				"_limitGlobalTriggerTime": 0,
				"_maxTriggerTime": -1,
				"_onlyAvailableWhenTokenValid": 0,
				"_playCharWordVoice": 1,
				"_playCharWordWithPassiveType": 0,
				"_playSkillBeginAnim": 0,
				"_rangeIdModeIndex": -1,
				"_rangeIdUsageType": 7,
				"_rangeToShow": {"m_FileID": 0, "m_PathID": 0},
				"_recoverSpIfNoTarget": 0,
				"_resetAbilityCooldownWhenCastEnd": 0,
				"_shouldCastLikeAttack": 0,
				"_showProgressToNext": 0,
				"_showSpAsBulletMode": 0,
				"_showSpBarEvenHidden": 0,
				"_showSpRatioAsProgress": 0,
				"_switchToState": 0,
				"_trigger": {"m_FileID": 0, "m_PathID": 0},
				"_uninterruptibleOnAbilityPredelay": 0,
				"_useEscapeTime": 0,
				"_useFaceVector": 0,
				"_useTriggerInManualMode": 0,
				"_writeDurationToAttackBlackboard": 0,
				"m_Enabled": 1,
				"m_Name": ""
			}"#,
		),
	)?;
	validate_configuration(
		layout.buff_ability,
		"buff ability",
		&["_buffs"],
		expected_configuration(
			r#"{
				"_attachPassiveBuffsOnDummy": 1,
				"_checkBuffEarlyFinish": 0,
				"_enableUpdateCooldown": 0,
				"_escapeTime": 0.0,
				"_forceAsDmgOrHealAbility": 0,
				"_ignoreIfOwnerDead": 0,
				"_ignoreInfiniteBuffCooldown": 0,
				"_interruptAbilityOnDetach": 0,
				"_interuptIfTargetDead": 0,
				"_metadata": {"blackboardPrefix": "", "namedAsAlias": ""},
				"_passiveBuffs": [],
				"_preDelay": 0.0,
				"_resetCdStrategy": 1,
				"_selector": {"m_FileID": 0, "m_PathID": 0},
				"_updateLifetimeWhenSpellStart": 0,
				"_waitForAttackEvent": 1,
				"_waitForCasting": 0,
				"m_Enabled": 1,
				"m_Name": ""
			}"#,
		),
	)?;
	if !layout
		.buff_ability
		.raw_fields
		.get("_buffs")
		.is_some_and(Value::is_array)
	{
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"buff ability",
			"$._buffs must be a list",
		));
	}
	let cast_effect: String = nonempty_cast_effect(layout.effect_hook)?;
	let start_event: i64 = layout
		.effect_hook
		.raw_fields
		.pointer("/_castEffects/0/_startEvent")
		.and_then(Value::as_i64)
		.filter(|event: &i64| REVIEWED_CAST_EFFECT_START_EVENTS.contains(event))
		.ok_or_else(|| {
			ChargeCostSkillTemplateDecodeError::new(
				"effect hook",
				format!(
					"$._castEffects[0]._startEvent must be one of {REVIEWED_CAST_EFFECT_START_EVENTS:?}"
				),
			)
		})?;
	let mut effect_hook_configuration: Value = expected_configuration(
		r#"{
			"_attachEffects": [],
			"_castEffects": [{
				"_delayIfFirstAttack": 0.0,
				"_directionType": 0,
				"_effects": ["__CAST_EFFECT__", "", "", ""],
				"_endEvent": 3,
				"_hookEffectAfterFirstAttack": "",
				"_oneshot": 1,
				"_onlyOnce": 0,
				"_startEvent": 0,
				"_useFourDirectionalFace": 0
			}],
			"_castTargetEffects": [],
			"_convertCastEndToAttachFinish": 0,
			"_hitEffects": [],
			"_inputTargetEffects": [],
			"_preDelayFactor": 0.0,
			"_recalculatePlaybackSpeed": 0,
			"m_Enabled": 1,
			"m_Name": ""
		}"#,
	);
	effect_hook_configuration["_castEffects"][0]["_effects"][0] = Value::String(cast_effect);
	effect_hook_configuration["_castEffects"][0]["_startEvent"] = Value::from(start_event);
	validate_configuration(
		layout.effect_hook,
		"effect hook",
		&[],
		effect_hook_configuration,
	)
}

fn expected_configuration(serialized: &str) -> Value {
	serde_json::from_str(serialized)
		.expect("internal official prefab configuration must be valid JSON")
}

/// Compares every serialized field except Unity identity fields and the named `free` fields,
/// which the caller validates on its own.
fn validate_configuration(
	behavior: &BattlePrefabMonoBehaviour,
	component: &'static str,
	free: &[&str],
	expected: Value,
) -> Result<(), ChargeCostSkillTemplateDecodeError> {
	let fields: &Map<String, Value> = behavior.raw_fields.as_object().ok_or_else(|| {
		ChargeCostSkillTemplateDecodeError::new(component, "raw TypeTree fields are not an object")
	})?;
	let mut actual: Map<String, Value> = fields.clone();
	for unity_field in ["m_GameObject", "m_Script"] {
		if actual.remove(unity_field).is_none() {
			return Err(ChargeCostSkillTemplateDecodeError::new(
				component,
				format!("missing Unity identity field {unity_field:?}"),
			));
		}
	}
	for field in free {
		if actual.remove(*field).is_none() {
			return Err(ChargeCostSkillTemplateDecodeError::new(
				component,
				format!("missing field $.{field}"),
			));
		}
	}
	match first_value_mismatch(&expected, &Value::Object(actual), "$") {
		Some(detail) => Err(ChargeCostSkillTemplateDecodeError::new(component, detail)),
		None => Ok(()),
	}
}

fn nonempty_cast_effect(
	behavior: &BattlePrefabMonoBehaviour,
) -> Result<String, ChargeCostSkillTemplateDecodeError> {
	behavior
		.raw_fields
		.pointer("/_castEffects/0/_effects/0")
		.and_then(Value::as_str)
		.filter(|text: &&str| !text.is_empty())
		.map(str::to_owned)
		.ok_or_else(|| {
			ChargeCostSkillTemplateDecodeError::new(
				"effect hook",
				"$._castEffects[0]._effects[0] must be non-empty text",
			)
		})
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

/// The owner neither points anywhere nor emits typed facts, and only its buff ability carries
/// buffs.
fn validate_owner_edges_and_facts(
	layout: &ChargeCostOwnerLayout<'_>,
) -> Result<(), ChargeCostSkillTemplateDecodeError> {
	for (component, behavior) in [
		("skill controller", layout.controller),
		("buff ability", layout.buff_ability),
		("effect hook", layout.effect_hook),
	] {
		if !behavior.references.is_empty()
			|| !behavior.unavailable_references.is_empty()
			|| !behavior.referenced_mono_behaviour_indices.is_empty()
			|| !behavior.semantic_facts.is_empty()
		{
			return Err(ChargeCostSkillTemplateDecodeError::new(
				component,
				format!(
					"unexpected references={:?}, unavailable={:?}, facts={:?}",
					behavior.references,
					behavior.unavailable_references,
					behavior
						.semantic_facts
						.iter()
						.map(|fact| fact.kind())
						.collect::<Vec<_>>()
				),
			));
		}
	}
	for (component, behavior) in [
		("skill controller", layout.controller),
		("effect hook", layout.effect_hook),
	] {
		if !behavior.buffs.is_empty() {
			return Err(ChargeCostSkillTemplateDecodeError::new(
				component,
				format!("unexpectedly carries buffs {:?}", behavior.buffs),
			));
		}
	}
	Ok(())
}

/// Decodes every `charge_cost` buff on the ability through the building block and names every
/// other buff as remainder. A `charge_cost` buff the block rejects fails the whole template: the
/// owner is claimed, so an undecodable grant on it is drift, not an omission.
fn decode_ability_buffs(
	layout: &ChargeCostOwnerLayout<'_>,
	blackboard: &[BlackboardEntry],
) -> Result<(Vec<ChargeCostGrant>, Vec<String>), ChargeCostSkillTemplateDecodeError> {
	let ability: &BattlePrefabMonoBehaviour = layout.buff_ability;
	let mut grants: Vec<ChargeCostGrant> = Vec::new();
	let mut remainder: Vec<String> = Vec::new();
	for spec in &ability.buffs {
		if !is_charge_cost_buff(spec) {
			remainder.push(format!(
				"buff {:?} ({:?}) at {}",
				spec.buff_key.as_deref().unwrap_or_default(),
				spec.template_key.as_deref().unwrap_or_default(),
				spec.field_path
			));
			continue;
		}
		let raw: &Value = serialized_buff(ability, spec).ok_or_else(|| {
			ChargeCostSkillTemplateDecodeError::new(
				"charge_cost buff",
				format!(
					"{} does not resolve in the serialized ability",
					spec.field_path
				),
			)
		})?;
		let grant: ChargeCostGrant = decode_charge_cost_buff(spec, raw, blackboard).map_err(
			|error: ChargeCostBuffDecodeError| {
				ChargeCostSkillTemplateDecodeError::new("charge_cost buff", error.to_string())
			},
		)?;
		if grants
			.iter()
			.any(|decoded: &ChargeCostGrant| decoded.amount_key == grant.amount_key)
		{
			return Err(ChargeCostSkillTemplateDecodeError::new(
				"charge_cost buff",
				format!(
					"two grants read the same blackboard key {:?}",
					grant.amount_key
				),
			));
		}
		grants.push(grant);
	}
	Ok((grants, remainder))
}

/// Names every behaviour outside the owner, which the template does not execute.
fn remainder_behaviours(
	graph: &BattlePrefabGraph,
	layout: &ChargeCostOwnerLayout<'_>,
) -> Vec<String> {
	graph
		.mono_behaviours
		.iter()
		.filter(|behavior: &&BattlePrefabMonoBehaviour| !layout.owns(behavior))
		.map(|behavior: &BattlePrefabMonoBehaviour| {
			let location: &str = behavior
				.game_object_index
				.and_then(|index: usize| graph.game_objects.get(index))
				.map_or("an unowned object", |object| object.hierarchy_path.as_str());
			let script: String = behavior
				.script_id
				.map_or_else(|| "no script".to_owned(), |id: [u8; 16]| hex(&id));
			let buffs: Vec<&str> = behavior
				.buffs
				.iter()
				.filter_map(|spec: &BattlePrefabBuffSpec| spec.buff_key.as_deref())
				.collect();
			format!("behavior {script} on {location} with buffs {buffs:?}")
		})
		.collect()
}

/// Consumes exactly the owner's issues: its three unknown scripts and one missing custom-buff
/// semantic per decoded grant. Every other issue must come from remainder evidence, which the
/// template reports rather than consumes.
fn validate_issues(
	graph: &BattlePrefabGraph,
	layout: &ChargeCostOwnerLayout<'_>,
	grants: &[ChargeCostGrant],
) -> Result<Vec<BattlePrefabSemanticIssue>, ChargeCostSkillTemplateDecodeError> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = vec![
		unknown_issue(layout.controller, SKILL_CONTROLLER_SCRIPT),
		unknown_issue(layout.buff_ability, BUFF_ABILITY_SCRIPT),
		unknown_issue(layout.effect_hook, EFFECT_HOOK_SCRIPT),
	];
	expected.extend(grants.iter().map(|grant: &ChargeCostGrant| {
		BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
			source: layout.buff_ability.source.clone(),
			field_path: grant.field_path.clone(),
			buff_key: Some(grant.buff_key.clone()),
			template_key: Some(CHARGE_COST_TEMPLATE.to_owned()),
		}
	}));
	let missing: Vec<&BattlePrefabSemanticIssue> = expected
		.iter()
		.filter(|issue: &&BattlePrefabSemanticIssue| !graph.issues.contains(*issue))
		.collect();
	let decoded_paths: BTreeSet<&str> = grants
		.iter()
		.map(|grant: &ChargeCostGrant| grant.field_path.as_str())
		.collect();
	let unexplained: Vec<&BattlePrefabSemanticIssue> = graph
		.issues
		.iter()
		.filter(|issue: &&BattlePrefabSemanticIssue| !expected.contains(*issue))
		.filter(|issue: &&BattlePrefabSemanticIssue| {
			!issue_is_remainder(issue, layout, &decoded_paths)
		})
		.collect();
	if !missing.is_empty() || !unexplained.is_empty() {
		return Err(ChargeCostSkillTemplateDecodeError::new(
			"semantic issues",
			format!("missing {missing:?}; unexplained {unexplained:?}"),
		));
	}
	expected.sort_by_key(issue_sort_key);
	Ok(expected)
}

/// An issue belongs to remainder evidence when it comes from a behaviour outside the owner, or is
/// the missing semantics of a buff on the ability that the template did not decode.
fn issue_is_remainder(
	issue: &BattlePrefabSemanticIssue,
	layout: &ChargeCostOwnerLayout<'_>,
	decoded_paths: &BTreeSet<&str>,
) -> bool {
	match issue {
		BattlePrefabSemanticIssue::CustomBuffSemanticsMissing {
			source, field_path, ..
		} if *source == layout.buff_ability.source => {
			!decoded_paths.contains(field_path.as_str())
				&& layout
					.buff_ability
					.buffs
					.iter()
					.any(|spec: &BattlePrefabBuffSpec| {
						spec.field_path == *field_path && !is_charge_cost_buff(spec)
					})
		}
		BattlePrefabSemanticIssue::MissingScriptId { source }
		| BattlePrefabSemanticIssue::UnknownScriptId { source, .. }
		| BattlePrefabSemanticIssue::ScriptSchemaHashMismatch { source, .. }
		| BattlePrefabSemanticIssue::ScriptHandlerFailed { source, .. }
		| BattlePrefabSemanticIssue::CustomBuffSemanticsMissing { source, .. }
		| BattlePrefabSemanticIssue::UnavailableExternalReference { source, .. } => {
			![layout.controller, layout.buff_ability, layout.effect_hook]
				.iter()
				.any(|owner: &&BattlePrefabMonoBehaviour| owner.source == *source)
		}
	}
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
	layout: &ChargeCostOwnerLayout<'_>,
	gamedata_revision: &str,
) -> Vec<SemanticProvenance> {
	[
		(
			layout.controller,
			SKILL_CONTROLLER_SCRIPT,
			SKILL_CONTROLLER_SCHEMA,
			vec![
				"$._maxTriggerTime".to_owned(),
				"$._allowNoTarget".to_owned(),
				"$._canCastWithNoSp".to_owned(),
				"$._allowSpRecoveryWhenAffecting".to_owned(),
			],
		),
		(
			layout.buff_ability,
			BUFF_ABILITY_SCRIPT,
			BUFF_ABILITY_SCHEMA,
			vec![
				"$._buffs".to_owned(),
				"$._waitForAttackEvent".to_owned(),
				"$._waitForCasting".to_owned(),
				"$._selector".to_owned(),
			],
		),
		(
			layout.effect_hook,
			EFFECT_HOOK_SCRIPT,
			EFFECT_HOOK_SCHEMA,
			vec!["$._castEffects".to_owned()],
		),
	]
	.into_iter()
	.map(
		|(behavior, script_hex, schema_hex, field_paths)| SemanticProvenance::BattlePrefab {
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
		},
	)
	.collect()
}

fn issue_sort_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
}

fn hex(bytes: &[u8; 16]) -> String {
	bytes
		.iter()
		.map(|byte: &u8| format!("{byte:02x}"))
		.collect()
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
	use std::num::NonZeroU32;

	use crate::data::{
		ArkDataSnapshot, BattlePrefabSnapshot, OperatorBuild, OperatorIdentity, OperatorModule,
		PotentialRank,
	};
	use crate::simulation::TickRate;
	use serde_json::json;

	use crate::mechanics::{
		battle_prefab_handlers::built_in_battle_prefab_handler_registry,
		battle_prefab_interpreter::{
			interpret_battle_prefab_asset, BattlePrefabGraph, BattlePrefabRootSection,
			BattlePrefabSemanticIssue,
		},
		combat::{EffectTarget, ExpectedEffect, TaggedEffect},
	};

	use super::{
		charge_cost_grant_cadence, claims_charge_cost_skill_template, parse_id,
		try_decode_charge_cost_skill_template, ChargeCostGrantCadence, ChargeCostGrantCadenceError,
		ChargeCostSkillActiveSpan, ChargeCostSkillSpLifecycle, DecodedChargeCostSkillTemplate,
		BUFF_ABILITY_SCRIPT, COMMON_ASSET_DIRECTORY, SKILL_CONTROLLER_SCRIPT,
	};

	struct LocalSnapshots {
		snapshot: ArkDataSnapshot,
		prefabs: BattlePrefabSnapshot,
	}

	fn local_snapshots() -> LocalSnapshots {
		let gamedata_path = crate::test_support::configured_gamedata_dir();
		let prefab_path = crate::test_support::configured_battle_prefab_dir();
		let snapshot: ArkDataSnapshot =
			ArkDataSnapshot::load(&gamedata_path).expect("verified local gamedata");
		let prefabs: BattlePrefabSnapshot =
			BattlePrefabSnapshot::load(&prefab_path).expect("verified local battle prefabs");
		prefabs
			.ensure_compatible_with(&snapshot)
			.expect("matching local revisions");
		LocalSnapshots { snapshot, prefabs }
	}

	fn common_skill_graph(local: &LocalSnapshots, prefab_id: &str) -> BattlePrefabGraph {
		interpret_battle_prefab_asset(
			&local.prefabs,
			"[uc]skills",
			&format!("{COMMON_ASSET_DIRECTORY}/{prefab_id}.prefab"),
			BattlePrefabRootSection::MainAsset,
			prefab_id,
			&built_in_battle_prefab_handler_registry(),
		)
		.expect("interpret verified common skill graph")
	}

	fn maximal_build(local: &LocalSnapshots, operator_id: &str, skill: u8) -> OperatorBuild {
		let template: OperatorBuild = local
			.snapshot
			.operators()
			.maximal_builds(operator_id, skill)
			.expect("operator maximal builds should resolve")
			.into_iter()
			.next()
			.expect("operator should have one maximal build");
		OperatorBuild::new(
			OperatorIdentity::Id(operator_id.to_owned()),
			template.elite(),
			template.level(),
			template.trust_percent(),
			template.skill(),
			template.skill_level(),
			template.potential(),
			OperatorModule::None,
			0,
		)
	}

	/// Decodes one build's selected skill against the common root it names.
	fn decode(local: &LocalSnapshots, build: &OperatorBuild) -> DecodedChargeCostSkillTemplate {
		let resolved = local
			.snapshot
			.operators()
			.resolve(build)
			.expect("resolve reviewed local build");
		let selected_skill = resolved.selected_skill();
		let graph: BattlePrefabGraph = common_skill_graph(
			local,
			selected_skill.prefab_id().expect("common skill prefab ID"),
		);
		try_decode_charge_cost_skill_template(
			&graph,
			selected_skill,
			&local.snapshot,
			local.snapshot.resolved_revision(),
		)
		.expect("reviewed charge-cost owner must not drift")
		.expect("reviewed charge_cost buff must claim the owner")
	}

	fn grant_amounts(decoded: &DecodedChargeCostSkillTemplate) -> Vec<(&str, u32)> {
		decoded
			.grants
			.iter()
			.map(|grant| (grant.amount_key.as_str(), grant.amount))
			.collect()
	}

	fn tick_rate() -> TickRate {
		TickRate::new(NonZeroU32::new(30).expect("test tick rate is non-zero"))
	}

	/// 讯使's first skill at level ten: thirteen of thirty SP, one SP a second, instantaneous.
	fn courier_first_skill_lifecycle() -> ChargeCostSkillSpLifecycle {
		ChargeCostSkillSpLifecycle {
			initial_sp: 13,
			sp_cost: 30,
			sp_per_pulse: 1,
			pulse_interval_ticks: 30,
			active_span_ticks: 0,
		}
	}

	#[test]
	fn an_instantaneous_skill_recharges_from_its_cast_and_a_timed_one_from_its_end() {
		// The first cast waits only for the SP the level does not start with; every later cast
		// waits for the whole cost, and a timed skill waits for its span first.
		assert_eq!(
			charge_cost_grant_cadence(courier_first_skill_lifecycle(), 9).unwrap(),
			ChargeCostGrantCadence {
				first_offset_ticks: 510,
				period_ticks: 900,
				amount: 9,
			}
		);
		assert_eq!(
			charge_cost_grant_cadence(
				ChargeCostSkillSpLifecycle {
					active_span_ticks: 450,
					..courier_first_skill_lifecycle()
				},
				3,
			)
			.unwrap(),
			ChargeCostGrantCadence {
				first_offset_ticks: 510,
				period_ticks: 1_350,
				amount: 3,
			}
		);
	}

	#[test]
	fn a_skill_that_starts_at_its_cost_grants_in_its_owners_deployment_tick() {
		assert_eq!(
			charge_cost_grant_cadence(
				ChargeCostSkillSpLifecycle {
					initial_sp: 30,
					..courier_first_skill_lifecycle()
				},
				9,
			)
			.unwrap()
			.first_offset_ticks,
			0
		);
		// An initial SP beyond the cost is capped at it rather than shortening anything further.
		assert_eq!(
			charge_cost_grant_cadence(
				ChargeCostSkillSpLifecycle {
					initial_sp: 45,
					..courier_first_skill_lifecycle()
				},
				9,
			)
			.unwrap(),
			charge_cost_grant_cadence(
				ChargeCostSkillSpLifecycle {
					initial_sp: 30,
					..courier_first_skill_lifecycle()
				},
				9,
			)
			.unwrap()
		);
	}

	#[test]
	fn a_partial_pulse_still_costs_a_whole_interval() {
		// Four SP a pulse covers thirty SP in eight pulses, not seven and a half.
		assert_eq!(
			charge_cost_grant_cadence(
				ChargeCostSkillSpLifecycle {
					initial_sp: 0,
					sp_per_pulse: 4,
					..courier_first_skill_lifecycle()
				},
				9,
			)
			.unwrap(),
			ChargeCostGrantCadence {
				first_offset_ticks: 240,
				period_ticks: 240,
				amount: 9,
			}
		);
	}

	#[test]
	fn a_lifecycle_that_never_completes_has_no_cadence() {
		for lifecycle in [
			ChargeCostSkillSpLifecycle {
				sp_cost: 0,
				..courier_first_skill_lifecycle()
			},
			ChargeCostSkillSpLifecycle {
				sp_per_pulse: 0,
				..courier_first_skill_lifecycle()
			},
			ChargeCostSkillSpLifecycle {
				pulse_interval_ticks: 0,
				..courier_first_skill_lifecycle()
			},
		] {
			assert_eq!(
				charge_cost_grant_cadence(lifecycle, 9),
				Err(ChargeCostGrantCadenceError::UnsupportedSpLifecycle)
			);
		}
		assert_eq!(
			charge_cost_grant_cadence(courier_first_skill_lifecycle(), 0),
			Err(ChargeCostGrantCadenceError::EmptyGrant)
		);
		assert_eq!(
			charge_cost_grant_cadence(
				ChargeCostSkillSpLifecycle {
					pulse_interval_ticks: u64::MAX,
					..courier_first_skill_lifecycle()
				},
				9,
			),
			Err(ChargeCostGrantCadenceError::TickOverflow)
		);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn the_couriers_two_skills_repeat_at_the_ticks_replay_grants_at() {
		let local: LocalSnapshots = local_snapshots();

		// 讯使's automatic first skill: replay casts it at 510 and every 900 ticks after, each
		// cast granting nine DP. The same three numbers are pinned in the 1-1 replay baseline.
		let first = decode(&local, &maximal_build(&local, "char_198_blackd", 1));
		assert_eq!(
			first
				.grant_cadences(
					local
						.snapshot
						.operators()
						.resolve(&maximal_build(&local, "char_198_blackd", 1))
						.expect("resolve 讯使's first skill")
						.selected_skill(),
					tick_rate(),
				)
				.expect("the reviewed automatic owner has a cadence"),
			[ChargeCostGrantCadence {
				first_offset_ticks: 510,
				period_ticks: 900,
				amount: 9,
			}]
		);

		// Her manual second skill lasts fifteen seconds and grants three DP a cast. Nothing in a
		// retained slot casts it, so only the period is what replay can be checked against: the
		// 1-1 baseline's daemon-driven run grants at 1290, 2640 and 3990.
		let second = decode(&local, &maximal_build(&local, "char_198_blackd", 2));
		let [cadence] = second
			.grant_cadences(
				local
					.snapshot
					.operators()
					.resolve(&maximal_build(&local, "char_198_blackd", 2))
					.expect("resolve 讯使's second skill")
					.selected_skill(),
				tick_rate(),
			)
			.expect("the reviewed manual owner has a cadence")[..]
		else {
			panic!("讯使's second skill carries exactly one reviewed grant");
		};
		assert_eq!(cadence.period_ticks, 1_350);
		assert_eq!(cadence.amount, 3);
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn local_verified_charge_cost_skill_decodes_and_one_field_drift_fails_closed() {
		let local: LocalSnapshots = local_snapshots();
		let courier = maximal_build(&local, "char_198_blackd", 1);
		let decoded: DecodedChargeCostSkillTemplate = decode(&local, &courier);

		assert_eq!(
			decoded.mechanic_id.as_str(),
			"skcom_charge_cost[2]/template/charge-cost"
		);
		assert_eq!(decoded.selected_skill_id, "skcom_charge_cost[2]");
		assert_eq!(grant_amounts(&decoded), [("cost", 9)]);
		assert_eq!(
			decoded.active_span,
			ChargeCostSkillActiveSpan::Instantaneous
		);
		assert!(decoded.remainder.is_empty(), "{:?}", decoded.remainder);
		assert_eq!(decoded.prefab_sources.len(), 3);
		assert_eq!(decoded.consumed_issues.len(), 4);
		assert_eq!(
			decoded
				.consumed_issues
				.iter()
				.filter(|issue| matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { .. }))
				.count(),
			3
		);
		let [effect]: [TaggedEffect; 1] = decoded
			.activation_effects()
			.try_into()
			.expect("one activation effect");
		assert_eq!(effect.target, EffectTarget::SelfActor);
		assert_eq!(effect.effect, ExpectedEffect::GainDp { amount: 9 });

		// The amount follows the selected level's blackboard, not the operator: the same root
		// serves the α and γ variants at their own grants. 芬 is three-star, so her highest build
		// stops at E1 and skill level 7.
		let fang: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id("char_123_fang".to_owned()),
			1,
			55,
			200,
			1,
			7,
			PotentialRank::P6,
			OperatorModule::None,
			0,
		);
		assert_eq!(grant_amounts(&decode(&local, &fang)), [("cost", 6)]);
		assert_eq!(
			grant_amounts(&decode(&local, &maximal_build(&local, "char_102_texas", 1))),
			[("cost", 12)]
		);

		let resolved = local
			.snapshot
			.operators()
			.resolve(&courier)
			.expect("resolve 讯使's maximal build");
		let selected_skill = resolved.selected_skill();
		let graph: BattlePrefabGraph = common_skill_graph(&local, "skcom_charge_cost");
		let decode_drifted = |graph: &BattlePrefabGraph| {
			try_decode_charge_cost_skill_template(
				graph,
				selected_skill,
				&local.snapshot,
				local.snapshot.resolved_revision(),
			)
		};

		let mut drifted_controller: BattlePrefabGraph = graph.clone();
		drifted_controller
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.script_id == Some(parse_id(SKILL_CONTROLLER_SCRIPT)))
			.expect("reviewed skill controller")
			.raw_fields["_maxTriggerTime"] = json!(1);
		let drift = decode_drifted(&drifted_controller)
			.expect_err("a limited trigger count must fail closed");
		assert!(drift.to_string().contains("$._maxTriggerTime"), "{drift}");

		let mut drifted_buff: BattlePrefabGraph = graph.clone();
		drifted_buff
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.script_id == Some(parse_id(BUFF_ABILITY_SCRIPT)))
			.expect("reviewed buff ability")
			.raw_fields["_buffs"][0]["maxStackCnt"] = json!(2);
		let drift = decode_drifted(&drifted_buff)
			.expect_err("a constant charge_cost field that changes must fail closed");
		assert!(drift.to_string().contains("maxStackCnt"), "{drift}");

		// A charge_cost buff that also modifies attributes is the unreviewed shape 清道夫 and
		// 香草 carry, so it no longer claims the root rather than drifting.
		let mut modified_buff: BattlePrefabGraph = graph.clone();
		let ability = modified_buff
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.script_id == Some(parse_id(BUFF_ABILITY_SCRIPT)))
			.expect("reviewed buff ability");
		let modifier = json!({"attributeType": 1, "formulaItem": 0, "value": 0.1});
		ability.raw_fields["_buffs"][0]["attributes"]["attributeModifiers"] =
			json!([modifier.clone()]);
		ability.buffs[0].attribute_modifiers = vec![modifier];
		assert!(!claims_charge_cost_skill_template(&modified_buff));

		// Only the root's buff ability claims: the same buff on a child object would leave the
		// decoder reading a root ability with nothing to grant.
		let mut child_owned: BattlePrefabGraph = graph.clone();
		child_owned
			.mono_behaviours
			.iter_mut()
			.find(|behavior| behavior.script_id == Some(parse_id(BUFF_ABILITY_SCRIPT)))
			.expect("reviewed buff ability")
			.game_object_index = None;
		assert!(!claims_charge_cost_skill_template(&child_owned));

		let manual_skill_resolved = local
			.snapshot
			.operators()
			.resolve(&maximal_build(&local, "char_198_blackd", 2))
			.expect("resolve 讯使's second skill");
		let mismatch = try_decode_charge_cost_skill_template(
			&graph,
			manual_skill_resolved.selected_skill(),
			&local.snapshot,
			local.snapshot.resolved_revision(),
		)
		.expect_err("a skill level from another root must not borrow this template");
		assert!(mismatch.to_string().contains("prefab ID"), "{mismatch}");
	}

	#[test]
	#[ignore = "requires configured gamedata and battle-prefab snapshots"]
	fn reviewed_owners_grant_and_name_everything_else_as_remainder() {
		let local: LocalSnapshots = local_snapshots();

		// Flametail's first skill is the same owner with its grant under its own key, plus an
		// evasion action on another script that the template does not execute.
		let flametail = decode(&local, &maximal_build(&local, "char_420_flamtl", 1));
		assert_eq!(grant_amounts(&flametail), [("cost", 6)]);
		assert_eq!(
			flametail.active_span,
			ChargeCostSkillActiveSpan::Instantaneous
		);
		assert_eq!(flametail.consumed_issues.len(), 4);
		let [evasion] = flametail.remainder.as_slice() else {
			panic!("{:?}", flametail.remainder);
		};
		assert!(
			evasion.contains("1328426c1a15720df144f67138d0b7b9")
				&& evasion.contains("flamtl_s_1[evade]"),
			"{evasion}"
		);

		// Courier's second skill is manual and lasts fifteen seconds. Its one-time grant strips
		// the buff key from the blackboard name; its periodic grant, which also raises defence, is
		// another template and stays remainder, together with the blackboard entries it reads.
		let courier = decode(&local, &maximal_build(&local, "char_198_blackd", 2));
		assert_eq!(grant_amounts(&courier), [("blackd_s_2[once].cost", 3)]);
		assert_eq!(
			courier.active_span,
			ChargeCostSkillActiveSpan::Timed { seconds: 15.0 }
		);
		assert_eq!(courier.consumed_issues.len(), 4);
		assert_eq!(courier.remainder.len(), 2, "{:?}", courier.remainder);
		assert!(courier.remainder[0].contains("\"blackd_s_2[period]\" (\"periodic_cost\")"));
		assert!(courier.remainder[1].starts_with("blackboard ["));
		assert!(!courier.remainder[1].contains("[once]"));
	}

	#[test]
	#[ignore = "requires configured battle-prefab snapshot"]
	fn unreviewed_charge_cost_owners_stay_unclaimed() {
		let local: LocalSnapshots = local_snapshots();
		// The first four carry their grant on the same buff ability in an unreviewed variant: with
		// an attack modifier and a skill-long lifetime, or under a duration key. The rest attach it
		// from other ability scripts whose timing is not reviewed.
		for prefab_id in [
			"skchr_scave_2",
			"skchr_wyvern_1",
			"skchr_makiri_1",
			"skchr_sleach_3",
			"skchr_texas_2",
			"skchr_saga_2",
			"skchr_buildr_2",
			"skchr_closur_2",
		] {
			let graph: BattlePrefabGraph = common_skill_graph(&local, prefab_id);
			assert!(
				!claims_charge_cost_skill_template(&graph),
				"{prefab_id} must stay outside the reviewed charge_cost owner"
			);
		}
	}
}
