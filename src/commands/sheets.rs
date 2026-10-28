//! `pastel sheets`: before/after thumbnails for a whole category at once.
use std::collections::BTreeMap;
use std::fs;

use ::image::{Rgb, RgbImage, imageops};
use anyhow::{Result, bail};

use crate::config::glob::fnmatch;
use crate::image::draw::{self, PANEL_BG};
use crate::image::png::Png;
use crate::project::Project;

const THUMB: u32 = 150;

/// A THUMB x THUMB thumbnail over a checkerboard, never enlarged.
fn thumb(im: &Png) -> RgbImage {
    let im = draw::to_rgba_image(im);
    let scale = (THUMB as f64 / im.width() as f64)
        .min(THUMB as f64 / im.height() as f64)
        .min(1.0);
    let w = ((im.width() as f64 * scale).round() as u32).clamp(1, THUMB);
    let h = ((im.height() as f64 * scale).round() as u32).clamp(1, THUMB);
    let small = if (w, h) == im.dimensions() {
        im
    } else {
        draw::resize(&im, w, h)
    };
    let mut base = draw::checker(THUMB, THUMB, 6);
    draw::composite(
        &mut base,
        &small,
        ((THUMB - w) / 2) as i64,
        ((THUMB - h) / 2) as i64,
    );
    base
}

/// One sheet per category matching `category` (a wildcard; default all)
/// in build/sheets/, with up to `limit` files each.
///
/// Needs a finished build: compares data/*/graphics with updated_graphics.
/// Files the build left unchanged have no output and are counted, not shown.
pub fn run(p: &Project, category: Option<&str>, limit: usize) -> Result<()> {
    let wanted = category.unwrap_or("*");
    let mut groups: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for f in p.pngs() {
        if let Some(c) = p.cfg.classify(f)
            && c != "passthrough"
            && fnmatch(c, wanted)
        {
            groups.entry(c).or_default().push(f);
        }
    }
    if groups.is_empty() {
        bail!("No recolored categories match '{wanted}'.");
    }
    let out_dir = p.build_dir.join("sheets");
    fs::create_dir_all(&out_dir)?;
    for (c, files) in groups {
        let files = &files[..files.len().min(limit)];
        let cols = 4; // 4 before/after pairs per row
        let rows = files.len().div_ceil(cols) as u32;
        let (cell_w, cell_h) = (2 * THUMB + 6, THUMB + 20);
        let mut sheet = RgbImage::from_pixel(
            cols as u32 * (cell_w + 12),
            rows * (cell_h + 8) + 30,
            PANEL_BG,
        );
        let title = format!("{c}: {} file(s), each shown before | after", files.len());
        draw::text(&mut sheet, 8, 8, &title, Rgb([255, 225, 150]), 15);
        let mut unchanged = 0;
        for (i, f) in files.iter().enumerate() {
            let x = (i % cols) as u32 * (cell_w + 12) + 6;
            let y = (i / cols) as u32 * (cell_h + 8) + 30;
            let (Ok(before), Ok(after)) = (Png::open(&p.src_path(f)), Png::open(&p.out_path(f)))
            else {
                unchanged += 1;
                continue;
            };
            imageops::replace(&mut sheet, &thumb(&before), x as i64, y as i64);
            imageops::replace(&mut sheet, &thumb(&after), (x + THUMB + 6) as i64, y as i64);
            let name: String = f.rsplit('/').next().unwrap().chars().take(40).collect();
            draw::text(
                &mut sheet,
                x,
                y + THUMB + 3,
                &name,
                Rgb([210, 210, 220]),
                11,
            );
        }
        let path = out_dir.join(format!("{c}.png"));
        sheet.save(&path)?;
        let note = if unchanged > 0 {
            format!(" ({unchanged} unchanged)")
        } else {
            String::new()
        };
        println!("{}{note}", p.show(&path));
    }
    Ok(())
}
