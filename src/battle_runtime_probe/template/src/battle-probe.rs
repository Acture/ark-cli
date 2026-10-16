//! Original runtime and battle proof in a self-owned Unity Player.
#![no_std]
#[path = "battle-probe-api.rs"]
mod engine_probe_api;

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

static LOG: AtomicUsize = AtomicUsize::new(0);
static TIMER: AtomicUsize = AtomicUsize::new(0);
static LOAD_THREAD: AtomicU32 = AtomicU32::new(0);
static CALLBACKS: AtomicU32 = AtomicU32::new(0);
static STATUS: AtomicU32 = AtomicU32::new(0);
static LOADED_AT: AtomicUsize = AtomicUsize::new(0);

const LOG_ENV: [u16; 22] = [
	65, 82, 75, 95, 82, 85, 78, 84, 73, 77, 69, 95, 80, 82, 79, 66, 69, 95, 76, 79, 71, 0,
];
const PLAYER: [u16; 16] = [
	85, 110, 105, 116, 121, 80, 108, 97, 121, 101, 114, 46, 100, 108, 108, 0,
];
const ASSEMBLY: [u16; 17] = [
	71, 97, 109, 101, 65, 115, 115, 101, 109, 98, 108, 121, 46, 100, 108, 108, 0,
];

#[link(name = "kernel32")]
extern "system" {
	fn GetCurrentThreadId() -> u32;
	fn GetTickCount64() -> u64;
	fn GetEnvironmentVariableW(name: *const u16, value: *mut u16, size: u32) -> u32;
	fn GetModuleHandleW(name: *const u16) -> *mut c_void;
	fn CreateFileW(
		name: *const u16,
		access: u32,
		share: u32,
		security: *const c_void,
		creation: u32,
		flags: u32,
		template: *mut c_void,
	) -> *mut c_void;
	fn WriteFile(
		file: *mut c_void,
		buffer: *const u8,
		length: u32,
		written: *mut u32,
		overlapped: *mut c_void,
	) -> i32;
	fn CloseHandle(handle: *mut c_void) -> i32;
}

type TimerCallback = unsafe extern "system" fn(*mut c_void, u32, usize, u32);

#[link(name = "user32")]
extern "system" {
	fn SetTimer(
		window: *mut c_void,
		id: usize,
		milliseconds: u32,
		callback: Option<TimerCallback>,
	) -> usize;
	fn KillTimer(window: *mut c_void, id: usize) -> i32;
}

struct Line {
	bytes: [u8; 384],
	length: usize,
}

impl Line {
	fn text(&mut self, value: &str) {
		if value.len() > self.bytes.len() - self.length {
			return;
		}
		// Both bounds were checked above. Avoid core's panic/formatting machinery and its
		// C-runtime dependencies in this standalone observer DLL.
		unsafe {
			memcpy(
				self.bytes.as_mut_ptr().add(self.length).cast(),
				value.as_ptr().cast(),
				value.len(),
			);
		}
		self.length += value.len();
	}

	fn number(&mut self, mut value: u32) {
		let mut digits: [u8; 10] = [0; 10];
		let mut length: usize = 0;
		loop {
			unsafe {
				digits
					.as_mut_ptr()
					.add(length)
					.write((value % 10) as u8 + b'0')
			};
			length += 1;
			value /= 10;
			if value == 0 {
				break;
			}
		}
		if length > self.bytes.len() - self.length {
			return;
		}
		while length != 0 {
			length -= 1;
			unsafe {
				self.bytes
					.as_mut_ptr()
					.add(self.length)
					.write(digits.as_ptr().add(length).read());
			}
			self.length += 1;
		}
	}

	fn boolean(&mut self, value: bool) {
		self.text(if value { "true" } else { "false" });
	}
}

// Milliseconds since plugin load, for profiling where a run's wall-clock time goes.
unsafe fn elapsed_ms() -> u32 {
	GetTickCount64().wrapping_sub(LOADED_AT.load(Ordering::Acquire) as u64) as u32
}

unsafe fn record(stage: &str) {
	let handle: usize = LOG.load(Ordering::Acquire);
	if handle == 0 {
		return;
	}
	let thread: u32 = GetCurrentThreadId();
	let load_thread: u32 = LOAD_THREAD.load(Ordering::Acquire);
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"");
	line.text(stage);
	line.text("\",\"thread_id\":");
	line.number(thread);
	line.text(",\"load_thread_id\":");
	line.number(load_thread);
	line.text(",\"same_thread\":");
	line.boolean(thread == load_thread);
	line.text(",\"callback_count\":");
	line.number(CALLBACKS.load(Ordering::Acquire));
	line.text(",\"unity_player_loaded\":");
	line.boolean(!GetModuleHandleW(PLAYER.as_ptr()).is_null());
	line.text(",\"game_assembly_loaded\":");
	line.boolean(!GetModuleHandleW(ASSEMBLY.as_ptr()).is_null());
	line.text(",\"elapsed_ms\":");
	line.number(elapsed_ms());
	line.text("}\n");
	let mut written: u32 = 0;
	if WriteFile(
		handle as *mut c_void,
		line.bytes.as_ptr(),
		line.length as u32,
		&mut written,
		core::ptr::null_mut(),
	) == 0 || written != line.length as u32
	{
		STATUS.store(4, Ordering::Release);
	}
}

unsafe extern "system" fn tick(_: *mut c_void, _: u32, _: usize, _: u32) {
	let count: u32 = CALLBACKS.fetch_add(1, Ordering::AcqRel) + 1;
	if GetCurrentThreadId() != LOAD_THREAD.load(Ordering::Acquire) {
		record("unexpected_thread");
		STATUS.store(5, Ordering::Release);
		let timer: usize = TIMER.swap(0, Ordering::AcqRel);
		KillTimer(core::ptr::null_mut(), timer);
		return;
	}
	if count <= 3 || count == 10 || count % 100 == 0 {
		record("message_loop_callback");
	}
	if count >= 10 {
		engine_probe_api::tick();
	}
}

/// Unity's preloaded-plugin ABI. The smoke test invokes this entry without a Unity runtime.
#[no_mangle]
pub unsafe extern "system" fn UnityPluginLoad(_: *mut c_void) {
	if LOG.load(Ordering::Acquire) != 0 {
		return;
	}
	let mut path: [u16; 520] = [0; 520];
	let length: u32 = GetEnvironmentVariableW(LOG_ENV.as_ptr(), path.as_mut_ptr(), 520);
	if length == 0 || length >= 520 {
		return;
	}
	// Require an explicit drive-absolute output path. Never default to the application directory.
	if length < 3 || path[1] != 58 || (path[2] != 47 && path[2] != 92) {
		STATUS.store(2, Ordering::Release);
		return;
	}
	let handle: *mut c_void = CreateFileW(
		path.as_ptr(),
		4,
		3,
		core::ptr::null(),
		4,
		0x80,
		core::ptr::null_mut(),
	);
	if handle as isize == -1 {
		STATUS.store(2, Ordering::Release);
		return;
	}
	LOG.store(handle as usize, Ordering::Release);
	LOAD_THREAD.store(GetCurrentThreadId(), Ordering::Release);
	LOADED_AT.store(GetTickCount64() as usize, Ordering::Release);
	CALLBACKS.store(0, Ordering::Release);
	STATUS.store(1, Ordering::Release);
	record("plugin_load");
	let timer: usize = SetTimer(core::ptr::null_mut(), 0, 25, Some(tick));
	TIMER.store(timer, Ordering::Release);
	if timer == 0 {
		STATUS.store(3, Ordering::Release);
		record("timer_unavailable");
	}
}

#[no_mangle]
pub unsafe extern "system" fn UnityPluginUnload() {
	let timer: usize = TIMER.swap(0, Ordering::AcqRel);
	if timer != 0 {
		KillTimer(core::ptr::null_mut(), timer);
	}
	record("plugin_unload");
	let handle: usize = LOG.swap(0, Ordering::AcqRel);
	if handle != 0 {
		CloseHandle(handle as *mut c_void);
	}
}

#[no_mangle]
pub extern "system" fn ArkBattleProbeStatus() -> u32 {
	STATUS.load(Ordering::Acquire)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
	unsafe { core::arch::asm!("ud2", options(noreturn)) }
}

// Supply the compiler's byte intrinsics without importing a C runtime. Volatile operations
// prevent LLVM from lowering these definitions recursively into calls to themselves.
#[no_mangle]
unsafe extern "C" fn memcpy(
	destination: *mut c_void,
	source: *const c_void,
	length: usize,
) -> *mut c_void {
	for offset in 0..length {
		destination
			.cast::<u8>()
			.add(offset)
			.write_volatile(source.cast::<u8>().add(offset).read_volatile());
	}
	destination
}

#[no_mangle]
unsafe extern "C" fn memset(destination: *mut c_void, value: i32, length: usize) -> *mut c_void {
	for offset in 0..length {
		destination
			.cast::<u8>()
			.add(offset)
			.write_volatile(value as u8);
	}
	destination
}
