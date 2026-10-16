use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
	assert_eq!(
		env::var("TARGET").as_deref(),
		Ok("x86_64-pc-windows-msvc"),
		"the Unity probe only supports Windows x64 MSVC"
	);
	let root: PathBuf = env::var_os("WindowsSdkDir")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from("C:/Program Files (x86)/Windows Kits/10"));
	let mut versions: Vec<(Vec<u32>, PathBuf)> = fs::read_dir(root.join("Lib"))
		.expect("Windows SDK library directory is required")
		.filter_map(Result::ok)
		.filter_map(|entry| {
			let path: PathBuf = entry.path().join("um/x64");
			let version: Option<Vec<u32>> = entry
				.file_name()
				.to_string_lossy()
				.split('.')
				.map(|part| part.parse::<u32>().ok())
				.collect();
			(path.join("kernel32.lib").is_file() && path.join("user32.lib").is_file())
				.then(|| version.map(|version| (version, path)))
				.flatten()
		})
		.collect();
	versions.sort();
	let sdk: PathBuf = versions
		.pop()
		.expect("Windows x64 SDK import libraries are required")
		.1;
	println!("cargo:rustc-link-search=native={}", sdk.display());
	println!("cargo:rustc-cdylib-link-arg=/NOENTRY");
	println!("cargo:rustc-cdylib-link-arg=/NODEFAULTLIB");
	println!("cargo:rerun-if-env-changed=WindowsSdkDir");
}
