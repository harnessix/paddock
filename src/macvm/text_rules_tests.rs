//! The rules over every text: every function has a test, no user text
//! outside text.rs, the seed covers a first login, and the Mac script of
//! the first run matches these texts.

use super as t;
use super::tests::{GIB, SIZE};
use super::{SetupPhase, Share, StartKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---- The rules -----------------------------------------------------------

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The names of the top-level `pub fn` of a source text.
fn pub_fns(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|l| l.strip_prefix("pub fn "))
        .map(|l| l.split(['(', '<']).next().unwrap().to_string())
        .collect()
}

/// The names of the `#[test]` functions of a source text.
fn test_fns(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    lines
        .windows(2)
        .filter(|w| w[0].trim() == "#[test]")
        .filter_map(|w| w[1].trim().strip_prefix("fn "))
        .map(|l| l.split('(').next().unwrap().to_string())
        .collect()
}

/// The files of the text module: `(sources, test files)`. The sources are
/// text.rs and every non-test file under src/macvm/text/; the tests are
/// text_tests.rs, text_more_tests.rs and every `*_tests.rs` under
/// src/macvm/text/.
fn text_files() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut sources = vec![root().join("src/macvm/text.rs")];
    let mut tests = vec![
        root().join("src/macvm/text_tests.rs"),
        root().join("src/macvm/text_more_tests.rs"),
    ];
    let mut children = Vec::new();
    rust_files(&root().join("src/macvm/text"), &mut children);
    for f in children {
        if f.to_string_lossy().ends_with("_tests.rs") {
            tests.push(f);
        } else {
            sources.push(f);
        }
    }
    (sources, tests)
}

/// Each source file of the text module has every `pub fn` tested by a
/// test of the same name in the test files, and no two sources share a
/// function name (every child is re-exported into `text`).
#[test]
fn every_function_has_a_test() {
    let (sources, tests) = text_files();
    assert!(sources.len() >= 3, "{sources:?}");
    let tests: Vec<String> = tests
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let names = test_fns(&tests.join("\n"));
    let mut all = Vec::new();
    let mut total = 0;
    for src in &sources {
        let fns = pub_fns(&std::fs::read_to_string(src).unwrap());
        let missing: Vec<&String> = fns.iter().filter(|f| !names.contains(f)).collect();
        assert!(
            missing.is_empty(),
            "{} functions with no test of their name: {missing:?}",
            src.display()
        );
        total += fns.len();
        for f in fns {
            assert!(!all.contains(&f), "{f} is in two text files");
            all.push(f);
        }
    }
    assert!(total > 60, "{all:?}");
    assert!(
        names.len() >= total,
        "{} tests for {} functions",
        names.len(),
        total
    );
}

/// The macros whose string literal reaches the user.
const MACROS: [&str; 8] = [
    "println!(",
    "eprintln!(",
    "print!(",
    "eprint!(",
    "format!(",
    "bail!(",
    "anyhow!(",
    ".context(",
];

/// The places of user text in a source file: `(line, literal)` of each
/// macro above whose first argument is a literal with a space outside the
/// `{}` holes, and of each print macro with any literal. A line that ends
/// with `// not user text` is a log key or a path and passes.
fn user_texts(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for mac in MACROS {
        let mut from = 0;
        while let Some(at) = src[from..].find(mac) {
            let start = from + at;
            from = start + mac.len();
            let line_no = src[..start].matches('\n').count() + 1;
            let line = src.lines().nth(line_no - 1).unwrap_or("");
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || line.trim_end().ends_with("// not user text") {
                continue;
            }
            // `println!(` is also the tail of `eprintln!(`: count it once.
            if mac.starts_with("print") && src[..start].ends_with('e') {
                continue;
            }
            let rest = src[from..].trim_start();
            let Some(lit) = rest.strip_prefix('"') else {
                continue;
            };
            let mut text = String::new();
            let mut esc = false;
            for c in lit.chars() {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    break;
                }
                text.push(c);
            }
            let print = mac.contains("print");
            let mut bare = String::new();
            let mut depth = 0;
            for c in text.chars() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ if depth == 0 => bare.push(c),
                    _ => {}
                }
            }
            if print && !bare.trim().is_empty() || bare.contains(' ') {
                out.push((line_no, text));
            }
        }
    }
    out
}

#[test]
fn user_texts_finds_the_forms() {
    let src = r#"
let a = format!("{name}.partial");
let b = format!("slot-{n}");
let c = format!("the copy failed: {e}");
eprintln!("{}", t);
eprintln!("stopped");
bail!(
    "two VMs run"
);
let d = format!("a b"); // not user text
// println!("in a comment");
x.context("open the stamp")?;
"#;
    let mut found: Vec<usize> = user_texts(src).into_iter().map(|(l, _)| l).collect();
    found.sort();
    assert_eq!(found, [4, 6, 7, 12]);
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The files that may hold user text: text.rs and its children. Test
/// files (`*_tests.rs`) pass too: they hold expected texts and assert
/// messages.
fn allowed(rel: &str) -> bool {
    rel == "src/macvm/text.rs" || rel.starts_with("src/macvm/text/")
}

#[test]
fn no_user_text_outside_text_rs() {
    let mut files = Vec::new();
    rust_files(&root().join("src/macvm"), &mut files);
    rust_files(&root().join("src/guest"), &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("macvm/text.rs")),
        "{files:?}"
    );
    let mut bad = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if allowed(&rel) || rel.ends_with("_tests.rs") || rel.ends_with("/tests.rs") {
            continue;
        }
        let src = std::fs::read_to_string(&f).unwrap();
        for (line, text) in user_texts(&src) {
            bad.push(format!("{rel}:{line}: {text:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "user text outside src/macvm/text.rs (add a function there and call it):\n{}",
        bad.join("\n")
    );
}

/// The hosts of a first ChatGPT login: `(host, port)`.
fn first_login() -> Vec<(String, u16)> {
    #[derive(serde::Deserialize)]
    struct Fixture {
        hosts: Vec<Conn>,
    }
    #[derive(serde::Deserialize)]
    struct Conn {
        host: String,
        port: u16,
    }
    let f: Fixture = toml::from_str(include_str!("testdata/chatgpt-first-login.toml")).unwrap();
    f.hosts.into_iter().map(|c| (c.host, c.port)).collect()
}

/// A seed glob against a host: `x.y` is the name itself, `*.x.y` every
/// name under it (not x.y). The firewall matches the same way.
fn seed_matches(seed: &str, host: &str) -> bool {
    match seed.strip_prefix("*.") {
        Some(base) => host
            .strip_suffix(base)
            .is_some_and(|head| head.ends_with('.') && head.len() > 1),
        None => seed.eq_ignore_ascii_case(host),
    }
}

#[test]
fn first_login_hosts_are_seeded() {
    let p = crate::profile::builtin("chatgpt-desktop").unwrap();
    let seeds = p.app.unwrap().macos.unwrap().seed_hosts;
    assert!(!seeds.is_empty());
    for s in &seeds {
        assert!(
            s != "*" && !s.contains(':') && !s.ends_with('.'),
            "seed {s:?}"
        );
    }
    let hosts = first_login();
    assert!(hosts.len() >= 5, "{hosts:?}");
    for (host, port) in hosts {
        assert_eq!(port, 443, "{host}: a seed is for port 443 only");
        assert!(
            seeds.iter().any(|s| seed_matches(s, &host)),
            "{host} of a first login has no seed in profiles/chatgpt-desktop.toml: the first run would ask"
        );
    }
    assert!(seed_matches("*.openai.com", "auth.openai.com"));
    assert!(!seed_matches("*.openai.com", "openai.com"));
    assert!(!seed_matches("*.openai.com", "evilopenai.com"));
    assert!(!seed_matches("chatgpt.com", "x.chatgpt.com"));
}

/// The output of each text function for the Mac script's check lines.
fn sample(name: &str) -> Option<String> {
    let dl = Path::new("/Users/me/Library/Application Support/paddock/macvm/profiles/chatgpt-desktop/default/downloads");
    let web = PathBuf::from("/Users/me/src/web");
    Some(match name {
        "no_golden" => t::no_golden("ChatGPT", SIZE, 30),
        "fetch_question" => t::fetch_question("https://x/y.ipsw", "15.2", SIZE, 30),
        "progress" => t::progress(SetupPhase::Download, 5, Some(Duration::from_secs(600))),
        "golden_ready" => t::golden_ready("15.2", 23 * GIB),
        "start_backend" => t::start_backend("app"),
        "start_memory" => t::start_memory("chatgpt-desktop", 4),
        "start_display" => t::start_display(3024, 1964, 2, false),
        "start_shares" => t::start_shares(
            "chatgpt-desktop",
            &[Share {
                path: &web,
                writable: true,
            }],
        ),
        "start_downloads" => t::start_downloads(dl),
        "start_network" => t::start_network("egress"),
        "start_kind" => [StartKind::Cold, StartKind::Warm]
            .map(|k| t::start_kind(k, 6, None))
            .join("\n"),
        "start_copy" => t::start_copy("ChatGPT", 20),
        "start_continues" => t::start_continues("chatgpt-desktop"),
        "configure_wrong_password" => t::configure_wrong_password(),
        "password_replaced" => t::password_replaced(),
        "seed_offline_done" => t::seed_offline_done(9, &crate::macvm::diskimage::Fsck::Clean),
        "trust_reported" => t::trust_reported(),
        _ => return None,
    })
}

/// The Mac script checks the terminal for fixed parts of these texts. Its
/// lines `want CHECK FUNCTION 'PART'` must name a part that the function
/// prints, so a change of a text here fails on Linux, not on the Mac.
#[test]
fn first_run_script_matches_texts() {
    let script =
        std::fs::read_to_string(root().join("packaging/macos/checks/macvm-first-run.sh")).unwrap();
    let fns: Vec<String> = text_files()
        .0
        .iter()
        .flat_map(|f| pub_fns(&std::fs::read_to_string(f).unwrap()))
        .collect();
    let mut n = 0;
    for line in script
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("want "))
    {
        let mut parts = line.splitn(4, ' ');
        let (_, _check, func, part) = (
            parts.next(),
            parts.next(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let part = part.trim_matches('\'');
        assert!(
            fns.iter().any(|f| f == func),
            "script names no text.rs function {func}"
        );
        let out = sample(func).unwrap_or_else(|| panic!("no sample of {func} in text_tests.rs"));
        assert!(
            out.contains(part),
            "the script wants {part:?} from {func}, which prints {out:?}"
        );
        n += 1;
    }
    for f in [
        "start_backend",
        "start_memory",
        "start_display",
        "start_shares",
        "start_downloads",
        "start_network",
        "start_kind",
        "start_copy",
        "start_continues",
    ] {
        assert!(
            script.contains(&format!(" {f} '")),
            "the script does not check the start line {f}"
        );
    }
    assert!(n >= 12, "{n} checks");
}
