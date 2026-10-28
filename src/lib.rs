//! pastel: recolors Factorio's graphics into a brighter, pastel look.
//!
//! The game's PNGs are read from `data/<folder>/graphics` (copied in from the
//! game), recolored according to `pastel.toml`, and every file that changes
//! is written to `data/<folder>/updated_graphics`. From those, `pastel mod`
//! assembles a Factorio mod that swaps the game's sprites for the new ones.
//! Files that stay unchanged get no output; the game keeps using its own.
//!
//! How it fits together:
//!
//! - [`config`]: pastel.toml, its categories and the `[classify]` rules that
//!   put every PNG into one of them.
//! - [`color`]: sRGB <-> OKLab / OKLCH conversion and gamut mapping.
//! - [`image`]: PNG reading and writing that keeps each file's exact format,
//!   float views of the pixels, and drawing helpers for previews.
//! - [`recolor`]: the color transforms, one module per category type.
//! - [`project`]: the project folder: source files, category statistics, and
//!   recoloring a single file.
//! - [`manifest`]: the record of the last build, so unchanged files are skipped.
//! - [`commands`]: what each `pastel` command does.
//! - [`cli`]: the command line, and running the command it names.
pub mod cli;
pub mod color;
pub mod commands;
pub mod config;
pub mod image;
pub mod manifest;
pub mod project;
pub mod recolor;
