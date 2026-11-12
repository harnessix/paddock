//! The state rules, the ticket grouping, and a golden test of the JSON and
//! the text, on fixture runs, fixture mail and fixture notes folders in a
//! temporary home.

use super::origin::{self, Origin};
use super::tickets::{self, TicketFile};
use super::*;
use crate::env::HostEnv;
use crate::mail::{self, Dir, Kind, Message};
use std::path::Path;

mod invisible;

fn said(ts: u64, text: &str) -> Option<Said> {
    Some(Said {
        ts,
        text: text.into(),
    })
}

fn facts() -> Facts {
    Facts {
        id: "claude-p-aaaaaa".into(),
        harness: "claude".into(),
        repo: "/r".into(),
        started: 100,
        ..Facts::default()
    }
}

#[test]
fn a_new_run_works_since_its_start() {
    assert_eq!(rules::run_state(&facts()), (State::Working, 100));
}

#[test]
fn a_card_or_a_question_makes_a_run_wait_before_anything_else() {
    let mut f = facts();
    f.status = said(300, "blocked: no key");
    f.stuck_since = Some(250);
    f.cards = vec![Said {
        ts: 400,
        text: "run git push".into(),
    }];
    f.ask = said(350, "which branch?");
    assert_eq!(rules::run_state(&f), (State::Waiting, 350));
    assert_eq!(rules::last(&f, State::Waiting), "which branch?");
}

#[test]
fn blocked_and_done_come_from_the_last_status() {
    let mut f = facts();
    f.status = said(200, "blocked: needs the key");
    f.idle_since = Some(250);
    assert_eq!(rules::run_state(&f), (State::Blocked, 200));
    f.status = said(300, "done: merged");
    assert_eq!(rules::run_state(&f), (State::Done, 300));
    // A later `working` replaces both.
    f.status = said(400, "working");
    f.idle_since = None;
    assert_eq!(rules::run_state(&f), (State::Working, 400));
    // `blockedX:` is not blocked.
    f.status = said(500, "blockedfoo: x");
    assert_eq!(rules::run_state(&f).0, State::Working);
}

#[test]
fn idle_with_unread_mail_or_the_same_screen_is_stuck() {
    let mut f = facts();
    f.idle_since = Some(500);
    assert_eq!(rules::run_state(&f), (State::Idle, 500));
    f.unread_mail = true;
    assert_eq!(rules::run_state(&f), (State::Stuck, 500));
    let mut g = facts();
    g.stuck_since = Some(600);
    assert_eq!(rules::run_state(&g), (State::Stuck, 600));
}

#[test]
fn an_ended_run_is_done_after_a_done_status_and_ended_otherwise() {
    let mut f = facts();
    f.ended = Some(900);
    f.cards = vec![Said::default()];
    assert_eq!(rules::run_state(&f), (State::Ended, 900));
    f.status = said(800, "done: all green");
    assert_eq!(rules::run_state(&f), (State::Done, 800));
    f.status = said(800, "working");
    f.end_text = Some("exit 0".into());
    assert_eq!(rules::last(&f, State::Ended), "working");
    f.status = None;
    assert_eq!(rules::last(&f, State::Ended), "exit 0");
}

#[test]
fn the_last_line_is_the_newest_message_on_one_line() {
    let mut f = facts();
    f.status = said(200, "working");
    f.say = said(300, "KV2: code\nand  14 tests\tpass\x1b[31m");
    assert_eq!(
        rules::last(&f, State::Working),
        "KV2: code and 14 tests pass"
    );
    f.say = said(100, "old");
    assert_eq!(rules::last(&f, State::Working), "working");
    let long = "x".repeat(500);
    f.say = said(999, &long);
    assert_eq!(
        rules::last(&f, State::Working).chars().count(),
        rules::MAX_LAST
    );
}

#[test]
fn a_ticket_takes_the_most_urgent_state_of_its_runs() {
    use State::*;
    assert_eq!(rules::ticket_state([Done, Working, Idle]), Working);
    assert_eq!(rules::ticket_state([Ended, Done]), Done);
    assert_eq!(rules::ticket_state([Idle, Stuck, Blocked]), Blocked);
    assert_eq!(rules::ticket_state([Working, Waiting]), Waiting);
    assert_eq!(rules::ticket_state([]), Open);
    // The order of the spec.
    let mut all = State::ALL.to_vec();
    all.sort();
    assert_eq!(
        all,
        [Waiting, Blocked, Stuck, Working, Idle, Open, Done, Ended]
    );
}

#[test]
fn ticket_ids_titles_and_status_lines() {
    assert_eq!(tickets::id_of("KV2-worker.md"), "KV2");
    assert_eq!(tickets::id_of("HORSE3E-f5-policy.md"), "HORSE3E");
    assert_eq!(tickets::id_of("horse3e.wake-f1.md"), "horse3e");
    assert_eq!(tickets::id_of("NOTES.md"), "NOTES");
    assert_eq!(tickets::id_of("-odd.md"), "-odd");
    let t = tickets::parse(
        "KV2-worker.md",
        b"Intro\n# KV2: named kv namespaces\n\nStatus: open, first.\n  Status: not this\n",
    );
    assert_eq!(t.id, "KV2");
    assert_eq!(t.title, "named kv namespaces");
    assert_eq!(t.file, ".paddock/tickets/KV2-worker.md");
    assert_eq!(t.status.as_deref(), Some("open, first."));
    let t = tickets::parse("CR1-corral.md", b"## CR1 - Corral\n");
    assert_eq!(t.title, "Corral");
    assert_eq!(t.status, None);
    let t = tickets::parse("X-y.md", b"no heading");
    assert_eq!(t.title, "X-y");
    let t = tickets::parse("KV-a.md", b"# KV2 is not KV\n");
    assert_eq!(t.title, "KV2 is not KV");
}

#[test]
fn a_ticket_id_drops_the_escapes_of_its_file_name() {
    let t = tickets::parse("X\x1b]52;c;aGk=\x07-a.md", b"Status: open.\n");
    assert!(!t.id.chars().any(char::is_control), "{:?}", t.id);
    assert_eq!(t.id, "X");
    let t = tickets::parse("\x1b[2J-a.md", b"Status: open.\n");
    assert_eq!(t.id, "?");
}

#[test]
fn the_text_view_puts_the_most_urgent_row_first_with_or_without_a_ticket() {
    let tmp = tempfile::tempdir().unwrap();
    let env = fixture(tmp.path());
    let text = text::render(&collect(&env, NOW), None, false);
    for block in text.split("\n\n") {
        let states: Vec<State> = block
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1))
            .filter_map(|w| State::ALL.into_iter().find(|s| s.name() == w))
            .collect();
        assert!(states.windows(2).all(|w| w[0] <= w[1]), "{block}");
    }
}

#[test]
fn a_prompt_path_names_its_ticket_from_any_checkout() {
    for p in [
        ".paddock/tickets/KV2-worker.md",
        "./.paddock/tickets/KV2-worker.md",
        "/home/demo/wt/x/.paddock/tickets/KV2-worker.md",
        "/home/demo/.local/share/paddock/loft/ab/tree/.paddock/tickets/KV2-worker.md",
    ] {
        assert_eq!(tickets::ticket_name(p), Some("KV2-worker.md"), "{p}");
    }
    assert_eq!(tickets::ticket_name("tickets/KV2-worker.md"), None);
    assert_eq!(tickets::ticket_name("/tmp/prompt.md"), None);
}

fn ticket(name: &str, status: Option<&str>, body: &[u8]) -> TicketFile {
    let mut t = tickets::parse(name, body);
    t.status = status.map(str::to_string);
    t
}

#[test]
fn runs_group_under_their_tickets_by_file_then_by_text() {
    let files = vec![
        ticket("A1-x.md", None, b"# A1: first"),
        ticket("B2-y.md", None, b"# B2: second"),
        ticket("C3-z.md", Some("open."), b"# C3: third"),
        ticket("D4-w.md", None, b"# D4: no run, no status"),
    ];
    let by_file = Origin {
        file: Some(".paddock/tickets/A1-x.md".into()),
        ..Origin::default()
    };
    let by_text = Origin {
        sha256: origin::sha256_hex(b"# B2: second"),
        ..Origin::default()
    };
    let lost = Origin {
        file: Some(".paddock/tickets/GONE.md".into()),
        sha256: origin::sha256_hex(b"other"),
        ..Origin::default()
    };
    let run = |id: &str, o: Option<Origin>, st: Option<Said>| Facts {
        id: id.into(),
        harness: "codex".into(),
        repo: "/r/proj".into(),
        started: 10,
        origin: o,
        status: st,
        ..Facts::default()
    };
    let facts = vec![
        run(
            "codex-p-000001",
            Some(by_file.clone()),
            said(20, "done: ok"),
        ),
        run("codex-p-000002", Some(by_file), None),
        run("codex-p-000003", Some(by_text), said(30, "blocked: x")),
        run("codex-p-000004", Some(lost), None),
        run("codex-p-000005", None, None),
    ];
    let mut map = BTreeMap::new();
    map.insert(PathBuf::from("/r/proj"), files);
    let b = build(&facts, &map, 0, vec![]);
    let p = &b.projects[0];
    assert_eq!(p.name, "proj");
    let ids: Vec<(&str, State)> = p.tickets.iter().map(|t| (t.id.as_str(), t.state)).collect();
    assert_eq!(
        ids,
        [
            ("B2", State::Blocked),
            ("A1", State::Working),
            ("C3", State::Open)
        ]
    );
    let a1: Vec<(&str, State)> = p.tickets[1]
        .runs
        .iter()
        .map(|r| (r.id.as_str(), r.state))
        .collect();
    assert_eq!(a1, [("000002", State::Working), ("000001", State::Done)]);
    let loose: Vec<&str> = p
        .runs_without_ticket
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(loose, ["000004", "000005"]);
}

#[test]
fn ended_runs_stay_longer_with_a_ticket() {
    let files = vec![ticket("A1-x.md", None, b"# A1")];
    let mut map = BTreeMap::new();
    map.insert(PathBuf::from("/r"), files);
    let mut f = facts();
    f.repo = "/r".into();
    let now = 10 * collect::ENDED_TICKET;
    f.ended = Some(now - collect::ENDED_LOOSE - 1);
    assert!(!collect::keep(&f, &map, now));
    f.origin = Some(Origin {
        file: Some(".paddock/tickets/A1-x.md".into()),
        ..Origin::default()
    });
    assert!(collect::keep(&f, &map, now));
    f.ended = Some(now - collect::ENDED_TICKET - 1);
    assert!(!collect::keep(&f, &map, now));
    f.ended = None;
    assert!(collect::keep(&f, &map, now));
}

#[test]
fn text_lines_are_cut_to_the_width() {
    assert_eq!(text::cut("abcdef", Some(4)), "abc…");
    assert_eq!(text::cut("abcdef", Some(6)), "abcdef");
    assert_eq!(text::cut("abcdef", None), "abcdef");
    assert_eq!(text::cut("äöüäöü", Some(3)), "äö…");
}

#[test]
fn origin_records_round_trip_and_refuse_bad_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let env = fixture_env(tmp.path());
    let file = Path::new(".paddock/tickets/A1-x.md");
    origin::record(&env, "claude-p-abc123", Some(file), b"task", Some("m1")).unwrap();
    let o = origin::load(&env, "claude-p-abc123").unwrap();
    assert_eq!(o.file.as_deref(), Some(".paddock/tickets/A1-x.md"));
    assert_eq!(o.sha256, origin::sha256_hex(b"task"));
    assert_eq!(o.model.as_deref(), Some("m1"));
    assert!(origin::record(&env, "../x", None, b"", None).is_err());
    assert!(origin::load(&env, "../x").is_none());
}

// ---- golden: the whole board from a fixture home ----

/// A fixed clock: 2026-11-11T16:00:00Z.
const NOW: u64 = 1_794_412_800;

fn fixture_env(dir: &Path) -> HostEnv {
    let mut vars = std::collections::BTreeMap::new();
    for (k, sub) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let p = dir.join(sub);
        std::fs::create_dir_all(&p).unwrap();
        vars.insert(k.to_string(), p.display().to_string());
    }
    HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars)
}

/// A run with a mailbox. `alive`: a run record with this test's pid.
struct Fx<'a> {
    env: &'a HostEnv,
}

impl Fx<'_> {
    fn run(&self, id: &str, project: &Path, started: u64, alive: bool) {
        let harness = id.split('-').next().unwrap();
        if alive {
            crate::state::Running {
                id: id.into(),
                harness: harness.into(),
                project: project.into(),
                pid: std::process::id(),
                started,
                ..Default::default()
            }
            .register(self.env)
            .unwrap();
        }
        let d = mail::dir(self.env, id).unwrap();
        std::fs::create_dir_all(&d).unwrap();
        let starter = mail::Starter {
            created: started,
            ..mail::Starter::host()
        };
        std::fs::write(
            d.join("starter.json"),
            serde_json::to_vec(&starter).unwrap(),
        )
        .unwrap();
        let info = mail::RunInfo {
            id: id.into(),
            harness: harness.into(),
            project: project.into(),
            // Beyond any pid_max: never alive.
            pid: 1_073_741_000,
            pid_start: None,
            started,
            detached: true,
            delivery: None,
            horse_session: None,
        };
        mail::set_run_info(self.env, &info).unwrap();
    }

    fn mail(
        &self,
        id: &str,
        dir: Dir,
        (seq, ts): (u64, u64),
        kind: Kind,
        text: &str,
        reply: Option<u64>,
    ) {
        let mut m = Message::new(if dir == Dir::Up { "agent" } else { "host" }, kind, text);
        m.seq = seq;
        m.ts = ts;
        m.reply_to = reply;
        let f = mail::dir(self.env, id)
            .unwrap()
            .join(format!("{}.jsonl", dir.name()));
        let mut line = serde_json::to_vec(&m).unwrap();
        line.push(b'\n');
        let mut old = std::fs::read(&f).unwrap_or_default();
        old.extend(line);
        std::fs::write(f, old).unwrap();
    }

    fn end(&self, id: &str, ts: u64, text: &str) {
        let mut m = Message::event("ended", text);
        m.ts = ts;
        m.seq = 99;
        let f = mail::dir(self.env, id).unwrap().join("end");
        std::fs::write(f, serde_json::to_vec(&m).unwrap()).unwrap();
    }

    fn origin(&self, id: &str, file: Option<&str>, prompt: &[u8], model: Option<&str>) {
        origin::record(self.env, id, file.map(Path::new), prompt, model).unwrap();
    }

    fn record(&self, id: &str, project: &Path, idle: Option<u64>, stuck: Option<u64>) {
        crate::detach::Record {
            id: id.into(),
            harness: "claude".into(),
            project: project.into(),
            started: 0,
            idle_since: idle,
            stuck: stuck.map(|since| crate::detach::Stuck {
                since,
                screen: String::new(),
                error: None,
            }),
            ..Default::default()
        }
        .save(self.env)
        .unwrap();
    }

    fn card(&self, id: &str, sandbox: &str, created: u64, expires: u64, summary: &str) {
        let d = crate::broker::approvals_dir(self.env);
        std::fs::create_dir_all(&d).unwrap();
        let p = serde_json::json!({
            "id": id, "sandbox": sandbox, "harness": "codex", "project": "/p",
            "capability": "git-push", "summary": summary, "detail": "", "reason": "",
            "created": created, "expires": expires, "session_allowed": false,
        });
        std::fs::write(d.join(format!("{id}.json")), p.to_string()).unwrap();
    }

    fn slot(&self, n: u32, holder: &str, expires: u64) {
        let root = crate::kv::kv_root(self.env);
        std::fs::create_dir_all(&root).unwrap();
        let mut st = crate::kv::store::Store::open(&root.join("user.db")).unwrap();
        let w = crate::kv::store::Writer {
            sandbox: holder,
            harness: "claude",
        };
        let q = crate::kv::store::Quota {
            bytes: 1 << 20,
            keys: 100,
        };
        st.put(
            &crate::kv::shard::Ns::User.key(),
            &format!("lease:compile-slot-{n}"),
            "{}",
            None,
            Some(expires),
            &w,
            q,
            NOW - 60,
        )
        .unwrap();
    }
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const BETA_TICKET: &str = "# LF1: change ledger\n\nStatus: open.\n";

/// Two projects: `alpha` keeps its tickets in its own `.paddock/`, `beta`
/// in its loft. Every run state, a card, an expired card, a held and an
/// expired compile slot.
fn fixture(dir: &Path) -> HostEnv {
    let env = fixture_env(dir);
    let alpha = dir.join("src/alpha");
    let beta = dir.join("src/beta");
    let t = alpha.join(".paddock/tickets");
    write(&t.join("KV2-worker.md"), "# KV2: named kv namespaces\n");
    write(
        &t.join("AUTH1-login.md"),
        "# AUTH1: login refresh\nStatus: open, needs a design.\n",
    );
    write(&t.join("MAIL2-worker.md"), "# MAIL2: mail hooks\n");
    write(&t.join("CR1-corral.md"), "# CR1 - corral\n");
    write(&t.join("NOTES-misc.md"), "# loose notes, no status\n");
    std::fs::create_dir_all(&beta).unwrap();
    let loft = crate::loft::Loft::of(&env, &beta).notes().join("tickets");
    write(&loft.join("LF1-change-ledger.md"), BETA_TICKET);
    let fx = Fx { env: &env };
    let h = 3600;
    // KV2: working, chosen model, a status then a message.
    fx.run("claude-alpha-71c3ac", &alpha, NOW - h, true);
    fx.origin(
        "claude-alpha-71c3ac",
        Some(".paddock/tickets/KV2-worker.md"),
        b"x",
        Some("claude-opus-5-5"),
    );
    fx.mail(
        "claude-alpha-71c3ac",
        Dir::Up,
        (1, NOW - 900),
        Kind::Status,
        "working",
        None,
    );
    fx.mail(
        "claude-alpha-71c3ac",
        Dir::Up,
        (2, NOW - 600),
        Kind::Say,
        "KV2: code and 14 tests pass",
        None,
    );
    // MAIL2: one run waits on a card, one ended done.
    fx.run("codex-alpha-0b0b0b", &alpha, NOW - 2 * h, true);
    fx.origin(
        "codex-alpha-0b0b0b",
        Some("/elsewhere/wt/.paddock/tickets/MAIL2-worker.md"),
        b"y",
        None,
    );
    fx.card(
        "req-1",
        "codex-alpha-0b0b0b",
        NOW - 120,
        NOW + 600,
        "git push to origin/main",
    );
    fx.card(
        "req-2",
        "codex-alpha-0b0b0b",
        NOW - 9000,
        NOW - 1,
        "expired card",
    );
    fx.run("codex-alpha-0c0c0c", &alpha, NOW - 5 * h, false);
    fx.origin(
        "codex-alpha-0c0c0c",
        Some(".paddock/tickets/MAIL2-worker.md"),
        b"z",
        None,
    );
    fx.mail(
        "codex-alpha-0c0c0c",
        Dir::Up,
        (1, NOW - 4 * h),
        Kind::Status,
        "done: hooks merged",
        None,
    );
    fx.end("codex-alpha-0c0c0c", NOW - 4 * h + 30, "exit 0");
    // CR1: blocked.
    fx.run("claude-alpha-0d0d0d", &alpha, NOW - h, true);
    fx.origin(
        "claude-alpha-0d0d0d",
        Some(".paddock/tickets/CR1-corral.md"),
        b"w",
        None,
    );
    fx.mail(
        "claude-alpha-0d0d0d",
        Dir::Up,
        (1, NOW - 300),
        Kind::Status,
        "blocked: needs the signing key",
        None,
    );
    // Without a ticket: stuck (idle, unread mail), idle, ended long ago.
    fx.run("claude-alpha-0e0e0e", &alpha, NOW - h, true);
    fx.record("claude-alpha-0e0e0e", &alpha, Some(NOW - 1200), None);
    fx.mail(
        "claude-alpha-0e0e0e",
        Dir::Down,
        (1, NOW - 1000),
        Kind::Say,
        "go on",
        None,
    );
    fx.run("grok-alpha-0f0f0f", &alpha, NOW - h, true);
    fx.record("grok-alpha-0f0f0f", &alpha, Some(NOW - 700), None);
    fx.run("codex-alpha-101010", &alpha, NOW - 9 * h, false);
    fx.end("codex-alpha-101010", NOW - 2 * h, "exit 1");
    // beta: a prompt with the text of the ticket, and an open question.
    fx.run("claude-beta-202020", &beta, NOW - h, true);
    fx.origin("claude-beta-202020", None, BETA_TICKET.as_bytes(), None);
    fx.mail(
        "claude-beta-202020",
        Dir::Up,
        (1, NOW - 500),
        Kind::Ask,
        "Which ledger format?",
        None,
    );
    fx.mail(
        "claude-beta-202020",
        Dir::Up,
        (2, NOW - 400),
        Kind::Ask,
        "Answered one",
        None,
    );
    fx.mail(
        "claude-beta-202020",
        Dir::Down,
        (3, NOW - 300),
        Kind::Say,
        "json",
        Some(2),
    );
    // A stuck screen on a beta run without ticket.
    fx.run("codex-beta-303030", &beta, NOW - h, true);
    fx.record("codex-beta-303030", &beta, None, Some(NOW - 240));
    fx.slot(1, "claude-alpha-71c3ac", NOW + 600);
    fx.slot(2, "codex-alpha-0b0b0b", NOW - 1);
    env
}

fn golden(name: &str, got: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/board")
        .join(name);
    if std::env::var_os("PADDOCK_BLESS").is_some() {
        write(&path, got);
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        got,
        want,
        "{} differs; PADDOCK_BLESS=1 rewrites it",
        path.display()
    );
}

fn placeholders(dir: &Path, s: &str) -> String {
    let real = std::fs::canonicalize(dir).unwrap();
    let s = s
        .replace(&real.display().to_string(), "/TMP")
        .replace(&dir.display().to_string(), "/TMP");
    // The loft key is a hash of the project path, which holds the temporary
    // folder, so it differs on each run.
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find("/paddock/loft/") {
        let (head, tail) = rest.split_at(i + "/paddock/loft/".len());
        out.push_str(head);
        let key = tail.find('/').unwrap_or(tail.len());
        out.push_str("KEY");
        rest = &tail[key..];
    }
    out.push_str(rest);
    out
}

#[test]
fn golden_json_and_text_of_a_fixture_home() {
    let tmp = tempfile::tempdir().unwrap();
    let env = fixture(tmp.path());
    let snap = collect(&env, NOW);
    let json = to_json(&snap.board, true).unwrap() + "\n";
    golden("board.json", &placeholders(tmp.path(), &json));
    let text = text::render(&snap, None, false);
    golden("board.txt", &placeholders(tmp.path(), &text));
    let narrow = text::render(&snap, Some(60), false);
    assert!(narrow.lines().all(|l| l.chars().count() <= 60), "{narrow}");
    golden("board-60.txt", &placeholders(tmp.path(), &narrow));
    // The JSON reads back into the contract types.
    let back: Board = serde_json::from_str(&json).unwrap();
    assert_eq!(back, snap.board);
}

#[test]
fn every_state_word_is_in_the_contract() {
    let words: Vec<String> = State::ALL
        .iter()
        .map(|s| {
            serde_json::to_value(s)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        words,
        ["waiting", "blocked", "stuck", "working", "idle", "open", "done", "ended"]
    );
    for s in State::ALL {
        assert_eq!(serde_json::to_value(s).unwrap(), s.name());
    }
}

#[test]
fn an_empty_home_has_an_empty_board() {
    let tmp = tempfile::tempdir().unwrap();
    let env = fixture_env(tmp.path());
    let snap = collect(&env, NOW);
    assert_eq!(
        serde_json::to_value(&snap.board).unwrap(),
        serde_json::json!({"version": 1, "approvals_waiting": 0, "compile_slots": [], "projects": []})
    );
    assert_eq!(text::render(&snap, None, false), "No runs on the board.\n");
}

#[test]
fn a_loft_path_is_rejected_for_every_listed_character_and_for_length() {
    // The set that the panel (Board.loftDir) uses too. Every code point is
    // listed, so a change on one side shows here.
    let mut rejected: Vec<u32> = (0x00..=0x1F).chain(0x7F..=0x9F).collect();
    rejected.extend([0x00AD, 0x061C, 0x180E]);
    rejected.extend(0x200B..=0x200F);
    rejected.extend([0x2028, 0x2029]);
    rejected.extend(0x202A..=0x202E);
    rejected.extend(0x2060..=0x206F);
    rejected.push(0xFEFF);
    rejected.extend(0xFFF9..=0xFFFB);
    for &cp in &rejected {
        let c = char::from_u32(cp).unwrap();
        assert!(
            !plain_path(&format!("/home/demo/lo{c}ft")),
            "U+{cp:04X} passes"
        );
    }
    // The neighbours of each range, and other characters, pass.
    for cp in [
        0x20, 0x7E, 0xA0, 0xAC, 0xAE, 0x061B, 0x061D, 0x180D, 0x180F, 0x200A, 0x2010, 0x2027,
        0x202F, 0x205F, 0x2070, 0xFEFE, 0xFF00, 0xFFF8, 0xFFFC, 0xE9, 0x1F40E,
    ] {
        let c = char::from_u32(cp).unwrap();
        assert!(
            plain_path(&format!("/home/demo/lo{c}ft")),
            "U+{cp:04X} is rejected"
        );
    }
    // Every other code point below U+10000 passes.
    let passing = (0..0x1_0000u32)
        .filter_map(char::from_u32)
        .filter(|c| !rejected.contains(&(*c as u32)))
        .filter(|c| !plain_path(&format!("/a{c}")))
        .map(|c| format!("U+{:04X}", c as u32))
        .collect::<Vec<_>>();
    assert!(passing.is_empty(), "rejected but not listed: {passing:?}");

    // At most 1024 characters (code points), as the panel counts.
    assert_eq!(LOFT_PATH_MAX, 1024);
    let at = format!("/{}", "a".repeat(1023));
    assert!(plain_path(&at));
    assert!(!plain_path(&format!("{at}a")));
    // A character outside the BMP counts as one.
    let horses = format!("/{}", "\u{1F40E}".repeat(1023));
    assert_eq!(horses.chars().count(), 1024);
    assert!(plain_path(&horses));
    assert!(!plain_path(&format!("{horses}\u{1F40E}")));
}
