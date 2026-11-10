//! Broker operations for the originating sandbox's peer jobs.
use super::{Broker, Request, Response, Status, PROTOCOL};
use crate::exclave::jobs::{Event, EventWriter, Sink};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::Duration;

impl Broker {
    pub(super) fn handle_exclave(&self, req: &Request) -> Response {
        if req.v != PROTOCOL {
            return Response::new(Status::Invalid, "unsupported protocol version");
        }
        let text = |key: &str| req.args.get(key).and_then(|v| v.as_str()).unwrap_or("");
        let result = match text("op") {
            "start" => self.exclave_start(req),
            "finish" => self
                .exclave_jobs
                .finish(
                    text("id"),
                    req.args.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
                )
                .map(|_| serde_json::json!({})),
            "poll" => self.exclave_poll(
                text("id"),
                req.args.get("after").and_then(|v| v.as_u64()).unwrap_or(0),
            ),
            _ => Err(anyhow::anyhow!("invalid exclave operation")),
        };
        match result {
            Ok(value) => Response::done(value),
            Err(_) => Response::new(Status::Invalid, "exclave request could not be accepted"),
        }
    }

    fn exclave_start(&self, req: &Request) -> anyhow::Result<serde_json::Value> {
        let id = req
            .args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing operation"))?;
        let argv: Vec<String> =
            serde_json::from_value(req.args.get("argv").cloned().unwrap_or_default())?;
        if argv.is_empty() || argv[0].is_empty() {
            anyhow::bail!("missing command");
        }
        let outputs: Vec<String> = serde_json::from_value(
            req.args
                .get("outputs")
                .cloned()
                .unwrap_or(serde_json::json!([])),
        )?;
        let includes: Vec<String> = serde_json::from_value(
            req.args
                .get("includes")
                .cloned()
                .unwrap_or(serde_json::json!([])),
        )?;
        let network = req
            .args
            .get("network")
            .and_then(|v| v.as_str())
            .unwrap_or("registries");
        if !matches!(network, "none" | "registries" | "egress") {
            anyhow::bail!("invalid network");
        }
        let gpu = req
            .args
            .get("gpu")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let peer = req
            .args
            .get("peer")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let cwd = req
            .args
            .get("cwd")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing working directory"))?;
        let sandbox_project = self.ctx.project.canonicalize()?;
        let project = match req.args.get("root").and_then(|v| v.as_str()) {
            Some(root) => {
                let root = std::path::Path::new(root).canonicalize()?;
                if !root.is_dir() || !root.starts_with(&sandbox_project) {
                    anyhow::bail!("exclave root is outside the project");
                }
                root
            }
            None => sandbox_project,
        };
        let cwd = std::path::Path::new(cwd).canonicalize()?;
        let relative = cwd
            .strip_prefix(&project)
            .map_err(|_| anyhow::anyhow!("working directory is outside the project"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid working directory"))?
            .to_string();
        let argv = crate::exclave::command_in(&relative, argv)?;
        let sink = self.exclave_jobs.start(id)?;
        let env = self.env.clone();
        let run_id = self.ctx.id.clone();
        let network = network.to_owned();
        std::thread::spawn(move || {
            let terminal = sink.clone();
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker(
                    env, run_id, project, peer, network, gpu, argv, outputs, includes, sink,
                )
            }))
            .is_err()
            {
                let _ = terminal.send(Event::Unknown("peer job failed".into()));
            }
        });
        Ok(serde_json::json!({"started":true}))
    }

    fn exclave_poll(&self, id: &str, after: u64) -> anyhow::Result<serde_json::Value> {
        let Some((seq, event)) = self.exclave_jobs.poll(id, after, Duration::from_secs(2))? else {
            return Ok(serde_json::json!({}));
        };
        Ok(match event {
            Event::Output { stream, bytes } => {
                serde_json::json!({"seq":seq,"event":"output","stream":stream,"data":STANDARD.encode(bytes)})
            }
            Event::Exit(code) => serde_json::json!({"seq":seq,"event":"exit","code":code}),
            Event::NotStarted(reason) => {
                serde_json::json!({"seq":seq,"event":"not_started","reason":reason})
            }
            Event::Unknown(reason) => {
                serde_json::json!({"seq":seq,"event":"unknown","reason":reason})
            }
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    env: crate::env::HostEnv,
    run_id: String,
    project: std::path::PathBuf,
    peer: Option<String>,
    network: String,
    gpu: bool,
    argv: Vec<String>,
    outputs: Vec<String>,
    includes: Vec<String>,
    sink: Sink,
) {
    let report = |event: &str, detail: String| {
        let _ = crate::mail::append(
            &env,
            &run_id,
            crate::mail::Dir::Up,
            crate::mail::Message::event(event, detail),
        );
    };
    let configured = match crate::exclave::Peers::load(&env) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink.send(Event::Unknown("invalid peer configuration".into()));
            return;
        }
    };
    let name = match configured.select(peer.as_deref()) {
        Ok(name) => name.to_owned(),
        Err(e) => {
            let _ = sink.send(Event::Output {
                stream: "stderr".into(),
                bytes: format!("exclave: {e}\n").into_bytes(),
            });
            let _ = sink.send(Event::Exit(1));
            return;
        }
    };
    let loaded = match crate::manifest::Manifest::load(&env, &project) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink.send(Event::Unknown("cannot read snapshot policy".into()));
            return;
        }
    };
    let mut hidden = Vec::new();
    for path in &loaded.merged.secrets.hide {
        let expanded = env.expand(path, &project);
        if let Ok(relative) = expanded.strip_prefix(&project) {
            hidden.push(relative.to_path_buf());
        }
    }
    let snapshot = match crate::exclave::snapshot::capture(&project, &includes, &hidden) {
        Ok(v) => v,
        Err(e) => {
            let message = e.to_string();
            if message.starts_with("symlink ")
                && message.ends_with(" escapes the project or is invalid")
            {
                let _ = sink.send(Event::Output {
                    stream: "stderr".into(),
                    bytes: format!("{message}\n").into_bytes(),
                });
                let _ = sink.send(Event::Exit(1));
            } else {
                let _ = sink.send(Event::Unknown("cannot create a safe snapshot".into()));
            }
            return;
        }
    };
    let capability = match crate::exclave::identity::project(&env, &name, &project) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink.send(Event::Unknown("cannot create project identity".into()));
            return;
        }
    };
    let mut session = match crate::exclave::client::Session::connect(&configured, &name, capability)
    {
        Ok(v) => v,
        Err(e) if e.is::<crate::exclave::client::Unreachable>() => {
            let reason = format!("peer {name} is unreachable: {e}");
            if !gpu {
                report(
                    "exclave-fallback",
                    format!("exclave: {reason}; running locally"),
                );
            }
            let _ = sink.send(Event::NotStarted(reason));
            return;
        }
        Err(e) => {
            let _ = sink.send(Event::Output {
                stream: "stderr".into(),
                bytes: format!("exclave: {e}\n").into_bytes(),
            });
            let _ = sink.send(Event::Exit(1));
            return;
        }
    };
    let hash = match session.upload(&snapshot) {
        Ok(v) => v,
        Err(e) => {
            let _ = sink.send(Event::Output {
                stream: "stderr".into(),
                bytes: format!("exclave: peer snapshot transfer failed: {e}\n").into_bytes(),
            });
            let _ = sink.send(Event::Exit(1));
            return;
        }
    };
    match session.reserve() {
        Ok(true) => {}
        Ok(false) => {
            let _ = sink.send(Event::Output {
                stream: "stderr".into(),
                bytes: b"exclave: peer is busy\n".to_vec(),
            });
            let _ = sink.send(Event::Exit(1));
            return;
        }
        Err(e) => {
            let _ = sink.send(Event::Output {
                stream: "stderr".into(),
                bytes: format!("exclave: peer reservation failed: {e}\n").into_bytes(),
            });
            let _ = sink.send(Event::Exit(1));
            return;
        }
    }
    report(
        "exclave-dispatch",
        format!("exclave: dispatching to peer {name}, snapshot {hash}"),
    );
    let mut stdout = EventWriter {
        sink: sink.clone(),
        stream: "stdout",
    };
    let mut stderr = EventWriter {
        sink: sink.clone(),
        stream: "stderr",
    };
    match session.run_with_outputs(
        hash.clone(),
        network,
        gpu,
        argv,
        outputs,
        &project,
        &hidden,
        &mut stdout,
        &mut stderr,
    ) {
        Ok(code) => {
            report(
                "exclave-ended",
                format!("exclave: peer {name}, snapshot {hash}, exit {code}"),
            );
            let _ = sink.send(Event::Exit(code));
        }
        Err(e) if e.is::<crate::exclave::client::PeerError>() => {
            let _ = sink.send(Event::Output {
                stream: "stderr".into(),
                bytes: format!("exclave: {e}\n").into_bytes(),
            });
            let _ = sink.send(Event::Exit(1));
        }
        Err(_) => {
            report(
                "exclave-unknown",
                format!("exclave: peer {name}, snapshot {hash}, result unknown"),
            );
            let _ = sink.send(Event::Unknown("remote result unknown".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    fn unreachable_peer_falls_back_and_reports_without_transport_arguments() {
        check_peer_failure(false);
    }

    #[test]
    fn peer_rejection_is_reported_without_fallback() {
        check_peer_failure(true);
    }

    fn check_peer_failure(reject: bool) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        assert!(std::process::Command::new("git")
            .current_dir(&project)
            .args(["init", "-q"])
            .env("HOME", temp.path())
            .status()
            .unwrap()
            .success());
        std::fs::write(project.join("file"), "content").unwrap();
        assert!(std::process::Command::new("git")
            .current_dir(&project)
            .args(["add", "file"])
            .env("HOME", temp.path())
            .status()
            .unwrap()
            .success());
        let env = crate::env::HostEnv::from_parts(
            temp.path().join("home"),
            "test".into(),
            1,
            1,
            BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]),
        );
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        let canary = "PRIVATE_TRANSPORT_CANARY";
        let transport = if reject {
            let script = temp.path().join("peer.sh");
            std::fs::write(&script, "read hello\nprintf '%s\\n' '{\"status\":\"done\",\"result\":{\"type\":\"error\",\"message\":\"No space left on device\"}}'\n").unwrap();
            format!("['/bin/sh', '{}']", script.display())
        } else {
            format!("['/no-such-exclave-command', '{canary}']")
        };
        std::fs::write(
            env.paddock_config().join("config.toml"),
            format!("[exclave.peers.fake]\ntransport = {transport}\n"),
        )
        .unwrap();
        let id = "test-run";
        crate::mail::create(&env, id, &crate::mail::Starter::host()).unwrap();
        let ctx = super::super::Context {
            id: id.into(),
            harness: "test".into(),
            project: project.clone(),
            repo: None,
            chain: vec![],
            depth: 0,
            role: None,
        };
        let broker = Broker::new(
            env.clone(),
            ctx,
            super::super::HostPolicy::default(),
            Arc::new(super::super::DenyAll),
        );
        let op = "a".repeat(64);
        let req = |args: serde_json::Value| Request {
            v: PROTOCOL,
            cap: "exclave".into(),
            args: serde_json::from_value(args).unwrap(),
            reason: String::new(),
        };
        let response = broker.handle(&req(serde_json::json!({"op":"start", "id":op, "argv":["true"], "peer":"fake", "cwd":project})));
        assert_eq!(response.status, Status::Done);
        let result = broker.handle(&req(serde_json::json!({"op":"poll", "id":op, "after":0})));
        if reject {
            assert_eq!(result.result["event"], "output");
            let bytes = STANDARD
                .decode(result.result["data"].as_str().unwrap())
                .unwrap();
            assert!(String::from_utf8(bytes)
                .unwrap()
                .contains("No space left on device"));
            let next = broker.handle(&req(
                serde_json::json!({"op":"poll", "id":op, "after":result.result["seq"]}),
            ));
            assert_eq!(next.result["event"], "exit");
        } else {
            assert_eq!(result.result["event"], "not_started");
            assert!(result.result["reason"]
                .as_str()
                .unwrap()
                .contains("could not start exclave transport"));
        }
        let mail = crate::mail::read(&env, id, crate::mail::Dir::Up);
        assert_eq!(mail.iter().any(|m| m.text.contains("unreachable")), !reject);
        assert!(!format!("{result:?} {mail:?}").contains(canary));
    }
}
