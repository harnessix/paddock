use super::*;
#[cfg(target_os = "linux")]
use crate::backend::Backend;
#[cfg(target_os = "linux")]
use crate::backend::Launch;
use crate::build::{build_with, Options};
use crate::profile::Harness;
#[cfg(target_os = "linux")]
use crate::spec::MountKind;
use std::collections::BTreeMap;

/// A temp home with the data of a fake harness, a key, paddock's own
/// folders, links and special files.
fn home(t: &Path) -> HostEnv {
    let h = t.join("home");
    for d in [
        ".fake/sub",
        ".fake/skills",
        ".ssh",
        ".config/paddock",
        ".agents/skills",
    ] {
        std::fs::create_dir_all(h.join(d)).unwrap();
    }
    std::fs::write(h.join(".fake/conv.jsonl"), "{\"turn\":1}\n").unwrap();
    std::fs::write(h.join(".fake/old.txt"), "old\n").unwrap();
    std::fs::write(h.join(".fake/sub/x"), "x").unwrap();
    std::fs::write(h.join(".fake/auth.json"), "{\"token\":\"host\"}").unwrap();
    std::fs::write(h.join(".fake.json"), "{\"projects\":{}}").unwrap();
    std::fs::write(h.join(".ssh/id_ed25519"), "key").unwrap();
    std::fs::create_dir_all(h.join(".local/share/paddock/homes")).unwrap();
    std::fs::write(h.join(".local/share/paddock/homes/f"), "p").unwrap();
    std::os::unix::fs::symlink("sub/x", h.join(".fake/in")).unwrap();
    std::os::unix::fs::symlink("../.ssh/id_ed25519", h.join(".fake/out")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", h.join(".fake/abs")).unwrap();
    std::os::unix::fs::symlink(h.join(".fake/sub/x"), h.join(".fake/abs_in")).unwrap();
    let _sock = std::os::unix::net::UnixListener::bind(h.join(".fake/sock")).unwrap();
    std::mem::forget(_sock);
    let fifo = std::ffi::CString::new(h.join(".fake/fifo").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let mut vars = BTreeMap::new();
    vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    HostEnv::from_parts(h, "t".into(), 1000, 1000, vars)
}

fn plan(env: &HostEnv, id: &str) -> Plan {
    plan_of(env, id, SnapKind::Overlay)
}

fn plan_of(env: &HostEnv, id: &str, kind: SnapKind) -> Plan {
    Plan::create_kind(
        env,
        id,
        vec![".fake".into(), ".fake.json".into()],
        vec![".fake/auth.json".into()],
        kind,
    )
    .unwrap()
}

/// The overlay folders of an overlay plan.
fn lower(p: &Plan) -> &Path {
    p.lower.as_deref().unwrap()
}
fn upper(p: &Plan) -> &Path {
    p.upper.as_deref().unwrap()
}
fn work(p: &Plan) -> &Path {
    p.work.as_deref().unwrap()
}

#[test]
fn the_snapshot_holds_the_data_roots_and_nothing_else() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let p = plan(&env, "fake-a");
    let c = p.fill(&env.home, u64::MAX, None).unwrap();
    let l = lower(&p);
    assert_eq!(
        std::fs::read_to_string(l.join(".fake/conv.jsonl")).unwrap(),
        "{\"turn\":1}\n"
    );
    assert_eq!(std::fs::read_to_string(l.join(".fake/sub/x")).unwrap(), "x");
    assert!(l.join(".fake.json").is_file());
    assert!(l.join(".fake/skills").is_dir());
    // A link inside its root comes in as a link.
    assert_eq!(
        std::fs::read_link(l.join(".fake/in")).unwrap(),
        Path::new("sub/x")
    );
    assert!(std::fs::symlink_metadata(l.join(".fake/abs_in")).is_ok());
    // Absent, not denied.
    for gone in [
        ".ssh",
        ".local",
        ".config",
        ".agents",
        ".fake/auth.json",
        ".fake/out",
        ".fake/abs",
        ".fake/sock",
        ".fake/fifo",
    ] {
        assert!(
            std::fs::symlink_metadata(l.join(gone)).is_err(),
            "{gone} is in the snapshot"
        );
    }
    assert_eq!(c.skipped_links.len(), 2, "{:?}", c.skipped_links);
    assert_eq!(c.skipped_special.len(), 2, "{:?}", c.skipped_special);
    assert_eq!(c.files, 4);
    let line = c.line();
    assert!(
        line.contains("2 symlink(s)") && line.contains("2 socket(s)"),
        "{line}"
    );
    // The host files are never changed.
    assert_eq!(
        std::fs::read_to_string(env.home.join(".fake/auth.json")).unwrap(),
        "{\"token\":\"host\"}"
    );
    p.remove().unwrap();
    assert!(!p.dir.exists());
}

#[test]
fn invalid_roots_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    for bad in [
        "",
        ".",
        "..",
        "../x",
        ".fake/../.ssh",
        "/etc",
        ".config",
        ".local",
        ".local/share",
        ".local/state",
        ".cache",
        ".agents",
        ".ssh",
        ".ssh/keys",
        ".gnupg",
        ".pki",
        ".password-store",
        ".local/share/paddock",
        ".local/share/paddock/homes",
        ".config/paddock",
        ".aws",
    ] {
        assert!(validate_root(&env, bad).is_err(), "{bad:?} was accepted");
    }
    for good in [".fake", ".fake.json", ".config/fake", ".local/share/fake"] {
        validate_root(&env, good).unwrap();
    }
}

#[test]
fn the_user_configuration_adds_roots_and_a_project_file_cannot() {
    let user = "[profiles.fake]\ndata = [\".fake-extra\"]\n[storage]\nshadow_copy_limit_mib = 1\n";
    assert_eq!(user_roots_in(user, "fake").unwrap(), vec![".fake-extra"]);
    assert!(user_roots_in(user, "other").unwrap().is_empty());
    assert!(user_roots_in("[profiles.fake]\ndata = \"x\"\n", "fake").is_err());
    assert_eq!(copy_limit_in(user), 1 << 20);
    assert_eq!(copy_limit_in(""), DEFAULT_LIMIT_MIB << 20);
    crate::manifest::Manifest::parse_user(user).unwrap();

    // A project paddock.toml can set neither the roots nor the mode.
    let t = tempfile::tempdir().unwrap();
    for text in [
        "[profiles.fake]\ndata = [\".fake-extra\"]\n",
        "profile = \"default\"\n",
        "[profile]\ndefault = true\n",
    ] {
        let f = t.path().join(crate::manifest::PROJECT_FILE);
        std::fs::write(&f, text).unwrap();
        assert!(
            crate::manifest::Manifest::read(&f).is_err(),
            "{text} in a project file"
        );
    }
}

#[test]
fn without_reflinks_the_size_cap_refuses() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let p = plan(&env, "fake-cap");
    let err = p.fill(&env.home, 4, Some(false)).unwrap_err().to_string();
    assert!(
        err.contains("no reflinks") && err.contains("cap is 4 bytes"),
        "{err}"
    );
    assert!(err.contains("shadow_copy_limit_mib"), "{err}");
    // Nothing was copied.
    assert!(!lower(&p).join(".fake").exists());
    // With reflinks the cap does not count.
    p.fill(&env.home, 4, Some(true)).unwrap();
    assert!(lower(&p).join(".fake/conv.jsonl").is_file());
}

#[test]
fn a_run_folder_on_another_file_system_is_refused() {
    // /dev/shm is a tmpfs on most hosts; skip where it is the same device.
    let shm = Path::new("/dev/shm");
    let t = tempfile::tempdir().unwrap();
    let env0 = home(t.path());
    let Ok(other) = tempfile::tempdir_in(shm) else {
        eprintln!("skipped: no /dev/shm");
        return;
    };
    if std::fs::metadata(shm).unwrap().dev() == std::fs::metadata(&env0.home).unwrap().dev() {
        eprintln!("skipped: /dev/shm is on the file system of the home");
        return;
    }
    let mut vars = BTreeMap::new();
    vars.insert(
        "XDG_DATA_HOME".to_string(),
        other.path().display().to_string(),
    );
    let env = HostEnv::from_parts(env0.home.clone(), "t".into(), 1000, 1000, vars);
    let err = Plan::create(&env, "fake-dev", vec![".fake".into()], vec![])
        .unwrap_err()
        .to_string();
    assert!(err.contains("another one"), "{err}");
    assert!(!run_dir(&env, "fake-dev").exists());
}

#[test]
fn links_stay_inside_their_root_or_are_skipped() {
    let h = Path::new("/home/demo");
    let root = Path::new(".fake");
    let rel = Path::new(".fake/a/link");
    assert!(link_stays_inside(h, rel, Path::new("b"), root));
    assert!(link_stays_inside(h, rel, Path::new("../c"), root));
    assert!(link_stays_inside(h, rel, Path::new(".."), root));
    assert!(!link_stays_inside(h, rel, Path::new("../.."), root));
    assert!(!link_stays_inside(h, rel, Path::new("../../.ssh"), root));
    assert!(!link_stays_inside(
        h,
        rel,
        Path::new("../../../../etc"),
        root
    ));
    assert!(link_stays_inside(
        h,
        rel,
        Path::new("/home/demo/.fake/x"),
        root
    ));
    assert!(!link_stays_inside(
        h,
        rel,
        Path::new("/home/demo/.ssh"),
        root
    ));
    assert!(!link_stays_inside(h, rel, Path::new("/etc/passwd"), root));
}

#[test]
fn the_summary_counts_the_changes_of_the_run_only() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let p = plan(&env, "fake-sum");
    p.fill(&env.home, u64::MAX, None).unwrap();
    // paddock's own writes before the start: a copy-up and a credential.
    p.copy_up(Path::new(".fake.json")).unwrap();
    assert_eq!(
        std::fs::read_to_string(upper(&p).join(".fake.json")).unwrap(),
        "{\"projects\":{}}"
    );
    std::fs::create_dir_all(upper(&p).join(".fake")).unwrap();
    std::fs::write(upper(&p).join(".fake/auth.json"), "{}").unwrap();
    // Copy-up of a file the snapshot lacks does nothing.
    p.copy_up(Path::new(".fake/none.toml")).unwrap();
    assert!(!upper(&p).join(".fake/none.toml").exists());
    let base = p.baseline();
    assert_eq!(base.len(), 2);
    assert_eq!(p.summary(&base, &[]).total(), 0);

    // What the run does, as the overlay records it.
    std::fs::write(upper(&p).join(".fake/conv.jsonl"), "{\"turn\":2}\n").unwrap();
    std::fs::write(upper(&p).join(".fake/new.txt"), "new").unwrap();
    std::fs::write(upper(&p).join(".bash_history"), "ls").unwrap();
    // A mount point that bwrap made is no change of the run.
    std::fs::write(upper(&p).join(".mounted"), "").unwrap();
    let wh = std::ffi::CString::new(upper(&p).join(".fake/old.txt").to_str().unwrap()).unwrap();
    // SAFETY: mknod with a NUL-terminated path. Only root can make the
    // whiteout outside an overlay, so the test checks it where it can.
    let whiteout = unsafe { libc::mknod(wh.as_ptr(), libc::S_IFCHR | 0o600, 0) } == 0;
    let s = p.summary(&base, &[PathBuf::from(".mounted")]);
    assert_eq!(s.added, 2, "{:?}", s.paths);
    assert_eq!(s.changed, 1, "{:?}", s.paths);
    assert_eq!(s.deleted, usize::from(whiteout));
    assert_eq!(s.outside, 1);
    let lines = s.lines().join("\n");
    assert!(lines.contains("+ ~/.bash_history"), "{lines}");
    assert!(lines.contains("~ ~/.fake/conv.jsonl"), "{lines}");
    assert!(!lines.contains("auth.json"), "{lines}");
    // The work folder of an overlay has mode 000; removal copes.
    std::fs::create_dir_all(work(&p).join("work")).unwrap();
    std::fs::set_permissions(
        work(&p).join("work"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    p.remove().unwrap();
    assert!(!p.dir.exists());
}

fn fake_profile() -> Profile {
    toml::from_str(
        r#"
name = "fake"
[state]
data = [".fake", ".fake.json"]
auth = [".fake/auth.json"]
seed = [".fake/conv.jsonl"]
import_ro = [".fake/skills", ".agents/skills"]
[[state.json_import]]
file = ".fake.json"
keys = [["projects"]]
[[state.toml_set]]
file = ".fake/config.toml"
path = ["projects", "{project}", "trust_level"]
value = "trusted"
[state.history]
project_folders = ".fake/projects"
"#,
    )
    .unwrap()
}

fn built(env: &HostEnv, t: &Path, profile: Profile) -> anyhow::Result<crate::build::Built> {
    let project = t.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        env.paddock_config().join("config.toml"),
        "[devices]\nmount = false\n[kv]\nenabled = false\n",
    )
    .unwrap();
    let h = Harness {
        profile,
        exe: PathBuf::from("/usr/bin/true"),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: "fake".into(),
        project: Some(project),
        assume_yes: true,
        inherit_env: Some(false),
        profile_default: true,
        ..Default::default()
    };
    build_with(env, &opts, h)
}

// The overlay is the kind of Linux only.
#[cfg(target_os = "linux")]
#[test]
fn the_spec_has_one_overlay_at_the_home_and_nothing_selective_inside_a_root() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let b = built(&env, t.path(), fake_profile()).unwrap();
    let s = &b.spec;
    let p = s.shadow.as_ref().expect("shadow plan");
    assert_eq!(s.home_src, upper(p));
    let at_home: Vec<_> = s.mounts.iter().filter(|m| m.dst == env.home).collect();
    assert_eq!(at_home.len(), 1);
    assert_eq!(at_home[0].kind, MountKind::Overlay);
    assert_eq!(at_home[0].src.as_deref(), Some(upper(p)));
    let l = at_home[0].layers.as_ref().unwrap();
    assert_eq!((l.lower.as_path(), l.work.as_path()), (lower(p), work(p)));
    assert_eq!(
        s.mounts
            .iter()
            .filter(|m| m.kind == MountKind::Overlay && m.dst.starts_with(&env.home))
            .filter(|m| m.dst == env.home)
            .count(),
        1
    );
    // No import, history bind or other mount inside a data root.
    for m in &s.mounts {
        let Ok(rel) = m.dst.strip_prefix(&env.home) else {
            continue;
        };
        assert!(
            p.root_of(rel).is_none(),
            "{} is mounted inside a data root ({})",
            m.dst.display(),
            m.origin
        );
    }
    // An import_ro path outside the roots stays.
    assert!(s
        .mounts
        .iter()
        .any(|m| m.dst == env.home.join(".agents/skills")));
    assert!(!b.shared_history);
    let json = serde_json::to_value(s).unwrap();
    assert_eq!(json["shadow"]["profile"], "default");
    crate::shadow::discard(s);
    assert!(!p.dir.exists());
}

#[test]
fn a_harness_without_roots_refuses_and_leaves_no_run_folder() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let mut prof = fake_profile();
    prof.state.data.clear();
    let err = built(&env, t.path(), prof)
        .err()
        .expect("refused")
        .to_string();
    assert!(err.contains("no data roots"), "{err}");
    let mut prof = fake_profile();
    prof.state.data = vec![".ssh".into()];
    let err = built(&env, t.path(), prof)
        .err()
        .expect("refused")
        .to_string();
    assert!(err.contains("keys or passwords"), "{err}");
    let left = std::fs::read_dir(runs_root(&env))
        .map(|r| r.count())
        .unwrap_or(0);
    assert_eq!(left, 0);
}

#[test]
fn only_bwrap_runs_a_shadow_sandbox() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let b = built(&env, t.path(), fake_profile()).unwrap();
    for name in ["docker", "podman", "srt"] {
        let be = crate::backend::by_name(name).unwrap();
        let err = be.check(&env, &b.spec).unwrap_err().to_string();
        assert!(err.contains("only on the bwrap backend"), "{name}: {err}");
    }
    discard(&b.spec);
}

/// A backend that plays the harness on the host: it writes into the upper
/// folder as the overlay would, and reports what the credential delivery
/// put there.
#[cfg(target_os = "linux")]
struct FakeOverlay;

#[cfg(target_os = "linux")]
impl Backend for FakeOverlay {
    fn name(&self) -> &'static str {
        "test"
    }
    fn check(&self, _: &HostEnv, _: &crate::spec::SandboxSpec) -> Result<()> {
        Ok(())
    }
    fn render(&self, env: &HostEnv, spec: &crate::spec::SandboxSpec, _: bool) -> Result<Launch> {
        let p = spec.shadow.as_ref().unwrap();
        let u = upper(p).display().to_string();
        let l = lower(p).display().to_string();
        let proj = spec.project.display().to_string();
        let script = format!(
            "cat '{u}/.fake/auth.json' > '{proj}/auth-seen'; \
             ls -a '{l}' > '{proj}/lower-seen'; \
             cp '{l}/.fake/conv.jsonl' '{u}/.fake/conv.jsonl' && echo more >> '{u}/.fake/conv.jsonl'; \
             echo new > '{u}/.fake/new.txt'"
        );
        Ok(Launch {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script],
            env: crate::backend::base_env(env, spec),
            files: vec![],
        })
    }
}

// The overlay is the kind of Linux only.
#[cfg(target_os = "linux")]
#[test]
fn a_run_changes_only_its_snapshot_and_the_run_folder_goes() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let before: Vec<(PathBuf, Vec<u8>)> = [".fake/conv.jsonl", ".fake/auth.json", ".fake.json"]
        .iter()
        .map(|r| {
            let p = env.home.join(r);
            let b = std::fs::read(&p).unwrap();
            (p, b)
        })
        .collect();
    let b = built(&env, t.path(), fake_profile()).unwrap();
    let dir = b.spec.shadow.as_ref().unwrap().dir.clone();
    let proj = b.spec.project.clone();
    let _server = crate::view::guard::TmuxGuard::socket(crate::view::socket(&env));
    let out = crate::run::run(&env, b, &FakeOverlay, true, true).unwrap();
    // The run started a tmux server for `paddock view`. Stop it.
    if let Some(tmux) = crate::view::Tmux::find(&env) {
        for id in tmux.sessions() {
            tmux.kill(&id);
        }
    }
    assert_eq!(out.code, 0);
    // The credential delivery put the login into the upper folder.
    assert_eq!(
        std::fs::read_to_string(proj.join("auth-seen")).unwrap(),
        "{\"token\":\"host\"}"
    );
    let lower = std::fs::read_to_string(proj.join("lower-seen")).unwrap();
    assert!(
        lower.contains(".fake") && !lower.contains(".ssh"),
        "{lower}"
    );
    for (p, b) in before {
        assert_eq!(std::fs::read(&p).unwrap(), b, "{} changed", p.display());
    }
    assert!(!env.home.join(".fake/new.txt").exists());
    assert!(!dir.exists(), "the run folder is left");
}

// The overlay is the kind of Linux only.
#[cfg(target_os = "linux")]
#[test]
fn bwrap_mounts_the_overlay_at_the_home_instead_of_the_bind() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    if env.which("bwrap").is_none() {
        eprintln!("skipped: no bwrap on PATH");
        return;
    }
    let mut b = built(&env, t.path(), fake_profile()).unwrap();
    b.spec.network = crate::spec::Network::None;
    let p = b.spec.shadow.clone().unwrap();
    let launch = crate::backend::bwrap::Bwrap
        .render(&env, &b.spec, false)
        .unwrap();
    let a = &launch.args;
    let s = |x: &Path| x.display().to_string();
    let home = s(&env.home);
    let at = a
        .windows(6)
        .position(|w| {
            w == [
                "--overlay-src".to_string(),
                s(lower(&p)),
                "--overlay".into(),
                s(upper(&p)),
                s(work(&p)),
                home.clone(),
            ]
        })
        .expect("the overlay at the home");
    assert!(
        !a.windows(3).any(|w| w[0] == "--bind" && w[2] == home),
        "a bind at the home too: {a:?}"
    );
    // The project comes after the home.
    let proj = s(&b.spec.project);
    let pj = a
        .windows(3)
        .position(|w| w[0] == "--bind" && w[2] == proj)
        .unwrap();
    assert!(pj > at);
    discard(&b.spec);
}

#[test]
fn a_clone_plan_makes_the_home_only() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let p = plan_of(&env, "fake-clone", SnapKind::Clone);
    assert_eq!(p.home, p.dir.join("home"));
    assert!(p.home.is_dir());
    assert_eq!((&p.lower, &p.upper, &p.work), (&None, &None, &None));
    let names: Vec<_> = std::fs::read_dir(&p.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("home")]);
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["kind"], "clone");
    assert!(json.get("lower").is_none(), "{json}");
    // A clone has the file already: copy-up does nothing.
    p.copy_up(Path::new(".fake.json")).unwrap();
    assert!(!p.home.join(".fake.json").exists());
    // The overlay kind keeps its three folders, and the home is the upper.
    let o = plan(&env, "fake-overlay");
    assert_eq!(o.home, upper(&o));
    assert!(lower(&o).is_dir() && work(&o).is_dir());
    p.remove().unwrap();
    assert!(!p.dir.exists());
}

#[test]
fn a_clone_fill_copies_counts_and_keeps_paddocks_files() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let p = plan_of(&env, "fake-cfill", SnapKind::Clone);
    // A file that paddock wrote before the copy stays, as an upper folder
    // hides the lower one.
    std::fs::write(p.home.join(".fake.json"), "paddock").unwrap();
    let c = p.fill(&env.home, u64::MAX, Some(false)).unwrap();
    let h = &p.home;
    assert_eq!(
        std::fs::read_to_string(h.join(".fake/conv.jsonl")).unwrap(),
        "{\"turn\":1}\n"
    );
    assert_eq!(std::fs::read_to_string(h.join(".fake/sub/x")).unwrap(), "x");
    assert_eq!(
        std::fs::read_to_string(h.join(".fake.json")).unwrap(),
        "paddock"
    );
    // conv.jsonl, old.txt and sub/x; .fake.json was there; never auth.json.
    assert_eq!(c.files, 3);
    assert_eq!(c.bytes, 11 + 4 + 1);
    assert!(!h.join(".fake/auth.json").exists());
    assert!(!h.join(".ssh").exists());
    assert_eq!(c.skipped_special.len(), 2, "{:?}", c.skipped_special);
    assert_eq!(c.skipped_links.len(), 2, "{:?}", c.skipped_links);
    // A relative link stays; an absolute one into the root points into the
    // clone, which is the home of the sandbox.
    assert_eq!(
        std::fs::read_link(h.join(".fake/in")).unwrap(),
        Path::new("sub/x")
    );
    assert_eq!(
        std::fs::read_link(h.join(".fake/abs_in")).unwrap(),
        h.join(".fake/sub/x")
    );
    let line = c.line();
    assert!(line.contains("(plain copies: no clones here)"), "{line}");
    // The cap names clones, not reflinks.
    let q = plan_of(&env, "fake-ccap", SnapKind::Clone);
    let err = q.fill(&env.home, 4, Some(false)).unwrap_err().to_string();
    assert!(
        err.contains("no clones") && err.contains("cap is 4 bytes"),
        "{err}"
    );
    let with = Copied {
        reflinks: true,
        clones: true,
        ..Default::default()
    };
    assert!(with.line().contains("(clones)"));
    p.remove().unwrap();
    q.remove().unwrap();
}

#[test]
fn the_clone_summary_compares_with_the_baseline() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let p = plan_of(&env, "fake-csum", SnapKind::Clone);
    p.fill(&env.home, u64::MAX, Some(false)).unwrap();
    let h = p.home.clone();
    // paddock's own writes before the start: a settings file and a login.
    std::fs::write(h.join(".fake/config.toml"), "trust = 1\n").unwrap();
    std::fs::write(h.join(".fake/auth.json"), "{}").unwrap();
    std::fs::create_dir_all(h.join(".fake/empty")).unwrap();
    let base = p.baseline();
    assert!(base.files.contains_key(Path::new(".fake/config.toml")));
    assert!(base.dirs.contains(Path::new(".fake/sub")));
    assert_eq!(p.summary_clone(&base).total(), 0);

    // What the run does.
    let x = h.join(".fake/sub/x");
    let mtime = std::fs::metadata(&x).unwrap().modified().unwrap();
    std::fs::write(h.join(".fake/conv.jsonl"), "{\"turn\":1}\n{\"turn\":2}\n").unwrap();
    std::fs::write(h.join(".fake/new.txt"), "new").unwrap();
    std::fs::remove_file(h.join(".fake/old.txt")).unwrap();
    std::fs::remove_dir(h.join(".fake/empty")).unwrap();
    std::fs::write(h.join(".bash_history"), "ls").unwrap();
    // The private temporary folder of a Seatbelt run is no news.
    std::fs::create_dir_all(h.join(".paddock-tmp/id")).unwrap();
    std::fs::write(h.join(".paddock-tmp/id/t"), "t").unwrap();
    // The limit: a rewrite with the same size and the old change time is
    // not seen.
    std::fs::write(&x, "y").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&x)
        .unwrap()
        .set_modified(mtime)
        .unwrap();

    let s = p.summary_clone(&base);
    assert_eq!(s.added, 2, "{:?}", s.paths);
    assert_eq!(s.changed, 1, "{:?}", s.paths);
    assert_eq!(s.deleted, 2, "{:?}", s.paths);
    assert_eq!(s.outside, 1, "{:?}", s.paths);
    let lines = s.lines().join("\n");
    assert!(lines.contains("+ ~/.fake/new.txt"), "{lines}");
    assert!(lines.contains("+ ~/.bash_history"), "{lines}");
    assert!(lines.contains("~ ~/.fake/conv.jsonl"), "{lines}");
    assert!(lines.contains("- ~/.fake/old.txt"), "{lines}");
    assert!(lines.contains("- ~/.fake/empty"), "{lines}");
    for absent in ["config.toml", "auth.json", "sub/x", ".paddock-tmp"] {
        assert!(!lines.contains(absent), "{absent}: {lines}");
    }
    p.remove().unwrap();
}

/// A Seatbelt run builds a clone at the home and no overlay.
#[cfg(target_os = "macos")]
#[test]
fn the_spec_of_a_mac_has_the_clone_as_its_home_and_no_mount_there() {
    let t = tempfile::tempdir().unwrap();
    let env = home(t.path());
    let b = built(&env, t.path(), fake_profile()).unwrap();
    let s = &b.spec;
    let p = s.shadow.as_ref().expect("shadow plan");
    assert_eq!(p.kind, SnapKind::Clone);
    assert_eq!(s.home_src, p.home);
    assert!(!s.mounts.iter().any(|m| m.dst == env.home));
    assert!(crate::backend::seatbelt::run::refuse(s).is_ok());
    discard(s);
    assert!(!p.dir.exists());
}
