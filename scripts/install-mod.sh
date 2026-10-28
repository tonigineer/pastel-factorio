#!/usr/bin/env bash
# Install an assembled mod folder (<name>_<version>/) into the Factorio mods
# folder, replacing any other version of it and the older Pastel Water mod.
# Files are hard-linked when both folders are on the same disk.
# Usage: install-mod.sh <assembled mod dir> <mods dir>
source "$(dirname "$0")/lib.sh"

src=$(realpath "$1")
mods=$2
release=$(basename "$src")
name=${release%_*}

require_factorio_closed
[[ -d "$src" ]] || die "Missing $src. Run 'just assemble' first."
[[ -d "$mods" ]] || die "Missing $mods. Set factorio_user_dir in pastel.toml."

for old in "$mods/$name" "$mods/$name"_* "$mods/pastel-water" "$mods"/pastel-water_*; do
    [[ -e "$old" && "$old" != "$mods/$release" ]] || continue
    echo "Removing $old"
    rm -rf "$old"
done
echo "Installing $mods/$release"
rsync -a --delete --link-dest="$src" "$src/" "$mods/$release/"
