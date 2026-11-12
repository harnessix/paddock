//! Sixel, the fallback protocol: the frame is decoded, scaled to the
//! pixel size of its cells, put on a fixed palette of 216 colors (a
//! 6x6x6 cube, so the output is the same for the same frame), and
//! written in bands of six rows with run-length coding.

use super::png::Rgba;

/// The levels of each channel of the palette.
const LEVELS: u32 = 6;

/// Scale `img` to `w` x `h` pixels by the mean of each source box (by
/// the nearest pixel when the frame grows).
pub fn scale(img: &Rgba, w: u32, h: u32) -> Rgba {
    let (sw, sh) = (img.width as u64, img.height as u64);
    let (w, h) = (w.max(1), h.max(1));
    let mut data = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as u64 {
        let y0 = y * sh / h as u64;
        let y1 = ((y + 1) * sh / h as u64).max(y0 + 1).min(sh);
        for x in 0..w as u64 {
            let x0 = x * sw / w as u64;
            let x1 = ((x + 1) * sw / w as u64).max(x0 + 1).min(sw);
            let mut acc = [0u64; 4];
            for sy in y0..y1 {
                let row = (sy * sw) as usize * 4;
                for sx in x0..x1 {
                    let p = row + sx as usize * 4;
                    for (k, a) in acc.iter_mut().enumerate() {
                        *a += u64::from(img.data[p + k]);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            for a in acc {
                data.push((a / n) as u8);
            }
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

/// The palette index of an RGBA pixel, over black.
fn index(p: &[u8]) -> u16 {
    let a = u32::from(p[3]);
    let level = |c: u8| (u32::from(c) * a / 255 * (LEVELS - 1) + 127) / 255;
    (level(p[0]) * LEVELS * LEVELS + level(p[1]) * LEVELS + level(p[2])) as u16
}

/// The sixel image of `img`, as is: scale it first with [`scale`].
pub fn encode(img: &Rgba) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let idx: Vec<u16> = img
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| index(p))
        .collect();
    let mut used = [false; 216];
    for &i in &idx {
        used[i as usize] = true;
    }
    let mut out = format!("\x1bP0;1;0q\"1;1;{w};{h}").into_bytes();
    for (i, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        let i = i as u32;
        let pct = |l: u32| l * 100 / (LEVELS - 1);
        out.extend_from_slice(
            format!("#{i};2;{};{};{}", pct(i / 36), pct(i / 6 % 6), pct(i % 6)).as_bytes(),
        );
    }
    // One line of sixel bits per color, filled in one pass over the band.
    let mut lines = vec![0u8; 216 * w];
    for band in (0..h).step_by(6) {
        let rows = (h - band).min(6);
        let mut in_band = [false; 216];
        for r in 0..rows {
            let y = band + r;
            for (x, &i) in idx[y * w..(y + 1) * w].iter().enumerate() {
                in_band[i as usize] = true;
                lines[i as usize * w + x] |= 1 << r;
            }
        }
        let mut first = true;
        for color in (0..216).filter(|&c| in_band[c]) {
            let line = &mut lines[color * w..(color + 1) * w];
            for cell in line.iter_mut() {
                *cell += 0x3f;
            }
            if !first {
                out.push(b'$');
            }
            first = false;
            out.extend_from_slice(format!("#{color}").as_bytes());
            run_length(line, &mut out);
            line.fill(0);
        }
        out.push(b'-');
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

fn run_length(line: &[u8], out: &mut Vec<u8>) {
    // A trailing run of empty sixels is left out.
    let end = line.iter().rposition(|&c| c != 0x3f).map_or(0, |p| p + 1);
    let mut i = 0;
    while i < end {
        let c = line[i];
        let mut n = 1;
        while i + n < end && line[i + n] == c {
            n += 1;
        }
        if n > 3 {
            out.extend_from_slice(format!("!{n}").as_bytes());
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n(c, n));
        }
        i += n;
    }
}
