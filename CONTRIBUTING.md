# Contributing

## Checks

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 -m unittest discover -s src/ark_solver/tools/tests   # when changing ingestion helpers
```

The solver's artifact build needs FlatBuffers `flatc` 25.12.19 on `PATH` (for example
`brew install flatbuffers`, or the binaries from the FlatBuffers release). Initialize the one code
submodule with `git submodule update --init --recursive -- vendor/ark-unpacker`.
