//! Opt-in Unity engine calls from the native preload callback's own message thread.
use super::{
	elapsed_ms, record, GetCurrentThreadId, GetEnvironmentVariableW, GetModuleHandleW, Line,
	WriteFile, ASSEMBLY, CALLBACKS, LOG, STATUS,
};
use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

static PHASE: AtomicU32 = AtomicU32::new(0);
static FIRST_FRAME: AtomicI32 = AtomicI32::new(0);
static INSTANCE: AtomicI32 = AtomicI32::new(0);

#[link(name = "kernel32")]
extern "system" {
	fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
}

type Pointer = *mut c_void;
type NoArgs = unsafe extern "C" fn() -> Pointer;
type Invoke = unsafe extern "C" fn(Pointer, Pointer, *mut Pointer, *mut Pointer) -> Pointer;
type GcFree = unsafe extern "C" fn(u32);

struct Api {
	domain: NoArgs,
	thread: NoArgs,
	assembly: unsafe extern "C" fn(Pointer, *const c_char) -> Pointer,
	image: unsafe extern "C" fn(Pointer) -> Pointer,
	class: unsafe extern "C" fn(Pointer, *const c_char, *const c_char) -> Pointer,
	method: unsafe extern "C" fn(Pointer, *const c_char, i32) -> Pointer,
	virtual_method: unsafe extern "C" fn(Pointer, Pointer) -> Pointer,
	is_value_type: unsafe extern "C" fn(Pointer) -> bool,
	new_object: unsafe extern "C" fn(Pointer) -> Pointer,
	unbox: unsafe extern "C" fn(Pointer) -> Pointer,
	invoke: Invoke,
	gc_new: unsafe extern "C" fn(Pointer, bool) -> u32,
	gc_free: GcFree,
	icall: unsafe extern "C" fn(*const c_char) -> Pointer,
}

impl Api {
	unsafe fn load(module: Pointer) -> Result<Self, &'static str> {
		macro_rules! bind {
			($name:literal, $ty:ty) => {{
				let pointer: Pointer = GetProcAddress(module, $name.as_ptr().cast());
				if pointer.is_null() {
					return Err("api_export_missing");
				}
				core::mem::transmute::<Pointer, $ty>(pointer)
			}};
		}
		Ok(Self {
			domain: bind!(b"il2cpp_domain_get\0", NoArgs),
			thread: bind!(b"il2cpp_thread_current\0", NoArgs),
			assembly: bind!(
				b"il2cpp_domain_assembly_open\0",
				unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
			),
			image: bind!(
				b"il2cpp_assembly_get_image\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			class: bind!(
				b"il2cpp_class_from_name\0",
				unsafe extern "C" fn(Pointer, *const c_char, *const c_char) -> Pointer
			),
			method: bind!(
				b"il2cpp_class_get_method_from_name\0",
				unsafe extern "C" fn(Pointer, *const c_char, i32) -> Pointer
			),
			virtual_method: bind!(
				b"il2cpp_object_get_virtual_method\0",
				unsafe extern "C" fn(Pointer, Pointer) -> Pointer
			),
			is_value_type: bind!(
				b"il2cpp_class_is_valuetype\0",
				unsafe extern "C" fn(Pointer) -> bool
			),
			new_object: bind!(
				b"il2cpp_object_new\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			unbox: bind!(
				b"il2cpp_object_unbox\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			invoke: bind!(b"il2cpp_runtime_invoke\0", Invoke),
			gc_new: bind!(
				b"il2cpp_gchandle_new\0",
				unsafe extern "C" fn(Pointer, bool) -> u32
			),
			gc_free: bind!(b"il2cpp_gchandle_free\0", GcFree),
			icall: bind!(
				b"il2cpp_resolve_icall\0",
				unsafe extern "C" fn(*const c_char) -> Pointer
			),
		})
	}

	unsafe fn class_named(
		&self,
		image: Pointer,
		namespace: &[u8],
		name: &[u8],
	) -> Result<Pointer, &'static str> {
		let class: Pointer = (self.class)(image, namespace.as_ptr().cast(), name.as_ptr().cast());
		if class.is_null() {
			Err("unity_class_missing")
		} else {
			Ok(class)
		}
	}

	unsafe fn call(
		&self,
		class: Pointer,
		object: Pointer,
		name: &[u8],
		args: &mut [Pointer],
	) -> Result<Pointer, &'static str> {
		let mut method: Pointer = (self.method)(class, name.as_ptr().cast(), args.len() as i32);
		if method.is_null() {
			return Err("unity_method_missing");
		}
		if !object.is_null() && !(self.is_value_type)(class) {
			method = (self.virtual_method)(object, method);
			if method.is_null() {
				return Err("unity_virtual_method_missing");
			}
		}
		let mut exception: Pointer = core::ptr::null_mut();
		let parameters: *mut Pointer = if args.is_empty() {
			core::ptr::null_mut()
		} else {
			args.as_mut_ptr()
		};
		let result: Pointer = (self.invoke)(method, object, parameters, &mut exception);
		if exception.is_null() {
			Ok(result)
		} else {
			exception_log(exception);
			Err("unity_method_exception")
		}
	}

	unsafe fn integer(
		&self,
		class: Pointer,
		object: Pointer,
		name: &[u8],
	) -> Result<i32, &'static str> {
		let boxed: Pointer = self.call(class, object, name, &mut [])?;
		if boxed.is_null() {
			return Err("integer_result_null");
		}
		let data: Pointer = (self.unbox)(boxed);
		if data.is_null() {
			return Err("integer_unbox_null");
		}
		Ok(data.cast::<i32>().read())
	}

	unsafe fn root(&self, object: Pointer) -> Result<Rooted, &'static str> {
		if object.is_null() {
			return Err("object_result_null");
		}
		let handle: u32 = (self.gc_new)(object, false);
		if handle == 0 {
			return Err("gc_root_failed");
		}
		Ok(Rooted {
			handle,
			free: self.gc_free,
		})
	}
}

struct Rooted {
	handle: u32,
	free: GcFree,
}
impl Drop for Rooted {
	fn drop(&mut self) {
		unsafe { (self.free)(self.handle) };
	}
}

fn signed(line: &mut Line, value: i32) {
	if value < 0 {
		line.text("-");
	}
	line.number(value.unsigned_abs());
}

unsafe fn sample(stage: &str, frame: i32, instance: i32, roundtrip: i32) {
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"");
	line.text(stage);
	line.text("\",\"thread_id\":");
	line.number(GetCurrentThreadId());
	line.text(",\"callback_count\":");
	line.number(CALLBACKS.load(Ordering::Acquire));
	line.text(",\"frame_count\":");
	signed(&mut line, frame);
	line.text(",\"instance_id\":");
	signed(&mut line, instance);
	line.text(",\"roundtrip_instance_id\":");
	signed(&mut line, roundtrip);
	line.text("}\n");
	let mut written: u32 = 0;
	if WriteFile(
		LOG.load(Ordering::Acquire) as Pointer,
		line.bytes.as_ptr(),
		line.length as u32,
		&mut written,
		core::ptr::null_mut(),
	) == 0 || written != line.length as u32
	{
		STATUS.store(4, Ordering::Release);
	}
}

unsafe fn create_roundtrip(api: &Api, image: Pointer, frame: i32) -> Result<(), &'static str> {
	let game_class: Pointer = api.class_named(image, b"UnityEngine\0", b"GameObject\0")?;
	let object_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Object\0")?;
	let component_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Component\0")?;
	let object: Pointer = (api.new_object)(game_class);
	let _object_root: Rooted = api.root(object)?;
	record("game_object_ctor_start");
	api.call(game_class, object, b".ctor\0", &mut [])?;
	let result: Result<(i32, i32), &'static str> = (|| {
		record("transform_roundtrip_start");
		let transform: Pointer = api.call(game_class, object, b"get_transform\0", &mut [])?;
		let _transform_root: Rooted = api.root(transform)?;
		let back: Pointer = api.call(component_class, transform, b"get_gameObject\0", &mut [])?;
		let _back_root: Rooted = api.root(back)?;
		let instance: i32 = api.integer(object_class, object, b"GetInstanceID\0")?;
		let roundtrip: i32 = api.integer(object_class, back, b"GetInstanceID\0")?;
		if instance == 0 || instance != roundtrip {
			return Err("native_object_identity_mismatch");
		}
		Ok((instance, roundtrip))
	})();
	// Clean up the native GameObject even if its property round trip raised an exception.
	record("game_object_destroy_start");
	api.call(
		object_class,
		core::ptr::null_mut(),
		b"DestroyImmediate\0",
		&mut [object],
	)?;
	record("game_object_destroy_complete");
	let (instance, roundtrip): (i32, i32) = result?;
	INSTANCE.store(instance, Ordering::Release);
	FIRST_FRAME.store(frame, Ordering::Release);
	sample("engine_object_roundtrip", frame, instance, roundtrip);
	PHASE.store(1, Ordering::Release);
	Ok(())
}

unsafe fn probe() -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	if module.is_null() {
		return Ok(());
	}
	let api: Api = Api::load(module)?;
	let domain: Pointer = (api.domain)();
	if domain.is_null() || (api.thread)().is_null() {
		return Ok(());
	}
	let assembly: Pointer = (api.assembly)(domain, b"UnityEngine.CoreModule\0".as_ptr().cast());
	if assembly.is_null() {
		return Err("core_assembly_missing");
	}
	let image: Pointer = (api.image)(assembly);
	if image.is_null() {
		return Err("core_image_missing");
	}
	if PHASE.load(Ordering::Acquire) == 4 {
		if progress_original_scene(&api, domain, image)? {
			PHASE.store(
				if environment_flag(b"ARK_RUNTIME_RUN_BATTLE") {
					5
				} else {
					2
				},
				Ordering::Release,
			);
		}
		return Ok(());
	}
	if PHASE.load(Ordering::Acquire) == 5 {
		// Let the original Update drain its resource queue before advancing logic.
		if !battle_preloads_ready(&api, domain)? {
			return Ok(());
		}
		if environment_flag(b"ARK_RUNTIME_PRESENTATION") {
			return present_battle(&api, image);
		}
		record("original_battle_contiguous_manual_ticks");
		// Keep Unity render updates out of the bounded manual simulation. The original
		// OnTick drives deterministic coroutines and Physics2D itself in this mode.
		for _ in 0..1800 {
			if progress_battle(&api, domain)? {
				PHASE.store(2, Ordering::Release);
				return Ok(());
			}
		}
		return Err("battle_contiguous_tick_bound_exceeded");
	}
	let time_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Time\0")?;
	let frame: i32 = api.integer(time_class, core::ptr::null_mut(), b"get_frameCount\0")?;
	if PHASE.load(Ordering::Acquire) == 1 {
		if frame > FIRST_FRAME.load(Ordering::Acquire) {
			let instance: i32 = INSTANCE.load(Ordering::Acquire);
			sample("engine_probe_complete", frame, instance, instance);
			battle_component(&api, domain, image)?;
			if environment_flag(b"ARK_RUNTIME_SERVICES") {
				load_services(&api, domain, image)?;
			}
			if environment_flag(b"ARK_RUNTIME_SCENE_BUNDLES") {
				load_scene_bundles(&api, domain)?;
			}
			// Lua scripts route through the loaded bundles; load them before any battle code.
			if environment_flag(b"ARK_RUNTIME_LUA_HOTFIX") {
				load_original_lua(&api, domain)?;
			}
			PHASE.store(
				if environment_flag(b"ARK_RUNTIME_SCENE_LOAD") {
					4
				} else {
					2
				},
				Ordering::Release,
			);
		}
		return Ok(());
	}
	let scene_class: Pointer =
		api.class_named(image, b"UnityEngine.SceneManagement\0", b"SceneManager\0")?;
	let scenes: i32 = api.integer(scene_class, core::ptr::null_mut(), b"get_sceneCount\0")?;
	if frame <= 0 || scenes <= 0 {
		return Ok(());
	}
	record("runtime_and_scene_ready");
	if (api.icall)(b"UnityEngine.Component::get_gameObject()\0".as_ptr().cast()).is_null() {
		return Err("component_icall_missing");
	}
	record("component_icall_registered");
	create_roundtrip(&api, image, frame)
}

static TICK_RUNNING: AtomicU32 = AtomicU32::new(0);

// Presented battle: the original loop drives it as in the client. Its FixedUpdate ticks
// BattleController.OnTick through the game mode, whose forwarder runs the battle program; Update,
// coroutines and UI run as Unity schedules them. Pacing follows game seconds, scaled by the
// original speed level and the time scale.
unsafe fn present_battle(api: &Api, image: Pointer) -> Result<(), &'static str> {
	if LOOP_READY.load(Ordering::Acquire) != 0 {
		// Every original tick must pass through the forwarder; otherwise the battle would run
		// without the program and without observations, so stop loudly.
		let game: Pointer =
			(api.image)((api.assembly)((api.domain)(), b"Assembly-CSharp\0".as_ptr().cast()));
		let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
		let frames: i32 = api.integer(class, core::ptr::null_mut(), b"get_fixedFrameCnt\0")?;
		if LOOP_DONE.load(Ordering::Acquire) == 0
			&& frames as u32 > RUN_TICKS.load(Ordering::Acquire) + 1
		{
			return Err("original_loop_bypassed_tick_forwarder");
		}
		// The original ends a battle through its own flow, which stops ticking first; observe
		// the finish from here, since no further tick reaches the forwarder.
		if LOOP_DONE.load(Ordering::Acquire) == 0 && observe_loop_finish(api, game, class)? {
			LOOP_DONE.store(1, Ordering::Release);
		}
		if LOOP_DONE.load(Ordering::Acquire) != 0 {
			PHASE.store(2, Ordering::Release);
		}
		return Ok(());
	}
	let time_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Time\0")?;
	// Time scale as f32 bits for 1, 2, 4, 8 or 16; the no-CRT probe uses no floating point.
	let mut scale: u32 = match environment_digits(b"ARK_RUNTIME_TIME_SCALE").unwrap_or(1) {
		2 => 0x4000_0000,
		4 => 0x4080_0000,
		8 => 0x4100_0000,
		16 => 0x4180_0000,
		_ => 0x3F80_0000,
	};
	api.call(
		time_class,
		core::ptr::null_mut(),
		b"set_timeScale\0",
		&mut [(&mut scale as *mut u32).cast()],
	)?;
	set_speed_level(api, environment_digits(b"ARK_RUNTIME_SPEED_LEVEL").unwrap_or(1))?;
	LOOP_READY.store(1, Ordering::Release);
	record("original_battle_presented");
	Ok(())
}

pub unsafe fn tick() {
	if matches!(PHASE.load(Ordering::Acquire), 2 | 3) {
		return;
	}
	let env: [u16; 25] = [
		65, 82, 75, 95, 82, 85, 78, 84, 73, 77, 69, 95, 69, 78, 71, 73, 78, 69, 95, 80, 82, 79, 66, 69, 0,
	];
	let mut value: [u16; 3] = [0; 3];
	if GetEnvironmentVariableW(env.as_ptr(), value.as_mut_ptr(), 3) != 1 || value[0] != 49 {
		return;
	}
	// Unity can pump window messages while a native call is still in progress.
	// A nested callback must not repeat bootstrap or advance the same battle tick.
	if TICK_RUNNING
		.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
		.is_err()
	{
		if PHASE.load(Ordering::Acquire) == 5 && CALLBACKS.load(Ordering::Acquire) == 100 {
			let base: usize = module_base();
			let mut frames: [Pointer; 64] = [core::ptr::null_mut(); 64];
			let count: u16 =
				RtlCaptureStackBackTrace(0, 64, frames.as_mut_ptr(), core::ptr::null_mut());
			for index in 0..count as usize {
				let address: usize = frames.as_ptr().add(index).read() as usize;
				if address >= base && address - base < 214586344 {
					scene_number("owned_deployment_wait_game_rva", (address - base) as u32);
				}
			}
		}
		return;
	}
	if let Err(stage) = probe() {
		record(stage);
		record("engine_probe_failed");
		PHASE.store(3, Ordering::Release);
	}
	TICK_RUNNING.store(0, Ordering::Release);
}

unsafe fn module_base() -> usize {
	GetModuleHandleW(ASSEMBLY.as_ptr()) as usize
}
include!("battle-component.rs");
include!("battle-level.rs");
include!("battle-scene.rs");
include!("battle-services.rs");
include!("battle-actions.rs");
include!("battle-events.rs");
