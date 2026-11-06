//! Tests of the request shape, with rows in the shapes that Claude Code
//! writes (`type`, `sessionId`, `id`, `timestamp`, `version`, `querySource`,
//! `shapeHash`, `params`, `keep`, `tail`, `cacheControl`, `requestDigest`).

use super::super::{ingest, transcript_path, VERSION};
use super::*;
use crate::horse::store::Store;
use std::path::Path;

pub(crate) const SID: &str = "5d1c7a8e-2b3f-4c6d-9e0a-1f2b3c4d5e6f";
pub(crate) const CWD: &str = "/home/demo/project";

pub(crate) fn hash(c: char) -> String {
    c.to_string().repeat(64)
}

pub(crate) fn user(uuid: &str, parent: Option<&str>) -> String {
    let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
    format!(
        r#"{{"parentUuid":{parent},"isSidechain":false,"type":"user","message":{{"role":"user","content":"hi {uuid}"}},"uuid":"{uuid}","timestamp":"2026-11-04T10:00:00.000Z","cwd":"{CWD}","sessionId":"{SID}","version":"{VERSION}"}}"#
    )
}

pub(crate) fn assistant(uuid: &str, parent: &str, read: u64, write: u64) -> String {
    format!(
        r#"{{"parentUuid":"{parent}","isSidechain":false,"type":"assistant","message":{{"id":"msg_{uuid}","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{{"type":"text","text":"ok"}}],"usage":{{"input_tokens":3,"cache_creation_input_tokens":{write},"cache_read_input_tokens":{read},"output_tokens":5}}}},"uuid":"{uuid}","timestamp":"2026-11-04T10:00:01.000Z","cwd":"{CWD}","sessionId":"{SID}","version":"{VERSION}"}}"#
    )
}

pub(crate) fn request(shape: &str, at: &str) -> String {
    format!(
        r#"{{"type":"api-request","sessionId":"{SID}","id":"req-{at}","timestamp":"{at}","version":"{VERSION}","querySource":"repl_main_thread","shapeHash":"{shape}","params":{{"model":"claude-opus-5-5","max_tokens":32000}},"keep":0,"tail":["{h}"],"cacheControl":[{{"path":["system",2],"value":{{"type":"ephemeral"}}}},{{"path":["messages",2,"content",0],"value":{{"type":"ephemeral","ttl":"1h"}}}},{{"path":["messages",0,"content",0],"value":{{"type":"ephemeral"}}}}],"requestDigest":"{h}"}}"#,
        h = hash('d')
    )
}

/// Write `rows` as the transcript of `SID` and ingest it into a new store.
pub(crate) fn ingested(t: &Path, rows: &[String]) -> (Store, String) {
    let path = transcript_path(&t.join("claude"), CWD, SID);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut body = rows.join("\n");
    body.push('\n');
    std::fs::write(&path, body).unwrap();
    let st = Store::open(&t.join("horse")).unwrap();
    let got = ingest(&st, &path, VERSION, None).unwrap();
    (st, got.session)
}

#[test]
fn latest_takes_the_last_request_row() {
    let t = tempfile::tempdir().unwrap();
    let rows = [
        user("u1", None),
        request(&hash('a'), "2026-11-04T10:00:00.250Z"),
        assistant("a1", "u1", 0, 6000),
        user("u2", Some("a1")),
        request(&hash('b'), "2026-11-04T10:05:00.500Z"),
        assistant("a2", "u2", 6000, 40),
        r#"{"type":"api-request-shape","shapeHash":"cccc","shape":{"system":[]}}"#.into(),
    ];
    let (st, sid) = ingested(t.path(), &rows);
    let nodes = st.session_nodes(&st.session(&sid).unwrap()).unwrap();
    let s = latest(&nodes).unwrap();
    assert_eq!(s.hash, hash('b'));
    assert_eq!(
        s.at_ms,
        crate::waiting::parse_rfc3339("2026-11-04T10:05:00Z").unwrap() * 1000 + 500
    );
    assert_eq!(s.breakpoints, [0, 2]);
    assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));
}

#[test]
fn a_chain_without_a_request_row_has_no_shape() {
    let t = tempfile::tempdir().unwrap();
    let rows = [
        user("u1", None),
        assistant("a1", "u1", 0, 10),
        r#"{"type":"api-request-blob","hash":"x","message":{}}"#.into(),
    ];
    let (st, sid) = ingested(t.path(), &rows);
    let nodes = st.session_nodes(&st.session(&sid).unwrap()).unwrap();
    assert_eq!(latest(&nodes), None);
    assert_eq!(latest(&[]), None);
}

#[test]
fn request_rows_stay_nodes_byte_for_byte() {
    let t = tempfile::tempdir().unwrap();
    let req = request(&hash('a'), "2026-11-04T10:00:00Z");
    let rows = [user("u1", None), req.clone(), assistant("a1", "u1", 0, 10)];
    let (st, sid) = ingested(t.path(), &rows);
    let nodes = st.session_nodes(&st.session(&sid).unwrap()).unwrap();
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[1].raw, req.as_bytes());
    assert_eq!(nodes[1].meta.key.as_deref(), Some("type:api-request"));
    assert!(row_is_api_request(&nodes[1].raw));
    assert!(!row_is_api_request(&nodes[0].raw));
    assert!(!row_is_api_request(
        br#"{"type":"api-request-shape","shapeHash":"x"}"#
    ));
    assert!(!row_is_api_request(b"not json"));
    // The message view skips it.
    assert_eq!(super::super::view(&nodes[1].raw), None);
    assert!(super::super::view(&nodes[0].raw).is_some());
}

#[test]
fn bad_or_foreign_request_rows_give_no_shape() {
    let ok = request(&hash('a'), "2026-11-04T10:00:00Z");
    assert!(parse(ok.as_bytes()).is_some());
    let agent = ok.replacen(r#""id":"#, r#""agentId":"ag1","id":"#, 1);
    assert_eq!(parse(agent.as_bytes()), None);
    let no_time = ok.replace("2026-11-04T10:00:00Z", "yesterday");
    assert_eq!(parse(no_time.as_bytes()), None);
    let no_hash = ok.replace(&hash('a'), "");
    assert_eq!(parse(no_hash.as_bytes()), None);
    assert_eq!(parse(b"{"), None);
    assert_eq!(
        time_ms("2026-11-04T10:00:00.1Z").map(|m| m % 1000),
        Some(100)
    );
    assert_eq!(time_ms("2026-11-04T10:00:00Z").map(|m| m % 1000), Some(0));
}
