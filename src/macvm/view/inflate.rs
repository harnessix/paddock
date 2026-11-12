//! A small zlib (RFC 1950) and deflate (RFC 1951) decoder for the PNG
//! frames of the guest. The output has a hard limit: a frame that
//! inflates past it fails before the allocation grows, so a guest cannot
//! make the host allocate without bound.

/// Inflate a zlib stream into at most `limit` bytes.
pub fn zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 6 {
        return Err("the zlib stream is too short".into());
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0f != 8 || cmf >> 4 > 7 {
        return Err("the zlib stream does not use deflate".into());
    }
    if (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err("the zlib header check fails".into());
    }
    if flg & 0x20 != 0 {
        return Err("the zlib stream needs a preset dictionary".into());
    }
    let mut r = Bits::new(&data[2..]);
    let out = inflate(&mut r, limit)?;
    let end = 2 + r.byte_end();
    let tail = data
        .get(end..end + 4)
        .ok_or("the zlib stream has no checksum")?;
    let want = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]);
    if adler32(&out) != want {
        return Err("the zlib checksum does not match".into());
    }
    Ok(out)
}

pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    nbits: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits {
            data,
            pos: 0,
            bit: 0,
            nbits: 0,
        }
    }

    fn need(&mut self, n: u32) -> Result<u32, String> {
        while self.nbits < n {
            let b = *self
                .data
                .get(self.pos)
                .ok_or("the deflate stream ends early")?;
            self.pos += 1;
            self.bit |= u32::from(b) << self.nbits;
            self.nbits += 8;
        }
        let v = self.bit & ((1u32 << n) - 1);
        self.bit >>= n;
        self.nbits -= n;
        Ok(v)
    }

    fn align(&mut self) {
        self.bit = 0;
        self.nbits = 0;
    }

    /// The first byte after the stream: whole unread bytes in the buffer
    /// go back.
    fn byte_end(&self) -> usize {
        self.pos - (self.nbits / 8) as usize
    }
}

const MAXBITS: usize = 15;

struct Huffman {
    count: [u16; MAXBITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, String> {
        let mut count = [0u16; MAXBITS + 1];
        for &l in lengths {
            count[l as usize] += 1;
        }
        let mut left: i32 = 1;
        for &c in &count[1..] {
            left <<= 1;
            left -= i32::from(c);
            if left < 0 {
                return Err("a deflate code is over-subscribed".into());
            }
        }
        let mut offs = [0u16; MAXBITS + 1];
        for len in 1..MAXBITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        count[0] = 0;
        Ok(Huffman { count, symbol })
    }

    fn decode(&self, r: &mut Bits) -> Result<u16, String> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAXBITS {
            code |= r.need(1)? as i32;
            let count = i32::from(self.count[len]);
            if code - first < count {
                return Ok(self.symbol[(index + code - first) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("a deflate code is invalid".into())
    }
}

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn inflate(r: &mut Bits, limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let last = r.need(1)?;
        match r.need(2)? {
            0 => stored(r, &mut out, limit)?,
            1 => {
                let (lit, dist) = fixed()?;
                codes(r, &mut out, limit, &lit, &dist)?
            }
            2 => {
                let (lit, dist) = dynamic(r)?;
                codes(r, &mut out, limit, &lit, &dist)?
            }
            _ => return Err("a deflate block has type 3".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

fn too_big() -> String {
    "the frame inflates past its size limit".into()
}

fn stored(r: &mut Bits, out: &mut Vec<u8>, limit: usize) -> Result<(), String> {
    // The buffered bits are less than a byte after the header, so the
    // next whole byte is at `pos`.
    r.align();
    let at = r.pos;
    let hdr = r.data.get(at..at + 4).ok_or("a stored block ends early")?;
    let len = u16::from_le_bytes([hdr[0], hdr[1]]);
    let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
    if len != !nlen {
        return Err("a stored block has a wrong length".into());
    }
    let len = len as usize;
    let body = r
        .data
        .get(at + 4..at + 4 + len)
        .ok_or("a stored block ends early")?;
    if out.len() + len > limit {
        return Err(too_big());
    }
    out.extend_from_slice(body);
    r.pos = at + 4 + len;
    Ok(())
}

fn fixed() -> Result<(Huffman, Huffman), String> {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    Ok((Huffman::new(&l)?, Huffman::new(&[5u8; 30])?))
}

fn dynamic(r: &mut Bits) -> Result<(Huffman, Huffman), String> {
    let nlen = r.need(5)? as usize + 257;
    let ndist = r.need(5)? as usize + 1;
    let ncode = r.need(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err("a deflate block has too many codes".into());
    }
    let mut lengths = [0u8; 320];
    for &o in &ORDER[..ncode] {
        lengths[o] = r.need(3)? as u8;
    }
    let lencode = Huffman::new(&lengths[..19])?;
    let mut lengths = [0u8; 320];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = lencode.decode(r)?;
        if sym < 16 {
            lengths[i] = sym as u8;
            i += 1;
            continue;
        }
        let (val, rep) = match sym {
            16 => {
                if i == 0 {
                    return Err("a deflate repeat has no previous length".into());
                }
                (lengths[i - 1], 3 + r.need(2)?)
            }
            17 => (0, 3 + r.need(3)?),
            _ => (0, 11 + r.need(7)?),
        };
        let rep = rep as usize;
        if i + rep > nlen + ndist {
            return Err("a deflate repeat runs past the lengths".into());
        }
        lengths[i..i + rep].fill(val);
        i += rep;
    }
    if lengths[256] == 0 {
        return Err("a deflate block has no end code".into());
    }
    let lit = Huffman::new(&lengths[..nlen])?;
    let dist = Huffman::new(&lengths[nlen..nlen + ndist])?;
    Ok((lit, dist))
}

fn codes(
    r: &mut Bits,
    out: &mut Vec<u8>,
    limit: usize,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), String> {
    loop {
        let sym = lit.decode(r)?;
        if sym < 256 {
            if out.len() >= limit {
                return Err(too_big());
            }
            out.push(sym as u8);
            continue;
        }
        if sym == 256 {
            return Ok(());
        }
        let s = (sym - 257) as usize;
        if s >= 29 {
            return Err("a deflate length code is invalid".into());
        }
        let len = LBASE[s] as usize + r.need(u32::from(LEXT[s]))? as usize;
        let d = dist.decode(r)? as usize;
        if d >= 30 {
            return Err("a deflate distance code is invalid".into());
        }
        let back = DBASE[d] as usize + r.need(u32::from(DEXT[d]))? as usize;
        if back > out.len() {
            return Err("a deflate distance reaches before the start".into());
        }
        if out.len() + len > limit {
            return Err(too_big());
        }
        let start = out.len() - back;
        for k in 0..len {
            let b = out[start + k];
            out.push(b);
        }
    }
}
