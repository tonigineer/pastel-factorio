//! `pastel build`: recolor every source file whose source or settings
//! changed since the last build, validate each file written, and record the
//! result in build/manifest.json and build/build_report.csv.
//!
//! Only files that change get an output in data/*/updated_graphics; for
//! everything else the game keeps using its own file.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use super::validate::validate_pair;
use super::{classify_all, read_png, thread_pool};
use crate::config::glob::only_filter;
use crate::manifest::{CODE_VERSION, Manifest, Record, sha1_file};
use crate::project::{Project, is_png};

/// Options for [`run`].
#[derive(Clone, Debug, Default)]
pub struct BuildArgs {
    /// Only rebuild these categories (comma-separated, wildcards ok); files
    /// in other categories stay exactly as they are. None rebuilds everything.
    pub only: Option<String>,
    /// Delete existing output and rebuild from scratch.
    pub clean: bool,
    /// Skip the already-recolored-input check.
    pub force: bool,
    /// Parallel worker threads (default: CPU count - 1).
    pub jobs: Option<usize>,
}

/// What a build did, counted in source files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildSummary {
    /// Processed and written to the output.
    pub recolored: usize,
    /// Processed, and stay vanilla (no output).
    pub left_vanilla: usize,
    /// Unchanged since the last build, so not processed.
    pub up_to_date: usize,
    /// Outside `--only`, so left as they were.
    pub not_selected: usize,
    /// Outputs removed because their source no longer exists.
    pub stale_removed: usize,
}

impl BuildSummary {
    /// Files processed this time (recolored or left vanilla).
    pub fn processed(&self) -> usize {
        self.recolored + self.left_vanilla
    }
}

// ---------------------------------------------------------------------------
// One file
// ---------------------------------------------------------------------------

struct Outcome {
    rel: String,
    changed: bool,
    size: Option<u64>,
    hash: Option<String>,
    problems: Vec<String>,
}

/// Recolor one file into the output. Files that stay unchanged get no
/// output; the game keeps using the vanilla file for those.
fn process(p: &Project, rel: &str, category: Option<&str>, ctype: &str) -> Outcome {
    let (src, out) = (p.src_path(rel), p.out_path(rel));
    let mut changed = false;
    let run = |changed: &mut bool| -> Result<Vec<String>> {
        if is_png(rel)
            && ctype != "passthrough"
            && let (Some(im), Some(category)) = (read_png(&src), category)
            && let Some(new) = p.recolor(rel, category, &im)?
        {
            fs::create_dir_all(out.parent().unwrap())?;
            let tmp = out.with_file_name(format!(
                "{}.tmp",
                out.file_name().unwrap().to_string_lossy()
            ));
            new.save(&tmp)?;
            fs::rename(&tmp, &out)?;
            *changed = true;
            return validate_pair(&src, &out, ctype);
        }
        if out.exists() {
            fs::remove_file(&out).with_context(|| format!("removing old output for {rel}"))?;
        }
        Ok(vec![])
    };
    // Report errors and keep going.
    let problems = run(&mut changed).unwrap_or_else(|e| vec![format!("error: {e:#}")]);
    let (size, hash) = if changed {
        (
            fs::metadata(&out).ok().map(|m| m.len()),
            sha1_file(&out).ok(),
        )
    } else {
        (None, None)
    };
    Outcome {
        rel: rel.to_string(),
        changed,
        size,
        hash,
        problems,
    }
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Remove every empty folder under `root` (not `root` itself).
fn remove_empty_dirs(root: &Path) {
    let dirs = walkdir::WalkDir::new(root)
        .min_depth(1)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir());
    for d in dirs {
        let _ = fs::remove_dir(d.path()); // fails, as intended, unless empty
    }
}

/// Rebuild what changed since the last build. Fails if a PNG has no
/// category, if sources look already recolored (unless `force`), or if a
/// written file fails validation (the manifest and report are still saved).
pub fn run(p: &Project, args: &BuildArgs) -> Result<BuildSummary> {
    let t0 = Instant::now();
    let cats = classify_all(p, true)?;
    let only = only_filter(args.only.as_deref());
    let mut prev = Manifest::load(&p.build_dir)
        .map(|m| m.files)
        .unwrap_or_default();

    if args.clean {
        for (_, out_root) in p.output_roots()? {
            if out_root.exists() {
                println!("Removing {}", p.show(&out_root));
                fs::remove_dir_all(&out_root)?;
            }
        }
        prev.clear();
    }

    // With --only, files in other categories are left exactly as they are:
    // not processed, their output untouched, their record from the last
    // build kept.
    let mut selected: BTreeMap<&str, &str> = BTreeMap::new();
    let mut records = BTreeMap::new();
    let mut not_selected = 0;
    for (f, c) in &cats {
        let c = c.as_deref().unwrap_or_default();
        if only(c) {
            selected.insert(f.as_str(), c);
        } else {
            not_selected += 1;
            if let Some(old) = prev.get(f) {
                records.insert(f.clone(), old.clone());
            }
        }
    }
    let not_selected_note = if not_selected > 0 {
        format!(", {not_selected} skipped (not selected)")
    } else {
        String::new()
    };

    println!("Hashing {} source files...", cats.len());
    let src_hash: HashMap<&str, String> = cats
        .keys()
        .collect::<Vec<_>>()
        .par_iter()
        .map(|f| Ok((f.as_str(), sha1_file(&p.src_path(f))?)))
        .collect::<Result<_>>()?;

    // Guard: a source file that is byte-identical to something we produced earlier
    // means recolored files were copied back in as "vanilla".
    let produced: HashSet<&str> = prev
        .values()
        .filter(|m| m.out.is_some() && m.out != m.src)
        .filter_map(|m| m.out.as_deref())
        .collect();
    let recolored_inputs: Vec<&str> = cats
        .keys()
        .map(String::as_str)
        .filter(|f| produced.contains(src_hash[f].as_str()))
        .collect();
    if !recolored_inputs.is_empty() && !args.force {
        println!(
            "\nSTOP: {} source file(s) in data/*/graphics look already recolored \
             (they match files this pipeline produced). Examples:",
            recolored_inputs.len()
        );
        for f in recolored_inputs.iter().take(10) {
            println!("    {f}");
        }
        println!(
            "\nCopy fresh vanilla graphics folders into data/ (Steam: Verify integrity of game files first).\n\
             If you are sure these are vanilla, rerun with --force."
        );
        bail!("Stopped: the source graphics look already recolored.");
    }

    // Stats for 'transfer' categories are computed once, up front.
    let mut stats = HashMap::new();
    let used: BTreeSet<&str> = selected
        .values()
        .copied()
        .filter(|c| *c != "passthrough")
        .collect();
    for c in used {
        if let Some(s) = p.stats_for(c)? {
            println!(
                "  {c}: measured median L={:.2} C={:.3} h={:.0} over {} file(s)",
                s.l,
                s.c,
                s.h,
                s.files.unwrap_or(0)
            );
            stats.insert(c, serde_json::to_value(&s)?);
        }
    }

    let mut jobs = Vec::new();
    let mut up_to_date = 0;
    for (&f, &c) in &selected {
        let ctype = p.cfg.category_type(c)?;
        let cfg_hash = format!(
            "{CODE_VERSION}:{}",
            if ctype == "passthrough" {
                "copy".into()
            } else {
                p.cfg.category_hash(c, stats.get(c))?
            }
        );
        let cfg_hash = match p.cfg.keep_from(f) {
            Some(x) if ctype != "passthrough" => format!("{cfg_hash}:keep{x}"),
            _ => cfg_hash,
        };
        let out = p.out_path(f);
        if let Some(old) = prev.get(f) {
            let fresh = old.src.as_deref() == Some(src_hash[f].as_str())
                && old.cfg.as_deref() == Some(cfg_hash.as_str())
                && old.problems.is_empty()
                && match old.size {
                    Some(size) => fs::metadata(&out).is_ok_and(|m| m.len() == size),
                    None => !out.exists(),
                };
            if fresh {
                records.insert(f.to_string(), old.clone());
                up_to_date += 1;
                continue;
            }
        }
        let rec = Record {
            src: Some(src_hash[f].clone()),
            cfg: Some(cfg_hash),
            category: cats[f].clone(),
            ..Default::default()
        };
        records.insert(f.to_string(), rec);
        jobs.push((f, c, ctype));
    }

    println!(
        "{} file(s) to process, {up_to_date} unchanged since last build{not_selected_note}.",
        jobs.len()
    );
    let mut problems_total = Vec::new();
    let mut changed_n = 0;
    if !jobs.is_empty() {
        let done = AtomicUsize::new(0);
        let total = jobs.len();
        let outcomes: Vec<Outcome> = thread_pool(args.jobs)?.install(|| {
            jobs.par_iter()
                .map(|&(f, c, ctype)| {
                    let o = process(p, f, Some(c), ctype);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(100) || n == total {
                        print!("\r  {n}/{total}");
                        std::io::stdout().flush().ok();
                    }
                    o
                })
                .collect()
        });
        println!();
        for o in outcomes {
            let r = records.get_mut(&o.rel).unwrap();
            r.out = o.hash;
            r.size = o.size;
            r.problems = o.problems.clone();
            changed_n += o.changed as usize;
            if !o.problems.is_empty() {
                problems_total.push((o.rel, o.problems));
            }
        }
    }

    // Remove outputs whose source no longer exists, and folders left empty.
    let stale = p.extra_outputs()?;
    for (_, path) in &stale {
        fs::remove_file(path)?;
    }
    for (_, out_root) in p.output_roots()? {
        remove_empty_dirs(&out_root);
    }

    fs::create_dir_all(&p.build_dir)?;
    let manifest = Manifest::new(records);
    manifest.save(&p.build_dir)?;
    let mut w = csv::Writer::from_path(p.build_dir.join("build_report.csv"))?;
    w.write_record(["path", "category", "problems"])?;
    for (f, r) in &manifest.files {
        w.write_record([
            f.as_str(),
            r.category.as_deref().unwrap_or(""),
            &r.problems.join("; "),
        ])?;
    }
    w.flush()?;

    let summary = BuildSummary {
        recolored: changed_n,
        left_vanilla: jobs.len() - changed_n,
        up_to_date,
        not_selected,
        stale_removed: stale.len(),
    };
    println!(
        "\nDone in {:.0}s: {} recolored, {} left vanilla, {} up to date{not_selected_note}, {} stale removed.",
        t0.elapsed().as_secs_f64(),
        summary.recolored,
        summary.left_vanilla,
        summary.up_to_date,
        summary.stale_removed
    );
    let outs: Vec<String> = p.output_roots()?.iter().map(|(_, r)| p.show(r)).collect();
    println!("Output: {}", outs.join(", "));
    if !problems_total.is_empty() {
        println!("\n{} file(s) FAILED validation:", problems_total.len());
        for (rel, pr) in problems_total.iter().take(30) {
            println!("   {rel}: {}", pr.join("; "));
        }
        println!("Full list in build/build_report.csv. Don't install until these are fixed.");
        bail!(
            "Build failed: {} file(s) failed validation.",
            problems_total.len()
        );
    }
    // Records kept for files outside --only may carry problems from before.
    let earlier = manifest
        .files
        .iter()
        .filter(|(f, r)| !r.problems.is_empty() && !selected.contains_key(f.as_str()))
        .count();
    if earlier > 0 {
        println!(
            "The rebuilt files passed validation, but {earlier} file(s) not selected still have \
             problems from an earlier build (see build/build_report.csv)."
        );
    } else {
        println!("All files passed validation.");
    }
    Ok(summary)
}
