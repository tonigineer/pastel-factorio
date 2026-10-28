//! Loading pastel.toml and settings.toml, and classifying files into categories.
//!
//! pastel.toml describes how the recoloring works: game paths, groups,
//! categories and rules. settings.toml holds what a player tunes: the global
//! knobs, which groups are on, the named colors and the liquid colors. Its
//! values are merged over pastel.toml's when loading.
//!
//! Categories are kept as JSON values, as written in pastel.toml but with
//! `inherit` resolved and named colors ("$grass") expanded. The transforms
//! read their settings out of them with the helpers in [`value`].
pub mod glob;
pub mod value;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde_json::{Map, Value, json};

use self::glob::glob_to_regex;
use self::value::deep_merge;

/// Every category `type` there is (see [`crate::recolor`]).
const SPECIAL_TYPES: &[&str] = &["passthrough", "remap", "transfer", "shadow", "lut"];

/// One `[classify]` rule: files matching `pattern` belong to `category`.
#[derive(Debug)]
pub struct Rule {
    /// The glob as written in pastel.toml.
    pub pattern: String,
    regex: Regex,
    /// Name of a category defined under `[categories]`.
    pub category: String,
}

/// A region of matching files that is never recolored: every column from
/// `x_from` to the right edge.
#[derive(Debug)]
struct Keep {
    regex: Regex,
    x_from: usize,
}

/// A `[[groups]]` entry: a named set of files that can be switched off.
/// A file belongs to the first group with a pattern that matches it.
#[derive(Debug)]
pub struct Group {
    /// The group's name, as liquids refer to it.
    pub name: String,
    /// Whether the group is recolored at all; when off, its files stay vanilla.
    pub enabled: bool,
    /// Category for the group's files that no `[classify]` rule matches.
    pub category: Option<String>,
    patterns: Vec<Regex>,
}

/// Category that disabled groups fall back to.
pub const PASSTHROUGH: &str = "passthrough";

/// File name of the project config, in the project folder.
pub const CONFIG_FILE: &str = "pastel.toml";
/// File name of the player's settings, in the project folder (optional).
pub const SETTINGS_FILE: &str = "settings.toml";

/// A TOML file as JSON, or an empty table when `optional` and missing.
fn read_toml(path: &Path, optional: bool) -> Result<Value> {
    if optional && !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let doc: toml::Table =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(serde_json::to_value(doc)?)
}

/// The settings from pastel.toml.
#[derive(Debug)]
pub struct Config {
    /// Where the project config was read from.
    pub path: PathBuf,
    /// Where the player's settings were read from (it may not exist).
    pub settings_path: PathBuf,
    /// The `[classify]` rules, in order; the first one that matches wins.
    pub rules: Vec<Rule>,
    keep: Vec<Keep>,
    /// The `[[groups]]`, in order.
    pub groups: Vec<Group>,
    /// The `[liquids]` table, with named colors expanded.
    liquids: Map<String, Value>,
    /// The `[global]` table (an empty table when missing).
    pub globals: Value,
    /// Named colors from `[colors]`, used elsewhere as "$name".
    pub colors: Map<String, Value>,
    /// Every category with `inherit` resolved and named colors expanded.
    categories: BTreeMap<String, Value>,
}

impl Config {
    /// Read and check pastel.toml in the project folder `root`.
    pub fn load(root: &Path) -> Result<Config> {
        let path = root.join(CONFIG_FILE);
        let settings_path = root.join(SETTINGS_FILE);
        let mut doc = read_toml(&path, false)?;
        let settings = read_toml(&settings_path, true)?;
        // The player's [global], [colors] and [liquids] win over pastel.toml's.
        for key in ["global", "colors", "liquids"] {
            if doc[key].is_null() {
                doc[key] = json!({});
            }
            if let Some(own) = settings[key].as_object() {
                if key == "liquids"
                    && let Some(name) = own.keys().find(|n| doc[key].get(n.as_str()).is_none())
                {
                    bail!(
                        "{SETTINGS_FILE} sets liquid '{name}', which {CONFIG_FILE} doesn't define under [liquids]"
                    );
                }
                deep_merge(&mut doc[key], Value::Object(own.clone()));
            }
        }
        let switches = settings["groups"].as_object().cloned().unwrap_or_default();

        let mut rules = Vec::new();
        for r in doc["classify"]["rules"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let (Some(pattern), Some(category)) = (r["match"].as_str(), r["category"].as_str())
            else {
                bail!("every rule in [classify] needs `match` and `category`, got {r}");
            };
            rules.push(Rule {
                pattern: pattern.into(),
                regex: glob_to_regex(pattern),
                category: category.into(),
            });
        }

        let mut keep = Vec::new();
        for k in doc["classify"]["keep"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let (Some(pattern), Some(x_from)) = (k["match"].as_str(), k["x_from"].as_u64()) else {
                bail!("every entry in [classify] keep needs `match` and `x_from`, got {k}");
            };
            keep.push(Keep {
                regex: glob_to_regex(pattern),
                x_from: x_from as usize,
            });
        }

        let mut groups = Vec::new();
        for g in doc["groups"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(name) = g["name"].as_str() else {
                bail!("every [[groups]] entry needs a `name`, got {g}");
            };
            let patterns = g["match"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(glob_to_regex)
                        .collect()
                })
                .unwrap_or_default();
            let enabled = switches.get(name).unwrap_or(&g["enabled"]);
            groups.push(Group {
                name: name.into(),
                enabled: enabled.as_bool().unwrap_or(true),
                category: g["category"].as_str().map(str::to_string),
                patterns,
            });
        }

        for (name, on) in &switches {
            if !on.is_boolean() {
                bail!("{SETTINGS_FILE}: [groups] {name} must be true or false");
            }
            if !groups.iter().any(|g| &g.name == name) {
                bail!(
                    "{SETTINGS_FILE} switches group '{name}', which {CONFIG_FILE} doesn't define"
                );
            }
        }

        let globals = match &doc["global"] {
            Value::Null => json!({}),
            v => v.clone(),
        };
        // 1 is the pastel look as designed; lower blends back toward the game's
        // own lightness (brightness) or toward gray (saturation).
        for knob in ["brightness", "saturation"] {
            if let Some(v) = value::as_f64(&globals[knob])
                && !(0.0..=1.0).contains(&v)
            {
                bail!("[global] {knob} = {v} is out of range: use a value from 0 to 1");
            }
        }
        let colors = doc["colors"].as_object().cloned().unwrap_or_default();
        let raw = doc["categories"].as_object().cloned().unwrap_or_default();

        let mut cfg = Config {
            path,
            settings_path,
            rules,
            keep,
            groups,
            liquids: Map::new(),
            globals,
            colors,
            categories: BTreeMap::new(),
        };
        if let Value::Object(liquids) = cfg.expand_colors(doc["liquids"].clone())? {
            for (name, l) in &liquids {
                if let Some(g) = l["group"].as_str()
                    && !cfg.groups.iter().any(|x| x.name == g)
                {
                    bail!(
                        "liquid '{name}' names group '{g}', which isn't defined under [[groups]]"
                    );
                }
            }
            cfg.liquids = liquids;
        }
        for name in raw.keys() {
            let cat = cfg.resolve(&raw, name, 0)?;
            cfg.categories.insert(name.clone(), cat);
        }
        if !cfg.groups.is_empty() && !cfg.categories.contains_key(PASSTHROUGH) {
            bail!("[[groups]] need a category named '{PASSTHROUGH}' for switched-off groups");
        }
        for g in &cfg.groups {
            if let Some(c) = &g.category
                && !cfg.categories.contains_key(c)
            {
                bail!(
                    "group '{}' uses category '{c}', which isn't defined under [categories]",
                    g.name
                );
            }
        }
        for r in &cfg.rules {
            if !cfg.categories.contains_key(&r.category) {
                bail!(
                    "category '{}' is used in a rule but not defined under [categories]",
                    r.category
                );
            }
        }
        Ok(cfg)
    }

    /// One category's settings with `inherit` applied (the category's own
    /// settings merged over its parent's) and named colors expanded.
    fn resolve(&self, raw: &Map<String, Value>, name: &str, depth: usize) -> Result<Value> {
        if depth > 50 {
            bail!("category '{name}': `inherit` loops back on itself");
        }
        let Some(Value::Object(own)) = raw.get(name) else {
            bail!("category '{name}' is used but not defined under [categories]");
        };
        let mut own = own.clone();
        let cat = match own.remove("inherit") {
            Some(Value::String(parent)) => {
                let mut base = self.resolve(raw, &parent, depth + 1)?;
                deep_merge(&mut base, Value::Object(own));
                base
            }
            _ => Value::Object(own),
        };
        let cat = self.expand_colors(cat)?;
        let t = cat["type"].as_str().unwrap_or("");
        if !SPECIAL_TYPES.contains(&t) {
            bail!("category '{name}' has unknown type {}", cat["type"]);
        }
        Ok(cat)
    }

    /// `v` with every "$name" string replaced by the named color.
    fn expand_colors(&self, v: Value) -> Result<Value> {
        Ok(match v {
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, x)| Ok((k, self.expand_colors(x)?)))
                    .collect::<Result<_>>()?,
            ),
            Value::Array(a) => Value::Array(
                a.into_iter()
                    .map(|x| self.expand_colors(x))
                    .collect::<Result<_>>()?,
            ),
            Value::String(s) if s.starts_with('$') => self
                .colors
                .get(&s[1..])
                .cloned()
                .ok_or_else(|| anyhow!("unknown named color {s}"))?,
            v => v,
        })
    }

    /// When pastel.toml or settings.toml was last modified (for --watch).
    pub fn mtime(&self) -> Option<SystemTime> {
        let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
        modified(&self.path).max(modified(&self.settings_path))
    }

    /// A category's resolved settings.
    pub fn category(&self, name: &str) -> Result<&Value> {
        self.categories
            .get(name)
            .ok_or_else(|| anyhow!("category '{name}' is used but not defined under [categories]"))
    }

    /// A category's `type`: "passthrough", "remap", "transfer", "shadow" or "lut".
    pub fn category_type(&self, name: &str) -> Result<&str> {
        Ok(self.category(name)?["type"].as_str().unwrap_or(""))
    }

    /// Fingerprint of everything that decides how a category recolors a file.
    pub fn category_hash(&self, name: &str, stats: Option<&Value>) -> Result<String> {
        let blob = json!({"cat": self.category(name)?, "global": self.globals, "stats": stats});
        let digest = sha1_smol::Sha1::from(blob.to_string()).digest().to_string();
        Ok(digest[..16].to_string())
    }

    /// Category for a path relative to data/, e.g. 'base/graphics/terrain/grass-1.png':
    /// the first matching rule's, else the file's group's. Passthrough when
    /// the file's group is switched off; None when nothing applies.
    pub fn classify(&self, rel: &str) -> Option<&str> {
        let group = self.group_of(rel);
        let category = match self.matching_rule(rel) {
            Some(r) => r.category.as_str(),
            None => group?.category.as_deref()?,
        };
        match group {
            Some(g) if !g.enabled => Some(PASSTHROUGH),
            _ => Some(category),
        }
    }

    /// The group a path belongs to: the first with a matching pattern.
    pub fn group_of(&self, rel: &str) -> Option<&Group> {
        self.groups
            .iter()
            .find(|g| g.patterns.iter().any(|r| r.is_match(rel)))
    }

    /// The `[liquids]` entries whose group is switched on (or that have none).
    pub fn liquids(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.liquids.iter().filter(|(_, l)| {
            l["group"]
                .as_str()
                .is_none_or(|g| self.groups.iter().any(|x| x.name == g && x.enabled))
        })
    }

    /// First column of the region of `rel` that stays unchanged, if any.
    pub fn keep_from(&self, rel: &str) -> Option<usize> {
        self.keep
            .iter()
            .find(|k| k.regex.is_match(rel))
            .map(|k| k.x_from)
    }

    /// The first rule that matches a path relative to data/.
    pub fn matching_rule(&self, rel: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.regex.is_match(rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = r##"
[[groups]]
name = "vulcanus"
enabled = false
category = "ground"
match = ["space-age/graphics/terrain/vulcanus/**"]

[[groups]]
name = "buildings"
category = "entity"
match = ["*/graphics/entity/**"]

[liquids]
lava = { group = "vulcanus", tiles = ["lava"], color = "#F49A82" }
water = { tiles = ["water"], lighten = 0.5 }

[categories.passthrough]
type = "passthrough"
[categories.ground]
type = "remap"
[categories.entity]
type = "remap"
[categories.shadow]
type = "shadow"

[classify]
rules = [{ match = "**/*shadow*.png", category = "shadow" }]
"##;

    fn load() -> Config {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CONFIG_FILE), TOML).unwrap();
        Config::load(dir.path()).unwrap()
    }

    #[test]
    fn groups_supply_categories_and_switch_off() {
        let cfg = load();
        // A rule wins over the group's category; the group's category fills in.
        assert_eq!(
            cfg.classify("base/graphics/entity/lab/lab-shadow.png"),
            Some("shadow")
        );
        assert_eq!(
            cfg.classify("base/graphics/entity/lab/lab.png"),
            Some("entity")
        );
        // A switched-off group stays vanilla, rule or not.
        assert_eq!(
            cfg.classify("space-age/graphics/terrain/vulcanus/ash.png"),
            Some(PASSTHROUGH)
        );
        assert_eq!(
            cfg.classify("space-age/graphics/terrain/vulcanus/rock-shadow.png"),
            Some(PASSTHROUGH)
        );
        // Neither a rule nor a group: no category.
        assert_eq!(cfg.classify("core/graphics/cursor.png"), None);
    }

    fn load_with_settings(settings: &str) -> Result<Config> {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CONFIG_FILE), TOML).unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), settings).unwrap();
        Config::load(dir.path())
    }

    #[test]
    fn settings_switch_groups_and_set_liquids() {
        let cfg = load_with_settings(
            "[groups]\nvulcanus = true\nbuildings = false\n[liquids]\nlava = { strength = 0.3 }\n",
        )
        .unwrap();
        assert_eq!(
            cfg.classify("space-age/graphics/terrain/vulcanus/ash.png"),
            Some("ground")
        );
        assert_eq!(
            cfg.classify("base/graphics/entity/lab/lab.png"),
            Some(PASSTHROUGH)
        );
        let lava = cfg.liquids().find(|(n, _)| *n == "lava").unwrap().1;
        assert_eq!(lava["strength"], 0.3);
        assert_eq!(lava["color"], "#F49A82");
    }

    #[test]
    fn global_knobs_stay_between_0_and_1() {
        assert!(load_with_settings("[global]\nbrightness = 0.5\nsaturation = 0\n").is_ok());
        assert!(load_with_settings("[global]\nbrightness = 1.4\n").is_err());
        assert!(load_with_settings("[global]\nsaturation = -0.1\n").is_err());
    }

    #[test]
    fn settings_reject_unknown_names() {
        assert!(load_with_settings("[groups]\nmars = false\n").is_err());
        assert!(load_with_settings("[liquids]\nmagma = { strength = 1.0 }\n").is_err());
    }

    #[test]
    fn liquids_follow_their_group() {
        let cfg = load();
        let names: Vec<&String> = cfg.liquids().map(|(n, _)| n).collect();
        assert_eq!(names, ["water"]);
    }
}
