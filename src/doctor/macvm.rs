//! The `macvm` block of `paddock doctor` on macOS: the golden and its seal,
//! the host, the entitlement of `paddock-vm`, the profiles, the sessions and
//! the free space. The lines come from a [`View`] that the tests build by
//! hand.

use crate::env::HostEnv;
use crate::macvm::golden::{self, Manifest, Prev};
use crate::macvm::host;
use crate::macvm::layout::Layout;
use crate::macvm::lock;
use crate::macvm::manage::{self, GoldenView};
use crate::macvm::security;
use crate::macvm::text::{self, Holder, SessionEnd};
use std::path::Path;

/// The free space that a cold start of a profile needs.
pub const COLD_START_BYTES: u64 = crate::macvm::session::CLONE_NEEDS;
/// The entitlement that the Virtualization framework needs.
pub const ENTITLEMENT: &str = "com.apple.security.virtualization";

/// What the block is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub golden: GoldenView,
    /// `sw_vers -productVersion` of the host.
    pub host_version: Option<String>,
    pub apple_silicon: bool,
    /// Whether `paddock-vm` carries the entitlement: `None` when the helper
    /// is not there.
    pub entitlement: Option<bool>,
    pub profiles: Vec<ProfileView>,
    pub sessions: Vec<(String, crate::macvm::host::Session)>,
    /// The two VM slots of the Mac and their holders.
    pub slots: Vec<Option<Holder>>,
    /// The mode of the slots folder, `None` before the first start
    /// (checklist item 16).
    pub slots_dir: Option<u32>,
    /// The free bytes of the volume of the `macvm` folder.
    pub free: u64,
    /// The previous golden, kept for a rollback or ready for `gc`.
    pub prev: Prev,
}

/// One profile in the block: its size, how its last session ended and
/// when, or the pid of the session that runs it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileView {
    /// `app/home`.
    pub name: String,
    pub bytes: u64,
    pub end: SessionEnd,
    /// The day of the last stamp write, `2026-11-03`.
    pub when: Option<String>,
    pub running: Option<u32>,
    /// The files of the profile that are not at their mode (checklist
    /// item 5, `security::mode_issues`).
    pub modes: Vec<String>,
}

impl ProfileView {
    /// The words in brackets after the size.
    pub fn note(&self) -> String {
        match (self.running, self.end, &self.when) {
            (Some(pid), _, _) => format!("running, pid {pid}"),
            (None, SessionEnd::QuitWithSave | SessionEnd::QuitWithoutSave, _) => "clean".into(),
            (None, SessionEnd::NeverStarted, _) => "never started".into(),
            (None, end, Some(day)) => format!("{} {day}", text::session_end(end)),
            (None, end, None) => text::session_end(end).into(),
        }
    }
}

impl View {
    /// Read the Mac. The entitlement comes from `codesign`.
    pub fn read(env: &HostEnv, host_version: Option<&str>) -> Self {
        let layout = Layout::new(env);
        let entitlement = crate::vm::host::helper_path(env)
            .ok()
            .map(|p| helper_entitled(&p));
        let sessions = manage::running_sessions(&layout);
        let profiles = layout
            .profile_dirs()
            .iter()
            .map(|(a, h, d)| {
                let profile = layout.profile(a, h);
                let stamp = crate::macvm::session::Stamp::read(&profile.stamp()).ok();
                ProfileView {
                    name: format!("{a}/{h}"),
                    bytes: golden::used_bytes_in(d),
                    end: host::end_of(stamp.as_ref()),
                    when: stamp.as_ref().and_then(|s| s.saved.as_deref()).map(day_of),
                    running: sessions
                        .iter()
                        .find(|(_, s)| s.app == *a && s.home == *h)
                        .map(|(_, s)| s.pid),
                    modes: security::mode_issues(&profile),
                }
            })
            .collect();
        Self {
            golden: GoldenView::read(&layout),
            host_version: host_version.map(str::to_string),
            apple_silicon: cfg!(target_arch = "aarch64"),
            entitlement,
            profiles,
            sessions,
            slots: lock::holders(Path::new(lock::SLOT_DIR), &env.user),
            slots_dir: security::slots_dir_mode(Path::new(lock::SLOT_DIR)),
            free: golden::free_bytes(&layout.root, env.var("PADDOCK_MACVM_FAKE_FREE")).unwrap_or(0),
            prev: golden::prev_state(&layout),
        }
    }
}

/// `2026-11-03` of `2026-11-03T10:00:00Z`.
fn day_of(time: &str) -> String {
    time.split('T').next().unwrap_or(time).to_string()
}

/// `codesign -d --entitlements - PATH` names the entitlement.
fn helper_entitled(helper: &Path) -> bool {
    std::process::Command::new("/usr/bin/codesign")
        .args(["-d", "--entitlements", "-", "--xml"])
        .arg(helper)
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| {
            let text = String::from_utf8_lossy(&o.stdout) + String::from_utf8_lossy(&o.stderr);
            text.contains(ENTITLEMENT)
        })
}

/// The lines of the block, with their first word `ok`, `warn` or `no`.
pub fn lines(v: &View) -> Vec<String> {
    let mut out = vec!["macvm:".to_string()];
    match &v.golden {
        GoldenView::Missing => {
            out.push("  no       golden: none yet. paddock macvm setup builds it (15 GB download, 25 GB on disk)".into())
        }
        GoldenView::Unreadable(e) => out.push(format!("  no       golden: {e}")),
        GoldenView::Unsealed(m) => out.push(format!(
            "  warn     golden macOS {}: not sealed, setup stopped in step {} ({}). paddock macvm setup continues it",
            m.version_text(),
            m.state.number(),
            m.state.name()
        )),
        GoldenView::Sealed {
            manifest,
            size,
            flags_ok,
        } => {
            let date = manifest
                .sealed
                .as_deref()
                .map(|s| s.split('T').next().unwrap_or(s).to_string())
                .unwrap_or_else(|| "?".into());
            if *flags_ok {
                out.push(format!(
                    "  ok       golden macOS {}, sealed {date}, {}, flags 0444 uchg",
                    manifest.version_text(),
                    text::gib(*size)
                ));
            } else {
                out.push(format!(
                    "  warn     golden macOS {}, sealed {date}, {}: the images lost their 0444 uchg seal. paddock macvm update rebuilds the golden",
                    manifest.version_text(),
                    text::gib(*size)
                ));
            }
        }
    }
    let host = v.host_version.as_deref().unwrap_or("?");
    let silicon = if v.apple_silicon {
        "Apple silicon"
    } else {
        "not Apple silicon: the macOS guest needs an M series Mac"
    };
    let ent = match v.entitlement {
        Some(true) => "Virtualization entitlement on paddock-vm".to_string(),
        Some(false) => format!("paddock-vm lacks the {ENTITLEMENT} entitlement: sign it with packaging/macos/paddock-vm.entitlements"),
        None => "paddock-vm is not installed next to paddock".to_string(),
    };
    let host_ok = v.apple_silicon && v.entitlement == Some(true);
    out.push(format!(
        "  {:<8} host macOS {host}, {silicon}, {ent}",
        if host_ok { "ok" } else { "no" }
    ));
    if let (Some(m), Some(h)) = (golden_manifest(&v.golden), v.host_version.as_deref()) {
        if golden::version_older(&m.version, h) {
            out.push(format!(
                "  warn     golden older than the host ({} < {h}): paddock macvm update",
                m.version
            ));
        }
    }
    match &v.prev {
        Prev::None => {}
        Prev::Kept { version } => out.push(format!(
            "  ok       golden.prev macOS {version}, kept for paddock macvm rollback until a profile completes a session on the new golden"
        )),
        Prev::Removable { version } => out.push(format!(
            "  ok       golden.prev macOS {version}: a profile completed a session on the new golden, paddock gc removes it"
        )),
    }
    if v.profiles.is_empty() {
        out.push("  ok       profiles: none".into());
    } else {
        let list: Vec<String> = v
            .profiles
            .iter()
            .map(|p| format!("{} {} ({})", p.name, text::gib(p.bytes), p.note()))
            .collect();
        out.push(format!("  ok       profiles: {}", list.join(", ")));
        let wrong: Vec<String> = v
            .profiles
            .iter()
            .flat_map(|p| p.modes.iter().map(move |m| format!("{} {m}", p.name)))
            .collect();
        if wrong.is_empty() {
            out.push(
                "  ok       profile modes: folders 0700, disks, saved states and backups 0600"
                    .into(),
            );
        } else {
            out.push(format!(
                "  warn     profile modes: {}: chmod 700 the folders and 600 the files, or paddock app reset NAME --all",
                wrong.join(", ")
            ));
        }
    }
    let used: Vec<String> = v.slots.iter().flatten().map(text::holder_short).collect();
    let total = v.slots.len().max(lock::SLOTS);
    let holders = if used.is_empty() {
        String::new()
    } else {
        format!(" ({})", used.join(", "))
    };
    if used.len() >= total {
        out.push(format!(
            "  warn     slots: {} of {total} in use{holders}: a third session is refused until one stops (paddock app stop NAME)",
            used.len()
        ));
    } else {
        out.push(format!(
            "  ok       slots: {} of {total} in use{holders}",
            used.len()
        ));
    }
    match v.slots_dir {
        None => out.push(format!(
            "  ok       slots folder {}: not made yet, the first start makes it drwxrwxrwt",
            lock::SLOT_DIR
        )),
        Some(mode) if mode == security::SLOTS_DIR_MODE => out.push(format!(
            "  ok       slots folder {}: drwxrwxrwt, slot files 0666",
            lock::SLOT_DIR
        )),
        Some(mode) => out.push(format!(
            "  warn     slots folder {}: {}, so the sessions of another user cannot take a slot: sudo chmod 1777 {}",
            lock::SLOT_DIR,
            security::ls_mode(mode),
            lock::SLOT_DIR
        )),
    }
    out.push("  ok       no host permission needed (Accessibility: not needed, Screen Recording: not needed)".into());
    if v.free < COLD_START_BYTES {
        out.push(format!(
            "  warn     disk: {} free, a cold start needs {}",
            text::gb(v.free),
            text::gb(COLD_START_BYTES)
        ));
    } else {
        out.push(format!("  ok       disk: {} free", text::gb(v.free)));
    }
    out
}

fn golden_manifest(g: &GoldenView) -> Option<&Manifest> {
    g.manifest()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macvm::golden::State;

    fn sealed(version: &str) -> GoldenView {
        let mut m = Manifest::new("15.2", "24C101", "0.2.1");
        m.version = version.into();
        m.build = "24C101".into();
        m.sealed = Some("2026-11-01T10:00:00Z".into());
        m.at(State::Sealed);
        GoldenView::Sealed {
            manifest: m,
            size: 23_400_000_000,
            flags_ok: true,
        }
    }

    fn view(golden: GoldenView) -> View {
        View {
            golden,
            host_version: Some("15.2".into()),
            apple_silicon: true,
            entitlement: Some(true),
            profiles: vec![
                ProfileView {
                    name: "chatgpt-desktop/default".into(),
                    bytes: 6_300_000_000,
                    end: SessionEnd::QuitWithSave,
                    when: Some("2026-11-04".into()),
                    running: None,
                    modes: vec![],
                },
                ProfileView {
                    name: "vscode/default".into(),
                    bytes: 9_100_000_000,
                    end: SessionEnd::Crashed,
                    when: Some("2026-11-03".into()),
                    running: None,
                    modes: vec![],
                },
            ],
            sessions: vec![],
            slots: vec![None, None],
            slots_dir: Some(security::SLOTS_DIR_MODE),
            free: 120 << 30,
            prev: Prev::None,
        }
    }

    #[test]
    fn macvm_block_cases() {
        let l = lines(&view(sealed("15.2")));
        assert_eq!(l[0], "macvm:");
        assert!(l[1].starts_with("  ok       golden macOS 15.2 (24C101), sealed 2026-11-01, 21.8 GiB, flags 0444 uchg"), "{}", l[1]);
        assert!(l[2].starts_with("  ok       host macOS 15.2, Apple silicon, Virtualization entitlement on paddock-vm"), "{}", l[2]);
        assert!(
            l.iter()
                .any(|x| x == "  ok       profiles: chatgpt-desktop/default 5.9 GiB (clean), vscode/default 8.5 GiB (crashed 2026-11-03)"),
            "{l:?}"
        );
        assert!(
            l.iter().any(|x| x == "  ok       slots: 0 of 2 in use"),
            "{l:?}"
        );

        // The profile notes: running, never started, crashed without a day.
        let mut v = view(sealed("15.2"));
        v.profiles[0].running = Some(812);
        v.profiles[1] = ProfileView {
            name: "x/default".into(),
            bytes: 0,
            end: SessionEnd::NeverStarted,
            when: None,
            running: None,
            modes: vec![],
        };
        v.profiles.push(ProfileView {
            name: "y/default".into(),
            bytes: 1 << 30,
            end: SessionEnd::Crashed,
            when: None,
            running: None,
            modes: vec![],
        });
        let l = lines(&v);
        assert!(
            l.iter().any(|x| x
                == "  ok       profiles: chatgpt-desktop/default 5.9 GiB (running, pid 812), x/default 0 GiB (never started), y/default 1 GiB (crashed)"),
            "{l:?}"
        );
        assert_eq!(day_of("2026-11-03T10:00:00Z"), "2026-11-03");

        // The slots: one of two, then both, with their holders.
        let mut v = view(sealed("15.2"));
        v.slots = vec![
            Some(Holder::Mine {
                app: "chatgpt-desktop".into(),
                pid: 812,
            }),
            None,
        ];
        let l = lines(&v);
        assert!(
            l.iter()
                .any(|x| x == "  ok       slots: 1 of 2 in use (chatgpt-desktop (you, pid 812))"),
            "{l:?}"
        );
        v.slots[1] = Some(Holder::User { name: "ana".into() });
        let l = lines(&v);
        assert!(
            l.iter().any(|x| x.starts_with(
                "  warn     slots: 2 of 2 in use (chatgpt-desktop (you, pid 812), a VM of user ana): a third session is refused"
            )),
            "{l:?}"
        );
        assert!(l.iter().any(|x| x.contains("no host permission needed")));
        assert!(
            l.iter().any(|x| x
                == "  ok       profile modes: folders 0700, disks, saved states and backups 0600"),
            "{l:?}"
        );
        assert!(
            l.iter().any(|x| x
                .starts_with("  ok       slots folder /private/tmp/dev.paddock.macvm: drwxrwxrwt")),
            "{l:?}"
        );
        v.profiles[0].modes = vec!["data.img 0644 (expected 0600)".into()];
        let l = lines(&v);
        assert!(l.iter().any(|x| x.starts_with("  warn     profile modes: chatgpt-desktop/default data.img 0644 (expected 0600): chmod")), "{l:?}");
        assert!(
            l.iter().any(|x| x == "  ok       disk: 129 GB free"),
            "{l:?}"
        );
        assert!(!l.iter().any(|x| x.contains("older than the host")));

        let mut m = Manifest::new("15.2", "24C101", "0.2.1");
        m.version = "15.2".into();
        m.at(State::Guided);
        let l = lines(&view(GoldenView::Unsealed(m)));
        assert!(
            l[1].starts_with(
                "  warn     golden macOS 15.2: not sealed, setup stopped in step 4 (guided)"
            ),
            "{}",
            l[1]
        );
        assert!(l[1].contains("paddock macvm setup"));

        let l = lines(&view(GoldenView::Missing));
        assert!(
            l[1].starts_with("  no       golden: none yet. paddock macvm setup"),
            "{}",
            l[1]
        );

        let l = lines(&view(GoldenView::Unreadable(
            "schema 9: paddock macvm remove".into(),
        )));
        assert!(l[1].contains("paddock macvm remove"));

        // A golden older than the host.
        let mut v = view(sealed("15.1"));
        v.host_version = Some("15.2".into());
        let l = lines(&v);
        assert!(
            l.iter().any(|x| x
                == "  warn     golden older than the host (15.1 < 15.2): paddock macvm update"),
            "{l:?}"
        );

        // A lost seal, a missing entitlement, little space, no silicon.
        let mut v = view(sealed("15.2"));
        if let GoldenView::Sealed { flags_ok, .. } = &mut v.golden {
            *flags_ok = false;
        }
        v.entitlement = Some(false);
        v.apple_silicon = false;
        v.free = 8_000_000_000;
        let l = lines(&v);
        assert!(
            l[1].starts_with("  warn     golden") && l[1].contains("lost their 0444 uchg seal"),
            "{}",
            l[1]
        );
        assert!(
            l[2].starts_with("  no       host")
                && l[2].contains(ENTITLEMENT)
                && l[2].contains("M series"),
            "{}",
            l[2]
        );
        assert!(
            l.iter()
                .any(|x| x == "  warn     disk: 8 GB free, a cold start needs 9 GB"),
            "{l:?}"
        );
        let mut v = view(sealed("15.2"));
        v.entitlement = None;
        assert!(lines(&v)[2].contains("not installed"));

        // The previous golden: kept, then removable.
        let mut v = view(sealed("15.3"));
        assert!(!lines(&v).iter().any(|x| x.contains("golden.prev")));
        v.prev = Prev::Kept {
            version: "15.2 (24C101)".into(),
        };
        let l = lines(&v);
        assert!(
            l.iter().any(|x| x.starts_with(
                "  ok       golden.prev macOS 15.2 (24C101), kept for paddock macvm rollback"
            )),
            "{l:?}"
        );
        v.prev = Prev::Removable {
            version: "15.2 (24C101)".into(),
        };
        let l = lines(&v);
        assert!(
            l.iter()
                .any(|x| x.contains("golden.prev macOS 15.2 (24C101)") && x.contains("paddock gc")),
            "{l:?}"
        );
    }
}
