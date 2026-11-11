//! SQLite rows as bytes: the node body of an agy row.
//!
//! A row is its columns in table order, each with its name and its typed
//! value. The encoding keeps every value exactly: an integer as 8 bytes,
//! a real by its bits, text and blobs as their bytes. The same row always
//! has the same bytes, so equal rows are equal nodes.
//!
//! ```text
//! row   := count { name value }
//! name  := len bytes
//! value := 0 | 1 i64-be | 2 f64-bits-be | 3 len bytes | 4 len bytes
//! ```
//!
//! Counts and lengths are LEB128 varints. A table set (the header of a
//! conversation) is `count { name count { len row } }`.

use super::proto::{put_varint, read_varint};
use anyhow::{bail, Context as _, Result};
use rusqlite::types::{Value, ValueRef};

/// One column value. Text keeps its bytes as SQLite stores them.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    Real(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl Cell {
    pub fn from_ref(v: ValueRef<'_>) -> Cell {
        match v {
            ValueRef::Null => Cell::Null,
            ValueRef::Integer(i) => Cell::Int(i),
            ValueRef::Real(f) => Cell::Real(f),
            ValueRef::Text(t) => Cell::Text(t.to_vec()),
            ValueRef::Blob(b) => Cell::Blob(b.to_vec()),
        }
    }

    /// The value to bind. Text that is not UTF-8 is refused: paddock does
    /// not guess another encoding.
    pub fn to_value(&self) -> Result<Value> {
        Ok(match self {
            Cell::Null => Value::Null,
            Cell::Int(i) => Value::Integer(*i),
            Cell::Real(f) => Value::Real(*f),
            Cell::Text(t) => {
                Value::Text(String::from_utf8(t.clone()).context("an agy text value is not UTF-8")?)
            }
            Cell::Blob(b) => Value::Blob(b.clone()),
        })
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Cell::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Cell::Text(t) => std::str::from_utf8(t).ok(),
            _ => None,
        }
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Cell::Blob(b) | Cell::Text(b) => Some(b),
            _ => None,
        }
    }
}

/// A row: column names and values in table order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row(pub Vec<(String, Cell)>);

impl Row {
    pub fn get(&self, col: &str) -> Option<&Cell> {
        self.0.iter().find(|(n, _)| n == col).map(|(_, v)| v)
    }

    pub fn set(&mut self, col: &str, v: Cell) {
        match self.0.iter_mut().find(|(n, _)| n == col) {
            Some(c) => c.1 = v,
            None => self.0.push((col.into(), v)),
        }
    }

    /// The integer `idx` of an idx-keyed row.
    pub fn idx(&self) -> Result<i64> {
        self.get("idx")
            .and_then(Cell::int)
            .context("an agy row has no integer idx")
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, self.0.len() as u64);
        for (name, v) in &self.0 {
            put_bytes(&mut out, name.as_bytes());
            match v {
                Cell::Null => out.push(0),
                Cell::Int(i) => {
                    out.push(1);
                    out.extend_from_slice(&i.to_be_bytes());
                }
                Cell::Real(f) => {
                    out.push(2);
                    out.extend_from_slice(&f.to_bits().to_be_bytes());
                }
                Cell::Text(t) => {
                    out.push(3);
                    put_bytes(&mut out, t);
                }
                Cell::Blob(b) => {
                    out.push(4);
                    put_bytes(&mut out, b);
                }
            }
        }
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Row> {
        let mut r = Reader { buf: raw, pos: 0 };
        let row = r.row()?;
        if r.pos != raw.len() {
            bail!("an agy row has bytes after its end");
        }
        Ok(row)
    }
}

/// Named tables of rows, in order.
pub type Tables = Vec<(String, Vec<Row>)>;

pub fn encode_tables(t: &Tables) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, t.len() as u64);
    for (name, rows) in t {
        put_bytes(&mut out, name.as_bytes());
        put_varint(&mut out, rows.len() as u64);
        for row in rows {
            put_bytes(&mut out, &row.encode());
        }
    }
    out
}

pub fn decode_tables(raw: &[u8]) -> Result<Tables> {
    let mut r = Reader { buf: raw, pos: 0 };
    let n = r.varint()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let name = r.name()?;
        let rows = r.varint()?;
        let mut v = Vec::new();
        for _ in 0..rows {
            v.push(Row::decode(r.bytes()?)?);
        }
        out.push((name, v));
    }
    if r.pos != raw.len() {
        bail!("an agy table set has bytes after its end");
    }
    Ok(out)
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Result<u64> {
        let (v, n) = read_varint(&self.buf[self.pos..]).context("a bad agy row")?;
        self.pos += n;
        Ok(v)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .context("an agy row ends early")?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.varint()?;
        self.take(usize::try_from(n)?)
    }

    fn name(&mut self) -> Result<String> {
        Ok(String::from_utf8(self.bytes()?.to_vec())?)
    }

    fn eight(&mut self) -> Result<[u8; 8]> {
        Ok(self.take(8)?.try_into()?)
    }

    fn row(&mut self) -> Result<Row> {
        let n = self.varint()?;
        let mut cols = Vec::new();
        for _ in 0..n {
            let name = self.name()?;
            let tag = self.take(1)?[0];
            let v = match tag {
                0 => Cell::Null,
                1 => Cell::Int(i64::from_be_bytes(self.eight()?)),
                2 => Cell::Real(f64::from_bits(u64::from_be_bytes(self.eight()?))),
                3 => Cell::Text(self.bytes()?.to_vec()),
                4 => Cell::Blob(self.bytes()?.to_vec()),
                t => bail!("an agy row has an unknown value type {t}"),
            };
            cols.push((name, v));
        }
        Ok(Row(cols))
    }
}
