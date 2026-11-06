//! The worked example `mac`: workers in a pasture build and test their
//! worktree on a Mac through a device. `paddock device add --example mac`
//! writes the three tables (`[credentials.mac-ssh]`, `[devices.mac]`,
//! `[devices.mac.sandbox]`) and the script `build.sh`, and prints what the
//! Mac side needs. The README shows the same tables. The test suite
//! parses them and runs the script against a stand-in sshd.
//!
//! What the device reaches: one host and port (`connect`), the key through
//! a filtering ssh agent (the key file never enters the device sandbox),
//! the host key file read-only under the Mac's name (`HostKeyAlias`), and
//! the caller's worktree read-only. Nothing else of the host.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The device name.
pub const DEVICE: &str = "mac";
/// The credential name.
pub const CREDENTIAL: &str = "mac-ssh";
/// The pasture of the example.
pub const PASTURE: &str = "workers";
/// The local port inside the device sandbox that reaches the Mac's sshd.
pub const LOCAL_PORT: u16 = 2222;
/// The line to add to a `[pastures.NAME]` table. `add` never writes it.
pub const PASTURE_LINE: &str = "devices = [\"mac\"]";

/// `build.sh`: one line in (`test` or `check`), the caller's worktree to
/// the Mac with rsync, then cargo there, the output streamed back.
///
/// `read -r what || true`: `printf test` sends no newline, and `read`
/// then returns 1 with the word already in the variable. `mkdir -p` first:
/// rsync makes the last folder of a target, not its parents. `exec` for
/// the last ssh: the script's process is the ssh process, so a cancel
/// (the caller closes the file) ends the session on the Mac at once.
/// `~/.cargo/bin` on the PATH of the Mac side: a login shell without a
/// terminal does not read the profile that puts rustup's cargo there.
pub const BUILD_SH: &str = r#"#!/bin/sh
# The device `mac` of paddock: build or test the caller's worktree on the
# Mac. Request: one line, "test" or "check". Anything else is rejected.
#
# paddock sets PADDOCK_CALLER (the sandbox id) and PADDOCK_CALLER_MOUNT
# (its project, read-only). The table sets SSH (the ssh command that
# reaches the Mac through the connect port), MAC (the account on the Mac)
# and MAC_DIR (the folder on the Mac that holds one copy per caller;
# "paddock" in the home by default).
set -eu
read -r what || true
case "$what" in
  test|check) ;;
  *) echo "request must be test or check" >&2; exit 65 ;;
esac
dest="${MAC_DIR:-paddock}/$PADDOCK_CALLER"
$SSH "$MAC" "mkdir -p '$dest'"
rsync -a --delete --exclude .git -e "$SSH" "$PADDOCK_CALLER_MOUNT/" "$MAC:$dest/"
exec $SSH "$MAC" "PATH=\"\$HOME/.cargo/bin:\$PATH\"; export PATH; cd '$dest' && cargo $what 2>&1"
"#;

/// The example with its two names, and what a test overrides.
#[derive(Debug, Clone)]
pub struct MacExample {
    /// The Mac, as the host resolves it: `mac.local`.
    pub host: String,
    /// The account on the Mac.
    pub user: String,
    /// The port of sshd on the Mac.
    pub port: u16,
    /// The home of the user on this machine: the absolute paths of the
    /// files that the device sandbox binds. On macOS the device sandbox
    /// has a private home, so `~` in the ssh command would miss them.
    pub home: PathBuf,
    /// The public key file that the filtering agent signs with.
    pub key: String,
    /// The host key file of the Mac, under its name.
    pub known_hosts: String,
    /// The script.
    pub script: String,
    /// The folder of the script, bound read-only.
    pub script_dir: String,
    /// More options of the ssh command (a test adds `-F /dev/null`).
    pub ssh_extra: Vec<String>,
    /// More variables of the device sandbox (a test sets `MAC_DIR`).
    pub env_extra: BTreeMap<String, String>,
    /// `connect` reaches this host and port (a test: the loopback sshd).
    pub connect_host: String,
    pub connect_port: u16,
}

impl MacExample {
    /// The usual setup: the key `~/.ssh/id_mac`, the host key
    /// file `~/.ssh/known_hosts_mac`, the script in the configuration
    /// folder.
    pub fn new(host: &str, user: &str, home: &Path) -> Self {
        let h = home.display();
        Self {
            host: host.to_string(),
            user: user.to_string(),
            port: 22,
            home: home.to_path_buf(),
            key: "~/.ssh/id_mac.pub".into(),
            known_hosts: format!("{h}/.ssh/known_hosts_mac"),
            script: "~/.config/paddock/devices/mac/build.sh".into(),
            script_dir: "~/.config/paddock/devices/mac".into(),
            ssh_extra: Vec::new(),
            env_extra: BTreeMap::new(),
            connect_host: host.to_string(),
            connect_port: 22,
        }
    }

    /// The ssh command of the table: through the connect port, the host
    /// key under the Mac's name, no prompt ever.
    pub fn ssh_command(&self) -> String {
        let mut s = format!(
            "ssh -p {LOCAL_PORT} -o HostKeyAlias={} -o UserKnownHostsFile={} -o StrictHostKeyChecking=yes -o BatchMode=yes",
            self.host, self.known_hosts
        );
        for e in &self.ssh_extra {
            s.push(' ');
            s.push_str(e);
        }
        s
    }

    /// The three tables, with comments, as `add` writes them.
    pub fn tables(&self) -> String {
        let mut env = vec![
            ("SSH".to_string(), self.ssh_command()),
            ("MAC".to_string(), format!("{}@127.0.0.1", self.user)),
        ];
        env.extend(self.env_extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        let env: Vec<String> = env
            .iter()
            .map(|(k, v)| format!("{k} = {}", toml_string(v)))
            .collect();
        format!(
            r#"[credentials.{CREDENTIAL}]
via = "agent"
kind = "ssh"
keys = [{key}]                        # the agent signs with this key only; the key file never enters a sandbox

[devices.{DEVICE}]
shape = "exec"
run = "sandbox"
command = [{script}]
enabled = true
grant = ["pasture:{PASTURE}"]       # the members of [pastures.{PASTURE}] see it
mode = "ask"                        # the first request of each sandbox shows a card
stream = true                       # the cargo output comes line by line
max_request = "4K"
max_response = "16M"
timeout = 1200
concurrency = 1                     # a second request waits for the first

[devices.{DEVICE}.sandbox]
network = "none"
connect = ["{LOCAL_PORT}:{chost}:{cport}"]   # 127.0.0.1:{LOCAL_PORT} inside reaches {chost}:{cport} from the host
ro = [{known_hosts}, {script_dir}]
credentials = ["{CREDENTIAL}"]
caller = ["project:ro"]             # the worktree at /run/paddock/caller/project
env = {{ {env} }}
mem = "512M"
"#,
            key = toml_string(&self.key),
            script = toml_string(&self.script),
            chost = self.connect_host,
            cport = self.connect_port,
            known_hosts = toml_string(&self.known_hosts),
            script_dir = toml_string(&self.script_dir),
            env = env.join(", "),
        )
    }

    /// The path of the script on this machine.
    pub fn script_path(&self) -> PathBuf {
        super::config::expand_home(&self.script, &self.home)
    }

    /// The path of the host key file on this machine.
    pub fn known_hosts_path(&self) -> PathBuf {
        super::config::expand_home(&self.known_hosts, &self.home)
    }

    /// The path of the public key on this machine.
    pub fn key_path(&self) -> PathBuf {
        super::config::expand_home(&self.key, &self.home)
    }

    /// What the Mac side needs, step by step, after `add` wrote the
    /// tables. `host_key` is the line that `ssh-keyscan` found, or the
    /// reason it found none.
    pub fn steps(&self, host_key: Result<&str, &str>) -> String {
        let MacExample {
            host, user, port, ..
        } = self;
        let key_priv = self.key.trim_end_matches(".pub").to_string();
        let kh = &self.known_hosts;
        let p = if *port == 22 {
            String::new()
        } else {
            format!(" -p {port}")
        };
        let scan = match host_key {
            Ok(line) => format!(
                "   ssh-keyscan found this line. Compare it with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the Mac, then add it:\n     {line}\n"
            ),
            Err(why) => format!(
                "   ssh-keyscan could not read the key now ({why}). Run it when the Mac is on, and compare the line with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the Mac.\n"
            ),
        };
        format!(
            r#"What the Mac side needs, in this order:

1. On this machine: a key for the Mac, held by your ssh agent. The device gets an agent socket that signs with this key only; the key file never enters a sandbox.
     ssh-keygen -t ed25519 -N "" -f {key_priv} -C paddock-mac
     ssh-add {key_priv}
   paddock must start where SSH_AUTH_SOCK names that agent.

2. On the Mac ({host}): Remote Login on for the account {user} (System Settings > General > Sharing), and the public key in its authorized_keys:
     ssh-copy-id -i {key_priv}.pub{p} {user}@{host}

3. On this machine: the host key of the Mac in {kh}, under the name {host}. The device checks it under that name although it connects to 127.0.0.1:{LOCAL_PORT}.
     ssh-keyscan{p} -t ed25519 {host} >> {kh}
{scan}
4. On the Mac: rust. rustup puts cargo in ~/.cargo/bin, and the script puts that folder on the PATH itself, so a plain rustup install is enough:
     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

5. Grant the device to the pasture of the workers. This command never writes [pastures]; add the line yourself:
     [pastures.{PASTURE}]
     members = ["codex", "claude"]
     {PASTURE_LINE}

6. Check it from a folder with a Cargo.toml (the check uses the current folder as the caller's project):
     paddock device check {DEVICE} --send check

Then a worker in the pasture runs:
     printf test | paddock-host device {DEVICE} -
"#
        )
    }
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The host key line of `host`, as `ssh-keyscan -p PORT -t ed25519 HOST`
/// prints it: `host ssh-ed25519 AAAA...`. The first key line of the
/// output. The error is the reason: no `ssh-keyscan`, no answer.
pub fn keyscan(host: &str, port: u16) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let child = Command::new("ssh-keyscan")
        .args(["-T", "8", "-p", &port.to_string(), "-t", "ed25519", host])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ssh-keyscan: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("ssh-keyscan: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    match keyscan_line(&text) {
        Some(l) => Ok(l),
        None => Err(format!(
            "no answer from {host}:{port}: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .find(|l| !l.starts_with('#') && !l.trim().is_empty())
                .unwrap_or("nothing")
                .trim()
        )),
    }
}

/// The first key line of an `ssh-keyscan` output: not a comment, three
/// fields, the second a key type.
pub fn keyscan_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find_map(|l| {
        if l.starts_with('#') {
            return None;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() >= 3 && (f[1].starts_with("ssh-") || f[1].starts_with("ecdsa-")))
            .then(|| l.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::config::{Mode, Net, ParseEnv, Run, Scope, Shape};
    use crate::devices::DevicesCfg;

    fn example() -> MacExample {
        MacExample::new("mac.local", "me", Path::new("/home/demo"))
    }

    #[test]
    fn the_tables_parse_to_the_design() {
        let ex = example();
        let text = ex.tables();
        let cfg = DevicesCfg::parse_with(
            &text,
            ParseEnv {
                home: Some(Path::new("/home/demo")),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.enabled, ["mac"]);
        let d = &cfg.user[0];
        assert_eq!(d.name, "mac");
        assert_eq!(d.shape, Shape::Exec);
        assert_eq!(d.run, Run::Sandbox);
        assert_eq!(d.scope, Scope::Run);
        assert_eq!(d.mode, Mode::Ask);
        assert!(d.stream);
        assert_eq!(
            d.command,
            ["/home/demo/.config/paddock/devices/mac/build.sh"]
        );
        assert_eq!(d.limits.max_request, 4 << 10);
        assert_eq!(d.limits.max_response, 16 << 20);
        assert_eq!(d.limits.timeout, 1200);
        assert_eq!(d.limits.concurrency, 1);
        assert_eq!(
            d.grant
                .as_ref()
                .map(|g| g.iter().map(|g| g.render()).collect::<Vec<_>>()),
            Some(vec!["pasture:workers".to_string()])
        );
        let p = d.sandbox.as_ref().unwrap();
        assert_eq!(p.network, Net::None);
        assert_eq!(p.connect.len(), 1);
        assert_eq!(p.connect[0].render(), "2222:mac.local:22");
        assert_eq!(
            p.ro,
            [
                PathBuf::from("/home/demo/.ssh/known_hosts_mac"),
                PathBuf::from("/home/demo/.config/paddock/devices/mac")
            ]
        );
        assert_eq!(p.credentials, ["mac-ssh"]);
        assert_eq!(p.caller.len(), 1);
        assert_eq!(p.mem, Some(512 << 20));
        assert_eq!(p.env["MAC"], "me@127.0.0.1");
        assert_eq!(
            p.env["SSH"],
            "ssh -p 2222 -o HostKeyAlias=mac.local -o UserKnownHostsFile=/home/demo/.ssh/known_hosts_mac -o StrictHostKeyChecking=yes -o BatchMode=yes"
        );
        // The credential: an ssh agent that signs with one key.
        let creds = crate::credentials::parse(&text).unwrap();
        let c = &creds["mac-ssh"];
        assert_eq!(c.via, crate::credentials::Via::Agent);
        assert_eq!(c.kind(), crate::credentials::Kind::Ssh);
        assert_eq!(c.keys, ["~/.ssh/id_mac.pub"]);
        // No [pastures] table: the user adds the line.
        assert!(!text.contains("\n[pastures"));
        assert_eq!(PASTURE_LINE, "devices = [\"mac\"]");
    }

    #[test]
    fn a_test_override_renders_its_own_connect_and_env() {
        let mut ex = example();
        ex.connect_host = "127.0.0.1".into();
        ex.connect_port = 40022;
        ex.ssh_extra.push("-F /dev/null".into());
        ex.env_extra
            .insert("MAC_DIR".into(), "/tmp/machome/paddock".into());
        let text = ex.tables();
        assert!(
            text.contains("connect = [\"2222:127.0.0.1:40022\"]"),
            "{text}"
        );
        assert!(
            text.contains("MAC_DIR = \"/tmp/machome/paddock\""),
            "{text}"
        );
        assert!(ex.ssh_command().ends_with("-o BatchMode=yes -F /dev/null"));
    }

    #[test]
    fn the_script_rejects_with_65_and_streams_through_exec() {
        assert!(BUILD_SH.starts_with("#!/bin/sh\n"));
        assert!(BUILD_SH.contains("exit 65"));
        assert!(BUILD_SH.contains("request must be test or check"));
        assert!(BUILD_SH.contains("read -r what || true"));
        assert!(BUILD_SH.contains("mkdir -p"));
        assert!(BUILD_SH.contains("--exclude .git"));
        assert!(BUILD_SH.contains("\nexec $SSH"));
        assert!(BUILD_SH.contains("$PADDOCK_CALLER_MOUNT/"));
        assert!(BUILD_SH.ends_with('\n'));
    }

    #[test]
    fn the_steps_name_the_host_the_user_and_the_files() {
        let ex = MacExample::new("mac.local", "me", Path::new("/home/demo"));
        let s = ex.steps(Ok("mac.local ssh-ed25519 AAAAC3 host"));
        assert!(
            s.contains("ssh-copy-id -i ~/.ssh/id_mac.pub me@mac.local"),
            "{s}"
        );
        assert!(s.contains("ssh-keyscan -t ed25519 mac.local >> /home/demo/.ssh/known_hosts_mac"));
        assert!(s.contains("     mac.local ssh-ed25519 AAAAC3 host\n"));
        assert!(s.contains("ssh-add ~/.ssh/id_mac\n"));
        assert!(s.contains("paddock device check mac --send check"));
        assert!(s.contains("printf test | paddock-host device mac -"));
        assert!(s.contains("devices = [\"mac\"]"));
        let s = ex.steps(Err("no answer from mac.local:22: nothing"));
        assert!(s.contains(
            "ssh-keyscan could not read the key now (no answer from mac.local:22: nothing)"
        ));
        let mut ex = ex;
        ex.port = 2200;
        let s = ex.steps(Err("x"));
        assert!(
            s.contains("ssh-copy-id -i ~/.ssh/id_mac.pub -p 2200 me@mac.local"),
            "{s}"
        );
        assert!(
            s.contains("ssh-keyscan -p 2200 -t ed25519 mac.local"),
            "{s}"
        );
    }

    #[test]
    fn keyscan_lines_skip_comments_and_take_the_first_key() {
        let out = "# mac.local:22 SSH-2.0-OpenSSH_9.9\nmac.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 \n# done\n";
        assert_eq!(
            keyscan_line(out).as_deref(),
            Some("mac.local ssh-ed25519 AAAAC3NzaC1lZDI1NTE5")
        );
        assert_eq!(keyscan_line("# only a comment\n"), None);
        assert_eq!(keyscan_line(""), None);
        assert_eq!(keyscan_line("mac.local no-key"), None);
    }

    #[test]
    fn keyscan_of_a_closed_port_is_an_error_that_names_the_host() {
        if std::process::Command::new("ssh-keyscan")
            .arg("-h")
            .output()
            .is_err()
        {
            eprintln!("skipped: no ssh-keyscan");
            return;
        }
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let e = keyscan("127.0.0.1", port).unwrap_err();
        assert!(
            e.starts_with(&format!("no answer from 127.0.0.1:{port}")),
            "{e}"
        );
    }
}
