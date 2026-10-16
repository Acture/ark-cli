# Ark Battle Runtime Probe

This portable Rust library carries the exact sources of the Windows x64 no-CRT Unity plugin.
`ark-battle-runtime` materializes `FILES` into its owned state and builds the isolated DLL there.
The DLL retains its own `panic = "abort"` profiles and has no Cargo dependencies. Importing this
source package on macOS or Linux does not compile or load the Windows plugin.

The actual plugin source is in `template/src/`. `template/Cargo.toml.in` becomes `Cargo.toml`
when materialized. A Windows x64 Rust toolchain and Windows SDK import libraries are required
only when building that DLL. This package contains no client code or extracted game assets.
