//! Measuring the overall color of a category (or one file), which the
//! 'transfer' transform moves to its target.
use serde::{Deserialize, Serialize};

use crate::color::{Rgb, fmod, lab_to_lch, srgb_to_oklab};

/// Median color of a category (or one file), in OKLCH.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stats {
    /// Median lightness.
    #[serde(rename = "L")]
    pub l: f64,
    /// Median chroma.
    #[serde(rename = "C")]
    pub c: f64,
    /// Hue of the mean color, in degrees.
    pub h: f64,
    /// Number of pixels measured.
    #[serde(default)]
    pub n: usize,
    /// Number of files measured (category stats only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
}

/// Median OKLCH of the opaque pixels across a list of RGBA images.
pub fn color_stats(images: &[Vec<[u8; 4]>]) -> Stats {
    const MAX_PIXELS: usize = 400_000;
    let per = (MAX_PIXELS / images.len().max(1)).max(1);
    let mut px: Vec<[u8; 3]> = Vec::new();
    for im in images {
        let opaque: Vec<[u8; 3]> = im
            .iter()
            .filter(|p| p[3] > 230)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        if opaque.len() > per {
            // An even sample across the image.
            px.extend((0..per).map(|k| opaque[k * opaque.len() / per]));
        } else {
            px.extend(opaque);
        }
    }
    if px.is_empty() {
        return Stats {
            l: 0.5,
            c: 0.0,
            h: 0.0,
            n: 0,
            files: None,
        };
    }
    let lab: Vec<Rgb> = px
        .iter()
        .map(|p| srgb_to_oklab(p.map(|v| v as f64 / 255.0)))
        .collect();
    let n = lab.len() as f64;
    let mean_a = lab.iter().map(|v| v[1]).sum::<f64>() / n;
    let mean_b = lab.iter().map(|v| v[2]).sum::<f64>() / n;
    let (ls, cs): (Vec<f64>, Vec<f64>) = lab
        .iter()
        .map(|&v| {
            let (l, c, _) = lab_to_lch(v);
            (l, c)
        })
        .unzip();
    Stats {
        l: median(ls),
        c: median(cs),
        h: fmod(mean_b.atan2(mean_a).to_degrees(), 360.0),
        n: px.len(),
        files: None,
    }
}

/// Median of a non-empty list (mean of the middle two for even lengths).
fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2.0
    }
}
