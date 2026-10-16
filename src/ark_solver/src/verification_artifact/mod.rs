//! Canonical internal verification artifacts.

mod generated_wire;

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use self::generated_wire::strategy_artifact::rogue::strategy as wire;
use flatbuffers::FlatBufferBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const STRATEGY_ARTIFACT_SCHEMA_VERSION: u32 = 4;
pub const SHA256_LENGTH: usize = 32;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeterministicDamageStatus {
	Unsupported,
	Feasible,
	Infeasible,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedStrategyStatus {
	Unsupported,
	ExpectedVictory,
	ExpectedDefeat,
	Incomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializedActionKind {
	Deploy,
	Skill,
	Retreat,
	SpeedUp,
	SkillDaemon,
	Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFacing {
	None,
	Left,
	Right,
	Up,
	Down,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactExactBuild {
	pub operator_id: String,
	pub elite: u8,
	pub level: u32,
	pub trust_percent: u16,
	pub potential_rank: u8,
	pub skill_slot: u8,
	pub skill_level: u8,
	pub module_id: Option<String>,
	pub module_level: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactAction {
	pub action_index: u32,
	pub kind: MaterializedActionKind,
	pub operator_id: Option<String>,
	pub row: Option<i32>,
	pub column: Option<i32>,
	pub facing: ArtifactFacing,
	pub scheduled_tick: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactEnemyAssessment {
	pub enemy_instance_id: String,
	pub spawned_tick: Option<u64>,
	pub deadline_tick: Option<u64>,
	pub max_hp_micros: i64,
	pub remaining_hp_micros: i64,
	pub credited_damage_micros: i64,
	pub death_tick: Option<u64>,
	pub leaked_tick: Option<u64>,
	pub deadline_margin_ticks: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactOperatorAssessment {
	pub operator_id: String,
	pub max_hp_micros: i64,
	pub remaining_hp_micros: i64,
	/// Start tick of the operator's most recent deployment episode.
	pub deployed_tick: Option<u64>,
	/// Death tick of that deployment episode, or `None` if it did not end in death.
	pub death_tick: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ArtifactUnsupportedMechanic {
	pub mechanic_id: String,
	pub origin: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StrategyArtifact {
	pub schema_version: u32,
	pub data_revision: String,
	pub mechanics_revision: String,
	pub guide_sha256: [u8; SHA256_LENGTH],
	pub guide_variant_id: String,
	pub damage_status: DeterministicDamageStatus,
	pub battlefield_status: ExpectedStrategyStatus,
	pub minimum_deadline_margin_ticks: i64,
	pub minimum_operator_hp_micros: i64,
	pub actions: Vec<ArtifactAction>,
	pub builds: Vec<ArtifactExactBuild>,
	pub enemies: Vec<ArtifactEnemyAssessment>,
	pub operators: Vec<ArtifactOperatorAssessment>,
	pub unsupported: Vec<ArtifactUnsupportedMechanic>,
	pub trace_sha256: [u8; SHA256_LENGTH],
}

impl StrategyArtifact {
	pub fn canonicalize(&mut self) {
		self.actions.sort_by_key(|action| action.action_index);
		self.builds.sort();
		self.enemies.sort();
		self.operators.sort();
		self.unsupported.sort();
	}

	pub fn validate(&self) -> Result<(), StrategyArtifactError> {
		if self.schema_version != STRATEGY_ARTIFACT_SCHEMA_VERSION {
			return Err(StrategyArtifactError::UnsupportedSchemaVersion {
				found: self.schema_version,
			});
		}
		require_nonblank("data_revision", &self.data_revision)?;
		require_sha256_hex("mechanics_revision", &self.mechanics_revision)?;
		require_nonblank("guide_variant_id", &self.guide_variant_id)?;
		for (expected, action) in self.actions.iter().enumerate() {
			let expected: u32 =
				u32::try_from(expected).map_err(|_| StrategyArtifactError::ActionIndexOverflow)?;
			if action.action_index != expected {
				return Err(StrategyArtifactError::NonCanonicalActionIndex {
					expected,
					found: action.action_index,
				});
			}
			if action.kind == MaterializedActionKind::Deploy
				&& (action.operator_id.as_deref().is_none_or(str::is_empty)
					|| action.row.is_none()
					|| action.column.is_none())
			{
				return Err(StrategyArtifactError::IncompleteDeployAction {
					index: action.action_index,
				});
			}
		}
		ensure_unique(
			self.builds.iter().map(|build| build.operator_id.as_str()),
			"build operator",
		)?;
		ensure_unique(
			self.enemies
				.iter()
				.map(|enemy| enemy.enemy_instance_id.as_str()),
			"enemy instance",
		)?;
		ensure_unique(
			self.operators
				.iter()
				.map(|operator| operator.operator_id.as_str()),
			"operator assessment",
		)?;
		ensure_unique(
			self.unsupported
				.iter()
				.map(|mechanic| (mechanic.mechanic_id.as_str(), mechanic.origin.as_str())),
			"unsupported mechanic",
		)?;
		for build in &self.builds {
			require_nonblank("build.operator_id", &build.operator_id)?;
			if !(1..=6).contains(&build.potential_rank) {
				return Err(StrategyArtifactError::InvalidPotentialRank {
					operator_id: build.operator_id.clone(),
					found: build.potential_rank,
				});
			}
			if build.trust_percent > 200 {
				return Err(StrategyArtifactError::InvalidTrust {
					operator_id: build.operator_id.clone(),
					found: build.trust_percent,
				});
			}
		}
		for enemy in &self.enemies {
			require_nonblank("enemy.enemy_instance_id", &enemy.enemy_instance_id)?;
			validate_optional_tick("enemy.spawned_tick", enemy.spawned_tick)?;
			validate_optional_tick("enemy.deadline_tick", enemy.deadline_tick)?;
			validate_optional_tick("enemy.death_tick", enemy.death_tick)?;
			validate_optional_tick("enemy.leaked_tick", enemy.leaked_tick)?;
		}
		for operator in &self.operators {
			require_nonblank("operator.operator_id", &operator.operator_id)?;
			validate_optional_tick("operator.deployed_tick", operator.deployed_tick)?;
			validate_optional_tick("operator.death_tick", operator.death_tick)?;
		}
		for mechanic in &self.unsupported {
			require_nonblank("unsupported.mechanic_id", &mechanic.mechanic_id)?;
			require_nonblank("unsupported.origin", &mechanic.origin)?;
		}
		Ok(())
	}

	pub fn encode(&self) -> Result<Vec<u8>, StrategyArtifactError> {
		let mut canonical: Self = self.clone();
		canonical.canonicalize();
		canonical.validate()?;
		let object: wire::StrategyArtifactT = canonical.into_wire();
		let mut builder: FlatBufferBuilder<'_> = FlatBufferBuilder::new();
		let root = object.pack(&mut builder);
		wire::finish_strategy_artifact_buffer(&mut builder, root);
		Ok(builder.finished_data().to_vec())
	}

	pub fn decode(bytes: &[u8]) -> Result<Self, StrategyArtifactError> {
		if !wire::strategy_artifact_buffer_has_identifier(bytes) {
			return Err(StrategyArtifactError::InvalidIdentifier);
		}
		let root = wire::root_as_strategy_artifact(bytes)
			.map_err(|error| StrategyArtifactError::InvalidFlatBuffer(error.to_string()))?;
		let artifact: Self = Self::from_wire(root.unpack())?;
		artifact.validate()?;
		Ok(artifact)
	}

	pub fn artifact_id(&self) -> Result<[u8; SHA256_LENGTH], StrategyArtifactError> {
		let bytes: Vec<u8> = self.encode()?;
		Ok(Sha256::digest(bytes).into())
	}

	fn into_wire(self) -> wire::StrategyArtifactT {
		wire::StrategyArtifactT {
			schema_version: self.schema_version,
			data_revision: self.data_revision,
			mechanics_revision: self.mechanics_revision,
			guide_sha256: self.guide_sha256.to_vec(),
			guide_variant_id: self.guide_variant_id,
			damage_status: damage_status_to_wire(self.damage_status),
			battlefield_status: battlefield_status_to_wire(self.battlefield_status),
			minimum_deadline_margin_ticks: self.minimum_deadline_margin_ticks,
			minimum_operator_hp_micros: self.minimum_operator_hp_micros,
			actions: self.actions.into_iter().map(action_to_wire).collect(),
			builds: self.builds.into_iter().map(build_to_wire).collect(),
			enemies: self.enemies.into_iter().map(enemy_to_wire).collect(),
			operators: self.operators.into_iter().map(operator_to_wire).collect(),
			unsupported: self
				.unsupported
				.into_iter()
				.map(unsupported_to_wire)
				.collect(),
			trace_sha256: self.trace_sha256.to_vec(),
		}
	}

	fn from_wire(value: wire::StrategyArtifactT) -> Result<Self, StrategyArtifactError> {
		Ok(Self {
			schema_version: value.schema_version,
			data_revision: value.data_revision,
			mechanics_revision: value.mechanics_revision,
			guide_sha256: digest_from_wire("guide_sha256", value.guide_sha256)?,
			guide_variant_id: value.guide_variant_id,
			damage_status: damage_status_from_wire(value.damage_status)?,
			battlefield_status: battlefield_status_from_wire(value.battlefield_status)?,
			minimum_deadline_margin_ticks: value.minimum_deadline_margin_ticks,
			minimum_operator_hp_micros: value.minimum_operator_hp_micros,
			actions: value
				.actions
				.into_iter()
				.map(action_from_wire)
				.collect::<Result<_, _>>()?,
			builds: value.builds.into_iter().map(build_from_wire).collect(),
			enemies: value
				.enemies
				.into_iter()
				.map(enemy_from_wire)
				.collect::<Result<_, _>>()?,
			operators: value
				.operators
				.into_iter()
				.map(operator_from_wire)
				.collect::<Result<_, _>>()?,
			unsupported: value
				.unsupported
				.into_iter()
				.map(unsupported_from_wire)
				.collect(),
			trace_sha256: digest_from_wire("trace_sha256", value.trace_sha256)?,
		})
	}
}

fn action_to_wire(value: ArtifactAction) -> wire::MaterializedActionT {
	wire::MaterializedActionT {
		action_index: value.action_index,
		kind: action_kind_to_wire(value.kind),
		operator_id: value.operator_id,
		row: value.row.unwrap_or_default(),
		column: value.column.unwrap_or_default(),
		facing: facing_to_wire(value.facing),
		scheduled_tick: value.scheduled_tick,
	}
}

fn action_from_wire(
	value: wire::MaterializedActionT,
) -> Result<ArtifactAction, StrategyArtifactError> {
	let kind: MaterializedActionKind = action_kind_from_wire(value.kind)?;
	let has_position: bool = kind == MaterializedActionKind::Deploy;
	Ok(ArtifactAction {
		action_index: value.action_index,
		kind,
		operator_id: value.operator_id,
		row: has_position.then_some(value.row),
		column: has_position.then_some(value.column),
		facing: facing_from_wire(value.facing)?,
		scheduled_tick: value.scheduled_tick,
	})
}

fn build_to_wire(value: ArtifactExactBuild) -> wire::ExactBuildT {
	wire::ExactBuildT {
		operator_id: value.operator_id,
		elite: value.elite,
		level: value.level,
		trust_percent: value.trust_percent,
		potential_rank: value.potential_rank,
		skill_slot: value.skill_slot,
		skill_level: value.skill_level,
		module_id: value.module_id,
		module_level: value.module_level,
	}
}

fn build_from_wire(value: wire::ExactBuildT) -> ArtifactExactBuild {
	ArtifactExactBuild {
		operator_id: value.operator_id,
		elite: value.elite,
		level: value.level,
		trust_percent: value.trust_percent,
		potential_rank: value.potential_rank,
		skill_slot: value.skill_slot,
		skill_level: value.skill_level,
		module_id: value.module_id,
		module_level: value.module_level,
	}
}

fn enemy_to_wire(value: ArtifactEnemyAssessment) -> wire::EnemyAssessmentT {
	wire::EnemyAssessmentT {
		enemy_instance_id: value.enemy_instance_id,
		spawned_tick: optional_tick_to_wire(value.spawned_tick),
		deadline_tick: optional_tick_to_wire(value.deadline_tick),
		max_hp_micros: value.max_hp_micros,
		remaining_hp_micros: value.remaining_hp_micros,
		credited_damage_micros: value.credited_damage_micros,
		death_tick: optional_tick_to_wire(value.death_tick),
		leaked_tick: optional_tick_to_wire(value.leaked_tick),
		deadline_margin_ticks: value.deadline_margin_ticks,
	}
}

fn enemy_from_wire(
	value: wire::EnemyAssessmentT,
) -> Result<ArtifactEnemyAssessment, StrategyArtifactError> {
	Ok(ArtifactEnemyAssessment {
		enemy_instance_id: value.enemy_instance_id,
		spawned_tick: optional_tick_from_wire("enemy.spawned_tick", value.spawned_tick)?,
		deadline_tick: optional_tick_from_wire("enemy.deadline_tick", value.deadline_tick)?,
		max_hp_micros: value.max_hp_micros,
		remaining_hp_micros: value.remaining_hp_micros,
		credited_damage_micros: value.credited_damage_micros,
		death_tick: optional_tick_from_wire("enemy.death_tick", value.death_tick)?,
		leaked_tick: optional_tick_from_wire("enemy.leaked_tick", value.leaked_tick)?,
		deadline_margin_ticks: value.deadline_margin_ticks,
	})
}

fn operator_to_wire(value: ArtifactOperatorAssessment) -> wire::OperatorAssessmentT {
	wire::OperatorAssessmentT {
		operator_id: value.operator_id,
		max_hp_micros: value.max_hp_micros,
		remaining_hp_micros: value.remaining_hp_micros,
		deployed_tick: optional_tick_to_wire(value.deployed_tick),
		death_tick: optional_tick_to_wire(value.death_tick),
	}
}

fn operator_from_wire(
	value: wire::OperatorAssessmentT,
) -> Result<ArtifactOperatorAssessment, StrategyArtifactError> {
	Ok(ArtifactOperatorAssessment {
		operator_id: value.operator_id,
		max_hp_micros: value.max_hp_micros,
		remaining_hp_micros: value.remaining_hp_micros,
		deployed_tick: optional_tick_from_wire("operator.deployed_tick", value.deployed_tick)?,
		death_tick: optional_tick_from_wire("operator.death_tick", value.death_tick)?,
	})
}

fn optional_tick_to_wire(value: Option<u64>) -> i64 {
	value
		.map(|tick| i64::try_from(tick).expect("optional tick was validated before encoding"))
		.unwrap_or(-1)
}

fn optional_tick_from_wire(
	field: &'static str,
	value: i64,
) -> Result<Option<u64>, StrategyArtifactError> {
	match value {
		-1 => Ok(None),
		0.. => Ok(Some(value.cast_unsigned())),
		_ => Err(StrategyArtifactError::InvalidOptionalTick { field, value }),
	}
}

fn unsupported_to_wire(value: ArtifactUnsupportedMechanic) -> wire::UnsupportedMechanicT {
	wire::UnsupportedMechanicT {
		mechanic_id: value.mechanic_id,
		origin: value.origin,
	}
}

fn unsupported_from_wire(value: wire::UnsupportedMechanicT) -> ArtifactUnsupportedMechanic {
	ArtifactUnsupportedMechanic {
		mechanic_id: value.mechanic_id,
		origin: value.origin,
	}
}

fn damage_status_to_wire(value: DeterministicDamageStatus) -> wire::DamageStatus {
	match value {
		DeterministicDamageStatus::Unsupported => wire::DamageStatus::Unsupported,
		DeterministicDamageStatus::Feasible => wire::DamageStatus::Feasible,
		DeterministicDamageStatus::Infeasible => wire::DamageStatus::Infeasible,
	}
}

fn damage_status_from_wire(
	value: wire::DamageStatus,
) -> Result<DeterministicDamageStatus, StrategyArtifactError> {
	match value {
		wire::DamageStatus::Unsupported => Ok(DeterministicDamageStatus::Unsupported),
		wire::DamageStatus::Feasible => Ok(DeterministicDamageStatus::Feasible),
		wire::DamageStatus::Infeasible => Ok(DeterministicDamageStatus::Infeasible),
		_ => Err(StrategyArtifactError::UnknownEnum {
			field: "damage_status",
			value: value.0,
		}),
	}
}

fn battlefield_status_to_wire(value: ExpectedStrategyStatus) -> wire::ExpectedStatus {
	match value {
		ExpectedStrategyStatus::Unsupported => wire::ExpectedStatus::Unsupported,
		ExpectedStrategyStatus::ExpectedVictory => wire::ExpectedStatus::ExpectedVictory,
		ExpectedStrategyStatus::ExpectedDefeat => wire::ExpectedStatus::ExpectedDefeat,
		ExpectedStrategyStatus::Incomplete => wire::ExpectedStatus::Incomplete,
	}
}

fn battlefield_status_from_wire(
	value: wire::ExpectedStatus,
) -> Result<ExpectedStrategyStatus, StrategyArtifactError> {
	match value {
		wire::ExpectedStatus::Unsupported => Ok(ExpectedStrategyStatus::Unsupported),
		wire::ExpectedStatus::ExpectedVictory => Ok(ExpectedStrategyStatus::ExpectedVictory),
		wire::ExpectedStatus::ExpectedDefeat => Ok(ExpectedStrategyStatus::ExpectedDefeat),
		wire::ExpectedStatus::Incomplete => Ok(ExpectedStrategyStatus::Incomplete),
		_ => Err(StrategyArtifactError::UnknownEnum {
			field: "battlefield_status",
			value: value.0,
		}),
	}
}

fn action_kind_to_wire(value: MaterializedActionKind) -> wire::ActionKind {
	match value {
		MaterializedActionKind::Deploy => wire::ActionKind::Deploy,
		MaterializedActionKind::Skill => wire::ActionKind::Skill,
		MaterializedActionKind::Retreat => wire::ActionKind::Retreat,
		MaterializedActionKind::SpeedUp => wire::ActionKind::SpeedUp,
		MaterializedActionKind::SkillDaemon => wire::ActionKind::SkillDaemon,
		MaterializedActionKind::Other => wire::ActionKind::Other,
	}
}

fn action_kind_from_wire(
	value: wire::ActionKind,
) -> Result<MaterializedActionKind, StrategyArtifactError> {
	match value {
		wire::ActionKind::Deploy => Ok(MaterializedActionKind::Deploy),
		wire::ActionKind::Skill => Ok(MaterializedActionKind::Skill),
		wire::ActionKind::Retreat => Ok(MaterializedActionKind::Retreat),
		wire::ActionKind::SpeedUp => Ok(MaterializedActionKind::SpeedUp),
		wire::ActionKind::SkillDaemon => Ok(MaterializedActionKind::SkillDaemon),
		wire::ActionKind::Other => Ok(MaterializedActionKind::Other),
		_ => Err(StrategyArtifactError::UnknownEnum {
			field: "action.kind",
			value: value.0,
		}),
	}
}

fn facing_to_wire(value: ArtifactFacing) -> wire::Facing {
	match value {
		ArtifactFacing::None => wire::Facing::None,
		ArtifactFacing::Left => wire::Facing::Left,
		ArtifactFacing::Right => wire::Facing::Right,
		ArtifactFacing::Up => wire::Facing::Up,
		ArtifactFacing::Down => wire::Facing::Down,
	}
}

fn facing_from_wire(value: wire::Facing) -> Result<ArtifactFacing, StrategyArtifactError> {
	match value {
		wire::Facing::None => Ok(ArtifactFacing::None),
		wire::Facing::Left => Ok(ArtifactFacing::Left),
		wire::Facing::Right => Ok(ArtifactFacing::Right),
		wire::Facing::Up => Ok(ArtifactFacing::Up),
		wire::Facing::Down => Ok(ArtifactFacing::Down),
		_ => Err(StrategyArtifactError::UnknownEnum {
			field: "action.facing",
			value: value.0.cast_unsigned(),
		}),
	}
}

fn digest_from_wire(
	field: &'static str,
	value: Vec<u8>,
) -> Result<[u8; SHA256_LENGTH], StrategyArtifactError> {
	value.try_into().map_err(
		|value: Vec<u8>| StrategyArtifactError::InvalidDigestLength {
			field,
			found: value.len(),
		},
	)
}

fn require_nonblank(field: &'static str, value: &str) -> Result<(), StrategyArtifactError> {
	if value.trim().is_empty() {
		Err(StrategyArtifactError::BlankField { field })
	} else {
		Ok(())
	}
}

fn require_sha256_hex(field: &'static str, value: &str) -> Result<(), StrategyArtifactError> {
	if value.len() == SHA256_LENGTH * 2
		&& value
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
	{
		Ok(())
	} else {
		Err(StrategyArtifactError::InvalidSha256Hex { field })
	}
}

fn validate_optional_tick(
	field: &'static str,
	value: Option<u64>,
) -> Result<(), StrategyArtifactError> {
	let Some(value) = value else {
		return Ok(());
	};
	i64::try_from(value)
		.map(|_| ())
		.map_err(|_| StrategyArtifactError::TickOverflow { field, value })
}

fn ensure_unique<T: Ord>(
	values: impl IntoIterator<Item = T>,
	field: &'static str,
) -> Result<(), StrategyArtifactError> {
	let mut seen: BTreeSet<T> = BTreeSet::new();
	for value in values {
		if !seen.insert(value) {
			return Err(StrategyArtifactError::DuplicateIdentity { field });
		}
	}
	Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StrategyArtifactError {
	UnsupportedSchemaVersion { found: u32 },
	InvalidIdentifier,
	InvalidFlatBuffer(String),
	BlankField { field: &'static str },
	InvalidSha256Hex { field: &'static str },
	InvalidDigestLength { field: &'static str, found: usize },
	NonCanonicalActionIndex { expected: u32, found: u32 },
	ActionIndexOverflow,
	IncompleteDeployAction { index: u32 },
	DuplicateIdentity { field: &'static str },
	InvalidPotentialRank { operator_id: String, found: u8 },
	InvalidTrust { operator_id: String, found: u16 },
	TickOverflow { field: &'static str, value: u64 },
	InvalidOptionalTick { field: &'static str, value: i64 },
	UnknownEnum { field: &'static str, value: u8 },
}

impl Display for StrategyArtifactError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::UnsupportedSchemaVersion { found } => {
				write!(
					formatter,
					"unsupported strategy artifact schema version {found}"
				)
			}
			Self::InvalidIdentifier => formatter.write_str("invalid strategy artifact identifier"),
			Self::InvalidFlatBuffer(error) => {
				write!(formatter, "invalid strategy artifact FlatBuffer: {error}")
			}
			Self::BlankField { field } => write!(formatter, "{field} must not be blank"),
			Self::InvalidSha256Hex { field } => {
				write!(
					formatter,
					"{field} must be 64 lowercase hexadecimal characters"
				)
			}
			Self::InvalidDigestLength { field, found } => {
				write!(formatter, "{field} must contain 32 bytes, found {found}")
			}
			Self::NonCanonicalActionIndex { expected, found } => write!(
				formatter,
				"materialized action index must be contiguous: expected {expected}, found {found}"
			),
			Self::ActionIndexOverflow => formatter.write_str("materialized action index overflow"),
			Self::IncompleteDeployAction { index } => {
				write!(
					formatter,
					"Deploy action {index} is missing its operator or position"
				)
			}
			Self::DuplicateIdentity { field } => write!(formatter, "duplicate {field} identity"),
			Self::InvalidPotentialRank { operator_id, found } => write!(
				formatter,
				"build {operator_id} has invalid potential rank {found}; expected P1..P6"
			),
			Self::InvalidTrust { operator_id, found } => write!(
				formatter,
				"build {operator_id} has invalid trust {found}; expected 0..=200"
			),
			Self::TickOverflow { field, value } => {
				write!(formatter, "{field} tick {value} exceeds the artifact range")
			}
			Self::InvalidOptionalTick { field, value } => {
				write!(
					formatter,
					"{field} has invalid encoded optional tick {value}"
				)
			}
			Self::UnknownEnum { field, value } => {
				write!(formatter, "{field} has unknown encoded value {value}")
			}
		}
	}
}

impl Error for StrategyArtifactError {}
