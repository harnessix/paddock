//! Sandbox-side client for a broker-owned peer job.
use crate::broker::{Request, Status, PROTOCOL};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn request(args: serde_json::Value) -> Request {
    Request {
        v: PROTOCOL,
        cap: "exclave".into(),
        args: serde_json::from_value(args).unwrap_or_default(),
        reason: String::new(),
    }
}
fn call(args: serde_json::Value) -> Result<serde_json::Value, String> {
    let response = super::send(&request(args))?;
    if response.status != Status::Done {
        return Err(response.message);
    }
    Ok(response.result)
}

pub fn main(args: &[String]) -> i32 {
    let Some(("run", rest)) = args.split_first().map(|(s, rest)| (s.as_str(), rest)) else {
        eprintln!("usage: paddock-host exclave run [--on NAME] [--root DIR] [--network MODE] [--include PATH] [--output PATH] -- COMMAND...");
        return 2;
    };
    let mut peer = None;
    let mut root: Option<String> = None;
    let mut network = "registries".to_string();
    let mut gpu = false;
    let mut outputs = Vec::new();
    let mut includes = Vec::new();
    let mut pos = 0;
    while pos < rest.len() && rest[pos] != "--" {
        if rest[pos] == "--gpu" {
            gpu = true;
            pos += 1;
            continue;
        }
        if pos + 1 >= rest.len() {
            eprintln!("exclave: option requires a value");
            return 2;
        }
        match rest[pos].as_str() {
            "--on" => peer = Some(rest[pos + 1].clone()),
            "--root" => root = Some(rest[pos + 1].clone()),
            "--network" => network = rest[pos + 1].clone(),
            "--output" => outputs.push(rest[pos + 1].clone()),
            "--include" => includes.push(rest[pos + 1].clone()),
            _ => {
                eprintln!("exclave: unknown option");
                return 2;
            }
        }
        pos += 2;
    }
    if pos >= rest.len() || pos + 1 >= rest.len() {
        eprintln!("exclave: expected -- COMMAND");
        return 2;
    }
    let argv = &rest[pos + 1..];
    let mut random = [0u8; 32];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .is_err()
    {
        eprintln!("exclave: cannot create operation id");
        return 7;
    }
    let id = crate::manifest::hex(&random);
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("exclave: cannot determine working directory");
            return 7;
        }
    };
    let root = match root {
        Some(p) => match std::path::Path::new(&p).canonicalize() {
            Ok(p) => Some(p),
            Err(_) => {
                eprintln!("exclave: cannot resolve root");
                return 2;
            }
        },
        None => None,
    };
    let request = serde_json::json!({"root":root, "op":"start", "id":id, "peer":peer, "network":network, "gpu":gpu, "outputs":outputs, "includes":includes, "argv":argv, "cwd":cwd});
    if let Err(e) = call(request) {
        eprintln!("exclave: start result unknown: {e}");
        return 7;
    }
    let mut after = 0u64;
    loop {
        let response = match call(serde_json::json!({"op":"poll", "id":id, "after":after})) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("exclave: result unknown: {e}");
                return 7;
            }
        };
        let Some(seq) = response.get("seq").and_then(|v| v.as_u64()) else {
            continue;
        };
        match response.get("event").and_then(|v| v.as_str()) {
            Some("output") => {
                let Some(data) = response
                    .get("data")
                    .and_then(|v| v.as_str())
                    .and_then(|s| STANDARD.decode(s).ok())
                else {
                    eprintln!("exclave: invalid output");
                    return 7;
                };
                let result = match response.get("stream").and_then(|v| v.as_str()) {
                    Some("stdout") => {
                        let mut out = std::io::stdout().lock();
                        out.write_all(&data).and_then(|_| out.flush())
                    }
                    Some("stderr") => {
                        let mut out = std::io::stderr().lock();
                        out.write_all(&data).and_then(|_| out.flush())
                    }
                    _ => {
                        eprintln!("exclave: invalid output stream");
                        return 7;
                    }
                };
                if result.is_err() {
                    return 7;
                }
            }
            Some("exit") => {
                let _ = call(serde_json::json!({"op":"finish", "id":id, "seq":seq}));
                return response
                    .get("code")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(7)
                    .clamp(0, 255) as i32;
            }
            Some("not_started") => {
                let _ = call(serde_json::json!({"op":"finish", "id":id, "seq":seq}));
                let reason = response
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("peer unavailable");
                if gpu {
                    eprintln!("exclave: {reason}; GPU request cannot run locally");
                    return 1;
                }
                eprintln!("exclave: {reason}; running locally");
                let status = Command::new(&argv[0])
                    .args(&argv[1..])
                    .env("PADDOCK_EXCLAVE_LOCAL", "1")
                    .stdin(Stdio::null())
                    .status();
                return match status {
                    Ok(s) => {
                        use std::os::unix::process::ExitStatusExt;
                        s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))
                    }
                    Err(_) => 127,
                };
            }
            Some("unknown") => {
                let _ = call(serde_json::json!({"op":"finish", "id":id, "seq":seq}));
                eprintln!("exclave: remote result unknown");
                return 7;
            }
            _ => {
                eprintln!("exclave: invalid peer response");
                return 7;
            }
        }
        after = seq;
    }
}
