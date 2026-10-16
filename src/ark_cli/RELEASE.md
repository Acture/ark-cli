# Ark CLI standalone distribution

This package contains `ark-cli` (`ark-cli.exe` on Windows), this README, the project's `LICENSE`
(AGPL-3.0-or-later) with its section 7 additional permission in `NOTICE`, and a default
`rogue.toml`. It contains no game client, owned Unity Player, or
game data.
It supports executable archives, crates.io installation, and GitHub Release distribution.
The crates.io package is `arknights-cli`; the installed executable is `ark-cli`.

Run `ark-cli --help` for `level solve`, `level validate`, `replay`, `ingest`, and `battle` commands.
Solver workflows require the matching verified gamedata and battle-prefab snapshots, plus the
MAA job files described by each command's help. Data files are supplied separately.

Configuration resolves from `ROGUE_CONFIG`, then the nearest `rogue.toml` in the working directory
or its ancestors, then beside the executable. Relative configured paths are relative to the
config file. Without a config, defaults are `assets`, `client`, and `client/_gamedata` under the
working directory. Edit the bundled config or supply `ROGUE_CONFIG` for your local data layout.

The executable can run outside its source checkout. The native probe sources and Unity template
are embedded in the executable. Original battles require Windows x64 and a user-owned official
client; run `ark-cli battle setup`, then `ark-cli battle run JOB.json`. The Windows release also
embeds the third-party TypeTreeGeneratorAPI, Capstone and TPK files and a prebuilt probe (notices in
`THIRD_PARTY.md`), and `setup` downloads the prebuilt owned Player from this project's GitHub
Release. Other builds download the pinned third-party wheels during `setup` and compile the probe,
which needs Rust and a Windows SDK. Building the owned Player locally (`setup --build-player`)
needs Unity 2021.3.39f1, a valid Unity license, the IL2CPP module and MSVC.

Legacy Android/operator extraction also requires an independently configured ArkUnpacker
installation through `ARK_UNPACKER_ROOT` and `ARK_UNPACKER_PYTHON`. That tool and its Python
environment are supplied separately. Ingestion commands run directly through the library;
they do not launch another Ark CLI executable.

Help and version exit with 0; invalid arguments exit with 2 before workflows run. Solver and
ingestion runtime errors exit with 1. Battle reports with `passed: false` exit with 1, and battle
runtime errors exit with 2. A battle that ends in an original loss still exits with 0 when its
execution checks pass; read `battle_outcome.status` in the report for the result. Other successful commands exit with 0.

To build from source, including crates.io installation, install Rust 1.89 or newer and
FlatBuffers `flatc` 25.12.19 on `PATH`. From a workspace checkout, run:

```fish
cargo build --locked -p arknights-cli --release
cargo install --path src/ark_cli --locked
```

After the initial crate release, install directly with `cargo install arknights-cli --locked`.
The CLI source package includes its unit and integration tests. Test fixtures are generated
in temporary directories and require no client files or repository checkout.

To package a local host build on macOS or Linux:

```sh
bash src/ark_cli/tools/package.sh
bash src/ark_cli/tools/package.sh --skip-build --debug
```

On Windows, invoke the PowerShell packaging script. The release workflow first assembles the native
bundle (pinned wheels plus `ark-cli battle build-probe`) and builds with `bundled-native`:

```powershell
./src/ark_cli/tools/package.ps1
./src/ark_cli/tools/package.ps1 -SkipBuild -Profile debug
$env:ARK_NATIVE_BUNDLE_DIR = 'path/to/bundle'; ./src/ark_cli/tools/package.ps1 -Features bundled-native
```

The owned Player archive, the battle runtime, is published separately under the tag
`ark-battle-runtime-unity2021.3.39f1-r2`, from `ark-cli battle package-player`; its SHA-256 is
pinned in `src/battle_runtime/src/tools.rs`.

Packaging defaults to a locked release build. Debug flags use `target/debug` for local checks;
`--skip-build` / `-SkipBuild` packages an existing executable. The Unix script also honors
`CARGO_TARGET_DIR`; the Windows script reads Cargo's effective target directory. Outputs go to
`assets/releases/ark-cli-VERSION-PLATFORM-ARCH`, with a neighboring ZIP on Windows or `.tar.gz`
on Unix, plus an archive `.sha256` file. Existing outputs are refused. Packaging only copies
files and never publishes or uploads an archive.

For a coordinated crates.io release, keep all five internal crate versions aligned and
publish in this order: `ark-battle-runtime-probe`, `ark-unity`, `ark-solver`, `ark-battle-runtime`,
then `arknights-cli`. The first three can be published independently; the battle runtime requires
the probe source provider and Unity crate, and the CLI requires the solver and battle runtime.
Wait for each required dependency version to become available before publishing its dependents.
From a clean checkout, verify the workspace gates and review each source package using
`cargo package --workspace --locked`. This verifies the source archives together even before
the internal dependency versions have been published. Then explicitly publish with
`cargo publish -p PACKAGE --locked` in that order. Cargo uses local path dependencies during
development and the declared crate versions for published source packages.

The repository's `.github/workflows/release.yml` builds and packages Windows x64, macOS arm64,
and Linux x64 executables. A pushed tag such as `ark-cli-v0.2.0` attaches the archives and SHA256
files to the corresponding GitHub Release. A manual `workflow_dispatch` run builds and validates
packages without creating a release. Publishing the crates and pushing a release tag require
explicit release actions; local build and packaging commands perform no remote publication.
