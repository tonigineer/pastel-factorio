//! The commands behind the command line.
//!
//! Each command prints its progress and results as it goes, and returns an
//! error when it fails; `pastel` then prints the error and exits with 1.
pub mod build;
pub mod package;
pub mod preview;
pub mod report;
pub mod scan;
pub mod sheets;
pub mod validate;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Result, bail};

use crate::image::png::Png;
use crate::project::{Project, is_png};

/// Category of every source file: PNGs by rule, everything else passthrough.
/// Lists PNGs no rule matches, and fails if `fail`.
fn classify_all(p: &Project, fail: bool) -> Result<BTreeMap<String, Option<String>>> {
    let mut result = BTreeMap::new();
    let mut unmatched = Vec::new();
    for f in p.all_files() {
        let c = if is_png(f) {
            p.cfg.classify(f).map(str::to_string)
        } else {
            Some("passthrough".into())
        };
        if c.is_none() {
            unmatched.push(f.as_str());
        }
        result.insert(f.clone(), c);
    }
    if !unmatched.is_empty() {
        fs::create_dir_all(&p.build_dir)?;
        fs::write(
            p.build_dir.join("unmatched.txt"),
            unmatched.join("\n") + "\n",
        )?;
        println!(
            "{} PNG(s) match no rule in pastel.toml (full list in build/unmatched.txt):",
            unmatched.len()
        );
        for f in unmatched.iter().take(25) {
            println!("    {f}");
        }
        if fail {
            bail!("Every PNG needs a category: add [classify] rules for these to pastel.toml.");
        }
    }
    Ok(result)
}

/// Category name as the reports print it; "None" for unmatched files.
fn cat_name(c: &Option<String>) -> &str {
    c.as_deref().unwrap_or("None")
}

/// The source decoded, or None if it isn't a readable image. Some game files
/// named .png are empty (0 bytes) placeholders; those are copied unchanged.
fn read_png(path: &Path) -> Option<Png> {
    if fs::metadata(path).ok()?.len() == 0 {
        return None;
    }
    Png::open(path).ok()
}

/// Worker threads for the per-file work (default: CPU count - 1).
fn thread_pool(jobs: Option<usize>) -> Result<rayon::ThreadPool> {
    let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
    let n = jobs.unwrap_or((cpus - 1).max(1));
    Ok(rayon::ThreadPoolBuilder::new().num_threads(n).build()?)
}
