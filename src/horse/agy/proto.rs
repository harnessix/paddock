//! The few protobuf fields that paddock reads in agy blobs.
//!
//! No generated code: a varint reader that walks the top-level fields of
//! a message. A step payload stays raw bytes. paddock reads field 1 (the
//! step type) and field 4 (the status) of a step, and field 6 (the
//! conversation id) of the trajectory metadata, which a fork rewrites.

use anyhow::{bail, Context as _, Result};

/// A LEB128 varint at the start of `b`: the value and its length.
pub fn read_varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v: u64 = 0;
    for (i, byte) in b.iter().enumerate().take(10) {
        v |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// One top-level field: its number, its wire type, its value (the varint
/// or the bytes of a length-delimited field) and its whole span.
#[derive(Debug, Clone, PartialEq)]
pub struct Field<'a> {
    pub number: u64,
    pub wire: u8,
    pub varint: u64,
    pub data: &'a [u8],
    pub span: &'a [u8],
}

/// The top-level fields of a message, in order. A malformed message is an
/// error.
pub fn fields(msg: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < msg.len() {
        let start = pos;
        let (key, n) = read_varint(&msg[pos..]).context("a bad protobuf key")?;
        pos += n;
        let number = key >> 3;
        let wire = (key & 7) as u8;
        if number == 0 {
            bail!("a protobuf field has number 0");
        }
        let (varint, data) = match wire {
            0 => {
                let (v, n) = read_varint(&msg[pos..]).context("a bad protobuf varint")?;
                pos += n;
                (v, &msg[pos - n..pos])
            }
            1 | 5 => {
                let n = if wire == 1 { 8 } else { 4 };
                let end = pos + n;
                if end > msg.len() {
                    bail!("a protobuf field ends early");
                }
                pos = end;
                (0, &msg[end - n..end])
            }
            2 => {
                let (len, n) = read_varint(&msg[pos..]).context("a bad protobuf length")?;
                pos += n;
                let end = pos
                    .checked_add(usize::try_from(len)?)
                    .filter(|e| *e <= msg.len())
                    .context("a protobuf field ends early")?;
                let d = &msg[pos..end];
                pos = end;
                (len, d)
            }
            w => bail!("protobuf wire type {w} is not supported"),
        };
        out.push(Field {
            number,
            wire,
            varint,
            data,
            span: &msg[start..pos],
        });
    }
    Ok(out)
}

/// The last varint field `number`, as protobuf reads a repeated scalar.
pub fn varint(msg: &[u8], number: u64) -> Result<Option<u64>> {
    Ok(fields(msg)?
        .iter()
        .rev()
        .find(|f| f.number == number && f.wire == 0)
        .map(|f| f.varint))
}

/// The last length-delimited field `number`.
pub fn bytes(msg: &[u8], number: u64) -> Result<Option<&[u8]>> {
    Ok(fields(msg)?
        .into_iter()
        .rev()
        .find(|f| f.number == number && f.wire == 2)
        .map(|f| f.data))
}

/// The message with every length-delimited field `number` set to `value`.
/// Every other field keeps its bytes and its place.
pub fn replace_bytes(msg: &[u8], number: u64, value: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(msg.len() + value.len());
    for f in fields(msg)? {
        if f.number == number && f.wire == 2 {
            put_varint(&mut out, number << 3 | 2);
            put_varint(&mut out, value.len() as u64);
            out.extend_from_slice(value);
        } else {
            out.extend_from_slice(f.span);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_read_and_a_rewrite_keeps_the_other_bytes() {
        let mut m = Vec::new();
        put_varint(&mut m, 1 << 3);
        put_varint(&mut m, 300);
        put_varint(&mut m, 6 << 3 | 2);
        put_varint(&mut m, 3);
        m.extend_from_slice(b"old");
        put_varint(&mut m, 18 << 3 | 2);
        put_varint(&mut m, 1);
        m.push(b'p');
        assert_eq!(varint(&m, 1).unwrap(), Some(300));
        assert_eq!(bytes(&m, 6).unwrap(), Some(&b"old"[..]));
        let n = replace_bytes(&m, 6, b"newer").unwrap();
        assert_eq!(bytes(&n, 6).unwrap(), Some(&b"newer"[..]));
        assert_eq!(bytes(&n, 18).unwrap(), Some(&b"p"[..]));
        assert_eq!(&n[..3], &m[..3]);
        assert!(fields(&[0x0a, 0x05, 0x01]).is_err());
    }
}
