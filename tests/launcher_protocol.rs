//! `paddock --launcher-protocol` with the built binary: the exact answer,
//! no file written, no harness started. The environment is cleared; HOME,
//! the XDG folders, the working folder and PATH are in a temporary folder.
//! PATH holds fake harness commands that record a start.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const MAC: &[u8] = include_bytes!("fixtures/launcher-protocol/mac-v1.json");
const OTHER: &[u8] = include_bytes!("fixtures/launcher-protocol/other-v1.json");

/// The harness commands that a misrouted query could start.
const AGENTS: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "copilot",
    "opencode",
    "grok",
    "agy",
    "fake-agent",
];

const DIRS: &[(&str, &str)] = &[
    ("HOME", "home"),
    ("TMPDIR", "tmp"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_RUNTIME_DIR", "runtime"),
];

struct World {
    t: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let t = tempfile::tempdir().unwrap();
        for (_, dir) in DIRS {
            std::fs::create_dir(t.path().join(dir)).unwrap();
        }
        let bin = t.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for agent in AGENTS {
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"{agent} $*\" >> '{}'\n",
                t.path().join("starts").display()
            );
            let p = bin.join(agent);
            std::fs::write(&p, script).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        World { t }
    }

    fn root(&self) -> &Path {
        self.t.path()
    }

    fn paddock(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_paddock"));
        cmd.args(args)
            .env_clear()
            .env("PATH", self.root().join("bin"))
            .env("USER", "test")
            .current_dir(self.root().join("home"))
            .stdin(Stdio::null());
        for (var, dir) in DIRS {
            cmd.env(var, self.root().join(dir));
        }
        cmd.output().unwrap()
    }

    /// Every path below the root, with the bytes of each file.
    fn snapshot(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn walk(root: &Path, dir: &Path, map: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(root).unwrap().to_path_buf();
                let kind = std::fs::symlink_metadata(&p).unwrap().file_type();
                if kind.is_dir() {
                    map.insert(rel, None);
                    walk(root, &p, map);
                } else {
                    map.insert(rel, Some(std::fs::read(&p).unwrap_or_default()));
                }
            }
        }
        let mut map = BTreeMap::new();
        walk(self.root(), self.root(), &mut map);
        map
    }

    fn starts(&self) -> Option<String> {
        std::fs::read_to_string(self.root().join("starts")).ok()
    }
}

fn expected() -> &'static [u8] {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        MAC
    } else {
        OTHER
    }
}

#[test]
fn the_query_prints_the_exact_reply_and_changes_nothing() {
    let w = World::new();
    let before = w.snapshot();
    let o = w.paddock(&["--launcher-protocol"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert_eq!(
        o.stdout,
        expected(),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(w.snapshot(), before);
    assert_eq!(w.starts(), None);
}

#[test]
fn the_query_with_other_root_arguments_is_refused_and_changes_nothing() {
    let w = World::new();
    let before = w.snapshot();
    for args in [
        &["--launcher-protocol", "claude"][..],
        &["--launcher-protocol", "run", "codex"],
        &["--launcher-protocol", "--help"],
        &["-v", "--launcher-protocol"],
        &["--launcher-protocol=1"],
    ] {
        let o = w.paddock(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {o:?}");
        assert!(o.stdout.is_empty(), "{args:?}: {o:?}");
        assert_eq!(
            o.stderr, b"paddock: --launcher-protocol must be the only argument\n",
            "{args:?}"
        );
        assert_eq!(w.snapshot(), before, "{args:?}");
    }
    assert_eq!(w.starts(), None);
}

#[test]
fn the_fake_agents_record_a_start() {
    // The control of the checks above: a start is seen.
    let w = World::new();
    let o = Command::new(w.root().join("bin/fake-agent"))
        .arg("--launcher-protocol")
        .env_clear()
        .env("HOME", w.root().join("home"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    assert_eq!(
        w.starts().as_deref(),
        Some("fake-agent --launcher-protocol\n")
    );
}

/// While paddock for Mac lives in this repository, its vendored copies
/// must equal the fixtures of this core, and its pin must name this
/// version. After the split the folder is gone and the launcher repository
/// checks its own pin.
#[test]
fn the_launcher_vendors_the_fixtures_of_this_core() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let launcher = root.join("mac/launcher");
    if !launcher.exists() {
        return;
    }
    for (core, copy) in [
        ("tests/fixtures/launcher", "launcher"),
        ("tests/fixtures/launcher/json", "launcher/json"),
        ("tests/fixtures/launcher-protocol", "launcher-protocol"),
    ] {
        let names = |dir: &Path| -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| [".json", ".jsonl", ".err"].iter().any(|x| n.ends_with(x)))
                .collect();
            v.sort();
            v
        };
        let core = root.join(core);
        let copy = launcher.join("Fixtures/core").join(copy);
        assert_eq!(names(&core), names(&copy), "{}", copy.display());
        for name in names(&core) {
            assert_eq!(
                std::fs::read(core.join(&name)).unwrap(),
                std::fs::read(copy.join(&name)).unwrap(),
                "{name}: copy it again into {}",
                copy.display()
            );
        }
    }
    let pin = std::fs::read_to_string(launcher.join("core.pin")).unwrap();
    let version = pin.lines().find_map(|l| l.strip_prefix("version="));
    assert_eq!(version, Some(env!("CARGO_PKG_VERSION")));
}
