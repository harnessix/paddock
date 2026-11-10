use super::evidence::{self, Gate, Outcome, Status, Tests};
use super::forecast::{self, Rng, Verdict};
use super::history::{self, SnapTask, Snapshot, Source};
use super::measure::DAY;
use super::*;
use crate::speckit::board;

const T0: u64 = 1_790_000_000;

/// A synthetic history: tasks appear, finish, go back, sit in review.
struct Hist {
    at: u64,
    tasks: Vec<SnapTask>,
    snaps: Vec<Snapshot>,
    next: u32,
}

impl Hist {
    fn new(open: usize) -> Self {
        let mut h = Hist {
            at: T0,
            tasks: Vec::new(),
            snaps: Vec::new(),
            next: 1,
        };
        h.add(open);
        h.snap();
        h
    }

    fn add(&mut self, n: usize) -> &mut Self {
        for _ in 0..n {
            self.tasks.push(SnapTask {
                id: format!("T{:03}", self.next),
                checked: false,
                state: Some("todo".into()),
                branch: None,
            });
            self.next += 1;
        }
        self
    }

    fn finish(&mut self, n: usize) -> &mut Self {
        let open = |t: &&mut SnapTask| !t.checked && t.state.as_deref() != Some("review");
        for t in self.tasks.iter_mut().filter(open).take(n) {
            t.checked = true;
            t.state = Some("done".into());
        }
        self
    }

    fn set(&mut self, id: &str, checked: bool, state: &str) -> &mut Self {
        let t = self.tasks.iter_mut().find(|t| t.id == id).unwrap();
        t.checked = checked;
        t.state = Some(state.into());
        self
    }

    fn snap(&mut self) -> &mut Self {
        self.snaps.push(Snapshot {
            at: self.at,
            source: Source::RepoGit,
            tasks: self.tasks.clone(),
        });
        self
    }

    /// One day: `f` tasks finish and `d` new ones appear, in four steps.
    fn day(&mut self, f: usize, d: usize) -> &mut Self {
        for q in 0..4 {
            self.at += DAY / 4;
            self.finish(f / 4 + usize::from(q < f % 4));
            self.add(d / 4 + usize::from(q < d % 4));
            self.snap();
        }
        self
    }

    fn input(&self) -> Input {
        Input {
            feature: "001-test".into(),
            now: self.at,
            snapshots: self.snaps.clone(),
            sources: vec![Source::RepoGit],
            git: true,
            ..Default::default()
        }
    }
}

fn quick() -> Opts {
    Opts {
        trials: 2000,
        ..Opts::default()
    }
}

#[test]
fn a_converging_history_gets_a_date_and_a_final_count() {
    let mut h = Hist::new(30);
    for _ in 0..7 {
        h.day(4, 1);
    }
    let r = analyze(&h.input(), &quick());
    assert_eq!(
        r.forecast.verdict,
        Verdict::Converging,
        "{}",
        render::text(&r)
    );
    assert!(!r.forecast.early);
    let (p50, p85) = (r.forecast.p50.unwrap(), r.forecast.p85.unwrap());
    assert!(p50.at.unwrap() > r.now && p85.at.unwrap() >= p50.at.unwrap());
    // 9 open tasks at 3 net a day: about three days.
    let d = p50.days.unwrap();
    assert!((2.0..=4.5).contains(&d), "{d}");
    assert!(p85.total >= p50.total && p50.total >= r.counts.scope);
    assert_eq!(r.counts.discovered, 7);
    assert_eq!(r.window.days, 7);
    let text = render::text(&r);
    assert!(text.contains("P50 ") && text.contains("P85 "), "{text}");
    assert!(text.contains("burn-up"), "{text}");
}

#[test]
fn a_diverging_history_says_not_converging_and_shows_no_date() {
    let mut h = Hist::new(10);
    for _ in 0..5 {
        h.day(2, 3);
    }
    let r = analyze(&h.input(), &quick());
    assert_eq!(r.forecast.verdict, Verdict::NotConverging);
    assert!(r.forecast.p50.is_none() && r.forecast.p85.is_none());
    assert!(r.forecast.arrival_per_day > r.forecast.finish_per_day);
    let text = render::text(&r);
    assert!(text.contains("Not converging"), "{text}");
    // Equal rates do not converge either.
    let mut h = Hist::new(10);
    for _ in 0..5 {
        h.day(2, 2);
    }
    let r = analyze(&h.input(), &quick());
    assert_eq!(r.forecast.verdict, Verdict::NotConverging);
}

#[test]
fn the_ninety_percent_syndrome_is_seen() {
    // 36 of 40 done after three days, then every finished task brings a
    // new one, and one task sits in review for days.
    let mut h = Hist::new(40);
    h.day(12, 0).day(12, 0).day(12, 0);
    h.set("T037", false, "review").snap();
    for _ in 0..7 {
        h.day(2, 2);
    }
    let r = analyze(&h.input(), &quick());
    let done_share = 100 * r.counts.done / r.counts.scope;
    assert!(done_share >= 80, "{done_share}");
    assert_eq!(r.forecast.verdict, Verdict::NotConverging);
    let stall = r.near_end_stall.as_deref().expect("stall");
    assert!(stall.contains("did not go down"), "{stall}");
    let t37 = r.tasks.iter().find(|t| t.id == "T037").unwrap();
    assert!(t37.almost_secs >= 3 * DAY, "{}", t37.almost_secs);
    assert!(t37.in_review_since.is_some());
    let text = render::text(&r);
    assert!(text.contains("stuck ") && text.contains("T037"), "{text}");
}

fn record(id: &str, verdict: Option<&str>, report: &str) -> board::Record {
    board::Record {
        id: id.into(),
        state: board::State::Done,
        verdict: verdict.map(str::to_string),
        report: Some(board::Report {
            ok: true,
            text: report.into(),
            at: T0,
        }),
        ..Default::default()
    }
}

#[test]
fn skips_are_not_done_and_evidence_needs_another_party() {
    let mut h = Hist::new(8);
    h.day(8, 0);
    let mut i = h.input();
    let done = T0 + DAY / 4;
    for (id, verdict, report) in [
        ("T001", Some("pass"), "implemented, all tests pass"),
        ("T002", None, "implemented"),
        ("T003", None, "implemented"),
        ("T004", Some("pass"), "Sandbox tests skipped (no bwrap)."),
        (
            "T005",
            Some("pass"),
            "unit tests pass; the sandbox tests skip here",
        ),
        ("T006", Some("pass"), "implemented"),
        ("T007", Some("pass"), "implemented"),
        ("T008", None, "implemented"),
    ] {
        i.records.insert(id.into(), record(id, verdict, report));
    }
    i.merged.insert("T002".into(), true);
    i.merged.insert("T008".into(), false);
    let gate = |task: &str, outcome, at| Gate {
        task: task.into(),
        gate: "real-sandbox".into(),
        outcome,
        at,
        note: String::new(),
        peer: None,
        snapshot_hash: None,
    };
    i.gates = vec![
        gate("T006", Outcome::Skip, done + 10),
        gate("T007", Outcome::Fail, h.at),
        // A skip that a later pass replaced is evidence.
        gate("T003", Outcome::Skip, done + 10),
        gate("T003", Outcome::Pass, done + 20),
    ];
    let r = analyze(&i, &quick());
    let st = |id: &str| {
        r.tasks
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .evidence
            .unwrap()
    };
    assert_eq!(st("T001"), Status::Proven);
    assert_eq!(st("T002"), Status::Proven);
    assert_eq!(st("T003"), Status::Proven);
    assert_eq!(st("T004"), Status::SkippedOnly);
    assert_eq!(st("T005"), Status::Proven);
    assert_eq!(st("T006"), Status::SkippedOnly);
    assert_eq!(st("T007"), Status::FailedAfterDone);
    assert_eq!(st("T008"), Status::Claimed);
    let c = &r.counts;
    assert_eq!((c.done, c.proven, c.claimed), (8, 4, 1));
    assert_eq!(
        (c.skipped_only, c.failed_after_done, c.partly_skipped),
        (2, 1, 1)
    );
    assert_eq!(c.fails_after_done, 1);
    // The not-done tasks keep the forecast open.
    assert_eq!(r.forecast.open, 3);
    assert_ne!(r.forecast.verdict, Verdict::Done);
    // Strict: the claimed task counts as open too.
    let strict = analyze(
        &i,
        &Opts {
            strict: true,
            ..quick()
        },
    );
    assert_eq!(strict.forecast.open, 4);
    let text = render::text(&r);
    assert!(
        text.contains("not done, tests only skipped: T004, T006"),
        "{text}"
    );
    assert!(
        text.contains("partly skipped (a part waits for a real run): T005"),
        "{text}"
    );
    assert!(text.contains("no evidence: T008"), "{text}");
}

#[test]
fn a_plain_folder_says_what_it_cannot_know() {
    let mut h = Hist::new(5);
    h.finish(2).snap();
    let mut i = h.input();
    i.snapshots = vec![i.snapshots.pop().unwrap()];
    i.sources.clear();
    i.git = false;
    i.start_hint = Some(T0 - 3 * DAY);
    let r = analyze(&i, &quick());
    assert!(!r.start_known);
    assert_eq!(r.start, T0 - 3 * DAY);
    assert_eq!(r.forecast.verdict, Verdict::NoActivity);
    assert_eq!((r.counts.done, r.counts.claimed), (2, 2));
    let k = r.cannot_know.join("\n");
    assert!(k.contains("no history"), "{k}");
    assert!(k.contains("no git repository"), "{k}");
    assert!(k.contains("a guess"), "{k}");
    let text = render::text(&r);
    assert!(text.contains("burn-up: needs a history"), "{text}");
    assert!(text.contains("paddock cannot know:"), "{text}");
}

#[test]
fn one_seed_gives_one_forecast() {
    let mut h = Hist::new(25);
    for (f, d) in [(5, 1), (1, 0), (3, 2), (6, 0), (0, 1), (4, 1), (2, 0)] {
        h.day(f, d);
    }
    let i = h.input();
    let a = serde_json::to_string(&analyze(&i, &Opts::default())).unwrap();
    let b = serde_json::to_string(&analyze(&i, &Opts::default())).unwrap();
    assert_eq!(a, b);
    let seeds: std::collections::BTreeSet<String> = (1..6)
        .map(|seed| {
            let r = analyze(&i, &Opts { seed, ..quick() });
            format!("{:?}", r.forecast.p85)
        })
        .collect();
    assert!(seeds.len() > 1, "other seeds give other samples: {seeds:?}");
    // The generator is fixed: these numbers never change.
    let mut rng = Rng::new(1);
    assert_eq!(rng.next_u64(), 0x910a_2dec_8902_5cc1);
    assert!((0..1000).all(|_| rng.below(7) < 7));
}

#[test]
fn few_finished_tasks_give_a_wide_labelled_range() {
    let mut h = Hist::new(12);
    h.day(1, 0).day(1, 0).day(1, 0);
    let r = analyze(&h.input(), &quick());
    assert_eq!(r.forecast.verdict, Verdict::Converging);
    assert!(r.forecast.early);
    let (lo, hi, p50) = (
        r.forecast.low.unwrap(),
        r.forecast.high.unwrap(),
        r.forecast.p50.unwrap(),
    );
    assert!(lo.days.unwrap() <= p50.days.unwrap() * 0.5 + 1e-9);
    assert!(hi.days.unwrap() >= p50.days.unwrap() * 2.0 - 1e-9);
    let text = render::text(&r);
    assert!(
        text.contains("Early forecast") && text.contains("wide range"),
        "{text}"
    );
}

#[test]
fn the_trend_of_p85_is_shown() {
    // Discovery slows down: the forecast narrows.
    let mut h = Hist::new(30);
    for (f, d) in [(2, 4), (2, 4), (3, 2), (4, 1), (4, 0), (5, 0), (5, 0)] {
        h.day(f, d);
    }
    let r = analyze(&h.input(), &quick());
    assert!(r.trend.len() >= 4, "{:?}", r.trend);
    assert_eq!(r.trend.first().unwrap().verdict, Verdict::NotConverging);
    assert_ne!(r.trend_verdict, "unknown");
    assert!(render::text(&r).contains("P85 trend"));
    // Discovery speeds up: the date slips.
    let mut h = Hist::new(30);
    for (f, d) in [(5, 0), (5, 0), (5, 1), (5, 2), (5, 3), (5, 3), (5, 4)] {
        h.day(f, d);
    }
    let r = analyze(&h.input(), &quick());
    assert!(
        matches!(r.trend_verdict, "slipping" | "widening"),
        "{} {:?}",
        r.trend_verdict,
        r.trend
    );
}

#[test]
fn reopened_and_removed_tasks_change_the_open_work() {
    let mut h = Hist::new(6);
    h.day(4, 0);
    h.at += 3600;
    h.set("T001", false, "todo").snap();
    h.at += 3600;
    h.tasks.retain(|t| t.id != "T006");
    h.snap();
    let r = analyze(&h.input(), &quick());
    assert_eq!(r.counts.reopened, 1);
    assert_eq!(r.counts.removed, 1);
    assert_eq!(r.counts.scope, 5);
    assert_eq!(r.counts.open, 2);
    let t1 = r.tasks.iter().find(|t| t.id == "T001").unwrap();
    assert_eq!(t1.reopened, 1);
    assert!(t1.age_secs.is_some());
}

#[test]
fn a_short_history_has_hour_steps() {
    // The horse case: everything within seven hours.
    let mut h = Hist::new(20);
    for _ in 0..7 {
        h.at += 3600;
        h.finish(2).add(1).snap();
    }
    let r = analyze(&h.input(), &quick());
    assert_eq!(r.window.days, 6);
    assert!(r.window.width >= 3600 && r.window.width < DAY);
    assert_eq!(r.forecast.verdict, Verdict::Converging);
    assert!(
        r.forecast.finish_per_day > 40.0,
        "{}",
        r.forecast.finish_per_day
    );
}

#[test]
fn reports_are_read_for_skipped_tests() {
    assert_eq!(
        evidence::tests_of("Sandbox tests skipped (no bwrap)."),
        Tests::SkippedOnly
    );
    assert_eq!(
        evidence::tests_of("the bwrap form skips here"),
        Tests::SkippedOnly
    );
    assert_eq!(
        evidence::tests_of("21 tests ran and pass; 2 sandbox tests skip"),
        Tests::PartlySkipped
    );
    assert_eq!(
        evidence::tests_of("full cargo test 0 failed, the e2e did not run"),
        Tests::PartlySkipped
    );
    assert_eq!(
        evidence::tests_of("implemented, all tests pass"),
        Tests::Unknown
    );
    assert_eq!(
        evidence::commits_in("Commit 860dd78. Also 5fb6728, not deadbeef or 1234567"),
        vec!["860dd78", "5fb6728"]
    );
}

#[test]
fn notes_and_snapshots_are_read() {
    use history::parse_note;
    assert_eq!(
        parse_note("review, paddock/spawn-a"),
        (Some("review".into()), Some("paddock/spawn-a".into()))
    );
    assert_eq!(
        parse_note("changes asked, b"),
        (Some("todo".into()), Some("b".into()))
    );
    assert_eq!(
        parse_note("paddock/spawn-a"),
        (None, Some("paddock/spawn-a".into()))
    );
    assert_eq!(parse_note("running"), (Some("running".into()), None));
    let tasks = history::from_text(
        "# Tasks\n\n## Phase 1\n\n- [X] T001 A <!-- paddock: br-1 -->\n- [ ] T002 [P] B <!-- paddock: review, br-2 -->\n<!--\n- [ ] T009 sample\n-->\n",
    );
    assert_eq!(tasks.len(), 2);
    assert!(tasks[0].checked && tasks[0].branch.as_deref() == Some("br-1"));
    assert_eq!(tasks[1].state.as_deref(), Some("review"));
    // Merging drops a snapshot that tells nothing new.
    let s = |at, checked| Snapshot {
        at,
        source: Source::RepoGit,
        tasks: vec![SnapTask {
            id: "T001".into(),
            checked,
            state: None,
            branch: None,
        }],
    };
    let merged = history::merge(vec![s(3, true), s(1, false), s(2, false)]);
    assert_eq!(merged.iter().map(|s| s.at).collect::<Vec<_>>(), vec![1, 3]);
}

#[test]
fn the_state_log_grows_only_on_change() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("spec/f/history.jsonl");
    let one = vec![SnapTask {
        id: "T001".into(),
        checked: false,
        state: Some("todo".into()),
        branch: None,
    }];
    history::log_states(&path, one.clone(), 10);
    history::log_states(&path, one.clone(), 20);
    let mut two = one.clone();
    two[0].state = Some("review".into());
    history::log_states(&path, two, 30);
    let log = history::read_log(&path);
    assert_eq!(log.iter().map(|s| s.at).collect::<Vec<_>>(), vec![10, 30]);
    assert_eq!(log[1].source, Source::StateLog);
}

#[test]
fn appetite_steps_and_warnings_go_to_the_mailbox_once() {
    use super::appetite::{self, Appetite};
    assert_eq!(appetite::steps(49.0), Vec::<u32>::new());
    assert_eq!(appetite::steps(80.0), vec![50, 80]);
    assert_eq!(appetite::steps(151.0), vec![50, 80, 100, 125, 150]);
    assert_eq!(appetite::parse("3d").unwrap(), Some(3 * DAY));
    assert_eq!(appetite::parse("off").unwrap(), None);
    assert!(appetite::parse("soon").is_err());
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let run = "fake-proj-1a2b3c";
    crate::mail::create(&env, run, &crate::mail::Starter::host()).unwrap();
    let path = t.path().join("appetite.json");
    appetite::save(
        &path,
        &Appetite {
            secs: 2 * DAY,
            text: "2d".into(),
            set_at: T0,
            warned: vec![],
        },
    )
    .unwrap();
    let events = || -> Vec<String> {
        crate::mail::read(&env, run, crate::mail::Dir::Up)
            .into_iter()
            .filter(|m| m.event.as_deref() == Some("appetite"))
            .map(|m| m.text)
            .collect()
    };
    assert!(appetite::warn(&env, &path, Some(run), "001-test", 30.0, "x").is_empty());
    assert_eq!(
        appetite::warn(&env, &path, Some(run), "001-test", 85.0, "P50 soon."),
        vec![50, 80]
    );
    assert!(appetite::warn(&env, &path, Some(run), "001-test", 90.0, "x").is_empty());
    assert_eq!(
        appetite::warn(&env, &path, Some(run), "001-test", 126.0, "x"),
        vec![100, 125]
    );
    let e = events();
    assert_eq!(e.len(), 2, "{e:?}");
    assert!(
        e[0].contains("85 % of its appetite of 2d") && e[0].contains("P50 soon."),
        "{}",
        e[0]
    );
    assert!(e[0].contains("cut scope") && e[0].contains("extend") && e[0].contains("stop"));
    // The coordinator gets the same in its down box.
    let down = crate::mail::read(&env, run, crate::mail::Dir::Down);
    assert_eq!(
        down.iter()
            .filter(|m| m.event.as_deref() == Some("appetite"))
            .count(),
        2
    );
    // Without a run nothing counts as warned.
    appetite::save(
        &path,
        &Appetite {
            secs: DAY,
            text: "1d".into(),
            set_at: T0,
            warned: vec![],
        },
    )
    .unwrap();
    assert!(appetite::warn(&env, &path, None, "001-test", 300.0, "x").is_empty());
    assert!(appetite::load(&path).unwrap().warned.is_empty());
}

#[test]
fn the_forecast_never_runs_away() {
    // One finished task in a week, 50 open: no date within the limit.
    let mut h = Hist::new(50);
    h.day(1, 0);
    for _ in 0..6 {
        h.day(0, 0);
    }
    let r = analyze(&h.input(), &quick());
    let p85 = r.forecast.p85.unwrap();
    assert!(p85.at.is_none() || p85.days.unwrap() <= forecast::MAX_STEPS as f64);
    assert!(render::text(&r).contains("forecast"));
}

#[test]
fn a_rewritten_list_restarts_the_measures() {
    // Seven tasks, then the list is rewritten to 24 (nine of them done
    // already), then the work goes on.
    let mut h = Hist::new(7);
    h.finish(6);
    h.at += 3600;
    h.snap();
    h.at += 600;
    h.add(17);
    for t in h.tasks.iter_mut().skip(7).take(9) {
        t.checked = true;
        t.state = Some("done".into());
    }
    h.snap();
    for _ in 0..6 {
        h.at += 3600;
        h.finish(2).add(1).snap();
    }
    let i = h.input();
    assert_eq!(super::rewrite(&i.snapshots), Some(2));
    let r = analyze(&i, &quick());
    assert_eq!(r.rewritten_at, Some(T0 + 3600 + 600));
    assert_eq!(r.start, T0 + 3600 + 600);
    // Tasks that came in checked are no new work and no finish.
    assert_eq!(r.counts.discovered, 6);
    assert_eq!(r.forecast.verdict, Verdict::Converging);
    assert!(
        r.cannot_know[0].contains("rewritten"),
        "{:?}",
        r.cannot_know
    );
    // The burn-up keeps the whole history.
    assert_eq!(r.burnup.len(), i.snapshots.len());
    // A few new tasks are no rewrite.
    let mut h = Hist::new(3);
    h.at += 60;
    h.add(4).snap();
    assert_eq!(super::rewrite(&h.input().snapshots), None);
}
