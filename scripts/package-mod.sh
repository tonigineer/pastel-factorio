#!/usr/bin/env bash
# Zip an assembled mod folder (<name>_<version>/) the way Factorio expects.
# PNGs are stored as is; they are already compressed.
# Usage: package-mod.sh <assembled mod dir> <output zip>
source "$(dirname "$0")/lib.sh"

src=$(realpath "$1")
out=$(realpath -m "$2")

[[ -d "$src" ]] || die "Missing $src. Run 'just assemble' first."
mkdir -p "$(dirname "$out")"
rm -f "$out"
(cd "$(dirname "$src")" && zip -qrX -n .png "$out" "$(basename "$src")/")
echo "Wrote $out"
