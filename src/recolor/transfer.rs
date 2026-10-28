//! The 'transfer' transform: move the category's median color to a target
//! color, keeping (and optionally scaling) the variation around it.
use anyhow::{Result, bail};
use serde_json::Value;

use crate::color::{fmod, hex_to_lch};
use crate::config::value::{as_f64, num, string};
use crate::recolor::Lch;
use crate::recolor::hue::{HueRemap, wrap};
use crate::recolor::stats::Stats;

/// Roll off lightness above `knee` smoothly instead of clipping to white,
/// so bright specks keep some color and texture.
fn soft_max(l: f64, knee: Option<f64>) -> f64 {
    match knee {
        Some(k) if l > k => k + (1.0 - k) * (1.0 - (-(l - k).max(0.0) / (1.0 - k)).exp()),
        _ => l,
    }
}

/// The 'transfer' transform: move the category's median color to a target.
pub(super) fn transfer_lch(src: Lch, p: &Value, g: &Value, stats: &Stats) -> Result<Lch> {
    let brightness = num(g, "brightness", 1.0);
    let saturation = num(g, "saturation", 1.0);
    let Some(target) = string(p, "target") else {
        bail!("a 'transfer' category needs a `target` color")
    };
    let (tl, tc, th) = hex_to_lch(target)?;
    let (sl, sc, sh) = (stats.l, stats.c, stats.h);
    let tl = sl + super::scale_brightening(tl - sl, brightness);
    let sp = &p["spread"];
    let (spl, spc, sph) = (num(sp, "L", 1.0), num(sp, "C", 1.0), num(sp, "h", 1.0));
    let knee = as_f64(&p["soft_max"]);
    let chroma_max = num(p, "chroma_max", 0.4);
    // Optional fine hue remap after the transfer (e.g. push patches apart).
    let remap = HueRemap::new(&p["hue_anchors"])?;
    let n = src.l.len();
    let mut out = Lch {
        l: vec![0.0; n],
        c: vec![0.0; n],
        h: vec![0.0; n],
    };
    for i in 0..n {
        out.l[i] = soft_max(tl + (src.l[i] - sl) * spl, knee).clamp(0.0, 1.0);
        out.c[i] = ((tc + (src.c[i] - sc) * spc).max(0.0) * saturation).min(chroma_max);
        let h = fmod(th + wrap(src.h[i] - sh) * sph, 360.0);
        out.h[i] = remap.as_ref().map_or(h, |r| r.apply(h));
    }
    Ok(out)
}
