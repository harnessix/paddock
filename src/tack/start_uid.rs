//! The protection part of a start (Linux): the level, the helper account,
//! the wrapper folder and the ACL grant. `protect` makes the plan; this
//! file does what the plan needs on the system.

use super::acl::{GrantReport, Ledger};
use super::caller::Caller;
use super::config::Tack;
use super::protect::{self, Grant, Ids, Level, PlanInput, Registry, StartPlan};
use super::proto::{Decision, Start};
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long a grant may take before the start fails.
pub const GRANT_TIMEOUT: Duration = Duration::from_secs(100);

/// Runs an ACL grant as the user.
pub trait Granter: Send + Sync {
    fn grant(&self, g: &Grant, as_user: &Ids) -> Result<GrantReport, String>;
}

/// The real one: `paddock tack acl-grant` of tackd's own binary, as the
/// user, with the grant on its standard input.
pub struct AsUser {
    pub binary: PathBuf,
}

impl Granter for AsUser {
    fn grant(&self, g: &Grant, ids: &Ids) -> Result<GrantReport, String> {
        let uid = ids.uid;
        let gid = ids.gid;
        let groups: Vec<libc::gid_t> = ids.groups.clone();
        // SAFETY: geteuid cannot fail.
        let drop_user = unsafe { libc::geteuid() } != uid;
        let mut cmd = Command::new(&self.binary);
        cmd.args(["tack", "acl-grant"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: async-signal-safe calls on memory made before fork.
        unsafe {
            cmd.pre_exec(move || {
                if drop_user {
                    if libc::setgroups(groups.len() as _, groups.as_ptr()) != 0
                        || libc::setresgid(gid, gid, gid) != 0
                        || libc::setresuid(uid, uid, uid) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::geteuid() != uid || libc::getuid() != uid {
                        return Err(io::Error::from_raw_os_error(libc::EPERM));
                    }
                }
                Ok(())
            });
        }
        let mut child = crate::env::spawn_retrying(&mut cmd)
            .map_err(|e| format!("cannot run {} tack acl-grant: {e}", self.binary.display()))?;
        let input = serde_json::to_vec(g).map_err(|e| e.to_string())?;
        if let Some(mut i) = child.stdin.take() {
            let _ = i.write_all(&input);
        }
        let pid = child.id() as libc::pid_t;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let out = match rx.recv_timeout(GRANT_TIMEOUT) {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(format!("the ACL grant failed: {e}")),
            Err(_) => {
                // SAFETY: kill takes plain integers; the thread reaps it.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                return Err(format!(
                    "the ACL grant for {} took longer than {GRANT_TIMEOUT:?}: start the program in a smaller folder, or set protect = \"refs\" in its entry",
                    g.folder.display()
                ));
            }
        };
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if msg.is_empty() {
                format!("the ACL grant failed ({})", out.status)
            } else {
                msg
            });
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("the ACL grant answered {e}"))
    }
}

/// What tackd needs for `protect = "uid"`.
pub struct Protection {
    /// tackd's state folder: the registry, the ledger, the homes and the
    /// wrapper folders.
    pub state: PathBuf,
    /// The owner of that folder: root, or the test user.
    pub owner: u32,
    /// The binary that the wrapper links point to: tackd's own.
    pub binary: Option<PathBuf>,
    pub granter: Box<dyn Granter>,
    /// Check each helper against the passwd database (off in tests with
    /// made-up helpers).
    pub check_accounts: bool,
}

impl Protection {
    pub fn system(state: PathBuf) -> Self {
        // After an update renamed a new copy over it, the kernel names
        // the running file "PATH (deleted)": the links point to PATH.
        let binary = std::env::current_exe().ok().map(|p| {
            let s = p.to_string_lossy();
            match s.strip_suffix(" (deleted)") {
                Some(x) => PathBuf::from(x),
                None => p,
            }
        });
        Protection {
            state,
            owner: 0,
            granter: Box::new(AsUser {
                binary: binary
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("/proc/self/exe")),
            }),
            binary,
            check_accounts: true,
        }
    }

    pub fn registry(&self) -> Result<Registry, String> {
        Registry::load(&self.state, self.owner)
    }

    /// The plan of one start, with what it took: the wrapper folder and the
    /// ACL grant are in place when it returns. The notes go to the audit.
    pub fn prepare(
        &self,
        peer_uid: u32,
        c: &Caller,
        req: &Start,
        d: &Decision,
        cfg: &Tack,
        reg: &Result<Registry, String>,
    ) -> Result<(StartPlan, Vec<String>), String> {
        let empty = Registry::default();
        let r = reg.as_ref().unwrap_or(&empty);
        let nested = r.of_helper(peer_uid).cloned();
        let owner = nested.as_ref().map(|h| h.owner).unwrap_or(peer_uid);
        let p = d
            .program
            .as_ref()
            .ok_or("the decision names no program file")?;
        let eff = protect::effective(cfg, &d.secrets, Path::new(&p.path), Path::new(&p.real));
        let level = if nested.is_some() {
            Level::Uid
        } else {
            eff.level
        };
        let helper = nested.clone().or_else(|| r.of_owner(owner).cloned());
        let mut notes = Vec::new();
        if level == Level::Uid {
            if let Err(e) = reg {
                return Err(format!("its entry has protect = \"uid\" and {e}"));
            }
            if let (Some(h), true) = (&helper, self.check_accounts) {
                super::accounts::check(h)?;
            }
        }
        let names: Vec<String> = cfg.secrets.keys().cloned().collect();
        let mut listed: Vec<String> = cfg
            .secrets
            .values()
            .flat_map(|s| s.allow.iter())
            .map(|a| super::program::real_path(&a.program).display().to_string())
            .collect();
        listed.sort();
        listed.dedup();
        let env = protect::unwrap_env(&req.env);
        let wrap = match (level, &self.binary) {
            (Level::Uid, Some(bin)) => {
                let path = env
                    .iter()
                    .find_map(|e| e.strip_prefix("PATH="))
                    .unwrap_or("/usr/local/bin:/usr/bin:/bin");
                let skip: Vec<PathBuf> = super::accounts::by_uid(owner)
                    .map(|pw| vec![pw.home.join(".local/share/paddock/tack/bin")])
                    .unwrap_or_default();
                match super::wrap::ensure(&self.state.join("wrap"), self.owner, path, bin, &skip) {
                    Ok(w) => Some(w),
                    Err(e) => {
                        notes.push(format!(
                            "no exec wrapper ({e}): the programs that it starts get the values"
                        ));
                        None
                    }
                }
            }
            _ => None,
        };
        let ids = Ids {
            uid: c.uid,
            gid: c.gid,
            groups: c.groups.clone(),
        };
        let plan = protect::plan(&PlanInput {
            level,
            write: eff.write,
            caller: &ids,
            helper: helper.as_ref(),
            nested: nested.is_some(),
            cwd: Path::new(&req.cwd),
            env: &req.env,
            wrap: wrap.as_deref(),
            names: &names,
            listed: &listed,
        })?;
        if nested.is_some() {
            notes.push("started by a program at protect = uid, so it runs as the same helper and gets no new folder".into());
        }
        // A folder that the user does not own: the user could not change
        // its ACL anyway, and the ledger holds only the user's folders.
        let mut plan = plan;
        if let Some(g) = &plan.grant {
            use std::os::unix::fs::MetadataExt;
            let owner_of = std::fs::metadata(&g.folder).map(|m| m.uid());
            if owner_of.as_ref().ok() != Some(&g.owner) {
                notes.push(format!(
                    "{} is not your folder: no ACL, the helper reaches it as any user does",
                    g.folder.display()
                ));
                plan.grant = None;
            }
        }
        if let Some(g) = &plan.grant {
            Ledger::new(&self.state).add(g).map_err(|e| {
                format!("cannot record the ACL grant in tackd's ledger ({e}), so it grants nothing")
            })?;
            let rep = self.granter.grant(g, &ids)?;
            notes.push(format!(
                "{} {}: {}",
                if g.write {
                    "read and write in"
                } else {
                    "reads"
                },
                g.folder.display(),
                rep.summary()
            ));
            for e in rep.errors.iter().take(2) {
                notes.push(format!("ACL: {e}"));
            }
        }
        Ok((plan, notes))
    }
}

#[cfg(test)]
#[path = "start_uid_tests.rs"]
mod tests;
