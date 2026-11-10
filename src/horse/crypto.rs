//! Authenticated encryption for horse store records.

use super::keys::Key;
use anyhow::{bail, Result};
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"paddock-horse-content\x01";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

pub(super) fn has_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn cipher(key: &Key) -> Result<LessSafeKey> {
    let key = UnboundKey::new(&aead::CHACHA20_POLY1305, key.as_bytes())
        .map_err(|_| anyhow::anyhow!("cannot initialize horse content cipher"))?;
    Ok(LessSafeKey::new(key))
}

/// The caller binds each record to its type, identity, and owning key.
pub(super) fn seal(key: &Key, context: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let nonce = super::random_bytes::<NONCE_LEN>()?;
    let mut body = Zeroizing::new(plaintext.to_vec());
    cipher(key)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(context),
            &mut *body,
        )
        .map_err(|_| anyhow::anyhow!("cannot encrypt horse content"))?;
    let mut record = Vec::with_capacity(MAGIC.len() + NONCE_LEN + body.len());
    record.extend_from_slice(MAGIC);
    record.extend_from_slice(&nonce);
    record.extend_from_slice(&body);
    Ok(record)
}

pub(super) fn open(key: &Key, context: &[u8], record: &[u8]) -> Result<Vec<u8>> {
    if !record.starts_with(MAGIC) || record.len() < MAGIC.len() + NONCE_LEN + TAG_LEN {
        bail!("invalid encrypted horse content");
    }
    let nonce = record[MAGIC.len()..MAGIC.len() + NONCE_LEN]
        .try_into()
        .expect("checked record length");
    let mut body = Zeroizing::new(record[MAGIC.len() + NONCE_LEN..].to_vec());
    let plaintext = cipher(key)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(context),
            &mut body[..],
        )
        .map_err(|_| anyhow::anyhow!("horse content authentication failed"))?;
    Ok(plaintext.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_records_authenticate_context_and_content() -> Result<()> {
        let key = Key::generate()?;
        let value = b"sensitive session canary\0\xff";
        let first = seal(&key, b"objects/id", value)?;
        let second = seal(&key, b"objects/id", value)?;
        assert_ne!(first, second);
        assert!(!first.windows(value.len()).any(|window| window == value));
        assert_eq!(open(&key, b"objects/id", &first)?, value);
        assert!(open(&key, b"objects/other", &first).is_err());
        assert!(open(&Key::generate()?, b"objects/id", &first).is_err());
        let mut changed = first.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(open(&key, b"objects/id", &changed).is_err());
        assert!(open(&key, b"objects/id", &first[..first.len() - 1]).is_err());
        Ok(())
    }
}
