//! Host-owned opt-in routing of common build commands.
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};

pub fn load(env: &HostEnv) -> Result<Vec<String>> {
    let text = match std::fs::read_to_string(env.paddock_config().join("config.toml")) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => bail!("cannot read exclave user configuration"),
    };
    let doc = crate::manifest::user_config_doc(&text)?;
    let Some(value) = doc.get("exclave").and_then(|v| v.get("prefer")) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .context("exclave prefer must be a list")?
        .iter()
        .map(|v| {
            let s = v.as_str().context("invalid exclave preference")?;
            if !matches!(s, "build" | "test") {
                bail!("invalid exclave preference");
            }
            Ok(s.to_owned())
        })
        .collect()
}

/// Shell wrapper contains only public routing choices, never peer settings.
pub fn cargo_wrapper(prefer: &[String]) -> String {
    let patterns = prefer.join("|");
    format!(
        r##"#!/bin/sh
if [ -z "${{PADDOCK_EXCLAVE_LOCAL:-}}" ]; then
  case "${{1:-}}" in {patterns}) exec paddock host exclave run -- cargo "$@";; esac
fi
old=$PATH
PATH=
oldifs=$IFS
IFS=:
for dir in $old; do
  if [ "$dir" != /run/paddock/bin ]; then PATH="${{PATH:+$PATH:}}$dir"; fi
done
IFS=$oldifs
real=$(command -v cargo) || {{ echo 'cargo not found' >&2; exit 127; }}
PATH=$old
export PATH
exec "$real" "$@"
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[test]
    fn cargo_wrapper_routes_only_selected_commands_and_can_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("wrapper");
        std::fs::write(&script, cargo_wrapper(&["build".into(), "test".into()])).unwrap();
        for (name, body) in [
            ("paddock", "#!/bin/sh\nprintf 'remote:%s' \"$*\"\n"),
            ("cargo", "#!/bin/sh\nprintf 'local:%s' \"$*\"\n"),
        ] {
            let file = dir.path().join(name);
            std::fs::write(&file, body).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let run = |arg: &str, fallback: bool| {
            let mut c = Command::new("/bin/sh");
            c.arg(&script).arg(arg).env_clear().env("PATH", dir.path());
            if fallback {
                c.env("PADDOCK_EXCLAVE_LOCAL", "1");
            }
            c.output().unwrap().stdout
        };
        assert_eq!(run("test", false), b"remote:host exclave run -- cargo test");
        assert_eq!(run("check", false), b"local:check");
        assert_eq!(run("build", true), b"local:build");
    }
}
