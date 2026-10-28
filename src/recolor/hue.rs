//! Moving hues around the color wheel: piecewise-linear remapping through
//! anchor points, and hue differences that wrap around 360 degrees.
use anyhow::{Result, bail};
use serde_json::Value;

use crate::color::fmod;
use crate::config::value::as_f64;

/// Wrap hue differences into -180..180.
pub fn wrap(d: f64) -> f64 {
    fmod(d + 180.0, 360.0) - 180.0
}

/// Piecewise-linear, order-preserving remap of the hue circle.
///
/// anchors: list of [source_hue, target_hue]. Hues between two anchors are
/// interpolated; the mapping wraps around the circle.
#[derive(Debug)]
pub struct HueRemap {
    src: Vec<f64>,
    off: Vec<f64>,
}

impl HueRemap {
    /// The remap for a `hue_anchors` setting; None when it is missing or empty.
    pub fn new(anchors: &Value) -> Result<Option<HueRemap>> {
        let Some(list) = anchors.as_array().filter(|a| !a.is_empty()) else {
            return Ok(None);
        };
        let mut a = Vec::new();
        for pair in list {
            match pair
                .as_array()
                .map(|p| p.iter().map(as_f64).collect::<Vec<_>>())
                .as_deref()
            {
                Some([Some(s), Some(t)]) => a.push((fmod(*s, 360.0), *t)),
                _ => bail!("hue_anchors entries must be [from, to] pairs, got {pair}"),
            }
        }
        a.sort_by(|x, y| x.0.total_cmp(&y.0));
        // Unwrap targets so consecutive anchors move by their shortest path,
        // and close the circle with a wrapped copy of the first anchor.
        let mut src: Vec<f64> = a.iter().map(|x| x.0).collect();
        let mut off: Vec<f64> = a.iter().map(|&(s, t)| wrap(t - s)).collect();
        if a.len() > 1 {
            src.push(src[0] + 360.0);
            off.push(off[0]);
        }
        Ok(Some(HueRemap { src, off }))
    }

    /// The new hue for hue `h` (degrees).
    pub fn apply(&self, h: f64) -> f64 {
        if self.src.len() == 1 {
            return fmod(h + self.off[0], 360.0);
        }
        let hh = if h < self.src[0] { h + 360.0 } else { h };
        fmod(h + interp(hh, &self.src, &self.off), 360.0)
    }
}

/// numpy.interp for one value.
fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let last = xp.len() - 1;
    if x < xp[0] {
        return fp[0];
    }
    if x >= xp[last] {
        return fp[last];
    }
    let j = xp.partition_point(|&v| v <= x) - 1;
    let slope = (fp[j + 1] - fp[j]) / (xp[j + 1] - xp[j]);
    slope * (x - xp[j]) + fp[j]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hue_remap_wraps() {
        let r = HueRemap::new(&json!([[40, 50], [320, 330]]))
            .unwrap()
            .unwrap();
        assert!((r.apply(40.0) - 50.0).abs() < 1e-9);
        assert!((r.apply(320.0) - 330.0).abs() < 1e-9);
        assert!((r.apply(0.0) - 10.0).abs() < 1e-9);
        let one = HueRemap::new(&json!([[10, 350]])).unwrap().unwrap();
        assert!((one.apply(5.0) - 345.0).abs() < 1e-9);
    }
}
