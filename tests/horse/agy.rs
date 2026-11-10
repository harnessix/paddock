//! The hidden agy commands on the command line: ingest, a fork and a
//! projection. A synthetic conversation database only.

use super::*;

const ID: &str = "5c1d3b2f-8e4a-4b6c-8d7e-1f2a3b4c5d6e";

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A step payload: field 1 the type, field 4 the status, then the body.
fn step(ty: u64, field: u64, text: &str) -> Vec<u8> {
    let mut p = Vec::new();
    varint(&mut p, 1 << 3);
    varint(&mut p, ty);
    varint(&mut p, 4 << 3);
    varint(&mut p, 3);
    let mut body = Vec::new();
    varint(&mut body, 1 << 3 | 2);
    varint(&mut body, text.len() as u64);
    body.extend_from_slice(text.as_bytes());
    varint(&mut p, field << 3 | 2);
    varint(&mut p, body.len() as u64);
    p.extend_from_slice(&body);
    p
}

/// An agy home with one conversation of two turns.
fn agy_home(home: &Path) -> std::path::PathBuf {
    let path = home.join("conversations").join(format!("{ID}.db"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(
        r#"
CREATE TABLE trajectory_meta (trajectory_id text, cascade_id text, trajectory_type integer, source integer, PRIMARY KEY (trajectory_id));
CREATE TABLE steps (idx integer, step_type integer NOT NULL DEFAULT 0, status integer NOT NULL DEFAULT 0, has_subtrajectory numeric NOT NULL DEFAULT false, metadata blob, error_details blob, permissions blob, task_details blob, render_info blob, step_payload blob, step_format integer NOT NULL DEFAULT 0, PRIMARY KEY (idx));
CREATE INDEX idx_steps_status ON steps(status);
CREATE INDEX idx_steps_step_type ON steps(step_type);
CREATE TABLE gen_metadata (idx integer, data blob, size integer NOT NULL DEFAULT 0, PRIMARY KEY (idx));
CREATE TABLE executor_metadata (idx integer, data blob, PRIMARY KEY (idx));
CREATE TABLE parent_references (idx integer, data blob, PRIMARY KEY (idx));
CREATE TABLE trajectory_metadata_blob (id text DEFAULT "main", data blob, PRIMARY KEY (id));
CREATE TABLE battle_mode_infos (idx integer, data blob, PRIMARY KEY (idx));
"#,
    )
    .unwrap();
    c.execute(
        "INSERT INTO trajectory_meta VALUES ('0f1e2d3c-4b5a-4987-8a6b-5c4d3e2f1a0b', ?1, 4, 17)",
        [ID],
    )
    .unwrap();
    let rows = [
        (14, 19, "first"),
        (15, 20, "one"),
        (14, 19, "second"),
        (15, 20, "two"),
    ];
    for (i, (ty, field, text)) in rows.iter().enumerate() {
        c.execute(
            "INSERT INTO steps (idx, step_type, status, step_payload) VALUES (?1, ?2, 3, ?3)",
            rusqlite::params![i as i64, ty, step(*ty, *field, text)],
        )
        .unwrap();
    }
    path
}

#[test]
fn ingest_fork_and_project_an_agy_conversation_on_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let home = t.join("agy-home");
    let db = agy_home(&home);
    let ingest = |v: &str| {
        cmd(t, &["ingest-agy", db.to_str().unwrap(), "--agy-version", v])
            .output()
            .unwrap()
    };
    // Only the pinned version reads the database.
    let o = ingest("1.4.11");
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("1.4.12"),
        "{o:?}"
    );
    let o = ingest("1.4.12");
    assert!(o.status.success(), "{o:?}");
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(out.contains(": 4 new, 0 kept"), "{out}");
    let sid = out.split(':').next().unwrap().to_string();

    // The session projects under its id into another agy home.
    let copy = t.join("copy-home");
    let p = paddock(
        t,
        &["project-agy", &sid, "--agy-home", copy.to_str().unwrap()],
    );
    let (id, path) = p.trim().split_once(' ').unwrap();
    assert_eq!(id, ID);
    assert!(Path::new(path).is_file());
    assert!(copy.join("conversation_summaries.db").is_file());

    // A fork at the end of the first turn is a new conversation.
    let log = paddock(t, &["log", &sid]);
    assert!(log.contains("step planner_response, done"), "{log}");
    let fork = paddock(t, &["fork", &sid, "--at", "steps:1"])
        .trim()
        .to_string();
    let p = paddock(
        t,
        &["project-agy", &fork, "--agy-home", home.to_str().unwrap()],
    );
    assert!(p.contains("cold cache"), "{p}");
    let (fid, fpath) = p.lines().next().unwrap().split_once(' ').unwrap();
    assert_ne!(fid, ID);
    let c = rusqlite::Connection::open(fpath).unwrap();
    let idx: Vec<i64> = c
        .prepare("SELECT idx FROM steps ORDER BY idx")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(idx, [0, 1]);
    let s = rusqlite::Connection::open(home.join("conversation_summaries.db")).unwrap();
    let count: i64 = s
        .query_row(
            "SELECT step_count FROM conversation_summaries WHERE conversation_id = ?1",
            [fid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}
