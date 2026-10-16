# Ark CLI

`arknights-cli` provides the independently buildable `ark-cli` executable for ingestion, solver
replay, verification, and original-engine execution. It calls the `ark-solver` and `ark-battle-runtime`
libraries directly.

From the workspace root:

```fish
cargo build --release -p arknights-cli
cargo install --path src/ark_cli --locked
ark-cli --help
ark-cli level solve --help
ark-cli level validate --help
ark-cli replay --help
ark-cli ingest --help
ark-cli battle --help
```

The release executable is `target/release/ark-cli` (`ark-cli.exe` on Windows). The package
supports executable archives and crates.io publication. After the first coordinated crate
release, install it from crates.io with `cargo install arknights-cli --locked`. Original battles
(`battle`) require Windows, a user-owned client, and the battle runtime prerequisites documented in
[`RELEASE.md`](RELEASE.md).

Ingestion resolves `rogue.toml` from `ROGUE_CONFIG`, the nearest working-directory ancestor,
then beside the executable. Without a config, data paths default to `assets`, `client`, and
`client/_gamedata` under the working directory. Paths in a config are relative to that file.

Help and version exit with 0; invalid arguments exit with 2 before workflows run. Solver and
ingestion runtime errors exit with 1. Battle reports with `passed: false` exit with 1, and battle
runtime errors exit with 2. Successful commands exit with 0.

Use [`RELEASE.md`](RELEASE.md) for standalone runtime requirements and the local Windows/Unix
packaging scripts. Packaging writes a versioned directory, archive, and SHA256 under
`assets/releases` and refuses existing outputs.
