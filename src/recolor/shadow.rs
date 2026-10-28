//! The 'shadow' transform: lighter shadows, optionally tinted, with exactly
//! the same shape.
use anyhow::Result;
use serde_json::Value;

use crate::color::hex_to_unit;
use crate::config::value::{num, string};
use crate::image::pixels;
use crate::image::png::{Mode, Png};

/// Soften a shadow sprite: scale alpha, set RGB to the tint color.
pub(super) fn shadow_image(im: &Png, cat: &Value) -> Result<Option<Png>> {
    let scale = num(cat, "alpha_scale", 0.6);
    let tint = string(cat, "tint").map(hex_to_unit).transpose()?;
    // Scale opacity, but never turn a visible pixel fully invisible,
    // so the shadow's shape stays exactly the same.
    let soften = |a: &f64| {
        if *a > 0.0 {
            (a * scale).max(1.0 / 255.0)
        } else {
            0.0
        }
    };

    Ok(match im.mode {
        Mode::P => {
            let (mut pal, pal_a) = pixels::palette_arrays(im);
            if let Some(t) = tint {
                for (c, &a) in pal.iter_mut().zip(&pal_a) {
                    if a > 0.0 {
                        *c = t;
                    }
                }
            }
            let soft: Vec<f64> = pal_a.iter().map(soften).collect();
            Some(pixels::rebuild_palette(im, &pal, &soft))
        }
        Mode::Rgba | Mode::La => {
            let (mut rgb, a) = pixels::to_arrays(im);
            let a = a.unwrap();
            if let (Some(t), Mode::Rgba) = (tint, im.mode) {
                for (c, &a) in rgb.iter_mut().zip(&a) {
                    if a > 0.0 {
                        *c = t;
                    }
                }
            }
            let soft: Vec<f64> = a.iter().map(soften).collect();
            Some(pixels::from_arrays(im, &rgb, Some(&soft)))
        }
        _ => None, // no alpha to soften: leave unchanged
    })
}
