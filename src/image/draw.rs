//! Small drawing helpers for previews and contact sheets.
use ::image::{Rgb, RgbImage, RgbaImage, imageops};
use font8x8::{BASIC_FONTS, UnicodeFonts};

use crate::image::pixels;
use crate::image::png::Png;

/// Background color of panels, captions and sheets.
pub const PANEL_BG: Rgb<u8> = Rgb([32, 30, 40]);

/// Any decoded PNG as an RGBA image.
pub fn to_rgba_image(im: &Png) -> RgbaImage {
    let px = pixels::to_rgba_display(im);
    RgbaImage::from_raw(im.width as u32, im.height as u32, px.concat())
        .expect("pixel count matches size")
}

/// Light/gray checkerboard with `s`-pixel squares, shown behind transparency.
pub fn checker(w: u32, h: u32, s: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| {
        if (x / s + y / s) % 2 == 1 {
            Rgb([204; 3])
        } else {
            Rgb([244; 3])
        }
    })
}

/// `top` composited over an opaque background, at (x, y).
pub fn composite(base: &mut RgbImage, top: &RgbaImage, x: i64, y: i64) {
    for (tx, ty, p) in top.enumerate_pixels() {
        let (bx, by) = (x + tx as i64, y + ty as i64);
        if bx < 0 || by < 0 || bx >= base.width() as i64 || by >= base.height() as i64 {
            continue;
        }
        let a = p[3] as u32;
        let dst = base.get_pixel_mut(bx as u32, by as u32);
        for k in 0..3 {
            dst[k] = ((p[k] as u32 * a + dst[k] as u32 * (255 - a) + 127) / 255) as u8;
        }
    }
}

/// Region of an image; parts outside it come out transparent.
pub fn crop(im: &RgbaImage, x: i64, y: i64, w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |cx, cy| {
        let (sx, sy) = (x + cx as i64, y + cy as i64);
        if sx < 0 || sy < 0 || sx >= im.width() as i64 || sy >= im.height() as i64 {
            ::image::Rgba([0; 4])
        } else {
            *im.get_pixel(sx as u32, sy as u32)
        }
    })
}

/// The image repeated n x n times.
pub fn tile(im: &RgbaImage, n: u32) -> RgbaImage {
    let mut out = RgbaImage::new(im.width() * n, im.height() * n);
    for i in 0..n {
        for j in 0..n {
            imageops::replace(
                &mut out,
                im,
                (i * im.width()) as i64,
                (j * im.height()) as i64,
            );
        }
    }
    out
}

/// Resize: nearest neighbour when enlarging (keeps pixels crisp), Lanczos when shrinking.
pub fn resize(im: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let filter = if w >= im.width() {
        imageops::FilterType::Nearest
    } else {
        imageops::FilterType::Lanczos3
    };
    imageops::resize(im, w, h, filter)
}

/// Text in an 8x8 bitmap font, scaled to roughly `size` pixels tall, or
/// smaller when the line wouldn't fit.
pub fn text(img: &mut RgbImage, x: u32, y: u32, s: &str, color: Rgb<u8>, size: u32) {
    let mut scale = (size / 8).max(1);
    if x + s.chars().count() as u32 * 8 * scale > img.width() {
        scale = 1;
    }
    for (i, ch) in s.chars().enumerate() {
        let Some(glyph) = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?')) else {
            continue;
        };
        let gx = x + i as u32 * 8 * scale;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits >> col & 1 == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (gx + col * scale + dx, y + row as u32 * scale + dy);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, color);
                        }
                    }
                }
            }
        }
    }
}
