use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

const BINARY: &str = env!("CARGO_BIN_EXE_ark-cli");

struct TestDirectory(PathBuf);

impl TestDirectory {
	fn new() -> Self {
		static NEXT: AtomicU64 = AtomicU64::new(0);
		let path: PathBuf = std::env::temp_dir().join(format!(
			"ark-cli-test-{}-{}",
			std::process::id(),
			NEXT.fetch_add(1, Ordering::Relaxed)
		));
		fs::create_dir(&path).unwrap();
		Self(path)
	}

	fn path(&self) -> &Path {
		&self.0
	}
}

impl Drop for TestDirectory {
	fn drop(&mut self) {
		let _result: std::io::Result<()> = fs::remove_dir_all(&self.0);
	}
}

fn invoke(arguments: &[&str]) -> Output {
	Command::new(BINARY).args(arguments).output().unwrap()
}

fn stdout(output: &Output) -> String {
	String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
	String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn root_help_lists_all_workflows() {
	let output: Output = invoke(&["--help"]);
	assert!(output.status.success(), "{}", stderr(&output));
	let help: String = stdout(&output);
	for command in ["level", "replay", "ingest", "battle"] {
		assert!(help.contains(command), "missing {command}: {help}");
	}
	assert!(!help.contains("coverage"));
}

#[test]
fn every_nested_help_exits_successfully() {
	let workflows: &[&[&str]] = &[
		&["level"],
		&["level", "solve"],
		&["level", "validate"],
		&["replay"],
		&["ingest"],
		&["ingest", "identify-spine"],
		&["ingest", "fetch"],
		&["ingest", "fetch-gamedata"],
		&["ingest", "extract"],
		&["ingest", "extract-battle-data"],
		&["battle"],
		&["battle", "setup"],
		&["battle", "run"],
		&["battle", "min-training"],
		&["battle", "convert"],
		&["battle", "latency"],
		&["battle", "tune-skills"],
		&["battle", "squad"],
		&["battle", "compare"],
	];
	for workflow in workflows {
		let mut arguments: Vec<&str> = workflow.to_vec();
		arguments.push("--help");
		let output: Output = invoke(&arguments);
		assert!(output.status.success(), "{workflow:?}: {}", stderr(&output));
		let executable_name: &str = Path::new(BINARY).file_name().unwrap().to_str().unwrap();
		let usage: String = format!("Usage: {executable_name} {}", workflow.join(" "));
		assert!(stdout(&output).contains(&usage), "{}", stdout(&output));
	}
}

#[test]
fn version_identifies_the_independent_package() {
	let output: Output = invoke(&["--version"]);
	assert!(output.status.success(), "{}", stderr(&output));
	assert_eq!(
		stdout(&output).trim(),
		concat!("ark-cli ", env!("CARGO_PKG_VERSION"))
	);
}

#[test]
fn battle_help_does_not_create_state() {
	let directory: TestDirectory = TestDirectory::new();
	let state: PathBuf = directory.path().join("state");
	let output: Output = Command::new(BINARY)
		.args(["battle", "--state"])
		.arg(&state)
		.args(["setup", "--help"])
		.output()
		.unwrap();
	assert!(output.status.success(), "{}", stderr(&output));
	assert!(!state.exists());
}

#[test]
fn invalid_battle_seed_and_budgets_are_rejected_before_state_creation() {
	let directory: TestDirectory = TestDirectory::new();
	let invalid: &[(&str, &str)] = &[
		("--seed", "2147483648"),
		("--seed", "invalid"),
		("--max-ticks", "0"),
		("--max-ticks", "54001"),
		("--max-ticks", "invalid"),
		("--resource-rounds", "0"),
		("--resource-rounds", "9"),
		("--resource-rounds", "invalid"),
	];
	for (index, (option, value)) in invalid.iter().enumerate() {
		let state: PathBuf = directory.path().join(format!("state-{index}"));
		let output: Output = Command::new(BINARY)
			.args(["battle", "--state"])
			.arg(&state)
			.args(["run", "missing-job.json", option, value])
			.output()
			.unwrap();
		assert_eq!(
			output.status.code(),
			Some(2),
			"{option}={value}: {}",
			stderr(&output)
		);
		assert!(stderr(&output).contains(option));
		assert!(!state.exists(), "{option}={value} created state");
	}
}

fn original_run(seed: u32) -> Value {
	json!({
		"passed": true,
		"maa_job_complete": true,
		"battle_outcome": { "terminal_verified": true, "status": "original_win" },
		"battle_inputs": {
			"actions_sha256": "actions",
			"squad_sha256": "squad",
			"seed": seed,
			"max_ticks": 12000
		},
		"effective_inputs": [],
		"execution_inputs": [],
		"events": []
	})
}

#[test]
fn battle_compare_preserves_report_exit_codes_without_resolving_host() {
	let directory: TestDirectory = TestDirectory::new();
	let first: PathBuf = directory.path().join("first.json");
	let second: PathBuf = directory.path().join("second.json");
	let state: PathBuf = directory.path().join("unused-state");
	fs::write(&first, serde_json::to_vec(&original_run(1)).unwrap()).unwrap();
	for (seed, expected_code, expected_passed) in [(1, 0, true), (2, 1, false)] {
		fs::write(&second, serde_json::to_vec(&original_run(seed)).unwrap()).unwrap();
		let output: Output = Command::new(BINARY)
			.args(["battle", "--state"])
			.arg(&state)
			.args(["--client", "missing-client", "compare"])
			.arg(&first)
			.arg(&second)
			.output()
			.unwrap();
		assert_eq!(
			output.status.code(),
			Some(expected_code),
			"{}",
			stderr(&output)
		);
		let report: Value = serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(report["passed"], expected_passed);
		assert_eq!(report["battle_inputs_equal"], expected_passed);
		assert!(directory.path().join("repeatability.json").is_file());
		assert!(!state.exists());
	}
}

#[test]
fn battle_runtime_errors_exit_two() {
	let directory: TestDirectory = TestDirectory::new();
	let output: Output = Command::new(BINARY)
		.current_dir(directory.path())
		.args([
			"battle",
			"compare",
			"missing-first.json",
			"missing-second.json",
		])
		.output()
		.unwrap();
	assert_eq!(output.status.code(), Some(2));
	assert!(stderr(&output).starts_with("error:"));
}

#[test]
fn copied_binary_runs_ingestion_and_discovers_adjacent_configuration() {
	let directory: TestDirectory = TestDirectory::new();
	let executable_directory: PathBuf = directory.path().join("bin");
	let working_directory: PathBuf = directory.path().join("work");
	fs::create_dir(&executable_directory).unwrap();
	fs::create_dir(&working_directory).unwrap();
	let executable: PathBuf = executable_directory.join(Path::new(BINARY).file_name().unwrap());
	fs::copy(BINARY, &executable).unwrap();
	let skeleton: PathBuf = working_directory.join("skeleton.json");
	fs::write(&skeleton, r#"{"skeleton":{"spine":"4.2.33"}}"#).unwrap();
	let output: Output = Command::new(&executable)
		.current_dir(&working_directory)
		.env_remove("ROGUE_CONFIG")
		.args(["ingest", "identify-spine", "skeleton.json", "--limit", "1"])
		.output()
		.unwrap();
	assert!(output.status.success(), "{}", stderr(&output));
	assert!(stdout(&output).contains("4.2.33"));

	let configuration: PathBuf = executable_directory.join("rogue.toml");
	fs::write(&configuration, "assets_dir = [\n").unwrap();
	fs::write(working_directory.join("bundle.ab"), []).unwrap();
	let output: Output = Command::new(&executable)
		.current_dir(&working_directory)
		.env_remove("ROGUE_CONFIG")
		.args(["ingest", "extract", "bundle.ab"])
		.output()
		.unwrap();
	assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
	let error: String = stderr(&output);
	assert!(error.contains("parse config at"), "{error}");
	assert!(
		error.contains(&configuration.display().to_string()),
		"{error}"
	);
}
