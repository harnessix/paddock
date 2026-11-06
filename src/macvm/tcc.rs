//! The Accessibility, Full Disk Access, Screen Recording and Photos
//! permissions of the guest agent, set offline in the TCC database of the
//! stopped guest, platform-neutral.
//!
//! With the guest running, System Integrity Protection guards
//! `/Library/Application Support/com.apple.TCC/TCC.db`; a person must turn
//! the switch on in System Settings. With the guest stopped, the file is a
//! plain SQLite database on the Data volume. `paddock macvm setup` stops
//! the guest after the agent's first hello (the agent asked for the
//! permission at its start, so macOS wrote the agent's row), mounts the
//! Data volume without owners and sets the row to allowed:
//!
//! - an existing row: `auth_value` 2 (allowed), `auth_reason` 3, its own
//!   `csreq` kept;
//! - no row: a new one with the code requirement of the agent binary;
//! - every value goes in as a bound parameter, never as SQL text;
//! - one transaction, then `integrity_check` must say `ok`, a WAL is
//!   checkpointed into the database, and no `-wal`, `-shm` or `-journal`
//!   file that this write made stays (it would belong to 99:99).
//!
//! The code requirement: the agent is signed with the designated
//! requirement `identifier "dev.paddock.guest"`
//! (`packaging/macos/build-guest.sh`), so the row stays valid when the
//! agent updates itself in a session: an ad-hoc signature without it would
//! pin the hash of one build.

use super::diskimage::{Attached, DiskTool};
use super::text;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

/// The database on the Data volume.
pub const DB: &str = "Library/Application Support/com.apple.TCC/TCC.db";
/// The Accessibility service.
pub const ACCESSIBILITY: &str = "kTCCServiceAccessibility";
/// Full Disk Access. The agent copies the guest's home onto the data
/// disk: without it each guarded folder (Desktop, the data of other apps)
/// waits for a prompt that nobody answers. The home is the sandbox's own.
pub const FULL_DISK: &str = "kTCCServiceSystemPolicyAllFiles";
/// The Photos library: the copy of `~/Pictures` asks for it on its own.
/// A service of the user, in the user's database ([`USER_DB`]).
pub const PHOTOS: &str = "kTCCServicePhotos";
/// Screen Recording: the agent captures the image of each guest window
/// once (`CGWindowListCreateImage`) and measures its corner radius from
/// the alpha of the corners, so the host clip matches the window. Without
/// it the capture shows no window and the host uses the radius of the
/// guest's macOS version.
pub const SCREEN_CAPTURE: &str = "kTCCServiceScreenCapture";
/// The services that the agent gets in the system database, in order.
pub const SERVICES: &[&str] = &[ACCESSIBILITY, FULL_DISK, SCREEN_CAPTURE];
/// The services that the agent gets in the user's database.
pub const USER_SERVICES: &[&str] = &[PHOTOS];
/// The database of the guest's user on the Data volume.
pub const USER_DB: &str = "Users/paddock/Library/Application Support/com.apple.TCC/TCC.db";
/// The agent as TCC names it: its path.
pub const CLIENT: &str = "/Library/Application Support/paddock/current/paddock-guest";
/// `client_type` of a path client.
pub const CLIENT_TYPE_PATH: i64 = 1;
/// `auth_value` allowed.
pub const ALLOWED: i64 = 2;
/// `auth_reason` of a row that the person set in System Settings, as
/// macOS 27 records it.
pub const REASON_USER_SET: i64 = 3;
/// The identifier of the agent's signature.
pub const IDENTIFIER: &str = "dev.paddock.guest";

/// What the patch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Patched {
    /// The row was allowed already.
    Already,
    /// The row of the agent is allowed now; `csreq_added` when it had no
    /// requirement and got the agent's.
    Updated { csreq_added: bool },
    /// No row was there: a new one.
    Inserted,
}

/// The compiled code requirement `identifier "ID"`, as `csreq -r- -b`
/// writes it: a requirement blob (magic `0xfade0c00`, length, kind 1 for
/// an expression), the operator `opIdent` (2), and the string as its
/// length and bytes, padded to four bytes. Big-endian.
pub fn identifier_requirement(id: &str) -> Vec<u8> {
    let mut expr = Vec::new();
    expr.extend_from_slice(&2u32.to_be_bytes());
    expr.extend_from_slice(&(id.len() as u32).to_be_bytes());
    expr.extend_from_slice(id.as_bytes());
    while expr.len() % 4 != 0 {
        expr.push(0);
    }
    let mut blob = Vec::new();
    blob.extend_from_slice(&0xfade_0c00u32.to_be_bytes());
    blob.extend_from_slice(&((12 + expr.len()) as u32).to_be_bytes());
    blob.extend_from_slice(&1u32.to_be_bytes());
    blob.extend(expr);
    blob
}

/// The designated requirement in the output of `codesign -d -r- FILE`:
/// the text after `designated =>`. Older `codesign` prints it with a `# `
/// in front.
pub fn designated(codesign_output: &str) -> Option<String> {
    codesign_output.lines().find_map(|l| {
        let l = l.trim().trim_start_matches('#').trim();
        l.strip_prefix("designated =>")
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
    })
}

/// True when a requirement text is the one of the agent's signature.
pub fn is_agent_identifier(requirement: &str) -> bool {
    requirement == format!("identifier \"{IDENTIFIER}\"") // not user text
}

/// The compiled designated requirement of the agent binary, for a TCC
/// row that has none: `codesign -d -r-` gives its text; the identifier
/// requirement of paddock's signature is built here, any other one is
/// compiled with `csreq -b`. When the tools fail (no binary, no Mac), the
/// identifier requirement of the signature that `build-guest.sh` makes.
pub fn agent_requirement(agent: Option<&Path>) -> Vec<u8> {
    let fallback = identifier_requirement(IDENTIFIER);
    let Some(agent) = agent else {
        return fallback;
    };
    let argv: Vec<String> = vec![
        "/usr/bin/codesign".into(),
        "-d".into(),
        "-r-".into(),
        agent.display().to_string(),
    ];
    let dr = match super::diskimage::run(&argv) {
        Ok((true, out, err)) => designated(&out).or_else(|| designated(&err)),
        _ => None,
    };
    let Some(dr) = dr else {
        return fallback;
    };
    if is_agent_identifier(&dr) {
        return fallback;
    }
    compile(&dr).unwrap_or_else(|e| {
        log::warn!("macvm: {e:#}");
        fallback
    })
}

/// `csreq -r FILE -b OUT` of a requirement text.
fn compile(requirement: &str) -> Result<Vec<u8>> {
    let dir = tempfile_dir()?;
    let input = dir.join("req.txt");
    let output = dir.join("req.bin");
    std::fs::write(&input, requirement).with_context(|| text::cannot_write(&input))?;
    let argv: Vec<String> = vec![
        "/usr/bin/csreq".into(),
        "-r".into(),
        input.display().to_string(),
        "-b".into(),
        output.display().to_string(),
    ];
    let r = super::diskimage::run(&argv);
    let blob = std::fs::read(&output);
    let _ = std::fs::remove_dir_all(&dir);
    match r? {
        (true, _, _) => blob.with_context(|| text::cannot_read(&output)),
        (false, out, err) => Err(anyhow!(text::disk_command_failed(
            &argv.join(" "),
            out.trim(),
            err.trim()
        ))),
    }
}

fn tempfile_dir() -> Result<PathBuf> {
    let d = std::env::temp_dir().join(format!("paddock-csreq-{}", std::process::id())); // not user text
    crate::privfs::dir(&d)?;
    Ok(d)
}

/// The `-wal`, `-shm` and `-journal` files of a database.
fn side_files(db: &Path) -> [PathBuf; 3] {
    let with = |s: &str| {
        let mut o = db.as_os_str().to_os_string();
        o.push(s);
        PathBuf::from(o)
    };
    [with("-wal"), with("-shm"), with("-journal")]
}

/// Allow the row of the agent for `service`, inside `tx`.
fn allow_service(
    tx: &rusqlite::Transaction,
    db: &Path,
    columns: &[(String, bool)],
    service: &str,
    csreq: &mut dyn FnMut() -> Result<Vec<u8>>,
) -> Result<Patched> {
    let row: Option<(i64, Option<Vec<u8>>)> = tx
        .query_row(
            "SELECT auth_value, csreq FROM access WHERE service = ?1 AND client = ?2 AND client_type = ?3",
            params![service, CLIENT, CLIENT_TYPE_PATH],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let now = crate::state::now() as i64;
    let patched = match row {
        Some((ALLOWED, Some(_))) => Patched::Already,
        Some((_, existing)) => {
            let csreq_added = existing.as_ref().is_none_or(|b| b.is_empty());
            let req = match existing.filter(|b| !b.is_empty()) {
                Some(b) => b,
                None => csreq()?,
            };
            tx.execute(
                "UPDATE access SET auth_value = ?1, auth_reason = ?2, csreq = ?3, last_modified = ?4 \
                 WHERE service = ?5 AND client = ?6 AND client_type = ?7",
                params![ALLOWED, REASON_USER_SET, req, now, service, CLIENT, CLIENT_TYPE_PATH],
            )?;
            Patched::Updated { csreq_added }
        }
        None => {
            let req = csreq()?;
            // Every column without a default must get a value: an unknown
            // one means a schema that paddock does not know.
            let known = [
                "service",
                "client",
                "client_type",
                "auth_value",
                "auth_reason",
                "auth_version",
                "csreq",
                "flags",
                "last_modified",
                "indirect_object_identifier",
                "boot_uuid",
                "last_reminded",
            ];
            if let Some((name, _)) = columns
                .iter()
                .find(|(n, required)| *required && !known.contains(&n.as_str()))
            {
                return Err(anyhow!(text::tcc_schema_unknown(db, name)));
            }
            let has = |n: &str| columns.iter().any(|(c, _)| c == n);
            let mut names = vec![
                "service",
                "client",
                "client_type",
                "auth_value",
                "auth_reason",
                "auth_version",
                "csreq",
            ];
            let mut values: Vec<rusqlite::types::Value> = vec![
                service.to_string().into(),
                CLIENT.to_string().into(),
                CLIENT_TYPE_PATH.into(),
                ALLOWED.into(),
                REASON_USER_SET.into(),
                1i64.into(),
                req.into(),
            ];
            for (n, v) in [
                ("flags", rusqlite::types::Value::from(0i64)),
                ("last_modified", now.into()),
                ("indirect_object_identifier", "UNUSED".to_string().into()),
                ("boot_uuid", "UNUSED".to_string().into()),
                ("last_reminded", now.into()),
            ] {
                if has(n) {
                    names.push(n);
                    values.push(v);
                }
            }
            let marks: Vec<String> = (1..=names.len()).map(|i| format!("?{i}")).collect(); // not user text
            let sql = format!(
                // not user text
                "INSERT INTO access ({}) VALUES ({})",
                names.join(", "),
                marks.join(", ")
            );
            tx.execute(&sql, rusqlite::params_from_iter(values))?;
            Patched::Inserted
        }
    };
    Ok(patched)
}

/// The columns of `access` that a row of paddock needs.
const NEEDED: [&str; 7] = [
    "service",
    "client",
    "client_type",
    "auth_value",
    "auth_reason",
    "auth_version",
    "csreq",
];

/// Set the agent's Accessibility row of the database `db` to allowed.
/// `csreq` gives the code requirement for a row that has none or is new.
pub fn allow(db: &Path, csreq: impl FnOnce() -> Result<Vec<u8>>) -> Result<Patched> {
    allow_in(db, SERVICES, csreq)
}

/// [`allow`] for the agent's rows of `services`. The report names the
/// first.
pub fn allow_in(
    db: &Path,
    services: &[&str],
    csreq: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Patched> {
    if !db.is_file() {
        return Err(anyhow!(text::tcc_db_missing(db)));
    }
    let before: Vec<bool> = side_files(db).iter().map(|p| p.exists()).collect();
    let r = allow_open(db, services, csreq);
    // A side file that this write made would stay with the owner 99:99.
    let mut left = Vec::new();
    for (p, was) in side_files(db).iter().zip(before) {
        if !was && p.exists() {
            let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            if p.to_string_lossy().ends_with("-wal") && len > 0 {
                left.push(p.clone());
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let patched = r?;
    if let Some(p) = left.first() {
        return Err(anyhow!(text::tcc_wal_left(p)));
    }
    Ok(patched)
}

fn allow_open(
    db: &Path,
    services: &[&str],
    csreq: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Patched> {
    let mut c = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| text::cannot_open(db))?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    let columns = columns(&c)?;
    if let Some(missing) = NEEDED
        .iter()
        .find(|n| !columns.iter().any(|(c, _)| c == *n))
    {
        return Err(anyhow!(text::tcc_schema_unknown(db, missing)));
    }
    let tx = c.transaction()?;
    // One requirement for every row: the one that macOS recorded in the
    // row of the first service, else made once, only when a row needs it.
    let mut made: Option<Vec<u8>> = tx
        .query_row(
            "SELECT csreq FROM access WHERE service = ?1 AND client = ?2 AND client_type = ?3",
            params![services[0], CLIENT, CLIENT_TYPE_PATH],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten()
        .filter(|b| !b.is_empty());
    let mut csreq = Some(csreq);
    let mut req = || -> Result<Vec<u8>> {
        if made.is_none() {
            made = Some((csreq.take().expect("called once"))()?);
        }
        Ok(made.clone().unwrap_or_default())
    };
    let mut patched = Patched::Already;
    for (i, service) in services.iter().enumerate() {
        let p = allow_service(&tx, db, &columns, service, &mut req)?;
        // The report names the row of the first service.
        if i == 0 {
            patched = p;
        }
    }
    tx.commit()?;
    let check: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(anyhow!(text::tcc_integrity(db, &check)));
    }
    let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    }
    c.close().map_err(|(_, e)| e)?;
    Ok(patched)
}

/// The columns of `access`: the name, and whether a value is required
/// (NOT NULL without a default and not part of a key with a default).
fn columns(c: &Connection) -> Result<Vec<(String, bool)>> {
    let mut st = c.prepare("PRAGMA table_info(access)")?;
    let rows = st.query_map([], |r| {
        let name: String = r.get(1)?;
        let notnull: i64 = r.get(3)?;
        let default: Option<String> = r.get(4)?;
        Ok((name, notnull != 0 && default.is_none()))
    })?;
    let out: Vec<(String, bool)> = rows.collect::<rusqlite::Result<_>>()?;
    if out.is_empty() {
        return Err(anyhow!(text::tcc_no_access_table()));
    }
    Ok(out)
}

/// The Accessibility row of the agent as it is now: `auth_value` and
/// `auth_reason`, or `None`.
pub fn row(db: &Path) -> Result<Option<(i64, i64)>> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| text::cannot_open(db))?;
    Ok(c.query_row(
        "SELECT auth_value, auth_reason FROM access WHERE service = ?1 AND client = ?2 AND client_type = ?3",
        params![ACCESSIBILITY, CLIENT, CLIENT_TYPE_PATH],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

/// The requirement in the agent's Accessibility row.
fn requirement_of(db: &Path) -> Result<Vec<u8>> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| text::cannot_open(db))?;
    c.query_row(
        "SELECT csreq FROM access WHERE service = ?1 AND client = ?2 AND client_type = ?3",
        params![ACCESSIBILITY, CLIENT, CLIENT_TYPE_PATH],
        |r| r.get::<_, Option<Vec<u8>>>(0),
    )
    .optional()?
    .flatten()
    .filter(|b| !b.is_empty())
    .ok_or_else(|| anyhow!(text::tcc_db_missing(db)))
}

/// The offline trust of `disk`: attach, mount the Data volume without
/// owners under `mnt`, allow the row, detach. The detach runs whatever the
/// patch did.
pub fn trust_disk(
    tool: &mut dyn DiskTool,
    disk: &Path,
    mnt: &Path,
    csreq: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Patched> {
    tool.detach_stale(mnt);
    let a: Attached = tool.attach(disk, mnt)?;
    let r = allow(&a.data.join(DB), csreq).and_then(|p| {
        // The user's database exists after the first login of the golden.
        // Its rows take the requirement of the agent's system rows.
        let user = a.data.join(USER_DB);
        if user.is_file() {
            let req = requirement_of(&a.data.join(DB))?;
            allow_in(&user, USER_SERVICES, || Ok(req))?;
        }
        Ok(p)
    });
    let d = tool.detach(&a);
    super::diskimage::remove_mount_root(mnt);
    let patched = r?;
    d?;
    Ok(patched)
}

#[cfg(test)]
#[path = "tcc_tests.rs"]
mod tests;
