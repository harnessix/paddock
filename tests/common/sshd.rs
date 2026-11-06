//! A stand-in Mac for the tests of the worked example `mac`: an sshd of
//! the test user on a loopback port, with a host key and a user key made
//! for the test, an ssh agent that holds the user key, and a folder that
//! stands for the home on the Mac. Nothing of the user's own ssh
//! configuration is read: the ssh commands of the tests pass
//! `-F /dev/null`, and sshd reads the configuration file of the test.
//!
//! The known-hosts line that the tests write is what `ssh-keyscan -p 22
//! HOST` prints for a Mac: `HOST ssh-ed25519 KEY`, under the alias that
//! the device's ssh command names with `HostKeyAlias`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The tools that the stand-in needs. None of them: the test skips.
pub fn missing_tool() -> Option<&'static str> {
    ["sshd", "ssh", "ssh-keygen", "ssh-agent", "ssh-add", "rsync"]
        .into_iter()
        .find(|t| which(t).is_none())
}

/// The absolute path of a tool: sshd must be started by its full path.
pub fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/usr/local/sbin", "/sbin"].map(PathBuf::from))
        .map(|d| d.join(tool))
        .find(|p| p.is_file())
}

pub struct Sshd {
    sshd: Child,
    agent: Child,
    /// The port on 127.0.0.1.
    pub port: u16,
    /// The folder of the keys and the configuration.
    pub dir: PathBuf,
    /// The private user key.
    pub key: PathBuf,
    /// The public user key: what the credential names.
    pub key_pub: PathBuf,
    /// The socket of the ssh agent that holds the user key.
    pub agent_sock: PathBuf,
    /// The folder on the "Mac": the `MAC_DIR` of the device, where each
    /// caller gets `paddock/<caller>`.
    pub mac_home: PathBuf,
    /// The user that ssh logs in as: the test user, since an sshd of a
    /// user can only start sessions of that user.
    pub user: String,
    /// The public host key of the sshd, one line (`ssh-ed25519 AAAA...`).
    pub host_key: String,
    pub log: PathBuf,
}

impl Sshd {
    /// Start the stand-in in `dir`. None when a tool is missing (the
    /// caller prints why with `missing_tool`).
    pub fn start(dir: &Path) -> Option<Self> {
        if missing_tool().is_some() {
            return None;
        }
        std::fs::create_dir_all(dir).unwrap();
        let hostkey = dir.join("ssh_host_ed25519_key");
        let key = dir.join("id_test");
        for k in [&hostkey, &key] {
            let ok = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "paddock-test", "-f"])
                .arg(k)
                .status()
                .unwrap()
                .success();
            assert!(ok, "ssh-keygen -f {}", k.display());
        }
        let key_pub = PathBuf::from(format!("{}.pub", key.display()));
        let host_key = std::fs::read_to_string(format!("{}.pub", hostkey.display()))
            .unwrap()
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let mac_home = dir.join("machome");
        std::fs::create_dir_all(&mac_home).unwrap();
        let user = whoami();
        // The authorized key passes the PATH and the toolchain folders of
        // the test, so cargo on the stand-in Mac is the test's cargo.
        let mut environment = vec![format!(
            "PATH={}",
            std::env::var("PATH").unwrap_or_default()
        )];
        for v in ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"] {
            if let Ok(val) = std::env::var(v) {
                environment.push(format!("{v}={val}"));
            }
        }
        let env_opts: Vec<String> = environment
            .iter()
            .map(|e| format!("environment=\"{e}\""))
            .collect();
        std::fs::write(
            dir.join("authorized_keys"),
            format!(
                "{} {}",
                env_opts.join(","),
                std::fs::read_to_string(&key_pub).unwrap().trim()
            ),
        )
        .unwrap();
        let port = free_port();
        let log = dir.join("sshd.log");
        std::fs::write(
            dir.join("sshd_config"),
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {hk}\nAuthorizedKeysFile {ak}\nPidFile none\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nStrictModes no\nPermitUserEnvironment yes\nLogLevel VERBOSE\nAllowUsers {user}\n",
                hk = hostkey.display(),
                ak = dir.join("authorized_keys").display(),
            ),
        )
        .unwrap();
        let sshd = Command::new(which("sshd").unwrap())
            .args(["-D", "-e", "-f"])
            .arg(dir.join("sshd_config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .expect("sshd");
        let agent_sock = dir.join("agent.sock");
        let agent = Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&agent_sock)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ssh-agent");
        let me = Self {
            sshd,
            agent,
            port,
            dir: dir.to_path_buf(),
            key,
            key_pub,
            agent_sock,
            mac_home,
            user,
            host_key,
            log,
        };
        wait_for("the agent socket", Duration::from_secs(10), || {
            me.agent_sock.exists()
        });
        assert!(Command::new("ssh-add")
            .arg(&me.key)
            .env("SSH_AUTH_SOCK", &me.agent_sock)
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        // Not a TCP connect: sshd counts a connection without a login as
        // a penalty against the address, and delays the next ones.
        wait_for("sshd to listen", Duration::from_secs(10), || {
            me.log_text().contains("Server listening")
        });
        Some(me)
    }

    /// The known-hosts line of the Mac under `alias`, as `ssh-keyscan`
    /// would print it for port 22.
    pub fn known_hosts_line(&self, alias: &str) -> String {
        format!("{alias} {}\n", self.host_key)
    }

    /// The `MAC_DIR` of the device: the copies land in `paddock/<caller>`
    /// under the stand-in home.
    pub fn mac_dir(&self) -> PathBuf {
        self.mac_home.join("paddock")
    }

    /// The text of the sshd log so far.
    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The ssh sessions that sshd has open now, by its log: one
    /// "Accepted publickey" per session minus the "Closing connection"
    /// lines, so a build that still runs on the stand-in Mac counts 1.
    pub fn open_sessions(&self) -> usize {
        let t = self.log_text();
        let opened = t.matches("Accepted publickey").count();
        let closed = t.matches("Closing connection").count();
        opened.saturating_sub(closed)
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.sshd.kill();
        let _ = self.sshd.wait();
        let _ = self.agent.kill();
        let _ = self.agent.wait();
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn whoami() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| {
            String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_string()
        })
}

pub fn wait_for(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < within,
            "{what} did not happen within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A small crate for the stand-in to check and test: one passing test, a
/// name that is a canary for the audit log (which must never hold a body).
pub const CRATE_NAME: &str = "canary7e1f";

pub fn write_crate(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{CRATE_NAME}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn two() -> u32 { 2 }\n#[cfg(test)]\nmod tests { #[test] fn two_is_two() { assert_eq!(super::two(), 2); } }\n",
    )
    .unwrap();
}
