//! Private MAA-to-planning adapter behind the strategy-binding interface.

use std::{
	collections::BTreeMap,
	error::Error,
	fmt::{Display, Formatter},
};

use crate::data::{GridPosition, MapDimensions, StageDifficulty};

use crate::maa_job::{
	MaaJobBuildHint, MaaJobDifficulty, MaaJobDirection, MaaJobGuideSeed, MaaJobOperator,
};
use crate::mechanics::domain::Facing;
use crate::planning::{
	CandidatePoolSpec, DeploymentAnchor, GuideDeploymentActionSpec, GuideDeploymentDomain,
	GuideSchedulingConditions,
};

/// Extracts the complete deployment sequence and marks one source action in canonical MAA order.
///
/// Operator choice, skill metadata, geometry, and complete action conditions come from the selected
/// guide seed. Builds remain candidates from the caller's roster or catalog domain, and no
/// deployment tick is inferred from an optional `time_elapsed` condition.
pub(crate) fn guide_deployment_domain_from_maa_job(
	guide_seed: &MaaJobGuideSeed<'_>,
	source_action_index: usize,
	map_dimensions: MapDimensions,
	build_domain: CandidatePoolSpec,
) -> Result<GuideDeploymentDomain, GuideSeedConversionError> {
	let mut selected_slots: BTreeMap<String, String> = BTreeMap::new();
	for build in guide_seed.build_hints() {
		if let Some(existing) = selected_slots.insert(
			build.slot_name().to_owned(),
			build.operator_name().to_owned(),
		) {
			if existing != build.operator_name() {
				return Err(GuideSeedConversionError::ConflictingSlotSelection {
					slot_name: build.slot_name().to_owned(),
					first_operator_name: existing,
					second_operator_name: build.operator_name().to_owned(),
				});
			}
		}
	}
	let mut source_action_found: bool = false;
	let mut actions: Vec<GuideDeploymentActionSpec> = Vec::new();
	for deployment in guide_seed.deployment_hints() {
		let action_index: usize = deployment.action_index();
		let is_source_slot: bool = action_index == source_action_index;
		source_action_found |= is_source_slot;

		let build: MaaJobBuildHint<'_> = deployment.build();
		let operator: &MaaJobOperator = build.operator();
		let deployment_anchor: DeploymentAnchor = DeploymentAnchor {
			cell: query_cell_from_maa_location(deployment.cell(), map_dimensions)?,
			facing: facing_from_maa_direction(deployment.facing(), action_index)?,
		};
		actions.push(GuideDeploymentActionSpec {
			action_index,
			is_source_slot,
			slot_name: build.slot_name().to_owned(),
			operator_name: build.operator_name().to_owned(),
			skill: operator.skill(),
			skill_usage: operator.skill_usage(),
			skill_times: operator.skill_times(),
			deployment_anchor,
			conditions: GuideSchedulingConditions {
				minimum_dp: deployment.conditions().costs(),
				cost_changes: deployment.conditions().cost_changes(),
				kills: deployment.conditions().kills(),
				cooling: deployment.conditions().cooling(),
				pre_delay_millis: deployment.conditions().pre_delay(),
				post_delay_millis: deployment.conditions().post_delay(),
				time_elapsed_millis: deployment.conditions().time_elapsed(),
			},
		});
	}
	if !source_action_found {
		return Err(GuideSeedConversionError::DeploymentActionNotFound {
			action_index: source_action_index,
		});
	}

	let difficulty: StageDifficulty = match guide_seed.difficulty() {
		MaaJobDifficulty::Normal => StageDifficulty::Normal,
		MaaJobDifficulty::Challenge => StageDifficulty::Raid,
		MaaJobDifficulty::Unspecified => {
			return Err(GuideSeedConversionError::UnspecifiedDifficulty);
		}
	};
	Ok(GuideDeploymentDomain {
		source_id: guide_seed.source_id().to_owned(),
		content_sha256: guide_seed.content_sha256().to_owned(),
		stage_name: guide_seed.stage_name().to_owned(),
		difficulty,
		source_action_index,
		selected_slots,
		build_domain,
		actions,
	})
}

fn query_cell_from_maa_location(
	[x, y]: [i32; 2],
	map_dimensions: MapDimensions,
) -> Result<GridPosition, GuideSeedConversionError> {
	let col: u32 = u32::try_from(x)
		.ok()
		.filter(|col: &u32| *col < map_dimensions.width)
		.ok_or(GuideSeedConversionError::XCoordinateOutOfBounds {
			value: x,
			map_width: map_dimensions.width,
		})?;
	let y_from_top: u32 = u32::try_from(y)
		.ok()
		.filter(|row: &u32| *row < map_dimensions.height)
		.ok_or(GuideSeedConversionError::YCoordinateOutOfBounds {
			value: y,
			map_height: map_dimensions.height,
		})?;
	let row: u32 = map_dimensions
		.height
		.checked_sub(1)
		.and_then(|bottom_row: u32| bottom_row.checked_sub(y_from_top))
		.ok_or(GuideSeedConversionError::YCoordinateOutOfBounds {
			value: y,
			map_height: map_dimensions.height,
		})?;
	Ok(GridPosition { row, col })
}

fn facing_from_maa_direction(
	direction: MaaJobDirection,
	action_index: usize,
) -> Result<Facing, GuideSeedConversionError> {
	let facing: Facing = match direction {
		MaaJobDirection::Up => Facing::Up,
		MaaJobDirection::Right => Facing::Right,
		MaaJobDirection::Down => Facing::Down,
		MaaJobDirection::Left => Facing::Left,
		MaaJobDirection::None => {
			return Err(GuideSeedConversionError::MissingFacing { action_index });
		}
	};
	Ok(facing)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GuideSeedConversionError {
	UnspecifiedDifficulty,
	DeploymentActionNotFound {
		action_index: usize,
	},
	XCoordinateOutOfBounds {
		value: i32,
		map_width: u32,
	},
	YCoordinateOutOfBounds {
		value: i32,
		map_height: u32,
	},
	MissingFacing {
		action_index: usize,
	},
	ConflictingSlotSelection {
		slot_name: String,
		first_operator_name: String,
		second_operator_name: String,
	},
}

impl Display for GuideSeedConversionError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::UnspecifiedDifficulty => {
				write!(formatter, "MAA job file difficulty must be selected")
			}
			Self::DeploymentActionNotFound { action_index } => write!(
				formatter,
				"MAA job file deployment action {action_index} was not found"
			),
			Self::XCoordinateOutOfBounds { value, map_width } => write!(
				formatter,
				"MAA job file x coordinate {value} is outside map bounds [0, {map_width})"
			),
			Self::YCoordinateOutOfBounds { value, map_height } => write!(
				formatter,
				"MAA job file top-origin y coordinate {value} is outside map bounds [0, {map_height})"
			),
			Self::MissingFacing { action_index } => write!(
				formatter,
				"MAA job file deployment action {action_index} has no facing"
			),
			Self::ConflictingSlotSelection {
				slot_name,
				first_operator_name,
				second_operator_name,
			} => write!(
				formatter,
				"MAA job file slot {slot_name:?} selects conflicting operators \
				 {first_operator_name:?} and {second_operator_name:?}"
			),
		}
	}
}

impl Error for GuideSeedConversionError {}

#[cfg(test)]
mod tests {
	use crate::data::{GridPosition, MapDimensions, StageDifficulty};

	use crate::maa_job::{parse_maa_job, MaaJobDifficulty, MaaJobSourceLineage};
	use crate::mechanics::domain::Facing;
	use crate::planning::{BasePanelBuildSpec, CandidatePoolSpec};

	use super::{
		guide_deployment_domain_from_maa_job, query_cell_from_maa_location,
		GuideSeedConversionError,
	};
	use crate::planning::GuideDeploymentDomain;

	fn convert_location(
		location: [i32; 2],
		map_dimensions: MapDimensions,
	) -> Result<GridPosition, GuideSeedConversionError> {
		query_cell_from_maa_location(location, map_dimensions)
	}

	#[test]
	fn converts_top_origin_xy_to_stage_coordinates() {
		let cell: GridPosition = convert_location(
			[3, 2],
			MapDimensions {
				width: 8,
				height: 6,
			},
		)
		.unwrap();

		assert_eq!(cell.row, 3);
		assert_eq!(cell.col, 3);
	}

	#[test]
	fn extracts_complete_source_order_with_source_marker_and_explicit_build_domain() {
		let guide = parse_maa_job(
			r#"
			{
				"minimum_required": "v6.0.0",
				"stage_name": "TEST-1",
				"difficulty": 1,
				"opers": [
					{
						"name": "前卫",
						"skill": 1,
						"skill_usage": 0,
						"requirements": {"elite": 1, "level": 55}
					},
					{"name": "源干员", "skill": 2, "skill_usage": 1},
					{"name": "后卫", "skill": 3, "skill_usage": 0, "skill_times": 2}
				],
				"actions": [
					{
						"type": "Deploy",
						"name": "前卫",
						"location": [1, 0],
						"direction": "Down",
						"costs": 12,
						"cost_changes": 1,
						"pre_delay": 200
					},
					{"type": "SpeedUp"},
					{
						"type": "Deploy",
						"name": "源干员",
						"location": [2, 1],
						"direction": "Left",
						"costs": 20,
						"kills": 3
					},
					{"type": "Output"},
					{"type": "ResetStopwatch"},
					{
						"type": "Deploy",
						"name": "后卫",
						"location": [3, 2],
						"direction": "Up",
						"time_elapsed": 1500
					}
				]
			}
			"#,
		)
		.unwrap();
		let digest: String = "cd".repeat(32);
		let lineage = MaaJobSourceLineage::new("fixture:ordered", &digest).unwrap();
		let guide_seed = guide
			.guide_seeds(lineage, MaaJobDifficulty::Normal, 1)
			.unwrap()
			.next()
			.unwrap();
		let build_domain: CandidatePoolSpec = CandidatePoolSpec::Roster {
			builds: vec![BasePanelBuildSpec {
				operator_id: "char_domain".to_owned(),
				elite: 2,
				level: 90,
			}],
		};

		let extracted: GuideDeploymentDomain = guide_deployment_domain_from_maa_job(
			&guide_seed,
			2,
			MapDimensions {
				width: 5,
				height: 4,
			},
			build_domain.clone(),
		)
		.unwrap();

		assert_eq!(extracted.build_domain, build_domain);
		assert_eq!(extracted.difficulty, StageDifficulty::Normal);
		assert_eq!(
			extracted
				.actions
				.iter()
				.map(|action| action.action_index)
				.collect::<Vec<_>>(),
			vec![0, 2, 5]
		);
		assert_eq!(
			extracted
				.actions
				.iter()
				.map(|action| action.is_source_slot)
				.collect::<Vec<_>>(),
			vec![false, true, false]
		);
		assert_eq!(extracted.actions[0].operator_name, "前卫");
		assert_eq!(extracted.actions[0].skill, Some(1));
		assert_eq!(extracted.actions[0].skill_usage, Some(0));
		assert_eq!(extracted.actions[0].deployment_anchor.cell.row, 3);
		assert_eq!(extracted.actions[0].deployment_anchor.cell.col, 1);
		assert_eq!(extracted.actions[0].deployment_anchor.facing, Facing::Down);
		assert_eq!(extracted.actions[0].conditions.minimum_dp, Some(12));
		assert_eq!(extracted.actions[0].conditions.cost_changes, Some(1));
		assert_eq!(extracted.actions[0].conditions.pre_delay_millis, Some(200));
		assert_eq!(extracted.actions[1].conditions.kills, Some(3));
		assert_eq!(extracted.actions[2].skill_times, Some(2));
		assert_eq!(
			extracted.actions[2].conditions.time_elapsed_millis,
			Some(1500)
		);
	}

	#[test]
	fn rejects_x_outside_map_width() {
		let error: GuideSeedConversionError = convert_location(
			[4, 0],
			MapDimensions {
				width: 4,
				height: 3,
			},
		)
		.unwrap_err();

		assert_eq!(
			error,
			GuideSeedConversionError::XCoordinateOutOfBounds {
				value: 4,
				map_width: 4,
			}
		);
	}

	#[test]
	fn rejects_y_outside_map_height() {
		let error: GuideSeedConversionError = convert_location(
			[0, 3],
			MapDimensions {
				width: 4,
				height: 3,
			},
		)
		.unwrap_err();

		assert_eq!(
			error,
			GuideSeedConversionError::YCoordinateOutOfBounds {
				value: 3,
				map_height: 3,
			}
		);
	}

	#[test]
	fn rejects_negative_coordinates() {
		let x_error: GuideSeedConversionError = convert_location(
			[-1, 0],
			MapDimensions {
				width: 4,
				height: 3,
			},
		)
		.unwrap_err();
		let y_error: GuideSeedConversionError = convert_location(
			[0, -1],
			MapDimensions {
				width: 4,
				height: 3,
			},
		)
		.unwrap_err();

		assert_eq!(
			x_error,
			GuideSeedConversionError::XCoordinateOutOfBounds {
				value: -1,
				map_width: 4,
			}
		);
		assert_eq!(
			y_error,
			GuideSeedConversionError::YCoordinateOutOfBounds {
				value: -1,
				map_height: 3,
			}
		);
	}
}
