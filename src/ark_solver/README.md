# Ark Solver library

`ark-solver` owns the existing analysis, data ingestion and independent replay modules. The
[`ark-cli`](../ark_cli/README.md) application owns process entry points, command parsing and exit
policy. Its commands call this library directly.

`src/` keeps the existing conceptual module boundaries. `tests/` and `examples/` sit beside their
consumer, `schema/` feeds the FlatBuffers build, and `tools/` contains the optional legacy Android
ingestion exporter and setup helpers. This library does not depend on the battle runtime.

Configuration discovery uses `ROGUE_CONFIG`, the nearest working-directory/ancestor `rogue.toml`,
then a file beside the executable. With no file, built-in `assets`, `client` and
`client/_gamedata` paths resolve against the working directory. No build-machine path is read.

Legacy extraction discovers a checkout-local `vendor/ark-unpacker`, an executable-adjacent
`tools/ark-unpacker`, or an explicit `ARK_UNPACKER_ROOT`. `ARK_UNPACKER_PYTHON` overrides its Python
executable. The TypeTree exporter is embedded; the third-party tool and interpreter are external
inputs. Original-engine native execution uses the separate Rust host and Unity packages.

```fish
cargo test -p ark-solver
cargo run -p arknights-cli -- level --help
cargo run -p arknights-cli -- ingest --help
python3 -m unittest discover -s src/ark_solver/tools/tests
```
