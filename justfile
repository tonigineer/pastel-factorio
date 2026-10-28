set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# The game paths come from the [paths] table in pastel.toml; a leading ~ is expanded.
config := "pastel.toml"
factorio_dir_raw := shell('sed -n "s/^factorio_dir *= *\"\([^\"]*\)\".*/\1/p" "$1"', config)
user_dir_raw := shell('sed -n "s/^factorio_user_dir *= *\"\([^\"]*\)\".*/\1/p" "$1"', config)
factorio_dir := if factorio_dir_raw == "" { error("Set factorio_dir under [paths] in pastel.toml") } else { replace_regex(factorio_dir_raw, "^~", home_directory()) }
user_dir := if user_dir_raw == "" { error("Set factorio_user_dir under [paths] in pastel.toml") } else { replace_regex(user_dir_raw, "^~", home_directory()) }
game_data := factorio_dir / "data"
mods_dir := user_dir / "mods"

# Graphics folders under data/ that get recolored.
folders := "base core elevated-rails quality recycler space-age"

# The folders older versions of this project replaced in the game itself.
replaced_folders := "base core"

mod_name := "pastel-factorio"
mod_src := "mods" / mod_name
mod_version := shell('sed -n "s/.*\"version\": *\"\([^\"]*\)\".*/\1/p" "$1"', mod_src / "info.json")
mod_release := mod_name + "_" + mod_version
mod_dir := "build/mod" / mod_release
mod_zip := "build" / mod_release + ".zip"

# The recoloring tool, built on first use.
tool := "cargo run --quiet --release --"

[private]
default:
    @{{ just_executable() }} --justfile "{{ justfile() }}" --list

# --- setup ---------------------------------------------------------------

[doc('Build the pastel tool')]
[group('setup')]
setup:
    cargo build --release

[doc('Show the paths this justfile uses (set in pastel.toml)')]
[group('setup')]
paths:
    @echo "Factorio install:  {{ factorio_dir }}"
    @echo "Factorio mods:     {{ mods_dir }}"
    @echo "Mod:               {{ mod_release }}"

# --- recolor -------------------------------------------------------------

[doc("Copy the game's vanilla graphics into data/")]
[group('recolor')]
import-graphics:
    scripts/import-graphics.sh "{{ game_data }}" {{ folders }}

[doc('Recolor the graphics into data/*/updated_graphics (args go to `pastel build`)')]
[group('recolor')]
build *args:
    {{ tool }} build "$@"

[doc('Recolor one file and show before/after, e.g. `just preview grass-1.png --watch`')]
[group('recolor')]
preview name *args:
    {{ tool }} --preview "$@"

[doc("Find the sprites the game draws as light (from Factorio's own data) and update their rules")]
[group('recolor')]
scan:
    scripts/dump-data.sh "{{ factorio_dir }}" build/dump
    {{ tool }} scan build/dump/write/script-output/data-raw-dump.json

[doc('Re-check the recolored files against the originals')]
[group('recolor')]
validate:
    {{ tool }} validate

[doc('Before/after contact sheets per category (after a build)')]
[group('recolor')]
sheets *args:
    {{ tool }} sheets "$@"

[doc('Run any pastel command, e.g. `just pastel classify`')]
[group('recolor')]
pastel *args:
    {{ tool }} "$@"

# --- mod -----------------------------------------------------------------

[doc('Assemble the mod (Lua + recolored graphics) in build/mod/')]
[group('mod')]
assemble: build
    {{ tool }} mod "{{ mod_src }}"

[doc('Zip the mod into build/, for sharing')]
[group('mod')]
package: assemble
    scripts/package-mod.sh "{{ mod_dir }}" "{{ mod_zip }}"

# --- game ----------------------------------------------------------------

[doc('Quit Factorio first. Build the mod and install it into the mods folder')]
[group('game')]
install: assemble
    scripts/install-mod.sh "{{ mod_dir }}" "{{ mods_dir }}"

[doc('Quit Factorio first. Remove the mod from the mods folder')]
[group('game')]
uninstall:
    scripts/uninstall-mod.sh "{{ mods_dir }}" "{{ mod_name }}"

[doc("Quit Factorio first. Put back the game's own graphics if an older version replaced them")]
[group('game')]
restore-graphics:
    scripts/restore-graphics.sh "{{ game_data }}" {{ replaced_folders }}

# --- misc ----------------------------------------------------------------

[doc('Delete build output (keeps the imported vanilla graphics)')]
[group('misc')]
clean:
    rm -rf build data/*/updated_graphics
