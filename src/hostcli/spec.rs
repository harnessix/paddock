//! `paddock-host spec`: the tasks of a Spec Kit feature (speckit.rs).

use crate::broker::{Request, Status, PROTOCOL};
use std::collections::BTreeMap;

pub const HELP: &str = r#"paddock-host spec: the tasks of the Spec Kit feature of this project.

  paddock-host spec status [--feature NAME] [--json]
  paddock-host spec review ID pass|changes [TEXT]
  paddock-host spec note

A project with a .specify/ folder is a Spec Kit project. paddock keeps the
state of each task of tasks.md in paddock kv and writes the checkboxes of
tasks.md itself. Do not edit tasks.md to mark a task: paddock marks it.

status    the active feature (from .specify/feature.json), each task with its
          state (todo, claimed, running, review, done, failed), owner and
          branch, the tasks that can start now, and the tasks that wait for
          a review. Ready [P] tasks in one group can run at the same time.
review    the verdict on a task in review, after you read and merged its
          branch: pass marks it done, changes sends it back to todo, and
          TEXT reaches the next worker. Only the sandbox that started the
          worker reviews.
note      the Spec Kit lines of the session note: the feature, the task of
          this run and its acceptance text, and the spec paths.

Start a worker for a task with
  paddock-host spawn HARNESS --task T012      one task
  paddock-host spawn HARNESS --task next      the next ready task
Run it again for each ready [P] task: they run side by side, up to the
limits of [spawn] in the user configuration. paddock claims each task with
a lease, so two workers never take one task, and a task whose worker died
is free again.

The worker reports with `paddock-host done "SUMMARY"`, or with
`paddock-host done --status failed "WHY"`. When it ends well, the task
waits for your review.

Each task is a record in the tree scope of paddock kv. Watch them with
  paddock-host kv watch --scope tree --prefix spec/FEATURE/task/
"#;

fn send(args: BTreeMap<String, serde_json::Value>) -> Result<crate::broker::Response, String> {
    super::send(&Request {
        v: PROTOCOL,
        cap: "spec".into(),
        args,
        reason: String::new(),
    })
}

/// The Spec Kit lines of the session note. Empty outside a Spec Kit
/// project, and when paddock cannot answer.
pub fn note_lines() -> Vec<String> {
    let mut a = BTreeMap::new();
    a.insert("action".into(), serde_json::json!("note"));
    match send(a) {
        Ok(r) if r.status == Status::Done => r.result["lines"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn main(args: &[String]) -> i32 {
    if args.is_empty()
        || args
            .iter()
            .any(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        print!("{HELP}");
        return if args.is_empty() {
            super::EXIT_USAGE
        } else {
            0
        };
    }
    let p = match super::parse(&args[1..], &["feature"]) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("paddock-host spec: {e}");
            return super::EXIT_USAGE;
        }
    };
    let mut a = BTreeMap::new();
    let action = args[0].as_str();
    a.insert("action".into(), serde_json::json!(action));
    if let Some(f) = p.opts.get("feature") {
        a.insert("feature".into(), serde_json::json!(f));
    }
    match (action, p.rest.as_slice()) {
        ("status" | "note", []) => {}
        ("review", [id, verdict, text @ ..]) if text.len() <= 1 => {
            if !matches!(verdict.as_str(), "pass" | "changes") {
                eprintln!("paddock-host spec review: the verdict is pass or changes");
                return super::EXIT_USAGE;
            }
            a.insert("id".into(), serde_json::json!(id));
            a.insert("verdict".into(), serde_json::json!(verdict));
            if let Some(t) = text.first() {
                a.insert("text".into(), serde_json::json!(t));
            }
        }
        _ => {
            eprintln!("paddock-host spec: use status, review ID pass|changes [TEXT], or note\nRun `paddock-host spec --help`.");
            return super::EXIT_USAGE;
        }
    }
    let r = match send(a) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("paddock-host: {e}\nThis command works only inside a paddock sandbox.");
            return super::EXIT_NO_HOST;
        }
    };
    if p.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    } else if r.status != Status::Done {
        eprintln!("paddock-host spec: {}", r.message);
    } else {
        match action {
            "status" => println!("{}", r.result["text"].as_str().unwrap_or("")),
            "note" => {
                for l in r.result["lines"].as_array().into_iter().flatten() {
                    println!("{}", l.as_str().unwrap_or(""));
                }
            }
            _ => println!(
                "{}: {}",
                r.result["id"].as_str().unwrap_or(""),
                r.result["state"].as_str().unwrap_or("")
            ),
        }
    }
    r.status.exit_code()
}

pub const GAUGE_HELP: &str = r#"paddock-host gauge: how far the Spec Kit feature of this project is.

  paddock-host gauge [FEATURE] [--seed N] [--window DAYS] [--strict] [--json]

paddock measures the feature on the host, from the history of tasks.md, the
task records, the gate results and the workers' mailboxes. No agent reports
a percentage. It prints:

- done with evidence: a task counts only when its work is merged, its review
  passed or a gate passed, and no gate only skipped or failed after it;
- tasks discovered per day, reopened tasks, fails after done;
- the age of each open task, and the time tasks sat almost done (in review,
  or a worker blocked);
- the forecast: P50 and P85 of the finish date and of the final task count,
  or "not converging" when new tasks come as fast as tasks finish;
- the trend of P85, the appetite share used, and a burn-up chart.

Use it before you start the next worker: when it says "not converging",
cut scope or shape the tasks again instead of adding workers. --strict
counts only tasks with evidence as finished.
"#;

/// `paddock-host gauge`: the gauge of the feature, computed on the host.
pub fn gauge_main(args: &[String]) -> i32 {
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        print!("{GAUGE_HELP}");
        return 0;
    }
    let strict = args.iter().any(|a| a == "--strict");
    let rest: Vec<String> = args.iter().filter(|a| *a != "--strict").cloned().collect();
    let p = match super::parse(&rest, &["feature", "seed", "window"]) {
        Ok(p) if p.rest.len() <= 1 => p,
        Ok(_) => {
            eprintln!("paddock-host gauge: at most one FEATURE\nRun `paddock-host gauge --help`.");
            return super::EXIT_USAGE;
        }
        Err(e) => {
            eprintln!("paddock-host gauge: {e}");
            return super::EXIT_USAGE;
        }
    };
    let mut a = BTreeMap::new();
    a.insert("action".into(), serde_json::json!("gauge"));
    if let Some(f) = p.rest.first().or(p.opts.get("feature")) {
        a.insert("feature".into(), serde_json::json!(f));
    }
    for k in ["seed", "window"] {
        if let Some(v) = p.opts.get(k) {
            match v.parse::<u64>() {
                Ok(n) => {
                    a.insert(k.into(), serde_json::json!(n));
                }
                Err(_) => {
                    eprintln!("paddock-host gauge: --{k} is a number");
                    return super::EXIT_USAGE;
                }
            }
        }
    }
    if strict {
        a.insert("strict".into(), serde_json::json!(true));
    }
    let r = match send(a) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("paddock-host: {e}\nThis command works only inside a paddock sandbox.");
            return super::EXIT_NO_HOST;
        }
    };
    if p.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    } else if r.status != Status::Done {
        eprintln!("paddock-host gauge: {}", r.message);
    } else {
        print!("{}", r.result["text"].as_str().unwrap_or(""));
    }
    r.status.exit_code()
}

/// Route a worker's on-demand trainer request without asking for approval.
pub fn gauge_or_train_main(command: &str, args: &[String]) -> i32 {
    if command == "gauge" {
        return gauge_main(args);
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        println!("paddock-host train [--deep]: request an independent review of this run\npaddock-host train feedback ADVICE adopted|not-adopted [NOTE]: record feedback");
        return 0;
    }
    if args.first().is_some_and(|arg| arg == "feedback") {
        if !(3..=4).contains(&args.len()) || !matches!(args[2].as_str(), "adopted" | "not-adopted")
        {
            eprintln!("paddock-host train: use train feedback ADVICE adopted|not-adopted [NOTE]");
            return super::EXIT_USAGE;
        }
        let request = Request {
            v: PROTOCOL,
            cap: "train".into(),
            args: [
                ("op".into(), serde_json::json!("feedback")),
                ("advice".into(), serde_json::json!(args[1])),
                ("adopted".into(), serde_json::json!(args[2] == "adopted")),
                (
                    "note".into(),
                    serde_json::json!(args.get(3).map(String::as_str).unwrap_or("")),
                ),
            ]
            .into(),
            reason: String::new(),
        };
        return match super::send(&request) {
            Ok(response) => {
                if response.status == Status::Done {
                    println!("trainer feedback recorded");
                } else {
                    eprintln!("paddock-host train: {}", response.message);
                }
                response.status.exit_code()
            }
            Err(error) => {
                eprintln!("paddock-host train: {error}");
                super::EXIT_NO_HOST
            }
        };
    }
    if args.iter().any(|arg| arg != "--deep") {
        eprintln!("paddock-host train: use train [--deep]");
        return super::EXIT_USAGE;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ u64::from(std::process::id());
    let request = Request {
        v: PROTOCOL,
        cap: "train".into(),
        args: [
            ("request".into(), serde_json::json!(seed)),
            (
                "deep".into(),
                serde_json::json!(args.iter().any(|arg| arg == "--deep")),
            ),
        ]
        .into(),
        reason: String::new(),
    };
    match super::send(&request) {
        Ok(response) => {
            if response.status == Status::Done {
                println!("trainer requested");
            } else {
                eprintln!("paddock-host train: {}", response.message);
            }
            response.status.exit_code()
        }
        Err(error) => {
            eprintln!("paddock-host train: {error}");
            super::EXIT_NO_HOST
        }
    }
}
