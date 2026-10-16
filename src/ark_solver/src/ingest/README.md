# Ark Ingest

Asset prep CLI. **Reads** files from a user-owned Arknights client and
**writes** standardized local asset and data layouts under caller-selected paths.

This tool does **not** bundle or redistribute game assets. It only works
with user-supplied files and public hot-update metadata.

Local unpacker workflows require the Fish/Python setup described below.

When an output option is omitted, `ark-cli ingest` loads `rogue.toml` (or `$ROGUE_CONFIG`) and uses
its configured roots. Relative config values resolve against the config file, not the process
working directory. Writer workflows create missing destination directories on demand; loading
configuration alone never changes the filesystem.

## Build
```fish
cargo build -p arknights-cli
```

## `identify-spine`
Scan already extracted `.skel` / `.json` files and report the detected
Spine runtime version.

```fish
cargo run -p arknights-cli -- ingest identify-spine /path/to/extracted-client
cargo run -p arknights-cli -- ingest identify-spine path/to/some.skel
```

## `fetch`
Download matching hot-update bundles from Hypergryph's public,
unauthenticated CDN. The command restores the `.ab` payload from the CDN's
`.dat` wrapper and mirrors the manifest's logical path under
`<client_dir>/hot_update/` by default.

```fish
# Preview exactly what would download.
cargo run -p arknights-cli -- ingest fetch --dry-run --filter 'chararts/char_002_amiya.ab'

# Download every Amiya operator art bundle into the configured client root.
cargo run -p arknights-cli -- ingest fetch --filter 'chararts/char_*_amiya.ab'

# Download everything into a custom directory.
cargo run -p arknights-cli -- ingest fetch --all --to path/to/hot_update
```

Notes:
- `--filter` is required unless `--all` is passed.
- Existing files are skipped when their MD5 already matches the manifest.
- `--force` re-downloads even valid files.
- `--jobs` controls Rayon download parallelism (default `4`).

## `fetch-gamedata`

Download a revision-pinned thin gamedata snapshot. The default profile writes a minimal table set
to the configured `gamedata_dir`:

```fish
cargo run -p arknights-cli -- ingest fetch-gamedata

# Fetch the analysis tables and only BD-7's level payload into an explicit path.
cargo run -p arknights-cli -- ingest fetch-gamedata \
	--profile analysis \
	--stage BD-7 \
	--to /path/to/analysis-gamedata
```

`--revision` pins a Git commit or ref, repeatable `--stage` filters analysis levels, and
`--include-levels` requests the complete level tree instead of stage filters. The snapshot
manifest records the resolved revision and verifies reusable local files.

## `extract`
Wrap ArkUnpacker and normalize one operator bundle into a locked, standardized output layout.
The output defaults to the configured `assets_dir`; pass `--output` to override it.

### ArkUnpacker setup
ArkUnpacker's Python dependencies are intentionally **not** committed.
Create them locally before running `extract`:

```fish
src/ark_solver/tools/setup-ark-unpacker.fish
```

That script creates `vendor/ark-unpacker/.venv/` with the exact manual
package set ArkUnpacker needs on macOS / Python 3.12.

### Usage
```fish
cargo run -p arknights-cli -- ingest extract /path/to/char_002_amiya.ab

# Override the output operator id when the input path is ambiguous.
cargo run -p arknights-cli -- ingest extract path/to/bundles \
	--output /path/to/assets \
	--operator-id char_002_amiya
```

Output layout:
```text
<output>/operators/<id>/
  front.skel
  front.atlas
  front.png
  back.skel
  back.atlas
  back.png
  build.skel
  build.atlas
  build.png
  portrait.png
  manifest.json
```

If a variant is missing from the source bundle, its key is omitted from
`manifest.json`.

## `extract-battle-data`

Build a verified battle-prefab snapshot directly from a user-supplied Android
APK. The command verifies the referenced gamedata snapshot first, then exports
lossless TypeTree fields and serialization metadata with the vendored UnityPy. Its output defaults
to `<assets_dir>/analysis/battle-prefabs`; an explicit `--output` overrides that path:

```fish
src/ark_solver/tools/setup-ark-unpacker.fish
set gamedata_dir /path/from/rogue.toml/gamedata
set analysis_dir /path/from/rogue.toml/assets/analysis

cargo run -p arknights-cli -- ingest extract-battle-data \
	path/to/official.apk \
	--gamedata-snapshot $gamedata_dir \
	--output $analysis_dir/battle-prefabs \
	--operator sbell2 \
	--operator char_2025_shu
```

The seven common bundles (`[uc]skills`, `[uc]dynamicabilities`, `[uc]tiles`,
`[uc]envsystems`, `[uc]equips`, `[uc]globalbuffs`, and `[uc]projectiles`) are
always included. Repeat `--operator` to select gameplay charpacks. A short key
such as `sbell2` must resolve uniquely; otherwise pass a full id such as
`char_1046_sbell2`.
The output path must not exist. On macOS and Linux, the complete snapshot is
published with one atomic no-replace rename, so a racing writer cannot be
overwritten.

Rich JSON is substantially larger than the compressed bundles (the local
`[uc]skills` smoke was about 32 MiB). Export only the operators needed by the
solver roster; an eager all-charpack snapshot is intentionally unsupported.

Output layout:

```text
manifest.json
typetree/common/<bundle>.json
typetree/charpack/<operator-id>.json
```

`manifest.json` records the complete APK and per-bundle SHA-256 provenance,
export hashes, tool versions, the bound gamedata Git revision, and a stable
64-character `mechanics_revision`. Each rich JSON document retains CAB and
serialized-file identity, Unity version, external references, class/script
identities, canonical TypeTree schema metadata, optional resolved MonoScript
class names, and raw fields. It deliberately contains no operator-specific
mechanic translation or semantic IR.

The fish wrapper accepts the same inputs with positional character keys:

```fish
src/ark_solver/tools/unpack-official-battle-prefabs.fish \
	path/to/official.apk \
	$gamedata_dir \
	$analysis_dir/battle-prefabs \
	sbell2 shu
```

Keep snapshots under configured ignored roots or outside the repository. The default `assets/`
and `client/` roots are ignored; custom in-worktree paths must be ignored explicitly. Never commit
an APK, AssetBundle, or generated TypeTree document. Archive
integrity alone does not prove official provenance; use an unmodified client.
The APK does not identify the Kengxxiao Git commit, so pass the gamedata
snapshot that matches that client build; the command records and later enforces
the supplied pairing but cannot independently prove it.
