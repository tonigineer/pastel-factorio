//! The project on disk: finding source files, category statistics, and
//! recoloring a single file. Used by build, preview and validate.
//!
//! Layout of a project folder:
//!
//! ```text
//! pastel.toml                     the settings
//! data/<folder>/graphics          the game's files, e.g. data/base/graphics
//! data/<folder>/updated_graphics  the recolored files (only those that change)
//! build/                          manifest, reports, previews, the mod
//! ```
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use walkdir::WalkDir;

use crate::config::Config;
use crate::config::glob::glob_to_regex;
use crate::config::value::{num, string};
use crate::image::pixels;
use crate::image::png::Png;
use crate::recolor::stats::{Stats, color_stats};
use crate::recolor::{lut, recolor_image};

/// File names that are never part of the game's data (OS clutter).
pub const SKIP_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];
const STATS_VERSION: u32 = 2;

// Files used to measure a 'transfer' category's median color, unless the
// category sets `stats_from`. Decals, particles and transition sheets are
// excluded so they don't skew the measurement; they are still recolored.
static DEFAULT_STATS_EXCLUDE: LazyLock<Regex> =
    LazyLock::new(|| glob_to_regex("{**/decorative/**,**/particle/**,**/*-transition*/**}"));

/// One category's entry in build/stats.json.
#[derive(Serialize, Deserialize)]
struct CachedStats {
    sig: String,
    stats: Stats,
}

/// A project folder, with its settings and the list of source files.
#[derive(Debug)]
pub struct Project {
    /// The project folder (absolute).
    pub root: PathBuf,
    /// Its data/ folder, with the source and output graphics.
    pub data: PathBuf,
    /// Its build/ folder, for the manifest, reports and the mod.
    pub build_dir: PathBuf,
    /// The settings from pastel.toml.
    pub cfg: Config,
    files: Vec<String>,
    stats: Mutex<HashMap<String, Stats>>,
}

/// Whether a path names a PNG (by extension, any case).
pub fn is_png(rel: &str) -> bool {
    rel.to_lowercase().ends_with(".png")
}

/// The last component of a '/'-separated path.
fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// The `<folder>` in `data/<folder>/graphics`, from that graphics folder's path.
fn folder_name(graphics_root: &Path) -> String {
    graphics_root
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

impl Project {
    /// Open the project folder `root`: load pastel.toml and list the sources.
    pub fn open(root: &Path) -> Result<Project> {
        let root =
            fs::canonicalize(root).with_context(|| format!("project folder {}", root.display()))?;
        let data = root.join("data");
        let cfg = Config::load(&root)?;
        let mut p = Project {
            build_dir: root.join("build"),
            root,
            data,
            cfg,
            files: vec![],
            stats: Mutex::default(),
        };
        p.files = p.scan_files()?;
        Ok(p)
    }

    /// Reload the config files and forget measured statistics.
    pub fn reload_config(&mut self) -> Result<()> {
        self.cfg = Config::load(&self.root)?;
        self.stats.lock().unwrap().clear();
        Ok(())
    }

    /// A path relative to the project folder, for messages.
    pub fn show(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    // -- files ------------------------------------------------------------

    /// Every `data/<folder>/graphics` directory.
    pub fn source_roots(&self) -> Result<Vec<PathBuf>> {
        if !self.data.is_dir() {
            bail!(
                "No data folder found at {}.\n\
                 Copy the game's data/base/graphics and data/core/graphics into it first.",
                self.data.display()
            );
        }
        let mut roots: Vec<PathBuf> = fs::read_dir(&self.data)?
            .filter_map(|e| e.ok())
            .map(|e| e.path().join("graphics"))
            .filter(|p| p.is_dir())
            .collect();
        roots.sort();
        if roots.is_empty() {
            bail!(
                "No graphics folders found under {} (expected data/base/graphics and data/core/graphics).",
                self.data.display()
            );
        }
        Ok(roots)
    }

    /// Each output root (`data/<folder>/updated_graphics`), with its `<folder>`.
    pub fn output_roots(&self) -> Result<Vec<(String, PathBuf)>> {
        Ok(self
            .source_roots()?
            .into_iter()
            .map(|r| {
                (
                    folder_name(&r),
                    r.parent().unwrap().join("updated_graphics"),
                )
            })
            .collect())
    }

    /// Files under `dir`, relative to it, with '/' separators.
    pub fn walk_files(dir: &Path) -> Vec<String> {
        WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter(|e| !SKIP_NAMES.contains(&e.file_name().to_string_lossy().as_ref()))
            .filter_map(|e| {
                let rel = e.path().strip_prefix(dir).ok()?;
                Some(
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/"),
                )
            })
            .collect()
    }

    fn scan_files(&self) -> Result<Vec<String>> {
        let mut files = Vec::new();
        for r in self.source_roots()? {
            let folder = folder_name(&r);
            files.extend(
                Self::walk_files(&r)
                    .into_iter()
                    .map(|f| format!("{folder}/graphics/{f}")),
            );
        }
        files.sort();
        Ok(files)
    }

    /// All source files as paths relative to data/, e.g. base/graphics/x.png.
    pub fn all_files(&self) -> &[String] {
        &self.files
    }

    /// The source files that are PNGs.
    pub fn pngs(&self) -> impl Iterator<Item = &String> {
        self.files.iter().filter(|f| is_png(f))
    }

    /// Full path of a source file.
    pub fn src_path(&self, rel: &str) -> PathBuf {
        self.data.join(rel)
    }

    /// Full path of a source file's output, in `data/<folder>/updated_graphics`.
    pub fn out_path(&self, rel: &str) -> PathBuf {
        let mut parts: Vec<&str> = rel.split('/').collect();
        assert_eq!(parts.get(1), Some(&"graphics"), "{rel}");
        parts[1] = "updated_graphics";
        parts.iter().fold(self.data.clone(), |p, part| p.join(part))
    }

    /// Output files whose source no longer exists, as (the source path they
    /// stand for, relative to data/; the output's full path).
    pub fn extra_outputs(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut extra = Vec::new();
        for (folder, out_root) in self.output_roots()? {
            if !out_root.exists() {
                continue;
            }
            for f in Self::walk_files(&out_root) {
                let rel = format!("{folder}/graphics/{f}");
                if self.files.binary_search(&rel).is_err() {
                    extra.push((rel, out_root.join(&f)));
                }
            }
        }
        Ok(extra)
    }

    // -- category statistics (for 'transfer' categories) -------------------

    /// The median color of a 'transfer' category, cached in build/stats.json.
    pub fn stats_for(&self, category: &str) -> Result<Option<Stats>> {
        if self.cfg.category_type(category)? != "transfer" {
            return Ok(None);
        }
        let mut memo = self.stats.lock().unwrap();
        if let Some(s) = memo.get(category) {
            return Ok(Some(s.clone()));
        }
        let members: Vec<&String> = self
            .pngs()
            .filter(|f| self.cfg.classify(f) == Some(category))
            .collect();
        let mut chosen: Vec<&String> = members
            .iter()
            .copied()
            .filter(|f| self.is_stats_source(category, f).unwrap_or(false))
            .collect();
        if chosen.is_empty() {
            chosen = members;
        }
        let mut sig_parts = vec![json!(STATS_VERSION)];
        for f in &chosen {
            let m = fs::metadata(self.src_path(f))?;
            let mtime = m
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            sig_parts.push(json!([f, m.len(), mtime]));
        }
        let sig = sha1_smol::Sha1::from(serde_json::to_string(&sig_parts)?)
            .digest()
            .to_string();

        let cache_file = self.build_dir.join("stats.json");
        let mut cache: BTreeMap<String, serde_json::Value> = fs::read_to_string(&cache_file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let hit = cache
            .get(category)
            .and_then(|v| serde_json::from_value::<CachedStats>(v.clone()).ok())
            .filter(|c| c.sig == sig);
        let stats = match hit {
            Some(c) => c.stats,
            None => {
                // Skip empty/damaged files; they're copied through unchanged.
                let images: Vec<Vec<[u8; 4]>> = chosen
                    .iter()
                    .filter_map(|f| Png::open(&self.src_path(f)).ok())
                    .map(|im| pixels::to_rgba_display(&im))
                    .collect();
                let mut stats = color_stats(&images);
                stats.files = Some(chosen.len());
                cache.insert(
                    category.to_string(),
                    serde_json::to_value(CachedStats {
                        sig,
                        stats: stats.clone(),
                    })?,
                );
                fs::create_dir_all(&self.build_dir)?;
                fs::write(&cache_file, serde_json::to_string_pretty(&cache)?)?;
                stats
            }
        };
        memo.insert(category.to_string(), stats.clone());
        Ok(Some(stats))
    }

    /// Is this file one of the main tile sheets used to measure the category?
    /// (Decals, particles and transition sheets are not.)
    pub fn is_stats_source(&self, category: &str, rel: &str) -> Result<bool> {
        let cat = self.cfg.category(category)?;
        Ok(match string(cat, "stats_from") {
            Some(pattern) => glob_to_regex(pattern).is_match(rel),
            None => !DEFAULT_STATS_EXCLUDE.is_match(rel),
        })
    }

    /// Stats used to recolor one file.
    ///
    /// Normally the category-wide stats. With `per_file_match: m` (0..1), each
    /// main tile sheet is also measured on its own and its measured color moves
    /// m of the way from the category average toward its own average. The
    /// result: every sheet's own average lands close to the target color (at
    /// m = 1 exactly on it), so sheets that start out cooler or yellower than
    /// the others no longer end up standing out.
    pub fn stats_for_file(&self, category: &str, rel: &str, im: &Png) -> Result<Option<Stats>> {
        let Some(base) = self.stats_for(category)? else {
            return Ok(None);
        };
        let m = num(self.cfg.category(category)?, "per_file_match", 0.0);
        if m <= 0.0 || !self.is_stats_source(category, rel)? {
            return Ok(Some(base));
        }
        let own = color_stats(&[pixels::to_rgba_display(im)]);
        if own.n == 0 {
            return Ok(Some(base));
        }
        let dh = (own.h - base.h + 180.0).rem_euclid(360.0) - 180.0;
        Ok(Some(Stats {
            l: base.l + m * (own.l - base.l),
            c: base.c + m * (own.c - base.c),
            h: (base.h + m * dh).rem_euclid(360.0),
            n: own.n,
            files: None,
        }))
    }

    // -- recoloring one file ---------------------------------------------

    /// Recolored image for a source PNG, or None if it stays unchanged.
    pub fn recolor(&self, rel: &str, category: &str, im: &Png) -> Result<Option<Png>> {
        let cat = self.cfg.category(category)?;
        if self.cfg.category_type(category)? == "lut" {
            return lut::transform_file(im, cat, file_name(rel));
        }
        let stats = self.stats_for_file(category, rel, im)?;
        let mut new = recolor_image(im, cat, &self.cfg.globals, stats.as_ref())?;
        if let (Some(new), Some(x)) = (new.as_mut(), self.cfg.keep_from(rel)) {
            pixels::restore_columns(new, im, x)?;
        }
        Ok(new)
    }

    /// The category of a source PNG; an error if no rule matches it.
    pub fn classify(&self, rel: &str) -> Result<&str> {
        self.cfg
            .classify(rel)
            .ok_or_else(|| anyhow!("no rule in pastel.toml matches {rel}"))
    }
}
