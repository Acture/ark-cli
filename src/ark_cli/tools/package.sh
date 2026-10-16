#!/usr/bin/env bash
set -euo pipefail

usage() {
	printf 'Usage: bash package.sh [--skip-build] [--debug]\n'
}

fail() {
	printf '%s\n' "$1" >&2
	exit 1
}

profile=release
skip_build=false
for argument in "$@"; do
	case "$argument" in
		--skip-build) skip_build=true ;;
		--debug) profile=debug ;;
		-h | --help)
			usage
			exit 0
			;;
		*)
			printf 'Unexpected argument: %s\n' "$argument" >&2
			usage >&2
			exit 2
			;;
	esac
done

script_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
workspace_root="$(cd -- "$script_directory/../../.." && pwd -P)"
cd -- "$workspace_root"

case "$(uname -s)" in
	Darwin) platform=macos ;;
	Linux) platform=linux ;;
	*) fail 'Use package.ps1 to package a Windows executable.' ;;
esac

if [[ "$skip_build" == false ]]; then
	build_arguments=(build --locked -p arknights-cli)
	if [[ "$profile" == release ]]; then
		build_arguments+=(--release)
	fi
	cargo "${build_arguments[@]}"
fi

target_directory="$workspace_root/target"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
	target_directory="$(cd -- "$CARGO_TARGET_DIR" && pwd -P)"
fi
compiler_details="$(rustc -vV)"
host_pattern='^host: ([A-Za-z0-9_]+)-'
architecture=''
while IFS= read -r line; do
	if [[ "$line" =~ $host_pattern ]]; then
		architecture="${BASH_REMATCH[1]}"
	fi
done <<<"$compiler_details"
[[ -n "$architecture" ]] || fail 'Cannot determine the host build architecture.'

executable="$target_directory/$profile/ark-cli"
[[ -x "$executable" ]] ||
	fail "Executable not found at $executable; build that profile before using --skip-build."
version_output="$("$executable" --version)"
version_pattern='^ark-cli ([A-Za-z0-9.+-]+)$'
[[ "$version_output" =~ $version_pattern ]] ||
	fail "Cannot determine the executable version: $version_output"
cli_version="${BASH_REMATCH[1]}"

package_name="ark-cli-$cli_version-$platform-$architecture"
mkdir -p -- "$workspace_root/assets/releases"
release_root="$(cd -- "$workspace_root/assets/releases" && pwd -P)"
[[ "$release_root" == "$workspace_root/assets/releases" ]] ||
	fail "Release root is outside the intended workspace path: $release_root"
destination="$release_root/$package_name"
archive="$destination.tar.gz"
checksum="$archive.sha256"
for output_path in "$destination" "$archive" "$checksum"; do
	if [[ -e "$output_path" || -L "$output_path" ]]; then
		fail "Refusing to overwrite existing release output: $output_path"
	fi
done

mkdir -- "$destination"
cp -- "$executable" "$destination/ark-cli"
cp -- "$script_directory/../RELEASE.md" "$destination/README.md"
cp -- "$workspace_root/LICENSE" "$workspace_root/NOTICE" "$destination/"
printf "assets_dir = 'assets'\nclient_dir = 'client'\ngamedata_dir = 'client/_gamedata'\n" \
	>"$destination/rogue.toml"
tar -czf "$archive" -C "$release_root" "$package_name"

if command -v shasum >/dev/null; then
	checksum_output="$(shasum -a 256 "$archive")"
elif command -v sha256sum >/dev/null; then
	checksum_output="$(sha256sum "$archive")"
else
	fail 'Install shasum or sha256sum to calculate the archive checksum.'
fi
digest="${checksum_output%% *}"
[[ "$digest" =~ ^[[:xdigit:]]{64}$ ]] || fail "Cannot parse archive checksum: $checksum_output"
printf '%s  %s.tar.gz\n' "$digest" "$package_name" >"$checksum"
printf 'Packaged %s\nArchive %s\nSHA256 %s\n' "$destination" "$archive" "$checksum"
