//! `pastel scan`: learn from the game's own prototype data how files are used.
//!
//! File names don't say how the game draws a file, but its prototype data
//! does: `factorio --dump-data` writes all of it to `data-raw-dump.json`.
//! This command reads that file and rewrites two generated blocks in
//! pastel.toml:
//!
//! - Rules. Sprites drawn additively or as light (and tile lightmaps) get the
//!   `light` category: there, black means "add nothing", and a lifted black
//!   would show up as a square around the effect. Files that are data rather
//!   than pictures (tile effect maps, masks, shader noise) stay unchanged.
//! - Keep regions. Tile transition sheets hold several layers side by side;
//!   the mask layer decides where one tile cuts into the other, so every
//!   column from the mask's `x_offset` on stays unchanged.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::config::{CONFIG_FILE, PASSTHROUGH};
use crate::project::Project;

/// Category for sprites drawn additively or as light.
const LIGHT: &str = "light";
/// Marker lines around the generated blocks in pastel.toml.
const RULES: (&str, &str) = (
    "# BEGIN generated rules (`just scan`)",
    "# END generated rules",
);
const KEEP: (&str, &str) = (
    "# BEGIN generated keep (`just scan`)",
    "# END generated keep",
);
/// Tile layouts whose spritesheets are data (masks and effect maps).
const DATA_LAYOUTS: &[&str] = &[
    "effect_map_layout",
    "mask_layout",
    "background_mask_layout",
    "auxiliary_effect_mask_layout",
];

/// What the prototype data says about the files it uses.
#[derive(Debug, Default)]
struct Findings {
    /// For every sprite file: is each use additive/light (true) or normal?
    sprite_uses: BTreeMap<String, BTreeSet<bool>>,
    /// Tile lightmaps: drawn as light.
    lightmaps: BTreeSet<String>,
    /// Masks, effect maps and shader textures: data, not pictures.
    data: BTreeSet<String>,
    /// Transition sheets: the column where their mask layer starts.
    mask_columns: BTreeMap<String, u64>,
}

/// Does this sprite definition draw additively or as light?
fn draws_as_light(t: &Map<String, Value>) -> bool {
    let additive = matches!(
        t.get("blend_mode").and_then(Value::as_str),
        Some("additive" | "additive-soft")
    );
    additive || t.get("draw_as_light").and_then(Value::as_bool) == Some(true)
}

/// The image files a sprite definition names.
fn files_of(t: &Map<String, Value>) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    if let Some(f) = t.get("filename").and_then(Value::as_str) {
        out.push(f);
    }
    if let Some(list) = t.get("filenames").and_then(Value::as_array) {
        out.extend(list.iter().filter_map(Value::as_str));
    }
    if let Some(stripes) = t.get("stripes").and_then(Value::as_array) {
        out.extend(stripes.iter().filter_map(|s| s.get("filename")?.as_str()));
    }
    out
}

/// "__space-age__/graphics/x.png" as the project path "space-age/graphics/x.png".
fn project_path(game_path: &str) -> Option<String> {
    let rest = game_path.strip_prefix("__")?;
    let (module, rest) = rest.split_once("__/")?;
    Some(format!("{module}/{rest}"))
}

impl Findings {
    /// Walk the prototype data; `parent` is the key `v` was found under.
    fn collect(&mut self, v: &Value, parent: &str) {
        match v {
            Value::Object(t) => {
                let light = draws_as_light(t);
                for f in files_of(t) {
                    if let Some(rel) = project_path(f) {
                        self.sprite_uses.entry(rel).or_default().insert(light);
                    }
                }
                if let Some(rel) = t
                    .get("spritesheet")
                    .and_then(Value::as_str)
                    .and_then(project_path)
                {
                    if DATA_LAYOUTS.contains(&parent) {
                        self.data.insert(rel.clone());
                    }
                    if parent == "lightmap_layout" {
                        self.lightmaps.insert(rel.clone());
                    }
                    let mask_x = t.get("layout").and_then(|l| l.get("mask")?.get("x_offset"));
                    if let Some(x) = mask_x.and_then(Value::as_u64) {
                        let column = self.mask_columns.entry(rel).or_insert(x);
                        *column = (*column).min(x);
                    }
                }
                // A sprite's stripes only name its files (handled above); walking
                // them again would count each stripe as a normally drawn sprite.
                for (k, x) in t.iter().filter(|(k, _)| k.as_str() != "stripes") {
                    self.collect(x, k);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| self.collect(x, parent)),
            _ => {}
        }
    }

    /// The noise textures tile effects (shaders) use: data, not pictures.
    /// Their `image_textures` are pictures (lava, wetland surfaces) and are
    /// recolored like the rest of their planet.
    fn collect_shader_noise(&mut self, v: &Value, key: &str) {
        match v {
            Value::String(s) if key.contains("noise") => {
                if let Some(rel) = project_path(s).filter(|r| r.ends_with(".png")) {
                    self.data.insert(rel);
                }
            }
            Value::Object(t) => {
                for (k, x) in t {
                    // A texture's own fields (filename, size) keep its key.
                    let k = if k == "filename" { key } else { k.as_str() };
                    self.collect_shader_noise(x, k);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| self.collect_shader_noise(x, key)),
            _ => {}
        }
    }
}

/// Can `path` be written as a rule that matches just that file? (Rules are
/// globs; game file names never use glob characters, but check anyway.)
fn is_literal(path: &str) -> bool {
    !path.contains(['*', '?', '{', '}', '"'])
}

/// `config` with the lines between the `markers` replaced by `lines`, each
/// indented like the end marker.
fn replace_block(config: &str, markers: (&str, &str), lines: &[String]) -> Result<String> {
    let (Some(begin), Some(end)) = (config.find(markers.0), config.find(markers.1)) else {
        bail!(
            "{CONFIG_FILE} has no generated block: add the lines `{}` and `{}`",
            markers.0,
            markers.1
        );
    };
    let begin = begin + config[begin..].find('\n').map_or(0, |n| n + 1);
    let line_start = config[..end].rfind('\n').map_or(0, |n| n + 1);
    let indent = &config[line_start..end];
    let block: String = lines.iter().map(|l| format!("{indent}{l}\n")).collect();
    Ok(format!(
        "{}{}{}",
        &config[..begin],
        block,
        &config[line_start..]
    ))
}

/// Read `dump` (Factorio's data-raw-dump.json) and rewrite the generated blocks.
pub fn run(p: &Project, dump: &Path) -> Result<()> {
    let text = fs::read_to_string(dump).with_context(|| format!("reading {}", dump.display()))?;
    let raw: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", dump.display()))?;
    let mut found = Findings::default();
    found.collect(&raw, "");
    found.collect_shader_noise(&raw["tile-effect"], "");

    // Only our own files, and only files whose name can be a rule. A sprite
    // that is also drawn normally somewhere keeps its usual category.
    let ours: BTreeSet<&str> = p.pngs().map(String::as_str).collect();
    let usable = |rel: &&String| {
        let ok = is_literal(rel);
        if !ok {
            println!("    skipped {rel}: its name can't be written as a rule");
        }
        ours.contains(rel.as_str()) && ok
    };
    let data: Vec<&String> = found.data.iter().filter(usable).collect();
    let light: BTreeSet<&String> = found
        .sprite_uses
        .iter()
        .filter(|(_, how)| !how.contains(&false))
        .map(|(rel, _)| rel)
        .chain(&found.lightmaps)
        .filter(|rel| !found.data.contains(*rel))
        .filter(usable)
        .collect();
    let keep: Vec<(&String, &u64)> = found
        .mask_columns
        .iter()
        .filter(|(rel, _)| usable(rel))
        .collect();

    let rules: Vec<String> = data
        .iter()
        .map(|rel| format!("{{ match = \"{rel}\", category = \"{PASSTHROUGH}\" }},"))
        .chain(
            light
                .iter()
                .map(|rel| format!("{{ match = \"{rel}\", category = \"{LIGHT}\" }},")),
        )
        .collect();
    let keeps: Vec<String> = keep
        .iter()
        .map(|(rel, x)| format!("{{ match = \"{rel}\", x_from = {x} }},"))
        .collect();

    let path = p.root.join(CONFIG_FILE);
    let config =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let config = replace_block(&config, RULES, &rules)?;
    let config = replace_block(&config, KEEP, &keeps)?;
    fs::write(&path, config).with_context(|| format!("writing {}", path.display()))?;
    println!(
        "Wrote to {CONFIG_FILE}: {} sprite(s) drawn as light, {} data file(s) to leave unchanged, \
         {} transition sheet(s) with a mask column.",
        light.len(),
        data.len(),
        keep.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_light_data_and_masks() {
        let raw = json!({
            "explosion": {"a": {"filename": "__base__/graphics/glow.png", "blend_mode": "additive"}},
            "lamp": {"a": {"filename": "__base__/graphics/lamp.png", "draw_as_light": true},
                     "b": {"filename": "__base__/graphics/lamp.png"}},
            "x": {"stripes": [{"filename": "__space-age__/graphics/s.png"}], "blend_mode": "additive-soft"},
            "tile": {"lava": {"transitions": [{
                "spritesheet": "__space-age__/graphics/t.png",
                "layout": {"mask": {"x_offset": 2176}},
                "effect_map_layout": {"spritesheet": "__space-age__/graphics/fx.png"},
                "lightmap_layout": {"spritesheet": "__space-age__/graphics/lm.png"}
            }]}}
        });
        let mut f = Findings::default();
        f.collect(&raw, "");
        assert_eq!(
            f.sprite_uses["base/graphics/glow.png"],
            BTreeSet::from([true])
        );
        assert_eq!(
            f.sprite_uses["base/graphics/lamp.png"],
            BTreeSet::from([false, true])
        );
        assert_eq!(
            f.sprite_uses["space-age/graphics/s.png"],
            BTreeSet::from([true])
        );
        assert_eq!(f.mask_columns["space-age/graphics/t.png"], 2176);
        assert!(f.data.contains("space-age/graphics/fx.png"));
        assert!(f.lightmaps.contains("space-age/graphics/lm.png"));
    }

    #[test]
    fn shader_noise_is_data_but_images_are_not() {
        let mut f = Findings::default();
        f.collect_shader_noise(
            &json!({"lava": {"water": {
                "noise_textures": [{"filename": "__space-age__/graphics/noise.png", "size": 512}],
                "image_textures": [{"filename": "__space-age__/graphics/lava.png"}]
            }}}),
            "",
        );
        assert!(f.data.contains("space-age/graphics/noise.png"));
        assert!(!f.data.contains("space-age/graphics/lava.png"));
    }

    #[test]
    fn replaces_only_the_block() {
        let config =
            "a\n  # BEGIN generated rules (`just scan`)\n  old,\n  # END generated rules\nb\n";
        let out = replace_block(config, RULES, &["new,".into()]).unwrap();
        assert_eq!(
            out,
            "a\n  # BEGIN generated rules (`just scan`)\n  new,\n  # END generated rules\nb\n"
        );
    }

    #[test]
    fn rejects_glob_characters() {
        assert!(is_literal("base/graphics/entity/lab/lab.png"));
        assert!(!is_literal("a/b{1,2}.png"));
    }
}
