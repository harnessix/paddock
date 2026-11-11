//! The stand-ins on PATH: how a start of an allowlisted program reaches
//! tackd without the program's help.
//!
//! `paddock tack setup` writes one small script per allowlisted program
//! name (`gh`) into `~/.local/share/paddock/tack/bin`, and puts that folder
//! first on PATH:
//!
//! - login shells: a marked line in `~/.profile`, `~/.bash_profile`,
//!   `~/.bash_login` and `~/.zprofile` (the ones that exist, and
//!   `~/.profile` or `~/.zprofile` when the login shell needs one), and
//!   `~/.config/fish/conf.d/paddock-tack.fish` when fish is set up;
//! - `~/.config/environment.d/60-paddock-tack.conf`, for the systemd user
//!   session and what it starts;
//! - a paddock sandbox gets its own stand-ins first on its PATH
//!   (`run/host_tools.rs`).
//!
//! Every shell finds programs through PATH, so this works for bash, zsh,
//! fish, dash and the others alike. What it does not cover: a program
//! started by its absolute path (`/usr/bin/gh`), by a process that sets
//! its own PATH, or found earlier on PATH (a version manager that puts
//! its folder first at each prompt). Those start with the references
//! only; `paddock doctor` lists them, and `paddock tack run PROGRAM`
//! starts a program explicitly.
//!
//! A stand-in never leaves the program unusable: when paddock is gone, the
//! script runs the next program of that name on PATH itself.

use super::config::Tack;
use crate::env::HostEnv;
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

/// The second line of every stand-in script: `is_shim` looks for it.
pub const MARK: &str = "# paddock tack stand-in";
/// The marker of the PATH lines in shell startup files.
pub const LINE_MARK: &str = "# paddock tack: `paddock tack remove` takes this line out";
const ENVD: &str = "60-paddock-tack.conf";

/// The folder of the stand-ins.
pub fn dir(env: &HostEnv) -> PathBuf {
    env.paddock_data().join("tack/bin")
}

/// The host PATH for a sandbox, without the host's stand-ins: the sandbox
/// has its own.
pub fn host_path(env: &HostEnv) -> impl Iterator<Item = &PathBuf> {
    let own = dir(env);
    env.path.iter().filter(move |d| **d != own)
}

/// Whether `name` can be a stand-in file name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && name != "paddock"
        && name != "paddock-host"
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-@".contains(&b))
}

/// The program names of every allowlist entry, sorted, without duplicates.
pub fn names(cfg: &Tack) -> Vec<String> {
    let mut out: Vec<String> = cfg
        .secrets
        .values()
        .flat_map(|s| s.allow.iter())
        .filter_map(|a| a.program.file_name()?.to_str().map(str::to_string))
        .filter(|n| valid_name(n))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A text in single quotes for sh.
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The stand-in script for `name`. `paddock` is the paddock to run, by
/// absolute path, or `paddock` (by PATH, in a sandbox); `own` is the folder
/// of the script, which the fallback skips.
pub fn script(paddock: &str, name: &str, own: &Path) -> String {
    script_with(paddock, name, own, &[])
}

/// The script, with the secret names of the program for a sandbox, where
/// the stand-in cannot read the user configuration.
pub fn script_with(paddock: &str, name: &str, own: &Path, secrets: &[String]) -> String {
    let hint = if secrets.is_empty() {
        String::new()
    } else {
        format!("--secrets {} ", sq(&secrets.join(",")))
    };
    format!(
        "#!/bin/sh\n{MARK} for {name}: written by paddock tack setup; paddock tack remove deletes it.\n\
p={p}\n\
case \"$p\" in\n  /*) [ -x \"$p\" ] && exec \"$p\" tack run --shim {hint}-- {n} \"$@\" ;;\n  *) command -v \"$p\" >/dev/null 2>&1 && exec \"$p\" tack run --shim {hint}-- {n} \"$@\" ;;\nesac\n\
# paddock is gone: run the next {name} on PATH, with the references only.\n\
own={own}\nset -f\nIFS=:\nfor d in $PATH; do\n  [ \"$d\" = \"$own\" ] && continue\n  [ -x \"$d\"/{n} ] && [ ! -d \"$d\"/{n} ] && exec \"$d\"/{n} \"$@\"\ndone\n\
echo {nf} >&2\nexit 127\n",
        p = sq(paddock),
        n = sq(name),
        own = sq(&own.display().to_string()),
        nf = sq(&format!("{name}: not found")),
    )
}

/// Whether the file at `p` is a stand-in script (of the host or of a
/// sandbox): the stand-in skips it when it looks for the real program.
pub fn is_shim(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 256];
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    let text = String::from_utf8_lossy(&head[..n]);
    text.starts_with("#!") && text.lines().nth(1).is_some_and(|l| l.starts_with(MARK))
}

/// Write the stand-ins of `names` into `dir`, and delete stand-ins of
/// names that are no longer allowlisted. Returns what changed.
pub fn write_shims(dir: &Path, names: &[String], paddock: &str) -> Result<Vec<String>> {
    let plain: Vec<(String, Vec<String>)> = names.iter().map(|n| (n.clone(), vec![])).collect();
    write_shims_with(dir, &plain, paddock)
}

/// `write_shims`, with the secret names of each program.
pub fn write_shims_with(
    dir: &Path,
    shims: &[(String, Vec<String>)],
    paddock: &str,
) -> Result<Vec<String>> {
    crate::privfs::dir(dir)?;
    let names: Vec<String> = shims.iter().map(|(n, _)| n.clone()).collect();
    let mut out = Vec::new();
    for (n, secrets) in shims {
        let p = dir.join(n);
        let text = script_with(paddock, n, dir, secrets);
        if std::fs::read_to_string(&p).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        crate::gui::write_file(&p, &text, 0o755)?;
        out.push(format!("wrote     stand-in {}", p.display()));
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !names.contains(&n) && is_shim(&e.path()) {
                std::fs::remove_file(e.path())?;
                out.push(format!(
                    "deleted   stand-in {} (no longer allowlisted)",
                    e.path().display()
                ));
            }
        }
    }
    Ok(out)
}

/// The PATH line for sh-like login shells.
pub fn sh_line(dir: &Path) -> String {
    let d = sq(&dir.display().to_string());
    format!(
        "[ -d {d} ] && case \"$PATH\" in {d}|{d}:*) ;; *) PATH={d}:\"$PATH\"; export PATH ;; esac {LINE_MARK}"
    )
}

pub fn fish_text(dir: &Path) -> String {
    let d = sq(&dir.display().to_string());
    format!(
        "{LINE_MARK}\nif test -d {d}; and test \"$PATH[1]\" != {d}\n    set -gx PATH {d} $PATH\nend\n"
    )
}

pub fn envd_text(dir: &Path) -> String {
    format!("{LINE_MARK}\nPATH={}:${{PATH}}\n", dir.display())
}

/// The login shell of the user: `$SHELL`, else the passwd entry.
fn login_shell(env: &HostEnv) -> String {
    env.var("SHELL").map(str::to_string).unwrap_or_default()
}

/// The shell startup files that get the PATH line, and whether each must
/// be created.
pub fn profile_files(env: &HostEnv) -> Vec<(PathBuf, bool)> {
    let h = &env.home;
    let mut out = Vec::new();
    let bash = [".bash_profile", ".bash_login", ".profile"];
    let mut any = false;
    for f in bash {
        let p = h.join(f);
        if p.is_file() {
            out.push((p, false));
            any = true;
        }
    }
    if !any {
        out.push((h.join(".profile"), true));
    }
    let z = h.join(".zprofile");
    let zsh = login_shell(env).ends_with("/zsh") || h.join(".zshrc").is_file();
    if z.is_file() {
        out.push((z, false));
    } else if zsh {
        out.push((z, true));
    }
    out
}

fn fish_file(env: &HostEnv) -> Option<PathBuf> {
    let d = env.config_dir.join("fish");
    d.is_dir().then(|| d.join("conf.d/paddock-tack.fish"))
}

fn envd_file(env: &HostEnv) -> PathBuf {
    env.config_dir.join("environment.d").join(ENVD)
}

/// Add the marked line to a startup file, once.
fn add_line(p: &Path, line: &str) -> Result<bool> {
    let old = std::fs::read_to_string(p).unwrap_or_default();
    if old.lines().any(|l| l == line) {
        return Ok(false);
    }
    let kept: Vec<&str> = old.lines().filter(|l| !l.contains(LINE_MARK)).collect();
    let mut text = kept.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    write_keeping_mode(p, &text)?;
    Ok(true)
}

fn write_keeping_mode(p: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0o644);
    crate::gui::write_file(p, text, mode).with_context(|| format!("cannot write {}", p.display()))
}

/// Take the marked line out of a startup file. A file that is empty then
/// and had only that line is deleted.
fn remove_line(p: &Path) -> Result<bool> {
    let Ok(old) = std::fs::read_to_string(p) else {
        return Ok(false);
    };
    if !old.contains(LINE_MARK) {
        return Ok(false);
    }
    let kept: Vec<&str> = old.lines().filter(|l| !l.contains(LINE_MARK)).collect();
    if kept.iter().all(|l| l.trim().is_empty()) {
        std::fs::remove_file(p)?;
    } else {
        write_keeping_mode(p, &(kept.join("\n") + "\n"))?;
    }
    Ok(true)
}

/// `paddock tack setup` for the user: stand-ins and PATH. `paddock` is the
/// absolute path of the paddock that the stand-ins run.
pub fn install(env: &HostEnv, cfg: &Tack, paddock: &Path) -> Result<Vec<String>> {
    let d = dir(env);
    let mut out = write_shims(&d, &names(cfg), &paddock.display().to_string())?;
    let line = sh_line(&d);
    for (p, _create) in profile_files(env) {
        if add_line(&p, &line)? {
            out.push(format!("wrote     PATH line in {}", super::show(env, &p)));
        }
    }
    if let Some(f) = fish_file(env) {
        if std::fs::read_to_string(&f).ok() != Some(fish_text(&d)) {
            if let Some(parent) = f.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::gui::write_file(&f, &fish_text(&d), 0o644)?;
            out.push(format!("wrote     {}", super::show(env, &f)));
        }
    }
    let e = envd_file(env);
    if std::fs::read_to_string(&e).ok() != Some(envd_text(&d)) {
        if let Some(parent) = e.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::gui::write_file(&e, &envd_text(&d), 0o644)?;
        out.push(format!("wrote     {}", super::show(env, &e)));
    }
    Ok(out)
}

/// `paddock tack remove` for the user: the stand-ins and the PATH lines.
pub fn remove(env: &HostEnv) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let d = dir(env);
    write_shims(&d, &[], "paddock").map(|l| out.extend(l))?;
    let _ = std::fs::remove_dir(&d);
    let h = &env.home;
    for f in [".bash_profile", ".bash_login", ".profile", ".zprofile"] {
        let p = h.join(f);
        if remove_line(&p)? {
            out.push(format!("removed   PATH line from {}", super::show(env, &p)));
        }
    }
    let mut files = vec![envd_file(env)];
    files.extend(fish_file(env));
    for f in files {
        if std::fs::read_to_string(&f).is_ok_and(|t| t.contains(LINE_MARK)) {
            std::fs::remove_file(&f)?;
            out.push(format!("deleted   {}", super::show(env, &f)));
        }
    }
    Ok(out)
}

/// The stand-ins for a sandbox: they run the paddock of the sandbox (on
/// its PATH). Returns the folder, or None when nothing is allowlisted.
pub fn sandbox_shims(env: &HostEnv, run_dir: &Path) -> Result<Option<PathBuf>> {
    let cfg = super::config::load(env).unwrap_or_default();
    let n = names(&cfg);
    if n.is_empty() {
        return Ok(None);
    }
    let shims: Vec<(String, Vec<String>)> = n
        .into_iter()
        .map(|name| {
            let secrets = cfg
                .secrets
                .iter()
                .filter(|(_, s)| {
                    s.allow
                        .iter()
                        .any(|a| a.program.file_name().and_then(|f| f.to_str()) == Some(&name))
                })
                .map(|(k, _)| k.clone())
                .collect();
            (name, secrets)
        })
        .collect();
    let d = run_dir.join("tack-bin");
    write_shims_with(&d, &shims, "paddock")?;
    Ok(Some(d))
}

/// The real program of a stand-in: the first `name` on `path` that is not
/// in `skip` and is not a stand-in script. An absolute name is itself.
pub fn resolve(name: &str, path: &[PathBuf], skip: &[PathBuf]) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return crate::env::is_executable(&p).then_some(p);
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let skip: Vec<PathBuf> = skip.iter().map(|s| canon(s)).collect();
    path.iter()
        .filter(|d| d.is_absolute())
        .filter(|d| !skip.contains(&canon(d)))
        .map(|d| d.join(name))
        .find(|p| crate::env::is_executable(p) && !is_shim(p))
}

/// The doctor lines about routing: the stand-ins, PATH, and what is not
/// covered.
pub fn doctor_lines(env: &HostEnv, cfg: &Tack) -> Vec<String> {
    let mut out = Vec::new();
    let n = names(cfg);
    if n.is_empty() {
        return out;
    }
    let d = dir(env);
    let missing: Vec<&String> = n.iter().filter(|x| !is_shim(&d.join(x))).collect();
    if !missing.is_empty() {
        out.push(format!(
            "  CHECK    no stand-in for {}: run paddock tack setup",
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // A stand-in that runs a paddock that is gone falls back to the
    // references.
    for x in &n {
        let p = d.join(x);
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        if let Some(bin) = text.lines().find_map(|l| l.strip_prefix("p=")) {
            let bin = bin.trim_matches('\'');
            if bin.starts_with('/') && !crate::env::is_executable(Path::new(bin)) {
                out.push(format!(
                    "  CHECK    the stand-in {x} runs {bin}, which is gone: run paddock tack setup"
                ));
            }
        }
    }
    let first = env.path.first().map(|p| p == &d).unwrap_or(false);
    if env.path.contains(&d) {
        if !first {
            out.push(format!(
                "  note     {} is on PATH, not first: start a new login shell",
                super::show(env, &d)
            ));
        }
    } else {
        out.push(format!(
            "  CHECK    {} is not on PATH in this shell: start a new login shell (or log in again) after paddock tack setup",
            super::show(env, &d)
        ));
    }
    for x in &n {
        match env.which(x) {
            Some(p) if p.parent() == Some(d.as_path()) => {
                out.push(format!("  ok       {x} starts through its stand-in"))
            }
            Some(p) if env.path.contains(&d) => out.push(format!(
                "  CHECK    {x} is found at {} before its stand-in: it starts with the references only; paddock tack run {x} starts it with its secrets",
                p.display()
            )),
            _ => {}
        }
    }
    out.push("  note     not covered: a program started by its absolute path (/usr/bin/gh) or by a process with its own PATH gets the references only; paddock tack run PROGRAM starts it explicitly".into());
    out
}

#[cfg(test)]
#[path = "tackshims_tests.rs"]
mod tests;
