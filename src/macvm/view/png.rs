//! The PNG frames of the guest: the size check that every protocol
//! needs, a decoder into RGBA for the sixel encoder, and a plain encoder
//! for the fake frame source and the tests.
//!
//! The guest is not trusted. Every size is checked before any buffer is
//! made: [`MAX_SIDE`] per side, [`MAX_PIXELS`] per frame, and the inflate
//! output never grows past the size that the header promises.

use super::inflate;
use crate::macvm::text;

/// The PNG signature.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// The longest side of a frame, in pixels.
pub const MAX_SIDE: u32 = 16_384;
/// The most pixels of a frame: two 6K displays.
pub const MAX_PIXELS: u64 = 40_000_000;
/// The largest encoded frame, in bytes.
pub const MAX_BYTES: usize = 64 << 20;

/// The header of a PNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub color: u8,
    pub interlace: u8,
}

/// A decoded frame: `width * height * 4` bytes, RGBA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Read and check the header of `png` without decoding the pixels.
pub fn header(png: &[u8]) -> Result<Header, String> {
    if png.len() > MAX_BYTES {
        return Err(text::view_png_too_big(png.len(), MAX_BYTES));
    }
    if png.len() < 33 || png[..8] != SIGNATURE {
        return Err("the frame is not a PNG".into());
    }
    if &png[12..16] != b"IHDR" || be32(&png[8..12]) != 13 {
        return Err("the PNG does not start with its header".into());
    }
    let d = &png[16..29];
    let h = Header {
        width: be32(&d[0..4]),
        height: be32(&d[4..8]),
        depth: d[8],
        color: d[9],
        interlace: d[12],
    };
    if d[10] != 0 || d[11] != 0 {
        return Err("the PNG uses an unknown compression or filter".into());
    }
    if h.width == 0 || h.height == 0 || h.width > MAX_SIDE || h.height > MAX_SIDE {
        return Err(text::view_png_bad_side(h.width, h.height, MAX_SIDE));
    }
    if u64::from(h.width) * u64::from(h.height) > MAX_PIXELS {
        return Err(text::view_png_too_many_pixels(
            h.width, h.height, MAX_PIXELS,
        ));
    }
    let ok = match h.color {
        0 => matches!(h.depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(h.depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(h.depth, 8 | 16),
        _ => false,
    };
    if !ok {
        return Err(text::view_png_bad_color(h.color, h.depth));
    }
    if h.interlace > 1 {
        return Err("the PNG has an unknown interlace method".into());
    }
    Ok(h)
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn channels(color: u8) -> usize {
    match color {
        0 | 3 => 1,
        4 => 2,
        2 => 3,
        _ => 4,
    }
}

/// Decode `png` into RGBA. Interlaced PNGs fail with a reason: the guest
/// agent never writes them.
pub fn decode(png: &[u8]) -> Result<Rgba, String> {
    let h = header(png)?;
    if h.interlace != 0 {
        return Err("the PNG is interlaced, which the view does not decode".into());
    }
    let mut pos = 8;
    let mut idat = Vec::new();
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut seen_end = false;
    while pos + 12 <= png.len() {
        let len = be32(&png[pos..pos + 4]) as usize;
        let kind = &png[pos + 4..pos + 8];
        let body = png
            .get(pos + 8..pos + 8 + len)
            .ok_or("a PNG chunk runs past the end")?;
        let crc = png
            .get(pos + 8 + len..pos + 12 + len)
            .ok_or("a PNG chunk has no checksum")?;
        if crc32(&png[pos + 4..pos + 8 + len]) != be32(crc) {
            return Err("a PNG chunk checksum does not match".into());
        }
        match kind {
            b"IDAT" => idat.extend_from_slice(body),
            b"PLTE" => {
                palette = body
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| [c[0], c[1], c[2], 255])
                    .collect();
            }
            b"tRNS" if h.color == 3 => {
                for (p, &a) in palette.iter_mut().zip(body) {
                    p[3] = a;
                }
            }
            b"IEND" => {
                seen_end = true;
                break;
            }
            _ => {}
        }
        pos += 12 + len;
    }
    if !seen_end {
        return Err("the PNG has no end chunk".into());
    }
    if h.color == 3 && palette.is_empty() {
        return Err("the PNG has no palette".into());
    }
    let bpp_bits = channels(h.color) * h.depth as usize;
    let stride = (h.width as usize * bpp_bits).div_ceil(8);
    let raw_len = (stride + 1) * h.height as usize;
    let raw = inflate::zlib(&idat, raw_len)?;
    if raw.len() != raw_len {
        return Err("the PNG pixel data has a wrong length".into());
    }
    let rows = unfilter(&raw, stride, h.height as usize, bpp_bits.div_ceil(8))?;
    Ok(Rgba {
        width: h.width,
        height: h.height,
        data: expand(&rows, &h, stride, &palette),
    })
}

fn unfilter(raw: &[u8], stride: usize, height: usize, bpp: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; stride * height];
    for y in 0..height {
        let f = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, cur) = out.split_at_mut(y * stride);
        let prev = if y == 0 {
            None
        } else {
            Some(&done[(y - 1) * stride..])
        };
        let cur = &mut cur[..stride];
        for x in 0..stride {
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let b = prev.map_or(0, |p| p[x]);
            let c = if x >= bpp {
                prev.map_or(0, |p| p[x - bpp])
            } else {
                0
            };
            let pred = match f {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(text::view_png_bad_filter(f)),
            };
            cur[x] = src[x].wrapping_add(pred);
        }
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn expand(rows: &[u8], h: &Header, stride: usize, palette: &[[u8; 4]]) -> Vec<u8> {
    let (w, ht) = (h.width as usize, h.height as usize);
    let mut out = Vec::with_capacity(w * ht * 4);
    let depth = h.depth as usize;
    let ch = channels(h.color);
    for y in 0..ht {
        let row = &rows[y * stride..(y + 1) * stride];
        for x in 0..w {
            // One sample of channel `k` of pixel `x`, scaled to 8 bits.
            let sample = |k: usize| -> u8 {
                let i = x * ch + k;
                match depth {
                    16 => row[i * 2],
                    8 => row[i],
                    _ => {
                        let bit = i * depth;
                        let v = (row[bit / 8] >> (8 - depth - bit % 8)) & ((1 << depth) - 1);
                        if h.color == 3 {
                            v
                        } else {
                            (u16::from(v) * 255 / ((1u16 << depth) - 1)) as u8
                        }
                    }
                }
            };
            let px = match h.color {
                0 => {
                    let g = sample(0);
                    [g, g, g, 255]
                }
                2 => [sample(0), sample(1), sample(2), 255],
                3 => palette
                    .get(sample(0) as usize)
                    .copied()
                    .unwrap_or([0, 0, 0, 255]),
                4 => {
                    let g = sample(0);
                    [g, g, g, sample(1)]
                }
                _ => [sample(0), sample(1), sample(2), sample(3)],
            };
            out.extend_from_slice(&px);
        }
    }
    out
}

/// The CRC-32 of PNG chunks (ISO 3309).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Encode RGBA pixels as a PNG with stored deflate blocks: no
/// compression, exact bytes, for the fake source and the tests.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&inflate::adler32(&raw).to_be_bytes());
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = SIGNATURE.to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}
