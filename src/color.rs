//! sRGB <-> OKLab / OKLCH conversion and gamut mapping, per pixel.
//!
//! sRGB values are in 0..1. Lightness L is 0..1, chroma C is >= 0 and hue h
//! is in degrees 0..360.

const M1: [[f64; 3]; 3] = [
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
];
const M2: [[f64; 3]; 3] = [
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
];
// Inverses of M1 and M2, exactly as numpy.linalg.inv computes them, so results
// match the original Python pipeline bit for bit.
const M1_INV: [[f64; 3]; 3] = [
    [4.076741661347994, -3.3077115904081933, 0.23096992872942793],
    [-1.2684380040921763, 2.6097574006633715, -0.3413193963102196],
    [
        -0.004196086541837079,
        -0.7034186144594495,
        1.7076147009309446,
    ],
];
const M2_INV: [[f64; 3]; 3] = [
    [0.9999999984505196, 0.39633779217376774, 0.2158037580607588],
    [
        1.0000000088817607,
        -0.10556134232365633,
        -0.0638541747717059,
    ],
    [
        1.0000000546724108,
        -0.08948418209496574,
        -1.2914855378640917,
    ],
];

/// An RGB (or OKLab) triple; sRGB components are in 0..1.
pub type Rgb = [f64; 3];

/// Matrix times column vector.
fn mat3(x: Rgb, m: &[[f64; 3]; 3]) -> Rgb {
    std::array::from_fn(|i| m[i][0] * x[0] + m[i][1] * x[1] + m[i][2] * x[2])
}

/// One sRGB component to linear light.
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// One linear-light component to sRGB (negative values count as 0).
pub fn linear_to_srgb(c: f64) -> f64 {
    let c = c.max(0.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB color to OKLab.
pub fn srgb_to_oklab(rgb: Rgb) -> Rgb {
    let lms = mat3(rgb.map(srgb_to_linear), &M1);
    mat3(lms.map(f64::cbrt), &M2)
}

/// OKLab color to linear-light RGB (may be out of gamut).
pub fn oklab_to_linear(lab: Rgb) -> Rgb {
    let lms = mat3(lab, &M2_INV).map(|v| v.powf(3.0));
    mat3(lms, &M1_INV)
}

/// Floor modulo, like Python's `%` with a positive divisor.
pub fn fmod(a: f64, b: f64) -> f64 {
    a.rem_euclid(b)
}

/// Lightness, chroma, hue of an OKLab color.
pub fn lab_to_lch(lab: Rgb) -> (f64, f64, f64) {
    let c = lab[1].hypot(lab[2]);
    let h = fmod(lab[2].atan2(lab[1]).to_degrees(), 360.0);
    (lab[0], c, h)
}

/// OKLab color from lightness, chroma and hue (degrees).
pub fn lch_to_lab(l: f64, c: f64, h: f64) -> Rgb {
    let hr = h.to_radians();
    [l, c * hr.cos(), c * hr.sin()]
}

/// Convert OKLCH to sRGB, pulling out-of-gamut colors back by reducing chroma.
///
/// Lightness and hue are kept exactly; only chroma is lowered (binary search),
/// so colors never shift hue the way plain RGB clipping would.
pub fn lch_to_srgb_gamut(l: f64, c: f64, h: f64) -> Rgb {
    const ITERS: usize = 14;
    const EPS: f64 = 1e-4;
    let in_gamut = |lin: Rgb| lin.iter().all(|&v| (-EPS..=1.0 + EPS).contains(&v));
    let l = l.clamp(0.0, 1.0);
    let mut lin = oklab_to_linear(lch_to_lab(l, c, h));
    if !in_gamut(lin) {
        let (mut lo, mut hi) = (0.0, c);
        for _ in 0..ITERS {
            let mid = (lo + hi) / 2.0;
            if in_gamut(oklab_to_linear(lch_to_lab(l, mid, h))) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lin = oklab_to_linear(lch_to_lab(l, lo, h));
    }
    lin.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)).clamp(0.0, 1.0))
}

/// "#RRGGBB" (the # is optional) as bytes.
pub fn hex_to_rgb255(hex: &str) -> anyhow::Result<[u8; 3]> {
    let h = hex.trim_start_matches('#');
    let byte = |i: usize| {
        h.get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .ok_or_else(|| anyhow::anyhow!("'{hex}' is not a hex color like #A0D680"))
    };
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

/// "#RRGGBB" as sRGB in 0..1.
pub fn hex_to_unit(hex: &str) -> anyhow::Result<Rgb> {
    Ok(hex_to_rgb255(hex)?.map(|v| v as f64 / 255.0))
}

/// "#RRGGBB" as OKLCH (lightness, chroma, hue in degrees).
pub fn hex_to_lch(hex: &str) -> anyhow::Result<(f64, f64, f64)> {
    Ok(lab_to_lch(srgb_to_oklab(hex_to_unit(hex)?)))
}

/// Test image: hues left to right; from light at the top, through full
/// color, to dark at the bottom. Row-major RGB, w * h pixels.
pub fn hsv_card(w: usize, h: usize) -> Vec<Rgb> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let t = if h > 1 {
            y as f64 * (1.0 / (h - 1) as f64)
        } else {
            0.0
        };
        let s = (t * 2.0).clamp(0.0, 1.0);
        let v = (2.0 - t * 2.0).clamp(0.0, 1.0) * 0.95 + 0.05;
        for x in 0..w {
            let hue = x as f64 * (1.0 / w as f64);
            let i = (hue * 6.0).floor() as usize % 6;
            let f = hue * 6.0 - (hue * 6.0).floor();
            let (p, q, u) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
            out.push(match i {
                0 => [v, u, p],
                1 => [q, v, p],
                2 => [p, v, u],
                3 => [p, q, v],
                4 => [u, p, v],
                _ => [v, p, q],
            });
        }
    }
    out
}
