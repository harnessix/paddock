//! Codex's `notify` program as the safe-point signal of an interactive
//! Codex run in horse mode.
//!
//! Codex runs the `notify` command of its `config.toml` after each turn,
//! with one JSON argument (`agent-turn-complete`, the thread id and the
//! turn id). In horse mode paddock sets it in the sandbox copy of
//! `config.toml` to `paddock-host horse safe-point stop`, which tells the
//! host (hostcli/safe_point.rs). A `notify` of the user stays: it goes
//! after `--then`, and paddock's program runs it afterwards with the same
//! argument. The host copy of the file never changes, and outside horse
//! mode paddock writes no `notify`.

use crate::spec::HomeSetting;
use serde_json::Value;

/// Codex's configuration file, relative to the sandbox home.
pub const CONFIG: &str = ".codex/config.toml";
/// paddock's `notify` command.
pub const PROGRAM: [&str; 4] = ["paddock-host", "horse", "safe-point", "stop"];
/// What separates paddock's command from the user's.
pub const THEN: &str = "--then";

/// The `notify` list: `ours`, then the user's command `had` after
/// [`THEN`]. A list that is paddock's already (from an earlier start)
/// gives back the user's part only once.
pub fn chain(ours: &Value, had: Option<Vec<String>>) -> Vec<String> {
    let ours: Vec<String> = ours
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut user = had.unwrap_or_default();
    if user.starts_with(&ours) {
        user.drain(..ours.len());
        if user.first().map(String::as_str) == Some(THEN) {
            user.remove(0);
        }
    }
    let mut out = ours;
    if !user.is_empty() {
        out.push(THEN.into());
        out.extend(user);
    }
    out
}

/// Why the codex command cannot get paddock's `notify`: it sets `notify`
/// itself (`-c notify=...`), which wins over `config.toml`. `None` when
/// it can.
pub fn conflict(command: &[String]) -> Option<String> {
    let mut args = command.iter().skip(1);
    while let Some(a) = args.next() {
        let value = match a.as_str() {
            "-c" | "--config" => args.next().map(String::as_str),
            _ => a
                .strip_prefix("--config=")
                .or_else(|| a.strip_prefix("-c="))
                .or_else(|| a.strip_prefix("-c").filter(|v| !v.is_empty())),
        };
        let key = value.and_then(|v| v.split('=').next()).map(str::trim);
        if key == Some("notify") {
            return Some(
                "paddock gets the safe points from Codex's notify program, and the codex command sets notify".into(),
            );
        }
    }
    None
}

/// The setting that puts paddock's `notify` into the sandbox copy of the
/// Codex configuration.
pub fn setting() -> HomeSetting {
    HomeSetting {
        file: CONFIG.into(),
        format: "toml".into(),
        path: vec!["notify".into()],
        value: serde_json::json!(PROGRAM),
        origin: "horse mode: the safe points of the Codex TUI".into(),
        append: false,
        codex_trust: None,
        no_pty: false,
        chain: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> String {
        let s = setting();
        crate::privacy::set_toml(text, &[&s]).unwrap()
    }

    fn notify(text: &str) -> Option<Vec<String>> {
        let v: toml::Table = toml::from_str(text).unwrap();
        v.get("notify").map(|n| {
            n.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect()
        })
    }

    #[test]
    fn notify_is_paddocks_program_in_a_file_without_one() {
        let out = write("model = \"gpt-5.5\"\n");
        assert_eq!(notify(&out).unwrap(), PROGRAM);
        assert!(out.contains("model = \"gpt-5.5\""), "{out}");
    }

    #[test]
    fn a_user_notify_runs_after_paddocks_program() {
        let out = write("notify = [\"notify-send\", \"Codex\"]\n[projects]\n");
        let mut want: Vec<String> = PROGRAM.iter().map(|s| s.to_string()).collect();
        want.extend([THEN.into(), "notify-send".into(), "Codex".into()]);
        assert_eq!(notify(&out).unwrap(), want);
        // A second start keeps one chain.
        assert_eq!(notify(&write(&out)).unwrap(), want);
    }

    #[test]
    fn a_command_that_sets_notify_itself_conflicts() {
        let c = |a: &[&str]| conflict(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(c(&["codex", "-c", "notify=[\"x\"]"]).is_some());
        assert!(c(&["codex", "--config=notify = []"]).is_some());
        assert!(c(&["codex", "-cnotify=[]"]).is_some());
        assert!(c(&["codex", "-c", "model=\"m\""]).is_none());
        assert!(c(&["codex", "--dangerously-bypass-approvals-and-sandbox"]).is_none());
        assert!(c(&["codex", "notify"]).is_none());
    }

    #[test]
    fn no_notify_is_written_outside_horse_mode() {
        // The settings of a run outside horse mode never hold this one:
        // only the horse run setup adds it.
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(".codex")).unwrap();
        std::fs::write(t.path().join(CONFIG), "model = \"m\"\n").unwrap();
        let w = crate::privacy::write_settings(t.path(), &[]).unwrap();
        assert!(w.is_empty());
        let text = std::fs::read_to_string(t.path().join(CONFIG)).unwrap();
        assert_eq!(notify(&text), None);
        let w = crate::privacy::write_settings(t.path(), &[setting()]).unwrap();
        assert!(w.is_empty(), "{w:?}");
        let text = std::fs::read_to_string(t.path().join(CONFIG)).unwrap();
        assert_eq!(notify(&text).unwrap(), PROGRAM);
    }
}
