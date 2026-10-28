//! End to end: build and validate a tiny project with one file of each kind
//! (RGBA, palette with tRNS, grayscale, RGB terrain, a shadow, a passthrough
//! icon, an empty placeholder and a non-PNG), then check what was written,
//! what wasn't, and how rebuilding behaves.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pastel::commands::build::{self, BuildArgs, BuildSummary};
use pastel::commands::validate;
use pastel::image::png::{Mode, Png};
use pastel::manifest::{CODE_VERSION, Manifest, Record};
use pastel::project::Project;
use tempfile::TempDir;

const CONFIG: &str = r##"
[global]
brightness = 1.0
saturation = 1.0

[colors]
grass = "#A8D57B"
shadow = "#3A3560"

[categories.passthrough]
type = "passthrough"

[categories.entity]
type = "remap"
lift = { floor = 0.35, ceil = 0.95, gamma = 0.8 }
detail = { sigma = 1.5, gain = 1.1 }
chroma = { scale = 1.3, add = 0.02, max = 0.2 }
hue_anchors = [[60, 80], [250, 270]]
neutral = { hue = 290, chroma = 0.03 }
outline = { width = 1.0, strength = 0.5 }

[categories."terrain.grass"]
type = "transfer"
target = "$grass"
spread = { L = 1.0, C = 1.0, h = 0.5 }

[categories.shadow]
type = "shadow"
alpha_scale = 0.6
tint = "$shadow"

[classify]
keep = [
  { match = "base/graphics/entity/box/box.png", x_from = 12 },
]
rules = [
  { match = "**/*shadow*.png",                  category = "shadow" },
  { match = "base/graphics/icons/**",           category = "passthrough" },
  { match = "base/graphics/terrain/grass-*.png", category = "terrain.grass" },
  { match = "**",                               category = "entity" },
]
"##;

/// gAMA written into every fixture PNG (1/2.2, scaled by 100000).
const GAMMA: u32 = 45455;

const BOX: &str = "base/graphics/entity/box/box.png";
const SHADOW: &str = "base/graphics/entity/box/box-shadow.png";
const PALETTE: &str = "base/graphics/entity/box/box-palette.png";
const GRAY: &str = "base/graphics/entity/box/box-gray.png";
const EMPTY: &str = "base/graphics/entity/box/empty.png";
const GRASS_1: &str = "base/graphics/terrain/grass-1.png";
const GRASS_2: &str = "base/graphics/terrain/grass-2.png";
const ICON: &str = "base/graphics/icons/gear.png";
const NOTES: &str = "base/graphics/notes.txt";
const FRAME: &str = "core/graphics/gui/frame.png";

/// The files that get recolored; the others keep no output.
const RECOLORED: [&str; 7] = [BOX, SHADOW, PALETTE, GRAY, GRASS_1, GRASS_2, FRAME];
const ALL_FILES: usize = 10;

/// A project folder in a temporary directory, deleted when dropped.
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let f = Fixture {
            dir: tempfile::tempdir().unwrap(),
        };
        f.write_config(CONFIG);
        f.write_sources();
        f
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn write_config(&self, text: &str) {
        fs::write(self.root().join("pastel.toml"), text).unwrap();
    }

    fn src(&self, rel: &str) -> PathBuf {
        self.root().join("data").join(rel)
    }

    fn out(&self, rel: &str) -> PathBuf {
        self.project().out_path(rel)
    }

    fn write_sources(&self) {
        use ::png::ColorType::{Grayscale, Indexed, Rgb, Rgba};

        // RGBA: column 0 fully transparent (with color in it), column 1 half.
        let rgba = pixels(16, 8, |x, y| {
            let a = match x {
                0 => 0,
                1 => 128,
                _ => 255,
            };
            vec![40 + 10 * x, 60 + 15 * y, 90 + 5 * (x + y), a]
        });
        write_png(&self.src(BOX), 16, 8, Rgba, &rgba, None);

        let shadow = pixels(16, 8, |x, y| {
            let a = if (x + y) % 3 == 0 { 0 } else { 180 };
            vec![0, 0, 0, a]
        });
        write_png(&self.src(SHADOW), 16, 8, Rgba, &shadow, None);

        // Palette with a partial alpha table: entry 0 clear, 1 half, 2 and 3 opaque.
        let indices = pixels(8, 4, |x, y| vec![(x + y) % 4]);
        let palette = [200, 60, 40, 60, 160, 80, 40, 80, 200, 120, 120, 120];
        let trns = [0, 128, 255];
        write_png(
            &self.src(PALETTE),
            8,
            4,
            Indexed,
            &indices,
            Some((&palette, &trns)),
        );

        let gray = pixels(8, 8, |x, y| vec![20 + 25 * x + 3 * y]);
        write_png(&self.src(GRAY), 8, 8, Grayscale, &gray, None);

        for (rel, shift) in [(GRASS_1, 0), (GRASS_2, 9)] {
            let grass = pixels(16, 16, |x, y| {
                vec![
                    60 + (x * 7 + y * 3 + shift) % 40,
                    110 + (x * 5 + y * 11 + shift) % 50,
                    40 + (x * 3 + y * 7) % 30,
                ]
            });
            write_png(&self.src(rel), 16, 16, Rgb, &grass, None);
        }

        let frame = pixels(8, 8, |x, y| vec![90 + 8 * x, 90 + 8 * y, 100]);
        write_png(&self.src(FRAME), 8, 8, Rgb, &frame, None);

        let icon = pixels(4, 4, |x, y| vec![200, 20 * x, 20 * y, 255]);
        write_png(&self.src(ICON), 4, 4, Rgba, &icon, None);

        fs::write(self.src(EMPTY), b"").unwrap();
        fs::write(self.src(NOTES), b"not an image\n").unwrap();
    }

    fn project(&self) -> Project {
        Project::open(self.root()).unwrap()
    }

    fn build(&self, only: Option<&str>) -> anyhow::Result<BuildSummary> {
        let args = BuildArgs {
            only: only.map(str::to_string),
            jobs: Some(2),
            ..BuildArgs::default()
        };
        build::run(&self.project(), &args)
    }

    fn manifest(&self) -> Manifest {
        Manifest::load(&self.project().build_dir).expect("a manifest")
    }

    /// Every file in data/*/updated_graphics, as the source path it stands for.
    fn outputs(&self) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        for folder in ["base", "core"] {
            let root = self
                .root()
                .join("data")
                .join(folder)
                .join("updated_graphics");
            for f in Project::walk_files(&root) {
                found.insert(format!("{folder}/graphics/{f}"));
            }
        }
        found
    }

    /// Bytes and modification time of every output.
    fn snapshot(&self) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
        self.outputs()
            .into_iter()
            .map(|rel| {
                let path = self.out(&rel);
                let mtime = fs::metadata(&path).unwrap().modified().unwrap();
                (rel, (fs::read(&path).unwrap(), mtime))
            })
            .collect()
    }
}

/// Row-major samples of a w x h image, from one pixel's samples at a time.
fn pixels(w: u8, h: u8, px: impl Fn(u8, u8) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            out.extend(px(x, y));
        }
    }
    out
}

/// An 8-bit PNG with a gAMA chunk, written with the png crate (not with
/// pastel's own encoder).
fn write_png(
    path: &Path,
    w: u32,
    h: u32,
    color: ::png::ColorType,
    data: &[u8],
    palette: Option<(&[u8], &[u8])>,
) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = BufWriter::new(fs::File::create(path).unwrap());
    let mut enc = ::png::Encoder::new(file, w, h);
    enc.set_color(color);
    enc.set_depth(::png::BitDepth::Eight);
    enc.set_source_gamma(::png::ScaledFloat::from_scaled(GAMMA));
    if let Some((pal, trns)) = palette {
        enc.set_palette(pal.to_vec());
        enc.set_trns(trns.to_vec());
    }
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(data).unwrap();
    writer.finish().unwrap();
}

/// The alpha samples of an RGBA image.
fn alpha(im: &Png) -> Vec<u8> {
    assert_eq!(im.mode, Mode::Rgba);
    im.data.as_chunks::<4>().0.iter().map(|p| p[3]).collect()
}

#[test]
fn build_writes_only_recolored_files_and_keeps_their_format() {
    let fx = Fixture::new();
    let summary = fx.build(None).unwrap();
    assert_eq!(
        summary,
        BuildSummary {
            recolored: 7,
            left_vanilla: 3, // the passthrough icon, the empty file, notes.txt
            ..BuildSummary::default()
        }
    );
    let expected: BTreeSet<String> = RECOLORED.iter().map(|s| s.to_string()).collect();
    assert_eq!(fx.outputs(), expected);

    // Same mode, size and metadata as the source, and actually recolored.
    for rel in RECOLORED {
        let (src, out) = (
            Png::open(&fx.src(rel)).unwrap(),
            Png::open(&fx.out(rel)).unwrap(),
        );
        assert_eq!(src.gamma(), Some(0.45455), "{rel}: fixture lacks gAMA");
        assert!(src.signature().diff(&out.signature()).is_empty(), "{rel}");
        assert!(src.data != out.data || src.palette != out.palette, "{rel}");
    }

    // RGBA: alpha and the color of clear pixels unchanged; the `keep`
    // columns (12 and up) untouched.
    let (src, out) = (
        Png::open(&fx.src(BOX)).unwrap(),
        Png::open(&fx.out(BOX)).unwrap(),
    );
    assert_eq!(alpha(&src), alpha(&out));
    for (y, (a, b)) in src
        .data
        .chunks(16 * 4)
        .zip(out.data.chunks(16 * 4))
        .enumerate()
    {
        assert_eq!(a[..4], b[..4], "clear pixel in row {y}");
        assert_eq!(a[12 * 4..], b[12 * 4..], "kept columns in row {y}");
        assert_ne!(
            a[2 * 4..12 * 4],
            b[2 * 4..12 * 4],
            "recolored columns in row {y}"
        );
    }

    // Shadow: lighter, same shape.
    let (src, out) = (
        Png::open(&fx.src(SHADOW)).unwrap(),
        Png::open(&fx.out(SHADOW)).unwrap(),
    );
    for (a, b) in alpha(&src).into_iter().zip(alpha(&out)) {
        assert_eq!(a > 0, b > 0);
        assert!(b <= a);
    }
    assert_ne!(alpha(&src), alpha(&out));

    // Palette: same indices and alpha table, new colors.
    let (src, out) = (
        Png::open(&fx.src(PALETTE)).unwrap(),
        Png::open(&fx.out(PALETTE)).unwrap(),
    );
    assert_eq!(out.mode, Mode::P);
    assert_eq!(src.data, out.data);
    assert_eq!(out.trns.as_deref(), Some(&[0, 128, 255][..]));
    assert_eq!(
        src.palette[0], out.palette[0],
        "fully clear entry keeps its color"
    );
    assert_ne!(src.palette[2..], out.palette[2..]);

    // Grayscale stays grayscale.
    assert_eq!(Png::open(&fx.out(GRAY)).unwrap().mode, Mode::L);

    assert_eq!(validate::run(&fx.project(), Some(2)).unwrap(), 7);

    let manifest = fx.manifest();
    assert_eq!(manifest.version(), CODE_VERSION);
    assert_eq!(manifest.files.len(), ALL_FILES);
    for (rel, r) in &manifest.files {
        assert!(r.problems.is_empty(), "{rel}: {:?}", r.problems);
        assert_eq!(r.out.is_some(), RECOLORED.contains(&rel.as_str()), "{rel}");
    }
    assert_eq!(
        manifest.files[GRASS_1].category.as_deref(),
        Some("terrain.grass")
    );
    assert_eq!(
        manifest.files[NOTES].category.as_deref(),
        Some("passthrough")
    );

    // Building again changes nothing at all.
    let before = fx.snapshot();
    let manifest_text = fs::read_to_string(fx.root().join("build/manifest.json")).unwrap();
    let again = fx.build(None).unwrap();
    assert_eq!(
        again,
        BuildSummary {
            up_to_date: ALL_FILES,
            ..BuildSummary::default()
        }
    );
    assert_eq!(again.processed(), 0);
    assert_eq!(fx.snapshot(), before);
    assert_eq!(
        fs::read_to_string(fx.root().join("build/manifest.json")).unwrap(),
        manifest_text
    );
}

#[test]
fn only_leaves_other_categories_alone() {
    let fx = Fixture::new();
    fx.build(None).unwrap();
    let first = fx.snapshot();
    let first_records: BTreeMap<String, Record> = fx.manifest().files;

    // New settings for entity and shadow, but only shadow and terrain are
    // rebuilt: entity outputs and records must stay exactly as they were.
    fx.write_config(
        &CONFIG
            .replace("floor = 0.35", "floor = 0.5")
            .replace("alpha_scale = 0.6", "alpha_scale = 0.4"),
    );
    let summary = fx.build(Some("shadow, terrain.*")).unwrap();
    assert_eq!(
        summary,
        BuildSummary {
            recolored: 1,  // the shadow
            up_to_date: 2, // the grass sheets
            not_selected: 7,
            ..BuildSummary::default()
        }
    );
    let now = fx.snapshot();
    assert_eq!(
        now.keys().collect::<Vec<_>>(),
        first.keys().collect::<Vec<_>>()
    );
    for (rel, (bytes, mtime)) in &now {
        let unchanged = (bytes, mtime) == (&first[rel].0, &first[rel].1);
        assert_eq!(unchanged, rel != SHADOW, "{rel}");
    }
    let records = fx.manifest().files;
    assert_eq!(records.len(), ALL_FILES);
    for (rel, r) in &records {
        let same =
            serde_json::to_value(r).unwrap() == serde_json::to_value(&first_records[rel]).unwrap();
        assert_eq!(same, rel != SHADOW, "{rel}");
    }

    // A full build then catches up on the entity files.
    let summary = fx.build(None).unwrap();
    assert_eq!(
        summary,
        BuildSummary {
            recolored: 4,    // box, palette, gray, frame
            left_vanilla: 1, // the empty placeholder
            up_to_date: 5,
            ..BuildSummary::default()
        }
    );
    assert_ne!(fs::read(fx.out(BOX)).unwrap(), first[BOX].0);
    assert_eq!(validate::run(&fx.project(), Some(2)).unwrap(), 7);
    assert_eq!(fx.build(None).unwrap().processed(), 0);
}

#[test]
fn removed_sources_lose_their_output() {
    let fx = Fixture::new();
    fx.build(None).unwrap();
    fs::remove_file(fx.src(GRASS_2)).unwrap();
    let summary = fx.build(None).unwrap();
    assert_eq!(summary.stale_removed, 1);
    assert!(!fx.out(GRASS_2).exists());
    assert!(!fx.manifest().files.contains_key(GRASS_2));
    assert_eq!(validate::run(&fx.project(), Some(2)).unwrap(), 6);
}

#[test]
fn unclassified_pngs_fail_the_build() {
    let fx = Fixture::new();
    // Drop the catch-all rule.
    let config: Vec<&str> = CONFIG
        .lines()
        .filter(|l| !l.contains(r#"category = "entity""#))
        .collect();
    assert_eq!(config.len(), CONFIG.lines().count() - 1);
    fx.write_config(&config.join("\n"));
    let err = fx.build(None).unwrap_err().to_string();
    assert!(err.contains("[classify]"), "{err}");
    assert!(validate::run(&fx.project(), Some(2)).is_err());
    assert!(fx.outputs().is_empty());
    let unmatched = fs::read_to_string(fx.root().join("build/unmatched.txt")).unwrap();
    assert!(unmatched.contains(BOX) && unmatched.contains(FRAME));
}

#[test]
fn recolored_sources_stop_the_build() {
    let fx = Fixture::new();
    fx.build(None).unwrap();
    fs::copy(fx.out(BOX), fx.src(BOX)).unwrap();
    let err = fx.build(None).unwrap_err().to_string();
    assert!(err.contains("already recolored"), "{err}");

    let args = BuildArgs {
        force: true,
        jobs: Some(2),
        ..BuildArgs::default()
    };
    build::run(&fx.project(), &args).unwrap();
}

#[test]
fn validate_reports_broken_outputs() {
    let fx = Fixture::new();
    fx.build(None).unwrap();
    // An output with different alpha, and one for a file that is never recolored.
    fs::copy(fx.src(SHADOW), fx.out(BOX)).unwrap();
    fs::create_dir_all(fx.out(ICON).parent().unwrap()).unwrap();
    fs::copy(fx.src(ICON), fx.out(ICON)).unwrap();
    let err = validate::run(&fx.project(), Some(2)).unwrap_err();
    assert_eq!(err.to_string(), "Validation failed.");
}
