//! Pure, cached propagation for bounded layered tile fields.
//!
//! A pulse snapshots its source cells before applying any mutations. Saturated source increments
//! contribute to eligible orthogonal neighbors, but every contribution is cached before it is
//! applied. Consequently, cells created or saturated by propagation cannot propagate again during
//! the same pulse.

use std::collections::{BTreeMap, BTreeSet};

/// Canonical order used when one source examines its orthogonal neighbors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OrthogonalDirection {
	Down,
	Left,
	Up,
	Right,
}

/// Four orthogonal neighbors with an explicit, domain-significant order.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OrderedOrthogonalNeighbors<Cell> {
	pub down: Option<Cell>,
	pub left: Option<Cell>,
	pub up: Option<Cell>,
	pub right: Option<Cell>,
}

impl<Cell> OrderedOrthogonalNeighbors<Cell> {
	pub fn iter(&self) -> impl Iterator<Item = (OrthogonalDirection, &Cell)> {
		[
			(OrthogonalDirection::Down, self.down.as_ref()),
			(OrthogonalDirection::Left, self.left.as_ref()),
			(OrthogonalDirection::Up, self.up.as_ref()),
			(OrthogonalDirection::Right, self.right.as_ref()),
		]
		.into_iter()
		.filter_map(|(direction, cell)| cell.map(|cell| (direction, cell)))
	}
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TilePersistence {
	Permanent,
	Temporary,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SourceRangeRelation {
	Inside,
	Outside,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LayeredTileCell {
	pub layers: u16,
	pub persistence: TilePersistence,
	pub source_range: SourceRangeRelation,
}

impl LayeredTileCell {
	#[must_use]
	pub const fn permanent(layers: u16, source_range: SourceRangeRelation) -> Self {
		Self {
			layers,
			persistence: TilePersistence::Permanent,
			source_range,
		}
	}

	#[must_use]
	pub const fn temporary(layers: u16, source_range: SourceRangeRelation) -> Self {
		Self {
			layers,
			persistence: TilePersistence::Temporary,
			source_range,
		}
	}

	#[must_use]
	pub const fn consumes_outside_temporary_budget(self) -> bool {
		matches!(self.persistence, TilePersistence::Temporary)
			&& matches!(self.source_range, SourceRangeRelation::Outside)
	}
}

/// Classification applied only when propagation creates a previously absent cell.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PropagationDestination {
	pub persistence: TilePersistence,
	pub source_range: SourceRangeRelation,
}

impl PropagationDestination {
	#[must_use]
	pub const fn permanent(source_range: SourceRangeRelation) -> Self {
		Self {
			persistence: TilePersistence::Permanent,
			source_range,
		}
	}

	#[must_use]
	pub const fn temporary(source_range: SourceRangeRelation) -> Self {
		Self {
			persistence: TilePersistence::Temporary,
			source_range,
		}
	}

	#[must_use]
	pub const fn consumes_outside_temporary_budget(self) -> bool {
		matches!(self.persistence, TilePersistence::Temporary)
			&& matches!(self.source_range, SourceRangeRelation::Outside)
	}

	#[must_use]
	const fn as_empty_cell(self) -> LayeredTileCell {
		LayeredTileCell {
			layers: 0,
			persistence: self.persistence,
			source_range: self.source_range,
		}
	}
}

/// Ordering for source blocks. Neighbor order inside each source remains Down/Left/Up/Right.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PulseSourceOrder<Cell> {
	Known(Vec<Cell>),
	Unknown,
}

/// How simultaneous contributions to one destination become requested layers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ContributionMergePolicy {
	/// One destination receives at most one layer per cached propagation batch.
	DeduplicateDestination,
	/// Every distinct saturated source contributes one layer.
	AccumulatePerSource,
	/// The evidence does not establish whether contributions deduplicate or accumulate.
	Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedTileLayerPulse<Cell> {
	pub cells: BTreeMap<Cell, LayeredTileCell>,
	/// Only listed cells are eligible propagation destinations.
	pub eligible_destinations: BTreeMap<Cell, PropagationDestination>,
	/// Required for every source that is already saturated at pulse start.
	pub neighbors_by_source: BTreeMap<Cell, OrderedOrthogonalNeighbors<Cell>>,
	pub maximum_layers: u16,
	/// Maximum currently existing temporary cells outside the source's current range.
	pub maximum_outside_temporary_cells: u16,
	pub source_order: PulseSourceOrder<Cell>,
	pub contribution_merge: ContributionMergePolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerMutationCause<Cell> {
	PulseIncrement,
	CachedPropagation { contributing_sources: Vec<Cell> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TileLayerEvent<Cell> {
	LayersAdded {
		cell: Cell,
		cause: LayerMutationCause<Cell>,
		previous_layers: u16,
		added_layers: u16,
		current_layers: u16,
	},
	/// An attempted layer could not be stored because the destination was at capacity.
	SaturatedAttempt {
		cell: Cell,
		cause: LayerMutationCause<Cell>,
		at_layers: u16,
		attempted_layers: usize,
	},
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregatedContribution<Cell> {
	pub destination: Cell,
	pub contributing_sources: Vec<Cell>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceOrderWitness<Cell> {
	pub source_order: Vec<Cell>,
	pub selected_new_outside_temporary_cells: BTreeSet<Cell>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TileLayerPropagationAmbiguity<Cell> {
	SourceOrderChangesBudgetSelection {
		remaining_budget: u16,
		first: SourceOrderWitness<Cell>,
		second: SourceOrderWitness<Cell>,
	},
	ContributionMergeUnknown {
		destinations: Vec<AggregatedContribution<Cell>>,
	},
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileLayerPropagationOutcome<Cell> {
	pub cells: BTreeMap<Cell, LayeredTileCell>,
	pub pulse_start_sources: Vec<Cell>,
	pub saturated_sources: Vec<Cell>,
	pub removed_zero_layer_temporary_cells: Vec<Cell>,
	pub selected_new_outside_temporary_cells: BTreeSet<Cell>,
	pub rejected_new_outside_temporary_cells: BTreeSet<Cell>,
	pub aggregated_contributions: Vec<AggregatedContribution<Cell>>,
	pub events: Vec<TileLayerEvent<Cell>>,
	pub outside_temporary_cells_before: u16,
	pub outside_temporary_cells_after: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TileLayerPropagationResolution<Cell> {
	Resolved(TileLayerPropagationOutcome<Cell>),
	Ambiguous(TileLayerPropagationAmbiguity<Cell>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TileLayerPropagationError<Cell> {
	MaximumLayersIsZero,
	CellExceedsMaximumLayers {
		cell: Cell,
		layers: u16,
		maximum_layers: u16,
	},
	OutsideTemporaryBudgetAlreadyExceeded {
		current: u16,
		maximum: u16,
	},
	MissingNeighborsForSaturatedSource {
		source: Cell,
	},
	DuplicateOrthogonalNeighbor {
		source: Cell,
		destination: Cell,
	},
	KnownSourceOrderDoesNotMatchSnapshot {
		expected_sources: BTreeSet<Cell>,
		provided_sources: Vec<Cell>,
	},
	CellCountExceedsU16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EligibleEdge<Cell> {
	destination: Cell,
	profile: PropagationDestination,
}

/// Resolve one atomic layer-generation and cached-propagation pulse.
///
/// Temporary zero-layer cells are removed before the source snapshot and therefore release their
/// outside-range budget immediately. Permanent zero-layer cells remain and receive the pulse.
pub fn resolve_cached_tile_layer_pulse<Cell>(
	mut pulse: CachedTileLayerPulse<Cell>,
) -> Result<TileLayerPropagationResolution<Cell>, TileLayerPropagationError<Cell>>
where
	Cell: Clone + Ord,
{
	if pulse.maximum_layers == 0 {
		return Err(TileLayerPropagationError::MaximumLayersIsZero);
	}
	for (cell, state) in &pulse.cells {
		if state.layers > pulse.maximum_layers {
			return Err(TileLayerPropagationError::CellExceedsMaximumLayers {
				cell: cell.clone(),
				layers: state.layers,
				maximum_layers: pulse.maximum_layers,
			});
		}
	}

	let removed_zero_layer_temporary_cells: Vec<Cell> = pulse
		.cells
		.iter()
		.filter_map(|(cell, state): (&Cell, &LayeredTileCell)| {
			if state.persistence == TilePersistence::Temporary && state.layers == 0 {
				Some(cell.clone())
			} else {
				None
			}
		})
		.collect();
	for cell in &removed_zero_layer_temporary_cells {
		pulse.cells.remove(cell);
	}

	let outside_temporary_cells_before: u16 = outside_temporary_count(&pulse.cells)?;
	if outside_temporary_cells_before > pulse.maximum_outside_temporary_cells {
		return Err(
			TileLayerPropagationError::OutsideTemporaryBudgetAlreadyExceeded {
				current: outside_temporary_cells_before,
				maximum: pulse.maximum_outside_temporary_cells,
			},
		);
	}

	let pulse_start_sources: Vec<Cell> = pulse.cells.keys().cloned().collect();
	let mut saturated_sources: Vec<Cell> = Vec::new();
	let mut events: Vec<TileLayerEvent<Cell>> = Vec::new();
	for cell in &pulse_start_sources {
		let state: &mut LayeredTileCell = pulse
			.cells
			.get_mut(cell)
			.expect("pulse source was snapshotted from the cell map");
		if state.layers == pulse.maximum_layers {
			saturated_sources.push(cell.clone());
			events.push(TileLayerEvent::SaturatedAttempt {
				cell: cell.clone(),
				cause: LayerMutationCause::PulseIncrement,
				at_layers: state.layers,
				attempted_layers: 1,
			});
		} else {
			let previous_layers: u16 = state.layers;
			state.layers += 1;
			events.push(TileLayerEvent::LayersAdded {
				cell: cell.clone(),
				cause: LayerMutationCause::PulseIncrement,
				previous_layers,
				added_layers: 1,
				current_layers: state.layers,
			});
		}
	}

	let edges_by_source: BTreeMap<Cell, Vec<EligibleEdge<Cell>>> = collect_eligible_edges(
		&saturated_sources,
		&pulse.neighbors_by_source,
		&pulse.eligible_destinations,
	)?;
	let remaining_budget: u16 =
		pulse.maximum_outside_temporary_cells - outside_temporary_cells_before;
	let budget_candidates: BTreeSet<Cell> = edges_by_source
		.values()
		.flatten()
		.filter(|edge: &&EligibleEdge<Cell>| {
			!pulse.cells.contains_key(&edge.destination)
				&& edge.profile.consumes_outside_temporary_budget()
		})
		.map(|edge: &EligibleEdge<Cell>| edge.destination.clone())
		.collect();

	let (selected_new_outside_temporary_cells, effective_source_order): (
		BTreeSet<Cell>,
		Vec<Cell>,
	) = match &pulse.source_order {
		PulseSourceOrder::Known(order) => {
			validate_known_source_order(&saturated_sources, order)?;
			(
				select_budgeted_cells(order, &edges_by_source, &pulse.cells, remaining_budget),
				order.clone(),
			)
		}
		PulseSourceOrder::Unknown => {
			match resolve_unknown_source_order(
				&saturated_sources,
				&edges_by_source,
				&pulse.cells,
				&budget_candidates,
				remaining_budget,
			) {
				Ok(selected) => (selected, saturated_sources.clone()),
				Err(ambiguity) => {
					return Ok(TileLayerPropagationResolution::Ambiguous(ambiguity));
				}
			}
		}
	};
	let rejected_new_outside_temporary_cells: BTreeSet<Cell> = budget_candidates
		.difference(&selected_new_outside_temporary_cells)
		.cloned()
		.collect();

	let aggregated_contributions: Vec<AggregatedContribution<Cell>> = aggregate_contributions(
		&effective_source_order,
		&edges_by_source,
		&pulse.cells,
		&selected_new_outside_temporary_cells,
	);
	if pulse.contribution_merge == ContributionMergePolicy::Unknown {
		let ambiguous_destinations: Vec<AggregatedContribution<Cell>> = aggregated_contributions
			.iter()
			.filter(|contribution: &&AggregatedContribution<Cell>| {
				contribution.contributing_sources.len() > 1
			})
			.cloned()
			.collect();
		if !ambiguous_destinations.is_empty() {
			return Ok(TileLayerPropagationResolution::Ambiguous(
				TileLayerPropagationAmbiguity::ContributionMergeUnknown {
					destinations: ambiguous_destinations,
				},
			));
		}
	}

	for contribution in &aggregated_contributions {
		let requested_layers: usize = match pulse.contribution_merge {
			ContributionMergePolicy::DeduplicateDestination | ContributionMergePolicy::Unknown => 1,
			ContributionMergePolicy::AccumulatePerSource => contribution.contributing_sources.len(),
		};
		if !pulse.cells.contains_key(&contribution.destination) {
			let profile: PropagationDestination = *pulse
				.eligible_destinations
				.get(&contribution.destination)
				.expect("every contribution destination was eligibility-checked");
			pulse
				.cells
				.insert(contribution.destination.clone(), profile.as_empty_cell());
		}
		let state: &mut LayeredTileCell = pulse
			.cells
			.get_mut(&contribution.destination)
			.expect("existing or newly inserted propagation destination");
		let available_layers: usize = usize::from(pulse.maximum_layers - state.layers);
		let added_layers: usize = requested_layers.min(available_layers);
		let rejected_layers: usize = requested_layers - added_layers;
		let cause: LayerMutationCause<Cell> = LayerMutationCause::CachedPropagation {
			contributing_sources: contribution.contributing_sources.clone(),
		};
		if added_layers > 0 {
			let previous_layers: u16 = state.layers;
			let added_layers: u16 = u16::try_from(added_layers)
				.expect("added layers cannot exceed the u16 maximum layer count");
			state.layers += added_layers;
			events.push(TileLayerEvent::LayersAdded {
				cell: contribution.destination.clone(),
				cause: cause.clone(),
				previous_layers,
				added_layers,
				current_layers: state.layers,
			});
		}
		if rejected_layers > 0 {
			events.push(TileLayerEvent::SaturatedAttempt {
				cell: contribution.destination.clone(),
				cause,
				at_layers: state.layers,
				attempted_layers: rejected_layers,
			});
		}
	}

	let outside_temporary_cells_after: u16 = outside_temporary_count(&pulse.cells)?;
	Ok(TileLayerPropagationResolution::Resolved(
		TileLayerPropagationOutcome {
			cells: pulse.cells,
			pulse_start_sources,
			saturated_sources,
			removed_zero_layer_temporary_cells,
			selected_new_outside_temporary_cells,
			rejected_new_outside_temporary_cells,
			aggregated_contributions,
			events,
			outside_temporary_cells_before,
			outside_temporary_cells_after,
		},
	))
}

fn outside_temporary_count<Cell>(
	cells: &BTreeMap<Cell, LayeredTileCell>,
) -> Result<u16, TileLayerPropagationError<Cell>>
where
	Cell: Clone + Ord,
{
	u16::try_from(
		cells
			.values()
			.filter(|state: &&LayeredTileCell| state.consumes_outside_temporary_budget())
			.count(),
	)
	.map_err(|_| TileLayerPropagationError::CellCountExceedsU16)
}

fn collect_eligible_edges<Cell>(
	saturated_sources: &[Cell],
	neighbors_by_source: &BTreeMap<Cell, OrderedOrthogonalNeighbors<Cell>>,
	eligible_destinations: &BTreeMap<Cell, PropagationDestination>,
) -> Result<BTreeMap<Cell, Vec<EligibleEdge<Cell>>>, TileLayerPropagationError<Cell>>
where
	Cell: Clone + Ord,
{
	let mut result: BTreeMap<Cell, Vec<EligibleEdge<Cell>>> = BTreeMap::new();
	for source in saturated_sources {
		let neighbors: &OrderedOrthogonalNeighbors<Cell> =
			neighbors_by_source.get(source).ok_or_else(|| {
				TileLayerPropagationError::MissingNeighborsForSaturatedSource {
					source: source.clone(),
				}
			})?;
		let mut seen: BTreeSet<Cell> = BTreeSet::new();
		let mut edges: Vec<EligibleEdge<Cell>> = Vec::new();
		for (_, destination) in neighbors.iter() {
			if !seen.insert(destination.clone()) {
				return Err(TileLayerPropagationError::DuplicateOrthogonalNeighbor {
					source: source.clone(),
					destination: destination.clone(),
				});
			}
			if let Some(profile) = eligible_destinations.get(destination) {
				edges.push(EligibleEdge {
					destination: destination.clone(),
					profile: *profile,
				});
			}
		}
		result.insert(source.clone(), edges);
	}
	Ok(result)
}

fn validate_known_source_order<Cell>(
	saturated_sources: &[Cell],
	provided: &[Cell],
) -> Result<(), TileLayerPropagationError<Cell>>
where
	Cell: Clone + Ord,
{
	let expected_sources: BTreeSet<Cell> = saturated_sources.iter().cloned().collect();
	let provided_sources: BTreeSet<Cell> = provided.iter().cloned().collect();
	if provided.len() != expected_sources.len() || provided_sources != expected_sources {
		return Err(
			TileLayerPropagationError::KnownSourceOrderDoesNotMatchSnapshot {
				expected_sources,
				provided_sources: provided.to_vec(),
			},
		);
	}
	Ok(())
}

fn select_budgeted_cells<Cell>(
	source_order: &[Cell],
	edges_by_source: &BTreeMap<Cell, Vec<EligibleEdge<Cell>>>,
	existing_cells: &BTreeMap<Cell, LayeredTileCell>,
	remaining_budget: u16,
) -> BTreeSet<Cell>
where
	Cell: Clone + Ord,
{
	let mut selected: BTreeSet<Cell> = BTreeSet::new();
	for source in source_order {
		for edge in &edges_by_source[source] {
			if existing_cells.contains_key(&edge.destination)
				|| !edge.profile.consumes_outside_temporary_budget()
				|| selected.contains(&edge.destination)
			{
				continue;
			}
			if selected.len() == usize::from(remaining_budget) {
				continue;
			}
			selected.insert(edge.destination.clone());
		}
	}
	selected
}

fn resolve_unknown_source_order<Cell>(
	saturated_sources: &[Cell],
	edges_by_source: &BTreeMap<Cell, Vec<EligibleEdge<Cell>>>,
	existing_cells: &BTreeMap<Cell, LayeredTileCell>,
	budget_candidates: &BTreeSet<Cell>,
	remaining_budget: u16,
) -> Result<BTreeSet<Cell>, TileLayerPropagationAmbiguity<Cell>>
where
	Cell: Clone + Ord,
{
	if remaining_budget == 0 {
		return Ok(BTreeSet::new());
	}
	if budget_candidates.len() <= usize::from(remaining_budget) {
		return Ok(budget_candidates.clone());
	}

	// A candidate can enter the budget prefix iff it occurs within the first K distinct budgeted
	// destinations of at least one source block: put that source first. Its preceding destinations
	// cannot be bypassed by another block because they would already consume the same budget slots.
	let mut minimum_rank: BTreeMap<Cell, usize> = BTreeMap::new();
	for source in saturated_sources {
		let mut local_seen: BTreeSet<Cell> = BTreeSet::new();
		for edge in &edges_by_source[source] {
			if existing_cells.contains_key(&edge.destination)
				|| !edge.profile.consumes_outside_temporary_budget()
				|| !local_seen.insert(edge.destination.clone())
			{
				continue;
			}
			let rank: usize = local_seen.len();
			minimum_rank
				.entry(edge.destination.clone())
				.and_modify(|current: &mut usize| *current = (*current).min(rank))
				.or_insert(rank);
		}
	}
	let selectable: BTreeSet<Cell> = minimum_rank
		.into_iter()
		.filter_map(|(cell, rank): (Cell, usize)| {
			(rank <= usize::from(remaining_budget)).then_some(cell)
		})
		.collect();
	if selectable.len() <= usize::from(remaining_budget) {
		return Ok(selectable);
	}

	let first_order: Vec<Cell> = saturated_sources.to_vec();
	let first_selected: BTreeSet<Cell> = select_budgeted_cells(
		&first_order,
		edges_by_source,
		existing_cells,
		remaining_budget,
	);
	let alternative_cell: Cell = selectable
		.difference(&first_selected)
		.next()
		.expect("more selectable cells than budget guarantees an alternative")
		.clone();
	let alternative_source: Cell = saturated_sources
		.iter()
		.find(|source: &&Cell| {
			let mut rank: usize = 0;
			for edge in &edges_by_source[*source] {
				if existing_cells.contains_key(&edge.destination)
					|| !edge.profile.consumes_outside_temporary_budget()
				{
					continue;
				}
				rank += 1;
				if edge.destination == alternative_cell {
					return rank <= usize::from(remaining_budget);
				}
			}
			false
		})
		.expect("selectable alternative must have a witnessing source")
		.clone();
	let mut second_order: Vec<Cell> = vec![alternative_source.clone()];
	second_order.extend(
		saturated_sources
			.iter()
			.filter(|source: &&Cell| **source != alternative_source)
			.cloned(),
	);
	let second_selected: BTreeSet<Cell> = select_budgeted_cells(
		&second_order,
		edges_by_source,
		existing_cells,
		remaining_budget,
	);
	debug_assert!(first_selected != second_selected);
	Err(
		TileLayerPropagationAmbiguity::SourceOrderChangesBudgetSelection {
			remaining_budget,
			first: SourceOrderWitness {
				source_order: first_order,
				selected_new_outside_temporary_cells: first_selected,
			},
			second: SourceOrderWitness {
				source_order: second_order,
				selected_new_outside_temporary_cells: second_selected,
			},
		},
	)
}

fn aggregate_contributions<Cell>(
	source_order: &[Cell],
	edges_by_source: &BTreeMap<Cell, Vec<EligibleEdge<Cell>>>,
	existing_cells: &BTreeMap<Cell, LayeredTileCell>,
	selected_budgeted_cells: &BTreeSet<Cell>,
) -> Vec<AggregatedContribution<Cell>>
where
	Cell: Clone + Ord,
{
	let mut sources_by_destination: BTreeMap<Cell, BTreeSet<Cell>> = BTreeMap::new();
	for source in source_order {
		for edge in &edges_by_source[source] {
			let accepted: bool = existing_cells.contains_key(&edge.destination)
				|| !edge.profile.consumes_outside_temporary_budget()
				|| selected_budgeted_cells.contains(&edge.destination);
			if accepted {
				sources_by_destination
					.entry(edge.destination.clone())
					.or_default()
					.insert(source.clone());
			}
		}
	}
	sources_by_destination
		.into_iter()
		.map(
			|(destination, contributing_sources): (Cell, BTreeSet<Cell>)| AggregatedContribution {
				destination,
				contributing_sources: contributing_sources.into_iter().collect(),
			},
		)
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	type Cell = &'static str;

	fn neighbors(
		down: Cell,
		left: Cell,
		up: Cell,
		right: Cell,
	) -> OrderedOrthogonalNeighbors<Cell> {
		OrderedOrthogonalNeighbors {
			down: Some(down),
			left: Some(left),
			up: Some(up),
			right: Some(right),
		}
	}

	fn permanent(layers: u16) -> LayeredTileCell {
		LayeredTileCell::permanent(layers, SourceRangeRelation::Inside)
	}

	fn outside_temporary(layers: u16) -> LayeredTileCell {
		LayeredTileCell::temporary(layers, SourceRangeRelation::Outside)
	}

	fn new_outside_temporary() -> PropagationDestination {
		PropagationDestination::temporary(SourceRangeRelation::Outside)
	}

	fn new_permanent() -> PropagationDestination {
		PropagationDestination::permanent(SourceRangeRelation::Inside)
	}

	fn resolved(pulse: CachedTileLayerPulse<Cell>) -> TileLayerPropagationOutcome<Cell> {
		match resolve_cached_tile_layer_pulse(pulse).expect("valid pulse") {
			TileLayerPropagationResolution::Resolved(outcome) => outcome,
			TileLayerPropagationResolution::Ambiguous(ambiguity) => {
				panic!("pulse unexpectedly ambiguous: {ambiguity:?}")
			}
		}
	}

	#[test]
	fn orthogonal_neighbors_are_down_left_up_right() {
		let neighbors: OrderedOrthogonalNeighbors<Cell> = neighbors("D", "L", "U", "R");
		assert_eq!(
			neighbors.iter().collect::<Vec<_>>(),
			vec![
				(OrthogonalDirection::Down, &"D"),
				(OrthogonalDirection::Left, &"L"),
				(OrthogonalDirection::Up, &"U"),
				(OrthogonalDirection::Right, &"R"),
			]
		);
	}

	#[test]
	fn pulse_snapshot_prevents_new_cell_from_propagating_immediately() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("A", permanent(5))]),
			eligible_destinations: BTreeMap::from([
				("B", new_outside_temporary()),
				("C", new_outside_temporary()),
			]),
			neighbors_by_source: BTreeMap::from([
				("A", neighbors("B", "A-L", "A-U", "A-R")),
				("B", neighbors("C", "B-L", "B-U", "B-R")),
			]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 20,
			source_order: PulseSourceOrder::Known(vec!["A"]),
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};
		let outcome = resolved(pulse);

		assert_eq!(outcome.pulse_start_sources, vec!["A"]);
		assert_eq!(outcome.saturated_sources, vec!["A"]);
		assert_eq!(outcome.cells["B"].layers, 1);
		assert!(!outcome.cells.contains_key("C"));
		assert!(outcome.events.iter().any(|event| matches!(
			event,
			TileLayerEvent::SaturatedAttempt {
				cell: "A",
				cause: LayerMutationCause::PulseIncrement,
				attempted_layers: 1,
				..
			}
		)));
	}

	#[test]
	fn unknown_multi_source_merge_is_explicitly_ambiguous() {
		let base = CachedTileLayerPulse {
			cells: BTreeMap::from([("A", permanent(5)), ("B", permanent(5))]),
			eligible_destinations: BTreeMap::from([("X", new_outside_temporary())]),
			neighbors_by_source: BTreeMap::from([
				("A", neighbors("X", "A-L", "A-U", "A-R")),
				("B", neighbors("X", "B-L", "B-U", "B-R")),
			]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 20,
			source_order: PulseSourceOrder::Known(vec!["A", "B"]),
			contribution_merge: ContributionMergePolicy::Unknown,
		};

		let ambiguity = match resolve_cached_tile_layer_pulse(base.clone()).expect("valid pulse") {
			TileLayerPropagationResolution::Ambiguous(ambiguity) => ambiguity,
			TileLayerPropagationResolution::Resolved(_) => {
				panic!("merge evidence must be required")
			}
		};
		assert!(matches!(
			ambiguity,
			TileLayerPropagationAmbiguity::ContributionMergeUnknown { destinations }
				if destinations == vec![AggregatedContribution {
					destination: "X",
					contributing_sources: vec!["A", "B"],
				}]
		));

		let mut accumulated = base.clone();
		accumulated.contribution_merge = ContributionMergePolicy::AccumulatePerSource;
		assert_eq!(resolved(accumulated).cells["X"].layers, 2);

		let mut deduplicated = base;
		deduplicated.contribution_merge = ContributionMergePolicy::DeduplicateDestination;
		assert_eq!(resolved(deduplicated).cells["X"].layers, 1);
	}

	#[test]
	fn existing_cell_does_not_consume_another_budget_slot() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("SOURCE", permanent(5)), ("EXISTING", outside_temporary(2))]),
			eligible_destinations: BTreeMap::from([
				("NEW-FIRST", new_outside_temporary()),
				("EXISTING", new_outside_temporary()),
				("NEW-LATE", new_outside_temporary()),
			]),
			neighbors_by_source: BTreeMap::from([(
				"SOURCE",
				neighbors("NEW-FIRST", "EXISTING", "NEW-LATE", "INELIGIBLE"),
			)]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 2,
			source_order: PulseSourceOrder::Known(vec!["SOURCE"]),
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};
		let outcome = resolved(pulse);

		assert_eq!(outcome.outside_temporary_cells_before, 1);
		assert_eq!(outcome.outside_temporary_cells_after, 2);
		assert_eq!(
			outcome.selected_new_outside_temporary_cells,
			BTreeSet::from(["NEW-FIRST"])
		);
		assert_eq!(
			outcome.rejected_new_outside_temporary_cells,
			BTreeSet::from(["NEW-LATE"])
		);
		assert_eq!(outcome.cells["EXISTING"].layers, 4);
		assert_eq!(outcome.cells["NEW-FIRST"].layers, 1);
		assert!(!outcome.cells.contains_key("NEW-LATE"));
	}

	#[test]
	fn zero_layer_temporary_cell_is_removed_and_releases_budget() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([
				("SOURCE", permanent(5)),
				("DEAD-TEMP", outside_temporary(0)),
				("EMPTY-PERM", permanent(0)),
			]),
			eligible_destinations: BTreeMap::from([("NEW", new_outside_temporary())]),
			neighbors_by_source: BTreeMap::from([("SOURCE", neighbors("NEW", "L", "U", "R"))]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 1,
			source_order: PulseSourceOrder::Known(vec!["SOURCE"]),
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};
		let outcome = resolved(pulse);

		assert_eq!(
			outcome.removed_zero_layer_temporary_cells,
			vec!["DEAD-TEMP"]
		);
		assert!(!outcome.cells.contains_key("DEAD-TEMP"));
		assert_eq!(outcome.cells["EMPTY-PERM"].layers, 1);
		assert_eq!(outcome.cells["NEW"].layers, 1);
		assert_eq!(outcome.outside_temporary_cells_before, 0);
		assert_eq!(outcome.outside_temporary_cells_after, 1);
	}

	#[test]
	fn unknown_source_order_returns_witnesses_when_budget_selection_changes() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("A", permanent(5)), ("B", permanent(5))]),
			eligible_destinations: BTreeMap::from([
				("X", new_outside_temporary()),
				("Y", new_outside_temporary()),
			]),
			neighbors_by_source: BTreeMap::from([
				("A", neighbors("X", "A-L", "A-U", "A-R")),
				("B", neighbors("Y", "B-L", "B-U", "B-R")),
			]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 1,
			source_order: PulseSourceOrder::Unknown,
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};

		let ambiguity = match resolve_cached_tile_layer_pulse(pulse).expect("valid pulse") {
			TileLayerPropagationResolution::Ambiguous(ambiguity) => ambiguity,
			TileLayerPropagationResolution::Resolved(_) => panic!("source order must be ambiguous"),
		};
		let TileLayerPropagationAmbiguity::SourceOrderChangesBudgetSelection {
			remaining_budget,
			first,
			second,
		} = ambiguity
		else {
			panic!("expected source-order ambiguity");
		};
		assert_eq!(remaining_budget, 1);
		assert_ne!(
			first.selected_new_outside_temporary_cells,
			second.selected_new_outside_temporary_cells
		);
	}

	#[test]
	fn unknown_source_order_resolves_when_every_order_has_the_same_budget_prefix() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("A", permanent(5)), ("B", permanent(5))]),
			eligible_destinations: BTreeMap::from([
				("COMMON", new_outside_temporary()),
				("A-LATE", new_outside_temporary()),
				("B-LATE", new_outside_temporary()),
			]),
			neighbors_by_source: BTreeMap::from([
				("A", neighbors("COMMON", "A-LATE", "A-U", "A-R")),
				("B", neighbors("COMMON", "B-LATE", "B-U", "B-R")),
			]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 1,
			source_order: PulseSourceOrder::Unknown,
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};
		let outcome = resolved(pulse);

		assert_eq!(
			outcome.selected_new_outside_temporary_cells,
			BTreeSet::from(["COMMON"])
		);
		assert_eq!(outcome.cells["COMMON"].layers, 1);
	}

	#[test]
	fn permanent_destination_does_not_consume_temporary_budget() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("SOURCE", permanent(5))]),
			eligible_destinations: BTreeMap::from([
				("PERMANENT", new_permanent()),
				("TEMPORARY", new_outside_temporary()),
			]),
			neighbors_by_source: BTreeMap::from([(
				"SOURCE",
				neighbors("PERMANENT", "TEMPORARY", "U", "R"),
			)]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 0,
			source_order: PulseSourceOrder::Known(vec!["SOURCE"]),
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};
		let outcome = resolved(pulse);

		assert_eq!(outcome.cells["PERMANENT"].layers, 1);
		assert!(!outcome.cells.contains_key("TEMPORARY"));
		assert_eq!(outcome.outside_temporary_cells_after, 0);
	}

	#[test]
	fn accumulated_contributions_report_excess_as_saturated_attempts() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("A", permanent(1)), ("B", permanent(1))]),
			eligible_destinations: BTreeMap::from([("X", new_outside_temporary())]),
			neighbors_by_source: BTreeMap::from([
				("A", neighbors("X", "A-L", "A-U", "A-R")),
				("B", neighbors("X", "B-L", "B-U", "B-R")),
			]),
			maximum_layers: 1,
			maximum_outside_temporary_cells: 20,
			source_order: PulseSourceOrder::Known(vec!["A", "B"]),
			contribution_merge: ContributionMergePolicy::AccumulatePerSource,
		};
		let outcome = resolved(pulse);

		assert_eq!(outcome.cells["X"].layers, 1);
		assert!(outcome.events.iter().any(|event| matches!(
			event,
			TileLayerEvent::SaturatedAttempt {
				cell: "X",
				cause: LayerMutationCause::CachedPropagation { contributing_sources },
				attempted_layers: 1,
				..
			} if contributing_sources == &vec!["A", "B"]
		)));
	}

	#[test]
	fn known_source_order_must_cover_each_saturated_source_once() {
		let pulse = CachedTileLayerPulse {
			cells: BTreeMap::from([("A", permanent(5)), ("B", permanent(5))]),
			eligible_destinations: BTreeMap::new(),
			neighbors_by_source: BTreeMap::from([
				("A", neighbors("A-D", "A-L", "A-U", "A-R")),
				("B", neighbors("B-D", "B-L", "B-U", "B-R")),
			]),
			maximum_layers: 5,
			maximum_outside_temporary_cells: 20,
			source_order: PulseSourceOrder::Known(vec!["A", "A"]),
			contribution_merge: ContributionMergePolicy::DeduplicateDestination,
		};

		assert!(matches!(
			resolve_cached_tile_layer_pulse(pulse),
			Err(TileLayerPropagationError::KnownSourceOrderDoesNotMatchSnapshot { .. })
		));
	}
}
