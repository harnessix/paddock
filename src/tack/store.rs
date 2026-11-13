//! Where tack keeps the values: the existing backends.
//!
//! - `keyring`: the system keyring. On Linux it is the Secret Service,
//!   through `secret-tool` (libsecret), with the attributes
//!   `service = paddock-tack` and `name = NAME`. On macOS it is the login
//!   keychain, through `security`, with service `paddock-tack` and account
//!   NAME. paddock writes and reads these items.
//! - `command`: a password manager command that prints the value, as
//!   `from = "command"` of `[credentials]` does. paddock only reads it.
//!
//! Every value that comes out of a store is a `Secret`: locked in RAM and
//! wiped when it is dropped. A value goes into a store on the standard
//! input of the tool, never in its arguments, where other processes could
//! read it.

use super::config::{Backend, SecretCfg};
use crate::credentials::secret::Secret;
use crate::env::HostEnv;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The service attribute of tack's keyring items.
pub const SERVICE: &str = "paddock-tack";

/// How long a keyring tool may take. An unlock prompt can wait for the user.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// The largest value, as for `[credentials]`.
pub const MAX_VALUE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The store answers, and has no value under this name.
    Missing,
    /// The store is there but locked, or the user refused to unlock it.
    Locked(String),
    /// The store cannot be reached: no tool, no keyring service.
    Unavailable(String),
    /// Anything else, with the tool's own words.
    Failed(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Missing => write!(f, "the store has no value under this name"),
            StoreError::Locked(d) => write!(
                f,
                "the keyring is locked ({d}): unlock it in your desktop session, then run the command again"
            ),
            StoreError::Unavailable(d) => write!(f, "the store is not reachable: {d}"),
            StoreError::Failed(d) => write!(f, "the store failed: {d}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// One store of values.
pub trait Store {
    /// For messages: "system keyring (secret-tool)", "command op".
    fn describe(&self) -> String;
    /// Whether paddock can write values into it.
    fn writable(&self) -> bool;
    fn get(&self, name: &str) -> Result<Secret, StoreError>;
    fn put(&self, name: &str, value: &Secret) -> Result<(), StoreError>;
}

/// The store of one secret.
pub fn open(env: &HostEnv, cfg: &SecretCfg) -> Result<Box<dyn Store>, StoreError> {
    match cfg.backend {
        Backend::Keyring => Ok(Box::new(Keyring::find(env)?)),
        Backend::Command => Ok(Box::new(CommandStore {
            cmd: cfg.command.clone(),
            prepare: None,
        })),
    }
}

/// What tackd does to each store command before it starts: run it as the
/// calling user, with that user's environment.
pub type Prepare = std::sync::Arc<dyn Fn(&mut Command) + Send + Sync>;

/// The store of one secret for tackd: the tools run through `prepare`;
/// `env` (the caller's, as tackd sees it) finds the tools.
pub fn open_as(
    env: &HostEnv,
    cfg: &SecretCfg,
    prepare: Prepare,
) -> Result<Box<dyn Store>, StoreError> {
    match cfg.backend {
        Backend::Keyring => {
            let mut k = Keyring::find(env)?;
            k.prepare = Some(prepare);
            Ok(Box::new(k))
        }
        Backend::Command => {
            let mut cmd = cfg.command.clone();
            // The first word by the caller's PATH; tackd's own PATH is
            // not the user's.
            if let Some(p) = cmd.first().and_then(|w| env.which(w)) {
                cmd[0] = p.display().to_string();
            }
            Ok(Box::new(CommandStore {
                cmd,
                prepare: Some(prepare),
            }))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    /// Linux: `secret-tool` of libsecret.
    SecretTool(PathBuf),
    /// macOS: `security`.
    Security(PathBuf),
}

pub struct Keyring {
    tool: Tool,
    timeout: Duration,
    prepare: Option<Prepare>,
    /// The service attribute of the items: `SERVICE` for tack, another
    /// one for the keys of `[credentials]` (credentials/keys.rs).
    service: &'static str,
}

impl Keyring {
    /// The keyring tool of this host.
    pub fn find(env: &HostEnv) -> Result<Self, StoreError> {
        let (name, hint) = if cfg!(target_os = "macos") {
            (
                "security",
                "it comes with macOS; check that /usr/bin is on PATH",
            )
        } else {
            (
                "secret-tool",
                "install libsecret (Arch: pacman -S libsecret; Debian and Ubuntu: apt install libsecret-tools; Fedora: dnf install libsecret)",
            )
        };
        let path = env
            .which(name)
            .ok_or_else(|| StoreError::Unavailable(format!("{name} is not installed: {hint}")))?;
        let tool = if cfg!(target_os = "macos") {
            Tool::Security(path)
        } else {
            Tool::SecretTool(path)
        };
        Ok(Self::with_tool(tool))
    }

    pub fn with_tool(tool: Tool) -> Self {
        Self {
            tool,
            timeout: TIMEOUT,
            prepare: None,
            service: SERVICE,
        }
    }

    /// Items under another service attribute.
    pub fn service(mut self, service: &'static str) -> Self {
        self.service = service;
        self
    }

    /// Delete the item NAME. A missing item is not an error.
    pub fn delete(&self, name: &str) -> Result<(), StoreError> {
        let service = self.service;
        let (prog, args): (&Path, Vec<&str>) = match &self.tool {
            Tool::SecretTool(p) => (p, vec!["clear", "service", service, "name", name]),
            Tool::Security(p) => (
                p,
                vec!["delete-generic-password", "-s", service, "-a", name],
            ),
        };
        let out = run(prog, &args, None, self.timeout, self.prepare.as_ref())?;
        match (&self.tool, out.code) {
            (_, 0) => Ok(()),
            (Tool::Security(_), 44) => Ok(()),
            (Tool::SecretTool(_), _) if out.stderr.trim().is_empty() => Ok(()),
            _ => Err(classify(&out.stderr, out.code)),
        }
    }

    /// A shorter wait, for `paddock doctor`.
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    pub fn tool(&self) -> &Tool {
        &self.tool
    }
}

impl Store for Keyring {
    fn describe(&self) -> String {
        match &self.tool {
            Tool::SecretTool(_) => "system keyring (secret-tool)".into(),
            Tool::Security(_) => "login keychain (security)".into(),
        }
    }

    fn writable(&self) -> bool {
        true
    }

    fn get(&self, name: &str) -> Result<Secret, StoreError> {
        let service = self.service;
        let (prog, args): (&Path, Vec<&str>) = match &self.tool {
            Tool::SecretTool(p) => (p, vec!["lookup", "service", service, "name", name]),
            Tool::Security(p) => (
                p,
                vec!["find-generic-password", "-s", service, "-a", name, "-w"],
            ),
        };
        let out = run(prog, &args, None, self.timeout, self.prepare.as_ref())?;
        match (&self.tool, out.code) {
            (Tool::SecretTool(_), 0) if !out.stdout.is_empty() => Ok(out.stdout),
            // secret-tool says nothing when there is no such item.
            (Tool::SecretTool(_), _) if out.stderr.trim().is_empty() => Err(StoreError::Missing),
            (Tool::Security(_), 0) => {
                let v = out.stdout.as_bytes();
                let v = v.strip_suffix(b"\n").unwrap_or(v);
                if v.is_empty() {
                    return Err(StoreError::Missing);
                }
                Ok(Secret::new(v.to_vec()))
            }
            // errSecItemNotFound
            (Tool::Security(_), 44) => Err(StoreError::Missing),
            _ => Err(classify(&out.stderr, out.code)),
        }
    }

    fn put(&self, name: &str, value: &Secret) -> Result<(), StoreError> {
        if value.is_empty() || value.len() > MAX_VALUE {
            return Err(StoreError::Failed(format!(
                "a value must have 1 to {MAX_VALUE} bytes"
            )));
        }
        let out = match &self.tool {
            Tool::SecretTool(p) => {
                let label = if self.service == SERVICE {
                    format!("--label=paddock tack {name}")
                } else {
                    format!("--label=paddock key {name}")
                };
                run(
                    p,
                    &["store", &label, "service", self.service, "name", name],
                    Some(value),
                    self.timeout,
                    self.prepare.as_ref(),
                )?
            }
            Tool::Security(p) => {
                // Interactive mode reads the command from stdin, so the
                // value is not in the arguments; -X takes it as hex, so it
                // needs no quoting.
                let line = format!(
                    "add-generic-password -U -s {} -a {name} -l {}-{name} -X {}\n",
                    self.service,
                    self.service,
                    crate::manifest::hex(value.as_bytes())
                );
                let line = Secret::from(line);
                run(p, &["-i"], Some(&line), self.timeout, self.prepare.as_ref())?
            }
        };
        if out.code != 0 {
            return Err(classify(&out.stderr, out.code));
        }
        Ok(())
    }
}

/// A password manager command. It is only read.
pub struct CommandStore {
    pub cmd: Vec<String>,
    pub prepare: Option<Prepare>,
}

impl Store for CommandStore {
    fn describe(&self) -> String {
        format!(
            "command {}",
            self.cmd.first().map(String::as_str).unwrap_or("")
        )
    }

    fn writable(&self) -> bool {
        false
    }

    fn get(&self, _name: &str) -> Result<Secret, StoreError> {
        let Some(prog) = self.cmd.first() else {
            return Err(StoreError::Unavailable("no command".into()));
        };
        let none = |_: &mut Command| {};
        let prepare: &dyn Fn(&mut Command) = match &self.prepare {
            Some(p) => p.as_ref(),
            None => &none,
        };
        crate::credentials::run_command(&self.cmd, prepare).map_err(|e| {
            let msg = format!("{e:#}");
            if msg.starts_with("cannot run") {
                StoreError::Unavailable(format!("{msg}: is {prog} installed and on PATH?"))
            } else if msg.contains("printed nothing") {
                StoreError::Missing
            } else {
                StoreError::Failed(msg)
            }
        })
    }

    fn put(&self, _name: &str, _value: &Secret) -> Result<(), StoreError> {
        Err(StoreError::Failed(format!(
            "paddock cannot write into {}: put the value into your password manager yourself",
            self.describe()
        )))
    }
}

/// Map the error text of a keyring tool to a kind. The tools run with
/// LC_ALL=C, so the text is English.
fn classify(stderr: &str, code: i32) -> StoreError {
    let first = stderr.lines().next().unwrap_or("").trim();
    let detail = if first.is_empty() {
        format!("exit status {code}")
    } else {
        first.to_string()
    };
    let l = stderr.to_ascii_lowercase();
    // macOS: errSecInteractionNotAllowed (36, 25308), errSecAuthFailed (51).
    if l.contains("locked")
        || l.contains("dismissed")
        || l.contains("cancel")
        || l.contains("interaction is not allowed")
        || l.contains("prompt")
        || code == 36
        || code == 51
    {
        return StoreError::Locked(detail);
    }
    if l.contains("d-bus")
        || l.contains("dbus")
        || l.contains("org.freedesktop.secrets")
        || l.contains("secret service")
        || l.contains("no such secret collection")
        || l.contains("keychain could not be found")
    {
        return StoreError::Unavailable(format!(
            "{detail}. A Secret Service (gnome-keyring, KeePassXC) must run in your desktop session"
        ));
    }
    StoreError::Failed(detail)
}

struct Output {
    code: i32,
    stdout: Secret,
    stderr: String,
}

/// Run a keyring tool with a time limit. Its output is a `Secret`; its
/// input, when there is one, is copied into a `Secret` for the writer.
fn run(
    prog: &Path,
    args: &[&str],
    input: Option<&Secret>,
    timeout: Duration,
    prepare: Option<&Prepare>,
) -> Result<Output, StoreError> {
    let mut cmd = Command::new(prog);
    if let Some(p) = prepare {
        p(&mut cmd);
    }
    cmd.args(args)
        .env("LC_ALL", "C")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::env::spawn_retrying(&mut cmd)
        .map_err(|e| StoreError::Unavailable(format!("cannot run {}: {e}", prog.display())))?;
    if let (Some(v), Some(mut stdin)) = (input, child.stdin.take()) {
        let v = Secret::new(v.as_bytes().to_vec());
        // A thread, so that a tool that does not read cannot block paddock.
        std::thread::spawn(move || {
            let _ = stdin.write_all(v.as_bytes());
        });
    }
    let so = crate::broker::Pipe::read(child.stdout.take().expect("piped"), MAX_VALUE + 1);
    let se = crate::broker::Pipe::read(child.stderr.take().expect("piped"), 16 * 1024);
    let start = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(-1),
            Ok(None) => {}
            Err(e) => {
                return Err(StoreError::Failed(format!(
                    "cannot wait for {}: {e}",
                    prog.display()
                )))
            }
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StoreError::Locked(format!(
                "{} did not answer in {} seconds; an unlock prompt may be waiting",
                prog.display(),
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let deadline = (start + timeout).min(Instant::now() + crate::broker::PIPE_GRACE);
    let stdout = Secret::new(so.finish(deadline).0);
    let stderr = String::from_utf8_lossy(&se.finish(deadline).0).into_owned();
    if stdout.len() > MAX_VALUE {
        return Err(StoreError::Failed(format!(
            "{} printed more than {MAX_VALUE} bytes",
            prog.display()
        )));
    }
    Ok(Output {
        code,
        stdout,
        stderr,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    /// A store in memory, for the tests of other modules.
    #[derive(Default)]
    pub(crate) struct Memory {
        pub values: RefCell<BTreeMap<String, Vec<u8>>>,
        pub error: RefCell<Option<StoreError>>,
        /// put succeeds, but keeps nothing (a broken store).
        pub forget: bool,
        pub read_only: bool,
    }

    impl Store for Memory {
        fn describe(&self) -> String {
            "memory".into()
        }
        fn writable(&self) -> bool {
            !self.read_only
        }
        fn get(&self, name: &str) -> Result<Secret, StoreError> {
            if let Some(e) = self.error.borrow().clone() {
                return Err(e);
            }
            self.values
                .borrow()
                .get(name)
                .map(|v| Secret::new(v.clone()))
                .ok_or(StoreError::Missing)
        }
        fn put(&self, name: &str, value: &Secret) -> Result<(), StoreError> {
            if let Some(e) = self.error.borrow().clone() {
                return Err(e);
            }
            if !self.forget {
                self.values
                    .borrow_mut()
                    .insert(name.into(), value.as_bytes().to_vec());
            }
            Ok(())
        }
    }

    /// A fake `secret-tool` that keeps items as files in `dir`. `mode`:
    /// "ok", "locked", "nobus" or "forget".
    pub(crate) fn fake_secret_tool(dir: &Path, mode: &str) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let items = dir.join("items");
        std::fs::create_dir_all(&items).unwrap();
        let script = format!(
            r#"#!/bin/sh
items='{items}'
printf '%s\n' "$*" >> '{dir}/calls'
case '{mode}' in
  locked) printf '%s\n' "secret-tool: Cannot get secret of a locked object" >&2; exit 1 ;;
  nobus) printf '%s\n' "secret-tool: Cannot autolaunch D-Bus without X11 \$DISPLAY" >&2; exit 1 ;;
esac
cmd=$1; shift
case "$cmd" in
  store) shift; f="$items/$2-$4"; if [ '{mode}' = forget ]; then cat >/dev/null; else cat > "$f"; fi ;;
  lookup) f="$items/$2-$4"; [ -f "$f" ] || exit 1; cat "$f" ;;
  clear) rm -f "$items/$2-$4" ;;
  *) printf '%s\n' "unknown $cmd" >&2; exit 2 ;;
esac
"#,
            items = items.display(),
            dir = dir.display(),
        );
        let p = bin.join("secret-tool");
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn keyring(dir: &Path, mode: &str) -> Keyring {
        Keyring::with_tool(Tool::SecretTool(fake_secret_tool(dir, mode)))
    }

    #[test]
    fn the_keyring_keeps_and_returns_the_exact_bytes() {
        let t = tempfile::tempdir().unwrap();
        let k = keyring(t.path(), "ok");
        assert_eq!(k.get("GH_TOKEN").unwrap_err(), StoreError::Missing);
        let v = Secret::from("fake value with spaces \"quotes\" and $dollar");
        k.put("GH_TOKEN", &v).unwrap();
        assert_eq!(k.get("GH_TOKEN").unwrap().as_bytes(), v.as_bytes());
        // The value went in on stdin: it is in no argument.
        let calls = std::fs::read_to_string(t.path().join("calls")).unwrap();
        assert!(!calls.contains("fake value"), "{calls}");
        assert!(
            calls
                .contains("store --label=paddock tack GH_TOKEN service paddock-tack name GH_TOKEN"),
            "{calls}"
        );
    }

    #[test]
    fn a_locked_or_absent_keyring_is_named() {
        let t = tempfile::tempdir().unwrap();
        let e = keyring(t.path(), "locked").get("A").unwrap_err();
        assert!(matches!(e, StoreError::Locked(_)), "{e:?}");
        assert!(e.to_string().contains("unlock it"), "{e}");
        let t = tempfile::tempdir().unwrap();
        let e = keyring(t.path(), "nobus")
            .put("A", &Secret::from("x"))
            .unwrap_err();
        assert!(matches!(e, StoreError::Unavailable(_)), "{e:?}");
        assert!(e.to_string().contains("Secret Service"), "{e}");
        let gone = Keyring::with_tool(Tool::SecretTool(t.path().join("no-such-tool")));
        assert!(matches!(
            gone.get("A").unwrap_err(),
            StoreError::Unavailable(_)
        ));
        let env = HostEnv::from_parts(t.path().into(), "demo".into(), 1, 1, BTreeMap::new());
        let e = Keyring::find(&env).err().unwrap();
        assert!(e.to_string().contains("not installed"), "{e}");
    }

    #[test]
    fn a_tool_that_hangs_is_stopped_and_named() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("secret-tool");
        std::fs::write(&p, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let k = Keyring::with_tool(Tool::SecretTool(p)).timeout(Duration::from_millis(300));
        let start = Instant::now();
        let e = k.get("A").unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(e.to_string().contains("did not answer"), "{e}");
    }

    #[test]
    fn the_macos_keychain_gets_the_value_on_stdin_as_hex() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("security");
        let log = t.path().join("log");
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nprintf \"%s\\n\" \"ARGS $*\" >> '{0}'\nif [ \"$1\" = -i ]; then cat >> '{0}'; exit 0; fi\nif [ \"$5\" = MISSING ]; then exit 44; fi\nprintf 'fake-v\\n'\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let k = Keyring::with_tool(Tool::Security(p));
        k.put("A", &Secret::from("fake-v")).unwrap();
        assert_eq!(k.get("A").unwrap().as_str(), "fake-v");
        assert_eq!(k.get("MISSING").unwrap_err(), StoreError::Missing);
        let log = std::fs::read_to_string(&log).unwrap();
        assert!(log.contains("ARGS -i\nadd-generic-password -U -s paddock-tack -a A -l paddock-tack-A -X 66616b652d76\n"), "{log}");
        assert!(!log.contains("fake-v"), "{log}");
    }

    #[test]
    fn a_command_store_reads_and_refuses_to_write() {
        let c = CommandStore {
            cmd: vec!["printf".into(), "fake-from-manager".into()],
            prepare: None,
        };
        assert_eq!(c.get("A").unwrap().as_str(), "fake-from-manager");
        assert!(!c.writable());
        assert!(c
            .put("A", &Secret::from("x"))
            .unwrap_err()
            .to_string()
            .contains("password manager"));
        let gone = CommandStore {
            cmd: vec!["/no/such/manager".into()],
            prepare: None,
        };
        assert!(matches!(
            gone.get("A").unwrap_err(),
            StoreError::Unavailable(_)
        ));
        let empty = CommandStore {
            cmd: vec!["true".into()],
            prepare: None,
        };
        assert_eq!(empty.get("A").unwrap_err(), StoreError::Missing);
    }
}
