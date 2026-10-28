#!/usr/bin/env bash
# Remove every installed version of a mod (folder, zip or symlink).
# Usage: uninstall-mod.sh <mods dir> <mod name>
source "$(dirname "$0")/lib.sh"

mods=$1
name=$2

require_factorio_closed
for old in "$mods/$name" "$mods/$name"_*; do
    [[ -e "$old" || -L "$old" ]] || continue
    echo "Removing $old"
    rm -rf "$old"
done
