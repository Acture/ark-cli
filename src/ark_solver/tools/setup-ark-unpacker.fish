#!/usr/bin/env fish

if not command -q uv
    echo "error: uv is required but was not found in PATH" >&2
    exit 1
end

set -l script_dir (cd (dirname (status filename)) && pwd)
set -l repo_root (cd "$script_dir/../../.." && pwd)
set -l ark_root "$repo_root/vendor/ark-unpacker"

if not test -f "$ark_root/pyproject.toml"
    echo "error: expected ArkUnpacker at $ark_root" >&2
    exit 1
end

cd "$ark_root"; or exit 1
uv venv --python 3.12; or exit 1
uv pip install --python .venv/bin/python \
    'bson~=0.5' \
    'flatbuffers~=25.12' \
    'numpy~=2.2' \
    'Pillow~=11.3' \
    'psutil~=7.2' \
    'pycryptodome~=3.22' \
    'pydub-ng~=0.1' \
    'rich~=14.3' \
    'spine-asset~=1.1' \
    'UnityPy~=1.25' \
    'wannacri~=0.3'; or exit 1

echo "ArkUnpacker environment ready at $ark_root/.venv"
