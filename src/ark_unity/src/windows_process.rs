//! A narrowly scoped Windows GUI launcher with STARTF_USESHOWWINDOW set before process creation.
use anyhow::{ensure, Result};
use std::{
	collections::BTreeMap,
	ffi::{OsStr, OsString},
	fs::{File, OpenOptions},
	os::windows::{ffi::OsStrExt, io::AsRawHandle},
	path::Path,
	ptr,
};
use windows_sys::Win32::System::Threading::{
	DeleteProcThreadAttributeList, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
	EXTENDED_STARTUPINFO_PRESENT, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTUPINFOEXW,
};
use windows_sys::Win32::{
	Foundation::{
		CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
	},
	System::Threading::{
		CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
		CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION,
		STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES,
	},
};

struct Attributes(Vec<usize>);
impl Attributes {
	fn handles(handles: &[HANDLE]) -> Result<Self> {
		let mut size: usize = 0;
		// The first call obtains the required storage size; failure is expected.
		unsafe {
			InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
		}
		ensure!(
			size > 0 && size <= 65536,
			"Invalid process attribute buffer size"
		);
		let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
		ensure!(
			unsafe {
				InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut size)
			} != 0,
			"Initialize process attributes: {}",
			std::io::Error::last_os_error()
		);
		let mut result = Self(storage);
		ensure!(
			unsafe {
				UpdateProcThreadAttribute(
					result.0.as_mut_ptr().cast(),
					0,
					PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
					handles.as_ptr().cast(),
					std::mem::size_of_val(handles),
					ptr::null_mut(),
					ptr::null(),
				)
			} != 0,
			"Set explicit child handle list: {}",
			std::io::Error::last_os_error()
		);
		Ok(result)
	}
}
impl Drop for Attributes {
	fn drop(&mut self) {
		unsafe {
			DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
		}
	}
}

pub(crate) struct Child {
	handle: HANDLE,
	pid: u32,
}
pub(crate) struct Status(i32);
impl Status {
	pub fn code(&self) -> Option<i32> {
		Some(self.0)
	}
}
impl Child {
	pub fn id(&self) -> u32 {
		self.pid
	}
	pub fn try_wait(&mut self) -> Result<Option<Status>> {
		// SAFETY: handle is the owned live process handle returned by CreateProcessW.
		match unsafe { WaitForSingleObject(self.handle, 0) } {
			WAIT_TIMEOUT => Ok(None),
			WAIT_OBJECT_0 => {
				let mut exit: u32 = 0;
				ensure!(
					unsafe { GetExitCodeProcess(self.handle, &mut exit) } != 0,
					"GetExitCodeProcess: {}",
					std::io::Error::last_os_error()
				);
				Ok(Some(Status(exit as i32)))
			}
			_ => anyhow::bail!("WaitForSingleObject: {}", std::io::Error::last_os_error()),
		}
	}
	pub fn kill(&mut self) -> Result<()> {
		ensure!(
			unsafe { TerminateProcess(self.handle, 1) } != 0,
			"Terminate owned Player: {}",
			std::io::Error::last_os_error()
		);
		Ok(())
	}
	pub fn wait(&mut self) -> Result<Status> {
		ensure!(
			unsafe { WaitForSingleObject(self.handle, INFINITE) } == WAIT_OBJECT_0,
			"Wait for owned Player failed"
		);
		Ok(self.try_wait()?.expect("Signalled process"))
	}
}
impl Drop for Child {
	fn drop(&mut self) {
		if !matches!(self.try_wait(), Ok(Some(_))) {
			let _ = self.kill();
			let _ = self.wait();
		}
		// SAFETY: process ownership ends here, exactly once.
		unsafe {
			CloseHandle(self.handle);
		}
	}
}

fn quote(value: &OsStr) -> Vec<u16> {
	let mut result = vec![34];
	let mut slashes: usize = 0;
	for c in value.encode_wide() {
		if c == 92 {
			slashes += 1;
			continue;
		}
		if c == 34 {
			result.extend(std::iter::repeat_n(92, slashes * 2 + 1));
		} else {
			result.extend(std::iter::repeat_n(92, slashes));
		}
		slashes = 0;
		result.push(c);
	}
	result.extend(std::iter::repeat_n(92, slashes * 2));
	result.push(34);
	result
}

pub(crate) fn spawn(
	player: &Path,
	log: &Path,
	observer: &Path,
	probe: &BTreeMap<String, String>,
	stream: &File,
	visible: bool,
) -> Result<Child> {
	let executable = player.join("ArkBattleRuntime.exe");
	let (width, height): (&str, &str) = if visible {
		("1280", "720")
	} else {
		("160", "90")
	};
	let mut application: Vec<u16> = executable.as_os_str().encode_wide().collect();
	application.push(0);
	let args: Vec<OsString> = vec![
		executable.as_os_str().to_owned(),
		"-screen-fullscreen".into(),
		"0".into(),
		"-screen-width".into(),
		width.into(),
		"-screen-height".into(),
		height.into(),
		"-logFile".into(),
		log.as_os_str().to_owned(),
	];
	let mut command: Vec<u16> = Vec::new();
	for arg in args {
		if !command.is_empty() {
			command.push(32);
		}
		command.extend(quote(&arg));
	}
	command.push(0);
	let mut variables: BTreeMap<String, (OsString, OsString)> = std::env::vars_os()
		.filter(|(k, _)| {
			!k.to_string_lossy()
				.to_ascii_uppercase()
				.starts_with("ARK_RUNTIME_")
		})
		.map(|(k, v)| (k.to_string_lossy().to_ascii_uppercase(), (k, v)))
		.collect();
	for (key, value) in probe {
		variables.insert(key.to_ascii_uppercase(), (key.into(), value.into()));
	}
	variables.insert(
		"ARK_RUNTIME_PROBE_LOG".to_owned(),
		(
			"ARK_RUNTIME_PROBE_LOG".into(),
			observer.as_os_str().to_owned(),
		),
	);
	let mut environment: Vec<u16> = Vec::new();
	for (_, (key, value)) in variables {
		environment.extend(key.encode_wide());
		environment.push(61);
		environment.extend(value.encode_wide());
		environment.push(0);
	}
	environment.push(0);
	let mut directory: Vec<u16> = player.as_os_str().encode_wide().collect();
	directory.push(0);
	let input = OpenOptions::new().read(true).open("NUL")?;
	let stdout = stream.as_raw_handle() as HANDLE;
	let stdin = input.as_raw_handle() as HANDLE;
	// Only these two local file handles need inheritance for the child streams.
	ensure!(
		unsafe { SetHandleInformation(stdout, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } != 0,
		"Make stdout inheritable"
	);
	ensure!(
		unsafe { SetHandleInformation(stdin, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } != 0,
		"Make stdin inheritable"
	);
	let handles = [stdout, stdin];
	let mut attributes = Attributes::handles(&handles)?;
	let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
	startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
	startup.StartupInfo.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
	// SW_SHOWNORMAL when watching, SW_HIDE otherwise.
	startup.StartupInfo.wShowWindow = u16::from(visible);
	startup.StartupInfo.hStdInput = stdin;
	startup.StartupInfo.hStdOutput = stdout;
	startup.StartupInfo.hStdError = stdout;
	startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
	let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
	// SAFETY: all UTF-16 buffers are terminated, writable where required, and live through
	// this synchronous call. Startup handles remain live and the output struct is initialized.
	let created = unsafe {
		CreateProcessW(
			application.as_ptr(),
			command.as_mut_ptr(),
			ptr::null(),
			ptr::null(),
			1,
			CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
			environment.as_ptr().cast(),
			directory.as_ptr(),
			&startup.StartupInfo,
			&mut process,
		)
	};
	let error = std::io::Error::last_os_error();
	unsafe {
		SetHandleInformation(stdout, HANDLE_FLAG_INHERIT, 0);
		SetHandleInformation(stdin, HANDLE_FLAG_INHERIT, 0);
	}
	ensure!(created != 0, "Create owned Player process: {error}");
	unsafe {
		CloseHandle(process.hThread);
	}
	Ok(Child {
		handle: process.hProcess,
		pid: process.dwProcessId,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn command_line_quotes_spaces_slashes_and_embedded_quotes() {
		assert_eq!(
			String::from_utf16(&quote(OsStr::new("C:\\a b\\"))).unwrap(),
			"\"C:\\a b\\\\\""
		);
		assert_eq!(
			String::from_utf16(&quote(OsStr::new("a\"b"))).unwrap(),
			"\"a\\\"b\""
		);
	}
}
