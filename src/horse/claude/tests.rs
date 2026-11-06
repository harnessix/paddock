//! Tests of the Claude Code codec, with a parent transcript built by hand in
//! the shapes that the 2.1.317 binary writes (`insertMessageChain`: the
//! linkage, the message, then the session fields).

use super::request::{load, types, usage};
use super::*;
use crate::horse::store::Store;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const CWD: &str = "/home/demo/project";
const DIR: &str = "-home-demo-project";
const P: &str = "0b9e3f52-7a1c-4f0e-9d55-2c6a8e1b4d70";

const U1: &str = r#"{"role":"user","content":"Read notes.txt and say what it holds."}"#;
/// Thinking with its signature, an escape and a number with a trailing zero:
/// a parse and a new write would change these bytes.
const A1: &str = r#"{"id":"msg_01","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"I need the file. \u00e9t\u00e9","signature":"EqQBCkYIBxgCKkB+/sig//one=="},{"type":"tool_use","id":"toolu_01","name":"Read","input":{"file_path":"/home/demo/project/notes.txt"}}],"stop_reason":"tool_use","usage":{"input_tokens":3,"cache_creation_input_tokens":6000,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":6000},"output_tokens":40,"service_tier":"standard","speed":1.50}}"#;
const R1: &str = r#"{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":"1\tthe horse store works"}]}"#;
const A2: &str = r#"{"id":"msg_02","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"notes.txt says: the horse store works."}],"stop_reason":"end_turn","usage":{"input_tokens":5,"cache_creation_input_tokens":60,"cache_read_input_tokens":6000,"output_tokens":12}}"#;
const U2: &str = r#"{"role":"user","content":"Now count its words."}"#;
const A3: &str = r#"{"id":"msg_03","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"Four words."}],"usage":{"input_tokens":4,"cache_creation_input_tokens":20,"cache_read_input_tokens":6060,"output_tokens":3}}"#;
const U3: &str = r#"{"role":"user","content":"Instead, say it in French."}"#;
const A4: &str = r#"{"id":"msg_04","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"Le magasin fonctionne."}],"usage":{"input_tokens":4,"cache_creation_input_tokens":25,"cache_read_input_tokens":6060,"output_tokens":6}}"#;

/// A chain row as `insertMessageChain` builds it.
fn chain(ty: &str, uuid: &str, parent: Option<&str>, message: &str, sid: &str) -> String {
    let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
    format!(
        r#"{{"parentUuid":{parent},"isSidechain":false,"type":"{ty}","message":{message},"uuid":"{uuid}","timestamp":"2026-11-04T10:00:00.000Z","userType":"external","entrypoint":"cli","cwd":"{CWD}","sessionId":"{sid}","version":"{VERSION}","gitBranch":"main","sessionKind":"interactive"}}"#
    )
}

fn meta(body: &str, sid: &str) -> String {
    format!(r#"{{{body},"sessionId":"{sid}"}}"#)
}

fn last_prompt(leaf: &str, sid: &str) -> String {
    meta(
        &format!(r#""type":"last-prompt","lastPrompt":"x","leafUuid":"{leaf}""#),
        sid,
    )
}

/// Two turns, a rewind to the end of turn 1 and a new turn 2. The active
/// leaf is a4; u2 and a3 are an abandoned branch.
fn parent_rows() -> Vec<Vec<u8>> {
    [
        chain("user", "u1", None, U1, P),
        chain("assistant", "a1", Some("u1"), A1, P),
        chain("user", "r1", Some("a1"), R1, P),
        r#"{"type":"api-request-shape","shapeHash":"ab12","shape":{"system":[]}}"#.into(),
        meta(
            r#""type":"content-replacement","replacements":[{"toolUseId":"toolu_01","path":"tool-results/toolu_01.txt"}]"#,
            P,
        ),
        chain("assistant", "a2", Some("r1"), A2, P),
        last_prompt("a2", P),
        chain("user", "u2", Some("a2"), U2, P),
        chain("assistant", "a3", Some("u2"), A3, P),
        last_prompt("a3", P),
        meta(r#""type":"tools-used","tools":["Read"]"#, P),
        last_prompt("a2", P),
        chain("user", "u3", Some("a2"), U3, P),
        chain("assistant", "a4", Some("u3"), A4, P),
        last_prompt("a4", P),
    ]
    .into_iter()
    .map(String::into_bytes)
    .collect()
}

fn bytes(rows: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        out.extend_from_slice(r);
        out.push(b'\n');
    }
    out
}

/// A config folder with the parent transcript in it.
fn setup(t: &Path, rows: &[Vec<u8>]) -> PathBuf {
    let path = transcript_path(&t.join("claude"), CWD, P);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes(rows)).unwrap();
    path
}

fn store(t: &Path) -> Store {
    Store::open(&t.join("horse")).unwrap()
}

#[test]
fn project_dir_names_follow_claude_code() {
    assert_eq!(project_dir_name(CWD), DIR);
    assert_eq!(
        project_dir_name("/home/demo/my.app_x"),
        "-home-demo-my-app-x"
    );
    assert_eq!(project_dir_name("/é/😀"), "-----");
    assert_eq!(java_hash(""), 0);
    assert_eq!(java_hash("hello"), 99_162_322);
    assert_eq!(java_hash("polygenelubricants"), i32::MIN);
    let long = format!("/home/demo/{}", "a".repeat(300));
    let name = project_dir_name(&long);
    let tail = base36(java_hash(&long).unsigned_abs());
    assert_eq!(name.len(), 201 + tail.len());
    assert!(name.ends_with(&format!("-{tail}")));
    assert_eq!(base36(35), "z");
    assert_eq!(base36(36), "10");
    assert_eq!(
        transcript_path(Path::new("/home/demo/.claude"), CWD, P),
        Path::new(&format!("/home/demo/.claude/projects/{DIR}/{P}.jsonl"))
    );
}

#[test]
fn ingest_and_project_round_trip_byte_for_byte() {
    let t = tempfile::tempdir().unwrap();
    let rows = parent_rows();
    let file = setup(t.path(), &rows);
    let st = store(t.path());
    let r = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!(
        (r.added, r.kept, r.rewritten, r.torn),
        (15, 0, false, false)
    );
    let s = st.session(&r.session).unwrap();
    assert_eq!(s.binding.native_id.as_deref(), Some(P));
    assert_eq!(s.binding.harness, "claude");
    assert!(st.resolve_node(&s, "uuid:a3").is_ok());

    // The same file again adds nothing.
    let again = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!(
        (again.session.as_str(), again.added, again.kept),
        (r.session.as_str(), 0, 15)
    );

    let out = t.path().join("sandbox/.claude");
    let p = project(&st, &r.session, &out).unwrap();
    assert!(!p.forked);
    assert_eq!(p.path, out.join(format!("projects/{DIR}/{P}.jsonl")));
    assert_eq!(
        std::fs::read(&p.path).unwrap(),
        std::fs::read(&file).unwrap()
    );
    // Never over a file.
    let err = project(&st, &r.session, &out).unwrap_err().to_string();
    assert!(err.contains("never replaces"), "{err}");
}

#[test]
fn rows_dedupe_by_uuid_and_metadata_keeps_its_place() {
    let t = tempfile::tempdir().unwrap();
    let mut rows = parent_rows();
    let file = setup(t.path(), &rows);
    let st = store(t.path());
    let r = ingest(&st, &file, VERSION, None).unwrap();
    // A second writer appends a4 again with other bytes, then the same
    // last-prompt row as before, then a new turn.
    let dup = chain("assistant", "a4", Some("u3"), A3, P).into_bytes();
    rows.push(dup.clone());
    rows.push(last_prompt("a4", P).into_bytes());
    rows.push(chain("user", "u4", Some("a4"), U2, P).into_bytes());
    std::fs::write(&file, bytes(&rows)).unwrap();
    let r2 = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((r2.added, r2.kept, r2.rewritten), (2, 16, false));
    let nodes = st.session_nodes(&st.session(&r.session).unwrap()).unwrap();
    assert_eq!(nodes.len(), 17);
    assert!(nodes.iter().all(|n| n.raw != dup));
    // Both equal last-prompt rows stay: the order of the leaf pointers counts.
    let lp = last_prompt("a4", P).into_bytes();
    assert_eq!(nodes.iter().filter(|n| n.raw == lp).count(), 2);
    assert_eq!(nodes[15].raw, lp);
}

#[test]
fn a_torn_last_line_waits() {
    let t = tempfile::tempdir().unwrap();
    let rows = parent_rows();
    let file = setup(t.path(), &rows);
    let mut b = std::fs::read(&file).unwrap();
    b.extend_from_slice(br#"{"parentUuid":"a4","type":"us"#);
    std::fs::write(&file, &b).unwrap();
    let st = store(t.path());
    let r = ingest(&st, &file, VERSION, None).unwrap();
    assert!(r.torn);
    assert_eq!(r.added, 15);
}

#[test]
fn other_versions_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let rows = parent_rows();
    let file = setup(t.path(), &rows);
    let st = store(t.path());
    let err = ingest(&st, &file, "2.1.316", None).unwrap_err().to_string();
    assert!(err.contains("not supported"), "{err}");

    let r = ingest(&st, &file, VERSION, None).unwrap();
    st.update_session(&r.session, |s| s.binding.version = "2.1.318".into())
        .unwrap();
    let err = project(&st, &r.session, &t.path().join("out"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("2.1.318 is not supported"), "{err}");

    let old = String::from_utf8(rows[1].clone())
        .unwrap()
        .replace(VERSION, "2.1.316");
    let mut rows2 = rows.clone();
    rows2.push(old.into_bytes());
    std::fs::write(&file, bytes(&rows2)).unwrap();
    let err = ingest(&st, &file, VERSION, None).unwrap_err().to_string();
    assert!(err.contains("Claude Code 2.1.316"), "{err}");

    // A file name that is not a session id.
    let bad = file.with_file_name("notes.jsonl");
    std::fs::write(&bad, bytes(&rows)).unwrap();
    assert!(ingest(&st, &bad, VERSION, None).is_err());
}

#[test]
fn a_rewrite_of_a_big_file_keeps_the_dropped_rows() {
    let t = tempfile::tempdir().unwrap();
    let pad = "x".repeat(100 * 1024);
    let mut rows = parent_rows();
    // Turn 3 with large tool results: the file grows past 5 MiB.
    let mut prev = "a4".to_string();
    for i in 0..60 {
        let uuid = format!("big{i}");
        let msg = format!(r#"{{"role":"user","content":"{pad}"}}"#);
        rows.push(chain("user", &uuid, Some(&prev), &msg, P).into_bytes());
        prev = uuid;
    }
    let boundary = format!(
        r#"{{"parentUuid":null,"logicalParentUuid":"{prev}","isSidechain":false,"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false,"compactMetadata":{{"trigger":"auto","preTokens":160000}},"uuid":"cb1","timestamp":"2026-11-04T11:00:00.000Z","userType":"external","entrypoint":"cli","cwd":"{CWD}","sessionId":"{P}","version":"{VERSION}","gitBranch":"main"}}"#
    );
    rows.push(boundary.clone().into_bytes());
    let summary = chain(
        "user",
        "sum1",
        Some("cb1"),
        r#"{"role":"user","content":"Summary: notes.txt says the horse store works."}"#,
        P,
    );
    rows.push(summary.clone().into_bytes());
    rows.push(last_prompt("sum1", P).into_bytes());
    let file = setup(t.path(), &rows);
    assert!(std::fs::metadata(&file).unwrap().len() >= 5 * 1024 * 1024);
    let st = store(t.path());
    let r = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!(r.added, rows.len());
    let ino = std::fs::metadata(&file).unwrap().ino();

    // Claude Code rewrites: the rows before the boundary go, the metadata
    // comes again, a new turn follows. A temporary file is renamed over.
    let after = [
        boundary.clone(),
        summary.clone(),
        meta(
            r#""type":"content-replacement","replacements":[{"toolUseId":"toolu_01","path":"tool-results/toolu_01.txt"}]"#,
            P,
        ),
        meta(r#""type":"tools-used","tools":["Read"]"#, P),
        last_prompt("sum1", P),
        chain("assistant", "a5", Some("sum1"), A2, P),
    ]
    .map(String::into_bytes)
    .to_vec();
    let tmp = file.with_extension("jsonl.compact.tmp.0a1b2c3d");
    std::fs::write(&tmp, bytes(&after)).unwrap();
    std::fs::rename(&tmp, &file).unwrap();
    assert_ne!(std::fs::metadata(&file).unwrap().ino(), ino);

    let r2 = ingest(&st, &file, VERSION, None).unwrap();
    assert!(r2.rewritten);
    assert_eq!((r2.kept, r2.added), (2, 4));
    let s = st.session(&r.session).unwrap();
    let nodes = st.session_nodes(&s).unwrap();
    assert_eq!(nodes.len(), rows.len() + 4);
    // The rows that Claude Code dropped are still there, byte for byte.
    assert_eq!(nodes[0].raw, rows[0]);
    assert!(st.resolve_node(&s, "uuid:big0").is_ok());

    // Later appends to the rewritten file are appends again.
    let mut after2 = after.clone();
    after2.push(last_prompt("a5", P).into_bytes());
    std::fs::write(&file, bytes(&after2)).unwrap();
    let r3 = ingest(&st, &file, VERSION, None).unwrap();
    assert_eq!((r3.rewritten, r3.added, r3.kept), (false, 1, 6));

    // The whole history projects, and a fork before the boundary works.
    let out = t.path().join("out");
    let p = project(&st, &r.session, &out).unwrap();
    let full = std::fs::read(&p.path).unwrap();
    assert!(full.starts_with(&bytes(&rows)));
    let at = st.resolve_node(&s, "uuid:a1").unwrap();
    let f = st.fork(&r.session, &at, None).unwrap();
    let fp = project(&st, &f.id, &t.path().join("out2")).unwrap();
    let frows = read_file(&fp.path).unwrap().rows;
    assert_eq!(load(&frows).unwrap().leaf, "a1");
}

/// What `--fork-session` writes for the same path, as the research reads
/// it: the rows with the new session id and no fork marker.
fn native_fork_session(parent: &[Vec<u8>], uuids: &[&str], sid: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for u in uuids {
        let r = parent
            .iter()
            .find(|r| info(r).unwrap().uuid.as_deref() == Some(u))
            .unwrap();
        out.push(json::edit(r, &[("sessionId", json::quote(sid))], &[]).unwrap());
    }
    for r in parent {
        let ty = info(r).unwrap().ty;
        if ty == "content-replacement" || ty == "tools-used" {
            out.push(json::edit(r, &[("sessionId", json::quote(sid))], &[]).unwrap());
        }
    }
    out
}

#[test]
fn a_fork_projects_like_branch_and_sends_the_same_prefix() {
    let t = tempfile::tempdir().unwrap();
    let rows = parent_rows();
    let file = setup(t.path(), &rows);
    let st = store(t.path());
    let r = ingest(&st, &file, VERSION, None).unwrap();
    let s = st.session(&r.session).unwrap();
    let head = s.head.clone().unwrap();
    let f = st.fork(&r.session, &head, None).unwrap();
    let cfg = t.path().join("sandbox/.claude");
    let p = project(&st, &f.id, &cfg).unwrap();
    assert!(p.forked);
    check_session_id(&p.native_id).unwrap();
    assert_ne!(p.native_id, P);
    assert_eq!(
        p.path,
        cfg.join(format!("projects/{DIR}/{}.jsonl", p.native_id))
    );
    let mode = |q: &Path| std::fs::metadata(q).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&p.path), 0o600);
    assert_eq!(mode(p.path.parent().unwrap()), 0o700);

    // The file is a transcript of the pinned version that the codec reads.
    let fork = read_file(&p.path).unwrap();
    assert_eq!(fork.session_id, p.native_id);
    let infos: Vec<Info> = fork.rows.iter().map(|r| info(r).unwrap()).collect();
    let chain_uuids: Vec<&str> = infos
        .iter()
        .filter(|n| n.is_chain())
        .map(|n| n.uuid.as_deref().unwrap())
        .collect();
    assert_eq!(chain_uuids, ["u1", "a1", "r1", "a2", "u3", "a4"]);
    let parents: Vec<Option<&str>> = infos
        .iter()
        .filter(|n| n.is_chain())
        .map(|n| n.parent.as_deref())
        .collect();
    assert_eq!(
        parents,
        [
            None,
            Some("u1"),
            Some("a1"),
            Some("r1"),
            Some("a2"),
            Some("u3")
        ]
    );
    for (n, raw) in infos.iter().zip(&fork.rows) {
        assert_eq!(n.session_id.as_deref(), Some(p.native_id.as_str()));
        if n.is_chain() {
            let from = json::raw_value(raw, "forkedFrom").unwrap().unwrap();
            let want = format!(
                r#"{{"sessionId":"{P}","messageUuid":"{}"}}"#,
                n.uuid.as_deref().unwrap()
            );
            assert_eq!(from, want.as_bytes());
            assert!(json::raw_value(raw, "sessionKind").unwrap().is_none());
        }
    }
    // Bookkeeping of the parent does not go along, as with /branch.
    assert_eq!(
        types(&fork.rows).unwrap(),
        [
            "assistant",
            "content-replacement",
            "last-prompt",
            "tools-used",
            "user"
        ]
    );

    // The prefix proof: the projected fork loads the same messages, byte
    // for byte and in order, and the same state as the parent at the fork
    // point and as a --fork-session copy.
    let at_parent = load(&rows).unwrap();
    let projected = load(&fork.rows).unwrap();
    let native = load(&native_fork_session(
        &rows,
        &["u1", "a1", "r1", "a2", "u3", "a4"],
        "11111111-2222-4333-8444-555555555555",
    ))
    .unwrap();
    assert_eq!(projected, at_parent);
    assert_eq!(projected, native);
    assert_eq!(projected.path[1].message, A1.as_bytes());

    // The store follows the file, and later appends by Claude Code are
    // appends to the fork session.
    let f = st.session(&f.id).unwrap();
    assert_eq!(f.binding.native_id.as_deref(), Some(p.native_id.as_str()));
    assert_eq!(f.fork_of.as_ref().unwrap().node, head);
    let mut more = fork.rows.clone();
    more.push(chain("user", "u9", Some("a4"), U2, &p.native_id).into_bytes());
    std::fs::write(&p.path, bytes(&more)).unwrap();
    let r2 = ingest(&st, &p.path, VERSION, None).unwrap();
    assert_eq!(
        (r2.session.as_str(), r2.added, r2.rewritten),
        (f.id.as_str(), 1, false)
    );

    // A fork at a chain row cuts there; a fork of a fork needs no parent file.
    let at = st.resolve_node(&s, "uuid:a2").unwrap();
    let g = st.fork(&r.session, &at, None).unwrap();
    let gp = project(&st, &g.id, &cfg).unwrap();
    let g_rows = read_file(&gp.path).unwrap().rows;
    let gl = load(&g_rows).unwrap();
    assert_eq!(gl.leaf, "a2");
    assert_eq!(gl.path.len(), 4);
    // Only the state rows before the fork point go along.
    assert_eq!(
        types(&g_rows).unwrap(),
        ["assistant", "content-replacement", "last-prompt", "user"]
    );
}

#[test]
fn opaque_blocks_stay_byte_exact_in_a_fork() {
    let t = tempfile::tempdir().unwrap();
    let rows = parent_rows();
    let file = setup(t.path(), &rows);
    let st = store(t.path());
    let r = ingest(&st, &file, VERSION, None).unwrap();
    let s = st.session(&r.session).unwrap();
    let at = st.resolve_node(&s, "uuid:a1").unwrap();
    let f = st.fork(&r.session, &at, None).unwrap();
    let p = project(&st, &f.id, &t.path().join("c")).unwrap();
    let out = std::fs::read(&p.path).unwrap();
    let hay = String::from_utf8(out).unwrap();
    assert!(hay.contains(A1));
    assert!(
        hay.contains(r#"I need the file. \u00e9t\u00e9","signature":"EqQBCkYIBxgCKkB+/sig//one=="#)
    );
    assert!(hay.contains(r#""speed":1.50"#));
}

#[test]
fn usage_counts_each_response_once() {
    let mut rows = parent_rows();
    // A response split over two rows that share message.id.
    rows.insert(2, chain("assistant", "a1b", Some("a1"), A1, P).into_bytes());
    let u = usage(&rows);
    assert_eq!(u.requests, 4);
    assert_eq!(u.cache_read, 6000 + 6060 + 6060);
    assert_eq!(u.cache_creation, 6000 + 60 + 20 + 25);
    assert_eq!(u.output, 40 + 12 + 3 + 6);
    let v = view(rows[1].as_slice()).unwrap();
    assert_eq!(
        (v.role.as_str(), v.text.as_str()),
        ("assistant", "[thinking] [call Read]")
    );
}
