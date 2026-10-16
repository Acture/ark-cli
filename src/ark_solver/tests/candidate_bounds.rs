use ark_solver::data::ResolvedOperatorBuild;
use ark_solver::planning::{compile_candidate_bounds, CandidateError, ReplacementCandidateBounds};

#[test]
fn resolved_build_adapter_has_the_public_fail_closed_signature() {
	let adapter: fn(&ResolvedOperatorBuild) -> Result<ReplacementCandidateBounds, CandidateError> =
		compile_candidate_bounds;
	let _adapter = adapter;
}
