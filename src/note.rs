//! One way to print a note of paddock on the run path (`paddock run`,
//! `paddock HARNESS`, `paddock shell`): a line on stderr with a level.
//!
//! - `-q` hides info and warnings, and keeps errors. `-qq` also hides
//!   errors: only a fatal error (the error that `main` prints) stays.
//! - A note that the user must act on, such as an approval or a question,
//!   has the level [`Level::Ask`]. `-q` and `-qq` never hide it.
//! - Without `-q`, the line is as before: `paddock: TEXT`, or
//!   `paddock: warning: TEXT` for a warning.
//! - Hidden or not, a run keeps its notes (`record`). After the run
//!   registers, they go to `notes.json` in its run folder, and
//!   `paddock ps --json` shows them as `run_notes`, each with `level` and
//!   `text`. The launcher shows them in the Details of the run.
//!
//! Warnings and errors of the `log` macros (logging.rs) go to the record
//! too, so a `log::warn!` of the run path is a note as well.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

/// The field of `paddock ps --json` that holds the notes of a run.
pub const PS_FIELD: &str = "run_notes";
/// The file in the run folder (`paddock_run()/ID/`) that holds them.
pub const FILE: &str = "notes.json";
/// A run keeps at most this many notes.
const MAX: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warning,
    Error,
    /// The user must act on it: an approval, a question, a password.
    /// Never hidden.
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub level: Level,
    pub text: String,
}

struct Record {
    on: bool,
    notes: Vec<Note>,
    file: Option<PathBuf>,
}

static QUIET: AtomicU8 = AtomicU8::new(0);
static RECORD: Mutex<Record> = Mutex::new(Record {
    on: false,
    notes: Vec::new(),
    file: None,
});

/// The number of `-q` on the command line. Call once, early in `main`.
pub fn set_quiet(count: u8) {
    QUIET.store(count, Ordering::Relaxed);
}

/// Whether a note of `level` shows on stderr with `quiet` times `-q`.
pub fn shown(level: Level, quiet: u8) -> bool {
    match level {
        Level::Ask => true,
        Level::Error => quiet < 2,
        Level::Info | Level::Warning => quiet == 0,
    }
}

/// The stderr line of a note, as paddock printed it before this module.
pub fn line(level: Level, text: &str) -> String {
    match level {
        Level::Info | Level::Ask => format!("paddock: {text}"),
        Level::Warning => format!("paddock: warning: {text}"),
        Level::Error => format!("paddock: error: {text}"),
    }
}

/// One line on stderr. A failed write is dropped: `eprintln!` panics on
/// it, and a closed terminal (EIO) or a closed pipe (EPIPE) must not end
/// the run before its end steps.
pub fn err_line(line: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}

/// Print a note (unless `-q` hides it), and keep it in the record.
pub fn say(level: Level, text: impl Into<String>) {
    let text = text.into();
    if shown(level, QUIET.load(Ordering::Relaxed)) {
        err_line(&line(level, &text));
    }
    crate::logging::journal(level_of(level), "paddock::note", &text);
    keep(level, text);
}

/// A line that already has its form (`paddock: ...`, or a line of a
/// list): printed as it is, with the filter of `level`. The record keeps
/// it without the `paddock: ` prefix.
pub fn say_line(level: Level, line: &str) {
    if shown(level, QUIET.load(Ordering::Relaxed)) {
        err_line(line);
    }
    let line = line.trim_start_matches('\n');
    let text = line.strip_prefix("paddock: ").unwrap_or(line);
    crate::logging::journal(level_of(level), "paddock::note", text);
    keep(level, text.to_string());
}

/// One note of `level` for each text.
pub fn each(level: Level, texts: &[String]) {
    texts.iter().for_each(|t| say(level, t.as_str()));
}

pub fn info(text: impl Into<String>) {
    say(Level::Info, text)
}

pub fn warn(text: impl Into<String>) {
    say(Level::Warning, text)
}

pub fn error(text: impl Into<String>) {
    say(Level::Error, text)
}

/// A note that the user must act on. `-q` never hides it.
pub fn ask(text: impl Into<String>) {
    say(Level::Ask, text)
}

fn level_of(level: Level) -> log::Level {
    match level {
        Level::Info | Level::Ask => log::Level::Info,
        Level::Warning => log::Level::Warn,
        Level::Error => log::Level::Error,
    }
}

/// From now on, this process keeps its notes: a run starts.
pub fn record() {
    if let Ok(mut r) = RECORD.lock() {
        r.on = true;
    }
}

/// A warning or an error of the `log` macros, for the record.
pub(crate) fn keep_log(level: log::Level, text: &str) {
    let level = match level {
        log::Level::Error => Level::Error,
        log::Level::Warn => Level::Warning,
        _ => return,
    };
    keep(level, text.to_string());
}

fn keep(level: Level, text: String) {
    let Ok(mut r) = RECORD.lock() else {
        return;
    };
    if !r.on || r.notes.len() >= MAX {
        return;
    }
    r.notes.push(Note { level, text });
    if let Some(f) = r.file.clone() {
        write(&f, &r.notes);
    }
}

/// The run registered: its notes go to `notes.json` in `run_dir`, now and
/// at each new note.
pub fn attach(run_dir: &Path) {
    let Ok(mut r) = RECORD.lock() else {
        return;
    };
    if !r.on {
        return;
    }
    let f = run_dir.join(FILE);
    write(&f, &r.notes);
    r.file = Some(f);
}

fn write(f: &Path, notes: &[Note]) {
    if let Ok(bytes) = serde_json::to_vec(notes) {
        if let Err(e) = crate::privfs::write_atomic(f, bytes) {
            log::debug!("cannot write the notes of the run: {e}");
        }
    }
}

/// The notes of a run, from its run folder. Empty when it has none.
pub fn load(run_dir: &Path) -> Vec<Note> {
    std::fs::read(run_dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// The notes that this process kept, for tests.
pub fn kept() -> Vec<Note> {
    RECORD.lock().map(|r| r.notes.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hides_info_and_warnings_and_keeps_errors() {
        assert!(shown(Level::Info, 0));
        assert!(shown(Level::Warning, 0));
        assert!(!shown(Level::Info, 1));
        assert!(!shown(Level::Warning, 1));
        assert!(shown(Level::Error, 1));
        assert!(!shown(Level::Error, 2));
    }

    #[test]
    fn a_note_to_act_on_is_never_hidden() {
        for q in 0..4 {
            assert!(shown(Level::Ask, q));
        }
    }

    #[test]
    fn lines_keep_the_old_form() {
        assert_eq!(line(Level::Info, "x"), "paddock: x");
        assert_eq!(line(Level::Ask, "x"), "paddock: x");
        assert_eq!(line(Level::Warning, "x"), "paddock: warning: x");
        assert_eq!(line(Level::Error, "x"), "paddock: error: x");
    }

    #[test]
    fn notes_serialize_with_level_and_text() {
        let n = Note {
            level: Level::Warning,
            text: "t".into(),
        };
        assert_eq!(
            serde_json::to_string(&n).unwrap(),
            r#"{"level":"warning","text":"t"}"#
        );
    }

    #[test]
    fn the_record_goes_to_the_run_folder() {
        let tmp = tempfile::tempdir().unwrap();
        record();
        set_quiet(1);
        info("hidden but kept");
        error("an error");
        attach(tmp.path());
        warn("after the start");
        set_quiet(0);
        let on_disk = load(tmp.path());
        for t in ["hidden but kept", "an error", "after the start"] {
            assert!(on_disk.iter().any(|n| n.text == t), "{t}: {on_disk:?}");
        }
        assert!(on_disk
            .iter()
            .any(|n| n.level == Level::Error && n.text == "an error"));
        assert_eq!(kept().len(), on_disk.len());
    }
}
