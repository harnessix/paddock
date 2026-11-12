//! List approval cards with optional strict validation.

use super::*;

fn scan_cards(dir: &Path, strict: bool) -> io::Result<Vec<Pending>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if strict {
                missing_card(dir)?;
            }
            return Ok(out);
        }
        Err(e) if strict => return Err(e),
        Err(_) => return Ok(out),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if strict => return Err(e),
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.extension().is_some_and(|x| x == "json") {
            continue;
        }
        let text = match evidence::read_text(&path) {
            Ok(text) => text,
            // A card can be withdrawn after the directory was read.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if strict {
                    missing_card(&path)?;
                }
                continue;
            }
            Err(e) if strict => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("cannot read {}: {e}", path.display()),
                ))
            }
            Err(_) => continue,
        };
        let req: Pending = match serde_json::from_str(&text) {
            Ok(req) => req,
            Err(e) if strict => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid approval {}: {e}", path.display()),
                ))
            }
            Err(_) => continue,
        };
        if strict
            && (!valid_id(&req.id)
                || path.file_stem().and_then(|s| s.to_str()) != Some(req.id.as_str()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid approval identity in {}", path.display()),
            ));
        }
        out.push(req);
    }
    Ok(out)
}

/// Every request that waits for an answer, from every running sandbox.
pub fn pending(env: &HostEnv) -> Vec<Pending> {
    pending_inner(env, false).unwrap_or_default()
}

/// Strict callers reject any unreadable or corrupt store instead of
/// describing it as empty. The CLI uses the per-id report below.
pub fn pending_checked(env: &HostEnv) -> io::Result<Vec<Pending>> {
    pending_inner(env, true)
}

fn pending_inner(env: &HostEnv, strict: bool) -> io::Result<Vec<Pending>> {
    let dir = approvals_dir(env);
    // Check before recovery too: recovery must not hide a malformed card.
    if strict {
        scan_cards(&dir, true)?;
    }
    recover_sidecars(env, strict)?;
    let out = scan_cards(&dir, strict)?;
    // A question of a sandbox that no longer runs: its paddock process was
    // killed before it could withdraw the question. Older than a minute, so
    // a question that is being put on the card is never removed.
    let mut visible = Vec::new();
    for p in out {
        if strict && valid_id(&p.id) {
            let _lock = lock_approvals(&dir, &p.id)?;
            if owner_state(&dir, &p.id)? == OwnerState::Unknown {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("approval {} owner liveness is unknown", p.id),
                ));
            }
            if let Some(answer) = answer_text(&dir, &p.id)? {
                if !answer_valid(&p, &answer) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("approval {} has an invalid answer sidecar", p.id),
                    ));
                }
            }
        }
        // A selected connect is no longer actionable, even while its
        // application is waiting for the rules lock.
        if p.capability == CONNECT_CAPABILITY && valid_id(&p.id) {
            let selected = lock_approvals(&dir, &p.id).and_then(|_lock| answer_text(&dir, &p.id));
            match selected {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(e) if strict => return Err(e),
                Err(e) => {
                    log::warn!("cannot inspect approval {}: {e}", p.id);
                    continue;
                }
            }
        }
        let candidate = (p.capability == ASK_CAPABILITY
            && state::now().saturating_sub(p.created) > 60)
            || p.connect.as_ref().is_some_and(|c| !helper_alive(c.pid));
        if candidate && valid_id(&p.id) {
            match reap_stale(env, &p) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) if strict => return Err(e),
                Err(e) => log::warn!("cannot clean stale approval {}: {e}", p.id),
            }
        }
        // An accepted ask reply remains available to its owner, but it no
        // longer counts as an unanswered card.
        if p.capability == ASK_CAPABILITY && valid_id(&p.id) {
            let shown = lock_approvals(&dir, &p.id).and_then(|_lock| {
                let path = dir.join(format!("{}.json", p.id));
                match std::fs::symlink_metadata(&path) {
                    Ok(m) if m.is_file() => {}
                    Ok(_) if strict => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "approval card is not a file",
                        ))
                    }
                    Ok(_) => return Ok(false),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        if strict {
                            missing_card(&path)?;
                        }
                        return Ok(false);
                    }
                    Err(e) if strict => return Err(e),
                    Err(_) => return Ok(false),
                }
                Ok(answer_text(&dir, &p.id)?.is_none())
            });
            match shown {
                Ok(false) => continue,
                Ok(true) => {}
                Err(e) if strict => return Err(e),
                Err(e) => log::warn!("cannot inspect approval {}: {e}", p.id),
            }
        }
        visible.push(p);
    }
    // Host requests first: questions of agents never push them off the card.
    visible.sort_by_key(|p| (p.capability == ASK_CAPABILITY, p.created));
    Ok(visible)
}
