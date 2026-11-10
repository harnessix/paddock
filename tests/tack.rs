//! `paddock tack` end to end, with the built binary: import moves a value
//! from a shell startup file into the keyring and leaves a reference,
//! nothing prints a value, a locked or missing keyring changes no file,
//! and the configuration refuses what it must refuse. The keyring is a
//! fake `secret-tool` that keeps its items in the test folder, and the
//! values are fakes. Temporary folders only.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(target_os = "linux")]
const VALUE: &str = "fake-token-for-the-tack-test";

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    /// No system folders on PATH: the host's own secret-tool stays out.
    bare: bool,
}

impl World {
    /// `mode` of the fake keyring: "ok", "locked", or "none" for no
    /// secret-tool at all.
    fn new(mode: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        for d in ["home", "cfg/paddock", "bin", "items"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        if mode != "none" {
            let script = format!(
                r#"#!/bin/sh
items='{items}'
printf '%s\n' "$*" >> '{root}/calls'
if [ '{mode}' = locked ]; then printf '%s\n' "secret-tool: Cannot get secret of a locked object" >&2; exit 1; fi
cmd=$1; shift
case "$cmd" in
  store) shift; cat > "$items/$2-$4" ;;
  lookup) [ -f "$items/$2-$4" ] || exit 1; cat "$items/$2-$4" ;;
  *) exit 2 ;;
esac
"#,
                items = root.join("items").display(),
                root = root.display(),
                mode = mode,
            );
            let p = root.join("bin/secret-tool");
            std::fs::write(&p, script).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        World {
            _tmp: tmp,
            root,
            bare: mode == "none",
        }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn config(&self, text: &str) {
        std::fs::write(self.root.join("cfg/paddock/config.toml"), text).unwrap();
    }

    fn paddock(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = self.command(Path::new(env!("CARGO_BIN_EXE_paddock")), env);
        c.args(args);
        c.output().unwrap()
    }

    /// `program` with this world's environment and nothing of the host's.
    fn command(&self, program: &Path, env: &[(&str, &str)]) -> Command {
        let sys = if self.bare { "" } else { ":/usr/bin:/bin" };
        let path = format!("{}{sys}", self.root.join("bin").display());
        let mut c = Command::new(program);
        c.env_clear()
            .env("HOME", self.home())
            .env("PATH", path)
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("USER", "demo")
            .current_dir(self.home());
        for (k, v) in env {
            c.env(k, v);
        }
        c
    }

    #[cfg(target_os = "linux")]
    fn stored(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join("items").join(format!("paddock-tack-{name}"))).ok()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

// The fake store is secret-tool. On macOS the store is the login
// keychain through security, which a test must never touch.
#[cfg(target_os = "linux")]
#[test]
fn import_moves_the_value_into_the_keyring_and_leaves_a_reference() {
    let w = World::new("ok");
    let rc = w.home().join(".bashrc");
    let before = format!("alias ll='ls -l'\nexport GH_TOKEN={VALUE}\n");
    std::fs::write(&rc, &before).unwrap();
    let env_file = w.home().join("project.env");
    std::fs::write(&env_file, format!("GH_TOKEN=\"{VALUE}\"\n")).unwrap();

    let dry = w.paddock(
        &["tack", "import", "GH_TOKEN", "--file", "project.env"],
        &[],
    );
    let out = text(&dry);
    assert!(dry.status.success(), "{out}");
    assert!(
        out.contains("store    GH_TOKEN into system keyring (secret-tool)"),
        "{out}"
    );
    assert!(
        out.contains("~/.bashrc:2  becomes  export GH_TOKEN=tack://GH_TOKEN"),
        "{out}"
    );
    assert!(out.contains("Dry run: nothing changed"), "{out}");
    assert!(!out.contains(VALUE), "{out}");
    assert_eq!(read(&rc), before);
    assert_eq!(w.stored("GH_TOKEN"), None);

    let real = w.paddock(
        &[
            "tack",
            "import",
            "GH_TOKEN",
            "--file",
            "project.env",
            "--yes",
        ],
        &[],
    );
    let out = text(&real);
    assert!(real.status.success(), "{out}");
    assert!(!out.contains(VALUE), "{out}");
    assert_eq!(w.stored("GH_TOKEN").as_deref(), Some(VALUE));
    assert_eq!(
        read(&rc),
        "alias ll='ls -l'\nexport GH_TOKEN=tack://GH_TOKEN\n"
    );
    assert_eq!(read(&env_file), "GH_TOKEN=tack://GH_TOKEN\n");
    assert!(out.contains("to undo: cp "), "{out}");
    // The value never was an argument of the keyring tool.
    let calls = read(&w.root.join("calls"));
    assert!(!calls.contains(VALUE), "{calls}");

    // A second import has nothing to do.
    let again = w.paddock(&["tack", "import", "GH_TOKEN", "--yes"], &[]);
    assert!(
        text(&again).contains("done     GH_TOKEN"),
        "{}",
        text(&again)
    );

    let list = w.paddock(&["tack", "list"], &[]);
    let out = text(&list);
    assert!(
        out.contains("GH_TOKEN") && out.contains("tack://GH_TOKEN"),
        "{out}"
    );
    assert!(out.contains("imported, not in the configuration"), "{out}");
    let json = w.paddock(&["tack", "list", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(v["api_version"], "paddock.tack/v1");
    assert_eq!(v["secrets"][0]["reference"], "tack://GH_TOKEN");

    // The log has names and paths, never the value.
    let log = read(&w.root.join("state/paddock/tack/log.jsonl"));
    assert!(
        log.contains("\"GH_TOKEN\"") && !log.contains(VALUE),
        "{log}"
    );
}

// The fake store is secret-tool. On macOS the store is the login
// keychain through security, which a test must never touch.
#[cfg(target_os = "linux")]
#[test]
fn a_locked_keyring_changes_no_file() {
    let w = World::new("locked");
    let rc = w.home().join(".zshrc");
    let before = format!("export GH_TOKEN={VALUE}\n");
    std::fs::write(&rc, &before).unwrap();
    let o = w.paddock(&["tack", "import", "GH_TOKEN", "--yes"], &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("the keyring is locked") && out.contains("unlock it"),
        "{out}"
    );
    assert!(!out.contains(VALUE), "{out}");
    assert_eq!(read(&rc), before);
}

// The fake store is secret-tool. On macOS the store is the login
// keychain through security, which a test must never touch.
#[cfg(target_os = "linux")]
#[test]
fn without_a_keyring_tool_the_message_says_what_to_install() {
    let w = World::new("none");
    std::fs::write(
        w.home().join(".profile"),
        format!("export GH_TOKEN={VALUE}\n"),
    )
    .unwrap();
    let o = w.paddock(&["tack", "import", "GH_TOKEN", "--yes"], &[]);
    let out = text(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("secret-tool is not installed: install libsecret"),
        "{out}"
    );
    assert!(read(&w.home().join(".profile")).contains(VALUE));
}

#[test]
fn the_configuration_refuses_a_shell_on_the_allowlist() {
    let w = World::new("ok");
    w.config(
        "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"/usr/bin/bash\"\n",
    );
    let o = w.paddock(&["tack", "list"], &[]);
    let out = text(&o);
    assert!(!o.status.success(), "{out}");
    assert!(
        out.contains("/usr/bin/bash cannot be allowlisted: bash is a shell"),
        "{out}"
    );
    let o = w.paddock(&["tack", "import", "GH_TOKEN"], &[]);
    assert!(!o.status.success());
}

// The fake store is secret-tool. On macOS the store is the login
// keychain through security, which a test must never touch.
#[cfg(target_os = "linux")]
#[test]
fn doctor_has_a_tack_block() {
    let w = World::new("ok");
    w.config(
        "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"/no/such/gh\"\n",
    );
    std::fs::write(
        w.home().join(".bashrc"),
        format!("export GH_TOKEN={VALUE}\n"),
    )
    .unwrap();
    let o = w.paddock(&["doctor"], &[("GH_TOKEN", VALUE)]);
    let out = text(&o);
    let block = out
        .split("Secrets for allowlisted programs (paddock tack):")
        .nth(1)
        .unwrap_or_else(|| panic!("no tack block in:\n{out}"));
    for want in [
        "missing  GH_TOKEN  is not in the system keyring (secret-tool): paddock tack import GH_TOKEN",
        "missing  GH_TOKEN  allowlisted program /no/such/gh is not there",
        "plain    GH_TOKEN  has a plain value in ~/.bashrc:1",
        "plain    GH_TOKEN  has a plain value in this environment",
    ] {
        assert!(block.contains(want), "missing {want:?} in:\n{block}");
    }
    assert!(!out.contains(VALUE), "{out}");
}

#[test]
fn setup_and_remove_without_root_are_the_stand_ins_and_on_the_host_only() {
    let w = World::new("ok");
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    w.config(
        "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"/usr/bin/gh\"\n",
    );
    // As the user: the stand-ins and the PATH lines, no system file.
    let o = w.paddock(&["tack", "setup"], &[]);
    assert!(o.status.success(), "{}", text(&o));
    let shim = w.root.join("data/paddock/tack/bin/gh");
    let script = std::fs::read_to_string(&shim).unwrap_or_default();
    assert!(script.contains("tack run --shim -- 'gh'"), "{}", text(&o));
    assert!(text(&o).contains("Stand-ins for gh"), "{}", text(&o));
    let o = w.paddock(&["tack", "remove"], &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!shim.exists());
    for cmd in ["setup", "remove"] {
        // In a sandbox it refuses, and says why.
        let o = w.paddock(&["tack", cmd], &[("PADDOCK", "1")]);
        assert!(!o.status.success());
        assert!(text(&o).contains("runs on the host only"), "{}", text(&o));
    }
    let o = w.paddock(&["tack", "serve"], &[]);
    assert!(!o.status.success());
    assert!(text(&o).contains("tackd runs as root"), "{}", text(&o));
}

/// Hermetic: the stand-in looks for tackd under the test folder (not the
/// host's /run and /etc), PATH has no system folder, and both stand-ins
/// run in a session of the test's own, whose leader is a shell that waits
/// for them, so the host's login session, terminal and tackd stay out.
#[test]
fn a_stand_in_without_tackd_runs_the_program_with_the_reference() {
    use std::os::unix::process::CommandExt;
    let mut w = World::new("ok");
    w.bare = true;
    let gh = w.root.join("bin/gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\nprintf \"%s\\n\" \"gh $* token=$GH_TOKEN\"\nexit 3\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    w.config(&format!(
        "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\n",
        gh.display()
    ));
    let run = w.root.join("run").display().to_string();
    let tackd = w.root.join("tackd").display().to_string();
    let env = [
        ("GH_TOKEN", "tack://GH_TOKEN"),
        ("XDG_RUNTIME_DIR", run.as_str()),
        ("PADDOCK_TACK_TEST_ROOT", tackd.as_str()),
    ];
    let out = w.root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let mut c = w.command(Path::new("/bin/sh"), &env);
    c.arg("-c")
        .arg(
            "\"$0\" tack run gh pr list >\"$1/out1\" 2>\"$1/err1\"; printf \"%s\\n\" $? >\"$1/code1\"\n\
             \"$0\" tack run gh >\"$1/out2\" 2>\"$1/err2\"; printf \"%s\\n\" $? >\"$1/code2\"",
        )
        .arg(env!("CARGO_BIN_EXE_paddock"))
        .arg(&out);
    // SAFETY: setsid is async-signal-safe; the child is a new session's
    // leader, with no controlling terminal.
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let f = |n: &str| read(&out.join(n));
    let said = format!("{}{}", f("out1"), f("err1"));
    assert_eq!(f("code1").trim(), "3", "{said}");
    assert_eq!(f("out1"), "gh pr list token=tack://GH_TOKEN\n");
    let note = if cfg!(target_os = "linux") {
        "tackd is not installed"
    } else {
        "starts programs with their secrets on Linux only"
    };
    assert!(f("err1").contains(note), "{said}");
    // The second one in the same session runs, without the note.
    assert_eq!(f("code2").trim(), "3", "{}{}", f("out2"), f("err2"));
    assert_eq!(f("out2"), "gh  token=tack://GH_TOKEN\n");
    assert_eq!(f("err2"), "", "the note is once per session");
    // One mark, under the given runtime folder.
    let marks: Vec<_> = std::fs::read_dir(w.root.join("run/paddock/tack-notes"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(marks.len(), 1, "{marks:?}");
}

#[test]
fn status_names_the_mode_and_json_has_its_fields() {
    let w = World::new("ok");
    let o = w.paddock(&["tack", "status"], &[]);
    let t = text(&o);
    // Either tackd is installed on this host, or the rootless mode is said.
    assert!(t.contains("rootless mode") || t.contains("tackd"), "{t}");
    let o = w.paddock(&["tack", "status", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    for k in ["paddock", "installed", "binary", "socket", "tackd"] {
        assert!(v.get(k).is_some(), "{k}: {v}");
    }
    assert_eq!(v["paddock"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn check_names_a_program_that_is_not_on_path() {
    let w = World::new("ok");
    let o = w.paddock(&["tack", "check", "no-such-program-xyz", "--flag"], &[]);
    assert!(!o.status.success());
    assert!(
        text(&o).contains("no-such-program-xyz is not a program on PATH"),
        "{}",
        text(&o)
    );
}
