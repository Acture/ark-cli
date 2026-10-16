use std::sync::{
	atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering},
	mpsc::{self, Sender},
	Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ark_solver::mechanics::domain::Facing;
use ark_solver::planning::TickSpan;
use ark_solver::workflow::{
	LevelSolveCandidateProgress, LevelSolveDeadOperatorProgress, LevelSolveEnemyProgress,
	LevelSolveJointPlanningOutcome, LevelSolvePendingActionProgress, LevelSolveProgressEvent,
	LevelSolveProgressObserver, LevelSolveProgressUpdate, LevelSolveReplayDpStepProgress,
	LevelSolveReplayMemberProgress, LevelSolveReplayVariantOutcome, LevelSolveSelectionEngine,
};

const SOLVE_PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CliSolveProgressPhase {
	Preparing = 0,
	Searching = 1,
	Finished = 2,
}

impl CliSolveProgressPhase {
	const fn label(self) -> &'static str {
		match self {
			Self::Preparing => "准备",
			Self::Searching => "搜索",
			Self::Finished => "完成",
		}
	}

	fn load(value: u8) -> Self {
		match value {
			1 => Self::Searching,
			2 => Self::Finished,
			_ => Self::Preparing,
		}
	}
}

pub(super) struct CliSolveProgressObserver {
	started: Instant,
	proposal_limit: AtomicUsize,
	requested_top_k: AtomicUsize,
	ticks_per_second: AtomicUsize,
	phase: AtomicU8,
	proposals_generated: AtomicUsize,
	proposals_queued: AtomicUsize,
	replay_completed: AtomicUsize,
	verified_jobs: AtomicUsize,
	queue_depth: AtomicUsize,
	in_flight: AtomicUsize,
	selection_completed: AtomicUsize,
	selection_micros: AtomicU64,
	joint_completed: AtomicUsize,
	joint_micros: AtomicU64,
	proposal_samples: Mutex<CliRateSamples>,
	replay_samples: Mutex<CliRateSamples>,
}

#[derive(Default)]
struct CliRateSamples {
	first: Option<Instant>,
	last: Option<Instant>,
	count: usize,
}

impl CliRateSamples {
	fn record(&mut self, at: Instant) {
		self.first.get_or_insert(at);
		self.last = Some(at);
		self.count += 1;
	}

	fn rate_per_second(&self) -> Option<f64> {
		if self.count < 2 {
			return None;
		}
		let elapsed: Duration = self.last?.duration_since(self.first?);
		(elapsed > Duration::ZERO).then(|| (self.count - 1) as f64 / elapsed.as_secs_f64())
	}
}

impl CliSolveProgressObserver {
	pub(super) fn new(started: Instant) -> Self {
		Self {
			started,
			proposal_limit: AtomicUsize::new(0),
			requested_top_k: AtomicUsize::new(0),
			ticks_per_second: AtomicUsize::new(0),
			phase: AtomicU8::new(CliSolveProgressPhase::Preparing as u8),
			proposals_generated: AtomicUsize::new(0),
			proposals_queued: AtomicUsize::new(0),
			replay_completed: AtomicUsize::new(0),
			verified_jobs: AtomicUsize::new(0),
			queue_depth: AtomicUsize::new(0),
			in_flight: AtomicUsize::new(0),
			selection_completed: AtomicUsize::new(0),
			selection_micros: AtomicU64::new(0),
			joint_completed: AtomicUsize::new(0),
			joint_micros: AtomicU64::new(0),
			proposal_samples: Mutex::new(CliRateSamples::default()),
			replay_samples: Mutex::new(CliRateSamples::default()),
		}
	}

	fn ticks_per_second(&self) -> u32 {
		u32::try_from(self.ticks_per_second.load(Ordering::Acquire)).unwrap_or(0)
	}

	pub(super) fn start_heartbeat(self: &Arc<Self>) -> SolveProgressHeartbeat {
		let (stop_sender, stop_receiver) = mpsc::channel::<()>();
		let progress: Arc<Self> = Arc::clone(self);
		let handle: JoinHandle<()> = thread::spawn(move || loop {
			match stop_receiver.recv_timeout(SOLVE_PROGRESS_HEARTBEAT_INTERVAL) {
				Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
				Err(mpsc::RecvTimeoutError::Timeout) => progress.print_summary("heartbeat"),
			}
		});
		SolveProgressHeartbeat {
			stop_sender: Some(stop_sender),
			handle: Some(handle),
		}
	}

	fn print_summary(&self, trigger: &str) {
		let elapsed: Duration = self.started.elapsed();
		let generated: usize = self.proposals_generated.load(Ordering::Acquire);
		let queued: usize = self.proposals_queued.load(Ordering::Acquire);
		let replayed: usize = self.replay_completed.load(Ordering::Acquire);
		let verified: usize = self.verified_jobs.load(Ordering::Acquire);
		let queue_depth: usize = self.queue_depth.load(Ordering::Acquire);
		let in_flight: usize = self.in_flight.load(Ordering::Acquire);
		let phase: CliSolveProgressPhase =
			CliSolveProgressPhase::load(self.phase.load(Ordering::Acquire));
		let proposal_rate: Option<f64> = self
			.proposal_samples
			.lock()
			.ok()
			.and_then(|samples| samples.rate_per_second());
		let replay_rate: Option<f64> = self
			.replay_samples
			.lock()
			.ok()
			.and_then(|samples| samples.rate_per_second());
		let selection_average: String = format_average_duration(
			self.selection_micros.load(Ordering::Acquire),
			self.selection_completed.load(Ordering::Acquire),
		);
		let joint_average: String = format_average_duration(
			self.joint_micros.load(Ordering::Acquire),
			self.joint_completed.load(Ordering::Acquire),
		);
		let proposal_rate: String =
			proposal_rate.map_or_else(|| "待测".to_owned(), |rate| format!("{rate:.2} 个/秒"));
		let replay_rate: String =
			replay_rate.map_or_else(|| "待测".to_owned(), |rate| format!("{rate:.2} 个/秒"));
		eprintln!(
			"进度[{}] {} | 阶段 {} | 提案 {generated}/{}（{proposal_rate}）| 回放 \
			 {replayed}/{queued}（{replay_rate}）| 有效任务 {verified}/{} | 队列 {queue_depth} / \
			 执行中 {in_flight} | 平均选择 {selection_average} / 联合 {joint_average}",
			progress_trigger_label(trigger),
			format_duration(elapsed),
			phase.label(),
			self.proposal_limit.load(Ordering::Acquire),
			self.requested_top_k.load(Ordering::Acquire),
		);
	}
}

pub(super) struct SolveProgressHeartbeat {
	stop_sender: Option<Sender<()>>,
	handle: Option<JoinHandle<()>>,
}

impl Drop for SolveProgressHeartbeat {
	fn drop(&mut self) {
		if let Some(sender) = self.stop_sender.take() {
			let _result: Result<(), mpsc::SendError<()>> = sender.send(());
		}
		if let Some(handle) = self.handle.take() {
			let _result: thread::Result<()> = handle.join();
		}
	}
}

impl LevelSolveProgressObserver for CliSolveProgressObserver {
	fn observe(&self, update: LevelSolveProgressUpdate) {
		let elapsed: String = format_duration(update.elapsed);
		match update.event {
			LevelSolveProgressEvent::PreparationStarted {
				proposal_limit,
				requested_top_k,
				ticks_per_second,
			} => {
				self.proposal_limit.store(proposal_limit, Ordering::Release);
				self.requested_top_k
					.store(requested_top_k, Ordering::Release);
				self.ticks_per_second.store(
					usize::try_from(ticks_per_second).unwrap_or(usize::MAX),
					Ordering::Release,
				);
				self.phase
					.store(CliSolveProgressPhase::Preparing as u8, Ordering::Release);
				eprintln!(
					"准备求解：最多测试 {proposal_limit} 个提案，目标生成 {requested_top_k} 个有效任务"
				);
			}
			LevelSolveProgressEvent::SearchStarted {
				replay_worker_count,
				replay_queue_capacity,
				replay_inflight_capacity,
			} => {
				self.phase
					.store(CliSolveProgressPhase::Searching as u8, Ordering::Release);
				eprintln!(
					"开始搜索（总耗时 {elapsed}）：{replay_worker_count} 个回放线程，队列容量 \
					 {replay_queue_capacity}，并发上限 {replay_inflight_capacity}"
				);
			}
			LevelSolveProgressEvent::SelectionStarted { proposal_index } => {
				eprintln!("选择 #{}：开始（总耗时 {elapsed}）", proposal_index + 1);
			}
			LevelSolveProgressEvent::SelectionFinished {
				proposal_index,
				duration,
				engine,
				selected_candidate_count,
			} => {
				self.selection_completed.fetch_add(1, Ordering::AcqRel);
				self.selection_micros
					.fetch_add(duration_micros(duration), Ordering::AcqRel);
				eprintln!(
					"选择 #{}：{}，选中 {selected_candidate_count} 名干员，耗时 {}",
					proposal_index + 1,
					selection_engine_label(engine),
					format_duration(duration),
				);
			}
			LevelSolveProgressEvent::JointPlanningStarted {
				proposal_index,
				selected_candidates,
			} => {
				eprintln!(
					"联合规划 #{}：开始（总耗时 {elapsed}，干员：{}）",
					proposal_index + 1,
					format_candidates(&selected_candidates)
				);
			}
			LevelSolveProgressEvent::JointPlanningFinished {
				proposal_index,
				duration,
				outcome,
			} => {
				self.joint_completed.fetch_add(1, Ordering::AcqRel);
				self.joint_micros
					.fetch_add(duration_micros(duration), Ordering::AcqRel);
				eprintln!(
					"联合规划 #{}：{}，耗时 {}",
					proposal_index + 1,
					format_joint_outcome(&outcome),
					format_duration(duration),
				);
			}
			LevelSolveProgressEvent::ProposalGenerated {
				proposal_index,
				selected_candidates,
				total_deployment_cost,
				replay_required,
				proposal_limit,
			} => {
				let generated: usize = proposal_index + 1;
				self.proposals_generated.store(generated, Ordering::Release);
				if let Ok(mut samples) = self.proposal_samples.lock() {
					samples.record(Instant::now());
				}
				eprintln!(
					"提案 #{}/{}：已生成（总耗时 {elapsed}，总费用 {total_deployment_cost} DP，{}）\n{}",
					proposal_index + 1,
					proposal_limit,
					if replay_required {
						"需要回放"
					} else {
						"无需回放"
					},
					format_proposal_candidates(
						&selected_candidates,
						self.ticks_per_second(),
					)
				);
			}
			LevelSolveProgressEvent::ProposalQueued {
				proposal_index,
				replay_sequence,
				queue_depth,
				in_flight,
			} => {
				self.proposals_queued
					.fetch_max(replay_sequence + 1, Ordering::AcqRel);
				self.queue_depth.store(queue_depth, Ordering::Release);
				self.in_flight.store(in_flight, Ordering::Release);
				eprintln!(
					"提案 #{}：进入回放 #{}（总耗时 {elapsed}，队列 {queue_depth}，执行中 \
					 {in_flight}）",
					proposal_index + 1,
					replay_sequence + 1,
				);
			}
			LevelSolveProgressEvent::ReplayProposalStarted {
				proposal_index,
				replay_sequence,
				selected_candidates,
			} => {
				eprintln!(
					"开始回放 #{}（提案 #{}，总耗时 {elapsed}）：{}",
					replay_sequence + 1,
					proposal_index + 1,
					format_candidates(&selected_candidates)
				);
			}
			LevelSolveProgressEvent::ReplayVariantStarted {
				proposal_index,
				replay_sequence,
				variant_index,
				variant_count,
				guide_variant_id: _,
				members,
				dp_order,
			} => {
				eprintln!(
					"{}",
					format_replay_variant_plan(ReplayVariantPlanFormat {
						proposal_index,
						replay_sequence,
						variant_index,
						variant_count,
						elapsed: &elapsed,
						members: &members,
						dp_order: &dp_order,
						ticks_per_second: self.ticks_per_second(),
					})
				);
			}
			LevelSolveProgressEvent::ReplayVariantFinished {
				proposal_index,
				replay_sequence,
				variant_index,
				variant_count,
				duration,
				outcome,
			} => {
				eprintln!(
					"测试方案 #{variant_index}/{variant_count} 结果（提案 #{}，回放 #{}，耗时 \
					 {}，总耗时 {elapsed}）\n{}",
					proposal_index + 1,
					replay_sequence + 1,
					format_duration(duration),
					format_replay_outcome(&outcome, self.ticks_per_second())
				);
			}
			LevelSolveProgressEvent::ReplayProposalExportFailed {
				proposal_index,
				replay_sequence,
				reason_code,
				detail,
			} => {
				eprintln!(
					"提案 #{} / 回放 #{}：胜利但无法导出（总耗时 {elapsed}）\n  机制原因：\
					 {reason_code}\n  详情：{}",
					proposal_index + 1,
					replay_sequence + 1,
					compact_text(&detail, 240),
				);
			}
			LevelSolveProgressEvent::ReplayProposalMaterialized {
				proposal_index,
				replay_sequence,
				artifact_id: _,
			} => {
				eprintln!(
					"提案 #{} / 回放 #{}：回放线程已物化可导出任务，等待按序接纳（总耗时 \
					 {elapsed}）",
					proposal_index + 1,
					replay_sequence + 1,
				);
			}
			LevelSolveProgressEvent::ReplayProposalAccepted {
				proposal_index,
				replay_sequence,
				artifact_id: _,
			} => {
				eprintln!(
					"提案 #{} / 回放 #{}：已按序接纳为有效任务（总耗时 {elapsed}）",
					proposal_index + 1,
					replay_sequence + 1,
				);
			}
			LevelSolveProgressEvent::ReplayCompleted {
				proposal_index,
				replay_sequence,
				replay_completed,
				verified_jobs,
				queue_depth,
				in_flight,
				duration,
			} => {
				self.replay_completed
					.store(replay_completed, Ordering::Release);
				self.verified_jobs.store(verified_jobs, Ordering::Release);
				self.queue_depth.store(queue_depth, Ordering::Release);
				self.in_flight.store(in_flight, Ordering::Release);
				if let Ok(mut samples) = self.replay_samples.lock() {
					samples.record(Instant::now());
				}
				eprintln!(
					"回放 #{}（提案 #{}）：完成，耗时 {}（总耗时 {elapsed}）；已完成 \
					 {replay_completed}，有效任务 {verified_jobs}，队列 {queue_depth}，执行中 \
					 {in_flight}",
					replay_sequence + 1,
					proposal_index + 1,
					format_duration(duration),
				);
				self.print_summary("replay");
			}
			LevelSolveProgressEvent::SearchFinished {
				proposals_generated,
				proposals_queued,
				replay_completed,
				verified_jobs,
			} => {
				self.proposals_generated
					.store(proposals_generated, Ordering::Release);
				self.proposals_queued
					.store(proposals_queued, Ordering::Release);
				self.replay_completed
					.store(replay_completed, Ordering::Release);
				self.verified_jobs.store(verified_jobs, Ordering::Release);
				self.queue_depth.store(0, Ordering::Release);
				self.in_flight.store(0, Ordering::Release);
				self.phase
					.store(CliSolveProgressPhase::Finished as u8, Ordering::Release);
				self.print_summary("finished");
			}
		}
	}
}

struct ReplayVariantPlanFormat<'a> {
	proposal_index: usize,
	replay_sequence: usize,
	variant_index: usize,
	variant_count: usize,
	elapsed: &'a str,
	members: &'a [LevelSolveReplayMemberProgress],
	dp_order: &'a [LevelSolveReplayDpStepProgress],
	ticks_per_second: u32,
}

fn format_replay_variant_plan(plan: ReplayVariantPlanFormat<'_>) -> String {
	let ReplayVariantPlanFormat {
		proposal_index,
		replay_sequence,
		variant_index,
		variant_count,
		elapsed,
		members,
		dp_order,
		ticks_per_second,
	} = plan;
	let mut lines: Vec<String> = vec![format!(
		"测试方案 #{variant_index}/{variant_count}（提案 #{}，回放 #{}，总耗时 {elapsed}）",
		proposal_index + 1,
		replay_sequence + 1,
	)];
	if members.is_empty() {
		lines.push("  无部署干员".to_owned());
	} else {
		for (member_index, member) in members.iter().enumerate() {
			lines.extend([
				format!("  干员 {}", member_index + 1),
				format!(
					"    部署时间(秒)：{}",
					format_tick_seconds_value(member.deploy_tick, ticks_per_second)
				),
				format!("    干员名：{}", format_candidate_name(&member.candidate)),
				format!(
					"    E等级技能潜能：{}",
					format_candidate_build(&member.candidate)
				),
				format!(
					"    位置朝向：{}",
					format_candidate_position(&member.candidate)
				),
				format!(
					"    开技能时间段：{}",
					format_active_spans(&member.active_spans, ticks_per_second)
				),
			]);
		}
	}
	lines.push("  DP消耗时序：".to_owned());
	lines.push(format_replay_dp_order(dp_order, ticks_per_second));
	lines.join("\n")
}

fn format_candidates(candidates: &[LevelSolveCandidateProgress]) -> String {
	if candidates.is_empty() {
		return "无".to_owned();
	}
	candidates
		.iter()
		.map(format_candidate_name)
		.collect::<Vec<_>>()
		.join("、")
}

fn format_proposal_candidates(
	candidates: &[LevelSolveCandidateProgress],
	ticks_per_second: u32,
) -> String {
	if candidates.is_empty() {
		return "  无候选干员".to_owned();
	}
	candidates
		.iter()
		.enumerate()
		.map(|(candidate_index, candidate)| {
			let deploy_time: String = candidate.deploy_tick.map_or_else(
				|| "未知".to_owned(),
				|tick| format_tick_seconds_value(tick, ticks_per_second),
			);
			format!(
				"  #{} {} | {} | {} | 部署 {} 秒 | 技能 {}",
				candidate_index + 1,
				format_candidate_name(candidate),
				format_candidate_build(candidate),
				format_candidate_position(candidate),
				deploy_time,
				format_active_spans(&candidate.active_spans, ticks_per_second),
			)
		})
		.collect::<Vec<_>>()
		.join("\n")
}

fn format_candidate_name(candidate: &LevelSolveCandidateProgress) -> String {
	candidate
		.operator_name
		.as_deref()
		.map(str::trim)
		.filter(|name| !name.is_empty())
		.unwrap_or("未知干员")
		.to_owned()
}

fn format_candidate_build(candidate: &LevelSolveCandidateProgress) -> String {
	let elite_level: String = match (candidate.elite, candidate.level) {
		(Some(elite), Some(level)) => format!("E{elite} L{level}"),
		_ => "E等级未知".to_owned(),
	};
	let skill: String = match (candidate.skill, candidate.skill_level) {
		(Some(skill), Some(level)) => format!("技能 {skill} Lv{level}"),
		_ => "技能未知".to_owned(),
	};
	let potential: String = candidate
		.potential
		.map_or_else(|| "潜能未知".to_owned(), |rank| format!("潜能 {rank}"));
	let module: String = candidate
		.module
		.as_deref()
		.map(str::trim)
		.filter(|module| !module.is_empty())
		.map_or_else(
			|| "模组未知".to_owned(),
			|module| {
				if module == "None" {
					"无模组".to_owned()
				} else {
					format!("模组 {module}")
				}
			},
		);
	format!("{elite_level} / {skill} / {potential} / {module}")
}

fn format_candidate_position(candidate: &LevelSolveCandidateProgress) -> String {
	match (candidate.deployment_cell, candidate.facing) {
		(Some(cell), Some(facing)) => {
			format!("({}, {}) / {}", cell.row, cell.col, facing_label(facing))
		}
		_ => "未知".to_owned(),
	}
}

fn format_active_spans(spans: &[TickSpan], ticks_per_second: u32) -> String {
	if spans.is_empty() {
		return "不开技能".to_owned();
	}
	spans
		.iter()
		.map(|span| {
			format!(
				"{}–{} 秒",
				format_tick_seconds_value(span.start, ticks_per_second),
				format_tick_seconds_value(span.end, ticks_per_second)
			)
		})
		.collect::<Vec<_>>()
		.join("，")
}

const fn facing_label(facing: Facing) -> &'static str {
	match facing {
		Facing::Up => "向上",
		Facing::Right => "向右",
		Facing::Down => "向下",
		Facing::Left => "向左",
	}
}

const fn selection_engine_label(engine: LevelSolveSelectionEngine) -> &'static str {
	match engine {
		LevelSolveSelectionEngine::SingletonFastPath => "单干员快速选择",
		LevelSolveSelectionEngine::MixedInteger => "混合整数规划",
	}
}

fn format_joint_outcome(outcome: &LevelSolveJointPlanningOutcome) -> String {
	match outcome {
		LevelSolveJointPlanningOutcome::Scored {
			damage_feasible,
			minimum_cumulative_coverage_millionths,
			total_due_remaining_hp_scaled,
		} => format!(
			"{}，最低累计覆盖 {:.3}%，剩余 HP {:.3}",
			if *damage_feasible {
				"伤害可行"
			} else {
				"伤害不足"
			},
			*minimum_cumulative_coverage_millionths as f64 / 10_000.0,
			*total_due_remaining_hp_scaled as f64 / 1_000_000.0,
		),
		LevelSolveJointPlanningOutcome::Failed {
			reason_code,
			detail,
		} => format!("失败（{reason_code}）：{}", compact_text(detail, 160)),
	}
}

fn format_replay_dp_order(
	steps: &[LevelSolveReplayDpStepProgress],
	ticks_per_second: u32,
) -> String {
	if steps.is_empty() {
		return "    无 DP 消耗".to_owned();
	}
	steps
		.iter()
		.enumerate()
		.map(|(step_index, step)| {
			format!(
				"    #{} {} 秒：消耗 {} DP，{} → {}",
				step_index + 1,
				format_tick_seconds_value(step.deploy_tick, ticks_per_second),
				step.cost,
				step.dp_before,
				step.dp_after
			)
		})
		.collect::<Vec<_>>()
		.join("\n")
}

fn format_replay_outcome(
	outcome: &LevelSolveReplayVariantOutcome,
	ticks_per_second: u32,
) -> String {
	match outcome {
		LevelSolveReplayVariantOutcome::ExpectedVictory {
			success_probability_millionths,
		} => format!(
			"  结果：胜利\n  胜利概率：{:.6}%",
			f64::from(*success_probability_millionths) / 10_000.0
		),
		LevelSolveReplayVariantOutcome::UnverifiedVictory {
			success_probability_millionths,
			issues,
		} => format!(
			"  结果：胜利但未通过验证\n  胜利概率：{}\n  机制原因：\n{}",
			success_probability_millionths.map_or_else(
				|| "未提供".to_owned(),
				|probability| format!("{:.6}%", f64::from(probability) / 10_000.0)
			),
			format_issues(issues)
		),
		LevelSolveReplayVariantOutcome::GoalLeak {
			terminal,
			leaked_enemies,
			dead_operators,
		} => {
			format!(
				"  结果：漏怪\n  终局原因：{}\n  漏怪详情：\n{}\n  干员死亡：\n{}",
				optional_text(terminal),
				format_enemies(leaked_enemies, ticks_per_second),
				format_dead_operators(dead_operators, ticks_per_second)
			)
		}
		LevelSolveReplayVariantOutcome::ExpectedDefeat {
			terminal,
			remaining_enemies,
			dead_operators,
		} => format!(
			"  结果：战斗失败\n  终局原因：{}\n  剩余敌人：\n{}\n  干员死亡：\n{}",
			optional_text(terminal),
			format_enemies(remaining_enemies, ticks_per_second),
			format_dead_operators(dead_operators, ticks_per_second)
		),
		LevelSolveReplayVariantOutcome::Incomplete {
			terminal,
			issues,
			pending_actions,
			leaked_enemies,
			dead_operators,
			unsupported_count,
			unmodeled_count,
			conservative_count,
			approximated_count,
		} => format!(
			"  结果：回放未完成\n  终局原因：{}\n  待执行动作：\n{}\n  漏怪详情：\n{}\n  干员死亡：\n{}\n  \
			 机制统计：不支持 {unsupported_count}，未建模 {unmodeled_count}，保守省略 \
			 {conservative_count}，已触达近似 {approximated_count}\n  机制原因：\n{}",
			optional_text(terminal),
			format_pending_actions(pending_actions, ticks_per_second),
			format_enemies(leaked_enemies, ticks_per_second),
			format_dead_operators(dead_operators, ticks_per_second),
			format_issues(issues)
		),
		LevelSolveReplayVariantOutcome::Unsupported {
			issues,
			unsupported_count,
			unmodeled_count,
			conservative_count,
			approximated_count,
		} => format!(
			"  结果：存在不支持的机制\n  机制统计：不支持 {unsupported_count}，未建模 \
			 {unmodeled_count}，保守省略 {conservative_count}，已触达近似 \
			 {approximated_count}\n  机制原因：\n{}",
			format_issues(issues)
		),
		LevelSolveReplayVariantOutcome::MaterializationFailed {
			reason_code,
			detail,
		} => format!(
			"  结果：方案无法构造\n  机制原因：{reason_code}\n  详情：{}",
			compact_text(detail, 240)
		),
	}
}

fn format_enemies(enemies: &[LevelSolveEnemyProgress], ticks_per_second: u32) -> String {
	if enemies.is_empty() {
		return "    无".to_owned();
	}
	enemies
		.iter()
		.map(|enemy| {
			let leak_time: String = enemy.leaked_tick.map_or_else(
				|| "未记录".to_owned(),
				|tick| format_tick_seconds_value(tick, ticks_per_second),
			);
			format!(
				"    - 漏怪时间(秒)：{leak_time}；敌人 key：{}；剩余 HP：{:.3}",
				enemy.enemy_key,
				enemy.remaining_hp_micros as f64 / 1_000_000.0,
			)
		})
		.collect::<Vec<_>>()
		.join("\n")
}

fn format_dead_operators(
	operators: &[LevelSolveDeadOperatorProgress],
	ticks_per_second: u32,
) -> String {
	if operators.is_empty() {
		return "    无".to_owned();
	}
	operators
		.iter()
		.map(|operator| {
			format!(
				"    - 死亡时间(秒)：{}；干员：{}",
				format_tick_seconds_value(operator.death_tick, ticks_per_second),
				operator.operator_id,
			)
		})
		.collect::<Vec<_>>()
		.join("\n")
}

fn format_pending_actions(
	actions: &[LevelSolvePendingActionProgress],
	ticks_per_second: u32,
) -> String {
	if actions.is_empty() {
		return "    无".to_owned();
	}
	let mut lines: Vec<String> = actions
		.iter()
		.take(5)
		.map(|action| {
			format!(
				"    - 时间(秒)：{}；动作 #{}：{}；干员：{}；状态：{}",
				format_tick_seconds_value(action.requested_tick, ticks_per_second),
				action.action_index,
				action.kind,
				action.operator_id.as_deref().unwrap_or("无"),
				action.status
			)
		})
		.collect();
	if actions.len() > lines.len() {
		lines.push(format!(
			"    - 另有 {} 个待执行动作",
			actions.len() - lines.len()
		));
	}
	lines.join("\n")
}

fn optional_text(value: &Option<String>) -> &str {
	value.as_deref().unwrap_or("未提供")
}

fn format_issues(issues: &[String]) -> String {
	let mut lines: Vec<String> = issues
		.iter()
		.take(3)
		.map(|issue| format!("    - {}", compact_text(issue, 160)))
		.collect();
	if issues.len() > lines.len() {
		lines.push(format!("    - 另有 {} 条", issues.len() - lines.len()));
	}
	if lines.is_empty() {
		"    无".to_owned()
	} else {
		lines.join("\n")
	}
}

fn format_tick_seconds_value(tick: u64, ticks_per_second: u32) -> String {
	if ticks_per_second == 0 {
		return "不可用".to_owned();
	}
	format!("{:.3}", tick as f64 / f64::from(ticks_per_second))
}

fn progress_trigger_label(trigger: &str) -> &str {
	match trigger {
		"heartbeat" => "定时",
		"proposal" => "提案",
		"replay" => "回放",
		"finished" => "完成",
		_ => "更新",
	}
}

fn compact_text(value: &str, maximum_chars: usize) -> String {
	let mut chars = value.chars();
	let prefix: String = chars.by_ref().take(maximum_chars).collect();
	if chars.next().is_some() {
		format!("{prefix}...")
	} else {
		prefix
	}
}

fn format_duration(duration: Duration) -> String {
	let seconds: f64 = duration.as_secs_f64();
	if seconds >= 60.0 {
		format!("{}m{:.1}s", duration.as_secs() / 60, seconds % 60.0)
	} else {
		format!("{seconds:.1}s")
	}
}

fn duration_micros(duration: Duration) -> u64 {
	u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn format_average_duration(total_micros: u64, count: usize) -> String {
	if count == 0 {
		return "unknown".to_owned();
	}
	let divisor: u64 = u64::try_from(count).unwrap_or(u64::MAX);
	format_duration(Duration::from_micros(total_micros / divisor))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn proposal_log_lists_every_assignment_dimension_in_readable_form() {
		let output: String = format_proposal_candidates(
			&[LevelSolveCandidateProgress {
				candidate_key: "candidate-key".to_owned(),
				operator_id: Some("char_103_angel".to_owned()),
				operator_name: Some("能天使".to_owned()),
				stable_build_id: Some("build".to_owned()),
				elite: Some(2),
				level: Some(90),
				skill: Some(2),
				skill_level: Some(10),
				potential: Some(6),
				module: Some("None".to_owned()),
				deployment_cell: Some(ark_solver::data::GridPosition { row: 5, col: 3 }),
				facing: Some(Facing::Right),
				deploy_tick: Some(60),
				active_spans: vec![ark_solver::planning::TickSpan {
					start: 90,
					end: 120,
				}],
			}],
			30,
		);

		assert!(output.contains("能天使"));
		assert!(output.contains("E2 L90 / 技能 2 Lv10 / 潜能 6"));
		assert!(output.contains("无模组"));
		assert!(output.contains("(5, 3) / 向右"));
		assert!(output.contains("部署 2.000 秒"));
		assert!(output.contains("技能 3.000–4.000 秒"));
	}
}
