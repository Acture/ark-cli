use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::data::{raw_roots::RawRoots, ArkDataError};

const CHARACTER_TABLE_PATH: &str = "character_table.json";
const BATTLE_EQUIP_TABLE_PATH: &str = "battle_equip_table.json";
const CHAR_PATCH_TABLE_PATH: &str = "char_patch_table.json";
const FAVOR_TABLE_PATH: &str = "favor_table.json";
const RANGE_TABLE_PATH: &str = "range_table.json";
const SKILL_TABLE_PATH: &str = "skill_table.json";
const UNIEQUIP_TABLE_PATH: &str = "uniequip_table.json";

/// Exact operator lookup key supplied by a guide or caller.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OperatorIdentity {
	Id(String),
	Name(String),
}

/// Exact module selection. Only `None` is currently resolvable.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OperatorModule {
	None,
	Id(String),
}

/// Player-facing potential rank. `P1` applies no potential upgrades; `P6` applies all five.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PotentialRank {
	P1,
	P2,
	P3,
	P4,
	P5,
	P6,
}

impl PotentialRank {
	pub const fn number(self) -> u8 {
		match self {
			Self::P1 => 1,
			Self::P2 => 2,
			Self::P3 => 3,
			Self::P4 => 4,
			Self::P5 => 5,
			Self::P6 => 6,
		}
	}

	fn applied_upgrade_count(self) -> usize {
		usize::from(self.number() - 1)
	}

	fn from_upgrade_index(index: usize) -> Result<Self, ArkDataError> {
		match index {
			0 => Ok(Self::P2),
			1 => Ok(Self::P3),
			2 => Ok(Self::P4),
			3 => Ok(Self::P5),
			4 => Ok(Self::P6),
			_ => Err(ArkDataError::InvalidOperatorTable {
				destination_path: CHARACTER_TABLE_PATH,
				reason: "potential rank index exceeds P6".to_owned(),
			}),
		}
	}
}

/// Verified deployment-position category from `character_table.position`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OperatorPosition {
	Melee,
	Ranged,
	All,
	None,
	Unknown(String),
}

impl OperatorPosition {
	fn from_source(value: &str) -> Self {
		match value {
			"MELEE" => Self::Melee,
			"RANGED" => Self::Ranged,
			"ALL" => Self::All,
			"NONE" => Self::None,
			unknown => Self::Unknown(unknown.to_owned()),
		}
	}
}

/// Exact phase keyframe used by the base-panel-only analysis profile.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BasePanelBuild {
	identity: OperatorIdentity,
	elite: u8,
	level: u32,
}

impl BasePanelBuild {
	pub const fn new(identity: OperatorIdentity, elite: u8, level: u32) -> Self {
		Self {
			identity,
			elite,
			level,
		}
	}

	pub const fn identity(&self) -> &OperatorIdentity {
		&self.identity
	}

	pub const fn elite(&self) -> u8 {
		self.elite
	}

	pub const fn level(&self) -> u32 {
		self.level
	}
}

/// Fully specified operator progression requested from one immutable snapshot.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperatorBuild {
	identity: OperatorIdentity,
	elite: u8,
	level: u32,
	trust_percent: u8,
	skill: u8,
	skill_level: u8,
	potential: PotentialRank,
	module: OperatorModule,
	module_level: u8,
}

impl OperatorBuild {
	#[allow(clippy::too_many_arguments)]
	pub const fn new(
		identity: OperatorIdentity,
		elite: u8,
		level: u32,
		trust_percent: u8,
		skill: u8,
		skill_level: u8,
		potential: PotentialRank,
		module: OperatorModule,
		module_level: u8,
	) -> Self {
		Self {
			identity,
			elite,
			level,
			trust_percent,
			skill,
			skill_level,
			potential,
			module,
			module_level,
		}
	}

	pub const fn identity(&self) -> &OperatorIdentity {
		&self.identity
	}

	pub const fn elite(&self) -> u8 {
		self.elite
	}

	pub const fn level(&self) -> u32 {
		self.level
	}

	pub const fn trust_percent(&self) -> u8 {
		self.trust_percent
	}

	pub const fn skill(&self) -> u8 {
		self.skill
	}

	pub const fn skill_level(&self) -> u8 {
		self.skill_level
	}

	pub const fn potential(&self) -> PotentialRank {
		self.potential
	}

	pub const fn module(&self) -> &OperatorModule {
		&self.module
	}

	pub const fn module_level(&self) -> u8 {
		self.module_level
	}
}

/// Typed operator panel stats. Exact builds include only resolved static panel modifiers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperatorBaseStats {
	max_hp: f64,
	attack: f64,
	defense: f64,
	magic_resistance: f64,
	block_count: u32,
	attack_speed: f64,
	base_attack_time_seconds: f64,
	respawn_time_seconds: u32,
}

impl OperatorBaseStats {
	pub const fn max_hp(self) -> f64 {
		self.max_hp
	}

	pub const fn attack(self) -> f64 {
		self.attack
	}

	pub const fn defense(self) -> f64 {
		self.defense
	}

	pub const fn magic_resistance(self) -> f64 {
		self.magic_resistance
	}

	pub const fn block_count(self) -> u32 {
		self.block_count
	}

	pub const fn attack_speed(self) -> f64 {
		self.attack_speed
	}

	pub const fn base_attack_time_seconds(self) -> f64 {
		self.base_attack_time_seconds
	}

	pub const fn respawn_time_seconds(self) -> u32 {
		self.respawn_time_seconds
	}
}

/// Display metadata for a non-panel `CUSTOM` potential row.
///
/// The description is not a machine-readable combat effect. Numerical runtime semantics are
/// represented by the potential-gated trait or talent candidate selected for the exact build.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PotentialCustomEffect {
	rank: PotentialRank,
	description: String,
}

impl PotentialCustomEffect {
	pub const fn rank(&self) -> PotentialRank {
		self.rank
	}

	pub fn description(&self) -> &str {
		&self.description
	}
}

/// One typed blackboard entry retained verbatim for replay.
#[derive(Clone, Debug, PartialEq)]
pub struct BlackboardEntry {
	key: String,
	value: f64,
	value_string: Option<String>,
}

impl BlackboardEntry {
	pub fn key(&self) -> &str {
		&self.key
	}

	pub const fn value(&self) -> f64 {
		self.value
	}

	pub fn value_string(&self) -> Option<&str> {
		self.value_string.as_deref()
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillActivationType {
	Auto,
	Manual,
	Passive,
	Unknown(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillDurationType {
	None,
	Ammo,
	Unknown(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkillSpType {
	IncreaseWithTime,
	IncreaseWhenAttack,
	IncreaseWhenTakenDamage,
	UnknownName(String),
	UnknownCode(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillSpMetadata {
	sp_type: SkillSpType,
	max_charges: u32,
	cost: u32,
	initial: u32,
	increment: f64,
}

impl SkillSpMetadata {
	pub const fn sp_type(&self) -> &SkillSpType {
		&self.sp_type
	}

	pub const fn max_charges(&self) -> u32 {
		self.max_charges
	}

	pub const fn cost(&self) -> u32 {
		self.cost
	}

	pub const fn initial(&self) -> u32 {
		self.initial
	}

	pub const fn increment(&self) -> f64 {
		self.increment
	}
}

/// Exact selected skill level, retained as dynamic replay metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedSkillMetadata {
	skill_id: String,
	override_prefab_key: Option<String>,
	override_token_key: Option<String>,
	name: String,
	description: Option<String>,
	prefab_id: Option<String>,
	activation_type: SkillActivationType,
	duration_type: SkillDurationType,
	duration_seconds: f64,
	range_id: Option<String>,
	sp: SkillSpMetadata,
	blackboard: Vec<BlackboardEntry>,
}

impl SelectedSkillMetadata {
	pub fn skill_id(&self) -> &str {
		&self.skill_id
	}

	pub fn override_prefab_key(&self) -> Option<&str> {
		self.override_prefab_key.as_deref()
	}

	pub fn override_token_key(&self) -> Option<&str> {
		self.override_token_key.as_deref()
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub fn description(&self) -> Option<&str> {
		self.description.as_deref()
	}

	pub fn prefab_id(&self) -> Option<&str> {
		self.prefab_id.as_deref()
	}

	pub const fn activation_type(&self) -> &SkillActivationType {
		&self.activation_type
	}

	pub const fn duration_type(&self) -> &SkillDurationType {
		&self.duration_type
	}

	pub const fn duration_seconds(&self) -> f64 {
		self.duration_seconds
	}

	pub fn range_id(&self) -> Option<&str> {
		self.range_id.as_deref()
	}

	pub const fn sp(&self) -> &SkillSpMetadata {
		&self.sp
	}

	pub fn blackboard(&self) -> &[BlackboardEntry] {
		&self.blackboard
	}
}

/// Conditional character effect retained as replay metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct OperatorEffectCandidate {
	unlock_phase: String,
	unlock_level: u32,
	required_potential_rank: u8,
	prefab_key: Option<String>,
	name: Option<String>,
	description: Option<String>,
	range_id: Option<String>,
	blackboard: Vec<BlackboardEntry>,
	token_key: Option<String>,
	hidden: bool,
}

impl OperatorEffectCandidate {
	pub fn unlock_phase(&self) -> &str {
		&self.unlock_phase
	}

	pub const fn unlock_level(&self) -> u32 {
		self.unlock_level
	}

	pub const fn required_potential_rank(&self) -> u8 {
		self.required_potential_rank
	}

	pub fn prefab_key(&self) -> Option<&str> {
		self.prefab_key.as_deref()
	}

	pub fn name(&self) -> Option<&str> {
		self.name.as_deref()
	}

	pub fn description(&self) -> Option<&str> {
		self.description.as_deref()
	}

	pub fn range_id(&self) -> Option<&str> {
		self.range_id.as_deref()
	}

	pub fn blackboard(&self) -> &[BlackboardEntry] {
		&self.blackboard
	}

	pub fn token_key(&self) -> Option<&str> {
		self.token_key.as_deref()
	}

	pub const fn hidden(&self) -> bool {
		self.hidden
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperatorTalentGroup {
	candidates: Vec<OperatorEffectCandidate>,
}

impl OperatorTalentGroup {
	pub fn candidates(&self) -> &[OperatorEffectCandidate] {
		&self.candidates
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LegalOperatorModule {
	module_id: String,
	max_level: u8,
	unlock_elite: u8,
	unlock_level: u32,
}

impl LegalOperatorModule {
	pub fn module_id(&self) -> &str {
		&self.module_id
	}

	pub const fn max_level(&self) -> u8 {
		self.max_level
	}

	pub const fn unlock_elite(&self) -> u8 {
		self.unlock_elite
	}

	pub const fn unlock_level(&self) -> u32 {
		self.unlock_level
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OperatorModulePartTarget {
	Display,
	OverwriteBattleData,
	Talent,
	TalentDataOnly,
	Trait,
	TraitDataOnly,
	Unknown(String),
}

/// Dynamic module talent candidate retained without applying its blackboard to the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct OperatorModuleTalentCandidate {
	display_range_id: bool,
	upgrade_description: Option<String>,
	talent_index: i32,
	valid_mode_indices: Option<Vec<i32>>,
	unlock_phase: String,
	unlock_level: u32,
	required_potential_rank: u8,
	prefab_key: Option<String>,
	name: Option<String>,
	description: Option<String>,
	range_id: Option<String>,
	blackboard: Vec<BlackboardEntry>,
	token_key: Option<String>,
	hidden: bool,
}

impl OperatorModuleTalentCandidate {
	pub const fn display_range_id(&self) -> bool {
		self.display_range_id
	}

	pub fn upgrade_description(&self) -> Option<&str> {
		self.upgrade_description.as_deref()
	}

	pub const fn talent_index(&self) -> i32 {
		self.talent_index
	}

	pub fn valid_mode_indices(&self) -> Option<&[i32]> {
		self.valid_mode_indices.as_deref()
	}

	pub fn unlock_phase(&self) -> &str {
		&self.unlock_phase
	}

	pub const fn unlock_level(&self) -> u32 {
		self.unlock_level
	}

	pub const fn required_potential_rank(&self) -> u8 {
		self.required_potential_rank
	}

	pub fn prefab_key(&self) -> Option<&str> {
		self.prefab_key.as_deref()
	}

	pub fn name(&self) -> Option<&str> {
		self.name.as_deref()
	}

	pub fn description(&self) -> Option<&str> {
		self.description.as_deref()
	}

	pub fn range_id(&self) -> Option<&str> {
		self.range_id.as_deref()
	}

	pub fn blackboard(&self) -> &[BlackboardEntry] {
		&self.blackboard
	}

	pub fn token_key(&self) -> Option<&str> {
		self.token_key.as_deref()
	}

	pub const fn hidden(&self) -> bool {
		self.hidden
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperatorModuleTraitCandidate {
	additional_description: Option<String>,
	override_description: Option<String>,
	unlock_phase: String,
	unlock_level: u32,
	required_potential_rank: u8,
	prefab_key: Option<String>,
	range_id: Option<String>,
	blackboard: Vec<BlackboardEntry>,
}

impl OperatorModuleTraitCandidate {
	pub fn additional_description(&self) -> Option<&str> {
		self.additional_description.as_deref()
	}

	pub fn override_description(&self) -> Option<&str> {
		self.override_description.as_deref()
	}

	pub fn unlock_phase(&self) -> &str {
		&self.unlock_phase
	}

	pub const fn unlock_level(&self) -> u32 {
		self.unlock_level
	}

	pub const fn required_potential_rank(&self) -> u8 {
		self.required_potential_rank
	}

	pub fn prefab_key(&self) -> Option<&str> {
		self.prefab_key.as_deref()
	}

	pub fn range_id(&self) -> Option<&str> {
		self.range_id.as_deref()
	}

	pub fn blackboard(&self) -> &[BlackboardEntry] {
		&self.blackboard
	}
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperatorModulePart {
	resource_key: Option<String>,
	target: OperatorModulePartTarget,
	is_token: bool,
	valid_in_game_tag: Option<String>,
	valid_in_map_tag: Option<String>,
	talent_candidates: Vec<OperatorModuleTalentCandidate>,
	trait_candidates: Vec<OperatorModuleTraitCandidate>,
}

impl OperatorModulePart {
	pub fn resource_key(&self) -> Option<&str> {
		self.resource_key.as_deref()
	}

	pub const fn target(&self) -> &OperatorModulePartTarget {
		&self.target
	}

	pub const fn is_token(&self) -> bool {
		self.is_token
	}

	pub fn valid_in_game_tag(&self) -> Option<&str> {
		self.valid_in_game_tag.as_deref()
	}

	pub fn valid_in_map_tag(&self) -> Option<&str> {
		self.valid_in_map_tag.as_deref()
	}

	pub fn talent_candidates(&self) -> &[OperatorModuleTalentCandidate] {
		&self.talent_candidates
	}

	pub fn trait_candidates(&self) -> &[OperatorModuleTraitCandidate] {
		&self.trait_candidates
	}
}

/// Selected real module level with static and dynamic payloads kept separate.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedOperatorModule {
	module_id: String,
	level: u8,
	attribute_blackboard: Vec<BlackboardEntry>,
	parts: Vec<OperatorModulePart>,
	token_attribute_blackboard: BTreeMap<String, Vec<BlackboardEntry>>,
}

impl ResolvedOperatorModule {
	pub fn module_id(&self) -> &str {
		&self.module_id
	}

	pub const fn level(&self) -> u8 {
		self.level
	}

	pub fn attribute_blackboard(&self) -> &[BlackboardEntry] {
		&self.attribute_blackboard
	}

	pub fn parts(&self) -> &[OperatorModulePart] {
		&self.parts
	}

	pub const fn token_attribute_blackboard(&self) -> &BTreeMap<String, Vec<BlackboardEntry>> {
		&self.token_attribute_blackboard
	}
}

/// Canonical, revision-pinned result of resolving an exact supported build.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedOperatorBuild {
	resolved_revision: String,
	stable_id: String,
	operator_id: String,
	name: String,
	position: OperatorPosition,
	profession: String,
	group_id: Option<String>,
	sub_profession_id: String,
	build: OperatorBuild,
	skill_id: String,
	selected_skill: SelectedSkillMetadata,
	range_id: String,
	deployment_cost: u32,
	base_stats: OperatorBaseStats,
	trust_battle_phase: u32,
	custom_potential_effects: Vec<PotentialCustomEffect>,
	selected_module: Option<ResolvedOperatorModule>,
	trait_candidates: Vec<OperatorEffectCandidate>,
	talent_groups: Vec<OperatorTalentGroup>,
}

/// Revision-pinned phase-keyframe stats without trust, talents, skills, potential, or modules.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedBasePanelBuild {
	resolved_revision: String,
	stable_id: String,
	operator_id: String,
	name: String,
	position: OperatorPosition,
	profession: String,
	sub_profession_id: String,
	build: BasePanelBuild,
	range_id: String,
	deployment_cost: u32,
	base_stats: OperatorBaseStats,
}

impl ResolvedBasePanelBuild {
	pub fn resolved_revision(&self) -> &str {
		&self.resolved_revision
	}

	pub fn stable_id(&self) -> &str {
		&self.stable_id
	}

	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub const fn position(&self) -> &OperatorPosition {
		&self.position
	}

	pub fn profession(&self) -> &str {
		&self.profession
	}

	pub fn sub_profession_id(&self) -> &str {
		&self.sub_profession_id
	}

	pub const fn build(&self) -> &BasePanelBuild {
		&self.build
	}

	pub fn range_id(&self) -> &str {
		&self.range_id
	}

	pub const fn deployment_cost(&self) -> u32 {
		self.deployment_cost
	}

	pub const fn base_stats(&self) -> OperatorBaseStats {
		self.base_stats
	}
}

impl ResolvedOperatorBuild {
	pub fn resolved_revision(&self) -> &str {
		&self.resolved_revision
	}

	pub fn stable_id(&self) -> &str {
		&self.stable_id
	}

	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub const fn position(&self) -> &OperatorPosition {
		&self.position
	}

	pub fn profession(&self) -> &str {
		&self.profession
	}

	pub fn group_id(&self) -> Option<&str> {
		self.group_id.as_deref()
	}

	pub fn profession_category_mask(&self) -> Option<u16> {
		match self.profession.as_str() {
			"WARRIOR" => Some(1),
			"SNIPER" => Some(2),
			"TANK" => Some(4),
			"MEDIC" => Some(8),
			"SUPPORT" => Some(16),
			"CASTER" => Some(32),
			"SPECIAL" => Some(64),
			"PIONEER" => Some(512),
			_ => None,
		}
	}

	pub fn sub_profession_id(&self) -> &str {
		&self.sub_profession_id
	}

	/// Returns the canonical build, whose identity is always the resolved operator ID.
	pub const fn build(&self) -> &OperatorBuild {
		&self.build
	}

	pub fn skill_id(&self) -> &str {
		&self.skill_id
	}

	pub const fn selected_skill(&self) -> &SelectedSkillMetadata {
		&self.selected_skill
	}

	pub fn range_id(&self) -> &str {
		&self.range_id
	}

	pub const fn deployment_cost(&self) -> u32 {
		self.deployment_cost
	}

	pub const fn base_stats(&self) -> OperatorBaseStats {
		self.base_stats
	}

	/// Returns the fully resolved static panel, excluding conditional combat effects.
	pub const fn static_stats(&self) -> OperatorBaseStats {
		self.base_stats
	}

	pub const fn trust_battle_phase(&self) -> u32 {
		self.trust_battle_phase
	}

	/// Returns display-only metadata for applied `CUSTOM` potential rows.
	///
	/// Runtime values come from [`Self::selected_trait_candidate`] and
	/// [`Self::selected_talent_candidates`], not from matching these descriptions.
	pub fn custom_potential_effects(&self) -> &[PotentialCustomEffect] {
		&self.custom_potential_effects
	}

	/// Reports whether an applied `CUSTOM` potential row is represented by selected effect data.
	///
	/// Absorption is structural: a trait or talent group must contain a candidate at the
	/// potential's exact upgrade threshold, and that group's selected candidate must be at that
	/// threshold or a later one. Descriptions are never matched.
	pub fn custom_potential_effect_is_absorbed(&self, effect: &PotentialCustomEffect) -> bool {
		let threshold: u8 = effect.rank().number().saturating_sub(1);
		effect_group_absorbs_custom_potential(
			&self.trait_candidates,
			self.selected_trait_candidate(),
			threshold,
		) || self
			.talent_groups
			.iter()
			.any(|group: &OperatorTalentGroup| {
				effect_group_absorbs_custom_potential(
					group.candidates(),
					select_effect_candidate(group.candidates(), &self.build),
					threshold,
				)
			})
	}

	pub const fn selected_module(&self) -> Option<&ResolvedOperatorModule> {
		self.selected_module.as_ref()
	}

	pub fn trait_candidates(&self) -> &[OperatorEffectCandidate] {
		&self.trait_candidates
	}

	pub fn talent_groups(&self) -> &[OperatorTalentGroup] {
		&self.talent_groups
	}

	/// Returns the active trait candidate for this exact elite, level, and potential build.
	///
	/// `requiredPotentialRank` is the number of applied potential upgrades (`0` at P1 through
	/// `5` at P6). The highest unlocked candidate is selected without interpreting descriptions.
	pub fn selected_trait_candidate(&self) -> Option<&OperatorEffectCandidate> {
		select_effect_candidate(&self.trait_candidates, &self.build)
	}

	/// Iterates over the active candidate from each unlocked talent group for this exact build.
	///
	/// Potential-gated candidates carry the numerical runtime semantics associated with `CUSTOM`
	/// potential rows; callers must not infer those semantics from the display description.
	pub fn selected_talent_candidates(
		&self,
	) -> impl Iterator<Item = &OperatorEffectCandidate> + '_ {
		self.talent_groups
			.iter()
			.filter_map(|group: &OperatorTalentGroup| {
				select_effect_candidate(group.candidates(), &self.build)
			})
	}
}

fn select_effect_candidate<'a>(
	candidates: &'a [OperatorEffectCandidate],
	build: &OperatorBuild,
) -> Option<&'a OperatorEffectCandidate> {
	candidates
		.iter()
		.filter(|candidate: &&OperatorEffectCandidate| effect_candidate_is_active(candidate, build))
		.max_by_key(|candidate: &&OperatorEffectCandidate| {
			(
				effect_candidate_elite(candidate).unwrap_or_default(),
				candidate.unlock_level(),
				candidate.required_potential_rank(),
			)
		})
}

fn effect_candidate_is_active(candidate: &OperatorEffectCandidate, build: &OperatorBuild) -> bool {
	let Some(required_elite) = effect_candidate_elite(candidate) else {
		return false;
	};
	let level_unlocked: bool = build.elite() > required_elite
		|| (build.elite() == required_elite && build.level() >= candidate.unlock_level());
	let applied_potential_rank: u8 = build.potential().number().saturating_sub(1);
	level_unlocked && applied_potential_rank >= candidate.required_potential_rank()
}

fn effect_candidate_elite(candidate: &OperatorEffectCandidate) -> Option<u8> {
	candidate
		.unlock_phase()
		.strip_prefix("PHASE_")?
		.parse::<u8>()
		.ok()
}

fn effect_group_absorbs_custom_potential(
	candidates: &[OperatorEffectCandidate],
	selected: Option<&OperatorEffectCandidate>,
	threshold: u8,
) -> bool {
	let threshold_exists: bool = candidates
		.iter()
		.any(|candidate: &OperatorEffectCandidate| {
			candidate.required_potential_rank() == threshold
		});
	threshold_exists
		&& selected.is_some_and(|candidate: &OperatorEffectCandidate| {
			candidate.required_potential_rank() >= threshold
		})
}

/// Immutable operator and skill index compiled from verified snapshot roots.
#[derive(Debug)]
pub struct OperatorCatalog {
	resolved_revision: String,
	records: BTreeMap<String, RawCharacter>,
	ids_by_name: BTreeMap<String, Vec<String>>,
	skill_records: BTreeMap<String, RawSkill>,
	range_ids: BTreeSet<String>,
	favor_battle_phases: BTreeMap<u8, u32>,
	char_equip: BTreeMap<String, Vec<String>>,
	module_records: BTreeMap<String, RawUniEquip>,
	battle_equip_records: BTreeMap<String, RawBattleEquip>,
}

impl OperatorCatalog {
	pub(in crate::data) fn from_roots(
		roots: &RawRoots,
		resolved_revision: String,
	) -> Result<Self, ArkDataError> {
		let character_table: &Map<String, Value> = root_object(roots, CHARACTER_TABLE_PATH)?;
		let mut records: BTreeMap<String, RawCharacter> = BTreeMap::new();
		let mut ids_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
		for (operator_id, value) in character_table {
			if operator_id.trim().is_empty() {
				return Err(ArkDataError::InvalidOperatorTable {
					destination_path: CHARACTER_TABLE_PATH,
					reason: "operator ID must not be blank".to_owned(),
				});
			}
			let record: RawCharacter = serde_json::from_value(value.clone()).map_err(|source| {
				ArkDataError::InvalidOperatorRecord {
					operator_id: operator_id.clone(),
					field: "record",
					reason: source.to_string(),
				}
			})?;
			if let Some(name) = record
				.name
				.as_deref()
				.filter(|name| !name.trim().is_empty())
			{
				ids_by_name
					.entry(name.to_owned())
					.or_default()
					.push(operator_id.clone());
			}
			records.insert(operator_id.clone(), record);
		}
		let char_patch_table: &Map<String, Value> = root_object(roots, CHAR_PATCH_TABLE_PATH)?;
		let patch_chars: Option<&Map<String, Value>> = match char_patch_table.get("patchChars") {
			None => None,
			Some(value) => {
				Some(
					value
						.as_object()
						.ok_or_else(|| ArkDataError::InvalidOperatorTable {
							destination_path: CHAR_PATCH_TABLE_PATH,
							reason: "patchChars must be an object".to_owned(),
						})?,
				)
			}
		};
		for (operator_id, value) in patch_chars.into_iter().flatten() {
			if operator_id.trim().is_empty() {
				return Err(ArkDataError::InvalidOperatorTable {
					destination_path: CHAR_PATCH_TABLE_PATH,
					reason: "patch form ID must not be blank".to_owned(),
				});
			}
			if records.contains_key(operator_id) {
				return Err(ArkDataError::InvalidOperatorTable {
					destination_path: CHAR_PATCH_TABLE_PATH,
					reason: format!("duplicate operator form ID {operator_id:?}"),
				});
			}
			let record: RawCharacter = serde_json::from_value(value.clone()).map_err(|source| {
				ArkDataError::InvalidOperatorRecord {
					operator_id: operator_id.clone(),
					field: "record",
					reason: source.to_string(),
				}
			})?;
			if let Some(name) = record
				.name
				.as_deref()
				.filter(|name: &&str| !name.trim().is_empty())
			{
				ids_by_name
					.entry(name.to_owned())
					.or_default()
					.push(operator_id.clone());
			}
			records.insert(operator_id.clone(), record);
		}
		for ids in ids_by_name.values_mut() {
			ids.sort();
		}

		let skill_table: &Map<String, Value> = root_object(roots, SKILL_TABLE_PATH)?;
		let mut skill_records: BTreeMap<String, RawSkill> = BTreeMap::new();
		for (skill_id, value) in skill_table {
			let record: RawSkill = serde_json::from_value(value.clone()).map_err(|source| {
				ArkDataError::InvalidOperatorTable {
					destination_path: SKILL_TABLE_PATH,
					reason: format!("skill {skill_id:?}: {source}"),
				}
			})?;
			skill_records.insert(skill_id.clone(), record);
		}
		let range_ids: BTreeSet<String> = root_object(roots, RANGE_TABLE_PATH)?
			.keys()
			.cloned()
			.collect();
		let favor_table: RawFavorTable =
			serde_json::from_value(roots.table(FAVOR_TABLE_PATH).cloned().ok_or(
				ArkDataError::InvalidOperatorTable {
					destination_path: FAVOR_TABLE_PATH,
					reason: "root is missing".to_owned(),
				},
			)?)
			.map_err(|source| ArkDataError::InvalidOperatorTable {
				destination_path: FAVOR_TABLE_PATH,
				reason: source.to_string(),
			})?;
		let mut favor_battle_phases: BTreeMap<u8, u32> = BTreeMap::new();
		for frame in favor_table.favor_frames {
			let percent: u8 = frame.data.percent;
			if percent > 200 {
				return Err(ArkDataError::InvalidOperatorTable {
					destination_path: FAVOR_TABLE_PATH,
					reason: "favor percent exceeds 200".to_owned(),
				});
			}
			if favor_battle_phases
				.insert(percent, frame.data.battle_phase)
				.is_some()
			{
				return Err(ArkDataError::InvalidOperatorTable {
					destination_path: FAVOR_TABLE_PATH,
					reason: format!("duplicate favor percent {percent}"),
				});
			}
		}
		let uniequip_table: RawUniEquipTable =
			serde_json::from_value(roots.table(UNIEQUIP_TABLE_PATH).cloned().ok_or(
				ArkDataError::InvalidOperatorTable {
					destination_path: UNIEQUIP_TABLE_PATH,
					reason: "root is missing".to_owned(),
				},
			)?)
			.map_err(|source| ArkDataError::InvalidOperatorTable {
				destination_path: UNIEQUIP_TABLE_PATH,
				reason: source.to_string(),
			})?;
		let battle_equip_table: &Map<String, Value> = root_object(roots, BATTLE_EQUIP_TABLE_PATH)?;
		let mut battle_equip_records: BTreeMap<String, RawBattleEquip> = BTreeMap::new();
		for (module_id, value) in battle_equip_table {
			let record: RawBattleEquip =
				serde_json::from_value(value.clone()).map_err(|source| {
					ArkDataError::InvalidOperatorTable {
						destination_path: BATTLE_EQUIP_TABLE_PATH,
						reason: format!("module {module_id:?}: {source}"),
					}
				})?;
			battle_equip_records.insert(module_id.clone(), record);
		}

		Ok(Self {
			resolved_revision,
			records,
			ids_by_name,
			skill_records,
			range_ids,
			favor_battle_phases,
			char_equip: uniequip_table.char_equip,
			module_records: uniequip_table.equip_dict,
			battle_equip_records,
		})
	}

	/// Enumerates operator records, skipping the character table's token and trap records.
	pub fn operator_ids(&self) -> impl Iterator<Item = &str> {
		self.records
			.iter()
			.filter(|(_, record): &(&String, &RawCharacter)| {
				!matches!(record.profession.as_deref(), Some("TOKEN" | "TRAP"))
			})
			.map(|(operator_id, _): (&String, &RawCharacter)| operator_id.as_str())
	}

	/// Enumerates unlocked real modules owned by one exact form for an ordinary stage.
	pub fn legal_modules(
		&self,
		operator_id_or_name: &str,
		elite: u8,
		level: u32,
	) -> Result<Vec<LegalOperatorModule>, ArkDataError> {
		let operator_id: &str = self.resolve_id_or_name(operator_id_or_name)?;
		let record: &RawCharacter = self
			.records
			.get(operator_id)
			.expect("resolved operator ID must have a record");
		let phase: &RawPhase = record
			.phases
			.as_deref()
			.unwrap_or_default()
			.get(usize::from(elite))
			.ok_or_else(|| invalid_build(operator_id, "elite", "phase does not exist"))?;
		let max_level: u32 = phase
			.max_level
			.ok_or_else(|| invalid_record(operator_id, "phases[].maxLevel", "value is missing"))?;
		if level == 0 || level > max_level {
			return Err(invalid_build(
				operator_id,
				"level",
				"level is outside the selected phase",
			));
		}
		let mut legal: Vec<LegalOperatorModule> = Vec::new();
		for module_id in self
			.char_equip
			.get(operator_id)
			.map(Vec::as_slice)
			.unwrap_or_default()
		{
			let metadata: &RawUniEquip = self.module_records.get(module_id).ok_or_else(|| {
				invalid_record(
					operator_id,
					"uniequip_table.charEquip",
					"listed module is absent from equipDict",
				)
			})?;
			if metadata.module_id != *module_id {
				return Err(invalid_record(
					operator_id,
					"uniequip_table.equipDict[].uniEquipId",
					"module map key does not match module ID",
				));
			}
			let owner_id: &str = metadata
				.template_id
				.as_deref()
				.unwrap_or(&metadata.character_id);
			if owner_id != operator_id {
				return Err(invalid_record(
					operator_id,
					"uniequip_table.equipDict[].charId",
					"module is not owned by the exact operator form",
				));
			}
			match metadata.kind.as_str() {
				"INITIAL" => continue,
				"ADVANCED" if metadata.is_special => continue,
				"ADVANCED" => {}
				_ => {
					return Err(unsupported(
						operator_id,
						"uniequip_table.equipDict[].type",
						"unknown module type",
					));
				}
			}
			let unlock_elite: u8 = parse_phase_index(
				operator_id,
				"uniequip_table.equipDict[].unlockEvolvePhase",
				&metadata.unlock_phase,
			)?;
			if elite < unlock_elite || (elite == unlock_elite && level < metadata.unlock_level) {
				continue;
			}
			let battle_record: &RawBattleEquip =
				self.battle_equip_records.get(module_id).ok_or_else(|| {
					invalid_record(
						operator_id,
						"battle_equip_table",
						"real module has no battle record",
					)
				})?;
			let max_module_level: u8 = battle_record
				.phases
				.iter()
				.map(|phase: &RawBattleEquipPhase| phase.level)
				.max()
				.ok_or_else(|| {
					invalid_record(
						operator_id,
						"battle_equip_table.phases",
						"real module has no phases",
					)
				})?;
			legal.push(LegalOperatorModule {
				module_id: module_id.clone(),
				max_level: max_module_level,
				unlock_elite,
				unlock_level: metadata.unlock_level,
			});
		}
		legal.sort_by(|left: &LegalOperatorModule, right: &LegalOperatorModule| {
			left.module_id.cmp(&right.module_id)
		});
		Ok(legal)
	}

	/// Enumerates the exact max-static variants for one operator and selected skill slot.
	pub fn maximal_builds(
		&self,
		operator_id_or_name: &str,
		selected_skill: u8,
	) -> Result<Vec<OperatorBuild>, ArkDataError> {
		let operator_id: &str = self.resolve_id_or_name(operator_id_or_name)?;
		let record: &RawCharacter = self
			.records
			.get(operator_id)
			.expect("resolved operator ID must have a record");
		let elite: u8 = 2;
		let phase: &RawPhase = record
			.phases
			.as_deref()
			.unwrap_or_default()
			.get(usize::from(elite))
			.ok_or_else(|| invalid_build(operator_id, "elite", "E2 phase does not exist"))?;
		let level: u32 = phase
			.max_level
			.ok_or_else(|| invalid_record(operator_id, "phases[].maxLevel", "value is missing"))?;
		let legal_modules: Vec<LegalOperatorModule> =
			self.legal_modules(operator_id, elite, level)?;
		let modules: Vec<(OperatorModule, u8)> = if legal_modules.is_empty() {
			vec![(OperatorModule::None, 0)]
		} else {
			legal_modules
				.into_iter()
				.map(|module: LegalOperatorModule| {
					(OperatorModule::Id(module.module_id), module.max_level)
				})
				.collect()
		};
		let mut builds: Vec<OperatorBuild> = Vec::with_capacity(modules.len());
		for (module, module_level) in modules {
			let build: OperatorBuild = OperatorBuild::new(
				OperatorIdentity::Id(operator_id.to_owned()),
				elite,
				level,
				200,
				selected_skill,
				10,
				PotentialRank::P6,
				module,
				module_level,
			);
			self.resolve(&build)?;
			builds.push(build);
		}
		Ok(builds)
	}

	/// Returns the highest phase's maximum-level base-panel keyframe for one exact operator ID.
	pub fn highest_phase_base_panel(
		&self,
		operator_id: &str,
	) -> Result<BasePanelBuild, ArkDataError> {
		let record: &RawCharacter =
			self.records
				.get(operator_id)
				.ok_or_else(|| ArkDataError::OperatorNotFound {
					identity: OperatorIdentity::Id(operator_id.to_owned()),
				})?;
		let (elite, phase): (usize, &RawPhase) = record
			.phases
			.as_deref()
			.unwrap_or_default()
			.iter()
			.enumerate()
			.rev()
			.find(|(_, phase): &(usize, &RawPhase)| phase.max_level.is_some())
			.ok_or_else(|| invalid_record(operator_id, "phases", "no phase has a maximum level"))?;
		let elite: u8 = u8::try_from(elite)
			.map_err(|_| invalid_record(operator_id, "phases", "phase index exceeds u8"))?;
		let level: u32 = phase.max_level.expect("selected phase has a maximum level");
		Ok(BasePanelBuild::new(
			OperatorIdentity::Id(operator_id.to_owned()),
			elite,
			level,
		))
	}

	/// Resolves only the explicitly declared base-panel phase keyframe.
	pub fn resolve_base_panel(
		&self,
		build: &BasePanelBuild,
	) -> Result<ResolvedBasePanelBuild, ArkDataError> {
		let operator_id: &str = self.resolve_id(build.identity())?;
		let record: &RawCharacter = self
			.records
			.get(operator_id)
			.expect("resolved operator ID must have a record");
		let name: &str = record
			.name
			.as_deref()
			.filter(|name| !name.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "name", "name must not be blank"))?;
		let position: OperatorPosition = record
			.position
			.as_deref()
			.map(OperatorPosition::from_source)
			.ok_or_else(|| invalid_record(operator_id, "position", "value is missing"))?;
		let profession: &str = record
			.profession
			.as_deref()
			.filter(|value: &&str| !value.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "profession", "value is missing"))?;
		let sub_profession_id: &str = record
			.sub_profession_id
			.as_deref()
			.filter(|value: &&str| !value.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "subProfessionId", "value is missing"))?;
		let phase: &RawPhase = record
			.phases
			.as_deref()
			.unwrap_or_default()
			.get(usize::from(build.elite()))
			.ok_or_else(|| invalid_build(operator_id, "elite", "phase does not exist"))?;
		let max_level: u32 = phase
			.max_level
			.ok_or_else(|| invalid_record(operator_id, "phases[].maxLevel", "value is missing"))?;
		if build.level() == 0 || build.level() > max_level {
			return Err(invalid_build(
				operator_id,
				"level",
				"level is outside the selected phase",
			));
		}
		let frame: &RawAttributeKeyFrame = phase
			.attribute_key_frames
			.as_deref()
			.unwrap_or_default()
			.iter()
			.find(|frame: &&RawAttributeKeyFrame| frame.level == Some(build.level()))
			.ok_or_else(|| {
				unsupported(
					operator_id,
					"level",
					"level is not an exact character-table keyframe",
				)
			})?;
		let range_id: &str = phase
			.range_id
			.as_deref()
			.filter(|range_id| !range_id.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "phases[].rangeId", "value is missing"))?;
		if !self.range_ids.contains(range_id) {
			return Err(invalid_record(
				operator_id,
				"phases[].rangeId",
				"range is absent from the verified range table",
			));
		}
		let raw_stats: RawBaseStats = frame.data.clone();
		let (base_stats, deployment_cost): (OperatorBaseStats, u32) =
			raw_stats.compile(operator_id)?;
		let canonical_build: BasePanelBuild = BasePanelBuild::new(
			OperatorIdentity::Id(operator_id.to_owned()),
			build.elite(),
			build.level(),
		);
		let stable_id: String = format!(
			"{}:{operator_id}:base-panel:e{}:l{}",
			self.resolved_revision,
			build.elite(),
			build.level()
		);
		Ok(ResolvedBasePanelBuild {
			resolved_revision: self.resolved_revision.clone(),
			stable_id,
			operator_id: operator_id.to_owned(),
			name: name.to_owned(),
			position,
			profession: profession.to_owned(),
			sub_profession_id: sub_profession_id.to_owned(),
			build: canonical_build,
			range_id: range_id.to_owned(),
			deployment_cost,
			base_stats,
		})
	}

	pub fn resolve(&self, build: &OperatorBuild) -> Result<ResolvedOperatorBuild, ArkDataError> {
		let operator_id: &str = self.resolve_id(build.identity())?;
		let record: &RawCharacter = self
			.records
			.get(operator_id)
			.expect("resolved operator ID must have a record");
		let name: &str = record
			.name
			.as_deref()
			.filter(|name| !name.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "name", "name must not be blank"))?;
		let position: OperatorPosition = record
			.position
			.as_deref()
			.map(OperatorPosition::from_source)
			.ok_or_else(|| invalid_record(operator_id, "position", "value is missing"))?;
		let profession: &str = record
			.profession
			.as_deref()
			.filter(|value: &&str| !value.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "profession", "value is missing"))?;
		let sub_profession_id: &str = record
			.sub_profession_id
			.as_deref()
			.filter(|value: &&str| !value.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "subProfessionId", "value is missing"))?;

		if build.trust_percent() > 200 {
			return Err(invalid_build(
				operator_id,
				"trust_percent",
				"trust must be in 0..=200",
			));
		}
		match build.module() {
			OperatorModule::None if build.module_level() == 0 => {}
			OperatorModule::None => {
				return Err(invalid_build(
					operator_id,
					"module_level",
					"module level must be 0 when no module is selected",
				));
			}
			OperatorModule::Id(_) if build.module_level() == 0 => {
				return Err(invalid_build(
					operator_id,
					"module_level",
					"real modules start at level 1",
				));
			}
			OperatorModule::Id(_) => {}
		}

		let phases: &[RawPhase] = record.phases.as_deref().unwrap_or_default();
		let phase: &RawPhase = phases
			.get(usize::from(build.elite()))
			.ok_or_else(|| invalid_build(operator_id, "elite", "phase does not exist"))?;
		let max_level: u32 = phase
			.max_level
			.ok_or_else(|| invalid_record(operator_id, "phases[].maxLevel", "value is missing"))?;
		if build.level() == 0 || build.level() > max_level {
			return Err(invalid_build(
				operator_id,
				"level",
				"level is outside the selected phase",
			));
		}
		let mut raw_stats: RawBaseStats = interpolate_keyframes(
			operator_id,
			phase.attribute_key_frames.as_deref().unwrap_or_default(),
			build.level(),
			"phases[].attributesKeyFrames",
		)?;
		let range_id: &str = phase
			.range_id
			.as_deref()
			.filter(|range_id| !range_id.trim().is_empty())
			.ok_or_else(|| invalid_record(operator_id, "phases[].rangeId", "value is missing"))?;
		if !self.range_ids.contains(range_id) {
			return Err(invalid_record(
				operator_id,
				"phases[].rangeId",
				"range is absent from the verified range table",
			));
		}

		let skills: &[RawCharacterSkill] = record.skills.as_deref().unwrap_or_default();
		let skill_index: usize = usize::from(build.skill().checked_sub(1).ok_or_else(|| {
			invalid_build(
				operator_id,
				"skill",
				"skill is one-based and must be positive",
			)
		})?);
		let skill_id: &str = skills
			.get(skill_index)
			.and_then(|skill| skill.skill_id.as_deref())
			.filter(|skill_id| !skill_id.trim().is_empty())
			.ok_or_else(|| invalid_build(operator_id, "skill", "skill slot does not exist"))?;
		let character_skill: &RawCharacterSkill =
			skills.get(skill_index).expect("selected skill slot exists");
		let skill_unlock: &RawUnlockCondition =
			character_skill.unlock_condition.as_ref().ok_or_else(|| {
				invalid_record(
					operator_id,
					"skills[].unlockCond",
					"selected skill unlock condition is missing",
				)
			})?;
		validate_unlock_condition(
			operator_id,
			"skills[].unlockCond",
			skill_unlock,
			build.elite(),
			build.level(),
		)?;
		let skill_record: &RawSkill = self.skill_records.get(skill_id).ok_or_else(|| {
			invalid_record(
				operator_id,
				"skills[].skillId",
				"skill is absent from the verified skill table",
			)
		})?;
		let skill_levels: &[RawSkillLevel] = skill_record.levels.as_deref().unwrap_or_default();
		if build.skill_level() == 0 || usize::from(build.skill_level()) > skill_levels.len() {
			return Err(invalid_build(
				operator_id,
				"skill_level",
				"skill level does not exist",
			));
		}
		let common_upgrade_count: usize = usize::from(build.skill_level().min(7).saturating_sub(1));
		let common_conditions: &[RawSkillLevelUpCondition] = record
			.all_skill_level_up_conditions
			.as_deref()
			.unwrap_or_default();
		if common_conditions.len() < common_upgrade_count {
			return Err(invalid_build(
				operator_id,
				"skill_level",
				"common skill unlock condition does not exist",
			));
		}
		for condition in common_conditions.iter().take(common_upgrade_count) {
			let unlock: &RawUnlockCondition =
				condition.unlock_condition.as_ref().ok_or_else(|| {
					invalid_build(
						operator_id,
						"skill_level",
						"common skill unlock condition does not exist",
					)
				})?;
			validate_unlock_condition(
				operator_id,
				"allSkillLvlup[].unlockCond",
				unlock,
				build.elite(),
				build.level(),
			)?;
		}
		if build.skill_level() > 7 {
			let mastery_index: usize = usize::from(build.skill_level() - 8);
			let mastery_unlock: &RawUnlockCondition = character_skill
				.level_up_cost_conditions
				.as_deref()
				.unwrap_or_default()
				.get(mastery_index)
				.and_then(|condition: &RawSkillLevelUpCondition| {
					condition.unlock_condition.as_ref()
				})
				.ok_or_else(|| {
					invalid_build(
						operator_id,
						"skill_level",
						"mastery unlock condition does not exist",
					)
				})?;
			validate_unlock_condition(
				operator_id,
				"skills[].levelUpCostCond[].unlockCond",
				mastery_unlock,
				build.elite(),
				build.level(),
			)?;
		}
		let skill_level_index: usize = usize::from(build.skill_level() - 1);
		let selected_skill: SelectedSkillMetadata = compile_skill_metadata(
			operator_id,
			skill_id,
			character_skill,
			&skill_levels[skill_level_index],
			&self.range_ids,
		)?;
		let trait_candidates: Vec<OperatorEffectCandidate> =
			compile_trait_candidates(operator_id, record, &self.range_ids)?;
		let talent_groups: Vec<OperatorTalentGroup> =
			compile_talent_groups(operator_id, record, &self.range_ids)?;

		let trust_battle_phase: u32 = *self
			.favor_battle_phases
			.get(&build.trust_percent())
			.ok_or_else(|| {
				invalid_record(
					operator_id,
					"favor_table.favorFrames",
					"requested trust percent has no battlePhase",
				)
			})?;
		let favor_frames: &[RawAttributeKeyFrame] =
			record.favor_key_frames.as_deref().unwrap_or_default();
		let favor_stat_cap: u32 = favor_frames
			.iter()
			.filter_map(|frame: &RawAttributeKeyFrame| frame.level)
			.max()
			.ok_or_else(|| {
				invalid_record(
					operator_id,
					"favorKeyFrames",
					"operator has no favor stat keyframes",
				)
			})?;
		let capped_trust_battle_phase: u32 = trust_battle_phase.min(favor_stat_cap);
		let favor_stats: RawBaseStats = interpolate_keyframes(
			operator_id,
			favor_frames,
			capped_trust_battle_phase,
			"favorKeyFrames",
		)?;
		raw_stats.add_assign(&favor_stats);

		let potential_ranks: &[RawPotentialRank] =
			record.potential_ranks.as_deref().unwrap_or_default();
		let upgrade_count: usize = build.potential().applied_upgrade_count();
		if potential_ranks.len() < upgrade_count {
			return Err(invalid_build(
				operator_id,
				"potential",
				"selected potential rank does not exist",
			));
		}
		let mut custom_potential_effects: Vec<PotentialCustomEffect> = Vec::new();
		for (index, rank) in potential_ranks.iter().take(upgrade_count).enumerate() {
			match rank.kind.as_str() {
				"BUFF" => {
					let buff: &RawPotentialBuff = rank.buff.as_ref().ok_or_else(|| {
						invalid_record(operator_id, "potentialRanks[].buff", "BUFF has no payload")
					})?;
					for modifier in &buff.attributes.attribute_modifiers {
						raw_stats.apply_modifier(operator_id, modifier)?;
					}
				}
				"CUSTOM" => {
					let description: &str = rank
						.description
						.as_deref()
						.filter(|description: &&str| !description.trim().is_empty())
						.ok_or_else(|| {
							invalid_record(
								operator_id,
								"potentialRanks[].description",
								"CUSTOM description is missing",
							)
						})?;
					custom_potential_effects.push(PotentialCustomEffect {
						rank: PotentialRank::from_upgrade_index(index)?,
						description: description.to_owned(),
					});
				}
				_ => {
					return Err(unsupported(
						operator_id,
						"potentialRanks[].type",
						"unknown potential rank kind",
					));
				}
			}
		}
		let selected_module: Option<ResolvedOperatorModule> = self.compile_selected_module(
			operator_id,
			build.elite(),
			build.level(),
			build.module(),
			build.module_level(),
			&mut raw_stats,
		)?;

		let (base_stats, deployment_cost): (OperatorBaseStats, u32) =
			raw_stats.compile(operator_id)?;
		let canonical_build: OperatorBuild = OperatorBuild::new(
			OperatorIdentity::Id(operator_id.to_owned()),
			build.elite(),
			build.level(),
			build.trust_percent(),
			build.skill(),
			build.skill_level(),
			build.potential(),
			build.module().clone(),
			build.module_level(),
		);
		let stable_module_id: &str = match build.module() {
			OperatorModule::None => "none",
			OperatorModule::Id(module_id) => module_id,
		};
		let stable_id: String = format!(
			"{}:{operator_id}:e{}:l{}:t{}:p{}:s{}:sl{}:m-{stable_module_id}:ml{}",
			self.resolved_revision,
			build.elite(),
			build.level(),
			build.trust_percent(),
			build.potential().number(),
			build.skill(),
			build.skill_level(),
			build.module_level()
		);

		Ok(ResolvedOperatorBuild {
			resolved_revision: self.resolved_revision.clone(),
			stable_id,
			operator_id: operator_id.to_owned(),
			name: name.to_owned(),
			position,
			profession: profession.to_owned(),
			group_id: record.group_id.clone(),
			sub_profession_id: sub_profession_id.to_owned(),
			build: canonical_build,
			skill_id: skill_id.to_owned(),
			selected_skill,
			range_id: range_id.to_owned(),
			deployment_cost,
			base_stats,
			trust_battle_phase: capped_trust_battle_phase,
			custom_potential_effects,
			selected_module,
			trait_candidates,
			talent_groups,
		})
	}

	fn compile_selected_module(
		&self,
		operator_id: &str,
		elite: u8,
		level: u32,
		module: &OperatorModule,
		module_level: u8,
		raw_stats: &mut RawBaseStats,
	) -> Result<Option<ResolvedOperatorModule>, ArkDataError> {
		let OperatorModule::Id(module_id) = module else {
			return Ok(None);
		};
		let legal: Vec<LegalOperatorModule> = self.legal_modules(operator_id, elite, level)?;
		let descriptor: &LegalOperatorModule = legal
			.iter()
			.find(|candidate: &&LegalOperatorModule| candidate.module_id == *module_id)
			.ok_or_else(|| {
				invalid_build(
					operator_id,
					"module",
					"module is not a legal real ordinary module for this exact form",
				)
			})?;
		if module_level > descriptor.max_level {
			return Err(invalid_build(
				operator_id,
				"module_level",
				"module level does not exist",
			));
		}
		let battle_record: &RawBattleEquip = self
			.battle_equip_records
			.get(module_id)
			.expect("legal module must have a battle record");
		let mut matching_phases = battle_record
			.phases
			.iter()
			.filter(|phase: &&RawBattleEquipPhase| phase.level == module_level);
		let selected_phase: &RawBattleEquipPhase = matching_phases.next().ok_or_else(|| {
			invalid_build(operator_id, "module_level", "module level does not exist")
		})?;
		if matching_phases.next().is_some() {
			return Err(invalid_record(
				operator_id,
				"battle_equip_table.phases[].equipLevel",
				"module level is duplicated",
			));
		}
		for entry in &selected_phase.attribute_blackboard {
			raw_stats.apply_module_modifier(operator_id, entry)?;
		}
		compile_resolved_module(operator_id, module_id, selected_phase, &self.range_ids).map(Some)
	}

	fn resolve_id(&self, identity: &OperatorIdentity) -> Result<&str, ArkDataError> {
		match identity {
			OperatorIdentity::Id(operator_id) => self
				.records
				.get_key_value(operator_id)
				.map(|(canonical, _)| canonical.as_str())
				.ok_or_else(|| ArkDataError::OperatorNotFound {
					identity: identity.clone(),
				}),
			OperatorIdentity::Name(name) => match self.ids_by_name.get(name) {
				None => Err(ArkDataError::OperatorNotFound {
					identity: identity.clone(),
				}),
				Some(ids) if ids.len() == 1 => Ok(ids[0].as_str()),
				Some(ids) => Err(ArkDataError::AmbiguousOperatorName {
					name: name.clone(),
					candidate_ids: ids.clone(),
				}),
			},
		}
	}

	fn resolve_id_or_name(&self, value: &str) -> Result<&str, ArkDataError> {
		if let Some((canonical, _)) = self.records.get_key_value(value) {
			return Ok(canonical);
		}
		match self.ids_by_name.get(value) {
			None => Err(ArkDataError::OperatorNotFound {
				identity: OperatorIdentity::Name(value.to_owned()),
			}),
			Some(ids) if ids.len() == 1 => Ok(ids[0].as_str()),
			Some(ids) => Err(ArkDataError::AmbiguousOperatorName {
				name: value.to_owned(),
				candidate_ids: ids.clone(),
			}),
		}
	}
}

fn root_object<'a>(
	roots: &'a RawRoots,
	destination_path: &'static str,
) -> Result<&'a Map<String, Value>, ArkDataError> {
	roots
		.table(destination_path)
		.and_then(Value::as_object)
		.ok_or_else(|| ArkDataError::InvalidOperatorTable {
			destination_path,
			reason: "root must be a JSON object".to_owned(),
		})
}

fn invalid_record(operator_id: &str, field: &'static str, reason: &str) -> ArkDataError {
	ArkDataError::InvalidOperatorRecord {
		operator_id: operator_id.to_owned(),
		field,
		reason: reason.to_owned(),
	}
}

fn invalid_build(operator_id: &str, field: &'static str, reason: &str) -> ArkDataError {
	ArkDataError::InvalidOperatorBuild {
		operator_id: operator_id.to_owned(),
		field,
		reason: reason.to_owned(),
	}
}

fn unsupported(operator_id: &str, field: &'static str, reason: &str) -> ArkDataError {
	ArkDataError::UnsupportedOperatorProgression {
		operator_id: operator_id.to_owned(),
		field,
		reason: reason.to_owned(),
	}
}

fn parse_phase_index(
	operator_id: &str,
	field: &'static str,
	phase: &str,
) -> Result<u8, ArkDataError> {
	let index: &str = phase
		.strip_prefix("PHASE_")
		.ok_or_else(|| invalid_record(operator_id, field, "phase must use PHASE_<index>"))?;
	index
		.parse::<u8>()
		.map_err(|_| invalid_record(operator_id, field, "phase index must be a u8"))
}

fn validate_unlock_condition(
	operator_id: &str,
	field: &'static str,
	condition: &RawUnlockCondition,
	elite: u8,
	level: u32,
) -> Result<(), ArkDataError> {
	let required_elite: u8 = parse_phase_index(operator_id, field, &condition.phase)?;
	if elite < required_elite || (elite == required_elite && level < condition.level) {
		return Err(invalid_build(
			operator_id,
			"skill_level",
			"selected build has not unlocked this skill level",
		));
	}
	Ok(())
}

fn compile_skill_metadata(
	operator_id: &str,
	skill_id: &str,
	character_skill: &RawCharacterSkill,
	level: &RawSkillLevel,
	range_ids: &BTreeSet<String>,
) -> Result<SelectedSkillMetadata, ArkDataError> {
	if level.name.trim().is_empty() {
		return Err(invalid_record(
			operator_id,
			"skill_table.levels[].name",
			"name must not be blank",
		));
	}
	if !level.duration.is_finite() {
		return Err(invalid_record(
			operator_id,
			"skill_table.levels[].duration",
			"duration must be finite",
		));
	}
	if !level.sp_data.increment.is_finite() {
		return Err(invalid_record(
			operator_id,
			"skill_table.levels[].spData.increment",
			"increment must be finite",
		));
	}
	let range_id: Option<String> = match level.range_id.as_deref() {
		None => None,
		Some(value) if value.trim().is_empty() => {
			return Err(invalid_record(
				operator_id,
				"skill_table.levels[].rangeId",
				"range ID must not be blank",
			));
		}
		Some(value) if !range_ids.contains(value) => {
			return Err(invalid_record(
				operator_id,
				"skill_table.levels[].rangeId",
				"range is absent from the verified range table",
			));
		}
		Some(value) => Some(value.to_owned()),
	};
	let blackboard: Vec<BlackboardEntry> = level
		.blackboard
		.iter()
		.map(|entry: &RawBlackboardEntry| {
			if entry.key.trim().is_empty() {
				return Err(invalid_record(
					operator_id,
					"skill_table.levels[].blackboard[].key",
					"key must not be blank",
				));
			}
			if !entry.value.is_finite() {
				return Err(invalid_record(
					operator_id,
					"skill_table.levels[].blackboard[].value",
					"value must be finite",
				));
			}
			Ok(BlackboardEntry {
				key: entry.key.clone(),
				value: entry.value,
				value_string: entry.value_string.clone(),
			})
		})
		.collect::<Result<Vec<_>, ArkDataError>>()?;
	let activation_type: SkillActivationType = match level.skill_type.as_str() {
		"AUTO" => SkillActivationType::Auto,
		"MANUAL" => SkillActivationType::Manual,
		"PASSIVE" => SkillActivationType::Passive,
		unknown => SkillActivationType::Unknown(unknown.to_owned()),
	};
	let duration_type: SkillDurationType = match level.duration_type.as_str() {
		"NONE" => SkillDurationType::None,
		"AMMO" => SkillDurationType::Ammo,
		unknown => SkillDurationType::Unknown(unknown.to_owned()),
	};
	let sp_type: SkillSpType = match &level.sp_data.sp_type {
		RawStringOrCode::Name(name) => match name.as_str() {
			"INCREASE_WITH_TIME" => SkillSpType::IncreaseWithTime,
			"INCREASE_WHEN_ATTACK" => SkillSpType::IncreaseWhenAttack,
			"INCREASE_WHEN_TAKEN_DAMAGE" => SkillSpType::IncreaseWhenTakenDamage,
			unknown => SkillSpType::UnknownName(unknown.to_owned()),
		},
		RawStringOrCode::Code(code) => SkillSpType::UnknownCode(*code),
	};
	Ok(SelectedSkillMetadata {
		skill_id: skill_id.to_owned(),
		override_prefab_key: character_skill.override_prefab_key.clone(),
		override_token_key: character_skill.override_token_key.clone(),
		name: level.name.clone(),
		description: level.description.clone(),
		prefab_id: level.prefab_id.clone(),
		activation_type,
		duration_type,
		duration_seconds: level.duration,
		range_id,
		sp: SkillSpMetadata {
			sp_type,
			max_charges: level.sp_data.max_charges,
			cost: level.sp_data.cost,
			initial: level.sp_data.initial,
			increment: level.sp_data.increment,
		},
		blackboard,
	})
}

fn compile_blackboard(
	operator_id: &str,
	field: &'static str,
	entries: &[RawBlackboardEntry],
) -> Result<Vec<BlackboardEntry>, ArkDataError> {
	entries
		.iter()
		.map(|entry: &RawBlackboardEntry| {
			if entry.key.trim().is_empty() {
				return Err(invalid_record(operator_id, field, "key must not be blank"));
			}
			if !entry.value.is_finite() {
				return Err(invalid_record(operator_id, field, "value must be finite"));
			}
			Ok(BlackboardEntry {
				key: entry.key.clone(),
				value: entry.value,
				value_string: entry.value_string.clone(),
			})
		})
		.collect()
}

fn compile_trait_candidates(
	operator_id: &str,
	record: &RawCharacter,
	range_ids: &BTreeSet<String>,
) -> Result<Vec<OperatorEffectCandidate>, ArkDataError> {
	record
		.trait_data
		.as_ref()
		.and_then(|bundle: &RawTraitBundle| bundle.candidates.as_deref())
		.unwrap_or_default()
		.iter()
		.map(|candidate: &RawTraitCandidate| {
			Ok(OperatorEffectCandidate {
				unlock_phase: candidate.unlock_condition.phase.clone(),
				unlock_level: candidate.unlock_condition.level,
				required_potential_rank: candidate.required_potential_rank,
				prefab_key: candidate.prefab_key.clone(),
				name: None,
				description: candidate.override_description.clone(),
				range_id: validate_optional_range(
					operator_id,
					"trait.candidates[].rangeId",
					&candidate.range_id,
					range_ids,
				)?,
				blackboard: compile_blackboard(
					operator_id,
					"trait.candidates[].blackboard",
					&candidate.blackboard,
				)?,
				token_key: None,
				hidden: false,
			})
		})
		.collect()
}

fn compile_talent_groups(
	operator_id: &str,
	record: &RawCharacter,
	range_ids: &BTreeSet<String>,
) -> Result<Vec<OperatorTalentGroup>, ArkDataError> {
	record
		.talents
		.as_deref()
		.unwrap_or_default()
		.iter()
		.map(|group: &RawTalentGroup| {
			let candidates: Vec<OperatorEffectCandidate> = group
				.candidates
				.as_deref()
				.unwrap_or_default()
				.iter()
				.map(|candidate: &RawTalentCandidate| {
					Ok(OperatorEffectCandidate {
						unlock_phase: candidate.unlock_condition.phase.clone(),
						unlock_level: candidate.unlock_condition.level,
						required_potential_rank: candidate.required_potential_rank,
						prefab_key: candidate.prefab_key.clone(),
						name: candidate.name.clone(),
						description: candidate.description.clone(),
						range_id: validate_optional_range(
							operator_id,
							"talents[].candidates[].rangeId",
							&candidate.range_id,
							range_ids,
						)?,
						blackboard: compile_blackboard(
							operator_id,
							"talents[].candidates[].blackboard",
							&candidate.blackboard,
						)?,
						token_key: candidate.token_key.clone(),
						hidden: candidate.hidden,
					})
				})
				.collect::<Result<Vec<_>, ArkDataError>>()?;
			Ok(OperatorTalentGroup { candidates })
		})
		.collect()
}

fn validate_optional_range(
	operator_id: &str,
	field: &'static str,
	range_id: &Option<String>,
	range_ids: &BTreeSet<String>,
) -> Result<Option<String>, ArkDataError> {
	match range_id.as_deref() {
		None => Ok(None),
		Some(value) if value.trim().is_empty() => Err(invalid_record(
			operator_id,
			field,
			"range ID must not be blank",
		)),
		Some(value) if !range_ids.contains(value) => Err(invalid_record(
			operator_id,
			field,
			"range is absent from the verified range table",
		)),
		Some(value) => Ok(Some(value.to_owned())),
	}
}

fn compile_resolved_module(
	operator_id: &str,
	module_id: &str,
	phase: &RawBattleEquipPhase,
	range_ids: &BTreeSet<String>,
) -> Result<ResolvedOperatorModule, ArkDataError> {
	let attribute_blackboard: Vec<BlackboardEntry> = compile_blackboard(
		operator_id,
		"battle_equip_table.phases[].attributeBlackboard",
		&phase.attribute_blackboard,
	)?;
	let parts: Vec<OperatorModulePart> = phase
		.parts
		.iter()
		.map(|part: &RawModulePart| {
			let talent_candidates: Vec<OperatorModuleTalentCandidate> = part
				.talent_bundle
				.candidates
				.as_deref()
				.unwrap_or_default()
				.iter()
				.map(|candidate: &RawModuleTalentCandidate| {
					Ok(OperatorModuleTalentCandidate {
						display_range_id: candidate.display_range_id,
						upgrade_description: candidate.upgrade_description.clone(),
						talent_index: candidate.talent_index,
						valid_mode_indices: candidate.valid_mode_indices.clone(),
						unlock_phase: candidate.unlock_condition.phase.clone(),
						unlock_level: candidate.unlock_condition.level,
						required_potential_rank: candidate.required_potential_rank,
						prefab_key: candidate.prefab_key.clone(),
						name: candidate.name.clone(),
						description: candidate.description.clone(),
						range_id: validate_optional_range(
							operator_id,
							"battle_equip_table.phases[].parts[].talent.rangeId",
							&candidate.range_id,
							range_ids,
						)?,
						blackboard: compile_blackboard(
							operator_id,
							"battle_equip_table.phases[].parts[].talent.blackboard",
							&candidate.blackboard,
						)?,
						token_key: candidate.token_key.clone(),
						hidden: candidate.hidden,
					})
				})
				.collect::<Result<Vec<_>, ArkDataError>>()?;
			let trait_candidates: Vec<OperatorModuleTraitCandidate> = part
				.trait_bundle
				.candidates
				.as_deref()
				.unwrap_or_default()
				.iter()
				.map(|candidate: &RawModuleTraitCandidate| {
					Ok(OperatorModuleTraitCandidate {
						additional_description: candidate.additional_description.clone(),
						override_description: candidate.override_description.clone(),
						unlock_phase: candidate.unlock_condition.phase.clone(),
						unlock_level: candidate.unlock_condition.level,
						required_potential_rank: candidate.required_potential_rank,
						prefab_key: candidate.prefab_key.clone(),
						range_id: validate_optional_range(
							operator_id,
							"battle_equip_table.phases[].parts[].trait.rangeId",
							&candidate.range_id,
							range_ids,
						)?,
						blackboard: compile_blackboard(
							operator_id,
							"battle_equip_table.phases[].parts[].trait.blackboard",
							&candidate.blackboard,
						)?,
					})
				})
				.collect::<Result<Vec<_>, ArkDataError>>()?;
			let target: OperatorModulePartTarget = match part.target.as_str() {
				"DISPLAY" => OperatorModulePartTarget::Display,
				"OVERWRITE_BATTLE_DATA" => OperatorModulePartTarget::OverwriteBattleData,
				"TALENT" => OperatorModulePartTarget::Talent,
				"TALENT_DATA_ONLY" => OperatorModulePartTarget::TalentDataOnly,
				"TRAIT" => OperatorModulePartTarget::Trait,
				"TRAIT_DATA_ONLY" => OperatorModulePartTarget::TraitDataOnly,
				unknown => OperatorModulePartTarget::Unknown(unknown.to_owned()),
			};
			Ok(OperatorModulePart {
				resource_key: part.resource_key.clone(),
				target,
				is_token: part.is_token,
				valid_in_game_tag: part.valid_in_game_tag.clone(),
				valid_in_map_tag: part.valid_in_map_tag.clone(),
				talent_candidates,
				trait_candidates,
			})
		})
		.collect::<Result<Vec<_>, ArkDataError>>()?;
	let token_attribute_blackboard: BTreeMap<String, Vec<BlackboardEntry>> = phase
		.token_attribute_blackboard
		.iter()
		.map(|(token_id, entries): (&String, &Vec<RawBlackboardEntry>)| {
			compile_blackboard(
				operator_id,
				"battle_equip_table.phases[].tokenAttributeBlackboard",
				entries,
			)
			.map(|compiled: Vec<BlackboardEntry>| (token_id.clone(), compiled))
		})
		.collect::<Result<BTreeMap<_, _>, ArkDataError>>()?;
	Ok(ResolvedOperatorModule {
		module_id: module_id.to_owned(),
		level: phase.level,
		attribute_blackboard,
		parts,
		token_attribute_blackboard,
	})
}

#[derive(Debug, Deserialize)]
struct RawCharacter {
	name: Option<String>,
	position: Option<String>,
	profession: Option<String>,
	#[serde(rename = "groupId")]
	group_id: Option<String>,
	#[serde(rename = "subProfessionId")]
	sub_profession_id: Option<String>,
	phases: Option<Vec<RawPhase>>,
	skills: Option<Vec<RawCharacterSkill>>,
	#[serde(rename = "allSkillLvlup")]
	all_skill_level_up_conditions: Option<Vec<RawSkillLevelUpCondition>>,
	#[serde(rename = "potentialRanks")]
	potential_ranks: Option<Vec<RawPotentialRank>>,
	#[serde(rename = "favorKeyFrames")]
	favor_key_frames: Option<Vec<RawAttributeKeyFrame>>,
	#[serde(rename = "trait")]
	trait_data: Option<RawTraitBundle>,
	talents: Option<Vec<RawTalentGroup>>,
}

#[derive(Debug, Deserialize)]
struct RawPhase {
	#[serde(rename = "rangeId")]
	range_id: Option<String>,
	#[serde(rename = "maxLevel")]
	max_level: Option<u32>,
	#[serde(rename = "attributesKeyFrames")]
	attribute_key_frames: Option<Vec<RawAttributeKeyFrame>>,
}

#[derive(Debug, Deserialize)]
struct RawAttributeKeyFrame {
	level: Option<u32>,
	data: RawBaseStats,
}

#[derive(Debug, Deserialize)]
struct RawTraitBundle {
	candidates: Option<Vec<RawTraitCandidate>>,
}

#[derive(Debug, Deserialize)]
struct RawTraitCandidate {
	#[serde(rename = "unlockCondition")]
	unlock_condition: RawUnlockCondition,
	#[serde(rename = "requiredPotentialRank")]
	required_potential_rank: u8,
	#[serde(rename = "prefabKey")]
	prefab_key: Option<String>,
	#[serde(rename = "rangeId")]
	range_id: Option<String>,
	blackboard: Vec<RawBlackboardEntry>,
	#[serde(rename = "overrideDescripton")]
	override_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTalentGroup {
	candidates: Option<Vec<RawTalentCandidate>>,
}

#[derive(Debug, Deserialize)]
struct RawTalentCandidate {
	#[serde(rename = "unlockCondition")]
	unlock_condition: RawUnlockCondition,
	#[serde(rename = "requiredPotentialRank")]
	required_potential_rank: u8,
	#[serde(rename = "prefabKey")]
	prefab_key: Option<String>,
	name: Option<String>,
	description: Option<String>,
	#[serde(rename = "rangeId")]
	range_id: Option<String>,
	blackboard: Vec<RawBlackboardEntry>,
	#[serde(rename = "tokenKey")]
	token_key: Option<String>,
	#[serde(rename = "isHideTalent")]
	hidden: bool,
}

#[derive(Debug, Deserialize)]
struct RawCharacterSkill {
	#[serde(rename = "skillId")]
	skill_id: Option<String>,
	#[serde(rename = "overridePrefabKey")]
	override_prefab_key: Option<String>,
	#[serde(rename = "overrideTokenKey")]
	override_token_key: Option<String>,
	#[serde(rename = "unlockCond")]
	unlock_condition: Option<RawUnlockCondition>,
	#[serde(rename = "levelUpCostCond")]
	level_up_cost_conditions: Option<Vec<RawSkillLevelUpCondition>>,
}

#[derive(Debug, Deserialize)]
struct RawSkillLevelUpCondition {
	#[serde(rename = "unlockCond")]
	unlock_condition: Option<RawUnlockCondition>,
}

#[derive(Debug, Deserialize)]
struct RawUnlockCondition {
	phase: String,
	level: u32,
}

#[derive(Debug, Deserialize)]
struct RawSkill {
	levels: Option<Vec<RawSkillLevel>>,
}

#[derive(Debug, Deserialize)]
struct RawSkillLevel {
	name: String,
	description: Option<String>,
	#[serde(rename = "skillType")]
	skill_type: String,
	#[serde(rename = "durationType")]
	duration_type: String,
	duration: f64,
	#[serde(rename = "prefabId")]
	prefab_id: Option<String>,
	#[serde(rename = "rangeId")]
	range_id: Option<String>,
	#[serde(rename = "spData")]
	sp_data: RawSkillSpData,
	blackboard: Vec<RawBlackboardEntry>,
}

#[derive(Debug, Deserialize)]
struct RawSkillSpData {
	#[serde(rename = "spType")]
	sp_type: RawStringOrCode,
	#[serde(rename = "maxChargeTime")]
	max_charges: u32,
	#[serde(rename = "spCost")]
	cost: u32,
	#[serde(rename = "initSp")]
	initial: u32,
	increment: f64,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawStringOrCode {
	Name(String),
	Code(i64),
}

#[derive(Clone, Debug, Deserialize)]
struct RawBlackboardEntry {
	key: String,
	value: f64,
	#[serde(rename = "valueStr")]
	value_string: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPotentialRank {
	#[serde(rename = "type")]
	kind: String,
	description: Option<String>,
	buff: Option<RawPotentialBuff>,
}

#[derive(Debug, Deserialize)]
struct RawPotentialBuff {
	attributes: RawPotentialAttributes,
}

#[derive(Debug, Deserialize)]
struct RawPotentialAttributes {
	#[serde(rename = "attributeModifiers")]
	attribute_modifiers: Vec<RawAttributeModifier>,
}

#[derive(Debug, Deserialize)]
struct RawAttributeModifier {
	#[serde(rename = "attributeType")]
	attribute_type: String,
	#[serde(rename = "formulaItem")]
	formula_item: String,
	value: f64,
	#[serde(rename = "loadFromBlackboard")]
	load_from_blackboard: bool,
	#[serde(rename = "fetchBaseValueFromSourceEntity")]
	fetch_base_value_from_source_entity: bool,
}

#[derive(Debug, Deserialize)]
struct RawFavorTable {
	#[serde(rename = "favorFrames")]
	#[serde(default)]
	favor_frames: Vec<RawFavorFrame>,
}

#[derive(Debug, Deserialize)]
struct RawFavorFrame {
	data: RawFavorFrameData,
}

#[derive(Debug, Deserialize)]
struct RawFavorFrameData {
	percent: u8,
	#[serde(rename = "battlePhase")]
	battle_phase: u32,
}

#[derive(Debug, Deserialize)]
struct RawUniEquipTable {
	#[serde(rename = "charEquip")]
	#[serde(default)]
	char_equip: BTreeMap<String, Vec<String>>,
	#[serde(rename = "equipDict")]
	#[serde(default)]
	equip_dict: BTreeMap<String, RawUniEquip>,
}

#[derive(Debug, Deserialize)]
struct RawUniEquip {
	#[serde(rename = "uniEquipId")]
	module_id: String,
	#[serde(rename = "charId")]
	character_id: String,
	#[serde(rename = "tmplId")]
	template_id: Option<String>,
	#[serde(rename = "type")]
	kind: String,
	#[serde(rename = "isSpecialEquip")]
	is_special: bool,
	#[serde(rename = "unlockEvolvePhase")]
	unlock_phase: String,
	#[serde(rename = "unlockLevel")]
	unlock_level: u32,
}

#[derive(Debug, Deserialize)]
struct RawBattleEquip {
	phases: Vec<RawBattleEquipPhase>,
}

#[derive(Debug, Deserialize)]
struct RawBattleEquipPhase {
	#[serde(rename = "equipLevel")]
	level: u8,
	#[serde(rename = "attributeBlackboard")]
	attribute_blackboard: Vec<RawBlackboardEntry>,
	parts: Vec<RawModulePart>,
	#[serde(rename = "tokenAttributeBlackboard")]
	token_attribute_blackboard: BTreeMap<String, Vec<RawBlackboardEntry>>,
}

#[derive(Debug, Deserialize)]
struct RawModulePart {
	#[serde(rename = "resKey")]
	resource_key: Option<String>,
	target: String,
	#[serde(rename = "isToken")]
	is_token: bool,
	#[serde(rename = "validInGameTag")]
	valid_in_game_tag: Option<String>,
	#[serde(rename = "validInMapTag")]
	valid_in_map_tag: Option<String>,
	#[serde(rename = "addOrOverrideTalentDataBundle")]
	talent_bundle: RawModuleTalentBundle,
	#[serde(rename = "overrideTraitDataBundle")]
	trait_bundle: RawModuleTraitBundle,
}

#[derive(Debug, Deserialize)]
struct RawModuleTalentBundle {
	candidates: Option<Vec<RawModuleTalentCandidate>>,
}

#[derive(Debug, Deserialize)]
struct RawModuleTraitBundle {
	candidates: Option<Vec<RawModuleTraitCandidate>>,
}

#[derive(Debug, Deserialize)]
struct RawModuleTalentCandidate {
	#[serde(rename = "displayRangeId")]
	display_range_id: bool,
	#[serde(rename = "upgradeDescription")]
	upgrade_description: Option<String>,
	#[serde(rename = "talentIndex")]
	talent_index: i32,
	#[serde(rename = "validModeIndices")]
	valid_mode_indices: Option<Vec<i32>>,
	#[serde(rename = "unlockCondition")]
	unlock_condition: RawUnlockCondition,
	#[serde(rename = "requiredPotentialRank")]
	required_potential_rank: u8,
	#[serde(rename = "prefabKey")]
	prefab_key: Option<String>,
	name: Option<String>,
	description: Option<String>,
	#[serde(rename = "rangeId")]
	range_id: Option<String>,
	blackboard: Vec<RawBlackboardEntry>,
	#[serde(rename = "tokenKey")]
	token_key: Option<String>,
	#[serde(rename = "isHideTalent")]
	hidden: bool,
}

#[derive(Debug, Deserialize)]
struct RawModuleTraitCandidate {
	#[serde(rename = "additionalDescription")]
	additional_description: Option<String>,
	#[serde(rename = "overrideDescripton")]
	override_description: Option<String>,
	#[serde(rename = "unlockCondition")]
	unlock_condition: RawUnlockCondition,
	#[serde(rename = "requiredPotentialRank")]
	required_potential_rank: u8,
	#[serde(rename = "prefabKey")]
	prefab_key: Option<String>,
	#[serde(rename = "rangeId")]
	range_id: Option<String>,
	blackboard: Vec<RawBlackboardEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawBaseStats {
	#[serde(rename = "maxHp")]
	max_hp: f64,
	#[serde(rename = "atk")]
	attack: f64,
	#[serde(rename = "def")]
	defense: f64,
	#[serde(rename = "magicResistance")]
	magic_resistance: f64,
	cost: f64,
	#[serde(rename = "blockCnt")]
	block_count: f64,
	#[serde(rename = "attackSpeed")]
	attack_speed: f64,
	#[serde(rename = "baseAttackTime")]
	base_attack_time_seconds: f64,
	#[serde(rename = "respawnTime")]
	respawn_time_seconds: f64,
}

impl RawBaseStats {
	fn interpolate(&self, upper: &Self, ratio: f64) -> Self {
		Self {
			max_hp: lerp(self.max_hp, upper.max_hp, ratio),
			attack: lerp(self.attack, upper.attack, ratio),
			defense: lerp(self.defense, upper.defense, ratio),
			magic_resistance: lerp(self.magic_resistance, upper.magic_resistance, ratio),
			cost: lerp(self.cost, upper.cost, ratio),
			block_count: lerp(self.block_count, upper.block_count, ratio),
			attack_speed: lerp(self.attack_speed, upper.attack_speed, ratio),
			base_attack_time_seconds: lerp(
				self.base_attack_time_seconds,
				upper.base_attack_time_seconds,
				ratio,
			),
			respawn_time_seconds: lerp(
				self.respawn_time_seconds,
				upper.respawn_time_seconds,
				ratio,
			),
		}
	}

	fn add_assign(&mut self, delta: &Self) {
		self.max_hp += delta.max_hp;
		self.attack += delta.attack;
		self.defense += delta.defense;
		self.magic_resistance += delta.magic_resistance;
		self.cost += delta.cost;
		self.block_count += delta.block_count;
		self.attack_speed += delta.attack_speed;
		self.base_attack_time_seconds += delta.base_attack_time_seconds;
		self.respawn_time_seconds += delta.respawn_time_seconds;
	}

	fn apply_modifier(
		&mut self,
		operator_id: &str,
		modifier: &RawAttributeModifier,
	) -> Result<(), ArkDataError> {
		if modifier.formula_item != "ADDITION"
			|| modifier.load_from_blackboard
			|| modifier.fetch_base_value_from_source_entity
		{
			return Err(unsupported(
				operator_id,
				"potentialRanks[].buff.attributes.attributeModifiers[].formulaItem",
				"only direct ADDITION modifiers are static",
			));
		}
		if !modifier.value.is_finite() {
			return Err(invalid_record(
				operator_id,
				"potentialRanks[].buff.attributes.attributeModifiers[].value",
				"value must be finite",
			));
		}
		match modifier.attribute_type.as_str() {
			"MAX_HP" => self.max_hp += modifier.value,
			"ATK" => self.attack += modifier.value,
			"DEF" => self.defense += modifier.value,
			"MAGIC_RESISTANCE" => self.magic_resistance += modifier.value,
			"COST" => self.cost += modifier.value,
			"ATTACK_SPEED" => self.attack_speed += modifier.value,
			"RESPAWN_TIME" => self.respawn_time_seconds += modifier.value,
			_ => {
				return Err(unsupported(
					operator_id,
					"potentialRanks[].buff.attributes.attributeModifiers[].attributeType",
					"unknown static attribute modifier",
				));
			}
		}
		Ok(())
	}

	fn apply_module_modifier(
		&mut self,
		operator_id: &str,
		entry: &RawBlackboardEntry,
	) -> Result<(), ArkDataError> {
		if entry.value_string.is_some() || !entry.value.is_finite() {
			return Err(unsupported(
				operator_id,
				"battle_equip_table.phases[].attributeBlackboard[]",
				"static module attributes require a finite numeric value",
			));
		}
		match entry.key.as_str() {
			"max_hp" => self.max_hp += entry.value,
			"atk" => self.attack += entry.value,
			"def" => self.defense += entry.value,
			"magic_resistance" => self.magic_resistance += entry.value,
			"cost" => self.cost += entry.value,
			"block_cnt" => self.block_count += entry.value,
			"attack_speed" => self.attack_speed += entry.value,
			"respawn_time" => self.respawn_time_seconds += entry.value,
			_ => {
				return Err(unsupported(
					operator_id,
					"battle_equip_table.phases[].attributeBlackboard[].key",
					"unknown static module attribute",
				));
			}
		}
		Ok(())
	}

	fn compile(self, operator_id: &str) -> Result<(OperatorBaseStats, u32), ArkDataError> {
		let numeric: [(&'static str, f64); 6] = [
			("maxHp", self.max_hp),
			("atk", self.attack),
			("def", self.defense),
			("magicResistance", self.magic_resistance),
			("attackSpeed", self.attack_speed),
			("baseAttackTime", self.base_attack_time_seconds),
		];
		for (field, value) in numeric {
			if !value.is_finite() {
				return Err(invalid_record(operator_id, field, "value must be finite"));
			}
		}
		if self.max_hp < 0.0 || self.attack < 0.0 || self.defense < 0.0 {
			return Err(invalid_record(
				operator_id,
				"attributes",
				"HP, attack, and defense must be non-negative",
			));
		}
		if self.attack_speed <= 0.0 || self.base_attack_time_seconds <= 0.0 {
			return Err(invalid_record(
				operator_id,
				"attributes",
				"attack speed and base attack time must be positive",
			));
		}

		let deployment_cost: u32 = exact_u32(operator_id, "cost", self.cost)?;
		let block_count: u32 = exact_u32(operator_id, "blockCnt", self.block_count)?;
		let respawn_time_seconds: u32 =
			exact_u32(operator_id, "respawnTime", self.respawn_time_seconds)?;
		Ok((
			OperatorBaseStats {
				max_hp: self.max_hp,
				attack: self.attack,
				defense: self.defense,
				magic_resistance: self.magic_resistance,
				block_count,
				attack_speed: self.attack_speed,
				base_attack_time_seconds: self.base_attack_time_seconds,
				respawn_time_seconds,
			},
			deployment_cost,
		))
	}
}

fn interpolate_keyframes(
	operator_id: &str,
	frames: &[RawAttributeKeyFrame],
	level: u32,
	field: &'static str,
) -> Result<RawBaseStats, ArkDataError> {
	let mut ordered: Vec<(&RawAttributeKeyFrame, u32)> = frames
		.iter()
		.map(|frame: &RawAttributeKeyFrame| {
			frame
				.level
				.map(|level: u32| (frame, level))
				.ok_or_else(|| invalid_record(operator_id, field, "keyframe level is missing"))
		})
		.collect::<Result<Vec<_>, ArkDataError>>()?;
	ordered.sort_by_key(|(_, frame_level): &(&RawAttributeKeyFrame, u32)| *frame_level);
	for pair in ordered.windows(2) {
		if pair[0].1 == pair[1].1 {
			return Err(invalid_record(
				operator_id,
				field,
				"keyframe levels must be unique",
			));
		}
	}
	let (lower, lower_level): (&RawAttributeKeyFrame, u32) = ordered
		.iter()
		.rev()
		.find(|(_, frame_level): &&(&RawAttributeKeyFrame, u32)| *frame_level <= level)
		.copied()
		.ok_or_else(|| invalid_record(operator_id, field, "keyframes do not cover the level"))?;
	let lower_stats: RawBaseStats = lower.data.clone();
	if lower_level == level {
		return Ok(lower_stats);
	}
	let (upper, upper_level): (&RawAttributeKeyFrame, u32) = ordered
		.iter()
		.find(|(_, frame_level): &&(&RawAttributeKeyFrame, u32)| *frame_level >= level)
		.copied()
		.ok_or_else(|| invalid_record(operator_id, field, "keyframes do not cover the level"))?;
	let upper_stats: RawBaseStats = upper.data.clone();
	let numerator: f64 = f64::from(level - lower_level);
	let denominator: f64 = f64::from(upper_level - lower_level);
	Ok(lower_stats.interpolate(&upper_stats, numerator / denominator))
}

fn lerp(lower: f64, upper: f64, ratio: f64) -> f64 {
	lower + (upper - lower) * ratio
}

fn exact_u32(operator_id: &str, field: &'static str, value: f64) -> Result<u32, ArkDataError> {
	if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
		return Err(invalid_record(
			operator_id,
			field,
			"value must be an exact non-negative u32",
		));
	}
	Ok(value as u32)
}
