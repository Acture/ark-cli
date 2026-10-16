//! Unity 2021 SerializedFile v22 metadata and a bounded release TypeTree codec.
//! Object payloads are retained verbatim; only explicitly owned manager objects are encoded.

use super::bundle::Reader;
use super::UNITY_VERSION;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use unity_asset_binary::typetree::TypeTreeNode;

pub(crate) type Node = TypeTreeNode;
pub(crate) type Pointer = (i32, i64);
pub(crate) type MonoHead = (String, Pointer, Pointer);

#[derive(Debug, Clone)]
pub(crate) struct Type {
	pub class: i32,
	pub stripped: u8,
	pub script_index: u16,
	pub script_id: Option<[u8; 16]>,
	pub hash: [u8; 16],
	pub tree: Option<Node>,
	pub dependencies: Vec<i32>,
	pub reference_names: [String; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Object {
	pub id: i64,
	pub type_index: usize,
	pub raw: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct External {
	pub empty: String,
	pub guid: [u8; 16],
	pub kind: i32,
	pub path: String,
}

#[derive(Debug, Clone)]
pub(crate) struct File {
	pub types: Vec<Type>,
	pub objects: Vec<Object>,
	pub externals: Vec<External>,
	pub scripts: Vec<(i32, i64)>,
	pub reference_types: Vec<Type>,
	pub enable_trees: bool,
	platform: i32,
	unknown: u64,
	user: String,
}

impl File {
	pub fn parse(data: &[u8]) -> Result<Self> {
		ensure!(
			data.len() <= 512 * 1024 * 1024,
			"SerializedFile exceeds bounded input size"
		);
		let mut h: Reader<'_> = Reader::new(data, true);
		h.take(8)?;
		ensure!(h.u32()? == 22, "Expected SerializedFile v22");
		h.take(4)?;
		ensure!(
			h.u8()? == 0 && h.take(3)? == [0, 0, 0],
			"Expected little-endian SerializedFile"
		);
		let metadata: usize = h.u32()? as usize;
		ensure!(
			h.u64()? == data.len() as u64,
			"SerializedFile length differs"
		);
		let data_offset: usize = usize::try_from(h.u64()?)?;
		let unknown: u64 = h.u64()?;
		ensure!(
			metadata <= data.len()
				&& metadata
					.checked_add(48)
					.is_some_and(|end| end <= data_offset)
				&& data_offset <= data.len(),
			"SerializedFile metadata outside buffer"
		);
		let mut r: Reader<'_> = Reader::new(
			data.get(
				48..48_usize
					.checked_add(metadata)
					.context("Metadata overflow")?,
			)
			.context("Truncated metadata")?,
			false,
		);
		ensure!(
			r.zstring(128)? == UNITY_VERSION,
			"Unexpected SerializedFile Unity version"
		);
		let platform: i32 = r.i32()?;
		let enable_trees: bool = match r.u8()? {
			0 => false,
			1 => true,
			_ => bail!("Invalid tree-enable flag"),
		};
		let count: usize = r.count(65536)?;
		let mut types: Vec<Type> = Vec::with_capacity(count);
		for _ in 0..count {
			types.push(read_type(&mut r, enable_trees, false)?);
		}
		let count: usize = r.count(4_000_000)?;
		let mut objects: Vec<Object> = Vec::with_capacity(count.min(data.len() / 24));
		let mut ids: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
		let mut ranges: BTreeMap<usize, usize> = BTreeMap::new();
		for _ in 0..count {
			r.align(4)?;
			let id: i64 = r.i64()?;
			ensure!(ids.insert(id), "Duplicate SerializedFile path ID {id}");
			let start: usize = data_offset
				.checked_add(usize::try_from(r.u64()?)?)
				.context("Object start overflow")?;
			let size: usize = r.u32()? as usize;
			let type_index: usize = r.u32()? as usize;
			ensure!(
				type_index < types.len(),
				"Invalid SerializedFile type index"
			);
			let end: usize = start.checked_add(size).context("Object end overflow")?;
			if size > 0 {
				if let Some((previous_start, previous_end)) = ranges.range(..=start).next_back() {
					ensure!(
					*previous_end <= start,
					"Overlapping serialized object payloads: object {id} {start}..{end}, previous {previous_start}..{previous_end}"
				);
				}
				if let Some((next_start, _)) = ranges.range(start..).next() {
					ensure!(end <= *next_start, "Overlapping serialized object payloads");
				}
				ranges.insert(start, end);
			}
			objects.push(Object {
				id,
				type_index,
				raw: data
					.get(start..end)
					.context("Object outside SerializedFile")?
					.to_vec(),
			});
		}
		let count: usize = r.count(1_000_000)?;
		let mut scripts: Vec<(i32, i64)> = Vec::new();
		for _ in 0..count {
			let index: i32 = r.i32()?;
			r.align(4)?;
			scripts.push((index, r.i64()?));
		}
		let count: usize = r.count(65536)?;
		let mut externals: Vec<External> = Vec::new();
		for _ in 0..count {
			externals.push(External {
				empty: r.zstring(4096)?,
				guid: r.take(16)?.try_into()?,
				kind: r.i32()?,
				path: r.zstring(4096)?,
			});
		}
		let count: usize = r.count(65536)?;
		let mut reference_types: Vec<Type> = Vec::new();
		for _ in 0..count {
			reference_types.push(read_type(&mut r, enable_trees, true)?);
		}
		let user: String = r.zstring(1_048_576)?;
		ensure!(
			r.pos == metadata,
			"Unclaimed SerializedFile metadata: {} of {metadata}",
			r.pos
		);
		Ok(Self {
			types,
			objects,
			externals,
			scripts,
			reference_types,
			enable_trees,
			platform,
			unknown,
			user,
		})
	}

	pub fn class(&self, obj: &Object) -> i32 {
		self.types[obj.type_index].class
	}
	pub fn object(&self, id: i64) -> Result<&Object> {
		self.objects
			.iter()
			.find(|obj| obj.id == id)
			.with_context(|| format!("Missing serialized object {id}"))
	}
	pub fn one_class(&self, class: i32) -> Result<&Object> {
		let found: Vec<&Object> = self
			.objects
			.iter()
			.filter(|obj| self.class(obj) == class)
			.collect();
		ensure!(
			found.len() == 1,
			"Expected one class {class}, found {}",
			found.len()
		);
		Ok(found[0])
	}
	pub fn decode(&self, obj: &Object) -> Result<Value> {
		let tree: &Node = self.types[obj.type_index]
			.tree
			.as_ref()
			.context("Serialized object has no field description")?;
		decode(tree, &obj.raw)
			.with_context(|| format!("Decode class {}, object {}", self.class(obj), obj.id))
	}
	pub fn replace(&mut self, id: i64, value: &Value) -> Result<()> {
		let obj: &Object = self.object(id)?;
		let tree: &Node = self.types[obj.type_index]
			.tree
			.as_ref()
			.context("Owned object has no field description")?;
		let raw: Vec<u8> = encode(tree, value)?;
		self.objects
			.iter_mut()
			.find(|obj| obj.id == id)
			.context("Object disappeared")?
			.raw = raw;
		Ok(())
	}
	pub fn external(&mut self, path: &str) -> Result<i32> {
		if let Some(index) = self.externals.iter().position(|entry| entry.path == path) {
			return Ok(i32::try_from(index + 1)?);
		}
		self.externals.push(External {
			empty: String::new(),
			guid: [0; 16],
			kind: 0,
			path: path.to_owned(),
		});
		Ok(i32::try_from(self.externals.len())?)
	}
	pub fn save(&self) -> Result<Vec<u8>> {
		let mut metadata: Vec<u8> = Vec::new();
		zstring(&mut metadata, UNITY_VERSION)?;
		i32le(&mut metadata, self.platform);
		metadata.push(u8::from(self.enable_trees));
		count(&mut metadata, self.types.len())?;
		for value in &self.types {
			write_type(&mut metadata, value, self.enable_trees, false)?;
		}
		count(&mut metadata, self.objects.len())?;
		let mut data: Vec<u8> = Vec::new();
		for obj in &self.objects {
			align(&mut metadata, 4);
			align(&mut data, 8);
			metadata.extend(obj.id.to_le_bytes());
			metadata.extend((data.len() as u64).to_le_bytes());
			count(&mut metadata, obj.raw.len())?;
			count(&mut metadata, obj.type_index)?;
			data.extend_from_slice(&obj.raw);
		}
		count(&mut metadata, self.scripts.len())?;
		for (index, id) in &self.scripts {
			i32le(&mut metadata, *index);
			align(&mut metadata, 4);
			metadata.extend(id.to_le_bytes());
		}
		count(&mut metadata, self.externals.len())?;
		for ext in &self.externals {
			zstring(&mut metadata, &ext.empty)?;
			metadata.extend(ext.guid);
			i32le(&mut metadata, ext.kind);
			zstring(&mut metadata, &ext.path)?;
		}
		count(&mut metadata, self.reference_types.len())?;
		for value in &self.reference_types {
			write_type(&mut metadata, value, self.enable_trees, true)?;
		}
		zstring(&mut metadata, &self.user)?;
		let data_offset: usize = (48 + metadata.len() + 15) & !15;
		let file_size: usize = data_offset
			.checked_add(data.len())
			.context("SerializedFile size overflow")?;
		let mut result: Vec<u8> = vec![0; 8];
		result.extend(22_u32.to_be_bytes());
		result.extend([0; 8]);
		result.extend(u32::try_from(metadata.len())?.to_be_bytes());
		result.extend((file_size as u64).to_be_bytes());
		result.extend((data_offset as u64).to_be_bytes());
		result.extend(self.unknown.to_be_bytes());
		result.extend(metadata);
		result.resize(data_offset, 0);
		result.extend(data);
		let checked: File = Self::parse(&result)?;
		self.verify_objects(&checked)
			.context("Serialized save changed object payload or identity")?;
		let independent =
			unity_asset_binary::asset::SerializedFileParser::from_bytes(result.clone())
				.context("Independent SerializedFile decoder rejected output")?;
		ensure!(
			independent.objects.len() == self.objects.len(),
			"Independent object count differs"
		);
		for (wire, original) in independent.objects.iter().zip(&self.objects) {
			ensure!(
				wire.path_id == original.id
					&& wire.type_index == i32::try_from(original.type_index)?
					&& independent.object_bytes(wire)? == original.raw,
				"Independent decoder found changed object {}",
				original.id
			);
		}
		Ok(result)
	}
	pub fn verify_objects(&self, other: &Self) -> Result<()> {
		ensure!(
			self.objects == other.objects,
			"Original object payloads changed"
		);
		Ok(())
	}
}

fn read_type(r: &mut Reader<'_>, enabled: bool, reference: bool) -> Result<Type> {
	let class: i32 = r.i32()?;
	let stripped: u8 = r.u8()?;
	let script_index: u16 = r.u16()?;
	let script_id: Option<[u8; 16]> =
		if (!reference && class == 114) || (reference && script_index != u16::MAX) {
			Some(r.take(16)?.try_into()?)
		} else {
			None
		};
	let hash: [u8; 16] = r.take(16)?.try_into()?;
	let tree: Option<Node> = if enabled { Some(read_tree(r)?) } else { None };
	let mut dependencies: Vec<i32> = Vec::new();
	let mut reference_names: [String; 3] = Default::default();
	if enabled {
		if reference {
			for name in &mut reference_names {
				*name = r.zstring(4096)?;
			}
		} else {
			let n: usize = r.count(65536)?;
			for _ in 0..n {
				dependencies.push(r.i32()?);
			}
		}
	}
	Ok(Type {
		class,
		stripped,
		script_index,
		script_id,
		hash,
		tree,
		dependencies,
		reference_names,
	})
}

fn write_type(out: &mut Vec<u8>, value: &Type, enabled: bool, reference: bool) -> Result<()> {
	i32le(out, value.class);
	out.push(value.stripped);
	out.extend(value.script_index.to_le_bytes());
	if let Some(script) = value.script_id {
		out.extend(script);
	}
	out.extend(value.hash);
	if enabled {
		write_tree(
			out,
			value.tree.as_ref().context("Missing restored TypeTree")?,
		)?;
		if reference {
			for name in &value.reference_names {
				zstring(out, name)?;
			}
		} else {
			count(out, value.dependencies.len())?;
			for d in &value.dependencies {
				i32le(out, *d);
			}
		}
	}
	Ok(())
}

fn read_tree(r: &mut Reader<'_>) -> Result<Node> {
	let count: usize = r.count(1_000_000)?;
	let string_size: usize = r.count(16 * 1024 * 1024)?;
	ensure!(count > 0, "Empty embedded TypeTree");
	let wire: &[u8] = r.take(
		count
			.checked_mul(32)
			.context("TypeTree node size overflow")?,
	)?;
	let strings: &[u8] = r.take(string_size)?;
	let mut nodes: Vec<Node> = Vec::with_capacity(count);
	let mut n: Reader<'_> = Reader::new(wire, false);
	for _ in 0..count {
		let version: i32 = i32::from(n.u16()?);
		let level: i32 = i32::from(n.u8()?);
		let flags: i32 = i32::from(n.u8()?);
		let type_name: String = tree_string(strings, n.u32()?)?;
		let name: String = tree_string(strings, n.u32()?)?;
		let mut node: Node = Node::with_info(type_name, name, n.i32()?);
		node.index = n.i32()?;
		node.meta_flags = n.i32()?;
		node.ref_type_hash = n.u64()?;
		node.version = version;
		node.level = level;
		node.type_flags = flags;
		nodes.push(node);
	}
	from_flat(nodes)
}

pub(crate) fn from_flat(nodes: Vec<Node>) -> Result<Node> {
	fn build(nodes: &[Node], position: &mut usize, depth: i32) -> Result<Node> {
		ensure!(depth <= 256, "TypeTree depth exceeds bound");
		let mut node: Node = nodes
			.get(*position)
			.context("Missing TypeTree root")?
			.clone();
		ensure!(node.level == depth, "TypeTree level jumps at {}", *position);
		*position += 1;
		while nodes.get(*position).is_some_and(|next| next.level > depth) {
			node.children.push(build(nodes, position, depth + 1)?);
		}
		Ok(node)
	}
	let mut position: usize = 0;
	let root: Node = build(&nodes, &mut position, 0)?;
	ensure!(position == nodes.len(), "Multiple TypeTree roots");
	Ok(root)
}

fn flatten<'a>(node: &'a Node, nodes: &mut Vec<(&'a Node, u8)>, level: u8) -> Result<()> {
	nodes.push((node, level));
	for child in &node.children {
		flatten(
			child,
			nodes,
			level.checked_add(1).context("TypeTree too deep")?,
		)?;
	}
	Ok(())
}

fn write_tree(out: &mut Vec<u8>, root: &Node) -> Result<()> {
	let mut flat: Vec<(&Node, u8)> = Vec::new();
	flatten(root, &mut flat, 0)?;
	let mut strings: Vec<u8> = Vec::new();
	let mut offsets: BTreeMap<&str, u32> = BTreeMap::new();
	for (node, _) in &flat {
		for name in [&node.type_name, &node.name] {
			if !offsets.contains_key(name.as_str()) {
				offsets.insert(name, u32::try_from(strings.len())?);
				zstring(&mut strings, name)?;
			}
		}
	}
	count(out, flat.len())?;
	count(out, strings.len())?;
	for (index, (node, level)) in flat.iter().enumerate() {
		out.extend(u16::try_from(node.version)?.to_le_bytes());
		out.push(*level);
		out.push(u8::try_from(node.type_flags)?);
		out.extend(offsets[node.type_name.as_str()].to_le_bytes());
		out.extend(offsets[node.name.as_str()].to_le_bytes());
		i32le(out, node.byte_size);
		i32le(out, i32::try_from(index)?);
		i32le(out, node.meta_flags);
		out.extend(node.ref_type_hash.to_le_bytes());
	}
	out.extend(strings);
	Ok(())
}

// Unity's published global TypeTree string buffer. The ordered table matches
// unity-asset 0.3.0 typetree/common_strings.rs (MIT, Mingzhen Zhuang).
fn tree_string(strings: &[u8], offset: u32) -> Result<String> {
	if offset & 0x8000_0000 == 0 {
		let mut r: Reader<'_> = Reader::new(strings, false);
		r.pos = offset as usize;
		return r.zstring(4096);
	}
	let names: &str = "AABB\0AnimationClip\0AnimationCurve\0AnimationState\0Array\0Base\0BitField\0bitset\0bool\0char\0ColorRGBA\0Component\0data\0deque\0double\0dynamic_array\0FastPropertyName\0first\0float\0Font\0GameObject\0Generic Mono\0GradientNEW\0GUID\0GUIStyle\0int\0list\0long long\0map\0Matrix4x4f\0MdFour\0MonoBehaviour\0MonoScript\0m_ByteSize\0m_Curve\0m_EditorClassIdentifier\0m_EditorHideFlags\0m_Enabled\0m_ExtensionPtr\0m_GameObject\0m_Index\0m_IsArray\0m_IsStatic\0m_MetaFlag\0m_Name\0m_ObjectHideFlags\0m_PrefabInternal\0m_PrefabParentObject\0m_Script\0m_StaticEditorFlags\0m_Type\0m_Version\0Object\0pair\0PPtr<Component>\0PPtr<GameObject>\0PPtr<Material>\0PPtr<MonoBehaviour>\0PPtr<MonoScript>\0PPtr<Object>\0PPtr<Prefab>\0PPtr<Sprite>\0PPtr<TextAsset>\0PPtr<Texture>\0PPtr<Texture2D>\0PPtr<Transform>\0Prefab\0Quaternionf\0Rectf\0RectInt\0RectOffset\0second\0set\0short\0size\0SInt16\0SInt32\0SInt64\0SInt8\0staticvector\0string\0TextAsset\0TextMesh\0Texture\0Texture2D\0Transform\0TypelessData\0UInt16\0UInt32\0UInt64\0UInt8\0unsigned int\0unsigned long long\0unsigned short\0vector\0Vector2f\0Vector3f\0Vector4f\0m_ScriptingClassIdentifier\0Gradient\0Type*\0int2_storage\0int3_storage\0BoundsInt\0m_CorrespondingSourceObject\0m_PrefabInstance\0m_PrefabAsset\0FileSize\0Hash128\0RenderingLayerMask\0fixed_array\0";
	let position: usize = (offset & 0x7fff_ffff) as usize;
	ensure!(
		position == 0 || names.as_bytes().get(position.wrapping_sub(1)) == Some(&0),
		"Common TypeTree string offset is not a boundary"
	);
	let mut r: Reader<'_> = Reader::new(names.as_bytes(), false);
	r.pos = position;
	r.zstring(4096)
}

pub(crate) fn align(out: &mut Vec<u8>, width: usize) {
	while !out.len().is_multiple_of(width) {
		out.push(0);
	}
}
pub(crate) fn i32le(out: &mut Vec<u8>, value: i32) {
	out.extend(value.to_le_bytes());
}
pub(crate) fn count(out: &mut Vec<u8>, value: usize) -> Result<()> {
	out.extend(u32::try_from(value)?.to_le_bytes());
	Ok(())
}
pub(crate) fn zstring(out: &mut Vec<u8>, value: &str) -> Result<()> {
	ensure!(!value.as_bytes().contains(&0), "NUL in serialized string");
	out.extend(value.as_bytes());
	out.push(0);
	Ok(())
}

pub(crate) fn pointer(value: &Value) -> Result<(i32, i64)> {
	Ok((
		i32::try_from(
			value["m_FileID"]
				.as_i64()
				.context("Missing pointer file ID")?,
		)?,
		value["m_PathID"]
			.as_i64()
			.context("Missing pointer path ID")?,
	))
}

pub(crate) fn mono_head(raw: &[u8]) -> Result<MonoHead> {
	let mut r: Reader<'_> = Reader::new(raw, false);
	let game: (i32, i64) = (r.i32()?, r.i64()?);
	r.u8()?;
	r.align(4)?;
	let script: (i32, i64) = (r.i32()?, r.i64()?);
	let name: String = r.string()?;
	Ok((name, script, game))
}

pub(crate) fn text_asset(raw: &[u8]) -> Result<(String, Vec<u8>)> {
	let mut r: Reader<'_> = Reader::new(raw, false);
	let name: String = r.string()?;
	let size: usize = r.count(256 * 1024 * 1024)?;
	let script: Vec<u8> = r.take(size)?.to_vec();
	r.align(4)?;
	ensure!(r.pos == raw.len(), "Unclaimed TextAsset payload");
	Ok((name, script))
}

#[derive(Debug, Clone)]
pub(crate) struct Script {
	pub namespace: String,
	pub class: String,
	pub assembly: String,
}

pub(crate) fn mono_script(raw: &[u8]) -> Result<Script> {
	let mut r: Reader<'_> = Reader::new(raw, false);
	r.string()?;
	r.i32()?;
	r.take(16)?;
	let class: String = r.string()?;
	let namespace: String = r.string()?;
	let assembly: String = r.string()?;
	ensure!(r.pos == raw.len(), "Unclaimed MonoScript serialized shape");
	Ok(Script {
		namespace,
		class,
		assembly,
	})
}

fn array_node(node: &Node) -> Option<&Node> {
	if node.type_name == "Array" {
		Some(node)
	} else if node.children.len() == 1 && node.children[0].type_name == "Array" {
		Some(&node.children[0])
	} else {
		None
	}
}

pub(crate) fn decode(tree: &Node, raw: &[u8]) -> Result<Value> {
	let mut r: Reader<'_> = Reader::new(raw, false);
	let value: Value = read_value(tree, &mut r, 0)?;
	ensure!(
		r.pos == raw.len(),
		"TypeTree leaves {} unclaimed object bytes",
		raw.len().saturating_sub(r.pos)
	);
	Ok(value)
}

fn read_value(node: &Node, r: &mut Reader<'_>, depth: usize) -> Result<Value> {
	ensure!(depth <= 256, "TypeTree value nesting exceeds bound");
	let value: Value =
		match node.type_name.as_str() {
			"TypelessData" => {
				let size: usize = r.count(256 * 1024 * 1024)?;
				json!({"$unity_raw": r.take(size)?})
			}
			"string" => {
				let bytes: Vec<u8> = r.string_bytes()?;
				match String::from_utf8(bytes) {
					Ok(s) => Value::String(s),
					Err(error) => json!({"$unity_bytes": error.into_bytes()}),
				}
			}
			"bool" => {
				let b: u8 = r.u8()?;
				ensure!(b <= 1, "Noncanonical serialized bool");
				Value::Bool(b != 0)
			}
			"char" | "UInt8" | "unsigned char" => json!(r.u8()?),
			"SInt8" => json!(r.u8()? as i8),
			"short" | "SInt16" => json!(r.u16()? as i16),
			"UInt16" | "unsigned short" => json!(r.u16()?),
			"int" | "SInt32" => json!(r.i32()?),
			"UInt32" | "unsigned int" => json!(r.u32()?),
			"long long" | "SInt64" => json!(r.i64()?),
			"UInt64" | "unsigned long long" | "FileSize" => json!(r.u64()?),
			"float" => {
				let bits: u32 = r.u32()?;
				let f: f32 = f32::from_bits(bits);
				if f.is_finite() {
					json!(f)
				} else {
					json!({"$float_bits": bits})
				}
			}
			"double" => {
				let bits: u64 = r.u64()?;
				let f: f64 = f64::from_bits(bits);
				if f.is_finite() {
					json!(f)
				} else {
					json!({"$double_bits": bits})
				}
			}
			_ => {
				if let Some(array) = array_node(node) {
					ensure!(array.children.len() == 2, "Unexpected Array node shape");
					let count: usize = r.count(16_000_000)?;
					ensure!(
						count <= r.data.len().saturating_sub(r.pos) + 1,
						"Array count exceeds remaining byte bound"
					);
					let mut values: Vec<Value> = Vec::with_capacity(count.min(4096));
					for index in 0..count {
						values.push(read_value(&array.children[1], r, depth + 1).with_context(
							|| format!("Array {}[{index}] at {}", node.name, r.pos),
						)?);
					}
					if !std::ptr::eq(array, node) && array.meta_flags & 0x4000 != 0 {
						r.align(4)?;
					}
					Value::Array(values)
				} else if node.type_name == "pair" {
					ensure!(node.children.len() == 2, "Unexpected pair shape");
					Value::Array(vec![
						read_value(&node.children[0], r, depth + 1)?,
						read_value(&node.children[1], r, depth + 1)?,
					])
				} else if !node.children.is_empty() {
					let mut object: Map<String, Value> = Map::new();
					for child in &node.children {
						ensure!(
							!object.contains_key(&child.name),
							"Duplicate TypeTree member {}",
							child.name
						);
						let position: usize = r.pos;
						object.insert(
							child.name.clone(),
							read_value(child, r, depth + 1).with_context(|| {
								format!("Field {} ({}) at {position}", child.name, child.type_name)
							})?,
						);
					}
					Value::Object(object)
				} else if node.byte_size >= 0 {
					json!({"$unity_raw": r.take(node.byte_size as usize)?})
				} else {
					bail!(
						"Unsupported TypeTree leaf {} ({})",
						node.type_name,
						node.name
					);
				}
			}
		};
	if node.meta_flags & 0x4000 != 0 {
		r.align(4)?;
	}
	Ok(value)
}

pub(crate) fn encode(tree: &Node, value: &Value) -> Result<Vec<u8>> {
	let mut out: Vec<u8> = Vec::new();
	write_value(tree, value, &mut out, 0)?;
	Ok(out)
}

fn unsigned(value: &Value) -> Result<u64> {
	value
		.as_u64()
		.context("Expected unsigned serialized integer")
}
fn signed(value: &Value) -> Result<i64> {
	value.as_i64().context("Expected signed serialized integer")
}
fn bytes(value: &Value) -> Result<Vec<u8>> {
	value
		.as_array()
		.context("Expected raw bytes")?
		.iter()
		.map(|v| Ok(u8::try_from(unsigned(v)?)?))
		.collect()
}

fn write_value(node: &Node, value: &Value, out: &mut Vec<u8>, depth: usize) -> Result<()> {
	ensure!(depth <= 256, "TypeTree value nesting exceeds bound");
	match node.type_name.as_str() {
		"TypelessData" => {
			let raw: Vec<u8> = bytes(&value["$unity_raw"])?;
			count(out, raw.len())?;
			out.extend(raw);
		}
		"string" => {
			let raw: Vec<u8> = if let Some(s) = value.as_str() {
				s.as_bytes().to_vec()
			} else {
				bytes(&value["$unity_bytes"])?
			};
			count(out, raw.len())?;
			out.extend(raw);
			align(out, 4);
		}
		"bool" => out.push(u8::from(value.as_bool().context("Expected bool")?)),
		"char" | "UInt8" | "unsigned char" => out.push(u8::try_from(unsigned(value)?)?),
		"SInt8" => out.push(i8::try_from(signed(value)?)? as u8),
		"short" | "SInt16" => out.extend(i16::try_from(signed(value)?)?.to_le_bytes()),
		"UInt16" | "unsigned short" => out.extend(u16::try_from(unsigned(value)?)?.to_le_bytes()),
		"int" | "SInt32" => out.extend(i32::try_from(signed(value)?)?.to_le_bytes()),
		"UInt32" | "unsigned int" => out.extend(u32::try_from(unsigned(value)?)?.to_le_bytes()),
		"long long" | "SInt64" => out.extend(signed(value)?.to_le_bytes()),
		"UInt64" | "unsigned long long" | "FileSize" => out.extend(unsigned(value)?.to_le_bytes()),
		"float" => {
			let bits: u32 = if let Some(bits) = value.get("$float_bits") {
				u32::try_from(unsigned(bits)?)?
			} else {
				(value.as_f64().context("Expected float")? as f32).to_bits()
			};
			out.extend(bits.to_le_bytes());
		}
		"double" => {
			let bits: u64 = if let Some(bits) = value.get("$double_bits") {
				unsigned(bits)?
			} else {
				value.as_f64().context("Expected double")?.to_bits()
			};
			out.extend(bits.to_le_bytes());
		}
		_ => {
			if let Some(array) = array_node(node) {
				ensure!(array.children.len() == 2, "Unexpected Array shape");
				let values: &[Value] = value.as_array().context("Expected serialized array")?;
				count(out, values.len())?;
				for item in values {
					write_value(&array.children[1], item, out, depth + 1)?;
				}
				if !std::ptr::eq(array, node) && array.meta_flags & 0x4000 != 0 {
					align(out, 4);
				}
			} else if node.type_name == "pair" {
				let values: &[Value] = value.as_array().context("Expected pair")?;
				ensure!(
					values.len() == 2 && node.children.len() == 2,
					"Invalid pair"
				);
				for (child, item) in node.children.iter().zip(values) {
					write_value(child, item, out, depth + 1)?;
				}
			} else if !node.children.is_empty() {
				for child in &node.children {
					write_value(
						child,
						value
							.get(&child.name)
							.with_context(|| format!("Missing serialized field {}", child.name))?,
						out,
						depth + 1,
					)?;
				}
			} else {
				let raw: Vec<u8> = bytes(&value["$unity_raw"])?;
				ensure!(
					node.byte_size >= 0 && raw.len() == node.byte_size as usize,
					"Opaque field size differs"
				);
				out.extend(raw);
			}
		}
	}
	if node.meta_flags & 0x4000 != 0 {
		align(out, 4);
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	fn scalar(ty: &str, name: &str, size: i32) -> Node {
		let mut n: Node = Node::with_info(ty.into(), name.into(), size);
		n.version = 1;
		n
	}
	#[test]
	fn full_type_tree_roundtrip_keeps_nan_bits_and_rejects_trailing_bytes() {
		let mut tree: Node = scalar("Example", "Base", -1);
		tree.children = vec![scalar("float", "f", 4), scalar("string", "s", -1)];
		let raw: Vec<u8> = vec![0x45, 0x23, 0xc1, 0x7f, 1, 0, 0, 0, 0xff, 0, 0, 0];
		assert_eq!(encode(&tree, &decode(&tree, &raw).unwrap()).unwrap(), raw);
		let mut bad: Vec<u8> = raw.clone();
		bad.push(0);
		assert!(decode(&tree, &bad).is_err());
		assert!(decode(&tree, &raw[..5]).is_err());
	}
	#[test]
	fn serialized_clone_preserves_original_payloads_and_restored_trees() {
		let tree: Node = scalar("int", "Base", 4);
		let original: File = File {
			types: vec![Type {
				class: 49,
				stripped: 0,
				script_index: u16::MAX,
				script_id: None,
				hash: [3; 16],
				tree: Some(tree),
				dependencies: vec![],
				reference_names: Default::default(),
			}],
			objects: vec![Object {
				id: -42,
				type_index: 0,
				raw: vec![0xff, 0x00, 0x23, 0x45],
			}],
			externals: vec![],
			scripts: vec![],
			reference_types: vec![],
			enable_trees: true,
			platform: 19,
			unknown: 0,
			user: String::new(),
		};
		let encoded: Vec<u8> = original.save().unwrap();
		let reloaded: File = File::parse(&encoded).unwrap();
		original.verify_objects(&reloaded).unwrap();
		assert_eq!(reloaded.types[0].tree.as_ref().unwrap().type_name, "int");
		assert!(File::parse(&encoded[..encoded.len() - 1]).is_err());
		let mut with_empty: File = original;
		with_empty.objects.insert(
			0,
			Object {
				id: -43,
				type_index: 0,
				raw: Vec::new(),
			},
		);
		let empty_encoded: Vec<u8> = with_empty.save().unwrap();
		assert_eq!(
			File::parse(&empty_encoded).unwrap().objects,
			with_empty.objects
		);
	}
}
