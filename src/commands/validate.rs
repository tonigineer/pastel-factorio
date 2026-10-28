//! `pastel validate`, and the check of one source/output pair that the
//! build also runs on every file it writes.
//!
//! A recolored file must decode exactly like the original apart from its
//! colors: same mode, size and metadata, same alpha (for shadows: the same
//! shape), same palette indices, and unchanged transparent pixels.
use std::path::Path;

use anyhow::{Result, bail};
use rayon::prelude::*;

use super::{cat_name, classify_all, read_png, thread_pool};
use crate::image::pixels;
use crate::image::png::{Mode, Png};
use crate::project::{Project, is_png};

/// Problems with an output file (empty = fine). Only recolored files are
/// written; for everything else the game keeps using the vanilla file, so a
/// missing output is fine.
pub fn validate_pair(src: &Path, out: &Path, ctype: &str) -> Result<Vec<String>> {
    if !out.exists() {
        return Ok(vec![]);
    }
    let a = if is_png(&src.to_string_lossy()) && ctype != "passthrough" {
        read_png(src)
    } else {
        None
    };
    let Some(a) = a else {
        return Ok(vec!["output for a file that is never recolored".into()]);
    };
    let b = Png::open(out)?;
    let problems = a.signature().diff(&b.signature());
    if !problems.is_empty() {
        return Ok(problems);
    }
    let mut problems = Vec::new();
    match a.mode {
        Mode::Rgba | Mode::La => {
            let ch = if a.mode == Mode::Rgba { 4 } else { 2 };
            let alpha = |im: &Png| {
                im.data
                    .chunks_exact(ch)
                    .map(|p| p[ch - 1])
                    .collect::<Vec<u8>>()
            };
            let (aa, ab) = (alpha(&a), alpha(&b));
            if ctype == "shadow" {
                if aa.iter().zip(&ab).any(|(x, y)| (*x > 0) != (*y > 0)) {
                    problems.push("shadow shape changed".into());
                }
            } else if aa != ab {
                problems.push("alpha changed".into());
            }
            if a.mode == Mode::Rgba {
                let moved = a
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(b.data.as_chunks::<4>().0)
                    .any(|(p, q)| p[3] == 0 && p[..3] != q[..3]);
                if moved {
                    problems.push("RGB of transparent pixels changed".into());
                }
            }
        }
        Mode::P => {
            if a.data != b.data {
                problems.push("palette indices changed".into());
            }
            let (_, pa) = pixels::palette_arrays(&a);
            let (_, pb) = pixels::palette_arrays(&b);
            if ctype == "shadow" {
                let shape = |v: &[f64]| v.iter().map(|&x| x > 0.0).collect::<Vec<_>>();
                if shape(&pa) != shape(&pb) {
                    problems.push("shadow shape changed".into());
                }
            } else if pa != pb {
                problems.push("palette alpha changed".into());
            }
        }
        _ => {}
    }
    Ok(problems)
}

/// Check every output against its source, and look for outputs whose source
/// is gone. Returns the number of recolored files; fails if any problem
/// turned up (after listing them).
pub fn run(p: &Project, jobs: Option<usize>) -> Result<usize> {
    let cats = classify_all(p, true)?;
    let list: Vec<(&String, &str)> = cats
        .iter()
        .map(|(f, c)| Ok((f, p.cfg.category_type(cat_name(c))?)))
        .collect::<Result<_>>()?;
    let bad: Vec<(String, Vec<String>)> = thread_pool(jobs)?.install(|| {
        list.par_iter()
            .map(|&(f, ctype)| {
                let problems = validate_pair(&p.src_path(f), &p.out_path(f), ctype)
                    .unwrap_or_else(|e| vec![format!("error: {e:#}")]);
                (f.clone(), problems)
            })
            .filter(|(_, pr)| !pr.is_empty())
            .collect()
    });
    let extra = p.extra_outputs()?;
    for (rel, pr) in bad.iter().take(50) {
        println!("   {rel}: {}", pr.join("; "));
    }
    for (rel, _) in extra.iter().take(20) {
        println!("   extra file in output: {rel}");
    }
    if !bad.is_empty() || !extra.is_empty() {
        println!(
            "{} problem file(s), {} extra file(s).",
            bad.len(),
            extra.len()
        );
        bail!("Validation failed.");
    }
    let recolored = list.iter().filter(|(f, _)| p.out_path(f).exists()).count();
    println!(
        "All {recolored} recolored files OK: same sizes, modes, metadata and alpha as the source."
    );
    Ok(recolored)
}
