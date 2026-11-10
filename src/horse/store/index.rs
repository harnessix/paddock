//! Authenticated clear metadata for navigating encrypted objects.

use super::{Hash, IndexedNode, Meta, Node, Store};
use anyhow::{bail, Context as _, Result};
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::io::Read as _;

const MAGIC: &[u8] = b"paddock-horse-index\x01";
const TAG_LEN: usize = 32;
const MAX_INDEX: usize = 4096;

pub(super) fn has_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

#[derive(Serialize, Deserialize)]
pub(super) struct Public {
    kind: String,
    parent: Option<Hash>,
    size: u64,
    time: Option<u64>,
}

fn authentication(store: &Store, hash: &Hash, bytes: &[u8]) -> Result<(hmac::Key, Vec<u8>)> {
    let key = store
        .keyring
        .as_ref()
        .context("horse keyring is unavailable")?
        .horse_key("shared")?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes());
    let mut context = b"paddock horse object index\0".to_vec();
    context.extend_from_slice(hash.as_str().as_bytes());
    context.extend_from_slice(bytes);
    Ok((key, context))
}

pub(super) fn seal(
    store: &Store,
    hash: &Hash,
    meta: &Meta,
    raw: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let time = serde_json::from_slice::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.get("time").and_then(|value| value.as_u64()));
    let public = Public {
        kind: meta.kind.clone(),
        parent: meta.parent.clone(),
        size: u64::try_from(raw.len())?,
        time,
    };
    let index = serde_json::to_vec(&public)?;
    if index.len() > MAX_INDEX {
        bail!("horse object metadata is too large");
    }
    let ciphertext = super::encryption::seal(store, "object", hash.as_str(), plaintext)?;
    let mut result = Vec::with_capacity(MAGIC.len() + 4 + index.len() + TAG_LEN + ciphertext.len());
    result.extend_from_slice(MAGIC);
    result.extend_from_slice(&(index.len() as u32).to_be_bytes());
    result.extend_from_slice(&index);
    let (key, context) = authentication(store, hash, &index)?;
    result.extend_from_slice(hmac::sign(&key, &context).as_ref());
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

fn parse(store: &Store, hash: &Hash, bytes: &[u8]) -> Result<Option<(Public, usize)>> {
    if !bytes.starts_with(MAGIC) {
        return Ok(None);
    }
    let start = MAGIC.len();
    let length = bytes
        .get(start..start + 4)
        .context("truncated horse object index")?;
    let length = u32::from_be_bytes(length.try_into().expect("four bytes")) as usize;
    if length > MAX_INDEX {
        bail!("horse object index is too large");
    }
    let end = start + 4 + length;
    let index = bytes
        .get(start + 4..end)
        .context("truncated horse object index")?;
    let actual = bytes
        .get(end..end + TAG_LEN)
        .context("truncated horse object index tag")?;
    let (key, context) = authentication(store, hash, index)?;
    if hmac::verify(&key, &context, actual).is_err() {
        bail!("horse object index authentication failed");
    }
    let public = serde_json::from_slice(index).context("invalid horse object index")?;
    Ok(Some((public, end + TAG_LEN)))
}

pub(super) fn open(store: &Store, hash: &Hash, bytes: &[u8]) -> Result<(Vec<u8>, Option<Public>)> {
    match parse(store, hash, bytes)? {
        Some((public, offset)) => Ok((
            super::encryption::open(store, "object", hash.as_str(), &bytes[offset..])?,
            Some(public),
        )),
        None => Ok((
            super::encryption::open(store, "object", hash.as_str(), bytes)?,
            None,
        )),
    }
}

pub(super) fn verify_node(public: &Public, node: &Node) -> Result<()> {
    if public.kind != node.meta.kind
        || public.parent != node.meta.parent
        || public.size != node.raw.len() as u64
    {
        bail!("horse object index disagrees with its content");
    }
    Ok(())
}

/// Read and authenticate only the small index, without decrypting content.
pub(super) fn read(store: &Store, hash: &Hash) -> Result<Option<IndexedNode>> {
    if !store.encrypted {
        return Ok(None);
    }
    let mut file = std::fs::File::open(store.object_path(hash))?;
    let mut prefix = vec![0; MAGIC.len() + 4];
    if file.read_exact(&mut prefix).is_err() {
        return Ok(None);
    }
    if !prefix.starts_with(MAGIC) {
        return Ok(None);
    }
    let length = u32::from_be_bytes(prefix[MAGIC.len()..].try_into().expect("four bytes")) as usize;
    if length > MAX_INDEX {
        bail!("horse object index is too large");
    }
    let mut rest = vec![0; length + TAG_LEN];
    file.read_exact(&mut rest)
        .context("truncated horse object index")?;
    prefix.extend_from_slice(&rest);
    let (public, _) = parse(store, hash, &prefix)?.context("invalid horse object index")?;
    Ok(Some(IndexedNode {
        hash: hash.clone(),
        kind: public.kind,
        parent: public.parent,
        size: public.size,
        time: public.time,
    }))
}

impl super::Store {
    pub(crate) fn read_ref_bounded(
        &self,
        rel: &str,
        budget: u64,
        inspected: &mut u64,
    ) -> Result<Option<Vec<u8>>> {
        use std::io::Read as _;
        let file = match std::fs::File::open(self.root.join(rel)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let remaining = budget.saturating_sub(*inspected);
        let mut bytes = Vec::new();
        file.take(remaining.saturating_add(1))
            .read_to_end(&mut bytes)?;
        *inspected = inspected.saturating_add(bytes.len() as u64);
        if *inspected > budget {
            bail!("peek grant byte limit reached");
        }
        Ok(Some(if self.encrypted {
            super::encryption::open(self, "ref", rel, &bytes)?
        } else {
            bytes
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_encrypted_objects_remain_readable_without_an_index() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::env::HostEnv::from_parts(
            temp.path().to_path_buf(),
            "test".into(),
            0,
            0,
            Default::default(),
        );
        let store = Store::open_host(&env)?;
        let hash = store.put_blob("test", b"legacy encrypted bytes")?;
        let path = store.object_path(&hash);
        let indexed = std::fs::read(&path)?;
        let (_, offset) = parse(&store, &hash, &indexed)?.context("missing index")?;
        std::fs::write(&path, &indexed[offset..])?;
        assert_eq!(store.get(&hash)?.raw, b"legacy encrypted bytes");
        let mut inspected = 0;
        let nodes = store.indexed_chain(Some(&hash), u64::MAX, &mut inspected)?;
        assert_eq!(nodes[0].hash, hash);
        assert!(inspected > 0);
        Ok(())
    }
}
