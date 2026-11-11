//! Host-only peer configuration and byte-stream transport.
//!
//! Transport arguments must never enter broker messages or diagnostic output.

pub mod cache;
pub mod client;
pub mod config;
pub mod identity;
pub mod jobs;
pub mod prefer;
pub mod protocol;
pub mod sandbox;
pub mod serve;
pub mod snapshot;

use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

/// Peer settings are deliberately neither serializable nor printable.
pub struct Peers(BTreeMap<String, Transport>);

struct Transport(Vec<String>);

impl Peers {
    /// Read only the host user configuration, never a project manifest.
    pub fn load(env: &HostEnv) -> Result<Self> {
        let file = env.paddock_config().join("config.toml");
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self(BTreeMap::new())),
            Err(_) => bail!("cannot read exclave user configuration"),
        };
        // Parser diagnostics include source text, which may contain transport secrets.
        let doc: toml::Value =
            toml::from_str(&text).map_err(|_| anyhow::anyhow!("invalid user configuration"))?;
        let mut peers = BTreeMap::new();
        if let Some(value) = doc.get("exclave").and_then(|v| v.get("peers")) {
            let table = value.as_table().context("exclave peers must be a table")?;
            for (name, peer) in table {
                if name.is_empty()
                    || name.len() > 64
                    || !name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                {
                    bail!("invalid exclave peer name");
                }
                let argv = peer
                    .get("transport")
                    .and_then(|v| v.as_array())
                    .context("peer transport must be a nonempty list of strings")?;
                let argv: Vec<String> = argv
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .filter(|s| !s.contains('\0'))
                            .map(str::to_owned)
                            .context("peer transport must be a nonempty list of strings")
                    })
                    .collect::<Result<_>>()?;
                if argv.first().is_none_or(|s| s.is_empty()) {
                    bail!("peer transport must be a nonempty list of strings");
                }
                peers.insert(name.clone(), Transport(argv));
            }
        }
        Ok(Self(peers))
    }

    /// Choose the explicitly named peer or the first configured peer.
    pub fn select<'a>(&'a self, name: Option<&'a str>) -> Result<&'a str> {
        match name {
            Some(name) if self.0.contains_key(name) => Ok(name),
            Some(_) => bail!("unknown exclave peer"),
            None => self
                .0
                .keys()
                .next()
                .map(String::as_str)
                .context("no exclave peer configured"),
        }
    }

    /// Start a configured byte stream. Do not expose command or OS error text.
    pub fn connect(&self, name: &str) -> Result<Child> {
        let transport = self.0.get(name).context("unknown exclave peer")?;
        Command::new(&transport.0[0])
            .process_group(0)
            .args(&transport.0[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| anyhow::anyhow!("could not start exclave transport"))
    }
}

/// Preserve a project-relative working directory without shell interpolation.
pub fn command_in(cwd: &str, command: Vec<String>) -> Result<Vec<String>> {
    use std::path::{Component, Path};
    if !cwd.is_empty()
        && !Path::new(cwd)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        bail!("invalid working directory");
    }
    if command.is_empty() {
        bail!("missing command");
    }
    if cwd.is_empty() {
        return Ok(command);
    }
    let mut result = vec![
        "/bin/sh".into(),
        "-c".into(),
        "cd \"$1\" || exit; shift; exec \"$@\"".into(),
        "exclave".into(),
        format!("/work/{cwd}"),
    ];
    result.extend(command);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn env(dir: &std::path::Path) -> HostEnv {
        HostEnv::from_parts(dir.to_path_buf(), "test".into(), 1, 1, BTreeMap::new())
    }

    #[test]
    fn host_configuration_opens_a_private_byte_stream() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(
            env.paddock_config().join("config.toml"),
            "[exclave.peers.test]\ntransport = ['/bin/cat']\n",
        )
        .unwrap();
        let mut child = Peers::load(&env).unwrap().connect("test").unwrap();
        child.stdin.take().unwrap().write_all(b"hello").unwrap();
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(output, "hello");
    }

    #[test]
    fn transport_canary_is_absent_from_errors() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        let file = env.paddock_config().join("config.toml");
        let canary = "EXCLAVE_SECRET_CANARY";
        std::fs::write(
            &file,
            format!("[exclave.peers.test]\ntransport = ['/{canary}']\n"),
        )
        .unwrap();
        let err = Peers::load(&env).unwrap().connect("test").err().unwrap();
        assert!(!format!("{err:#}").contains(canary));
        std::fs::write(
            &file,
            format!("[exclave.peers.test]\ntransport = ['{canary}'\n"),
        )
        .unwrap();
        let err = Peers::load(&env).err().unwrap();
        assert!(!format!("{err:#}").contains(canary));
    }
}
