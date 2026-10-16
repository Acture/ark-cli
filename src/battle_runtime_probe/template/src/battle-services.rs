// Bootstrap original services using byte-identical serialized configs and original DBLoader.
unsafe fn managed_literal(module: Pointer, value: &[u8]) -> Result<Pointer, &'static str> {
	let string_new = level_bind!(
		module,
		b"il2cpp_string_new\0",
		unsafe extern "C" fn(*const c_char) -> Pointer
	);
	Ok(string_new(value.as_ptr().cast()))
}

unsafe fn method_owner(
	module: Pointer,
	mut class: Pointer,
	name: &[u8],
	count: i32,
) -> Result<Pointer, &'static str> {
	let parent = level_bind!(
		module,
		b"il2cpp_class_get_parent\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let method = level_bind!(
		module,
		b"il2cpp_class_get_method_from_name\0",
		unsafe extern "C" fn(Pointer, *const c_char, i32) -> Pointer
	);
	// Walk declared methods via typed_call; this export also searches ancestors, so use
	// explicit enumeration to return the actual declaring class for generic table methods.
	let methods = level_bind!(
		module,
		b"il2cpp_class_get_methods\0",
		unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
	);
	let get_name = level_bind!(
		module,
		b"il2cpp_method_get_name\0",
		unsafe extern "C" fn(Pointer) -> *const c_char
	);
	let get_count = level_bind!(
		module,
		b"il2cpp_method_get_param_count\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	while !class.is_null() {
		if !method(class, name.as_ptr().cast(), count).is_null() {
			let mut iterator: Pointer = core::ptr::null_mut();
			loop {
				let value: Pointer = methods(class, &mut iterator);
				if value.is_null() {
					break;
				}
				if c_name_matches(get_name(value), name) && get_count(value) == count as u32 {
					return Ok(class);
				}
			}
		}
		class = parent(class);
	}
	Err("service_inherited_method_missing")
}

unsafe fn load_services(
	api: &Api,
	domain: Pointer,
	core_image: Pointer,
) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let common_assembly: Pointer = (api.assembly)(domain, b"Torappu.Common\0".as_ptr().cast());
	let game_assembly: Pointer = (api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast());
	if common_assembly.is_null() || game_assembly.is_null() {
		return Err("service_assembly_missing");
	}
	let common: Pointer = (api.image)(common_assembly);
	let game: Pointer = (api.image)(game_assembly);
	let initialize_class = level_bind!(
		module,
		b"il2cpp_runtime_class_init\0",
		unsafe extern "C" fn(Pointer)
	);
	for name in [
		b"CameraController\0".as_slice(),
		b"ArtCollection\0".as_slice(),
	] {
		let class: Pointer = api.class_named(game, b"Torappu.Battle\0", name)?;
		initialize_class(class);
		let reflected: Pointer = reflected_class(module, class)?;
		let _reflection_root: Rooted = api.root(reflected)?;
	}
	let resources: Pointer = api.class_named(core_image, b"UnityEngine\0", b"Resources\0")?;
	record("original_services_load_start");
	let script_path: Pointer = managed_literal(module, b"arkbootstrap/scripts\0")?;
	let scripts: Pointer = typed_call(
		api,
		resources,
		core::ptr::null_mut(),
		b"LoadAll\0",
		&[b"System.String\0"],
		&mut [script_path],
	)?;
	let _scripts_root: Rooted = api.root(scripts)?;
	let array_length = level_bind!(
		module,
		b"il2cpp_array_length\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	scene_number("original_builtin_scripts_loaded", array_length(scripts));
	for (key, namespace, name, image) in [
		(
			b"arkbootstrap/GlobalOptions\0".as_slice(),
			b"Torappu\0".as_slice(),
			b"GlobalOptions\0".as_slice(),
			common,
		),
		(
			b"arkbootstrap/ResourceOptions\0".as_slice(),
			b"Torappu.Resource\0".as_slice(),
			b"ResourceOptions\0".as_slice(),
			common,
		),
		(
			b"arkbootstrap/DBOptions\0".as_slice(),
			b"Torappu.DB\0".as_slice(),
			b"DBOptions\0".as_slice(),
			game,
		),
		(
			b"arkbootstrap/GradingOptions\0".as_slice(),
			b"Torappu.Grading\0".as_slice(),
			b"GradingOptions\0".as_slice(),
			game,
		),
		(
			b"arkbootstrap/AudioOptions\0".as_slice(),
			b"Torappu.Audio\0".as_slice(),
			b"AudioOptions\0".as_slice(),
			game,
		),
		(
			b"arkbootstrap/BasicUIOptions\0".as_slice(),
			b"Torappu.UI\0".as_slice(),
			b"BasicUIOptions\0".as_slice(),
			game,
		),
		(
			b"arkbootstrap/LuaOptions\0".as_slice(),
			b"Torappu.Lua\0".as_slice(),
			b"LuaOptions\0".as_slice(),
			game,
		),
	] {
		let path: Pointer = managed_literal(module, key)?;
		let config: Pointer = typed_call(
			api,
			resources,
			core::ptr::null_mut(),
			b"Load\0",
			&[b"System.String\0"],
			&mut [path],
		)?;
		let _root: Rooted = api.root(config)?;
		let expected: Pointer = api.class_named(image, namespace, name)?;
		if class_of(config) != expected {
			return Err("service_config_class_mismatch");
		}
		let owner: Pointer = method_owner(module, expected, b"get_instance\0", 0)?;
		if api.call(owner, core::ptr::null_mut(), b"get_instance\0", &mut [])? != config {
			return Err("service_config_singleton_mismatch");
		}
		record("original_service_config_bound");
	}
	let local: Pointer =
		api.class_named(game, b"Torappu.Resource.Local\0", b"LocalResourceManager\0")?;
	let owner: Pointer = method_owner(module, local, b"get_instance\0", 0)?;
	let manager: Pointer = api.call(owner, core::ptr::null_mut(), b"get_instance\0", &mut [])?;
	let _manager_root: Rooted = api.root(manager)?;
	let router: Pointer = api.class_named(common, b"Torappu.Resource\0", b"ResourceManager\0")?;
	typed_call(
		api,
		router,
		core::ptr::null_mut(),
		b"ResourceManagerEntryOnly_SetImplementation\0",
		&[b"Torappu.Resource.IResourceManager\0"],
		&mut [manager],
	)?;
	api.call(local, manager, b"InitIfNot\0", &mut [])?;
	if !managed_bool(api, local, manager, b"get_inited\0")? {
		return Err("service_resource_manager_not_initialized");
	}
	let font_root: Pointer = managed_literal(module, b"arkbootstrap/fonts\0")?;
	let fonts: Pointer = typed_call(
		api,
		resources,
		core::ptr::null_mut(),
		b"LoadAll\0",
		&[b"System.String\0"],
		&mut [font_root],
	)?;
	let _fonts_root: Rooted = api.root(fonts)?;
	let count_array = level_bind!(
		module,
		b"il2cpp_array_length\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	let core_class: Pointer = (api.image)((api.assembly)(domain, b"mscorlib\0".as_ptr().cast()));
	let array_class: Pointer = api.class_named(core_class, b"System\0", b"Array\0")?;
	let string_class: Pointer = api.class_named(core_class, b"System\0", b"String\0")?;
	let object_class: Pointer = api.class_named(core_image, b"UnityEngine\0", b"Object\0")?;
	let registry: Pointer = api.class_named(game, b"Torappu.UI\0", b"AbFontResourceRegistry\0")?;
	let font_prefix: Pointer = managed_literal(module, b"arkbootstrap/fonts/\0")?;
	api.call(registry, core::ptr::null_mut(), b"_EnsureInited\0", &mut [])?;
	for index in 0..count_array(fonts) {
		let font: Pointer = array_item(api, array_class, fonts, index as i32)?;
		let name: Pointer = api.call(object_class, font, b"get_name\0", &mut [])?;
		let path: Pointer = typed_call(
			api,
			string_class,
			core::ptr::null_mut(),
			b"Concat\0",
			&[b"System.String\0", b"System.String\0"],
			&mut [font_prefix, name],
		)?;
		typed_call(
			api,
			registry,
			core::ptr::null_mut(),
			b"Register\0",
			&[b"System.String\0", b"System.String\0"],
			&mut [name, path],
		)?;
		managed_ascii(module, name, "original_font_registered")?;
	}
	scene_number("original_fonts_registered", count_array(fonts));
	record("original_resource_manager_bound");
	let mscorlib: Pointer = (api.assembly)(domain, b"mscorlib\0".as_ptr().cast());
	let file: Pointer = api.class_named((api.image)(mscorlib), b"System.IO\0", b"File\0")?;
	let list_path: Pointer = environment_string(module, b"ARK_RUNTIME_SERVICE_TABLES")?;
	let text: Pointer = api.call(
		file,
		core::ptr::null_mut(),
		b"ReadAllText\0",
		&mut [list_path],
	)?;
	let _text_root: Rooted = api.root(text)?;
	let chars = level_bind!(
		module,
		b"il2cpp_string_chars\0",
		unsafe extern "C" fn(Pointer) -> *const u16
	);
	let length = level_bind!(
		module,
		b"il2cpp_string_length\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let new_string = level_bind!(
		module,
		b"il2cpp_string_new_utf16\0",
		unsafe extern "C" fn(*const u16, i32) -> Pointer
	);
	let loader: Pointer = api.class_named(game, b"Torappu.DB\0", b"DBLoader\0")?;
	let abstract_table: Pointer = api.class_named(game, b"Torappu.DB\0", b"AbstractTable\0")?;
	let mut count: u32 = 0;
	let mut start: i32 = 0;
	for index in 0..length(text) {
		if chars(text).offset(index as isize).read() != 10 {
			continue;
		}
		let end: i32 = if index > start && chars(text).offset((index - 1) as isize).read() == 13 {
			index - 1
		} else {
			index
		};
		let mut separator: i32 = start;
		while separator < end && chars(text).offset(separator as isize).read() != 124 {
			separator += 1;
		}
		if separator == end {
			return Err("service_table_descriptor_invalid");
		}
		let path: Pointer = new_string(chars(text).offset(start as isize), separator - start);
		let _path_root: Rooted = api.root(path)?;
		managed_ascii(module, path, "original_service_resource")?;
		let mut table: Pointer = typed_call(
			api,
			resources,
			core::ptr::null_mut(),
			b"Load\0",
			&[b"System.String\0"],
			&mut [path],
		)?;
		if table.is_null() {
			// Native deserialization of a builtin script can fail while its actual runtime
			// class is retained. Restore only its claimed serialized configuration shape.
			let mut name: [u8; 64] = [0; 64];
			let name_length: i32 = separator - start - 13;
			if name_length <= 0 || name_length >= 64 {
				return Err("service_class_name_invalid");
			}
			for i in 0..name_length {
				name.as_mut_ptr()
					.offset(i as isize)
					.write(chars(text).offset((start + 13 + i) as isize).read() as u8);
			}
			let class: Pointer = api.class_named(game, b"Torappu\0", &name)?;
			let reflected: Pointer = reflected_class(module, class)?;
			let scriptable: Pointer =
				api.class_named(core_image, b"UnityEngine\0", b"ScriptableObject\0")?;
			table = typed_call(
				api,
				scriptable,
				core::ptr::null_mut(),
				b"CreateInstance\0",
				&[b"System.Type\0"],
				&mut [reflected],
			)?;
			let _created_root: Rooted = api.root(table)?;
			if class_of(table) != class {
				return Err("service_created_table_class_mismatch");
			}
			record("original_table_created_from_retained_class");
			let config_path: Pointer = new_string(
				chars(text).offset((separator + 1) as isize),
				end - separator - 1,
			);
			let config_json: Pointer = api.call(
				file,
				core::ptr::null_mut(),
				b"ReadAllText\0",
				&mut [config_path],
			)?;
			let _json_root: Rooted = api.root(config_json)?;
			let json_assembly: Pointer =
				(api.assembly)(domain, b"UnityEngine.JSONSerializeModule\0".as_ptr().cast());
			if json_assembly.is_null() {
				return Err("service_json_assembly_missing");
			}
			let json: Pointer = api.class_named(
				(api.image)(json_assembly),
				b"UnityEngine\0",
				b"JsonUtility\0",
			)?;
			typed_call(
				api,
				json,
				core::ptr::null_mut(),
				b"FromJsonOverwrite\0",
				&[b"System.String\0", b"System.Object\0"],
				&mut [config_json, table],
			)?;
			record("original_table_configuration_restored");
		}
		let _table_root: Rooted = api.root(table)?;
		let class: Pointer = class_of(table);
		let config: Pointer = api.call(abstract_table, table, b"GetConfig\0", &mut [])?;
		let config_class: Pointer = api.class_named(game, b"Torappu.DB\0", b"TableConfig\0")?;
		managed_ascii(
			module,
			read_reference(module, config_class, config, b"assetPath\0")?,
			"original_table_asset_path",
		)?;
		let runtime_path: Pointer =
			api.call(abstract_table, table, b"_GenerateRuntimeResPath\0", &mut [])?;
		managed_ascii(module, runtime_path, "original_table_runtime_path")?;
		api.call(loader, core::ptr::null_mut(), b"LoadTable\0", &mut [table])?;
		let inited_owner: Pointer = method_owner(module, class, b"get_inited\0", 0)?;
		if !managed_bool(api, inited_owner, table, b"get_inited\0")? {
			return Err("service_table_not_initialized");
		}
		let validate_owner: Pointer = method_owner(module, class, b"Validate\0", 0)?;
		if !managed_bool(api, validate_owner, table, b"Validate\0")? {
			return Err("service_table_not_valid");
		}
		record("original_service_table_validated");
		if environment_flag(b"ARK_RUNTIME_EXPORT_IDENTITIES") {
			// Operator, stage, module and trust tables, serialized by the original tables.
			for (name, output, stage) in [
				(
					b"CharacterDB\0".as_slice(),
					b"ARK_RUNTIME_CHARACTER_OUTPUT".as_slice(),
					"original_character_identity_exported",
				),
				(
					b"StageDB\0".as_slice(),
					b"ARK_RUNTIME_STAGE_OUTPUT".as_slice(),
					"original_stage_identity_exported",
				),
				(
					b"UniEquipDB\0".as_slice(),
					b"ARK_RUNTIME_MODULE_OUTPUT".as_slice(),
					"original_module_table_exported",
				),
				(
					b"FavorDB\0".as_slice(),
					b"ARK_RUNTIME_FAVOR_OUTPUT".as_slice(),
					"original_favor_table_exported",
				),
			] {
				if class != api.class_named(game, b"Torappu\0", name)? {
					continue;
				}
				let owner: Pointer = method_owner(module, class, b"SerializeToString\0", 1)?;
				let mut intended: u8 = 0;
				let json: Pointer = api.call(
					owner,
					table,
					b"SerializeToString\0",
					&mut [(&mut intended as *mut u8).cast()],
				)?;
				let _json_root: Rooted = api.root(json)?;
				write_original_json(api, module, json, output)?;
				record(stage);
			}
		}
		count += 1;
		scene_number("original_service_table_initialized", count);
		start = index + 1;
	}
	if count == 0 || start != length(text) {
		return Err("service_table_list_incomplete");
	}
	let updates: Pointer = api.class_named(game, b"Torappu.Resource\0", b"HotUpdateMgr\0")?;
	let mut force: u8 = 0;
	api.call(
		updates,
		core::ptr::null_mut(),
		b"Init\0",
		&mut [(&mut force as *mut u8).cast()],
	)?;
	record("original_local_resource_status_initialized");
	record("original_services_complete");
	Ok(())
}

unsafe fn static_reference(
	module: Pointer,
	class: Pointer,
	name: &[u8],
) -> Result<Pointer, &'static str> {
	let class_init = level_bind!(
		module,
		b"il2cpp_runtime_class_init\0",
		unsafe extern "C" fn(Pointer)
	);
	let field = level_bind!(
		module,
		b"il2cpp_class_get_field_from_name\0",
		unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
	);
	let static_get = level_bind!(
		module,
		b"il2cpp_field_static_get_value\0",
		unsafe extern "C" fn(Pointer, Pointer)
	);
	class_init(class);
	let handle: Pointer = field(class, name.as_ptr().cast());
	if handle.is_null() {
		return Err("static_field_missing");
	}
	let mut value: Pointer = core::ptr::null_mut();
	static_get(handle, (&mut value as *mut Pointer).cast());
	Ok(value)
}

// The client's code hotfixes: the original LuaManager creates the xLua environment and runs the
// entry script through the original decrypter and resource routes; the entry registers every
// defined hotfix. The Scheduler fix's bridge field proves registration.
unsafe fn load_original_lua(api: &Api, domain: Pointer) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let manager: Pointer = api.class_named(game, b"Torappu.Lua\0", b"LuaManager\0")?;
	api.call(manager, core::ptr::null_mut(), b"InitIfNot\0", &mut [])?;
	record("original_lua_environment_initialized");
	let scheduler: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Scheduler\0")?;
	let bridge: &[u8] = b"__Hotfix0_CheckActionEnabled\0";
	if static_reference(module, scheduler, bridge)?.is_null() {
		return Err("original_lua_hotfix_not_registered");
	}
	record("original_lua_hotfixes_registered");
	Ok(())
}

static RUN_CONTROLLER: AtomicU32 = AtomicU32::new(0);
static RUN_LEVEL: AtomicU32 = AtomicU32::new(0);
static RUN_FRAME: AtomicU32 = AtomicU32::new(0);
static RUN_TICKS: AtomicU32 = AtomicU32::new(0);
static RUN_SPAWNED: AtomicU32 = AtomicU32::new(0);
static RUN_DEPLOYED: AtomicU32 = AtomicU32::new(0);
static RUN_DAMAGED: AtomicU32 = AtomicU32::new(0);
static RUN_MOVED: AtomicU32 = AtomicU32::new(0);
static FIRST_ENEMY_ID: AtomicI32 = AtomicI32::new(0);
static FIRST_ENEMY_ROW: AtomicI32 = AtomicI32::new(i32::MIN);
static FIRST_ENEMY_COL: AtomicI32 = AtomicI32::new(i32::MIN);

struct OwnedStackObserver(Pointer);
impl Drop for OwnedStackObserver {
	fn drop(&mut self) {
		if !self.0.is_null() {
			unsafe {
				RemoveVectoredExceptionHandler(self.0);
			}
		}
	}
}

unsafe fn number_result(api: &Api, value: Pointer) -> Result<u32, &'static str> {
	if value.is_null() {
		return Err("battle_numeric_result_null");
	}
	Ok((api.unbox)(value).cast::<u32>().read())
}

unsafe fn battle_tick_limit() -> Result<u32, &'static str> {
	if !environment_flag(b"ARK_RUNTIME_BATTLE_UNTIL_FINISHED") {
		return Ok(900);
	}
	let mut name: [u16; 32] = [0; 32];
	let key: &[u8] = b"ARK_RUNTIME_BATTLE_MAX_TICKS";
	for index in 0..key.len() {
		name.as_mut_ptr()
			.add(index)
			.write(key.as_ptr().add(index).read() as u16);
	}
	let mut value: [u16; 16] = [0; 16];
	let length: u32 = GetEnvironmentVariableW(name.as_ptr(), value.as_mut_ptr(), 16);
	if length == 0 || length > 5 {
		return Err("battle_tick_limit_missing_or_invalid");
	}
	let mut limit: u32 = 0;
	for index in 0..length as usize {
		let digit: u16 = value.as_ptr().add(index).read();
		if !(48..=57).contains(&digit) {
			return Err("battle_tick_limit_invalid_digit");
		}
		limit = limit * 10 + (digit - 48) as u32;
	}
	if limit == 0 || limit > 54000 {
		return Err("battle_tick_limit_out_of_bounds");
	}
	Ok(limit)
}

unsafe fn battle_terminal_state(
	api: &Api,
	game: Pointer,
	controller: Pointer,
) -> Result<(bool, bool, i32), &'static str> {
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let finished: bool = managed_bool(api, class, controller, b"get_isFinished\0")?;
	let playing: bool = managed_bool(api, class, controller, b"get_isPlaying\0")?;
	let result: i32 = api.integer(class, controller, b"get_gameResult\0")?;
	let mut side: i32 = 0;
	let life: u32 = number_result(
		api,
		typed_call(
			api,
			class,
			controller,
			b"GetLifePoint\0",
			&[b"Torappu.PlayerSide\0"],
			&mut [(&mut side as *mut i32).cast()],
		)?,
	)?;
	scene_number(
		"original_battle_requested_ticks",
		RUN_TICKS.load(Ordering::Acquire),
	);
	scene_number("original_battle_is_finished", finished as u32);
	scene_number("original_battle_is_playing", playing as u32);
	scene_number("original_battle_game_result", result as u32);
	scene_number("original_battle_life_point", life);
	Ok((finished, playing, result))
}

unsafe fn release_battle_roots(api: &Api) {
	for handle in [
		RUN_CONTROLLER.swap(0, Ordering::AcqRel),
		RUN_FRAME.swap(0, Ordering::AcqRel),
		RUN_LEVEL.swap(0, Ordering::AcqRel),
	] {
		let _root: Rooted = Rooted {
			handle,
			free: api.gc_free,
		};
	}
}

// Per-tick trace of each deployed operator's original state: every parameterless boolean or
// integer getter of its skill (most derived class first) and of the Character class, recorded
// only when a value changes. It shows which original state a skill button follows.
const TRACE_UNITS: usize = 16;
const TRACE_GETTERS: usize = 256;
const TRACE_UNSET: i32 = i32::MIN;
const TRACE_FAILED: i32 = i32::MIN + 1;
static SKILL_TRACE: AtomicU32 = AtomicU32::new(0);
static mut TRACE_IDS: [i32; TRACE_UNITS] = [0; TRACE_UNITS];
static mut TRACE_VALUES: [[i32; TRACE_GETTERS]; TRACE_UNITS] =
	[[TRACE_UNSET; TRACE_GETTERS]; TRACE_UNITS];

unsafe fn skill_trace_enabled() -> bool {
	match SKILL_TRACE.load(Ordering::Acquire) {
		0 => {
			let enabled: bool = environment_flag(b"ARK_RUNTIME_SKILL_TRACE");
			SKILL_TRACE.store(if enabled { 2 } else { 1 }, Ordering::Release);
			enabled
		}
		state => state == 2,
	}
}

unsafe fn trace_value(unit: i32, owner: &str, name: *const c_char, value: i32) {
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"original_unit_trace\",\"tick\":");
	line.number(RUN_TICKS.load(Ordering::Acquire));
	line.text(",\"unit\":");
	line.number(unit as u32);
	line.text(",\"owner\":\"");
	line.text(owner);
	line.text("\",\"name\":\"");
	let bytes: *const u8 = name.cast();
	let mut offset: usize = 0;
	while offset < 96 {
		let byte: u8 = bytes.add(offset).read();
		if byte == 0 {
			break;
		}
		if (32..=126).contains(&byte) && byte != 34 && byte != 92 {
			line.bytes.as_mut_ptr().add(line.length).write(byte);
			line.length += 1;
		}
		offset += 1;
	}
	line.text("\",\"value\":");
	if value == TRACE_FAILED {
		line.text("null");
	} else {
		if value < 0 {
			line.text("-");
		}
		line.number(value.unsigned_abs());
	}
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

unsafe fn trace_units(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let parent = level_bind!(
		module,
		b"il2cpp_class_get_parent\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let methods = level_bind!(
		module,
		b"il2cpp_class_get_methods\0",
		unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
	);
	let get_name = level_bind!(
		module,
		b"il2cpp_method_get_name\0",
		unsafe extern "C" fn(Pointer) -> *const c_char
	);
	let get_count = level_bind!(
		module,
		b"il2cpp_method_get_param_count\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	let get_flags = level_bind!(
		module,
		b"il2cpp_method_get_flags\0",
		unsafe extern "C" fn(Pointer, *mut u32) -> u32
	);
	let return_type = level_bind!(
		module,
		b"il2cpp_method_get_return_type\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let type_kind = level_bind!(
		module,
		b"il2cpp_type_get_type\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let get_parameter = level_bind!(
		module,
		b"il2cpp_method_get_param\0",
		unsafe extern "C" fn(Pointer, u32) -> Pointer
	);
	let type_name = level_bind!(
		module,
		b"il2cpp_type_get_name\0",
		unsafe extern "C" fn(Pointer) -> *const c_char
	);
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let manager: Pointer = api.call(class, controller, b"get_unitManager\0", &mut [])?;
	let manager_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"UnitManager\0")?;
	let units: Pointer = api.call(manager_class, manager, b"get_characters\0", &mut [])?;
	let units_class: Pointer = class_of(units);
	let count: u32 = number_result(api, api.call(units_class, units, b"get_count\0", &mut [])?)?;
	let character_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Character\0")?;
	let ids: *mut i32 = core::ptr::addr_of_mut!(TRACE_IDS).cast::<i32>();
	for offset in 0..count.min(64) {
		let mut index: i32 = offset as i32;
		let unit: Pointer = api.call(
			units_class,
			units,
			b"get_Item\0",
			&mut [(&mut index as *mut i32).cast()],
		)?;
		if unit.is_null() {
			continue;
		}
		let instance: i32 = api.integer(class_of(unit), unit, b"GetInstanceID\0")?;
		let Some(slot) = (0..TRACE_UNITS)
			.find(|slot| *ids.add(*slot) == instance)
			.or_else(|| (0..TRACE_UNITS).find(|slot| *ids.add(*slot) == 0))
		else {
			continue;
		};
		*ids.add(slot) = instance;
		let values: *mut i32 = core::ptr::addr_of_mut!(TRACE_VALUES)
			.cast::<[i32; TRACE_GETTERS]>()
			.add(slot)
			.cast::<i32>();
		let skill: Pointer = api
			.call(character_class, unit, b"get_skill\0", &mut [])
			.unwrap_or(core::ptr::null_mut());
		let mut getter: usize = 0;
		// The skill's class chain, then the Character class alone.
		for (owner, object, mut current, chain) in [
			("skill", skill, if skill.is_null() { skill } else { class_of(skill) }, true),
			("character", unit, character_class, false),
		] {
			while !current.is_null() && !object.is_null() {
				let mut iterator: Pointer = core::ptr::null_mut();
				loop {
					let method: Pointer = methods(current, &mut iterator);
					if method.is_null() || getter >= TRACE_GETTERS - 1 {
						break;
					}
					let name: *const c_char = get_name(method);
					if *values.add(TRACE_GETTERS - 1) == TRACE_UNSET {
						trace_value(instance, "method", name, get_count(method) as i32);
						for parameter in 0..get_count(method).min(4) {
							trace_value(
								instance,
								"parameter",
								type_name(get_parameter(method, parameter)),
								parameter as i32,
							);
						}
					}
					let prefix: *const u8 = name.cast();
					let getter_name: bool = prefix.read() == b'g'
						&& prefix.add(1).read() == b'e'
						&& prefix.add(2).read() == b't'
						&& prefix.add(3).read() == b'_';
					let predicate_name: bool =
						(prefix.read() == b'I' || prefix.read() == b'i') && prefix.add(1).read() == b's';
					if get_count(method) != 0 || !(getter_name || predicate_name) {
						continue;
					}
					let mut implementation: u32 = 0;
					// METHOD_ATTRIBUTE_STATIC
					if get_flags(method, &mut implementation) & 0x10 != 0 {
						continue;
					}
					// IL2CPP_TYPE_BOOLEAN or IL2CPP_TYPE_I4
					let kind: i32 = type_kind(return_type(method));
					if kind != 0x02 && kind != 0x08 {
						continue;
					}
					let resolved: Pointer = (api.virtual_method)(object, method);
					let mut exception: Pointer = core::ptr::null_mut();
					let boxed: Pointer = (api.invoke)(
						if resolved.is_null() { method } else { resolved },
						object,
						core::ptr::null_mut(),
						&mut exception,
					);
					let value: i32 = if !exception.is_null() || boxed.is_null() {
						TRACE_FAILED
					} else {
						let data: Pointer = (api.unbox)(boxed);
						if kind == 0x02 {
							data.cast::<u8>().read() as i32
						} else {
							data.cast::<i32>().read()
						}
					};
					if *values.add(getter) != value {
						*values.add(getter) = value;
						trace_value(instance, owner, name, value);
					}
					getter += 1;
				}
				current = if chain { parent(current) } else { core::ptr::null_mut() };
			}
		}
		// Method names are listed once per unit.
		*values.add(TRACE_GETTERS - 1) = 0;
	}
	Ok(())
}

unsafe fn battle_snapshot(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let manager: Pointer = api.call(class, controller, b"get_unitManager\0", &mut [])?;
	let manager_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"UnitManager\0")?;
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	for (getter, stage) in [
		(b"get_enemies\0".as_slice(), "original_battle_enemy_count"),
		(
			b"get_characters\0".as_slice(),
			"original_battle_character_count",
		),
	] {
		let units: Pointer = api.call(manager_class, manager, getter, &mut [])?;
		let units_class: Pointer = class_of(units);
		let count: u32 =
			number_result(api, api.call(units_class, units, b"get_count\0", &mut [])?)?;
		scene_number(stage, count);
		if c_name_matches(getter.as_ptr().cast(), b"get_characters\0") {
			RUN_DEPLOYED.fetch_max(count, Ordering::AcqRel);
		}
		if count > 64 {
			return Err("battle_snapshot_unit_bound_exceeded");
		}
		for offset in 0..count {
			let enemy: bool = c_name_matches(getter.as_ptr().cast(), b"get_enemies\0");
			scene_number(
				if enemy {
					"original_enemy_snapshot_index"
				} else {
					"original_character_snapshot_index"
				},
				offset,
			);
			let mut index: i32 = offset as i32;
			let unit: Pointer = api.call(
				units_class,
				units,
				b"get_Item\0",
				&mut [(&mut index as *mut i32).cast()],
			)?;
			let visual: Pointer = api.class_named(game, b"Torappu.Battle\0", b"VisualObject\0")?;
			let instance: i32 = api.integer(class_of(unit), unit, b"GetInstanceID\0")?;
			scene_number(
				if enemy {
					"original_enemy_instance_id"
				} else {
					"original_character_instance_id"
				},
				instance as u32,
			);
			let pos: Pointer = api.call(visual, unit, b"get_gridPosition\0", &mut [])?;
			if enemy
				&& (FIRST_ENEMY_ID.load(Ordering::Acquire) == 0
					|| FIRST_ENEMY_ID.load(Ordering::Acquire) == instance)
			{
				let data: *const i32 = (api.unbox)(pos).cast();
				let row: i32 = data.read();
				let col: i32 = data.add(1).read();
				let first_row: i32 = FIRST_ENEMY_ROW.load(Ordering::Acquire);
				if first_row == i32::MIN {
					FIRST_ENEMY_ID.store(instance, Ordering::Release);
					FIRST_ENEMY_ROW.store(row, Ordering::Release);
					FIRST_ENEMY_COL.store(col, Ordering::Release);
				} else if first_row != row || FIRST_ENEMY_COL.load(Ordering::Acquire) != col {
					RUN_MOVED.store(1, Ordering::Release);
				}
			}
			let grid: Pointer = api.class_named(game, b"Torappu\0", b"GridPosition\0")?;
			let text: Pointer = api.call(grid, (api.unbox)(pos), b"ToString\0", &mut [])?;
			managed_ascii(
				module,
				text,
				if c_name_matches(getter.as_ptr().cast(), b"get_enemies\0") {
					"original_enemy_grid"
				} else {
					"original_character_grid"
				},
			)?;
			let entity: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Entity\0")?;
			let hp: Pointer = api.call(entity, unit, b"get_hp\0", &mut [])?;
			let fp: Pointer = class_of(hp);
			if enemy {
				let maximum: Pointer = api.call(entity, unit, b"get_maxHp\0", &mut [])?;
				let less: Pointer = api.call(
					fp,
					core::ptr::null_mut(),
					b"op_LessThan\0",
					&mut [(api.unbox)(hp), (api.unbox)(maximum)],
				)?;
				if less.is_null() {
					return Err("battle_hp_comparison_null");
				}
				if (api.unbox)(less).cast::<u8>().read() != 0 {
					RUN_DAMAGED.store(1, Ordering::Release);
				}
			}
			let text: Pointer = api.call(fp, (api.unbox)(hp), b"ToString\0", &mut [])?;
			managed_ascii(
				module,
				text,
				if c_name_matches(getter.as_ptr().cast(), b"get_enemies\0") {
					"original_enemy_hp"
				} else {
					"original_character_hp"
				},
			)?;
		}
	}
	let scheduler: Pointer = api.call(class, controller, b"get_scheduler\0", &mut [])?;
	let scheduler_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Scheduler\0")?;
	for (getter, stage) in [
		(
			b"get_spawnedEnemiesCnt\0".as_slice(),
			"original_battle_spawned_enemies",
		),
		(
			b"get_killedEnemiesCnt\0".as_slice(),
			"original_battle_killed_enemies",
		),
	] {
		let count: u32 =
			number_result(api, api.call(scheduler_class, scheduler, getter, &mut [])?)?;
		scene_number(stage, count);
		if c_name_matches(getter.as_ptr().cast(), b"get_spawnedEnemiesCnt\0") {
			RUN_SPAWNED.fetch_max(count, Ordering::AcqRel);
		}
	}
	let mut side: i32 = 0;
	let cost: u32 = number_result(
		api,
		typed_call(
			api,
			class,
			controller,
			b"GetCost\0",
			&[b"Torappu.PlayerSide\0"],
			&mut [(&mut side as *mut i32).cast()],
		)?,
	)?;
	scene_number("original_battle_dp", cost);
	Ok(())
}

unsafe fn load_original_avg(api: &Api, domain: Pointer) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let core: Pointer = (api.image)((api.assembly)(
		domain,
		b"UnityEngine.CoreModule\0".as_ptr().cast(),
	));
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let resources: Pointer = api.class_named(core, b"UnityEngine\0", b"Resources\0")?;
	let game_object: Pointer = api.class_named(core, b"UnityEngine\0", b"GameObject\0")?;
	let video_class: Pointer = api.class_named(game, b"Torappu.Video\0", b"VideoManager\0")?;
	let video_object: Pointer = (api.new_object)(game_object);
	let _video_object_root: Rooted = api.root(video_object)?;
	api.call(game_object, video_object, b".ctor\0", &mut [])?;
	let video_type: Pointer = reflected_class(module, video_class)?;
	let video: Pointer = typed_call(
		api,
		game_object,
		video_object,
		b"AddComponent\0",
		&[b"System.Type\0"],
		&mut [video_type],
	)?;
	let video_owner: Pointer = method_owner(module, video_class, b"get_instance\0", 0)?;
	if api.call(
		video_owner,
		core::ptr::null_mut(),
		b"get_instance\0",
		&mut [],
	)? != video
	{
		return Err("original_video_service_binding_mismatch");
	}
	record("original_video_service_bound");
	let path: Pointer = managed_literal(module, b"arkbootstrap/AVGControllerPrefab\0")?;
	let prefab: Pointer = typed_call(
		api,
		resources,
		core::ptr::null_mut(),
		b"Load\0",
		&[b"System.String\0"],
		&mut [path],
	)?;
	let _prefab_root: Rooted = api.root(prefab)?;
	record("original_avg_prefab_loaded");
	let owner: Pointer = (api.new_object)(game_object);
	let _owner_root: Rooted = api.root(owner)?;
	api.call(game_object, owner, b".ctor\0", &mut [])?;
	let mut active: u8 = 0;
	api.call(
		game_object,
		owner,
		b"SetActive\0",
		&mut [(&mut active as *mut u8).cast()],
	)?;
	let avg_class: Pointer = api.class_named(game, b"Torappu.AVG\0", b"AVG\0")?;
	let reflected: Pointer = reflected_class(module, avg_class)?;
	let avg: Pointer = typed_call(
		api,
		game_object,
		owner,
		b"AddComponent\0",
		&[b"System.Type\0"],
		&mut [reflected],
	)?;
	let _avg_root: Rooted = api.root(avg)?;
	let field = level_bind!(
		module,
		b"il2cpp_class_get_field_from_name\0",
		unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
	);
	let set = level_bind!(
		module,
		b"il2cpp_field_set_value_object\0",
		unsafe extern "C" fn(Pointer, Pointer, Pointer)
	);
	let target: Pointer = field(avg_class, b"_prefab\0".as_ptr().cast());
	if target.is_null() {
		return Err("original_avg_prefab_field_missing");
	}
	set(avg, target, prefab);
	record("original_avg_source_configuration_applied");
	active = 1;
	api.call(
		game_object,
		owner,
		b"SetActive\0",
		&mut [(&mut active as *mut u8).cast()],
	)?;
	let avg_owner: Pointer = method_owner(module, avg_class, b"get_instance\0", 0)?;
	if api.call(avg_owner, core::ptr::null_mut(), b"get_instance\0", &mut [])? != avg {
		return Err("original_avg_service_binding_mismatch");
	}
	record("original_avg_service_bound");
	let class: Pointer = api.class_named(game, b"Torappu.AVG\0", b"AVGController\0")?;
	let owner: Pointer = method_owner(module, class, b"get_instance\0", 0)?;
	let controller: Pointer = api.call(owner, core::ptr::null_mut(), b"get_instance\0", &mut [])?;
	let _controller_root: Rooted = api.root(controller)?;
	record("original_avg_controller_bound");
	Ok(())
}

unsafe fn preload_camera_assets(
	api: &Api,
	domain: Pointer,
	bundle_class: Pointer,
	bundle: Pointer,
) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"CameraController\0")?;
	let reflected: Pointer = reflected_class(module, class)?;
	let _type_root: Rooted = api.root(reflected)?;
	let core: Pointer = (api.image)((api.assembly)(
		domain,
		b"UnityEngine.CoreModule\0".as_ptr().cast(),
	));
	let game_object: Pointer = api.class_named(core, b"UnityEngine\0", b"GameObject\0")?;
	for key in [
		b"dyn/battle/[pack]common/prefabs/legacycameracontroller.prefab\0".as_slice(),
		b"dyn/battle/[pack]common/prefabs/cameracontroller.prefab\0".as_slice(),
	] {
		let path: Pointer = managed_literal(module, key)?;
		let _path_root: Rooted = api.root(path)?;
		managed_ascii(module, path, "original_camera_archive_path")?;
		let asset: Pointer = typed_call(
			api,
			bundle_class,
			bundle,
			b"LoadAsset\0",
			&[b"System.String\0"],
			&mut [path],
		)?;
		let _asset_root: Rooted = api.root(asset)?;
		let component: Pointer = typed_call(
			api,
			game_object,
			asset,
			b"GetComponent\0",
			&[b"System.Type\0"],
			&mut [reflected],
		)?;
		scene_number(
			"original_camera_archive_component_present",
			(!component.is_null()) as u32,
		);
	}
	Ok(())
}

#[link(name = "kernel32")]
extern "system" {
	fn AddVectoredExceptionHandler(
		first: u32,
		handler: unsafe extern "system" fn(*mut Pointer) -> i32,
	) -> Pointer;
	fn RemoveVectoredExceptionHandler(handler: Pointer) -> u32;
	fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut Pointer, hash: *mut u32)
		-> u16;
}

// A bounded, read-only stack observer for exceptions in this owned Player. It always
// continues normal exception handling and never changes game code or exception state.
unsafe extern "system" fn owned_exception_stack(info: *mut Pointer) -> i32 {
	if info.is_null() {
		return 0;
	}
	let exception: Pointer = info.read();
	if exception.is_null() || exception.cast::<u32>().read() != 0xe06d7363 {
		return 0;
	}
	let base: usize = GetModuleHandleW(ASSEMBLY.as_ptr()) as usize;
	let mut frames: [Pointer; 32] = [core::ptr::null_mut(); 32];
	let count: u16 = RtlCaptureStackBackTrace(0, 32, frames.as_mut_ptr(), core::ptr::null_mut());
	record("owned_cpp_exception_stack_start");
	for index in 0..count as usize {
		let address: usize = frames.as_ptr().add(index).read() as usize;
		if address >= base && address - base < 214586344 {
			scene_number("owned_cpp_exception_game_rva", (address - base) as u32);
		}
	}
	0
}

unsafe fn nested_class(
	module: Pointer,
	class: Pointer,
	expected: &[u8],
) -> Result<Pointer, &'static str> {
	let nested = level_bind!(
		module,
		b"il2cpp_class_get_nested_types\0",
		unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
	);
	let name = level_bind!(
		module,
		b"il2cpp_class_get_name\0",
		unsafe extern "C" fn(Pointer) -> *const c_char
	);
	let mut iterator: Pointer = core::ptr::null_mut();
	loop {
		let item: Pointer = nested(class, &mut iterator);
		if item.is_null() {
			return Err("battle_nested_class_missing");
		}
		if c_name_matches(name(item), expected) {
			return Ok(item);
		}
	}
}

unsafe fn custom_squad(
	api: &Api,
	module: Pointer,
	game: Pointer,
) -> Result<(Pointer, Rooted), &'static str> {
	let domain: Pointer = (api.domain)();
	let core: Pointer = (api.image)((api.assembly)(domain, b"mscorlib\0".as_ptr().cast()));
	let file: Pointer = api.class_named(core, b"System.IO\0", b"File\0")?;
	let path: Pointer = environment_string(module, b"ARK_RUNTIME_SQUAD_INPUT")?;
	let _path_root: Rooted = api.root(path)?;
	let text: Pointer = api.call(file, core::ptr::null_mut(), b"ReadAllText\0", &mut [path])?;
	let _text_root: Rooted = api.root(text)?;
	let chars = level_bind!(
		module,
		b"il2cpp_string_chars\0",
		unsafe extern "C" fn(Pointer) -> *const u16
	);
	let length = level_bind!(
		module,
		b"il2cpp_string_length\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let new_string = level_bind!(
		module,
		b"il2cpp_string_new_utf16\0",
		unsafe extern "C" fn(*const u16, i32) -> Pointer
	);
	let new_array = level_bind!(
		module,
		b"il2cpp_array_new\0",
		unsafe extern "C" fn(Pointer, usize) -> Pointer
	);
	let size: i32 = length(text);
	if size <= 0 || size > 24000 {
		return Err("custom_squad_size_invalid");
	}
	let data: *const u16 = chars(text);
	let mut count: i32 = 0;
	for index in 0..size {
		if data.offset(index as isize).read() == 10 {
			count += 1;
		}
	}
	if !(1..=12).contains(&count) || data.offset((size - 1) as isize).read() != 10 {
		return Err("custom_squad_count_invalid");
	}
	let slot_class: Pointer = api.class_named(game, b"Torappu\0", b"AdvancedCharacterInst\0")?;
	let slot_type: Pointer = reflected_class(module, slot_class)?;
	let _type_root: Rooted = api.root(slot_type)?;
	let slots: Pointer = new_array(slot_class, count as usize);
	let slots_root: Rooted = api.root(slots)?;
	let array_class: Pointer = api.class_named(core, b"System\0", b"Array\0")?;
	let json_image: Pointer = (api.image)((api.assembly)(
		domain,
		b"UnityEngine.JSONSerializeModule\0".as_ptr().cast(),
	));
	let utility: Pointer = api.class_named(json_image, b"UnityEngine\0", b"JsonUtility\0")?;
	let character: Pointer = api.class_named(game, b"Torappu\0", b"CharacterInst\0")?;
	let mut start: i32 = 0;
	let mut index: i32 = 0;
	for end in 0..size {
		if data.offset(end as isize).read() != 10 {
			continue;
		}
		if end <= start {
			return Err("custom_squad_empty_slot");
		}
		let json: Pointer = new_string(data.offset(start as isize), end - start);
		let _json_root: Rooted = api.root(json)?;
		let slot: Pointer = typed_call(
			api,
			utility,
			core::ptr::null_mut(),
			b"FromJson\0",
			&[b"System.String\0", b"System.Type\0"],
			&mut [json, slot_type],
		)?;
		let _slot_root: Rooted = api.root(slot)?;
		if !managed_bool(api, character, slot, b"get_isValid\0")? {
			return Err("original_custom_squad_slot_invalid");
		}
		typed_call(
			api,
			array_class,
			slots,
			b"SetValue\0",
			&[b"System.Object\0", b"System.Int32\0"],
			&mut [slot, (&mut index as *mut i32).cast()],
		)?;
		index += 1;
		start = end + 1;
	}
	scene_number("original_custom_squad_slot_count", index as u32);
	Ok((slots, slots_root))
}

unsafe fn export_battle_squad(
	api: &Api,
	module: Pointer,
	game: Pointer,
	player: Pointer,
	class: Pointer,
) -> Result<(), &'static str> {
	let serializer: Pointer = api.class_named(game, b"Torappu\0", b"HGUnityJsonSerializer\0")?;
	let declared: Pointer = reflected_class(module, class)?;
	let _declared_root: Rooted = api.root(declared)?;
	let json: Pointer = api.call(
		serializer,
		core::ptr::null_mut(),
		b"SerializeToJson\0",
		&mut [player, declared],
	)?;
	let _json_root: Rooted = api.root(json)?;
	write_original_json(api, module, json, b"ARK_RUNTIME_SQUAD_OUTPUT")?;
	record("original_battle_squad_exported");
	Ok(())
}

unsafe fn write_original_json(
	api: &Api,
	module: Pointer,
	json: Pointer,
	variable: &[u8],
) -> Result<(), &'static str> {
	let path: Pointer = environment_string(module, variable)?;
	let _path_root: Rooted = api.root(path)?;
	let core: Pointer = (api.image)((api.assembly)(
		(api.domain)(),
		b"mscorlib\0".as_ptr().cast(),
	));
	let file: Pointer = api.class_named(core, b"System.IO\0", b"File\0")?;
	let encoding: Pointer = api.class_named(core, b"System.Text\0", b"Encoding\0")?;
	let utf8: Pointer = api.call(encoding, core::ptr::null_mut(), b"get_UTF8\0", &mut [])?;
	let _utf8_root: Rooted = api.root(utf8)?;
	let bytes: Pointer = typed_call(
		api,
		encoding,
		utf8,
		b"GetBytes\0",
		&[b"System.String\0"],
		&mut [json],
	)?;
	let _bytes_root: Rooted = api.root(bytes)?;
	typed_call(
		api,
		file,
		core::ptr::null_mut(),
		b"WriteAllBytes\0",
		&[b"System.String\0", b"System.Byte[]\0"],
		&mut [path, bytes],
	)?;
	Ok(())
}

unsafe fn create_battle_input(
	api: &Api,
	module: Pointer,
	game: Pointer,
	level: Pointer,
) -> Result<(), &'static str> {
	let field = level_bind!(
		module,
		b"il2cpp_class_get_field_from_name\0",
		unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
	);
	let field_type = level_bind!(
		module,
		b"il2cpp_field_get_type\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let class_from_type = level_bind!(
		module,
		b"il2cpp_class_from_type\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let set_object = level_bind!(
		module,
		b"il2cpp_field_set_value_object\0",
		unsafe extern "C" fn(Pointer, Pointer, Pointer)
	);
	let set_value = level_bind!(
		module,
		b"il2cpp_field_set_value\0",
		unsafe extern "C" fn(Pointer, Pointer, Pointer)
	);
	let inout: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleInOut\0")?;
	let input_class: Pointer = nested_class(module, inout, b"InParams\0")?;
	let boxed: Pointer = (api.new_object)(input_class);
	let _input_root: Rooted = api.root(boxed)?;
	let list_field: Pointer = field(input_class, b"playerDataList\0".as_ptr().cast());
	if list_field.is_null() {
		return Err("battle_input_list_field_missing");
	}
	let list_class: Pointer = class_from_type(field_type(list_field));
	let list: Pointer = (api.new_object)(list_class);
	let _list_root: Rooted = api.root(list)?;
	api.call(list_class, list, b".ctor\0", &mut [])?;
	let player_class: Pointer =
		api.class_named(game, b"Torappu.Battle\0", b"BattlePlayerData\0")?;
	let converter: Pointer =
		api.class_named(game, b"Torappu.Battle\0", b"BattleDataConverter\0")?;
	let mut difficulty: i32 = 1;
	let mut side: i32 = 0;
	let custom: bool = environment_flag(b"ARK_RUNTIME_CUSTOM_SQUAD");
	let squad: Option<(Pointer, Rooted)> = if custom {
		Some(custom_squad(api, module, game)?)
	} else {
		None
	};
	let slots: Pointer = squad
		.as_ref()
		.map_or(core::ptr::null_mut(), |value| value.0);
	let mut memory: u8 = (!custom) as u8;
	let mut reset: u8 = 1;
	let mut selectable: u8 = 0;
	let player: Pointer = api.call(
		converter,
		core::ptr::null_mut(),
		b"ConvertToBattlePlayerData\0",
		&mut [
			slots,
			level,
			(&mut difficulty as *mut i32).cast(),
			core::ptr::null_mut(),
			(&mut memory as *mut u8).cast(),
			(&mut side as *mut i32).cast(),
			(&mut reset as *mut u8).cast(),
			(&mut selectable as *mut u8).cast(),
		],
	)?;
	let _player_root: Rooted = api.root(player)?;
	export_battle_squad(api, module, game, player, player_class)?;
	record(if custom {
		"original_custom_squad_converted"
	} else {
		"original_level_predefined_squad_converted"
	});
	// Original PlayerSide.DEFAULT; the normal battle's VoicePlayer reads GetDeck(DEFAULT).
	// The scene's SIDE_A/SIDE_B fields belong to its separate editor test-player setup.
	let side_field: Pointer = field(player_class, b"playerSide\0".as_ptr().cast());
	if side_field.is_null() {
		return Err("battle_input_side_field_missing");
	}
	set_value(player, side_field, (&mut side as *mut i32).cast());
	api.call(list_class, list, b"Add\0", &mut [player])?;
	for (name, value) in [
		(b"levelData\0".as_slice(), level),
		(b"playerDataList\0".as_slice(), list),
		(
			b"battleId\0".as_slice(),
			managed_literal(module, b"ark-independent-battle\0")?,
		),
	] {
		let item: Pointer = field(input_class, name.as_ptr().cast());
		if item.is_null() {
			return Err("battle_input_reference_field_missing");
		}
		set_object(boxed, item, value);
	}
	let mut difficulty: i32 = 1; // Original LevelData.Difficulty.NORMAL.
	let item: Pointer = field(input_class, b"difficulty\0".as_ptr().cast());
	if item.is_null() {
		return Err("battle_input_difficulty_field_missing");
	}
	set_value(boxed, item, (&mut difficulty as *mut i32).cast());
	// Supply the original nullable mode before BattleLauncher.Start consumes InParams.
	let mode_field: Pointer = field(input_class, b"gameModeMeta\0".as_ptr().cast());
	if mode_field.is_null() {
		return Err("battle_input_mode_field_missing");
	}
	let nullable_class: Pointer = class_from_type(field_type(mode_field));
	let nullable: Pointer = (api.new_object)(nullable_class);
	let _nullable_root: Rooted = api.root(nullable)?;
	let meta: Pointer = create_manual_meta(api, module, game)?;
	let _meta_root: Rooted = api.root(meta)?;
	api.call(
		nullable_class,
		(api.unbox)(nullable),
		b".ctor\0",
		&mut [(api.unbox)(meta)],
	)?;
	set_value(boxed, mode_field, (api.unbox)(nullable));
	let singleton_owner: Pointer = method_owner(module, inout, b"get_instance\0", 0)?;
	let holder: Pointer = api.call(
		singleton_owner,
		core::ptr::null_mut(),
		b"get_instance\0",
		&mut [],
	)?;
	let _holder_root: Rooted = api.root(holder)?;
	api.call(inout, holder, b"SetInParams\0", &mut [(api.unbox)(boxed)])?;
	if !managed_bool(api, inout, holder, b"get_hasInput\0")? {
		return Err("battle_input_not_bound");
	}
	record("original_battle_input_bound");
	Ok(())
}

unsafe fn prepare_battle_input(api: &Api, domain: Pointer) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let utils: Pointer = api.class_named(game, b"Torappu.Battle\0", b"LevelUtils\0")?;
	let level_id: Pointer = environment_string(module, b"ARK_RUNTIME_BATTLE_LEVEL_ID")?;
	managed_ascii(module, level_id, "original_battle_level_id")?;
	record("original_battle_level_load_start");
	let level: Pointer = api.call(
		utils,
		core::ptr::null_mut(),
		b"LoadLevel\0",
		&mut [level_id],
	)?;
	let level_root: Rooted = api.root(level)?;
	record("original_battle_level_loaded");
	create_battle_input(api, module, game, level)?;
	RUN_LEVEL.store(level_root.handle, Ordering::Release);
	core::mem::forget(level_root);
	Ok(())
}

unsafe fn start_battle(
	api: &Api,
	domain: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	load_probe_actions(api, module, domain)?;
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	if !managed_bool(api, class, controller, b"get_haveGameLoaded\0")? {
		return Err("original_scene_launcher_did_not_load_battle");
	}
	record("original_battle_load_game_complete");
	let deterministic: bool =
		managed_bool(api, class, core::ptr::null_mut(), b"get_isDeterministic\0")?;
	scene_number("original_battle_deterministic", deterministic as u32);
	let presented: bool = environment_flag(b"ARK_RUNTIME_PRESENTATION");
	if !deterministic && !presented {
		return Err("battle_original_deterministic_mode_not_active");
	}
	let common: Pointer = (api.image)((api.assembly)(domain, b"Torappu.Common\0".as_ptr().cast()));
	let facade: Pointer = api.class_named(common, b"Torappu\0", b"UnityCrossVersionFacade\0")?;
	let physics_auto: bool = managed_bool(
		api,
		facade,
		core::ptr::null_mut(),
		b"get_Physics2DAutoSimulation\0",
	)?;
	scene_number("original_battle_physics_auto", physics_auto as u32);
	if physics_auto && !presented {
		return Err("battle_original_manual_physics_not_active");
	}
	initialize_probe_seed(api, module, game, controller)?;
	if environment_flag(b"ARK_RUNTIME_PRESENTATION") {
		install_tick_forwarder(api, class)?;
	}
	// A presented battle is started by the original launcher, as in the client; starting it
	// here as well would run the wave timeline twice.
	if !presented && !managed_bool(api, class, controller, b"get_isPlaying\0")? {
		api.call(class, controller, b"StartGame\0", &mut [])?;
	}
	if !presented && !managed_bool(api, class, controller, b"get_isPlaying\0")? {
		return Err("battle_game_not_playing");
	}
	record("original_battle_started");
	let root: Rooted = api.root(controller)?;
	RUN_CONTROLLER.store(root.handle, Ordering::Release);
	core::mem::forget(root);
	let nested = level_bind!(
		module,
		b"il2cpp_class_get_nested_types\0",
		unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
	);
	let name = level_bind!(
		module,
		b"il2cpp_class_get_name\0",
		unsafe extern "C" fn(Pointer) -> *const c_char
	);
	let mut iterator: Pointer = core::ptr::null_mut();
	let frame_class: Pointer = loop {
		let item: Pointer = nested(class, &mut iterator);
		if item.is_null() {
			return Err("battle_frame_class_missing");
		}
		if c_name_matches(name(item), b"FrameData\0") {
			break item;
		}
	};
	let frame: Pointer = (api.new_object)(frame_class);
	let root: Rooted = api.root(frame)?;
	api.call(frame_class, frame, b".ctor\0", &mut [])?;
	RUN_FRAME.store(root.handle, Ordering::Release);
	core::mem::forget(root);
	Ok(())
}

unsafe fn battle_preloads_ready(api: &Api, domain: Pointer) -> Result<bool, &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let target = level_bind!(
		module,
		b"il2cpp_gchandle_get_target\0",
		unsafe extern "C" fn(u32) -> Pointer
	);
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let controller: Pointer = target(RUN_CONTROLLER.load(Ordering::Acquire));
	let pools: Pointer = read_reference(module, class, controller, b"m_paddingPools\0")?;
	if pools.is_null() {
		return Err("battle_original_pool_queue_missing");
	}
	let count: i32 = api.integer(class_of(pools), pools, b"get_Count\0")?;
	scene_number("original_battle_pending_pools", count as u32);
	if count < 0 {
		return Err("battle_original_pool_queue_count_invalid");
	}
	if count == 0 {
		record("original_battle_preloads_complete");
	}
	Ok(count == 0)
}

unsafe fn progress_battle(api: &Api, domain: Pointer) -> Result<bool, &'static str> {
	scene_number(
		"original_battle_batch_enter",
		RUN_TICKS.load(Ordering::Acquire),
	);
	let _observer: OwnedStackObserver =
		OwnedStackObserver(AddVectoredExceptionHandler(1, owned_exception_stack));
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let target = level_bind!(
		module,
		b"il2cpp_gchandle_get_target\0",
		unsafe extern "C" fn(u32) -> Pointer
	);
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let controller: Pointer = target(RUN_CONTROLLER.load(Ordering::Acquire));
	let frame: Pointer = target(RUN_FRAME.load(Ordering::Acquire));
	if controller.is_null() || frame.is_null() {
		return Err("battle_tick_roots_missing");
	}
	let mut additional: u8 = 0;
	let limit: u32 = battle_tick_limit()?;
	for _ in 0..30 {
		if RUN_TICKS.load(Ordering::Acquire) >= limit
			|| managed_bool(api, class, controller, b"get_isFinished\0")?
		{
			break;
		}
		let ticks: u32 = RUN_TICKS.load(Ordering::Acquire);
		plan_in_flight(api, module, domain, game, controller, frame, ticks)?;
		api.call(
			class,
			controller,
			b"ManualFrameTick\0",
			&mut [frame, (&mut additional as *mut u8).cast()],
		)?;
		after_tick(api, module, game, controller, ticks)?;
	}
	observe_batch(api, module, game, class, controller, limit)
}

// After one original tick: count it, confirm in-flight effects, record events.
unsafe fn after_tick(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	ticks: u32,
) -> Result<(), &'static str> {
	RUN_TICKS.fetch_add(1, Ordering::AcqRel);
	settle_in_flight(api, module, game, controller, ticks)?;
	observe_events(api, module, game, controller)?;
	if skill_trace_enabled() {
		trace_units(api, module, game, controller)?;
	}
	Ok(())
}

// Snapshot and terminal checks after a batch of ticks; true once the run is complete.
unsafe fn observe_batch(
	api: &Api,
	module: Pointer,
	game: Pointer,
	class: Pointer,
	controller: Pointer,
	limit: u32,
) -> Result<bool, &'static str> {
	let boxed: Pointer = api.call(
		class,
		core::ptr::null_mut(),
		b"get_fixedFrameCnt\0",
		&mut [],
	)?;
	let _root: Rooted = api.root(boxed)?;
	let frames: u32 = (api.unbox)(boxed).cast::<u32>().read();
	scene_number("original_battle_fixed_frames", frames);
	// Only the probe advances the battle; the original loop must not have ticked it.
	if frames != RUN_TICKS.load(Ordering::Acquire) {
		return Err("original_loop_advanced_battle");
	}
	battle_snapshot(api, module, game, controller)?;
	let (finished, playing, result): (bool, bool, i32) =
		battle_terminal_state(api, game, controller)?;
	if finished {
		let index: u32 = ORDER_CURSOR.load(Ordering::Acquire);
		let count: u32 = ACTION_COUNT.load(Ordering::Acquire);
		// SkillDaemon is the final command; it completes when the battle ends naturally. The job
		// is complete only once every issued command's effect was also confirmed.
		if index + 1 == count
			&& core::ptr::addr_of!(PROBE_ACTIONS)
				.cast::<ProbeAction>()
				.add(index as usize)
				.read()
				.kind == KIND_SKILL_DAEMON
		{
			record("original_maa_skill_daemon_complete");
			complete_command();
		}
		if NEXT_ACTION.load(Ordering::Acquire) == count {
			record("original_maa_job_complete");
		} else {
			record("original_maa_job_incomplete_at_finish");
		}
	}
	record("original_battle_batch_observed");
	let until_finished: bool = environment_flag(b"ARK_RUNTIME_BATTLE_UNTIL_FINISHED");
	if !finished && RUN_TICKS.load(Ordering::Acquire) < limit {
		return Ok(false);
	}
	if until_finished {
		if finished {
			if playing || !matches!(result, 1 | 2) {
				return Err("battle_terminal_state_inconsistent");
			}
			record("original_battle_natural_finish_complete");
		} else {
			record("original_battle_tick_budget_exhausted");
		}
		if RUN_DEPLOYED.load(Ordering::Acquire) >= 2
			&& RUN_SPAWNED.load(Ordering::Acquire) > 0
			&& RUN_MOVED.load(Ordering::Acquire) != 0
			&& RUN_DAMAGED.load(Ordering::Acquire) != 0
		{
			record("original_enemy_movement_verified");
			record("original_enemy_damage_verified");
			record("original_battle_ticks_complete");
		}
		release_battle_roots(api);
		return Ok(true);
	}
	if frames < 900 {
		return Err("battle_frames_not_advanced");
	}
	if RUN_DEPLOYED.load(Ordering::Acquire) < 2 {
		return Err("battle_deployments_not_observed");
	}
	if RUN_SPAWNED.load(Ordering::Acquire) == 0 {
		return Err("battle_enemy_spawn_not_observed");
	}
	if RUN_MOVED.load(Ordering::Acquire) == 0 {
		return Err("battle_enemy_movement_not_observed");
	}
	if RUN_DAMAGED.load(Ordering::Acquire) == 0 {
		return Err("battle_enemy_damage_not_observed");
	}
	record("original_enemy_movement_verified");
	record("original_enemy_damage_verified");
	release_battle_roots(api);
	record("original_battle_ticks_complete");
	Ok(true)
}

// Original-loop driving. The original FixedUpdate invokes BattleController.OnTick through its game
// mode; OnTick's method pointer is replaced before the first FixedUpdate builds that delegate, so
// each original tick passes through this forwarder.
static ON_TICK_ORIGINAL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
// Set once preloads are ready and the presented speed is applied; ticks before it are held.
static LOOP_READY: AtomicU32 = AtomicU32::new(0);
// Set once the run is complete or failed; later ticks pass straight to the original.
static LOOP_DONE: AtomicU32 = AtomicU32::new(0);
static QUEUE_OFFSET: AtomicU32 = AtomicU32::new(0);
// OnTick's MethodInfo, and whether its virtualMethodPointer was replaced as well.
static ON_TICK_METHOD: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static ON_TICK_VIRTUAL: AtomicU32 = AtomicU32::new(0);

unsafe fn install_tick_forwarder(api: &Api, class: Pointer) -> Result<(), &'static str> {
	let method: Pointer = (api.method)(class, b"OnTick\0".as_ptr().cast(), 0);
	if method.is_null() {
		return Err("original_on_tick_missing");
	}
	// MethodInfo begins with methodPointer and virtualMethodPointer.
	let pointers: *mut usize = method.cast::<usize>();
	let code: usize = pointers.read();
	ON_TICK_ORIGINAL.store(code, Ordering::Release);
	ON_TICK_METHOD.store(method as usize, Ordering::Release);
	let forwarder: usize = tick_forwarder as *const () as usize;
	pointers.write(forwarder);
	if pointers.add(1).read() == code {
		pointers.add(1).write(forwarder);
		ON_TICK_VIRTUAL.store(1, Ordering::Release);
	}
	record("original_loop_tick_forwarder_installed");
	Ok(())
}

unsafe fn set_speed_level(api: &Api, level: u32) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let target = level_bind!(
		module,
		b"il2cpp_gchandle_get_target\0",
		unsafe extern "C" fn(u32) -> Pointer
	);
	let game: Pointer = (api.image)((api.assembly)((api.domain)(), b"Assembly-CSharp\0".as_ptr().cast()));
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let controller: Pointer = target(RUN_CONTROLLER.load(Ordering::Acquire));
	let mut wanted: i32 = if level == 2 { 2 } else { 1 };
	api.call(
		class,
		controller,
		b"set_speedLevel\0",
		&mut [(&mut wanted as *mut i32).cast()],
	)?;
	if api.integer(class, controller, b"get_speedLevel\0")? != wanted {
		return Err("presented_speed_not_applied");
	}
	scene_number("original_battle_presented_speed", wanted as u32);
	Ok(())
}

// Run the original OnTick through il2cpp_runtime_invoke, so a managed exception it throws is
// caught here, recorded, and the battle continues, as Unity does for an exception in
// FixedUpdate. A managed exception must never unwind through this forwarder. The method's
// pointers are restored for the call, so the invoke reaches the original code.
unsafe fn original_tick(api: &Api, controller: Pointer) {
	let method: Pointer = ON_TICK_METHOD.load(Ordering::Acquire) as Pointer;
	let pointers: *mut usize = method.cast::<usize>();
	let code: usize = ON_TICK_ORIGINAL.load(Ordering::Acquire);
	let forwarder: usize = tick_forwarder as *const () as usize;
	let replaced_virtual: bool = ON_TICK_VIRTUAL.load(Ordering::Acquire) != 0;
	pointers.write(code);
	if replaced_virtual {
		pointers.add(1).write(code);
	}
	let mut exception: Pointer = core::ptr::null_mut();
	(api.invoke)(method, controller, core::ptr::null_mut(), &mut exception);
	pointers.write(forwarder);
	if replaced_virtual {
		pointers.add(1).write(forwarder);
	}
	if !exception.is_null() {
		exception_log(exception);
		scene_number("original_tick_exception", RUN_TICKS.load(Ordering::Acquire));
	}
}

unsafe extern "C" fn tick_forwarder(controller: Pointer, _method: Pointer) {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let Ok(api) = Api::load(module) else {
		record("original_loop_api_missing");
		return;
	};
	// Before preloads are ready the battle holds, as the client does while loading.
	if LOOP_READY.load(Ordering::Acquire) == 0 {
		return;
	}
	if LOOP_DONE.load(Ordering::Acquire) != 0 {
		original_tick(&api, controller);
		return;
	}
	if let Err(reason) = looped_tick(&api, controller) {
		record(reason);
		record("engine_probe_failed");
		LOOP_DONE.store(1, Ordering::Release);
	}
}

// The controller's PlayerOperationQueue field, found by its type so no layout is assumed.
unsafe fn operation_queue(module: Pointer, class: Pointer, controller: Pointer) -> Result<Pointer, &'static str> {
	if QUEUE_OFFSET.load(Ordering::Acquire) == 0 {
		let fields = level_bind!(
			module,
			b"il2cpp_class_get_fields\0",
			unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
		);
		let field_type = level_bind!(
			module,
			b"il2cpp_field_get_type\0",
			unsafe extern "C" fn(Pointer) -> Pointer
		);
		let field_offset = level_bind!(
			module,
			b"il2cpp_field_get_offset\0",
			unsafe extern "C" fn(Pointer) -> usize
		);
		let type_name = level_bind!(
			module,
			b"il2cpp_type_get_name\0",
			unsafe extern "C" fn(Pointer) -> *const c_char
		);
		let mut iterator: Pointer = core::ptr::null_mut();
		loop {
			let field: Pointer = fields(class, &mut iterator);
			if field.is_null() {
				return Err("original_operation_queue_field_missing");
			}
			let name: *const u8 = type_name(field_type(field)).cast();
			// The type name ends with "PlayerOperationQueue".
			// Volatile reads keep this loop from becoming a C runtime strlen call.
			let mut length: usize = 0;
			while length < 256 && core::ptr::read_volatile(name.add(length)) != 0 {
				length += 1;
			}
			let suffix: &[u8] = b"PlayerOperationQueue";
			if length >= suffix.len()
				&& (0..suffix.len())
					.all(|index| name.add(length - suffix.len() + index).read() == suffix[index])
			{
				QUEUE_OFFSET.store(field_offset(field) as u32, Ordering::Release);
				break;
			}
		}
	}
	let queue: Pointer = controller
		.cast::<u8>()
		.add(QUEUE_OFFSET.load(Ordering::Acquire) as usize)
		.cast::<Pointer>()
		.read();
	if queue.is_null() {
		return Err("original_operation_queue_null");
	}
	Ok(queue)
}

// One original tick in the presented battle: issue due operations into the original player
// operation queue (as ManualFrameTick does, and as player input does), run the original tick,
// then confirm effects and observe, as the manual driver does.
unsafe fn looped_tick(api: &Api, controller: Pointer) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let domain: Pointer = (api.domain)();
	let target = level_bind!(
		module,
		b"il2cpp_gchandle_get_target\0",
		unsafe extern "C" fn(u32) -> Pointer
	);
	let game: Pointer = (api.image)((api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast()));
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let frame: Pointer = target(RUN_FRAME.load(Ordering::Acquire));
	if frame.is_null() {
		return Err("battle_tick_roots_missing");
	}
	let limit: u32 = battle_tick_limit()?;
	let ticks: u32 = RUN_TICKS.load(Ordering::Acquire);
	if ticks % 30 == 0 {
		scene_number("original_battle_batch_enter", ticks);
	}
	plan_in_flight(api, module, domain, game, controller, frame, ticks)?;
	let frame_class: Pointer = nested_class(module, class, b"FrameData\0")?;
	let queue: Pointer = operation_queue(module, class, controller)?;
	api.call(frame_class, frame, b"DumpToPlayerOpQueue\0", &mut [queue])?;
	original_tick(api, controller);
	after_tick(api, module, game, controller, ticks)?;
	let finished: bool = managed_bool(api, class, controller, b"get_isFinished\0")?;
	if finished || RUN_TICKS.load(Ordering::Acquire) % 30 == 0 || RUN_TICKS.load(Ordering::Acquire) >= limit {
		if observe_batch(api, module, game, class, controller, limit)? {
			LOOP_DONE.store(1, Ordering::Release);
		}
	}
	Ok(())
}

// A finished presented battle, observed from the probe callback: true once observed complete.
unsafe fn observe_loop_finish(api: &Api, game: Pointer, class: Pointer) -> Result<bool, &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let target = level_bind!(
		module,
		b"il2cpp_gchandle_get_target\0",
		unsafe extern "C" fn(u32) -> Pointer
	);
	let controller: Pointer = target(RUN_CONTROLLER.load(Ordering::Acquire));
	if controller.is_null() || !managed_bool(api, class, controller, b"get_isFinished\0")? {
		return Ok(false);
	}
	observe_batch(api, module, game, class, controller, battle_tick_limit()?)
}
