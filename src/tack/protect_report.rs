//! The protection lines of `paddock tack status`, `paddock doctor` and
//! `paddock explain`: per allowlist entry its level, and whether the
//! helper account and POSIX ACLs work on this host.

use super::config::Tack;
use super::protect::{self, Helper, Level, Registry, REFS_LIMIT, UID_GAIN};
use std::path::Path;

/// What this host has for `protect = "uid"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// This system can run programs at `uid` (Linux).
    pub supported: bool,
    /// The user's helper account: Ok(None) when setup did not make one.
    pub helper: Result<Option<Helper>, String>,
    /// Whether an ACL entry stays on a file of the user.
    pub acl: Result<(), String>,
    /// Where the ACL was tried.
    pub acl_at: String,
}

/// Look at this host for the user `uid`, trying ACLs in `probe_dir`.
pub fn host(state: &Path, uid: u32, probe_dir: &Path, shown: String) -> Host {
    let supported = cfg!(target_os = "linux");
    let helper = Registry::load(state, 0).and_then(|r| match r.of_owner(uid).cloned() {
        None => Ok(None),
        Some(h) => super::accounts::check(&h).map(|_| Some(h)),
    });
    let probe_uid = helper
        .as_ref()
        .ok()
        .and_then(|o| o.as_ref())
        .map(|h| h.uid)
        .unwrap_or(uid);
    let acl = probe(probe_dir, probe_uid);
    Host {
        supported,
        helper,
        acl,
        acl_at: shown,
    }
}

#[cfg(target_os = "linux")]
fn probe(dir: &Path, uid: u32) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    super::acl::probe(dir, uid)
}

#[cfg(not(target_os = "linux"))]
fn probe(_: &Path, _: u32) -> Result<(), String> {
    Err("not checked on this system".into())
}

/// One line per allowlist entry: its level, where the level comes from,
/// and what it means.
pub fn entry_lines(cfg: &Tack, helper: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, s) in &cfg.secrets {
        for a in &s.allow {
            let p = a.program.display();
            let from = protect::source(a);
            out.push(match a.level() {
                Level::Uid => format!(
                    "  uid      {name}  {p}: protect = \"uid\" ({from}), {}: runs as {}; {UID_GAIN}",
                    if a.write { "reads and writes the working folder" } else { "reads the working folder" },
                    helper.unwrap_or("your helper account")
                ),
                Level::Refs => format!(
                    "  refs     {name}  {p}: protect = \"refs\" ({from}): runs as you; {REFS_LIMIT}"
                ),
            });
        }
    }
    out
}

/// The status and doctor lines: the entries, then the host checks when
/// an entry needs them.
pub fn lines(cfg: &Tack, h: &Host) -> Vec<String> {
    let name = h
        .helper
        .as_ref()
        .ok()
        .and_then(|o| o.as_ref())
        .map(|x| x.name.as_str());
    let mut out = entry_lines(cfg, name);
    if !protect::wants_uid(cfg) {
        return out;
    }
    if !h.supported {
        out.push("  note     protect = \"uid\" is for Linux only in this version: on this system tackd starts no program, and every process sees the references".into());
        return out;
    }
    match &h.helper {
        Ok(Some(x)) => out.push(format!(
            "  ok       helper account {} (uid {}) is there: programs at protect = \"uid\" run as it",
            x.name, x.uid
        )),
        Ok(None) => out.push("  missing  you have no helper account yet, so a program at protect = \"uid\" does not start: run sudo paddock tack setup (or set protect = \"refs\" in its entry)".into()),
        Err(e) => out.push(format!("  error    helper account: {e}")),
    }
    match &h.acl {
        Ok(()) => out.push(format!(
            "  ok       POSIX ACLs work in {}: the helper can be given your working folder",
            h.acl_at
        )),
        Err(e) => out.push(format!(
            "  error    POSIX ACLs do not work in {} ({e}): a program at protect = \"uid\" cannot reach your folders there",
            h.acl_at
        )),
    }
    out
}

/// The lines for the user of `env`, on this host.
pub fn lines_for(env: &crate::env::HostEnv) -> Vec<String> {
    let cfg = match super::config::load(env) {
        Ok(c) => c,
        Err(e) => return vec![format!("  error    {e:#}")],
    };
    if cfg.secrets.values().all(|s| s.allow.is_empty()) {
        return Vec::new();
    }
    let dir = super::state_dir(env);
    let h = host(
        Path::new(super::STATE_DIR),
        env.uid,
        &dir,
        super::show(env, &dir),
    );
    lines(&cfg, &h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Tack {
        super::super::config::parse(text).unwrap()
    }

    const TWO: &str = "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"/usr/bin/gh\"\n[tack.secrets.NPM_TOKEN]\n[[tack.secrets.NPM_TOKEN.allow]]\nprogram = \"/usr/bin/npm\"\n";

    fn helper() -> Helper {
        Helper {
            owner: 1000,
            name: "_tack_alice".into(),
            uid: 981,
            gid: 981,
            home: "/var/lib/paddock-tackd/home/_tack_alice".into(),
        }
    }

    #[test]
    fn each_entry_says_its_level_and_what_refs_does_not_protect() {
        let h = Host {
            supported: true,
            helper: Ok(Some(helper())),
            acl: Ok(()),
            acl_at: "~/.local/state/paddock/tack".into(),
        };
        let l = lines(&cfg(TWO), &h);
        assert!(l[0].starts_with("  uid      GH_TOKEN  /usr/bin/gh: protect = \"uid\" (the default of the built-in profile for gh), reads the working folder: runs as _tack_alice;"), "{l:?}");
        assert!(l[1].contains("NPM_TOKEN  /usr/bin/npm: protect = \"refs\" (the default): runs as you; other processes of your user can still read this program's environment"), "{l:?}");
        assert!(
            l[2].contains("ok       helper account _tack_alice (uid 981)"),
            "{l:?}"
        );
        assert!(
            l[3].contains("ok       POSIX ACLs work in ~/.local/state/paddock/tack"),
            "{l:?}"
        );
        assert_eq!(l.len(), 4);
    }

    #[test]
    fn a_missing_helper_or_acl_support_is_named_with_the_fix() {
        let h = Host {
            supported: true,
            helper: Ok(None),
            acl: Err("Operation not supported".into()),
            acl_at: "~/x".into(),
        };
        let l = lines(&cfg(TWO), &h).join("\n");
        assert!(
            l.contains("missing  you have no helper account yet")
                && l.contains("sudo paddock tack setup"),
            "{l}"
        );
        assert!(
            l.contains("error    POSIX ACLs do not work in ~/x (Operation not supported)"),
            "{l}"
        );
        let mac = Host {
            supported: false,
            ..h
        };
        let l = lines(&cfg(TWO), &mac).join("\n");
        assert!(
            l.contains("Linux only") && !l.contains("helper account yet"),
            "{l}"
        );
    }

    #[test]
    fn only_refs_entries_need_no_host_check() {
        let h = Host {
            supported: true,
            helper: Ok(None),
            acl: Err("x".into()),
            acl_at: "~/x".into(),
        };
        let l = lines(
            &cfg("[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/npm\"\n"),
            &h,
        );
        assert_eq!(l.len(), 1, "{l:?}");
        let w = lines(&cfg("[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/tool\"\nprotect = \"uid\"\nwrite = true\n"), &h);
        assert!(
            w[0].contains("(set in its entry), reads and writes the working folder"),
            "{w:?}"
        );
    }
}
