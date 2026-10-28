#!/usr/bin/env bash
# Put the vanilla graphics back from <folder>/graphics.vanilla.
# Usage: restore-graphics.sh <game data dir> <folder>...
source "$(dirname "$0")/lib.sh"

game_data=$1
shift

require_factorio_closed
for f in "$@"; do
    dst="$game_data/$f/graphics"
    if [[ -d "$dst.vanilla" ]]; then
        echo "Restoring $dst"
        rm -rf "$dst"
        mv "$dst.vanilla" "$dst"
    else
        echo "No backup at $dst.vanilla, leaving $dst alone"
    fi
done
