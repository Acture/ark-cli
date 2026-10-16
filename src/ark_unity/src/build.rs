//! Build and verify the clean, user-owned IL2CPP Player with the official Unity toolchain.
use crate::{io, UNITY_VERSION};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
	fs,
	path::{Path, PathBuf},
	process::{Command, Stdio},
	time::{Duration, Instant},
};

const ARTIFACTS: &[&str] = &[
	"ArkBattleRuntime.exe",
	"UnityPlayer.dll",
	"GameAssembly.dll",
	"ArkBattleRuntime_Data/globalgamemanagers",
	"ArkBattleRuntime_Data/boot.config",
	"ArkBattleRuntime_Data/il2cpp_data/Metadata/global-metadata.dat",
	"ArkBattleRuntime_Data/Plugins/x86_64/ArkBattleProbe.dll",
];

pub fn verified_player(record: &Path) -> Result<PathBuf> {
	let value: Value = io::read_json(record)?;
	ensure!(
		value["result"] == "BuildSucceeded",
		"Player build did not succeed"
	);
	let player: PathBuf = PathBuf::from(value["player"].as_str().context("Player path missing")?);
	let artifacts = value["artifacts"]
		.as_array()
		.context("Player artifact inventory missing")?;
	for required in ARTIFACTS
		.iter()
		.filter(|x| !x.ends_with("ArkBattleProbe.dll"))
	{
		let entry = artifacts
			.iter()
			.find(|a| a["path"] == *required)
			.with_context(|| format!("Required Player artifact absent: {required}"))?;
		ensure!(
			entry["sha256"] == io::sha256_file(&player.join(required))?,
			"Player artifact changed: {required}"
		);
	}
	ensure!(
		value["full_inventory"] == snapshot_player(&player)?,
		"Owned Player tree differs from its complete build inventory"
	);
	Ok(player)
}

pub fn snapshot_player(player: &Path) -> Result<Value> {
	let mut files: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
	for entry in walkdir::WalkDir::new(player) {
		let entry = entry?;
		ensure!(
			!entry.file_type().is_symlink(),
			"Owned Player contains a symbolic link"
		);
		if !entry.file_type().is_file() {
			continue;
		}
		let relative = entry
			.path()
			.strip_prefix(player)?
			.to_string_lossy()
			.replace('\\', "/");
		if relative.starts_with("ArkBattleRuntime_BackUpThisFolder") {
			continue;
		}
		files.insert(
			relative.clone(),
			json!({"path":relative,"sha256":io::sha256_file(entry.path())?,"size":entry.metadata()?.len()}),
		);
	}
	Ok(json!(files.into_values().collect::<Vec<_>>()))
}

fn hidden(command: &mut Command) {
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		command.creation_flags(0x08000000);
	}
	command.stdin(Stdio::null());
}

fn checked_json(mut command: Command) -> Result<Value> {
	hidden(&mut command);
	let output = command.output()?;
	ensure!(
		output.status.success(),
		"Tool failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	let value: Value = serde_json::from_slice(&output.stdout)?;
	Ok(value)
}

fn cli_json(cli: &Path, args: &[&str]) -> Result<Value> {
	let mut command = Command::new(cli);
	command.args(args).arg("--json");
	let value = checked_json(command)?;
	ensure!(value["success"] == true, "Unity CLI failed: {value}");
	Ok(value)
}

pub fn build(state: &Path, template: &Path, probe: &Path) -> Result<PathBuf> {
	ensure!(cfg!(windows), "Owned IL2CPP Player build requires Windows");
	let configured = std::env::var_os("ARK_UNITY_CLI").map(PathBuf::from);
	let home =
		std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".unity/bin/unity.exe"));
	let mut candidates: Vec<PathBuf> = configured.into_iter().chain(home).collect();
	if let Some(path) = std::env::var_os("PATH") {
		candidates.extend(std::env::split_paths(&path).map(|p| p.join("unity.exe")));
	}
	let cli = candidates
		.into_iter()
		.find(|p| p.is_file())
		.context("Unity CLI not found; set ARK_UNITY_CLI")?;
	let editor = cli_json(&cli, &["editors", "verify", UNITY_VERSION])?;
	ensure!(
		editor["data"]["ok"] == true
			&& editor["data"]["components"].as_array().is_some_and(|c| c
				.iter()
				.any(|m| m["component"] == "windows-il2cpp" && m["status"] == "ok")),
		"Unity {UNITY_VERSION} Windows IL2CPP component unavailable"
	);
	let licensing = licensing_client()?;
	ensure!(
		cli_json(&cli, &["license", "status"])?["data"]["active"] == true,
		"Activate an official Unity Editor license before building"
	);
	let vswhere: PathBuf =
		PathBuf::from("C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe");
	let mut command = Command::new(vswhere);
	// Without -utf8, localized product names arrive in the system code page.
	command.args([
		"-products",
		"*",
		"-requires",
		"Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
		"-format",
		"json",
		"-utf8",
	]);
	ensure!(
		checked_json(command)?
			.as_array()
			.is_some_and(|v| v.iter().any(|t| t["isComplete"] == true)),
		"MSVC x64 build tools missing"
	);
	let project: PathBuf = io::owned(state, &state.join("unity-project"))?;
	let long_source: PathBuf = project.join("Library/Bee/artifacts/WinPlayerBuildProgram/il2cppOutput/cpp/UnityEngine.UIElementsNativeModule_CodeGen.c");
	ensure!(
		long_source.as_os_str().len() < 260,
		"MSVC project path too long; set a shorter ARK_BATTLE_RUNTIME_STATE"
	);
	fs::create_dir_all(&project)?;
	for entry in walkdir::WalkDir::new(template) {
		let entry = entry?;
		if !entry.file_type().is_file() {
			continue;
		}
		let target = project.join(entry.path().strip_prefix(template)?);
		fs::create_dir_all(target.parent().unwrap())?;
		fs::copy(entry.path(), target)?;
	}
	let plugin = project.join("Assets/Plugins/x86_64/ArkBattleProbe.dll");
	fs::create_dir_all(plugin.parent().unwrap())?;
	fs::copy(probe, plugin)?;
	let output_root = project.join("_PlayerOutput");
	let previous: Vec<PathBuf> = if output_root.is_dir() {
		fs::read_dir(&output_root)?
			.map(|e| e.map(|e| e.path()))
			.collect::<std::io::Result<_>>()?
	} else {
		vec![]
	};
	let record_root = io::stage_dir(state, "player")?;
	let log = record_root.join("editor.log");
	let stream = fs::File::create(record_root.join("cli-result.json"))?;
	let mut command = Command::new(&cli);
	command
		.arg("run")
		.arg(&project)
		.args([
			"--editor-version",
			UNITY_VERSION,
			"--timeout",
			"900",
			"--log-file",
		])
		.arg(&log)
		.args([
			"--no-tail",
			"--json",
			"--",
			"-executeMethod",
			"ArkBattleRuntimeBuilder.Build",
		])
		.stdout(stream)
		.stderr(Stdio::inherit());
	hidden(&mut command);
	let started = Instant::now();
	let mut process = command.spawn()?;
	let status = loop {
		if let Some(status) = process.try_wait()? {
			break status;
		}
		if started.elapsed() > Duration::from_secs(930) {
			process.kill()?;
			process.wait()?;
			anyhow::bail!("Unity build timeout; read {}", log.display());
		}
		std::thread::sleep(Duration::from_millis(500));
	};
	let mut record = json!({"schema":"ark-battle-player-build-v3","project":project,"cli":cli,"cli_exit_code":status.code(),"licensing":licensing,"probe_sha256":io::sha256_file(probe)?,"elapsed_seconds":started.elapsed().as_secs_f64(),"result":"BuildFailed"});
	let outcome = (|| -> Result<()> {
		ensure!(
			status.success(),
			"Unity build failed; read {}",
			log.display()
		);
		let outputs: Vec<PathBuf> = fs::read_dir(&output_root)?
			.map(|e| e.map(|e| e.path()))
			.collect::<std::io::Result<Vec<_>>>()?
			.into_iter()
			.filter(|p| !previous.contains(p) && p.join("player-build-result.json").is_file())
			.collect();
		ensure!(outputs.len() == 1, "Expected one new Player build report");
		let player = &outputs[0];
		let report: Value = io::read_json(&player.join("player-build-result.json"))?;
		ensure!(
			report["result"] == "Succeeded"
				&& report["unityVersion"] == UNITY_VERSION
				&& report["pluginPreloaded"] == true,
			"Build version/preload check failed"
		);
		let mut artifacts: Vec<Value> = Vec::new();
		for path in ARTIFACTS {
			let file = player.join(path);
			let size = fs::metadata(&file)?.len();
			ensure!(size > 0, "Empty Player artifact");
			artifacts.push(json!({"path":path,"sha256":io::sha256_file(&file)?,"size":size}));
		}
		record["result"] = json!("BuildSucceeded");
		record["player"] = json!(player);
		record["report"] = report;
		record["artifacts"] = json!(artifacts);
		record["full_inventory"] = snapshot_player(player)?;
		Ok(())
	})();
	let path = record_root.join("build.json");
	io::write_json(&path, &record)?;
	outcome?;
	Ok(path)
}

fn licensing_client() -> Result<Value> {
	let path: PathBuf = PathBuf::from(std::env::var_os("APPDATA").context("APPDATA missing")?)
		.join("UnityHub/external-modules/licensingClient/Unity.Licensing.Client.exe");
	let mut command = Command::new("powershell");
	// Pass the path in an environment variable: it never becomes PowerShell source text.
	command.args(["-NoProfile","-Command","$s = Get-AuthenticodeSignature -LiteralPath $env:ARK_BUILD_LICENSE_CLIENT; if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'Unity Technologies') { exit 1 }; ConvertTo-Json -Compress -InputObject @(Get-Process -Name 'Unity.Licensing.Client' -ErrorAction SilentlyContinue | ForEach-Object { $_.Path })"])
		.env("ARK_BUILD_LICENSE_CLIENT",&path)
		// A PowerShell 7 parent's module path makes Windows PowerShell load incompatible modules.
		.env_remove("PSModulePath");
	let paths = checked_json(command)?;
	let paths = paths
		.as_array()
		.context("Licensing process query returned no list")?;
	if paths.is_empty() {
		let mut command = Command::new(&path);
		command
			.args([
				"--namedPipe",
				&format!("Unity-LicenseClient-{}", std::env::var("USERNAME")?),
				"--disable-auto-shutdown",
				"--cloudEnvironment",
				"production",
			])
			.stdout(Stdio::null())
			.stderr(Stdio::null());
		hidden(&mut command);
		command.spawn()?;
		return Ok(json!({"started_by_build":true}));
	}
	ensure!(
		paths.len() == 1 && paths[0].as_str().is_some_and(|p| Path::new(p) == path),
		"A different licensing client is running"
	);
	Ok(json!({"started_by_build":false}))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn complete_inventory_rejects_added_and_changed_runtime_files() -> Result<()> {
		let root: PathBuf = std::env::temp_dir().join(format!(
			"ark-player-inventory-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)?
				.as_nanos()
		));
		let player: PathBuf = root.join("player");
		let mut artifacts: Vec<Value> = Vec::new();
		for relative in ARTIFACTS {
			let path: PathBuf = player.join(relative);
			fs::create_dir_all(path.parent().unwrap())?;
			fs::write(&path, b"owned fixture")?;
			artifacts.push(json!({"path":relative,"sha256":io::sha256_file(&path)?}));
		}
		let data: PathBuf = player.join("ArkBattleRuntime_Data/resources.assets");
		fs::write(&data, b"original resources")?;
		let record: PathBuf = root.join("build.json");
		io::write_json(
			&record,
			&json!({"result":"BuildSucceeded","player":player,"artifacts":artifacts,"full_inventory":snapshot_player(&player)?}),
		)?;
		assert_eq!(verified_player(&record)?, player);
		let plugin: PathBuf = player.join("ArkBattleRuntime_Data/Plugins/unrecorded.dll");
		fs::write(&plugin, b"unexpected plugin")?;
		assert!(verified_player(&record).is_err());
		fs::remove_file(plugin)?;
		fs::write(data, b"modified resources")?;
		assert!(verified_player(&record).is_err());
		fs::remove_dir_all(io::owned(&std::env::temp_dir(), &root)?)?;
		Ok(())
	}
}
