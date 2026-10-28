//! The 'remap' transform: lightness lift with detail preservation, outline
//! keeping, hue remapping, neutral tinting, cartoon shading and chroma
//! shaping, optionally with separate settings for muted (grayish) pixels.
use anyhow::Result;
use serde_json::Value;

use crate::color::{Rgb, fmod, lch_to_lab};
use crate::config::value::{as_f64, deep_merge, num, truthy};
use crate::recolor::blur::{Edge, gaussian, weighted_blur};
use crate::recolor::hue::{HueRemap, wrap};
use crate::recolor::{Dims, Lch};

/// The general 'remap' transform on OKLCH arrays.
pub(super) fn remap_lch(
    mut src: Lch,
    alpha: Option<&[f64]>,
    d: Dims,
    p: &Value,
    g: &Value,
    image_shaped: bool,
) -> Result<Lch> {
    let n = src.l.len();
    let brightness = num(g, "brightness", 1.0);
    let saturation = num(g, "saturation", 1.0);

    // 1. Lightness lift, applied to a blurred base; local detail added back.
    let lift = &p["lift"];
    let det = &p["detail"];
    let sigma = if image_shaped {
        num(det, "sigma", 0.0)
    } else {
        0.0
    };
    let gain = num(det, "gain", 1.0);
    let l_base = weighted_blur(&src.l, alpha, d, sigma);

    // Smooth the color (not the lightness) a little before remapping, so
    // grain in grayish areas doesn't turn into rainbow speckles once chroma
    // is boosted. Gives cleaner, flatter, more cartoon-like color areas.
    let cb = if image_shaped {
        num(det, "color_blur", 0.0)
    } else {
        0.0
    };
    if cb > 0.0 {
        let lab: Vec<Rgb> = (0..n)
            .map(|i| lch_to_lab(src.l[i], src.c[i], src.h[i]))
            .collect();
        let a = weighted_blur(&lab.iter().map(|v| v[1]).collect::<Vec<_>>(), alpha, d, cb);
        let b = weighted_blur(&lab.iter().map(|v| v[2]).collect::<Vec<_>>(), alpha, d, cb);
        for i in 0..n {
            src.c[i] = a[i].hypot(b[i]);
            src.h[i] = fmod(b[i].atan2(a[i]).to_degrees(), 360.0);
        }
    }
    let floor = num(lift, "floor", 0.0);
    let ceil = num(lift, "ceil", 1.0);
    let gamma = num(lift, "gamma", 1.0);
    let mut l2: Vec<f64> = (0..n)
        .map(|i| {
            let lb = l_base[i];
            let lifted = floor + (ceil - floor) * lb.clamp(0.0, 1.0).powf(gamma);
            // Global brightness scales only the brightening: a deliberate darkening
            // (belt rubber, hazard stripes) stays as designed at any brightness.
            let lifted = lb + super::scale_brightening(lifted - lb, brightness);
            (lifted + gain * (src.l[i] - lb)).clamp(0.0, 1.0)
        })
        .collect();

    // 1b. Outline: near the sprite's edge (where it meets transparency), pull
    //     lightness back toward the original dark contour, so the silhouette
    //     stays crisp against light ground instead of fading into it.
    let ol = &p["outline"];
    if let (true, Some(alpha), true) = (truthy(ol), alpha, image_shaped) {
        let width = num(ol, "width", 2.0);
        let strength = num(ol, "strength", 0.6);
        let solid: Vec<f64> = alpha
            .iter()
            .map(|&a| if a > 0.5 { 1.0 } else { 0.0 })
            .collect();
        let blurred = gaussian(&solid, d, width, Edge::Constant);
        for i in 0..n {
            let edge = ((1.0 - blurred[i]) * 2.0).clamp(0.0, 1.0) * solid[i];
            l2[i] += edge * strength * (src.l[i].min(l2[i]) - l2[i]);
        }
    }

    // 2. Hue remap. Optionally only for muted colors, so saturated accents
    //    (belt yellow, inserter blue) keep their hue while drab panels shift.
    let remap = HueRemap::new(&p["hue_anchors"])?;
    let chroma_range = match p["hue_chroma_range"]
        .as_array()
        .map(|r| r.iter().map(as_f64).collect::<Vec<_>>())
    {
        Some(r) if r.len() == 2 => Some((r[0].unwrap_or(0.0), r[1].unwrap_or(0.0))),
        _ => None,
    };
    let neu = truthy(&p["neutral"]).then_some(&p["neutral"]);
    let sh = truthy(&p["shade"]).then_some(&p["shade"]);
    let ch = &p["chroma"];
    let (scale, add, max) = (
        num(ch, "scale", 1.0),
        num(ch, "add", 0.0),
        num(ch, "max", 0.4),
    );

    let mut h2 = vec![0.0; n];
    let mut c2 = vec![0.0; n];
    for i in 0..n {
        let (c, h) = (src.c[i], src.h[i]);
        let mut hh = remap.as_ref().map_or(h, |r| r.apply(h));
        if let Some((lo, hi)) = chroma_range {
            let w = ((hi - c) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
            hh = fmod(h + w * wrap(hh - h), 360.0);
        }

        // 3. Neutral tint: near-gray pixels pick up a category hue.
        let mut cc = c;
        if let Some(neu) = neu {
            let w = (1.0 - c / num(neu, "chroma_below", 0.03)).clamp(0.0, 1.0);
            hh = fmod(hh + w * wrap(num(neu, "hue", 0.0) - hh), 360.0);
            cc += w * num(neu, "chroma", 0.02);
        }

        // 3b. Cartoon shading: the originally dark areas lean toward a cool
        //     shade hue (lavender) instead of reading as gray-black once lifted.
        if let Some(sh) = sh {
            let below = num(sh, "below", 0.35);
            let w = ((below - l_base[i]) / below).clamp(0.0, 1.0) * num(sh, "strength", 0.6);
            hh = fmod(hh + w * wrap(num(sh, "hue", 290.0) - hh), 360.0);
            cc += w * num(sh, "chroma", 0.03);
        }

        // 4. Chroma shaping.
        c2[i] = (cc * scale + add).min(max).max(0.0) * saturation;
        h2[i] = hh;
    }
    Ok(Lch {
        l: l2,
        c: c2,
        h: h2,
    })
}

/// 'remap' with separate settings for muted (grayish) pixels.
///
/// `muted` holds remap settings that override the category's own for pixels
/// with chroma below `muted.below`; pixels above `muted.above` use the
/// category's settings, and those in between blend the two. This lets a belt's
/// gray rubber go dark while its colored arrows go pastel. `muted.blur` decides
/// by the chroma of the surroundings (blur radius in pixels), so specks of
/// gray inside a colored area don't flip on their own.
pub(super) fn remap_muted(
    src: Lch,
    alpha: Option<&[f64]>,
    d: Dims,
    cat: &Value,
    g: &Value,
    image_shaped: bool,
) -> Result<Lch> {
    let muted = &cat["muted"];
    let (below, above) = (num(muted, "below", 0.04), num(muted, "above", 0.08));
    let mut p_muted = cat.clone();
    deep_merge(&mut p_muted, muted.clone());
    let blur = if image_shaped {
        num(muted, "blur", 0.0)
    } else {
        0.0
    };
    let chroma = weighted_blur(&src.c, alpha, d, blur);
    let colored = remap_lch(src.clone(), alpha, d, cat, g, image_shaped)?;
    let gray = remap_lch(src, alpha, d, &p_muted, g, image_shaped)?;
    let mut out = colored;
    for (i, c) in chroma.iter().enumerate() {
        let w = ((above - c) / (above - below).max(1e-6)).clamp(0.0, 1.0);
        out.l[i] += w * (gray.l[i] - out.l[i]);
        out.c[i] += w * (gray.c[i] - out.c[i]);
        out.h[i] = fmod(out.h[i] + w * wrap(gray.h[i] - out.h[i]), 360.0);
    }
    Ok(out)
}
