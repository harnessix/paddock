use super::fixture::{Fixture, CWD, ID};
use super::*;
use crate::horse::guard::{self, Op};

fn rows(path: &Path) -> Vec<Vec<u8>> {
    read_lines(path).unwrap().0
}

/// The session of the round trip: reasoning, a tool call, an interjection
/// and a rewind marker.
fn rich() -> Fixture {
    let mut f = Fixture::two_turns();
    f.rewind(1);
    f.prompt(1, "Fix the warning another way").reply("Done.");
    f
}

#[test]
fn round_trip_is_byte_exact_and_the_request_body_is_the_same() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let dir = rich().write(&t.path().join("in"));
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(!r.torn && !r.rewritten);
    let out = project(&st, &r.session, &t.path().join("out"), false).unwrap();
    assert_eq!(out.native_id, ID);
    assert!(!out.forked);
    for f in [UPDATES_FILE, CHAT_FILE, SUMMARY_FILE] {
        assert_eq!(
            std::fs::read(dir.join(f)).unwrap(),
            std::fs::read(out.path.join(f)).unwrap(),
            "{f}"
        );
    }
    assert_eq!(
        request::body(&rows(&dir.join(CHAT_FILE))),
        request::body(&rows(&out.path.join(CHAT_FILE)))
    );
    // A second ingest adds nothing, and a projection never overwrites.
    let again = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!((again.session, again.added), (r.session.clone(), 0));
    assert!(project(&st, &r.session, &t.path().join("out"), false).is_err());
}

#[test]
fn the_keys_are_the_event_id_and_the_chat_content() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    // A row without an event id has its line as the key.
    f.updates
        .push(r#"{"timestamp":1,"method":"session/update","params":{"update":{}}}"#.into());
    let dir = f.write(t.path());
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    let nodes = st.session_nodes(&st.session(&r.session).unwrap()).unwrap();
    let keys: Vec<&str> = nodes.iter().filter_map(|n| n.meta.key.as_deref()).collect();
    assert!(keys.contains(&format!("event:{ID}-1").as_str()));
    assert!(keys.contains(&format!("line:{}", f.updates.len() - 1).as_str()));
    assert!(keys.iter().any(|k| k.starts_with("chat:1:")));
    // The system prompt and the first prompt come before the first update
    // of turn 1; the chat items of turn 0 come after the updates of turn 0.
    let first_turn1 = keys.iter().position(|k| k.starts_with("chat:1:")).unwrap();
    let turn1_update = nodes
        .iter()
        .position(|n| String::from_utf8_lossy(&n.raw).contains("Now fix the warning"))
        .unwrap();
    assert!(turn1_update < first_turn1);
    assert!(keys[..turn1_update]
        .iter()
        .any(|k| k.starts_with("chat:0:")));
}

#[test]
fn torn_lines_truncation_and_a_chat_rewrite_lose_and_duplicate_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    let dir = f.write(t.path());
    let first = ingest(&st, &dir, VERSION, None).unwrap();
    let s = |st: &Store| st.session(&first.session).unwrap();
    let before = st.session_nodes(&s(&st)).unwrap();

    // A torn last line stays pending.
    let full = std::fs::read_to_string(dir.join(UPDATES_FILE)).unwrap();
    f.prompt(2, "One more");
    let line = f.updates.last().unwrap().clone();
    std::fs::write(
        dir.join(UPDATES_FILE),
        format!("{full}{}", &line[..line.len() / 2]),
    )
    .unwrap();
    let torn = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(torn.torn);
    assert_eq!(torn.added, 0);
    std::fs::write(dir.join(UPDATES_FILE), format!("{full}{line}\n")).unwrap();
    assert_eq!(ingest(&st, &dir, VERSION, None).unwrap().added, 1);

    // A truncated log keeps every node.
    let cut: Vec<&str> = full.lines().take(3).collect();
    std::fs::write(dir.join(UPDATES_FILE), cut.join("\n") + "\n").unwrap();
    let short = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!(short.added, 0);
    assert!(short.missing > 0);
    std::fs::write(dir.join(UPDATES_FILE), format!("{full}{line}\n")).unwrap();
    // A row that changed is refused.
    std::fs::write(dir.join(UPDATES_FILE), full.replacen("tests", "TESTS", 1)).unwrap();
    assert!(ingest(&st, &dir, VERSION, None).is_err());
    std::fs::write(dir.join(UPDATES_FILE), format!("{full}{line}\n")).unwrap();

    // A compaction rewrites chat_history.jsonl by a rename.
    f.compact("Summary: the tests pass and the warning is fixed.");
    f.write(t.path());
    let w = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(w.rewritten);
    assert_eq!(w.added, 2, "the index node and the summary item");
    let after = st.session_nodes(&s(&st)).unwrap();
    for n in &before {
        assert!(after.iter().any(|a| a.hash == n.hash), "node lost");
    }
    let chat_keys: Vec<_> = after
        .iter()
        .filter(|n| n.meta.container.as_deref() == Some(CHAT) && !replay::is_rewrite(n))
        .map(|n| n.meta.key.clone())
        .collect();
    let unique: HashSet<_> = chat_keys.iter().collect();
    assert_eq!(unique.len(), chat_keys.len(), "node duplicated");
    // The projection holds the rewritten history, and a later turn appends.
    let files = replay::replay(&after).unwrap();
    assert_eq!(files.chat, rows(&dir.join(CHAT_FILE)));
    f.prompt(3, "Go on").reply("Gone on.");
    f.write(t.path());
    let next = ingest(&st, &dir, VERSION, None).unwrap();
    assert!(!next.rewritten);
    let files = replay::replay(&st.session_nodes(&s(&st)).unwrap()).unwrap();
    assert_eq!(files.chat, rows(&dir.join(CHAT_FILE)));
    assert_eq!(files.updates, rows(&dir.join(UPDATES_FILE)));
}

#[test]
fn a_fork_at_a_turn_projects_as_a_native_fork() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let f = Fixture::two_turns();
    let dir = f.write(&t.path().join("home"));
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    let at = turn_end(&st, &r.session, 0).unwrap();
    let v = guard::check(&st, &r.session, &Op::Fork { at: at.clone() }, 0).unwrap();
    assert!(!v.keeps && v.reasons[0].contains("starts cold"));
    let fork = st.fork(&r.session, &at, None).unwrap();
    let p = project(&st, &fork.id, &t.path().join("home"), false).unwrap();
    assert!(p.forked);
    assert_ne!(p.native_id, ID);
    assert_eq!(p.path.parent(), dir.parent());

    let summary: Value =
        serde_json::from_slice(&std::fs::read(p.path.join(SUMMARY_FILE)).unwrap()).unwrap();
    assert_eq!(summary["info"]["id"], p.native_id.as_str());
    assert_eq!(summary["info"]["cwd"], CWD);
    assert_eq!(summary["parent_session_id"], ID);
    assert_eq!(summary["session_kind"], "fork");
    assert!(summary["forked_at"].is_string());
    assert!(summary.get("last_turn_summary").is_none());
    assert_eq!(summary["chat_format_version"], 1);

    // The updates of turn 0 under the new id; every other byte stays.
    let native = rows(&dir.join(UPDATES_FILE));
    let opens = replay::update_opens(&native).unwrap();
    let end = opens.iter().position(|o| *o == Some(1)).unwrap();
    let expect: Vec<Vec<u8>> = native[..end]
        .iter()
        .map(|r| {
            String::from_utf8(r.clone())
                .unwrap()
                .replace(
                    &format!("\"sessionId\":\"{ID}\""),
                    &format!("\"sessionId\":\"{}\"", p.native_id),
                )
                .into_bytes()
        })
        .collect();
    assert_eq!(rows(&p.path.join(UPDATES_FILE)), expect);
    assert_eq!(summary["num_messages"], expect.len());

    // The request body proof: the projected fork gives the model what a
    // native fork at prompt 0 gives it.
    let chat = rows(&dir.join(CHAT_FILE));
    let keep = request::cut(&chat, 0).unwrap();
    assert!(keep > 1 && keep < chat.len());
    assert_eq!(
        request::body(&rows(&p.path.join(CHAT_FILE))),
        request::body(&chat[..keep])
    );
    // The fork is a session of its own with the new id.
    let again = ingest(&st, &p.path, VERSION, None).unwrap();
    assert_eq!((again.session, again.added), (fork.id, 0));
}

#[test]
fn a_swap_keeps_the_session_id_and_the_prefix() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let dir = Fixture::two_turns().write(&home);
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    let s = st.session(&r.session).unwrap();
    let at = turn_end(&st, &r.session, 0).unwrap();
    let to = swap::resolve_branch(&st, &s, &at.as_str()[..12]).unwrap();
    let v = guard::check(&st, &r.session, &Op::Swap { to: to.clone() }, 0).unwrap();
    assert!(v.keeps, "{}", v.message());
    assert!(v.notes[0].contains("keeps its id"));
    let old = std::fs::read(dir.join(UPDATES_FILE)).unwrap();

    // Grok holds its summary lock: nothing changes.
    let lock = std::fs::File::create(dir.join("summary.json.lock")).unwrap();
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock on a descriptor that this test owns.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(swap::swap_file(&st, &r.session, &to, &home).is_err());
    assert_eq!(st.session(&r.session).unwrap().head, s.head);
    // SAFETY: as above.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    drop(lock);

    let sw = swap::swap_file(&st, &r.session, &to, &home).unwrap();
    assert_eq!(sw.native, ID);
    assert_eq!(sw.path, dir);
    assert_eq!(
        std::fs::read(sw.backup.unwrap().join(UPDATES_FILE)).unwrap(),
        old
    );
    let s2 = st.session(&r.session).unwrap();
    assert_eq!(s2.head.as_ref(), Some(&to));
    assert_eq!(s2.binding.native_id.as_deref(), Some(ID));
    let kept = st.session(sw.kept.as_ref().unwrap()).unwrap();
    assert_eq!(kept.head, s.head);
    // The folder holds the prefix up to the fork point, byte for byte.
    let branch = replay::replay(&st.chain(Some(&to)).unwrap()).unwrap();
    assert_eq!(rows(&dir.join(UPDATES_FILE)), branch.updates);
    assert_eq!(rows(&dir.join(CHAT_FILE)), branch.chat);
    assert!(old.starts_with(&replay::jsonl(&branch.updates)));
    // The next ingest of the folder finds the same session and adds nothing.
    let again = ingest(&st, &dir, VERSION, None).unwrap();
    assert_eq!((again.session, again.added), (r.session.clone(), 0));
    // A swap back to the old head.
    let back = swap::swap_file(&st, &r.session, s.head.as_ref().unwrap(), &home).unwrap();
    assert!(
        back.kept.is_some(),
        "the branch head stays in a session of its own"
    );
    assert_eq!(std::fs::read(dir.join(UPDATES_FILE)).unwrap(), old);
}

#[test]
fn an_unpinned_version_refuses_ingest_and_projection() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let f = Fixture::two_turns();
    let dir = f.write(t.path());
    assert!(ingest(&st, &dir, "1.0.69", None).is_err());
    assert!(check_version("1.0.71").is_err());
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    st.update_session(&r.session, |s| s.binding.version = "1.0.69".into())
        .unwrap();
    assert!(project(&st, &r.session, &t.path().join("out"), false).is_err());
    assert!(ingest(&st, &dir, VERSION, None).is_err());
    // Another chat format refuses too.
    let t2 = tempfile::tempdir().unwrap();
    let dir = f.write(t2.path());
    let s = f
        .summary()
        .replace("\"chat_format_version\": 1", "\"chat_format_version\": 2");
    std::fs::write(dir.join(SUMMARY_FILE), s).unwrap();
    let st2 = Store::open(&t2.path().join("store")).unwrap();
    let e = ingest(&st2, &dir, VERSION, None).unwrap_err();
    assert!(format!("{e:#}").contains("chat_format_version 2"));
}

#[test]
fn the_working_directory_is_encoded_as_grok_does() {
    assert_eq!(
        encode_cwd("/home/demo/my project"),
        "%2Fhome%2Fdemo%2Fmy%20project"
    );
    assert_eq!(
        encode_cwd("/home/demo/a-b_c.d~e"),
        "%2Fhome%2Fdemo%2Fa-b_c.d~e"
    );
    let long = format!("/home/demo/{}/Mein Projekt", "\u{4e2d}".repeat(30));
    let enc = encode_cwd(&long);
    let (slug, hash) = enc.rsplit_once('-').unwrap();
    assert_eq!(slug, "mein-projekt");
    assert_eq!(hash.len(), 16);
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(hash, &blake3::hash(long.as_bytes()).to_hex().as_str()[..16]);
    // A session in a long working directory round-trips, with the .cwd file.
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::new(ID, &long);
    f.prompt(0, "hi").reply("hello");
    let dir = f.write(&t.path().join("in"));
    let r = ingest(&st, &dir, VERSION, None).unwrap();
    let p = project(&st, &r.session, &t.path().join("out"), false).unwrap();
    assert_eq!(
        std::fs::read_to_string(p.path.parent().unwrap().join(".cwd")).unwrap(),
        long
    );
    // A folder in the wrong place is refused.
    let wrong = t.path().join("x/sessions/elsewhere").join(ID);
    std::fs::create_dir_all(&wrong).unwrap();
    std::fs::write(wrong.join(SUMMARY_FILE), f.summary()).unwrap();
    assert!(ingest(&st, &wrong, VERSION, None).is_err());
}
