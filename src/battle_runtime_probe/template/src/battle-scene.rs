// Load the pinned archive closure; scene activation is a separate explicit gate.
static SCENE_OPERATION: AtomicU32 = AtomicU32::new(0);
static SCENE_BASE_COUNT: AtomicI32 = AtomicI32::new(0);
unsafe fn scene_number(stage: &str, value: u32) {
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"");
	line.text(stage);
	line.text("\",\"value\":");
	line.number(value);
	line.text(",\"thread_id\":");
	line.number(GetCurrentThreadId());
	line.text(",\"elapsed_ms\":");
	line.number(elapsed_ms());
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

unsafe fn scene_string(module: Pointer, value: Pointer) -> Result<(), &'static str> {
	managed_ascii(module, value, "original_scene_path")
}
unsafe fn managed_ascii(module: Pointer, value: Pointer, stage: &str) -> Result<(), &'static str> {
	let length_fn = level_bind!(
		module,
		b"il2cpp_string_length\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let chars_fn = level_bind!(
		module,
		b"il2cpp_string_chars\0",
		unsafe extern "C" fn(Pointer) -> *const u16
	);
	let length: i32 = length_fn(value);
	if length <= 0 || length > 256 {
		return Err("scene_path_length_invalid");
	}
	let chars: *const u16 = chars_fn(value);
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"");
	line.text(stage);
	line.text("\",\"path\":\"");
	for index in 0..length as usize {
		let character: u16 = chars.add(index).read();
		if !(32..=126).contains(&character) || character == 34 || character == 92 {
			return Err("scene_path_not_plain_ascii");
		}
		line.bytes
			.as_mut_ptr()
			.add(line.length)
			.write(character as u8);
		line.length += 1;
	}
	line.text("\"}\n");
	let mut written: u32 = 0;
	if WriteFile(
		LOG.load(Ordering::Acquire) as Pointer,
		line.bytes.as_ptr(),
		line.length as u32,
		&mut written,
		core::ptr::null_mut(),
	) == 0 || written != line.length as u32
	{
		return Err("scene_path_log_failed");
	}
	Ok(())
}

unsafe fn array_item(
	api: &Api,
	array_class: Pointer,
	array: Pointer,
	index: i32,
) -> Result<Pointer, &'static str> {
	let mut actual_index: i32 = index;
	typed_call(
		api,
		array_class,
		array,
		b"GetValue\0",
		&[b"System.Int32\0"],
		&mut [(&mut actual_index as *mut i32).cast()],
	)
}

// Matches the host's scene closure bound.
const MAX_SCENE_BUNDLES: usize = 512;

struct BundleRoots {
	handles: [u32; MAX_SCENE_BUNDLES],
	count: usize,
	free: GcFree,
}
impl Drop for BundleRoots {
	fn drop(&mut self) {
		for index in 0..self.count {
			unsafe { (self.free)(self.handles.as_ptr().add(index).read()) };
		}
	}
}

unsafe fn load_scene_bundles(api: &Api, domain: Pointer) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let assembly: Pointer =
		(api.assembly)(domain, b"UnityEngine.AssetBundleModule\0".as_ptr().cast());
	let core: Pointer = (api.assembly)(domain, b"mscorlib\0".as_ptr().cast());
	if assembly.is_null() || core.is_null() {
		return Err("scene_bundle_assembly_missing");
	}
	let bundle_class: Pointer =
		api.class_named((api.image)(assembly), b"UnityEngine\0", b"AssetBundle\0")?;
	let core_image: Pointer = (api.image)(core);
	let array_class: Pointer = api.class_named(core_image, b"System\0", b"Array\0")?;
	let file_class: Pointer = api.class_named(core_image, b"System.IO\0", b"File\0")?;
	let length_fn = level_bind!(
		module,
		b"il2cpp_array_length\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	let list_path: Pointer = environment_string(module, b"ARK_RUNTIME_SCENE_LIST")?;
	let _list_path_root: Rooted = api.root(list_path)?;
	let paths: Pointer = typed_call(
		api,
		file_class,
		core::ptr::null_mut(),
		b"ReadAllText\0",
		&[b"System.String\0"],
		&mut [list_path],
	)?;
	let _paths_root: Rooted = api.root(paths)?;
	let string_length = level_bind!(
		module,
		b"il2cpp_string_length\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let string_chars = level_bind!(
		module,
		b"il2cpp_string_chars\0",
		unsafe extern "C" fn(Pointer) -> *const u16
	);
	let string_new = level_bind!(
		module,
		b"il2cpp_string_new_utf16\0",
		unsafe extern "C" fn(*const u16, i32) -> Pointer
	);
	let list_length: i32 = string_length(paths);
	if list_length <= 0 || list_length > 256 * 1024 {
		return Err("scene_bundle_list_length_invalid");
	}
	let chars: *const u16 = string_chars(paths);
	let mut count: u32 = 0;
	for offset in 0..list_length as usize {
		if chars.add(offset).read() == 10 {
			count += 1;
		}
	}
	if count == 0 || count > MAX_SCENE_BUNDLES as u32 {
		return Err("scene_bundle_count_invalid");
	}
	scene_number("original_scene_bundle_count", count);
	let mut start: usize = 0;
	let mut bundle_roots: BundleRoots = BundleRoots {
		handles: [0; MAX_SCENE_BUNDLES],
		count: 0,
		free: api.gc_free,
	};
	for index in 0..count {
		scene_number("original_scene_bundle_load_index", index);
		let mut end: usize = start;
		while end < list_length as usize && chars.add(end).read() != 10 {
			end += 1;
		}
		if end == list_length as usize {
			return Err("scene_bundle_list_unterminated");
		}
		let trimmed: usize = if end > start && chars.add(end - 1).read() == 13 {
			end - 1
		} else {
			end
		};
		if trimmed <= start || trimmed - start >= 1024 {
			return Err("scene_bundle_path_length_invalid");
		}
		let path: Pointer = string_new(chars.add(start), (trimmed - start) as i32);
		start = end + 1;
		let _path_root: Rooted = api.root(path)?;
		let bundle: Pointer = typed_call(
			api,
			bundle_class,
			core::ptr::null_mut(),
			b"LoadFromFile\0",
			&[b"System.String\0"],
			&mut [path],
		)?;
		if bundle.is_null() {
			return Err("original_scene_bundle_load_null");
		}
		let bundle_root: Rooted = api.root(bundle)?;
		bundle_roots
			.handles
			.as_mut_ptr()
			.add(bundle_roots.count)
			.write(bundle_root.handle);
		bundle_roots.count += 1;
		core::mem::forget(bundle_root);
		if environment_flag(b"ARK_RUNTIME_RUN_BATTLE") && index + 1 < count {
			let wanted: Pointer = environment_string(module, b"ARK_RUNTIME_CAMERA_BUNDLE_PATH")?;
			let string_class: Pointer = api.class_named(core_image, b"System\0", b"String\0")?;
			let equal: Pointer = typed_call(
				api,
				string_class,
				core::ptr::null_mut(),
				b"Equals\0",
				&[b"System.String\0", b"System.String\0"],
				&mut [path, wanted],
			)?;
			if (api.unbox)(equal).cast::<u8>().read() != 0 {
				preload_camera_assets(api, domain, bundle_class, bundle)?;
			}
		}
		if index + 1 == count {
			if !managed_bool(
				api,
				bundle_class,
				bundle,
				b"get_isStreamedSceneAssetBundle\0",
			)? {
				return Err("original_bundle_not_scene");
			}
			let scene_paths: Pointer =
				api.call(bundle_class, bundle, b"GetAllScenePaths\0", &mut [])?;
			let _scene_paths_root: Rooted = api.root(scene_paths)?;
			if length_fn(scene_paths) != 1 {
				return Err("original_scene_path_count_mismatch");
			}
			let scene_path: Pointer = array_item(api, array_class, scene_paths, 0)?;
			let _scene_path_root: Rooted = api.root(scene_path)?;
			scene_string(module, scene_path)?;
			if environment_flag(b"ARK_RUNTIME_SCENE_LOAD") {
				if environment_flag(b"ARK_RUNTIME_RUN_BATTLE") {
					let target = level_bind!(
						module,
						b"il2cpp_gchandle_get_target\0",
						unsafe extern "C" fn(u32) -> Pointer
					);
					for index in 0..bundle_roots.count - 1 {
						let handle: u32 = bundle_roots.handles.as_ptr().add(index).read();
						let loaded: Pointer = target(handle);
						// Use the original archive loader to resolve script registrations and
						// prefab dependencies before later original Resources lookups.
						let assets: Pointer =
							api.call(bundle_class, loaded, b"LoadAllAssets\0", &mut [])?;
						let _assets_root: Rooted = api.root(assets)?;
						scene_number("original_archive_assets_preloaded", length_fn(assets));
					}
					record("original_dynamic_assets_preloaded");
					load_original_avg(api, domain)?;
					prepare_battle_input(api, domain)?;
				}
				start_original_scene(api, domain, scene_path)?;
			}
		}
	}
	record("original_scene_bundles_complete");
	Ok(())
}

unsafe fn start_original_scene(
	api: &Api,
	domain: Pointer,
	scene_path: Pointer,
) -> Result<(), &'static str> {
	let mut lookup: Pointer = scene_path;
	if environment_flag(b"ARK_RUNTIME_SCENE_NAME") {
		let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
		let length_fn = level_bind!(
			module,
			b"il2cpp_string_length\0",
			unsafe extern "C" fn(Pointer) -> i32
		);
		let chars_fn = level_bind!(
			module,
			b"il2cpp_string_chars\0",
			unsafe extern "C" fn(Pointer) -> *const u16
		);
		let new_fn = level_bind!(
			module,
			b"il2cpp_string_new_utf16\0",
			unsafe extern "C" fn(*const u16, i32) -> Pointer
		);
		let length: i32 = length_fn(scene_path);
		if length <= 6 || length > 256 {
			return Err("scene_lookup_path_length_invalid");
		}
		let chars: *const u16 = chars_fn(scene_path);
		let mut start: usize = 0;
		for index in 0..length as usize {
			if chars.add(index).read() == 47 {
				start = index + 1;
			}
		}
		for (offset, expected) in [46, 117, 110, 105, 116, 121].iter().enumerate() {
			if chars.add(length as usize - 6 + offset).read() != *expected {
				return Err("scene_lookup_extension_mismatch");
			}
		}
		if length as usize - 6 <= start {
			return Err("scene_lookup_name_empty");
		}
		lookup = new_fn(chars.add(start), length - 6 - start as i32);
	}
	let _lookup_root: Rooted = api.root(lookup)?;
	scene_number(
		"original_scene_lookup_by_name",
		environment_flag(b"ARK_RUNTIME_SCENE_NAME") as u32,
	);
	let assembly: Pointer = (api.assembly)(domain, b"UnityEngine.CoreModule\0".as_ptr().cast());
	if assembly.is_null() {
		return Err("scene_core_assembly_missing");
	}
	let image: Pointer = (api.image)(assembly);
	let manager: Pointer =
		api.class_named(image, b"UnityEngine.SceneManagement\0", b"SceneManager\0")?;
	let count: i32 = api.integer(manager, core::ptr::null_mut(), b"get_sceneCount\0")?;
	SCENE_BASE_COUNT.store(count, Ordering::Release);
	scene_number("original_scene_count_before", count as u32);
	let mut mode: i32 = 1; // Original LoadSceneMode.Additive, from this version's metadata.
	record("original_scene_load_start");
	let operation: Pointer = typed_call(
		api,
		manager,
		core::ptr::null_mut(),
		b"LoadSceneAsync\0",
		&[
			b"System.String\0",
			b"UnityEngine.SceneManagement.LoadSceneMode\0",
		],
		&mut [lookup, (&mut mode as *mut i32).cast()],
	)?;
	let root: Rooted = api.root(operation)?;
	SCENE_OPERATION.store(root.handle, Ordering::Release);
	core::mem::forget(root);
	record("original_scene_load_requested");
	Ok(())
}

unsafe fn progress_original_scene(
	api: &Api,
	domain: Pointer,
	image: Pointer,
) -> Result<bool, &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let target = level_bind!(
		module,
		b"il2cpp_gchandle_get_target\0",
		unsafe extern "C" fn(u32) -> Pointer
	);
	let handle: u32 = SCENE_OPERATION.load(Ordering::Acquire);
	if handle == 0 {
		return Err("scene_operation_root_missing");
	}
	let operation: Pointer = target(handle);
	if operation.is_null() {
		return Err("scene_operation_target_null");
	}
	let async_class: Pointer = api.class_named(image, b"UnityEngine\0", b"AsyncOperation\0")?;
	if !managed_bool(api, async_class, operation, b"get_isDone\0")? {
		return Ok(false);
	}
	SCENE_OPERATION.store(0, Ordering::Release);
	let _operation_root: Rooted = Rooted {
		handle,
		free: api.gc_free,
	};
	record("original_scene_load_complete");
	let manager: Pointer =
		api.class_named(image, b"UnityEngine.SceneManagement\0", b"SceneManager\0")?;
	let count: i32 = api.integer(manager, core::ptr::null_mut(), b"get_sceneCount\0")?;
	if count != SCENE_BASE_COUNT.load(Ordering::Acquire) + 1 {
		return Err("scene_count_mismatch");
	}
	scene_number("original_scene_count_after", count as u32);
	let battle_assembly: Pointer = (api.assembly)(domain, b"Assembly-CSharp\0".as_ptr().cast());
	if battle_assembly.is_null() {
		return Err("scene_battle_assembly_missing");
	}
	let battle_image: Pointer = (api.image)(battle_assembly);
	let class: Pointer =
		api.class_named(battle_image, b"Torappu.Battle\0", b"BattleController\0")?;
	let object_class: Pointer = api.class_named(image, b"UnityEngine\0", b"Object\0")?;
	let reflected: Pointer = reflected_class(module, class)?;
	let _type_root: Rooted = api.root(reflected)?;
	let controller: Pointer = typed_call(
		api,
		object_class,
		core::ptr::null_mut(),
		b"FindObjectOfType\0",
		&[b"System.Type\0"],
		&mut [reflected],
	)?;
	let _controller_root: Rooted = api.root(controller)?;
	let class_parent = level_bind!(
		module,
		b"il2cpp_class_get_parent\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	if class_of(controller) != class {
		return Err("scene_controller_type_mismatch");
	}
	let singleton: Pointer = api.call(
		class_parent(class),
		core::ptr::null_mut(),
		b"get_instanceOrNull\0",
		&mut [],
	)?;
	if singleton != controller {
		return Err("scene_controller_singleton_mismatch");
	}
	record("original_scene_controller_bound");
	for (field, type_name) in [
		(b"_map\0".as_slice(), b"Map\0".as_slice()),
		(b"_scheduler\0".as_slice(), b"Scheduler\0".as_slice()),
		(b"_factory\0".as_slice(), b"BattleFactory\0".as_slice()),
	] {
		let value: Pointer = read_reference(module, class, controller, field)?;
		let expected: Pointer = api.class_named(battle_image, b"Torappu.Battle\0", type_name)?;
		if value.is_null() || class_of(value) != expected {
			return Err("scene_framework_reference_mismatch");
		}
	}
	record("original_scene_framework_references_bound");
	let map: Pointer = read_reference(module, class, controller, b"_map\0")?;
	let map_class: Pointer = api.class_named(battle_image, b"Torappu.Battle\0", b"Map\0")?;
	let width: i32 = api.integer(map_class, map, b"get_width\0")?;
	let height: i32 = api.integer(map_class, map, b"get_height\0")?;
	let (expected_width, expected_height): (i32, i32) = if environment_flag(b"ARK_RUNTIME_RUN_BATTLE")
	{
		let target = level_bind!(
			module,
			b"il2cpp_gchandle_get_target\0",
			unsafe extern "C" fn(u32) -> Pointer
		);
		let level: Pointer = target(RUN_LEVEL.load(Ordering::Acquire));
		let level_class: Pointer = api.class_named(battle_image, b"Torappu\0", b"LevelData\0")?;
		let original_map: Pointer = read_reference(module, level_class, level, b"mapData\0")?;
		let original_map_class: Pointer =
			api.class_named(battle_image, b"Torappu\0", b"MapData\0")?;
		(
			api.integer(original_map_class, original_map, b"get_width\0")?,
			api.integer(original_map_class, original_map, b"get_height\0")?,
		)
	} else {
		(7, 6)
	};
	if width != expected_width || height != expected_height {
		return Err("scene_map_dimensions_mismatch");
	}
	scene_number("original_scene_map_width", width as u32);
	scene_number("original_scene_map_height", height as u32);
	if read_reference(module, class, controller, b"m_playerOpQueue\0")?.is_null()
		|| (!environment_flag(b"ARK_RUNTIME_RUN_BATTLE")
			&& managed_bool(api, class, controller, b"get_isPlaying\0")?)
	{
		return Err("scene_controller_initial_state_mismatch");
	}
	record("original_scene_framework_complete");
	if environment_flag(b"ARK_RUNTIME_RUN_BATTLE") {
		start_battle(api, domain, controller)?;
	}
	Ok(true)
}
