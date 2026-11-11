//! Ingest across rewrites of a Codex rollout: a torn last line, and a
//! resume that appends under the same id. Synthetic files only.

use super::*;
use crate::horse::rewrite_check::{assert_no_loss, has_key, nodes};
use std::path::PathBuf;

const ID: &str = "01934b20-2222-7777-8888-123456789abc";

fn meta() -> String {
    format!(
        r#"{{"timestamp":"2026-11-04T00:00:00Z","type":"session_meta","payload":{{"id":"{ID}","session_id":"{ID}","cli_version":"{VERSION}","cwd":"/home/demo/project"}}}}"#
    )
}

fn item(n: u32, text: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-11-04T00:00:{n:02}Z","type":"response_item","payload":{{"type":"message","id":"msg_{n}","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
    )
}

fn event(n: u32, ty: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-11-04T00:00:{n:02}Z","type":"event_msg","payload":{{"type":"{ty}"}}}}"#
    )
}

fn lines(rows: &[String]) -> String {
    rows.iter().map(|r| format!("{r}\n")).collect()
}

fn setup(t: &Path, rows: &[String]) -> (Store, PathBuf) {
    let p = t.join(format!(
        "codex/sessions/2026/11/04/rollout-2026-11-04T00-00-00-{ID}.jsonl"
    ));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, lines(rows)).unwrap();
    (Store::open(&t.join("store")).unwrap(), p)
}

fn turn() -> Vec<String> {
    vec![meta(), item(1, "Run the tests."), event(2, "task_complete")]
}

#[test]
fn a_torn_last_line_waits_and_is_read_once() {
    let t = tempfile::tempdir().unwrap();
    let rows = turn();
    let (st, p) = setup(t.path(), &rows);
    let first = ingest(&st, &p, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    let next = item(3, "Now fix the warning.");
    let half = &next[..next.len() / 2];
    std::fs::write(&p, format!("{}{half}", lines(&rows))).unwrap();
    let torn = ingest(&st, &p, VERSION, None).unwrap();
    assert_eq!((torn.torn, torn.added), (true, 0));
    std::fs::write(&p, format!("{}{next}\n", lines(&rows))).unwrap();
    let done = ingest(&st, &p, VERSION, None).unwrap();
    assert_eq!((done.torn, done.added, done.kept), (false, 1, rows.len()));
    assert_eq!(ingest(&st, &p, VERSION, None).unwrap().added, 0);
    assert_no_loss(&st, &first.session, &before);
}

#[test]
fn a_resume_appends_to_the_rollout_under_the_same_id() {
    let t = tempfile::tempdir().unwrap();
    let rows = turn();
    let (st, p) = setup(t.path(), &rows);
    let first = ingest(&st, &p, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // `codex resume` opens the same file and appends the next turn.
    let mut more = rows.clone();
    more.extend([
        event(3, "thread_settings_applied"),
        item(4, "Go on."),
        event(5, "task_complete"),
    ]);
    std::fs::write(&p, lines(&more)).unwrap();
    let r = ingest(&st, &p, VERSION, None).unwrap();
    assert_eq!(
        (r.session.as_str(), r.kept, r.added),
        (first.session.as_str(), 3, 3)
    );
    assert_no_loss(&st, &first.session, &before);
    assert!(has_key(&nodes(&st, &first.session), "response:msg_4"));
    assert_eq!(st.sessions().unwrap().len(), 1);
}

#[test]
fn a_torn_line_that_a_resume_heals_is_skipped() {
    let t = tempfile::tempdir().unwrap();
    let rows = turn();
    let (st, p) = setup(t.path(), &rows);
    let first = ingest(&st, &p, VERSION, None).unwrap();
    let before = nodes(&st, &first.session);

    // A crash cut the last line. The resumed writer ends the line with a
    // newline before it appends, and Codex skips the line when it reads.
    let lost = item(3, "Lost in the crash.");
    let next = item(4, "Go on.");
    std::fs::write(
        &p,
        format!("{}{}\n{next}\n", lines(&rows), &lost[..lost.len() / 2]),
    )
    .unwrap();
    let r = ingest(&st, &p, VERSION, None).unwrap();
    assert_eq!((r.kept, r.added), (3, 1));
    let chain = nodes(&st, &first.session);
    assert!(has_key(&chain, "response:msg_4") && !has_key(&chain, "response:msg_3"));
    assert_no_loss(&st, &first.session, &before);
    assert_eq!(ingest(&st, &p, VERSION, None).unwrap().added, 0);

    // The projection is the rollout without the broken line.
    let out = project(&st, &first.session, &t.path().join("out"), false).unwrap();
    let mut want = rows.clone();
    want.push(next);
    assert_eq!(std::fs::read_to_string(out.path).unwrap(), lines(&want));
}
