//! Float views of the pixel data, and rebuilding images from them.
//!
//! The transforms work on RGB in 0..1 (and alpha in 0..1); these functions
//! convert a [`Png`] to that form and back into the same mode, and give any
//! image as plain RGBA bytes for previews and statistics.
use anyhow::{Result, bail};

use crate::color::Rgb;
use crate::image::png::{Mode, Png};

/// Round to the nearest byte (ties to even, as numpy does) and clamp.
pub fn to_u8(v: f64) -> u8 {
    (v * 255.0).round_ties_even().clamp(0.0, 255.0) as u8
}

/// Pillow reads a tRNS that makes exactly one palette entry fully
/// transparent (and the rest opaque) as a single index, not a table.
fn simple_palette_trns(t: &[u8]) -> Option<usize> {
    let zero = t.iter().position(|&a| a == 0)?;
    t.iter()
        .enumerate()
        .all(|(i, &a)| if i == zero { a == 0 } else { a == 255 })
        .then_some(zero)
}

/// For a 'P' image: palette as floats and per-entry alpha in 0..1.
pub fn palette_arrays(im: &Png) -> (Vec<Rgb>, Vec<f64>) {
    let pal = im
        .palette
        .iter()
        .map(|c| c.map(|v| v as f64 / 255.0))
        .collect();
    let mut alpha = vec![1.0; im.palette.len()];
    if let Some(t) = &im.trns {
        for (a, &v) in alpha.iter_mut().zip(t) {
            *a = v as f64 / 255.0;
        }
    }
    (pal, alpha)
}

/// New 'P' image with the same index data and a replaced palette / tRNS.
pub fn rebuild_palette(im: &Png, pal_rgb: &[Rgb], pal_alpha: &[f64]) -> Png {
    let mut out = im.clone();
    out.palette = pal_rgb.iter().map(|c| c.map(to_u8)).collect();
    if im.trns.is_some() || pal_alpha.iter().any(|&a| a < 1.0) {
        let mut a: Vec<u8> = pal_alpha.iter().map(|&v| to_u8(v)).collect();
        // Keep a full alpha table the same length; Pillow writes one
        // covering the whole palette when it read a single transparent index.
        if let Some(t) = im
            .trns
            .as_deref()
            .filter(|t| simple_palette_trns(t).is_none())
        {
            a.truncate(t.len());
        }
        out.trns = Some(a);
    }
    out
}

/// Float RGB and optional alpha of an RGBA / RGB / LA / L image.
pub fn to_arrays(im: &Png) -> (Vec<Rgb>, Option<Vec<f64>>) {
    let f = |v: u8| v as f64 / 255.0;
    let d = &im.data;
    match im.mode {
        Mode::Rgba => (
            d.as_chunks::<4>()
                .0
                .iter()
                .map(|p| [f(p[0]), f(p[1]), f(p[2])])
                .collect(),
            Some(d.as_chunks::<4>().0.iter().map(|p| f(p[3])).collect()),
        ),
        Mode::Rgb => (
            d.as_chunks::<3>()
                .0
                .iter()
                .map(|p| [f(p[0]), f(p[1]), f(p[2])])
                .collect(),
            None,
        ),
        Mode::La => (
            d.as_chunks::<2>().0.iter().map(|p| [f(p[0]); 3]).collect(),
            Some(d.as_chunks::<2>().0.iter().map(|p| f(p[1])).collect()),
        ),
        Mode::L => (d.iter().map(|&v| [f(v); 3]).collect(), None),
        m => panic!("to_arrays does not handle mode {m}"),
    }
}

/// Rebuild an image of `im`'s mode from float arrays. Grayscale modes take
/// the Rec. 709 luma of the color.
pub fn from_arrays(im: &Png, rgb: &[Rgb], alpha: Option<&[f64]>) -> Png {
    let luma = |c: &Rgb| to_u8(c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722);
    let a = |i: usize| to_u8(alpha.expect("alpha for a mode with alpha")[i]);
    let mut data = Vec::with_capacity(rgb.len() * im.mode.channels());
    for (i, c) in rgb.iter().enumerate() {
        match im.mode {
            Mode::Rgba => data.extend([to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), a(i)]),
            Mode::Rgb => data.extend(c.map(to_u8)),
            Mode::La => data.extend([luma(c), a(i)]),
            Mode::L => data.push(luma(c)),
            m => panic!("from_arrays does not handle mode {m}"),
        }
    }
    im.with_data(data)
}

/// Put back the original pixels from column `x_from` to the right edge.
pub fn restore_columns(new: &mut Png, orig: &Png, x_from: usize) -> Result<()> {
    if new.mode == Mode::P {
        bail!("can't keep part of a palette image unchanged");
    }
    let ch = new.mode.channels();
    let row = new.width * ch;
    let start = x_from.min(new.width) * ch;
    for (n, o) in new.data.chunks_mut(row).zip(orig.data.chunks(row)) {
        n[start..].copy_from_slice(&o[start..]);
    }
    Ok(())
}

/// Transparent color key (tRNS on RGB or L images), as the 8-bit sample(s).
pub fn color_key(im: &Png) -> Option<Vec<u8>> {
    let t = im.trns.as_deref()?;
    match im.mode {
        Mode::Rgb if t.len() >= 6 => Some(vec![t[1], t[3], t[5]]),
        Mode::L if t.len() >= 2 => Some(vec![t[1]]),
        _ => None,
    }
}

/// Any image as RGBA bytes (row-major), for previews and statistics.
pub fn to_rgba_display(im: &Png) -> Vec<[u8; 4]> {
    let key = color_key(im);
    let d = &im.data;
    match im.mode {
        Mode::Rgba => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect(),
        Mode::Rgb => d
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| {
                [
                    p[0],
                    p[1],
                    p[2],
                    if key.as_deref() == Some(p) { 0 } else { 255 },
                ]
            })
            .collect(),
        Mode::La => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        Mode::L => d
            .iter()
            .map(|&v| [v, v, v, if key.as_deref() == Some(&[v]) { 0 } else { 255 }])
            .collect(),
        Mode::One => d
            .iter()
            .map(|&v| if v > 0 { [255; 4] } else { [0, 0, 0, 255] })
            .collect(),
        Mode::P => {
            let (_, alpha) = palette_arrays(im);
            d.iter()
                .map(|&i| {
                    let i = i as usize;
                    let [r, g, b] = im.palette.get(i).copied().unwrap_or([0; 3]);
                    [r, g, b, alpha.get(i).map_or(255, |&a| to_u8(a))]
                })
                .collect()
        }
    }
}

/// An 8-bit RGB image in `like`'s mode (used for color lookup tables).
pub fn from_rgb8(like: &Png, rgb: &[[u8; 3]]) -> Result<Png> {
    let data = match like.mode {
        Mode::Rgb => rgb.concat(),
        Mode::Rgba => rgb.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        m => bail!("expected an RGB image, got mode {m}"),
    };
    Ok(like.with_data(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_trns() {
        assert_eq!(simple_palette_trns(&[255, 0, 255]), Some(1));
        assert_eq!(simple_palette_trns(&[0]), Some(0));
        assert_eq!(simple_palette_trns(&[255, 0, 128]), None);
    }
}
