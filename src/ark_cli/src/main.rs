#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod battle;
mod ingestion;
mod solver;

#[derive(Debug, Parser)]
#[command(
	name = "ark-cli",
	version,
	about = "Prepare Arknights data, verify strategies, and execute original-engine battles."
)]
struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
	#[command(flatten)]
	Solver(solver::Command),
	/// Prepare assets and revision-pinned data from user-supplied client files.
	Ingest(ingestion::IngestionArgs),
	/// Run MAA jobs and battle programs on the original battle code from your own client.
	Battle(battle::BattleCommand),
}

fn main() -> ExitCode {
	match Cli::parse().command {
		Command::Solver(command) => match solver::run(command) {
			Ok(()) => ExitCode::SUCCESS,
			Err(error) => {
				eprintln!("error: {error}");
				ExitCode::FAILURE
			}
		},
		Command::Ingest(args) => finish(ingestion::run(args), 1),
		Command::Battle(args) => finish(battle::run(args), 2),
	}
}

fn finish(result: anyhow::Result<ExitCode>, error_code: u8) -> ExitCode {
	match result {
		Ok(code) => code,
		Err(error) => {
			eprintln!("error: {error:#}");
			ExitCode::from(error_code)
		}
	}
}
