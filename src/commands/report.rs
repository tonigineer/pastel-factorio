//! `pastel classify` and `pastel inventory`: reports on the source files.
use std::collections::HashMap;
use std::fs;

use anyhow::Result;

use super::{cat_name, classify_all};
use crate::image::png::Png;
use crate::manifest::sha1_file;
use crate::project::{Project, is_png};

/// Write build/classification_report.csv (every file, its category, its
/// group and the rule that put it there; "(group)" when the group's category
/// applied) and print how many files each category has.
pub fn classify(p: &Project) -> Result<()> {
    let cats = classify_all(p, false)?;
    fs::create_dir_all(&p.build_dir)?;
    let path = p.build_dir.join("classification_report.csv");
    let mut w = csv::Writer::from_path(&path)?;
    w.write_record(["path", "category", "group", "rule"])?;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (f, c) in &cats {
        let group = p.cfg.group_of(f).map_or("", |g| g.name.as_str());
        let rule = if is_png(f) {
            p.cfg
                .matching_rule(f)
                .map_or("(group)", |r| r.pattern.as_str())
        } else {
            "(non-PNG)"
        };
        w.write_record([f.as_str(), c.as_deref().unwrap_or(""), group, rule])?;
        *counts.entry(cat_name(c)).or_default() += 1;
    }
    w.flush()?;
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    for (c, n) in counts {
        println!("{n:6}  {c}");
    }
    println!("Written to {}", p.show(&path));
    Ok(())
}

/// Write build/inventory.csv: every source file with its category, size,
/// image format and hash.
pub fn inventory(p: &Project) -> Result<()> {
    let cats = classify_all(p, false)?;
    fs::create_dir_all(&p.build_dir)?;
    let path = p.build_dir.join("inventory.csv");
    let mut w = csv::Writer::from_path(&path)?;
    w.write_record([
        "path", "category", "bytes", "width", "height", "mode", "gamma", "sha1",
    ])?;
    for (f, c) in &cats {
        let src = p.src_path(f);
        let mut row = vec![
            f.clone(),
            c.clone().unwrap_or_default(),
            fs::metadata(&src)?.len().to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            sha1_file(&src)?,
        ];
        if is_png(f) {
            match Png::open(&src) {
                Ok(im) => {
                    row[3] = im.width.to_string();
                    row[4] = im.height.to_string();
                    row[5] = im.mode.to_string();
                    row[6] = im.gamma().map(|g| g.to_string()).unwrap_or_default();
                }
                Err(e) => row[5] = format!("unreadable: {:#}", e.root_cause()),
            }
        }
        w.write_record(&row)?;
    }
    w.flush()?;
    println!("{} files. Written to {}", cats.len(), p.show(&path));
    Ok(())
}
