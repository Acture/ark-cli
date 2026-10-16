unsafe fn exception_log(exception: Pointer) {
	exception_part(exception, b"il2cpp_format_exception\0", "managed_exception");
	exception_part(
		exception,
		b"il2cpp_format_stack_trace\0",
		"managed_exception_stack",
	);
}
unsafe fn exception_part(exception: Pointer, export: &[u8], stage: &str) {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let function: Pointer = GetProcAddress(module, export.as_ptr().cast());
	if function.is_null() {
		return;
	}
	let format: unsafe extern "C" fn(Pointer, *mut c_char, i32) = core::mem::transmute(function);
	let mut message: [u8; 2048] = [0; 2048];
	format(exception, message.as_mut_ptr().cast(), 2048);
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"");
	line.text(stage);
	line.text("\",\"message\":\"");
	let mut index: usize = 0;
	while index < 2047 && line.length < 300 {
		// index stays within the fixed buffer; JSON termination has reserved space.
		let value: u8 = message.as_ptr().add(index).read();
		if value == 0 {
			break;
		}
		match value {
			b'"' => line.text("\\\""),
			b'\\' => line.text("\\\\"),
			b'\n' => line.text("\\n"),
			b'\r' => line.text("\\r"),
			32..=126 => {
				line.bytes.as_mut_ptr().add(line.length).write(value);
				line.length += 1;
			}
			_ => line.text("?"),
		}
		index += 1;
	}
	line.text("\",\"truncated\":");
	line.boolean(message.as_ptr().add(index).read() != 0);
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

unsafe fn battle_component(api: &Api, domain: Pointer, image: Pointer) -> Result<(), &'static str> {
	record("battle_type_resolve_start");
	let assembly: Pointer = (api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast());
	if assembly.is_null() {
		return Err("battle_assembly_missing");
	}
	let battle_image: Pointer = (api.image)(assembly);
	if battle_image.is_null() {
		return Err("battle_image_missing");
	}
	let battle_class: Pointer =
		api.class_named(battle_image, b"Torappu.Battle\0", b"BattleController\0")?;
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let type_export: Pointer = GetProcAddress(module, b"il2cpp_class_get_type\0".as_ptr().cast());
	let reflection_export: Pointer =
		GetProcAddress(module, b"il2cpp_type_get_object\0".as_ptr().cast());
	let object_class_export: Pointer =
		GetProcAddress(module, b"il2cpp_object_get_class\0".as_ptr().cast());
	if type_export.is_null() || reflection_export.is_null() || object_class_export.is_null() {
		return Err("reflection_api_missing");
	}
	let class_type: unsafe extern "C" fn(Pointer) -> Pointer = core::mem::transmute(type_export);
	let reflection_type: unsafe extern "C" fn(Pointer) -> Pointer =
		core::mem::transmute(reflection_export);
	let object_class_fn: unsafe extern "C" fn(Pointer) -> Pointer =
		core::mem::transmute(object_class_export);
	let parent_export: Pointer =
		GetProcAddress(module, b"il2cpp_class_get_parent\0".as_ptr().cast());
	if parent_export.is_null() {
		return Err("parent_api_missing");
	}
	let class_parent: unsafe extern "C" fn(Pointer) -> Pointer =
		core::mem::transmute(parent_export);
	let singleton_class: Pointer = class_parent(battle_class);
	if singleton_class.is_null() {
		return Err("singleton_base_missing");
	}
	let activate: bool = environment_flag(b"ARK_RUNTIME_BATTLE_ACTIVATE");
	let native_type: Pointer = class_type(battle_class);
	if native_type.is_null() {
		return Err("battle_type_null");
	}
	let reflected: Pointer = reflection_type(native_type);
	let _type_root: Rooted = api.root(reflected)?;
	let game_class: Pointer = api.class_named(image, b"UnityEngine\0", b"GameObject\0")?;
	let object_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Object\0")?;
	let component_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Component\0")?;
	let owner: Pointer = (api.new_object)(game_class);
	let _owner_root: Rooted = api.root(owner)?;
	api.call(game_class, owner, b".ctor\0", &mut [])?;
	let result: Result<i32, &'static str> = (|| {
		// Attach to an inactive object for this component identity test, before any
		// activation or explicit battle initialization is requested.
		let mut active: bool = false;
		api.call(
			game_class,
			owner,
			b"SetActive\0",
			&mut [(&mut active as *mut bool).cast()],
		)?;
		record("battle_component_add_start");
		let battle: Pointer = api.call(
			game_class,
			owner,
			b"Internal_AddComponentWithType\0",
			&mut [reflected],
		)?;
		let _battle_root: Rooted = api.root(battle)?;
		if object_class_fn(battle) != battle_class {
			return Err("battle_component_type_mismatch");
		}
		record("battle_component_owner_start");
		let back: Pointer = api.call(component_class, battle, b"get_gameObject\0", &mut [])?;
		let _back_root: Rooted = api.root(back)?;
		let owner_id: i32 = api.integer(object_class, owner, b"GetInstanceID\0")?;
		let back_id: i32 = api.integer(object_class, back, b"GetInstanceID\0")?;
		if owner_id == 0 || owner_id != back_id {
			return Err("battle_component_owner_mismatch");
		}
		if activate || environment_flag(b"ARK_RUNTIME_BATTLE_INIT") {
			initialize_controller(api, module, battle_image, battle_class, battle, !activate)?;
		}
		if activate {
			activate_controller(
				api,
				module,
				game_class,
				singleton_class,
				battle_class,
				owner,
				battle,
			)?;
			if environment_flag(b"ARK_RUNTIME_DECODE_LEVEL") {
				decode_level(api, domain, image, battle_image)?;
			}
		}
		Ok(owner_id)
	})();
	record("battle_owner_destroy_start");
	api.call(
		object_class,
		core::ptr::null_mut(),
		b"DestroyImmediate\0",
		&mut [owner],
	)?;
	record("battle_owner_destroy_complete");
	if activate {
		let remaining: Pointer = api.call(
			singleton_class,
			core::ptr::null_mut(),
			b"get_instanceOrNull\0",
			&mut [],
		)?;
		if !remaining.is_null() {
			return Err("battle_singleton_not_cleared");
		}
		record("battle_singleton_destroy_cleared");
	}
	let owner_id: i32 = result?;
	let time_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Time\0")?;
	let frame: i32 = api.integer(time_class, core::ptr::null_mut(), b"get_frameCount\0")?;
	sample("battle_component_complete", frame, owner_id, owner_id);
	Ok(())
}

unsafe fn create_manual_meta(
	api: &Api,
	module: Pointer,
	image: Pointer,
) -> Result<Pointer, &'static str> {
	macro_rules! bind {
		($name:literal, $ty:ty) => {{
			let pointer: Pointer = GetProcAddress(module, $name.as_ptr().cast());
			if pointer.is_null() {
				return Err("meta_api_export_missing");
			}
			core::mem::transmute::<Pointer, $ty>(pointer)
		}};
	}
	let class_init = bind!(
		b"il2cpp_runtime_class_init\0",
		unsafe extern "C" fn(Pointer)
	);
	let value_size = bind!(
		b"il2cpp_class_value_size\0",
		unsafe extern "C" fn(Pointer, *mut u32) -> i32
	);
	let field = bind!(
		b"il2cpp_class_get_field_from_name\0",
		unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
	);
	let static_get = bind!(
		b"il2cpp_field_static_get_value\0",
		unsafe extern "C" fn(Pointer, Pointer)
	);
	let value_box = bind!(
		b"il2cpp_value_box\0",
		unsafe extern "C" fn(Pointer, Pointer) -> Pointer
	);
	let field_set = bind!(
		b"il2cpp_field_set_value\0",
		unsafe extern "C" fn(Pointer, Pointer, Pointer)
	);
	let meta_class: Pointer = api.class_named(image, b"Torappu.Battle\0", b"GameModeMeta\0")?;
	class_init(meta_class);
	let mut alignment: u32 = 0;
	let size: i32 = value_size(meta_class, &mut alignment);
	let default: Pointer = field(meta_class, b"DEFAULT\0".as_ptr().cast());
	if size <= 0 || size > 64 || default.is_null() {
		return Err("game_mode_meta_shape_mismatch");
	}
	let mut data: [u8; 64] = [0; 64];
	static_get(default, data.as_mut_ptr().cast());
	let meta: Pointer = value_box(meta_class, data.as_mut_ptr().cast());
	let _meta_root: Rooted = api.root(meta)?;
	let field_offset = bind!(
		b"il2cpp_field_get_offset\0",
		unsafe extern "C" fn(Pointer) -> usize
	);
	// Manual ticks need a deterministic game mode that allows them. A presented battle keeps the
	// original default meta, as the client does, so the original loop drives it.
	let presented: bool = environment_flag(b"ARK_RUNTIME_PRESENTATION");
	let mut enabled: bool = true;
	for (name, stage) in [
		(b"isDeterministic\0".as_slice(), "original_default_meta_deterministic"),
		(b"allowManualTick\0".as_slice(), "original_default_meta_manual_tick"),
	] {
		let target: Pointer = field(meta_class, name.as_ptr().cast());
		if target.is_null() {
			return Err("game_mode_meta_field_missing");
		}
		// A boxed value's field offset includes the object header.
		let value: u8 = (api.unbox)(meta)
			.cast::<u8>()
			.add(field_offset(target) - 2 * core::mem::size_of::<usize>())
			.read();
		scene_number(stage, value as u32);
		if !presented {
			field_set(meta, target, (&mut enabled as *mut bool).cast());
		}
	}
	record("original_default_meta_loaded");
	Ok(meta)
}

unsafe fn initialize_controller(
	api: &Api,
	module: Pointer,
	image: Pointer,
	controller_class: Pointer,
	controller: Pointer,
	direct_on_init: bool,
) -> Result<(), &'static str> {
	let meta: Pointer = create_manual_meta(api, module, image)?;
	let _meta_root: Rooted = api.root(meta)?;
	let value: Pointer = (api.unbox)(meta);
	if value.is_null() {
		return Err("game_mode_meta_unbox_null");
	}
	record("battle_early_init_start");
	api.call(
		controller_class,
		core::ptr::null_mut(),
		b"BattleInitializerOnly_EarlyInit\0",
		&mut [value],
	)?;
	record("battle_early_init_complete");
	if direct_on_init {
		record("battle_controller_on_init_start");
		api.call(controller_class, controller, b"OnInit\0", &mut [])?;
		record("battle_controller_on_init_complete");
	}
	Ok(())
}

// A decimal environment variable of at most nine digits; None when absent or malformed.
unsafe fn environment_digits(key: &[u8]) -> Option<u32> {
	let mut name: [u16; 64] = [0; 64];
	for index in 0..key.len().min(63) {
		name.as_mut_ptr()
			.add(index)
			.write(key.as_ptr().add(index).read() as u16);
	}
	let mut value: [u16; 16] = [0; 16];
	let length: u32 = GetEnvironmentVariableW(name.as_ptr(), value.as_mut_ptr(), 16);
	if length == 0 || length > 9 {
		return None;
	}
	let mut number: u32 = 0;
	for index in 0..length as usize {
		let digit: u16 = value.as_ptr().add(index).read();
		if !(48..=57).contains(&digit) {
			return None;
		}
		number = number * 10 + (digit - 48) as u32;
	}
	Some(number)
}

unsafe fn environment_flag(name: &[u8]) -> bool {
	if name.len() >= 64 {
		return false;
	}
	let mut key: [u16; 64] = [0; 64];
	for index in 0..name.len() {
		key.as_mut_ptr()
			.add(index)
			.write(name.as_ptr().add(index).read() as u16);
	}
	let mut value: [u16; 3] = [0; 3];
	GetEnvironmentVariableW(key.as_ptr(), value.as_mut_ptr(), 3) == 1 && value[0] == 49
}

unsafe fn managed_bool(
	api: &Api,
	class: Pointer,
	object: Pointer,
	name: &[u8],
) -> Result<bool, &'static str> {
	let boxed: Pointer = api.call(class, object, name, &mut [])?;
	let _root: Rooted = api.root(boxed)?;
	let data: Pointer = (api.unbox)(boxed);
	if data.is_null() {
		return Err("boolean_unbox_null");
	}
	Ok(data.cast::<u8>().read() != 0)
}

unsafe fn activate_controller(
	api: &Api,
	module: Pointer,
	game_class: Pointer,
	singleton_class: Pointer,
	controller_class: Pointer,
	owner: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let before: Pointer = api.call(
		singleton_class,
		core::ptr::null_mut(),
		b"get_instanceOrNull\0",
		&mut [],
	)?;
	if !before.is_null() {
		return Err("battle_singleton_already_present");
	}
	record("battle_singleton_before_null");
	record("battle_activation_start");
	let mut active: bool = true;
	api.call(
		game_class,
		owner,
		b"SetActive\0",
		&mut [(&mut active as *mut bool).cast()],
	)?;
	if !managed_bool(api, game_class, owner, b"get_activeInHierarchy\0")? {
		return Err("battle_owner_not_active");
	}
	let instance: Pointer = api.call(
		singleton_class,
		core::ptr::null_mut(),
		b"get_instanceOrNull\0",
		&mut [],
	)?;
	if instance != controller
		|| !managed_bool(
			api,
			singleton_class,
			core::ptr::null_mut(),
			b"get_hasInstance\0",
		)? {
		return Err("battle_singleton_binding_mismatch");
	}
	record("battle_singleton_bound");
	let field_export: Pointer = GetProcAddress(
		module,
		b"il2cpp_class_get_field_from_name\0".as_ptr().cast(),
	);
	let get_export: Pointer = GetProcAddress(module, b"il2cpp_field_get_value\0".as_ptr().cast());
	if field_export.is_null() || get_export.is_null() {
		return Err("controller_field_api_missing");
	}
	let field: unsafe extern "C" fn(Pointer, *const c_char) -> Pointer =
		core::mem::transmute(field_export);
	let get: unsafe extern "C" fn(Pointer, Pointer, Pointer) = core::mem::transmute(get_export);
	let queue_field: Pointer = field(controller_class, b"m_playerOpQueue\0".as_ptr().cast());
	if queue_field.is_null() {
		return Err("controller_queue_field_missing");
	}
	let mut queue: Pointer = core::ptr::null_mut();
	get(controller, queue_field, (&mut queue as *mut Pointer).cast());
	if queue.is_null() {
		return Err("controller_awake_queue_not_initialized");
	}
	if managed_bool(api, controller_class, controller, b"get_isPlaying\0")? {
		return Err("controller_playing_before_load");
	}
	record("battle_awake_queue_initialized");
	record("battle_activation_complete");
	Ok(())
}
