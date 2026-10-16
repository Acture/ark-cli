//! Bounded UnityFS v8 framing; original archive entries remain opaque on repack.
//!
//! LZ4AK token/offset repair follows ArkUnpacker `src/lz4ak/Block.py`,
//! Copyright (c) 2022-2026 Harry Huang, BSD-3-Clause, pinned by the repository's
//! vendor/ark-unpacker gitlink. Upstream credits Kengxxiao and MooncellWiki/UnityPy.
//! Redistribution terms are retained in `ARK_UNPACKER_LICENSE.txt` beside this file.

use super::UNITY_VERSION;
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_DIRECTORY: usize = 4 * 1024 * 1024;
const MAX_PAYLOAD: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
	pub name: String,
	pub flags: u32,
	pub bytes: Vec<u8>,
}

#[derive(Debug)]
struct Directory {
	blocks: Vec<(usize, usize, u16)>,
	nodes: Vec<(usize, usize, u32, String)>,
	payload_start: u64,
}

pub(crate) struct Reader<'a> {
	pub data: &'a [u8],
	pub pos: usize,
	pub big: bool,
}

impl<'a> Reader<'a> {
	pub fn new(data: &'a [u8], big: bool) -> Self {
		Self { data, pos: 0, big }
	}
	pub fn take(&mut self, len: usize) -> Result<&'a [u8]> {
		let end: usize = self.pos.checked_add(len).context("Wire offset overflow")?;
		let result: &[u8] = self
			.data
			.get(self.pos..end)
			.context("Truncated Unity buffer")?;
		self.pos = end;
		Ok(result)
	}
	pub fn u8(&mut self) -> Result<u8> {
		Ok(self.take(1)?[0])
	}
	pub fn u16(&mut self) -> Result<u16> {
		let b: [u8; 2] = self.take(2)?.try_into()?;
		Ok(if self.big {
			u16::from_be_bytes(b)
		} else {
			u16::from_le_bytes(b)
		})
	}
	pub fn u32(&mut self) -> Result<u32> {
		let b: [u8; 4] = self.take(4)?.try_into()?;
		Ok(if self.big {
			u32::from_be_bytes(b)
		} else {
			u32::from_le_bytes(b)
		})
	}
	pub fn i32(&mut self) -> Result<i32> {
		Ok(self.u32()? as i32)
	}
	pub fn u64(&mut self) -> Result<u64> {
		let b: [u8; 8] = self.take(8)?.try_into()?;
		Ok(if self.big {
			u64::from_be_bytes(b)
		} else {
			u64::from_le_bytes(b)
		})
	}
	pub fn i64(&mut self) -> Result<i64> {
		Ok(self.u64()? as i64)
	}
	pub fn count(&mut self, bound: usize) -> Result<usize> {
		let count: usize = self.u32()? as usize;
		ensure!(
			count <= bound,
			"Unity collection exceeds bound: {count} > {bound}"
		);
		Ok(count)
	}
	pub fn zstring(&mut self, bound: usize) -> Result<String> {
		let remaining: &[u8] = self
			.data
			.get(self.pos..)
			.context("String offset exceeds buffer")?;
		let end: usize = remaining
			.iter()
			.take(bound + 1)
			.position(|b| *b == 0)
			.context("Unterminated Unity string")?;
		let result: String = std::str::from_utf8(self.take(end)?)?.to_owned();
		self.take(1)?;
		Ok(result)
	}
	pub fn align(&mut self, alignment: usize) -> Result<()> {
		let padding: usize = (alignment - self.pos % alignment) % alignment;
		self.take(padding)?;
		Ok(())
	}
	pub fn string_bytes(&mut self) -> Result<Vec<u8>> {
		let count: usize = self.count(MAX_PAYLOAD)?;
		let result: Vec<u8> = self.take(count)?.to_vec();
		self.align(4)?;
		Ok(result)
	}
	pub fn string(&mut self) -> Result<String> {
		Ok(String::from_utf8(self.string_bytes()?)?)
	}
}

fn decompress(data: &[u8], size: usize, compression: u16) -> Result<Vec<u8>> {
	ensure!(
		size <= MAX_PAYLOAD,
		"Decompressed Unity block exceeds bound"
	);
	let result: Vec<u8> = match compression & 63 {
		0 => data.to_vec(),
		2 | 3 => lz4_flex::block::decompress(data, size)?,
		4 => lz4_flex::block::decompress(&repair_lz4ak(data, size)?, size)?,
		other => bail!("Unsupported original Unity compression {other}"),
	};
	ensure!(result.len() == size, "Unity block decoded length differs");
	Ok(result)
}

fn extra(data: &[u8], offset: &mut usize) -> Result<usize> {
	let mut sum: usize = 0;
	loop {
		let byte: u8 = *data
			.get(*offset)
			.context("Truncated LZ4AK extended length")?;
		*offset += 1;
		sum = sum
			.checked_add(byte as usize)
			.context("LZ4AK length overflow")?;
		if byte != 255 {
			return Ok(sum);
		}
	}
}

fn repair_lz4ak(data: &[u8], size: usize) -> Result<Vec<u8>> {
	let mut fixed: Vec<u8> = data.to_vec();
	let mut input: usize = 0;
	let mut output: usize = 0;
	while input < fixed.len() {
		let token: u8 = fixed[input];
		fixed[input] = token.rotate_left(4);
		input += 1;
		let mut literals: usize = (token & 15) as usize;
		let mut matches: usize = (token >> 4) as usize;
		if literals == 15 {
			literals += extra(&fixed, &mut input)?;
		}
		input = input
			.checked_add(literals)
			.context("LZ4AK literal offset overflow")?;
		output = output
			.checked_add(literals)
			.context("LZ4AK output overflow")?;
		ensure!(
			input <= fixed.len() && output <= size,
			"LZ4AK literal exceeds buffer"
		);
		if output == size {
			ensure!(input == fixed.len(), "LZ4AK trailing bytes");
			break;
		}
		ensure!(input + 2 <= fixed.len(), "Truncated LZ4AK match offset");
		let distance: usize = u16::from_be_bytes([fixed[input], fixed[input + 1]]) as usize;
		ensure!(
			distance > 0 && distance <= output,
			"LZ4AK invalid match distance"
		);
		fixed.swap(input, input + 1);
		input += 2;
		if matches == 15 {
			matches += extra(&fixed, &mut input)?;
		}
		output = output
			.checked_add(matches + 4)
			.context("LZ4AK match overflow")?;
		ensure!(output <= size, "LZ4AK match exceeds declared output");
	}
	ensure!(output == size, "LZ4AK output incomplete");
	Ok(fixed)
}

fn directory(stream: &mut File) -> Result<Directory> {
	let length: u64 = stream.metadata()?.len();
	let mut prefix: Vec<u8> = vec![0; usize::try_from(length.min(512))?];
	stream.read_exact(&mut prefix)?;
	let mut r: Reader<'_> = Reader::new(&prefix, true);
	ensure!(
		r.zstring(32)? == "UnityFS" && r.u32()? == 8,
		"Expected UnityFS v8"
	);
	r.zstring(128)?;
	ensure!(
		r.zstring(128)? == UNITY_VERSION,
		"Unexpected original Unity version"
	);
	ensure!(r.u64()? == length, "UnityFS length differs from header");
	let compressed: usize = r.u32()? as usize;
	let decoded: usize = r.u32()? as usize;
	let flags: u32 = r.u32()?;
	ensure!(
		compressed <= MAX_DIRECTORY && decoded <= MAX_DIRECTORY && flags & 0x400 == 0,
		"UnityFS directory bound or unsupported encryption flag"
	);
	r.align(16)?;
	let start: u64 = if flags & 0x80 != 0 {
		length
			.checked_sub(compressed as u64)
			.context("Directory outside UnityFS")?
	} else {
		r.pos as u64
	};
	stream.seek(SeekFrom::Start(start))?;
	let mut raw: Vec<u8> = vec![0; compressed];
	stream.read_exact(&mut raw)?;
	let metadata: Vec<u8> = decompress(&raw, decoded, (flags & 63) as u16)?;
	let mut m: Reader<'_> = Reader::new(&metadata, true);
	m.take(16)?;
	let count: usize = m.count(100_000)?;
	let mut blocks: Vec<(usize, usize, u16)> = Vec::with_capacity(count);
	for _ in 0..count {
		blocks.push((m.u32()? as usize, m.u32()? as usize, m.u16()?));
	}
	let count: usize = m.count(4096)?;
	let mut nodes: Vec<(usize, usize, u32, String)> = Vec::with_capacity(count);
	for _ in 0..count {
		nodes.push((
			usize::try_from(m.u64()?)?,
			usize::try_from(m.u64()?)?,
			m.u32()?,
			m.zstring(4096)?,
		));
	}
	ensure!(m.pos == metadata.len(), "Unclaimed UnityFS directory bytes");
	let mut payload_start: u64 = if flags & 0x80 != 0 {
		r.pos as u64
	} else {
		start + compressed as u64
	};
	if flags & 0x200 != 0 {
		payload_start = (payload_start + 15) & !15;
	}
	Ok(Directory {
		blocks,
		nodes,
		payload_start,
	})
}

pub(crate) fn names(path: &Path) -> Result<Vec<String>> {
	Ok(directory(&mut File::open(path)?)?
		.nodes
		.into_iter()
		.map(|node| node.3)
		.collect())
}

pub(crate) fn load(path: &Path) -> Result<Vec<Entry>> {
	let mut stream: File = File::open(path)?;
	let dir: Directory = directory(&mut stream)?;
	let total: usize = dir.blocks.iter().try_fold(0_usize, |sum, block| {
		sum.checked_add(block.0)
			.context("UnityFS block sum overflow")
	})?;
	ensure!(
		total <= MAX_PAYLOAD,
		"UnityFS archive exceeds decompressed bound"
	);
	validate_nodes(&dir.nodes, total)?;
	stream.seek(SeekFrom::Start(dir.payload_start))?;
	let mut payload: Vec<u8> = Vec::with_capacity(total);
	for (size, compressed, flags) in dir.blocks {
		ensure!(
			compressed <= MAX_PAYLOAD,
			"UnityFS compressed block exceeds bound"
		);
		let mut block: Vec<u8> = vec![0; compressed];
		stream.read_exact(&mut block)?;
		payload.extend(decompress(&block, size, flags)?);
	}
	let mut result: Vec<Entry> = Vec::new();
	for (offset, size, flags, name) in dir.nodes {
		let end: usize = offset.checked_add(size).context("UnityFS node overflow")?;
		result.push(Entry {
			name,
			flags,
			bytes: payload
				.get(offset..end)
				.context("UnityFS node outside payload")?
				.to_vec(),
		});
	}
	Ok(result)
}

/// Admit the complete directory before allocating either decompressed or retained entries.
/// In particular, aliases cannot multiply one bounded payload into thousands of copies.
fn validate_nodes(nodes: &[(usize, usize, u32, String)], payload_size: usize) -> Result<()> {
	let mut retained: usize = 0;
	let mut names: BTreeSet<&str> = BTreeSet::new();
	let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(nodes.len());
	for (offset, size, _, name) in nodes {
		retained = retained
			.checked_add(*size)
			.context("UnityFS retained entry size overflow")?;
		ensure!(
			retained <= MAX_PAYLOAD,
			"UnityFS aggregate entry bytes exceed bound"
		);
		let end: usize = offset.checked_add(*size).context("UnityFS node overflow")?;
		ensure!(end <= payload_size, "UnityFS node outside payload");
		ensure!(names.insert(name), "Duplicate CAB node {name}");
		if *size > 0 {
			ranges.push((*offset, end));
		}
	}
	ranges.sort_unstable();
	ensure!(
		ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
		"Overlapping UnityFS node ranges"
	);
	Ok(())
}

pub(crate) fn save(entries: &[Entry]) -> Result<Vec<u8>> {
	ensure!(
		!entries.is_empty() && entries.len() <= 4096,
		"Invalid UnityFS node count"
	);
	let mut payload: Vec<u8> = Vec::new();
	let mut metadata: Vec<u8> = vec![0; 16];
	metadata.extend(1_u32.to_be_bytes());
	let total: usize = entries.iter().try_fold(0_usize, |sum, entry| {
		sum.checked_add(entry.bytes.len())
			.context("UnityFS size overflow")
	})?;
	ensure!(total <= MAX_PAYLOAD, "UnityFS output exceeds bound");
	for entry in entries {
		payload.extend_from_slice(&entry.bytes);
	}
	let compressed: Vec<u8> = lz4_flex::block::compress(&payload);
	metadata.extend(u32::try_from(payload.len())?.to_be_bytes());
	metadata.extend(u32::try_from(compressed.len())?.to_be_bytes());
	metadata.extend(3_u16.to_be_bytes());
	metadata.extend(u32::try_from(entries.len())?.to_be_bytes());
	let mut offset: u64 = 0;
	for entry in entries {
		ensure!(!entry.name.as_bytes().contains(&0), "NUL in CAB name");
		metadata.extend(offset.to_be_bytes());
		metadata.extend((entry.bytes.len() as u64).to_be_bytes());
		metadata.extend(entry.flags.to_be_bytes());
		metadata.extend(entry.name.as_bytes());
		metadata.push(0);
		offset += entry.bytes.len() as u64;
	}
	let info: Vec<u8> = lz4_flex::block::compress(&metadata);
	let mut result: Vec<u8> = b"UnityFS\0".to_vec();
	result.extend(8_u32.to_be_bytes());
	result.extend(b"5.x.x\0");
	result.extend(UNITY_VERSION.as_bytes());
	result.push(0);
	let size_offset: usize = result.len();
	result.extend(0_u64.to_be_bytes());
	result.extend(u32::try_from(info.len())?.to_be_bytes());
	result.extend(u32::try_from(metadata.len())?.to_be_bytes());
	result.extend(0x43_u32.to_be_bytes());
	while !result.len().is_multiple_of(16) {
		result.push(0);
	}
	result.extend(info);
	result.extend(compressed);
	let size: u64 = result.len() as u64;
	result[size_offset..size_offset + 8].copy_from_slice(&size.to_be_bytes());
	let options = unity_asset_binary::bundle::BundleLoadOptions {
		load_assets: false,
		max_memory: Some(MAX_PAYLOAD),
		max_compressed_blocks_info_size: Some(MAX_DIRECTORY),
		max_blocks_info_size: Some(MAX_DIRECTORY),
		max_compressed_block_size: Some(MAX_PAYLOAD),
		max_blocks: 100_000,
		max_nodes: 4096,
		..Default::default()
	};
	let independent =
		unity_asset_binary::bundle::BundleParser::from_bytes_with_options(result.clone(), options)
			.context("Independent UnityFS decoder rejected output")?;
	ensure!(
		independent.nodes.len() == entries.len(),
		"Independent CAB node count differs"
	);
	for (node, original) in independent.nodes.iter().zip(entries) {
		ensure!(
			node.name == original.name
				&& node.flags == original.flags
				&& independent.extract_node_data(node)? == original.bytes,
			"Independent CAB bytes differ for {}",
			original.name
		);
	}
	Ok(result)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn directory_only_fixture(nodes: &[(u64, u64, &str)], payload_size: u32) -> Vec<u8> {
		let mut directory: Vec<u8> = vec![0; 16];
		directory.extend(1_u32.to_be_bytes());
		directory.extend(payload_size.to_be_bytes());
		directory.extend(0_u32.to_be_bytes());
		directory.extend(0_u16.to_be_bytes());
		directory.extend((nodes.len() as u32).to_be_bytes());
		for (offset, size, name) in nodes {
			directory.extend(offset.to_be_bytes());
			directory.extend(size.to_be_bytes());
			directory.extend(4_u32.to_be_bytes());
			directory.extend(name.as_bytes());
			directory.push(0);
		}
		let mut result: Vec<u8> = b"UnityFS\0".to_vec();
		result.extend(8_u32.to_be_bytes());
		result.extend(b"5.x.x\0");
		result.extend(UNITY_VERSION.as_bytes());
		result.push(0);
		let size_offset: usize = result.len();
		result.extend(0_u64.to_be_bytes());
		result.extend((directory.len() as u32).to_be_bytes());
		result.extend((directory.len() as u32).to_be_bytes());
		result.extend(0x40_u32.to_be_bytes());
		while !result.len().is_multiple_of(16) {
			result.push(0);
		}
		result.extend(directory);
		let size: u64 = result.len() as u64;
		result[size_offset..size_offset + 8].copy_from_slice(&size.to_be_bytes());
		result
	}

	fn rejected_directory(bytes: &[u8], reason: &str) {
		let path: std::path::PathBuf = std::env::temp_dir().join(format!(
			"ark-bundle-invalid-{}-{}.ab",
			std::process::id(),
			chrono::Utc::now().timestamp_nanos_opt().unwrap()
		));
		std::fs::write(&path, bytes).unwrap();
		let error = load(&path).unwrap_err();
		std::fs::remove_file(path).unwrap();
		assert!(error.to_string().contains(reason), "{error:#}");
	}

	#[test]
	fn overlapping_directory_nodes_are_refused_before_payload_decompression() {
		rejected_directory(
			&directory_only_fixture(&[(0, 4, "CAB-a"), (0, 4, "CAB-b")], 4),
			"Overlapping UnityFS node ranges",
		);
		assert!(validate_nodes(
			&[
				(0, 0, 0, "empty".into()),
				(0, 4, 4, "CAB-a".into()),
				(4, 2, 0, "CAB-a.resS".into())
			],
			6
		)
		.is_ok());
	}

	#[test]
	fn aggregate_directory_bytes_are_refused_without_allocating_advertised_payload() {
		let size: u64 = (MAX_PAYLOAD / 2 + 1) as u64;
		rejected_directory(
			&directory_only_fixture(
				&[(0, size, "CAB-a"), (0, size, "CAB-b")],
				MAX_PAYLOAD as u32,
			),
			"aggregate entry bytes exceed bound",
		);
		rejected_directory(
			&directory_only_fixture(&[(u64::MAX, 4, "CAB-a")], 4),
			"UnityFS node overflow",
		);
	}

	#[test]
	fn ark_literals_are_repaired_and_bad_matches_fail() {
		assert_eq!(decompress(&[3, b'a', b'b', b'c'], 3, 4).unwrap(), b"abc");
		for raw in [vec![0], vec![1, b'a', 0, 0], vec![15]] {
			assert!(decompress(&raw, 20, 4).is_err());
		}
	}
	#[test]
	fn framing_preserves_every_entry_byte() {
		let entries: Vec<Entry> = vec![
			Entry {
				name: "CAB-01234".into(),
				flags: 4,
				bytes: (0..=255).collect(),
			},
			Entry {
				name: "CAB-01234.resS".into(),
				flags: 0,
				bytes: vec![4, 0, 9, 255],
			},
		];
		let path = std::env::temp_dir().join(format!(
			"ark-bundle-{}-{}.ab",
			std::process::id(),
			chrono::Utc::now().timestamp_nanos_opt().unwrap()
		));
		std::fs::write(&path, save(&entries).unwrap()).unwrap();
		assert_eq!(load(&path).unwrap(), entries);
		assert_eq!(names(&path).unwrap(), vec!["CAB-01234", "CAB-01234.resS"]);
		std::fs::remove_file(path).unwrap();
	}
}
