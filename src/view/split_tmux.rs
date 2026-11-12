//! What `paddock split` needs from tmux: a tmux that is new enough, and a
//! pane title that stays on paddock's label.
//!
//! tmux 3.5 and newer have `allow-set-title`: with it off, a program in a
//! pane cannot set the pane title with an OSC sequence. tmux 3.2 to 3.4
//! have no such option, and `set-option -q` hides that. On every tmux, the
//! hook `pane-title-changed` sets the title back to `@paddock_title`, an
//! option that only paddock sets. The border reads only the pane options,
//! so the label holds also before the hook runs.

use super::{s, Tmux};
use anyhow::{bail, Result};
use std::process::Stdio;

/// The oldest tmux for the split: 3.2 has pane options and the hook
/// `pane-title-changed`.
pub const OLDEST: (u32, u32) = (3, 2);

/// The hook command. It sets the title only when the title is not the
/// label, because `select-pane -T` runs the hook again.
pub const TITLE_HOOK: &str =
    "if-shell -F '#{!=:#{pane_title},#{@paddock_title}}' 'select-pane -T \"#{@paddock_title}\"'";

/// The version in the output of `tmux -V`: `tmux 3.4`, `tmux 3.3a`,
/// `tmux next-3.6`. None for a build without a number, such as `tmux master`.
pub fn parse_version(out: &str) -> Option<(u32, u32)> {
    let word = out.split_whitespace().nth(1)?;
    let num = word
        .rsplit('-')
        .find(|p| p.starts_with(|c: char| c.is_ascii_digit()))?;
    let (major, rest) = num.split_once('.')?;
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The version of tmux `t`, when `tmux -V` names one.
pub fn version(t: &Tmux) -> Option<(u32, u32)> {
    let out = t.cmd().arg("-V").stdout(Stdio::piped()).output().ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

/// Refuse a tmux older than OLDEST. A tmux without a version number
/// continues: the split then checks that tmux took its options.
pub fn check_version(t: &Tmux) -> Result<()> {
    match version(t) {
        Some(v) if v < OLDEST => bail!("{}", too_old(v, &t.exe.display().to_string())),
        _ => Ok(()),
    }
}

/// The message for a tmux older than OLDEST.
pub fn too_old(v: (u32, u32), exe: &str) -> String {
    format!(
        "paddock split needs tmux {}.{} or newer, and {exe} is tmux {}.{}. With an older tmux, a program in a pane could change what the pane shows. Update tmux, or open each pane in its own terminal window: paddock split --windows [PANES]",
        OLDEST.0, OLDEST.1, v.0, v.1
    )
}

/// A flag of tmux.
pub fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// The hook of session window `win`: a pane that the user opens has no
/// hook of its own.
pub fn session_args(win: &str) -> Vec<String> {
    s(&["set-hook", "-t", win, "pane-title-changed", TITLE_HOOK])
}

/// Whether the session of window `win` has the hook.
pub fn took(t: &Tmux, win: &str) -> bool {
    t.output(&s(&["show-hooks", "-t", win, "pane-title-changed"]))
        .is_some_and(|o| o.contains("pane-title-changed"))
}

/// The commands that keep the title of pane `pane` on its label: the hook
/// of the pane, and the title set to the label now.
pub fn title_args(pane: &str) -> Vec<String> {
    let mut v = s(&[
        "set-hook",
        "-p",
        "-t",
        pane,
        "pane-title-changed",
        TITLE_HOOK,
    ]);
    v.push(";".into());
    v.extend(s(&["select-pane", "-t", pane, "-T", "#{@paddock_title}"]));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_of_tmux() {
        assert_eq!(parse_version("tmux 3.4\n"), Some((3, 4)));
        assert_eq!(parse_version("tmux 3.3a"), Some((3, 3)));
        assert_eq!(parse_version("tmux 3.8"), Some((3, 8)));
        assert_eq!(parse_version("tmux next-3.6"), Some((3, 6)));
        assert_eq!(parse_version("tmux 2.9a"), Some((2, 9)));
        assert_eq!(parse_version("tmux master"), None);
        assert_eq!(parse_version(""), None);
        assert!((3, 1) < OLDEST && (3, 2) >= OLDEST && (2, 9) < OLDEST);
        let m = too_old((3, 1), "/usr/bin/tmux");
        assert!(
            m.contains("tmux 3.2 or newer") && m.contains("is tmux 3.1"),
            "{m}"
        );
        assert!(m.contains("paddock split --windows"), "{m}");
    }

    #[test]
    fn the_title_hook_reads_only_paddocks_label() {
        assert!(TITLE_HOOK.contains("#{@paddock_title}"));
        let a = title_args("%3");
        assert_eq!(
            a[..5],
            s(&["set-hook", "-p", "-t", "%3", "pane-title-changed"])[..]
        );
        assert!(a.ends_with(&s(&["-T", "#{@paddock_title}"])));
    }
}
