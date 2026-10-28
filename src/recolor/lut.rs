//! Factorio color lookup tables (core/graphics/color_luts/*.png).
//!
//! Each file is a 16x16x16 3D LUT stored as a 256x16 RGB image: 16 tiles of
//! 16x16 pixels side by side. Within a tile, red increases left to right and
//! green top to bottom; blue increases from tile to tile.
use anyhow::{Result, bail};
use serde_json::Value;

use crate::color::{Rgb, hex_to_unit};
use crate::config::value::{num, string, truthy};
use crate::image::pixels;
use crate::image::png::Png;

const N: usize = 16;

/// Table indexed `[b][g][r]`, values 0..1.
#[derive(Debug)]
pub struct Table(Vec<Rgb>);

impl Table {
    fn at(&self, b: usize, g: usize, r: usize) -> Rgb {
        self.0[(b * N + g) * N + r]
    }
}

/// Read the table out of a LUT image.
pub fn decode(im: &Png) -> Result<Table> {
    if im.size() != (N * N, N) {
        bail!(
            "unexpected LUT size {}x{} (expected 256x16)",
            im.width,
            im.height
        );
    }
    let px = pixels::to_rgba_display(im);
    let mut t = vec![[0.0; 3]; N * N * N];
    for b in 0..N {
        for g in 0..N {
            for r in 0..N {
                let p = px[g * N * N + b * N + r];
                t[(b * N + g) * N + r] = [p[0], p[1], p[2]].map(|v| v as f64 / 255.0);
            }
        }
    }
    Ok(Table(t))
}

/// The table as an image in `like`'s mode.
pub fn encode(table: &Table, like: &Png) -> Result<Png> {
    let mut px = vec![[0u8; 3]; N * N * N];
    for b in 0..N {
        for g in 0..N {
            for r in 0..N {
                px[g * N * N + b * N + r] = table.at(b, g, r).map(pixels::to_u8);
            }
        }
    }
    pixels::from_rgb8(like, &px)
}

/// The table that maps every color to itself.
fn identity() -> Table {
    let v = |i: usize| i as f64 / (N - 1) as f64;
    let mut t = Vec::with_capacity(N * N * N);
    for b in 0..N {
        for g in 0..N {
            for r in 0..N {
                t.push([v(r), v(g), v(b)]);
            }
        }
    }
    Table(t)
}

/// How to make a LUT gentler.
///
/// softness       0..1, blends each output color back toward its input
///                (0 = unchanged, 1 = identity / no grading at all).
/// moon           hex color; the darkest outputs are lifted toward it with a
///                screen blend, so night shadows become dim violet, not black.
/// moon_strength  0..1 strength of that lift.
/// tint, tint_strength   optional multiplicative color cast (e.g. warm dusk).
#[derive(Debug)]
pub struct Soften {
    softness: f64,
    moon: Option<Rgb>,
    moon_strength: f64,
    tint: Option<Rgb>,
    tint_strength: f64,
}

impl Soften {
    /// Settings for one file from the category's `files:` entry; None if the
    /// file isn't listed (it stays unchanged).
    pub fn for_file(cat: &Value, name: &str) -> Result<Option<Soften>> {
        let fc = &cat["files"][name];
        if !truthy(fc) {
            return Ok(None);
        }
        Ok(Some(Soften {
            softness: num(fc, "softness", 0.0),
            moon: string(fc, "moon").map(hex_to_unit).transpose()?,
            moon_strength: num(fc, "moon_strength", 0.0),
            tint: string(fc, "tint").map(hex_to_unit).transpose()?,
            tint_strength: num(fc, "tint_strength", 0.0),
        }))
    }

    /// The softened table.
    pub fn apply(&self, table: &Table) -> Table {
        let ident = identity();
        Table(
            table
                .0
                .iter()
                .zip(&ident.0)
                .map(|(t, id)| {
                    let mut out: Rgb =
                        std::array::from_fn(|k| t[k] + self.softness * (id[k] - t[k]));
                    if let (Some(tint), true) = (self.tint, self.tint_strength != 0.0) {
                        let ts = self.tint_strength;
                        out = std::array::from_fn(|k| out[k] * (1.0 - ts + ts * tint[k]));
                    }
                    if let (Some(moon), true) = (self.moon, self.moon_strength != 0.0) {
                        out = std::array::from_fn(|k| {
                            1.0 - (1.0 - out[k]) * (1.0 - moon[k] * self.moon_strength)
                        });
                    }
                    out.map(|v| v.clamp(0.0, 1.0))
                })
                .collect(),
        )
    }
}

/// Grade an RGB color (0..1) through a table with trilinear interpolation.
pub fn apply(table: &Table, rgb: Rgb) -> Rgb {
    let x = rgb.map(|v| v.clamp(0.0, 1.0) * (N - 1) as f64);
    let i0 = x.map(|v| (v.floor() as usize).min(N - 2));
    let f: Rgb = std::array::from_fn(|k| x[k] - i0[k] as f64);
    let [r0, g0, b0] = i0;
    let mut out = [0.0; 3];
    for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
        for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
            for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
                let w = wb * wg * wr;
                let c = table.at(b0 + db, g0 + dg, r0 + dr);
                for k in 0..3 {
                    out[k] += c[k] * w;
                }
            }
        }
    }
    out
}

/// The softened LUT image, or None to leave the file unchanged.
pub fn transform_file(im: &Png, cat: &Value, name: &str) -> Result<Option<Png>> {
    let Some(soften) = Soften::for_file(cat, name)? else {
        return Ok(None);
    };
    Ok(Some(encode(&soften.apply(&decode(im)?), im)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_grades_to_itself() {
        let t = identity();
        let c = [0.2, 0.55, 0.9];
        let out = apply(&t, c);
        assert!((0..3).all(|k| (out[k] - c[k]).abs() < 1e-12));
    }
}
