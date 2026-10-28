//! Color transforms.
//!
//! Category types (set with `type =` in pastel.toml):
//!
//!   passthrough  File is copied unchanged.
//!   remap        General transform: lightness lift with detail preservation,
//!                hue remapping, neutral tinting and chroma shaping.
//!   transfer     Moves the category's overall color (measured across all files
//!                in the category) to a target color, keeping the variation
//!                around it. Used for terrain and ores.
//!   shadow       Softens opacity and optionally tints.
//!   lut          Color lookup tables (see lut.rs).
//!
//! remap and transfer work per pixel in OKLCH (see remap.rs, transfer.rs);
//! this module converts to and from it and handles the image modes.
pub mod blur;
pub mod hue;
pub mod lut;
mod remap;
mod shadow;
pub mod stats;
mod transfer;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::color::{Rgb, lab_to_lch, lch_to_lab, lch_to_srgb_gamut, srgb_to_oklab};
use crate::config::value::{num, truthy};
use crate::image::pixels;
use crate::image::png::{Mode, Png};

use self::stats::Stats;

/// A lightness change `delta`, scaled by the global brightness only when it
/// brightens. Darkening is part of a category's design, not of its lift.
pub(crate) fn scale_brightening(delta: f64, brightness: f64) -> f64 {
    if delta > 0.0 {
        delta * brightness
    } else {
        delta
    }
}

/// A 2D float image (or a 1-row list of palette entries).
#[derive(Clone, Copy, Debug)]
pub struct Dims {
    /// Width in pixels.
    pub w: usize,
    /// Height in pixels.
    pub h: usize,
}

/// Lightness, chroma and hue of every pixel, as separate arrays.
#[derive(Clone)]
struct Lch {
    l: Vec<f64>,
    c: Vec<f64>,
    h: Vec<f64>,
}

/// Recolor float RGB with optional alpha according to a category.
fn recolor_rgb(
    rgb: &[Rgb],
    alpha: Option<&[f64]>,
    d: Dims,
    cat: &Value,
    g: &Value,
    stats: Option<&Stats>,
    image_shaped: bool,
) -> Result<Vec<Rgb>> {
    let lab: Vec<Rgb> = rgb.iter().map(|&c| srgb_to_oklab(c)).collect();
    let mut src = Lch {
        l: vec![],
        c: vec![],
        h: vec![],
    };
    for &v in &lab {
        let (l, c, h) = lab_to_lch(v);
        src.l.push(l);
        src.c.push(c);
        src.h.push(h);
    }
    let out = match cat["type"].as_str() {
        Some("remap") if truthy(&cat["muted"]) => {
            remap::remap_muted(src, alpha, d, cat, g, image_shaped)?
        }
        Some("remap") => remap::remap_lch(src, alpha, d, cat, g, image_shaped)?,
        Some("transfer") => {
            let Some(stats) = stats else {
                bail!("'transfer' needs category statistics")
            };
            transfer::transfer_lch(src, cat, g, stats)?
        }
        t => bail!("cannot recolor with type {t:?}"),
    };
    // strength: 1 = full recolor, 0 = original, 0.5 = halfway. Blended in OKLab,
    // so halfway is a perceptually even mix of the original and the new color.
    let s = num(cat, "strength", 1.0);
    Ok((0..lab.len())
        .map(|i| {
            let (mut l, mut c, mut h) = (out.l[i], out.c[i], out.h[i]);
            if s != 1.0 {
                let new = lch_to_lab(l, c, h);
                let mixed: Rgb = std::array::from_fn(|k| lab[i][k] + s * (new[k] - lab[i][k]));
                (l, c, h) = lab_to_lch(mixed);
            }
            lch_to_srgb_gamut(l, c, h)
        })
        .collect())
}

/// The recolored image, or None if the file should be copied unchanged.
///
/// `cat` is the category's settings, `g` the `[global]` table and `stats`
/// the category's measured color (needed by 'transfer'). 'lut' files go
/// through [`lut::transform_file`] instead.
pub fn recolor_image(
    im: &Png,
    cat: &Value,
    g: &Value,
    stats: Option<&Stats>,
) -> Result<Option<Png>> {
    let t = cat["type"].as_str().unwrap_or("");
    if t == "passthrough" || !im.mode.supported() {
        return Ok(None);
    }
    if t == "shadow" {
        return shadow::shadow_image(im, cat);
    }
    if t != "remap" && t != "transfer" {
        bail!("recolor_image cannot apply type {t}");
    }

    if im.mode == Mode::P {
        let (pal, pal_a) = pixels::palette_arrays(im);
        if pal.is_empty() {
            return Ok(None);
        }
        let d = Dims { w: pal.len(), h: 1 };
        let mut new = recolor_rgb(&pal, Some(&pal_a), d, cat, g, stats, false)?;
        for i in 0..new.len() {
            if pal_a[i] <= 0.0 {
                new[i] = pal[i];
            }
        }
        return Ok(Some(pixels::rebuild_palette(im, &new, &pal_a)));
    }

    let (rgb, alpha) = pixels::to_arrays(im);
    let d = Dims {
        w: im.width,
        h: im.height,
    };
    let mut new = recolor_rgb(&rgb, alpha.as_deref(), d, cat, g, stats, true)?;
    // Fully transparent pixels and color-keyed pixels keep their exact value.
    let key = pixels::color_key(im);
    let ch = if im.mode == Mode::Rgb { 3 } else { 1 };
    for i in 0..new.len() {
        let clear = alpha.as_ref().is_some_and(|a| a[i] == 0.0);
        let keyed = key
            .as_deref()
            .is_some_and(|k| im.data[i * ch..(i + 1) * ch] == *k);
        if clear || keyed {
            new[i] = rgb[i];
        }
    }
    Ok(Some(pixels::from_arrays(im, &new, alpha.as_deref())))
}

#[cfg(test)]
mod tests {
    #[test]
    fn brightness_scales_only_brightening() {
        assert_eq!(super::scale_brightening(0.4, 0.5), 0.2);
        assert_eq!(super::scale_brightening(-0.3, 0.5), -0.3);
        assert_eq!(super::scale_brightening(0.4, 1.0), 0.4);
    }
}
