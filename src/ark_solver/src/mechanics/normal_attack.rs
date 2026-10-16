use std::{
	error::Error,
	fmt::{Display, Formatter},
	num::{NonZeroU32, NonZeroU64},
};

/// Number of fixed-point subunits in one damage unit.
pub const FIXED_DAMAGE_SCALE: u64 = 1_000_000;
/// Fixed-point scale used by expected-damage outcome probabilities.
pub const EXPECTED_DAMAGE_PROBABILITY_SCALE: u32 = 1_000_000;

/// Both mitigated damage families floor at this fraction of the raw hit. See
/// `ActorCombatFacts::REVIEWED_MINIMUM_DAMAGE_RATIO` for the reviewed formulas behind it.
const DAMAGE_FLOOR_NUMERATOR: u64 = 5;
const DAMAGE_FLOOR_DENOMINATOR: u64 = 100;
const MAX_ARTS_RESISTANCE_SCALED_PERCENT: u64 = 100 * FIXED_DAMAGE_SCALE;
const U64_EXCLUSIVE_UPPER_BOUND_AS_F64: f64 = 18_446_744_073_709_551_616.0;

/// Non-negative damage with exactly six fixed decimal places.
///
/// Floating-point inputs are rounded to the nearest micro-unit. Arithmetic that discards
/// fractional micro-units, such as resistance and the physical damage floor, rounds down.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FixedDamage(u64);

impl FixedDamage {
	pub const MAX: Self = Self(u64::MAX);
	pub const ZERO: Self = Self(0);

	#[must_use]
	pub const fn from_scaled(value: u64) -> Self {
		Self(value)
	}

	pub fn from_whole(value: u64) -> Result<Self, FixedDamageError> {
		value
			.checked_mul(FIXED_DAMAGE_SCALE)
			.map(Self)
			.ok_or(FixedDamageError::Overflow)
	}

	pub fn from_parts(whole: u64, fractional_micros: u32) -> Result<Self, FixedDamageError> {
		if u64::from(fractional_micros) >= FIXED_DAMAGE_SCALE {
			return Err(FixedDamageError::FractionOutOfRange);
		}
		let scaled_whole: u64 = whole
			.checked_mul(FIXED_DAMAGE_SCALE)
			.ok_or(FixedDamageError::Overflow)?;
		scaled_whole
			.checked_add(u64::from(fractional_micros))
			.map(Self)
			.ok_or(FixedDamageError::Overflow)
	}

	pub fn try_from_f64(value: f64) -> Result<Self, FixedDamageError> {
		if !value.is_finite() {
			return Err(FixedDamageError::NonFinite);
		}
		if value < 0.0 {
			return Err(FixedDamageError::Negative);
		}
		let scaled: f64 = value * FIXED_DAMAGE_SCALE as f64;
		let rounded: f64 = scaled.round();
		if !rounded.is_finite() || rounded >= U64_EXCLUSIVE_UPPER_BOUND_AS_F64 {
			return Err(FixedDamageError::Overflow);
		}
		Ok(Self(rounded as u64))
	}

	#[must_use]
	pub const fn scaled(self) -> u64 {
		self.0
	}

	#[must_use]
	pub const fn whole_floor(self) -> u64 {
		self.0 / FIXED_DAMAGE_SCALE
	}

	#[must_use]
	pub const fn is_zero(self) -> bool {
		self.0 == 0
	}

	pub fn checked_add(self, other: Self) -> Result<Self, FixedDamageError> {
		self.0
			.checked_add(other.0)
			.map(Self)
			.ok_or(FixedDamageError::Overflow)
	}

	pub fn checked_sub(self, other: Self) -> Result<Self, FixedDamageError> {
		self.0
			.checked_sub(other.0)
			.map(Self)
			.ok_or(FixedDamageError::Underflow)
	}

	pub fn checked_mul_ratio(
		self,
		numerator: u64,
		denominator: u64,
	) -> Result<Self, FixedDamageError> {
		if denominator == 0 {
			return Err(FixedDamageError::DivisionByZero);
		}
		let product: u128 = u128::from(self.0) * u128::from(numerator);
		let scaled: u128 = product / u128::from(denominator);
		u64::try_from(scaled)
			.map(Self)
			.map_err(|_| FixedDamageError::Overflow)
	}
}

impl TryFrom<f64> for FixedDamage {
	type Error = FixedDamageError;

	fn try_from(value: f64) -> Result<Self, Self::Error> {
		Self::try_from_f64(value)
	}
}

impl Display for FixedDamage {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		let whole: u64 = self.0 / FIXED_DAMAGE_SCALE;
		let fractional: u64 = self.0 % FIXED_DAMAGE_SCALE;
		write!(formatter, "{whole}.{fractional:06}")
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FixedDamageError {
	NonFinite,
	Negative,
	FractionOutOfRange,
	DivisionByZero,
	Underflow,
	Overflow,
}

impl Display for FixedDamageError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NonFinite => formatter.write_str("damage must be finite"),
			Self::Negative => formatter.write_str("damage must be non-negative"),
			Self::FractionOutOfRange => {
				formatter.write_str("fractional damage must be below one million micro-units")
			}
			Self::DivisionByZero => {
				formatter.write_str("damage ratio denominator must be positive")
			}
			Self::Underflow => formatter.write_str("damage arithmetic underflowed"),
			Self::Overflow => formatter.write_str("damage arithmetic overflowed"),
		}
	}
}

impl Error for FixedDamageError {}

/// Arts resistance expressed as a percentage with six fixed decimal places.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtsResistance(u64);

impl ArtsResistance {
	pub const ZERO: Self = Self(0);

	pub fn from_scaled_percent(value: u64) -> Result<Self, ArtsResistanceError> {
		if value > MAX_ARTS_RESISTANCE_SCALED_PERCENT {
			return Err(ArtsResistanceError::AboveOneHundredPercent);
		}
		Ok(Self(value))
	}

	pub fn try_from_percent_f64(value: f64) -> Result<Self, ArtsResistanceError> {
		let fixed: FixedDamage =
			FixedDamage::try_from_f64(value).map_err(ArtsResistanceError::InvalidValue)?;
		Self::from_scaled_percent(fixed.scaled())
	}

	#[must_use]
	pub const fn scaled_percent(self) -> u64 {
		self.0
	}
}

impl TryFrom<f64> for ArtsResistance {
	type Error = ArtsResistanceError;

	fn try_from(value: f64) -> Result<Self, Self::Error> {
		Self::try_from_percent_f64(value)
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ArtsResistanceError {
	InvalidValue(FixedDamageError),
	AboveOneHundredPercent,
}

impl Display for ArtsResistanceError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::InvalidValue(source) => write!(formatter, "invalid arts resistance: {source}"),
			Self::AboveOneHundredPercent => {
				formatter.write_str("arts resistance must not exceed 100 percent")
			}
		}
	}
}

impl Error for ArtsResistanceError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::InvalidValue(source) => Some(source),
			Self::AboveOneHundredPercent => None,
		}
	}
}

/// Defenses used by the base-panel normal-attack damage formulas.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemyDefenses {
	physical_defense: FixedDamage,
	arts_resistance: ArtsResistance,
}

impl EnemyDefenses {
	#[must_use]
	pub const fn new(physical_defense: FixedDamage, arts_resistance: ArtsResistance) -> Self {
		Self {
			physical_defense,
			arts_resistance,
		}
	}

	#[must_use]
	pub const fn physical_defense(self) -> FixedDamage {
		self.physical_defense
	}

	#[must_use]
	pub const fn arts_resistance(self) -> ArtsResistance {
		self.arts_resistance
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackDamageType {
	Physical,
	Arts,
	True,
}

/// One raw-damage outcome in an expected-value packet.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectedRawDamageOutcome {
	probability_millionths: u32,
	raw_damage: FixedDamage,
}

impl ExpectedRawDamageOutcome {
	pub fn new(
		probability_millionths: u32,
		raw_damage: FixedDamage,
	) -> Result<Self, ExpectedDamagePacketError> {
		if probability_millionths == 0 || probability_millionths > EXPECTED_DAMAGE_PROBABILITY_SCALE
		{
			return Err(ExpectedDamagePacketError::InvalidOutcomeProbability {
				probability_millionths,
			});
		}
		Ok(Self {
			probability_millionths,
			raw_damage,
		})
	}

	#[must_use]
	pub const fn probability_millionths(self) -> u32 {
		self.probability_millionths
	}

	#[must_use]
	pub const fn raw_damage(self) -> FixedDamage {
		self.raw_damage
	}
}

/// One packet applied to every target locked when the normal attack begins.
///
/// Probabilistic outcomes are individually passed through the target's nonlinear defense formula
/// before their weighted mean is applied to HP.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LockedTargetDamagePacket {
	damage_type: NormalAttackDamageType,
	outcomes: Vec<ExpectedRawDamageOutcome>,
	base_damage: FixedDamage,
}

impl LockedTargetDamagePacket {
	#[must_use]
	pub fn new(damage_type: NormalAttackDamageType, base_damage: FixedDamage) -> Self {
		Self {
			damage_type,
			outcomes: vec![ExpectedRawDamageOutcome {
				probability_millionths: EXPECTED_DAMAGE_PROBABILITY_SCALE,
				raw_damage: base_damage,
			}],
			base_damage,
		}
	}

	pub fn from_expected_outcomes(
		damage_type: NormalAttackDamageType,
		outcomes: Vec<ExpectedRawDamageOutcome>,
	) -> Result<Self, ExpectedDamagePacketError> {
		if outcomes.is_empty() {
			return Err(ExpectedDamagePacketError::EmptyOutcomes);
		}
		let probability_sum: u64 = outcomes
			.iter()
			.map(|outcome| u64::from(outcome.probability_millionths))
			.sum();
		if probability_sum != u64::from(EXPECTED_DAMAGE_PROBABILITY_SCALE) {
			return Err(ExpectedDamagePacketError::ProbabilitySum {
				probability_millionths: probability_sum,
			});
		}
		let base_damage: FixedDamage = weighted_damage(
			outcomes
				.iter()
				.map(|outcome| (outcome.probability_millionths, outcome.raw_damage)),
		)
		.map_err(ExpectedDamagePacketError::Arithmetic)?;
		Ok(Self {
			damage_type,
			outcomes,
			base_damage,
		})
	}

	#[must_use]
	pub const fn damage_type(&self) -> NormalAttackDamageType {
		self.damage_type
	}

	/// Raw expected damage, retained for audit and deterministic-call compatibility.
	#[must_use]
	pub const fn base_damage(&self) -> FixedDamage {
		self.base_damage
	}

	#[must_use]
	pub fn outcomes(&self) -> &[ExpectedRawDamageOutcome] {
		&self.outcomes
	}

	pub fn effective_damage(
		&self,
		defenses: EnemyDefenses,
	) -> Result<FixedDamage, FixedDamageError> {
		weighted_damage(self.outcomes.iter().map(|outcome| {
			(
				outcome.probability_millionths,
				effective_damage(self.damage_type, outcome.raw_damage, defenses),
			)
		}))
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExpectedDamagePacketError {
	EmptyOutcomes,
	InvalidOutcomeProbability { probability_millionths: u32 },
	ProbabilitySum { probability_millionths: u64 },
	Arithmetic(FixedDamageError),
}

impl Display for ExpectedDamagePacketError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::EmptyOutcomes => {
				formatter.write_str("expected-damage packet must contain at least one outcome")
			}
			Self::InvalidOutcomeProbability {
				probability_millionths,
			} => write!(
				formatter,
				"outcome probability must be within 1..={EXPECTED_DAMAGE_PROBABILITY_SCALE}, got \
				 {probability_millionths}"
			),
			Self::ProbabilitySum {
				probability_millionths,
			} => write!(
				formatter,
				"outcome probabilities must sum to {EXPECTED_DAMAGE_PROBABILITY_SCALE}, got \
				 {probability_millionths}"
			),
			Self::Arithmetic(source) => write!(formatter, "expected-damage arithmetic: {source}"),
		}
	}
}

impl Error for ExpectedDamagePacketError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::Arithmetic(source) => Some(source),
			Self::EmptyOutcomes
			| Self::InvalidOutcomeProbability { .. }
			| Self::ProbabilitySum { .. } => None,
		}
	}
}

fn weighted_damage<I, T>(outcomes: I) -> Result<FixedDamage, FixedDamageError>
where
	I: IntoIterator<Item = (u32, T)>,
	T: IntoExpectedDamage,
{
	let mut weighted_scaled: u128 = 0;
	for (probability_millionths, damage) in outcomes {
		let damage: FixedDamage = damage.into_expected_damage()?;
		weighted_scaled = weighted_scaled
			.checked_add(u128::from(damage.scaled()) * u128::from(probability_millionths))
			.ok_or(FixedDamageError::Overflow)?;
	}
	let expected_scaled: u128 = weighted_scaled / u128::from(EXPECTED_DAMAGE_PROBABILITY_SCALE);
	u64::try_from(expected_scaled)
		.map(FixedDamage::from_scaled)
		.map_err(|_| FixedDamageError::Overflow)
}

trait IntoExpectedDamage {
	fn into_expected_damage(self) -> Result<FixedDamage, FixedDamageError>;
}

impl IntoExpectedDamage for FixedDamage {
	fn into_expected_damage(self) -> Result<FixedDamage, FixedDamageError> {
		Ok(self)
	}
}

impl IntoExpectedDamage for Result<FixedDamage, FixedDamageError> {
	fn into_expected_damage(self) -> Result<FixedDamage, FixedDamageError> {
		self
	}
}

/// Computes post-defense damage before applying the target's remaining-HP cap.
pub fn effective_damage(
	damage_type: NormalAttackDamageType,
	base_damage: FixedDamage,
	defenses: EnemyDefenses,
) -> Result<FixedDamage, FixedDamageError> {
	let minimum: FixedDamage =
		base_damage.checked_mul_ratio(DAMAGE_FLOOR_NUMERATOR, DAMAGE_FLOOR_DENOMINATOR)?;
	match damage_type {
		NormalAttackDamageType::Physical => {
			let after_defense: FixedDamage = FixedDamage::from_scaled(
				base_damage
					.scaled()
					.saturating_sub(defenses.physical_defense().scaled()),
			);
			Ok(after_defense.max(minimum))
		}
		NormalAttackDamageType::Arts => {
			let retained_scaled_percent: u64 = MAX_ARTS_RESISTANCE_SCALED_PERCENT
				.checked_sub(defenses.arts_resistance().scaled_percent())
				.expect("validated arts resistance cannot exceed 100 percent");
			let after_resistance: FixedDamage = base_damage
				.checked_mul_ratio(retained_scaled_percent, MAX_ARTS_RESISTANCE_SCALED_PERCENT)?;
			Ok(after_resistance.max(minimum))
		}
		NormalAttackDamageType::True => Ok(base_damage),
	}
}

/// Standard game target ordering. Spatial scoring supplies progress and stable tie-break facts.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackTargetPriority {
	#[default]
	Standard,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackTargetFilter {
	GroundAndAir,
	GroundOnly,
	AirOnly,
}

impl NormalAttackTargetFilter {
	#[must_use]
	pub const fn allows(self, target_class: TargetClass) -> bool {
		match (self, target_class) {
			(Self::GroundAndAir, _)
			| (Self::GroundOnly, TargetClass::Ground)
			| (Self::AirOnly, TargetClass::Air) => true,
			(Self::GroundOnly, TargetClass::Air) | (Self::AirOnly, TargetClass::Ground) => false,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetClass {
	Ground,
	Air,
}

/// Reviewed base-panel normal-attack semantics.
///
/// The target set is locked once per attack. Every packet is then applied, in source order, to
/// every locked target without reacquiring a replacement after a kill.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NormalAttackProfile {
	target_priority: NormalAttackTargetPriority,
	target_count: NonZeroU32,
	packets: Vec<LockedTargetDamagePacket>,
	first_attack_delay_ticks: u64,
	attack_interval_ticks: NonZeroU64,
	target_filter: NormalAttackTargetFilter,
}

impl NormalAttackProfile {
	pub fn new(
		target_count: u32,
		packets: Vec<LockedTargetDamagePacket>,
		first_attack_delay_ticks: u64,
		attack_interval_ticks: u64,
		target_filter: NormalAttackTargetFilter,
	) -> Result<Self, NormalAttackProfileError> {
		let target_count: NonZeroU32 =
			NonZeroU32::new(target_count).ok_or(NormalAttackProfileError::ZeroTargetCount)?;
		if packets.is_empty() {
			return Err(NormalAttackProfileError::EmptyDamagePackets);
		}
		let attack_interval_ticks: NonZeroU64 = NonZeroU64::new(attack_interval_ticks)
			.ok_or(NormalAttackProfileError::ZeroAttackInterval)?;
		Ok(Self {
			target_priority: NormalAttackTargetPriority::Standard,
			target_count,
			packets,
			first_attack_delay_ticks,
			attack_interval_ticks,
			target_filter,
		})
	}

	#[must_use]
	pub const fn target_priority(&self) -> NormalAttackTargetPriority {
		self.target_priority
	}

	#[must_use]
	pub const fn target_count(&self) -> NonZeroU32 {
		self.target_count
	}

	#[must_use]
	pub fn packets(&self) -> &[LockedTargetDamagePacket] {
		&self.packets
	}

	#[must_use]
	pub const fn first_attack_delay_ticks(&self) -> u64 {
		self.first_attack_delay_ticks
	}

	#[must_use]
	pub const fn attack_interval_ticks(&self) -> NonZeroU64 {
		self.attack_interval_ticks
	}

	#[must_use]
	pub const fn target_filter(&self) -> NormalAttackTargetFilter {
		self.target_filter
	}

	/// Returns the tick for the zero-based attack index, or `None` on tick overflow.
	#[must_use]
	pub fn checked_attack_tick(
		&self,
		deployment_tick: u64,
		zero_based_attack_index: u64,
	) -> Option<u64> {
		let repeated_intervals: u64 = self
			.attack_interval_ticks
			.get()
			.checked_mul(zero_based_attack_index)?;
		deployment_tick
			.checked_add(self.first_attack_delay_ticks)?
			.checked_add(repeated_intervals)
	}

	/// Applies one attack transactionally to an already selected, ordered target set.
	///
	/// The caller owns spatial eligibility and standard-priority sorting. This method rejects
	/// too many, duplicate, or filter-ineligible locked targets.
	pub fn apply_locked_targets<Id: Eq>(
		&self,
		targets: &mut [EnemyDamageState<Id>],
	) -> Result<NormalAttackDamageReport, NormalAttackApplicationError> {
		let maximum: usize = usize::try_from(self.target_count.get())
			.expect("u32 target count must fit the supported platform's usize");
		if targets.len() > maximum {
			return Err(NormalAttackApplicationError::TooManyLockedTargets {
				locked: targets.len(),
				maximum: self.target_count,
			});
		}
		for duplicate_index in 1..targets.len() {
			if let Some(first_index) = targets[..duplicate_index]
				.iter()
				.position(|target| target.id() == targets[duplicate_index].id())
			{
				return Err(NormalAttackApplicationError::DuplicateLockedTarget {
					first_index,
					duplicate_index,
				});
			}
		}
		for (locked_target_index, target) in targets.iter().enumerate() {
			if !self.target_filter.allows(target.target_class()) {
				return Err(NormalAttackApplicationError::TargetRejectedByFilter {
					locked_target_index,
					filter: self.target_filter,
					target_class: target.target_class(),
				});
			}
		}

		let mut proposed: Vec<ProposedEnemyDamage> = targets
			.iter()
			.map(|target| ProposedEnemyDamage {
				remaining_hp: target.remaining_hp,
				accounting: target.accounting,
			})
			.collect();
		let application_capacity: usize = self.packets.len().checked_mul(targets.len()).ok_or(
			NormalAttackApplicationError::Arithmetic(FixedDamageError::Overflow),
		)?;
		let mut applications: Vec<AppliedLockedTargetPacket> =
			Vec::with_capacity(application_capacity);
		let mut accounting: DamageAccounting = DamageAccounting::ZERO;

		for (packet_index, packet) in self.packets.iter().enumerate() {
			for (locked_target_index, target) in targets.iter().enumerate() {
				let raw_damage: FixedDamage = packet
					.effective_damage(target.defenses())
					.map_err(NormalAttackApplicationError::Arithmetic)?;
				let application: HpCappedDamage =
					hp_capped_damage(proposed[locked_target_index].remaining_hp, raw_damage);
				proposed[locked_target_index].remaining_hp = application.remaining_hp_after();
				proposed[locked_target_index].accounting = proposed[locked_target_index]
					.accounting
					.checked_add(application.accounting())
					.map_err(NormalAttackApplicationError::Arithmetic)?;
				accounting = accounting
					.checked_add(application.accounting())
					.map_err(NormalAttackApplicationError::Arithmetic)?;
				applications.push(AppliedLockedTargetPacket {
					packet_index,
					locked_target_index,
					damage_type: packet.damage_type(),
					damage: application,
				});
			}
		}

		for (target, next) in targets.iter_mut().zip(proposed) {
			target.remaining_hp = next.remaining_hp;
			target.accounting = next.accounting;
		}
		Ok(NormalAttackDamageReport {
			applications,
			accounting,
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackProfileError {
	ZeroTargetCount,
	EmptyDamagePackets,
	ZeroAttackInterval,
}

impl Display for NormalAttackProfileError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::ZeroTargetCount => {
				formatter.write_str("normal attack target count must be positive")
			}
			Self::EmptyDamagePackets => {
				formatter.write_str("normal attack must contain at least one damage packet")
			}
			Self::ZeroAttackInterval => {
				formatter.write_str("normal attack interval must be positive")
			}
		}
	}
}

impl Error for NormalAttackProfileError {}

/// Typed fail-closed reasons for excluding a base-panel normal-attack candidate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum UnsupportedNormalAttackReason {
	SkillDependent,
	Splash,
	Chain,
	ChargedAttack,
	Summon,
	SpecialTargetPriority,
	DynamicTargetCount,
	ConditionalDamagePackets,
	SpecialTargetFilter,
	UnresolvedTiming,
	Other(String),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnsupportedNormalAttackProfile {
	reasons: Vec<UnsupportedNormalAttackReason>,
}

impl UnsupportedNormalAttackProfile {
	#[must_use]
	pub fn new(reason: UnsupportedNormalAttackReason) -> Self {
		Self {
			reasons: vec![reason],
		}
	}

	pub fn from_reasons(
		reasons: Vec<UnsupportedNormalAttackReason>,
	) -> Result<Self, UnsupportedNormalAttackProfileError> {
		if reasons.is_empty() {
			return Err(UnsupportedNormalAttackProfileError::NoReasons);
		}
		Ok(Self { reasons })
	}

	#[must_use]
	pub fn with_reason(mut self, reason: UnsupportedNormalAttackReason) -> Self {
		self.reasons.push(reason);
		self
	}

	#[must_use]
	pub fn reasons(&self) -> &[UnsupportedNormalAttackReason] {
		&self.reasons
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum UnsupportedNormalAttackProfileError {
	NoReasons,
}

impl Display for UnsupportedNormalAttackProfileError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NoReasons => {
				formatter.write_str("unsupported normal attack must include at least one reason")
			}
		}
	}
}

impl Error for UnsupportedNormalAttackProfileError {}

/// Fail-closed result of reviewing one candidate's normal-attack semantics.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackProfileSupport {
	Supported(NormalAttackProfile),
	Unsupported(UnsupportedNormalAttackProfile),
}

impl NormalAttackProfileSupport {
	#[must_use]
	pub const fn as_supported(&self) -> Option<&NormalAttackProfile> {
		match self {
			Self::Supported(profile) => Some(profile),
			Self::Unsupported(_) => None,
		}
	}

	#[must_use]
	pub fn unsupported_reasons(&self) -> Option<&[UnsupportedNormalAttackReason]> {
		match self {
			Self::Supported(_) => None,
			Self::Unsupported(profile) => Some(profile.reasons()),
		}
	}
}

/// Raw, HP-credited, and overkill damage. The invariant is `raw = credited + overkill`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DamageAccounting {
	raw: FixedDamage,
	credited: FixedDamage,
	overkill: FixedDamage,
}

impl DamageAccounting {
	pub const ZERO: Self = Self {
		raw: FixedDamage::ZERO,
		credited: FixedDamage::ZERO,
		overkill: FixedDamage::ZERO,
	};

	#[must_use]
	pub const fn raw(self) -> FixedDamage {
		self.raw
	}

	#[must_use]
	pub const fn credited(self) -> FixedDamage {
		self.credited
	}

	#[must_use]
	pub const fn overkill(self) -> FixedDamage {
		self.overkill
	}

	pub fn checked_add(self, other: Self) -> Result<Self, FixedDamageError> {
		Ok(Self {
			raw: self.raw.checked_add(other.raw)?,
			credited: self.credited.checked_add(other.credited)?,
			overkill: self.overkill.checked_add(other.overkill)?,
		})
	}
}

/// One raw damage amount after applying the target's remaining-HP cap.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HpCappedDamage {
	accounting: DamageAccounting,
	remaining_hp_after: FixedDamage,
}

impl HpCappedDamage {
	#[must_use]
	pub const fn accounting(self) -> DamageAccounting {
		self.accounting
	}

	#[must_use]
	pub const fn raw(self) -> FixedDamage {
		self.accounting.raw()
	}

	#[must_use]
	pub const fn credited(self) -> FixedDamage {
		self.accounting.credited()
	}

	#[must_use]
	pub const fn overkill(self) -> FixedDamage {
		self.accounting.overkill()
	}

	#[must_use]
	pub const fn remaining_hp_after(self) -> FixedDamage {
		self.remaining_hp_after
	}
}

/// Caps credited damage at the target's remaining HP without discarding raw or overkill damage.
#[must_use]
pub fn hp_capped_damage(remaining_hp: FixedDamage, raw_damage: FixedDamage) -> HpCappedDamage {
	let credited: FixedDamage = remaining_hp.min(raw_damage);
	let overkill: FixedDamage = FixedDamage::from_scaled(raw_damage.scaled() - credited.scaled());
	let remaining_hp_after: FixedDamage =
		FixedDamage::from_scaled(remaining_hp.scaled() - credited.scaled());
	HpCappedDamage {
		accounting: DamageAccounting {
			raw: raw_damage,
			credited,
			overkill,
		},
		remaining_hp_after,
	}
}

/// Mutable per-enemy scoring state. Existing missing HP is not attributed to this profile.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EnemyDamageState<Id> {
	id: Id,
	target_class: TargetClass,
	defenses: EnemyDefenses,
	max_hp: FixedDamage,
	remaining_hp: FixedDamage,
	accounting: DamageAccounting,
}

impl<Id> EnemyDamageState<Id> {
	#[must_use]
	pub fn new(
		id: Id,
		target_class: TargetClass,
		max_hp: FixedDamage,
		defenses: EnemyDefenses,
	) -> Self {
		Self {
			id,
			target_class,
			defenses,
			max_hp,
			remaining_hp: max_hp,
			accounting: DamageAccounting::ZERO,
		}
	}

	pub fn with_remaining_hp(
		id: Id,
		target_class: TargetClass,
		max_hp: FixedDamage,
		remaining_hp: FixedDamage,
		defenses: EnemyDefenses,
	) -> Result<Self, EnemyDamageStateError> {
		if remaining_hp > max_hp {
			return Err(EnemyDamageStateError::RemainingHpExceedsMaximum);
		}
		Ok(Self {
			id,
			target_class,
			defenses,
			max_hp,
			remaining_hp,
			accounting: DamageAccounting::ZERO,
		})
	}

	#[must_use]
	pub const fn id(&self) -> &Id {
		&self.id
	}

	#[must_use]
	pub const fn target_class(&self) -> TargetClass {
		self.target_class
	}

	#[must_use]
	pub const fn defenses(&self) -> EnemyDefenses {
		self.defenses
	}

	#[must_use]
	pub const fn max_hp(&self) -> FixedDamage {
		self.max_hp
	}

	#[must_use]
	pub const fn remaining_hp(&self) -> FixedDamage {
		self.remaining_hp
	}

	#[must_use]
	pub const fn accounting(&self) -> DamageAccounting {
		self.accounting
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EnemyDamageStateError {
	RemainingHpExceedsMaximum,
}

impl Display for EnemyDamageStateError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::RemainingHpExceedsMaximum => {
				formatter.write_str("remaining HP must not exceed maximum HP")
			}
		}
	}
}

impl Error for EnemyDamageStateError {}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ProposedEnemyDamage {
	remaining_hp: FixedDamage,
	accounting: DamageAccounting,
}

/// Audit record for one source packet applied to one locked target.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AppliedLockedTargetPacket {
	packet_index: usize,
	locked_target_index: usize,
	damage_type: NormalAttackDamageType,
	damage: HpCappedDamage,
}

impl AppliedLockedTargetPacket {
	#[must_use]
	pub const fn packet_index(self) -> usize {
		self.packet_index
	}

	#[must_use]
	pub const fn locked_target_index(self) -> usize {
		self.locked_target_index
	}

	#[must_use]
	pub const fn damage_type(self) -> NormalAttackDamageType {
		self.damage_type
	}

	#[must_use]
	pub const fn damage(self) -> HpCappedDamage {
		self.damage
	}
}

/// Aggregate and per-packet accounting for one normal attack.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NormalAttackDamageReport {
	applications: Vec<AppliedLockedTargetPacket>,
	accounting: DamageAccounting,
}

impl NormalAttackDamageReport {
	#[must_use]
	pub fn applications(&self) -> &[AppliedLockedTargetPacket] {
		&self.applications
	}

	#[must_use]
	pub const fn accounting(&self) -> DamageAccounting {
		self.accounting
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NormalAttackApplicationError {
	TooManyLockedTargets {
		locked: usize,
		maximum: NonZeroU32,
	},
	DuplicateLockedTarget {
		first_index: usize,
		duplicate_index: usize,
	},
	TargetRejectedByFilter {
		locked_target_index: usize,
		filter: NormalAttackTargetFilter,
		target_class: TargetClass,
	},
	Arithmetic(FixedDamageError),
}

impl Display for NormalAttackApplicationError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::TooManyLockedTargets { locked, maximum } => write!(
				formatter,
				"normal attack locked {locked} targets but supports at most {maximum}"
			),
			Self::DuplicateLockedTarget {
				first_index,
				duplicate_index,
			} => write!(
				formatter,
				"locked target {duplicate_index} duplicates target {first_index}"
			),
			Self::TargetRejectedByFilter {
				locked_target_index,
				filter,
				target_class,
			} => write!(
				formatter,
				"locked target {locked_target_index} class {target_class:?} is rejected by {filter:?}"
			),
			Self::Arithmetic(source) => write!(formatter, "normal attack scoring failed: {source}"),
		}
	}
}

impl Error for NormalAttackApplicationError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::Arithmetic(source) => Some(source),
			Self::TooManyLockedTargets { .. }
			| Self::DuplicateLockedTarget { .. }
			| Self::TargetRejectedByFilter { .. } => None,
		}
	}
}
