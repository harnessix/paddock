//! The hidden Grok Build commands on the command line: ingest, a fork, a
//! projection and a swap under the same session id. Synthetic files only.

use super::*;

const ID: &str = "019a6000-0000-7000-8000-0000000000aa";
const CWD: &str = "/home/demo/project";

fn update(n: u32, update: &str) -> String {
    let method = if update.contains("turn_completed") {
        "_x.ai/session/update"
    } else {
        "session/update"
    };
    format!(
        r#"{{"timestamp":{n},"method":"{method}","params":{{"sessionId":"{ID}","update":{update},"_meta":{{"eventId":"{ID}-{n}"}}}}}}"#
    )
}

/// A Grok session folder with two turns under the Grok home `home`.
fn grok_session(home: &Path) -> std::path::PathBuf {
    let dir = home.join("sessions/%2Fhome%2Fdemo%2Fproject").join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    let user = |p: u32, t: &str| {
        format!(
            r#"{{"sessionUpdate":"user_message_chunk","content":{{"type":"text","text":"{t}"}},"_meta":{{"promptIndex":{p}}}}}"#
        )
    };
    let agent = |t: &str| {
        format!(
            r#"{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"{t}"}}}}"#
        )
    };
    let done = r#"{"sessionUpdate":"turn_completed","stopReason":"end_turn"}"#;
    let updates = [
        update(1, &user(0, "first")),
        update(2, &agent("one")),
        update(3, done),
        update(4, &user(1, "second")),
        update(5, &agent("two")),
        update(6, done),
    ];
    let chat = [
        r#"{"type":"system","content":"You are Grok."}"#,
        r#"{"type":"user","content":[{"type":"text","text":"first"}],"prompt_index":0}"#,
        r#"{"type":"assistant","content":"one"}"#,
        r#"{"type":"user","content":[{"type":"text","text":"second"}],"prompt_index":1}"#,
        r#"{"type":"assistant","content":"two"}"#,
    ];
    std::fs::write(dir.join("updates.jsonl"), updates.join("\n") + "\n").unwrap();
    std::fs::write(dir.join("chat_history.jsonl"), chat.join("\n") + "\n").unwrap();
    std::fs::write(
        dir.join("summary.json"),
        format!(
            "{{\n  \"info\": {{\n    \"id\": \"{ID}\",\n    \"cwd\": \"{CWD}\"\n  }},\n  \"session_summary\": \"\",\n  \"num_messages\": 6,\n  \"chat_format_version\": 1\n}}"
        ),
    )
    .unwrap();
    dir
}

#[test]
fn ingest_fork_project_and_swap_a_grok_session_on_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let home = t.join("grok-home");
    let dir = grok_session(&home);
    let ingest = |dir: &Path, v: &str| {
        cmd(
            t,
            &["ingest-grok", dir.to_str().unwrap(), "--grok-version", v],
        )
        .output()
        .unwrap()
    };
    // Only the pinned version reads the files.
    let o = ingest(&dir, "1.0.69");
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("1.0.70"),
        "{o:?}"
    );
    let o = ingest(&dir, "1.0.70");
    assert!(o.status.success(), "{o:?}");
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(out.contains(": 11 new, 0 kept"), "{out}");
    let sid = out.split(':').next().unwrap().to_string();

    // The session projects byte for byte into another Grok home.
    let copy = t.join("copy-home");
    let p = paddock(
        t,
        &["project-grok", &sid, "--grok-home", copy.to_str().unwrap()],
    );
    let (id, path) = p.trim().split_once(' ').unwrap();
    assert_eq!(id, ID);
    for f in ["updates.jsonl", "chat_history.jsonl", "summary.json"] {
        assert_eq!(
            std::fs::read(dir.join(f)).unwrap(),
            std::fs::read(Path::new(path).join(f)).unwrap()
        );
    }

    // The end of the first turn: its last chat item, after its updates.
    let log = paddock(t, &["log", &sid]);
    let hash = log
        .lines()
        .find(|l| l.contains("chat assistant: one"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or_else(|| panic!("{log}"))
        .to_string();
    // A fork there is a native fork under a new id.
    let fork = paddock(t, &["fork", &sid, "--at", &hash])
        .trim()
        .to_string();
    let p = paddock(
        t,
        &["project-grok", &fork, "--grok-home", home.to_str().unwrap()],
    );
    assert!(p.contains("This fork starts with a cold cache."), "{p}");
    let (fid, fpath) = p.lines().next().unwrap().split_once(' ').unwrap();
    assert_ne!(fid, ID);
    let summary = std::fs::read_to_string(Path::new(fpath).join("summary.json")).unwrap();
    assert!(
        summary.contains(&format!("\"parent_session_id\":\"{ID}\"")),
        "{summary}"
    );
    assert!(summary.contains("\"session_kind\":\"fork\""), "{summary}");
    let up = std::fs::read_to_string(Path::new(fpath).join("updates.jsonl")).unwrap();
    assert_eq!(up.lines().count(), 3);
    assert!(up.contains(&format!("\"sessionId\":\"{fid}\"")));
    let chat = std::fs::read_to_string(Path::new(fpath).join("chat_history.jsonl")).unwrap();
    assert_eq!(chat.lines().count(), 3, "{chat}");

    // A swap without --grok-home changes nothing; with it the session
    // continues at the end of the first turn under the same id.
    let before = std::fs::read(dir.join("updates.jsonl")).unwrap();
    let o = cmd(t, &["swap", &sid, &hash]).output().unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("--grok-home"),
        "{o:?}"
    );
    assert_eq!(std::fs::read(dir.join("updates.jsonl")).unwrap(), before);
    let o = paddock(
        t,
        &["swap", &sid, &hash, "--grok-home", home.to_str().unwrap()],
    );
    assert!(o.contains(&format!("under Grok session {ID}")), "{o}");
    assert!(o.contains("the old head stays in session"), "{o}");
    let up = std::fs::read_to_string(dir.join("updates.jsonl")).unwrap();
    assert_eq!(up.lines().count(), 3, "{up}");
    assert!(!up.contains("second"));
    let o = ingest(&dir, "1.0.70");
    assert!(
        String::from_utf8_lossy(&o.stdout).starts_with(&format!("{sid}: 0 new")),
        "{o:?}"
    );
}
