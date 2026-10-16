#!/usr/bin/env fish

function print_usage
	printf '%s\n' \
		'Usage:' \
		'  unpack-official-battle-prefabs.fish APK GAMEDATA_SNAPSHOT OUTPUT_DIR [CHARACTER_KEY ...]' \
		'' \
		'Thin fish wrapper for `ark-cli ingest extract-battle-data`.' \
		'The output directory must not already exist.'
end

argparse 'h/help' -- $argv
or begin
	print_usage >&2
	exit 2
end

if set -q _flag_help
	print_usage
	exit 0
end

if test (count $argv) -lt 3
	print_usage >&2
	exit 2
end

set -l apk_path "$argv[1]"
set -l gamedata_snapshot "$argv[2]"
set -l output_dir "$argv[3]"
set -l character_keys $argv[4..-1]

set -l script_dir (cd (dirname (status --current-filename)); and pwd)
set -l repo_root (cd "$script_dir/../../.."; and pwd)
or begin
	echo 'error: could not resolve repository root' >&2
	exit 1
end

set -l command cargo run -p arknights-cli -- ingest extract-battle-data \
	"$apk_path" \
	--gamedata-snapshot "$gamedata_snapshot" \
	--output "$output_dir"
for character_key in $character_keys
	set -a command --operator "$character_key"
end

cd "$repo_root"; or exit 1
command $command
