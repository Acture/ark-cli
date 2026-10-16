//! Third-party native tooling and the prebuilt owned Player, placed in owned state with pinned
//! hashes: embedded in `bundled-native` builds, otherwise downloaded from pinned upstream files.
use crate::bundle::NATIVE_BUNDLE;
use crate::environment::{read_json, sha256_file, write_json};
use anyhow::{bail, ensure, Context, Result};
use ark_unity::{build, io};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
	fs,
	io::{Read, Write},
	path::{Path, PathBuf},
};

/// One pinned upstream archive and its SHA-256.
struct Archive {
	name: &'static str,
	url: &'static str,
	sha256: &'static str,
}

const TYPE_TREE_WHEEL: Archive = Archive {
	name: "typetreegeneratorapi-0.0.10-cp36-abi3-win_amd64.whl",
	url: "https://files.pythonhosted.org/packages/ed/bc/f911995cae93fa8fd882a677f6bc304965db6f066bbabdee6e761762d512/typetreegeneratorapi-0.0.10-cp36-abi3-win_amd64.whl",
	sha256: "e96003e973c8d52d221b4ae50f377bcaf7538f2098e437d5383a1b7f13fa72ba",
};
const UNITYPY_WHEEL: Archive = Archive {
	name: "unitypy-1.25.4-cp312-cp312-win_amd64.whl",
	url: "https://files.pythonhosted.org/packages/5a/ff/916b14f08ebbf4ee30a29e4b5bec592ed62dc0c0b9276a95f903a1da102c/unitypy-1.25.4-cp312-cp312-win_amd64.whl",
	sha256: "5e3f6fa44e23e7fc52a32f128d709a97901e0d10e3d87e0b602c5f5a58e3a4ef",
};

/// A tool file: where `ark-unity` reads it under owned state, its hash, and its upstream member.
struct Tool {
	path: &'static str,
	bundled: &'static str,
	sha256: &'static str,
	archive: &'static Archive,
	member: &'static str,
}

const TOOLS: [Tool; 3] = [
	Tool {
		path: "tools/typetreegenerator-0.0.10/TypeTreeGeneratorAPI.dll",
		bundled: "TypeTreeGeneratorAPI.dll",
		sha256: "bbb823d0ecad8dafb26bce014d63cdbfcfe135935a8fbee24a783b6eb8115b27",
		archive: &TYPE_TREE_WHEEL,
		member: "TypeTreeGeneratorAPI/TypeTreeGeneratorAPI.dll",
	},
	Tool {
		path: "tools/typetreegenerator-0.0.10/capstone.dll",
		bundled: "capstone.dll",
		sha256: "f03321188a1615d044314b1183dcd9a7ffda09286c31c369d109165d4892dcbb",
		archive: &TYPE_TREE_WHEEL,
		member: "TypeTreeGeneratorAPI/capstone.dll",
	},
	Tool {
		path: "tools/unitypy-1.25.4/lzma.tpk",
		bundled: "lzma.tpk",
		sha256: "16f1f8b809d1cd3bc3aeebf0cd43ecd3690b326a7a88b2273e57bcd83f5df608",
		archive: &UNITYPY_WHEEL,
		member: "UnityPy/resources/lzma.tpk",
	},
];

/// The prebuilt owned Player, published as a GitHub Release asset of this repository.
const PLAYER: Archive = Archive {
	name: "ark-battle-runtime-unity2021.3.39f1-r2.zip",
	url: "https://github.com/Acture/ark-cli/releases/download/ark-battle-runtime-unity2021.3.39f1-r2/ark-battle-runtime-unity2021.3.39f1-r2.zip",
	sha256: "804572ad72c05c2064e8528ab2bc001c79d77bb995c11996e46ff50f531da226",
};
const PLAYER_MANIFEST: &str = "player-manifest.json";
const LICENSES: &str = include_str!("../licenses/THIRD_PARTY.md");

pub(crate) fn bundled(name: &str) -> Option<&'static [u8]> {
	NATIVE_BUNDLE
		.iter()
		.find(|(file, _)| *file == name)
		.map(|(_, bytes)| *bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

/// The pinned archive, from the download cache or the network, after its hash matches.
fn archive(state: &Path, archive: &Archive, local: Option<&Path>) -> Result<Vec<u8>> {
	let cache: PathBuf = io::owned(state, &state.join("downloads").join(archive.name))?;
	let bytes: Vec<u8> = if let Some(local) = local {
		fs::read(local).with_context(|| format!("Read {}", local.display()))?
	} else if cache.is_file() {
		fs::read(&cache)?
	} else {
		eprintln!("INFO ark_battle_runtime: downloading {}", archive.url);
		let response = reqwest::blocking::Client::builder()
			.timeout(std::time::Duration::from_secs(600))
			.build()?
			.get(archive.url)
			.send()?
			.error_for_status()
			.with_context(|| format!("Download {}", archive.url))?;
		response.bytes()?.to_vec()
	};
	ensure!(
		archive.sha256.is_empty() || sha256_bytes(&bytes) == archive.sha256,
		"{} differs from its pinned SHA-256",
		archive.name
	);
	if local.is_none() && !cache.is_file() {
		fs::create_dir_all(cache.parent().unwrap())?;
		fs::write(&cache, &bytes)?;
	}
	Ok(bytes)
}

fn member(bytes: &[u8], name: &str) -> Result<Vec<u8>> {
	let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
	let mut file = zip
		.by_name(name)
		.with_context(|| format!("Archive has no {name}"))?;
	let mut output: Vec<u8> = Vec::new();
	file.read_to_end(&mut output)?;
	Ok(output)
}

/// Place every third-party tool with its pinned hash; report where each came from.
pub(crate) fn ensure_tools(state: &Path) -> Result<Value> {
	let mut report: Vec<Value> = Vec::new();
	for tool in &TOOLS {
		let path: PathBuf = io::owned(state, &state.join(tool.path))?;
		let origin: &str = if path.is_file() && sha256_file(&path)? == tool.sha256 {
			"present"
		} else {
			let (bytes, origin): (Vec<u8>, &str) = match bundled(tool.bundled) {
				Some(bytes) => (bytes.to_vec(), "embedded"),
				None => (
					member(&archive(state, tool.archive, None)?, tool.member)?,
					"downloaded",
				),
			};
			ensure!(
				sha256_bytes(&bytes) == tool.sha256,
				"{} differs from its pinned SHA-256",
				tool.path
			);
			fs::create_dir_all(path.parent().unwrap())?;
			fs::write(&path, &bytes)?;
			origin
		};
		report.push(json!({"path": path, "sha256": tool.sha256, "origin": origin}));
	}
	let notice: PathBuf = io::owned(state, &state.join("tools/THIRD_PARTY.md"))?;
	fs::write(&notice, LICENSES)?;
	Ok(json!({"tools": report, "licenses": notice}))
}

/// Install the prebuilt owned Player from the pinned release asset or an explicit local zip.
pub(crate) fn install_player(state: &Path, local: Option<&Path>) -> Result<PathBuf> {
	ensure!(
		local.is_some() || !PLAYER.sha256.is_empty(),
		"No prebuilt owned Player is published for this build yet; pass --player-zip"
	);
	let bytes: Vec<u8> = archive(state, &PLAYER, local)?;
	let digest: String = sha256_bytes(&bytes);
	let root: PathBuf = io::owned(
		state,
		&state
			.join("player")
			.join(format!("prebuilt-{}", &digest[..16])),
	)?;
	let player: PathBuf = root.join("player");
	let record: PathBuf = root.join("build.json");
	if record.is_file() && build::verified_player(&record).is_ok() {
		return Ok(record);
	}
	if root.exists() {
		fs::remove_dir_all(&root)?;
	}
	fs::create_dir_all(&player)?;
	let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes))?;
	let manifest: Value = serde_json::from_slice(&member(&bytes, PLAYER_MANIFEST)?)?;
	for index in 0..zip.len() {
		let mut file = zip.by_index(index)?;
		let Some(name) = file.enclosed_name() else {
			bail!("Player archive has an unsafe path");
		};
		if name == Path::new(PLAYER_MANIFEST) || file.is_dir() {
			continue;
		}
		let target: PathBuf = io::owned(&root, &player.join(&name))?;
		fs::create_dir_all(target.parent().unwrap())?;
		let mut output = fs::File::create(&target)?;
		std::io::copy(&mut file, &mut output)?;
		output.flush()?;
	}
	let inventory: Value = build::snapshot_player(&player)?;
	ensure!(
		inventory == manifest["full_inventory"],
		"Extracted Player differs from its packaged inventory"
	);
	write_json(
		&record,
		&json!({"schema":"ark-battle-player-build-v3","result":"BuildSucceeded","origin":"prebuilt",
			"archive":{"name":PLAYER.name,"sha256":digest},"player":player,
			"artifacts":manifest["artifacts"],"full_inventory":inventory,"unity_version":manifest["unity_version"]}),
	)?;
	build::verified_player(&record)?;
	Ok(record)
}

/// Package a verified owned Player for release: its files plus the complete inventory.
pub(crate) fn package_player(record: &Path, output: &Path) -> Result<Value> {
	let player: PathBuf = build::verified_player(record)?;
	let value: Value = read_json(record)?;
	let manifest: Value = json!({"unity_version": ark_unity::UNITY_VERSION,
		"artifacts": value["artifacts"], "full_inventory": value["full_inventory"]});
	let file = fs::File::create(output)?;
	let mut zip = zip::ZipWriter::new(file);
	let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
		.compression_method(zip::CompressionMethod::Deflated);
	for entry in value["full_inventory"]
		.as_array()
		.context("Player inventory missing")?
	{
		let relative: &str = entry["path"].as_str().context("Inventory path missing")?;
		zip.start_file(relative, options)?;
		zip.write_all(&fs::read(player.join(relative))?)?;
	}
	zip.start_file(PLAYER_MANIFEST, options)?;
	zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
	zip.finish()?;
	Ok(
		json!({"archive": output, "sha256": sha256_file(output)?, "files": value["full_inventory"].as_array().map_or(0, Vec::len)}),
	)
}
