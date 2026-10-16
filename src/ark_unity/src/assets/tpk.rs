//! Bounded TPK v1/v2 release registry reader.
//!
//! Format provenance: AssetRipper TPK and tpk-ar 0.2.4 (MIT), shipped with
//! UnityPy 1.25.4. v2 changes only the versioned common-string index; release
//! nodes retain their explicit string-buffer indices and original field sizes.

use super::bundle::Reader;
use super::serialized::Node;
use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};

const TARGET: u64 = (2021 << 48) | (3 << 32) | (39 << 16) | (3 << 8) | 1;

struct WireNode {
	ty: usize,
	name: usize,
	size: i32,
	version: i32,
	flags: i32,
	meta: i32,
	children: Vec<usize>,
}

pub(crate) struct Registry {
	roots: BTreeMap<i32, usize>,
	nodes: Vec<WireNode>,
	strings: Vec<String>,
}

struct LimitedOutput {
	data: Vec<u8>,
	limit: usize,
}
impl Write for LimitedOutput {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		if buf.len() > self.limit.saturating_sub(self.data.len()) {
			return Err(std::io::Error::other(
				"TPK decompressed output exceeds declared size",
			));
		}
		self.data.extend_from_slice(buf);
		Ok(buf.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

impl Registry {
	pub fn from_bytes(raw: &[u8]) -> Result<Self> {
		let mut header: Reader<'_> = Reader::new(raw, false);
		ensure!(header.u32()? == 0x2a4b5054, "Invalid TPK magic");
		let version: u8 = header.u8()?;
		ensure!(
			[1, 2].contains(&version),
			"Unreviewed TPK version {version}"
		);
		let compression: u8 = header.u8()?;
		ensure!(header.u8()? == 0, "Expected TPK TypeTreeInformation");
		header.take(5)?;
		let compressed: usize = header.count(32 * 1024 * 1024)?;
		let size: usize = header.count(256 * 1024 * 1024)?;
		let source: &[u8] = header.take(compressed)?;
		ensure!(header.pos == raw.len(), "TPK trailing compressed bytes");
		let data: Vec<u8> = match compression {
			0 => source.to_vec(),
			1 => lz4_flex::block::decompress(source, size)?,
			2 => {
				let mut output: LimitedOutput = LimitedOutput {
					data: Vec::new(),
					limit: size,
				};
				let options: lzma_rs::decompress::Options = lzma_rs::decompress::Options {
					unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(
						size as u64,
					)),
					memlimit: Some(64 * 1024 * 1024),
					allow_incomplete: false,
				};
				lzma_rs::lzma_decompress_with_options(
					&mut Cursor::new(source),
					&mut output,
					&options,
				)?;
				output.data
			}
			other => bail!("Unsupported TPK compression {other}"),
		};
		ensure!(data.len() == size, "TPK decompressed length differs");
		let mut r: Reader<'_> = Reader::new(&data, false);
		r.take(8)?;
		let count: usize = r.count(65536)?;
		r.take(count.checked_mul(8).context("TPK version table overflow")?)?;
		let count: usize = r.count(65536)?;
		let mut roots: BTreeMap<i32, usize> = BTreeMap::new();
		for _ in 0..count {
			let class: i32 = r.i32()?;
			let versions: usize = r.count(65536)?;
			let mut selected: Option<usize> = None;
			let mut previous: Option<u64> = None;
			for _ in 0..versions {
				let at: u64 = r.u64()?;
				ensure!(
					previous.is_none_or(|previous| previous < at),
					"TPK class versions are unordered"
				);
				previous = Some(at);
				let present: u8 = r.u8()?;
				ensure!(present <= 1, "Invalid TPK class-presence marker");
				let root: Option<usize> = if present == 1 {
					r.take(4)?;
					let flags: u8 = r.u8()?;
					if flags & 64 != 0 {
						r.u16()?;
					}
					if flags & 128 != 0 {
						Some(r.u16()? as usize)
					} else {
						None
					}
				} else {
					None
				};
				if at <= TARGET {
					selected = root;
				}
			}
			if let Some(root) = selected {
				ensure!(roots.insert(class, root).is_none(), "Duplicate TPK class");
			}
		}
		let versions: usize = r.count(65536)?;
		if version == 1 {
			r.take(
				versions
					.checked_mul(9)
					.context("TPK common-string overflow")?,
			)?;
			let indices: usize = r.count(65536)?;
			r.take(
				indices
					.checked_mul(2)
					.context("TPK common-index overflow")?,
			)?;
		} else {
			for _ in 0..versions {
				r.u64()?;
				let entries: usize = r.count(65536)?;
				r.take(
					entries
						.checked_mul(4)
						.context("TPK common-entry overflow")?,
				)?;
			}
		}
		let count: usize = r.count(65536)?;
		let mut nodes: Vec<WireNode> = Vec::with_capacity(count);
		for _ in 0..count {
			let ty: usize = r.u16()? as usize;
			let name: usize = r.u16()? as usize;
			let size: i32 = r.i32()?;
			let version: i32 = r.u16()? as i16 as i32;
			let flags: i32 = r.u8()? as i32;
			let meta: i32 = r.i32()?;
			let count: usize = r.u16()? as usize;
			let mut children: Vec<usize> = Vec::with_capacity(count);
			for _ in 0..count {
				children.push(r.u16()? as usize);
			}
			nodes.push(WireNode {
				ty,
				name,
				size,
				version,
				flags,
				meta,
				children,
			});
		}
		let count: usize = r.count(65536)?;
		let mut strings: Vec<String> = Vec::with_capacity(count);
		for _ in 0..count {
			let mut length: usize = 0;
			let mut terminated: bool = false;
			for shift in (0..35).step_by(7) {
				let byte: u8 = r.u8()?;
				length |= usize::from(byte & 127) << shift;
				if byte & 128 == 0 {
					terminated = true;
					break;
				}
			}
			ensure!(
				terminated && length <= 1_048_576,
				"TPK string length exceeds bound"
			);
			strings.push(std::str::from_utf8(r.take(length)?)?.to_owned());
		}
		ensure!(r.pos == data.len(), "Unclaimed TPK body bytes");
		ensure!(
			roots.values().all(|root| *root < nodes.len())
				&& nodes.iter().all(|node| node.ty < strings.len()
					&& node.name < strings.len()
					&& node.children.iter().all(|child| *child < nodes.len())),
			"TPK contains an invalid index"
		);
		Ok(Self {
			roots,
			nodes,
			strings,
		})
	}

	pub fn resolve(&self, class: i32) -> Result<Node> {
		let index: usize = *self
			.roots
			.get(&class)
			.with_context(|| format!("No pinned same-version release TPK class {class}"))?;
		self.node(index, 0, &mut 0, &mut BTreeSet::new())
	}
	fn node(
		&self,
		index: usize,
		depth: i32,
		counter: &mut i32,
		active: &mut BTreeSet<usize>,
	) -> Result<Node> {
		ensure!(
			depth <= 256 && *counter < 1_000_000 && active.insert(index),
			"TPK node cycle or traversal bound"
		);
		let wire: &WireNode = &self.nodes[index];
		let mut node: Node = Node::with_info(
			self.strings[wire.ty].clone(),
			self.strings[wire.name].clone(),
			wire.size,
		);
		node.version = wire.version;
		node.type_flags = wire.flags;
		node.meta_flags = wire.meta;
		node.index = *counter;
		node.level = depth;
		*counter += 1;
		for child in &wire.children {
			node.children
				.push(self.node(*child, depth + 1, counter, active)?);
		}
		active.remove(&index);
		Ok(node)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn tpk_truncation_and_decompression_bounds_reject_bad_inputs() {
		for buffer in [
			&[][..],
			&[0; 20][..],
			b"TPK*\x02\x00\x00\x00\x00\x00\x00\x00\xff\xff\xff\xff\xff\xff\xff\xff",
		] {
			assert!(Registry::from_bytes(buffer).is_err());
		}
		let mut output: LimitedOutput = LimitedOutput {
			data: Vec::new(),
			limit: 2,
		};
		assert!(output.write_all(b"abc").is_err());
		assert!(output.data.is_empty());
	}
	#[test]
	fn cyclic_release_trees_are_rejected_before_recursing() {
		let registry: Registry = Registry {
			roots: BTreeMap::from([(1, 0)]),
			nodes: vec![WireNode {
				ty: 0,
				name: 0,
				size: -1,
				version: 1,
				flags: 0,
				meta: 0,
				children: vec![0],
			}],
			strings: vec!["Cycle".into()],
		};
		assert!(registry.resolve(1).is_err());
	}
}
