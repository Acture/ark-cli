// Field descriptions of the requested script classes from the original IL2CPP runtime metadata,
// for TypeTree generation. One JSON object per class, reached from the requested scripts through
// their parents and field types; the generator applies Unity's serialization rules.

#[link(name = "kernel32")]
extern "system" {
	fn ReadFile(
		file: *mut c_void,
		buffer: *mut u8,
		length: u32,
		read: *mut u32,
		overlapped: *mut c_void,
	) -> i32;
}

const REFLECTION_INPUT_BYTES: usize = 1 << 20;
const REFLECTION_CLASSES: usize = 16384;
const REFLECTION_SLOTS: usize = 32768;
static mut REFLECTION_INPUT: [u8; REFLECTION_INPUT_BYTES] = [0; REFLECTION_INPUT_BYTES];
static mut REFLECTION_QUEUE: [(usize, bool); REFLECTION_CLASSES] = [(0, false); REFLECTION_CLASSES];
static mut REFLECTION_SEEN: [usize; REFLECTION_SLOTS] = [0; REFLECTION_SLOTS];
static mut REFLECTION_EXPANDED: [bool; REFLECTION_SLOTS] = [false; REFLECTION_SLOTS];
static mut REFLECTION_BUFFER: [u8; 8192] = [0; 8192];

struct ReflectionApi {
	fields: unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer,
	field_name: unsafe extern "C" fn(Pointer) -> *const c_char,
	field_flags: unsafe extern "C" fn(Pointer) -> i32,
	field_type: unsafe extern "C" fn(Pointer) -> Pointer,
	field_attribute: unsafe extern "C" fn(Pointer, Pointer) -> bool,
	type_kind: unsafe extern "C" fn(Pointer) -> i32,
	type_name: unsafe extern "C" fn(Pointer) -> *mut c_char,
	type_class: unsafe extern "C" fn(Pointer) -> Pointer,
	class_type: unsafe extern "C" fn(Pointer) -> Pointer,
	element: unsafe extern "C" fn(Pointer) -> Pointer,
	parent: unsafe extern "C" fn(Pointer) -> Pointer,
	name: unsafe extern "C" fn(Pointer) -> *const c_char,
	namespace: unsafe extern "C" fn(Pointer) -> *const c_char,
	image: unsafe extern "C" fn(Pointer) -> Pointer,
	image_name: unsafe extern "C" fn(Pointer) -> *const c_char,
	flags: unsafe extern "C" fn(Pointer) -> i32,
	is_enum: unsafe extern "C" fn(Pointer) -> bool,
	enum_base: unsafe extern "C" fn(Pointer) -> Pointer,
	subclass: unsafe extern "C" fn(Pointer, Pointer, bool) -> bool,
	free: unsafe extern "C" fn(Pointer),
}

// Buffered over a static block: the no-CRT probe keeps stack frames below one page.
struct ReflectionOutput {
	handle: *mut c_void,
	length: usize,
	failed: bool,
}

impl ReflectionOutput {
	unsafe fn flush(&mut self) {
		let mut written: u32 = 0;
		if self.length > 0
			&& (WriteFile(
				self.handle,
				core::ptr::addr_of!(REFLECTION_BUFFER).cast::<u8>(),
				self.length as u32,
				&mut written,
				core::ptr::null_mut(),
			) == 0 || written as usize != self.length)
		{
			self.failed = true;
		}
		self.length = 0;
	}
	unsafe fn byte(&mut self, value: u8) {
		if self.length == 8192 {
			self.flush();
		}
		core::ptr::addr_of_mut!(REFLECTION_BUFFER)
			.cast::<u8>()
			.add(self.length)
			.write(value);
		self.length += 1;
	}
	unsafe fn text(&mut self, text: &str) {
		for value in text.bytes() {
			self.byte(value);
		}
	}
	// A JSON string from a NUL-terminated runtime name; control bytes never occur in names.
	unsafe fn name(&mut self, name: *const c_char) {
		self.byte(b'"');
		if !name.is_null() {
			let mut index: usize = 0;
			while index < 4096 {
				let value: u8 = name.add(index).cast::<u8>().read();
				if value == 0 {
					break;
				}
				if value == b'"' || value == b'\\' {
					self.byte(b'\\');
				}
				if value >= 0x20 {
					self.byte(value);
				}
				index += 1;
			}
		}
		self.byte(b'"');
	}
	unsafe fn number(&mut self, value: i64) {
		if value < 0 {
			self.byte(b'-');
		}
		// A u64 has at most 20 decimal digits; the pointer writes stay inside the array.
		let mut digits: [u8; 20] = [0; 20];
		let mut count: usize = 0;
		let mut rest: u64 = value.unsigned_abs();
		loop {
			digits.as_mut_ptr().add(count).write(b'0' + (rest % 10) as u8);
			count += 1;
			rest /= 10;
			if rest == 0 {
				break;
			}
		}
		while count > 0 {
			count -= 1;
			self.byte(digits.as_ptr().add(count).read());
		}
	}
}

impl ReflectionApi {
	unsafe fn load(module: Pointer) -> Result<Self, &'static str> {
		Ok(Self {
			fields: level_bind!(
				module,
				b"il2cpp_class_get_fields\0",
				unsafe extern "C" fn(Pointer, *mut Pointer) -> Pointer
			),
			field_name: level_bind!(
				module,
				b"il2cpp_field_get_name\0",
				unsafe extern "C" fn(Pointer) -> *const c_char
			),
			field_flags: level_bind!(
				module,
				b"il2cpp_field_get_flags\0",
				unsafe extern "C" fn(Pointer) -> i32
			),
			field_type: level_bind!(
				module,
				b"il2cpp_field_get_type\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			field_attribute: level_bind!(
				module,
				b"il2cpp_field_has_attribute\0",
				unsafe extern "C" fn(Pointer, Pointer) -> bool
			),
			type_kind: level_bind!(
				module,
				b"il2cpp_type_get_type\0",
				unsafe extern "C" fn(Pointer) -> i32
			),
			type_name: level_bind!(
				module,
				b"il2cpp_type_get_name\0",
				unsafe extern "C" fn(Pointer) -> *mut c_char
			),
			type_class: level_bind!(
				module,
				b"il2cpp_class_from_type\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			class_type: level_bind!(
				module,
				b"il2cpp_class_get_type\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			element: level_bind!(
				module,
				b"il2cpp_class_get_element_class\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			parent: level_bind!(
				module,
				b"il2cpp_class_get_parent\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			name: level_bind!(
				module,
				b"il2cpp_class_get_name\0",
				unsafe extern "C" fn(Pointer) -> *const c_char
			),
			namespace: level_bind!(
				module,
				b"il2cpp_class_get_namespace\0",
				unsafe extern "C" fn(Pointer) -> *const c_char
			),
			image: level_bind!(
				module,
				b"il2cpp_class_get_image\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			image_name: level_bind!(
				module,
				b"il2cpp_image_get_name\0",
				unsafe extern "C" fn(Pointer) -> *const c_char
			),
			flags: level_bind!(
				module,
				b"il2cpp_class_get_flags\0",
				unsafe extern "C" fn(Pointer) -> i32
			),
			is_enum: level_bind!(
				module,
				b"il2cpp_class_is_enum\0",
				unsafe extern "C" fn(Pointer) -> bool
			),
			enum_base: level_bind!(
				module,
				b"il2cpp_class_enum_basetype\0",
				unsafe extern "C" fn(Pointer) -> Pointer
			),
			subclass: level_bind!(
				module,
				b"il2cpp_class_is_subclass_of\0",
				unsafe extern "C" fn(Pointer, Pointer, bool) -> bool
			),
			free: level_bind!(module, b"il2cpp_free\0", unsafe extern "C" fn(Pointer)),
		})
	}

	// The runtime's full type name, the key that field types refer to.
	unsafe fn key(&self, output: &mut ReflectionOutput, class: Pointer) {
		let name: *mut c_char = (self.type_name)((self.class_type)(class));
		output.name(name);
		if !name.is_null() {
			(self.free)(name.cast());
		}
	}
}

// Class pointers are recorded once, and once more when a script's base must be expanded after a
// field reached it first; a full slot table stops the export instead of looping.
unsafe fn reflection_enqueue(class: Pointer, expand: bool, count: &mut usize) -> Result<(), &'static str> {
	if class.is_null() {
		return Ok(());
	}
	let seen: *mut usize = core::ptr::addr_of_mut!(REFLECTION_SEEN).cast();
	let expanded: *mut bool = core::ptr::addr_of_mut!(REFLECTION_EXPANDED).cast();
	let mut slot: usize = (class as usize >> 4) % REFLECTION_SLOTS;
	loop {
		let current: usize = seen.add(slot).read();
		if current == class as usize {
			if !expand || expanded.add(slot).read() {
				return Ok(());
			}
			break;
		}
		if current == 0 {
			break;
		}
		slot = (slot + 1) % REFLECTION_SLOTS;
	}
	if *count == REFLECTION_CLASSES {
		return Err("reflection_class_bound_exceeded");
	}
	seen.add(slot).write(class as usize);
	expanded.add(slot).write(expand);
	core::ptr::addr_of_mut!(REFLECTION_QUEUE)
		.cast::<(usize, bool)>()
		.add(*count)
		.write((class as usize, expand));
	*count += 1;
	Ok(())
}

unsafe fn c_text_equals(pointer: *const c_char, expected: &[u8]) -> bool {
	c_name_matches(pointer, expected) && pointer.add(expected.len()).read() == 0
}

// The engine's own Object classes end a script's inherited fields; UnityEngine.UI and other
// managed Behaviours above them keep theirs.
unsafe fn reflection_engine_base(reflect: &ReflectionApi, class: Pointer) -> bool {
	c_text_equals((reflect.namespace)(class), b"UnityEngine")
		&& [
			b"Object".as_slice(),
			b"Component",
			b"Behaviour",
			b"MonoBehaviour",
			b"ScriptableObject",
		]
		.iter()
		.any(|name| c_text_equals((reflect.name)(class), name))
}

unsafe fn export_reflection(api: &Api, domain: Pointer) -> Result<(), &'static str> {
	let module: Pointer = GetModuleHandleW(ASSEMBLY.as_ptr());
	let reflect: ReflectionApi = ReflectionApi::load(module)?;
	let core: Pointer = (api.image)((api.assembly)(domain, b"UnityEngine.CoreModule\0".as_ptr().cast()));
	let unity_object: Pointer = api.class_named(core, b"UnityEngine\0", b"Object\0")?;
	let serialize_field: Pointer = api.class_named(core, b"UnityEngine\0", b"SerializeField\0")?;
	let serialize_reference: Pointer =
		api.class_named(core, b"UnityEngine\0", b"SerializeReference\0")?;
	let input: *mut c_void = reflection_file(b"ARK_RUNTIME_REFLECTION_INPUT", false)?;
	let buffer: *mut u8 = core::ptr::addr_of_mut!(REFLECTION_INPUT).cast();
	let mut size: u32 = 0;
	let read: i32 = ReadFile(
		input,
		buffer,
		REFLECTION_INPUT_BYTES as u32,
		&mut size,
		core::ptr::null_mut(),
	);
	CloseHandle(input);
	if read == 0 || size as usize >= REFLECTION_INPUT_BYTES {
		return Err("reflection_input_unreadable");
	}
	let handle: *mut c_void = reflection_file(b"ARK_RUNTIME_REFLECTION_OUTPUT", true)?;
	let mut output: ReflectionOutput = ReflectionOutput {
		handle,
		length: 0,
		failed: false,
	};
	let mut count: usize = 0;
	// Each request line is `assembly|namespace|class`, every part NUL-terminated in place.
	let mut start: usize = 0;
	let mut requested: u32 = 0;
	while start < size as usize {
		let mut end: usize = start;
		while end < size as usize && buffer.add(end).read() != b'\n' {
			end += 1;
		}
		let mut parts: [usize; 3] = [start, 0, 0];
		let mut part: usize = 1;
		for index in start..end {
			if buffer.add(index).read() == b'|' && part < 3 {
				buffer.add(index).write(0);
				parts.as_mut_ptr().add(part).write(index + 1);
				part += 1;
			}
		}
		let [assembly_name, namespace, name]: [usize; 3] = parts;
		if end < size as usize {
			buffer.add(end).write(0);
		}
		if part != 3 || end == size as usize {
			return Err("reflection_request_malformed");
		}
		let assembly: Pointer = (api.assembly)(domain, buffer.add(assembly_name).cast());
		let class: Pointer = if assembly.is_null() {
			core::ptr::null_mut()
		} else {
			(api.class)(
				(api.image)(assembly),
				buffer.add(namespace).cast(),
				buffer.add(name).cast(),
			)
		};
		if class.is_null() {
			output.text("{\"missing\":");
			output.name(buffer.add(assembly_name).cast());
			output.text(",\"namespace\":");
			output.name(buffer.add(namespace).cast());
			output.text(",\"name\":");
			output.name(buffer.add(name).cast());
			output.text("}\n");
		}
		reflection_enqueue(class, true, &mut count)?;
		requested += 1;
		start = end + 1;
	}
	let queue: *const (usize, bool) = core::ptr::addr_of!(REFLECTION_QUEUE).cast();
	let mut index: usize = 0;
	while index < count {
		let (address, requested_class): (usize, bool) = queue.add(index).read();
		let class: Pointer = address as Pointer;
		let flags: i32 = (reflect.flags)(class);
		let is_object: bool = class == unity_object || (reflect.subclass)(class, unity_object, false);
		let value_type: bool = (api.is_value_type)(class);
		let list: bool = c_text_equals((reflect.namespace)(class), b"System.Collections.Generic")
			&& c_text_equals((reflect.name)(class), b"List`1");
		// Fields matter for requested scripts and their managed bases, and for the value, list
		// and [Serializable] types a field can hold; engine bases and other references stop here.
		let expand: bool = if is_object {
			requested_class && !reflection_engine_base(&reflect, class)
		} else {
			flags & 0x2000 != 0 || value_type || list
		};
		output.text("{\"key\":");
		reflect.key(&mut output, class);
		output.text(",\"name\":");
		output.name((reflect.name)(class));
		output.text(",\"namespace\":");
		output.name((reflect.namespace)(class));
		output.text(",\"image\":");
		output.name((reflect.image_name)((reflect.image)(class)));
		output.text(",\"flags\":");
		output.number(i64::from(flags));
		output.text(",\"object\":");
		output.text(if is_object { "true" } else { "false" });
		output.text(",\"value_type\":");
		output.text(if value_type { "true" } else { "false" });
		if (reflect.is_enum)(class) {
			output.text(",\"enum\":");
			output.number(i64::from((reflect.type_kind)((reflect.enum_base)(class))));
		}
		if expand {
			let parent: Pointer = (reflect.parent)(class);
			if !parent.is_null() {
				output.text(",\"parent\":");
				reflect.key(&mut output, parent);
				reflection_enqueue(parent, is_object, &mut count)?;
			}
			output.text(",\"fields\":[");
			let mut iterator: Pointer = core::ptr::null_mut();
			let mut first: bool = true;
			loop {
				let field: Pointer = (reflect.fields)(class, &mut iterator);
				if field.is_null() {
					break;
				}
				if !first {
					output.byte(b',');
				}
				first = false;
				let ty: Pointer = (reflect.field_type)(field);
				let kind: i32 = (reflect.type_kind)(ty);
				output.text("{\"name\":");
				output.name((reflect.field_name)(field));
				output.text(",\"flags\":");
				output.number(i64::from((reflect.field_flags)(field)));
				if (reflect.field_attribute)(field, serialize_field) {
					output.text(",\"serialize_field\":true");
				}
				if (reflect.field_attribute)(field, serialize_reference) {
					output.text(",\"serialize_reference\":true");
				}
				output.text(",\"kind\":");
				output.number(i64::from(kind));
				// CLASS, VALUETYPE, GENERICINST and SZARRAY name a class; static fields are
				// described but never followed.
				if matches!(kind, 0x11 | 0x12 | 0x15 | 0x1d) {
					let target: Pointer = (reflect.type_class)(ty);
					let followed: Pointer = if kind == 0x1d {
						(reflect.element)(target)
					} else {
						target
					};
					if !followed.is_null() {
						if kind == 0x1d {
							output.text(",\"element_kind\":");
							output.number(i64::from((reflect.type_kind)((reflect.class_type)(followed))));
							output.text(",\"element\":");
						} else {
							output.text(",\"class\":");
						}
						reflect.key(&mut output, followed);
						if (reflect.field_flags)(field) & 0x10 == 0 {
							reflection_enqueue(followed, false, &mut count)?;
						}
					}
				}
				output.byte(b'}');
			}
			output.byte(b']');
		}
		output.text("}\n");
		index += 1;
	}
	output.flush();
	CloseHandle(handle);
	if output.failed {
		return Err("reflection_output_write_failed");
	}
	scene_number("original_reflection_requested", requested);
	scene_number("original_reflection_classes", count as u32);
	Ok(())
}

// An explicit drive-absolute path from the environment, opened for reading or created anew.
unsafe fn reflection_file(variable: &[u8], create: bool) -> Result<*mut c_void, &'static str> {
	let mut name: [u16; 64] = [0; 64];
	for index in 0..variable.len().min(63) {
		name.as_mut_ptr()
			.add(index)
			.write(variable.as_ptr().add(index).read() as u16);
	}
	let mut path: [u16; 520] = [0; 520];
	let length: u32 = GetEnvironmentVariableW(name.as_ptr(), path.as_mut_ptr(), 520);
	let separator: u16 = path.as_ptr().add(2).read();
	if length < 3
		|| length >= 520
		|| path.as_ptr().add(1).read() != 58
		|| (separator != 47 && separator != 92)
	{
		return Err("reflection_path_invalid");
	}
	let handle: *mut c_void = CreateFileW(
		path.as_ptr(),
		if create { 0x4000_0000 } else { 0x8000_0000 },
		1,
		core::ptr::null(),
		if create { 2 } else { 3 },
		0x80,
		core::ptr::null_mut(),
	);
	if handle as isize == -1 {
		return Err("reflection_file_unavailable");
	}
	Ok(handle)
}
