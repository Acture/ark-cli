use std::{
	collections::BTreeSet,
	error::Error,
	fmt::{Display, Formatter},
};

/// Deterministic natural-DP rules for a stage.
///
/// This model deliberately excludes retreats, refunds, talents, and stage-specific modifiers.
/// Regeneration completes at ticks divisible by `regen_interval_ticks`, including the interval
/// ending exactly at a deployment tick. DP above `max_dp` is discarded when each segment of
/// regeneration is applied and cannot be recovered by a later deployment. Skills grant DP only
/// through the [`PlannedDeploymentDpGrant`]s an ordered deployment carries.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeploymentDpRules {
	pub initial_dp: u64,
	pub max_dp: u64,
	pub regen_interval_ticks: u64,
	pub dp_per_interval: u64,
}

impl DeploymentDpRules {
	#[must_use]
	pub const fn new(
		initial_dp: u64,
		max_dp: u64,
		regen_interval_ticks: u64,
		dp_per_interval: u64,
	) -> Self {
		Self {
			initial_dp,
			max_dp,
			regen_interval_ticks,
			dp_per_interval,
		}
	}
}

/// One deployment whose DP feasibility must be verified.
///
/// Events are audited in ascending `(tick, order, stable_key)` order, independently of input
/// order. `order` is the caller's explicit same-tick ordering; `stable_key` is its deterministic
/// tie-breaker and must be unique across the prefix.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeploymentDpEvent {
	pub tick: u64,
	pub order: u64,
	pub stable_key: String,
	pub cost: u64,
}

impl DeploymentDpEvent {
	#[must_use]
	pub fn new(tick: u64, order: u64, stable_key: impl Into<String>, cost: u64) -> Self {
		Self {
			tick,
			order,
			stable_key: stable_key.into(),
			cost,
		}
	}
}

/// One automatic deployment-point grant that starts when its owner is deployed.
///
/// The first grant resolves `first_offset_ticks` after the owner's deployment tick and repeats
/// every `period_ticks` for the rest of the schedule. A grant resolving in a tick is spendable by
/// a deployment in that same tick, but never by the deployment that brought it: its owner pays
/// first, exactly as replay does.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlannedDeploymentDpGrant {
	pub first_offset_ticks: u64,
	pub period_ticks: u64,
	pub amount: u64,
}

impl PlannedDeploymentDpGrant {
	#[must_use]
	pub const fn new(first_offset_ticks: u64, period_ticks: u64, amount: u64) -> Self {
		Self {
			first_offset_ticks,
			period_ticks,
			amount,
		}
	}
}

/// One MAA deployment action awaiting deterministic DP scheduling.
///
/// The containing slice is the source action order. `earliest_tick` is an optional absolute
/// lower bound supplied by an explicit guard. `completion_delay_ticks` keeps the following
/// action from executing until this deployment action has completed. `grants` are the automatic
/// deployment-point grants this deployment brings onto the battlefield.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OrderedDeploymentDpAction {
	pub stable_key: String,
	pub cost: u64,
	/// Additional MAA `costs` guard. Scheduling waits for this threshold but does not spend it.
	pub minimum_dp_before_deployment: u64,
	pub earliest_tick: Option<u64>,
	pub completion_delay_ticks: u64,
	pub grants: Vec<PlannedDeploymentDpGrant>,
}

impl OrderedDeploymentDpAction {
	#[must_use]
	pub fn new(
		stable_key: impl Into<String>,
		cost: u64,
		earliest_tick: Option<u64>,
		completion_delay_ticks: u64,
	) -> Self {
		Self {
			stable_key: stable_key.into(),
			cost,
			minimum_dp_before_deployment: 0,
			earliest_tick,
			completion_delay_ticks,
			grants: Vec::new(),
		}
	}

	#[must_use]
	pub const fn with_minimum_dp_before_deployment(
		mut self,
		minimum_dp_before_deployment: u64,
	) -> Self {
		self.minimum_dp_before_deployment = minimum_dp_before_deployment;
		self
	}

	#[must_use]
	pub fn with_grants(mut self, grants: Vec<PlannedDeploymentDpGrant>) -> Self {
		self.grants = grants;
		self
	}
}

/// DP accounting for one accepted deployment event.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeploymentDpEventAudit {
	pub event: DeploymentDpEvent,
	/// Total regeneration intervals completed from tick zero through this event's tick.
	pub completed_regen_intervals: u64,
	/// Intervals completed since the preceding event in canonical order.
	pub newly_completed_regen_intervals: u64,
	/// DP credited from the new intervals after applying the stage cap.
	pub regenerated_dp: u64,
	/// Carried DP before this event's newly completed intervals are applied.
	pub dp_before_regeneration: u64,
	/// Capped DP immediately before paying this event's deployment cost.
	pub dp_before_deployment: u64,
	/// DP remaining immediately after paying this event's deployment cost.
	pub dp_after_deployment: u64,
}

/// One source-ordered deployment with its earliest deterministic DP-feasible time.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScheduledDeploymentDpAction {
	pub source_index: usize,
	pub action: OrderedDeploymentDpAction,
	/// Maximum of the preceding action's completion and this action's explicit lower bound.
	pub ready_tick: u64,
	pub deploy_tick: u64,
	pub completion_tick: u64,
	/// Total regeneration intervals completed from tick zero through `deploy_tick`.
	pub completed_regen_intervals: u64,
	/// Intervals completed since the preceding deployment in source order.
	pub newly_completed_regen_intervals: u64,
	/// DP credited from those intervals after applying the stage cap.
	pub regenerated_dp: u64,
	/// DP credited by skill grants since the preceding deployment, after applying the stage cap.
	pub granted_dp: u64,
	/// Carried DP immediately after the preceding deployment.
	pub dp_before_regeneration: u64,
	pub dp_before_deployment: u64,
	pub dp_after_deployment: u64,
}

/// Why a deployment prefix could not be verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeploymentDpVerificationError {
	InitialDpExceedsMaximum {
		initial_dp: u64,
		max_dp: u64,
	},
	ZeroRegenIntervalTicks,
	BlankStableKey {
		input_index: usize,
	},
	DuplicateStableKey {
		stable_key: String,
	},
	InsufficientDp {
		stable_key: String,
		tick: u64,
		order: u64,
		cost: u64,
		available_dp: u64,
		deficit: u64,
	},
}

impl Display for DeploymentDpVerificationError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::InitialDpExceedsMaximum { initial_dp, max_dp } => write!(
				formatter,
				"initial DP {initial_dp} exceeds maximum DP {max_dp}"
			),
			Self::ZeroRegenIntervalTicks => {
				formatter.write_str("DP regeneration interval must be positive")
			}
			Self::BlankStableKey { input_index } => {
				write!(
					formatter,
					"deployment event at input index {input_index} has a blank stable key"
				)
			}
			Self::DuplicateStableKey { stable_key } => {
				write!(formatter, "duplicate deployment stable key {stable_key}")
			}
			Self::InsufficientDp {
				stable_key,
				tick,
				order,
				cost,
				available_dp,
				deficit,
			} => write!(
				formatter,
				"deployment {stable_key} at tick {tick} order {order} costs {cost} DP with only \
				 {available_dp} available; deficit is {deficit} DP"
			),
		}
	}
}

impl Error for DeploymentDpVerificationError {}

/// Why an ordered deployment schedule could not be materialized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrderedDeploymentDpScheduleError {
	InvalidRules(DeploymentDpVerificationError),
	BlankStableKey {
		source_index: usize,
	},
	DuplicateStableKey {
		stable_key: String,
	},
	DeploymentCostExceedsMaximum {
		source_index: usize,
		stable_key: String,
		cost: u64,
		max_dp: u64,
	},
	MinimumDpBeforeDeploymentExceedsMaximum {
		source_index: usize,
		stable_key: String,
		minimum_dp_before_deployment: u64,
		max_dp: u64,
	},
	InsufficientDpWithoutCredit {
		source_index: usize,
		stable_key: String,
		deployment_cost: u64,
		minimum_dp_before_deployment: u64,
		required_dp: u64,
		available_dp: u64,
	},
	EmptyGrant {
		source_index: usize,
		stable_key: String,
		grant_index: usize,
	},
	NonPositiveGrantPeriod {
		source_index: usize,
		stable_key: String,
		grant_index: usize,
	},
	DeploymentTickOverflow {
		source_index: usize,
		stable_key: String,
	},
	CompletionTickOverflow {
		source_index: usize,
		stable_key: String,
		deploy_tick: u64,
		completion_delay_ticks: u64,
	},
}

impl Display for OrderedDeploymentDpScheduleError {
	fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::InvalidRules(error) => write!(formatter, "invalid deployment DP rules: {error}"),
			Self::BlankStableKey { source_index } => write!(
				formatter,
				"ordered deployment at source index {source_index} has a blank stable key"
			),
			Self::DuplicateStableKey { stable_key } => {
				write!(
					formatter,
					"duplicate ordered deployment stable key {stable_key}"
				)
			}
			Self::DeploymentCostExceedsMaximum {
				source_index,
				stable_key,
				cost,
				max_dp,
			} => write!(
				formatter,
				"ordered deployment {stable_key} at source index {source_index} costs {cost} DP, \
				 exceeding the stage maximum {max_dp}"
			),
			Self::MinimumDpBeforeDeploymentExceedsMaximum {
				source_index,
				stable_key,
				minimum_dp_before_deployment,
				max_dp,
			} => write!(
				formatter,
				"ordered deployment {stable_key} at source index {source_index} requires at least \
				 {minimum_dp_before_deployment} DP before deployment, exceeding the stage maximum \
				 {max_dp}"
			),
			Self::InsufficientDpWithoutCredit {
				source_index,
				stable_key,
				deployment_cost,
				minimum_dp_before_deployment,
				required_dp,
				available_dp,
			} => write!(
				formatter,
				"ordered deployment {stable_key} at source index {source_index} costs \
				 {deployment_cost} DP and requires at least {minimum_dp_before_deployment} DP; \
				 required DP {required_dp} exceeds the {available_dp} available, and neither \
				 passive regeneration nor a skill grant credits any more"
			),
			Self::EmptyGrant {
				source_index,
				stable_key,
				grant_index,
			} => write!(
				formatter,
				"grant {grant_index} of ordered deployment {stable_key} at source index \
				 {source_index} credits no DP"
			),
			Self::NonPositiveGrantPeriod {
				source_index,
				stable_key,
				grant_index,
			} => write!(
				formatter,
				"grant {grant_index} of ordered deployment {stable_key} at source index \
				 {source_index} repeats on a zero-tick period"
			),
			Self::DeploymentTickOverflow {
				source_index,
				stable_key,
			} => write!(
				formatter,
				"deployment tick overflowed while scheduling {stable_key} at source index \
				 {source_index}"
			),
			Self::CompletionTickOverflow {
				source_index,
				stable_key,
				deploy_tick,
				completion_delay_ticks,
			} => write!(
				formatter,
				"completion tick overflowed for {stable_key} at source index {source_index}: \
				 deployment tick {deploy_tick} plus completion delay {completion_delay_ticks}"
			),
		}
	}
}

impl Error for OrderedDeploymentDpScheduleError {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		match self {
			Self::InvalidRules(error) => Some(error),
			_ => None,
		}
	}
}

/// Schedules MAA deployments in source order at their earliest DP-feasible ticks.
///
/// Every action becomes ready only after the preceding action's completion and its own optional
/// absolute lower bound. If the carried DP is insufficient at that point, the scheduler walks the
/// battlefield's DP events — passive-regeneration boundaries and the grants already-deployed
/// actions brought — and executes at the first tick where DP reaches both the deployment cost and
/// the optional MAA `costs` guard. Execution deducts only the deployment cost. Same-tick
/// deployments remain in source order and spend from the same carried DP prefix.
///
/// Within one tick regeneration is credited before a grant, and both before any deployment spends,
/// which is what replay's own DP audit records. DP discarded at `max_dp` is never restored, so a
/// grant arriving at a full pool credits nothing. An action's own grants start only after it has
/// paid, so a grant whose first offset is zero funds the next deployment in that tick, not its own.
///
/// A deployment whose cost or guard exceeds `max_dp`, or whose deficit cannot be covered because
/// nothing credits DP any more, fails explicitly instead of searching forever.
pub fn schedule_ordered_deployments(
	rules: DeploymentDpRules,
	actions: &[OrderedDeploymentDpAction],
) -> Result<Vec<ScheduledDeploymentDpAction>, OrderedDeploymentDpScheduleError> {
	validate_rules(rules).map_err(OrderedDeploymentDpScheduleError::InvalidRules)?;
	validate_ordered_action_keys(actions)?;
	validate_ordered_action_grants(actions)?;

	let mut schedule: Vec<ScheduledDeploymentDpAction> = Vec::with_capacity(actions.len());
	let mut timeline = DeploymentDpTimeline::new(rules);
	let mut previous_completion_tick: u64 = 0;
	let mut previous_completed_regen_intervals: u64 = 0;

	for (source_index, action) in actions.iter().enumerate() {
		let overflow = || OrderedDeploymentDpScheduleError::DeploymentTickOverflow {
			source_index,
			stable_key: action.stable_key.clone(),
		};
		if action.cost > rules.max_dp {
			return Err(
				OrderedDeploymentDpScheduleError::DeploymentCostExceedsMaximum {
					source_index,
					stable_key: action.stable_key.clone(),
					cost: action.cost,
					max_dp: rules.max_dp,
				},
			);
		}
		if action.minimum_dp_before_deployment > rules.max_dp {
			return Err(
				OrderedDeploymentDpScheduleError::MinimumDpBeforeDeploymentExceedsMaximum {
					source_index,
					stable_key: action.stable_key.clone(),
					minimum_dp_before_deployment: action.minimum_dp_before_deployment,
					max_dp: rules.max_dp,
				},
			);
		}
		let required_dp: u64 = action.cost.max(action.minimum_dp_before_deployment);

		let ready_tick: u64 = previous_completion_tick.max(action.earliest_tick.unwrap_or(0));
		let dp_before_regeneration: u64 = timeline.dp;
		let mut credited: CreditedDp = timeline.advance_to(ready_tick);

		let mut deploy_tick: u64 = ready_tick;
		while timeline.dp < required_dp {
			let Some(next_tick) = timeline.next_event_tick(deploy_tick, overflow)? else {
				return Err(
					OrderedDeploymentDpScheduleError::InsufficientDpWithoutCredit {
						source_index,
						stable_key: action.stable_key.clone(),
						deployment_cost: action.cost,
						minimum_dp_before_deployment: action.minimum_dp_before_deployment,
						required_dp,
						available_dp: timeline.dp,
					},
				);
			};
			credited += timeline.advance_to(next_tick);
			deploy_tick = next_tick;
		}

		let completion_tick: u64 = deploy_tick
			.checked_add(action.completion_delay_ticks)
			.ok_or_else(
				|| OrderedDeploymentDpScheduleError::CompletionTickOverflow {
					source_index,
					stable_key: action.stable_key.clone(),
					deploy_tick,
					completion_delay_ticks: action.completion_delay_ticks,
				},
			)?;
		let dp_before_deployment: u64 = timeline.dp;
		let dp_after_deployment: u64 = dp_before_deployment - action.cost;
		timeline.dp = dp_after_deployment;
		timeline.start_grants(deploy_tick, &action.grants, overflow)?;

		let completed_regen_intervals: u64 = deploy_tick / rules.regen_interval_ticks;
		let newly_completed_regen_intervals: u64 =
			completed_regen_intervals - previous_completed_regen_intervals;

		schedule.push(ScheduledDeploymentDpAction {
			source_index,
			action: action.clone(),
			ready_tick,
			deploy_tick,
			completion_tick,
			completed_regen_intervals,
			newly_completed_regen_intervals,
			regenerated_dp: credited.regenerated,
			granted_dp: credited.granted,
			dp_before_regeneration,
			dp_before_deployment,
			dp_after_deployment,
		});
		previous_completion_tick = completion_tick;
		previous_completed_regen_intervals = completed_regen_intervals;
	}

	Ok(schedule)
}

/// DP credited to one deployment since the preceding one, split by source.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CreditedDp {
	regenerated: u64,
	granted: u64,
}

impl std::ops::AddAssign for CreditedDp {
	fn add_assign(&mut self, other: Self) {
		self.regenerated += other.regenerated;
		self.granted += other.granted;
	}
}

/// One active grant source and the absolute tick its next grant resolves at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingDpGrant {
	next_tick: u64,
	period_ticks: u64,
	amount: u64,
}

/// The battlefield's DP over time: regeneration boundaries plus every started grant.
///
/// The timeline is only ever advanced forward. Everything at or before the tick it has reached is
/// credited, except grants started at exactly that tick, which the next advance credits — that is
/// how an action's own zero-offset grant stays out of its own deployment cost.
struct DeploymentDpTimeline {
	rules: DeploymentDpRules,
	dp: u64,
	/// Regeneration intervals already credited.
	credited_regen_intervals: u64,
	pending_grants: Vec<PendingDpGrant>,
}

impl DeploymentDpTimeline {
	fn new(rules: DeploymentDpRules) -> Self {
		Self {
			rules,
			dp: rules.initial_dp,
			credited_regen_intervals: 0,
			pending_grants: Vec::new(),
		}
	}

	/// Credits every regeneration boundary and grant through `tick`, in chronological order.
	fn advance_to(&mut self, tick: u64) -> CreditedDp {
		let mut credited = CreditedDp::default();
		loop {
			if self.dp == self.rules.max_dp {
				// A full pool discards everything that follows, so the rest of the span only
				// moves the bookkeeping forward.
				self.fast_forward(tick);
				return credited;
			}
			let next_grant_tick: Option<u64> = self
				.pending_grants
				.iter()
				.map(|grant: &PendingDpGrant| grant.next_tick)
				.min()
				.filter(|grant_tick: &u64| *grant_tick <= tick);
			let boundary: u64 = next_grant_tick.unwrap_or(tick);
			credited.regenerated += self.credit_regeneration_through(boundary);
			let Some(grant_tick) = next_grant_tick else {
				return credited;
			};
			for index in 0..self.pending_grants.len() {
				if self.pending_grants[index].next_tick != grant_tick {
					continue;
				}
				let amount: u64 = self.pending_grants[index].amount;
				credited.granted += self.credit(amount);
				self.pending_grants[index].next_tick =
					grant_tick.saturating_add(self.pending_grants[index].period_ticks);
			}
		}
	}

	/// The earliest tick strictly after `after` at which DP changes, if any ever does.
	fn next_event_tick(
		&self,
		after: u64,
		overflow: impl Fn() -> OrderedDeploymentDpScheduleError,
	) -> Result<Option<u64>, OrderedDeploymentDpScheduleError> {
		let next_regen_tick: Option<u64> = if self.rules.dp_per_interval == 0 {
			None
		} else {
			Some(
				(after / self.rules.regen_interval_ticks)
					.checked_add(1)
					.and_then(|intervals: u64| {
						intervals.checked_mul(self.rules.regen_interval_ticks)
					})
					.ok_or_else(&overflow)?,
			)
		};
		Ok(self
			.pending_grants
			.iter()
			.map(|grant: &PendingDpGrant| grant.next_tick)
			.filter(|tick: &u64| *tick > after)
			.chain(next_regen_tick)
			.min())
	}

	/// Starts one deployment's grants from its deployment tick, after it has paid.
	fn start_grants(
		&mut self,
		deploy_tick: u64,
		grants: &[PlannedDeploymentDpGrant],
		overflow: impl Fn() -> OrderedDeploymentDpScheduleError,
	) -> Result<(), OrderedDeploymentDpScheduleError> {
		for grant in grants {
			self.pending_grants.push(PendingDpGrant {
				next_tick: deploy_tick
					.checked_add(grant.first_offset_ticks)
					.ok_or_else(&overflow)?,
				period_ticks: grant.period_ticks,
				amount: grant.amount,
			});
		}
		Ok(())
	}

	fn credit_regeneration_through(&mut self, tick: u64) -> u64 {
		let completed_intervals: u64 =
			(tick / self.rules.regen_interval_ticks).max(self.credited_regen_intervals);
		let new_intervals: u64 = completed_intervals - self.credited_regen_intervals;
		self.credited_regen_intervals = completed_intervals;
		let credited: u64 = credited_regeneration(self.dp, new_intervals, self.rules);
		self.dp += credited;
		credited
	}

	fn credit(&mut self, amount: u64) -> u64 {
		let credited: u64 = amount.min(self.rules.max_dp - self.dp);
		self.dp += credited;
		credited
	}

	fn fast_forward(&mut self, tick: u64) {
		self.credited_regen_intervals =
			(tick / self.rules.regen_interval_ticks).max(self.credited_regen_intervals);
		for grant in &mut self.pending_grants {
			if grant.next_tick > tick {
				continue;
			}
			let elapsed: u64 = tick - grant.next_tick;
			let periods: u64 = elapsed / grant.period_ticks + 1;
			grant.next_tick = grant
				.next_tick
				.saturating_add(periods.saturating_mul(grant.period_ticks));
		}
	}
}

/// Verifies every deployment against the natural-regeneration prefix available at its tick.
///
/// The verifier carries the DP remaining after each deployment. Before the next event, it adds
/// only the regeneration intervals completed since the preceding event, caps that sum at
/// `max_dp`, then pays the event's cost. Consequently, regeneration discarded at the cap is never
/// available to a later deployment. Same-tick events after the first receive zero new intervals.
///
/// Events carry no skill grants: this checks a prefix the caller already fixed in time, so a
/// grant it does not know about can only make the prefix more affordable, never less.
///
/// Returned audits use canonical `(tick, order, stable_key)` order. The function has no external
/// state and does not mutate the supplied events.
pub fn verify_deployment_dp_prefix(
	rules: DeploymentDpRules,
	events: &[DeploymentDpEvent],
) -> Result<Vec<DeploymentDpEventAudit>, DeploymentDpVerificationError> {
	validate_rules(rules)?;
	validate_event_keys(events)?;

	let mut ordered_events: Vec<&DeploymentDpEvent> = events.iter().collect();
	ordered_events.sort_unstable_by(|left, right| {
		left.tick
			.cmp(&right.tick)
			.then_with(|| left.order.cmp(&right.order))
			.then_with(|| left.stable_key.cmp(&right.stable_key))
	});

	let mut audits: Vec<DeploymentDpEventAudit> = Vec::with_capacity(ordered_events.len());
	let mut previous_completed_regen_intervals: u64 = 0;
	let mut current_dp: u64 = rules.initial_dp;
	for event in ordered_events {
		let completed_regen_intervals: u64 = event.tick / rules.regen_interval_ticks;
		let newly_completed_regen_intervals: u64 =
			completed_regen_intervals - previous_completed_regen_intervals;
		let dp_before_regeneration: u64 = current_dp;
		let regenerated_dp: u64 =
			credited_regeneration(current_dp, newly_completed_regen_intervals, rules);
		let dp_before_deployment: u64 = dp_before_regeneration + regenerated_dp;
		if event.cost > dp_before_deployment {
			let deficit: u64 = event.cost - dp_before_deployment;
			return Err(DeploymentDpVerificationError::InsufficientDp {
				stable_key: event.stable_key.clone(),
				tick: event.tick,
				order: event.order,
				cost: event.cost,
				available_dp: dp_before_deployment,
				deficit,
			});
		}

		let dp_after_deployment: u64 = dp_before_deployment - event.cost;
		audits.push(DeploymentDpEventAudit {
			event: event.clone(),
			completed_regen_intervals,
			newly_completed_regen_intervals,
			regenerated_dp,
			dp_before_regeneration,
			dp_before_deployment,
			dp_after_deployment,
		});
		previous_completed_regen_intervals = completed_regen_intervals;
		current_dp = dp_after_deployment;
	}

	Ok(audits)
}

fn credited_regeneration(
	current_dp: u64,
	completed_intervals: u64,
	rules: DeploymentDpRules,
) -> u64 {
	let remaining_capacity: u64 = rules.max_dp - current_dp;
	if rules.dp_per_interval == 0
		|| completed_intervals <= remaining_capacity / rules.dp_per_interval
	{
		completed_intervals * rules.dp_per_interval
	} else {
		remaining_capacity
	}
}

fn validate_rules(rules: DeploymentDpRules) -> Result<(), DeploymentDpVerificationError> {
	if rules.initial_dp > rules.max_dp {
		return Err(DeploymentDpVerificationError::InitialDpExceedsMaximum {
			initial_dp: rules.initial_dp,
			max_dp: rules.max_dp,
		});
	}
	if rules.regen_interval_ticks == 0 {
		return Err(DeploymentDpVerificationError::ZeroRegenIntervalTicks);
	}
	Ok(())
}

fn validate_ordered_action_grants(
	actions: &[OrderedDeploymentDpAction],
) -> Result<(), OrderedDeploymentDpScheduleError> {
	for (source_index, action) in actions.iter().enumerate() {
		for (grant_index, grant) in action.grants.iter().enumerate() {
			if grant.amount == 0 {
				return Err(OrderedDeploymentDpScheduleError::EmptyGrant {
					source_index,
					stable_key: action.stable_key.clone(),
					grant_index,
				});
			}
			if grant.period_ticks == 0 {
				return Err(OrderedDeploymentDpScheduleError::NonPositiveGrantPeriod {
					source_index,
					stable_key: action.stable_key.clone(),
					grant_index,
				});
			}
		}
	}
	Ok(())
}

fn validate_ordered_action_keys(
	actions: &[OrderedDeploymentDpAction],
) -> Result<(), OrderedDeploymentDpScheduleError> {
	let mut stable_keys: BTreeSet<&str> = BTreeSet::new();
	for (source_index, action) in actions.iter().enumerate() {
		if action.stable_key.trim().is_empty() {
			return Err(OrderedDeploymentDpScheduleError::BlankStableKey { source_index });
		}
		if !stable_keys.insert(action.stable_key.as_str()) {
			return Err(OrderedDeploymentDpScheduleError::DuplicateStableKey {
				stable_key: action.stable_key.clone(),
			});
		}
	}
	Ok(())
}

fn validate_event_keys(events: &[DeploymentDpEvent]) -> Result<(), DeploymentDpVerificationError> {
	let mut stable_keys: BTreeSet<&str> = BTreeSet::new();
	for (input_index, event) in events.iter().enumerate() {
		if event.stable_key.trim().is_empty() {
			return Err(DeploymentDpVerificationError::BlankStableKey { input_index });
		}
		if !stable_keys.insert(event.stable_key.as_str()) {
			return Err(DeploymentDpVerificationError::DuplicateStableKey {
				stable_key: event.stable_key.clone(),
			});
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::{
		schedule_ordered_deployments, verify_deployment_dp_prefix, DeploymentDpEvent,
		DeploymentDpRules, DeploymentDpVerificationError, OrderedDeploymentDpAction,
		OrderedDeploymentDpScheduleError, PlannedDeploymentDpGrant,
	};

	fn rules() -> DeploymentDpRules {
		DeploymentDpRules::new(5, 20, 10, 4)
	}

	#[test]
	fn a_grant_lands_off_the_regeneration_grid_and_is_spendable_in_its_own_tick() {
		// Regeneration alone credits 4 DP every 10 ticks from an emptied pool, so a 14 DP
		// deployment would wait until tick 40. A 6 DP grant at tick 23 covers the deficit at a
		// tick no regeneration boundary falls on, and the deployment executes in that tick.
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 5, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(23, 1_000, 6)]),
			OrderedDeploymentDpAction::new("waiter", 14, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules(), &actions).unwrap();

		assert_eq!(schedule[0].deploy_tick, 0);
		assert_eq!(schedule[1].deploy_tick, 23);
		assert_eq!(schedule[1].regenerated_dp, 8);
		assert_eq!(schedule[1].granted_dp, 6);
		assert_eq!(schedule[1].dp_before_deployment, 14);
		assert_eq!(schedule[1].dp_after_deployment, 0);
		// The audit still counts the regeneration intervals that closed before the grant tick.
		assert_eq!(schedule[1].completed_regen_intervals, 2);

		// Without the grant the same deployment waits for the fourth boundary.
		let passive: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 5, None, 0),
			OrderedDeploymentDpAction::new("waiter", 14, None, 0),
		];
		let passive_schedule = schedule_ordered_deployments(rules(), &passive).unwrap();
		assert_eq!(passive_schedule[1].deploy_tick, 40);
		assert_eq!(passive_schedule[1].granted_dp, 0);
	}

	#[test]
	fn a_grant_never_funds_its_own_deployment_but_funds_the_next_in_that_tick() {
		// The granter's own grant resolves at its deployment tick, which is exactly what a skill
		// that starts at full SP does. It must pay 5 DP out of the 5 it carries, and only then
		// may the grant pay for the deployment that follows in the same tick.
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 5, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(0, 1_000, 4)]),
			OrderedDeploymentDpAction::new("follower", 4, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules(), &actions).unwrap();

		assert_eq!(schedule[0].deploy_tick, 0);
		assert_eq!(schedule[0].dp_before_deployment, 5);
		assert_eq!(schedule[0].granted_dp, 0);
		assert_eq!(schedule[0].dp_after_deployment, 0);
		assert_eq!(schedule[1].deploy_tick, 0);
		assert_eq!(schedule[1].granted_dp, 4);
		assert_eq!(schedule[1].dp_before_deployment, 4);
		assert_eq!(schedule[1].dp_after_deployment, 0);
	}

	#[test]
	fn regeneration_is_credited_before_a_grant_sharing_its_tick() {
		// Both resolve at tick 20 with 11 DP carried: regeneration takes the pool to the 20 DP
		// cap and the grant then credits nothing. Crediting the grant first would have recorded
		// 5 granted and 4 regenerated, which is not what replay's own audit records.
		let rules = DeploymentDpRules::new(20, 20, 10, 4);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 9, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(20, 1_000, 5)]),
			OrderedDeploymentDpAction::new("waiter", 20, Some(20), 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		assert_eq!(schedule[1].deploy_tick, 20);
		assert_eq!(schedule[1].dp_before_regeneration, 11);
		assert_eq!(schedule[1].regenerated_dp, 8);
		assert_eq!(schedule[1].granted_dp, 1);
		assert_eq!(schedule[1].dp_before_deployment, 20);
	}

	#[test]
	fn a_grant_arriving_at_the_cap_is_discarded_and_never_restored() {
		let rules = DeploymentDpRules::new(20, 20, 10, 4);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 0, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(5, 1_000, 8)]),
			OrderedDeploymentDpAction::new("late", 20, Some(50), 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		assert_eq!(schedule[1].granted_dp, 0);
		assert_eq!(schedule[1].regenerated_dp, 0);
		assert_eq!(schedule[1].dp_before_deployment, 20);
		assert_eq!(schedule[1].dp_after_deployment, 0);
	}

	#[test]
	fn a_repeating_grant_covers_a_deficit_no_regeneration_can() {
		// A stage with no passive regeneration at all still schedules from grants alone, and the
		// deployment waits for as many periods as its cost needs.
		let rules = DeploymentDpRules::new(2, 20, 10, 0);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 2, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(7, 30, 3)]),
			OrderedDeploymentDpAction::new("waiter", 9, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		// Grants at ticks 7, 37 and 67 credit three DP each.
		assert_eq!(schedule[1].deploy_tick, 67);
		assert_eq!(schedule[1].granted_dp, 9);
		assert_eq!(schedule[1].regenerated_dp, 0);

		// Remove the grant and the same prefix has nothing left that could ever credit DP.
		let ungranted: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("granter", 2, None, 0),
			OrderedDeploymentDpAction::new("waiter", 9, None, 0),
		];
		assert_eq!(
			schedule_ordered_deployments(rules, &ungranted).unwrap_err(),
			OrderedDeploymentDpScheduleError::InsufficientDpWithoutCredit {
				source_index: 1,
				stable_key: "waiter".to_owned(),
				deployment_cost: 9,
				minimum_dp_before_deployment: 0,
				required_dp: 9,
				available_dp: 0,
			}
		);
	}

	#[test]
	fn two_granters_credit_in_a_shared_tick_and_keep_their_own_periods() {
		let rules = DeploymentDpRules::new(6, 40, 10, 1);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("fast", 3, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(4, 8, 2)]),
			OrderedDeploymentDpAction::new("slow", 3, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(12, 12, 5)]),
			OrderedDeploymentDpAction::new("waiter", 14, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		// Both granters deploy in tick 0 and empty the pool. The fast grant then fires at 4, 12,
		// 20 and the slow one at 12, 24 — they share tick 12 and keep their own periods — for 16
		// granted DP, beside the regeneration boundaries at 10 and 20.
		assert_eq!(schedule[0].deploy_tick, 0);
		assert_eq!(schedule[1].deploy_tick, 0);
		assert_eq!(schedule[2].deploy_tick, 24);
		assert_eq!(schedule[2].granted_dp, 16);
		assert_eq!(schedule[2].regenerated_dp, 2);
		assert_eq!(schedule[2].dp_before_deployment, 18);
		assert_eq!(schedule[2].dp_after_deployment, 4);
	}

	#[test]
	fn rejects_a_grant_that_credits_nothing_or_never_repeats() {
		let empty: Vec<OrderedDeploymentDpAction> =
			vec![OrderedDeploymentDpAction::new("granter", 0, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(1, 10, 0)])];
		assert_eq!(
			schedule_ordered_deployments(rules(), &empty).unwrap_err(),
			OrderedDeploymentDpScheduleError::EmptyGrant {
				source_index: 0,
				stable_key: "granter".to_owned(),
				grant_index: 0,
			}
		);

		let endless: Vec<OrderedDeploymentDpAction> =
			vec![OrderedDeploymentDpAction::new("granter", 0, None, 0)
				.with_grants(vec![PlannedDeploymentDpGrant::new(1, 0, 3)])];
		assert_eq!(
			schedule_ordered_deployments(rules(), &endless).unwrap_err(),
			OrderedDeploymentDpScheduleError::NonPositiveGrantPeriod {
				source_index: 0,
				stable_key: "granter".to_owned(),
				grant_index: 0,
			}
		);
	}

	#[test]
	fn schedules_earliest_affordable_ticks_in_source_order() {
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("zulu", 3, None, 5),
			OrderedDeploymentDpAction::new("alpha", 8, None, 0),
			OrderedDeploymentDpAction::new("middle", 6, Some(35), 0),
			OrderedDeploymentDpAction::new("final", 4, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules(), &actions).unwrap();

		assert_eq!(
			schedule
				.iter()
				.map(|scheduled| scheduled.action.stable_key.as_str())
				.collect::<Vec<&str>>(),
			vec!["zulu", "alpha", "middle", "final"]
		);
		assert_eq!(
			schedule
				.iter()
				.map(|scheduled| scheduled.source_index)
				.collect::<Vec<usize>>(),
			vec![0, 1, 2, 3]
		);
		assert_eq!(
			schedule
				.iter()
				.map(|scheduled| scheduled.deploy_tick)
				.collect::<Vec<u64>>(),
			vec![0, 20, 35, 40]
		);
		assert_eq!(schedule[0].completion_tick, 5);
		assert_eq!(schedule[0].dp_before_deployment, 5);
		assert_eq!(schedule[0].dp_after_deployment, 2);
		assert_eq!(schedule[1].ready_tick, 5);
		assert_eq!(schedule[1].newly_completed_regen_intervals, 2);
		assert_eq!(schedule[1].regenerated_dp, 8);
		assert_eq!(schedule[1].dp_before_deployment, 10);
		assert_eq!(schedule[1].dp_after_deployment, 2);
		assert_eq!(schedule[2].ready_tick, 35);
		assert_eq!(schedule[2].regenerated_dp, 4);
		assert_eq!(schedule[2].dp_before_deployment, 6);
		assert_eq!(schedule[2].dp_after_deployment, 0);
		assert_eq!(schedule[3].ready_tick, 35);
		assert_eq!(schedule[3].regenerated_dp, 4);
	}

	#[test]
	fn prior_completion_dominates_an_earlier_explicit_guard() {
		let rules = DeploymentDpRules::new(0, 10, 10, 1);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("first", 0, Some(10), 25),
			OrderedDeploymentDpAction::new("second", 0, Some(20), 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		assert_eq!(schedule[0].deploy_tick, 10);
		assert_eq!(schedule[0].completion_tick, 35);
		assert_eq!(schedule[1].ready_tick, 35);
		assert_eq!(schedule[1].deploy_tick, 35);
	}

	#[test]
	fn same_tick_actions_spend_dp_in_source_order() {
		let rules = DeploymentDpRules::new(10, 10, 10, 5);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("first", 4, None, 0),
			OrderedDeploymentDpAction::new("second", 6, None, 0),
			OrderedDeploymentDpAction::new("third", 1, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		assert_eq!(schedule[0].deploy_tick, 0);
		assert_eq!(schedule[0].dp_after_deployment, 6);
		assert_eq!(schedule[1].deploy_tick, 0);
		assert_eq!(schedule[1].dp_before_deployment, 6);
		assert_eq!(schedule[1].dp_after_deployment, 0);
		assert_eq!(schedule[2].ready_tick, 0);
		assert_eq!(schedule[2].deploy_tick, 10);
		assert_eq!(schedule[2].dp_before_deployment, 5);
	}

	#[test]
	fn discarded_regeneration_is_not_restored_after_a_guarded_deployment() {
		let rules = DeploymentDpRules::new(5, 5, 10, 2);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("guarded", 4, Some(20), 0),
			OrderedDeploymentDpAction::new("following", 4, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		assert_eq!(schedule[0].deploy_tick, 20);
		assert_eq!(schedule[0].regenerated_dp, 0);
		assert_eq!(schedule[0].dp_after_deployment, 1);
		assert_eq!(schedule[1].ready_tick, 20);
		assert_eq!(schedule[1].deploy_tick, 40);
		assert_eq!(schedule[1].regenerated_dp, 4);
		assert_eq!(schedule[1].dp_before_deployment, 5);
	}

	#[test]
	fn rejects_a_cost_above_the_stage_maximum() {
		let rules = DeploymentDpRules::new(5, 10, 10, 2);
		let actions: Vec<OrderedDeploymentDpAction> =
			vec![OrderedDeploymentDpAction::new("impossible", 11, None, 0)];

		let error = schedule_ordered_deployments(rules, &actions).unwrap_err();

		assert_eq!(
			error,
			OrderedDeploymentDpScheduleError::DeploymentCostExceedsMaximum {
				source_index: 0,
				stable_key: "impossible".to_owned(),
				cost: 11,
				max_dp: 10,
			}
		);
	}

	#[test]
	fn costs_guard_delays_deployment_without_being_spent() {
		let rules = DeploymentDpRules::new(5, 20, 10, 4);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("guarded", 3, None, 0)
				.with_minimum_dp_before_deployment(9),
			OrderedDeploymentDpAction::new("following", 6, None, 0),
		];

		let schedule = schedule_ordered_deployments(rules, &actions).unwrap();

		assert_eq!(schedule[0].deploy_tick, 10);
		assert_eq!(schedule[0].dp_before_deployment, 9);
		assert_eq!(schedule[0].dp_after_deployment, 6);
		assert_eq!(schedule[1].deploy_tick, 10);
		assert_eq!(schedule[1].dp_before_deployment, 6);
		assert_eq!(schedule[1].dp_after_deployment, 0);
	}

	#[test]
	fn rejects_a_costs_guard_above_the_stage_maximum() {
		let rules = DeploymentDpRules::new(5, 10, 10, 2);
		let actions: Vec<OrderedDeploymentDpAction> =
			vec![
				OrderedDeploymentDpAction::new("impossible-guard", 1, None, 0)
					.with_minimum_dp_before_deployment(11),
			];

		let error = schedule_ordered_deployments(rules, &actions).unwrap_err();

		assert_eq!(
			error,
			OrderedDeploymentDpScheduleError::MinimumDpBeforeDeploymentExceedsMaximum {
				source_index: 0,
				stable_key: "impossible-guard".to_owned(),
				minimum_dp_before_deployment: 11,
				max_dp: 10,
			}
		);
	}

	#[test]
	fn zero_regeneration_fails_only_when_the_carried_dp_is_insufficient() {
		let rules = DeploymentDpRules::new(3, 10, 10, 0);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("affordable", 3, Some(50), 0),
			OrderedDeploymentDpAction::new("unreachable", 1, None, 0),
		];

		let error = schedule_ordered_deployments(rules, &actions).unwrap_err();

		assert_eq!(
			error,
			OrderedDeploymentDpScheduleError::InsufficientDpWithoutCredit {
				source_index: 1,
				stable_key: "unreachable".to_owned(),
				deployment_cost: 1,
				minimum_dp_before_deployment: 0,
				required_dp: 1,
				available_dp: 0,
			}
		);
	}

	#[test]
	fn reports_deployment_tick_overflow() {
		let rules = DeploymentDpRules::new(0, 1, u64::MAX, 1);
		let actions: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("at-last-boundary", 1, Some(u64::MAX), 0),
			OrderedDeploymentDpAction::new("past-last-boundary", 1, None, 0),
		];

		let error = schedule_ordered_deployments(rules, &actions).unwrap_err();

		assert_eq!(
			error,
			OrderedDeploymentDpScheduleError::DeploymentTickOverflow {
				source_index: 1,
				stable_key: "past-last-boundary".to_owned(),
			}
		);
	}

	#[test]
	fn reports_action_completion_tick_overflow() {
		let rules = DeploymentDpRules::new(0, 0, 1, 0);
		let actions: Vec<OrderedDeploymentDpAction> = vec![OrderedDeploymentDpAction::new(
			"overflowing-completion",
			0,
			Some(u64::MAX),
			1,
		)];

		let error = schedule_ordered_deployments(rules, &actions).unwrap_err();

		assert_eq!(
			error,
			OrderedDeploymentDpScheduleError::CompletionTickOverflow {
				source_index: 0,
				stable_key: "overflowing-completion".to_owned(),
				deploy_tick: u64::MAX,
				completion_delay_ticks: 1,
			}
		);
	}

	#[test]
	fn ordered_scheduler_rejects_blank_and_duplicate_keys() {
		let blank: Vec<OrderedDeploymentDpAction> =
			vec![OrderedDeploymentDpAction::new(" ", 0, None, 0)];
		assert_eq!(
			schedule_ordered_deployments(rules(), &blank),
			Err(OrderedDeploymentDpScheduleError::BlankStableKey { source_index: 0 })
		);

		let duplicate: Vec<OrderedDeploymentDpAction> = vec![
			OrderedDeploymentDpAction::new("duplicate", 0, None, 0),
			OrderedDeploymentDpAction::new("duplicate", 0, None, 0),
		];
		assert_eq!(
			schedule_ordered_deployments(rules(), &duplicate),
			Err(OrderedDeploymentDpScheduleError::DuplicateStableKey {
				stable_key: "duplicate".to_owned(),
			})
		);
	}

	#[test]
	fn audits_events_in_canonical_order_with_exact_prefix_accounting() {
		let events: Vec<DeploymentDpEvent> = vec![
			DeploymentDpEvent::new(10, 1, "bravo", 4),
			DeploymentDpEvent::new(20, 0, "delta", 3),
			DeploymentDpEvent::new(0, 0, "alpha", 3),
			DeploymentDpEvent::new(10, 0, "charlie", 2),
		];

		let audits = verify_deployment_dp_prefix(rules(), &events).unwrap();

		assert_eq!(
			audits
				.iter()
				.map(|audit| audit.event.stable_key.as_str())
				.collect::<Vec<&str>>(),
			vec!["alpha", "charlie", "bravo", "delta"]
		);
		assert_eq!(audits[0].completed_regen_intervals, 0);
		assert_eq!(audits[0].newly_completed_regen_intervals, 0);
		assert_eq!(audits[0].regenerated_dp, 0);
		assert_eq!(audits[0].dp_before_regeneration, 5);
		assert_eq!(audits[0].dp_before_deployment, 5);
		assert_eq!(audits[0].dp_after_deployment, 2);
		assert_eq!(audits[1].completed_regen_intervals, 1);
		assert_eq!(audits[1].newly_completed_regen_intervals, 1);
		assert_eq!(audits[1].regenerated_dp, 4);
		assert_eq!(audits[1].dp_before_regeneration, 2);
		assert_eq!(audits[1].dp_before_deployment, 6);
		assert_eq!(audits[1].dp_after_deployment, 4);
		assert_eq!(audits[2].newly_completed_regen_intervals, 0);
		assert_eq!(audits[2].regenerated_dp, 0);
		assert_eq!(audits[2].dp_before_regeneration, 4);
		assert_eq!(audits[2].dp_before_deployment, 4);
		assert_eq!(audits[2].dp_after_deployment, 0);
		assert_eq!(audits[3].completed_regen_intervals, 2);
		assert_eq!(audits[3].newly_completed_regen_intervals, 1);
		assert_eq!(audits[3].regenerated_dp, 4);
		assert_eq!(audits[3].dp_before_regeneration, 0);
		assert_eq!(audits[3].dp_before_deployment, 4);
		assert_eq!(audits[3].dp_after_deployment, 1);
	}

	#[test]
	fn stable_key_breaks_equal_same_tick_orders_independently_of_input_order() {
		let forward: Vec<DeploymentDpEvent> = vec![
			DeploymentDpEvent::new(0, 7, "alpha", 2),
			DeploymentDpEvent::new(0, 7, "beta", 3),
		];
		let reverse: Vec<DeploymentDpEvent> = forward.iter().cloned().rev().collect();

		let forward_audit = verify_deployment_dp_prefix(rules(), &forward).unwrap();
		let reverse_audit = verify_deployment_dp_prefix(rules(), &reverse).unwrap();

		assert_eq!(forward_audit, reverse_audit);
		assert_eq!(forward_audit[0].event.stable_key, "alpha");
		assert_eq!(forward_audit[1].event.stable_key, "beta");
		assert_eq!(forward_audit[1].newly_completed_regen_intervals, 0);
		assert_eq!(forward_audit[1].dp_before_deployment, 3);
	}

	#[test]
	fn caps_incremental_regeneration_before_deployment() {
		let rules = DeploymentDpRules::new(5, 6, 10, 4);
		let events: Vec<DeploymentDpEvent> = vec![
			DeploymentDpEvent::new(0, 0, "initial-spend", 5),
			DeploymentDpEvent::new(20, 0, "capped", 6),
		];

		let audits = verify_deployment_dp_prefix(rules, &events).unwrap();

		assert_eq!(audits[1].completed_regen_intervals, 2);
		assert_eq!(audits[1].newly_completed_regen_intervals, 2);
		assert_eq!(audits[1].regenerated_dp, 6);
		assert_eq!(audits[1].dp_before_regeneration, 0);
		assert_eq!(audits[1].dp_before_deployment, 6);
		assert_eq!(audits[1].dp_after_deployment, 0);
	}

	#[test]
	fn does_not_restore_regeneration_discarded_at_an_earlier_cap() {
		let rules = DeploymentDpRules::new(5, 5, 10, 2);
		let events: Vec<DeploymentDpEvent> = vec![
			DeploymentDpEvent::new(20, 0, "after-overflow", 4),
			DeploymentDpEvent::new(30, 0, "one-interval-later", 4),
		];

		let error = verify_deployment_dp_prefix(rules, &events).unwrap_err();

		assert_eq!(
			error,
			DeploymentDpVerificationError::InsufficientDp {
				stable_key: "one-interval-later".to_owned(),
				tick: 30,
				order: 0,
				cost: 4,
				available_dp: 3,
				deficit: 1,
			}
		);
	}

	#[test]
	fn reports_the_exact_deficit_at_the_first_infeasible_event() {
		let rules = DeploymentDpRules::new(3, 10, 10, 2);
		let events: Vec<DeploymentDpEvent> = vec![DeploymentDpEvent::new(9, 4, "too-early", 5)];

		let error = verify_deployment_dp_prefix(rules, &events).unwrap_err();

		assert_eq!(
			error,
			DeploymentDpVerificationError::InsufficientDp {
				stable_key: "too-early".to_owned(),
				tick: 9,
				order: 4,
				cost: 5,
				available_dp: 3,
				deficit: 2,
			}
		);
	}

	#[test]
	fn rejects_duplicate_stable_keys_before_auditing() {
		let events: Vec<DeploymentDpEvent> = vec![
			DeploymentDpEvent::new(0, 0, "duplicate", 1),
			DeploymentDpEvent::new(10, 0, "duplicate", 1),
		];

		let error = verify_deployment_dp_prefix(rules(), &events).unwrap_err();

		assert_eq!(
			error,
			DeploymentDpVerificationError::DuplicateStableKey {
				stable_key: "duplicate".to_owned(),
			}
		);
	}

	#[test]
	fn rejects_zero_regen_interval() {
		let rules = DeploymentDpRules::new(5, 20, 0, 1);

		let error = verify_deployment_dp_prefix(rules, &[]).unwrap_err();

		assert_eq!(error, DeploymentDpVerificationError::ZeroRegenIntervalTicks);
	}

	#[test]
	fn rejects_initial_dp_above_maximum() {
		let rules = DeploymentDpRules::new(21, 20, 10, 1);

		let error = verify_deployment_dp_prefix(rules, &[]).unwrap_err();

		assert_eq!(
			error,
			DeploymentDpVerificationError::InitialDpExceedsMaximum {
				initial_dp: 21,
				max_dp: 20,
			}
		);
	}

	#[test]
	fn caps_regeneration_without_overflowing_the_machine_integer() {
		let rules = DeploymentDpRules::new(0, u64::MAX, 1, u64::MAX);
		let events: Vec<DeploymentDpEvent> = vec![DeploymentDpEvent::new(
			2,
			0,
			"capped-regeneration",
			u64::MAX,
		)];

		let audits = verify_deployment_dp_prefix(rules, &events).unwrap();

		assert_eq!(audits[0].regenerated_dp, u64::MAX);
		assert_eq!(audits[0].dp_before_deployment, u64::MAX);
		assert_eq!(audits[0].dp_after_deployment, 0);
	}

	#[test]
	fn caps_initial_plus_regeneration_without_overflowing() {
		let rules = DeploymentDpRules::new(1, u64::MAX, 1, u64::MAX);
		let events: Vec<DeploymentDpEvent> =
			vec![DeploymentDpEvent::new(1, 0, "capped-balance", u64::MAX)];

		let audits = verify_deployment_dp_prefix(rules, &events).unwrap();

		assert_eq!(audits[0].regenerated_dp, u64::MAX - 1);
		assert_eq!(audits[0].dp_before_deployment, u64::MAX);
		assert_eq!(audits[0].dp_after_deployment, 0);
	}

	#[test]
	fn permits_zero_regeneration_amount() {
		let rules = DeploymentDpRules::new(3, 3, 1, 0);
		let events: Vec<DeploymentDpEvent> =
			vec![DeploymentDpEvent::new(u64::MAX, 0, "no-regen", 3)];

		let audits = verify_deployment_dp_prefix(rules, &events).unwrap();

		assert_eq!(audits[0].regenerated_dp, 0);
		assert_eq!(audits[0].dp_before_deployment, 3);
		assert_eq!(audits[0].dp_after_deployment, 0);
	}

	#[test]
	fn accepts_an_empty_deployment_prefix() {
		let audits = verify_deployment_dp_prefix(rules(), &[]).unwrap();

		assert!(audits.is_empty());
	}
}
