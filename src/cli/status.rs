use anyhow::Result;
use paddock::env::HostEnv;
use paddock::state::{self, Running};

pub(crate) fn ps(env: &HostEnv, json: bool, mem: bool) -> Result<i32> {
    // Children follow their parent.
    let list = state::tree_order(Running::list(env));
    if json {
        let sessions = paddock::view::Tmux::find(env)
            .map(|t| t.sessions())
            .unwrap_or_default();
        let rows: Vec<serde_json::Value> = list
            .iter()
            .map(|r| {
                let mut v = serde_json::to_value(r).unwrap_or_default();
                if let (Some(o), Some((since, reason))) =
                    (v.as_object_mut(), paddock::freeze::frozen(env, &r.id))
                {
                    o.insert("frozen_since".into(), since.into());
                    o.insert("frozen_reason".into(), reason.into());
                }
                let timers = paddock::timer::read_summary(env, &r.id);
                if let (Some(o), false) = (v.as_object_mut(), timers.is_empty()) {
                    o.insert(
                        "timers".into(),
                        serde_json::to_value(timers).unwrap_or_default(),
                    );
                }
                if let Some(o) = v.as_object_mut() {
                    o.insert("short".into(), paddock::ids::short(&r.id).into());
                    let (view, tmux) = paddock::view::available(env, &sessions, &r.id);
                    o.insert("view".into(), view.into());
                    o.insert("view_tmux".into(), tmux.into());
                    o.insert(
                        "detached".into(),
                        paddock::detach::is_detached(env, &r.id).into(),
                    );
                    if let Some(since) = paddock::detach::idle_since(env, &r.id) {
                        o.insert("idle_since".into(), since.into());
                    }
                    if let Some(rec) = paddock::detach::Record::load(env, &r.id) {
                        if !rec.notes.is_empty() {
                            o.insert("notes".into(), rec.notes.into());
                        }
                        if let Some(s) = rec.stuck {
                            o.insert("stuck".into(), serde_json::to_value(s).unwrap_or_default());
                        }
                    }
                    o.insert(
                        "activity".into(),
                        match paddock::view::activity(env, &r.id) {
                            Some(line) => serde_json::Value::String(line),
                            None => serde_json::Value::Null,
                        },
                    );
                }
                // The model and effort that paddock chose for a child.
                let pick = r
                    .parent
                    .as_deref()
                    .zip(r.spawn_id.as_deref())
                    .and_then(|(p, s)| paddock::spawn::model::of_child(env, p, s));
                if let (Some(o), Some(p)) = (v.as_object_mut(), pick) {
                    o.insert("model".into(), p.model.into());
                    o.insert("effort".into(), p.effort.map(|e| e.name()).into());
                }
                if let (Some(o), Some(t)) = (v.as_object_mut(), spec_task(env, &r.id)) {
                    o.insert("spec_task".into(), t.id.into());
                    o.insert("spec_feature".into(), t.feature.into());
                    o.insert("spec_state".into(), t.state.name().into());
                }
                if let (Some(o), true) = (v.as_object_mut(), paddock::shadow::is_shadow(env, &r.id))
                {
                    o.insert("profile".into(), paddock::shadow::DEFAULT.into());
                }
                if let Some(o) = v.as_object_mut() {
                    o.insert(
                        "boundary_count".into(),
                        paddock::boundary::count(env, &r.id).into(),
                    );
                    o.insert(
                        "compactions".into(),
                        paddock::compaction::count(env, &r.id).into(),
                    );
                }
                let spent = paddock::devices::ledger::spent_of(env, &r.id);
                if let (Some(o), false) = (v.as_object_mut(), spent.is_empty()) {
                    let rows: Vec<serde_json::Value> = spent
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "device": s.device,
                                "spent": paddock::devices::ledger::money(s.spent),
                                "budget": paddock::devices::ledger::money(s.budget),
                            })
                        })
                        .collect();
                    o.insert("spent".into(), rows.into());
                }
                // The device instances of the sandbox (devices/sandbox).
                let devs = paddock::devices::sandbox::state::InstanceState::of_caller(env, &r.id);
                if let (Some(o), false) = (v.as_object_mut(), devs.is_empty()) {
                    let now = state::now();
                    o.insert(
                        "devices".into(),
                        devs.iter().map(|d| d.json(now)).collect::<Vec<_>>().into(),
                    );
                }
                let tunnels = paddock::monitor::read_seen(env, &r.id);
                if let (Some(o), false) = (v.as_object_mut(), tunnels.is_empty()) {
                    o.insert("tunnels".into(), tunnels.into());
                }
                if let (Some(o), Some(t)) = (
                    v.as_object_mut(),
                    paddock::monitor::read_traffic(env, &r.id),
                ) {
                    o.insert(
                        "traffic".into(),
                        serde_json::json!({ "sent": t.sent, "received": t.received }),
                    );
                }
                let creds = paddock::monitor::read_credential_files(env, &r.id);
                if let (Some(o), false) = (v.as_object_mut(), creds.is_empty()) {
                    o.insert(
                        "credential_files".into(),
                        serde_json::to_value(creds).unwrap_or_default(),
                    );
                }
                // What the panel marks: not acknowledged, not muted.
                let marks = paddock::monitor::marks(env, &r.id, &r.project);
                if let (Some(o), false) = (v.as_object_mut(), marks.is_empty()) {
                    o.insert(
                        "monitor_marks".into(),
                        serde_json::to_value(marks).unwrap_or_default(),
                    );
                }
                if mem {
                    v["memory"] =
                        serde_json::to_value(paddock::memory::of_run(r)).unwrap_or_default();
                }
                v
            })
            .collect();
        let now = state::now();
        let shared: Vec<serde_json::Value> =
            paddock::devices::sandbox::state::InstanceState::shared(env)
                .iter()
                .map(|d| d.json(now))
                .collect();
        println!(
            "{}",
            serde_json::json!({
                "api_version": "paddock.ps/v1",
                "shared_devices": shared,
                "sandboxes": rows,
                "ended": paddock::detach::recent_ended(env, state::now()),
            })
        );
    } else {
        // A shared device instance serves every run, so it stands once, at
        // the top, with the runs that hold a lease on it.
        let now = state::now();
        for d in paddock::devices::sandbox::state::InstanceState::shared(env) {
            let users = d.users.unwrap_or(0);
            println!(
                "shared device {:<15} {} user{}, running {}m, pid {}{}",
                d.device,
                users,
                if users == 1 { "" } else { "s" },
                now.saturating_sub(d.started) / 60,
                d.pid,
                match (&d.unit, &d.owner) {
                    (Some(u), _) => format!(", unit {u}"),
                    (None, Some(o)) => format!(", owned by {}", paddock::ids::short(o)),
                    (None, None) => String::new(),
                }
            );
        }
        if list.is_empty() {
            println!("No running sandboxes.");
        }
        let depth = |r: &Running| state::depth_in(&list, r);
        for r in &list {
            let id = format!("{}{}", "  ".repeat(depth(r)), r.id);
            let frozen = paddock::freeze::frozen(env, &r.id)
                .map(|(since, _)| format!("  frozen {}m", now.saturating_sub(since) / 60))
                .unwrap_or_default();
            let pasture = r
                .pasture
                .as_deref()
                .map(|p| format!("  pasture {p}"))
                .unwrap_or_default();
            let task = spec_task(env, &r.id)
                .map(|t| format!("  task {}", t.id))
                .unwrap_or_default();
            let rec = paddock::detach::Record::load(env, &r.id);
            let stuck = match rec.as_ref().and_then(|d| d.stuck.as_ref()) {
                Some(s) => format!("  stuck {}m", now.saturating_sub(s.since) / 60),
                None => String::new(),
            };
            println!(
                "{:<28} {:<8} {:<7} {:<7} {:>5}m  {}{pasture}{task}{frozen}{stuck}",
                id,
                r.harness,
                r.backend,
                r.gui,
                (now.saturating_sub(r.started)) / 60,
                r.project.display()
            );
            if mem {
                match paddock::memory::of_run(r) {
                    Some(m) => {
                        let size = |n: Option<u64>| {
                            n.map(|n| format!("{:.1} MiB", n as f64 / 1048576.0))
                                .unwrap_or_else(|| "unavailable".into())
                        };
                        println!(
                            "  memory: anon {}  file {}  swap {}",
                            size(m.anon_bytes),
                            size(m.file_bytes),
                            size(m.swap_bytes)
                        );
                        let high = if m.high_unlimited {
                            "max".to_string()
                        } else {
                            size(m.high_bytes)
                        };
                        println!("    memory.high: {high}");
                        for group in m.groups {
                            if !group.processes.is_empty() {
                                println!(
                                    "    {} (RSS {}, shared pages may be counted more than once):",
                                    group.name,
                                    size(Some(group.rss_bytes))
                                );
                                for p in group.processes {
                                    println!(
                                        "      {} {} {}",
                                        p.pid,
                                        p.name,
                                        size(Some(p.rss_bytes))
                                    );
                                }
                            }
                        }
                    }
                    None => println!("  memory: unavailable (no accessible sandbox cgroup)"),
                }
            }
            for n in rec.map(|d| d.notes).unwrap_or_default() {
                println!("{:<28} {n}", "");
            }
            for d in paddock::devices::sandbox::state::InstanceState::of_caller(env, &r.id) {
                println!(
                    "{:<28} device {} ({}, running {}m, pid {})",
                    "",
                    d.device,
                    d.run,
                    now.saturating_sub(d.started) / 60,
                    d.pid
                );
            }
        }
    }
    Ok(0)
}

pub(crate) fn freeze(env: &HostEnv, id: String, tree: bool, reason: String) -> Result<i32> {
    for f in paddock::freeze::freeze(env, &id, tree, &reason)? {
        let short = paddock::ids::short(&f);
        println!("Froze {short}. Thaw it with: paddock thaw {short}");
    }
    Ok(0)
}

pub(crate) fn thaw(env: &HostEnv, id: String, tree: bool) -> Result<i32> {
    for f in paddock::freeze::thaw(env, &id, tree)? {
        println!("Thawed {f}.");
    }
    Ok(0)
}

/// The Spec Kit task that a running sandbox holds.
fn spec_task(env: &HostEnv, id: &str) -> Option<paddock::speckit::board::Record> {
    let root = paddock::speckit::tree_root_of(env, id)?;
    paddock::speckit::record_of(env, &root, id).filter(|t| t.state.held())
}

pub(crate) fn view(env: &HostEnv, id: String, write: bool) -> Result<i32> {
    paddock::view::open(env, &id, write)?;
    Ok(0)
}

pub(crate) fn boundary(env: &HostEnv, id: String, json: bool, follow: bool) -> Result<i32> {
    paddock::boundary::cmd(env, &id, json, follow)
}

/// The most lines `paddock view --tail` prints. The sources hold no more:
/// the tmux capture keeps the last 120 lines of the pane, and the view log
/// is read from its last 8 KiB.
const TAIL_MAX: usize = 120;

/// `paddock view ID --tail N`: the end of the output of a run, as the
/// terminal UI shows it beside the run. A detached run keeps its output in
/// its tmux pane, another run in its view log.
pub(crate) fn view_tail(env: &HostEnv, id: &str, lines: usize, json: bool) -> Result<i32> {
    let n = lines.clamp(1, TAIL_MAX);
    let pane = paddock::view::Tmux::find(env)
        .filter(|t| t.has(id))
        .and_then(|t| t.pane_tail(id));
    let (source, text) = match pane {
        Some(text) => (Some("tmux"), Some(text)),
        None => match paddock::view::read_tail(&paddock::view::log_file(env, id)) {
            Some(text) => (Some("log"), Some(text)),
            None => (None, None),
        },
    };
    let rows = text
        .as_deref()
        .map(|t| tail_lines(t, n))
        .unwrap_or_default();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "api_version": "paddock.view-tail/v1",
                "id": id,
                "source": source,
                "lines": rows,
            })
        );
    } else {
        for l in rows {
            println!("{l}");
        }
    }
    Ok(0)
}

/// The last `n` lines of terminal output, as a terminal shows them: `\r`
/// starts the line again, escapes and control characters go. A tmux
/// capture also holds the empty screen area of the agent, so empty lines
/// at the start and the end go, and a run of empty lines becomes one.
fn tail_lines(text: &str, n: usize) -> Vec<String> {
    let mut rows: Vec<String> = text
        .split('\n')
        .map(|l| {
            let visible = l.rsplit('\r').find(|p| !p.is_empty()).unwrap_or("");
            paddock::view::strip_controls(visible)
                .trim_end()
                .to_string()
        })
        .collect();
    rows.dedup_by(|b, a| a.is_empty() && b.is_empty());
    while rows.last().is_some_and(|l| l.is_empty()) {
        rows.pop();
    }
    if rows.first().is_some_and(|l| l.is_empty()) {
        rows.remove(0);
    }
    let skip = rows.len().saturating_sub(n);
    rows.split_off(skip)
}

#[cfg(test)]
mod tail_tests {
    #[test]
    fn tail_lines_keep_the_visible_text_of_the_last_lines() {
        let text = "one\ntwo\x1b[31m red\x1b[0m\r\nthree\rTHREE\n\x07four\x1b]0;title\x07\n\n\n";
        assert_eq!(super::tail_lines(text, 3), vec!["two red", "THREE", "four"]);
        assert_eq!(super::tail_lines(text, 10).len(), 4);
        assert!(super::tail_lines("", 5).is_empty());
    }

    #[test]
    fn tail_lines_drop_the_empty_screen_area_of_a_capture() {
        // A tmux capture: the agent drew three lines at the top of a
        // 40-line screen, with blank rows in between and below.
        let mut text = String::from("\n\n  \nfirst\n\n\n\n\nsecond\n   \n\nthird");
        text.push_str(&"\n".repeat(37));
        assert_eq!(super::tail_lines(&text, 3), vec!["second", "", "third"]);
        assert_eq!(
            super::tail_lines(&text, 120),
            vec!["first", "", "second", "", "third"]
        );
        assert!(super::tail_lines("\n \n\t\n", 10).is_empty());
    }
}
