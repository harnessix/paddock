//! A registry reference: `registry://org/name@version`.
//!
//! The version is an exact version (`1.2.0`), `latest`, or the hash of the
//! artifact (`sha256:<64 hex>`).

use crate::horse::Hash;
use anyhow::{bail, Result};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

pub const SCHEME: &str = "registry://";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub org: String,
    pub name: String,
    pub version: Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Version {
    Exact(String),
    Latest,
    Hash(Hash),
}

/// An org or a name: `[a-z0-9][a-z0-9-]*`.
pub fn valid_ident(s: &str) -> bool {
    let mut b = s.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// An exact version: `[0-9A-Za-z.-]+`, and not `.` or `..`, so it is a
/// safe file name. `latest` is not an exact version.
pub fn valid_version(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s != "latest"
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
}

/// Compare two versions part by part, split at dots: numbers as numbers
/// (`1.10.0` > `1.9.0`), other parts as text. A number sorts before text,
/// and a version with more parts sorts after its prefix.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let mut x = a.split('.');
    let mut y = b.split('.');
    loop {
        match (x.next(), y.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = match (number(p), number(q)) {
                    (Some(m), Some(n)) => m.cmp(&n).then_with(|| p.len().cmp(&q.len())),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => p.cmp(q),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// A part of digits only, as a key that compares like the number of any
/// size: the length without leading zeros, then the digits.
fn number(p: &str) -> Option<(usize, String)> {
    if p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let t = p.trim_start_matches('0');
    Some((t.len(), t.to_string()))
}

impl FromStr for Reference {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Reference> {
        let Some(rest) = s.strip_prefix(SCHEME) else {
            bail!("{s:?} is not a registry reference: it must start with {SCHEME}");
        };
        let Some((path, version)) = rest.rsplit_once('@') else {
            bail!("{s:?} has no version: add @VERSION, @latest or @sha256:HASH");
        };
        let Some((org, name)) = path.split_once('/') else {
            bail!("{s:?} has no name: the form is {SCHEME}ORG/NAME@VERSION");
        };
        if !valid_ident(org) {
            bail!("{s:?}: the org {org:?} must be lower-case letters, digits and dashes, starting with a letter or digit");
        }
        if !valid_ident(name) {
            bail!("{s:?}: the name {name:?} must be lower-case letters, digits and dashes, starting with a letter or digit");
        }
        let version = parse_version(version).map_err(|e| anyhow::anyhow!("{s:?}: {e}"))?;
        Ok(Reference {
            org: org.to_string(),
            name: name.to_string(),
            version,
        })
    }
}

fn parse_version(v: &str) -> Result<Version> {
    if v == "latest" {
        return Ok(Version::Latest);
    }
    if let Some(h) = v.strip_prefix("sha256:") {
        return match Hash::parse(h) {
            Ok(h) => Ok(Version::Hash(h)),
            Err(_) => bail!("the hash {h:?} must be 64 lower-case hex digits"),
        };
    }
    if !valid_version(v) {
        bail!(
            "the version {v:?} must be letters, digits, dots and dashes, `latest`, or sha256:HASH"
        );
    }
    Ok(Version::Exact(v.to_string()))
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Version::Exact(v) => f.write_str(v),
            Version::Latest => f.write_str("latest"),
            Version::Hash(h) => write!(f, "sha256:{h}"),
        }
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCHEME}{}/{}@{}", self.org, self.name, self.version)
    }
}
