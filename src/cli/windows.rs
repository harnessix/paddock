use anyhow::{bail, Result};
use paddock::env::HostEnv;
use paddock::state::{self, Running};

pub(crate) fn windows(env: &HostEnv, json: bool, mark: bool, focus: Option<String>) -> Result<i32> {
    #[cfg(target_os = "macos")]
    return mac_windows(env, json, mark, focus);
    #[cfg(not(target_os = "macos"))]
    hypr_windows(env, json, mark, focus)
}

/// One marked window on macOS: an app window or the terminal window of a
/// run.
#[derive(Debug, PartialEq, serde::Serialize)]
struct MacRow {
    sandbox: String,
    kind: &'static str,
    harness: String,
    network: String,
    /// The window number of the window server.
    window: u32,
    /// The process that owns the window.
    pid: u32,
    /// The color of the frame: `sage` or `amber`.
    tone: &'static str,
    /// The text of the badge, `paddock · sample-notes · ask · vm · 2h`.
    badge: String,
    /// The question id of an approval panel.
    #[serde(skip_serializing_if = "String::is_empty")]
    question: String,
}

/// The windows that the marker marks, from the run records, the window
/// list (front first) and the process table.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_rows(
    list: Vec<Running>,
    wins: &[paddock::mark::Win],
    procs: &paddock::mark::ProcTable,
    vm: &dyn Fn(&[Running]) -> Vec<Option<paddock::mark::VmFacts>>,
) -> Vec<MacRow> {
    use paddock::mark::{self, Target, Tone};
    let runs = mark::marked_runs(list);
    let facts = vm(&runs);
    let now = state::now();
    let targets: Vec<Target> = runs.iter().map(|r| Target::of(r, procs)).collect();
    mark::match_windows(wins, &targets, std::process::id())
        .into_iter()
        .map(|(w, i)| MacRow {
            sandbox: runs[i].id.clone(),
            kind: match targets[i] {
                Target::Window(..) => "terminal",
                Target::Tree(_) => "app",
            },
            harness: runs[i].harness.clone(),
            network: runs[i].network.clone(),
            window: w.id,
            pid: w.pid,
            tone: Tone::of(&runs[i].network).word(),
            badge: mark::run_badge(&runs[i], facts.get(i).and_then(|f| f.as_ref()), now),
            question: String::new(),
        })
        .collect()
}

/// The firewall panels of the marker, one row each with `kind = approval`:
/// the window is the panel, the pid the marker.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn approval_rows(panels: Vec<paddock::mark::PanelRow>) -> Vec<MacRow> {
    panels
        .into_iter()
        .map(|p| MacRow {
            sandbox: p.sandbox,
            kind: "approval",
            harness: p.harness,
            network: "ask".into(),
            window: p.window,
            pid: p.pid,
            tone: "sage",
            badge: String::new(),
            question: p.question,
        })
        .collect()
}

/// The `paddock windows --json` answer on macOS: the envelope of Linux
/// (`api_version` and `windows`), so one reader takes both.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_json(rows: &[MacRow]) -> serde_json::Value {
    serde_json::json!({
        "api_version": "paddock.windows/v1",
        "windows": rows,
    })
}

/// `paddock windows` on macOS: the marked windows, without Hyprland. The
/// shield menu in the menu bar raises a window, so there is no --focus.
/// --mark starts the marker.
#[cfg(target_os = "macos")]
fn mac_windows(env: &HostEnv, json: bool, mark: bool, focus: Option<String>) -> Result<i32> {
    if focus.is_some() {
        bail!("on macOS, the shield menu in the menu bar raises a protected window");
    }
    if mark {
        paddock::mark::spawn(env);
    }
    let list = Running::list(env);
    let mut rows = if list.is_empty() {
        Vec::new()
    } else {
        let (wins, procs) = paddock::mark::screen();
        mac_rows(list, &wins, &procs, &|runs| {
            paddock::mark::vm::facts(env, runs)
        })
    };
    rows.extend(approval_rows(paddock::mark::read_panels(env)));
    if json {
        println!("{}", serde_json::to_string_pretty(&mac_json(&rows))?);
    } else {
        for r in &rows {
            println!(
                "{:<28} {:<9} {:<16} {:<10} {}",
                r.sandbox, r.kind, r.harness, r.network, r.window
            );
        }
    }
    Ok(0)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn hypr_windows(env: &HostEnv, json: bool, mark: bool, focus: Option<String>) -> Result<i32> {
    use paddock::windows;
    let light = windows::current_theme_is_light(&env.home);
    let (found, clients) = protected_windows(env);
    if let Some(address) = focus {
        if !windows::valid_address(&address) || !found.iter().any(|p| p.client.address == address) {
            bail!("{address} is not a protected window");
        }
        return Ok(if focus_address(&address) { 0 } else { 1 });
    }
    if mark {
        if let Some(clients) = &clients {
            mark_windows(env, &found, clients, light);
        }
    }
    if json {
        println!("{}", windows::to_json(&found, state::now(), light));
    } else if !mark {
        for p in &found {
            println!(
                "{:<28} {:<9} {:<16} {:<10} {}",
                p.sandbox,
                p.kind.as_str(),
                p.harness,
                p.network,
                p.client.address
            );
        }
    }
    Ok(0)
}

pub(crate) fn focus(env: &HostEnv, id: String, display: bool) -> Result<i32> {
    let Some(r) = Running::list(env).into_iter().find(|r| r.id == id) else {
        bail!("no running sandbox with id {id}")
    };
    let Some(found) = paddock::relay::hypr_clients() else {
        bail!("focus needs Hyprland (hyprctl was not found)");
    };
    let windows: Vec<(u32, String)> = found.into_iter().map(|c| (c.pid, c.address)).collect();
    let address_of = |pid: u32| {
        windows
            .iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, a)| a.clone())
    };
    let host_pid = if r.host_pid > 0 { r.host_pid } else { r.pid };
    let target = if display {
        r.window_pid.and_then(address_of)
    } else if r.gui == "app" {
        // An app sandbox draws its own host windows: find the window whose
        // process descends from this paddock process.
        windows.iter().find_map(|(pid, address)| {
            let mut p = *pid;
            for _ in 0..64 {
                if p == host_pid {
                    return Some(address.clone());
                }
                match state::parent_pid(p) {
                    Some(q) if q > 1 => p = q,
                    _ => return None,
                }
            }
            None
        })
    } else {
        // Walk up from paddock to the first process that owns a window: the terminal.
        let mut pid = host_pid;
        let mut found = None;
        for _ in 0..32 {
            if let Some(a) = address_of(pid) {
                found = Some(a);
                break;
            }
            match state::parent_pid(pid) {
                Some(p) if p > 1 => pid = p,
                _ => break,
            }
        }
        found
    };
    let Some(address) = target else {
        bail!("no window found for {id}; it may run without a terminal window")
    };
    Ok(if focus_address(&address) { 0 } else { 1 })
}

/// list is None without Hyprland.
fn protected_windows(
    env: &HostEnv,
) -> (
    Vec<paddock::windows::Protected>,
    Option<Vec<paddock::windows::Client>>,
) {
    use paddock::windows;
    let runs: Vec<windows::Run> = Running::list(env)
        .iter()
        .map(windows::Run::from_running)
        .collect();
    if runs.is_empty() {
        // No run: read the windows only when --mark has borders to take off.
        let marked = env.paddock_run().join("window-marks.json").exists();
        return (vec![], marked.then(windows::hypr_clients).flatten());
    }
    let Some(clients) = windows::hypr_clients() else {
        return (vec![], None);
    };
    let ttys = runs.iter().filter_map(|r| r.tty).collect();
    let holders = windows::pty_holders(&ttys);
    let procs = windows::Procs {
        parent: &state::parent_pid,
        mnt_ns: &windows::mnt_ns,
        holders: &holders,
    };
    let found = windows::find(&runs, &clients, &procs);
    (found, Some(clients))
}

/// `hyprctl ARGS`: true when Hyprland took the command.
fn hyprctl(args: &[String]) -> bool {
    std::process::Command::new("hyprctl")
        .args(args)
        .output()
        .map(|o| {
            let t = String::from_utf8_lossy(&o.stdout).to_lowercase();
            o.status.success() && !t.contains("error") && !t.contains("not found")
        })
        .unwrap_or(false)
}

/// paddock's mark on protected windows: a border, sage or amber by network
/// level, set on each window only, at run time. Nothing in the Hyprland
/// configuration changes. A window that lost its run (a terminal after the
/// run ends) gets its own border back.
fn mark_windows(
    env: &HostEnv,
    found: &[paddock::windows::Protected],
    clients: &[paddock::windows::Client],
    light: bool,
) {
    use paddock::windows::{self, Level};
    use std::collections::{BTreeMap, HashSet};
    let file = env.paddock_run().join("window-marks.json");
    let before: BTreeMap<String, Level> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let existing: HashSet<String> = clients.iter().map(|c| c.address.clone()).collect();
    let (marks, unmark) = windows::plan_marks(&before, found, &existing);
    let apply = |address: &str, level: Option<Level>| {
        let [lua, classic] = windows::border_commands(address, level, light);
        if hyprctl(&lua[0]) {
            hyprctl(&lua[1]);
            return;
        }
        // Hyprland before the Lua configuration. Debug only: the plugin runs
        // this every few seconds, so anything louder would fill the journal.
        log::debug!("window {address}: Lua set_prop failed, using setprop");
        if !hyprctl(&classic[0]) {
            log::debug!("window {address}: setprop failed too, no border");
        }
        hyprctl(&classic[1]);
    };
    for (address, level) in &marks {
        apply(address, Some(*level));
    }
    for address in &unmark {
        apply(address, None);
    }
    if marks != before {
        if marks.is_empty() {
            let _ = std::fs::remove_file(&file);
        } else if let Ok(t) = serde_json::to_string(&marks) {
            if let Err(e) = paddock::privfs::write(&file, t) {
                log::debug!("cannot save the window marks: {e:#}");
            }
        }
    }
}

/// Focus a window and raise it (Hyprland). Hyprland with a Lua
/// configuration takes the hl.dsp form; older versions take the classic
/// dispatcher. Try both, as Omarchy does.
fn focus_address(address: &str) -> bool {
    let ok = hyprctl(&[
        "dispatch".into(),
        format!("hl.dsp.focus({{ window = \"address:{address}\" }})"),
    ]) || hyprctl(&[
        "dispatch".into(),
        "focuswindow".into(),
        format!("address:{address}"),
    ]);
    if ok {
        // Focus alone can leave a floating window under others: raise it.
        let _ = hyprctl(&[
            "dispatch".into(),
            format!("hl.dsp.window.bring_to_top({{ window = \"address:{address}\" }})"),
        ]);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use paddock::mark::{ProcTable, Rect, Win};

    fn run(id: &str, gui: &str, pid: u32, window: Option<(u32, u32)>) -> Running {
        Running {
            id: id.into(),
            harness: id.split('-').next().unwrap_or(id).into(),
            network: "egress".into(),
            gui: gui.into(),
            pid,
            terminal_window: window.map(|w| w.0),
            terminal_pid: window.map(|w| w.1),
            ..Default::default()
        }
    }

    fn win(id: u32, pid: u32) -> Win {
        Win {
            id,
            pid,
            layer: 0,
            bounds: Rect::new(0.0, 30.0, 800.0, 600.0),
            alpha: 1.0,
        }
    }

    #[test]
    fn the_firewall_panels_are_approval_rows() {
        let t = tempfile::tempdir().unwrap();
        let env = paddock::env::HostEnv::from_parts(
            t.path().join("home"),
            "demo".into(),
            1000,
            1000,
            [(
                "XDG_RUNTIME_DIR".to_string(),
                t.path().join("run").display().to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert!(paddock::mark::read_panels(&env).is_empty());
        let mine = paddock::mark::PanelRow {
            question: "c-1".into(),
            window: 77,
            pid: std::process::id(),
            sandbox: "macvm-sample-notes".into(),
            harness: "sample-notes".into(),
        };
        let dead = paddock::mark::PanelRow {
            pid: 2_000_000_000,
            ..mine.clone()
        };
        paddock::mark::write_panels(&env, &[mine.clone(), dead]);
        let rows = approval_rows(paddock::mark::read_panels(&env));
        assert_eq!(rows.len(), 1, "a panel of a dead marker is not listed");
        assert_eq!(
            rows[0],
            MacRow {
                sandbox: "macvm-sample-notes".into(),
                kind: "approval",
                harness: "sample-notes".into(),
                network: "ask".into(),
                window: 77,
                pid: std::process::id(),
                tone: "sage",
                badge: String::new(),
                question: mine.question.clone(),
            }
        );
        // The JSON of macOS has the envelope of Linux; none is `[]`.
        let v = mac_json(&rows);
        assert_eq!(v["api_version"], "paddock.windows/v1");
        assert_eq!(v["windows"][0]["kind"], "approval");
        assert_eq!(v["windows"][0]["window"], 77);
        assert_eq!(mac_json(&[])["windows"], serde_json::json!([]));
        paddock::mark::write_panels(&env, &[]);
        assert!(!paddock::mark::panels_file(&env).exists());
    }

    #[test]
    fn the_macos_listing_has_app_and_terminal_windows() {
        let list = vec![
            run("vscode-api-1a2b", "app", 100, None),
            run("codex-api-3c4d", "off", 600, Some((42, 530))),
            // A terminal run without a recorded window (tmux): no row.
            run("claude-web-5e6f", "off", 700, None),
        ];
        // VS Code 100 has a helper 101 with the window. Terminal 530 has
        // windows 41 and 42.
        let procs: ProcTable = [(100, 1), (101, 100), (530, 1), (600, 532)]
            .into_iter()
            .collect();
        let wins = vec![win(41, 530), win(42, 530), win(7, 101), win(8, 999)];
        let rows = mac_rows(list, &wins, &procs, &|runs| vec![None; runs.len()]);
        let got: Vec<(&str, &str, u32, u32)> = rows
            .iter()
            .map(|r| (r.sandbox.as_str(), r.kind, r.window, r.pid))
            .collect();
        assert_eq!(
            got,
            vec![
                ("codex-api-3c4d", "terminal", 42, 530),
                ("vscode-api-1a2b", "app", 7, 101),
            ]
        );
        assert_eq!(rows[0].harness, "codex");
        let v = serde_json::to_value(&rows).unwrap();
        assert_eq!(v[0]["window"], 42);
        assert_eq!(v[0]["kind"], "terminal");
        assert_eq!(v[0]["tone"], "sage");
    }

    #[test]
    fn two_windows_of_a_guest_session_are_sage_with_vm() {
        let mut r = run("macvm-sample-notes", "app", 812, None);
        r.backend = "macvm".into();
        r.harness = "sample-notes".into();
        r.network = "ask".into();
        r.started = state::now();
        let procs: ProcTable = [(812, 1)].into_iter().collect();
        let wins = vec![win(11, 812), win(12, 812)];
        let facts = paddock::mark::VmFacts {
            display_name: "Sample Notes (paddock)".into(),
            ..Default::default()
        };
        let rows = mac_rows(vec![r], &wins, &procs, &|runs| {
            vec![Some(facts.clone()); runs.len()]
        });
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.tone, "sage");
            assert!(row.badge.contains(" · vm · "), "{}", row.badge);
            assert!(row.badge.starts_with("paddock · sample-notes · ask"));
        }
    }
}
