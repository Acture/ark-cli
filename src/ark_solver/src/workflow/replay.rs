//! In-memory replay workflow; filesystem layout and progress rendering stay in the CLI.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::num::NonZeroU32;

use crate::data::{ArkDataSnapshot, BattlePrefabSnapshot};
use crate::simulation::TickRate;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::strategy_binding::{
	enumerate_replay_guide_build_variants, prepare_replay_guide, replay_stage_difficulty,
};
use crate::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSeedSelection};
use crate::replay::{
	execute_expected_replay, materialize_expected_replay_input, ExpectedReplayAuditReport,
	ExpectedReplayCompileInput, ExpectedReplayReportStatus, PreparedReplayGuide,
};

pub const REPLAY_INDEX_SCHEMA_VERSION: u32 = 2;

/// Untrusted guide bytes plus explicit execution choices for standalone replay.
#[derive(Clone, Copy)]
pub struct ReplayRequest<'a> {
	pub snapshot: &'a ArkDataSnapshot,
	pub prefabs: &'a BattlePrefabSnapshot,
	pub guide_bytes: &'a [u8],
	pub difficulty: MaaJobDifficulty,
	pub ticks_per_second: u32,
}

/// One independently materialized replay variant ready for filesystem output.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayVariantOutput {
	pub stem: String,
	pub guide_variant_id: String,
	pub artifact_bytes: Vec<u8>,
	pub report: ExpectedReplayAuditReport,
}

/// Complete deterministic replay output and its serializable index.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayWorkflowOutput {
	pub index: ReplayIndex,
	pub variants: Vec<ReplayVariantOutput>,
}

/// Serializable index for all replay variants.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReplayIndex {
	pub schema_version: u32,
	pub data_revision: String,
	pub mechanics_revision: String,
	pub guide_sha256: String,
	pub entries: Vec<ReplayIndexEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReplayIndexEntry {
	pub guide_variant_id: String,
	pub status: ExpectedReplayReportStatus,
	pub artifact: String,
	pub report: String,
	pub unsupported_count: usize,
}

/// Parses one guide and independently replays every exact build variant in source order.
pub fn replay(request: ReplayRequest<'_>) -> Result<ReplayWorkflowOutput, ReplayWorkflowError> {
	request
		.prefabs
		.ensure_compatible_with(request.snapshot)
		.map_err(|error| {
			ReplayWorkflowError::new("snapshot_revision_mismatch", error.to_string())
		})?;
	if request.difficulty == MaaJobDifficulty::Unspecified {
		return Err(ReplayWorkflowError::new(
			"explicit_difficulty_required",
			"standalone replay requires Normal or Challenge difficulty",
		));
	}
	let ticks_per_second: NonZeroU32 =
		NonZeroU32::new(request.ticks_per_second).ok_or_else(|| {
			ReplayWorkflowError::new("zero_tick_rate", "ticks_per_second must be positive")
		})?;
	let guide_json: &str = std::str::from_utf8(request.guide_bytes).map_err(|error| {
		ReplayWorkflowError::new("guide_not_utf8", format!("guide is not UTF-8: {error}"))
	})?;
	let guide = parse_maa_job(guide_json)
		.map_err(|error| ReplayWorkflowError::new("guide_import_failed", error.to_string()))?;
	let guide_sha256: [u8; 32] = Sha256::digest(request.guide_bytes).into();
	let selections = guide
		.executable_seed_selections(request.difficulty, usize::MAX)
		.map_err(|error| ReplayWorkflowError::new("guide_selection_failed", error.to_string()))?;
	let tick_rate: TickRate = TickRate::new(ticks_per_second);
	let mut variants: Vec<ReplayVariantOutput> = Vec::new();
	let mut entries: Vec<ReplayIndexEntry> = Vec::new();
	for (selection_index, selection) in selections.enumerate() {
		let seed = MaaJobSeedSelection::materialize(selection);
		let build_variants = enumerate_replay_guide_build_variants(request.snapshot, &seed)
			.map_err(|error| {
				ReplayWorkflowError::new("build_variant_enumeration_failed", error.to_string())
			})?;
		for (variant_index, variant) in build_variants.into_iter().enumerate() {
			let guide_variant_id: String =
				format!("selection-{selection_index}:{}", variant.variant_id);
			let prepared: PreparedReplayGuide =
				prepare_replay_guide(request.snapshot, &seed, &variant.bindings).map_err(
					|error| {
						ReplayWorkflowError::new(
							"guide_preparation_failed",
							format!("failed to prepare {guide_variant_id}: {error}"),
						)
					},
				)?;
			let input = materialize_expected_replay_input(
				request.snapshot,
				request.prefabs,
				ExpectedReplayCompileInput {
					stage_name: seed.stage_name(),
					difficulty: replay_stage_difficulty(seed.difficulty()).map_err(|error| {
						ReplayWorkflowError::new("difficulty_mapping_failed", error.to_string())
					})?,
					bindings: &variant.bindings,
					guide: &prepared,
					tick_rate,
				},
			)
			.map_err(|error| {
				ReplayWorkflowError::new(
					"replay_materialization_failed",
					format!("failed to materialize {guide_variant_id}: {error}"),
				)
			})?;
			let output = execute_expected_replay(&input, guide_sha256, &guide_variant_id);
			let stem: String = format!("selection-{selection_index:03}-variant-{variant_index:03}");
			let artifact_name: String = format!("{stem}.rgs");
			let report_name: String = format!("{stem}.json");
			let artifact_bytes: Vec<u8> = output.artifact.encode().map_err(|error| {
				ReplayWorkflowError::new("artifact_encoding_failed", error.to_string())
			})?;
			entries.push(ReplayIndexEntry {
				guide_variant_id: guide_variant_id.clone(),
				status: output.report.status.clone(),
				artifact: artifact_name,
				report: report_name,
				unsupported_count: output.report.unsupported.len(),
			});
			variants.push(ReplayVariantOutput {
				stem,
				guide_variant_id,
				artifact_bytes,
				report: output.report,
			});
		}
	}
	Ok(ReplayWorkflowOutput {
		index: ReplayIndex {
			schema_version: REPLAY_INDEX_SCHEMA_VERSION,
			data_revision: request.snapshot.resolved_revision().to_owned(),
			mechanics_revision: request.prefabs.mechanics_revision().to_owned(),
			guide_sha256: hex_digest(guide_sha256),
			entries,
		},
		variants,
	})
}

fn hex_digest(digest: [u8; 32]) -> String {
	digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayWorkflowError {
	code: &'static str,
	detail: String,
}

impl ReplayWorkflowError {
	fn new(code: &'static str, detail: impl Into<String>) -> Self {
		Self {
			code,
			detail: detail.into(),
		}
	}

	#[must_use]
	pub const fn code(&self) -> &'static str {
		self.code
	}

	#[must_use]
	pub fn detail(&self) -> &str {
		&self.detail
	}
}

impl Display for ReplayWorkflowError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		write!(formatter, "{}: {}", self.code, self.detail)
	}
}

impl Error for ReplayWorkflowError {}

#[cfg(test)]
mod tests {
	use crate::maa_job::MaaJobDifficulty;

	#[test]
	fn replay_request_requires_an_explicit_difficulty_type() {
		assert_ne!(MaaJobDifficulty::Normal, MaaJobDifficulty::Unspecified);
		assert_ne!(MaaJobDifficulty::Challenge, MaaJobDifficulty::Unspecified);
	}
}
