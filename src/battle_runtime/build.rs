//! Include owned tooling sources at build time so the executable needs no source checkout.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn files(directory: &Path) -> Vec<PathBuf> {
	let mut result: Vec<PathBuf> = Vec::new();
	for entry in fs::read_dir(directory).expect("read bundled source directory") {
		let entry = entry.expect("read bundled source entry");
		let path: PathBuf = entry.path();
		let name = entry.file_name();
		if matches!(
			name.to_str(),
			Some("target" | ".git" | "Library" | "Temp" | "Logs" | "obj")
		) {
			continue;
		}
		assert!(
			!entry.file_type().unwrap().is_symlink(),
			"bundled sources must not be links"
		);
		if path.is_dir() {
			result.extend(files(&path));
		} else {
			result.push(path);
		}
	}
	result.sort();
	result
}

fn table(name: &str, base: &Path, paths: &[PathBuf]) -> String {
	let mut output: String = format!("pub const {name}: &[(&str, &[u8])] = &[\n");
	for path in paths {
		let relative: String = path
			.strip_prefix(base)
			.unwrap()
			.to_string_lossy()
			.replace('\\', "/");
		println!("cargo:rerun-if-changed={}", path.display());
		output.push_str(&format!(
			"({relative:?}, include_bytes!({:?})),\n",
			path.to_str().unwrap()
		));
	}
	output.push_str("];\n");
	output
}

/// Release builds with `bundled-native` embed third-party tools and the prebuilt probe from
/// `ARK_NATIVE_BUNDLE_DIR`; every other build embeds none and downloads or compiles instead.
fn native_bundle() -> String {
	const FILES: [&str; 5] = [
		"TypeTreeGeneratorAPI.dll",
		"capstone.dll",
		"lzma.tpk",
		"ArkBattleProbe.dll",
		"probe-source-digest.txt",
	];
	println!("cargo:rerun-if-env-changed=ARK_NATIVE_BUNDLE_DIR");
	if env::var_os("CARGO_FEATURE_BUNDLED_NATIVE").is_none() {
		return "pub const NATIVE_BUNDLE: &[(&str, &[u8])] = &[];
"
		.to_owned();
	}
	let directory: PathBuf = PathBuf::from(
		env::var_os("ARK_NATIVE_BUNDLE_DIR")
			.expect("bundled-native requires ARK_NATIVE_BUNDLE_DIR with the native bundle files"),
	);
	let paths: Vec<PathBuf> = FILES.iter().map(|name| directory.join(name)).collect();
	for path in &paths {
		assert!(
			path.is_file(),
			"native bundle file missing: {}",
			path.display()
		);
	}
	table("NATIVE_BUNDLE", &directory, &paths)
}

fn main() {
	let manifest: PathBuf = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
	let unity: PathBuf = manifest.join("unity");
	let mut output: String = table("UNITY_FILES", &unity, &files(&unity));
	output.push_str(&table("SOURCE_FILES", &manifest, &files(&manifest)));
	for directory in [&manifest, &unity] {
		println!("cargo:rerun-if-changed={}", directory.display());
	}
	output.push_str(&native_bundle());
	let generated: PathBuf = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bundled.rs");
	fs::write(generated, output).expect("write bundled tooling index");
}
