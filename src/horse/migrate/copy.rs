//! Convert and verify legacy objects and references without changing them.

use super::snapshot::{self, Snapshot};
use crate::horse::{store::Meta, Hash, Horse, Session, Store};
use anyhow::{bail, Context as _, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("migration source has a non-UTF-8 path")
}

fn object_hash(path: &Path) -> Result<Option<Hash>> {
    let text = text(path)?;
    let Some(rest) = text.strip_prefix("objects/") else {
        return Ok(None);
    };
    let Some((a, b)) = rest.split_once('/') else {
        bail!("invalid legacy object path {text}")
    };
    if a.len() != 2 || b.len() != 62 || b.contains('/') {
        bail!("invalid legacy object path {text}");
    }
    Ok(Some(Hash::parse(&format!("{a}{b}"))?))
}

fn excluded(path: &Path) -> bool {
    matches!(
        path.components()
            .next()
            .and_then(|c| c.as_os_str().to_str()),
        Some("objects" | "tmp" | "keys")
    ) || matches!(
        path.to_str(),
        Some("lock" | "key-check" | "encrypted-format")
    )
}

fn archive_name(relative: &str) -> String {
    format!(".paddock-migration/refs/{}", Hash::of(relative.as_bytes()))
}

fn reference_owner<'a>(
    path: &str,
    horses: &'a [Horse],
    sessions: &'a [Session],
) -> Option<&'a str> {
    let mut parts = path.split('/');
    let group = parts.next()?;
    let name = parts.next()?;
    match group {
        "horses" | "events" | "memory" | "views" => {
            let id = name.strip_suffix(".json").unwrap_or(name);
            horses
                .iter()
                .find(|horse| horse.id == id)
                .map(|horse| horse.id.as_str())
        }
        "sessions" | "inbox" | "deliveries" | "guard" | "approvals" => {
            let id = name
                .strip_suffix(".json")
                .or_else(|| name.strip_suffix(".jsonl"))
                .unwrap_or(name);
            sessions
                .iter()
                .find(|session| session.id == id)
                .map(|session| session.horse.as_str())
        }
        _ => None,
    }
}

fn object(root: &crate::safefs::Root, path: &Path, hash: &Hash) -> Result<(Meta, Vec<u8>)> {
    let bytes = snapshot::read(root, path)?;
    if Hash::of(&bytes) != *hash {
        bail!("legacy object {} does not match its hash", hash.short());
    }
    let newline = bytes
        .iter()
        .position(|b| *b == b'\n')
        .context("legacy object has no metadata")?;
    let meta =
        serde_json::from_slice(&bytes[..newline]).context("legacy object metadata is damaged")?;
    Ok((meta, bytes[newline + 1..].to_vec()))
}

fn add_chain(
    owners: &mut BTreeMap<Hash, String>,
    objects: &BTreeMap<Hash, Meta>,
    head: Option<&Hash>,
    horse: &str,
) -> Result<()> {
    let mut next = head.cloned();
    let mut seen = BTreeSet::new();
    while let Some(hash) = next {
        if !seen.insert(hash.clone()) {
            bail!("legacy object chain has a cycle");
        }
        let meta = objects
            .get(&hash)
            .with_context(|| format!("legacy node {} is missing", hash.short()))?;
        next = meta.parent.clone();
        owners.entry(hash).or_insert_with(|| horse.to_string());
    }
    Ok(())
}

/// Import every object and mutable reference, then compare the decrypted
/// bytes and metadata against the source. The source is read-only.
pub(super) fn build(source: &Path, snapshot: &Snapshot, target: &Store) -> Result<usize> {
    let root = crate::safefs::Root::open(source)?;
    let mut objects = BTreeMap::new();
    let mut horses = Vec::<Horse>::new();
    let mut sessions = Vec::<Session>::new();
    for path in &snapshot.files {
        if let Some(hash) = object_hash(path)? {
            let (meta, _) = object(&root, path, &hash)?;
            objects.insert(hash, meta);
        } else if let Some(name) = text(path)?
            .strip_prefix("horses/")
            .and_then(|n| n.strip_suffix(".json"))
        {
            let horse: Horse = serde_json::from_slice(&snapshot::read(&root, path)?)?;
            if horse.id != name {
                bail!("legacy horse id does not match its file");
            }
            horses.push(horse);
        } else if let Some(name) = text(path)?
            .strip_prefix("sessions/")
            .and_then(|n| n.strip_suffix(".json"))
        {
            let session: Session = serde_json::from_slice(&snapshot::read(&root, path)?)?;
            if session.id != name {
                bail!("legacy session id does not match its file");
            }
            sessions.push(session);
        }
    }
    let keys = target
        .keyring
        .as_ref()
        .context("migration target has no keyring")?;
    let mut owners = BTreeMap::new();
    for horse in &horses {
        keys.ensure_horse_key(&horse.id)?;
        add_chain(&mut owners, &objects, Some(&horse.dna), &horse.id)?;
        let event = format!("events/{}.json", horse.id);
        if snapshot.files.iter().any(|p| p == Path::new(&event)) {
            let head: Hash = serde_json::from_slice(&snapshot::read(&root, Path::new(&event))?)?;
            add_chain(&mut owners, &objects, Some(&head), &horse.id)?;
        }
    }
    for session in &sessions {
        if !horses.iter().any(|horse| horse.id == session.horse) {
            bail!("legacy session has no horse");
        }
        add_chain(&mut owners, &objects, session.head.as_ref(), &session.horse)?;
        add_chain(
            &mut owners,
            &objects,
            session.native_header.as_ref(),
            &session.horse,
        )?;
    }
    for path in &snapshot.files {
        if let Some(hash) = object_hash(path)? {
            let (meta, raw) = object(&root, path, &hash)?;
            let scoped = owners
                .get(&hash)
                .map(|horse| target.for_horse(horse))
                .transpose()?;
            let target = scoped.as_ref().unwrap_or(target);
            if target.put(&meta, &raw)? != hash {
                bail!("legacy object changed during migration");
            }
            let check = target.get(&hash)?;
            if check.meta != meta || check.raw != raw {
                bail!("encrypted object verification failed");
            }
        } else if !excluded(path) {
            let relative = text(path)?;
            if relative.starts_with(".paddock-migration/") {
                bail!("legacy store already contains reserved migration paths");
            }
            let bytes = snapshot::read(&root, path)?;
            let scoped = reference_owner(relative, &horses, &sessions)
                .map(|horse| target.for_horse(horse))
                .transpose()?;
            let writer = scoped.as_ref().unwrap_or(target);
            writer.write_ref(relative, &bytes)?;
            writer.write_ref(&archive_name(relative), &bytes)?;
            if writer.read_ref(relative)?.as_deref() != Some(bytes.as_slice()) {
                bail!("encrypted reference verification failed: {relative}");
            }
            if writer.read_ref(&archive_name(relative))?.as_deref() != Some(bytes.as_slice()) {
                bail!("encrypted reference archive verification failed: {relative}");
            }
        }
    }
    Ok(objects.len())
}

/// Verify that all immutable nodes in the retained copy still exist byte for
/// byte in the current encrypted store before that copy is dropped.
pub(super) fn verify_nodes(source: &Path, snapshot: &Snapshot, target: &Store) -> Result<usize> {
    let root = crate::safefs::Root::open(source)?;
    let mut count = 0;
    for path in &snapshot.files {
        if let Some(hash) = object_hash(path)? {
            let (meta, raw) = object(&root, path, &hash)?;
            let node = target.get(&hash)?;
            if node.meta != meta || node.raw != raw {
                bail!(
                    "encrypted node {} does not match the retained copy",
                    hash.short()
                );
            }
            count += 1;
        }
    }
    Ok(count)
}

pub(super) fn verify_all(source: &Path, snapshot: &Snapshot, target: &Store) -> Result<usize> {
    let count = verify_archive(source, snapshot, target)?;
    let root = crate::safefs::Root::open(source)?;
    for path in &snapshot.files {
        if !excluded(path) {
            let relative = text(path)?;
            let original = snapshot::read(&root, path)?;
            if target.read_ref(relative)?.as_deref() != Some(original.as_slice()) {
                bail!("encrypted reference verification failed: {relative}");
            }
        }
    }
    Ok(count)
}

/// Preserve the original mutable records inside the encrypted store even if
/// live references have legitimately changed since the migration.
pub(super) fn verify_archive(source: &Path, snapshot: &Snapshot, target: &Store) -> Result<usize> {
    let count = verify_nodes(source, snapshot, target)?;
    let root = crate::safefs::Root::open(source)?;
    for path in &snapshot.files {
        if !excluded(path) {
            let relative = text(path)?;
            let original = snapshot::read(&root, path)?;
            if target.read_ref(&archive_name(relative))?.as_deref() != Some(original.as_slice()) {
                bail!("encrypted reference archive verification failed: {relative}");
            }
        }
    }
    Ok(count)
}
