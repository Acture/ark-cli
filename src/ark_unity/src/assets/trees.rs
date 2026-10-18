//! Same-version release field descriptions plus field descriptions generated from the original
//! runtime IL2CPP metadata. Generated descriptions earn admission only after all original objects
//! roundtrip.

use super::bundle;
use super::serialized::{self, File, Node, Script};
use super::tpk::Registry;
use super::{pin, text, Catalog, UNITY_VERSION};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct Trees {
	registry: Registry,
	provenance: Value,
	original: BTreeMap<(i32, [u8; 16]), Node>,
	reflection: Reflection,
}

impl Trees {
	pub fn new(catalog: &Catalog, scene: &Value, reflection: &Path) -> Result<Self> {
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
			reflection: Reflection::load(reflection)?,
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

	pub fn restore(&self, file: &mut File, scripts: &BTreeMap<i64, Script>) -> Result<Vec<Value>> {
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
					let mut generated: Node = self.reflection.generate(&fullname)?;
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
							"original runtime IL2CPP metadata: {}:{fullname}",
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
		for record in &mut records {
			record["generator_provenance"] = self.reflection.provenance.clone();
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

/// Original runtime type kinds (Il2CppTypeEnum) of exported field types.
mod kind {
	pub const BOOLEAN: i64 = 0x02;
	pub const CHAR: i64 = 0x03;
	pub const I1: i64 = 0x04;
	pub const U1: i64 = 0x05;
	pub const I2: i64 = 0x06;
	pub const U2: i64 = 0x07;
	pub const I4: i64 = 0x08;
	pub const U4: i64 = 0x09;
	pub const I8: i64 = 0x0a;
	pub const U8: i64 = 0x0b;
	pub const R4: i64 = 0x0c;
	pub const R8: i64 = 0x0d;
	pub const STRING: i64 = 0x0e;
	pub const VALUETYPE: i64 = 0x11;
	pub const CLASS: i64 = 0x12;
	pub const GENERICINST: i64 = 0x15;
	pub const SZARRAY: i64 = 0x1d;
}

const ALIGN: i32 = 0x4000;
const TYPE_SERIALIZABLE: i64 = 0x2000;
const TYPE_ABSTRACT_OR_INTERFACE: i64 = 0x80 | 0x20;
const FIELD_ACCESS: i64 = 0x7;
const FIELD_PUBLIC: i64 = 0x6;
// Static, init-only, literal and NonSerialized fields never serialize.
const FIELD_UNSERIALIZED: i64 = 0x10 | 0x20 | 0x40 | 0x80;
// Unity's serialization depth limit for nested reference types.
const MAX_CLASS_DEPTH: usize = 10;

#[derive(Debug)]
struct Field {
	name: String,
	flags: i64,
	serialize_field: bool,
	serialize_reference: bool,
	kind: i64,
	class: Option<String>,
	element_kind: Option<i64>,
	element: Option<String>,
}

impl Field {
	fn parse(value: &Value) -> Result<Self> {
		Ok(Self {
			name: text(value, "name")?.to_owned(),
			flags: value["flags"].as_i64().context("Field flags missing")?,
			serialize_field: value["serialize_field"] == true,
			serialize_reference: value["serialize_reference"] == true,
			kind: value["kind"].as_i64().context("Field kind missing")?,
			class: value["class"].as_str().map(str::to_owned),
			element_kind: value["element_kind"].as_i64(),
			element: value["element"].as_str().map(str::to_owned),
		})
	}

	/// Unity serializes public or [SerializeField] instance fields that are writable.
	fn serialized(&self) -> bool {
		(self.flags & FIELD_ACCESS == FIELD_PUBLIC
			|| self.serialize_field
			|| self.serialize_reference)
			&& self.flags & FIELD_UNSERIALIZED == 0
	}
}

/// A class as the probe exported it from the original runtime metadata.
#[derive(Debug)]
struct Class {
	name: String,
	namespace: String,
	image: String,
	flags: i64,
	object: bool,
	value_type: bool,
	enum_kind: Option<i64>,
	parent: Option<String>,
	fields: Option<Vec<Field>>,
}

impl Class {
	/// The engine's own Object classes: a script's inherited fields end below them.
	fn engine_base(&self) -> bool {
		self.namespace == "UnityEngine"
			&& [
				"Object",
				"Component",
				"Behaviour",
				"MonoBehaviour",
				"ScriptableObject",
			]
			.contains(&self.name.as_str())
	}

	fn list(&self) -> bool {
		self.namespace == "System.Collections.Generic" && self.name == "List`1"
	}

	/// The .NET class libraries' own types never serialize, apart from primitives and List.
	fn system(&self) -> bool {
		self.image == "mscorlib.dll"
			|| self.image == "System.dll"
			|| self.image.starts_with("System.")
	}
}

/// Field descriptions from the original runtime IL2CPP metadata, generated by Unity's rules for
/// serializing MonoBehaviour fields.
pub(crate) struct Reflection {
	classes: BTreeMap<String, Class>,
	provenance: Value,
}

impl Reflection {
	pub fn load(path: &Path) -> Result<Self> {
		ensure!(
			fs::metadata(path)?.len() <= 512 * 1024 * 1024,
			"Runtime metadata export exceeds bound"
		);
		let mut classes: BTreeMap<String, Class> = BTreeMap::new();
		for line in fs::read_to_string(path)?
			.lines()
			.filter(|line| !line.is_empty())
		{
			let value: Value = serde_json::from_str(line)?;
			// A requested script the runtime does not know fails when it is generated.
			if value.get("missing").is_some() {
				continue;
			}
			let fields: Option<Vec<Field>> = value["fields"]
				.as_array()
				.map(|fields| fields.iter().map(Field::parse).collect())
				.transpose()?;
			let key: &str = text(&value, "key")?;
			// A class reached first as a reference appears again expanded when a script
			// inherits it; the expanded record wins.
			if fields.is_none() && classes.get(key).is_some_and(|class| class.fields.is_some()) {
				continue;
			}
			classes.insert(
				key.to_owned(),
				Class {
					name: text(&value, "name")?.to_owned(),
					namespace: text(&value, "namespace")?.to_owned(),
					image: text(&value, "image")?.to_owned(),
					flags: value["flags"].as_i64().context("Class flags missing")?,
					object: value["object"] == true,
					value_type: value["value_type"] == true,
					enum_kind: value["enum"].as_i64(),
					parent: value["parent"].as_str().map(str::to_owned),
					fields,
				},
			);
		}
		Ok(Self {
			classes,
			provenance: json!({"runtime_metadata": pin(path)?, "generator": "ark-unity-runtime-reflection-v1"}),
		})
	}

	fn class(&self, key: &str) -> Result<&Class> {
		self.classes
			.get(key)
			.with_context(|| format!("Original runtime metadata lacks class {key}"))
	}

	/// The script's MonoBehaviour description; the original header replaces its first four.
	pub fn generate(&self, fullname: &str) -> Result<Node> {
		let class: &Class = self.class(fullname)?;
		let mut root: Node = node("MonoBehaviour", "Base");
		for name in ["m_GameObject", "m_Enabled", "m_Script", "m_Name"] {
			root.children.push(node("", name));
		}
		root.children.extend(self.fields(fullname, class, 0)?);
		Ok(root)
	}

	/// Inherited fields first, then the class's own in declaration order.
	fn fields(&self, key: &str, class: &Class, depth: usize) -> Result<Vec<Node>> {
		let mut nodes: Vec<Node> = Vec::new();
		if let Some(parent) = &class.parent {
			let base: &Class = self.class(parent)?;
			if !base.engine_base() && !base.system() {
				nodes.extend(self.fields(parent, base, depth)?);
			}
		}
		let fields: &[Field] = class
			.fields
			.as_deref()
			.with_context(|| format!("Original runtime metadata lacks the fields of {key}"))?;
		for field in fields.iter().filter(|field| field.serialized()) {
			ensure!(
				!field.serialize_reference,
				"SerializeReference field {key}.{} is unsupported",
				field.name
			);
			let value: Option<Node> = if field.kind == kind::SZARRAY {
				self.array(
					&field.name,
					field.element_kind,
					field.element.as_deref(),
					depth,
				)?
			} else {
				self.value(&field.name, field.kind, field.class.as_deref(), depth)?
			};
			nodes.extend(value);
		}
		Ok(nodes)
	}

	fn array(
		&self,
		name: &str,
		element_kind: Option<i64>,
		element: Option<&str>,
		depth: usize,
	) -> Result<Option<Node>> {
		let element_kind: i64 = element_kind.context("Array field without its element")?;
		// Nested collections never serialize.
		if element_kind == kind::SZARRAY
			|| element
				.map(|key| self.class(key))
				.transpose()?
				.is_some_and(Class::list)
		{
			return Ok(None);
		}
		Ok(self
			.value("data", element_kind, element, depth)?
			.map(|data| array(name, data)))
	}

	fn value(
		&self,
		name: &str,
		kind: i64,
		class: Option<&str>,
		depth: usize,
	) -> Result<Option<Node>> {
		if let Some(node) = primitive(name, kind) {
			return Ok(Some(node));
		}
		if !matches!(kind, kind::VALUETYPE | kind::CLASS | kind::GENERICINST) {
			return Ok(None);
		}
		let key: &str = class.context("Field type without its class")?;
		let target: &Class = self.class(key)?;
		if let Some(base) = target.enum_kind {
			return Ok(primitive(name, base));
		}
		if target.object {
			return Ok(Some(pptr(name, &target.name)));
		}
		if target.namespace == "UnityEngine" {
			if let Some(node) = builtin(name, &target.name) {
				return Ok(Some(node));
			}
		}
		if target.list() {
			let items: &Field = target
				.fields
				.as_deref()
				.into_iter()
				.flatten()
				.find(|field| field.name == "_items" && field.kind == kind::SZARRAY)
				.with_context(|| format!("List {key} has no original item array"))?;
			return self.array(name, items.element_kind, items.element.as_deref(), depth);
		}
		if target.flags & TYPE_SERIALIZABLE == 0
			|| target.flags & TYPE_ABSTRACT_OR_INTERFACE != 0
			|| target.system()
			|| (!target.value_type && depth >= MAX_CLASS_DEPTH)
		{
			return Ok(None);
		}
		let mut composite: Node = node(&target.name, name);
		composite.children = self.fields(key, target, depth + 1)?;
		Ok(Some(composite))
	}
}

fn node(type_name: &str, name: &str) -> Node {
	let mut node: Node = Node::with_info(type_name.to_owned(), name.to_owned(), 0);
	node.version = 1;
	node
}

fn aligned(mut node: Node) -> Node {
	node.meta_flags |= ALIGN;
	node
}

fn array(name: &str, data: Node) -> Node {
	let mut items: Node = aligned(node("Array", "Array"));
	items.type_flags = 1;
	items.children = vec![node("int", "size"), data];
	let mut vector: Node = node("vector", name);
	vector.children.push(items);
	vector
}

fn pptr(name: &str, class: &str) -> Node {
	let mut pointer: Node = node(&format!("PPtr<${class}>"), name);
	pointer.children = vec![node("int", "m_FileID"), node("SInt64", "m_PathID")];
	pointer
}

/// Primitive fields; one- and two-byte values align the stream after them.
fn primitive(name: &str, kind: i64) -> Option<Node> {
	let (type_name, align): (&str, bool) = match kind {
		kind::BOOLEAN | kind::U1 => ("UInt8", true),
		kind::I1 => ("SInt8", true),
		kind::CHAR | kind::U2 => ("UInt16", true),
		kind::I2 => ("SInt16", true),
		kind::I4 => ("int", false),
		kind::U4 => ("unsigned int", false),
		kind::I8 => ("SInt64", false),
		kind::U8 => ("UInt64", false),
		kind::R4 => ("float", false),
		kind::R8 => ("double", false),
		kind::STRING => {
			let mut characters: Node = aligned(node("Array", "Array"));
			characters.type_flags = 1;
			characters.children = vec![node("int", "size"), node("char", "data")];
			let mut string: Node = node("string", name);
			string.children.push(characters);
			return Some(string);
		}
		_ => return None,
	};
	let value: Node = node(type_name, name);
	Some(if align { aligned(value) } else { value })
}

/// Engine value types that serialize through their own native shape.
fn builtin(name: &str, class: &str) -> Option<Node> {
	let leaves = |type_name: &str, names: &[&str]| -> Vec<Node> {
		names.iter().map(|leaf| node(type_name, leaf)).collect()
	};
	let children: Vec<Node> = match class {
		"Vector2" => leaves("float", &["x", "y"]),
		"Vector3" => leaves("float", &["x", "y", "z"]),
		"Vector4" | "Quaternion" => leaves("float", &["x", "y", "z", "w"]),
		"Color" => leaves("float", &["r", "g", "b", "a"]),
		"Rect" => leaves("float", &["x", "y", "width", "height"]),
		"Vector2Int" => leaves("int", &["x", "y"]),
		"Vector3Int" => leaves("int", &["x", "y", "z"]),
		"RectOffset" => leaves("int", &["m_Left", "m_Right", "m_Top", "m_Bottom"]),
		"LayerMask" => leaves("unsigned int", &["m_Bits"]),
		"Color32" => leaves("unsigned int", &["rgba"]),
		"Bounds" => vec![
			builtin("m_Center", "Vector3")?,
			builtin("m_Extent", "Vector3")?,
		],
		"AnimationCurve" => {
			let mut keyframe: Node = node("Keyframe", "data");
			keyframe.children = leaves("float", &["time", "value", "inSlope", "outSlope"]);
			keyframe.children.push(node("int", "weightedMode"));
			keyframe
				.children
				.extend(leaves("float", &["inWeight", "outWeight"]));
			let mut children: Vec<Node> = vec![array("m_Curve", keyframe)];
			children.extend(leaves(
				"int",
				&["m_PreInfinity", "m_PostInfinity", "m_RotationOrder"],
			));
			children
		}
		_ => return None,
	};
	let mut value: Node = node(class, name);
	value.children = children;
	Some(value)
}

#[cfg(test)]
mod tests {
	use super::*;
	fn export(lines: &[Value]) -> Reflection {
		let path: PathBuf = std::env::temp_dir().join(format!(
			"ark-reflection-{}-{}.jsonl",
			std::process::id(),
			chrono::Utc::now().timestamp_nanos_opt().unwrap()
		));
		let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
		fs::write(&path, text).unwrap();
		let reflection: Reflection = Reflection::load(&path).unwrap();
		fs::remove_file(&path).unwrap();
		reflection
	}

	fn class(
		key: &str,
		name: &str,
		namespace: &str,
		image: &str,
		flags: i64,
		object: bool,
	) -> Value {
		json!({"key": key, "name": name, "namespace": namespace, "image": image, "flags": flags,
			"object": object, "value_type": false})
	}

	fn shape(node: &Node, out: &mut Vec<(String, String, i32)>) {
		out.push((node.type_name.clone(), node.name.clone(), node.meta_flags));
		for child in &node.children {
			shape(child, out);
		}
	}

	#[test]
	fn runtime_metadata_follows_unity_monobehaviour_serialization() {
		let mut base = class("Game.Base", "Base", "Game", "Assembly-CSharp.dll", 0, true);
		base["parent"] = json!("UnityEngine.MonoBehaviour");
		base["fields"] = json!([{"name": "inherited", "flags": 6, "kind": 8}]);
		let mut script = class(
			"Game.Script",
			"Script",
			"Game",
			"Assembly-CSharp.dll",
			0,
			true,
		);
		script["parent"] = json!("Game.Base");
		script["fields"] = json!([
			{"name": "flag", "flags": 6, "kind": 2},
			{"name": "hidden", "flags": 1, "kind": 8},
			{"name": "kept", "flags": 1, "serialize_field": true, "kind": 3},
			{"name": "constant", "flags": 0x56, "kind": 8},
			{"name": "frozen", "flags": 0x26, "kind": 8},
			{"name": "skipped", "flags": 0x86, "kind": 8},
			{"name": "mode", "flags": 6, "kind": 0x11, "class": "Game.Mode"},
			{"name": "target", "flags": 6, "kind": 0x12, "class": "UnityEngine.Sprite"},
			{"name": "names", "flags": 6, "kind": 0x15, "class": "System.Collections.Generic.List<System.String>"},
			{"name": "bytes", "flags": 6, "kind": 0x1d, "element_kind": 5, "element": "System.Byte"},
			{"name": "nested", "flags": 6, "kind": 0x1d, "element_kind": 0x1d, "element": "System.Int32[]"},
			{"name": "entry", "flags": 6, "kind": 0x12, "class": "Game.Entry"},
			{"name": "table", "flags": 6, "kind": 0x15, "class": "System.Collections.Generic.Dictionary<System.String,System.String>"},
			{"name": "offset", "flags": 6, "kind": 0x11, "class": "UnityEngine.Vector2"}
		]);
		let mut mode = class(
			"Game.Mode",
			"Mode",
			"Game",
			"Assembly-CSharp.dll",
			0x101,
			false,
		);
		mode["value_type"] = json!(true);
		mode["enum"] = json!(8);
		let mut list = class(
			"System.Collections.Generic.List<System.String>",
			"List`1",
			"System.Collections.Generic",
			"mscorlib.dll",
			0x2001,
			false,
		);
		list["fields"] = json!([{"name": "_items", "flags": 1, "kind": 0x1d, "element_kind": 0x0e,
			"element": "System.String"}]);
		let mut entry = class(
			"Game.Entry",
			"Entry",
			"Game",
			"Assembly-CSharp.dll",
			0x2001,
			false,
		);
		entry["parent"] = json!("System.Object");
		entry["fields"] = json!([{"name": "label", "flags": 6, "kind": 0x0e}]);
		let mut vector = class(
			"UnityEngine.Vector2",
			"Vector2",
			"UnityEngine",
			"UnityEngine.CoreModule.dll",
			0x2109,
			false,
		);
		vector["value_type"] = json!(true);
		let reflection: Reflection = export(&[
			class(
				"UnityEngine.MonoBehaviour",
				"MonoBehaviour",
				"UnityEngine",
				"UnityEngine.CoreModule.dll",
				0,
				true,
			),
			class(
				"System.Object",
				"Object",
				"System",
				"mscorlib.dll",
				0x2001,
				false,
			),
			base,
			script,
			mode,
			class(
				"UnityEngine.Sprite",
				"Sprite",
				"UnityEngine",
				"UnityEngine.CoreModule.dll",
				0,
				true,
			),
			list,
			class(
				"System.String",
				"String",
				"System",
				"mscorlib.dll",
				0x2101,
				false,
			),
			class(
				"System.Byte",
				"Byte",
				"System",
				"mscorlib.dll",
				0x2109,
				false,
			),
			class(
				"System.Int32[]",
				"Int32[]",
				"System",
				"mscorlib.dll",
				0x2101,
				false,
			),
			entry,
			class(
				"System.Collections.Generic.Dictionary<System.String,System.String>",
				"Dictionary`2",
				"System.Collections.Generic",
				"mscorlib.dll",
				0x2001,
				false,
			),
			vector,
		]);
		let generated: Node = reflection.generate("Game.Script").unwrap();
		let names: Vec<&str> = generated.children[4..]
			.iter()
			.map(|node| node.name.as_str())
			.collect();
		assert_eq!(
			names,
			[
				"inherited",
				"flag",
				"kept",
				"mode",
				"target",
				"names",
				"bytes",
				"entry",
				"offset"
			]
		);
		let mut nodes: Vec<(String, String, i32)> = Vec::new();
		for node in &generated.children[4..] {
			shape(node, &mut nodes);
		}
		let expected: Vec<(&str, &str, i32)> = vec![
			("int", "inherited", 0),
			("UInt8", "flag", ALIGN),
			("UInt16", "kept", ALIGN),
			("int", "mode", 0),
			("PPtr<$Sprite>", "target", 0),
			("int", "m_FileID", 0),
			("SInt64", "m_PathID", 0),
			("vector", "names", 0),
			("Array", "Array", ALIGN),
			("int", "size", 0),
			("string", "data", 0),
			("Array", "Array", ALIGN),
			("int", "size", 0),
			("char", "data", 0),
			("vector", "bytes", 0),
			("Array", "Array", ALIGN),
			("int", "size", 0),
			("UInt8", "data", ALIGN),
			("Entry", "entry", 0),
			("string", "label", 0),
			("Array", "Array", ALIGN),
			("int", "size", 0),
			("char", "data", 0),
			("Vector2", "offset", 0),
			("float", "x", 0),
			("float", "y", 0),
		];
		assert_eq!(
			nodes,
			expected
				.into_iter()
				.map(|(ty, name, meta)| (ty.to_owned(), name.to_owned(), meta))
				.collect::<Vec<_>>()
		);
		assert!(reflection.generate("Game.Absent").is_err());
	}

	#[test]
	fn serialize_reference_fields_are_refused() {
		let mut script = class(
			"Game.Script",
			"Script",
			"Game",
			"Assembly-CSharp.dll",
			0,
			true,
		);
		script["parent"] = json!("UnityEngine.MonoBehaviour");
		script["fields"] = json!([{"name": "shared", "flags": 1, "serialize_reference": true,
			"kind": 0x12, "class": "Game.Script"}]);
		let reflection: Reflection = export(&[
			class(
				"UnityEngine.MonoBehaviour",
				"MonoBehaviour",
				"UnityEngine",
				"UnityEngine.CoreModule.dll",
				0,
				true,
			),
			script,
		]);
		assert!(reflection.generate("Game.Script").is_err());
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
