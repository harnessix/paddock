//! A plain `cargo test` measures nothing: no benchmark, no timing, no
//! throughput, no memory sample, no load test. This test reads the test
//! sources and fails when a test that is not ignored has a measurement.
//!
//! A measurement is one of these patterns in a function body:
//!
//! * a memory sample or a statistic: `/proc/self/statm`, `VmRSS`,
//!   `VmHWM`, `MiB/s`, `p50`, `p95`, `wakeups`;
//! * a print macro whose arguments name a time: `elapsed`, `took`, `lag`,
//!   `as_millis`, `as_secs_f`, `duration_since`, `started_ms`;
//! * an assertion that a time is below `Duration::from_millis(..)`: a
//!   latency bound. A deadline of seconds for "it happens at all" is a
//!   behavior check and is allowed.
//!
//! The scan looks at every `#[test]` function, and at every function of
//! a test-only file (tests/, `*tests.rs`, `src/**/tests/`), because a
//! shared helper can measure for its callers. A measurement in an ignored
//! test is allowed when the ignore has a reason: run those on purpose
//! with `--ignored`. A helper of a file whose tests are all ignored is
//! allowed. The other exceptions are in [`ALLOWED`], each with a reason.

use std::path::{Path, PathBuf};

/// Helpers that measure for ignored tests: (file, function). The scan
/// fails when a test that is not ignored calls one of them by name.
const HELPERS: &[(&str, &str)] = &[
    ("src/devices/fuse_tests.rs", "resident"),
    ("tests/common/scenarios.rs", "resident"),
    ("tests/common/scenarios.rs", "measure_big_stream"),
];

/// Functions that match a pattern and measure nothing: (file, function,
/// reason). `*` is every function of the file.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "src/memory.rs",
        "fake_scope_counts_nested_processes_and_unavailable_values",
        "writes VmRSS into a fake /proc and reads it back",
    ),
    (
        "src/gauge/tests.rs",
        "*",
        "p50 of a fixed history: arithmetic on fixtures",
    ),
    (
        "src/macvm/bench_tests.rs",
        "*",
        "percentile and target math on fixed numbers",
    ),
    (
        "src/macvm/text/bench_tests.rs",
        "*",
        "text of fixed bench results",
    ),
    (
        "src/macvm/readme_tests.rs",
        "macvm_section_names_commands_tiers_and_numbers",
        "reads the README text",
    ),
];

/// This file: its fixtures hold the patterns on purpose.
const ME: &str = "tests/no_measurement.rs";

const MARKERS: &[&str] = &[
    "/proc/self/statm",
    "VmRSS",
    "VmHWM",
    "MiB/s",
    "p50",
    "p95",
    "wakeups",
];
const PRINTS: &[&str] = &["println!(", "eprintln!(", "print!(", "eprint!("];
const TIMES: &[&str] = &[
    "elapsed",
    "took",
    "lag",
    "as_millis",
    "as_secs_f",
    "duration_since",
    "started_ms",
];

/// A function of a source file.
#[derive(Debug)]
struct Func {
    name: String,
    line: usize,
    test: bool,
    /// The reason of `#[ignore = ".."]`, or "" for a bare `#[ignore]`.
    ignore: Option<String>,
    body: String,
}

/// True when `word` is in `text` with no letter, digit or `_` next to it.
fn has_word(text: &str, word: &str) -> bool {
    let edge = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    text.match_indices(word).any(|(i, _)| {
        edge(text[..i].chars().next_back()) && edge(text[i + word.len()..].chars().next())
    })
}

/// The text from `start` to the `)` that closes the macro or call.
fn call_args(text: &str, start: usize) -> &str {
    let mut depth = 0;
    for (i, c) in text[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[start..start + i];
                }
            }
            _ => {}
        }
    }
    &text[start..]
}

/// The first measurement in `body`, if it has one.
fn measurement(body: &str) -> Option<String> {
    if let Some(m) = MARKERS.iter().find(|m| has_word(body, m)) {
        return Some(format!("`{m}`"));
    }
    for p in PRINTS {
        for (i, _) in body.match_indices(p) {
            // `println!(` is also the tail of `eprintln!(`: only whole names.
            if body[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let args = call_args(body, i + p.len() - 1);
            if let Some(t) = TIMES.iter().find(|t| has_word(args, t)) {
                return Some(format!("a print of `{t}`"));
            }
        }
    }
    for (i, _) in body.match_indices("assert!(") {
        let args = call_args(body, i + "assert!".len());
        let timed = ["elapsed", "duration_since", "lag"]
            .iter()
            .any(|t| has_word(args, t));
        if timed && args.contains('<') && args.contains("Duration::from_millis(") {
            return Some("a latency bound below a second".into());
        }
    }
    None
}

/// The functions of `text`, found by rustfmt layout: a `fn` line and the
/// `}` line at its indentation. Comment lines are left out of a body.
fn functions(text: &str) -> Vec<Func> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (n, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let Some(at) = t.find("fn ") else { continue };
        let head = &t[..at];
        let head_ok = head
            .split_whitespace()
            .all(|w| w == "pub" || w.starts_with("pub(") || w == "async" || w == "const");
        if !head_ok || t.starts_with("//") {
            continue;
        }
        let name: String = t[at + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || t.ends_with(';') {
            continue;
        }
        let indent = &l[..l.len() - t.len()];
        let end = if t.ends_with('}') {
            n
        } else {
            let close = format!("{indent}}}");
            (n + 1..lines.len())
                .find(|&e| lines[e] == close)
                .unwrap_or(lines.len() - 1)
        };
        let body: Vec<&str> = lines[n..=end]
            .iter()
            .copied()
            .filter(|b| !b.trim_start().starts_with("//"))
            .collect();
        let mut test = false;
        let mut ignore = None;
        for a in lines[..n].iter().rev() {
            let a = a.trim();
            if a.starts_with("#[test]") || a.starts_with("#[tokio::test") {
                test = true;
            } else if let Some(r) = a.strip_prefix("#[ignore") {
                let reason = r.split('"').nth(1).unwrap_or("");
                ignore = Some(reason.to_string());
            } else if !(a.starts_with("#[") || a.starts_with("//")) {
                break;
            }
        }
        out.push(Func {
            name,
            line: n + 1,
            test,
            ignore,
            body: body.join("\n"),
        });
    }
    out
}

fn test_only(rel: &str) -> bool {
    rel.starts_with("tests/") || rel.ends_with("tests.rs") || rel.contains("/tests/")
}

/// The problems of one file, as `file:line function: what`.
fn problems(rel: &str, text: &str) -> Vec<String> {
    let funcs = functions(text);
    let tests: Vec<&Func> = funcs.iter().filter(|f| f.test).collect();
    let all_ignored = !tests.is_empty() && tests.iter().all(|f| f.ignore.is_some());
    let mut out = Vec::new();
    for f in &funcs {
        if !(f.test || test_only(rel)) {
            continue;
        }
        let Some(what) = measurement(&f.body) else {
            continue;
        };
        let allowed = ALLOWED
            .iter()
            .any(|(file, name, _)| *file == rel && (*name == "*" || *name == f.name))
            || HELPERS
                .iter()
                .any(|(file, name)| *file == rel && *name == f.name);
        let fine = match &f.ignore {
            Some(reason) => !reason.is_empty(),
            None => allowed || (!f.test && all_ignored),
        };
        if !fine {
            let fix = if f.ignore.is_some() {
                "give the #[ignore] a reason that names the measurement"
            } else {
                "remove the measurement or add #[ignore = \"measurement: ...\"]"
            };
            out.push(format!("{rel}:{} {}: {what}; {fix}", f.line, f.name));
        }
    }
    out
}

/// The callers of a measuring helper that are neither ignored tests nor
/// helpers of [`HELPERS`].
fn helper_callers(all: &[(String, Func)]) -> Vec<String> {
    let mut out = Vec::new();
    for (_, helper) in HELPERS {
        let call = format!("{helper}(");
        for (rel, f) in all {
            let calls = f.body.match_indices(&call).any(|(i, _)| {
                let before = f.body[..i].chars().next_back();
                !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                    && !f.body[..i].ends_with("fn ")
            });
            let listed = HELPERS
                .iter()
                .any(|(file, name)| file == rel && *name == f.name);
            let ignored = f.ignore.as_ref().is_some_and(|r| !r.is_empty());
            if calls && !listed && !ignored {
                out.push(format!(
                    "{rel}:{} {}: calls the measuring helper {helper}; add #[ignore = \"measurement: ...\"]",
                    f.line, f.name
                ));
            }
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_test_that_runs_by_default_measures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    files.sort();
    let mut found = Vec::new();
    let mut all = Vec::new();
    for p in &files {
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        if rel == ME {
            continue;
        }
        let text = std::fs::read_to_string(p).unwrap();
        found.extend(problems(&rel, &text));
        all.extend(functions(&text).into_iter().map(|f| (rel.clone(), f)));
    }
    found.extend(helper_callers(&all));
    assert!(
        found.is_empty(),
        "tests that measure in a plain `cargo test`:\n{}",
        found.join("\n")
    );
    let named = ALLOWED
        .iter()
        .map(|(f, _, _)| f)
        .chain(HELPERS.iter().map(|(f, _)| f));
    for file in named {
        assert!(
            root.join(file).exists(),
            "ALLOWED names {file}, which is gone"
        );
    }
}

#[test]
fn the_scan_finds_each_kind_of_measurement() {
    let bad = [
        "let s = std::fs::read_to_string(\"/proc/self/statm\");",
        "eprintln!(\"{} MiB/s\", rate);",
        "eprintln!(\"round trip: {:?}\", t.elapsed());",
        "eprintln!(\n        \"start: {} ms\",\n        started_ms(&out)\n    );",
        "assert!(lag < Duration::from_millis(50), \"{lag:?}\");",
        "let p50 = times[4];",
    ];
    for b in bad {
        let text = format!("#[test]\nfn t() {{\n    {b}\n}}\n");
        assert_eq!(problems("tests/x.rs", &text).len(), 1, "not found: {b}");
        let ignored = format!("#[test]\n#[ignore = \"measurement: x\"]\nfn t() {{\n    {b}\n}}\n");
        assert!(problems("tests/x.rs", &ignored).is_empty(), "{b}");
        let bare = format!("#[test]\n#[ignore]\nfn t() {{\n    {b}\n}}\n");
        assert_eq!(problems("tests/x.rs", &bare).len(), 1, "no reason: {b}");
        let helper = format!("fn h() {{\n    {b}\n}}\n");
        assert_eq!(problems("tests/x.rs", &helper).len(), 1, "helper: {b}");
        assert!(problems("src/x.rs", &helper).is_empty(), "product: {b}");
    }
    let good = [
        "assert!(t.elapsed() < Duration::from_secs(5), \"no cancel\");",
        "eprintln!(\"skipped: {why}\");",
        "// eprintln!(\"took {:?}\", t.elapsed());",
        "let p500 = 1; let top_p50_at = 2;",
    ];
    for g in good {
        let text = format!("#[test]\nfn t() {{\n    {g}\n}}\n");
        assert!(problems("tests/x.rs", &text).is_empty(), "found: {g}");
    }
}
