//! `paddock doctor --escape --device NAME`: the escape probe of a device
//! sandbox. The probe (probe.sh of this folder) runs as the program of the
//! named device, in a device sandbox with the profile of its table, and
//! tries what a device program must not be able to do: see the host's
//! home or its runtime folder, read the private key of a credential, see
//! more of `~/.ssh` than the table binds, reach the host's loopback or the
//! LAN, write the caller's project, find `paddock-host`. The connect ports
//! of the table must answer inside. One line per check, as `paddock
//! doctor --escape` prints them.
//!
//! The probe is a shell script, so a device whose command is a script
//! works the same way; nothing of `paddock-host` is involved.

use super::config::DeviceDef;
use super::program::{Caller, Collect, Program};
use super::sandbox::{Instance, InstanceCtx, Launcher};
use super::{Call, DeviceCtx, DevicesCfg};
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// The probe script.
pub const PROBE: &str = include_str!("probe.sh");

/// Run the probe in the device sandbox of `name`. Prints the lines to
/// `out` and returns the number of failed checks. The instance is one of
/// a check: its caller is `probe-<time>-<pid>`, its project the current
/// folder, and it is stopped before the return.
pub fn run(env: &HostEnv, name: &str, out: &mut dyn Write) -> Result<usize> {
    let cfg = DevicesCfg::load(env)?;
    let Some(def) = cfg.user.iter().find(|d| d.name == name) else {
        bail!(
            "no [devices.{name}] table in {}. `paddock device ls` lists the devices",
            env.paddock_config().join("config.toml").display()
        );
    };
    if def.run != super::config::Run::Sandbox {
        bail!(
            "[devices.{name}]: run = \"{}\" has no device sandbox to probe",
            def.run.name()
        );
    }
    if let Some(why) = def.not_yet() {
        bail!("{why}");
    }
    let probe_dir = env.paddock_cache().join("device-probe").join(format!(
        "{}-{}",
        crate::state::now(),
        std::process::id()
    ));
    crate::privfs::dir(&probe_dir)
        .with_context(|| format!("cannot create {}", probe_dir.display()))?;
    let script = probe_dir.join("probe.sh");
    std::fs::write(&script, PROBE).with_context(|| format!("cannot write {}", script.display()))?;
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    let result = run_with(env, def, &probe_dir, &script, out);
    let _ = std::fs::remove_dir_all(&probe_dir);
    result
}

fn run_with(
    env: &HostEnv,
    def: &DeviceDef,
    probe_dir: &std::path::Path,
    script: &std::path::Path,
    out: &mut dyn Write,
) -> Result<usize> {
    // A listener on the host's loopback that the probe must not reach.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let probe = probe_def(env, def, probe_dir, script, port);
    let caller = format!("probe-{}-{}", crate::state::now(), std::process::id());
    let ctx = InstanceCtx {
        dir: super::sandbox::state::instance_dir(env, &probe.name, &caller),
        caller: caller.clone(),
        project: std::env::current_dir().unwrap_or_else(|_| env.home.clone()),
        harness: "doctor".to_string(),
        chain: Vec::new(),
    };
    writeln!(
        out,
        "device {}: the probe runs as its program, in its device sandbox ({})",
        def.name,
        Launcher::host().name()
    )?;
    let mut inst = Instance::start(env, &probe, &ctx, Launcher::host())?;
    let p = Program::connect(&inst.socket)?;
    let dctx = DeviceCtx {
        env: env.clone(),
        sandbox: caller.clone(),
        harness: ctx.harness.clone(),
        project: ctx.project.clone(),
        started_by: Vec::new(),
        approver: Arc::new(crate::broker::DenyAll),
        instances: super::sandbox::Instances::new(env.clone()),
    };
    let p = p.with_caller(Caller::for_device(&probe, &dctx));
    let call = Call::with_cancel(
        &caller,
        Duration::from_secs(probe.timeout() + 5),
        Arc::new(AtomicBool::new(false)),
    );
    let mut sink = Collect::default();
    let outcome = p.call(&call, &probe, b"probe", &mut sink);
    let log = inst.log.clone();
    inst.stop();
    drop(listener);
    let body = String::from_utf8_lossy(&sink.0).into_owned();
    match outcome {
        Ok(o) if o.status == "exit 0" => {}
        Ok(o) => bail!("the probe ended with {}: see {}", o.status, log.display()),
        Err(f) => bail!(
            "the probe failed: {} ({:?}); see {}",
            f.status,
            f.error,
            log.display()
        ),
    }
    // The envelope of an exec device: header lines, an empty line, the body.
    let lines = body.split_once("\n\n").map(|(_, b)| b).unwrap_or(&body);
    let mut failed = 0;
    let mut done = false;
    for l in lines.lines() {
        if l == "probe-done" {
            done = true;
            continue;
        }
        if l.starts_with("FAIL ") {
            failed += 1;
        }
        writeln!(out, "{l}")?;
    }
    if !done {
        bail!("the probe did not run to its end; see {}", log.display());
    }
    writeln!(
        out,
        "{}",
        if failed == 0 {
            "device sandbox escape probe: every check passed".to_string()
        } else {
            format!("device sandbox escape probe: {failed} check(s) failed")
        }
    )?;
    Ok(failed)
}

/// The table of the probe: the named device with the probe as its
/// program, the probe folder read-only, no stream, and the facts of the
/// host in `PADDOCK_PROBE_*` variables.
fn probe_def(
    env: &HostEnv,
    def: &DeviceDef,
    probe_dir: &std::path::Path,
    script: &std::path::Path,
    port: u16,
) -> DeviceDef {
    let mut d = def.clone();
    d.command = vec![script.display().to_string()];
    d.stream = false;
    d.limits.timeout = d.limits.timeout.clamp(60, 120);
    d.limits.max_response = d.limits.max_response.max(1 << 20);
    let mut p = d.sandbox.take().unwrap_or_default();
    p.ro.push(probe_dir.to_path_buf());
    let set = |p: &mut super::config::SandboxProfile, k: &str, v: String| {
        p.env.insert(k.to_string(), v);
    };
    set(&mut p, "PADDOCK_PROBE_PORT", port.to_string());
    if let Some(gw) = crate::doctor::default_gateway() {
        set(&mut p, "PADDOCK_PROBE_LAN", gw.to_string());
    }
    if let Some(rt) = env.var("XDG_RUNTIME_DIR") {
        set(&mut p, "PADDOCK_PROBE_RUNTIME", rt.to_string());
    }
    if let Some(f) = host_file(env) {
        set(&mut p, "PADDOCK_PROBE_HOSTFILE", f.display().to_string());
    }
    // `home = "read-only"` or "full": the home is the host's, and the
    // probe expects its entries.
    if p.home != crate::execbox::Home::Hidden {
        let n = std::fs::read_dir(&env.home).map(|r| r.count()).unwrap_or(0);
        set(&mut p, "PADDOCK_PROBE_HOME_ENTRIES", n.to_string());
        p.env.remove("PADDOCK_PROBE_HOSTFILE");
    }
    // The private keys of the agent credentials.
    let creds = crate::credentials::load(env).unwrap_or_default();
    let mut keys = Vec::new();
    let mut agents = 0;
    for c in &p.credentials {
        if let Some(k) = creds.get(c) {
            if k.via == crate::credentials::Via::Agent {
                agents += 1;
                for pk in &k.keys {
                    let pk = crate::credentials::sshagent::key_path(env, pk);
                    let private = pk.with_extension("");
                    keys.push(private.display().to_string());
                }
            }
        }
    }
    if !keys.is_empty() {
        set(&mut p, "PADDOCK_PROBE_KEYFILES", keys.join(":"));
    }
    if agents > 0 {
        set(&mut p, "PADDOCK_PROBE_AGENT", "1".into());
    }
    // ~/.ssh inside holds the ro files of the table under it, nothing else.
    let ssh = env.home.join(".ssh");
    if p.home == crate::execbox::Home::Hidden {
        let mut expect: Vec<String> =
            p.ro.iter()
                .filter_map(|r| r.strip_prefix(&ssh).ok())
                .filter_map(|r| r.components().next())
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
        expect.sort();
        expect.dedup();
        set(&mut p, "PADDOCK_PROBE_SSH_DIR", ssh.display().to_string());
        set(&mut p, "PADDOCK_PROBE_SSH_EXPECT", expect.join(" "));
    }
    let ports: Vec<String> = p.connect.iter().map(|c| c.local_port.to_string()).collect();
    if !ports.is_empty() {
        set(&mut p, "PADDOCK_PROBE_CONNECT", ports.join(":"));
    }
    d.sandbox = Some(p);
    d
}

/// A file of the host's home that the probe must not see: the first
/// regular file in it.
fn host_file(env: &HostEnv) -> Option<PathBuf> {
    std::fs::read_dir(&env.home)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::config::ParseEnv;

    fn env(dir: &std::path::Path) -> HostEnv {
        let vars = [("XDG_RUNTIME_DIR", dir.join("rt"))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.display().to_string()))
            .collect();
        HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars)
    }

    #[test]
    fn the_probe_table_names_the_facts_of_the_host() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        std::fs::create_dir_all(env.home.join(".ssh")).unwrap();
        std::fs::write(env.home.join("note.txt"), "x").unwrap();
        let text = crate::devices::examples::MacExample::new("mac.local", "me", &env.home)
            .tables()
            .replace("2222:mac.local:22", "2222:127.0.0.1:1")
            .replace(
                "~/.ssh/id_mac.pub",
                &format!("{}/.ssh/id_mac.pub", env.home.display()),
            );
        let cfg = DevicesCfg::parse_with(
            &text,
            ParseEnv {
                home: Some(&env.home),
                allow_local_connect: true,
            },
        )
        .unwrap();
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(env.paddock_config().join("config.toml"), &text).unwrap();
        let d = probe_def(
            &env,
            &cfg.user[0],
            t.path(),
            &t.path().join("probe.sh"),
            4242,
        );
        assert_eq!(d.command, [t.path().join("probe.sh").display().to_string()]);
        assert!(!d.stream);
        let p = d.sandbox.unwrap();
        assert!(p.ro.contains(&t.path().to_path_buf()));
        assert_eq!(p.env["PADDOCK_PROBE_PORT"], "4242");
        assert_eq!(p.env["PADDOCK_PROBE_CONNECT"], "2222");
        assert_eq!(p.env["PADDOCK_PROBE_AGENT"], "1");
        assert_eq!(
            p.env["PADDOCK_PROBE_KEYFILES"],
            env.home.join(".ssh/id_mac").display().to_string()
        );
        assert_eq!(p.env["PADDOCK_PROBE_SSH_EXPECT"], "known_hosts_mac");
        assert_eq!(
            p.env["PADDOCK_PROBE_HOSTFILE"],
            env.home.join("note.txt").display().to_string()
        );
        assert_eq!(
            p.env["PADDOCK_PROBE_RUNTIME"],
            t.path().join("rt").display().to_string()
        );
        // The profile of the table is kept: the credential, the caller, the connect.
        assert_eq!(p.credentials, ["mac-ssh"]);
        assert_eq!(p.caller.len(), 1);
        assert_eq!(p.connect[0].local_port, 2222);
    }

    #[test]
    fn the_probe_script_is_plain_sh_and_ends_with_the_done_line() {
        assert!(PROBE.starts_with("#!/bin/sh\n"));
        assert!(PROBE.trim_end().ends_with("echo \"probe-done\""));
        for name in [
            "home-empty",
            "host-home-hidden",
            "key-files-absent",
            "ssh-dir-only-ro-files",
            "agent-filters",
            "no-paddock-host",
            "caller-read-only",
            "no-interface-but-lo",
            "host-loopback-unreachable",
            "lan-unreachable",
            "connect-port-",
        ] {
            assert!(PROBE.contains(name), "{name}");
        }
        // sh -n: the script parses.
        let out = std::process::Command::new("sh")
            .arg("-n")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/src/devices/probe.sh"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_device_that_is_not_a_sandbox_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let env = env(t.path());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(
            env.paddock_config().join("config.toml"),
            "[devices.n]\nshape = \"exec\"\nrun = \"host\"\ncommand = [\"true\"]\n",
        )
        .unwrap();
        let mut out = Vec::new();
        let e = format!("{:#}", run(&env, "n", &mut out).unwrap_err());
        assert!(
            e.contains("run = \"host\" has no device sandbox to probe"),
            "{e}"
        );
        let e = format!("{:#}", run(&env, "none", &mut out).unwrap_err());
        assert!(e.starts_with("no [devices.none] table"), "{e}");
    }
}
