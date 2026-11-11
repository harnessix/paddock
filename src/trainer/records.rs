//! Read a run's existing records without modifying its horse session.

use super::brief::{ChecklistItem, Observation, Source};
use crate::{env::HostEnv, horse::Store, mail};
use anyhow::{Context as _, Result};
use serde_json::Value;

fn text_content(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|v| {
                if matches!(
                    v["type"].as_str(),
                    Some("text" | "input_text" | "output_text")
                ) {
                    v["text"].as_str()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Extract only explicit user and assistant messages, not tool results or
/// harness metadata. Unknown native shapes are omitted, not guessed.
pub(super) fn message(harness: &str, row: &Value) -> Option<(String, String)> {
    let msg = match harness {
        "codex" if row["type"] == "response_item" && row["payload"]["type"] == "message" => {
            &row["payload"]
        }
        "claude" if matches!(row["type"].as_str(), Some("user" | "assistant")) => &row["message"],
        "grok"
            if matches!(row["type"].as_str(), Some("user" | "assistant"))
                && row["synthetic_reason"] != "compaction_meta" =>
        {
            row
        }
        "dsh"
            if matches!(
                row["type"].as_str(),
                Some("user/message" | "assistant/message")
            ) =>
        {
            if row["data"]["message"].is_object() {
                &row["data"]["message"]
            } else {
                &row["data"]
            }
        }
        _ => return None,
    };
    if harness == "dsh" && msg["source"]["kind"] == "runtime-context" {
        return None;
    }
    let role = msg["role"]
        .as_str()
        .or_else(|| (harness == "grok").then(|| msg["type"].as_str()).flatten())?;
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let text = text_content(&msg["content"]);
    (!text.is_empty()).then(|| (role.to_owned(), text))
}

fn is_check(command: &str) -> bool {
    [
        "cargo test",
        "cargo clippy",
        "cargo check",
        "pytest",
        "npm test",
        "go test",
    ]
    .iter()
    .any(|check| {
        command
            .trim_start()
            .strip_prefix(check)
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
    })
}

/// Associate Claude's Bash tool use with an explicitly marked result.
pub(super) fn claude_checks(
    row: &Value,
    pending: &mut std::collections::HashMap<String, String>,
) -> Vec<Observation> {
    let mut out = Vec::new();
    let Some(blocks) = row["message"]["content"].as_array() else {
        return out;
    };
    for block in blocks {
        if block["type"] == "tool_use" && block["name"] == "Bash" {
            if let (Some(id), Some(command)) =
                (block["id"].as_str(), block["input"]["command"].as_str())
            {
                if is_check(command) {
                    pending.insert(id.into(), command.into());
                }
            }
        } else if block["type"] == "tool_result" {
            if let Some(name) = block["tool_use_id"]
                .as_str()
                .and_then(|id| pending.remove(id))
            {
                let passed = block["is_error"].as_bool().map(|is_error| !is_error);
                out.push(Observation::Check { name, passed });
            }
        }
    }
    out
}

/// Pair Codex command calls and their explicit exit results. An output that
/// lacks an exit code is not a successful check.
pub(super) fn codex_check(
    row: &Value,
    pending: &mut std::collections::HashMap<String, String>,
) -> Option<Observation> {
    if row["type"] != "response_item" {
        return None;
    }
    let payload = &row["payload"];
    let id = payload["call_id"].as_str()?;
    if payload["type"] == "function_call" {
        if !matches!(
            payload["name"].as_str(),
            Some("exec_command" | "shell" | "shell_command")
        ) {
            return None;
        }
        let args: Value = serde_json::from_str(payload["arguments"].as_str()?).ok()?;
        let command = args["cmd"].as_str().or_else(|| args["command"].as_str())?;
        if is_check(command) {
            pending.insert(id.to_owned(), command.to_owned());
        }
        return None;
    }
    if payload["type"] != "function_call_output" {
        return None;
    }
    let command = pending.remove(id)?;
    let output = payload["output"].as_str()?;
    let code = output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Process exited with code ")
            .and_then(|n| n.parse::<i32>().ok())
    })?;
    Some(Observation::Check {
        name: command,
        passed: Some(code == 0),
    })
}

pub(super) fn add_message(source: &mut Source, role: String, text: String) {
    if role == "assistant" {
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| line.ends_with('?') && !line.is_empty())
        {
            source
                .observations
                .push(Observation::Question(line.to_owned()));
        }
    }
    if source.goal.is_empty() && role == "user" {
        source.goal = text.clone();
    }
    source.observations.push(Observation::Turn { role, text });
}

/// Read a run's horse, mailbox, and (when present) Spec Kit/gauge records.
/// `base` must be supplied by the caller that owns the run's recorded base;
/// without one no comparison is invented.
pub fn read(env: &HostEnv, store: &Store, run: &str, base: Option<&str>) -> Result<Source> {
    let info = mail::run_info(env, run).context("run has no mailbox information")?;
    let horse =
        crate::horse::run::by_sandbox::find(store, run)?.context("run has no horse session")?;
    let session = store.session(&horse.session)?;
    let mut source = Source::default();
    let mut pending = std::collections::HashMap::new();
    let nodes = store.session_nodes(&session)?;
    if horse.harness == "agy" {
        let native: Vec<_> = nodes
            .into_iter()
            .filter(|node| {
                node.meta.harness.as_deref() == Some("agy")
                    && matches!(
                        node.meta.container.as_deref(),
                        Some(crate::horse::agy::ROW | crate::horse::agy::DROP)
                    )
            })
            .collect();
        let rows = crate::horse::agy::replay(&native)?;
        for row in crate::horse::agy::steps(&rows) {
            let ty = row
                .get("step_type")
                .and_then(crate::horse::agy::row::Cell::int);
            let (role, field) = match ty {
                Some(14) => ("user", 19),
                Some(15) => ("assistant", 20),
                _ => continue,
            };
            let Some(payload) = row
                .get("step_payload")
                .and_then(crate::horse::agy::row::Cell::bytes)
            else {
                continue;
            };
            let Some(message) = crate::horse::agy::proto::bytes(payload, field)? else {
                continue;
            };
            let Some(text) = crate::horse::agy::proto::bytes(message, 1)?
                .and_then(|b| std::str::from_utf8(b).ok())
            else {
                continue;
            };
            add_message(&mut source, role.to_owned(), text.to_owned());
        }
    } else {
        let raw: Vec<Vec<u8>> = if horse.harness == "grok" {
            let native: Vec<_> = nodes
                .into_iter()
                .filter(|node| {
                    node.meta.harness.as_deref() == Some("grok")
                        && matches!(
                            node.meta.container.as_deref(),
                            Some(crate::horse::grok::CHAT | crate::horse::grok::UPDATES)
                        )
                })
                .collect();
            crate::horse::grok::replay::replay(&native)?.chat
        } else {
            nodes.into_iter().map(|node| node.raw).collect()
        };
        for bytes in raw {
            let Ok(row) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if horse.harness == "codex" {
                if let Some(check) = codex_check(&row, &mut pending) {
                    source.observations.push(check);
                }
            }
            if horse.harness == "claude" {
                source
                    .observations
                    .extend(claude_checks(&row, &mut pending));
            }
            if let Some((role, text)) = message(&horse.harness, &row) {
                add_message(&mut source, role, text);
            }
        }
    }
    for m in mail::read(env, run, mail::Dir::Up) {
        match m.kind {
            mail::Kind::Ask => source.observations.push(Observation::Question(m.text)),
            mail::Kind::Status => source.observations.push(Observation::Status(m.text)),
            mail::Kind::Event
                if matches!(
                    m.event.as_deref(),
                    Some("idle" | "stall" | "trainer-before-done")
                ) =>
            {
                source.observations.push(Observation::Event(format!(
                    "{}: {}",
                    m.event.as_deref().unwrap_or("event"),
                    m.text
                )))
            }
            _ => {}
        }
    }
    if let Ok((mut input, layout)) =
        crate::gauge::collect::collect(env, &info.project, None, crate::state::now(), false)
    {
        if let Some(text) = layout.read(&crate::speckit::Layout::tasks_rel(&input.feature)) {
            input.snapshots.push(crate::gauge::history::Snapshot {
                at: input.now,
                source: crate::gauge::history::Source::Current,
                tasks: crate::gauge::history::from_text(&text),
            });
        }
        for gate in input.gates.iter().filter(|gate| {
            input
                .records
                .get(&gate.task)
                .and_then(|record| record.sandbox.as_deref())
                == Some(run)
        }) {
            match gate.outcome {
                crate::gauge::evidence::Outcome::Pass => {
                    source.observations.push(Observation::Check {
                        name: gate.gate.clone(),
                        passed: Some(true),
                    })
                }
                crate::gauge::evidence::Outcome::Fail => {
                    source.observations.push(Observation::Check {
                        name: gate.gate.clone(),
                        passed: Some(false),
                    })
                }
                crate::gauge::evidence::Outcome::Skip => {
                    source.observations.push(Observation::Check {
                        name: gate.gate.clone(),
                        passed: None,
                    })
                }
            }
        }
        let report = crate::gauge::analyze(&input, &crate::gauge::Opts::default());
        source.gauge.insert("scope".into(), report.counts.scope);
        source.gauge.insert("done".into(), report.counts.done);
        source.gauge.insert("proven".into(), report.counts.proven);
        source.gauge.insert("open".into(), report.counts.open);
        source.checklist_known = true;
        source.checklist_source = "current gauge".into();
        source.checklist = report
            .tasks
            .into_iter()
            .map(|task| ChecklistItem {
                text: format!("{} {}", task.id, task.text),
                claimed: task.done,
                passed: task.evidence == Some(crate::gauge::evidence::Status::Proven),
                elapsed_secs: task.agent_secs,
            })
            .collect();
    }
    if let (Some(base), Ok(Some(git))) = (base, crate::hostgit::Layout::discover(&info.project)) {
        // Restrict revisions to a verified object id rather than options or refs.
        if (7..=64).contains(&base.len()) && base.bytes().all(|b| b.is_ascii_hexdigit()) {
            let range = format!("{base}..HEAD");
            if let Ok(log) = git.query(&["log", "--format=%H", &range]) {
                source
                    .observations
                    .extend(log.lines().map(|s| Observation::Commit(s.into())));
            }
            source.diff_stat = git
                .sandboxed_read_only(
                    env,
                    &[
                        "diff",
                        "--stat",
                        "--no-ext-diff",
                        "--no-textconv",
                        base,
                        "--",
                    ],
                )
                .ok()
                .flatten()
                .unwrap_or_else(|| "worktree diff stat unavailable".into());
        }
    }
    Ok(source)
}

fn checklist_from_goal(source: &mut Source) {
    for line in source.goal.lines().map(str::trim) {
        let Some(rest) = line
            .strip_prefix("- [")
            .or_else(|| line.strip_prefix("* ["))
        else {
            continue;
        };
        let Some((mark, text)) = rest.split_once(']') else {
            continue;
        };
        if !matches!(mark, " " | "x" | "X") {
            continue;
        }
        source.checklist.push(ChecklistItem {
            text: text.trim().into(),
            claimed: mark != " ",
            passed: false,
            elapsed_secs: None,
        });
    }
    source.checklist_known = !source.checklist.is_empty();
    if source.checklist_known {
        source.checklist_source = "task at run start".into();
    }
}

/// Build a trainer brief only when the original run recorded its effective
/// credential masks. Older runs fail closed rather than leaking hidden paths.
pub fn brief_for_run(
    env: &HostEnv,
    store: &Store,
    run: &str,
    turns: usize,
    bytes: usize,
) -> Result<String> {
    let anchor = super::anchor::load(store, run)?
        .context("run has no trainer anchor or recorded credential masks")?;
    let mut source = read(env, store, run, anchor.base.as_deref())?;
    if !anchor.goal.is_empty() {
        source.goal = anchor.goal;
    }
    if !source.checklist_known {
        checklist_from_goal(&mut source);
    }
    super::brief::render(
        super::brief::build(&source, &anchor.hidden, turns, bytes),
        bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::horse::{dsh, live::fake, run::RunHorse};

    #[test]
    fn task_checkboxes_are_claims_without_independent_evidence() {
        let mut source = Source {
            goal: "- [x] Implement\n- [ ] Verify".into(),
            ..Default::default()
        };
        checklist_from_goal(&mut source);
        assert!(source.checklist_known);
        assert_eq!(source.checklist.len(), 2);
        assert!(source.checklist[0].claimed);
        assert!(!source.checklist[0].passed);
        assert!(!source.checklist[1].claimed);
    }

    #[test]
    fn diff_helper_mounts_the_worktree_read_only() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir()?;
        let bin = temp.path().join("bin");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&bin)?;
        std::fs::create_dir_all(project.join(".git/objects"))?;
        std::fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        let args_file = temp.path().join("args");
        let bwrap = bin.join("bwrap");
        std::fs::write(
            &bwrap,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '1 file changed\\n'\n",
                args_file.display()
            ),
        )?;
        std::fs::set_permissions(&bwrap, std::fs::Permissions::from_mode(0o755))?;
        let git = bin.join("git");
        std::fs::write(&git, "#!/bin/sh\nexit 0\n")?;
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755))?;
        let mut env = crate::privfs::tests::env_in(temp.path());
        env.path = vec![bin];
        let layout = crate::hostgit::Layout::discover(&project)?.unwrap();
        assert_eq!(
            layout
                .sandboxed_read_only(&env, &["diff", "--stat"])?
                .as_deref(),
            Some("1 file changed\n")
        );
        let args = std::fs::read_to_string(args_file)?;
        assert!(args.contains(&format!(
            "--ro-bind\n{}\n{}\n",
            project.display(),
            project.display()
        )));
        assert!(!args.contains(&format!(
            "--bind\n{}\n{}\n",
            project.display(),
            project.display()
        )));
        Ok(())
    }

    #[test]
    fn claude_check_results_require_a_matching_tool_call() {
        let mut pending = std::collections::HashMap::new();
        let call = serde_json::json!({"type":"assistant", "message":{"content":[{"type":"tool_use","name":"Bash","id":"c1","input":{"command":"cargo test foo"}}]}});
        let result = serde_json::json!({"type":"user", "message":{"content":[{"type":"tool_result","tool_use_id":"c1","is_error":true}]}});
        assert!(claude_checks(&call, &mut pending).is_empty());
        assert!(matches!(
            claude_checks(&result, &mut pending).as_slice(),
            [Observation::Check {
                passed: Some(false),
                ..
            }]
        ));
        assert!(claude_checks(&result, &mut pending).is_empty());
    }

    #[test]
    fn codex_records_only_checks_with_completed_exit_results() {
        let mut pending = std::collections::HashMap::new();
        let call = serde_json::json!({"type":"response_item", "payload":{"type":"function_call", "name":"exec_command", "call_id":"x", "arguments":"{\"cmd\":\"cargo test widget\"}"}});
        let out = serde_json::json!({"type":"response_item", "payload":{"type":"function_call_output", "call_id":"x", "output":"Process exited with code 1\nfailed"}});
        assert!(codex_check(&call, &mut pending).is_none());
        assert!(matches!(
            codex_check(&out, &mut pending),
            Some(Observation::Check {
                passed: Some(false),
                ..
            })
        ));
        assert!(codex_check(&out, &mut pending).is_none());
    }

    #[test]
    fn codex_trajectory_reports_repeated_checks_and_questions() -> Result<()> {
        use crate::horse::live::codex::fake as codex_fake;
        use serde_json::json;
        let temp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(temp.path());
        let store = Store::open_host(&env)?;
        let horse = store.new_default_horse()?;
        let native_id = codex_fake::OLD_THREAD;
        let mut rows = vec![
            codex_fake::meta(native_id),
            codex_fake::row(
                "response_item",
                codex_fake::message("user", "input_text", "Fix the check"),
            ),
        ];
        for i in 0..3 {
            let id = format!("call-{i}");
            rows.push(codex_fake::row("response_item", json!({"type":"function_call","name":"exec_command","call_id":id,"arguments":"{\"cmd\":\"cargo test widget\"}"})));
            rows.push(codex_fake::row("response_item", json!({"type":"function_call_output","call_id":id,"output":"Process exited with code 1\nfailed"})));
        }
        for _ in 0..2 {
            rows.push(codex_fake::row(
                "response_item",
                codex_fake::message("assistant", "output_text", "Which branch?"),
            ));
        }
        let native = codex_fake::write_rollout(&temp.path().join("native"), native_id, &rows);
        let id = crate::horse::codex::ingest(
            &store,
            &native,
            crate::horse::codex::VERSION,
            Some(&horse.id),
        )?
        .session;
        let before = store.session(&id)?;
        crate::horse::run::by_sandbox::record(
            &store,
            "codex-run",
            &RunHorse {
                horse: horse.id,
                session: id.clone(),
                harness: "codex".into(),
            },
        )?;
        mail::create(&env, "codex-run", &mail::Starter::host())?;
        mail::set_run_info(
            &env,
            &mail::RunInfo {
                id: "codex-run".into(),
                harness: "codex".into(),
                project: temp.path().into(),
                pid: 0,
                pid_start: None,
                started: 0,
                detached: false,
                delivery: None,
                horse_session: Some(id.clone()),
            },
        )?;
        let source = read(&env, &store, "codex-run", None)?;
        let brief = super::super::brief::build(&source, &[], 8, 4096);
        assert_eq!(brief.repeated_failures["cargo test widget"], 3);
        assert_eq!(brief.repeated_questions["Which branch?"], 2);
        assert_eq!(store.session(&id)?, before);
        Ok(())
    }

    #[test]
    fn reads_fake_grok_and_agy_native_histories() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(temp.path());
        let store = Store::open_host(&env)?;
        let horse = store.new_default_horse()?;
        let mut grok = crate::horse::grok::fixture::Fixture::new(
            crate::horse::grok::fixture::ID,
            crate::horse::grok::fixture::CWD,
        );
        grok.prompt(0, "grok goal").reply("grok reply");
        let path = grok.write(&temp.path().join("grok"));
        let grok_id = crate::horse::grok::ingest(
            &store,
            &path,
            crate::horse::grok::VERSION,
            Some(&horse.id),
        )?
        .session;
        let mut agy = crate::horse::agy::fixture::Fixture::new(crate::horse::agy::fixture::ID);
        agy.user("agy goal").reply("agy reply");
        let path = agy.write(&temp.path().join("agy"));
        let agy_id =
            crate::horse::agy::ingest(&store, &path, crate::horse::agy::VERSION, Some(&horse.id))?
                .session;
        for (harness, id, run) in [("grok", grok_id, "grok-run"), ("agy", agy_id, "agy-run")] {
            crate::horse::run::by_sandbox::record(
                &store,
                run,
                &RunHorse {
                    horse: horse.id.clone(),
                    session: id.clone(),
                    harness: harness.into(),
                },
            )?;
            mail::create(&env, run, &mail::Starter::host())?;
            mail::set_run_info(
                &env,
                &mail::RunInfo {
                    id: run.into(),
                    harness: harness.into(),
                    project: temp.path().into(),
                    pid: 0,
                    pid_start: None,
                    started: 0,
                    detached: false,
                    delivery: None,
                    horse_session: Some(id.clone()),
                },
            )?;
            let before = store.session(&id)?;
            let source = read(&env, &store, run, None)?;
            let brief = super::super::brief::build(&source, &[], 8, 4096);
            assert_eq!(brief.goal, format!("{harness} goal"));
            assert!(brief
                .turns
                .iter()
                .any(|turn| turn.text == format!("{harness} reply")));
            assert_eq!(before, store.session(&id)?);
        }
        Ok(())
    }

    #[test]
    fn reads_fake_session_and_mail_without_changing_the_session() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let env = crate::privfs::tests::env_in(temp.path());
        let store = Store::open_host(&env)?;
        let horse = store.new_default_horse()?;
        let root = temp.path().join("fake-native");
        fake::write_file(
            &root,
            "session-a",
            &fake::turn_rows(0, 1, "finish the task", "m1"),
        );
        let native = dsh::session_dir(&root, Some(fake::CWD), "session-a");
        let id = dsh::ingest(&store, &native, dsh::VERSION, Some(&horse.id))?.session;
        let before = store.session(&id)?;
        let rh = RunHorse {
            horse: horse.id,
            session: id.clone(),
            harness: "dsh".into(),
        };
        crate::horse::run::by_sandbox::record(&store, "test-run", &rh)?;
        mail::create(&env, "test-run", &mail::Starter::host())?;
        mail::set_run_info(
            &env,
            &mail::RunInfo {
                id: "test-run".into(),
                harness: "dsh".into(),
                project: temp.path().into(),
                pid: 0,
                pid_start: None,
                started: 0,
                detached: false,
                delivery: None,
                horse_session: Some(id.clone()),
            },
        )?;
        for _ in 0..2 {
            mail::append(
                &env,
                "test-run",
                mail::Dir::Up,
                mail::Message::new("worker", mail::Kind::Ask, "Which branch?"),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        mail::append(
            &env,
            "test-run",
            mail::Dir::Up,
            mail::Message::new("worker", mail::Kind::Status, "done: partial"),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::create_dir_all(temp.path().join(".specify"))?;
        std::fs::create_dir_all(temp.path().join("specs/001-test"))?;
        std::fs::write(
            temp.path().join("specs/001-test/tasks.md"),
            "## Phase 1\n- [ ] T001 Prove the change\n- [x] T002 Claimed done\n",
        )?;
        let source = read(&env, &store, "test-run", None)?;
        let brief = super::super::brief::build(&source, &[], 8, 4096);
        assert_eq!(brief.goal, "finish the task");
        assert_eq!(brief.repeated_questions["Which branch?"], 2);
        assert_eq!(brief.statuses, ["done: partial"]);
        assert!(brief.checklist_known);
        assert_eq!(brief.gauge["scope"], 2);
        assert_eq!(brief.checklist.len(), 2);
        assert!(brief.checklist.iter().all(|item| !item.passed));
        assert!(brief
            .turns
            .iter()
            .any(|turn| turn.text.contains("finish the task")));
        assert_eq!(store.session(&id)?, before);
        assert!(brief_for_run(&env, &store, "test-run", 8, 4096).is_err());
        super::super::anchor::save(
            &store,
            "test-run",
            &super::super::anchor::Anchor {
                goal: "Do not read private/config".into(),
                base: None,
                hidden: vec!["private/config".into()],
                ceiling: None,
            },
        )?;
        let protected = brief_for_run(&env, &store, "test-run", 8, 4096)?;
        assert!(!protected.contains("private/config"));
        Ok(())
    }
}
