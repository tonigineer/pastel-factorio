#!/usr/bin/env bash
# Write the game's prototype data (data-raw-dump.json) with the official
# mods enabled. Runs only Factorio's data stage: no window, no graphics.
# Uses its own config and mod folder, so your settings and mods are untouched.
# Usage: dump-data.sh <factorio dir> <work dir>
source "$(dirname "$0")/lib.sh"

factorio=$1
work=$(realpath -m "$2")

[[ -x "$factorio/bin/x64/factorio" ]] || die "Missing $factorio/bin/x64/factorio. Set factorio_dir in pastel.toml."
mkdir -p "$work/mods" "$work/write"
printf '[path]\nread-data=%s/data\nwrite-data=%s/write\n' "$factorio" "$work" > "$work/config.ini"

mods='{"name": "base", "enabled": true}'
for m in elevated-rails quality recycler space-age; do
    [[ -d "$factorio/data/$m" ]] && mods+=", {\"name\": \"$m\", \"enabled\": true}"
done
echo "{\"mods\": [$mods]}" > "$work/mods/mod-list.json"

echo "Reading the game's prototype data..."
"$factorio/bin/x64/factorio" -c "$work/config.ini" --mod-directory "$work/mods" --dump-data > "$work/factorio.log" 2>&1 \
    || die "Factorio failed; see $work/factorio.log"
[[ -f "$work/write/script-output/data-raw-dump.json" ]] || die "No data-raw-dump.json; see $work/factorio.log"
