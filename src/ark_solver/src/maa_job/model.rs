use std::fmt::{Display, Formatter};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Difficulty bit mask defined by the MAA job file schema.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MaaJobDifficultyMask(u8);

impl MaaJobDifficultyMask {
	pub const UNSPECIFIED: Self = Self(0);
	pub const NORMAL: Self = Self(1);
	pub const CHALLENGE: Self = Self(2);

	pub fn new(value: u8) -> Result<Self, String> {
		if value & !3 == 0 {
			Ok(Self(value))
		} else {
			Err(format!(
				"unsupported MAA job file difficulty bit mask {value}"
			))
		}
	}

	#[must_use]
	pub const fn value(self) -> u8 {
		self.0
	}

	#[must_use]
	pub const fn contains(self, difficulty: MaaJobDifficulty) -> bool {
		if self.0 == 0 {
			return true;
		}
		match difficulty {
			MaaJobDifficulty::Unspecified => false,
			MaaJobDifficulty::Normal => self.0 & Self::NORMAL.0 != 0,
			MaaJobDifficulty::Challenge => self.0 & Self::CHALLENGE.0 != 0,
		}
	}

	#[must_use]
	pub fn selected(self) -> Vec<MaaJobDifficulty> {
		if self == Self::UNSPECIFIED {
			return vec![MaaJobDifficulty::Unspecified];
		}
		let mut selected: Vec<MaaJobDifficulty> = Vec::with_capacity(2);
		if self.contains(MaaJobDifficulty::Normal) {
			selected.push(MaaJobDifficulty::Normal);
		}
		if self.contains(MaaJobDifficulty::Challenge) {
			selected.push(MaaJobDifficulty::Challenge);
		}
		selected
	}
}

impl Serialize for MaaJobDifficultyMask {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_u8(self.0)
	}
}

impl<'de> Deserialize<'de> for MaaJobDifficultyMask {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let value: u8 = u8::deserialize(deserializer)?;
		Self::new(value).map_err(D::Error::custom)
	}
}

/// One selected stage mode after expanding the wire difficulty mask.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaaJobDifficulty {
	Unspecified,
	Normal,
	Challenge,
}

impl Display for MaaJobDifficulty {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Unspecified => formatter.write_str("unspecified"),
			Self::Normal => formatter.write_str("normal"),
			Self::Challenge => formatter.write_str("challenge"),
		}
	}
}

/// Deployment facing in canonical MAA spelling.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum MaaJobDirection {
	Left,
	Right,
	Up,
	Down,
	None,
}

impl<'de> Deserialize<'de> for MaaJobDirection {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		match String::deserialize(deserializer)?.as_str() {
			"Left" | "左" => Ok(Self::Left),
			"Right" | "右" => Ok(Self::Right),
			"Up" | "上" => Ok(Self::Up),
			"Down" | "下" => Ok(Self::Down),
			"None" | "无" => Ok(Self::None),
			value => Err(D::Error::custom(format!(
				"unsupported MAA job file direction {value:?}"
			))),
		}
	}
}

/// MAA automatic-skill policy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MaaSkillUsage {
	Disabled,
	WhenReady,
	SpecifiedTimes,
	AutoDetermine,
}

impl MaaSkillUsage {
	pub fn new(value: u8) -> Result<Self, String> {
		match value {
			0 => Ok(Self::Disabled),
			1 => Ok(Self::WhenReady),
			2 => Ok(Self::SpecifiedTimes),
			3 => Ok(Self::AutoDetermine),
			_ => Err(format!("skill_usage must be in 0..=3, got {value}")),
		}
	}

	#[must_use]
	pub const fn value(self) -> u8 {
		match self {
			Self::Disabled => 0,
			Self::WhenReady => 1,
			Self::SpecifiedTimes => 2,
			Self::AutoDetermine => 3,
		}
	}
}

impl Serialize for MaaSkillUsage {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_u8(self.value())
	}
}

impl<'de> Deserialize<'de> for MaaSkillUsage {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		Self::new(u8::deserialize(deserializer)?).map_err(D::Error::custom)
	}
}

/// Selected timeout profile: `-1` waits indefinitely, `0` skips immediately, positive values
/// wait for that many milliseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MaaJobTimeout {
	Unlimited,
	Milliseconds(u32),
}

impl MaaJobTimeout {
	#[must_use]
	pub const fn is_branching(self) -> bool {
		matches!(self, Self::Milliseconds(_))
	}
}

impl Serialize for MaaJobTimeout {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		match self {
			Self::Unlimited => serializer.serialize_i64(-1),
			Self::Milliseconds(value) => serializer.serialize_u32(*value),
		}
	}
}

impl<'de> Deserialize<'de> for MaaJobTimeout {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let value: i64 = i64::deserialize(deserializer)?;
		match value {
			-1 => Ok(Self::Unlimited),
			0.. => u32::try_from(value)
				.map(Self::Milliseconds)
				.map_err(|_| D::Error::custom("timeout exceeds u32 milliseconds")),
			_ => Err(D::Error::custom(
				"timeout must be -1 or a non-negative millisecond value",
			)),
		}
	}
}

/// Optional guards, delays, and documentation attached to one action.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaaActionConditions {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub doc: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub doc_color: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub costs: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub cost_changes: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub kills: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub cooling: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub pre_delay: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub post_delay: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub time_elapsed: Option<i32>,
}

impl MaaActionConditions {
	#[must_use]
	pub fn doc(&self) -> Option<&str> {
		self.doc.as_deref()
	}

	#[must_use]
	pub fn doc_color(&self) -> Option<&str> {
		self.doc_color.as_deref()
	}

	#[must_use]
	pub const fn costs(&self) -> Option<i32> {
		self.costs
	}

	#[must_use]
	pub const fn cost_changes(&self) -> Option<i32> {
		self.cost_changes
	}

	#[must_use]
	pub const fn kills(&self) -> Option<i32> {
		self.kills
	}

	#[must_use]
	pub const fn cooling(&self) -> Option<i32> {
		self.cooling
	}

	#[must_use]
	pub const fn pre_delay(&self) -> Option<i32> {
		self.pre_delay
	}

	#[must_use]
	pub const fn post_delay(&self) -> Option<i32> {
		self.post_delay
	}

	#[must_use]
	pub const fn time_elapsed(&self) -> Option<i32> {
		self.time_elapsed
	}
}

/// Exact operator constraints supported by the selected MAA profile.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaaJobRequirements {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub elite: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub level: Option<u16>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub module: Option<i8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub module_level: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub potential: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub skill_level: Option<u8>,
}

impl MaaJobRequirements {
	#[must_use]
	pub const fn elite(&self) -> Option<u8> {
		self.elite
	}

	#[must_use]
	pub const fn level(&self) -> Option<u16> {
		self.level
	}

	#[must_use]
	pub const fn module(&self) -> Option<i8> {
		self.module
	}

	#[must_use]
	pub const fn module_level(&self) -> Option<u8> {
		self.module_level
	}

	#[must_use]
	pub const fn potential(&self) -> Option<u8> {
		self.potential
	}

	#[must_use]
	pub const fn skill_level(&self) -> Option<u8> {
		self.skill_level
	}
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaaJobOperator {
	pub name: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub skill: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub skill_usage: Option<MaaSkillUsage>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub skill_times: Option<u32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub requirements: Option<MaaJobRequirements>,
}

impl MaaJobOperator {
	#[must_use]
	pub fn name(&self) -> &str {
		&self.name
	}

	#[must_use]
	pub const fn skill(&self) -> Option<u8> {
		self.skill
	}

	#[must_use]
	pub const fn skill_usage(&self) -> Option<u8> {
		match self.skill_usage {
			Some(value) => Some(value.value()),
			None => None,
		}
	}

	#[must_use]
	pub const fn skill_times(&self) -> Option<u32> {
		self.skill_times
	}

	#[must_use]
	pub const fn requirements(&self) -> Option<&MaaJobRequirements> {
		self.requirements.as_ref()
	}
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaaJobGroup {
	pub name: String,
	#[serde(default)]
	pub opers: Vec<MaaJobOperator>,
}

impl MaaJobGroup {
	#[must_use]
	pub fn name(&self) -> &str {
		&self.name
	}

	#[must_use]
	pub fn operators(&self) -> &[MaaJobOperator] {
		&self.opers
	}
}

#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaaJobDocument {
	#[serde(default)]
	pub title: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub title_color: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub details: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub details_color: Option<String>,
}

/// The sole serialized MAA job file representation used for both import and export.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaaJob {
	pub minimum_required: String,
	pub stage_name: String,
	pub actions: Vec<MaaJobAction>,
	#[serde(default)]
	pub doc: MaaJobDocument,
	#[serde(default)]
	pub groups: Vec<MaaJobGroup>,
	#[serde(default)]
	pub opers: Vec<MaaJobOperator>,
	#[serde(default)]
	pub difficulty: MaaJobDifficultyMask,
}

impl MaaJob {
	#[must_use]
	pub fn stage_name(&self) -> &str {
		&self.stage_name
	}

	#[must_use]
	pub fn minimum_required(&self) -> &str {
		&self.minimum_required
	}

	#[must_use]
	pub fn title(&self) -> &str {
		&self.doc.title
	}

	#[must_use]
	pub fn details(&self) -> Option<&str> {
		self.doc.details.as_deref()
	}

	#[must_use]
	pub fn actions(&self) -> &[MaaJobAction] {
		&self.actions
	}

	#[must_use]
	pub fn groups(&self) -> &[MaaJobGroup] {
		&self.groups
	}

	#[must_use]
	pub fn fixed_operators(&self) -> &[MaaJobOperator] {
		&self.opers
	}
}

/// Supported MAA actions. Common conditions are flattened into each action object.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum MaaJobAction {
	/// The selected ark-solver profile accepts `timeout` on Deploy because MaaCore applies the
	/// readiness timeout while resolving deployment actions, despite the protocol prose listing
	/// the field under Skill. Exported earliest-ready jobs intentionally omit it.
	Deploy {
		name: String,
		location: [i32; 2],
		direction: MaaJobDirection,
		#[serde(skip_serializing_if = "Option::is_none")]
		timeout: Option<MaaJobTimeout>,
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	Skill {
		#[serde(skip_serializing_if = "Option::is_none")]
		name: Option<String>,
		#[serde(skip_serializing_if = "Option::is_none")]
		location: Option<[i32; 2]>,
		#[serde(skip_serializing_if = "Option::is_none")]
		timeout: Option<MaaJobTimeout>,
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	Retreat {
		#[serde(skip_serializing_if = "Option::is_none")]
		name: Option<String>,
		#[serde(skip_serializing_if = "Option::is_none")]
		location: Option<[i32; 2]>,
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	SpeedUp {
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	BulletTime {
		#[serde(skip_serializing_if = "Option::is_none")]
		name: Option<String>,
		#[serde(skip_serializing_if = "Option::is_none")]
		location: Option<[i32; 2]>,
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	SkillUsage {
		name: String,
		skill_usage: MaaSkillUsage,
		#[serde(skip_serializing_if = "Option::is_none")]
		skill_times: Option<u32>,
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	Output {
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	SkillDaemon {
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	MoveCamera {
		distance: [f64; 2],
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
	ResetStopwatch {
		#[serde(flatten)]
		conditions: MaaActionConditions,
	},
}

impl MaaJobAction {
	#[must_use]
	pub const fn conditions(&self) -> &MaaActionConditions {
		match self {
			Self::Deploy { conditions, .. }
			| Self::Skill { conditions, .. }
			| Self::Retreat { conditions, .. }
			| Self::SpeedUp { conditions }
			| Self::BulletTime { conditions, .. }
			| Self::SkillUsage { conditions, .. }
			| Self::Output { conditions }
			| Self::SkillDaemon { conditions }
			| Self::MoveCamera { conditions, .. }
			| Self::ResetStopwatch { conditions } => conditions,
		}
	}

	#[must_use]
	pub const fn timeout(&self) -> Option<MaaJobTimeout> {
		match self {
			Self::Deploy { timeout, .. } | Self::Skill { timeout, .. } => *timeout,
			_ => None,
		}
	}

	#[must_use]
	pub fn target_name(&self) -> Option<&str> {
		match self {
			Self::Deploy { name, .. } | Self::SkillUsage { name, .. } => Some(name),
			Self::Skill { name, .. }
			| Self::Retreat { name, .. }
			| Self::BulletTime { name, .. } => name.as_deref(),
			_ => None,
		}
	}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaaJobActionAccumulator {
	#[serde(rename = "type")]
	action_type: Option<String>,
	name: Option<String>,
	location: Option<[i32; 2]>,
	direction: Option<MaaJobDirection>,
	skill_usage: Option<MaaSkillUsage>,
	skill_times: Option<u32>,
	distance: Option<[f64; 2]>,
	timeout: Option<MaaJobTimeout>,
	skip_if_not_ready: Option<bool>,
	rear_delay: Option<i32>,
	doc: Option<String>,
	doc_color: Option<String>,
	costs: Option<i32>,
	cost_changes: Option<i32>,
	kills: Option<i32>,
	cooling: Option<i32>,
	pre_delay: Option<i32>,
	post_delay: Option<i32>,
	time_elapsed: Option<i32>,
}

impl MaaJobActionAccumulator {
	fn conditions(&self) -> MaaActionConditions {
		MaaActionConditions {
			doc: self.doc.clone(),
			doc_color: self.doc_color.clone(),
			costs: self.costs,
			cost_changes: self.cost_changes,
			kills: self.kills,
			cooling: self.cooling,
			pre_delay: self.pre_delay,
			post_delay: self.post_delay,
			time_elapsed: self.time_elapsed,
		}
	}

	fn normalized_timeout<E>(&self, skill_compat: bool) -> Result<Option<MaaJobTimeout>, E>
	where
		E: serde::de::Error,
	{
		if self.rear_delay.is_some() {
			return Err(E::custom(
				"rear_delay is ambiguous; use the canonical post_delay field",
			));
		}
		match (self.timeout, self.skip_if_not_ready) {
			(Some(_), Some(_)) => Err(E::custom(
				"timeout conflicts with deprecated skip_if_not_ready",
			)),
			(Some(timeout), None) => Ok(Some(timeout)),
			(None, Some(value)) if skill_compat => Ok(Some(if value {
				MaaJobTimeout::Milliseconds(0)
			} else {
				MaaJobTimeout::Unlimited
			})),
			(None, Some(_)) => Err(E::custom(
				"deprecated skip_if_not_ready is accepted only on Skill actions",
			)),
			(None, None) => Ok(None),
		}
	}

	fn reject_fields<E>(&self, fields: &[(&'static str, bool)]) -> Result<(), E>
	where
		E: serde::de::Error,
	{
		if let Some((field, _)) = fields.iter().find(|(_, present)| *present) {
			Err(E::custom(format!(
				"field {field} is not valid for this MAA job file action"
			)))
		} else {
			Ok(())
		}
	}
}

impl<'de> Deserialize<'de> for MaaJobAction {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let action: MaaJobActionAccumulator = MaaJobActionAccumulator::deserialize(deserializer)?;
		let action_type: &str = action.action_type.as_deref().unwrap_or("Deploy");
		let conditions: MaaActionConditions = action.conditions();
		let malformed = |message: &str| D::Error::custom(message.to_owned());
		match action_type {
			"Deploy" | "部署" => {
				action.reject_fields::<D::Error>(&[
					("skill_usage", action.skill_usage.is_some()),
					("skill_times", action.skill_times.is_some()),
					("distance", action.distance.is_some()),
				])?;
				let timeout: Option<MaaJobTimeout> =
					action.normalized_timeout::<D::Error>(false)?;
				Ok(Self::Deploy {
					name: action
						.name
						.filter(|name: &String| !name.trim().is_empty())
						.ok_or_else(|| malformed("Deploy requires a non-blank name"))?,
					location: action
						.location
						.ok_or_else(|| malformed("Deploy requires location [x, y]"))?,
					direction: action
						.direction
						.ok_or_else(|| malformed("Deploy requires direction"))?,
					timeout,
					conditions,
				})
			}
			"Skill" | "技能" => {
				action.reject_fields::<D::Error>(&[
					("direction", action.direction.is_some()),
					("skill_usage", action.skill_usage.is_some()),
					("skill_times", action.skill_times.is_some()),
					("distance", action.distance.is_some()),
				])?;
				let timeout: Option<MaaJobTimeout> = action.normalized_timeout::<D::Error>(true)?;
				let name: Option<String> = action.name.filter(|name| !name.trim().is_empty());
				if name.is_none() && action.location.is_none() {
					return Err(malformed("Skill requires a name or location"));
				}
				Ok(Self::Skill {
					name,
					location: action.location,
					timeout,
					conditions,
				})
			}
			"Retreat" | "撤退" | "BulletTime" | "子弹时间" => {
				action.reject_fields::<D::Error>(&[
					("direction", action.direction.is_some()),
					("skill_usage", action.skill_usage.is_some()),
					("skill_times", action.skill_times.is_some()),
					("distance", action.distance.is_some()),
					("timeout", action.timeout.is_some()),
				])?;
				action.normalized_timeout::<D::Error>(false)?;
				let name: Option<String> = action.name.filter(|name| !name.trim().is_empty());
				if name.is_none() && action.location.is_none() {
					return Err(malformed("Retreat/BulletTime requires a name or location"));
				}
				if matches!(action_type, "Retreat" | "撤退") {
					Ok(Self::Retreat {
						name,
						location: action.location,
						conditions,
					})
				} else {
					Ok(Self::BulletTime {
						name,
						location: action.location,
						conditions,
					})
				}
			}
			"SkillUsage" | "技能用法" => {
				action.reject_fields::<D::Error>(&[
					("location", action.location.is_some()),
					("direction", action.direction.is_some()),
					("distance", action.distance.is_some()),
					("timeout", action.timeout.is_some()),
				])?;
				action.normalized_timeout::<D::Error>(false)?;
				Ok(Self::SkillUsage {
					name: action
						.name
						.filter(|name| !name.trim().is_empty())
						.ok_or_else(|| malformed("SkillUsage requires a non-blank name"))?,
					skill_usage: action
						.skill_usage
						.ok_or_else(|| malformed("SkillUsage requires skill_usage"))?,
					skill_times: action.skill_times,
					conditions,
				})
			}
			"MoveCamera" | "移动镜头" => {
				action.reject_fields::<D::Error>(&[
					("name", action.name.is_some()),
					("location", action.location.is_some()),
					("direction", action.direction.is_some()),
					("skill_usage", action.skill_usage.is_some()),
					("skill_times", action.skill_times.is_some()),
					("timeout", action.timeout.is_some()),
				])?;
				action.normalized_timeout::<D::Error>(false)?;
				Ok(Self::MoveCamera {
					distance: action
						.distance
						.ok_or_else(|| malformed("MoveCamera requires distance [x, y]"))?,
					conditions,
				})
			}
			"SpeedUp"
			| "二倍速"
			| "Output"
			| "打印"
			| "SkillDaemon"
			| "摆完挂机"
			| "ResetStopwatch"
			| "重置全局计时器"
			| "重置计时器" => {
				action.reject_fields::<D::Error>(&[
					("name", action.name.is_some()),
					("location", action.location.is_some()),
					("direction", action.direction.is_some()),
					("skill_usage", action.skill_usage.is_some()),
					("skill_times", action.skill_times.is_some()),
					("distance", action.distance.is_some()),
					("timeout", action.timeout.is_some()),
				])?;
				action.normalized_timeout::<D::Error>(false)?;
				match action_type {
					"SpeedUp" | "二倍速" => Ok(Self::SpeedUp { conditions }),
					"Output" | "打印" => Ok(Self::Output { conditions }),
					"SkillDaemon" | "摆完挂机" => Ok(Self::SkillDaemon { conditions }),
					_ => Ok(Self::ResetStopwatch { conditions }),
				}
			}
			_ => Err(D::Error::custom(format!(
				"unsupported MAA job file action type {action_type:?}"
			))),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::MaaJobDifficulty;

	#[test]
	fn difficulty_serializes_with_stable_snake_case_spelling() {
		assert_eq!(
			serde_json::to_string(&MaaJobDifficulty::Unspecified)
				.expect("difficulty should serialize"),
			"\"unspecified\""
		);
		assert_eq!(
			serde_json::to_string(&MaaJobDifficulty::Normal).expect("difficulty should serialize"),
			"\"normal\""
		);
		assert_eq!(
			serde_json::to_string(&MaaJobDifficulty::Challenge)
				.expect("difficulty should serialize"),
			"\"challenge\""
		);
	}
}
