//! `paddock` without a command. On a terminal it starts the terminal
//! interface on its home screen. Without a terminal it prints the help, as
//! before. Each case runs the real binary, with a fake `paddock-tui` that
//! records how it was started.

use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct World {
    _tmp: tempfile::TempDir,
    bin: PathBuf,
    home: PathBuf,
    record: PathBuf,
}

impl World {
    /// `tui`: a fake `paddock-tui` is on PATH. It writes its arguments and
    /// the variables that paddock sets for it, then ends with code 7.
    fn new(tui: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let record = tmp.path().join("record");
        if tui {
            use std::os::unix::fs::PermissionsExt;
            let script = bin.join("paddock-tui");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\n{{ echo \"args=$*\"; echo \"start=${{PADDOCK_TUI_START-unset}}\"; echo \"bin=${{PADDOCK_BIN-unset}}\"; }} > '{}'\nexit 7\n",
                    record.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            _tmp: tmp,
            bin,
            home,
            record,
        }
    }

    fn paddock(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.args(args)
            .env_clear()
            // /bin for the `sh` of the fake interface. No node, no real paddock-tui.
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("XDG_RUNTIME_DIR", self.home.join("run"))
            .env("TERM", "xterm-256color")
            .current_dir(&self.home);
        c
    }

    /// Run with stdin and stdout on a terminal. stderr is a pipe, so the
    /// test reads it.
    fn on_terminal(&self, args: &[&str]) -> Output {
        self.on_terminal_with(args, &[])
    }

    /// [`Self::on_terminal`] with more variables.
    fn on_terminal_with(&self, args: &[&str], vars: &[(&str, &Path)]) -> Output {
        let (master, slave) = pty();
        let mut c = self.paddock(args);
        c.envs(vars.iter().copied());
        // SAFETY: each dup is a new owned fd, and Stdio takes it.
        unsafe {
            c.stdin(Stdio::from_raw_fd(libc::dup(slave)))
                .stdout(Stdio::from_raw_fd(libc::dup(slave)))
                .stderr(Stdio::piped());
        }
        let out = c.spawn().unwrap().wait_with_output().unwrap();
        // SAFETY: the two fds of openpty, closed once.
        unsafe {
            libc::close(slave);
            libc::close(master);
        }
        out
    }

    fn piped(&self, args: &[&str]) -> Output {
        self.paddock(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    }

    fn record(&self) -> Option<String> {
        std::fs::read_to_string(&self.record).ok()
    }
}

fn pty() -> (i32, i32) {
    let mut master = 0;
    let mut slave = 0;
    // SAFETY: openpty writes two new fds. The caller closes them.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    (master, slave)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_paddock"))
}

/// The fake interface must be the one that the test put on PATH: a
/// `paddock-tui` next to the built paddock would win.
fn no_sibling() -> bool {
    let sibling = exe().parent().unwrap().join("paddock-tui");
    if sibling.exists() {
        eprintln!("skipped: {} exists", sibling.display());
        return false;
    }
    true
}

#[test]
fn without_a_terminal_paddock_alone_prints_the_help_as_before() {
    let w = World::new(true);
    let out = w.piped(&[]);
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.starts_with("Run a coding-agent harness"), "{err}");
    assert!(err.contains("Usage: paddock"), "{err}");
    assert!(!err.contains("paddock-tui"), "{err}");
    assert!(out.stdout.is_empty());
    assert_eq!(w.record(), None, "the interface started without a terminal");
}

#[test]
fn on_a_terminal_paddock_alone_starts_the_interface_on_its_home_screen() {
    if !no_sibling() {
        return;
    }
    let w = World::new(true);
    let out = w.on_terminal(&[]);
    // The code of the interface is the code of paddock.
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    let record = w.record().expect("the interface did not start");
    assert!(record.contains("args=\n"), "{record}");
    assert!(record.contains("start=home\n"), "{record}");
    assert!(
        record.contains(&format!("bin={}\n", exe().display())),
        "{record}"
    );
}

#[test]
fn the_options_of_the_interface_pass_through() {
    if !no_sibling() {
        return;
    }
    let w = World::new(true);
    let out = w.on_terminal(&["--plain", "--readonly", "--no-mouse", "--theme", "light"]);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    let record = w.record().expect("the interface did not start");
    assert!(
        record.contains("args=--plain --readonly --no-mouse --theme light\n"),
        "{record}"
    );
    assert!(record.contains("start=home\n"), "{record}");
}

#[test]
fn stdout_in_a_pipe_is_not_a_terminal() {
    let w = World::new(true);
    let (master, slave) = pty();
    let mut c = w.paddock(&[]);
    // SAFETY: the dup is a new owned fd, and Stdio takes it.
    unsafe {
        c.stdin(Stdio::from_raw_fd(libc::dup(slave)));
    }
    let out = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    // SAFETY: the two fds of openpty, closed once.
    unsafe {
        libc::close(slave);
        libc::close(master);
    }
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("Usage: paddock"));
    assert_eq!(w.record(), None);
}

#[test]
fn without_the_interface_one_line_says_so_and_the_help_follows() {
    if !no_sibling() {
        return;
    }
    let w = World::new(false);
    let out = w.on_terminal(&[]);
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    let mut lines = err.lines();
    let first = lines.next().unwrap_or_default();
    // The one line of `paddock tui` without the interface. The paddock of
    // these tests is a development build: it never downloads.
    assert!(
        first.starts_with("The paddock TUI is not installed.")
            && first.contains("development build")
            && first.contains("tui/scripts/install.sh"),
        "{err}"
    );
    let tui = w.on_terminal(&["tui"]);
    assert_eq!(tui.status.code(), Some(127));
    assert_eq!(text(&tui.stderr), format!("{first}\n"));
    // The help follows, the same text as without a terminal.
    let rest: Vec<&str> = lines.collect();
    assert_eq!(rest.join("\n") + "\n", text(&w.piped(&[]).stderr));
}

#[test]
fn the_help_a_harness_and_the_commands_stay_as_they_are_on_a_terminal() {
    if !no_sibling() {
        return;
    }
    let w = World::new(true);
    // The help in both forms: exit 0, and no interface.
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let out = w.on_terminal(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert_eq!(w.record(), None, "{args:?} started the interface");
    }
    // A harness that is not installed fails as before, with its own words.
    let out = w.on_terminal(&["no-such-agent"]);
    assert_ne!(out.status.code(), Some(7));
    assert_eq!(w.record(), None, "a harness name started the interface");
    // An option that is not one of the interface: clap's error, as before.
    let out = w.on_terminal(&["--plain", "--nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("--plain"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(w.record(), None);
    // `paddock tui` starts the interface on its runs: no home screen.
    let out = w.on_terminal(&["tui", "--no-mouse"]);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    let record = w.record().expect("paddock tui did not start the interface");
    assert!(record.contains("args=--no-mouse\n"), "{record}");
    assert!(record.contains("start=unset\n"), "{record}");
}

/// `paddock` alone finds the interface as `paddock tui` does: here the
/// program that `PADDOCK_TUI` names, which is not on PATH.
#[test]
fn paddock_alone_and_paddock_tui_find_the_same_program() {
    if !no_sibling() {
        return;
    }
    let w = World::new(true);
    let named = w.home.join("my-tui");
    std::fs::rename(w.bin.join("paddock-tui"), &named).unwrap();
    let vars = [("PADDOCK_TUI", named.as_path())];
    let out = w.on_terminal_with(&[], &vars);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    let record = w.record().expect("PADDOCK_TUI did not start");
    assert!(record.contains("start=home\n"), "{record}");
    std::fs::remove_file(&w.record).unwrap();
    let out = w.on_terminal_with(&["tui"], &vars);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    let record = w.record().expect("PADDOCK_TUI did not start");
    assert!(record.contains("start=unset\n"), "{record}");
}
