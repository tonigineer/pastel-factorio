//! Gaussian blur, matching scipy.ndimage.gaussian_filter (truncate = 4), and
//! a blur weighted by alpha so transparent pixels don't bleed in.
use crate::recolor::Dims;

/// What a blur sees beyond the image's edge.
#[derive(Clone, Copy, Debug)]
pub enum Edge {
    /// Mirror at the edge: d c b a | a b c d | d c b a
    Reflect,
    /// Zero outside the image.
    Constant,
}

/// Normalized 1D kernel, radius 4 sigma (rounded), as scipy builds it.
fn gaussian_kernel(sigma: f64) -> Vec<f64> {
    let radius = (4.0 * sigma + 0.5) as i64;
    let s2 = sigma * sigma;
    let phi: Vec<f64> = (-radius..=radius)
        .map(|x| (-0.5 / s2 * (x * x) as f64).exp())
        .collect();
    let sum: f64 = phi.iter().sum();
    phi.iter().map(|v| v / sum).collect()
}

/// Correlate every line along one axis. Symmetric kernel, summed the way
/// scipy does it: center first, then pairs from the outside in.
fn blur_axis(src: &[f64], d: Dims, k: &[f64], vertical: bool, edge: Edge) -> Vec<f64> {
    let r = (k.len() / 2) as i64;
    let (len, lines) = if vertical { (d.h, d.w) } else { (d.w, d.h) };
    let idx = |line: usize, i: usize| {
        if vertical {
            i * d.w + line
        } else {
            line * d.w + i
        }
    };
    let mut out = vec![0.0; src.len()];
    let n = len as i64;
    let mut buf = vec![0.0; len + 2 * r as usize];
    for line in 0..lines {
        for (j, b) in buf.iter_mut().enumerate() {
            let i = j as i64 - r;
            *b = if (0..n).contains(&i) {
                src[idx(line, i as usize)]
            } else {
                match edge {
                    Edge::Constant => 0.0,
                    Edge::Reflect => {
                        let m = i.rem_euclid(2 * n);
                        src[idx(line, if m < n { m } else { 2 * n - 1 - m } as usize)]
                    }
                }
            };
        }
        for i in 0..len {
            let c = i + r as usize;
            let mut acc = buf[c] * k[r as usize];
            for j in (1..=r as usize).rev() {
                acc += (buf[c - j] + buf[c + j]) * k[r as usize - j];
            }
            out[idx(line, i)] = acc;
        }
    }
    out
}

/// Gaussian blur of a 2D float image (rows first, then columns).
pub fn gaussian(src: &[f64], d: Dims, sigma: f64, edge: Edge) -> Vec<f64> {
    let k = gaussian_kernel(sigma);
    let rows = blur_axis(src, d, &k, true, edge);
    blur_axis(&rows, d, &k, false, edge)
}

/// Blur `v`, weighting every pixel by its alpha so transparent pixels don't
/// pull the result toward their (meaningless) color. Pixels with almost no
/// opaque neighbours keep their value. No blur when `sigma` <= 0.
pub fn weighted_blur(v: &[f64], alpha: Option<&[f64]>, d: Dims, sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 {
        return v.to_vec();
    }
    let Some(w) = alpha else {
        return gaussian(v, d, sigma, Edge::Reflect);
    };
    let weighted: Vec<f64> = v.iter().zip(w).map(|(a, b)| a * b).collect();
    let num = gaussian(&weighted, d, sigma, Edge::Constant);
    let den = gaussian(w, d, sigma, Edge::Constant);
    (0..v.len())
        .map(|i| {
            if den[i] > 1e-4 {
                num[i] / den[i].max(1e-4)
            } else {
                v[i]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_sums_to_one() {
        let k = gaussian_kernel(1.5);
        assert_eq!(k.len(), 13);
        assert!((k.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn reflect_blur_keeps_flat_images_flat() {
        let d = Dims { w: 3, h: 2 };
        let out = gaussian(&[0.5; 6], d, 2.0, Edge::Reflect);
        assert!(out.iter().all(|v| (v - 0.5).abs() < 1e-12));
    }
}
