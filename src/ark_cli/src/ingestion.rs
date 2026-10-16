use std::{path::PathBuf, process::ExitCode};

use anyhow::Result;
use ark_solver::ingest::{self, ExtractArgs, ExtractBattleDataArgs, FetchArgs, FetchGameDataArgs};
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub(crate) struct IngestionArgs {
	#[command(subcommand)]
	command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
	/// Scan TARGET for Spine skeletons and print the runtime version of each.
	///
	/// Accepts a single file, a directory, or a zip-like container (`.apk` / `.zip`).
	IdentifySpine {
		/// Path to scan.
		target: PathBuf,
		/// Stop after this many hits (0 = unlimited).
		#[arg(long, default_value_t = 50)]
		limit: usize,
	},
	/// Download matching hot-update bundles from Hypergryph's public CDN.
	Fetch(FetchArgs),
	/// Download the thin JSON gamedata set, optionally including raw level JSON.
	FetchGamedata(FetchGameDataArgs),
	/// Extract operator assets from an AssetBundle via ArkUnpacker.
	Extract(ExtractArgs),
	/// Build a verified rich TypeTree snapshot from a user-owned Android APK.
	ExtractBattleData(ExtractBattleDataArgs),
}

pub(crate) fn run(args: IngestionArgs) -> Result<ExitCode> {
	match args.command {
		Command::IdentifySpine { target, limit } => ingest::identify_spine(&target, limit),
		Command::Fetch(args) => ingest::fetch(args),
		Command::FetchGamedata(args) => ingest::fetch_gamedata(args),
		Command::Extract(args) => ingest::extract(args),
		Command::ExtractBattleData(args) => ingest::extract_battle_data(args),
	}?;
	Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::Cli;
	use clap::Parser;

	#[test]
	fn fetch_gamedata_parses_repeatable_stage_filters() {
		let cli = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"fetch-gamedata",
			"--profile",
			"analysis",
			"--stage",
			"BD-7",
			"--stage",
			"1-7",
		])
		.expect("repeatable stage filters should parse");

		let crate::Command::Ingest(IngestionArgs {
			command: Command::FetchGamedata(args),
		}) = cli.command
		else {
			panic!("expected fetch-gamedata command");
		};
		let expected_stages: Vec<String> = vec!["BD-7".to_owned(), "1-7".to_owned()];
		assert_eq!(args.profile, ingest::GameDataProfile::Analysis);
		assert_eq!(args.stages, expected_stages);
	}

	#[test]
	fn fetch_gamedata_rejects_stage_with_all_levels() {
		let result = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"fetch-gamedata",
			"--profile",
			"analysis",
			"--stage",
			"BD-7",
			"--include-levels",
		]);
		let Err(error) = result else {
			panic!("--stage and --include-levels should conflict");
		};

		assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
	}

	#[test]
	fn generated_output_overrides_are_optional() {
		let fetch_cli: Cli = Cli::try_parse_from(["ark-cli", "ingest", "fetch", "--all"])
			.expect("fetch should use configured output when --to is omitted");
		let crate::Command::Ingest(IngestionArgs {
			command: Command::Fetch(fetch_args),
		}) = fetch_cli.command
		else {
			panic!("expected fetch command");
		};
		assert_eq!(fetch_args.to, None);

		let gamedata_cli: Cli = Cli::try_parse_from(["ark-cli", "ingest", "fetch-gamedata"])
			.expect("fetch-gamedata should use configured output when --to is omitted");
		let crate::Command::Ingest(IngestionArgs {
			command: Command::FetchGamedata(gamedata_args),
		}) = gamedata_cli.command
		else {
			panic!("expected fetch-gamedata command");
		};
		assert_eq!(gamedata_args.to, None);

		let extract_cli: Cli = Cli::try_parse_from(["ark-cli", "ingest", "extract", "bundle.ab"])
			.expect("extract should use configured output when --output is omitted");
		let crate::Command::Ingest(IngestionArgs {
			command: Command::Extract(extract_args),
		}) = extract_cli.command
		else {
			panic!("expected extract command");
		};
		assert_eq!(extract_args.output, None);

		let battle_data_cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"extract-battle-data",
			"client/official.apk",
			"--gamedata-snapshot",
			"client/_gamedata",
		])
		.expect("extract-battle-data should use configured output when --output is omitted");
		let crate::Command::Ingest(IngestionArgs {
			command: Command::ExtractBattleData(battle_data_args),
		}) = battle_data_cli.command
		else {
			panic!("expected extract-battle-data command");
		};
		assert_eq!(battle_data_args.output, None);
	}

	#[test]
	fn explicit_generated_output_overrides_are_preserved() {
		let fetch_cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"fetch",
			"--all",
			"--to",
			"custom/hot-update",
		])
		.unwrap();
		let crate::Command::Ingest(IngestionArgs {
			command: Command::Fetch(fetch_args),
		}) = fetch_cli.command
		else {
			panic!("expected fetch command");
		};
		assert_eq!(fetch_args.to, Some(PathBuf::from("custom/hot-update")));

		let gamedata_cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"fetch-gamedata",
			"--to",
			"custom/gamedata",
		])
		.unwrap();
		let crate::Command::Ingest(IngestionArgs {
			command: Command::FetchGamedata(gamedata_args),
		}) = gamedata_cli.command
		else {
			panic!("expected fetch-gamedata command");
		};
		assert_eq!(gamedata_args.to, Some(PathBuf::from("custom/gamedata")));

		let extract_cli: Cli = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"extract",
			"bundle.ab",
			"--output",
			"custom/assets",
		])
		.unwrap();
		let crate::Command::Ingest(IngestionArgs {
			command: Command::Extract(extract_args),
		}) = extract_cli.command
		else {
			panic!("expected extract command");
		};
		assert_eq!(extract_args.output, Some(PathBuf::from("custom/assets")));
	}

	#[test]
	fn extract_battle_data_parses_repeatable_operators() {
		let cli = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"extract-battle-data",
			"client/official.apk",
			"--gamedata-snapshot",
			"assets/analysis/gamedata",
			"--output",
			"assets/analysis/prefabs",
			"--operator",
			"sbell2",
			"--operator",
			"char_2025_shu",
		])
		.expect("repeatable operator keys should parse");

		let crate::Command::Ingest(IngestionArgs {
			command: Command::ExtractBattleData(args),
		}) = cli.command
		else {
			panic!("expected extract-battle-data command");
		};
		assert_eq!(args.operators, ["sbell2", "char_2025_shu"]);
	}

	#[test]
	fn extract_battle_data_rejects_removed_all_charpacks_option() {
		let result = Cli::try_parse_from([
			"ark-cli",
			"ingest",
			"extract-battle-data",
			"client/official.apk",
			"--gamedata-snapshot",
			"assets/analysis/gamedata",
			"--output",
			"assets/analysis/prefabs",
			"--all-charpacks",
		]);
		let Err(error) = result else {
			panic!("removed --all-charpacks option should be rejected");
		};

		assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
	}
}
