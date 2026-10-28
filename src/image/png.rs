//! PNG reading and writing that preserves the file's color mode and metadata.
//!
//! Factorio's PNGs come in several modes (RGBA, RGB, palette with tRNS alpha,
//! grayscale) and many carry a gAMA chunk. The recolored file must keep all of
//! that, so the game decodes it exactly as it did the original.
//!
//! Modes are named "RGBA", "RGB", "LA", "L", "P" (palette) and "1" (1-bit).
use std::fmt;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::{Compression, write::ZlibEncoder};

/// Color mode of a PNG, named as Pillow names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 8-bit color with alpha.
    Rgba,
    /// 8-bit color.
    Rgb,
    /// 8-bit grayscale with alpha.
    La,
    /// Grayscale, 2 to 8 bits (lower depths are scaled up to 0..255).
    L,
    /// Palette indices, 1 to 8 bits, with the palette alpha in tRNS.
    P,
    /// 1-bit grayscale. Left alone by every transform.
    One,
}

impl Mode {
    /// Modes the transforms know how to handle. Anything else is passed through.
    pub fn supported(self) -> bool {
        self != Mode::One
    }

    /// Samples per pixel in [`Png::data`].
    pub fn channels(self) -> usize {
        match self {
            Mode::Rgba => 4,
            Mode::Rgb => 3,
            Mode::La => 2,
            Mode::L | Mode::P | Mode::One => 1,
        }
    }

    /// The PNG color type for the IHDR chunk.
    fn color_type(self) -> u8 {
        match self {
            Mode::Rgba => 6,
            Mode::Rgb => 2,
            Mode::La => 4,
            Mode::L | Mode::One => 0,
            Mode::P => 3,
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Mode::Rgba => "RGBA",
            Mode::Rgb => "RGB",
            Mode::La => "LA",
            Mode::L => "L",
            Mode::P => "P",
            Mode::One => "1",
        })
    }
}

/// A decoded PNG: one byte per sample (palette indices and low bit depths are
/// unpacked), plus the chunks that must survive recoloring.
#[derive(Clone, Debug)]
pub struct Png {
    /// Color mode, as stored.
    pub mode: Mode,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Bit depth as stored; kept for palette and 1-bit images.
    bit_depth: u8,
    /// Samples, row-major, [`Mode::channels`] bytes per pixel.
    pub data: Vec<u8>,
    /// Palette entries (palette images only).
    pub palette: Vec<[u8; 3]>,
    /// Raw tRNS chunk: palette alpha table, or a transparent color key.
    pub trns: Option<Vec<u8>>,
    /// gAMA, sRGB, iCCP and pHYs chunks, raw, written back unchanged.
    meta: Vec<([u8; 4], Vec<u8>)>,
}

/// The metadata that must survive recoloring, in a comparable form.
#[derive(Debug, PartialEq)]
pub struct Signature {
    /// Color mode.
    pub mode: Mode,
    /// (width, height) in pixels.
    pub size: (usize, usize),
    /// Gamma from the gAMA chunk.
    pub gamma: Option<f64>,
    /// Rendering intent from the sRGB chunk.
    pub srgb: Option<u8>,
    /// Whether there is an embedded ICC profile.
    pub icc: bool,
}

impl Signature {
    /// (name, before, after) for every field that differs.
    pub fn diff(&self, other: &Signature) -> Vec<String> {
        let opt = |v: Option<String>| v.unwrap_or_else(|| "None".into());
        let fields = [
            ("mode", self.mode.to_string(), other.mode.to_string()),
            (
                "size",
                format!("{:?}", self.size),
                format!("{:?}", other.size),
            ),
            (
                "gamma",
                opt(self.gamma.map(|g| g.to_string())),
                opt(other.gamma.map(|g| g.to_string())),
            ),
            (
                "srgb",
                opt(self.srgb.map(|g| g.to_string())),
                opt(other.srgb.map(|g| g.to_string())),
            ),
            ("icc", self.icc.to_string(), other.icc.to_string()),
        ];
        fields
            .into_iter()
            .filter(|(_, a, b)| a != b)
            .map(|(k, a, b)| format!("{k} changed: {a} -> {b}"))
            .collect()
    }
}

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const KEPT_CHUNKS: [&[u8; 4]; 4] = [b"gAMA", b"sRGB", b"iCCP", b"pHYs"];

impl Png {
    /// Read and decode a PNG file.
    pub fn open(path: &Path) -> Result<Png> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Png::decode(&bytes).with_context(|| format!("decoding {}", path.display()))
    }

    /// Decode a PNG file's bytes. 16-bit images are not supported.
    pub fn decode(bytes: &[u8]) -> Result<Png> {
        if !bytes.starts_with(SIGNATURE) {
            bail!("not a PNG file");
        }
        let mut meta = Vec::new();
        let mut i = 8;
        while i + 8 <= bytes.len() {
            let n = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = bytes[i + 4..i + 8].try_into().unwrap();
            let Some(body) = bytes.get(i + 8..i + 8 + n) else {
                break;
            };
            // pHYs only when its unit is meters; other values carry no real size.
            if KEPT_CHUNKS.contains(&&kind) && !(&kind == b"pHYs" && body.get(8) != Some(&1)) {
                meta.push((kind, body.to_vec()));
            }
            if &kind == b"IDAT" || &kind == b"IEND" {
                break;
            }
            i += 12 + n;
        }

        let mut dec = ::png::Decoder::new(Cursor::new(bytes));
        dec.set_transformations(::png::Transformations::IDENTITY);
        let mut reader = dec.read_info()?;
        let info = reader.info();
        let (width, height) = (info.width as usize, info.height as usize);
        let bit_depth = info.bit_depth as u8;
        let mode = match (info.color_type, bit_depth) {
            (_, 16) => bail!("16-bit PNGs are not supported"),
            (::png::ColorType::Rgba, _) => Mode::Rgba,
            (::png::ColorType::Rgb, _) => Mode::Rgb,
            (::png::ColorType::GrayscaleAlpha, _) => Mode::La,
            (::png::ColorType::Grayscale, 1) => Mode::One,
            (::png::ColorType::Grayscale, _) => Mode::L,
            (::png::ColorType::Indexed, _) => Mode::P,
        };
        let palette = info
            .palette
            .as_deref()
            .unwrap_or(&[])
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        let trns = info.trns.as_deref().map(<[u8]>::to_vec);

        let mut buf = vec![0; reader.output_buffer_size().context("image too large")?];
        let frame = reader.next_frame(&mut buf)?;
        let samples = width * mode.channels();
        let mut data = Vec::with_capacity(samples * height);
        for row in buf.chunks(frame.line_size).take(height) {
            if bit_depth == 8 {
                data.extend_from_slice(&row[..samples]);
                continue;
            }
            let per_byte = 8 / bit_depth as usize;
            let mask = (1u8 << bit_depth) - 1;
            for x in 0..samples {
                let shift = 8 - bit_depth as usize * (x % per_byte + 1);
                let v = (row[x / per_byte] >> shift) & mask;
                data.push(match mode {
                    // Low-bit grayscale is scaled up to 0..255, as Pillow does.
                    Mode::L => v * (255 / mask),
                    _ => v,
                });
            }
        }
        Ok(Png {
            mode,
            width,
            height,
            bit_depth,
            data,
            palette,
            trns,
            meta,
        })
    }

    /// (width, height) in pixels.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Body of a kept metadata chunk.
    fn chunk(&self, kind: &[u8; 4]) -> Option<&[u8]> {
        self.meta
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, v)| v.as_slice())
    }

    /// Gamma from the gAMA chunk, e.g. 0.45455.
    pub fn gamma(&self) -> Option<f64> {
        let g = self.chunk(b"gAMA")?;
        Some(u32::from_be_bytes(g.get(..4)?.try_into().ok()?) as f64 / 100000.0)
    }

    /// The metadata that must survive recoloring.
    pub fn signature(&self) -> Signature {
        Signature {
            mode: self.mode,
            size: self.size(),
            gamma: self.gamma(),
            srgb: self.chunk(b"sRGB").and_then(|s| s.first().copied()),
            icc: self.chunk(b"iCCP").is_some(),
        }
    }

    /// The same image metadata with new pixel data (same mode and size).
    pub(crate) fn with_data(&self, data: Vec<u8>) -> Png {
        Png {
            data,
            ..self.clone()
        }
    }

    // -- encoding --------------------------------------------------------

    /// Encode with the original's mode, bit depth and metadata chunks.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let bit_depth = match self.mode {
            Mode::P | Mode::One => self.bit_depth,
            _ => 8,
        };
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend((self.width as u32).to_be_bytes());
        ihdr.extend((self.height as u32).to_be_bytes());
        ihdr.extend([bit_depth, self.mode.color_type(), 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        for (kind, body) in &self.meta {
            write_chunk(&mut out, kind, body);
        }
        if self.mode == Mode::P {
            write_chunk(&mut out, b"PLTE", &self.palette.concat());
        }
        if let Some(t) = &self.trns {
            write_chunk(&mut out, b"tRNS", t);
        }
        let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
        z.write_all(&self.filtered_rows(bit_depth))?;
        write_chunk(&mut out, b"IDAT", &z.finish()?);
        write_chunk(&mut out, b"IEND", &[]);
        Ok(out)
    }

    /// Encode and write to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, self.encode()?).with_context(|| format!("writing {}", path.display()))
    }

    /// Packed scanlines, each with a filter byte. Palette and low bit depth
    /// images use no filter; others pick the filter with the smallest sum of
    /// absolute differences per row (the usual libpng heuristic).
    fn filtered_rows(&self, bit_depth: u8) -> Vec<u8> {
        let ch = self.mode.channels();
        let rows: Vec<Vec<u8>> = self
            .data
            .chunks(self.width * ch)
            .map(|row| {
                if bit_depth == 8 {
                    return row.to_vec();
                }
                let per_byte = 8 / bit_depth as usize;
                let mut packed = vec![0u8; row.len().div_ceil(per_byte)];
                for (x, &v) in row.iter().enumerate() {
                    let v = if self.mode == Mode::One {
                        (v > 0) as u8
                    } else {
                        v
                    };
                    packed[x / per_byte] |= v << (8 - bit_depth as usize * (x % per_byte + 1));
                }
                packed
            })
            .collect();
        let adaptive = bit_depth == 8 && self.mode != Mode::P;
        let mut out = Vec::with_capacity(rows.iter().map(|r| r.len() + 1).sum());
        let empty = vec![0u8; rows.first().map_or(0, Vec::len)];
        for (y, row) in rows.iter().enumerate() {
            let prev = if y == 0 { &empty } else { &rows[y - 1] };
            if !adaptive {
                out.push(0);
                out.extend_from_slice(row);
                continue;
            }
            let best = (0..5u8)
                .map(|f| filter_row(f, row, prev, ch))
                .min_by_key(|r| {
                    r[1..]
                        .iter()
                        .map(|&b| (b as i8).unsigned_abs() as u64)
                        .sum::<u64>()
                })
                .unwrap();
            out.extend(best);
        }
        out
    }
}

/// One scanline filtered with PNG filter `kind` (0 none, 1 sub, 2 up,
/// 3 average, 4 Paeth), filter byte first.
fn filter_row(kind: u8, row: &[u8], prev: &[u8], bpp: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.len() + 1);
    out.push(kind);
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let pred = match kind {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            _ => paeth(a, b, c),
        };
        out.push(row[i].wrapping_sub(pred));
    }
    out
}

/// The Paeth predictor from the PNG specification.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = (
        (p - a as i16).abs(),
        (p - b as i16).abs(),
        (p - c as i16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Append a chunk: length, type, body and CRC.
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend((body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(body);
    let crc = crc32fast::hash(&out[start..]);
    out.extend(crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(im: &Png) -> Png {
        Png::decode(&im.encode().unwrap()).unwrap()
    }

    #[test]
    fn encode_decode_keeps_everything() {
        let base = Png {
            mode: Mode::Rgba,
            width: 3,
            height: 2,
            bit_depth: 8,
            data: (0..24).map(|v| v * 10).collect(),
            palette: vec![],
            trns: None,
            meta: vec![
                (*b"gAMA", 45455u32.to_be_bytes().to_vec()),
                (*b"sRGB", vec![0]),
            ],
        };
        let back = roundtrip(&base);
        assert_eq!(back.data, base.data);
        assert_eq!(back.gamma(), Some(0.45455));
        assert!(back.signature() == base.signature());

        let p = Png {
            mode: Mode::P,
            bit_depth: 2,
            width: 5,
            height: 3,
            data: (0..15).map(|v| (v % 4) as u8).collect(),
            palette: vec![[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]],
            trns: Some(vec![0, 128]),
            meta: vec![],
        };
        let back = roundtrip(&p);
        assert_eq!(back.data, p.data);
        assert_eq!(back.palette, p.palette);
        assert_eq!(back.trns, p.trns);
        assert_eq!(back.bit_depth, 2);
    }
}
