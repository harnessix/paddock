//! The lines of `paddock doctor` about `[devices.NAME]` tables: old
//! spellings with the new one, host devices, devices without a grant whose
//! sandbox reaches something, and `project:` grants whose folder does not
//! exist. None of them refuses a configuration.

use super::config::{DeviceDef, Run};
use super::grant::{tilde, Grant};
use super::DevicesCfg;
use crate::env::HostEnv;

/// One line per finding, in the format of `paddock doctor`, with the
/// host devices grouped under their heading.
pub fn lines(env: &HostEnv, cfg: &DevicesCfg) -> Vec<String> {
    let exec = crate::execbox::ExecCfg::load(env).unwrap_or_default();
    let mut v = Vec::new();
    for d in &cfg.user {
        v.extend(lines_for(d, &exec, &env.home, cfg!(target_os = "macos")));
    }
    v.extend(host_lines(cfg));
    v.extend(remote_lines(cfg));
    v.extend(shared_lines(env, cfg));
    v
}

/// The heading "devices of another paddock" and one line per
/// `run = "remote"` device: the transport and the device there, with the
/// grant that the other side needs. Nothing without one.
pub fn remote_lines(cfg: &DevicesCfg) -> Vec<String> {
    let remotes: Vec<&DeviceDef> = cfg
        .user
        .iter()
        .filter(|d| matches!(d.run, Run::Remote { .. }))
        .collect();
    if remotes.is_empty() {
        return Vec::new();
    }
    let mut v = vec![
        "  remote   devices of another paddock (the table there needs grant = [\"remote:HOST\"] for this machine):"
            .to_string(),
    ];
    for d in remotes {
        if let Some(line) = super::forward::doctor_line(d) {
            v.push(format!(
                "  remote     {line}{}",
                if d.enabled { "" } else { " (off)" }
            ));
        }
    }
    v
}

/// One line per `scope = "shared"` device: who starts its one instance per
/// user, whether one runs now, and how to hand it to the user manager
/// (devices/shared.rs).
pub fn shared_lines(env: &HostEnv, cfg: &DevicesCfg) -> Vec<String> {
    use super::config::Scope;
    let mut v = Vec::new();
    for d in cfg
        .user
        .iter()
        .filter(|d| d.scope == Scope::Shared && d.enabled)
    {
        let now = match super::shared::users_of(env, &d.name) {
            Some(1) => "one instance runs, 1 user".to_string(),
            Some(n) => format!("one instance runs, {n} users"),
            None => "no instance runs".to_string(),
        };
        let installed = super::shared::install::installed(env, &d.name);
        let mark = if installed { "ok" } else { "note" };
        let fix = if installed {
            String::new()
        } else {
            format!(
                ". `paddock device install {}` hands its socket to the user manager, so the instance outlives every run",
                d.name
            )
        };
        v.push(format!(
            "  {mark:<7}  {}: one instance per user, {} ({now}), idle {} s{fix}",
            d.name,
            super::shared::start_kind(env, &d.name).name(),
            d.idle
        ));
    }
    v
}

/// The heading "devices that run on the host as you" and one line per
/// host device with its command and folder. Nothing without one.
pub fn host_lines(cfg: &DevicesCfg) -> Vec<String> {
    let hosts: Vec<&DeviceDef> = cfg
        .user
        .iter()
        .filter(|d| matches!(d.run, Run::Host { .. }))
        .collect();
    if hosts.is_empty() {
        return Vec::new();
    }
    let mut v = vec![
        "  host     devices that run on the host as you, with your home, your keys and your network:"
            .to_string(),
    ];
    for d in hosts {
        v.push(format!(
            "  host       {}: `{}` in {}{}",
            d.name,
            d.command.join(" "),
            super::host::cwd_text(d),
            if d.enabled { "" } else { " (off)" }
        ));
    }
    v
}

/// The lines of one table. `macos` adds the limits that a Seatbelt device
/// sandbox cannot give.
fn lines_for(
    d: &DeviceDef,
    exec: &crate::execbox::ExecCfg,
    home: &std::path::Path,
    macos: bool,
) -> Vec<String> {
    let at = format!("[devices.{}]", d.name);
    let mut v = Vec::new();
    if macos && d.run == Run::Sandbox {
        let p = d.profile();
        if p.mem.is_some() || p.cpu_percent.is_some() {
            v.push(format!(
                "  note     {}",
                crate::backend::seatbelt::device::limits_notice(&d.name)
            ));
        }
    }
    for k in &d.old_keys {
        let new = match (k.as_str(), d.shape) {
            ("kind", super::config::Shape::HttpProxy) => continue,
            ("kind", _) => "shape = \"exec\" and run = \"sandbox\"".to_string(),
            ("max_body", _) => format!(
                "max_request = \"{}\" and max_response = \"{}\"",
                super::config::render_size(d.limits.max_request),
                super::config::render_size(d.limits.max_response)
            ),
            _ => continue,
        };
        let old = match k.as_str() {
            "kind" => "kind = \"command\"".to_string(),
            _ => k.clone(),
        };
        v.push(format!(
            "  note     {at} uses {old} (the spelling of 0.2.1, still read); the new spelling is {new}"
        ));
    }
    if d.grant.is_none() && d.run == Run::Sandbox {
        let p = d.profile().with_exec_base(exec);
        if p.reaches_something() {
            let reach: Vec<String> = p
                .reach_lines()
                .into_iter()
                .filter(|l| !l.starts_with("network") && !l.starts_with("caller"))
                .collect();
            v.push(format!(
                "  note     {at} is visible to every sandbox; add a grant (its sandbox reaches: {})",
                reach.join("; ")
            ));
        }
    }
    for g in d.grant.iter().flatten() {
        if let Grant::Project(p) = g {
            if !p.is_dir() {
                v.push(format!(
                    "  no       {at} grant project:{}: the folder does not exist, so no sandbox matches it",
                    tilde(p, home)
                ));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(text: &str) -> Vec<String> {
        let cfg = DevicesCfg::parse(text).unwrap();
        let exec = crate::execbox::ExecCfg::default();
        let mut v: Vec<String> = cfg
            .user
            .iter()
            .flat_map(|d| lines_for(d, &exec, std::path::Path::new("/home/demo"), false))
            .collect();
        v.extend(host_lines(&cfg));
        v
    }

    /// A shared device says who starts its one instance, whether one runs
    /// now, and how to hand it to the user manager.
    #[test]
    fn a_shared_device_gets_a_line_that_names_the_start_and_the_users() {
        let t = tempfile::tempdir().unwrap();
        for d in ["home", "cfg", "state", "run"] {
            std::fs::create_dir_all(t.path().join(d)).unwrap();
        }
        let vars: std::collections::BTreeMap<String, String> = [
            ("XDG_CONFIG_HOME", t.path().join("cfg")),
            ("XDG_STATE_HOME", t.path().join("state")),
            ("XDG_RUNTIME_DIR", t.path().join("run")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.display().to_string()))
        .collect();
        let env = HostEnv::from_parts(t.path().join("home"), "demo".into(), 1000, 1000, vars);
        let shared = concat!(
            "[devices.model]\n",
            "shape = \"serve\"\nenabled = true\nscope = \"shared\"\nidle = 600\n",
            "command = [\"./m\"]\n",
            "[devices.build]\nshape = \"exec\"\nenabled = true\ncommand = [\"./b\"]\n"
        );
        let cfg = DevicesCfg::parse(shared).unwrap();
        let v = shared_lines(&env, &cfg);
        assert_eq!(v.len(), 1, "only the shared device gets a line: {v:?}");
        let l = &v[0];
        assert!(l.contains("model: one instance per user"), "{l}");
        assert!(l.contains("no instance runs"), "{l}");
        assert!(l.contains("idle 600 s"), "{l}");
        assert!(l.contains("paddock device install model"), "{l}");
        // A device that is off gets no line.
        let off = DevicesCfg::parse(concat!(
            "[devices.model]\n",
            "shape = \"serve\"\nenabled = false\nscope = \"shared\"\ncommand = [\"./m\"]\n"
        ))
        .unwrap();
        assert!(shared_lines(&env, &off).is_empty());
    }

    #[test]
    fn an_old_spelling_gets_the_new_one() {
        let v =
            lines_of("[devices.jq]\nkind = \"command\"\ncommand = [\"jq\"]\nmax_body = \"256K\"\n");
        assert_eq!(
            v,
            [
                "  note     [devices.jq] uses kind = \"command\" (the spelling of 0.2.1, still read); the new spelling is shape = \"exec\" and run = \"sandbox\"",
                "  note     [devices.jq] uses max_body (the spelling of 0.2.1, still read); the new spelling is max_request = \"256K\" and max_response = \"256K\"",
            ]
        );
        // `kind = "http"` stays the spelling of a proxy: no line.
        let v = lines_of("[devices.o]\nkind = \"http\"\ntarget = \"127.0.0.1:1\"\n");
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn a_host_device_and_an_ungranted_reaching_device_get_a_line() {
        let v = lines_of(
            "[devices.notes]\nshape = \"exec\"\nrun = \"host\"\nenabled = true\ncommand = [\"notes\", \"-q\"]\ncwd = \"~/notes\"\n[devices.gui]\nshape = \"serve\"\nrun = \"host\"\ncommand = [\"gui-tool\"]\n",
        );
        assert_eq!(
            v,
            [
                "  host     devices that run on the host as you, with your home, your keys and your network:",
                "  host       gui: `gui-tool` in your home (off)",
                "  host       notes: `notes -q` in ~/notes",
            ]
        );
        let v = lines_of(
            "[devices.mac]\nshape = \"exec\"\ncommand = [\"x\"]\n[devices.mac.sandbox]\nconnect = [\"2222:mac.local:22\"]\ncredentials = [\"mac-ssh\"]\n",
        );
        assert_eq!(
            v,
            ["  note     [devices.mac] is visible to every sandbox; add a grant (its sandbox reaches: connect: 127.0.0.1:2222 inside reaches mac.local:22 from the host; credential: mac-ssh)"]
        );
        // With a grant, or with nothing reached, no line.
        assert!(lines_of("[devices.mac]\nshape = \"exec\"\ncommand = [\"x\"]\ngrant = [\"all\"]\n[devices.mac.sandbox]\ncredentials = [\"k\"]\n").is_empty());
        assert!(lines_of("[devices.mac]\nshape = \"exec\"\ncommand = [\"x\"]\n[devices.mac.sandbox]\nnetwork = \"egress\"\n").is_empty());
        // A 0.2.1 command with `[exec] paths` reaches those folders.
        let cfg =
            DevicesCfg::parse("[devices.jq]\nkind = \"command\"\ncommand = [\"jq\"]\n").unwrap();
        let exec = crate::execbox::ExecCfg {
            paths: vec!["/home/demo/scratch".into()],
            ..Default::default()
        };
        let v = lines_for(
            &cfg.user[0],
            &exec,
            std::path::Path::new("/home/demo"),
            false,
        );
        assert!(
            v.iter()
                .any(|l| l.contains("read-write: /home/demo/scratch")),
            "{v:?}"
        );
    }

    #[test]
    fn mem_and_cpus_get_a_line_on_macos_only() {
        let cfg = DevicesCfg::parse(
            "[devices.big]\nshape = \"exec\"\ncommand = [\"x\"]\ngrant = [\"all\"]\n[devices.big.sandbox]\nmem = \"1G\"\n",
        )
        .unwrap();
        let exec = crate::execbox::ExecCfg::default();
        let home = std::path::Path::new("/home/demo");
        assert!(lines_for(&cfg.user[0], &exec, home, false).is_empty());
        assert_eq!(
            lines_for(&cfg.user[0], &exec, home, true),
            ["  note     [devices.big.sandbox] mem and cpus are ignored: memory and cpu limits of devices need systemd. Not on macOS"]
        );
    }

    #[test]
    fn a_project_grant_whose_folder_is_missing_gets_a_line() {
        let v = lines_of("[devices.d]\nshape = \"exec\"\ncommand = [\"x\"]\ngrant = [\"project:/home/demo/no-such\"]\n");
        assert_eq!(
            v,
            ["  no       [devices.d] grant project:~/no-such: the folder does not exist, so no sandbox matches it"]
        );
    }
}
