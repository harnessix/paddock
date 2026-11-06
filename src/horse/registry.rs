//! The DNA registry: published DNA, found by `registry://org/name@version`.
//!
//! An artifact is the canonical text of a DNA (`Dna::encode`). Its hash is
//! the hash the store gives that DNA, so a registry hash and a store hash
//! of one DNA are equal. An index maps a reference to a signed manifest,
//! and the manifest to the hash of the artifact. The local trust policy
//! says which keys may sign for which org.
//!
//! This layer is pure: it reads and writes index folders and nothing else.

pub mod host;
pub mod index;
pub mod manifest;
pub mod reference;
pub mod trust;

pub use index::{configured, open_indexes, FileIndex, Transport};
pub use manifest::Manifest;
pub use reference::{Reference, Version};
pub use trust::{Keypair, Trust, TrustedKey, Verdict};

use crate::horse::{dna, Hash};
use anyhow::{bail, Context as _, Result};

/// A reference resolved to verified artifact bytes.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub reference: Reference,
    pub manifest: Manifest,
    pub bytes: Vec<u8>,
    /// The name of the index that had it.
    pub index: String,
    /// The trusted key that signed it; None for an unsigned artifact under
    /// a policy that allows one.
    pub signed_by: Option<String>,
}

/// Why `resolve` refused a reference, as a value that a caller can find
/// with `downcast_ref` on the error. The text names the index and, for an
/// untrusted key, the key; a caller that must not show those (an answer
/// to a sandbox) shows only `verdict` or `missing`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The trust verdict, when the trust policy refused the manifest.
    pub verdict: Option<Verdict>,
    /// True when no index has the reference.
    pub missing: bool,
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

fn refused(verdict: Verdict, message: String) -> anyhow::Error {
    anyhow::Error::new(Refusal {
        verdict: Some(verdict),
        missing: false,
        message,
    })
}

/// The hash of an artifact: the hash of the DNA object in the store.
pub fn artifact_hash(bytes: &[u8]) -> Hash {
    crate::horse::store::blob_hash(dna::MEDIA, bytes).expect("blob metadata always encodes")
}

/// Check that `bytes` are the artifact of `m`: the hash matches, and the
/// bytes are canonical DNA text.
pub fn check_artifact(m: &Manifest, bytes: &[u8]) -> Result<()> {
    let got = artifact_hash(bytes);
    if got != m.hash {
        bail!(
            "the artifact of {}@{} has the hash {got}, but its manifest says {}; it is refused",
            m.name,
            m.version,
            m.hash
        );
    }
    let text = std::str::from_utf8(bytes)
        .with_context(|| format!("the artifact of {}@{} is not UTF-8 text", m.name, m.version))?;
    let d = dna::parse(text)
        .with_context(|| format!("the artifact of {}@{} is not a DNA", m.name, m.version))?;
    if d.encode() != text {
        bail!(
            "the artifact of {}@{} is DNA but not in canonical form; publish the text that `Dna::encode` gives",
            m.name,
            m.version
        );
    }
    Ok(())
}

/// Find the manifest of `r`: the first index that has it wins.
fn find(
    indexes: &[Box<dyn Transport>],
    r: &Reference,
) -> Result<Option<(Manifest, usize, String)>> {
    for (i, t) in indexes.iter().enumerate() {
        let version = match &r.version {
            Version::Exact(v) => v.clone(),
            Version::Latest => {
                let vs = t
                    .versions(&r.org, &r.name)
                    .with_context(|| format!("index {}", t.name()))?;
                match vs.last() {
                    Some(v) => v.clone(),
                    None => continue,
                }
            }
            Version::Hash(h) => {
                if t.object(h)
                    .with_context(|| format!("index {}", t.name()))?
                    .is_none()
                {
                    continue;
                }
                let m = Manifest {
                    name: format!("{}/{}", r.org, r.name),
                    version: format!("sha256:{h}"),
                    hash: h.clone(),
                    parents: Vec::new(),
                    created_ms: 0,
                    author: None,
                    signature: None,
                };
                return Ok(Some((m.clone(), i, m.version)));
            }
        };
        if let Some(m) = t
            .manifest(&r.org, &r.name, &version)
            .with_context(|| format!("index {}", t.name()))?
        {
            return Ok(Some((m, i, version)));
        }
    }
    Ok(None)
}

/// Resolve a reference through the indexes, in order, to artifact bytes
/// that match their manifest, are DNA, and pass the trust policy.
pub fn resolve(indexes: &[Box<dyn Transport>], r: &Reference, policy: &Trust) -> Result<Resolved> {
    if indexes.is_empty() {
        bail!("cannot resolve {r}: no registry index is set up");
    }
    let Some((manifest, i, version)) =
        find(indexes, r).with_context(|| format!("cannot resolve {r}"))?
    else {
        let names: Vec<&str> = indexes.iter().map(|t| t.name()).collect();
        return Err(anyhow::Error::new(Refusal {
            verdict: None,
            missing: true,
            message: format!("{r} is not in any index ({})", names.join(", ")),
        }));
    };
    let t = &indexes[i];
    let from = t.name().to_string();
    let want = format!("{}/{}", r.org, r.name);
    // An index cannot serve the manifest of one name or version under
    // another: the trust check reads the org from the manifest.
    if manifest.name != want || manifest.version != version {
        bail!(
            "{r}: index {from} has the manifest of {}@{} in its place; it is refused",
            manifest.name,
            manifest.version
        );
    }
    let signed_by = match policy.allows(&manifest) {
        Verdict::Trusted(k) => Some(k),
        Verdict::Unsigned if !policy.require_signature => None,
        v @ Verdict::Unsigned => {
            return Err(refused(
                v,
                format!(
                "{r} from index {from} is not signed, and the trust policy requires a signature"
            ),
            ))
        }
        v @ Verdict::UntrustedKey => {
            return Err(refused(
                v,
                format!(
                    "{r} from index {from} is signed by the key {}, which is not trusted for {}",
                    manifest.author.as_deref().unwrap_or("?"),
                    r.org
                ),
            ))
        }
        v @ Verdict::BadSignature => return Err(refused(
            v,
            format!(
            "{r} from index {from} has a signature that does not verify: the manifest was changed after it was signed"
        ),
        )),
    };
    let Some(bytes) = t
        .object(&manifest.hash)
        .with_context(|| format!("cannot resolve {r}: index {from}"))?
    else {
        bail!(
            "{r}: index {from} has the manifest but not the artifact {}",
            manifest.hash
        );
    };
    check_artifact(&manifest, &bytes).with_context(|| format!("{r} from index {from}"))?;
    Ok(Resolved {
        reference: r.clone(),
        manifest,
        bytes,
        index: from,
        signed_by,
    })
}

#[cfg(test)]
mod tests;
