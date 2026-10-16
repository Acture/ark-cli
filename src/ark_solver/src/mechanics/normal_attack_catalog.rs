use std::error::Error;
use std::fmt::{Display, Formatter};
use std::num::NonZeroU64;

use crate::data::ResolvedBasePanelBuild;
use crate::simulation::TickRate;

use crate::mechanics::normal_attack::{
	FixedDamage, FixedDamageError, LockedTargetDamagePacket, NormalAttackDamageType,
	NormalAttackProfile, NormalAttackProfileError, NormalAttackTargetFilter,
};

const DECIMAL_SCALE: u64 = 1_000_000;
const PERCENT_SCALE: u128 = 100;
const U64_EXCLUSIVE_UPPER_BOUND_AS_F64: f64 = 18_446_744_073_709_551_616.0;

/// Reviewed v0 archetypes whose ordinary attacks do not require skill, talent, or special-case
/// semantics.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReviewedNormalAttackArchetype {
	CoreCaster,
	FastshotSniper,
	GuardianDefender,
	InstructorGuard,
	Pioneer,
	ProtectorDefender,
}

/// Explicit v0 first-attack timing rule.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackFirstDelayRule {
	/// The first attack lands one complete resolved attack interval after deployment.
	OneAttackInterval,
}

/// Supported catalog result with its reviewed semantics and compiled profile.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CatalogNormalAttackProfile {
	archetype: ReviewedNormalAttackArchetype,
	first_delay_rule: NormalAttackFirstDelayRule,
	profile: NormalAttackProfile,
}

impl CatalogNormalAttackProfile {
	#[must_use]
	pub const fn archetype(&self) -> ReviewedNormalAttackArchetype {
		self.archetype
	}

	#[must_use]
	pub const fn first_delay_rule(&self) -> NormalAttackFirstDelayRule {
		self.first_delay_rule
	}

	#[must_use]
	pub const fn profile(&self) -> &NormalAttackProfile {
		&self.profile
	}

	#[must_use]
	pub fn into_profile(self) -> NormalAttackProfile {
		self.profile
	}
}

/// Structured fail-closed result for a candidate outside the reviewed v0 catalog.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnsupportedCatalogNormalAttack {
	operator_id: String,
	reason: NormalAttackCatalogUnsupportedReason,
}

impl UnsupportedCatalogNormalAttack {
	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub const fn reason(&self) -> &NormalAttackCatalogUnsupportedReason {
		&self.reason
	}
}

/// A catalog compile always yields either reviewed semantics or an explicit exclusion.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackCatalogEntry {
	Supported(CatalogNormalAttackProfile),
	Unsupported(UnsupportedCatalogNormalAttack),
}

impl NormalAttackCatalogEntry {
	#[must_use]
	pub const fn as_supported(&self) -> Option<&CatalogNormalAttackProfile> {
		match self {
			Self::Supported(profile) => Some(profile),
			Self::Unsupported(_) => None,
		}
	}

	#[must_use]
	pub const fn as_unsupported(&self) -> Option<&UnsupportedCatalogNormalAttack> {
		match self {
			Self::Supported(_) => None,
			Self::Unsupported(unsupported) => Some(unsupported),
		}
	}
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackCatalogUnsupportedReason {
	UnreviewedArchetype {
		profession: String,
		sub_profession_id: String,
	},
	InvalidBaseDamage(FixedDamageError),
	InvalidAttackInterval(AttackIntervalError),
	ProfileInvariant(NormalAttackProfileError),
}

impl Display for NormalAttackCatalogUnsupportedReason {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::UnreviewedArchetype {
				profession,
				sub_profession_id,
			} => write!(
				formatter,
				"unreviewed normal-attack archetype {profession}/{sub_profession_id}"
			),
			Self::InvalidBaseDamage(source) => {
				write!(formatter, "invalid base-panel attack value: {source}")
			}
			Self::InvalidAttackInterval(source) => {
				write!(formatter, "invalid base-panel attack interval: {source}")
			}
			Self::ProfileInvariant(source) => {
				write!(
					formatter,
					"normal-attack profile invariant failed: {source}"
				)
			}
		}
	}
}

impl Error for NormalAttackCatalogUnsupportedReason {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::InvalidBaseDamage(source) => Some(source),
			Self::InvalidAttackInterval(source) => Some(source),
			Self::ProfileInvariant(source) => Some(source),
			Self::UnreviewedArchetype { .. } => None,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttackTimingField {
	BaseAttackTime,
	AttackSpeed,
}

impl Display for AttackTimingField {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::BaseAttackTime => formatter.write_str("base attack time"),
			Self::AttackSpeed => formatter.write_str("attack speed"),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttackIntervalError {
	NonFinite { field: AttackTimingField },
	NonPositive { field: AttackTimingField },
	BelowSixDecimalResolution { field: AttackTimingField },
	InputOverflow { field: AttackTimingField },
	TickOverflow,
}

impl Display for AttackIntervalError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NonFinite { field } => write!(formatter, "{field} must be finite"),
			Self::NonPositive { field } => write!(formatter, "{field} must be positive"),
			Self::BelowSixDecimalResolution { field } => {
				write!(formatter, "{field} rounds to zero at six-decimal precision")
			}
			Self::InputOverflow { field } => {
				write!(formatter, "{field} exceeds the six-decimal representation")
			}
			Self::TickOverflow => formatter.write_str("attack interval exceeds u64 ticks"),
		}
	}
}

impl Error for AttackIntervalError {}

/// Pure catalog inputs. `From<&ResolvedBasePanelBuild>` is the production adapter; the explicit
/// facts form keeps archetype review and timing rules independently testable.
#[derive(Clone, Debug, PartialEq)]
pub struct BasePanelNormalAttackFacts {
	operator_id: String,
	profession: String,
	sub_profession_id: String,
	attack: f64,
	base_attack_time_seconds: f64,
	attack_speed: f64,
}

impl BasePanelNormalAttackFacts {
	#[must_use]
	pub fn new(
		operator_id: String,
		profession: String,
		sub_profession_id: String,
		attack: f64,
		base_attack_time_seconds: f64,
		attack_speed: f64,
	) -> Self {
		Self {
			operator_id,
			profession,
			sub_profession_id,
			attack,
			base_attack_time_seconds,
			attack_speed,
		}
	}

	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub fn profession(&self) -> &str {
		&self.profession
	}

	#[must_use]
	pub fn sub_profession_id(&self) -> &str {
		&self.sub_profession_id
	}

	#[must_use]
	pub const fn attack(&self) -> f64 {
		self.attack
	}

	#[must_use]
	pub const fn base_attack_time_seconds(&self) -> f64 {
		self.base_attack_time_seconds
	}

	#[must_use]
	pub const fn attack_speed(&self) -> f64 {
		self.attack_speed
	}
}

impl From<&ResolvedBasePanelBuild> for BasePanelNormalAttackFacts {
	fn from(build: &ResolvedBasePanelBuild) -> Self {
		let stats = build.base_stats();
		Self {
			operator_id: build.operator_id().to_owned(),
			profession: build.profession().to_owned(),
			sub_profession_id: build.sub_profession_id().to_owned(),
			attack: stats.attack(),
			base_attack_time_seconds: stats.base_attack_time_seconds(),
			attack_speed: stats.attack_speed(),
		}
	}
}

/// Compiles one revision-pinned base-panel build through the reviewed v0 archetype catalog.
#[must_use]
pub fn compile_base_panel_normal_attack(
	build: &ResolvedBasePanelBuild,
	tick_rate: TickRate,
) -> NormalAttackCatalogEntry {
	let facts: BasePanelNormalAttackFacts = build.into();
	compile_base_panel_normal_attack_facts(&facts, tick_rate)
}

/// Compiles explicit base-panel facts with the same fail-closed production rules.
#[must_use]
pub fn compile_base_panel_normal_attack_facts(
	facts: &BasePanelNormalAttackFacts,
	tick_rate: TickRate,
) -> NormalAttackCatalogEntry {
	let (archetype, damage_type, target_filter): (
		ReviewedNormalAttackArchetype,
		NormalAttackDamageType,
		NormalAttackTargetFilter,
	) = match (facts.profession(), facts.sub_profession_id()) {
		("CASTER", "corecaster") => (
			ReviewedNormalAttackArchetype::CoreCaster,
			NormalAttackDamageType::Arts,
			NormalAttackTargetFilter::GroundAndAir,
		),
		("SNIPER", "fastshot") => (
			ReviewedNormalAttackArchetype::FastshotSniper,
			NormalAttackDamageType::Physical,
			NormalAttackTargetFilter::GroundAndAir,
		),
		("TANK", "guardian") => (
			ReviewedNormalAttackArchetype::GuardianDefender,
			NormalAttackDamageType::Physical,
			NormalAttackTargetFilter::GroundOnly,
		),
		// `protector` and `instructor` are the official sub-profession ids. Both archetypes attack
		// one blocked ground target for ordinary physical damage, with no archetype-level rule of
		// their own. The reviewed charpacks corroborate it: every `AttackAbility` component on
		// char_150_snakek and char_130_doberm declares `_damageType` 1, `_attackType` 1,
		// `_atkScale` 1.0, no target selector and no active buffs.
		("TANK", "protector") => (
			ReviewedNormalAttackArchetype::ProtectorDefender,
			NormalAttackDamageType::Physical,
			NormalAttackTargetFilter::GroundOnly,
		),
		("WARRIOR", "instructor") => (
			ReviewedNormalAttackArchetype::InstructorGuard,
			NormalAttackDamageType::Physical,
			NormalAttackTargetFilter::GroundOnly,
		),
		("PIONEER", "pioneer") => (
			ReviewedNormalAttackArchetype::Pioneer,
			NormalAttackDamageType::Physical,
			NormalAttackTargetFilter::GroundOnly,
		),
		(profession, sub_profession_id) => {
			return unsupported(
				facts,
				NormalAttackCatalogUnsupportedReason::UnreviewedArchetype {
					profession: profession.to_owned(),
					sub_profession_id: sub_profession_id.to_owned(),
				},
			);
		}
	};
	let base_damage: FixedDamage = match FixedDamage::try_from_f64(facts.attack()) {
		Ok(value) => value,
		Err(source) => {
			return unsupported(
				facts,
				NormalAttackCatalogUnsupportedReason::InvalidBaseDamage(source),
			);
		}
	};
	let attack_interval_ticks: NonZeroU64 = match deterministic_attack_interval_ticks(
		facts.base_attack_time_seconds(),
		facts.attack_speed(),
		tick_rate,
	) {
		Ok(value) => value,
		Err(source) => {
			return unsupported(
				facts,
				NormalAttackCatalogUnsupportedReason::InvalidAttackInterval(source),
			);
		}
	};
	let profile: NormalAttackProfile = match NormalAttackProfile::new(
		1,
		vec![LockedTargetDamagePacket::new(damage_type, base_damage)],
		attack_interval_ticks.get(),
		attack_interval_ticks.get(),
		target_filter,
	) {
		Ok(profile) => profile,
		Err(source) => {
			return unsupported(
				facts,
				NormalAttackCatalogUnsupportedReason::ProfileInvariant(source),
			);
		}
	};
	NormalAttackCatalogEntry::Supported(CatalogNormalAttackProfile {
		archetype,
		first_delay_rule: NormalAttackFirstDelayRule::OneAttackInterval,
		profile,
	})
}

/// Resolves `base_attack_time * 100 / attack_speed` to ticks and rounds up exactly after
/// normalizing both source values to six decimal places.
pub fn deterministic_attack_interval_ticks(
	base_attack_time_seconds: f64,
	attack_speed: f64,
	tick_rate: TickRate,
) -> Result<NonZeroU64, AttackIntervalError> {
	let base_time_scaled: u64 =
		positive_six_decimal(base_attack_time_seconds, AttackTimingField::BaseAttackTime)?;
	let attack_speed_scaled: u64 =
		positive_six_decimal(attack_speed, AttackTimingField::AttackSpeed)?;
	let numerator: u128 = u128::from(base_time_scaled)
		* PERCENT_SCALE
		* u128::from(tick_rate.ticks_per_second().get());
	let denominator: u128 = u128::from(attack_speed_scaled);
	let quotient: u128 = numerator / denominator;
	let rounded_up: u128 = quotient
		.checked_add(u128::from(!numerator.is_multiple_of(denominator)))
		.ok_or(AttackIntervalError::TickOverflow)?;
	let ticks: u64 = u64::try_from(rounded_up).map_err(|_| AttackIntervalError::TickOverflow)?;
	NonZeroU64::new(ticks).ok_or(AttackIntervalError::BelowSixDecimalResolution {
		field: AttackTimingField::BaseAttackTime,
	})
}

fn positive_six_decimal(value: f64, field: AttackTimingField) -> Result<u64, AttackIntervalError> {
	if !value.is_finite() {
		return Err(AttackIntervalError::NonFinite { field });
	}
	if value <= 0.0 {
		return Err(AttackIntervalError::NonPositive { field });
	}
	let scaled: f64 = (value * DECIMAL_SCALE as f64).round();
	if !scaled.is_finite() || scaled >= U64_EXCLUSIVE_UPPER_BOUND_AS_F64 {
		return Err(AttackIntervalError::InputOverflow { field });
	}
	let scaled: u64 = scaled as u64;
	if scaled == 0 {
		return Err(AttackIntervalError::BelowSixDecimalResolution { field });
	}
	Ok(scaled)
}

fn unsupported(
	facts: &BasePanelNormalAttackFacts,
	reason: NormalAttackCatalogUnsupportedReason,
) -> NormalAttackCatalogEntry {
	NormalAttackCatalogEntry::Unsupported(UnsupportedCatalogNormalAttack {
		operator_id: facts.operator_id().to_owned(),
		reason,
	})
}
