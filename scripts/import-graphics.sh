#!/usr/bin/env bash
# Copy the game's vanilla graphics into data/<folder>/graphics.
# Reads <folder>/graphics.vanilla instead when recolored graphics are installed.
# Usage: import-graphics.sh <game data dir> <folder>...
source "$(dirname "$0")/lib.sh"

game_data=$1
shift

for f in "$@"; do
    src="$game_data/$f/graphics"
    [[ -d "$src.vanilla" ]] && src="$src.vanilla"
    [[ -d "$src" ]] || die "Missing $src. Set factorio_dir in pastel.toml."
    echo "Copying $src -> data/$f/graphics"
    mkdir -p "data/$f"
    rsync -a --delete "$src/" "data/$f/graphics/"
done
