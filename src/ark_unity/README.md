# Unity resource and Player support

`ark-unity` is the Rust package for the owned Unity 2021.3.39f1 Player's asset and platform layer.
It is used by [`ark-battle-runtime`](../battle_runtime/README.md), with no dependency on Ark Solver's
battle model and no Python worker.

- `assets`: V077 manifests, hot-update routing and MD5 verification; UnityFS/LZ4AK; SerializedFile
  parsing, byte-preserving cloning, TypeTree restoration, CAB closure and original service settings.
- `build`: official Unity CLI, license and toolchain checks, Editor build invocation, complete
  owned Player inventory verification.
- `player`: bounded owned-process execution, hidden Unity windows, observer parsing and runtime gates.
- `io`: BOM-tolerant JSON, SHA pins, destination and owned-path checks.

Asset formats are deliberately limited to the reviewed client/Unity shapes. Original object bytes
must roundtrip exactly; generated serialized files are independently reparsed with
`unity-asset-binary`. Unsupported layouts fail explicitly. The LZ4AK adaptation's upstream BSD
notice is in `src/assets/ARK_UNPACKER_LICENSE.txt`.

## Local native metadata inputs

Stripped original IL2CPP types require two upstream tooling inputs. Both stay outside source
control; neither needs a Python interpreter at runtime:

- TypeTreeGeneratorAPI 0.0.10 native Windows library and its `capstone.dll` dependency. Place them
  in `STATE/tools/typetreegenerator-0.0.10/`, or set `ARK_TYPE_TREE_GENERATOR` to the API DLL.
- Same-version Unity release TypeTree data, such as UnityPy 1.25.4's `lzma.tpk`. Place it at
  `STATE/tools/unitypy-1.25.4/lzma.tpk`, or set `ARK_UNITY_TPK` to the file.

These are read directly through the native ABI and Rust TPK decoder. Their bytes, original
GameAssembly and metadata hashes are recorded as provenance. Native generated descriptions and
release descriptions are admitted only after every original object roundtrips byte-identically.
Keep the upstream licenses with local distributions of these dependencies.

## Validation

```powershell
cargo test -p ark-unity
cargo clippy -p ark-unity --all-targets -- -D warnings
```

The installed-client resource qualification is ignored by default. It requires `ARK_PC_CLIENT`,
staged native inputs, and existing qualified reference plan/Player records in `assets/battle-runtime`.
Use `ARK_RESOURCE_REFERENCE_PLAN` and `ARK_RESOURCE_REFERENCE_PLAYER` to select explicit records:

```powershell
cargo test -p ark-unity assets::catalog::tests::installed_client -- --ignored --nocapture
```
