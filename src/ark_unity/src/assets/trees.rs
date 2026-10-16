//! Same-version release field descriptions plus direct native IL2CPP metadata generation.
//! Generated descriptions earn admission only after all original objects roundtrip.

use super::bundle;
use super::serialized::{self, File, Node, Script};
use super::tpk::Registry;
use super::{pin, text, Catalog, UNITY_VERSION};
use anyhow::{ensure, Context, Result};
use libloading::Library;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

pub(crate) struct Trees {
	registry: Registry,
	provenance: Value,
	original: BTreeMap<(i32, [u8; 16]), Node>,
	generator: Option<Generator>,
}

impl Trees {
	pub fn new(catalog: &Catalog, scene: &Value) -> Result<Self> {
		let tpk: PathBuf = std::env::var_os("ARK_UNITY_TPK")
			.map(PathBuf::from)
			.unwrap_or_else(|| catalog.state.join("tools/unitypy-1.25.4/lzma.tpk"));
		ensure!(
			fs::metadata(&tpk)
				.with_context(|| format!(
					"Set ARK_UNITY_TPK to a same-version release TPK: {}",
					tpk.display()
				))?
				.len() <= 32 * 1024 * 1024,
			"TPK input exceeds bound"
		);
		let data: Vec<u8> = fs::read(&tpk)?;
		ensure!(data.len() >= 20, "Truncated TPK header");
		let size: u32 = u32::from_le_bytes(data[16..20].try_into()?);
		ensure!(
			size <= 256 * 1024 * 1024,
			"TPK decompressed body exceeds bound"
		);
		let registry: Registry = Registry::from_bytes(&data)?;
		let mut original: BTreeMap<(i32, [u8; 16]), Node> = BTreeMap::new();
		for record in scene["bundles"]
			.as_array()
			.context("Missing scene bundles")?
		{
			for entry in bundle::load(std::path::Path::new(text(record, "path")?))? {
				if entry.flags & 4 == 0 {
					continue;
				}
				for ty in File::parse(&entry.bytes)?.types {
					if let Some(tree) = ty.tree {
						original.entry((ty.class, ty.hash)).or_insert(tree);
					}
				}
			}
		}
		Ok(Self {
			registry,
			provenance: json!({"tpk": pin(&tpk)?, "decoder": "ark-unity-tpk-v1-v2", "unity_version": UNITY_VERSION}),
			original,
			generator: None,
		})
	}

	pub fn builtin(&self, class: i32) -> Result<Node> {
		self.registry.resolve(class)
	}

	pub fn fill_builtins(&self, file: &mut File) -> Result<()> {
		for ty in &mut file.types {
			if ty.tree.is_none() && ty.class != 114 {
				ty.tree = Some(self.builtin(ty.class)?);
			}
		}
		Ok(())
	}

	pub fn restore(
		&mut self,
		catalog: &Catalog,
		file: &mut File,
		scripts: &BTreeMap<i64, Script>,
	) -> Result<Vec<Value>> {
		ensure!(!file.enable_trees, "Expected stripped original metadata");
		let header: Node = self
			.original
			.iter()
			.find(|((class, _), _)| *class == 114)
			.map(|(_, tree)| tree.clone())
			.context("No exact original MonoBehaviour header in scene closure")?;
		let mut records: Vec<Value> = Vec::new();
		for index in 0..file.types.len() {
			let ty = &file.types[index];
			let objects: Vec<&serialized::Object> = file
				.objects
				.iter()
				.filter(|obj| obj.type_index == index)
				.collect();
			let (mut tree, origin): (Node, String) =
				if let Some(tree) = self.original.get(&(ty.class, ty.hash)) {
					(
						tree.clone(),
						"matching original archive class/hash".to_owned(),
					)
				} else if ty.class == 114 {
					let obj: &serialized::Object = objects
						.first()
						.context("Cannot claim a stripped script without an original object")?;
					let (_, pointer, _) = serialized::mono_head(&obj.raw)?;
					ensure!(
						pointer.0 == 1,
						"Stripped MonoBehaviour script is not in original mapper"
					);
					let script: &Script = scripts
						.get(&pointer.1)
						.context("Stripped MonoBehaviour script identity absent")?;
					let fullname: String = if script.namespace.is_empty() {
						script.class.clone()
					} else {
						format!("{}.{}", script.namespace, script.class)
					};
					if self.generator.is_none() {
						self.generator = Some(Generator::new(catalog)?);
					}
					let mut generated: Node = self
						.generator
						.as_ref()
						.context("Native generator absent")?
						.generate(
							script
								.assembly
								.strip_suffix(".dll")
								.unwrap_or(&script.assembly),
							&fullname,
						)?;
					ensure!(
						generated.children.len() >= 4 && header.children.len() >= 4,
						"MonoBehaviour header too short"
					);
					ensure!(
						generated.children[..4]
							.iter()
							.map(|n| &n.name)
							.eq(header.children[..4].iter().map(|n| &n.name)),
						"Generated MonoBehaviour header differs"
					);
					generated.children[..4].clone_from_slice(&header.children[..4]);
					generated.type_name = header.type_name.clone();
					normalize(&mut generated);
					(
						generated,
						format!(
							"original IL2CPP field metadata: {}:{fullname}",
							script.assembly
						),
					)
				} else {
					(
						self.builtin(ty.class)?,
						"same-version release TPK, byte-roundtrip checked".to_owned(),
					)
				};
			let mut counter: i32 = 0;
			finalize(&mut tree, &mut counter, 0)?;
			for obj in &objects {
				let value: Value = serialized::decode(&tree, &obj.raw).with_context(|| {
					format!(
						"Object {}, type {index}, class {}, {origin}",
						obj.id, ty.class
					)
				})?;
				ensure!(serialized::encode(&tree, &value)? == obj.raw, "Field description does not roundtrip original object {}, type {index}, {origin}", obj.id);
				if ty.class == 114 {
					ensure!(
						serialized::pointer(&value["m_Script"])?
							== serialized::mono_head(&obj.raw)?.1,
						"Generated script pointer differs from original"
					);
				}
			}
			records.push(json!({"type_index": index, "class_id": ty.class, "original_type_hash": ty.hash.iter().map(|b| format!("{b:02x}")).collect::<String>(), "origin": origin, "objects_checked": objects.len(), "decoder_provenance": self.provenance}));
			file.types[index].tree = Some(tree);
			file.types[index].dependencies.clear();
		}
		ensure!(
			file.reference_types.is_empty(),
			"Stripped reference-type restoration is unsupported"
		);
		file.enable_trees = true;
		if let Some(generator) = &self.generator {
			for record in &mut records {
				record["generator_provenance"] = generator.provenance.clone();
			}
		}
		Ok(records)
	}
}

fn normalize(node: &mut Node) {
	if node.children.len() == 1 && node.children[0].type_name == "Array" {
		let is_string: bool = node.type_name == "string"
			&& node.children[0]
				.children
				.get(1)
				.is_some_and(|n| n.type_name == "char");
		if !is_string {
			node.type_name = "vector".to_owned();
		}
	}
	if node.type_name.starts_with("PPtr<$") {
		node.type_name = node.type_name.replacen("PPtr<$", "PPtr<", 1);
	}
	for child in &mut node.children {
		normalize(child);
	}
}

fn finalize(node: &mut Node, counter: &mut i32, level: i32) -> Result<i32> {
	ensure!(level <= 256, "Generated TypeTree depth exceeds bound");
	node.index = *counter;
	*counter = counter.checked_add(1).context("TypeTree index overflow")?;
	node.level = level;
	let mut child_sizes: Vec<i32> = Vec::new();
	for child in &mut node.children {
		child_sizes.push(finalize(child, counter, level + 1)?);
	}
	let size: i32 = match node.type_name.as_str() {
		"SInt8" | "UInt8" | "char" | "bool" => 1,
		"short" | "SInt16" | "UInt16" | "unsigned short" => 2,
		"int" | "unsigned int" | "SInt32" | "UInt32" | "float" => 4,
		"long long" | "unsigned long long" | "SInt64" | "UInt64" | "double" => 8,
		"Array" | "string" => -1,
		_ if child_sizes.iter().any(|size| *size < 0) => -1,
		_ if !child_sizes.is_empty() => child_sizes.iter().try_fold(0_i32, |sum, size| {
			sum.checked_add(*size)
				.context("TypeTree fixed size overflow")
		})?,
		_ => node.byte_size,
	};
	if node.byte_size == 0 {
		node.byte_size = size;
	}
	Ok(size)
}

#[repr(C, packed(4))]
struct NativeNode {
	ty: *const c_char,
	name: *const c_char,
	level: i32,
	meta: i32,
}

type Init = unsafe extern "system" fn(*const c_char, *const c_char) -> *mut c_void;
type Load = unsafe extern "system" fn(*mut c_void, *const u8, i32, *const u8, i32) -> i32;
type Generate = unsafe extern "system" fn(
	*mut c_void,
	*const c_char,
	*const c_char,
	*mut *mut NativeNode,
	*mut i32,
) -> i32;
type FreeNodes = unsafe extern "system" fn(*mut NativeNode, i32) -> i32;
type Delete = unsafe extern "system" fn(*mut c_void) -> i32;

struct NativeHandle {
	ptr: *mut c_void,
	delete: Delete,
}

impl Drop for NativeHandle {
	fn drop(&mut self) {
		// SAFETY: this guard uniquely owns the API-created handle. Its paired callback
		// is captured before initialization and its library outlives the guard.
		unsafe {
			let _status: i32 = (self.delete)(self.ptr);
		}
	}
}

struct NativeNodes {
	ptr: *mut NativeNode,
	count: i32,
	free: FreeNodes,
}

impl NativeNodes {
	/// Takes ownership before validating the native result, including error outputs.
	///
	/// # Safety
	/// A non-null pointer must be API-owned and paired with this exact count/free callback.
	unsafe fn checked(
		ptr: *mut NativeNode,
		count: i32,
		status: i32,
		free: FreeNodes,
	) -> Result<Self> {
		let allocation: Self = Self { ptr, count, free };
		ensure!(
			status == 0 && !ptr.is_null() && (1..=1_000_000).contains(&count),
			"Native TypeTree generation failed or exceeded bound"
		);
		Ok(allocation)
	}
}

impl Drop for NativeNodes {
	fn drop(&mut self) {
		if !self.ptr.is_null() {
			// SAFETY: the native API supplied both pointer and count. The allocation is
			// returned to its paired API even when its result cannot be admitted by Rust.
			unsafe {
				let _status: i32 = (self.free)(self.ptr, self.count);
			}
		}
	}
}

struct Generator {
	// Fields drop in declaration order: the native handle must precede its library.
	handle: NativeHandle,
	library: Library,
	provenance: Value,
}

impl Generator {
	fn new(catalog: &Catalog) -> Result<Self> {
		let started: Instant = Instant::now();
		eprintln!("INFO Loading original IL2CPP field metadata in native TypeTree generator");
		let path: PathBuf = std::env::var_os("ARK_TYPE_TREE_GENERATOR")
			.map(PathBuf::from)
			.unwrap_or_else(|| {
				catalog
					.state
					.join("tools/typetreegenerator-0.0.10/TypeTreeGeneratorAPI.dll")
			});
		let path: PathBuf = fs::canonicalize(&path).with_context(|| {
			format!(
				"Set ARK_TYPE_TREE_GENERATOR to the native metadata generator: {}",
				path.display()
			)
		})?;
		let assembly: PathBuf = catalog.client.join("GameAssembly.dll");
		let metadata: PathBuf = catalog
			.data
			.join("il2cpp_data/Metadata/global-metadata.dat");
		let il2cpp: Vec<u8> = fs::read(&assembly)?;
		let meta: Vec<u8> = fs::read(&metadata)?;
		let il2cpp_len: i32 = i32::try_from(il2cpp.len())?;
		let meta_len: i32 = i32::try_from(meta.len())?;
		let version: CString = CString::new(UNITY_VERSION)?;
		let backend: CString = CString::new("AssetsTools")?;
		// SAFETY: the explicitly selected local library exports the documented 0.0.10 C ABI.
		// It remains loaded for the full generator lifetime; immutable input slices remain
		// alive until the synchronous load call returns. All native allocations are freed.
		unsafe {
			let library: Library = Library::new(&path)?;
			let init: libloading::Symbol<'_, Init> = library.get(b"TypeTreeGenerator_init\0")?;
			let delete: Delete = *library.get(b"TypeTreeGenerator_del\0")?;
			let ptr: *mut c_void = init(version.as_ptr(), backend.as_ptr());
			ensure!(
				!ptr.is_null(),
				"Native TypeTree generator initialization failed"
			);
			let handle: NativeHandle = NativeHandle { ptr, delete };
			// Compute fallible provenance while the handle guard is already active.
			let provenance: Value = json!({"library": pin(&path)?, "abi": "TypeTreeGeneratorAPI-0.0.10 C interface", "backend": "AssetsTools", "il2cpp": pin(&assembly)?, "metadata": pin(&metadata)?});
			let generator: Self = Self {
				handle,
				library,
				provenance,
			};
			let load: libloading::Symbol<'_, Load> =
				generator.library.get(b"TypeTreeGenerator_loadIL2CPP\0")?;
			ensure!(
				load(
					generator.handle.ptr,
					il2cpp.as_ptr(),
					il2cpp_len,
					meta.as_ptr(),
					meta_len
				) == 0,
				"Native IL2CPP metadata load failed"
			);
			eprintln!(
				"INFO Native IL2CPP field metadata loaded in {:.2}s",
				started.elapsed().as_secs_f64()
			);
			Ok(generator)
		}
	}
	fn generate(&self, assembly: &str, fullname: &str) -> Result<Node> {
		let assembly: CString = CString::new(assembly)?;
		let fullname: CString = CString::new(fullname)?;
		// SAFETY: output is an API-owned packed node array. We validate its count,
		// copy each field while the allocation is live, and free through the paired API.
		unsafe {
			let generate: libloading::Symbol<'_, Generate> = self
				.library
				.get(b"TypeTreeGenerator_generateTreeNodesRaw\0")?;
			let free: libloading::Symbol<'_, FreeNodes> =
				self.library.get(b"TypeTreeGenerator_freeTreeNodesRaw\0")?;
			let mut raw: *mut NativeNode = std::ptr::null_mut();
			let mut count: i32 = 0;
			let status: i32 = generate(
				self.handle.ptr,
				assembly.as_ptr(),
				fullname.as_ptr(),
				&mut raw,
				&mut count,
			);
			let allocation: NativeNodes = NativeNodes::checked(raw, count, status, *free)?;
			let mut nodes: Vec<Node> = Vec::with_capacity(allocation.count as usize);
			for index in 0..allocation.count as usize {
				let native: &NativeNode = &*allocation.ptr.add(index);
				ensure!(
					!native.ty.is_null() && !native.name.is_null(),
					"Null generated node string"
				);
				let mut node: Node = Node::with_info(
					CStr::from_ptr(native.ty).to_str()?.to_owned(),
					CStr::from_ptr(native.name).to_str()?.to_owned(),
					0,
				);
				node.level = native.level;
				node.meta_flags = native.meta;
				node.index = index as i32;
				node.version = 1;
				node.type_flags = i32::from(node.type_name == "Array");
				nodes.push(node);
			}
			serialized::from_flat(nodes)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};

	unsafe extern "system" fn count_delete(ptr: *mut c_void) -> i32 {
		// SAFETY: tests pass a live AtomicUsize as their opaque native allocation.
		unsafe {
			(&*ptr.cast::<AtomicUsize>()).fetch_add(1, Ordering::SeqCst);
		}
		0
	}

	unsafe extern "system" fn count_free(ptr: *mut NativeNode, _count: i32) -> i32 {
		// SAFETY: tests never decode this pointer and use the same opaque counter ABI.
		unsafe { count_delete(ptr.cast()) }
	}

	#[test]
	fn native_node_output_is_freed_when_count_or_status_is_rejected() {
		for (count, status) in [(0, 0), (-1, 0), (1_000_001, 0), (1, 7)] {
			let frees: AtomicUsize = AtomicUsize::new(0);
			let ptr: *mut NativeNode = std::ptr::from_ref(&frees).cast_mut().cast();
			// SAFETY: fake paired callbacks operate only on the live counter.
			assert!(unsafe { NativeNodes::checked(ptr, count, status, count_free) }.is_err());
			assert_eq!(frees.load(Ordering::SeqCst), 1);
		}
	}

	#[test]
	fn native_generator_handle_is_freed_when_provenance_pinning_fails() {
		let deletes: AtomicUsize = AtomicUsize::new(0);
		let missing: PathBuf = std::env::temp_dir().join(format!(
			"ark-missing-generator-pin-{}-{}",
			std::process::id(),
			chrono::Utc::now().timestamp_nanos_opt().unwrap()
		));
		let result: Result<Value> = {
			let _handle: NativeHandle = NativeHandle {
				ptr: std::ptr::from_ref(&deletes).cast_mut().cast(),
				delete: count_delete,
			};
			pin(&missing)
		};
		assert!(result.is_err());
		assert_eq!(deletes.load(Ordering::SeqCst), 1);
	}
	#[test]
	fn generated_shapes_normalize_without_changing_header_alignment() {
		let mut node: Node = Node::with_info("List<X>".into(), "items".into(), 0);
		let mut array: Node = Node::with_info("Array".into(), "Array".into(), 0);
		array.children = vec![
			Node::with_info("int".into(), "size".into(), 4),
			Node::with_info("PPtr<$Object>".into(), "data".into(), 12),
		];
		node.children.push(array);
		normalize(&mut node);
		finalize(&mut node, &mut 0, 0).unwrap();
		assert_eq!(node.type_name, "vector");
		assert_eq!(node.byte_size, -1);
		assert_eq!(node.children[0].children[1].type_name, "PPtr<Object>");
	}
}
