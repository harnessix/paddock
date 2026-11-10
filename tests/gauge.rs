//! `paddock gauge` on real projects: a git history, the horse feature's real
//! task history, a loft, a plain folder, appetite and gate results.
use std::path::{Path, PathBuf};
use std::process::Command;

const FEATURE: &str = "001-test";
const DAY: u64 = 86_400;

struct World {
    tmp: tempfile::TempDir,
}

impl World {
    fn new(git: bool, feature: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("project");
        std::fs::create_dir_all(p.join(".specify")).unwrap();
        std::fs::create_dir_all(p.join("specs").join(feature)).unwrap();
        std::fs::create_dir_all(tmp.path().join("home")).unwrap();
        std::fs::write(
            p.join(".specify/feature.json"),
            format!(r#"{{"feature_directory":"specs/{feature}"}}"#),
        )
        .unwrap();
        let w = Self { tmp };
        if git {
            w.git(&["init", "-q", "-b", "main"]);
        }
        w
    }

    fn project(&self) -> PathBuf {
        self.tmp.path().join("project")
    }

    fn git_at(&self, args: &[&str], at: Option<u64>) {
        let mut c = Command::new("git");
        c.current_dir(self.project())
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("HOME", self.tmp.path().join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if let Some(t) = at {
            c.env("GIT_AUTHOR_DATE", format!("@{t} +0000"))
                .env("GIT_COMMITTER_DATE", format!("@{t} +0000"));
        }
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git(&self, args: &[&str]) {
        self.git_at(args, None)
    }

    fn write_tasks(&self, feature: &str, text: &str) {
        std::fs::write(
            self.project().join("specs").join(feature).join("tasks.md"),
            text,
        )
        .unwrap();
    }

    /// Commit a version of tasks.md at time `at`.
    fn commit(&self, feature: &str, text: &str, at: u64) {
        self.write_tasks(feature, text);
        self.git_at(&["add", "-A"], Some(at));
        self.git_at(&["commit", "-q", "-m", "tasks"], Some(at));
    }

    fn paddock(&self, args: &[&str], extra: &[(&str, &str)]) -> (bool, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_paddock"));
        cmd.env_clear()
            .env("HOME", self.tmp.path().join("home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("TZ", "UTC")
            .current_dir(self.project())
            .args(args);
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (ok, out, err) = self.paddock(args, &[]);
        assert!(ok, "paddock {args:?} failed: {err}\n{out}");
        out
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut a = vec!["gauge", "--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.ok(&a)).unwrap()
    }
}

/// A tasks.md with `done` checked tasks of `total`.
fn tasks(total: usize, done: usize) -> String {
    let mut s = String::from("# Tasks\n\n## Phase 1\n\n");
    for i in 1..=total {
        let x = if i <= done { "X" } else { " " };
        s.push_str(&format!("- [{x}] T{i:03} Work item {i}\n"));
    }
    s
}

const T0: u64 = 1_790_000_000;

/// Commit a history: per day (finished, discovered).
fn history(w: &World, open: usize, days: &[(usize, usize)]) -> u64 {
    let (mut total, mut done, mut at) = (open, 0, T0);
    w.commit(FEATURE, &tasks(total, done), at);
    for &(f, d) in days {
        at += DAY;
        done += f;
        total += d;
        w.commit(FEATURE, &tasks(total, done), at);
    }
    at
}

#[test]
fn a_converging_git_history_gets_a_forecast_and_one_seed_one_answer() {
    let w = World::new(true, FEATURE);
    let end = history(
        &w,
        30,
        &[(4, 1), (5, 1), (3, 0), (4, 1), (5, 0), (4, 1), (4, 0)],
    );
    let at = (end + 3600).to_string();
    let text = w.ok(&["gauge", "--at", &at]);
    assert!(text.contains("feature 001-test: 8 snapshots"), "{text}");
    assert!(text.contains("(repository git history)"), "{text}");
    assert!(text.contains("P50 ") && text.contains("P85 "), "{text}");
    assert!(
        text.contains("burn-up (# done, - scope)") && text.contains("P85"),
        "{text}"
    );
    assert!(
        text.contains("discovered   4 in the last 7.0 days"),
        "{text}"
    );
    assert_eq!(
        text,
        w.ok(&["gauge", "--at", &at]),
        "one seed, one forecast"
    );
    let j = w.json(&["--at", &at]);
    assert_eq!(j["api_version"], "paddock.gauge/v1");
    assert_eq!(j["forecast"]["verdict"], "converging");
    assert_eq!(j["forecast"]["seed"], 1);
    assert_eq!(j["counts"]["scope"], 34);
    assert_eq!(j["counts"]["done"], 29);
    let p85 = j["forecast"]["p85"]["at"].as_u64().unwrap();
    assert!(p85 > end, "{j}");
    let other = w.json(&["--at", &at, "--seed", "7"]);
    assert_eq!(other["forecast"]["seed"], 7);
}

#[test]
fn a_diverging_git_history_is_not_converging() {
    let w = World::new(true, FEATURE);
    let end = history(&w, 10, &[(2, 3), (3, 3), (2, 4), (3, 3), (2, 3)]);
    let text = w.ok(&["gauge", "--at", &(end + 60).to_string()]);
    assert!(text.contains("Not converging"), "{text}");
    assert!(!text.contains("P50 "), "{text}");
}

/// The real task history of the horse feature: 19 versions of its tasks.md
/// from one day, with the prose taken out.
fn horse_versions() -> Vec<(u64, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gauge/horse");
    let mut v: Vec<(u64, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            let t = p.file_stem().unwrap().to_str().unwrap().parse().unwrap();
            (t, std::fs::read_to_string(p).unwrap())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn the_horse_feature_history() {
    let w = World::new(true, "001-horse");
    let versions = horse_versions();
    assert_eq!(versions.len(), 19);
    for (t, text) in &versions {
        w.commit("001-horse", text, *t);
    }
    let last = versions.last().unwrap().0;
    let at = (last + 600).to_string();
    let j = w.json(&["--at", &at]);
    assert_eq!(j["snapshots"], 19);
    assert_eq!(j["sources"][0], "repository git history");
    assert_eq!(j["counts"]["scope"], 29);
    assert_eq!(j["counts"]["done"], 28);
    // The morning audit: 17 done of 25. The evening: 28 of 29.
    let burn = j["burnup"].as_array().unwrap();
    let point = |scope: i64, done: i64| {
        burn.iter()
            .find(|p| p["scope"] == scope && p["done"] == done)
            .and_then(|p| p["at"].as_u64())
    };
    let morning = point(25, 17).expect("the morning audit point");
    assert!(point(29, 28).is_some());
    // From the morning on, 4 tasks were discovered and 11 finished.
    let tasks = j["tasks"].as_array().unwrap();
    let after = |key: &str| {
        tasks
            .iter()
            .filter(|t| t[key].as_u64().is_some_and(|x| x > morning))
            .count()
    };
    assert_eq!(after("first_seen"), 4);
    assert_eq!(after("done_at"), 11);
    // The task that waits for review.
    let t24 = tasks.iter().find(|t| t["id"] == "T024").unwrap();
    assert_eq!(t24["state"], "review");
    assert_eq!(t24["done"], false);
    assert!(t24["almost_secs"].as_u64().unwrap() > 0);
    assert_eq!(t24["branch"], "paddock/spawn-claude-75c54094");
    // Without records or merges nothing has evidence: the checkboxes are
    // claims, and gauge says so.
    assert_eq!(j["counts"]["claimed"], 28);
    assert_eq!(j["counts"]["proven"], 0);
    // At 03:30 the list was rewritten (7 tasks became 24): the measures
    // start there, and the end converges.
    assert_eq!(j["rewritten_at"], versions[1].0);
    assert_eq!(j["start"], versions[1].0);
    assert_eq!(j["forecast"]["verdict"], "converging", "{j}");
    let p85 = &j["forecast"]["p85"];
    assert!(p85["at"].as_u64().unwrap() > last && p85["total"].as_i64().unwrap() >= 29);
    // The whole history is seven hours: the steps are hours.
    assert!(j["window"]["width"].as_u64().unwrap() < DAY);
    // What the gauge said in the morning: the forecast that counted only
    // the 8 open tasks missed the 4 that came; the gauge at that time had
    // the discovery rate to warn of it.
    let then = w.json(&["--at", &(morning + 60).to_string()]);
    assert_eq!(then["counts"]["scope"], 25);
    assert_eq!(then["counts"]["open"], 8);
    assert!(
        then["forecast"]["arrival_per_day"].as_f64().unwrap() > 0.0,
        "{then}"
    );
    // One and a half hours of history: a labelled wide range, and a final
    // count above the 25 tasks of that time.
    assert_eq!(then["forecast"]["early"], true);
    assert!(
        then["forecast"]["high"]["total"].as_i64().unwrap() > 25,
        "{then}"
    );
    let text = w.ok(&["gauge", "--at", &at]);
    assert!(
        text.contains("done         28 of 29 tasks: 0 with evidence, 28 checked without evidence"),
        "{text}"
    );
    assert!(text.contains("T024 "), "{text}");
    assert_eq!(text, w.ok(&["gauge", "--at", &at]));
}

#[test]
fn a_plain_folder_degrades_and_starts_a_history() {
    let w = World::new(false, FEATURE);
    w.write_tasks(FEATURE, &tasks(4, 1));
    let text = w.ok(&["gauge"]);
    assert!(text.contains("(no history)"), "{text}");
    assert!(text.contains("paddock cannot know:"), "{text}");
    assert!(text.contains("no git repository"), "{text}");
    assert!(text.contains("burn-up: needs a history"), "{text}");
    let log = w
        .project()
        .join(".paddock/spec")
        .join(FEATURE)
        .join("history.jsonl");
    assert!(log.is_file(), "the state log starts");
    // A change later: the state log is now a history.
    w.write_tasks(FEATURE, &tasks(5, 3));
    let j = w.json(&[]);
    assert_eq!(j["sources"][0], "paddock's task state log");
    assert_eq!(j["snapshots"], 2);
    assert_eq!(j["counts"]["discovered"], 1);
    assert_eq!(j["start_known"], true);
    assert!(j["cannot_know"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str().unwrap().contains("first state log line")));
}

#[test]
fn appetite_and_gates_from_the_command_line() {
    let w = World::new(true, FEATURE);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    w.commit(FEATURE, &tasks(4, 0), now - 2 * DAY);
    w.commit(FEATURE, &tasks(4, 2), now - DAY);
    let out = w.ok(&["spec", "appetite", FEATURE, "3d"]);
    assert!(
        out.contains("appetite 3d") && out.contains("never stops"),
        "{out}"
    );
    assert!(w.ok(&["spec", "appetite", FEATURE]).contains("appetite 3d"));
    let text = w.ok(&["gauge"]);
    assert!(
        text.contains("appetite     3d: 2d 0h used (67 %)"),
        "{text}"
    );
    let (ok, _, err) = w.paddock(&["spec", "appetite", FEATURE, "soon"], &[]);
    assert!(!ok && err.contains("no appetite"), "{err}");
    // A gate that only skipped: the task is not done.
    let out = w.ok(&[
        "spec",
        "gate",
        "T001",
        "real-sandbox",
        "skip",
        "--note",
        "no bwrap",
    ]);
    assert!(out.contains("T001: gate real-sandbox skip"), "{out}");
    let j = w.json(&[]);
    let t1 = j["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T001")
        .unwrap()
        .clone();
    assert_eq!(t1["evidence"], "skipped-only");
    assert_eq!(j["counts"]["skipped_only"], 1);
    assert_eq!(j["forecast"]["open"], 3);
    w.ok(&["spec", "gate", "T001", "real-sandbox", "pass"]);
    let j = w.json(&[]);
    assert_eq!(j["counts"]["proven"], 1);
    assert_eq!(j["forecast"]["open"], 2);
    let (ok, _, err) = w.paddock(&["spec", "gate", "T001", "x", "maybe"], &[]);
    assert!(!ok && err.contains("pass, fail or skip"), "{err}");
    let (ok, _, err) = w.paddock(&["spec", "gate", "T001", "x", "pass"], &[("PADDOCK", "1")]);
    assert!(!ok && err.contains("host only"), "{err}");
    let hash = "a1".repeat(32);
    let args = [
        "spec",
        "gate",
        "T001",
        "remote",
        "pass",
        "--peer",
        "build_1",
        "--snapshot-hash",
        &hash,
    ];
    let (ok, _, err) = w.paddock(&args, &[("PADDOCK", "1")]);
    assert!(!ok && err.contains("host only"), "{err}");
    w.ok(&args);
    let gates = w
        .project()
        .join(".git/paddock/spec")
        .join(FEATURE)
        .join("gates.jsonl");
    let lines = std::fs::read_to_string(gates).unwrap();
    let record: serde_json::Value = serde_json::from_str(lines.lines().last().unwrap()).unwrap();
    assert_eq!(record["peer"], "build_1");
    assert_eq!(record["snapshot_hash"], hash);
    for bad in [
        vec![
            "spec",
            "gate",
            "T001",
            "x",
            "pass",
            "--peer",
            "bad/name",
            "--snapshot-hash",
            &hash,
        ],
        vec![
            "spec",
            "gate",
            "T001",
            "x",
            "pass",
            "--peer",
            "build_1",
            "--snapshot-hash",
            "bad",
        ],
        vec!["spec", "gate", "T001", "x", "pass", "--peer", "build_1"],
    ] {
        assert!(!w.paddock(&bad, &[]).0);
    }
    assert!(w
        .ok(&["spec", "appetite", FEATURE, "off"])
        .contains("no appetite"));
    assert!(!w.ok(&["gauge"]).contains("appetite "));
}

#[test]
fn a_branch_merged_into_head_is_evidence() {
    let w = World::new(true, FEATURE);
    w.commit(FEATURE, &tasks(2, 0), T0);
    w.git(&["checkout", "-q", "-b", "work-1"]);
    std::fs::write(w.project().join("code.txt"), "x").unwrap();
    w.git_at(&["add", "-A"], Some(T0 + 10));
    w.git_at(&["commit", "-q", "-m", "work"], Some(T0 + 10));
    w.git(&["checkout", "-q", "main"]);
    w.git(&["merge", "-q", "--no-ff", "-m", "merge", "work-1"]);
    let text = tasks(2, 2).replace("Work item 1", "Work item 1 <!-- paddock: work-1 -->");
    w.commit(FEATURE, &text, T0 + DAY);
    let j = w.json(&["--at", &(T0 + DAY + 60).to_string()]);
    let ev = |id: &str| {
        j["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .unwrap()["evidence"]
            .clone()
    };
    assert_eq!(ev("T001"), "proven", "{j}");
    assert_eq!(ev("T002"), "claimed");
}

#[test]
fn the_loft_history_is_read_on_the_host() {
    let w = World::new(true, FEATURE);
    w.commit(FEATURE, &tasks(3, 0), T0);
    w.ok(&["loft", "init", "--dir", "."]);
    w.ok(&["spec", "adopt", "--dir", ".", "--yes"]);
    let status: serde_json::Value =
        serde_json::from_str(&w.ok(&["spec", "status", "--json"])).unwrap();
    let store = PathBuf::from(status["store_path"].as_str().unwrap());
    assert!(store.join("specs").join(FEATURE).join("tasks.md").is_file());
    w.ok(&["loft", "commit", "-m", "one"]);
    std::fs::write(
        store.join("specs").join(FEATURE).join("tasks.md"),
        tasks(4, 2),
    )
    .unwrap();
    w.ok(&["loft", "commit", "-m", "two"]);
    let j = w.json(&[]);
    assert_eq!(j["sources"][0], "loft git history", "{j}");
    assert_eq!(j["counts"]["scope"], 4);
    assert_eq!(j["counts"]["done"], 2);
}

#[test]
fn a_sandbox_is_sent_to_paddock_host() {
    let w = World::new(false, FEATURE);
    w.write_tasks(FEATURE, &tasks(2, 1));
    let (ok, _, err) = w.paddock(&["gauge"], &[("PADDOCK", "1")]);
    assert!(!ok && err.contains("paddock-host gauge"), "{err}");
}
