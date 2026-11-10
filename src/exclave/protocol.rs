//! Bounded newline-delimited messages on a peer's stdio stream.
//!
//! File and output bytes use base64; neither transport configuration nor host
//! environment are part of this protocol.

use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{BufRead, Read, Write};

/// Large enough for a 1 MiB encoded blob, but bounded before JSON parsing.
pub const MAX_FRAME: u64 = 2 * 1024 * 1024;
/// Maximum decoded payload in a blob chunk.
pub const CHUNK_SIZE: usize = 256 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
        project: String,
    },
    HasBlobs {
        hashes: Vec<String>,
    },
    Blob {
        hash: String,
        data: String,
    },
    Manifest {
        manifest: super::snapshot::Manifest,
    },
    Reserve,
    BlobChunk {
        hash: String,
        offset: u64,
        data: String,
        final_chunk: bool,
    },
    Run {
        snapshot: String,
        #[serde(default = "default_network")]
        network: String,
        #[serde(default)]
        gpu: bool,
        command: Vec<String>,
        #[serde(default)]
        outputs: Vec<String>,
    },
}

fn default_network() -> String {
    "registries".into()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ready {
        version: u32,
        #[serde(default)]
        gpu_supported: bool,
    },
    Missing {
        hashes: Vec<String>,
    },
    Accepted,
    Busy,
    OutputManifest {
        manifest: super::snapshot::Manifest,
    },
    OutputChunk {
        hash: String,
        offset: u64,
        data: String,
        final_chunk: bool,
    },
    Output {
        stream: String,
        data: String,
    },
    Exit {
        code: i32,
    },
    Error {
        message: String,
    },
}

/// Messages use the same versioned request and status envelopes as the broker.
pub trait Frame: Serialize + DeserializeOwned {
    const REQUEST: bool;
}
impl Frame for Request {
    const REQUEST: bool = true;
}
impl Frame for Response {
    const REQUEST: bool = false;
}

pub fn read<T: Frame>(input: &mut impl BufRead) -> Result<Option<T>> {
    let mut frame = Vec::new();
    let n = input.take(MAX_FRAME + 1).read_until(b'\n', &mut frame)?;
    if n == 0 {
        return Ok(None);
    }
    if n as u64 > MAX_FRAME || frame.last() != Some(&b'\n') {
        bail!("invalid or oversized exclave frame");
    }
    let envelope: serde_json::Value =
        serde_json::from_slice(&frame).map_err(|_| anyhow::anyhow!("invalid exclave frame"))?;
    let payload = if T::REQUEST {
        if envelope.get("v").and_then(|v| v.as_u64()) != Some(crate::broker::PROTOCOL as u64)
            || envelope.get("cap").and_then(|v| v.as_str()) != Some("exclave")
        {
            bail!("invalid exclave request envelope");
        }
        envelope.get("args")
    } else {
        if envelope.get("status").and_then(|v| v.as_str()) != Some("done") {
            bail!("invalid exclave response envelope");
        }
        envelope.get("result")
    }
    .context("missing exclave frame")?;
    serde_json::from_value(payload.clone())
        .map(Some)
        .map_err(|_| anyhow::anyhow!("invalid exclave frame"))
}

pub fn write<T: Frame>(output: &mut impl Write, value: &T) -> Result<()> {
    let payload = serde_json::to_value(value)?;
    let envelope = if T::REQUEST {
        serde_json::json!({"v": crate::broker::PROTOCOL, "cap":"exclave", "args":payload, "reason":""})
    } else {
        serde_json::json!({"status":"done", "result":payload})
    };
    let data = serde_json::to_vec(&envelope)?;
    if data.len() as u64 + 1 > MAX_FRAME {
        bail!("oversized exclave frame");
    }
    output.write_all(&data)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_bounded_and_complete() {
        let mut bytes = Vec::new();
        write(
            &mut bytes,
            &Request::Hello {
                version: 1,
                project: "project".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            read::<Request>(&mut bytes.as_slice()).unwrap(),
            Some(Request::Hello { version: 1, .. })
        ));
        assert!(read::<Request>(&mut b"{}".as_slice()).is_err());
        let large = vec![b'x'; MAX_FRAME as usize + 1];
        assert!(read::<Request>(&mut large.as_slice()).is_err());
    }
}
