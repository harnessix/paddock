//! `paddock tui`: start the terminal interface. It is a separate program
//! (`tui/` in this repository), so the paddock binary gains no dependency.
//! paddock looks for it in this order: the program that `PADDOCK_TUI`
//! names, `paddock-tui` next to paddock, paddock's own copy in its data
//! folder, then `paddock-tui` on PATH. On a host without it, paddock
//! installs the one of its own release (src/update/tui.rs). It never uses
//! `npx`: npx would run code from npm and from the project folder (its
//! `node_modules` and `.npmrc`, which the agent can write) outside the
//! sandbox, with the power to answer every approval.

use anyhow::Result;
use paddock::env::HostEnv;
use paddock::update::{self, tui};
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

/// The variable that tells the interface where to start. An older
/// `paddock-tui` does not read it and starts on its runs.
const START_ENV: &str = "PADDOCK_TUI_START";

pub(crate) fn run(env: &HostEnv, args: Vec<String>) -> Result<i32> {
    let exe = std::env::current_exe().ok();
    let bin = match program(env, exe.as_deref()) {
        Ok(bin) => bin,
        Err(line) => {
            eprintln!("{line}");
            return Ok(127);
        }
    };
    exec(exe, bin, args, false)
}

/// `paddock` alone on a terminal: the home screen of the interface, as a
/// coding agent opens its own interface when you type its name. `None`:
/// this is not that case, and paddock goes on as before (it prints its
/// help). It finds or installs the interface as `paddock tui` does. When
/// it cannot, the one line of [`program`] comes first.
pub(crate) fn home(argv: &[OsString]) -> Option<Result<i32>> {
    use std::io::IsTerminal;
    let args = home_args(argv)?;
    // A pipe, a script or CI gets the help, as before.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    // Without a host environment, real_main reports the error as before.
    let env = HostEnv::detect().ok()?;
    let exe = std::env::current_exe().ok();
    match program(&env, exe.as_deref()) {
        Ok(bin) => Some(exec(exe, bin, args, true)),
        Err(line) => {
            eprintln!("{line}");
            None
        }
    }
}

/// The arguments of a `paddock` without a command, when all of them are
/// options of `paddock tui`: they go to the interface. `None` for anything
/// else, which clap reads as before.
fn home_args(argv: &[OsString]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        let a = a.to_str()?;
        match a {
            "--plain" | "--readonly" | "--read-only" | "--no-mouse" | "--reduce-motion" => {
                out.push(a.to_string())
            }
            "--theme" => {
                let v = it.next()?.to_str()?;
                if v != "dark" && v != "light" {
                    return None;
                }
                out.push(a.to_string());
                out.push(v.to_string());
            }
            "--theme=dark" | "--theme=light" => out.push(a.to_string()),
            _ => return None,
        }
    }
    Some(out)
}

fn exec(exe: Option<PathBuf>, bin: PathBuf, args: Vec<String>, home: bool) -> Result<i32> {
    let mut cmd = std::process::Command::new(bin);
    if let Some(exe) = exe {
        // The interface calls the same paddock, not another one on PATH.
        cmd.env("PADDOCK_BIN", exe);
    }
    if home {
        cmd.env(START_ENV, "home");
    } else {
        cmd.env_remove(START_ENV);
    }
    let err = cmd.args(args).exec();
    anyhow::bail!("could not start the terminal UI: {err}")
}

/// The installed program. Without one, paddock installs it, or the error
/// is the one line that says why not and what to run.
fn program(env: &HostEnv, exe: Option<&Path>) -> Result<PathBuf, String> {
    match tui::find(env, exe) {
        Ok(Some(bin)) => return Ok(bin),
        Ok(None) => {}
        Err(e) => return Err(format!("paddock tui: {e:#}")),
    }
    let gh = update::GitHub::from_env(env, std::time::Duration::from_secs(30));
    tui::obtain(env, &tui::Build::this(exe), &gh, false, &mut |line| {
        eprintln!("{line}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paddock_without_a_command_takes_only_the_options_of_the_interface() {
        let argv = |a: &[&str]| -> Vec<OsString> {
            std::iter::once("paddock")
                .chain(a.iter().copied())
                .map(OsString::from)
                .collect()
        };
        assert_eq!(home_args(&argv(&[])), Some(vec![]));
        assert_eq!(
            home_args(&argv(&[
                "--plain",
                "--readonly",
                "--no-mouse",
                "--theme",
                "light"
            ])),
            Some(
                ["--plain", "--readonly", "--no-mouse", "--theme", "light"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(
            home_args(&argv(&["--read-only", "--theme=dark"])),
            Some(["--read-only", "--theme=dark"].map(String::from).to_vec())
        );
        assert_eq!(
            home_args(&argv(&["--reduce-motion"])),
            Some(vec!["--reduce-motion".to_string()])
        );
        // A command, a harness, paddock's own options and a broken --theme
        // go to clap, as before.
        for other in [
            &["tui"][..],
            &["claude"],
            &["help"],
            &["--help"],
            &["-h"],
            &["--version"],
            &["-V"],
            &["-v"],
            &["--quiet"],
            &["--plain", "ps"],
            &["--theme"],
            &["--theme", "blue"],
            &["--theme=blue"],
            &["--home"],
        ] {
            assert_eq!(home_args(&argv(other)), None, "{other:?}");
        }
    }
}
