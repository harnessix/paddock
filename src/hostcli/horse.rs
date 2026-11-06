//! The sandbox broker client for horse views.
use crate::broker::{Request, Status, PROTOCOL};
use std::io::{Read, Write};

#[path = "channel.rs"]
pub(crate) mod channel;
#[path = "safe_point.rs"]
pub(crate) mod safe_point;

pub fn main(args: &[String]) -> i32 {
    let mut values = std::collections::BTreeMap::new();
    let usage = || {
        eprintln!("usage: paddock-host horse {{sessions|lineage|log SESSION [--last N]|inbox SESSION|memory put NAME|dna|dna put|registry fetch REF [--use] [--index NAME]|registry sync INDEX|safe-point stop|prompt [JSON]}}");
        2
    };
    // A hook of an interactive run: the reply is complete, or a turn starts.
    if args.first().map(String::as_str) == Some("safe-point") {
        return safe_point::main(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("registry") {
        match registry_args(&args[1..]) {
            Some(v) => values = v,
            None => return usage(),
        }
    }
    let op = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["sessions"] => "sessions",
        ["lineage"] => "lineage",
        ["log", session] | ["inbox", session] => {
            values.insert("session".into(), serde_json::json!(session));
            &args[0]
        }
        ["log", session, "--last", n] => {
            let Ok(n) = n.parse::<u64>() else {
                return usage();
            };
            values.insert("session".into(), serde_json::json!(session));
            values.insert("last".into(), serde_json::json!(n));
            "log"
        }
        ["registry", ..] => crate::horse::device::registry::FETCH,
        ["dna"] => "dna",
        ["dna", "put"] => {
            let mut bytes = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
                eprintln!("{e}");
                return 1;
            }
            values.insert("bytes".into(), serde_json::json!(bytes));
            "dna_put"
        }
        ["memory", "put", name] => {
            let mut bytes = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
                eprintln!("{e}");
                return 1;
            }
            values.insert("name".into(), serde_json::json!(name));
            values.insert("bytes".into(), serde_json::json!(bytes));
            "put"
        }
        _ => return usage(),
    };
    if !values.contains_key("op") {
        values.insert("op".into(), serde_json::json!(op));
    }
    let req = Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: values,
        reason: String::new(),
    };
    report(
        super::send(&req),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

/// The arguments of `registry fetch REF [--use] [--index NAME]` and
/// `registry sync INDEX`, with their op. None for anything else.
fn registry_args(args: &[String]) -> Option<std::collections::BTreeMap<String, serde_json::Value>> {
    use crate::horse::device::registry::{FETCH, SYNC};
    use serde_json::json;
    let mut v = std::collections::BTreeMap::new();
    match args.first().map(String::as_str)? {
        "sync" if args.len() == 2 => {
            v.insert("op".into(), json!(SYNC));
            v.insert("index".into(), json!(args[1]));
        }
        "fetch" => {
            v.insert("op".into(), json!(FETCH));
            let mut rest = args[1..].iter();
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--use" => {
                        v.insert("use".into(), json!(true));
                    }
                    "--index" => {
                        v.insert("index".into(), json!(rest.next()?));
                    }
                    r if !r.starts_with('-') && !v.contains_key("ref") => {
                        v.insert("ref".into(), json!(r));
                    }
                    _ => return None,
                }
            }
            v.get("ref")?;
        }
        _ => return None,
    }
    Some(v)
}

fn report(
    response: Result<crate::broker::Response, String>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    match response {
        Ok(resp) if resp.status == Status::Done => {
            let bytes: Vec<u8> =
                serde_json::from_value(resp.result["bytes"].clone()).unwrap_or_default();
            if let Err(e) = out.write_all(&bytes) {
                let _ = writeln!(err, "{e}");
                return 1;
            }
            0
        }
        Ok(resp) => {
            let _ = writeln!(err, "{}", resp.message);
            1
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

pub(super) fn dispatch(cmd: &str, rest: &[String]) -> Option<i32> {
    use super::{compacted, credential, device, forage, git, kv, pasture, spec, status, timer};
    Some(match cmd {
        "git" => git::git_main(rest),
        "git-remote-helper" => git::remote_helper(rest),
        "gh" => git::gh_main(rest),
        "kv" => kv::main(rest),
        "horse" => main(rest),
        "channel" => channel::main(rest),
        "timer" => timer::main(rest),
        "pasture" => pasture::main(rest),
        "forage" => forage::main(rest),
        "status" => status::main(rest),
        "compacted" => compacted::main(rest),
        "spec" => spec::main(rest),
        "device" => device::main(rest),
        "credential" => credential::main(rest),
        "tea" | "glab" => git::forge_main(cmd, rest),
        "forge"
            if rest
                .first()
                .is_some_and(|t| crate::broker::forge::tool(t).is_some()) =>
        {
            git::forge_main(&rest[0], &rest[1..])
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_arguments_become_one_request() {
        let a = |s: &[&str]| registry_args(&s.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        let v = a(&["fetch", "registry://t/x@1.0.0", "--use", "--index", "team"]).unwrap();
        assert_eq!(v["op"], "registry.fetch");
        assert_eq!(v["ref"], "registry://t/x@1.0.0");
        assert_eq!(v["use"], true);
        assert_eq!(v["index"], "team");
        let v = a(&["fetch", "registry://t/x@1.0.0"]).unwrap();
        assert!(!v.contains_key("use"));
        assert_eq!(a(&["sync", "team"]).unwrap()["op"], "registry.sync");
        assert!(a(&["fetch"]).is_none());
        assert!(a(&["fetch", "a", "b"]).is_none());
        assert!(a(&["fetch", "a", "--index"]).is_none());
        assert!(a(&["fetch", "a", "--force"]).is_none());
        assert!(a(&["sync"]).is_none());
        assert!(a(&["list"]).is_none());
    }

    #[test]
    fn sessions_without_a_horse_reports_the_device_message() {
        let req = Request {
            v: PROTOCOL,
            cap: "horse".into(),
            args: [("op".into(), serde_json::json!("sessions"))]
                .into_iter()
                .collect(),
            reason: String::new(),
        };
        let response = crate::horse::device::handle(None, &req);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(report(Ok(response), &mut out, &mut err), 1);
        assert!(out.is_empty());
        assert_eq!(err, b"this sandbox has no horse\n");
    }
}
