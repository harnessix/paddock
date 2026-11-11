//! The front of a proxy connection: the header line of the macOS guest
//! route when the first byte is `{` (the Seatbelt and Linux routes send a
//! request line), then the HTTP head. A header line longer than
//! `LINE_MAX` closes the connection, a header with another token too
//! (checklist item 15).

use super::{read_head, HostProxy};
use crate::macvm::proto::{Identity, ProxyHeader, LINE_MAX};
use anyhow::{anyhow, bail, Result};
use std::io::{BufRead, BufReader, Read};

/// What the front of a connection said.
pub(crate) struct Front {
    /// The guest process, from the header line.
    pub identity: Option<Identity>,
    /// The target named in the header line.
    pub target: Option<(String, u16)>,
    pub head: Vec<u8>,
    /// Bytes that arrived after the head.
    pub rest: Vec<u8>,
}

/// Read the header line, when there is one, and the HTTP head.
pub(crate) fn read_front(cfg: &HostProxy, client: &mut impl Read) -> Result<Front> {
    let mut reader = BufReader::with_capacity(8192, client);
    let first = match reader.fill_buf()?.first() {
        Some(b) => *b,
        None => bail!("the connection closed before the request"),
    };
    let (identity, target) = if first == b'{' {
        let mut line = Vec::new();
        let mut limited = (&mut reader).take(LINE_MAX as u64 + 1);
        let n = limited.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            if line.len() > LINE_MAX {
                bail!("the header line is longer than {LINE_MAX} bytes");
            }
            bail!("the connection closed in the header line");
        }
        line.pop();
        let text = String::from_utf8(line)?;
        let h = ProxyHeader::parse(&text).map_err(|e| anyhow!(e))?;
        if let Some(t) = &cfg.token {
            if h.token != *t {
                bail!("the header line has another token than the session");
            }
        }
        (Some(h.id), Some((h.host, h.port)))
    } else {
        (None, None)
    };
    let (head, mut rest) = read_head(&mut reader)?;
    rest.extend_from_slice(reader.buffer());
    Ok(Front {
        identity,
        target,
        head,
        rest,
    })
}

#[cfg(test)]
#[path = "front_tests.rs"]
mod tests;
