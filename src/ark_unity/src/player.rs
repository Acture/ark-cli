//! Starts and closes only an owned Player process; observes its original-engine callbacks.
use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(windows, test))]
use std::fs;
use std::path::Path;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(any(windows, test))]
use anyhow::ensure;
use anyhow::Result;
use serde_json::{json, Value};

#[cfg(windows)]
use crate::io;
use crate::UNITY_VERSION;

#[cfg(any(windows, test))]
fn events(path: &Path, allow_in_flight: bool) -> Result<Vec<Value>> {
	if !path.exists() {
		return Ok(Vec::new());
	}
	let bytes: Vec<u8> = fs::read(path)?;
	let mut result: Vec<Value> = Vec::new();
	let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
	for (index, line) in lines.iter().enumerate() {
		let line: &[u8] = line.strip_suffix(b"\r").unwrap_or(line);
		if line.is_empty() {
			continue;
		}
		match serde_json::from_slice::<Value>(line) {
			Ok(value) => {
				ensure!(value.is_object(), "Observer record is not an object");
				result.push(value);
			}
			Err(_) if allow_in_flight && index == lines.len() - 1 && !bytes.ends_with(b"\n") => {}
			Err(error) => return Err(error.into()),
		}
	}
	Ok(result)
}

pub fn diagnostics(log: &str) -> Value {
	fn unique(log: &str, accept: impl Fn(&str) -> bool) -> Vec<String> {
		let mut seen: BTreeSet<&str> = BTreeSet::new();
		log.lines()
			.filter(|line| accept(line) && seen.insert(line))
			.map(str::to_owned)
			.collect()
	}
	json!({
		"missing_script_messages":log.matches("The referenced script").count(),
		"missing_singletons":unique(log, |x| x.starts_with("There should be exactly one singleton")),
		"exception_headers":unique(log, |x| x.contains("Exception:")).into_iter().take(12).collect::<Vec<_>>(),
		"font_load_failure_messages":log.matches("[DynFontLoader] Failed to load font").count(),
		"object_pool_failure_messages":log.matches("[ObjectPool] Failed to create new object").count(),
		"resource_manager_null_reference_observed":log.contains("at Torappu.Resource.ResourceManager") && log.contains("NullReferenceException")
	})
}

pub fn assess(mut record: Value, observer: Vec<Value>, log: &str) -> Value {
	let windows: Vec<Value> = record["unity_windows"]
		.as_array()
		.cloned()
		.unwrap_or_default();
	let threads: BTreeSet<u64> = windows
		.iter()
		.filter_map(|w| w["thread_id"].as_u64())
		.collect();
	let callbacks: Vec<&Value> = observer
		.iter()
		.filter(|e| e["stage"] == "message_loop_callback")
		.collect();
	let on_thread: bool = !callbacks.is_empty()
		&& callbacks.iter().all(|e| {
			e["same_thread"] == true
				&& e["thread_id"]
					.as_u64()
					.is_some_and(|t| threads.contains(&t))
		});
	let first: Vec<&Value> = observer
		.iter()
		.filter(|e| e["stage"] == "engine_object_roundtrip")
		.collect();
	let last: Vec<&Value> = observer
		.iter()
		.filter(|e| e["stage"] == "engine_probe_complete")
		.collect();
	let frames: bool = first.len() == 1
		&& last.len() == 1
		&& first[0]["frame_count"]
			.as_u64()
			.is_some_and(|f| f > 0 && last[0]["frame_count"].as_u64().is_some_and(|l| l > f))
		&& last[0]["callback_count"].as_u64() > first[0]["callback_count"].as_u64();
	let version: bool = log.contains(&format!("Initialize engine version: {UNITY_VERSION}"));
	let common: bool = record["exit_code"] == 0
		&& record["forced_termination"] == false
		&& version
		&& !threads.is_empty()
		&& record.get("error").is_none();
	let has = |stage: &str| observer.iter().any(|e| e["stage"] == stage);
	let engine: bool = common
		&& observer
			.first()
			.is_some_and(|e| e["stage"] == "plugin_load")
		&& on_thread
		&& callbacks
			.iter()
			.any(|e| e["callback_count"].as_u64().is_some_and(|c| c >= 10))
		&& observer
			.iter()
			.filter(|e| e.get("unity_player_loaded").is_some())
			.all(|e| e["unity_player_loaded"] == true && e["game_assembly_loaded"] == true)
		&& frames
		&& first[0]["instance_id"].as_i64().is_some_and(|i| i != 0)
		&& first[0]["instance_id"] == first[0]["roundtrip_instance_id"]
		&& has("component_icall_registered")
		&& has("game_object_destroy_complete")
		&& !has("engine_probe_failed")
		&& first.iter().chain(&last).all(|e| {
			e["thread_id"]
				.as_u64()
				.is_some_and(|t| threads.contains(&t))
		});
	record["callbacks_on_unity_window_thread"] = json!(on_thread);
	record["frames_verified"] = json!(frames);
	record["unity_version_logged"] = json!(version);
	record["engine_verified"] = json!(engine);
	record["passed"] = json!(engine);
	record["runtime_diagnostics"] = diagnostics(log);
	record["events"] = json!(observer);
	record
}

#[cfg(not(windows))]
pub fn run(
	_player: &Path,
	_output: &Path,
	_probe: &BTreeMap<String, String>,
	_completion: &str,
	_deadline: u64,
	_visible: bool,
) -> Result<Value> {
	anyhow::bail!("Original Player execution requires Windows x64")
}

#[cfg(windows)]
pub fn run(
	player: &Path,
	output: &Path,
	probe: &BTreeMap<String, String>,
	completion: &str,
	deadline: u64,
	visible: bool,
) -> Result<Value> {
	ensure!((1..=900).contains(&deadline), "Invalid Player deadline");
	ensure!(
		player.join("ArkBattleRuntime.exe").is_file(),
		"Owned Player executable missing"
	);
	fs::create_dir(output)?;
	let log = output.join("player.log");
	let observer_log = output.join("observer.jsonl");
	let stream = fs::File::create(output.join("stdout.log"))?;
	let started: Instant = Instant::now();
	let child =
		crate::windows_process::spawn(player, &log, &observer_log, probe, &stream, visible)?;
	let mut owned = OwnedChild(child);
	let pid: u32 = owned.0.id();
	eprintln!("INFO ark_unity: owned Player PID {pid}, deadline {deadline}s");
	let mut record: Value = json!({"mode":"enabled","process_id":pid,"probe_environment":probe,"deadline_seconds":deadline,"forced_termination":false,"close_requested":false});
	let observation = (|| -> Result<()> {
		let mut ready: bool = false;
		while owned.0.try_wait()?.is_none() && started.elapsed() < Duration::from_secs(deadline) {
			let windows: Vec<Value> = windows::owned(pid, !visible)?;
			if !windows.is_empty() {
				record["unity_windows"] = json!(windows);
				ready = events(&observer_log, true)?.iter().any(|e| {
					e["stage"] == completion
						|| e["stage"] == "engine_probe_failed"
						|| e["stage"] == "original_battle_tick_budget_exhausted"
				});
				if ready {
					break;
				}
			}
			std::thread::sleep(Duration::from_millis(50));
		}
		record["observation_deadline_reached"] =
			json!(!ready && started.elapsed() >= Duration::from_secs(deadline));
		if owned.0.try_wait()?.is_none() {
			record["close_requested"] = json!(windows::close(pid)?);
			let closing: Instant = Instant::now();
			while owned.0.try_wait()?.is_none() && closing.elapsed() < Duration::from_secs(5) {
				std::thread::sleep(Duration::from_millis(50));
			}
			if owned.0.try_wait()?.is_none() {
				record["forced_termination"] = json!(true);
				owned.0.kill()?;
			}
		}
		record["exit_code"] = json!(owned.0.wait()?.code());
		Ok(())
	})();
	if let Err(error) = observation {
		record["error"] = json!(format!("{error:#}"));
		if owned.0.try_wait()?.is_none() {
			record["forced_termination"] = json!(true);
			owned.0.kill()?;
		}
		record["exit_code"] = json!(owned.0.wait()?.code());
	}
	record["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
	let observer: Vec<Value> = events(&observer_log, false)?;
	let log: String = String::from_utf8_lossy(&fs::read(&log).unwrap_or_default()).into_owned();
	let record: Value = assess(record, observer, &log);
	io::write_json(&output.join("result.json"), &record)?;
	Ok(record)
}

#[cfg(windows)]
struct OwnedChild(crate::windows_process::Child);
#[cfg(windows)]
impl Drop for OwnedChild {
	fn drop(&mut self) {
		if !matches!(self.0.try_wait(), Ok(Some(_))) {
			let _ = self.0.kill();
			let _ = self.0.wait();
		}
	}
}

#[cfg(windows)]
mod windows {
	use super::*;
	use windows_sys::{
		core::BOOL,
		Win32::{
			Foundation::{HWND, LPARAM},
			UI::WindowsAndMessaging::{
				EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW, ShowWindow,
				SW_HIDE, WM_CLOSE,
			},
		},
	};
	struct Visit {
		pid: u32,
		// Hide the Player's window unless it is shown for watching.
		hide: bool,
		windows: Vec<Value>,
	}
	unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
		// EnumWindows invokes this synchronously; data points to its caller's live Visit.
		let state: &mut Visit = unsafe { &mut *(data as *mut Visit) };
		let mut owner: u32 = 0;
		let thread: u32 = unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
		if owner == state.pid {
			let mut class: [u16; 256] = [0; 256];
			let len: i32 = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
			if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == "UnityWndClass" {
				if state.hide {
					unsafe {
						ShowWindow(hwnd, SW_HIDE);
					}
				}
				state.windows.push(
					json!({"handle":hwnd as usize,"thread_id":thread,"class":"UnityWndClass"}),
				);
			}
		}
		1
	}
	pub(super) fn owned(pid: u32, hide: bool) -> Result<Vec<Value>> {
		let mut state = Visit {
			pid,
			hide,
			windows: Vec::new(),
		};
		ensure!(
			unsafe { EnumWindows(Some(visit), &mut state as *mut Visit as LPARAM) } != 0,
			"EnumWindows failed: {}",
			std::io::Error::last_os_error()
		);
		Ok(state.windows)
	}
	pub(super) fn close(pid: u32) -> Result<bool> {
		// Closing needs only the handles; never change visibility here.
		let windows = owned(pid, false)?;
		for window in &windows {
			let handle = window["handle"].as_u64().unwrap() as usize as HWND;
			// Recheck ownership immediately before posting WM_CLOSE.
			let mut owner: u32 = 0;
			unsafe {
				GetWindowThreadProcessId(handle, &mut owner);
			}
			ensure!(
				owner == pid && unsafe { PostMessageW(handle, WM_CLOSE, 0, 0) } != 0,
				"Owned Player close failed"
			);
		}
		Ok(!windows.is_empty())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn incomplete_tail_is_allowed_only_during_live_polling() {
		let path =
			std::env::temp_dir().join(format!("ark-observer-parser-{}.jsonl", std::process::id()));
		fs::write(&path, b"{\"stage\":\"good\"}\n{\"stage\":").unwrap();
		assert_eq!(events(&path, true).unwrap().len(), 1);
		assert!(events(&path, false).is_err());
		fs::write(&path, b"bad completed record\n{\"stage\":\"good\"}\n").unwrap();
		assert!(events(&path, true).is_err());
		fs::remove_file(path).unwrap();
	}
	#[test]
	fn normal_exit_without_engine_evidence_is_not_passed() {
		let value = assess(
			json!({"exit_code":0,"forced_termination":false}),
			vec![],
			&format!("Initialize engine version: {UNITY_VERSION}"),
		);
		assert_eq!(value["passed"], false);
	}
	#[test]
	fn diagnostics_keep_resource_errors() {
		let value = diagnostics("NullReferenceException: x\nat Torappu.Resource.ResourceManager\n[ObjectPool] Failed to create new object: missing");
		assert_eq!(value["object_pool_failure_messages"], 1);
		assert_eq!(value["resource_manager_null_reference_observed"], true);
	}
}
