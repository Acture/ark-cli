// One pinned level decoded by the original signed FlatBuffer converter.
macro_rules! level_bind {
	($module:expr, $name:literal, $ty:ty) => {{
		let pointer: Pointer = GetProcAddress($module, $name.as_ptr().cast());
		if pointer.is_null() {
			return Err("level_api_export_missing");
		}
		core::mem::transmute::<Pointer, $ty>(pointer)
	}};
}

unsafe fn c_name_matches(pointer: *const c_char, expected: &[u8]) -> bool {
	if pointer.is_null() {
		return false;
	}
	for index in 0..expected.len() {
		if pointer.cast::<u8>().add(index).read() != expected.as_ptr().add(index).read() {
			return false;
		}
	}
	true
}

unsafe fn typed_call(
	api: &Api,
	class: Pointer,
	object: Pointer,
	name: &[u8],
	parameter_types: &[&[u8]],
	args: &mut [Pointer],
) -> Result<Pointer, &'static str> {
	if parameter_types.len() != args.len() {
		return Err("typed_call_argument_count_mismatch");
	}
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let methods = level_bind!(
		module,
		b"il2cpp_class_get_methods\0",
		unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
	);
	let method_name = level_bind!(
		module,
		b"il2cpp_method_get_name\0",
		unsafe extern "C" fn(Pointer) -> *const c_char
	);
	let count = level_bind!(
		module,
		b"il2cpp_method_get_param_count\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	let param = level_bind!(
		module,
		b"il2cpp_method_get_param\0",
		unsafe extern "C" fn(Pointer, u32) -> Pointer
	);
	let type_name = level_bind!(
		module,
		b"il2cpp_type_get_name\0",
		unsafe extern "C" fn(Pointer) -> *mut c_char
	);
	let free = level_bind!(module, b"il2cpp_free\0", unsafe extern "C" fn(Pointer));
	let is_generic = level_bind!(
		module,
		b"il2cpp_method_is_generic\0",
		unsafe extern "C" fn(Pointer) -> bool
	);
	let mut iterator: Pointer = core::ptr::null_mut();
	loop {
		let method: Pointer = methods(class, &mut iterator);
		if method.is_null() {
			return Err("typed_method_missing");
		}
		if is_generic(method)
			|| !c_name_matches(method_name(method), name)
			|| count(method) != args.len() as u32
		{
			continue;
		}
		let mut matches: bool = true;
		for (index, expected) in parameter_types.iter().enumerate() {
			let actual: *mut c_char = type_name(param(method, index as u32));
			let parameter_matches: bool = c_name_matches(actual, expected);
			if !parameter_matches {
				parameter_mismatch(name, index as u32, actual);
			}
			matches &= parameter_matches;
			free(actual.cast());
		}
		if !matches {
			continue;
		}
		let mut exception: Pointer = core::ptr::null_mut();
		let parameters: *mut Pointer = if args.is_empty() {
			core::ptr::null_mut()
		} else {
			args.as_mut_ptr()
		};
		let result: Pointer = (api.invoke)(method, object, parameters, &mut exception);
		if !exception.is_null() {
			exception_log(exception);
			return Err("typed_method_exception");
		}
		return Ok(result);
	}
}

unsafe fn environment_string(module: Pointer, name: &[u8]) -> Result<Pointer, &'static str> {
	if name.len() >= 64 {
		return Err("level_environment_key_too_long");
	}
	let mut key: [u16; 64] = [0; 64];
	for index in 0..name.len() {
		key.as_mut_ptr()
			.add(index)
			.write(name.as_ptr().add(index).read() as u16);
	}
	let mut value: [u16; 1024] = [0; 1024];
	let length: u32 = GetEnvironmentVariableW(key.as_ptr(), value.as_mut_ptr(), 1024);
	if length == 0 || length >= 1024 {
		return Err("level_environment_path_missing_or_too_long");
	}
	let new_string = level_bind!(
		module,
		b"il2cpp_string_new_utf16\0",
		unsafe extern "C" fn(*const u16, i32) -> Pointer
	);
	Ok(new_string(value.as_ptr(), length as i32))
}

unsafe fn reflected_class(module: Pointer, class: Pointer) -> Result<Pointer, &'static str> {
	let class_type = level_bind!(
		module,
		b"il2cpp_class_get_type\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let type_object = level_bind!(
		module,
		b"il2cpp_type_get_object\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	Ok(type_object(class_type(class)))
}

unsafe fn read_reference(
	module: Pointer,
	class: Pointer,
	object: Pointer,
	name: &[u8],
) -> Result<Pointer, &'static str> {
	let field = level_bind!(
		module,
		b"il2cpp_class_get_field_from_name\0",
		unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
	);
	let get = level_bind!(
		module,
		b"il2cpp_field_get_value\0",
		unsafe extern "C" fn(Pointer, Pointer, Pointer)
	);
	let info: Pointer = field(class, name.as_ptr().cast());
	if info.is_null() {
		return Err("level_reference_field_missing");
	}
	let mut value: Pointer = core::ptr::null_mut();
	get(object, info, (&mut value as *mut Pointer).cast());
	Ok(value)
}

unsafe fn decode_level(
	api: &Api,
	domain: Pointer,
	engine_image: Pointer,
	battle_image: Pointer,
) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let common: Pointer = (api.assembly)(domain, b"Torappu.Common\0".as_ptr().cast());
	let core: Pointer = (api.assembly)(domain, b"mscorlib\0".as_ptr().cast());
	if common.is_null() || core.is_null() {
		return Err("level_support_assembly_missing");
	}
	let common_image: Pointer = (api.image)(common);
	let core_image: Pointer = (api.image)(core);
	let file_class: Pointer = api.class_named(core_image, b"System.IO\0", b"File\0")?;
	let options_class: Pointer = api.class_named(common_image, b"Torappu\0", b"GlobalOptions\0")?;
	let scriptable_class: Pointer =
		api.class_named(engine_image, b"UnityEngine\0", b"ScriptableObject\0")?;
	let text_class: Pointer = api.class_named(engine_image, b"UnityEngine\0", b"TextAsset\0")?;
	let object_class: Pointer = api.class_named(engine_image, b"UnityEngine\0", b"Object\0")?;
	let options_type: Pointer = reflected_class(module, options_class)?;
	let _type_root: Rooted = api.root(options_type)?;
	record("original_global_options_create_start");
	let options: Pointer = typed_call(
		api,
		scriptable_class,
		core::ptr::null_mut(),
		b"CreateInstance\0",
		&[b"System.Type\0"],
		&mut [options_type],
	)?;
	let _options_root: Rooted = api.root(options)?;
	let result: Result<(), &'static str> = (|| {
		let key_path: Pointer = environment_string(module, b"ARK_RUNTIME_VERIFICATION_KEY")?;
		let _key_path_root: Rooted = api.root(key_path)?;
		let key_string: Pointer = typed_call(
			api,
			file_class,
			core::ptr::null_mut(),
			b"ReadAllText\0",
			&[b"System.String\0"],
			&mut [key_path],
		)?;
		let _key_string_root: Rooted = api.root(key_string)?;
		let key_asset: Pointer = (api.new_object)(text_class);
		let _key_asset_root: Rooted = api.root(key_asset)?;
		typed_call(
			api,
			text_class,
			key_asset,
			b".ctor\0",
			&[b"System.String\0"],
			&mut [key_string],
		)?;
		let field = level_bind!(
			module,
			b"il2cpp_class_get_field_from_name\0",
			unsafe extern "C" fn(Pointer, *const c_char) -> Pointer
		);
		let set_object = level_bind!(
			module,
			b"il2cpp_field_set_value_object\0",
			unsafe extern "C" fn(Pointer, Pointer, Pointer)
		);
		let key_field: Pointer = field(options_class, b"cryptoPubKey\0".as_ptr().cast());
		if key_field.is_null() {
			return Err("original_verification_key_field_missing");
		}
		set_object(options, key_field, key_asset);
		let class_parent = level_bind!(
			module,
			b"il2cpp_class_get_parent\0",
			unsafe extern "C" fn(Pointer) -> Pointer
		);
		let options_base: Pointer = class_parent(options_class);
		let current: Pointer = api.call(
			options_base,
			core::ptr::null_mut(),
			b"get_instance\0",
			&mut [],
		)?;
		if current != options
			|| read_reference(module, options_class, options, b"cryptoPubKey\0")? != key_asset
		{
			return Err("original_options_binding_mismatch");
		}
		record("original_verification_key_bound");
		let level_path: Pointer = environment_string(module, b"ARK_RUNTIME_LEVEL_INPUT")?;
		let _level_path_root: Rooted = api.root(level_path)?;
		let stream: Pointer = typed_call(
			api,
			file_class,
			core::ptr::null_mut(),
			b"OpenRead\0",
			&[b"System.String\0"],
			&mut [level_path],
		)?;
		let _stream_root: Rooted = api.root(stream)?;
		let factory_class: Pointer =
			api.class_named(battle_image, b"Torappu.DB\0", b"ConverterFactory\0")?;
		let mut converter_type: i32 = 7;
		record("original_level_converter_create_start");
		let converter: Pointer = api.call(
			factory_class,
			core::ptr::null_mut(),
			b"Create\0",
			&mut [(&mut converter_type as *mut i32).cast()],
		)?;
		let _converter_root: Rooted = api.root(converter)?;
		let converter_class: Pointer =
			api.class_named(battle_image, b"Torappu.DB\0", b"CrypticConverter\0")?;
		record("original_level_signed_decode_start");
		let decoded: Pointer = typed_call(
			api,
			converter_class,
			converter,
			b"DeserializeBytes\0",
			&[b"System.IO.Stream\0"],
			&mut [stream],
		)?;
		let _decoded_root: Rooted = api.root(decoded)?;
		record("original_level_signed_decode_complete");
		let stream_class: Pointer = api.class_named(core_image, b"System.IO\0", b"Stream\0")?;
		api.call(stream_class, stream, b"Close\0", &mut [])?;
		let memory_class: Pointer =
			api.class_named(core_image, b"System.IO\0", b"MemoryStream\0")?;
		let memory: Pointer = (api.new_object)(memory_class);
		let _memory_root: Rooted = api.root(memory)?;
		typed_call(
			api,
			memory_class,
			memory,
			b".ctor\0",
			&[b"System.Byte[]\0"],
			&mut [decoded],
		)?;
		let signed_class: Pointer = api.class_named(
			battle_image,
			b"Torappu.DB\0",
			b"FlatBufferSignedConverter\0",
		)?;
		let level_class: Pointer = api.class_named(battle_image, b"Torappu\0", b"LevelData\0")?;
		let level_type: Pointer = reflected_class(module, level_class)?;
		let _level_type_root: Rooted = api.root(level_type)?;
		record("original_level_typed_decode_start");
		let level: Pointer = typed_call(
			api,
			signed_class,
			converter,
			b"_DeserializeInternal\0",
			&[b"System.IO.MemoryStream\0", b"System.Type\0"],
			&mut [memory, level_type],
		)?;
		let _level_root: Rooted = api.root(level)?;
		record("original_level_typed_decode_complete");
		api.call(memory_class, memory, b"Close\0", &mut [])?;
		let map: Pointer = read_reference(module, level_class, level, b"mapData\0")?;
		let _map_root: Rooted = api.root(map)?;
		let map_class: Pointer = api.class_named(battle_image, b"Torappu\0", b"MapData\0")?;
		let width: i32 = api.integer(map_class, map, b"get_width\0")?;
		let height: i32 = api.integer(map_class, map, b"get_height\0")?;
		let waves: Pointer = read_reference(module, level_class, level, b"waves\0")?;
		let _waves_root: Rooted = api.root(waves)?;
		let array_length = level_bind!(
			module,
			b"il2cpp_array_length\0",
			unsafe extern "C" fn(Pointer) -> u32
		);
		if width <= 0 || height <= 0 || array_length(waves) == 0 {
			return Err("original_level_shape_invalid");
		}
		record("original_level_shape_verified");
		let serializer: Pointer =
			api.class_named(battle_image, b"Torappu\0", b"HGUnityJsonSerializer\0")?;
		let json: Pointer = api.call(
			serializer,
			core::ptr::null_mut(),
			b"SerializeToJson\0",
			&mut [level, level_type],
		)?;
		let _json_root: Rooted = api.root(json)?;
		record("original_level_serialized");
		let output: Pointer = environment_string(module, b"ARK_RUNTIME_LEVEL_OUTPUT")?;
		let _output_root: Rooted = api.root(output)?;
		record("original_level_output_write_start");
		let encoding_class: Pointer =
			api.class_named(core_image, b"System.Text\0", b"Encoding\0")?;
		let utf8: Pointer = api.call(
			encoding_class,
			core::ptr::null_mut(),
			b"get_UTF8\0",
			&mut [],
		)?;
		let _encoding_root: Rooted = api.root(utf8)?;
		let encoded: Pointer = typed_call(
			api,
			encoding_class,
			utf8,
			b"GetBytes\0",
			&[b"System.String\0"],
			&mut [json],
		)?;
		let _encoded_root: Rooted = api.root(encoded)?;
		typed_call(
			api,
			file_class,
			core::ptr::null_mut(),
			b"WriteAllBytes\0",
			&[b"System.String\0", b"System.Byte[]\0"],
			&mut [output, encoded],
		)?;
		level_shape(width, height, array_length(waves));
		record("original_level_decode_complete");
		api.call(
			object_class,
			core::ptr::null_mut(),
			b"DestroyImmediate\0",
			&mut [key_asset],
		)?;
		Ok(())
	})();
	api.call(
		object_class,
		core::ptr::null_mut(),
		b"DestroyImmediate\0",
		&mut [options],
	)?;
	result
}

unsafe fn parameter_mismatch(name: &[u8], index: u32, actual: *const c_char) {
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"typed_overload_skipped\",\"method\":\"");
	for offset in 0..name.len() {
		let value: u8 = name.as_ptr().add(offset).read();
		if value == 0 {
			break;
		}
		if line.length < 160 {
			line.bytes.as_mut_ptr().add(line.length).write(value);
			line.length += 1;
		}
	}
	line.text("\",\"parameter_index\":");
	line.number(index);
	line.text(",\"actual_type\":\"");
	if !actual.is_null() {
		for offset in 0..128 {
			let value: u8 = actual.cast::<u8>().add(offset).read();
			if value == 0 {
				break;
			}
			if (32..=126).contains(&value) && value != b'"' && value != b'\\' && line.length < 320 {
				line.bytes.as_mut_ptr().add(line.length).write(value);
				line.length += 1;
			}
		}
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
		STATUS.store(4, Ordering::Release);
	}
}

unsafe fn level_shape(width: i32, height: i32, waves: u32) {
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"original_level_decoded_shape\",\"map_width\":");
	signed(&mut line, width);
	line.text(",\"map_height\":");
	signed(&mut line, height);
	line.text(",\"wave_count\":");
	line.number(waves);
	line.text(",\"thread_id\":");
	line.number(GetCurrentThreadId());
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
