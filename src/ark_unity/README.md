# Unity resource and Player support

`ark-unity` is the Rust package for the owned Unity 2021.3.39f1 Player's asset and platform layer.
It is used by [`ark-battle-runtime`](../battle_runtime/README.md), with no dependency on Ark Solver's
battle model and no Python worker.

- `assets`: V078 manifests and hot-update routing over the installed files; UnityFS/LZ4AK; SerializedFile
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

Stripped original types take their field descriptions from two inputs, neither of which needs a
Python interpreter at runtime:

- Script classes: the original runtime IL2CPP metadata, exported by the battle runtime's probe in
  the owned Player (one JSON object per class). Fields follow Unity's MonoBehaviour serialization
  rules, so the client's protected binary is never read statically.
- Engine classes: same-version Unity release TypeTree data, such as UnityPy 1.25.4's `lzma.tpk`.
  Place it at `STATE/tools/unitypy-1.25.4/lzma.tpk`, or set `ARK_UNITY_TPK` to the file.

The export and TPK bytes are recorded as provenance. Generated and release descriptions are
admitted only after every original object roundtrips byte-identically. Keep the upstream license
with local distributions of the TPK data.

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
