//! Batch compilation of exact operator builds into independently reviewable mechanic projections.

use crate::data::ResolvedOperatorBuild;
use crate::simulation::TickRate;

use crate::mechanics::{
	mechanic_templates_phalanx::{
		try_decode_phalanx_trait, DecodedPhalanxTrait, PhalanxTraitDecodeError,
	},
	normal_attack_catalog::{
		compile_base_panel_normal_attack_facts, BasePanelNormalAttackFacts,
		NormalAttackCatalogEntry,
	},
	operator_mechanic_overrides::{
		review_operator_mechanic_override, OperatorMechanicOverrideEntry,
	},
	skill_damage::{
		compile_expected_damage_skill, ExpectedDamageSkillEntry, SkillDamageUnsupportedReason,
	},
};

#[derive(Clone, Copy, Debug)]
pub struct MechanicCompileRequest<'a> {
	builds: &'a [ResolvedOperatorBuild],
	tick_rate: TickRate,
}

impl<'a> MechanicCompileRequest<'a> {
	#[must_use]
	pub const fn new(builds: &'a [ResolvedOperatorBuild], tick_rate: TickRate) -> Self {
		Self { builds, tick_rate }
	}

	#[must_use]
	pub const fn builds(self) -> &'a [ResolvedOperatorBuild] {
		self.builds
	}

	#[must_use]
	pub const fn tick_rate(self) -> TickRate {
		self.tick_rate
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MechanicCompileComponent {
	NormalAttack,
	SkillDamage,
	PhalanxTrait,
	OperatorOverride,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompileSupportDisposition {
	Supported,
	Unmodeled,
	Unsupported,
	NotApplicable,
}

/// Structured support evidence; unsupported and unmodeled paths remain distinct.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompileSupportEvidence {
	component: MechanicCompileComponent,
	disposition: CompileSupportDisposition,
	reasons: Vec<String>,
}

impl CompileSupportEvidence {
	#[must_use]
	pub const fn component(&self) -> MechanicCompileComponent {
		self.component
	}

	#[must_use]
	pub const fn disposition(&self) -> CompileSupportDisposition {
		self.disposition
	}

	#[must_use]
	pub fn reasons(&self) -> &[String] {
		&self.reasons
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhalanxTraitCompileEntry {
	Supported(Box<DecodedPhalanxTrait>),
	NotApplicable,
	Unsupported(PhalanxTraitDecodeError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperatorMechanicCompileResult {
	stable_id: String,
	operator_id: String,
	normal_attack: NormalAttackCatalogEntry,
	skill_damage: ExpectedDamageSkillEntry,
	phalanx_trait: PhalanxTraitCompileEntry,
	operator_override: OperatorMechanicOverrideEntry,
	support: Vec<CompileSupportEvidence>,
}

impl OperatorMechanicCompileResult {
	#[must_use]
	pub fn stable_id(&self) -> &str {
		&self.stable_id
	}

	#[must_use]
	pub fn operator_id(&self) -> &str {
		&self.operator_id
	}

	#[must_use]
	pub const fn normal_attack(&self) -> &NormalAttackCatalogEntry {
		&self.normal_attack
	}

	#[must_use]
	pub const fn skill_damage(&self) -> &ExpectedDamageSkillEntry {
		&self.skill_damage
	}

	#[must_use]
	pub const fn phalanx_trait(&self) -> &PhalanxTraitCompileEntry {
		&self.phalanx_trait
	}

	#[must_use]
	pub const fn operator_override(&self) -> &OperatorMechanicOverrideEntry {
		&self.operator_override
	}

	#[must_use]
	pub fn support(&self) -> &[CompileSupportEvidence] {
		&self.support
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MechanicCompileResult {
	operators: Vec<OperatorMechanicCompileResult>,
}

impl MechanicCompileResult {
	#[must_use]
	pub fn operators(&self) -> &[OperatorMechanicCompileResult] {
		&self.operators
	}

	#[must_use]
	pub fn into_operators(self) -> Vec<OperatorMechanicCompileResult> {
		self.operators
	}

	#[must_use]
	pub fn is_complete(&self) -> bool {
		self.operators.iter().all(|operator| {
			operator.support.iter().all(|evidence| {
				!matches!(
					evidence.disposition,
					CompileSupportDisposition::Unmodeled | CompileSupportDisposition::Unsupported
				)
			})
		})
	}
}

#[must_use]
pub fn compile_operator_mechanics(request: MechanicCompileRequest<'_>) -> MechanicCompileResult {
	let operators: Vec<OperatorMechanicCompileResult> = request
		.builds
		.iter()
		.map(|build: &ResolvedOperatorBuild| compile_operator(build, request.tick_rate))
		.collect();
	MechanicCompileResult { operators }
}

fn compile_operator(
	build: &ResolvedOperatorBuild,
	tick_rate: TickRate,
) -> OperatorMechanicCompileResult {
	let stats = build.static_stats();
	let normal_attack_facts: BasePanelNormalAttackFacts = BasePanelNormalAttackFacts::new(
		build.operator_id().to_owned(),
		build.profession().to_owned(),
		build.sub_profession_id().to_owned(),
		stats.attack(),
		stats.base_attack_time_seconds(),
		stats.attack_speed(),
	);
	let normal_attack: NormalAttackCatalogEntry =
		compile_base_panel_normal_attack_facts(&normal_attack_facts, tick_rate);
	let skill_damage: ExpectedDamageSkillEntry = compile_expected_damage_skill(build, tick_rate);
	let phalanx_trait: PhalanxTraitCompileEntry = match try_decode_phalanx_trait(build) {
		Ok(Some(decoded)) => PhalanxTraitCompileEntry::Supported(Box::new(decoded)),
		Ok(None) => PhalanxTraitCompileEntry::NotApplicable,
		Err(error) => PhalanxTraitCompileEntry::Unsupported(error),
	};
	let operator_override: OperatorMechanicOverrideEntry = review_operator_mechanic_override(build);
	let support: Vec<CompileSupportEvidence> = vec![
		normal_attack_support(&normal_attack),
		skill_damage_support(&skill_damage),
		phalanx_support(&phalanx_trait),
		override_support(&operator_override),
	];
	OperatorMechanicCompileResult {
		stable_id: build.stable_id().to_owned(),
		operator_id: build.operator_id().to_owned(),
		normal_attack,
		skill_damage,
		phalanx_trait,
		operator_override,
		support,
	}
}

fn normal_attack_support(entry: &NormalAttackCatalogEntry) -> CompileSupportEvidence {
	match entry {
		NormalAttackCatalogEntry::Supported(_) => supported(MechanicCompileComponent::NormalAttack),
		NormalAttackCatalogEntry::Unsupported(unsupported) => evidence(
			MechanicCompileComponent::NormalAttack,
			CompileSupportDisposition::Unsupported,
			vec![unsupported.reason().to_string()],
		),
	}
}

fn skill_damage_support(entry: &ExpectedDamageSkillEntry) -> CompileSupportEvidence {
	match entry {
		ExpectedDamageSkillEntry::Supported(_) => supported(MechanicCompileComponent::SkillDamage),
		ExpectedDamageSkillEntry::Unsupported(unsupported) => evidence(
			MechanicCompileComponent::SkillDamage,
			CompileSupportDisposition::Unsupported,
			unsupported
				.reasons()
				.iter()
				.map(SkillDamageUnsupportedReason::to_string)
				.collect(),
		),
	}
}

fn phalanx_support(entry: &PhalanxTraitCompileEntry) -> CompileSupportEvidence {
	match entry {
		PhalanxTraitCompileEntry::Supported(_) => supported(MechanicCompileComponent::PhalanxTrait),
		PhalanxTraitCompileEntry::NotApplicable => evidence(
			MechanicCompileComponent::PhalanxTrait,
			CompileSupportDisposition::NotApplicable,
			Vec::new(),
		),
		PhalanxTraitCompileEntry::Unsupported(error) => evidence(
			MechanicCompileComponent::PhalanxTrait,
			CompileSupportDisposition::Unsupported,
			vec![error.to_string()],
		),
	}
}

fn override_support(entry: &OperatorMechanicOverrideEntry) -> CompileSupportEvidence {
	match entry {
		OperatorMechanicOverrideEntry::Matched(_) => {
			supported(MechanicCompileComponent::OperatorOverride)
		}
		OperatorMechanicOverrideEntry::Absent => evidence(
			MechanicCompileComponent::OperatorOverride,
			CompileSupportDisposition::NotApplicable,
			Vec::new(),
		),
		OperatorMechanicOverrideEntry::Drift(error) => evidence(
			MechanicCompileComponent::OperatorOverride,
			CompileSupportDisposition::Unmodeled,
			vec![error.to_string()],
		),
	}
}

fn supported(component: MechanicCompileComponent) -> CompileSupportEvidence {
	evidence(component, CompileSupportDisposition::Supported, Vec::new())
}

fn evidence(
	component: MechanicCompileComponent,
	disposition: CompileSupportDisposition,
	reasons: Vec<String>,
) -> CompileSupportEvidence {
	CompileSupportEvidence {
		component,
		disposition,
		reasons,
	}
}

#[cfg(test)]
mod tests {
	use super::{CompileSupportDisposition, CompileSupportEvidence, MechanicCompileComponent};

	#[test]
	fn incomplete_evidence_distinguishes_unmodeled_from_unsupported() {
		let unmodeled: CompileSupportEvidence = CompileSupportEvidence {
			component: MechanicCompileComponent::SkillDamage,
			disposition: CompileSupportDisposition::Unmodeled,
			reasons: vec!["reviewed conservative omission".to_owned()],
		};
		let unsupported: CompileSupportEvidence = CompileSupportEvidence {
			component: MechanicCompileComponent::PhalanxTrait,
			disposition: CompileSupportDisposition::Unsupported,
			reasons: vec!["source shape drift".to_owned()],
		};
		assert_ne!(unmodeled.disposition(), unsupported.disposition());
		assert_eq!(unmodeled.reasons(), ["reviewed conservative omission"]);
		assert_eq!(unsupported.reasons(), ["source shape drift"]);
	}
}
