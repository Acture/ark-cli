//! Independent validation of untrusted exported strategy bytes.

use crate::data::{ArkDataSnapshot, BattlePrefabSnapshot};
use sha2::{Digest, Sha256};

use super::solve::{
	validate_expected_damage_export_rebuilt, LevelSolvePrepareError, RebuiltExportValidationRequest,
};
use crate::maa_job::MaaJobDifficulty;
use crate::verification_artifact::StrategyArtifact;

pub use super::solve::ExpectedDamageExportValidationReport;

/// Raw, provenance-bearing inputs for validating one exported strategy.
///
/// No planner verdict, prepared replay input, decoded artifact, or parsed MAA job file crosses this
/// interface. The workflow rebuilds all of them from bytes and pinned snapshots.
#[derive(Clone, Copy)]
pub struct ExportValidationRequest<'a> {
	pub snapshot: &'a ArkDataSnapshot,
	pub prefabs: &'a BattlePrefabSnapshot,
	pub requested_level: &'a str,
	pub source_guide_bytes: &'a [u8],
	pub source_guide_source_id: &'a str,
	pub maa_job_bytes: &'a [u8],
	pub artifact_bytes: &'a [u8],
	pub difficulty: MaaJobDifficulty,
	pub guide_selection_budget: usize,
}

/// Rebuilds and replays an export from untrusted serialized inputs.
pub fn validate_export(
	request: ExportValidationRequest<'_>,
) -> Result<ExpectedDamageExportValidationReport, LevelSolvePrepareError> {
	let source_guide_json: &str =
		std::str::from_utf8(request.source_guide_bytes).map_err(|error| {
			LevelSolvePrepareError::new(
				"source_guide_not_utf8",
				format!("source guide is not UTF-8: {error}"),
			)
		})?;
	let maa_job_json: &str = std::str::from_utf8(request.maa_job_bytes).map_err(|error| {
		LevelSolvePrepareError::new(
			"strategy_not_utf8",
			format!("exported strategy is not UTF-8: {error}"),
		)
	})?;
	let artifact: StrategyArtifact = decode_artifact(request.artifact_bytes)?;
	let source_guide_content_sha256: String =
		hex_digest(Sha256::digest(request.source_guide_bytes).into());

	validate_expected_damage_export_rebuilt(RebuiltExportValidationRequest {
		snapshot: request.snapshot,
		prefabs: request.prefabs,
		requested_level: request.requested_level,
		source_guide_json,
		source_guide_source_id: request.source_guide_source_id,
		source_guide_content_sha256: &source_guide_content_sha256,
		maa_job_json,
		artifact: &artifact,
		artifact_bytes: request.artifact_bytes,
		difficulty: request.difficulty,
		guide_selection_budget: request.guide_selection_budget,
	})
}

fn decode_artifact(bytes: &[u8]) -> Result<StrategyArtifact, LevelSolvePrepareError> {
	StrategyArtifact::decode(bytes).map_err(|error| {
		LevelSolvePrepareError::new(
			"artifact_decode_failed",
			format!("failed to decode strategy artifact: {error}"),
		)
	})
}

fn hex_digest(digest: [u8; 32]) -> String {
	digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
	use super::{
		decode_artifact, validate_export, ExpectedDamageExportValidationReport,
		ExportValidationRequest, LevelSolvePrepareError,
	};
	use crate::maa_job::MaaJobDifficulty;

	type ValidationSeam =
		for<'a> fn(
			ExportValidationRequest<'a>,
		) -> Result<ExpectedDamageExportValidationReport, LevelSolvePrepareError>;
	type RawInputProjection =
		for<'a> fn(ExportValidationRequest<'a>) -> (&'a [u8], &'a [u8], &'a [u8], MaaJobDifficulty);

	fn project_raw_inputs<'a>(
		request: ExportValidationRequest<'a>,
	) -> (&'a [u8], &'a [u8], &'a [u8], MaaJobDifficulty) {
		let ExportValidationRequest {
			snapshot: _,
			prefabs: _,
			requested_level: _,
			source_guide_bytes,
			source_guide_source_id: _,
			maa_job_bytes,
			artifact_bytes,
			difficulty,
			guide_selection_budget: _,
		} = request;
		(
			source_guide_bytes,
			maa_job_bytes,
			artifact_bytes,
			difficulty,
		)
	}

	#[test]
	fn validation_interface_accepts_only_serialized_strategy_inputs() {
		let _validation_seam: ValidationSeam = validate_export;
		let _raw_input_projection: RawInputProjection = project_raw_inputs;
	}

	#[test]
	fn validation_decodes_artifact_bytes_inside_the_trust_boundary() {
		let error = decode_artifact(b"not-a-flatbuffer")
			.expect_err("untrusted invalid bytes must not become an artifact object");
		assert_eq!(error.code(), "artifact_decode_failed");
	}
}
