use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

use ark_solver::maa_job::{parse_maa_job, MaaJobSeed};

fn main() -> ExitCode {
	match run() {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("error: {error}");
			ExitCode::FAILURE
		}
	}
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
	let source = env::args()
		.nth(1)
		.ok_or("usage: inspect_maa_job <guide.json|->")?;
	let json = if source == "-" {
		let mut json = String::new();
		io::stdin().read_to_string(&mut json)?;
		json
	} else {
		fs::read_to_string(source)?
	};
	let guide = parse_maa_job(&json)?;
	let mut seeds: Vec<MaaJobSeed> = Vec::new();
	for difficulty in guide.difficulty.selected() {
		seeds.extend(
			guide
				.seed_selections(difficulty, usize::MAX)?
				.map(|selection| selection.materialize()),
		);
	}
	println!(
		"stage={} title={:?} actions={} groups={} fixed_operators={} seeds={}",
		guide.stage_name(),
		guide.title(),
		guide.actions().len(),
		guide.groups().len(),
		guide.fixed_operators().len(),
		seeds.len()
	);
	for seed in seeds {
		let operators = seed
			.operators()
			.iter()
			.map(|operator| operator.name())
			.collect::<Vec<_>>()
			.join(",");
		println!("difficulty={} operators={operators}", seed.difficulty());
	}
	Ok(())
}
