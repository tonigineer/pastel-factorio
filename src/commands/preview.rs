//! --preview: recolor one file in memory and show before and after together.
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ::image::{Rgb, RgbImage, RgbaImage, imageops};
use anyhow::{Context, Result, anyhow, bail};

use crate::color::{self, hex_to_rgb255};
use crate::config::value::truthy;
use crate::image::draw::{self, PANEL_BG};
use crate::image::pixels;
use crate::image::png::Png;
use crate::project::Project;
use crate::recolor::lut;

const TARGET_WIDTH: f64 = 1600.0;
const CAPTION_H: u32 = 30;
const LABEL_H: u32 = 24;
const GAP: u32 = 12;

/// What to show behind transparent pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// Light/gray checkerboard.
    Checker,
    /// The new grass color (`grass` in `[colors]`), where shadows are seen.
    Grass,
    /// Off-white.
    Light,
    /// Dark gray.
    Dark,
}

/// Options for [`run`].
#[derive(Clone, Debug)]
pub struct PreviewArgs {
    /// Filename or part of a path to look for.
    pub name: String,
    /// Take match N (from 1) instead of asking.
    pub pick: Option<usize>,
    /// Scale factor (default: fit about 1600px wide).
    pub zoom: Option<f64>,
    /// "x,y,w,h" region of the source image.
    pub crop: Option<String>,
    /// Background behind transparency (default checker; grass for shadows).
    pub bg: Option<Background>,
    /// Repeat the (cropped) image N x N.
    pub tile: Option<u32>,
    /// Re-render whenever pastel.toml changes.
    pub watch: bool,
    /// Save only, don't open a viewer.
    pub no_open: bool,
}

/// Exact filename matches first; otherwise substring matches on the path.
fn find_matches<'a>(p: &'a Project, query: &str) -> Vec<&'a String> {
    let q = query.to_lowercase().replace('\\', "/");
    let exact: Vec<&String> = p
        .pngs()
        .filter(|f| {
            let f = f.to_lowercase();
            f.rsplit('/').next() == Some(q.as_str()) || f == q || f.ends_with(&format!("/{q}"))
        })
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    p.pngs().filter(|f| f.to_lowercase().contains(&q)).collect()
}

/// The file to preview; asks when several match. None if the user cancels.
fn choose(p: &Project, query: &str, pick: Option<usize>) -> Result<Option<String>> {
    let matches = find_matches(p, query);
    match (matches.len(), pick) {
        (0, _) => bail!("No PNG under data/*/graphics matches '{query}'."),
        (1, _) => return Ok(Some(matches[0].clone())),
        (n, Some(k)) if !(1..=n).contains(&k) => bail!("--pick {k} is out of range (1-{n})."),
        (_, Some(k)) => return Ok(Some(matches[k - 1].clone())),
        _ => {}
    }
    println!("{} files match '{query}':", matches.len());
    for (i, f) in matches.iter().take(200).enumerate() {
        println!(
            "  {:3}. {f}   [{}]",
            i + 1,
            p.cfg.classify(f).unwrap_or("None")
        );
    }
    if matches.len() > 200 {
        println!(
            "  ... and {} more; type more of the name to narrow it down.",
            matches.len() - 200
        );
    }
    if !std::io::stdin().is_terminal() {
        bail!("Several matches; rerun with --pick N.");
    }
    let shown = matches.len().min(200);
    loop {
        print!("Which one? (number, or Enter to cancel) ");
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().lock().read_line(&mut ans)?;
        let ans = ans.trim();
        if ans.is_empty() {
            return Ok(None);
        }
        if let Ok(k) = ans.parse::<usize>()
            && (1..=shown).contains(&k)
        {
            return Ok(Some(matches[k - 1].clone()));
        }
    }
}

// ---------------------------------------------------------------------------

fn background(kind: Background, w: u32, h: u32, p: &Project) -> Result<RgbImage> {
    let hex = match kind {
        Background::Checker => return Ok(draw::checker(w, h, 8)),
        Background::Light => "#F4F1EA",
        Background::Dark => "#2A2833",
        Background::Grass => p
            .cfg
            .colors
            .get("grass")
            .and_then(|v| v.as_str())
            .unwrap_or("#A8E39A"),
    };
    Ok(RgbImage::from_pixel(w, h, Rgb(hex_to_rgb255(hex)?)))
}

/// A test image for LUT previews: hue/lightness sweep, gray ramp, and the
/// new world's palette colors as swatches, so you see the pastel world at night.
fn lut_card(p: &Project) -> Result<(Vec<color::Rgb>, u32, u32)> {
    const W: usize = 512;
    let mut px = color::hsv_card(W, 160);
    for _ in 0..24 {
        px.extend((0..W).map(|x| [x as f64 * (1.0 / (W - 1) as f64); 3]));
    }
    let names: Vec<&str> = [
        "grass",
        "dirt",
        "sand",
        "water",
        "deepwater",
        "iron",
        "copper",
        "coal",
        "stone",
        "uranium",
        "concrete",
    ]
    .into_iter()
    .filter(|n| p.cfg.colors.contains_key(*n))
    .collect();
    let mut row = vec![[0.0; 3]; W];
    if !names.is_empty() {
        let cell = W / names.len();
        for (i, n) in names.iter().enumerate() {
            let c = color::hex_to_unit(p.cfg.colors[*n].as_str().unwrap_or(""))?;
            row[i * cell..(i + 1) * cell].fill(c);
        }
    }
    for _ in 0..56 {
        px.extend_from_slice(&row);
    }
    let h = (px.len() / W) as u32;
    Ok((px, W as u32, h))
}

fn float_image(px: &[color::Rgb], w: u32, h: u32) -> RgbaImage {
    let bytes = px
        .iter()
        .flat_map(|c| {
            [
                pixels::to_u8(c[0]),
                pixels::to_u8(c[1]),
                pixels::to_u8(c[2]),
                255,
            ]
        })
        .collect();
    RgbaImage::from_raw(w, h, bytes).unwrap()
}

fn parse_crop(s: &str) -> Result<(i64, i64, u32, u32)> {
    let err = || anyhow!("--crop needs x,y,w,h, e.g. --crop 0,320,256,256");
    let v: Vec<&str> = s.split(',').map(str::trim).collect();
    let [x, y, w, h] = v.as_slice() else {
        return Err(err());
    };
    Ok((
        x.parse().map_err(|_| err())?,
        y.parse().map_err(|_| err())?,
        w.parse().map_err(|_| err())?,
        h.parse().map_err(|_| err())?,
    ))
}

/// Build the side-by-side comparison image for one file.
fn render(p: &Project, rel: &str, args: &PreviewArgs) -> Result<RgbImage> {
    let category = p.classify(rel)?;
    let cat = p.cfg.category(category)?;
    let ctype = p.cfg.category_type(category)?;
    let src = Png::open(&p.src_path(rel)).map_err(|_| {
        anyhow!("{rel} isn't a readable image (it may be an empty placeholder file); the build copies it unchanged.")
    })?;
    let mut note = String::new();

    let (mut before, mut after, bg);
    if ctype == "lut" {
        let name = rel.rsplit('/').next().unwrap();
        let (card, w, h) = lut_card(p)?;
        let old_t = lut::decode(&src)?;
        let soften = lut::Soften::for_file(cat, name)?;
        let new_t = soften.as_ref().map(|s| s.apply(&old_t));
        let grade = |t: &lut::Table| card.iter().map(|&c| lut::apply(t, c)).collect::<Vec<_>>();
        before = float_image(&grade(&old_t), w, h);
        after = float_image(&grade(new_t.as_ref().unwrap_or(&old_t)), w, h);
        note = "test card graded by the LUT".into();
        if !truthy(&cat["files"][name]) {
            note += " (not listed in pastel.toml: unchanged)";
        }
        bg = Background::Dark;
    } else {
        let new = p.recolor(rel, category, &src)?;
        if new.is_none() {
            note = if ctype == "passthrough" {
                "unchanged (passthrough)".into()
            } else {
                format!("unchanged (mode {})", src.mode)
            };
        }
        before = draw::to_rgba_image(&src);
        after = draw::to_rgba_image(new.as_ref().unwrap_or(&src));
        bg = args.bg.unwrap_or(if ctype == "shadow" {
            Background::Grass
        } else {
            Background::Checker
        });
        if ctype == "shadow" {
            note = "shown over the new grass color".into();
        }
    }

    if let Some(c) = &args.crop {
        let (x, y, w, h) = parse_crop(c)?;
        before = draw::crop(&before, x, y, w, h);
        after = draw::crop(&after, x, y, w, h);
    }
    if let Some(n) = args.tile.filter(|&n| n > 1) {
        before = draw::tile(&before, n);
        after = draw::tile(&after, n);
    }

    let caption = format!(
        "{rel}   [{category}]   {}x{} {}{}",
        src.width,
        src.height,
        src.mode,
        if note.is_empty() {
            String::new()
        } else {
            format!("   ({note})")
        }
    );
    compose(p, &before, &after, bg, args.zoom, &caption)
}

fn compose(
    p: &Project,
    before: &RgbaImage,
    after: &RgbaImage,
    bg: Background,
    zoom: Option<f64>,
    caption: &str,
) -> Result<RgbImage> {
    let (w, h) = (before.width() as f64, before.height() as f64);
    let stacked = w > 1.4 * h; // wide sheets go one above the other
    let scale = match zoom {
        Some(z) => z,
        None => {
            let avail = if stacked {
                TARGET_WIDTH
            } else {
                (TARGET_WIDTH - GAP as f64) / 2.0
            };
            let s = (avail / w).min(if stacked { 420.0 / h } else { 900.0 / h });
            if s > 1.0 { s.floor().max(1.0) } else { s } // integer upscale keeps pixels crisp
        }
    };
    let sw = ((w * scale).round_ties_even() as u32).max(1);
    let sh = ((h * scale).round_ties_even() as u32).max(1);

    let panel = |im: &RgbaImage, label: &str| -> Result<RgbImage> {
        let mut base = background(bg, sw, sh, p)?;
        draw::composite(&mut base, &draw::resize(im, sw, sh), 0, 0);
        let mut out = RgbImage::from_pixel(sw, sh + LABEL_H, PANEL_BG);
        imageops::replace(&mut out, &base, 0, LABEL_H as i64);
        draw::text(&mut out, 6, 4, label, Rgb([235, 235, 240]), 15);
        Ok(out)
    };
    let (pb, pa) = (panel(before, "BEFORE")?, panel(after, "AFTER")?);
    let (width, height, pos) = if stacked {
        (sw, 2 * pb.height() + GAP, [(0, 0), (0, pb.height() + GAP)])
    } else {
        (2 * sw + GAP, pb.height(), [(0, 0), (sw + GAP, 0)])
    };
    let mut out = RgbImage::from_pixel(width.max(700), height + CAPTION_H, PANEL_BG);
    imageops::replace(
        &mut out,
        &pb,
        pos[0].0 as i64,
        (pos[0].1 + CAPTION_H) as i64,
    );
    imageops::replace(
        &mut out,
        &pa,
        pos[1].0 as i64,
        (pos[1].1 + CAPTION_H) as i64,
    );
    draw::text(&mut out, 6, 7, caption, Rgb([255, 225, 150]), 15);
    Ok(out)
}

/// Open a file in the system's default viewer, without waiting for it.
fn open_file(path: &Path) {
    let cmd = if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()
    } else {
        Command::new("xdg-open")
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    let _ = cmd;
}

/// Find the file, render before/after into `build/preview/<name>_preview.png`
/// and open it; with `watch`, re-render whenever pastel.toml changes.
pub fn run(mut p: Project, args: &PreviewArgs) -> Result<()> {
    let Some(rel) = choose(&p, &args.name, args.pick)? else {
        return Ok(());
    };
    let out_dir = p.build_dir.join("preview");
    std::fs::create_dir_all(&out_dir)?;
    let stem = Path::new(&rel)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let out_path = out_dir.join(format!("{stem}_preview.png"));

    let once = |p: &Project| -> Result<()> {
        let t = Instant::now();
        render(p, &rel, args)?
            .save(&out_path)
            .with_context(|| format!("writing {}", out_path.display()))?;
        println!(
            "{rel} -> {}  ({:.1}s)",
            p.show(&out_path),
            t.elapsed().as_secs_f64()
        );
        Ok(())
    };

    once(&p)?;
    if !args.no_open {
        open_file(&out_path);
    }
    if !args.watch {
        return Ok(());
    }
    println!("Watching pastel.toml and settings.toml. Ctrl+C to stop.");
    let mut last = p.cfg.mtime();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let now = p.cfg.mtime();
        if now == last {
            continue;
        }
        last = now;
        // Keep watching through config typos.
        if let Err(e) = p.reload_config().and_then(|_| once(&p)) {
            println!("  config error: {e:#}");
        }
    }
}
