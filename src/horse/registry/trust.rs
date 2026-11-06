//! The local trust policy: which public keys may sign for which org, and
//! whether an unsigned artifact is allowed. Also the Ed25519 key pairs
//! that sign manifests.
//!
//! `trust.toml`:
//!
//! ```toml
//! require_signature = true
//!
//! [[key]]
//! org = "acme"
//! public = "<64 hex digits>"
//! name = "release key"
//! ```

use super::manifest::{lower_hex, Manifest};
use super::reference::valid_ident;
use crate::privfs;
use anyhow::{bail, Context as _, Result};
use ring::rand::SecureRandom as _;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trust {
    #[serde(default = "yes")]
    pub require_signature: bool,
    #[serde(default, rename = "key")]
    pub keys: Vec<TrustedKey>,
}

fn yes() -> bool {
    true
}

impl Default for Trust {
    fn default() -> Trust {
        Trust {
            require_signature: true,
            keys: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKey {
    pub org: String,
    /// The Ed25519 public key, 64 lower-case hex digits.
    pub public: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What the trust policy says about a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Signed by a key listed for the org: its name, or its public key.
    Trusted(String),
    Unsigned,
    UntrustedKey,
    BadSignature,
}

impl Verdict {
    /// The name of the verdict, without the key: `Trusted`, `Unsigned`,
    /// `UntrustedKey` or `BadSignature`.
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Trusted(_) => "Trusted",
            Verdict::Unsigned => "Unsigned",
            Verdict::UntrustedKey => "UntrustedKey",
            Verdict::BadSignature => "BadSignature",
        }
    }
}

impl Trust {
    /// Read the policy. A missing file is the default: a signature is
    /// required and no key is trusted.
    pub fn load(path: &Path) -> Result<Trust> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Trust::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        let t: Trust = toml::from_str(&text)
            .with_context(|| format!("{} is not a valid trust policy", path.display()))?;
        t.check()
            .with_context(|| format!("{} is not a valid trust policy", path.display()))?;
        Ok(t)
    }

    fn check(&self) -> Result<()> {
        for k in &self.keys {
            if !valid_ident(&k.org) {
                bail!(
                    "the org {:?} must be lower-case letters, digits and dashes",
                    k.org
                );
            }
            if lower_hex(&k.public, 32).is_none() {
                bail!(
                    "the key {:?} for {} is not 64 lower-case hex digits",
                    k.public,
                    k.org
                );
            }
        }
        Ok(())
    }

    /// Write the policy atomically, mode 0600.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.check()?;
        let text = toml::to_string(self).context("cannot encode the trust policy")?;
        privfs::write_atomic(path, text).with_context(|| format!("cannot write {}", path.display()))
    }

    /// Check a manifest: signed, by a key listed for its org, with a
    /// signature that verifies.
    pub fn allows(&self, m: &Manifest) -> Verdict {
        let (Some(author), Some(_)) = (m.author.as_deref(), m.signature.as_deref()) else {
            return Verdict::Unsigned;
        };
        let Ok((org, _)) = m.org_name() else {
            return Verdict::UntrustedKey;
        };
        let Some(k) = self
            .keys
            .iter()
            .find(|k| k.org == org && k.public == author)
        else {
            return Verdict::UntrustedKey;
        };
        if !m.verify(author) {
            return Verdict::BadSignature;
        }
        Verdict::Trusted(k.name.clone().unwrap_or_else(|| k.public.clone()))
    }
}

/// An Ed25519 key pair. The secret never shows in `Debug`.
pub struct Keypair {
    seed: [u8; 32],
    pair: Ed25519KeyPair,
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keypair({})", self.public_hex())
    }
}

impl Keypair {
    fn from_seed(seed: [u8; 32]) -> Result<Keypair> {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|e| anyhow::anyhow!("not an Ed25519 secret key: {e}"))?;
        Ok(Keypair { seed, pair })
    }

    /// The public key, 64 lower-case hex digits.
    pub fn public_hex(&self) -> String {
        crate::horse::hex(self.pair.public_key().as_ref())
    }

    pub(crate) fn sign_hex(&self, msg: &[u8]) -> String {
        crate::horse::hex(self.pair.sign(msg).as_ref())
    }
}

/// A new key pair from the system random source.
pub fn new_keypair() -> Keypair {
    let mut seed = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut seed)
        .expect("the system random source works");
    Keypair::from_seed(seed).expect("any 32 bytes are an Ed25519 seed")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    public: String,
    secret: String,
}

/// Read a key pair. The file must not be readable by group or others, and
/// its public key must match its secret.
pub fn load_keypair(path: &Path) -> Result<Keypair> {
    let meta =
        std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    if meta.permissions().mode() & 0o077 != 0 {
        bail!(
            "{} holds a secret key and is open to other users: run `chmod 600 {}`",
            path.display(),
            path.display()
        );
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let f: KeyFile =
        toml::from_str(&text).with_context(|| format!("{} is not a key file", path.display()))?;
    let Some(seed) = lower_hex(&f.secret, 32) else {
        bail!(
            "{}: the secret is not 64 lower-case hex digits",
            path.display()
        );
    };
    let mut s = [0u8; 32];
    s.copy_from_slice(&seed);
    let k = Keypair::from_seed(s)?;
    if k.public_hex() != f.public {
        bail!(
            "{}: the public key does not belong to the secret key",
            path.display()
        );
    }
    Ok(k)
}

/// Write a key pair atomically, mode 0600.
pub fn save_keypair(k: &Keypair, path: &Path) -> Result<()> {
    let f = KeyFile {
        public: k.public_hex(),
        secret: crate::horse::hex(&k.seed),
    };
    let text = toml::to_string(&f).context("cannot encode the key file")?;
    privfs::write_atomic(path, text).with_context(|| format!("cannot write {}", path.display()))
}
