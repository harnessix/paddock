//! The `macvm` section of `paddock doctor --report`: the doctor block, the
//! manifest, each stamp, the last 200 lines of each log of each profile,
//! the firewall rules files and the counts of the connection logs by
//! verdict. Every byte goes through the redaction of `report.rs` (user
//! name, host name, home path, variable values, tokens) plus this module's
//! own passes: a 32-hex run on a line with `token`, every line with a
//! password, and `--no-hosts`, which turns every host name of the rules
//! files and the connection logs into `host-N`.
//!
//! The reading is tolerant: a missing file is one `missing:` line and
//! never an error, because the bundle is for the case where things went
//! wrong.

use super::layout::Layout;
use super::session::Profile;
use super::status::{self, Counts};
use super::text;
use crate::env::HostEnv;
use crate::report::Redactor;
use std::path::{Path, PathBuf};

/// The lines of each log in the bundle.
pub const TAIL_LINES: usize = 200;
/// The longest file that goes in whole (the manifest, a stamp, a rules
/// file). Longer files are cut with a note.
const WHOLE_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Replace every host name of the rules files and the connection
    /// logs with `host-N`.
    pub no_hosts: bool,
    /// The lines of each log.
    pub tail: usize,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            no_hosts: false,
            tail: TAIL_LINES,
        }
    }
}

/// The firewall rules file of an app profile:
/// `~/.config/paddock/firewall/<app>.toml`.
pub fn rules_file(env: &HostEnv, app: &str) -> PathBuf {
    env.paddock_config()
        .join("firewall")
        .join(format!("{}.toml", super::layout::safe_name(app)))
}

/// The section for `paddock doctor --report`, redacted.
pub fn macvm_section(env: &HostEnv, opts: &Opts) -> String {
    let layout = Layout::new(env);
    let host_version = crate::onboard::MacProbe::host().version;
    let view = crate::doctor::macvm::View::read(env, host_version.as_deref());
    let doctor = crate::doctor::macvm::lines(&view);
    let raw = gather(env, &layout, &doctor, opts);
    redact(
        &raw.text,
        &Redactor::from_env(env),
        &raw.hosts,
        opts.no_hosts,
    )
}

/// The section before the redaction, and every host name it saw.
#[derive(Debug, Clone, Default)]
pub struct Raw {
    pub text: String,
    pub hosts: Vec<String>,
}

/// Read every file of the section. `doctor` is the `macvm` block of
/// `paddock doctor`, already built.
pub fn gather(env: &HostEnv, layout: &Layout, doctor: &[String], opts: &Opts) -> Raw {
    let mut out = String::new();
    let mut hosts = Vec::new();
    for l in doctor {
        out.push_str(l);
        out.push('\n');
    }
    out.push('\n');
    push_whole(&mut out, "manifest", &layout.golden.manifest);
    for (app, home, dir) in layout.profile_dirs() {
        let profile = Profile::new(&dir);
        out.push('\n');
        out.push_str(&text::report_profile_heading(&app, &home, &dir));
        out.push('\n');
        push_whole(&mut out, "stamp", &profile.stamp());
        for which in super::logs::Which::ALL {
            push_tail(&mut out, &which.path(&profile), opts.tail);
        }
        let rules = rules_file(env, &app);
        match std::fs::read(&rules) {
            Ok(bytes) => {
                let t = String::from_utf8_lossy(&bytes).into_owned();
                for h in hosts_in(&t) {
                    if !hosts.contains(&h) {
                        hosts.push(h);
                    }
                }
                out.push_str(&text::report_file_heading("rules", &rules));
                out.push('\n');
                out.push_str(&cut(&t));
                out.push('\n');
            }
            Err(_) => {
                out.push_str(&text::report_missing(&rules));
                out.push('\n');
            }
        }
        let connections = profile.connections_log();
        match std::fs::read(&connections) {
            Ok(bytes) => {
                let counts: Counts = status::count_connections(&bytes);
                for h in &counts.hosts {
                    if !hosts.contains(h) {
                        hosts.push(h.clone());
                    }
                }
                out.push_str(&text::report_file_heading("connections", &connections));
                out.push('\n');
                out.push_str(&text::report_connections(&counts.by_verdict()));
                out.push('\n');
            }
            Err(_) => {
                out.push_str(&text::report_missing(&connections));
                out.push('\n');
            }
        }
    }
    Raw { text: out, hosts }
}

fn push_whole(out: &mut String, kind: &str, path: &Path) {
    match std::fs::read(path) {
        Ok(bytes) => {
            out.push_str(&text::report_file_heading(kind, path));
            out.push('\n');
            out.push_str(&cut(&String::from_utf8_lossy(&bytes)));
            out.push('\n');
        }
        Err(_) => {
            out.push_str(&text::report_missing(path));
            out.push('\n');
        }
    }
}

fn push_tail(out: &mut String, path: &Path, n: usize) {
    match super::logs::tail(path, n) {
        Some(lines) => {
            out.push_str(&text::report_tail_heading(path, n));
            out.push('\n');
            for l in lines {
                out.push_str(&l);
                out.push('\n');
            }
        }
        None => {
            out.push_str(&text::report_missing(path));
            out.push('\n');
        }
    }
}

fn cut(t: &str) -> String {
    if t.len() <= WHOLE_MAX {
        return t.trim_end().to_string();
    }
    let mut end = WHOLE_MAX;
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut at {WHOLE_MAX} bytes]", t[..end].trim_end()) // not user text
}

/// The redaction of the section: the report's redactor, then the token
/// pattern, then the password lines, then the hosts behind `--no-hosts`.
pub fn redact(text: &str, r: &Redactor, hosts: &[String], no_hosts: bool) -> String {
    let mut t = r.redact(text);
    t = t
        .lines()
        .map(|l| {
            let lower = l.to_ascii_lowercase();
            if lower.contains("password") || lower.contains("passwd") {
                text::report_password_line()
            } else if lower.contains("token") {
                hide_hex_runs(l)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.ends_with('\n') {
        t.push('\n');
    }
    if no_hosts {
        let mut hosts: Vec<&String> = hosts.iter().collect();
        hosts.sort_by_key(|h| std::cmp::Reverse(h.len()));
        let mut n = 0;
        for h in hosts {
            if h.len() < 3 {
                continue;
            }
            let with = format!("host-{}", n + 1); // not user text
            let (replaced, found) = replace_ci(&t, h, &with);
            if found {
                t = replaced;
                n += 1;
            }
        }
        t = format!("{}\n{t}", text::report_no_hosts_note(n));
    }
    t
}

/// Replace every occurrence of `word` in `text`, whatever its case.
/// `(text, found)`.
fn replace_ci(text: &str, word: &str, with: &str) -> (String, bool) {
    let lower = text.to_ascii_lowercase();
    let word = word.to_ascii_lowercase();
    if word.is_empty() || !lower.contains(&word) {
        return (text.to_string(), false);
    }
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(i) = lower[from..].find(&word) {
        out.push_str(&text[from..from + i]);
        out.push_str(with);
        from += i + word.len();
    }
    out.push_str(&text[from..]);
    (out, true)
}

/// Every run of 32 or more hex digits in the line becomes `<token>`.
pub fn hide_hex_runs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut run = String::new();
    let flush = |out: &mut String, run: &mut String| {
        if run.len() >= 32 {
            out.push_str("<token>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in line.chars() {
        if c.is_ascii_hexdigit() {
            run.push(c);
        } else {
            flush(&mut out, &mut run);
            out.push(c);
        }
    }
    flush(&mut out, &mut run);
    out
}

/// The host names of a rules file or any text: every word with a dot
/// whose last part is letters (a domain) or that is four numbers (an
/// address), and that is not a file name. `*.example.com` gives
/// `example.com`.
pub fn hosts_in(text: &str) -> Vec<String> {
    const FILE_EXT: &[&str] = &[
        "toml", "json", "jsonl", "log", "img", "vzs", "app", "partial", "txt", "md", "rs", "sh",
        "plist", "sock", "ipsw", "icns", "html",
    ];
    let mut out: Vec<String> = Vec::new();
    for word in
        text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*')))
    {
        let w = word
            .trim_matches(|c| c == '.' || c == '*')
            .to_ascii_lowercase();
        if !w.contains('.') || w.len() < 3 || out.contains(&w) {
            continue;
        }
        let parts: Vec<&str> = w.split('.').collect();
        if parts.iter().any(|p| p.is_empty()) {
            continue;
        }
        let last = parts[parts.len() - 1];
        let ipv4 = parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok());
        let domain = last.chars().all(|c| c.is_ascii_alphabetic()) && last.len() >= 2;
        if FILE_EXT.contains(&last) || !(ipv4 || domain) {
            continue;
        }
        if w.starts_with('-') || w.contains("--") {
            continue;
        }
        out.push(w);
    }
    out
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
