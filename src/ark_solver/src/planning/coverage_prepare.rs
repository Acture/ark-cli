use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::data::{
	ArkDataError, ArkDataSnapshot, BasePanelBuild, Buildable, OperatorIdentity, OperatorPosition,
	ResolvedBasePanelBuild,
};

use super::query::{BasePanelBuildSpec, CandidatePoolSpec, CatalogPreset};

pub(crate) fn resolve_build_pool(
	snapshot: &ArkDataSnapshot,
	pool: &CandidatePoolSpec,
) -> Result<Vec<ResolvedBasePanelBuild>, CoveragePreparationError> {
	let catalog = snapshot.operators();
	let mut builds: Vec<ResolvedBasePanelBuild> = Vec::new();
	match pool {
		CandidatePoolSpec::Roster { builds: requested } => {
			for spec in requested {
				let build: BasePanelBuild = build_from_spec(spec);
				builds.push(
					catalog
						.resolve_base_panel(&build)
						.map_err(CoveragePreparationError::ArkData)?,
				);
			}
		}
		CandidatePoolSpec::CatalogPreset {
			preset: CatalogPreset::HighestPhaseBasePanel,
		} => {
			for operator_id in catalog.operator_ids() {
				let build: ResolvedBasePanelBuild = catalog
					.highest_phase_base_panel(operator_id)
					.and_then(|build| catalog.resolve_base_panel(&build))
					.map_err(CoveragePreparationError::ArkData)?;
				builds.push(build);
			}
		}
	}
	builds.sort_by(|left, right| left.stable_id().cmp(right.stable_id()));
	Ok(builds)
}

fn build_from_spec(spec: &BasePanelBuildSpec) -> BasePanelBuild {
	BasePanelBuild::new(
		OperatorIdentity::Id(spec.operator_id.clone()),
		spec.elite,
		spec.level,
	)
}

pub(crate) fn deployment_allowed(position: &OperatorPosition, buildable: &Buildable) -> bool {
	matches!(
		(position, buildable),
		(OperatorPosition::Melee, Buildable::Melee | Buildable::All)
			| (OperatorPosition::Ranged, Buildable::Ranged | Buildable::All)
			| (
				OperatorPosition::All,
				Buildable::Melee | Buildable::Ranged | Buildable::All
			)
	)
}

#[derive(Debug)]
pub enum CoveragePreparationError {
	ArkData(ArkDataError),
}

impl Display for CoveragePreparationError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::ArkData(source) => {
				write!(formatter, "Arknights data resolution failed: {source}")
			}
		}
	}
}

impl Error for CoveragePreparationError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::ArkData(source) => Some(source),
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::data::{Buildable, OperatorPosition};

	use super::deployment_allowed;

	#[test]
	fn deployment_legality_respects_operator_and_tile_categories() {
		assert!(deployment_allowed(
			&OperatorPosition::Melee,
			&Buildable::Melee
		));
		assert!(deployment_allowed(
			&OperatorPosition::Ranged,
			&Buildable::All
		));
		assert!(deployment_allowed(
			&OperatorPosition::All,
			&Buildable::Ranged
		));
		assert!(!deployment_allowed(
			&OperatorPosition::Melee,
			&Buildable::Ranged
		));
		assert!(!deployment_allowed(
			&OperatorPosition::None,
			&Buildable::All
		));
		assert!(!deployment_allowed(
			&OperatorPosition::Unknown("future".to_owned()),
			&Buildable::All
		));
	}
}
