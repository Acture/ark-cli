use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::environment::{read_json, sha256_file, write_json, RuntimeConfig};

/// Digest of the probe sources compiled into this executable, independent of the toolchain.
pub(crate) fn source_digest() -> String {
	let mut digest: Sha256 = Sha256::new();
	for (name, bytes) in crate::bundle::PROBE_FILES {
		digest.update(name.as_bytes());
		digest.update(b"\0");
		digest.update(bytes);
		digest.update(b"\0");
	}
	format!("{:x}", digest.finalize())
}

/// A probe DLL a release embedded, used only when built from exactly these probe sources.
fn embedded(config: &RuntimeConfig) -> Result<Option<PathBuf>> {
	let (Some(dll), Some(recorded)) = (
		crate::tools::bundled("ArkBattleProbe.dll"),
		crate::tools::bundled("probe-source-digest.txt"),
	) else {
		return Ok(None);
	};
	let digest: String = source_digest();
	if std::str::from_utf8(recorded)?.trim() != digest {
		return Ok(None);
	}
	let output: PathBuf = config
		.state
		.join("probe")
		.join(format!("embedded-{}", &digest[..16]));
	let path: PathBuf = output.join("ArkBattleProbe.dll");
	if !path.is_file() || fs::read(&path)? != dll {
		fs::create_dir_all(&output)?;
		fs::write(&path, dll)?;
		write_json(
			&output.join("build.json"),
			&json!({"schema": "ark-battle-probe-build-v3", "source_digest": digest, "builder": "embedded",
				"exit_code": 0, "dll_sha256": sha256_file(&path)?}),
		)?;
	}
	Ok(Some(path))
}

pub(crate) fn build(config: &RuntimeConfig) -> Result<PathBuf> {
	ensure!(
		cfg!(all(target_os = "windows", target_arch = "x86_64")),
		"The original-engine probe requires Windows x64"
	);
	if let Some(dll) = embedded(config)? {
		return Ok(dll);
	}
	compile(config)
}

/// Compile the probe from the embedded sources with the local Rust toolchain and Windows SDK.
pub(crate) fn compile(config: &RuntimeConfig) -> Result<PathBuf> {
	let root: PathBuf =
		crate::bundle::materialize(&config.state, "probe", crate::bundle::PROBE_FILES)?;
	let manifest: PathBuf = root.join("Cargo.toml");
	let version = Command::new("rustc").arg("--version").output()?;
	ensure!(version.status.success(), "rustc --version failed");
	let mut digest: Sha256 = Sha256::new();
	digest.update(&version.stdout);
	let mut identities: BTreeMap<String, String> = BTreeMap::new();
	for (name, bytes) in crate::bundle::PROBE_FILES {
		digest.update(name.as_bytes());
		digest.update(b"\0");
		digest.update(bytes);
		digest.update(b"\0");
		identities.insert((*name).to_owned(), format!("{:x}", Sha256::digest(bytes)));
	}
	let source_digest: String = format!("{:x}", digest.finalize());
	let output: PathBuf = config.state.join("probe").join(&source_digest[..16]);
	let dll: PathBuf = output.join("ArkBattleProbe.dll");
	let record_path: PathBuf = output.join("build.json");
	if record_path.is_file() && dll.is_file() {
		let record: Value = read_json(&record_path)?;
		if record["source_digest"] == source_digest
			&& record["exit_code"] == 0
			&& record["dll_sha256"] == sha256_file(&dll)?
		{
			return Ok(dll);
		}
	}
	fs::create_dir_all(output.join("sources"))?;
	for (name, bytes) in crate::bundle::PROBE_FILES {
		let path: PathBuf = output.join("sources").join(name);
		fs::create_dir_all(path.parent().unwrap())?;
		fs::write(path, bytes)?;
	}
	let sysroot = Command::new("rustc")
		.args(["--print", "sysroot"])
		.output()?;
	ensure!(sysroot.status.success(), "rustc --print sysroot failed");
	let linker: PathBuf = Path::new(String::from_utf8(sysroot.stdout)?.trim())
		.join("lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe");
	ensure!(linker.is_file(), "Installed rust-lld is required");
	let mut command: Command = Command::new("cargo");
	command
		.args(["build", "--offline", "--release", "--manifest-path"])
		.arg(&manifest)
		.args(["--target", "x86_64-pc-windows-msvc", "--target-dir"])
		.arg(config.state.join("cargo-probe"))
		.arg("--config")
		.arg(format!(
			"target.x86_64-pc-windows-msvc.linker={}",
			serde_json::to_string(&linker)?
		))
		.stdin(Stdio::null());
	// Cargo reports compiler diagnostics on stderr; keep them with the build record.
	let log: fs::File = fs::File::create(output.join("build.log"))?;
	command.stdout(log.try_clone()?).stderr(log);
	#[cfg(windows)]
	{
		use std::os::windows::process::CommandExt;
		command.creation_flags(0x08000000);
	}
	eprintln!(
		"INFO ark_battle_runtime: building Cargo probe {}",
		&source_digest[..16]
	);
	let status = command.status().context("Build the isolated Cargo probe")?;
	let mut record: Value = json!({"schema": "ark-battle-probe-build-v3", "source_digest": source_digest,
		"sources": identities, "builder": "cargo", "rustc": String::from_utf8(version.stdout)?.trim(),
		"exit_code": status.code(), "manifest": manifest});
	if status.success() {
		fs::copy(
			config
				.state
				.join("cargo-probe/x86_64-pc-windows-msvc/release/ark_battle_runtime_probe.dll"),
			&dll,
		)?;
		record["dll_sha256"] = json!(sha256_file(&dll)?);
	}
	write_json(&record_path, &record)?;
	ensure!(
		status.success(),
		"Probe build failed; see {}",
		record_path.display()
	);
	Ok(dll)
}
