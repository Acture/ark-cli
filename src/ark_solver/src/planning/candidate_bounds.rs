use crate::data::ResolvedOperatorBuild;

use super::envelope::{CandidateError, ReplacementCandidateBounds};

/// Compiles the static, proof-safe subset of a resolved operator build into pruning bounds.
///
/// A resolved build proves only its revision-bearing identity for hard-pruning purposes. Its base
/// deployment cost is not a certified lower bound after potential, talent, stage, squad, or relic
/// discounts. Its range ID alone does not prove a targetable-cell upper bound without deployment
/// position, facing, and range geometry. Character-table base attack and interval also are not
/// upper/lower bounds across talents, skills, trust, modules, summons, and battle effects.
/// Therefore every optional pruning bound remains unknown.
pub fn compile_candidate_bounds(
	build: &ResolvedOperatorBuild,
) -> Result<ReplacementCandidateBounds, CandidateError> {
	compile_static_candidate_bounds(build.stable_id())
}

pub(crate) fn compile_static_candidate_bounds(
	stable_id: &str,
) -> Result<ReplacementCandidateBounds, CandidateError> {
	ReplacementCandidateBounds::new(stable_id.to_owned(), None, None, None, None)
}

#[cfg(test)]
mod tests {
	use super::compile_static_candidate_bounds;

	#[test]
	fn compiles_only_the_proven_revision_bearing_identity() {
		let stable_id = "0123456789abcdef:char_test:e2:l90:s3:sl10:p0:m-none:ml0";
		let bounds = compile_static_candidate_bounds(stable_id).unwrap();

		assert_eq!(bounds.stable_id(), stable_id);
		assert_eq!(bounds.minimum_deployment_cost(), None);
		assert_eq!(bounds.targetable_cells_upper_bound(), None);
		assert_eq!(bounds.optimistic_damage_per_attack_upper_bound(), None);
		assert_eq!(bounds.minimum_attack_interval_ticks(), None);
	}
}
