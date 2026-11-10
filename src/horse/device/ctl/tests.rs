use super::grant::{self, Grant, COMMANDS};
use super::*;
use crate::horse::device::{Entry, Tree};
use crate::horse::live::fake;
use crate::horse::{dsh, Store};
use std::sync::Mutex;

/// An approval stub: a fixed answer, the cards it got and the ledger.
pub(super) struct Stub {
    answer: Answer,
    asked: Mutex<Vec<(String, String)>>,
    lines: Mutex<Vec<String>>,
}

impl Host for Stub {
    fn state_dir(&self) -> std::path::PathBuf {
        "/home/demo/.local/state/paddock".into()
    }
    fn ask(&self, summary: &str, detail: &str) -> Answer {
        self.asked
            .lock()
            .unwrap()
            .push((summary.into(), detail.into()));
        self.answer
    }
    fn record(&self, line: &str) {
        self.lines.lock().unwrap().push(line.into());
    }
}

pub(super) fn stub(answer: Answer) -> Stub {
    Stub {
        answer,
        asked: Mutex::default(),
        lines: Mutex::default(),
    }
}

impl Stub {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
    pub(super) fn cards(&self) -> Vec<(String, String)> {
        self.asked.lock().unwrap().clone()
    }
}

const SBX: &str = "sbx-ctl-test";

struct World {
    _tmp: tempfile::TempDir,
    store: Store,
    access: Access,
    /// A session of another horse.
    other: String,
    other_horse: String,
}

/// Two dsh turns whose times are a minute old, so the cache is warm.
fn fresh_rows() -> Vec<Vec<u8>> {
    let mut rows = fake::turn_rows(0, 1, "hello", "m1");
    rows.extend(fake::turn_rows(rows.len(), 2, "second", "m2"));
    let fresh = crate::horse::now_ms() - 60_000;
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let old = format!("\"time\":{}", fake::T0 + i as u64 * 100);
            let new = format!("\"time\":{}", fresh + i as u64 * 100);
            String::from_utf8(r.clone())
                .unwrap()
                .replace(&old, &new)
                .into_bytes()
        })
        .collect()
}

/// A dsh session `id` with two fresh turns in `horse`.
pub(crate) fn ingest(store: &Store, root: &std::path::Path, id: &str, horse: &str) -> String {
    fake::write_file(root, id, &fresh_rows());
    let path = dsh::session_dir(root, Some(fake::CWD), id);
    dsh::ingest(store, &path, dsh::VERSION, Some(horse))
        .unwrap()
        .session
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("store")).unwrap();
    let root = tmp.path().join("sessions");
    let a = store.new_default_horse().unwrap();
    let b = store.new_default_horse().unwrap();
    let own = ingest(&store, &root, "session-a", &a.id);
    let other = ingest(&store, &root, "session-b", &b.id);
    World {
        access: Access {
            store: store.clone(),
            horse: a.id,
            session: own,
        },
        _tmp: tmp,
        store,
        other,
        other_horse: b.id,
    }
}

/// A store with one horse and one dsh session in it: the temp folder,
/// the store, the horse and the session.
pub(super) fn world_parts() -> (tempfile::TempDir, Store, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let s = ingest(&store, &tmp.path().join("sessions"), "session-p", &h.id);
    (tmp, store, h.id, s)
}

/// A grant of `commands` for `sandbox`, with no move targets.
pub(super) fn give_to(store: &Store, sandbox: &str, commands: &[&str], budget: u64, tokens: u64) {
    grant::set(
        store,
        &Grant {
            sandbox: sandbox.into(),
            commands: commands.iter().map(|c| c.to_string()).collect(),
            horses: Vec::new(),
            budget,
            tokens,
            used: 0,
            spent_tokens: 0,
            granted_ms: 1,
        },
    )
    .unwrap();
}

fn give(w: &World, commands: &[&str], budget: u64, tokens: u64) {
    grant::set(
        &w.store,
        &Grant {
            sandbox: SBX.into(),
            commands: commands.iter().map(|c| c.to_string()).collect(),
            horses: if commands.contains(&"move") {
                vec![w.other_horse.clone()]
            } else {
                Vec::new()
            },
            budget,
            tokens,
            used: 0,
            spent_tokens: 0,
            granted_ms: 1,
        },
    )
    .unwrap();
}

fn sessions(w: &World) -> Vec<String> {
    w.store.horse(&w.access.horse).unwrap().sessions
}

fn tree(w: &World) -> Tree {
    w.access.tree().unwrap()
}

#[test]
fn commands_parse_one_line_each() {
    assert_eq!(
        Command::parse("fork s-1\n").unwrap(),
        Command::Fork {
            session: "s-1".into(),
            at: None
        }
    );
    assert_eq!(
        Command::parse("append s-1 note two  words here").unwrap(),
        Command::Append {
            session: "s-1".into(),
            role: "note".into(),
            text: "two  words here".into()
        }
    );
    assert_eq!(
        Command::parse("view s-1 mine role:user").unwrap(),
        Command::View {
            session: "s-1".into(),
            name: "mine".into(),
            filter: view::Filter::Role("user".into())
        }
    );
    assert!(Command::parse("fork s-1\nfork s-2").is_err());
    assert!(Command::parse("fork").is_err());
    assert!(Command::parse("append s-1 user").is_err());
    assert!(Command::parse("move s-1").is_err());
    assert!(Command::parse("view s-1 v sometimes").is_err());
    assert!(Command::parse("rewrite s-1").is_err());
    assert!(Command::parse("").is_err());
}

#[test]
fn an_ungranted_sandbox_is_refused_with_one_ledger_line() {
    let w = world();
    let host = stub(Answer::Approved);
    let before = sessions(&w);
    let f = run(&w.access, SBX, &format!("fork {}", w.access.session), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
    assert!(f.message.contains("no ctl grant"), "{}", f.message);
    assert_eq!(host.lines(), ["ctl fork refused no grant"]);
    assert!(host.cards().is_empty());
    assert_eq!(sessions(&w), before);
}

#[test]
fn a_granted_fork_makes_a_session_in_the_same_horse() {
    let w = world();
    give(&w, &["fork"], 5, 0);
    let host = stub(Answer::Denied);
    let own = w.access.session.clone();
    let out = run(&w.access, SBX, &format!("fork {own}"), &host).unwrap();
    let id = out.trim();
    let f = w.store.session(id).unwrap();
    assert_eq!(f.horse, w.access.horse);
    assert_eq!(f.fork_of.unwrap().session, own);
    assert_eq!(sessions(&w).len(), 2);
    // A fork at the head keeps the warm prefix: no card.
    assert!(host.cards().is_empty());
    assert_eq!(host.lines(), [format!("ctl fork {own} passed")]);
    let g = grant::get(&w.store, SBX).unwrap().unwrap();
    assert_eq!((g.used, g.spent_tokens), (1, 0));
    let events = crate::horse::run::events(&w.store, &w.access.horse).unwrap();
    assert!(events
        .iter()
        .any(|e| e["type"] == "ctl.fork" && e["session"] == own.as_str() && e["sandbox"] == SBX));
    // The fork is not granted for append.
    let f = run(&w.access, SBX, &format!("append {own} user hi"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
    assert_eq!(
        host.lines().last().unwrap(),
        "ctl append refused not granted"
    );
}

#[test]
fn a_session_of_another_horse_is_refused_for_every_grant() {
    let w = world();
    give(&w, &COMMANDS, 100, u64::MAX);
    let host = stub(Answer::Approved);
    let o = &w.other;
    let lines = [
        format!("fork {o}"),
        format!("append {o} user hi"),
        format!("move {o} {}", w.other_horse),
        format!("view {o} v all"),
        format!("disconnect {o}"),
        "fork s-does-not-exist".to_string(),
    ];
    for line in &lines {
        let f = run(&w.access, SBX, line, &host).unwrap_err();
        assert_eq!(f.kind, Kind::Refused, "{line}");
        assert!(f.message.contains("not a session of this horse"), "{line}");
        assert!(!f.message.contains(&w.other_horse), "{line}");
    }
    for l in host.lines() {
        assert!(l.ends_with("refused not this horse"), "{l}");
        assert!(!l.contains(o.as_str()), "{l}");
    }
    assert!(host.cards().is_empty());
    assert!(w.store.inbox(o).unwrap().is_empty());
    assert_eq!(
        w.store.horse(&w.other_horse).unwrap().sessions,
        std::slice::from_ref(o)
    );
    assert_eq!(grant::get(&w.store, SBX).unwrap().unwrap().used, 0);
}

#[test]
fn the_budget_runs_out_and_the_next_command_fails() {
    let w = world();
    give(&w, &["append"], 2, 0);
    let host = stub(Answer::Approved);
    let own = &w.access.session;
    for _ in 0..2 {
        run(&w.access, SBX, &format!("append {own} note ok"), &host).unwrap();
    }
    let f = run(&w.access, SBX, &format!("append {own} note late"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
    assert!(f.message.contains("used up"), "{}", f.message);
    assert_eq!(w.store.inbox(own).unwrap().len(), 2);
    assert_eq!(host.lines().last().unwrap(), "ctl append refused budget");
}

#[test]
fn a_pure_append_passes_with_no_card() {
    let w = world();
    give(&w, &["append"], 5, 0);
    let host = stub(Answer::Denied);
    let own = &w.access.session;
    let out = run(
        &w.access,
        SBX,
        &format!("append {own} system be brief"),
        &host,
    )
    .unwrap();
    let inbox = w.store.inbox(own).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(out.trim(), inbox[0].id);
    assert_eq!(
        (inbox[0].role.as_str(), inbox[0].text.as_str()),
        ("system", "be brief")
    );
    assert!(host.cards().is_empty());
    assert_eq!(host.lines(), [format!("ctl append {own} passed")]);
    let f = run(&w.access, SBX, &format!("append {own} boss hi"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Invalid);
}

/// A fork at the user message of turn 2: the cut is not at the end of a
/// reply, so the cache holds less than the fork sends.
pub(crate) const MID: &str = "seq:12";

#[test]
fn a_cache_breaking_fork_asks_with_the_cost_and_a_denial_writes_nothing() {
    let w = world();
    give(&w, &["fork"], 5, 1_000_000);
    let host = stub(Answer::Denied);
    let own = &w.access.session;
    let before = sessions(&w);
    let guard_before = w.store.guard_state(own).unwrap();
    let f = run(&w.access, SBX, &format!("fork {own} {MID}"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Denied);
    assert!(f.message.contains("nothing changed"), "{}", f.message);
    assert!(f.message.contains("instead:"), "{}", f.message);
    let cards = host.cards();
    assert_eq!(cards.len(), 1);
    let (summary, detail) = &cards[0];
    assert!(summary.contains("loses about"), "{summary}");
    assert!(summary.contains("tokens of cached prefix"), "{summary}");
    assert!(detail.contains("prefix guard:"), "{detail}");
    assert!(detail.contains("instead:"), "{detail}");
    assert_eq!(sessions(&w), before);
    assert_eq!(w.store.guard_state(own).unwrap(), guard_before);
    assert_eq!(host.lines(), [format!("ctl fork {own} denied")]);
    assert_eq!(grant::get(&w.store, SBX).unwrap().unwrap().used, 0);

    // Approved, the fork runs and its tokens come from the budget.
    let host = stub(Answer::Approved);
    let id = run(&w.access, SBX, &format!("fork {own} {MID}"), &host).unwrap();
    assert!(w.store.session(id.trim()).is_ok());
    let g = grant::get(&w.store, SBX).unwrap().unwrap();
    assert_eq!(g.used, 1);
    assert!(g.spent_tokens > 0);
    assert_eq!(
        host.lines(),
        [format!(
            "ctl fork {own} approved lost {} tokens",
            g.spent_tokens
        )]
    );
}

#[test]
fn a_break_over_the_token_budget_is_refused_before_the_card() {
    let w = world();
    give(&w, &["fork"], 5, 0);
    let host = stub(Answer::Approved);
    let own = &w.access.session;
    let f = run(&w.access, SBX, &format!("fork {own} {MID}"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
    assert!(f.message.contains("0 tokens left"), "{}", f.message);
    assert!(host.cards().is_empty());
    assert_eq!(sessions(&w).len(), 1);
    assert_eq!(host.lines(), ["ctl fork refused token budget"]);
}

#[test]
fn a_timed_out_card_writes_nothing() {
    let w = world();
    give(&w, &["fork"], 5, 1_000_000);
    let host = stub(Answer::TimedOut(7));
    let own = &w.access.session;
    let f = run(&w.access, SBX, &format!("fork {own} {MID}"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::TimedOut);
    assert!(f.message.contains("7 seconds"), "{}", f.message);
    assert_eq!(sessions(&w).len(), 1);
    assert_eq!(host.lines(), [format!("ctl fork {own} timed out")]);
}

#[test]
fn a_view_shows_the_filtered_messages_read_only() {
    let w = world();
    give(&w, &["view"], 5, 0);
    let host = stub(Answer::Denied);
    let own = &w.access.session;
    let out = run(&w.access, SBX, &format!("view {own} asks role:user"), &host).unwrap();
    assert_eq!(out, "self/views/asks\n");
    run(&w.access, SBX, &format!("view {own} last last:1"), &host).unwrap();
    let t = w.access.plain_tree().unwrap();
    assert_ne!(
        tree(&w).paths["self/views/asks"],
        t.paths["self/views/asks"]
    );
    let Entry::File(asks) = &t.paths["self/views/asks"] else {
        panic!("no view file")
    };
    let asks = String::from_utf8(asks.clone()).unwrap();
    assert!(asks.contains("user hello"), "{asks}");
    assert!(asks.contains("user second"), "{asks}");
    assert!(!asks.contains("assistant"), "{asks}");
    let Entry::File(last) = &t.paths["self/views/last"] else {
        panic!("no view file")
    };
    let last = String::from_utf8(last.clone()).unwrap();
    assert_eq!(last.lines().count(), 1);
    assert!(last.contains("echo: second"), "{last}");
    assert!(!crate::horse::device::writable("self/views/asks"));
    let f = run(&w.access, SBX, &format!("view {own} ../x all"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Invalid);
}

#[test]
fn a_move_goes_only_to_a_granted_horse() {
    let w = world();
    give(&w, &["move", "fork"], 5, 0);
    let host = stub(Answer::Denied);
    let third = w.store.new_default_horse().unwrap();
    let own = w.access.session.clone();
    let f = run(&w.access, SBX, &format!("move {own} {}", third.id), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
    assert_eq!(host.lines(), ["ctl move refused horse not granted"]);
    let out = run(
        &w.access,
        SBX,
        &format!("move {own} {}", w.other_horse),
        &host,
    )
    .unwrap();
    assert!(out.contains("moved"), "{out}");
    assert_eq!(w.store.session(&own).unwrap().horse, w.other_horse);
    assert!(host.cards().is_empty());
    // The session is gone from this horse: no more commands on it.
    let f = run(&w.access, SBX, &format!("fork {own}"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Refused);
}

#[test]
fn a_disconnect_needs_a_live_run() {
    let w = world();
    give(&w, &["disconnect"], 5, 0);
    let host = stub(Answer::Denied);
    let own = &w.access.session;
    let f = run(&w.access, SBX, &format!("disconnect {own}"), &host).unwrap_err();
    assert_eq!(f.kind, Kind::Failed);
    assert_eq!(host.lines(), [format!("ctl disconnect {own} failed")]);
    // A failed command gives its budget back.
    assert_eq!(grant::get(&w.store, SBX).unwrap().unwrap().used, 0);
}

#[test]
fn a_grant_is_checked_when_it_is_written() {
    let w = world();
    let mut g = Grant {
        sandbox: "../x".into(),
        commands: vec!["fork".into()],
        horses: Vec::new(),
        budget: 1,
        tokens: 0,
        used: 0,
        spent_tokens: 0,
        granted_ms: 1,
    };
    assert!(grant::set(&w.store, &g).is_err());
    g.sandbox = SBX.into();
    g.commands = vec!["rewrite".into()];
    assert!(grant::set(&w.store, &g).is_err());
    g.commands = vec!["move".into()];
    assert!(grant::set(&w.store, &g).is_err());
    g.horses = vec!["h-none".into()];
    assert!(grant::set(&w.store, &g).is_err());
    g.horses = vec![w.other_horse.clone()];
    grant::set(&w.store, &g).unwrap();
    assert_eq!(grant::list(&w.store).unwrap(), [g]);
    assert!(grant::revoke(&w.store, SBX).unwrap());
    assert!(!grant::revoke(&w.store, SBX).unwrap());
    assert!(grant::get(&w.store, "../x").unwrap().is_none());
}

#[test]
fn trainer_cannot_use_ctl_even_if_the_user_grants_it() {
    let w = world();
    give(&w, &["fork"], 5, 100_000);
    crate::horse::device::peek::grant::issue_trainer(
        &w.store,
        SBX,
        "trainer",
        &w.other_horse,
        &w.other,
        crate::horse::now_ms() + 60_000,
        100_000,
    )
    .unwrap();
    let host = stub(Answer::Approved);
    let before = sessions(&w);
    let error = run(&w.access, SBX, &format!("fork {}", w.access.session), &host).unwrap_err();
    assert_eq!(error.kind, Kind::Refused);
    assert!(host.cards().is_empty());
    assert_eq!(sessions(&w), before);
}
