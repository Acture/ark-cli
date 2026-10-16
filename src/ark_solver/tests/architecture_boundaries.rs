use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(path: &Path) -> Vec<PathBuf> {
	if !path.exists() {
		return Vec::new();
	}
	if path.is_file() {
		return path
			.extension()
			.is_some_and(|extension| extension == "rs")
			.then(|| path.to_owned())
			.into_iter()
			.collect();
	}

	let mut sources: Vec<PathBuf> = fs::read_dir(path)
		.unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
		.flat_map(|entry| {
			let entry = entry.unwrap_or_else(|error| {
				panic!("failed to read an entry under {}: {error}", path.display())
			});
			rust_sources(&entry.path())
		})
		.collect();
	sources.sort();
	sources
}

fn use_declarations(source: &str) -> Vec<String> {
	let mut declarations: Vec<String> = Vec::new();
	let mut current: Option<String> = None;
	for raw_line in source.lines() {
		let line: &str = raw_line.trim_start();
		if let Some(declaration) = current.as_mut() {
			declaration.push(' ');
			declaration.push_str(line);
			if line.contains(';') {
				declarations.push(current.take().expect("active use declaration exists"));
			}
			continue;
		}

		if line.starts_with("use ")
			|| line.starts_with("pub use ")
			|| line.starts_with("pub(crate) use ")
		{
			current = Some(line.to_owned());
			if line.contains(';') {
				declarations.push(current.take().expect("active use declaration exists"));
			}
		}
	}
	assert!(current.is_none(), "unterminated use declaration in source");
	declarations
}

fn contains_identifier(source: &str, identifier: &str) -> bool {
	source
		.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
		.any(|token| token == identifier)
}

fn assert_module_does_not_import(module_paths: &[PathBuf], forbidden_module: &str) {
	let sources: Vec<PathBuf> = module_paths
		.iter()
		.flat_map(|path| rust_sources(path))
		.collect();
	assert!(!sources.is_empty(), "module source set must not be empty");

	for path in sources {
		let source: String = fs::read_to_string(&path)
			.unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
		for declaration in use_declarations(&source) {
			assert!(
				!contains_identifier(&declaration, forbidden_module),
				"{} imports forbidden sibling module `{forbidden_module}` in `{declaration}`",
				path.display(),
			);
		}

		let absolute_path: String = format!("crate::{forbidden_module}::");
		let relative_path: String = format!("super::super::{forbidden_module}::");
		assert!(
			!source.contains(&absolute_path) && !source.contains(&relative_path),
			"{} references forbidden sibling module `{forbidden_module}`",
			path.display(),
		);
	}
}

#[test]
fn planning_and_replay_are_siblings_without_cross_imports() {
	let source_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	assert_module_does_not_import(&[source_root.join("planning")], "replay");
	assert_module_does_not_import(
		&[source_root.join("replay.rs"), source_root.join("replay")],
		"planning",
	);
}

#[test]
fn lower_layers_do_not_import_application_workflows() {
	let source_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	for module in [
		"maa_job",
		"verification_artifact",
		"mechanics",
		"planning",
		"replay",
	] {
		assert_module_does_not_import(&[source_root.join(module)], "workflow");
	}
}

#[test]
fn normalized_planning_and_replay_contracts_do_not_import_maa_dtos() {
	let source_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	assert_module_does_not_import(&[source_root.join("planning")], "maa_job");
	assert_module_does_not_import(&[source_root.join("replay")], "maa_job");
}

#[test]
fn maa_protocol_layer_does_not_import_planning_or_mechanics() {
	let source_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	let maa_sources = [source_root.join("maa_job")];
	assert_module_does_not_import(&maa_sources, "planning");
	assert_module_does_not_import(&maa_sources, "mechanics");
}

#[test]
fn simulation_remains_independent_of_product_and_io_modules() {
	let source_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	let simulation_sources: [PathBuf; 1] = [source_root.join("simulation")];
	for module in [
		"config",
		"data",
		"ingest",
		"maa_job",
		"mechanics",
		"planning",
		"verification_artifact",
		"workflow",
		"bevy",
		"engine",
	] {
		assert_module_does_not_import(&simulation_sources, module);
	}
}

#[test]
fn data_decoding_does_not_depend_on_application_modules() {
	let source_root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
	let data_sources: [PathBuf; 1] = [source_root.join("data")];
	for module in [
		"config",
		"ingest",
		"maa_job",
		"planning",
		"replay",
		"simulation",
		"verification_artifact",
		"workflow",
	] {
		assert_module_does_not_import(&data_sources, module);
	}
}
