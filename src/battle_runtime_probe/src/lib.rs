//! Portable source package for the Windows x64 no-CRT Unity preloaded plugin.
//!
//! The battle runtime materializes these exact bytes into its owned state directory, then builds the
//! isolated DLL with its own panic-abort profile. Linking this library never builds the DLL.

/// Relative paths and source bytes of the isolated, dependency-free Cargo DLL project.
pub const FILES: &[(&str, &[u8])] = &[
	("Cargo.toml", include_bytes!("../template/Cargo.toml.in")),
	("Cargo.lock", include_bytes!("../template/Cargo.lock")),
	("build.rs", include_bytes!("../template/build.rs")),
	(
		"src/battle-probe.rs",
		include_bytes!("../template/src/battle-probe.rs"),
	),
	(
		"src/battle-probe-api.rs",
		include_bytes!("../template/src/battle-probe-api.rs"),
	),
	(
		"src/battle-actions.rs",
		include_bytes!("../template/src/battle-actions.rs"),
	),
	(
		"src/battle-events.rs",
		include_bytes!("../template/src/battle-events.rs"),
	),
	(
		"src/battle-component.rs",
		include_bytes!("../template/src/battle-component.rs"),
	),
	(
		"src/battle-level.rs",
		include_bytes!("../template/src/battle-level.rs"),
	),
	(
		"src/battle-scene.rs",
		include_bytes!("../template/src/battle-scene.rs"),
	),
	(
		"src/battle-services.rs",
		include_bytes!("../template/src/battle-services.rs"),
	),
];
