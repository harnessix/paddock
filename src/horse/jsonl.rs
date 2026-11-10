//! The complete lines of a JSONL session file.
//!
//! A line without its newline at the end is a write in progress and waits.
//! A crash can also leave a cut line that the next write of the harness
//! ends with a newline before it appends; Claude Code, Codex and Grok
//! Build skip such a line when they read the file. So a complete line that
//! is not JSON at all is left out, here too. A line that is JSON but not a
//! row of the format is still an error of the codec.

/// The complete lines of `bytes` without their newline, and whether an
/// incomplete last line was left out.
pub(crate) fn lines(bytes: &[u8]) -> (Vec<Vec<u8>>, bool) {
    let mut rows: Vec<Vec<u8>> = bytes.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    let torn = rows.pop().is_some_and(|l| !l.is_empty());
    rows.retain(|r| r.is_empty() || is_json(r));
    (rows, torn)
}

fn is_json(raw: &[u8]) -> bool {
    serde_json::from_slice::<serde::de::IgnoredAny>(raw).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_line_that_a_newline_ended_is_left_out() {
        let (rows, torn) = lines(b"{\"a\":1}\n{\"b\":\n\n{\"c\":3}\n{\"d\"");
        assert_eq!(
            rows,
            [b"{\"a\":1}".to_vec(), Vec::new(), b"{\"c\":3}".to_vec()]
        );
        assert!(torn);
        let (rows, torn) = lines(b"[1]\n");
        assert_eq!((rows, torn), (vec![b"[1]".to_vec()], false));
    }
}
