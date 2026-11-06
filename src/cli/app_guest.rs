//! Reset, backup and restore of a profile of the macOS guest, and the
//! instance of the commands of `paddock app` that name one profile.

use anyhow::{bail, Result};
use paddock::env::HostEnv;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The profile of a name on the macOS guest route, when it exists.
fn guest_profile(
    env: &HostEnv,
    name: &str,
    n: u32,
) -> Result<(
    paddock::macvm::layout::Layout,
    paddock::macvm::session::Profile,
)> {
    use paddock::macvm::{host, layout::Layout, text};
    if !host::valid_name(name) {
        bail!(text::bad_app_name(name));
    }
    let layout = Layout::new(env);
    let profile = layout.profile(name, &paddock::macvm::instance::folder(n));
    if !profile.dir.is_dir() {
        bail!(text::no_profile(
            &paddock::macvm::instance::arg(name, n),
            &profile.dir
        ));
    }
    Ok((layout, profile))
}

/// Ask on the terminal. Without a terminal, print the command with
/// `--yes` and answer no.
fn ask_yes(yes: bool, question: &str, again: &str) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if yes {
        return Ok(true);
    }
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        println!("Run `{again}` to go on.");
        return Ok(false);
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut a = String::new();
    std::io::stdin().read_line(&mut a)?;
    if !matches!(a.trim(), "y" | "Y" | "yes") {
        println!("Nothing changed.");
        return Ok(false);
    }
    Ok(true)
}

/// `paddock app reset NAME [--data | --all] [--yes]`.
pub(crate) fn reset_cmd(
    env: &HostEnv,
    app: &str,
    n: u32,
    data: bool,
    all: bool,
    yes: bool,
) -> Result<i32> {
    use paddock::macvm::{backup, firewall, golden, host, instance, manage, session, text};
    let (layout, profile) = guest_profile(env, app, n)?;
    let home = instance::folder(n);
    let arg = instance::arg(app, n);
    let name = arg.as_str();
    if let Some(s) = host::running(&layout, app, &home) {
        bail!(text::profile_running(name, s.pid));
    }
    let tier = if all {
        text::ResetTier::All
    } else if data {
        text::ResetTier::Data
    } else {
        text::ResetTier::System
    };
    let size = golden::used_bytes_in(&profile.dir);
    let confirm = text::reset_confirm(name, tier, size);
    let question = confirm.trim_end_matches(" [y/N]").to_string();
    let again = format!(
        "paddock app reset {name}{} --yes",
        match tier {
            text::ResetTier::System => "",
            text::ResetTier::Data => " --data",
            text::ResetTier::All => " --all",
        }
    );
    println!("{question}");
    if !ask_yes(yes, "Reset?", &again)? {
        return Ok(1);
    }
    let rules = firewall::rules_file_if_any(env, &instance::key(app, n));
    let now = now_secs();
    let mut make_backup = |p: &session::Profile| {
        let free = session::free_bytes(&p.dir);
        let out = backup::backup(p, "pre-reset", false, now, free, &manage::clone_or_copy)?;
        if let backup::Outcome::Made(dir) = &out {
            println!(
                "{}",
                text::backup_pair_made(dir, golden::used_bytes(&p.data()))
            );
        }
        Ok(out)
    };
    let r = session::reset(
        &layout,
        app,
        &home,
        tier,
        rules.as_deref(),
        now,
        &mut make_backup,
    )?;
    println!(
        "{}",
        text::reset_done(name, tier, r.removed.len(), r.aside.as_deref())
    );
    Ok(0)
}

/// `paddock app backup NAME [--out DIR] [--stop]`.
pub(crate) fn backup_cmd(
    env: &HostEnv,
    app: &str,
    n: u32,
    out: Option<&std::path::Path>,
    stop: bool,
) -> Result<i32> {
    use paddock::macvm::{backup, golden, host, instance, manage, session, text};
    let (layout, profile) = guest_profile(env, app, n)?;
    let arg = instance::arg(app, n);
    let name = arg.as_str();
    if host::running(&layout, app, &instance::folder(n)).is_some() {
        if stop {
            host::stop(env, app, n, std::time::Duration::from_secs(120))?;
        } else {
            bail!(text::quit_first(name));
        }
    }
    let now = now_secs();
    if let Some(dir) = out {
        backup::backup_to(&profile, dir, now, &manage::clone_or_copy)?;
        println!("{}", text::backup_written(dir));
        return Ok(0);
    }
    let free = session::free_bytes(&profile.dir);
    match backup::backup(&profile, "manual", true, now, free, &manage::clone_or_copy)? {
        backup::Outcome::Made(dir) => {
            println!(
                "{}",
                text::backup_pair_made(&dir, golden::used_bytes(&profile.data()))
            );
            Ok(0)
        }
        backup::Outcome::Skipped { needed, free } => {
            println!("{}", text::backup_skipped(needed, free));
            Ok(1)
        }
        backup::Outcome::Nothing => {
            println!("{}", text::nothing_to_backup(name));
            Ok(1)
        }
    }
}

/// `paddock app restore NAME FOLDER|TIME`.
pub(crate) fn restore_cmd(env: &HostEnv, app: &str, n: u32, which: &str) -> Result<i32> {
    use paddock::macvm::{backup, host, instance, manage, text};
    let (layout, profile) = guest_profile(env, app, n)?;
    let arg = instance::arg(app, n);
    let name = arg.as_str();
    if let Some(s) = host::running(&layout, app, &instance::folder(n)) {
        bail!(text::profile_running(name, s.pid));
    }
    let Some(b) = backup::find(&profile, which) else {
        bail!(text::backup_not_found(name, which, &profile.backups()));
    };
    let aside = backup::restore(&profile, &b, now_secs(), &manage::clone_or_copy)?;
    println!("{}", text::restore_pair_done(name, &b.dir, &aside));
    Ok(0)
}

/// `paddock app backups NAME`.
pub(crate) fn backups_cmd(env: &HostEnv, app: &str, n: u32) -> Result<i32> {
    use paddock::macvm::{backup, instance, text};
    let (_, profile) = guest_profile(env, app, n)?;
    let arg = instance::arg(app, n);
    let name = arg.as_str();
    let all = backup::list(&profile);
    if all.is_empty() {
        println!("{}", text::no_backups(name, &profile.backups()));
        return Ok(1);
    }
    for b in all {
        if b.data_only {
            println!("{}", text::backup_list_data_only(&b.name));
        } else {
            println!(
                "{}",
                text::backup_list_line(&b.name, &b.when(), b.meta.bytes, b.meta.manual)
            );
        }
    }
    Ok(0)
}

/// The value of `--instance`.
pub(crate) fn parse_instance(s: &str) -> std::result::Result<u32, String> {
    paddock::macvm::instance::parse(s)
}

/// `--instance N` of the commands that name one profile of the macOS
/// guest. Without it: instance 1, the first profile of the app.
#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub(crate) struct Instance {
    /// macOS guest only: the instance of the app profile, from 1 (the
    /// first) to 99.
    #[arg(long, value_parser = parse_instance)]
    pub(crate) instance: Option<u32>,
}

impl Instance {
    pub(crate) fn n(&self) -> u32 {
        self.instance.unwrap_or(paddock::macvm::instance::FIRST)
    }
}
