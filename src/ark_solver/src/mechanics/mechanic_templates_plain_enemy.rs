//! Plain Enemy Prefab Template: a Mechanic Template over one whole pinned enemy battle prefab.
//!
//! The template admits exactly two reviewed forms of an enemy normal attack, a melee single-target
//! hit and a ranged single-target projectile, and rejects every other prefab as drift.
//!
//! **What is proven from serialized evidence.** The prefab is found by its exact container path in
//! exactly one enemy pack, at container fileID 0. Every MonoBehaviour carries one of the exact
//! `(script_id, old_type_hash)` pairs below and receives exactly one role, and roles are assigned
//! only by following serialized PPtr wiring from the root controller, never by hierarchy name.
//! Every GameObject is active and every MonoBehaviour enabled. Every serialized key of every role
//! is either Unity metadata, a pinned value, a reviewed edge, or an unbound presentation key, and
//! the key set must match exactly. Gameplay and timing values are pinned to the reviewed
//! observations. The interpreter's issue multiset must equal the set this template explains, and
//! the attack's damage family, apply way, range and block demand must agree with the tables.
//!
//! **What is a reviewed classification.** Script class names are absent from the export, so what
//! each schema *does* cannot be proven. The attack, trigger, target-filter, range-extension,
//! projectile and movement schemas are bound by the values observed on the reviewed records rather
//! than by name. The Graphic, FaceSwitcher, Spine, Shadow, BoneFollower and both effect schemas are
//! classified as presentation: their keys are fully covered, their timing-affecting values pinned,
//! their references close over this prefab (or, for the Spine skeleton asset only, point outside
//! the snapshot), and every other value is deliberately unbound.
//!
//! **Review notes.** The key and pin tables were generated with jq from the probe of the eight
//! enemy prefabs and both projectile prefabs at mechanics revision `824157d2…`, with the splash
//! contrast record excluded from constancy. Against design rules R9–R17:
//! - `_delayAfterReached` is serialized as integer `0` on the paracurve schema and as float `0.0`
//!   on the linear schema, so each movement schema carries its own literal.
//! - `_inputTargetEffects` is `[]` on every applicable record; its only observed entry shape belongs
//!   to the splash record, so it is pinned empty rather than reviewed by entry keys.
//! - `_damageType` 2 is observed only on the ranged schema; the melee schema accepts `{1, 2}` only
//!   because the table cross-check decides the family.
//! - Undocumented integer enums (`_attackType`, `_postFilter`, `_targetSide`, `_abnormalFlag`, …)
//!   are bound by observed value.
//! - GameObject activity and the Unity type of transform targets live in serialized objects the
//!   graph does not retain, so `decode_plain_enemy_prefab` checks them against the snapshot before
//!   the pure graph seam runs.
//!
//! The template decodes and reports the form, the projectile and its serialized speed; how a
//! projectile's flight is executed is replay's concern.

use std::{error::Error, fmt};

use crate::data::{
	BattlePrefabEntryKind, BattlePrefabManifestEntry, BattlePrefabObject, BattlePrefabSnapshot,
};
use serde_json::Value;

use crate::mechanics::{
	battle_prefab_handlers::{ATTACK_ABILITY_OLD_TYPE_HASH, ATTACK_ABILITY_SCRIPT_ID},
	battle_prefab_interpreter::{
		find_battle_prefab_containers, interpret_battle_prefab_asset, BattlePrefabAttackAbility,
		BattlePrefabBuffSpec, BattlePrefabContainerMatch, BattlePrefabGameObject,
		BattlePrefabGraph, BattlePrefabHandlerRegistry, BattlePrefabInterpretError,
		BattlePrefabMonoBehaviour, BattlePrefabObjectReference, BattlePrefabObjectSource,
		BattlePrefabRootSection, BattlePrefabSemantic, BattlePrefabSemanticIssue,
		BattlePrefabSemanticProvenance,
	},
	undescribed_enemy_catalog::ReviewedEnemyDamageType,
};

/// The table facts an enemy record must agree with, after stage overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct PlainEnemyTableFacts {
	pub prefab_key: String,
	pub apply_way: Option<String>,
	pub range_radius: Option<f64>,
	pub block_demand: u16,
	pub damage: ReviewedEnemyDamageType,
}

/// Result of consulting the pinned battle-prefab snapshot for one enemy record.
#[derive(Clone, Debug, PartialEq)]
pub enum PlainEnemyPrefabDecode {
	/// The snapshot pins no enemy pack, so no enemy prefab evidence exists to decode.
	NoEnemyPacks,
	Decoded(Box<DecodedPlainEnemyPrefab>),
}

/// One enemy prefab admitted as a reviewed plain form, with the evidence it consumed.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedPlainEnemyPrefab {
	pub location: BattlePrefabContainerMatch,
	pub form: PlainEnemyAttackForm,
	pub damage: ReviewedEnemyDamageType,
	/// One entry per role-labelled MonoBehaviour, the projectile's included.
	pub provenance: Vec<BattlePrefabSemanticProvenance>,
	/// Every interpreter issue this template explains, the projectile's included.
	pub consumed_issues: Vec<BattlePrefabSemanticIssue>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlainEnemyAttackForm {
	Melee,
	RangedProjectile {
		projectile_key: String,
		projectile: BattlePrefabContainerMatch,
		movement: ProjectileMovementSchema,
		/// Serialized projectile speed; decoded, not executed.
		speed: f64,
	},
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectileMovementSchema {
	Paracurve,
	Linear,
}

/// A fail-closed rejection naming the rule, the object, the field, and what was expected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlainEnemyPrefabDecodeError {
	EnemyPrefabNotFound {
		asset_path: String,
	},
	AmbiguousEnemyPrefab {
		asset_path: String,
		matches: Vec<BattlePrefabContainerMatch>,
	},
	EnemyPrefabOutsideEnemypack {
		location: Box<BattlePrefabContainerMatch>,
	},
	CrossCabContainer {
		location: Box<BattlePrefabContainerMatch>,
	},
	LocationMismatch {
		rule: &'static str,
		expected: String,
		found: String,
	},
	Interpretation(Box<BattlePrefabInterpretError>),
	RevisionMismatch {
		expected: String,
		found: String,
	},
	Issues {
		missing: Vec<BattlePrefabSemanticIssue>,
		unexpected: Vec<BattlePrefabSemanticIssue>,
	},
	UnexpectedFacts {
		source: Box<BattlePrefabObjectSource>,
		expected: String,
		found: String,
	},
	UnassignedBehaviour {
		source: Box<BattlePrefabObjectSource>,
		schema: String,
	},
	DuplicateRole {
		source: Box<BattlePrefabObjectSource>,
		first: &'static str,
		second: &'static str,
	},
	OwnedSetMismatch {
		game_object: Box<BattlePrefabObjectSource>,
		expected: String,
		found: String,
	},
	UnexpectedReference {
		source: Box<BattlePrefabObjectSource>,
		field: String,
		expected: String,
		found: String,
	},
	UnclassifiedField {
		role: &'static str,
		source: Box<BattlePrefabObjectSource>,
		field: String,
	},
	MissingField {
		role: &'static str,
		source: Box<BattlePrefabObjectSource>,
		field: String,
	},
	InactiveGameObject {
		source: Box<BattlePrefabObjectSource>,
		found: String,
	},
	DisabledBehaviour {
		source: Box<BattlePrefabObjectSource>,
		found: String,
	},
	UnsupportedModeWiring {
		source: Box<BattlePrefabObjectSource>,
		combat: String,
		attack: String,
		attack_trigger: String,
	},
	FieldDrift {
		rule: &'static str,
		role: &'static str,
		source: Box<BattlePrefabObjectSource>,
		field: String,
		expected: String,
		found: String,
	},
	TableDisagreement {
		check: &'static str,
		table: String,
		prefab: String,
	},
	ProjectileNotFound {
		asset_path: String,
	},
	AmbiguousProjectile {
		asset_path: String,
		matches: Vec<BattlePrefabContainerMatch>,
	},
	ProjectileOutsideProjectileBundle {
		location: Box<BattlePrefabContainerMatch>,
	},
	Projectile {
		projectile_key: String,
		error: Box<PlainEnemyPrefabDecodeError>,
	},
}

impl PlainEnemyPrefabDecodeError {
	/// The design rule this rejection enforces.
	#[must_use]
	pub const fn rule(&self) -> &'static str {
		match self {
			Self::EnemyPrefabNotFound { .. }
			| Self::AmbiguousEnemyPrefab { .. }
			| Self::EnemyPrefabOutsideEnemypack { .. }
			| Self::CrossCabContainer { .. } => "R1 locate",
			Self::LocationMismatch { rule, .. } | Self::FieldDrift { rule, .. } => rule,
			Self::Interpretation(_) => "R2 interpret",
			Self::RevisionMismatch { .. } => "R2 mechanics revision",
			Self::Issues { .. } => "R3 issue set",
			Self::UnexpectedFacts { .. } => "R4 fact set",
			Self::UnassignedBehaviour { .. }
			| Self::DuplicateRole { .. }
			| Self::OwnedSetMismatch { .. } => "R5 role partition",
			Self::UnexpectedReference { .. } => "R6 reviewed edges",
			Self::UnclassifiedField { .. } | Self::MissingField { .. } => "R7 field coverage",
			Self::InactiveGameObject { .. } | Self::DisabledBehaviour { .. } => {
				"R8 active and enabled"
			}
			Self::UnsupportedModeWiring { .. } => "R11 mode wiring",
			Self::TableDisagreement { .. } => "R18 table cross-check",
			Self::ProjectileNotFound { .. }
			| Self::AmbiguousProjectile { .. }
			| Self::ProjectileOutsideProjectileBundle { .. } => "R16 projectile locate",
			Self::Projectile { .. } => "R16 projectile",
		}
	}
}

impl fmt::Display for PlainEnemyPrefabDecodeError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "plain enemy prefab {}: ", self.rule())?;
		match self {
			Self::EnemyPrefabNotFound { asset_path } => {
				write!(formatter, "no container entry equals {asset_path:?}")
			}
			Self::AmbiguousEnemyPrefab {
				asset_path,
				matches,
			}
			| Self::AmbiguousProjectile {
				asset_path,
				matches,
			} => write!(
				formatter,
				"{asset_path:?} expected exactly one container entry, found {}: {}",
				matches.len(),
				matches
					.iter()
					.map(location_label)
					.collect::<Vec<String>>()
					.join("; ")
			),
			Self::EnemyPrefabOutsideEnemypack { location } => write!(
				formatter,
				"expected an Enemypack bundle, found {}",
				location_label(location)
			),
			Self::CrossCabContainer { location } => write!(
				formatter,
				"expected container asset fileID 0, found {}",
				location_label(location)
			),
			Self::LocationMismatch {
				expected, found, ..
			} => write!(formatter, "expected {expected}, found {found}"),
			Self::Interpretation(error) => write!(formatter, "{error}"),
			Self::RevisionMismatch { expected, found } => write!(
				formatter,
				"expected mechanics revision {expected:?}, found {found:?}"
			),
			Self::Issues {
				missing,
				unexpected,
			} => write!(
				formatter,
				"missing issues {missing:?}; unexpected issues {unexpected:?}"
			),
			Self::UnexpectedFacts {
				source,
				expected,
				found,
			} => write!(
				formatter,
				"{} semantic facts expected {expected}, found {found}",
				source_label(source)
			),
			Self::UnassignedBehaviour { source, schema } => write!(
				formatter,
				"MonoBehaviour {} with schema {schema} is reached by no reviewed role",
				source_label(source)
			),
			Self::DuplicateRole {
				source,
				first,
				second,
			} => write!(
				formatter,
				"MonoBehaviour {} is wired as both {first} and {second}",
				source_label(source)
			),
			Self::OwnedSetMismatch {
				game_object,
				expected,
				found,
			} => write!(
				formatter,
				"GameObject {} expected to own exactly {expected}, found {found}",
				source_label(game_object)
			),
			Self::UnexpectedReference {
				source,
				field,
				expected,
				found,
			} => write!(
				formatter,
				"{} field {field} expected {expected}, found {found}",
				source_label(source)
			),
			Self::UnclassifiedField {
				role,
				source,
				field,
			} => write!(
				formatter,
				"{role} {} has unreviewed field {field:?}",
				source_label(source)
			),
			Self::MissingField {
				role,
				source,
				field,
			} => write!(
				formatter,
				"{role} {} is missing reviewed field {field:?}",
				source_label(source)
			),
			Self::InactiveGameObject { source, found } => write!(
				formatter,
				"GameObject {} field m_IsActive expected true, found {found}",
				source_label(source)
			),
			Self::DisabledBehaviour { source, found } => write!(
				formatter,
				"MonoBehaviour {} field m_Enabled expected 1, found {found}",
				source_label(source)
			),
			Self::UnsupportedModeWiring {
				source,
				combat,
				attack,
				attack_trigger,
			} => write!(
				formatter,
				"mode {} expected melee wiring (_combat at the melee attack, null _attack and \
				 _attackTrigger) or ranged single-target wiring (_combat and _attack at the \
				 projectile attack, _attackTrigger at the trigger), found _combat {combat}, \
				 _attack {attack}, _attackTrigger {attack_trigger}",
				source_label(source)
			),
			Self::FieldDrift {
				role,
				source,
				field,
				expected,
				found,
				..
			} => write!(
				formatter,
				"{role} {} field {field} expected {expected}, found {found}",
				source_label(source)
			),
			Self::TableDisagreement {
				check,
				table,
				prefab,
			} => write!(formatter, "{check}: table has {table}, prefab has {prefab}"),
			Self::ProjectileNotFound { asset_path } => {
				write!(formatter, "no container entry equals {asset_path:?}")
			}
			Self::ProjectileOutsideProjectileBundle { location } => write!(
				formatter,
				"expected the Common bundle {PROJECTILE_BUNDLE:?}, found {}",
				location_label(location)
			),
			Self::Projectile {
				projectile_key,
				error,
			} => write!(formatter, "projectile {projectile_key:?}: {error}"),
		}
	}
}

impl Error for PlainEnemyPrefabDecodeError {}

/// Consults the pinned snapshot for one enemy record and decodes it when enemy packs are pinned.
///
/// Returns `NoEnemyPacks` only when the manifest pins no enemy pack at all. Every other locate,
/// interpretation, structure, pin or table failure is a precise error.
pub fn decode_plain_enemy_prefab(
	prefabs: &BattlePrefabSnapshot,
	registry: &BattlePrefabHandlerRegistry,
	facts: &PlainEnemyTableFacts,
) -> Result<PlainEnemyPrefabDecode, PlainEnemyPrefabDecodeError> {
	if !pins_enemy_packs(prefabs.entries()) {
		return Ok(PlainEnemyPrefabDecode::NoEnemyPacks);
	}
	let asset_path: String = format!("dyn/battle/prefabs/enemies/{}.prefab", facts.prefab_key);
	let matches: Vec<BattlePrefabContainerMatch> =
		find_battle_prefab_containers(prefabs, &asset_path).map_err(interpretation)?;
	let location: BattlePrefabContainerMatch =
		single_container(LocatedPrefab::Enemy, asset_path.clone(), matches)?;
	check_enemy_location(&location)?;
	let graph: BattlePrefabGraph =
		interpret_located(prefabs, registry, &location, &asset_path, &facts.prefab_key)?;
	check_revision(prefabs.mechanics_revision(), &graph)?;
	let lookup = |source: &BattlePrefabObjectSource| snapshot_object(prefabs, source);
	check_serialized_objects(&graph, &lookup)?;
	let projectile = |projectile_key: &str| -> LocatedProjectile {
		let asset_path: String =
			format!("dyn/battle/prefabs/{PROJECTILE_BUNDLE}/{projectile_key}.prefab");
		let matches: Vec<BattlePrefabContainerMatch> =
			find_battle_prefab_containers(prefabs, &asset_path).map_err(interpretation)?;
		let location: BattlePrefabContainerMatch =
			single_container(LocatedPrefab::Projectile, asset_path.clone(), matches)?;
		check_projectile_location(&location)?;
		let graph: BattlePrefabGraph =
			interpret_located(prefabs, registry, &location, &asset_path, projectile_key)?;
		check_serialized_objects(&graph, &lookup)?;
		Ok((location, graph))
	};
	decode_plain_enemy_graphs(&location, &graph, &projectile, facts)
		.map(|decoded: DecodedPlainEnemyPrefab| PlainEnemyPrefabDecode::Decoded(Box::new(decoded)))
}

/// Decodes an already interpreted enemy graph; `projectile` locates and interprets a projectile.
///
/// This is the pure seam: the caller guarantees GameObject activity and transform typing
/// (see `decode_plain_enemy_prefab`), and every other rule is checked here.
pub fn decode_plain_enemy_graphs(
	location: &BattlePrefabContainerMatch,
	graph: &BattlePrefabGraph,
	projectile: &dyn Fn(&str) -> LocatedProjectile,
	facts: &PlainEnemyTableFacts,
) -> Result<DecodedPlainEnemyPrefab, PlainEnemyPrefabDecodeError> {
	check_enemy_location(location)?;
	check_root(location, graph, &facts.prefab_key, "R2 enemy prefab root")?;
	check_behaviours_enabled(graph)?;
	let layout: EnemyLayout = partition_enemy(graph)?;
	check_field_coverage(graph, &layout.roles)?;
	check_edges(graph, &layout.roles)?;
	let mut consumed_issues: Vec<BattlePrefabSemanticIssue> =
		validate_exact_issues(expected_issues(graph, &layout.roles), &graph.issues)?;
	let melee_attack: Option<usize> = (layout.form == WiredForm::Melee).then_some(layout.attack);
	check_semantic_facts(graph, melee_attack)?;
	check_pins(graph, &layout.roles)?;
	let damage: ReviewedEnemyDamageType =
		prefab_damage(graph, layout.attack, layout.attack_role())?;
	let mut provenance: Vec<BattlePrefabSemanticProvenance> = role_provenance(graph, &layout.roles);
	let form: PlainEnemyAttackForm = match layout.form {
		WiredForm::Melee => PlainEnemyAttackForm::Melee,
		WiredForm::Ranged => {
			let projectile_key: String =
				string_field(graph, layout.attack, Role::RangedAttack, "_projectileKey")?;
			let decoded: DecodedProjectile = projectile(&projectile_key)
				.and_then(|(projectile_location, projectile_graph)| {
					decode_projectile(
						graph,
						&projectile_key,
						projectile_location,
						&projectile_graph,
					)
				})
				.map_err(|error: PlainEnemyPrefabDecodeError| {
					PlainEnemyPrefabDecodeError::Projectile {
						projectile_key: projectile_key.clone(),
						error: Box::new(error),
					}
				})?;
			consumed_issues.extend(decoded.issues);
			provenance.extend(decoded.provenance);
			decoded.form
		}
	};
	let block_volume: i64 =
		integer_field(graph, layout.controller, Role::Controller, "_blockVolume")?;
	cross_check_tables(facts, &form, damage, block_volume)?;
	Ok(DecodedPlainEnemyPrefab {
		location: location.clone(),
		form,
		damage,
		provenance,
		consumed_issues,
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocatedPrefab {
	Enemy,
	Projectile,
}

/// R1/R16: a container path must name exactly one entry across the pinned bundles.
fn single_container(
	kind: LocatedPrefab,
	asset_path: String,
	matches: Vec<BattlePrefabContainerMatch>,
) -> Result<BattlePrefabContainerMatch, PlainEnemyPrefabDecodeError> {
	let mut matches: Vec<BattlePrefabContainerMatch> = matches;
	match (matches.len(), kind) {
		(1, _) => Ok(matches.remove(0)),
		(0, LocatedPrefab::Enemy) => {
			Err(PlainEnemyPrefabDecodeError::EnemyPrefabNotFound { asset_path })
		}
		(0, LocatedPrefab::Projectile) => {
			Err(PlainEnemyPrefabDecodeError::ProjectileNotFound { asset_path })
		}
		(_, LocatedPrefab::Enemy) => Err(PlainEnemyPrefabDecodeError::AmbiguousEnemyPrefab {
			asset_path,
			matches,
		}),
		(_, LocatedPrefab::Projectile) => Err(PlainEnemyPrefabDecodeError::AmbiguousProjectile {
			asset_path,
			matches,
		}),
	}
}

/// R2/R16: a graph was interpreted at the expected mechanics revision.
fn check_revision(
	expected: &str,
	graph: &BattlePrefabGraph,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	if graph.mechanics_revision == expected {
		return Ok(());
	}
	Err(PlainEnemyPrefabDecodeError::RevisionMismatch {
		expected: expected.to_owned(),
		found: graph.mechanics_revision.clone(),
	})
}

/// Only a manifest with no enemy pack at all lacks enemy prefab evidence.
fn pins_enemy_packs(entries: &[BattlePrefabManifestEntry]) -> bool {
	entries.iter().any(|entry: &BattlePrefabManifestEntry| {
		entry.bundle_kind == BattlePrefabEntryKind::Enemypack
	})
}

/// R8 for GameObjects and R6 transform typing: evidence the graph names but does not retain.
fn check_serialized_objects<'a>(
	graph: &BattlePrefabGraph,
	lookup: &dyn Fn(&BattlePrefabObjectSource) -> Option<&'a BattlePrefabObject>,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	for game_object in &graph.game_objects {
		let active: Option<&Value> = lookup(&game_object.source)
			.and_then(|object: &BattlePrefabObject| object.fields.get("m_IsActive"));
		if active != Some(&Value::Bool(true)) {
			return Err(PlainEnemyPrefabDecodeError::InactiveGameObject {
				source: Box::new(game_object.source.clone()),
				found: active
					.map_or_else(|| "no serialized m_IsActive".to_owned(), Value::to_string),
			});
		}
	}
	for behaviour in &graph.mono_behaviours {
		for reference in &behaviour.references {
			if behaviour_index(graph, &reference.target).is_some() {
				continue;
			}
			let unity_type: Option<&str> = lookup(&reference.target)
				.map(|object: &BattlePrefabObject| object.unity_type.as_str());
			if unity_type != Some("Transform") {
				return Err(PlainEnemyPrefabDecodeError::UnexpectedReference {
					source: Box::new(behaviour.source.clone()),
					field: reference.field_path.clone(),
					expected: "a Transform".to_owned(),
					found: format!(
						"{} {}",
						unity_type.unwrap_or("no serialized object"),
						source_label(&reference.target)
					),
				});
			}
		}
	}
	Ok(())
}

fn interpretation(error: BattlePrefabInterpretError) -> PlainEnemyPrefabDecodeError {
	PlainEnemyPrefabDecodeError::Interpretation(Box::new(error))
}

fn interpret_located(
	prefabs: &BattlePrefabSnapshot,
	registry: &BattlePrefabHandlerRegistry,
	location: &BattlePrefabContainerMatch,
	asset_path: &str,
	prefab_name: &str,
) -> Result<BattlePrefabGraph, PlainEnemyPrefabDecodeError> {
	interpret_battle_prefab_asset(
		prefabs,
		&location.logical_path,
		asset_path,
		BattlePrefabRootSection::MainAsset,
		prefab_name,
		registry,
	)
	.map_err(interpretation)
}

fn snapshot_object<'p>(
	prefabs: &'p BattlePrefabSnapshot,
	source: &BattlePrefabObjectSource,
) -> Option<&'p BattlePrefabObject> {
	prefabs
		.object(&source.cab_name, 0, source.path_id)
		.ok()
		.map(|handle| handle.object())
}

/// A located and interpreted projectile prefab, as the graph seam's projectile resolver returns it.
type LocatedProjectile =
	Result<(BattlePrefabContainerMatch, BattlePrefabGraph), PlainEnemyPrefabDecodeError>;

const PROJECTILE_BUNDLE: &str = "[uc]projectiles";
const METADATA_KEYS: [&str; 4] = ["m_Enabled", "m_GameObject", "m_Name", "m_Script"];

fn source_label(source: &BattlePrefabObjectSource) -> String {
	format!("({}, pathID {})", source.cab_name, source.path_id)
}

fn location_label(location: &BattlePrefabContainerMatch) -> String {
	format!(
		"bundle {:?} ({:?}) container fileID {} root {}",
		location.logical_path,
		location.bundle_kind,
		location.container_file_id,
		source_label(&location.root)
	)
}

fn hex(bytes: &[u8; 16]) -> String {
	bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const fn hex_digit(digit: u8) -> u8 {
	match digit {
		b'0'..=b'9' => digit - b'0',
		b'a'..=b'f' => digit - b'a' + 10,
		_ => panic!("a script hash is lowercase hexadecimal"),
	}
}

/// Parses a reviewed 32-digit hexadecimal script ID or old type hash at compile time.
const fn script_hash(digits: &str) -> [u8; 16] {
	let bytes: &[u8] = digits.as_bytes();
	assert!(bytes.len() == 32, "a script hash has 32 hexadecimal digits");
	let mut output: [u8; 16] = [0; 16];
	let mut index: usize = 0;
	while index < 16 {
		output[index] = hex_digit(bytes[2 * index]) * 16 + hex_digit(bytes[2 * index + 1]);
		index += 1;
	}
	output
}

/// Every role a reviewed MonoBehaviour can hold, in the order errors and provenance report them.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Role {
	Controller,
	Steering,
	Mode,
	MeleeAttack,
	RangedAttack,
	AttackEffects,
	Trigger,
	TargetFilter,
	RangeExtension,
	Graphic,
	FaceSwitcher,
	Spine,
	Shadow,
	BoneFollower,
	Projectile,
	ProjectileEffects,
	MoveParacurve,
	MoveLinear,
}

/// How a reviewed value is bound.
#[derive(Clone, Copy)]
enum Pinned {
	/// Equal to one compact JSON literal, compared as `serde_json::Value` (integer ≠ float).
	Exact(&'static str),
	OneOf(&'static [&'static str]),
	NullReference,
	Satisfies {
		expected: &'static str,
		check: fn(&Value) -> bool,
	},
}

impl Pinned {
	fn accepts(self, found: &Value) -> bool {
		match self {
			Self::Exact(literal) => reviewed_literal(literal) == *found,
			Self::OneOf(literals) => literals
				.iter()
				.any(|literal: &&str| reviewed_literal(literal) == *found),
			Self::NullReference => is_null_reference(found),
			Self::Satisfies { check, .. } => check(found),
		}
	}

	fn expected(self) -> String {
		match self {
			Self::Exact(literal) => literal.to_owned(),
			Self::OneOf(literals) => format!("one of {}", literals.join(", ")),
			Self::NullReference => "a null PPtr".to_owned(),
			Self::Satisfies { expected, .. } => expected.to_owned(),
		}
	}
}

/// What a reviewed PPtr field may point at.
#[derive(Clone, Copy)]
enum Edge {
	Behaviour {
		roles: &'static [Role],
		nullable: bool,
	},
	Transform {
		nullable: bool,
	},
	/// The single reviewed field that points into a CAB the snapshot does not pin.
	OutsideSnapshot,
}

impl Edge {
	fn expected(self) -> String {
		match self {
			Self::Behaviour { roles, nullable } => {
				let names: Vec<&str> = roles.iter().map(|role: &Role| schema(*role).name).collect();
				let target: String = format!("a local PPtr to {}", names.join(" or "));
				if nullable {
					format!("{target}, or null")
				} else {
					target
				}
			}
			Self::Transform { nullable: true } => "a local in-graph Transform, or null".to_owned(),
			Self::Transform { nullable: false } => "a local in-graph Transform".to_owned(),
			Self::OutsideSnapshot => "a PPtr into a CAB outside the verified snapshot".to_owned(),
		}
	}
}

/// One reviewed script schema: identity, full key set, pins and edges.
struct ReviewedSchema {
	role: Role,
	name: &'static str,
	script_id: [u8; 16],
	old_type_hash: [u8; 16],
	keys: &'static [&'static str],
	pin_rule: &'static str,
	pins: &'static [&'static [(&'static str, Pinned)]],
	edges: &'static [(&'static str, Edge)],
}

fn reviewed_literal(literal: &str) -> Value {
	serde_json::from_str(literal).expect("a reviewed pin literal is compact JSON")
}

fn is_null_reference(value: &Value) -> bool {
	value.as_object().is_some_and(|reference| {
		reference.len() == 2
			&& reference.get("m_FileID").and_then(Value::as_i64) == Some(0)
			&& reference.get("m_PathID").and_then(Value::as_i64) == Some(0)
	})
}

fn is_non_empty_string(value: &Value) -> bool {
	value.as_str().is_some_and(|text: &str| !text.is_empty())
}

fn is_finite_positive(value: &Value) -> bool {
	value
		.as_f64()
		.is_some_and(|number: f64| number.is_finite() && number > 0.0)
}

const CAST_EFFECT_ENTRY_KEYS: &[&str] = &[
	"_delayIfFirstAttack",
	"_directionType",
	"_effects",
	"_endEvent",
	"_hookEffectAfterFirstAttack",
	"_oneshot",
	"_onlyOnce",
	"_startEvent",
	"_useFourDirectionalFace",
];

const HIT_EFFECT_ENTRY_KEYS: &[&str] = &[
	"_directionType",
	"_effects",
	"_endEvent",
	"_oneshot",
	"_onlyOnce",
	"_playIfNotInputTarget",
	"_playOnlyIfInputTarget",
	"_useFourDirectionalFace",
	"_useOwnerToTargetDirection",
];

fn is_effect_list(value: &Value, entry_keys: &[&str]) -> bool {
	value.as_array().is_some_and(|entries: &Vec<Value>| {
		entries.iter().all(|entry: &Value| {
			entry.as_object().is_some_and(|fields| {
				fields.len() == entry_keys.len()
					&& entry_keys
						.iter()
						.all(|key: &&str| fields.contains_key(*key))
					&& fields
						.get("_effects")
						.and_then(Value::as_array)
						.is_some_and(|effects: &Vec<Value>| effects.iter().all(Value::is_string))
			})
		})
	})
}

fn is_cast_effect_list(value: &Value) -> bool {
	is_effect_list(value, CAST_EFFECT_ENTRY_KEYS)
}

fn is_hit_effect_list(value: &Value) -> bool {
	is_effect_list(value, HIT_EFFECT_ENTRY_KEYS)
}

fn has_one_attack_animation_at_normal_speed(value: &Value) -> bool {
	let Some(entries) = value.as_array() else {
		return false;
	};
	let attacks: Vec<&Value> = entries
		.iter()
		.filter(|entry: &&Value| entry.get("animKey").and_then(Value::as_str) == Some("Attack"))
		.collect();
	matches!(
		attacks.as_slice(),
		[attack] if attack.get("speed").and_then(Value::as_f64) == Some(1.0)
	)
}

/// Rebuilds the fact the built-in attack-ability handler must have emitted from the same fields.
fn attack_ability_from_raw(fields: &Value) -> Option<BattlePrefabAttackAbility> {
	let selector: &Value = fields.get("_selector")?;
	Some(BattlePrefabAttackAbility {
		target_selector: BattlePrefabObjectReference {
			file_id: i32::try_from(selector.get("m_FileID")?.as_i64()?).ok()?,
			path_id: selector.get("m_PathID")?.as_i64()?,
		},
		attack_scale: fields.get("_atkScale")?.as_f64()?,
		attack_scale_key: fields.get("_atkScaleKey")?.as_str()?.to_owned(),
		attack_type: fields.get("_attackType")?.as_i64()?,
		damage_type: fields.get("_damageType")?.as_i64()?,
		element_damage_type: fields.get("_elementDamageType")?.as_i64()?,
		extra_damage_type: fields.get("_extraDamageType")?.as_i64()?,
		select_target_source: fields.get("_selectTargetSource")?.as_i64()?,
		select_target_timing: fields.get("_selectTargetTiming")?.as_i64()?,
	})
}

const CONTROLLER_NULL_PINS: &[(&str, Pinned)] = &[
	("_locateRange", Pinned::NullReference),
	("_uiMountPoint", Pinned::NullReference),
];
const MODE_NULL_PINS: &[(&str, Pinned)] = &[
	("_animatorHooker", Pinned::NullReference),
	("_rangeToShow", Pinned::NullReference),
];
/// The splash selector is the first attack pin, so a splash variant names it before anything else.
const ATTACK_SELECTOR_PINS: &[(&str, Pinned)] = &[("_selector", Pinned::NullReference)];
const ATTACK_SET_PINS: &[(&str, Pinned)] = &[
	("_damageType", Pinned::OneOf(&["1", "2"])),
	(
		"_maxAnimScale",
		Pinned::OneOf(&["1.0", "1.100000023841858"]),
	),
];
const PROJECTILE_KEY_PINS: &[(&str, Pinned)] = &[(
	"_projectileKey",
	Pinned::Satisfies {
		expected: "a non-empty string",
		check: is_non_empty_string,
	},
)];
const ATTACK_EFFECTS_LIST_PINS: &[(&str, Pinned)] = &[
	(
		"_castEffects",
		Pinned::Satisfies {
			expected: "cast-effect entries with the reviewed keys and string _effects",
			check: is_cast_effect_list,
		},
	),
	(
		"_hitEffects",
		Pinned::Satisfies {
			expected: "hit-effect entries with the reviewed keys and string _effects",
			check: is_hit_effect_list,
		},
	),
];
const GRAPHIC_ANIMATION_PINS: &[(&str, Pinned)] = &[(
	"_animations",
	Pinned::Satisfies {
		expected: "exactly one Attack animation, at speed 1.0",
		check: has_one_attack_animation_at_normal_speed,
	},
)];
const SHADOW_NULL_PINS: &[(&str, Pinned)] = &[
	("_boneToFollow", Pinned::NullReference),
	("_faceSwitcher", Pinned::NullReference),
	("_graphic", Pinned::NullReference),
	("_sprite", Pinned::NullReference),
];
const PROJECTILE_EXTRA_PINS: &[(&str, Pinned)] = &[
	("_bodyTransform", Pinned::NullReference),
	(
		"_lifeTime",
		Pinned::Satisfies {
			expected: "a finite number > 0",
			check: is_finite_positive,
		},
	),
];
const MOVE_SPEED_PINS: &[(&str, Pinned)] = &[(
	"_speed",
	Pinned::Satisfies {
		expected: "a finite number > 0",
		check: is_finite_positive,
	},
)];

const CONTROLLER_EDGES: &[(&str, Edge)] = &[
	(
		"$._modes[0]",
		Edge::Behaviour {
			roles: &[Role::Mode],
			nullable: false,
		},
	),
	(
		"$._animator",
		Edge::Behaviour {
			roles: &[Role::Graphic],
			nullable: false,
		},
	),
	(
		"$._shadow",
		Edge::Behaviour {
			roles: &[Role::Shadow],
			nullable: false,
		},
	),
	("$._effectTransform", Edge::Transform { nullable: true }),
];
const MODE_EDGES: &[(&str, Edge)] = &[
	(
		"$._combat",
		Edge::Behaviour {
			roles: &[Role::MeleeAttack, Role::RangedAttack],
			nullable: false,
		},
	),
	(
		"$._attack",
		Edge::Behaviour {
			roles: &[Role::RangedAttack],
			nullable: true,
		},
	),
	(
		"$._attackTrigger",
		Edge::Behaviour {
			roles: &[Role::Trigger],
			nullable: true,
		},
	),
	("$._bodyTransform", Edge::Transform { nullable: false }),
];
const TRANSFORM_OR_NULL: Edge = Edge::Transform { nullable: true };
const GRAPHIC_EDGES: &[(&str, Edge)] = &[
	(
		"$._faceSwitcher",
		Edge::Behaviour {
			roles: &[Role::FaceSwitcher],
			nullable: false,
		},
	),
	(
		"$._skeleton",
		Edge::Behaviour {
			roles: &[Role::Spine],
			nullable: false,
		},
	),
	("$._headTransform", TRANSFORM_OR_NULL),
	("$._hitTransform", TRANSFORM_OR_NULL),
	("$._muzzleTransform", TRANSFORM_OR_NULL),
	("$._specialPoint0", TRANSFORM_OR_NULL),
	("$._specialPoint1", TRANSFORM_OR_NULL),
	("$._specialPoint2", TRANSFORM_OR_NULL),
	("$._specialPoint3", TRANSFORM_OR_NULL),
	("$._specialPoint4", TRANSFORM_OR_NULL),
	("$._specialPoint5", TRANSFORM_OR_NULL),
	("$._specialPoint6", TRANSFORM_OR_NULL),
	("$._specialPoint7", TRANSFORM_OR_NULL),
	("$._specialPoint8", TRANSFORM_OR_NULL),
	("$._specialPoint9", TRANSFORM_OR_NULL),
	("$._specialPoint10", TRANSFORM_OR_NULL),
	("$._specialPoint11", TRANSFORM_OR_NULL),
	("$._specialPoint12", TRANSFORM_OR_NULL),
	("$._specialPoint13", TRANSFORM_OR_NULL),
	("$._specialPoint14", TRANSFORM_OR_NULL),
	("$._specialPoint15", TRANSFORM_OR_NULL),
];
const SPINE_EDGES: &[(&str, Edge)] = &[("$.skeletonDataAsset", Edge::OutsideSnapshot)];
const BONE_FOLLOWER_EDGES: &[(&str, Edge)] = &[(
	"$.skeletonRenderer",
	Edge::Behaviour {
		roles: &[Role::Spine],
		nullable: false,
	},
)];
const NO_EDGES: &[(&str, Edge)] = &[];

const PRESENTATION_RULE: &str = "R12 presentation timing pins";
const ATTACK_RULE: &str = "R13 attack pins";
const TRIGGER_RULE: &str = "R15 ranged trigger pins";
const PROJECTILE_RULE: &str = "R17 projectile pins";

/// Indexed by `Role as usize`.
const SCHEMAS: [ReviewedSchema; 18] = [
	ReviewedSchema {
		role: Role::Controller,
		name: "CONTROLLER",
		script_id: CONTROLLER_SCRIPT_ID,
		old_type_hash: CONTROLLER_OLD_TYPE_HASH,
		keys: CONTROLLER_KEYS,
		pin_rule: "R9 controller pins",
		pins: &[CONTROLLER_OBSERVED_PINS, CONTROLLER_NULL_PINS],
		edges: CONTROLLER_EDGES,
	},
	ReviewedSchema {
		role: Role::Steering,
		name: "STEERING",
		script_id: STEERING_SCRIPT_ID,
		old_type_hash: STEERING_OLD_TYPE_HASH,
		keys: STEERING_KEYS,
		pin_rule: "R10 steering pins",
		pins: &[STEERING_OBSERVED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::Mode,
		name: "MODE",
		script_id: MODE_SCRIPT_ID,
		old_type_hash: MODE_OLD_TYPE_HASH,
		keys: MODE_KEYS,
		pin_rule: "R11 mode pins",
		pins: &[MODE_OBSERVED_PINS, MODE_NULL_PINS],
		edges: MODE_EDGES,
	},
	ReviewedSchema {
		role: Role::MeleeAttack,
		name: "MELEE_ATTACK",
		script_id: MELEE_ATTACK_SCRIPT_ID,
		old_type_hash: MELEE_ATTACK_OLD_TYPE_HASH,
		keys: MELEE_ATTACK_KEYS,
		pin_rule: ATTACK_RULE,
		pins: &[
			ATTACK_SELECTOR_PINS,
			MELEE_ATTACK_OBSERVED_PINS,
			ATTACK_SET_PINS,
		],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::RangedAttack,
		name: "RANGED_ATTACK",
		script_id: RANGED_ATTACK_SCRIPT_ID,
		old_type_hash: RANGED_ATTACK_OLD_TYPE_HASH,
		keys: RANGED_ATTACK_KEYS,
		pin_rule: "R13/R14 projectile attack pins",
		pins: &[
			ATTACK_SELECTOR_PINS,
			RANGED_ATTACK_OBSERVED_PINS,
			ATTACK_SET_PINS,
			PROJECTILE_KEY_PINS,
		],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::AttackEffects,
		name: "ATTACK_EFFECTS",
		script_id: ATTACK_EFFECTS_SCRIPT_ID,
		old_type_hash: ATTACK_EFFECTS_OLD_TYPE_HASH,
		keys: ATTACK_EFFECTS_KEYS,
		pin_rule: PRESENTATION_RULE,
		pins: &[ATTACK_EFFECTS_OBSERVED_PINS, ATTACK_EFFECTS_LIST_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::Trigger,
		name: "TRIGGER",
		script_id: TRIGGER_SCRIPT_ID,
		old_type_hash: TRIGGER_OLD_TYPE_HASH,
		keys: TRIGGER_KEYS,
		pin_rule: TRIGGER_RULE,
		pins: &[TRIGGER_OBSERVED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::TargetFilter,
		name: "TARGET_FILTER",
		script_id: TARGET_FILTER_SCRIPT_ID,
		old_type_hash: TARGET_FILTER_OLD_TYPE_HASH,
		keys: TARGET_FILTER_KEYS,
		pin_rule: TRIGGER_RULE,
		pins: &[TARGET_FILTER_OBSERVED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::RangeExtension,
		name: "RANGE_EXTENSION",
		script_id: RANGE_EXTENSION_SCRIPT_ID,
		old_type_hash: RANGE_EXTENSION_OLD_TYPE_HASH,
		keys: RANGE_EXTENSION_KEYS,
		pin_rule: TRIGGER_RULE,
		pins: &[RANGE_EXTENSION_OBSERVED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::Graphic,
		name: "GRAPHIC",
		script_id: GRAPHIC_SCRIPT_ID,
		old_type_hash: GRAPHIC_OLD_TYPE_HASH,
		keys: GRAPHIC_KEYS,
		pin_rule: PRESENTATION_RULE,
		pins: &[GRAPHIC_OBSERVED_PINS, GRAPHIC_ANIMATION_PINS],
		edges: GRAPHIC_EDGES,
	},
	ReviewedSchema {
		role: Role::FaceSwitcher,
		name: "FACE_SWITCHER",
		script_id: FACE_SWITCHER_SCRIPT_ID,
		old_type_hash: FACE_SWITCHER_OLD_TYPE_HASH,
		keys: FACE_SWITCHER_KEYS,
		pin_rule: PRESENTATION_RULE,
		pins: &[FACE_SWITCHER_OBSERVED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::Spine,
		name: "SPINE",
		script_id: SPINE_SCRIPT_ID,
		old_type_hash: SPINE_OLD_TYPE_HASH,
		keys: SPINE_KEYS,
		pin_rule: PRESENTATION_RULE,
		pins: &[SPINE_OBSERVED_PINS],
		edges: SPINE_EDGES,
	},
	ReviewedSchema {
		role: Role::Shadow,
		name: "SHADOW",
		script_id: SHADOW_SCRIPT_ID,
		old_type_hash: SHADOW_OLD_TYPE_HASH,
		keys: SHADOW_KEYS,
		pin_rule: "R6 reviewed null edges",
		pins: &[SHADOW_OBSERVED_PINS, SHADOW_NULL_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::BoneFollower,
		name: "BONE_FOLLOWER",
		script_id: BONE_FOLLOWER_SCRIPT_ID,
		old_type_hash: BONE_FOLLOWER_OLD_TYPE_HASH,
		keys: BONE_FOLLOWER_KEYS,
		pin_rule: PRESENTATION_RULE,
		pins: &[BONE_FOLLOWER_OBSERVED_PINS],
		edges: BONE_FOLLOWER_EDGES,
	},
	ReviewedSchema {
		role: Role::Projectile,
		name: "PROJECTILE",
		script_id: PROJECTILE_SCRIPT_ID,
		old_type_hash: PROJECTILE_OLD_TYPE_HASH,
		keys: PROJECTILE_KEYS,
		pin_rule: PROJECTILE_RULE,
		pins: &[PROJECTILE_OBSERVED_PINS, PROJECTILE_EXTRA_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::ProjectileEffects,
		name: "PROJECTILE_EFFECTS",
		script_id: PROJECTILE_EFFECTS_SCRIPT_ID,
		old_type_hash: PROJECTILE_EFFECTS_OLD_TYPE_HASH,
		keys: PROJECTILE_EFFECTS_KEYS,
		pin_rule: PRESENTATION_RULE,
		pins: &[PROJECTILE_EFFECTS_OBSERVED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::MoveParacurve,
		name: "MOVE_PARACURVE",
		script_id: MOVE_PARACURVE_SCRIPT_ID,
		old_type_hash: MOVE_PARACURVE_OLD_TYPE_HASH,
		keys: MOVE_PARACURVE_KEYS,
		pin_rule: PROJECTILE_RULE,
		pins: &[MOVE_PARACURVE_OBSERVED_PINS, MOVE_SPEED_PINS],
		edges: NO_EDGES,
	},
	ReviewedSchema {
		role: Role::MoveLinear,
		name: "MOVE_LINEAR",
		script_id: MOVE_LINEAR_SCRIPT_ID,
		old_type_hash: MOVE_LINEAR_OLD_TYPE_HASH,
		keys: MOVE_LINEAR_KEYS,
		pin_rule: PROJECTILE_RULE,
		pins: &[MOVE_LINEAR_OBSERVED_PINS, MOVE_SPEED_PINS],
		edges: NO_EDGES,
	},
];

fn schema(role: Role) -> &'static ReviewedSchema {
	let reviewed: &'static ReviewedSchema = &SCHEMAS[role as usize];
	debug_assert_eq!(reviewed.role, role, "SCHEMAS is indexed by role");
	reviewed
}

fn has_schema(behaviour: &BattlePrefabMonoBehaviour, role: Role) -> bool {
	let reviewed: &ReviewedSchema = schema(role);
	behaviour.script_id == Some(reviewed.script_id)
		&& behaviour.old_type_hash == Some(reviewed.old_type_hash)
}

fn schema_label(behaviour: &BattlePrefabMonoBehaviour) -> String {
	let script: String = behaviour
		.script_id
		.as_ref()
		.map_or_else(|| "<no script ID>".to_owned(), hex);
	let hash: String = behaviour
		.old_type_hash
		.as_ref()
		.map_or_else(|| "<no type hash>".to_owned(), hex);
	format!("({script}, {hash})")
}

fn same_object(left: &BattlePrefabObjectSource, right: &BattlePrefabObjectSource) -> bool {
	left.path_id == right.path_id && left.cab_name.eq_ignore_ascii_case(&right.cab_name)
}

fn behaviour_index(graph: &BattlePrefabGraph, source: &BattlePrefabObjectSource) -> Option<usize> {
	graph
		.mono_behaviours
		.iter()
		.position(|behaviour: &BattlePrefabMonoBehaviour| same_object(&behaviour.source, source))
}

/// Resolves `$.field` or `$.field[index]` inside raw TypeTree fields.
fn raw_at<'v>(fields: &'v Value, field_path: &str) -> Option<&'v Value> {
	let path: &str = field_path.strip_prefix("$.")?;
	match path.split_once('[') {
		None => fields.get(path),
		Some((name, index)) => {
			let index: usize = index.strip_suffix(']')?.parse().ok()?;
			fields.get(name)?.get(index)
		}
	}
}

fn raw_field<'g>(
	graph: &'g BattlePrefabGraph,
	index: usize,
	role: Role,
	field: &str,
) -> Result<&'g Value, PlainEnemyPrefabDecodeError> {
	let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
	raw_at(&behaviour.raw_fields, &format!("$.{field}")).ok_or_else(|| {
		PlainEnemyPrefabDecodeError::MissingField {
			role: schema(role).name,
			source: Box::new(behaviour.source.clone()),
			field: field.to_owned(),
		}
	})
}

fn drift(
	graph: &BattlePrefabGraph,
	index: usize,
	role: Role,
	field: &str,
	expected: String,
	found: &Value,
) -> PlainEnemyPrefabDecodeError {
	let reviewed: &ReviewedSchema = schema(role);
	PlainEnemyPrefabDecodeError::FieldDrift {
		rule: reviewed.pin_rule,
		role: reviewed.name,
		source: Box::new(graph.mono_behaviours[index].source.clone()),
		field: field.to_owned(),
		expected,
		found: found.to_string(),
	}
}

fn integer_field(
	graph: &BattlePrefabGraph,
	index: usize,
	role: Role,
	field: &str,
) -> Result<i64, PlainEnemyPrefabDecodeError> {
	let value: &Value = raw_field(graph, index, role, field)?;
	value
		.as_i64()
		.ok_or_else(|| drift(graph, index, role, field, "an integer".to_owned(), value))
}

fn number_field(
	graph: &BattlePrefabGraph,
	index: usize,
	role: Role,
	field: &str,
) -> Result<f64, PlainEnemyPrefabDecodeError> {
	let value: &Value = raw_field(graph, index, role, field)?;
	value
		.as_f64()
		.ok_or_else(|| drift(graph, index, role, field, "a number".to_owned(), value))
}

fn string_field(
	graph: &BattlePrefabGraph,
	index: usize,
	role: Role,
	field: &str,
) -> Result<String, PlainEnemyPrefabDecodeError> {
	let value: &Value = raw_field(graph, index, role, field)?;
	value
		.as_str()
		.map(str::to_owned)
		.ok_or_else(|| drift(graph, index, role, field, "a string".to_owned(), value))
}

/// R1: an enemy prefab lives in an enemy pack and its container asset is local to the bundle.
fn check_enemy_location(
	location: &BattlePrefabContainerMatch,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	if location.bundle_kind != BattlePrefabEntryKind::Enemypack {
		return Err(PlainEnemyPrefabDecodeError::EnemyPrefabOutsideEnemypack {
			location: Box::new(location.clone()),
		});
	}
	if location.container_file_id != 0 {
		return Err(PlainEnemyPrefabDecodeError::CrossCabContainer {
			location: Box::new(location.clone()),
		});
	}
	Ok(())
}

/// R16: a projectile prefab lives in the Common projectile bundle, local to it.
fn check_projectile_location(
	location: &BattlePrefabContainerMatch,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	if location.logical_path != PROJECTILE_BUNDLE
		|| location.bundle_kind != BattlePrefabEntryKind::Common
	{
		return Err(
			PlainEnemyPrefabDecodeError::ProjectileOutsideProjectileBundle {
				location: Box::new(location.clone()),
			},
		);
	}
	if location.container_file_id != 0 {
		return Err(PlainEnemyPrefabDecodeError::LocationMismatch {
			rule: "R16 projectile locate",
			expected: "container asset fileID 0".to_owned(),
			found: location_label(location),
		});
	}
	Ok(())
}

/// R1/R2: one MainAsset root, named by the key, that is exactly the located container root.
fn check_root(
	location: &BattlePrefabContainerMatch,
	graph: &BattlePrefabGraph,
	prefab_key: &str,
	rule: &'static str,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	let mismatch =
		|expected: String, found: String| PlainEnemyPrefabDecodeError::LocationMismatch {
			rule,
			expected,
			found,
		};
	if graph.section != BattlePrefabRootSection::MainAsset {
		return Err(mismatch(
			"the MainAsset section".to_owned(),
			format!("{:?}", graph.section),
		));
	}
	if graph.prefab_name != prefab_key {
		return Err(mismatch(
			format!("prefab {prefab_key:?}"),
			format!("prefab {:?}", graph.prefab_name),
		));
	}
	let [root] = graph.root_game_object_indices.as_slice() else {
		return Err(mismatch(
			"exactly one root GameObject".to_owned(),
			format!("{} roots", graph.root_game_object_indices.len()),
		));
	};
	let root_object: &BattlePrefabGameObject = &graph.game_objects[*root];
	if root_object.name != prefab_key {
		return Err(mismatch(
			format!("root GameObject named {prefab_key:?}"),
			format!("{:?}", root_object.name),
		));
	}
	if location.logical_path != graph.logical_path || location.root != root_object.source {
		return Err(mismatch(
			format!(
				"the graph root at the located container {}",
				location_label(location)
			),
			format!(
				"root {} in bundle {:?}",
				source_label(&root_object.source),
				graph.logical_path
			),
		));
	}
	Ok(())
}

/// R8 for MonoBehaviours: `m_Enabled` is exactly 1.
fn check_behaviours_enabled(graph: &BattlePrefabGraph) -> Result<(), PlainEnemyPrefabDecodeError> {
	for behaviour in &graph.mono_behaviours {
		let enabled: Option<&Value> = behaviour.raw_fields.get("m_Enabled");
		if enabled.and_then(Value::as_i64) != Some(1) {
			return Err(PlainEnemyPrefabDecodeError::DisabledBehaviour {
				source: Box::new(behaviour.source.clone()),
				found: enabled.map_or_else(|| "no m_Enabled".to_owned(), Value::to_string),
			});
		}
	}
	Ok(())
}

/// A PPtr read from raw fields, resolved inside the graph.
enum EdgeTarget {
	Null,
	Behaviour(usize),
	/// A non-MonoBehaviour component of an in-graph GameObject.
	Component(BattlePrefabObjectSource),
	External {
		file_id: i64,
		path_id: i64,
	},
}

fn read_edge(
	graph: &BattlePrefabGraph,
	index: usize,
	role: Role,
	field_path: &str,
) -> Result<EdgeTarget, PlainEnemyPrefabDecodeError> {
	let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
	let value: &Value = raw_at(&behaviour.raw_fields, field_path).ok_or_else(|| {
		PlainEnemyPrefabDecodeError::MissingField {
			role: schema(role).name,
			source: Box::new(behaviour.source.clone()),
			field: field_path.to_owned(),
		}
	})?;
	let pptr: Option<(i64, i64)> = value
		.as_object()
		.filter(|reference| reference.len() == 2)
		.and_then(|reference| {
			Some((
				reference.get("m_FileID")?.as_i64()?,
				reference.get("m_PathID")?.as_i64()?,
			))
		});
	let Some((file_id, path_id)) = pptr else {
		return Err(PlainEnemyPrefabDecodeError::UnexpectedReference {
			source: Box::new(behaviour.source.clone()),
			field: field_path.to_owned(),
			expected: "a PPtr".to_owned(),
			found: value.to_string(),
		});
	};
	if path_id == 0 {
		return Ok(EdgeTarget::Null);
	}
	if file_id != 0 {
		return Ok(EdgeTarget::External { file_id, path_id });
	}
	let target: BattlePrefabObjectSource = BattlePrefabObjectSource {
		logical_path: behaviour.source.logical_path.clone(),
		cab_name: behaviour.source.cab_name.clone(),
		path_id,
	};
	if let Some(target_index) = behaviour_index(graph, &target) {
		return Ok(EdgeTarget::Behaviour(target_index));
	}
	graph
		.game_objects
		.iter()
		.flat_map(|game_object| game_object.component_sources.iter())
		.find(|component: &&BattlePrefabObjectSource| same_object(component, &target))
		.map(|component: &BattlePrefabObjectSource| EdgeTarget::Component(component.clone()))
		.ok_or_else(|| PlainEnemyPrefabDecodeError::UnexpectedReference {
			source: Box::new(behaviour.source.clone()),
			field: field_path.to_owned(),
			expected: "an object inside this prefab graph".to_owned(),
			found: source_label(&target),
		})
}

fn describe_target(graph: &BattlePrefabGraph, target: &EdgeTarget) -> String {
	match target {
		EdgeTarget::Null => "null".to_owned(),
		EdgeTarget::Behaviour(index) => {
			let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[*index];
			format!(
				"MonoBehaviour {} with schema {}",
				source_label(&behaviour.source),
				schema_label(behaviour)
			)
		}
		EdgeTarget::Component(source) => format!("component {}", source_label(source)),
		EdgeTarget::External { file_id, path_id } => format!("fileID {file_id} pathID {path_id}"),
	}
}

/// Follows one reviewed wiring edge that must reach a MonoBehaviour of `expected`'s schema.
fn require_behaviour(
	graph: &BattlePrefabGraph,
	index: usize,
	role: Role,
	field_path: &str,
	expected: Role,
) -> Result<usize, PlainEnemyPrefabDecodeError> {
	match read_edge(graph, index, role, field_path)? {
		EdgeTarget::Behaviour(target) if has_schema(&graph.mono_behaviours[target], expected) => {
			Ok(target)
		}
		other => Err(PlainEnemyPrefabDecodeError::UnexpectedReference {
			source: Box::new(graph.mono_behaviours[index].source.clone()),
			field: field_path.to_owned(),
			expected: format!("a local PPtr to {}", schema(expected).name),
			found: describe_target(graph, &other),
		}),
	}
}

fn owner(graph: &BattlePrefabGraph, index: usize) -> Result<usize, PlainEnemyPrefabDecodeError> {
	let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
	behaviour
		.game_object_index
		.ok_or_else(|| PlainEnemyPrefabDecodeError::UnassignedBehaviour {
			source: Box::new(behaviour.source.clone()),
			schema: format!(
				"{} outside the GameObject hierarchy",
				schema_label(behaviour)
			),
		})
}

fn role_names(roles: &[Role]) -> Vec<&'static str> {
	roles.iter().map(|role: &Role| schema(*role).name).collect()
}

/// Matches a GameObject's owned MonoBehaviours to exactly the required roles plus at most one of
/// each optional role, by exact schema.
fn claim_owned(
	graph: &BattlePrefabGraph,
	game_object: usize,
	required: &[Role],
	optional: &[Role],
) -> Result<Vec<(usize, Role)>, PlainEnemyPrefabDecodeError> {
	let owned: &[usize] = &graph.game_objects[game_object].mono_behaviour_indices;
	let mismatch = || PlainEnemyPrefabDecodeError::OwnedSetMismatch {
		game_object: Box::new(graph.game_objects[game_object].source.clone()),
		expected: format!(
			"{{{}}} plus at most one each of {{{}}}",
			role_names(required).join(", "),
			role_names(optional).join(", ")
		),
		found: owned
			.iter()
			.map(|index: &usize| schema_label(&graph.mono_behaviours[*index]))
			.collect::<Vec<String>>()
			.join(", "),
	};
	let mut claims: Vec<(usize, Role)> = Vec::with_capacity(owned.len());
	for index in owned {
		let role: Option<Role> = required
			.iter()
			.chain(optional)
			.copied()
			.find(|role: &Role| {
				has_schema(&graph.mono_behaviours[*index], *role)
					&& !claims.iter().any(|(_, claimed)| claimed == role)
			});
		match role {
			Some(role) => claims.push((*index, role)),
			None => return Err(mismatch()),
		}
	}
	if required
		.iter()
		.all(|role: &Role| claims.iter().any(|(_, claimed)| claimed == role))
	{
		Ok(claims)
	} else {
		Err(mismatch())
	}
}

fn assign_claims(
	graph: &BattlePrefabGraph,
	roles: &mut [Option<Role>],
	claims: &[(usize, Role)],
) -> Result<(), PlainEnemyPrefabDecodeError> {
	for (index, role) in claims {
		match roles[*index] {
			None => roles[*index] = Some(*role),
			Some(existing) if existing == *role => {}
			Some(existing) => {
				return Err(PlainEnemyPrefabDecodeError::DuplicateRole {
					source: Box::new(graph.mono_behaviours[*index].source.clone()),
					first: schema(existing).name,
					second: schema(*role).name,
				})
			}
		}
	}
	Ok(())
}

/// A wired role whose GameObject must own exactly that one MonoBehaviour.
fn claim_single(
	graph: &BattlePrefabGraph,
	roles: &mut [Option<Role>],
	index: usize,
	role: Role,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	let claims: Vec<(usize, Role)> = claim_owned(graph, owner(graph, index)?, &[role], &[])?;
	assign_claims(graph, roles, &claims)
}

fn claimed(claims: &[(usize, Role)], role: Role) -> usize {
	claims
		.iter()
		.find(|(_, claimed)| *claimed == role)
		.map(|(index, _)| *index)
		.expect("claim_owned returns every required role")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WiredForm {
	Melee,
	Ranged,
}

struct ModeWiring {
	form: WiredForm,
	attack: usize,
	trigger: Option<usize>,
}

/// R11 form by wiring: melee iff `_combat` reaches the melee attack with null `_attack` and
/// `_attackTrigger`; ranged iff `_combat == _attack` reach the projectile attack and
/// `_attackTrigger` reaches the trigger.
fn wire_mode(
	graph: &BattlePrefabGraph,
	mode: usize,
) -> Result<ModeWiring, PlainEnemyPrefabDecodeError> {
	let combat: EdgeTarget = read_edge(graph, mode, Role::Mode, "$._combat")?;
	let attack: EdgeTarget = read_edge(graph, mode, Role::Mode, "$._attack")?;
	let trigger: EdgeTarget = read_edge(graph, mode, Role::Mode, "$._attackTrigger")?;
	let reaches = |target: &EdgeTarget, role: Role| -> bool {
		matches!(target, EdgeTarget::Behaviour(index)
			if has_schema(&graph.mono_behaviours[*index], role))
	};
	match (&combat, &attack, &trigger) {
		(EdgeTarget::Behaviour(combat_index), EdgeTarget::Null, EdgeTarget::Null)
			if reaches(&combat, Role::MeleeAttack) =>
		{
			Ok(ModeWiring {
				form: WiredForm::Melee,
				attack: *combat_index,
				trigger: None,
			})
		}
		(
			EdgeTarget::Behaviour(combat_index),
			EdgeTarget::Behaviour(attack_index),
			EdgeTarget::Behaviour(trigger_index),
		) if combat_index == attack_index
			&& reaches(&combat, Role::RangedAttack)
			&& reaches(&trigger, Role::Trigger) =>
		{
			Ok(ModeWiring {
				form: WiredForm::Ranged,
				attack: *combat_index,
				trigger: Some(*trigger_index),
			})
		}
		_ => Err(PlainEnemyPrefabDecodeError::UnsupportedModeWiring {
			source: Box::new(graph.mono_behaviours[mode].source.clone()),
			combat: describe_target(graph, &combat),
			attack: describe_target(graph, &attack),
			attack_trigger: describe_target(graph, &trigger),
		}),
	}
}

struct EnemyLayout {
	roles: Vec<Option<Role>>,
	form: WiredForm,
	controller: usize,
	attack: usize,
}

impl EnemyLayout {
	const fn attack_role(&self) -> Role {
		match self.form {
			WiredForm::Melee => Role::MeleeAttack,
			WiredForm::Ranged => Role::RangedAttack,
		}
	}
}

/// R5: assigns every MonoBehaviour exactly one role by following wiring from the root controller.
fn partition_enemy(graph: &BattlePrefabGraph) -> Result<EnemyLayout, PlainEnemyPrefabDecodeError> {
	let mut roles: Vec<Option<Role>> = vec![None; graph.mono_behaviours.len()];
	let root_claims: Vec<(usize, Role)> = claim_owned(
		graph,
		graph.root_game_object_indices[0],
		&[Role::Controller, Role::Steering],
		&[],
	)?;
	assign_claims(graph, &mut roles, &root_claims)?;
	let controller: usize = claimed(&root_claims, Role::Controller);

	let modes: &Value = raw_field(graph, controller, Role::Controller, "_modes")?;
	if modes.as_array().map(Vec::len) != Some(1) {
		return Err(PlainEnemyPrefabDecodeError::UnexpectedReference {
			source: Box::new(graph.mono_behaviours[controller].source.clone()),
			field: "$._modes".to_owned(),
			expected: "exactly one mode reference".to_owned(),
			found: modes.to_string(),
		});
	}
	let mode: usize = require_behaviour(
		graph,
		controller,
		Role::Controller,
		"$._modes[0]",
		Role::Mode,
	)?;
	claim_single(graph, &mut roles, mode, Role::Mode)?;

	let wiring: ModeWiring = wire_mode(graph, mode)?;
	let attack_role: Role = match wiring.form {
		WiredForm::Melee => Role::MeleeAttack,
		WiredForm::Ranged => Role::RangedAttack,
	};
	let attack_claims: Vec<(usize, Role)> = claim_owned(
		graph,
		owner(graph, wiring.attack)?,
		&[attack_role],
		&[Role::AttackEffects],
	)?;
	assign_claims(graph, &mut roles, &attack_claims)?;
	if let Some(trigger) = wiring.trigger {
		let trigger_claims: Vec<(usize, Role)> = claim_owned(
			graph,
			owner(graph, trigger)?,
			&[Role::Trigger, Role::TargetFilter, Role::RangeExtension],
			&[],
		)?;
		assign_claims(graph, &mut roles, &trigger_claims)?;
	}

	let graphic: usize = require_behaviour(
		graph,
		controller,
		Role::Controller,
		"$._animator",
		Role::Graphic,
	)?;
	claim_single(graph, &mut roles, graphic, Role::Graphic)?;
	let shadow: usize = require_behaviour(
		graph,
		controller,
		Role::Controller,
		"$._shadow",
		Role::Shadow,
	)?;
	claim_single(graph, &mut roles, shadow, Role::Shadow)?;
	let face_switcher: usize = require_behaviour(
		graph,
		graphic,
		Role::Graphic,
		"$._faceSwitcher",
		Role::FaceSwitcher,
	)?;
	claim_single(graph, &mut roles, face_switcher, Role::FaceSwitcher)?;
	let spine: usize =
		require_behaviour(graph, graphic, Role::Graphic, "$._skeleton", Role::Spine)?;
	claim_single(graph, &mut roles, spine, Role::Spine)?;
	for index in 0..graph.mono_behaviours.len() {
		if !has_schema(&graph.mono_behaviours[index], Role::BoneFollower) {
			continue;
		}
		let renderer: EdgeTarget =
			read_edge(graph, index, Role::BoneFollower, "$.skeletonRenderer")?;
		if matches!(renderer, EdgeTarget::Behaviour(target) if target == spine) {
			claim_single(graph, &mut roles, index, Role::BoneFollower)?;
		}
	}
	require_every_role(graph, &roles)?;
	Ok(EnemyLayout {
		roles,
		form: wiring.form,
		controller,
		attack: wiring.attack,
	})
}

fn require_every_role(
	graph: &BattlePrefabGraph,
	roles: &[Option<Role>],
) -> Result<(), PlainEnemyPrefabDecodeError> {
	match roles.iter().position(Option::is_none) {
		None => Ok(()),
		Some(index) => {
			let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
			Err(PlainEnemyPrefabDecodeError::UnassignedBehaviour {
				source: Box::new(behaviour.source.clone()),
				schema: schema_label(behaviour),
			})
		}
	}
}

/// Every labelled behaviour, in role order and then graph order.
fn labelled(roles: &[Option<Role>]) -> Vec<(usize, Role)> {
	let mut labelled: Vec<(usize, Role)> = roles
		.iter()
		.enumerate()
		.filter_map(|(index, role)| role.map(|role: Role| (index, role)))
		.collect();
	labelled.sort_by_key(|(index, role)| (*role, *index));
	labelled
}

/// R7: each role's key set is exactly the reviewed keys plus Unity metadata.
fn check_field_coverage(
	graph: &BattlePrefabGraph,
	roles: &[Option<Role>],
) -> Result<(), PlainEnemyPrefabDecodeError> {
	for (index, role) in labelled(roles) {
		let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
		let reviewed: &ReviewedSchema = schema(role);
		let missing = |field: &str| PlainEnemyPrefabDecodeError::MissingField {
			role: reviewed.name,
			source: Box::new(behaviour.source.clone()),
			field: field.to_owned(),
		};
		let Some(fields) = behaviour.raw_fields.as_object() else {
			return Err(missing("$"));
		};
		if let Some(field) = METADATA_KEYS
			.iter()
			.chain(reviewed.keys)
			.find(|key: &&&str| !fields.contains_key(**key))
		{
			return Err(missing(field));
		}
		if let Some(field) = fields.keys().find(|key: &&String| {
			!METADATA_KEYS.contains(&key.as_str()) && !reviewed.keys.contains(&key.as_str())
		}) {
			return Err(PlainEnemyPrefabDecodeError::UnclassifiedField {
				role: reviewed.name,
				source: Box::new(behaviour.source.clone()),
				field: field.clone(),
			});
		}
	}
	Ok(())
}

fn edge_accepts(roles: &[Option<Role>], edge: Edge, target: &EdgeTarget) -> bool {
	match (edge, target) {
		(
			Edge::Behaviour { nullable: true, .. } | Edge::Transform { nullable: true },
			EdgeTarget::Null,
		) => true,
		(Edge::Behaviour { roles: allowed, .. }, EdgeTarget::Behaviour(index)) => {
			roles[*index].is_some_and(|role: Role| allowed.contains(&role))
		}
		(Edge::Transform { .. }, EdgeTarget::Component(_))
		| (Edge::OutsideSnapshot, EdgeTarget::External { .. }) => true,
		_ => false,
	}
}

/// R6: every reviewed edge reaches its reviewed kind, no other PPtr exists, and the only
/// reference outside the snapshot is each reviewed outside edge, retained exactly once.
fn check_edges(
	graph: &BattlePrefabGraph,
	roles: &[Option<Role>],
) -> Result<(), PlainEnemyPrefabDecodeError> {
	for (index, role) in labelled(roles) {
		let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
		let reviewed: &ReviewedSchema = schema(role);
		let unexpected = |field: &str, expected: String, found: String| {
			PlainEnemyPrefabDecodeError::UnexpectedReference {
				source: Box::new(behaviour.source.clone()),
				field: field.to_owned(),
				expected,
				found,
			}
		};
		let reviewed_edge = |field: &str| -> Option<Edge> {
			reviewed
				.edges
				.iter()
				.find(|(edge_field, _)| *edge_field == field)
				.map(|(_, edge)| *edge)
		};
		for (field, edge) in reviewed.edges {
			let target: EdgeTarget = read_edge(graph, index, role, field)?;
			if !edge_accepts(roles, *edge, &target) {
				return Err(unexpected(
					field,
					edge.expected(),
					describe_target(graph, &target),
				));
			}
			if matches!(edge, Edge::OutsideSnapshot) {
				let retained: usize = behaviour
					.unavailable_references
					.iter()
					.filter(|reference| reference.field_path == *field)
					.count();
				if retained != 1 {
					return Err(unexpected(
						field,
						"exactly one retained reference outside the verified snapshot".to_owned(),
						format!("{retained}"),
					));
				}
			}
		}
		for reference in &behaviour.references {
			if !matches!(
				reviewed_edge(&reference.field_path),
				Some(Edge::Behaviour { .. } | Edge::Transform { .. })
			) {
				return Err(unexpected(
					&reference.field_path,
					"no PPtr outside the reviewed edge table".to_owned(),
					format!("a reference to {}", source_label(&reference.target)),
				));
			}
		}
		for reference in &behaviour.unavailable_references {
			if !matches!(
				reviewed_edge(&reference.field_path),
				Some(Edge::OutsideSnapshot)
			) {
				return Err(unexpected(
					&reference.field_path,
					"no reference outside the verified snapshot".to_owned(),
					format!(
						"fileID {} pathID {} in {}",
						reference.file_id, reference.path_id, reference.target_cab
					),
				));
			}
		}
	}
	Ok(())
}

/// R3: every labelled behaviour without a built-in handler is an unknown script, and each
/// retained outside reference (only reviewed Spine skeleton assets survive R6) is an issue.
fn expected_issues(
	graph: &BattlePrefabGraph,
	roles: &[Option<Role>],
) -> Vec<BattlePrefabSemanticIssue> {
	let mut expected: Vec<BattlePrefabSemanticIssue> = Vec::new();
	for (index, _) in labelled(roles) {
		let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
		let handled: bool = behaviour.script_id == Some(ATTACK_ABILITY_SCRIPT_ID)
			&& behaviour.old_type_hash == Some(ATTACK_ABILITY_OLD_TYPE_HASH);
		if let (false, Some(script_id)) = (handled, behaviour.script_id) {
			expected.push(BattlePrefabSemanticIssue::UnknownScriptId {
				source: behaviour.source.clone(),
				script_id,
			});
		}
		for reference in &behaviour.unavailable_references {
			expected.push(BattlePrefabSemanticIssue::UnavailableExternalReference {
				source: behaviour.source.clone(),
				field_path: reference.field_path.clone(),
				file_id: reference.file_id,
				path_id: reference.path_id,
				target_cab: reference.target_cab.clone(),
			});
		}
	}
	expected
}

fn issue_key(issue: &BattlePrefabSemanticIssue) -> String {
	format!("{issue:?}")
}

/// R3: exact multiset equality; returns the consumed issues in a stable order.
fn validate_exact_issues(
	expected: Vec<BattlePrefabSemanticIssue>,
	found: &[BattlePrefabSemanticIssue],
) -> Result<Vec<BattlePrefabSemanticIssue>, PlainEnemyPrefabDecodeError> {
	let mut missing: Vec<BattlePrefabSemanticIssue> = expected;
	let mut unexpected: Vec<BattlePrefabSemanticIssue> = Vec::new();
	for issue in found {
		match missing.iter().position(|candidate| candidate == issue) {
			Some(position) => {
				missing.remove(position);
			}
			None => unexpected.push(issue.clone()),
		}
	}
	if missing.is_empty() && unexpected.is_empty() {
		let mut consumed: Vec<BattlePrefabSemanticIssue> = found.to_vec();
		consumed.sort_by_key(issue_key);
		return Ok(consumed);
	}
	missing.sort_by_key(issue_key);
	unexpected.sort_by_key(issue_key);
	Err(PlainEnemyPrefabDecodeError::Issues {
		missing,
		unexpected,
	})
}

/// R4: only the melee attack carries a fact, exactly the attack ability of its own fields, and no
/// MonoBehaviour carries a buff object.
fn check_semantic_facts(
	graph: &BattlePrefabGraph,
	melee_attack: Option<usize>,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	for (index, behaviour) in graph.mono_behaviours.iter().enumerate() {
		if !behaviour.buffs.is_empty() {
			return Err(PlainEnemyPrefabDecodeError::UnexpectedFacts {
				source: Box::new(behaviour.source.clone()),
				expected: "no buff objects".to_owned(),
				found: behaviour
					.buffs
					.iter()
					.map(|buff: &BattlePrefabBuffSpec| buff.field_path.as_str())
					.collect::<Vec<&str>>()
					.join(", "),
			});
		}
		let is_attack: bool = melee_attack == Some(index);
		let expected: Vec<BattlePrefabSemantic> = if is_attack {
			attack_ability_from_raw(&behaviour.raw_fields)
				.map(BattlePrefabSemantic::AttackAbility)
				.into_iter()
				.collect()
		} else {
			Vec::new()
		};
		let found: Vec<&BattlePrefabSemantic> = behaviour
			.semantic_facts
			.iter()
			.map(|fact| &fact.semantic)
			.collect();
		let foreign: bool = behaviour
			.semantic_facts
			.iter()
			.any(|fact| fact.provenance.source != behaviour.source);
		let agrees: bool = found.len() == expected.len()
			&& found
				.iter()
				.zip(&expected)
				.all(|(left, right)| *left == right);
		if !agrees || foreign || (is_attack && expected.is_empty()) {
			return Err(PlainEnemyPrefabDecodeError::UnexpectedFacts {
				source: Box::new(behaviour.source.clone()),
				expected: if is_attack {
					"exactly the attack ability of its own fields".to_owned()
				} else {
					"none".to_owned()
				},
				found: format!("{found:?}"),
			});
		}
	}
	Ok(())
}

/// R9–R15 and R17: every pin of every labelled behaviour, in role order.
fn check_pins(
	graph: &BattlePrefabGraph,
	roles: &[Option<Role>],
) -> Result<(), PlainEnemyPrefabDecodeError> {
	for (index, role) in labelled(roles) {
		for (field, pinned) in schema(role).pins.iter().flat_map(|table| table.iter()) {
			let found: &Value = raw_field(graph, index, role, field)?;
			if !pinned.accepts(found) {
				return Err(drift(graph, index, role, field, pinned.expected(), found));
			}
		}
	}
	Ok(())
}

fn prefab_damage(
	graph: &BattlePrefabGraph,
	attack: usize,
	role: Role,
) -> Result<ReviewedEnemyDamageType, PlainEnemyPrefabDecodeError> {
	match integer_field(graph, attack, role, "_damageType")? {
		1 => Ok(ReviewedEnemyDamageType::Physical),
		2 => Ok(ReviewedEnemyDamageType::Arts),
		_ => Err(drift(
			graph,
			attack,
			role,
			"_damageType",
			"1 (Physical) or 2 (Arts)".to_owned(),
			raw_field(graph, attack, role, "_damageType")?,
		)),
	}
}

fn role_provenance(
	graph: &BattlePrefabGraph,
	roles: &[Option<Role>],
) -> Vec<BattlePrefabSemanticProvenance> {
	labelled(roles)
		.into_iter()
		.filter_map(|(index, _)| {
			let behaviour: &BattlePrefabMonoBehaviour = &graph.mono_behaviours[index];
			Some(BattlePrefabSemanticProvenance {
				source: behaviour.source.clone(),
				script_id: behaviour.script_id?,
				old_type_hash: behaviour.old_type_hash?,
				mechanics_revision: graph.mechanics_revision.clone(),
			})
		})
		.collect()
}

struct DecodedProjectile {
	form: PlainEnemyAttackForm,
	issues: Vec<BattlePrefabSemanticIssue>,
	provenance: Vec<BattlePrefabSemanticProvenance>,
}

/// R16/R17: one root carrying exactly the projectile, its effects and one movement schema.
fn decode_projectile(
	enemy: &BattlePrefabGraph,
	projectile_key: &str,
	location: BattlePrefabContainerMatch,
	graph: &BattlePrefabGraph,
) -> Result<DecodedProjectile, PlainEnemyPrefabDecodeError> {
	check_projectile_location(&location)?;
	check_revision(&enemy.mechanics_revision, graph)?;
	check_root(&location, graph, projectile_key, "R16 projectile structure")?;
	let root: usize = graph.root_game_object_indices[0];
	if let Some(carrier) = graph
		.game_objects
		.iter()
		.enumerate()
		.find(|(index, game_object)| {
			*index != root && !game_object.mono_behaviour_indices.is_empty()
		})
		.map(|(_, game_object)| game_object)
	{
		return Err(PlainEnemyPrefabDecodeError::OwnedSetMismatch {
			game_object: Box::new(carrier.source.clone()),
			expected: "no MonoBehaviour below the projectile root".to_owned(),
			found: carrier
				.mono_behaviour_indices
				.iter()
				.map(|index: &usize| schema_label(&graph.mono_behaviours[*index]))
				.collect::<Vec<String>>()
				.join(", "),
		});
	}
	check_behaviours_enabled(graph)?;
	let claims: Vec<(usize, Role)> = claim_owned(
		graph,
		root,
		&[Role::Projectile, Role::ProjectileEffects],
		&[Role::MoveParacurve, Role::MoveLinear],
	)?;
	let movements: Vec<(usize, Role)> = claims
		.iter()
		.copied()
		.filter(|(_, role)| matches!(role, Role::MoveParacurve | Role::MoveLinear))
		.collect();
	let [(movement_index, movement_role)] = movements.as_slice() else {
		return Err(PlainEnemyPrefabDecodeError::OwnedSetMismatch {
			game_object: Box::new(graph.game_objects[root].source.clone()),
			expected: "exactly one of MOVE_PARACURVE or MOVE_LINEAR".to_owned(),
			found: format!("{} movement schemas", movements.len()),
		});
	};
	let mut roles: Vec<Option<Role>> = vec![None; graph.mono_behaviours.len()];
	assign_claims(graph, &mut roles, &claims)?;
	require_every_role(graph, &roles)?;
	check_field_coverage(graph, &roles)?;
	check_edges(graph, &roles)?;
	let issues: Vec<BattlePrefabSemanticIssue> =
		validate_exact_issues(expected_issues(graph, &roles), &graph.issues)?;
	check_semantic_facts(graph, None)?;
	check_pins(graph, &roles)?;
	let movement: ProjectileMovementSchema = match movement_role {
		Role::MoveParacurve => ProjectileMovementSchema::Paracurve,
		_ => ProjectileMovementSchema::Linear,
	};
	let speed: f64 = number_field(graph, *movement_index, *movement_role, "_speed")?;
	Ok(DecodedProjectile {
		form: PlainEnemyAttackForm::RangedProjectile {
			projectile_key: projectile_key.to_owned(),
			projectile: location,
			movement,
			speed,
		},
		issues,
		provenance: role_provenance(graph, &roles),
	})
}

/// R18: the table apply way, range radius, damage family and block demand agree with the prefab.
fn cross_check_tables(
	facts: &PlainEnemyTableFacts,
	form: &PlainEnemyAttackForm,
	damage: ReviewedEnemyDamageType,
	block_volume: i64,
) -> Result<(), PlainEnemyPrefabDecodeError> {
	let ranged: bool = matches!(form, PlainEnemyAttackForm::RangedProjectile { .. });
	let (apply_way, range_requirement): (&str, &str) = if ranged {
		(
			"RANGED",
			"the ranged form, which requires a finite radius > 0",
		)
	} else {
		("MELEE", "the melee form, which requires no radius or 0.0")
	};
	if facts.apply_way.as_deref() != Some(apply_way) {
		return Err(PlainEnemyPrefabDecodeError::TableDisagreement {
			check: "R18(a) apply way",
			table: format!("apply way {:?}", facts.apply_way),
			prefab: format!("a form that requires {apply_way}"),
		});
	}
	let range_agrees: bool = if ranged {
		facts
			.range_radius
			.is_some_and(|radius: f64| radius.is_finite() && radius > 0.0)
	} else {
		facts.range_radius.is_none_or(|radius: f64| radius == 0.0)
	};
	if !range_agrees {
		return Err(PlainEnemyPrefabDecodeError::TableDisagreement {
			check: "R18(b) range radius",
			table: format!("range radius {:?}", facts.range_radius),
			prefab: range_requirement.to_owned(),
		});
	}
	if facts.damage != damage {
		return Err(PlainEnemyPrefabDecodeError::TableDisagreement {
			check: "R18(c) damage family",
			table: format!("{:?}", facts.damage),
			prefab: format!("_damageType of {damage:?}"),
		});
	}
	if facts.block_demand != 1 || block_volume != 1 {
		return Err(PlainEnemyPrefabDecodeError::TableDisagreement {
			check: "R18(d) block demand",
			table: format!("block demand {}", facts.block_demand),
			prefab: format!("_blockVolume {block_volume}"),
		});
	}
	Ok(())
}

// Reviewed key tables and observed pins, generated with jq from the probe schema tables.
// The first table of every schema lists all non-metadata keys; the second, the values the design
// pins that are identical on every applicable record.

const CONTROLLER_SCRIPT_ID: [u8; 16] = script_hash("1933ff7e8a5c8203d6c79671cc9a6791");
const CONTROLLER_OLD_TYPE_HASH: [u8; 16] = script_hash("e0a963f6fbd3db540c55ee5a6e4bddb6");
const CONTROLLER_KEYS: &[&str] = &[
	"_allowFalldownWhenReborn",
	"_alwaysCheckCurrentPoint",
	"_alwaysHideHp",
	"_alwaysShowHp",
	"_animator",
	"_blockVolume",
	"_bodyDirectionPolicy",
	"_canNotExit",
	"_commonAbilities",
	"_deadEffect",
	"_defaultBodyColor",
	"_delayToBorn",
	"_disableBornTweenColor",
	"_disableRenderLocateRange",
	"_dontSetEnemyFaceByCursorWhenBorn",
	"_effectTransform",
	"_height",
	"_hideBulletSpModes",
	"_hideShadowOnRecycle",
	"_hideShadowOnReset",
	"_hudPluginReplacePairs",
	"_idleWhenBorn",
	"_isFixedRotation",
	"_keepMoveAnimScale",
	"_locateRange",
	"_modes",
	"_onlyDelayToBornOnTileStart",
	"_overrideGraphicBound",
	"_scaleMoveAnimationRange",
	"_scaleMoveBySpeed",
	"_shadow",
	"_showDebugName",
	"_showSpAsBulletMode",
	"_sideTypeIndex",
	"_specialBlockCondition",
	"_spineLikeHeightUpdateRatio",
	"_startEffect",
	"_uiMountPoint",
	"_useMoveDirAsFaceDir",
	"_useSpecificDeadAnim",
	"_useSpecificReachExitAnim",
	"_useSpineLikeHeightUpdate",
];
const CONTROLLER_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_commonAbilities", Pinned::Exact(r#"[]"#)),
	("_blockVolume", Pinned::Exact(r#"1"#)),
	("_canNotExit", Pinned::Exact(r#"0"#)),
	(
		"_specialBlockCondition",
		Pinned::Exact(r#"{"_buffKeyPairs":[],"_filterTags":[],"_type":0}"#),
	),
	("_delayToBorn", Pinned::Exact(r#"0.0"#)),
	("_onlyDelayToBornOnTileStart", Pinned::Exact(r#"0"#)),
	("_idleWhenBorn", Pinned::Exact(r#"0"#)),
	("_allowFalldownWhenReborn", Pinned::Exact(r#"0"#)),
	("_alwaysCheckCurrentPoint", Pinned::Exact(r#"0"#)),
	("_sideTypeIndex", Pinned::Exact(r#"1"#)),
	("_height", Pinned::Exact(r#"0.0"#)),
	("_isFixedRotation", Pinned::Exact(r#"0"#)),
	("_hideBulletSpModes", Pinned::Exact(r#"[]"#)),
	("_showSpAsBulletMode", Pinned::Exact(r#"0"#)),
	("_hudPluginReplacePairs", Pinned::Exact(r#"[]"#)),
	("_useMoveDirAsFaceDir", Pinned::Exact(r#"0"#)),
	("_scaleMoveBySpeed", Pinned::Exact(r#"0"#)),
	("_keepMoveAnimScale", Pinned::Exact(r#"0"#)),
	("_useSpineLikeHeightUpdate", Pinned::Exact(r#"0"#)),
];

const STEERING_SCRIPT_ID: [u8; 16] = script_hash("fb9006fb34c5a7ba2b40664173060ef7");
const STEERING_OLD_TYPE_HASH: [u8; 16] = script_hash("ba19f74e4020864cd0418564c60daca3");
const STEERING_KEYS: &[&str] = &["_halfBodyWidth", "_maxSteeringForce", "_steeringFactor"];
const STEERING_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_halfBodyWidth", Pinned::Exact(r#"0.20000000298023224"#)),
	("_maxSteeringForce", Pinned::Exact(r#"10.0"#)),
	("_steeringFactor", Pinned::Exact(r#"8.0"#)),
];

const MODE_SCRIPT_ID: [u8; 16] = script_hash("fef8a056370e18af843c5e5925ee80f8");
const MODE_OLD_TYPE_HASH: [u8; 16] = script_hash("237f52a73adae7e87e66a1372409859c");
const MODE_KEYS: &[&str] = &[
	"_addIndexToSignalId",
	"_alwaysHideHp",
	"_alwaysHideSp",
	"_animatorHooker",
	"_attack",
	"_attackTrigger",
	"_bodyTransform",
	"_combat",
	"_extraRangesToShow",
	"_generalAbilities",
	"_hideUIAttackRange",
	"_includeIndexZero",
	"_overrideDeadEffect",
	"_overrideStartEffect",
	"_rangeToShow",
	"_updateAttackRangeExtendByAttributeDirty",
];
const MODE_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_generalAbilities", Pinned::Exact(r#"[]"#)),
	("_extraRangesToShow", Pinned::Exact(r#"[]"#)),
	(
		"_updateAttackRangeExtendByAttributeDirty",
		Pinned::Exact(r#"0"#),
	),
	("_addIndexToSignalId", Pinned::Exact(r#"0"#)),
	("_includeIndexZero", Pinned::Exact(r#"0"#)),
	(
		"_alwaysHideHp",
		Pinned::Exact(r#"{"isDefined":0,"value":0}"#),
	),
	(
		"_alwaysHideSp",
		Pinned::Exact(r#"{"isDefined":0,"value":0}"#),
	),
];

const MELEE_ATTACK_SCRIPT_ID: [u8; 16] = script_hash("c1b44c3bb3490f6d64ea0cd1d609440c");
const MELEE_ATTACK_OLD_TYPE_HASH: [u8; 16] = script_hash("620b15910b23f3f2fc515105148ba9e2");
const MELEE_ATTACK_KEYS: &[&str] = &[
	"_activeBuffs",
	"_affectedBySlowDown",
	"_allowNoTarget",
	"_alwaysIncludeTarget",
	"_alwaysPutIncludeTargetFirst",
	"_animKey",
	"_atkScale",
	"_atkScaleKey",
	"_attachPassiveBuffsOnDummy",
	"_attackType",
	"_checkFaceToFirstCastTargetDownAttack",
	"_cooldown",
	"_cooldownKey",
	"_damageType",
	"_deliveryParaWhenReplaceActionNode",
	"_downAnimKey",
	"_downEndAnimKey",
	"_elementDamageType",
	"_endAnimKey",
	"_epDamageRatio",
	"_escapeTime",
	"_extraDamageType",
	"_faceToDefault",
	"_faceToFirstCastTarget",
	"_faceToFront",
	"_faceToTarget",
	"_forceAsDmgOrHealAbility",
	"_forceFaceToDirection",
	"_ignoreIfOwnerDead",
	"_ignorePalsyInterrupt",
	"_interruptAbilityOnDetach",
	"_interuptIfTargetDead",
	"_maxAnimScale",
	"_metadata",
	"_onlyPlayEndAnimWhenIsLastAbility",
	"_preDelay",
	"_resetCdStrategy",
	"_selectTargetSource",
	"_selectTargetTiming",
	"_selector",
	"_timeMode",
	"_upAnimKey",
	"_upEndAnimKey",
	"_useDynamicAttackType",
	"_waitForAttackEvent",
];
const MELEE_ATTACK_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_attackType", Pinned::Exact(r#"1"#)),
	("_useDynamicAttackType", Pinned::Exact(r#"0"#)),
	("_preDelay", Pinned::Exact(r#"0.0"#)),
	("_waitForAttackEvent", Pinned::Exact(r#"1"#)),
	("_activeBuffs", Pinned::Exact(r#"[]"#)),
	("_elementDamageType", Pinned::Exact(r#"0"#)),
	("_extraDamageType", Pinned::Exact(r#"0"#)),
	("_atkScale", Pinned::Exact(r#"1.0"#)),
	("_atkScaleKey", Pinned::Exact(r#""atk_scale""#)),
	("_selectTargetSource", Pinned::Exact(r#"2"#)),
	("_selectTargetTiming", Pinned::Exact(r#"0"#)),
	("_cooldown", Pinned::Exact(r#"0.0"#)),
	("_cooldownKey", Pinned::Exact(r#""duration""#)),
	("_allowNoTarget", Pinned::Exact(r#"0"#)),
	("_alwaysIncludeTarget", Pinned::Exact(r#"1"#)),
	("_epDamageRatio", Pinned::Exact(r#"0.0"#)),
	("_escapeTime", Pinned::Exact(r#"0.0"#)),
	("_forceAsDmgOrHealAbility", Pinned::Exact(r#"0"#)),
	("_interuptIfTargetDead", Pinned::Exact(r#"0"#)),
	("_interruptAbilityOnDetach", Pinned::Exact(r#"0"#)),
	("_ignoreIfOwnerDead", Pinned::Exact(r#"0"#)),
	("_ignorePalsyInterrupt", Pinned::Exact(r#"0"#)),
	("_attachPassiveBuffsOnDummy", Pinned::Exact(r#"1"#)),
	("_affectedBySlowDown", Pinned::Exact(r#"1"#)),
	("_resetCdStrategy", Pinned::Exact(r#"1"#)),
	("_timeMode", Pinned::Exact(r#"0"#)),
	("_deliveryParaWhenReplaceActionNode", Pinned::Exact(r#"0"#)),
	(
		"_metadata",
		Pinned::Exact(r#"{"blackboardPrefix":"","namedAsAlias":""}"#),
	),
	("_alwaysPutIncludeTargetFirst", Pinned::Exact(r#"0"#)),
	// The Graphic's Attack animation speed is pinned; these tie the attack to that animation.
	("_animKey", Pinned::Exact(r#""Attack""#)),
	("_endAnimKey", Pinned::Exact(r#""Idle""#)),
	("_upAnimKey", Pinned::Exact(r#""""#)),
	("_downAnimKey", Pinned::Exact(r#""""#)),
	("_upEndAnimKey", Pinned::Exact(r#""""#)),
	("_downEndAnimKey", Pinned::Exact(r#""""#)),
];

const RANGED_ATTACK_SCRIPT_ID: [u8; 16] = script_hash("272d087f5fc103e95f2935609b8bbf1d");
const RANGED_ATTACK_OLD_TYPE_HASH: [u8; 16] = script_hash("a70c746b9e1d817d212b0f7ca934d004");
const RANGED_ATTACK_KEYS: &[&str] = &[
	"_activeBuffs",
	"_affectedBySlowDown",
	"_allowNoTarget",
	"_alwaysIncludeTarget",
	"_animKey",
	"_atkScale",
	"_atkScaleKey",
	"_attachPassiveBuffsOnDummy",
	"_attackType",
	"_checkFaceToFirstCastTargetDownAttack",
	"_cooldown",
	"_cooldownKey",
	"_damageType",
	"_deliveryParaWhenReplaceActionNode",
	"_downAnimKey",
	"_downEndAnimKey",
	"_elementDamageType",
	"_emitToInputPosWhenTargetIsInvalid",
	"_emitToInputRootTileWhenTargetIsInvalid",
	"_endAnimKey",
	"_epDamageRatio",
	"_escapeTime",
	"_extraDamageType",
	"_extraProjectileKeys",
	"_faceToDefault",
	"_faceToFirstCastTarget",
	"_faceToFront",
	"_faceToTarget",
	"_fireAttackFinishWhenProjectileInvalid",
	"_fireCreateProjectileEvent",
	"_forceAsDmgOrHealAbility",
	"_forceFaceToDirection",
	"_ignoreIfOwnerDead",
	"_ignorePalsyInterrupt",
	"_interruptAbilityOnDetach",
	"_interuptIfTargetDead",
	"_maxAnimScale",
	"_metadata",
	"_minPostDelayWhenProjectileInvalid",
	"_mountGroup",
	"_mountPointType",
	"_notClearProjectileWhenCastEnd",
	"_onlyPlayEndAnimWhenIsLastAbility",
	"_preDelay",
	"_projectileDestination",
	"_projectileKey",
	"_resetCdStrategy",
	"_selectTargetSource",
	"_selectTargetTiming",
	"_selector",
	"_timeMode",
	"_transferSource",
	"_upAnimKey",
	"_upEndAnimKey",
	"_useCachedAtkOnly",
	"_useDynamicAttackType",
	"_useMountGroup",
	"_waitForAnimEndWhenProjectileInvalid",
	"_waitForAttackEvent",
	"_waitForProjectileInvalid",
];
const RANGED_ATTACK_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_attackType", Pinned::Exact(r#"1"#)),
	("_useDynamicAttackType", Pinned::Exact(r#"0"#)),
	("_preDelay", Pinned::Exact(r#"0.0"#)),
	("_waitForAttackEvent", Pinned::Exact(r#"1"#)),
	("_activeBuffs", Pinned::Exact(r#"[]"#)),
	("_elementDamageType", Pinned::Exact(r#"0"#)),
	("_extraDamageType", Pinned::Exact(r#"0"#)),
	("_atkScale", Pinned::Exact(r#"1.0"#)),
	("_atkScaleKey", Pinned::Exact(r#""atk_scale""#)),
	("_selectTargetSource", Pinned::Exact(r#"2"#)),
	("_selectTargetTiming", Pinned::Exact(r#"0"#)),
	("_cooldown", Pinned::Exact(r#"0.0"#)),
	("_cooldownKey", Pinned::Exact(r#""duration""#)),
	("_allowNoTarget", Pinned::Exact(r#"0"#)),
	("_alwaysIncludeTarget", Pinned::Exact(r#"1"#)),
	("_epDamageRatio", Pinned::Exact(r#"0.0"#)),
	("_escapeTime", Pinned::Exact(r#"0.0"#)),
	("_forceAsDmgOrHealAbility", Pinned::Exact(r#"0"#)),
	("_interuptIfTargetDead", Pinned::Exact(r#"0"#)),
	("_interruptAbilityOnDetach", Pinned::Exact(r#"0"#)),
	("_ignoreIfOwnerDead", Pinned::Exact(r#"0"#)),
	("_ignorePalsyInterrupt", Pinned::Exact(r#"0"#)),
	("_attachPassiveBuffsOnDummy", Pinned::Exact(r#"1"#)),
	("_affectedBySlowDown", Pinned::Exact(r#"1"#)),
	("_resetCdStrategy", Pinned::Exact(r#"1"#)),
	("_timeMode", Pinned::Exact(r#"0"#)),
	("_deliveryParaWhenReplaceActionNode", Pinned::Exact(r#"0"#)),
	(
		"_metadata",
		Pinned::Exact(r#"{"blackboardPrefix":"","namedAsAlias":""}"#),
	),
	("_extraProjectileKeys", Pinned::Exact(r#"[]"#)),
	("_projectileDestination", Pinned::Exact(r#"0"#)),
	("_waitForProjectileInvalid", Pinned::Exact(r#"0"#)),
	(
		"_fireAttackFinishWhenProjectileInvalid",
		Pinned::Exact(r#"0"#),
	),
	("_fireCreateProjectileEvent", Pinned::Exact(r#"0"#)),
	("_notClearProjectileWhenCastEnd", Pinned::Exact(r#"0"#)),
	("_emitToInputPosWhenTargetIsInvalid", Pinned::Exact(r#"0"#)),
	(
		"_emitToInputRootTileWhenTargetIsInvalid",
		Pinned::Exact(r#"0"#),
	),
	(
		"_minPostDelayWhenProjectileInvalid",
		Pinned::Exact(r#"0.0"#),
	),
	(
		"_waitForAnimEndWhenProjectileInvalid",
		Pinned::Exact(r#"0"#),
	),
	("_transferSource", Pinned::Exact(r#"0"#)),
	("_useCachedAtkOnly", Pinned::Exact(r#"0"#)),
	("_useMountGroup", Pinned::Exact(r#"0"#)),
	(
		"_mountGroup",
		Pinned::Exact(r#"{"loopType":0,"mountPoints":[]}"#),
	),
	// The Graphic's Attack animation speed is pinned; these tie the attack to that animation.
	("_animKey", Pinned::Exact(r#""Attack""#)),
	("_endAnimKey", Pinned::Exact(r#""Idle""#)),
	("_upAnimKey", Pinned::Exact(r#""""#)),
	("_downAnimKey", Pinned::Exact(r#""""#)),
	("_upEndAnimKey", Pinned::Exact(r#""""#)),
	("_downEndAnimKey", Pinned::Exact(r#""""#)),
];

const ATTACK_EFFECTS_SCRIPT_ID: [u8; 16] = script_hash("33f2cf8eeccb67cd86fa554b93e178ad");
const ATTACK_EFFECTS_OLD_TYPE_HASH: [u8; 16] = script_hash("78a80a284e4b68170bc79447219d63c3");
const ATTACK_EFFECTS_KEYS: &[&str] = &[
	"_attachEffects",
	"_castEffects",
	"_castTargetEffects",
	"_convertCastEndToAttachFinish",
	"_hitEffects",
	"_inputTargetEffects",
	"_preDelayFactor",
	"_recalculatePlaybackSpeed",
];
const ATTACK_EFFECTS_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_preDelayFactor", Pinned::Exact(r#"0.0"#)),
	("_recalculatePlaybackSpeed", Pinned::Exact(r#"0"#)),
	("_convertCastEndToAttachFinish", Pinned::Exact(r#"0"#)),
	("_attachEffects", Pinned::Exact(r#"[]"#)),
	("_castTargetEffects", Pinned::Exact(r#"[]"#)),
	("_inputTargetEffects", Pinned::Exact(r#"[]"#)),
];

const TRIGGER_SCRIPT_ID: [u8; 16] = script_hash("e79f9cdc1738eb7cf612b213533905cb");
const TRIGGER_OLD_TYPE_HASH: [u8; 16] = script_hash("95d0b5928e12e465b31b080fb4682302");
const TRIGGER_KEYS: &[&str] = &["_keepTarget", "_minTargetNum", "_overrideSearchTargetTick"];
const TRIGGER_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_keepTarget", Pinned::Exact(r#"0"#)),
	("_minTargetNum", Pinned::Exact(r#"1"#)),
	("_overrideSearchTargetTick", Pinned::Exact(r#"-1"#)),
];

const TARGET_FILTER_SCRIPT_ID: [u8; 16] = script_hash("7f215a8296c7f945d539ac5d7db1ff7a");
const TARGET_FILTER_OLD_TYPE_HASH: [u8; 16] = script_hash("0b01d694f140798a39efce8ee828e447");
const TARGET_FILTER_KEYS: &[&str] = &[
	"_abilityInputTargetAsSortSource",
	"_abnormalCombo",
	"_abnormalFlag",
	"_checkUnitType",
	"_excludeAbnormalFlag",
	"_excludeOwner",
	"_excludeSomeAbnormalFlags",
	"_forceIgnoreCamouflage",
	"_ignoreAllyTargetFree",
	"_ignoreHealFree",
	"_ignoreHitRange",
	"_ignoreTargetFree",
	"_limitTargetNum",
	"_maxNum",
	"_maxTargetKey",
	"_needProfessionMask",
	"_onlyIgnoreSomeOfTargetFreeCase",
	"_postFilter",
	"_professionMask",
	"_sortByTauntAtLast",
	"_targetCategory",
	"_targetMotion",
	"_targetSide",
	"_unitTypeMask",
];
const TARGET_FILTER_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_abilityInputTargetAsSortSource", Pinned::Exact(r#"0"#)),
	("_abnormalCombo", Pinned::Exact(r#"2"#)),
	("_abnormalFlag", Pinned::Exact(r#"44"#)),
	("_checkUnitType", Pinned::Exact(r#"0"#)),
	("_excludeAbnormalFlag", Pinned::Exact(r#"45"#)),
	("_excludeOwner", Pinned::Exact(r#"0"#)),
	("_excludeSomeAbnormalFlags", Pinned::Exact(r#"0"#)),
	("_forceIgnoreCamouflage", Pinned::Exact(r#"0"#)),
	("_ignoreAllyTargetFree", Pinned::Exact(r#"0"#)),
	("_ignoreHealFree", Pinned::Exact(r#"0"#)),
	("_ignoreHitRange", Pinned::Exact(r#"0"#)),
	("_ignoreTargetFree", Pinned::Exact(r#"0"#)),
	("_limitTargetNum", Pinned::Exact(r#"1"#)),
	("_maxNum", Pinned::Exact(r#"1"#)),
	("_maxTargetKey", Pinned::Exact(r#""max_target""#)),
	("_needProfessionMask", Pinned::Exact(r#"0"#)),
	("_onlyIgnoreSomeOfTargetFreeCase", Pinned::Exact(r#"0"#)),
	("_postFilter", Pinned::Exact(r#"4"#)),
	("_professionMask", Pinned::Exact(r#"0"#)),
	("_sortByTauntAtLast", Pinned::Exact(r#"0"#)),
	("_targetCategory", Pinned::Exact(r#"1"#)),
	("_targetMotion", Pinned::Exact(r#"1"#)),
	("_targetSide", Pinned::Exact(r#"2"#)),
	("_unitTypeMask", Pinned::Exact(r#"0"#)),
];

const RANGE_EXTENSION_SCRIPT_ID: [u8; 16] = script_hash("d0a496d1886e8b4658e7302f3f8d5963");
const RANGE_EXTENSION_OLD_TYPE_HASH: [u8; 16] = script_hash("d2f2894b10c99b7d58ac7015f861c2ad");
const RANGE_EXTENSION_KEYS: &[&str] = &["_extendable", "_scaleble"];
const RANGE_EXTENSION_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_extendable", Pinned::Exact(r#"0"#)),
	("_scaleble", Pinned::Exact(r#"0"#)),
];

const GRAPHIC_SCRIPT_ID: [u8; 16] = script_hash("a18f5c3cda61bf9fd6796f13fea2b161");
const GRAPHIC_OLD_TYPE_HASH: [u8; 16] = script_hash("b3d577e254d054fdf11e8f9f91d6afe3");
const GRAPHIC_KEYS: &[&str] = &[
	"_animations",
	"_cacheBoundsCenter",
	"_enableShowDamageFlash",
	"_faceSwitcher",
	"_headTransform",
	"_hitTransform",
	"_mixSettings",
	"_muzzleTransform",
	"_neverImmediatelyFaceSwitcher",
	"_resetSkinToInitial",
	"_skeleton",
	"_specialPoint0",
	"_specialPoint1",
	"_specialPoint10",
	"_specialPoint11",
	"_specialPoint12",
	"_specialPoint13",
	"_specialPoint14",
	"_specialPoint15",
	"_specialPoint2",
	"_specialPoint3",
	"_specialPoint4",
	"_specialPoint5",
	"_specialPoint6",
	"_specialPoint7",
	"_specialPoint8",
	"_specialPoint9",
	"_spineScale",
	"_spineSkins",
];
const GRAPHIC_OBSERVED_PINS: &[(&str, Pinned)] = &[];

const FACE_SWITCHER_SCRIPT_ID: [u8; 16] = script_hash("e93140dafde43ede0e2d9f64f68c9ab8");
const FACE_SWITCHER_OLD_TYPE_HASH: [u8; 16] = script_hash("12ef59bd4b354194ed3ce30e151da633");
const FACE_SWITCHER_KEYS: &[&str] = &["_defaultLOrR", "_switchTime"];
const FACE_SWITCHER_OBSERVED_PINS: &[(&str, Pinned)] = &[];

const SPINE_SCRIPT_ID: [u8; 16] = script_hash("f876ea348585b6913552bc77fdf5df5a");
const SPINE_OLD_TYPE_HASH: [u8; 16] = script_hash("972ca33b6c5ad13e80352c77f75ab487");
const SPINE_KEYS: &[&str] = &[
	"_animationName",
	"addNormals",
	"calculateTangents",
	"clearStateOnDisable",
	"disableRenderingOnOverride",
	"fixDrawOrder",
	"immutableTriangles",
	"initialFlipX",
	"initialFlipY",
	"initialSkinName",
	"loop",
	"maskInteraction",
	"maskMaterials",
	"pmaVertexColors",
	"separatorSlotNames",
	"singleSubmesh",
	"skeletonDataAsset",
	"timeScale",
	"tintBlack",
	"updateWhenInvisible",
	"useClipping",
	"zSpacing",
];
const SPINE_OBSERVED_PINS: &[(&str, Pinned)] = &[("timeScale", Pinned::Exact(r#"1.0"#))];

const SHADOW_SCRIPT_ID: [u8; 16] = script_hash("f15baa441aefa1c939957e2ffcf5f68e");
const SHADOW_OLD_TYPE_HASH: [u8; 16] = script_hash("0f3619b6903859660e6f1097aa16fdc5");
const SHADOW_KEYS: &[&str] = &[
	"_alwaysUseSpineOffset",
	"_attachOffset",
	"_autoAttachToGround",
	"_boneToFollow",
	"_easeType",
	"_enableAdvanced",
	"_faceSwitcher",
	"_graphic",
	"_simpleFollow",
	"_simplyUseTileHeight",
	"_specialInit",
	"_sprite",
];
const SHADOW_OBSERVED_PINS: &[(&str, Pinned)] = &[];

const BONE_FOLLOWER_SCRIPT_ID: [u8; 16] = script_hash("c4574e89845e6881464da7b69eae50f8");
const BONE_FOLLOWER_OLD_TYPE_HASH: [u8; 16] = script_hash("a9346cf944d24fef2d63e7680649099d");
const BONE_FOLLOWER_KEYS: &[&str] = &[
	"boneName",
	"followBoneRotation",
	"followLocalScale",
	"followSkeletonFlip",
	"followXYPosition",
	"followZPosition",
	"initializeOnAwake",
	"maintainedAxisOrientation",
	"skeletonRenderer",
	"useAppliedLocalScale",
];
const BONE_FOLLOWER_OBSERVED_PINS: &[(&str, Pinned)] = &[];

const PROJECTILE_SCRIPT_ID: [u8; 16] = script_hash("ff3dc693af34e1a68582bf88579b323a");
const PROJECTILE_OLD_TYPE_HASH: [u8; 16] = script_hash("4c87f6f827ebc02ed034579165593037");
const PROJECTILE_KEYS: &[&str] = &[
	"_actionController",
	"_allowZeroBlockCntLimit",
	"_alwaysHitTraceTargetInTheEnd",
	"_alwaysHitTraceTargetWhenReached",
	"_alwaysReachInTheEnd",
	"_bodyTransform",
	"_canHitSameTargetMultipleTimes",
	"_clearMainEffectWhenReached",
	"_containsChildrenBehaviours",
	"_eliminatableStatus",
	"_fireProjectileReachedEvent",
	"_fixDealHitTargetLeft",
	"_getLifeTimeFromBB",
	"_getMaxHitNumFromBB",
	"_hitNumType",
	"_keepAlreadyHitTime",
	"_lifeTime",
	"_lifeTimeType",
	"_limitedMaxHitNumToSourceBlockedCnt",
	"_mainEffect",
	"_managedBySource",
	"_maxHitNum",
	"_maxHitNumBBKey",
	"_movementAdjustableStatus",
	"_overrideAudioSignal",
	"_projectileType",
	"_randomDelayToBorn",
	"_stopAfterFirstHit",
	"_stopAfterMaxHit",
	"_stopGraphicProjectileWhenClearMainEffect",
	"_stopWhenSourceInvalid",
];
const PROJECTILE_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_maxHitNum", Pinned::Exact(r#"1"#)),
	("_hitNumType", Pinned::Exact(r#"1"#)),
	("_stopAfterMaxHit", Pinned::Exact(r#"1"#)),
	("_stopAfterFirstHit", Pinned::Exact(r#"0"#)),
	("_canHitSameTargetMultipleTimes", Pinned::Exact(r#"0"#)),
	("_alwaysHitTraceTargetInTheEnd", Pinned::Exact(r#"1"#)),
	("_alwaysHitTraceTargetWhenReached", Pinned::Exact(r#"0"#)),
	("_alwaysReachInTheEnd", Pinned::Exact(r#"0"#)),
	("_projectileType", Pinned::Exact(r#"0"#)),
	("_getMaxHitNumFromBB", Pinned::Exact(r#"0"#)),
	("_maxHitNumBBKey", Pinned::Exact(r#""""#)),
	("_limitedMaxHitNumToSourceBlockedCnt", Pinned::Exact(r#"0"#)),
	("_allowZeroBlockCntLimit", Pinned::Exact(r#"0"#)),
	("_containsChildrenBehaviours", Pinned::Exact(r#"0"#)),
	("_eliminatableStatus", Pinned::Exact(r#"0"#)),
	("_movementAdjustableStatus", Pinned::Exact(r#"0"#)),
	("_fireProjectileReachedEvent", Pinned::Exact(r#"0"#)),
	("_fixDealHitTargetLeft", Pinned::Exact(r#"0"#)),
	("_managedBySource", Pinned::Exact(r#"0"#)),
	("_stopWhenSourceInvalid", Pinned::Exact(r#"0"#)),
	("_getLifeTimeFromBB", Pinned::Exact(r#"0"#)),
	("_lifeTimeType", Pinned::Exact(r#"1"#)),
	("_randomDelayToBorn", Pinned::Exact(r#"0.0"#)),
	("_keepAlreadyHitTime", Pinned::Exact(r#"-1.0"#)),
	(
		"_actionController",
		Pinned::Exact(concat!(
			r#"{"_addExtraBuffsWhenReached":0,"_detachAllBuffsWhenStopped":0,"#,
			r#""_detachAllExtraBuffsWhenStopped":0,"_detachBuffsWhenTargetLeave":0,"#,
			r#""_dontAddBuffs":0,"_excludeAttachments":0,"_extraBuffsInTheEnd":[],"#,
			r#""_onlyAddBuffsToTargetOnce":0,"_onlyAddBuffsToTraceTarget":0,"#,
			r#""_useExtraBuffsWhenStopHit":0,"_useSelfBlackboard":0}"#,
		)),
	),
];

const PROJECTILE_EFFECTS_SCRIPT_ID: [u8; 16] = script_hash("7f5a3b4bce12ab7643db9556a2bb4356");
const PROJECTILE_EFFECTS_OLD_TYPE_HASH: [u8; 16] = script_hash("469b104756a2cbc6e7908e2a1d5368d8");
const PROJECTILE_EFFECTS_KEYS: &[&str] = &[
	"_alwaysPlayStartEffects",
	"_clearEffectsOnTileOnStop",
	"_clearHitEffectOnStop",
	"_clearPlayEffectOnStop",
	"_clearReachedEffectWhenStop",
	"_effectOnTrigger",
	"_effectsToTargetWhenStart",
	"_effectsToTileWhenStart",
	"_effectsWhenHit",
	"_effectsWhenReached",
	"_effectsWhenStart",
	"_effectsWhenStop",
	"_followTargetGround",
	"_forceReachTargetGround",
	"_mainEffectToReplace",
	"_onlyPlayHitEffectOnNotTraceTarget",
	"_onlyPlayHitEffectOnTraceTarget",
	"_onlyPlayHitEffectOnce",
	"_playStartEffectOnlyOnProjectile",
	"_repalceEffectsWhenHit",
	"_replaceHitEffectAfterFirstHit",
	"_replaceMainEffectWhenReach",
	"_setReachEffectOnProjectile",
	"_syncEffectScaleViaProjectile",
	"_useSourceFaceVector",
	"_useSourceFaceVectorOnHit",
	"_useTraceTargetWhenReach",
];
const PROJECTILE_EFFECTS_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_followTargetGround", Pinned::Exact(r#"0"#)),
	("_forceReachTargetGround", Pinned::Exact(r#"0"#)),
	("_useTraceTargetWhenReach", Pinned::Exact(r#"0"#)),
	("_effectOnTrigger", Pinned::Exact(r#"[]"#)),
	("_effectsToTargetWhenStart", Pinned::Exact(r#"[]"#)),
	("_effectsToTileWhenStart", Pinned::Exact(r#"[]"#)),
];

const MOVE_PARACURVE_SCRIPT_ID: [u8; 16] = script_hash("143457e638b5e2cb72c18aae98df15a9");
const MOVE_PARACURVE_OLD_TYPE_HASH: [u8; 16] = script_hash("a79c8a9fdc9afa4e0bce48f46888ad76");
const MOVE_PARACURVE_KEYS: &[&str] = &[
	"_awayFromSourceAsDirection",
	"_comeBack",
	"_comeBackSpeedScale",
	"_delayAfterReached",
	"_delayTime",
	"_exactMatchCameraForward",
	"_fixOnProjectileReached",
	"_flyToPredefinedLocation",
	"_flyToTargetLocationOnly",
	"_forceReachedWhenTimeup",
	"_forceResetToTargetPosWhenReached",
	"_freezeRotationXY",
	"_freezeRotationZ",
	"_handleWithIBuildableTarget",
	"_inverseParacurve",
	"_noRaiseHeightThreshold",
	"_onlyReachedInTime",
	"_predefinedLocation",
	"_raiseHeight",
	"_randomHeightRange",
	"_randomInverseParacurve",
	"_resetToTargetPosWhenReached",
	"_resetToTracetargetMapPosV3",
	"_rotateToTarget",
	"_sourceDirectionArcAngleRatio",
	"_speed",
	"_updateDelayTimeOnlyOnce",
	"_useRandomHeight",
	"_useSourceDirection",
	"_useSourceDirectionArcAngleWithRatio",
	"_useSourceDirectionRandomArcAngleStart",
	"_useSourceToTargetArcAngleWithRatio",
	"_useSourceToTargetRandomArcAngleStart",
	"_useStartToTarget",
	"_useTargetDirection",
];
const MOVE_PARACURVE_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_comeBack", Pinned::Exact(r#"0"#)),
	("_flyToPredefinedLocation", Pinned::Exact(r#"0"#)),
	("_flyToTargetLocationOnly", Pinned::Exact(r#"0"#)),
	("_forceReachedWhenTimeup", Pinned::Exact(r#"1"#)),
	("_useSourceDirection", Pinned::Exact(r#"0"#)),
	("_useTargetDirection", Pinned::Exact(r#"0"#)),
	("_useStartToTarget", Pinned::Exact(r#"0"#)),
	("_awayFromSourceAsDirection", Pinned::Exact(r#"0"#)),
	("_delayAfterReached", Pinned::Exact(r#"0"#)),
	("_onlyReachedInTime", Pinned::Exact(r#"0.0"#)),
	("_delayTime", Pinned::Exact(r#"0.0"#)),
	("_useRandomHeight", Pinned::Exact(r#"0"#)),
	("_randomInverseParacurve", Pinned::Exact(r#"0"#)),
	// Where and how the flight ends; arc height, arc angles and rotation stay unbound shape.
	("_inverseParacurve", Pinned::Exact(r#"0"#)),
	("_predefinedLocation", Pinned::Exact(r#"0"#)),
	("_handleWithIBuildableTarget", Pinned::Exact(r#"0"#)),
	("_fixOnProjectileReached", Pinned::Exact(r#"0"#)),
	("_resetToTargetPosWhenReached", Pinned::Exact(r#"0"#)),
	("_forceResetToTargetPosWhenReached", Pinned::Exact(r#"0"#)),
	("_resetToTracetargetMapPosV3", Pinned::Exact(r#"0"#)),
	("_updateDelayTimeOnlyOnce", Pinned::Exact(r#"0"#)),
];

const MOVE_LINEAR_SCRIPT_ID: [u8; 16] = script_hash("24e93061e69052d3a8d4fffb3590e907");
const MOVE_LINEAR_OLD_TYPE_HASH: [u8; 16] = script_hash("aa57bb66ed6d7ec1aa1172c84732600d");
const MOVE_LINEAR_KEYS: &[&str] = &[
	"_addInertia",
	"_awayFromSourceAsDirection",
	"_clearTraceTargetWhenReached",
	"_comeBack",
	"_comeBackSpeedScale",
	"_delayAfterReached",
	"_delayToReachAfterHit",
	"_delayToStart",
	"_deltaSpeedPerSec",
	"_distance",
	"_exactMatchCameraForward",
	"_finalSpeed",
	"_fixOnProjectileReached",
	"_flyToPredefinedLocation",
	"_flyToTargetLocationOnly",
	"_forceReachedWhenTimeup",
	"_forceResetToTargetPosWhenReached",
	"_forceUseTargetMountPoint",
	"_freezeRotationXY",
	"_freezeRotationZ",
	"_handleWithIBuildableTarget",
	"_moveType",
	"_moveTypeData",
	"_onlyReachedInTime",
	"_predefinedLocation",
	"_randomOffset",
	"_resetToTargetPosWhenReached",
	"_resetToTracetargetMapPosV3",
	"_rotateToTarget",
	"_rotateToTargetOnDelayToStart",
	"_skipSmoothLerpDirection",
	"_sourceDirectionArcAngleRatio",
	"_speed",
	"_turnSpeed",
	"_updateDelayTimeOnlyOnce",
	"_useDynamicSpeed",
	"_useSourceDirection",
	"_useSourceDirectionArcAngleWithRatio",
	"_useSourceDirectionRandomArcAngleStart",
	"_useSourceToTargetArcAngleWithRatio",
	"_useSourceToTargetRandomArcAngleStart",
	"_useStartToTarget",
	"_useTargetDirection",
];
const MOVE_LINEAR_OBSERVED_PINS: &[(&str, Pinned)] = &[
	("_comeBack", Pinned::Exact(r#"0"#)),
	("_flyToPredefinedLocation", Pinned::Exact(r#"0"#)),
	("_flyToTargetLocationOnly", Pinned::Exact(r#"0"#)),
	("_forceReachedWhenTimeup", Pinned::Exact(r#"1"#)),
	("_useSourceDirection", Pinned::Exact(r#"0"#)),
	("_useTargetDirection", Pinned::Exact(r#"0"#)),
	("_useStartToTarget", Pinned::Exact(r#"0"#)),
	("_awayFromSourceAsDirection", Pinned::Exact(r#"0"#)),
	("_delayAfterReached", Pinned::Exact(r#"0.0"#)),
	("_onlyReachedInTime", Pinned::Exact(r#"0.0"#)),
	("_useDynamicSpeed", Pinned::Exact(r#"0"#)),
	("_moveTypeData", Pinned::Exact(r#"{"data":[]}"#)),
	("_delayToStart", Pinned::Exact(r#"0.0"#)),
	("_delayToReachAfterHit", Pinned::Exact(r#"0.0"#)),
	// Where and how the flight ends; rotation and smoothing stay unbound shape.
	("_moveType", Pinned::Exact(r#"1"#)),
	("_clearTraceTargetWhenReached", Pinned::Exact(r#"1"#)),
	("_distance", Pinned::Exact(r#"0.0"#)),
	("_finalSpeed", Pinned::Exact(r#"0.0"#)),
	("_deltaSpeedPerSec", Pinned::Exact(r#"0.0"#)),
	("_turnSpeed", Pinned::Exact(r#"0.0"#)),
	("_addInertia", Pinned::Exact(r#"0"#)),
	(
		"_randomOffset",
		Pinned::Exact(r#"{"x":0.0,"y":0.0,"z":0.0}"#),
	),
	("_predefinedLocation", Pinned::Exact(r#"0"#)),
	("_handleWithIBuildableTarget", Pinned::Exact(r#"0"#)),
	("_fixOnProjectileReached", Pinned::Exact(r#"0"#)),
	("_resetToTargetPosWhenReached", Pinned::Exact(r#"0"#)),
	("_forceResetToTargetPosWhenReached", Pinned::Exact(r#"0"#)),
	("_resetToTracetargetMapPosV3", Pinned::Exact(r#"0"#)),
	("_forceUseTargetMountPoint", Pinned::Exact(r#"0"#)),
	("_updateDelayTimeOnlyOnce", Pinned::Exact(r#"0"#)),
];

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use crate::data::{BattlePrefabEntryKind, BattlePrefabManifestEntry, BattlePrefabObject};
	use serde_json::{json, Map, Value};

	use super::{
		attack_ability_from_raw, check_revision, check_serialized_objects,
		decode_plain_enemy_graphs, pins_enemy_packs, reviewed_literal, schema, single_container,
		DecodedPlainEnemyPrefab, Edge, LocatedPrefab, Pinned, PlainEnemyAttackForm,
		PlainEnemyPrefabDecodeError, PlainEnemyTableFacts, ProjectileMovementSchema,
		ReviewedSchema, Role, ATTACK_ABILITY_OLD_TYPE_HASH, ATTACK_ABILITY_SCRIPT_ID,
		METADATA_KEYS, SCHEMAS,
	};
	use crate::mechanics::{
		battle_prefab_interpreter::{
			BattlePrefabBuffSpec, BattlePrefabContainerMatch, BattlePrefabGameObject,
			BattlePrefabGraph, BattlePrefabMonoBehaviour, BattlePrefabObjectSource,
			BattlePrefabResolvedReference, BattlePrefabRootSection, BattlePrefabSemantic,
			BattlePrefabSemanticFact, BattlePrefabSemanticIssue, BattlePrefabSemanticProvenance,
			BattlePrefabUnavailableReference,
		},
		undescribed_enemy_catalog::ReviewedEnemyDamageType,
	};

	type DecodeError = PlainEnemyPrefabDecodeError;

	const ENEMY_CAB: &str = "CAB-enemy-fixture";
	const PROJECTILE_CAB: &str = "CAB-projectile-fixture";
	const ABSENT_CAB: &str = "CAB-skeleton-absent";
	const REVISION: &str = "fixture-mechanics-revision";
	const ENEMY_KEY: &str = "enemy_fixture";
	const PROJECTILE_KEY: &str = "projectile_fixture";

	const ROOT: usize = 0;
	const GRAPHIC: usize = 1;
	const FACE: usize = 2;
	const SPINE: usize = 3;
	const DEFAULT: usize = 4;
	const BODY: usize = 5;
	const ATTACK: usize = 6;
	const SHADOW: usize = 7;
	const TRIGGER: usize = 8;

	struct FixtureBehaviour {
		script_id: [u8; 16],
		old_type_hash: [u8; 16],
		fields: Map<String, Value>,
	}

	struct FixtureNode {
		name: String,
		parent: Option<usize>,
		behaviours: Vec<FixtureBehaviour>,
	}

	/// A programmatic prefab laid out like the probed dumps; no asset bytes are committed.
	struct FixturePrefab {
		cab: &'static str,
		logical_path: &'static str,
		key: &'static str,
		nodes: Vec<FixtureNode>,
	}

	fn go_path(node: usize) -> i64 {
		(node as i64 + 1) * 100
	}

	fn transform_path(node: usize) -> i64 {
		go_path(node) + 1
	}

	fn behaviour_path(node: usize, slot: usize) -> i64 {
		go_path(node) + 2 + slot as i64
	}

	fn pptr(path_id: i64) -> Value {
		json!({"m_FileID": 0, "m_PathID": path_id})
	}

	fn node(name: &str, parent: Option<usize>, behaviours: Vec<FixtureBehaviour>) -> FixtureNode {
		FixtureNode {
			name: name.to_owned(),
			parent,
			behaviours,
		}
	}

	fn pinned(role: Role, key: &str) -> Option<Pinned> {
		schema(role)
			.pins
			.iter()
			.flat_map(|table: &&[(&str, Pinned)]| table.iter())
			.find(|(field, _)| *field == key)
			.map(|(_, pinned)| *pinned)
	}

	fn is_edge_key(role: Role, key: &str) -> bool {
		schema(role).edges.iter().any(|(field, _)| {
			field
				.strip_prefix("$.")
				.and_then(|path: &str| path.split('[').next())
				== Some(key)
		})
	}

	/// Values for keys whose reviewed binding is a predicate rather than one literal.
	fn sample_value(key: &str) -> Option<Value> {
		match key {
			"_modes" => Some(json!([])),
			"_projectileKey" => Some(json!(PROJECTILE_KEY)),
			"_lifeTime" => Some(json!(10.0)),
			"_speed" => Some(json!(5.0)),
			"_animations" => Some(json!([
				{"animKey": "Idle", "animName": "Idle", "ignoreMissing": 0, "loop": 1, "speed": 1.0},
				{"animKey": "Attack", "animName": "Attack", "ignoreMissing": 0, "loop": 0, "speed": 1.0},
			])),
			"_castEffects" | "_hitEffects" => Some(json!([])),
			"skeletonDataAsset" => Some(json!({"m_FileID": 2, "m_PathID": 77})),
			_ => None,
		}
	}

	fn reviewed(role: Role) -> FixtureBehaviour {
		let reviewed: &ReviewedSchema = schema(role);
		let mut fields: Map<String, Value> = Map::new();
		for key in reviewed.keys {
			let value: Value = match (sample_value(key), pinned(role, key)) {
				(Some(sample), _) => sample,
				(None, Some(Pinned::Exact(literal))) => reviewed_literal(literal),
				(None, Some(Pinned::OneOf(literals))) => reviewed_literal(literals[0]),
				(None, Some(Pinned::NullReference)) => pptr(0),
				(None, Some(Pinned::Satisfies { .. })) => panic!("{key} needs a sample value"),
				(None, None) if is_edge_key(role, key) => pptr(0),
				(None, None) => json!(0),
			};
			fields.insert((*key).to_owned(), value);
		}
		fields.insert("m_Enabled".to_owned(), json!(1));
		fields.insert("m_Name".to_owned(), json!(""));
		fields.insert("m_Script".to_owned(), json!({"m_FileID": 1, "m_PathID": 7}));
		FixtureBehaviour {
			script_id: reviewed.script_id,
			old_type_hash: reviewed.old_type_hash,
			fields,
		}
	}

	fn unknown_behaviour() -> FixtureBehaviour {
		let mut fields: Map<String, Value> = Map::new();
		fields.insert("_extendable".to_owned(), json!(0));
		fields.insert("m_Enabled".to_owned(), json!(1));
		FixtureBehaviour {
			script_id: [0xbb; 16],
			old_type_hash: [0x5c; 16],
			fields,
		}
	}

	impl FixturePrefab {
		fn fields(&mut self, node: usize, slot: usize) -> &mut Map<String, Value> {
			&mut self.nodes[node].behaviours[slot].fields
		}

		fn set(&mut self, node: usize, slot: usize, field: &str, value: Value) -> &mut Self {
			self.fields(node, slot).insert(field.to_owned(), value);
			self
		}

		fn source(&self, path_id: i64) -> BattlePrefabObjectSource {
			BattlePrefabObjectSource {
				logical_path: self.logical_path.to_owned(),
				cab_name: self.cab.to_owned(),
				path_id,
			}
		}

		/// Builds the graph the interpreter would produce, deriving references, issues and facts
		/// from the raw fields so they cannot drift from them.
		fn assemble(&self) -> BattlePrefabGraph {
			let mut game_objects: Vec<BattlePrefabGameObject> = Vec::new();
			let mut mono_behaviours: Vec<BattlePrefabMonoBehaviour> = Vec::new();
			for (node_index, fixture) in self.nodes.iter().enumerate() {
				let mut component_sources: Vec<BattlePrefabObjectSource> =
					vec![self.source(transform_path(node_index))];
				let mut mono_behaviour_indices: Vec<usize> = Vec::new();
				for (slot, behaviour) in fixture.behaviours.iter().enumerate() {
					let source: BattlePrefabObjectSource =
						self.source(behaviour_path(node_index, slot));
					component_sources.push(source.clone());
					mono_behaviour_indices.push(mono_behaviours.len());
					let mut raw: Map<String, Value> = behaviour.fields.clone();
					raw.insert("m_GameObject".to_owned(), pptr(go_path(node_index)));
					mono_behaviours.push(BattlePrefabMonoBehaviour {
						game_object_index: Some(node_index),
						source,
						script_id: Some(behaviour.script_id),
						old_type_hash: Some(behaviour.old_type_hash),
						script_class: None,
						raw_fields: Value::Object(raw),
						buffs: Vec::new(),
						semantic_facts: Vec::new(),
						references: Vec::new(),
						unavailable_references: Vec::new(),
						referenced_mono_behaviour_indices: Vec::new(),
					});
				}
				game_objects.push(BattlePrefabGameObject {
					source: self.source(go_path(node_index)),
					name: fixture.name.clone(),
					hierarchy_path: fixture.name.clone(),
					component_sources,
					mono_behaviour_indices,
					child_indices: Vec::new(),
				});
			}
			for (node_index, fixture) in self.nodes.iter().enumerate() {
				if let Some(parent) = fixture.parent {
					game_objects[parent].child_indices.push(node_index);
				}
			}
			let mut issues: Vec<BattlePrefabSemanticIssue> = Vec::new();
			for index in 0..mono_behaviours.len() {
				let mut pptrs: Vec<(String, i32, i64)> = Vec::new();
				collect_pptrs(&mono_behaviours[index].raw_fields, "$", None, &mut pptrs);
				let source: BattlePrefabObjectSource = mono_behaviours[index].source.clone();
				for (field_path, file_id, path_id) in pptrs {
					if path_id == 0 {
						continue;
					}
					if file_id != 0 {
						issues.push(BattlePrefabSemanticIssue::UnavailableExternalReference {
							source: source.clone(),
							field_path: field_path.clone(),
							file_id,
							path_id,
							target_cab: ABSENT_CAB.to_owned(),
						});
						mono_behaviours[index].unavailable_references.push(
							BattlePrefabUnavailableReference {
								field_path,
								file_id,
								path_id,
								target_cab: ABSENT_CAB.to_owned(),
							},
						);
						continue;
					}
					let target: BattlePrefabObjectSource = self.source(path_id);
					let behaviour_target: Option<usize> = mono_behaviours
						.iter()
						.position(|behaviour| behaviour.source == target);
					assert!(
						behaviour_target.is_some()
							|| game_objects
								.iter()
								.any(|game_object| game_object.component_sources.contains(&target)),
						"fixture reference {field_path} must target an object in the fixture"
					);
					let behaviour: &mut BattlePrefabMonoBehaviour = &mut mono_behaviours[index];
					behaviour
						.references
						.push(BattlePrefabResolvedReference { field_path, target });
					if let Some(target_index) = behaviour_target {
						if !behaviour
							.referenced_mono_behaviour_indices
							.contains(&target_index)
						{
							behaviour
								.referenced_mono_behaviour_indices
								.push(target_index);
						}
					}
				}
			}
			for behaviour in &mut mono_behaviours {
				let script_id: [u8; 16] = behaviour.script_id.expect("fixture script ID");
				let old_type_hash: [u8; 16] = behaviour.old_type_hash.expect("fixture type hash");
				if script_id != ATTACK_ABILITY_SCRIPT_ID {
					issues.push(BattlePrefabSemanticIssue::UnknownScriptId {
						source: behaviour.source.clone(),
						script_id,
					});
					continue;
				}
				assert_eq!(old_type_hash, ATTACK_ABILITY_OLD_TYPE_HASH);
				behaviour.semantic_facts = attack_ability_from_raw(&behaviour.raw_fields)
					.map(|attack| BattlePrefabSemanticFact {
						provenance: BattlePrefabSemanticProvenance {
							source: behaviour.source.clone(),
							script_id,
							old_type_hash,
							mechanics_revision: REVISION.to_owned(),
						},
						semantic: BattlePrefabSemantic::AttackAbility(attack),
					})
					.into_iter()
					.collect();
			}
			BattlePrefabGraph {
				mechanics_revision: REVISION.to_owned(),
				logical_path: self.logical_path.to_owned(),
				asset_path: format!("fixture/{}.prefab", self.key),
				section: BattlePrefabRootSection::MainAsset,
				prefab_name: self.key.to_owned(),
				root_game_object_indices: vec![ROOT],
				game_objects,
				mono_behaviours,
				issues,
			}
		}
	}

	fn collect_pptrs(
		value: &Value,
		path: &str,
		field: Option<&str>,
		output: &mut Vec<(String, i32, i64)>,
	) {
		match value {
			Value::Object(object)
				if object.contains_key("m_FileID") && object.contains_key("m_PathID") =>
			{
				if !matches!(field, Some("m_GameObject" | "m_Script")) {
					let file_id: i32 = object["m_FileID"]
						.as_i64()
						.and_then(|id: i64| i32::try_from(id).ok())
						.expect("fixture fileID");
					let path_id: i64 = object["m_PathID"].as_i64().expect("fixture pathID");
					output.push((path.to_owned(), file_id, path_id));
				}
			}
			Value::Object(object) => {
				for (key, child) in object {
					collect_pptrs(child, &format!("{path}.{key}"), Some(key), output);
				}
			}
			Value::Array(values) => {
				for (index, child) in values.iter().enumerate() {
					collect_pptrs(child, &format!("{path}[{index}]"), field, output);
				}
			}
			_ => {}
		}
	}

	fn base_enemy(attack_node: FixtureNode) -> FixturePrefab {
		let mut prefab: FixturePrefab = FixturePrefab {
			cab: ENEMY_CAB,
			logical_path: "enm_pfb_fixture",
			key: ENEMY_KEY,
			nodes: vec![
				node(
					ENEMY_KEY,
					None,
					vec![reviewed(Role::Controller), reviewed(Role::Steering)],
				),
				node("Graphic", Some(ROOT), vec![reviewed(Role::Graphic)]),
				node(
					"FaceSwitcher",
					Some(GRAPHIC),
					vec![reviewed(Role::FaceSwitcher)],
				),
				node("Spine", Some(FACE), vec![reviewed(Role::Spine)]),
				node("Default", Some(ROOT), vec![reviewed(Role::Mode)]),
				node("Body", Some(DEFAULT), Vec::new()),
				attack_node,
				node("Shadow", Some(ROOT), vec![reviewed(Role::Shadow)]),
			],
		};
		prefab
			.set(ROOT, 0, "_modes", json!([pptr(behaviour_path(DEFAULT, 0))]))
			.set(ROOT, 0, "_animator", pptr(behaviour_path(GRAPHIC, 0)))
			.set(ROOT, 0, "_shadow", pptr(behaviour_path(SHADOW, 0)))
			.set(ROOT, 0, "_effectTransform", pptr(transform_path(BODY)))
			.set(GRAPHIC, 0, "_faceSwitcher", pptr(behaviour_path(FACE, 0)))
			.set(GRAPHIC, 0, "_skeleton", pptr(behaviour_path(SPINE, 0)))
			.set(GRAPHIC, 0, "_headTransform", pptr(transform_path(SPINE)))
			.set(GRAPHIC, 0, "_hitTransform", pptr(transform_path(SPINE)))
			.set(DEFAULT, 0, "_combat", pptr(behaviour_path(ATTACK, 0)))
			.set(DEFAULT, 0, "_bodyTransform", pptr(transform_path(BODY)));
		prefab
	}

	/// gopro-shaped: a melee attack with its effects on `Default/Body/Combat`.
	fn melee_prefab() -> FixturePrefab {
		base_enemy(node(
			"Combat",
			Some(BODY),
			vec![reviewed(Role::MeleeAttack), reviewed(Role::AttackEffects)],
		))
	}

	/// wteeth-shaped: the melee form with a larger animation scale and no effect transform.
	fn wteeth_prefab() -> FixturePrefab {
		let mut prefab: FixturePrefab = melee_prefab();
		prefab
			.set(ATTACK, 0, "_maxAnimScale", json!(1.100000023841858))
			.set(ROOT, 0, "_effectTransform", pptr(0))
			.set(
				SPINE,
				0,
				"skeletonDataAsset",
				json!({"m_FileID": 3, "m_PathID": 91}),
			);
		prefab
	}

	/// mocock (effects, two bone followers) and wizard_2 (no effects, one) share this layout.
	fn ranged_prefab(
		attack_effects: bool,
		bone_followers: usize,
		damage_type: i64,
	) -> FixturePrefab {
		let mut attack: Vec<FixtureBehaviour> = vec![reviewed(Role::RangedAttack)];
		if attack_effects {
			attack.push(reviewed(Role::AttackEffects));
		}
		let mut prefab: FixturePrefab = base_enemy(node("Attack", Some(BODY), attack));
		prefab.nodes.push(node(
			"Trigger",
			Some(ATTACK),
			vec![
				reviewed(Role::Trigger),
				reviewed(Role::TargetFilter),
				reviewed(Role::RangeExtension),
			],
		));
		for index in 0..bone_followers {
			let follower: usize = prefab.nodes.len();
			prefab.nodes.push(node(
				&format!("BonePoint{index}"),
				Some(SPINE),
				vec![reviewed(Role::BoneFollower)],
			));
			prefab.set(
				follower,
				0,
				"skeletonRenderer",
				pptr(behaviour_path(SPINE, 0)),
			);
		}
		prefab
			.set(DEFAULT, 0, "_attack", pptr(behaviour_path(ATTACK, 0)))
			.set(
				DEFAULT,
				0,
				"_attackTrigger",
				pptr(behaviour_path(TRIGGER, 0)),
			)
			.set(
				GRAPHIC,
				0,
				"_muzzleTransform",
				pptr(transform_path(TRIGGER + 1)),
			)
			.set(ATTACK, 0, "_damageType", json!(damage_type))
			.set(
				SPINE,
				0,
				"skeletonDataAsset",
				json!({"m_FileID": 3, "m_PathID": 55}),
			);
		prefab
	}

	fn mocock_prefab() -> FixturePrefab {
		ranged_prefab(true, 2, 1)
	}

	fn wizard_prefab() -> FixturePrefab {
		ranged_prefab(false, 1, 2)
	}

	fn projectile_prefab(movement: ProjectileMovementSchema) -> FixturePrefab {
		let move_role: Role = match movement {
			ProjectileMovementSchema::Paracurve => Role::MoveParacurve,
			ProjectileMovementSchema::Linear => Role::MoveLinear,
		};
		FixturePrefab {
			cab: PROJECTILE_CAB,
			logical_path: "[uc]projectiles",
			key: PROJECTILE_KEY,
			nodes: vec![node(
				PROJECTILE_KEY,
				None,
				vec![
					reviewed(Role::Projectile),
					reviewed(Role::ProjectileEffects),
					reviewed(move_role),
				],
			)],
		}
	}

	fn container(
		graph: &BattlePrefabGraph,
		bundle_kind: BattlePrefabEntryKind,
	) -> BattlePrefabContainerMatch {
		BattlePrefabContainerMatch {
			logical_path: graph.logical_path.clone(),
			bundle_kind,
			container_file_id: 0,
			root: graph.game_objects[ROOT].source.clone(),
		}
	}

	fn table_facts(
		apply_way: Option<&str>,
		range_radius: Option<f64>,
		damage: ReviewedEnemyDamageType,
	) -> PlainEnemyTableFacts {
		PlainEnemyTableFacts {
			prefab_key: ENEMY_KEY.to_owned(),
			apply_way: apply_way.map(str::to_owned),
			range_radius,
			block_demand: 1,
			damage,
		}
	}

	fn melee_facts() -> PlainEnemyTableFacts {
		table_facts(Some("MELEE"), None, ReviewedEnemyDamageType::Physical)
	}

	fn mocock_facts() -> PlainEnemyTableFacts {
		table_facts(
			Some("RANGED"),
			Some(1.75),
			ReviewedEnemyDamageType::Physical,
		)
	}

	fn wizard_facts() -> PlainEnemyTableFacts {
		table_facts(Some("RANGED"), Some(2.0), ReviewedEnemyDamageType::Arts)
	}

	fn decode_graphs(
		graph: &BattlePrefabGraph,
		projectile: Option<BattlePrefabGraph>,
		facts: &PlainEnemyTableFacts,
	) -> Result<DecodedPlainEnemyPrefab, DecodeError> {
		let location: BattlePrefabContainerMatch =
			container(graph, BattlePrefabEntryKind::Enemypack);
		let resolve =
			|key: &str| -> Result<(BattlePrefabContainerMatch, BattlePrefabGraph), DecodeError> {
				let graph: BattlePrefabGraph = projectile
					.clone()
					.filter(|graph: &BattlePrefabGraph| graph.prefab_name == key)
					.ok_or_else(|| DecodeError::ProjectileNotFound {
						asset_path: key.to_owned(),
					})?;
				Ok((container(&graph, BattlePrefabEntryKind::Common), graph))
			};
		decode_plain_enemy_graphs(&location, graph, &resolve, facts)
	}

	fn decode(
		enemy: &FixturePrefab,
		projectile: Option<&FixturePrefab>,
		facts: &PlainEnemyTableFacts,
	) -> Result<DecodedPlainEnemyPrefab, DecodeError> {
		decode_graphs(
			&enemy.assemble(),
			projectile.map(FixturePrefab::assemble),
			facts,
		)
	}

	fn paracurve() -> FixturePrefab {
		projectile_prefab(ProjectileMovementSchema::Paracurve)
	}

	fn linear() -> FixturePrefab {
		projectile_prefab(ProjectileMovementSchema::Linear)
	}

	fn rejected(
		enemy: &FixturePrefab,
		projectile: Option<&FixturePrefab>,
		facts: &PlainEnemyTableFacts,
	) -> DecodeError {
		decode(enemy, projectile, facts).expect_err("the fixture must be rejected")
	}

	/// The innermost error, looking through projectile wrapping.
	fn innermost(error: &DecodeError) -> &DecodeError {
		match error {
			DecodeError::Projectile { error, .. } => innermost(error),
			other => other,
		}
	}

	fn assert_drift(error: &DecodeError, expected_field: &str) {
		match innermost(error) {
			DecodeError::FieldDrift { field, .. } => assert_eq!(field, expected_field, "{error}"),
			other => panic!("expected drift on {expected_field}, found {other}"),
		}
	}

	fn assert_rule(error: &DecodeError, expected_rule: &str) {
		assert_eq!(innermost(error).rule(), expected_rule, "{error}");
	}

	#[test]
	fn reviewed_tables_are_internally_consistent() {
		for (index, reviewed) in SCHEMAS.iter().enumerate() {
			assert_eq!(reviewed.role as usize, index, "{}", reviewed.name);
			let keys: BTreeSet<&str> = reviewed.keys.iter().copied().collect();
			assert_eq!(
				keys.len(),
				reviewed.keys.len(),
				"{} keys are unique",
				reviewed.name
			);
			assert!(METADATA_KEYS.iter().all(|key: &&str| !keys.contains(key)));
			let mut bound: BTreeSet<&str> = BTreeSet::new();
			for (field, pin) in reviewed.pins.iter().flat_map(|table| table.iter()) {
				assert!(
					keys.contains(field),
					"{} pins unknown key {field}",
					reviewed.name
				);
				assert!(bound.insert(field), "{} pins {field} twice", reviewed.name);
				if let Pinned::Exact(literal) = pin {
					let _: Value = reviewed_literal(literal);
				}
			}
			for (field, edge) in reviewed.edges {
				let key: &str = field
					.strip_prefix("$.")
					.and_then(|path: &str| path.split('[').next())
					.expect("edge paths are rooted field paths");
				assert!(
					keys.contains(key),
					"{} edge on unknown key {key}",
					reviewed.name
				);
				assert!(
					!bound.contains(key),
					"{} edge {key} is also pinned",
					reviewed.name
				);
				assert!(!edge.expected().is_empty());
			}
		}
		// Distinct script IDs make every role claim unique, so a DuplicateRole cannot arise from
		// claim_owned and stays a defensive guard.
		let script_ids: BTreeSet<[u8; 16]> = SCHEMAS
			.iter()
			.map(|reviewed: &ReviewedSchema| reviewed.script_id)
			.collect();
		assert_eq!(script_ids.len(), SCHEMAS.len());
		assert_eq!(
			schema(Role::MeleeAttack).script_id,
			ATTACK_ABILITY_SCRIPT_ID
		);
		assert_eq!(
			schema(Role::MeleeAttack).old_type_hash,
			ATTACK_ABILITY_OLD_TYPE_HASH
		);
		assert!(matches!(
			schema(Role::Spine).edges,
			[("$.skeletonDataAsset", Edge::OutsideSnapshot)]
		));
	}

	#[test]
	fn gopro_shaped_melee_prefab_is_supported() {
		let decoded: DecodedPlainEnemyPrefab =
			decode(&melee_prefab(), None, &melee_facts()).expect("melee form decodes");
		assert_eq!(decoded.form, PlainEnemyAttackForm::Melee);
		assert_eq!(decoded.damage, ReviewedEnemyDamageType::Physical);
		assert_eq!(
			decoded.location.bundle_kind,
			BattlePrefabEntryKind::Enemypack
		);
		assert_eq!(decoded.provenance.len(), 9);
		// Eight unknown scripts (the attack ability has a handler) plus the skeleton reference.
		assert_eq!(decoded.consumed_issues.len(), 9);
		let unavailable: Vec<&BattlePrefabSemanticIssue> = decoded
			.consumed_issues
			.iter()
			.filter(|issue| {
				matches!(
					issue,
					BattlePrefabSemanticIssue::UnavailableExternalReference { .. }
				)
			})
			.collect();
		assert!(matches!(
			unavailable.as_slice(),
			[BattlePrefabSemanticIssue::UnavailableExternalReference { field_path, file_id: 2, .. }]
				if field_path == "$.skeletonDataAsset"
		));
	}

	#[test]
	fn wteeth_variant_with_larger_animation_scale_and_no_effect_transform_is_supported() {
		let decoded: DecodedPlainEnemyPrefab =
			decode(&wteeth_prefab(), None, &melee_facts()).expect("wteeth variant decodes");
		assert_eq!(decoded.form, PlainEnemyAttackForm::Melee);
		assert!(decoded.consumed_issues.iter().any(|issue| matches!(
			issue,
			BattlePrefabSemanticIssue::UnavailableExternalReference { file_id: 3, .. }
		)));
	}

	#[test]
	fn mocock_shaped_ranged_paracurve_prefab_is_supported() {
		let decoded: DecodedPlainEnemyPrefab =
			decode(&mocock_prefab(), Some(&paracurve()), &mocock_facts())
				.expect("ranged paracurve form decodes");
		let PlainEnemyAttackForm::RangedProjectile {
			projectile_key,
			projectile,
			movement,
			speed,
		} = &decoded.form
		else {
			panic!("expected the ranged form, found {:?}", decoded.form);
		};
		assert_eq!(projectile_key, PROJECTILE_KEY);
		assert_eq!(projectile.logical_path, "[uc]projectiles");
		assert_eq!(*movement, ProjectileMovementSchema::Paracurve);
		assert_eq!(*speed, 5.0);
		assert_eq!(decoded.damage, ReviewedEnemyDamageType::Physical);
		// controller, steering, mode, attack, effects, trigger ×3, graphic, face, spine, shadow,
		// two bone followers, then the projectile's three behaviours; every one is an unknown
		// script, plus the Spine skeleton reference.
		assert_eq!(decoded.provenance.len(), 17);
		assert_eq!(decoded.consumed_issues.len(), 18);
		assert!(decoded.consumed_issues.iter().all(
			|issue| !matches!(issue, BattlePrefabSemanticIssue::UnknownScriptId { script_id, .. }
				if *script_id == ATTACK_ABILITY_SCRIPT_ID)
		));
	}

	#[test]
	fn wizard_shaped_ranged_linear_arts_prefab_is_supported() {
		let decoded: DecodedPlainEnemyPrefab =
			decode(&wizard_prefab(), Some(&linear()), &wizard_facts())
				.expect("ranged linear form decodes");
		assert!(matches!(
			decoded.form,
			PlainEnemyAttackForm::RangedProjectile {
				movement: ProjectileMovementSchema::Linear,
				..
			}
		));
		assert_eq!(decoded.damage, ReviewedEnemyDamageType::Arts);
		// Twelve enemy behaviours (no effects, one bone follower) and three projectile behaviours.
		assert_eq!(decoded.provenance.len(), 15);
		assert_eq!(decoded.consumed_issues.len(), 16);
	}

	/// Adds an aoemag-shaped `Selector` child under the attack GameObject.
	fn add_selector(prefab: &mut FixturePrefab) -> usize {
		let selector: usize = prefab.nodes.len();
		let mut filter: FixtureBehaviour = reviewed(Role::TargetFilter);
		filter.fields.insert("_limitTargetNum".to_owned(), json!(0));
		filter.fields.insert("_postFilter".to_owned(), json!(0));
		prefab.nodes.push(node(
			"Selector",
			Some(ATTACK),
			vec![filter, unknown_behaviour()],
		));
		selector
	}

	#[test]
	fn ranged_single_target_is_supported_but_the_splash_variant_of_the_same_prefab_is_not() {
		decode(&mocock_prefab(), Some(&paracurve()), &mocock_facts())
			.expect("the single-target ranged prefab is supported");

		let mut splash: FixturePrefab = mocock_prefab();
		let selector: usize = add_selector(&mut splash);
		let mut attack: FixtureBehaviour = reviewed(Role::MeleeAttack);
		for (field, value) in [
			("_attackType", json!(0)),
			("_useDynamicAttackType", json!(1)),
			("_preDelay", json!(0.6669999957084656)),
			("_waitForAttackEvent", json!(0)),
			("_damageType", json!(2)),
			("_selector", pptr(behaviour_path(selector, 0))),
		] {
			attack.fields.insert(field.to_owned(), value);
		}
		splash.nodes[ATTACK].behaviours[0] = attack;
		let error: DecodeError = rejected(&splash, Some(&paracurve()), &wizard_facts());
		assert!(
			matches!(error, DecodeError::UnsupportedModeWiring { .. }),
			"{error}"
		);
	}

	#[test]
	fn each_splash_marker_alone_is_rejected() {
		// A selector reaching an extra GameObject fails the partition before any edge is read.
		let mut selector_set: FixturePrefab = melee_prefab();
		let selector: usize = add_selector(&mut selector_set);
		selector_set.set(ATTACK, 0, "_selector", pptr(behaviour_path(selector, 0)));
		assert!(matches!(
			rejected(&selector_set, None, &melee_facts()),
			DecodeError::UnassignedBehaviour { .. }
		));

		// A selector reaching an already labelled object is the attack's own unreviewed edge.
		for (enemy, target) in [
			(melee_prefab(), pptr(behaviour_path(SHADOW, 0))),
			(melee_prefab(), pptr(transform_path(BODY))),
			(mocock_prefab(), pptr(behaviour_path(TRIGGER, 1))),
		] {
			let mut selected: FixturePrefab = enemy;
			selected.set(ATTACK, 0, "_selector", target);
			let facts: PlainEnemyTableFacts = if selected.nodes.len() > TRIGGER {
				mocock_facts()
			} else {
				melee_facts()
			};
			let error: DecodeError = rejected(&selected, Some(&paracurve()), &facts);
			assert!(
				matches!(&error, DecodeError::UnexpectedReference { field, .. }
					if field == "$._selector"),
				"{error}"
			);
		}

		for (field, value) in [
			("_attackType", json!(0)),
			("_useDynamicAttackType", json!(1)),
			("_preDelay", json!(0.6669999957084656)),
			("_waitForAttackEvent", json!(0)),
		] {
			let mut melee: FixturePrefab = melee_prefab();
			melee.set(ATTACK, 0, field, value.clone());
			assert_drift(&rejected(&melee, None, &melee_facts()), field);
			let mut ranged: FixturePrefab = mocock_prefab();
			ranged.set(ATTACK, 0, field, value);
			assert_drift(
				&rejected(&ranged, Some(&paracurve()), &mocock_facts()),
				field,
			);
		}

		let mut unreached: FixturePrefab = melee_prefab();
		add_selector(&mut unreached);
		assert!(matches!(
			rejected(&unreached, None, &melee_facts()),
			DecodeError::UnassignedBehaviour { .. }
		));

		for (field, value) in [("_limitTargetNum", json!(0)), ("_postFilter", json!(0))] {
			let mut ranged: FixturePrefab = mocock_prefab();
			ranged.set(TRIGGER, 1, field, value);
			assert_drift(
				&rejected(&ranged, Some(&paracurve()), &mocock_facts()),
				field,
			);
		}

		let mut mortar: FixturePrefab = melee_prefab();
		mortar.set(ATTACK, 0, "_attackType", json!(0)).set(
			ATTACK,
			0,
			"_useDynamicAttackType",
			json!(1),
		);
		assert_drift(&rejected(&mortar, None, &melee_facts()), "_attackType");
	}

	#[test]
	fn extra_effects_are_rejected() {
		let melee_cases: [(usize, &str, Value); 8] = [
			(ATTACK, "_activeBuffs", json!([{"buffKey": "x"}])),
			(DEFAULT, "_generalAbilities", json!([pptr(0)])),
			(ROOT, "_commonAbilities", json!([pptr(0)])),
			(
				ROOT,
				"_specialBlockCondition",
				json!({"_buffKeyPairs": [], "_filterTags": [], "_type": 1}),
			),
			(ATTACK, "_elementDamageType", json!(1)),
			(ATTACK, "_extraDamageType", json!(1)),
			(ATTACK, "_atkScale", json!(1.2)),
			(ATTACK, "_cooldown", json!(1.0)),
		];
		for (node_index, field, value) in melee_cases {
			let mut melee: FixturePrefab = melee_prefab();
			melee.set(node_index, 0, field, value);
			assert_drift(&rejected(&melee, None, &melee_facts()), field);
		}

		let mut extra_keys: FixturePrefab = mocock_prefab();
		extra_keys.set(
			ATTACK,
			0,
			"_extraProjectileKeys",
			json!(["projectile_other"]),
		);
		assert_drift(
			&rejected(&extra_keys, Some(&paracurve()), &mocock_facts()),
			"_extraProjectileKeys",
		);

		let mut extra_buffs: Value = json!({
			"_addExtraBuffsWhenReached": 0,
			"_detachAllBuffsWhenStopped": 0,
			"_detachAllExtraBuffsWhenStopped": 0,
			"_detachBuffsWhenTargetLeave": 0,
			"_dontAddBuffs": 0,
			"_excludeAttachments": 0,
			"_extraBuffsInTheEnd": [],
			"_onlyAddBuffsToTargetOnce": 0,
			"_onlyAddBuffsToTraceTarget": 0,
			"_useExtraBuffsWhenStopHit": 0,
			"_useSelfBlackboard": 0,
		});
		extra_buffs["_extraBuffsInTheEnd"] = json!([{"buffKey": "burn"}]);
		let projectile_cases: [(usize, &str, Value); 6] = [
			(0, "_maxHitNum", json!(2)),
			(0, "_canHitSameTargetMultipleTimes", json!(1)),
			(0, "_actionController", extra_buffs),
			(0, "_lifeTimeType", json!(0)),
			(2, "_comeBack", json!(1)),
			(2, "_forceReachedWhenTimeup", json!(0)),
		];
		for (slot, field, value) in projectile_cases {
			let mut projectile: FixturePrefab = paracurve();
			projectile.set(ROOT, slot, field, value);
			let error: DecodeError = rejected(&mocock_prefab(), Some(&projectile), &mocock_facts());
			assert!(matches!(error, DecodeError::Projectile { .. }), "{error}");
			assert_drift(&error, field);
		}

		let mut extra_behaviour: FixturePrefab = paracurve();
		extra_behaviour.nodes[ROOT]
			.behaviours
			.push(reviewed(Role::MoveLinear));
		assert_rule(
			&rejected(&mocock_prefab(), Some(&extra_behaviour), &mocock_facts()),
			"R5 role partition",
		);

		let missing: DecodeError = rejected(&mocock_prefab(), None, &mocock_facts());
		assert!(
			matches!(&missing, DecodeError::Projectile { projectile_key, error }
				if projectile_key == PROJECTILE_KEY
					&& matches!(**error, DecodeError::ProjectileNotFound { .. })),
			"{missing}"
		);
	}

	#[test]
	fn schema_drift_and_structure_changes_are_rejected() {
		let mut unknown: FixturePrefab = melee_prefab();
		unknown
			.nodes
			.push(node("Extra", Some(ROOT), vec![unknown_behaviour()]));
		assert!(matches!(
			rejected(&unknown, None, &melee_facts()),
			DecodeError::UnassignedBehaviour { .. }
		));

		let mut rehashed: FixturePrefab = melee_prefab();
		rehashed.nodes[SHADOW].behaviours[0].old_type_hash = [0x0f; 16];
		assert_rule(
			&rejected(&rehashed, None, &melee_facts()),
			"R6 reviewed edges",
		);

		let mut hash_issue: BattlePrefabGraph = melee_prefab().assemble();
		let shadow_source: BattlePrefabObjectSource =
			hash_issue.game_objects[SHADOW].component_sources[1].clone();
		for issue in &mut hash_issue.issues {
			let is_shadow: bool = matches!(
				issue,
				BattlePrefabSemanticIssue::UnknownScriptId { source, .. } if *source == shadow_source
			);
			if is_shadow {
				*issue = BattlePrefabSemanticIssue::ScriptSchemaHashMismatch {
					source: shadow_source.clone(),
					script_id: schema(Role::Shadow).script_id,
					actual_old_type_hash: Some([0x0f; 16]),
					expected_old_type_hashes: Vec::new(),
				};
			}
		}
		assert!(matches!(
			decode_graphs(&hash_issue, None, &melee_facts()),
			Err(DecodeError::Issues { ref missing, ref unexpected })
				if missing.len() == 1 && unexpected.len() == 1
		));

		let mut added: FixturePrefab = mocock_prefab();
		added.set(TRIGGER, 1, "_addedField", json!(0));
		assert!(matches!(
			rejected(&added, Some(&paracurve()), &mocock_facts()),
			DecodeError::UnclassifiedField { ref field, .. } if field == "_addedField"
		));

		let mut removed: FixturePrefab = melee_prefab();
		removed.fields(DEFAULT, 0).remove("_hideUIAttackRange");
		assert!(matches!(
			rejected(&removed, None, &melee_facts()),
			DecodeError::MissingField { ref field, .. } if field == "_hideUIAttackRange"
		));

		let mut second_outside: FixturePrefab = melee_prefab();
		second_outside.set(
			DEFAULT,
			0,
			"_animatorHooker",
			json!({"m_FileID": 3, "m_PathID": 5}),
		);
		assert_rule(
			&rejected(&second_outside, None, &melee_facts()),
			"R6 reviewed edges",
		);

		let mut wrong_spine_field: FixturePrefab = melee_prefab();
		wrong_spine_field.set(
			SPINE,
			0,
			"separatorSlotNames",
			json!([{"m_FileID": 2, "m_PathID": 78}]),
		);
		let error: DecodeError = rejected(&wrong_spine_field, None, &melee_facts());
		assert!(
			matches!(&error, DecodeError::UnexpectedReference { field, .. }
				if field == "$.separatorSlotNames[0]"),
			"{error}"
		);

		let mut null_skeleton: FixturePrefab = melee_prefab();
		null_skeleton.set(SPINE, 0, "skeletonDataAsset", pptr(0));
		let error: DecodeError = rejected(&null_skeleton, None, &melee_facts());
		assert!(
			matches!(&error, DecodeError::UnexpectedReference { field, .. }
				if field == "$.skeletonDataAsset"),
			"{error}"
		);

		let mut disabled: FixturePrefab = melee_prefab();
		disabled.set(ATTACK, 0, "m_Enabled", json!(0));
		assert!(matches!(
			rejected(&disabled, None, &melee_facts()),
			DecodeError::DisabledBehaviour { .. }
		));

		let mut two_modes: FixturePrefab = melee_prefab();
		two_modes.set(
			ROOT,
			0,
			"_modes",
			json!([
				pptr(behaviour_path(DEFAULT, 0)),
				pptr(behaviour_path(DEFAULT, 0))
			]),
		);
		assert_rule(
			&rejected(&two_modes, None, &melee_facts()),
			"R6 reviewed edges",
		);

		let mut attack_without_trigger: FixturePrefab = mocock_prefab();
		attack_without_trigger.set(DEFAULT, 0, "_attackTrigger", pptr(0));
		assert!(matches!(
			rejected(&attack_without_trigger, Some(&paracurve()), &mocock_facts()),
			DecodeError::UnsupportedModeWiring { .. }
		));

		let mut no_range_extension: FixturePrefab = mocock_prefab();
		no_range_extension.nodes[TRIGGER].behaviours.pop();
		assert!(matches!(
			rejected(&no_range_extension, Some(&paracurve()), &mocock_facts()),
			DecodeError::OwnedSetMismatch { .. }
		));

		let mut locate_range: FixturePrefab = melee_prefab();
		locate_range.set(ROOT, 0, "_locateRange", pptr(transform_path(BODY)));
		assert_rule(
			&rejected(&locate_range, None, &melee_facts()),
			"R6 reviewed edges",
		);

		let mut fast_attack: FixturePrefab = melee_prefab();
		fast_attack.set(
			GRAPHIC,
			0,
			"_animations",
			json!([{"animKey": "Attack", "animName": "Attack", "ignoreMissing": 0, "loop": 0, "speed": 1.5}]),
		);
		assert_drift(&rejected(&fast_attack, None, &melee_facts()), "_animations");

		let mut delayed_effects: FixturePrefab = melee_prefab();
		delayed_effects.set(ATTACK, 1, "_preDelayFactor", json!(0.5));
		assert_drift(
			&rejected(&delayed_effects, None, &melee_facts()),
			"_preDelayFactor",
		);

		let mut other_revision: BattlePrefabGraph = paracurve().assemble();
		other_revision.mechanics_revision = "another-revision".to_owned();
		let error: DecodeError = decode_graphs(
			&mocock_prefab().assemble(),
			Some(other_revision),
			&mocock_facts(),
		)
		.expect_err("a projectile from another revision is rejected");
		assert!(
			matches!(innermost(&error), DecodeError::RevisionMismatch { .. }),
			"{error}"
		);

		let graph: BattlePrefabGraph = melee_prefab().assemble();
		let unused =
			|_: &str| -> Result<(BattlePrefabContainerMatch, BattlePrefabGraph), DecodeError> {
				panic!("a melee prefab resolves no projectile")
			};
		let mut charpack: BattlePrefabContainerMatch =
			container(&graph, BattlePrefabEntryKind::Charpack);
		assert!(matches!(
			decode_plain_enemy_graphs(&charpack, &graph, &unused, &melee_facts()),
			Err(DecodeError::EnemyPrefabOutsideEnemypack { .. })
		));
		charpack.bundle_kind = BattlePrefabEntryKind::Enemypack;
		charpack.container_file_id = 1;
		assert!(matches!(
			decode_plain_enemy_graphs(&charpack, &graph, &unused, &melee_facts()),
			Err(DecodeError::CrossCabContainer { .. })
		));
	}

	fn melee_seam(
		location: &BattlePrefabContainerMatch,
		graph: &BattlePrefabGraph,
	) -> Result<DecodedPlainEnemyPrefab, DecodeError> {
		let unused =
			|_: &str| -> Result<(BattlePrefabContainerMatch, BattlePrefabGraph), DecodeError> {
				panic!("a melee prefab resolves no projectile")
			};
		decode_plain_enemy_graphs(location, graph, &unused, &melee_facts())
	}

	fn assert_location_mismatch(result: Result<DecodedPlainEnemyPrefab, DecodeError>, rule: &str) {
		let error: DecodeError = result.expect_err("the location is rejected");
		assert!(
			matches!(innermost(&error), DecodeError::LocationMismatch { rule: found, .. }
				if *found == rule),
			"{error}"
		);
	}

	#[test]
	fn locator_and_root_mismatches_are_rejected() {
		const ENEMY_ROOT: &str = "R2 enemy prefab root";
		let graph: BattlePrefabGraph = melee_prefab().assemble();
		let location: BattlePrefabContainerMatch =
			container(&graph, BattlePrefabEntryKind::Enemypack);
		melee_seam(&location, &graph).expect("the reviewed melee graph decodes");

		let mut renamed_root: BattlePrefabGraph = graph.clone();
		renamed_root.game_objects[ROOT].name = "enemy_other".to_owned();
		assert_location_mismatch(melee_seam(&location, &renamed_root), ENEMY_ROOT);

		let mut renamed_prefab: BattlePrefabGraph = graph.clone();
		renamed_prefab.prefab_name = "enemy_other".to_owned();
		assert_location_mismatch(melee_seam(&location, &renamed_prefab), ENEMY_ROOT);

		let mut two_roots: BattlePrefabGraph = graph.clone();
		two_roots.root_game_object_indices.push(SHADOW);
		assert_location_mismatch(melee_seam(&location, &two_roots), ENEMY_ROOT);

		let mut talents: BattlePrefabGraph = graph.clone();
		talents.section = BattlePrefabRootSection::Talents;
		assert_location_mismatch(melee_seam(&location, &talents), ENEMY_ROOT);

		let mut other_root: BattlePrefabContainerMatch = location.clone();
		other_root.root = graph.game_objects[SHADOW].source.clone();
		assert_location_mismatch(melee_seam(&other_root, &graph), ENEMY_ROOT);

		let mut other_bundle: BattlePrefabContainerMatch = location.clone();
		other_bundle.logical_path = "enm_pfb_other".to_owned();
		assert_location_mismatch(melee_seam(&other_bundle, &graph), ENEMY_ROOT);

		let found: BattlePrefabContainerMatch = location.clone();
		for (kind, not_found, ambiguous) in [
			(LocatedPrefab::Enemy, "R1 locate", "R1 locate"),
			(
				LocatedPrefab::Projectile,
				"R16 projectile locate",
				"R16 projectile locate",
			),
		] {
			assert_eq!(
				single_container(kind, "path".to_owned(), vec![found.clone()]),
				Ok(found.clone())
			);
			let missing: DecodeError = single_container(kind, "path".to_owned(), Vec::new())
				.expect_err("no match is rejected");
			assert!(matches!(
				(&missing, kind),
				(
					DecodeError::EnemyPrefabNotFound { .. },
					LocatedPrefab::Enemy
				) | (
					DecodeError::ProjectileNotFound { .. },
					LocatedPrefab::Projectile
				)
			));
			assert_eq!(missing.rule(), not_found);
			let two: DecodeError =
				single_container(kind, "path".to_owned(), vec![found.clone(), found.clone()])
					.expect_err("two packs are rejected");
			assert!(matches!(
				(&two, kind),
				(DecodeError::AmbiguousEnemyPrefab { matches, .. }, LocatedPrefab::Enemy)
					| (DecodeError::AmbiguousProjectile { matches, .. }, LocatedPrefab::Projectile)
					if matches.len() == 2
			));
			assert_eq!(two.rule(), ambiguous);
		}

		check_revision(REVISION, &graph).expect("the snapshot revision is accepted");
		assert!(matches!(
			check_revision("another-revision", &graph),
			Err(DecodeError::RevisionMismatch { ref expected, ref found })
				if expected == "another-revision" && found == REVISION
		));
	}

	fn decode_with_projectile_location(adjust: fn(&mut BattlePrefabContainerMatch)) -> DecodeError {
		let enemy: BattlePrefabGraph = mocock_prefab().assemble();
		let location: BattlePrefabContainerMatch =
			container(&enemy, BattlePrefabEntryKind::Enemypack);
		let resolve =
			|_: &str| -> Result<(BattlePrefabContainerMatch, BattlePrefabGraph), DecodeError> {
				let graph: BattlePrefabGraph = paracurve().assemble();
				let mut projectile: BattlePrefabContainerMatch =
					container(&graph, BattlePrefabEntryKind::Common);
				adjust(&mut projectile);
				Ok((projectile, graph))
			};
		let error: DecodeError =
			decode_plain_enemy_graphs(&location, &enemy, &resolve, &mocock_facts())
				.expect_err("the projectile location is rejected");
		assert!(matches!(error, DecodeError::Projectile { .. }), "{error}");
		error
	}

	#[test]
	fn projectile_locations_outside_the_common_projectile_bundle_are_rejected() {
		for adjust in [
			(|location: &mut BattlePrefabContainerMatch| {
				location.bundle_kind = BattlePrefabEntryKind::Charpack;
			}) as fn(&mut BattlePrefabContainerMatch),
			|location: &mut BattlePrefabContainerMatch| {
				location.logical_path = "[uc]other".to_owned();
			},
		] {
			let error: DecodeError = decode_with_projectile_location(adjust);
			assert!(
				matches!(
					innermost(&error),
					DecodeError::ProjectileOutsideProjectileBundle { .. }
				),
				"{error}"
			);
		}
		let cross_cab: DecodeError =
			decode_with_projectile_location(|location: &mut BattlePrefabContainerMatch| {
				location.container_file_id = 1;
			});
		assert_location_mismatch(Err(cross_cab), "R16 projectile locate");
		let other_root: DecodeError =
			decode_with_projectile_location(|location: &mut BattlePrefabContainerMatch| {
				location.root.path_id = transform_path(ROOT);
			});
		assert_location_mismatch(Err(other_root), "R16 projectile structure");
	}

	#[test]
	fn projectile_structure_outside_the_reviewed_root_is_rejected() {
		let mut unknown_on_root: FixturePrefab = paracurve();
		unknown_on_root.nodes[ROOT]
			.behaviours
			.push(unknown_behaviour());
		let error: DecodeError =
			rejected(&mocock_prefab(), Some(&unknown_on_root), &mocock_facts());
		assert!(
			matches!(innermost(&error), DecodeError::OwnedSetMismatch { game_object, .. }
				if game_object.path_id == go_path(ROOT)),
			"{error}"
		);

		let mut carrying_child: FixturePrefab = paracurve();
		carrying_child
			.nodes
			.push(node("Trail", Some(ROOT), vec![unknown_behaviour()]));
		let error: DecodeError = rejected(&mocock_prefab(), Some(&carrying_child), &mocock_facts());
		assert!(
			matches!(innermost(&error), DecodeError::OwnedSetMismatch { game_object, expected, .. }
				if game_object.path_id == go_path(1) && expected.contains("below the projectile root")),
			"{error}"
		);
	}

	#[test]
	fn facts_other_than_the_melee_attack_ability_are_rejected() {
		let melee: BattlePrefabGraph = melee_prefab().assemble();
		let attack: usize = melee.game_objects[ATTACK].mono_behaviour_indices[0];
		let shadow: usize = melee.game_objects[SHADOW].mono_behaviour_indices[0];

		let mut duplicated: BattlePrefabGraph = melee.clone();
		let fact: BattlePrefabSemanticFact =
			duplicated.mono_behaviours[attack].semantic_facts[0].clone();
		duplicated.mono_behaviours[attack]
			.semantic_facts
			.push(fact.clone());
		assert!(matches!(
			decode_graphs(&duplicated, None, &melee_facts()),
			Err(DecodeError::UnexpectedFacts { ref source, .. })
				if **source == melee.mono_behaviours[attack].source
		));

		let mut disagreeing: BattlePrefabGraph = melee.clone();
		let mut drifted: Value = disagreeing.mono_behaviours[attack].raw_fields.clone();
		drifted["_damageType"] = json!(2);
		disagreeing.mono_behaviours[attack].semantic_facts[0].semantic =
			BattlePrefabSemantic::AttackAbility(
				attack_ability_from_raw(&drifted).expect("the drifted fields still decode"),
			);
		assert_ne!(
			disagreeing.mono_behaviours[attack].semantic_facts,
			melee.mono_behaviours[attack].semantic_facts
		);
		assert!(matches!(
			decode_graphs(&disagreeing, None, &melee_facts()),
			Err(DecodeError::UnexpectedFacts { .. })
		));

		let mut foreign: BattlePrefabGraph = melee.clone();
		foreign.mono_behaviours[shadow].semantic_facts.push(fact);
		assert!(matches!(
			decode_graphs(&foreign, None, &melee_facts()),
			Err(DecodeError::UnexpectedFacts { ref source, .. })
				if **source == melee.mono_behaviours[shadow].source
		));

		let mut ranged_fact: BattlePrefabGraph = mocock_prefab().assemble();
		let ranged_attack: usize = ranged_fact.game_objects[ATTACK].mono_behaviour_indices[0];
		ranged_fact.mono_behaviours[ranged_attack]
			.semantic_facts
			.push(melee.mono_behaviours[attack].semantic_facts[0].clone());
		assert!(matches!(
			decode_graphs(&ranged_fact, Some(paracurve().assemble()), &mocock_facts()),
			Err(DecodeError::UnexpectedFacts { .. })
		));

		let mut buffed: BattlePrefabGraph = melee.clone();
		buffed.mono_behaviours[shadow]
			.buffs
			.push(BattlePrefabBuffSpec {
				field_path: "$._effects[0]".to_owned(),
				buff_key: Some("burn".to_owned()),
				template_key: None,
				attribute_modifiers: vec![json!({"attributeType": 1})],
			});
		assert!(matches!(
			decode_graphs(&buffed, None, &melee_facts()),
			Err(DecodeError::UnexpectedFacts { ref found, .. }) if found == "$._effects[0]"
		));
	}

	#[test]
	fn table_disagreement_is_rejected() {
		let mut physical: PlainEnemyTableFacts = mocock_facts();
		physical.damage = ReviewedEnemyDamageType::Physical;
		let mut blocked: PlainEnemyTableFacts = melee_facts();
		blocked.block_demand = 2;
		let cases: [(
			FixturePrefab,
			Option<FixturePrefab>,
			PlainEnemyTableFacts,
			&str,
		); 7] = [
			(
				mocock_prefab(),
				Some(paracurve()),
				melee_facts(),
				"apply way",
			),
			(
				melee_prefab(),
				None,
				table_facts(Some("RANGED"), None, ReviewedEnemyDamageType::Physical),
				"apply way",
			),
			(
				melee_prefab(),
				None,
				table_facts(None, None, ReviewedEnemyDamageType::Physical),
				"apply way",
			),
			(
				mocock_prefab(),
				Some(paracurve()),
				table_facts(Some("RANGED"), None, ReviewedEnemyDamageType::Physical),
				"range radius",
			),
			(
				melee_prefab(),
				None,
				table_facts(Some("MELEE"), Some(1.5), ReviewedEnemyDamageType::Physical),
				"range radius",
			),
			(wizard_prefab(), Some(linear()), physical, "damage family"),
			(melee_prefab(), None, blocked, "block demand"),
		];
		for (enemy, projectile, facts, check_name) in cases {
			let error: DecodeError = rejected(&enemy, projectile.as_ref(), &facts);
			assert!(
				matches!(&error, DecodeError::TableDisagreement { check, .. } if check.contains(check_name)),
				"{check_name}: {error}"
			);
		}

		let mut unknown_family: FixturePrefab = melee_prefab();
		unknown_family.set(ATTACK, 0, "_damageType", json!(3));
		assert_drift(
			&rejected(&unknown_family, None, &melee_facts()),
			"_damageType",
		);
	}

	fn manifest_entry(bundle_kind: BattlePrefabEntryKind) -> BattlePrefabManifestEntry {
		BattlePrefabManifestEntry {
			logical_path: "fixture".to_owned(),
			bundle_kind,
			source_apk_path: "fixture".to_owned(),
			source_sha256: "0".repeat(64),
			source_byte_length: 1,
			export_path: "fixture.json".to_owned(),
			export_sha256: "0".repeat(64),
			export_byte_length: 1,
		}
	}

	#[test]
	fn only_a_manifest_without_enemy_packs_has_no_enemy_prefab_evidence() {
		assert!(!pins_enemy_packs(&[]));
		assert!(!pins_enemy_packs(&[
			manifest_entry(BattlePrefabEntryKind::Common),
			manifest_entry(BattlePrefabEntryKind::Charpack),
		]));
		assert!(pins_enemy_packs(&[
			manifest_entry(BattlePrefabEntryKind::Common),
			manifest_entry(BattlePrefabEntryKind::Enemypack),
		]));
	}

	fn serialized(unity_type: &str, fields: Value) -> BattlePrefabObject {
		BattlePrefabObject {
			path_id: 0,
			class_id: 0,
			type_index: 0,
			unity_type: unity_type.to_owned(),
			byte_start: 0,
			byte_size: 0,
			script_class: None,
			fields,
		}
	}

	#[test]
	fn inactive_game_objects_and_non_transform_targets_are_rejected() {
		let graph: BattlePrefabGraph = mocock_prefab().assemble();
		let active: BattlePrefabObject = serialized("GameObject", json!({"m_IsActive": true}));
		let inactive: BattlePrefabObject = serialized("GameObject", json!({"m_IsActive": false}));
		let transform: BattlePrefabObject = serialized("Transform", json!({}));
		let renderer: BattlePrefabObject = serialized("MeshRenderer", json!({}));
		let trigger_source: BattlePrefabObjectSource = graph.game_objects[TRIGGER].source.clone();

		let accepted = |source: &BattlePrefabObjectSource| -> Option<&BattlePrefabObject> {
			Some(if source.path_id % 100 == 0 {
				&active
			} else {
				&transform
			})
		};
		check_serialized_objects(&graph, &accepted).expect("an active graph is accepted");

		let inactive_trigger = |source: &BattlePrefabObjectSource| -> Option<&BattlePrefabObject> {
			Some(match source.path_id % 100 {
				0 if *source == trigger_source => &inactive,
				0 => &active,
				_ => &transform,
			})
		};
		assert!(matches!(
			check_serialized_objects(&graph, &inactive_trigger),
			Err(DecodeError::InactiveGameObject { ref source, .. }) if **source == trigger_source
		));

		let renderer_targets = |source: &BattlePrefabObjectSource| -> Option<&BattlePrefabObject> {
			Some(if source.path_id % 100 == 0 {
				&active
			} else {
				&renderer
			})
		};
		assert_rule(
			&check_serialized_objects(&graph, &renderer_targets)
				.expect_err("a transform edge onto a renderer is rejected"),
			"R6 reviewed edges",
		);
	}
}
