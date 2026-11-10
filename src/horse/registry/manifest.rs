//! The manifest of a published DNA: what a version of `org/name` is, who
//! signed it, and the hash of its artifact.
//!
//! The signature covers `canonical()`: the manifest as JSON with sorted
//! keys, no whitespace and no `signature` field. An Ed25519 key signs it.

use super::reference::valid_ident;
use super::trust::Keypair;
use crate::horse::Hash;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// `org/name`.
    pub name: String,
    pub version: String,
    /// The hash of the artifact, as the store hashes a DNA.
    pub hash: Hash,
    /// The DNA this one came from. Lineage only: never followed.
    #[serde(default)]
    pub parents: Vec<Hash>,
    pub created_ms: u64,
    /// The public key of the signer, 64 hex digits.
    #[serde(default)]
    pub author: Option<String>,
    /// The Ed25519 signature over `canonical()`, 128 hex digits.
    #[serde(default)]
    pub signature: Option<String>,
}

impl Manifest {
    /// The org and the name of `org/name`.
    pub fn org_name(&self) -> Result<(&str, &str)> {
        match self.name.split_once('/') {
            Some((o, n)) if valid_ident(o) && valid_ident(n) => Ok((o, n)),
            _ => bail!(
                "the manifest name {:?} is not ORG/NAME in lower-case letters, digits and dashes",
                self.name
            ),
        }
    }

    fn fields(&self) -> BTreeMap<&'static str, Value> {
        let mut m = BTreeMap::new();
        m.insert("name", Value::from(self.name.clone()));
        m.insert("version", Value::from(self.version.clone()));
        m.insert("hash", Value::from(self.hash.as_str()));
        m.insert(
            "parents",
            Value::from(
                self.parents
                    .iter()
                    .map(|p| Value::from(p.as_str()))
                    .collect::<Vec<_>>(),
            ),
        );
        m.insert("created_ms", Value::from(self.created_ms));
        m.insert(
            "author",
            self.author.clone().map(Value::from).unwrap_or(Value::Null),
        );
        m
    }

    /// The signed bytes: JSON with sorted keys, no whitespace, and no
    /// `signature`.
    pub fn canonical(&self) -> Vec<u8> {
        serde_json::to_vec(&self.fields()).expect("a manifest always encodes")
    }

    /// Sign with `key`: sets `author` to its public key, then `signature`.
    pub fn sign(&mut self, key: &Keypair) {
        self.author = Some(key.public_hex());
        self.signature = Some(key.sign_hex(&self.canonical()));
    }

    /// Whether `signature` is a valid signature of `public_hex` over
    /// `canonical()`. False for a missing or malformed signature or key.
    pub fn verify(&self, public_hex: &str) -> bool {
        let Some(sig) = self.signature.as_deref() else {
            return false;
        };
        let (Some(public), Some(sig)) = (lower_hex(public_hex, 32), lower_hex(sig, 64)) else {
            return false;
        };
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
            .verify(&self.canonical(), &sig)
            .is_ok()
    }

    /// Parse a manifest file. Unknown fields are refused, so every field
    /// of a manifest is covered by its signature.
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        let m: Manifest = serde_json::from_slice(bytes).context("not a valid DNA manifest")?;
        m.org_name()?;
        if m.version.is_empty() {
            bail!("the manifest of {} has an empty version", m.name);
        }
        Ok(m)
    }

    /// The manifest file: canonical JSON with the signature, and a newline.
    pub fn encode(&self) -> Vec<u8> {
        let mut f = self.fields();
        f.insert(
            "signature",
            self.signature
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        let mut b = serde_json::to_vec(&f).expect("a manifest always encodes");
        b.push(b'\n');
        b
    }
}

/// Bytes of exactly `len` from lower-case hex.
pub(crate) fn lower_hex(s: &str, len: usize) -> Option<Vec<u8>> {
    if s.len() != len * 2 || s.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    crate::manifest::unhex(s)
}
