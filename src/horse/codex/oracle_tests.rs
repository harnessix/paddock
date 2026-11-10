//! Round trips against records serialized by each supported upstream tag.
use super::*;
use std::io::Write;

const ID: &str = "01900000-0000-7000-8000-000000000001";
const CORPORA: [(&str, &[u8]); 2] = [
    (
        "0.166.0",
        include_bytes!("../../../tests/fixtures/horse/codex-rollouts/0.166.0.jsonl"),
    ),
    (
        "0.167.0",
        include_bytes!("../../../tests/fixtures/horse/codex-rollouts/0.167.0.jsonl"),
    ),
];

#[test]
fn upstream_corpora_round_trip_and_resume_after_a_torn_tail() {
    for (version, corpus) in CORPORA {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!(
            "input/sessions/2026/01/01/rollout-2026-01-01T00-00-00-{ID}.jsonl"
        ));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, corpus).unwrap();
        let store = Store::open(&dir.path().join("store")).unwrap();
        let first = ingest(&store, &path, version, None).unwrap();
        let projection = project(&store, &first.session, &dir.path().join("out"), false).unwrap();
        assert_eq!(
            std::fs::read(&projection.path).unwrap(),
            corpus,
            "{version}"
        );

        let extra = b"{\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n";
        let split = extra.len() / 2;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&extra[..split]).unwrap();
        let torn = ingest(&store, &path, version, None).unwrap();
        assert!(torn.torn);
        assert_eq!(torn.added, 0);
        file.write_all(&extra[split..]).unwrap();
        let replay = ingest(&store, &path, version, None).unwrap();
        assert_eq!((replay.added, replay.kept), (1, first.added));
        assert_eq!(ingest(&store, &path, version, None).unwrap().added, 0);
        let again = project(&store, &first.session, &dir.path().join("out2"), false).unwrap();
        assert_eq!(
            std::fs::read(again.path).unwrap(),
            std::fs::read(&path).unwrap()
        );
    }
}

#[test]
fn producer_and_reader_versions_are_recorded_separately() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!(
        "input/sessions/2026/01/01/rollout-2026-01-01T00-00-00-{ID}.jsonl"
    ));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, CORPORA[0].1).unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let first = ingest(&store, &path, "0.167.0", None).unwrap();
    let session = store.session(&first.session).unwrap();
    assert_eq!(session.binding.version, "0.166.0");
    assert_eq!(session.codex_reader_version.as_deref(), Some("0.167.0"));
    let nodes = store.session_nodes(&session).unwrap();
    assert!(nodes
        .iter()
        .all(|n| n.meta.version.as_deref() == Some("0.167.0")));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"timestamp\":\"t\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n").unwrap();
    let next = ingest(&store, &path, "0.166.0", None).unwrap();
    assert_eq!(next.added, 1);
    let session = store.session(&first.session).unwrap();
    assert_eq!(session.binding.version, "0.166.0");
    assert_eq!(session.codex_reader_version.as_deref(), Some("0.166.0"));
    let nodes = store.session_nodes(&session).unwrap();
    assert_eq!(
        nodes.last().unwrap().meta.version.as_deref(),
        Some("0.166.0")
    );
    assert_eq!(
        nodes.first().unwrap().meta.version.as_deref(),
        Some("0.167.0")
    );
}
