use crate::simulation::result::CHOICE_MASS_TOLERANCE;
use crate::simulation::{ChoiceId, ChoiceSelection, SimulationIssue, SimulationSeed, Weighted};

pub(in crate::simulation) enum ChoiceSource<'a> {
	Seed(StableRng),
	Tape {
		entries: &'a [ChoiceSelection],
		cursor: usize,
	},
}

impl ChoiceSource<'_> {
	pub(in crate::simulation) fn select<T>(
		&mut self,
		choice_id: ChoiceId,
		branches: &[Weighted<T>],
	) -> Result<usize, SimulationIssue> {
		let mass = branches
			.iter()
			.try_fold(0.0, |sum, branch| {
				let next = sum + branch.probability();
				next.is_finite().then_some(next)
			})
			.ok_or(SimulationIssue::NonFiniteProbability { choice_id })?;
		if (mass - 1.0).abs() > CHOICE_MASS_TOLERANCE {
			return Err(SimulationIssue::InvalidChoiceMass { choice_id, mass });
		}

		match self {
			Self::Seed(rng) => select_seeded(rng, choice_id, branches),
			Self::Tape { entries, cursor } => {
				select_from_tape(entries, cursor, choice_id, branches)
			}
		}
	}

	pub(in crate::simulation) fn finished_issue(&self) -> Option<SimulationIssue> {
		match self {
			Self::Seed(_) => None,
			Self::Tape { entries, cursor } => {
				let count = entries.len().saturating_sub(*cursor);
				(count != 0).then_some(SimulationIssue::UnusedTapeEntries { count })
			}
		}
	}
}

fn select_seeded<T>(
	rng: &mut StableRng,
	choice_id: ChoiceId,
	branches: &[Weighted<T>],
) -> Result<usize, SimulationIssue> {
	let sample = rng.next_unit_f64();
	let mut cumulative = 0.0;
	for (index, branch) in branches.iter().enumerate() {
		cumulative += branch.probability();
		if sample < cumulative {
			return Ok(index);
		}
	}
	Err(SimulationIssue::MissingChoiceMass { choice_id })
}

fn select_from_tape<T>(
	entries: &[ChoiceSelection],
	cursor: &mut usize,
	choice_id: ChoiceId,
	branches: &[Weighted<T>],
) -> Result<usize, SimulationIssue> {
	let Some(selection) = entries.get(*cursor) else {
		return Err(SimulationIssue::MissingTapeEntry { choice_id });
	};
	if selection.choice_id != choice_id {
		return Err(SimulationIssue::TapeChoiceMismatch {
			expected: choice_id,
			actual: selection.choice_id,
		});
	}
	let Some(branch) = branches.get(selection.branch_index) else {
		return Err(SimulationIssue::TapeBranchOutOfRange {
			choice_id,
			branch_index: selection.branch_index,
		});
	};
	if branch.probability() == 0.0 {
		return Err(SimulationIssue::ImpossibleTapeBranch {
			choice_id,
			branch_index: selection.branch_index,
		});
	}
	*cursor += 1;
	Ok(selection.branch_index)
}

pub(in crate::simulation) struct StableRng {
	state: u64,
}

impl StableRng {
	pub(in crate::simulation) const fn new(seed: SimulationSeed) -> Self {
		Self { state: seed.0 }
	}

	fn next_unit_f64(&mut self) -> f64 {
		self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
		let mut value = self.state;
		value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
		value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
		value ^= value >> 31;
		let mantissa = value >> 11;
		(mantissa as f64) * (1.0 / ((1_u64 << 53) as f64))
	}
}
