use std::{path::PathBuf, process::ExitCode};

use anyhow::Result;
use ark_battle_runtime::{
	comparison, Host, HostConfig, LadderSearch, PlayerSource, Presentation, RunOptions, Training,
};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::Value;

#[derive(Debug, Args)]
pub(crate) struct BattleCommand {
	/// Owned state directory (default: $ARK_BATTLE_RUNTIME_STATE or ./assets/battle-runtime).
	#[arg(long, global = true)]
	state: Option<PathBuf>,
	/// Official PC client directory (default: $ARK_PC_CLIENT, the one setup recorded, or the
	/// Hypergryph Launcher default location).
	#[arg(long, global = true)]
	client: Option<PathBuf>,
	#[command(subcommand)]
	command: Action,
}

/// Generated squad training; every axis defaults to the original maximum.
#[derive(Debug, Args)]
struct TrainingArgs {
	/// Level at the highest elite phase: `max` or 1..90 (capped by the original phase maximum).
	#[arg(long, default_value = "max", value_parser = max_or_number)]
	level: MaxOr,
	/// Specialization rank 0..3 (skill level 7 plus this, elite 2 only).
	#[arg(long, default_value_t = 3, value_parser = clap::value_parser!(i64).range(0..=3))]
	mastery: i64,
	/// Module level 0..3; 0 equips none. The job's module type, else the first module.
	#[arg(long, default_value_t = 3, value_parser = clap::value_parser!(i64).range(0..=3))]
	module: i64,
	/// Trust: `max` or a percent 0..200 from the original favor frames.
	#[arg(long, default_value = "max", value_parser = max_or_number)]
	trust: MaxOr,
}

impl TrainingArgs {
	fn training(&self) -> Training {
		Training {
			level: self.level.0,
			mastery: self.mastery,
			module: self.module,
			trust: self.trust.0,
		}
	}
}

/// Battle options shared by run and ladder.
#[derive(Debug, Args)]
struct BattleArgs {
	#[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(0..=2_147_483_647))]
	seed: u32,
	#[arg(long, default_value_t = 36_000, value_parser = clap::value_parser!(u32).range(1..=54_000))]
	max_ticks: u32,
	#[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=8))]
	resource_rounds: u32,
	/// Skip the client's Lua code hotfixes, to compare their effect on a battle.
	#[arg(long)]
	no_lua_hotfix: bool,
	/// Record every change of the operators' original skill and unit state per tick.
	#[arg(long)]
	trace: bool,
	/// How the battle is driven: fast (original manual ticks, fastest and repeatable), frames
	/// (the original loop as in the client, hidden) or watch (the original loop, visible).
	#[arg(long, value_enum, default_value_t = Present::Fast)]
	presentation: Present,
	/// Original speed level a presented battle starts at: 1 or 2.
	#[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=2))]
	speed: u32,
	/// Game seconds per real second in a presented battle: 1, 2, 4, 8 or 16.
	#[arg(long, default_value_t = 1, value_parser = time_scale)]
	time_scale: u32,
	/// Ticks every unit operation waits after its conditions are met: a declared latency, such
	/// as a screen-recognition client's (30 ticks = 1 s at speed 1).
	#[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=900))]
	delay: u32,
}

fn time_scale(value: &str) -> Result<u32, String> {
	match value.parse::<u32>() {
		Ok(scale @ (1 | 2 | 4 | 8 | 16)) => Ok(scale),
		_ => Err(format!("expected 1, 2, 4, 8 or 16, got {value}")),
	}
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Present {
	Fast,
	Frames,
	Watch,
}

impl BattleArgs {
	fn options(&self, repeat: bool) -> RunOptions {
		RunOptions {
			seed: self.seed,
			repeat,
			max_ticks: self.max_ticks,
			resource_rounds: self.resource_rounds,
			lua_hotfix: !self.no_lua_hotfix,
			trace: self.trace,
			presentation: match self.presentation {
				Present::Fast => Presentation::Fast,
				Present::Frames => Presentation::Frames,
				Present::Watch => Presentation::Watch,
			},
			speed: self.speed,
			time_scale: self.time_scale,
			delay: self.delay,
		}
	}
}

/// `max` (the original maximum) or an explicit number.
#[derive(Clone, Copy, Debug)]
struct MaxOr(Option<i64>);

fn max_or_number(value: &str) -> Result<MaxOr, String> {
	if value == "max" {
		return Ok(MaxOr(None));
	}
	value
		.parse::<i64>()
		.map(|number| MaxOr(Some(number)))
		.map_err(|_| format!("expected `max` or a number, got {value}"))
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Search {
	Tiers,
	Bisect,
}

impl From<Search> for LadderSearch {
	fn from(search: Search) -> Self {
		match search {
			Search::Tiers => Self::Tiers,
			Search::Bisect => Self::Bisect,
		}
	}
}

#[derive(Debug, Subcommand)]
enum Action {
	/// Prepare the battle runtime from your installed client. Rerun after a client update.
	Setup {
		/// Install the owned Player from a local release archive instead of downloading it.
		#[arg(long, conflicts_with = "build_player")]
		player_zip: Option<PathBuf>,
		/// Build the owned Player locally with Unity 2021.3.39f1 (maintainers).
		#[arg(long)]
		build_player: bool,
	},
	/// Execute an MAA job or a battle program on the original battle code.
	Run {
		job: PathBuf,
		/// Explicit squad file; otherwise one is generated at the training options below.
		#[arg(long)]
		squad: Option<PathBuf>,
		#[command(flatten)]
		training: TrainingArgs,
		#[command(flatten)]
		battle: BattleArgs,
		/// Run twice and compare pinned inputs and normalized observations.
		#[arg(long)]
		repeat: bool,
	},
	/// Write the squad a job would run with at the given training, to inspect or edit.
	Squad {
		job: PathBuf,
		output: PathBuf,
		#[command(flatten)]
		training: TrainingArgs,
	},
	/// Find the lowest training at which the job still wins with no leak.
	MinTraining {
		job: PathBuf,
		/// How to choose the next training: fixed tiers, or bisection (fewer runs and exact
		/// levels and trust, assuming higher training never loses where lower training wins).
		#[arg(long, value_enum, default_value_t = Search::Tiers)]
		search: Search,
		/// After the team ladder, lower each operator alone for per-operator minimums.
		#[arg(long)]
		per_operator: bool,
		/// Pass a tier that wins although enemies reached an exit (default: no leaks).
		#[arg(long)]
		allow_leaks: bool,
		#[command(flatten)]
		battle: BattleArgs,
	},
	/// Convert an MAA job to a battle program (ark-battle-program-v1), or a battle program to an
	/// MAA job, naming every reason MAA cannot express it.
	Convert { input: PathBuf, output: PathBuf },
	/// Find the largest input latency at which the job still wins with no leak.
	Latency {
		job: PathBuf,
		#[arg(long)]
		squad: Option<PathBuf>,
		#[command(flatten)]
		training: TrainingArgs,
		#[command(flatten)]
		battle: BattleArgs,
		/// Largest latency searched, in ticks.
		#[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=900))]
		max_delay: u32,
	},
	/// Try later timings for each skill step and write the best program that wins with no leak.
	TuneSkills {
		job: PathBuf,
		#[arg(long)]
		squad: Option<PathBuf>,
		#[command(flatten)]
		training: TrainingArgs,
		#[command(flatten)]
		battle: BattleArgs,
		/// Waits to try per skill step, in ticks.
		#[arg(long, value_delimiter = ',', default_value = "0,15,30,60,90",
			value_parser = clap::value_parser!(u32).range(0..=900))]
		waits: Vec<u32>,
	},
	/// Compare pinned inputs and normalized original-engine observations.
	Compare { first: PathBuf, second: PathBuf },
	/// Maintainers: package the verified owned Player as its release archive.
	#[command(hide = true)]
	PackagePlayer { output: PathBuf },
	/// Maintainers: compile the probe and write it with its source digest for a release bundle.
	#[command(hide = true)]
	BuildProbe { output: PathBuf },
}

pub(crate) fn run(args: BattleCommand) -> Result<ExitCode> {
	let report: Value = execute(args)?;
	println!("{}", serde_json::to_string_pretty(&report)?);
	Ok(
		if report.get("passed").is_some_and(|value| value == false) {
			ExitCode::FAILURE
		} else {
			ExitCode::SUCCESS
		},
	)
}

fn execute(args: BattleCommand) -> Result<Value> {
	if let Action::Compare { first, second } = &args.command {
		return comparison::compare(first, second);
	}
	match &args.command {
		Action::BuildProbe { output } => {
			return Host::new(HostConfig::state_only(args.state)?).build_probe(output);
		}
		Action::Convert { input, output } => {
			return Host::new(HostConfig::state_only(args.state)?).convert(input, output);
		}
		_ => {}
	}
	let host: Host = Host::new(HostConfig::resolve(args.state, args.client)?);
	match args.command {
		Action::Setup {
			player_zip,
			build_player,
		} => host.setup(&match (player_zip, build_player) {
			(Some(zip), _) => PlayerSource::Archive(zip),
			(None, true) => PlayerSource::Build,
			(None, false) => PlayerSource::Installed,
		}),
		Action::Run {
			job,
			squad,
			training,
			battle,
			repeat,
		} => host.run(
			&job,
			squad.as_deref(),
			&training.training(),
			&battle.options(repeat),
		),
		Action::Squad {
			job,
			output,
			training,
		} => host.squad_for(&job, &output, &training.training()),
		Action::MinTraining {
			job,
			search,
			per_operator,
			allow_leaks,
			battle,
		} => host.ladder(
			&job,
			&battle.options(false),
			search.into(),
			per_operator,
			allow_leaks,
		),
		Action::Latency {
			job,
			squad,
			training,
			battle,
			max_delay,
		} => host.margin(
			&job,
			squad.as_deref(),
			&training.training(),
			&battle.options(false),
			max_delay,
		),
		Action::TuneSkills {
			job,
			squad,
			training,
			battle,
			waits,
		} => host.advise(
			&job,
			squad.as_deref(),
			&training.training(),
			&battle.options(false),
			&waits,
		),
		Action::PackagePlayer { output } => host.package_player(&output),
		Action::Compare { .. } | Action::BuildProbe { .. } | Action::Convert { .. } => {
			unreachable!()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Cli, Command};
	use clap::Parser;

	fn parse(arguments: &[&str]) -> BattleCommand {
		let cli: Cli = Cli::try_parse_from(arguments).unwrap();
		let Command::Battle(args) = cli.command else {
			panic!("expected battle command");
		};
		args
	}

	#[test]
	fn run_defaults_to_maximum_training_and_global_paths_parse() {
		let args = parse(&[
			"ark-cli", "battle", "run", "job.json", "--state", "state", "--client", "client",
		]);
		assert_eq!(args.state, Some(PathBuf::from("state")));
		assert_eq!(args.client, Some(PathBuf::from("client")));
		let Action::Run {
			squad,
			training,
			battle,
			repeat,
			..
		} = args.command
		else {
			panic!("expected run command");
		};
		assert_eq!(squad, None);
		assert_eq!(training.training(), Training::default());
		assert_eq!(
			(battle.seed, battle.max_ticks, battle.resource_rounds),
			(1, 36_000, 3)
		);
		assert!(!battle.no_lua_hotfix && !repeat);
	}

	#[test]
	fn training_options_and_player_sources_parse() {
		let args = parse(&[
			"ark-cli",
			"battle",
			"run",
			"job.json",
			"--level",
			"60",
			"--mastery",
			"1",
			"--module",
			"0",
			"--trust",
			"50",
		]);
		let Action::Run { training, .. } = args.command else {
			panic!("expected run command");
		};
		assert_eq!(
			training.training(),
			Training {
				level: Some(60),
				mastery: 1,
				module: 0,
				trust: Some(50)
			}
		);
		let setup = parse(&["ark-cli", "battle", "setup", "--player-zip", "p.zip"]);
		assert!(matches!(
			setup.command,
			Action::Setup {
				player_zip: Some(_),
				..
			}
		));
		let advise = parse(&[
			"ark-cli",
			"battle",
			"tune-skills",
			"job.json",
			"--waits",
			"0,30",
			"--delay",
			"6",
		]);
		let Action::TuneSkills { waits, battle, .. } = advise.command else {
			panic!("expected advise command");
		};
		assert_eq!((waits, battle.delay), (vec![0, 30], 6));
		assert!(Cli::try_parse_from([
			"ark-cli",
			"battle",
			"tune-skills",
			"job.json",
			"--waits",
			"901"
		])
		.is_err());
		let ladder = parse(&[
			"ark-cli",
			"battle",
			"min-training",
			"job.json",
			"--search",
			"bisect",
		]);
		assert!(matches!(
			ladder.command,
			Action::MinTraining {
				search: Search::Bisect,
				..
			}
		));
		assert!(Cli::try_parse_from([
			"ark-cli",
			"battle",
			"setup",
			"--player-zip",
			"p.zip",
			"--build-player"
		])
		.is_err());
	}
}
