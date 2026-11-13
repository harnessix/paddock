//! The claims of `/private/tmp` in paddock: at the start of a Seatbelt run,
//! in its profile, at its end, in `paddock gc` and in `paddock doctor`.

use super::{
    claim, load_all, release, release_dead, resolve, stop_stale, HostProcs, Place, Record,
    Resolved, Vars, WantKind, ROOT,
};
use crate::env::HostEnv;
use crate::seatbelt::{Access, Policy};
use crate::spec::SandboxSpec;
use std::path::{Path, PathBuf};

pub fn records(env: &HostEnv) -> PathBuf {
    env.paddock_state().join(super::RECORDS)
}

fn live(pid: u32) -> bool {
    crate::state::alive(pid)
}

fn place<'a>(env: &HostEnv, records: &'a Path, live: &'a dyn Fn(u32) -> bool) -> Place<'a> {
    Place {
        root: Path::new(ROOT),
        records,
        uid: env.uid,
        live,
        procs: &HostProcs,
    }
}

/// A warning that never fails: at the end of a run the terminal can be
/// gone (a closed window), and a panic there would skip the rest of the end.
fn warn(text: &str) {
    use std::io::Write as _;
    log::warn!("/private/tmp: {text}");
    let _ = writeln!(std::io::stderr(), "paddock: warning: {text}");
}

/// The run folders of the live runs: their processes are never stale.
fn live_run_folders(env: &HostEnv) -> Vec<PathBuf> {
    crate::state::Running::list(env)
        .into_iter()
        // A sandbox whose paddock process is gone (kill -9) is not live:
        // what it left is stale (TMP1-F3).
        .filter(|r| r.host_pid <= 1 || live(r.host_pid))
        .map(|r| crate::backend::seatbelt::real(&env.paddock_run().join(&r.id)))
        .collect()
}

/// Every paddock command on macOS: release the records of runs whose
/// paddock process is gone (a kill -9), so their processes stop and their
/// names go. Cheap when there is nothing to do: one folder listing.
pub fn sweep(env: &HostEnv) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let dir = records(env);
    if !dir.is_dir() {
        return;
    }
    for p in release_dead(&place(env, &dir, &live)) {
        warn(&p);
    }
}

/// At the start of a Seatbelt run: claim the `[[macos.tmp]]` entries of the
/// profile, note what the run gets, and drop the launch flags that a
/// granted claim makes needless (`--no-daemon` of codex).
pub fn start_run(
    env: &HostEnv,
    spec: &mut SandboxSpec,
    profile: &crate::profile::Profile,
    exe: &Path,
) {
    let wants = &profile.macos.tmp;
    if wants.is_empty() {
        return;
    }
    let home = crate::backend::seatbelt::real(&spec.home_src);
    let vars = Vars {
        uid: env.uid,
        home: &home,
        run: &spec.id,
    };
    let mut resolved: Vec<Resolved> = Vec::new();
    for w in wants {
        match resolve(w, &vars) {
            Ok(r) => resolved.push(r),
            Err(e) => crate::note::warn(format!(
                "the {} profile names a bad /private/tmp entry: {e:#}",
                profile.name
            )),
        }
    }
    let dir = records(env);
    // A server of an earlier run of this home that lost its record: it
    // holds the socket of this home, and codex would connect to it.
    let (stopped, problems) = stop_stale(&place(env, &dir, &live), &home, &live_run_folders(env));
    if !stopped.is_empty() {
        crate::note::info(format!(
            "stopped {} processes that an earlier run of this sandbox home left: {stopped:?}",
            stopped.len()
        ));
    }
    for p in problems {
        crate::note::warn(p);
    }
    let got = match claim(
        &place(env, &dir, &live),
        &spec.id,
        std::process::id(),
        &resolved,
    ) {
        Ok(c) => c,
        Err(e) => {
            crate::note::warn(format!(
                "no fixed names in /private/tmp for this run: {e:#}"
            ));
            return;
        }
    };
    if let Some(mut rec) = got.record {
        // Never the run folder: other paddock processes remove it once the
        // paddock process of the run is gone (TMP1-F5).
        match super::make_probe(&dir, &spec.id) {
            Ok(p) => rec.probe_read = p,
            Err(e) => warn(&format!(
                "{e:#}: the end of this run cannot find its processes"
            )),
        }
        rec.probe_write =
            crate::backend::seatbelt::real(&crate::backend::seatbelt::run::private_tmp(spec));
        rec.home = home.clone();
        if let Err(e) = super::save(&dir, &rec) {
            log::warn!("{e:#}");
        }
    }
    for l in &got.granted {
        log::info!("/private/tmp: {l}");
    }
    for l in &got.refused {
        crate::note::warn(l.clone());
    }
    // Only a flag that the harness took from `args_if_supported` goes.
    let supported = crate::run::supported_args(
        env,
        exe,
        &profile.launch.args_if_supported,
        &profile.launch.probe_args,
    );
    for a in got.drop_args.iter().filter(|a| supported.contains(a)) {
        if let Some(i) = spec.command.iter().skip(1).position(|c| c == a) {
            spec.command.remove(i + 1);
            log::info!(
                "{a} is not needed: the claim in /private/tmp gives the harness what it needs"
            );
        }
    }
}

/// The Seatbelt rules of the claims of `run`: each entry alone, never the
/// root or the entries of the host and of other runs.
pub fn allow(env: &HostEnv, pol: &mut Policy, run: &str) {
    let Some(rec) = load_all(&records(env)).into_iter().find(|r| r.run == run) else {
        return;
    };
    policy(pol, &rec);
}

pub fn policy(pol: &mut Policy, rec: &Record) {
    use super::ItemKind;
    let probe = &rec.probe_read;
    if probe
        .file_name()
        .is_some_and(|n| n.to_string_lossy() == format!("{}{}", rec.run, super::PROBE))
    {
        pol.add_file(
            probe,
            Access::ReadOnly,
            "the probe of this run: the end finds the processes of this run by it",
        );
    }
    for i in &rec.items {
        let p = rec.root.join(&i.path);
        match i.kind {
            ItemKind::Dir => pol.add(p, Access::ReadWrite, "a /private/tmp folder of this run"),
            ItemKind::File => pol.add_file(p, Access::ReadWrite, "a /private/tmp file of this run"),
            ItemKind::Socket | ItemKind::Entry => pol.add_file(
                p,
                Access::SocketFile,
                "a /private/tmp entry of this run: the file or socket at this name only",
            ),
            ItemKind::Folder => pol.add_file(
                p,
                Access::CreateOnly,
                "a /private/tmp folder of all runs: it may be made again, never read or changed",
            ),
        }
    }
}

/// At the end of a run: stop its named processes and remove its entries.
pub fn end_run(env: &HostEnv, run: &str) {
    let dir = records(env);
    let has = dir.join(format!("{run}.json")).exists();
    log::debug!("/private/tmp: end of run {run}, record {has}");
    if dir.is_dir() {
        let _ = crate::privfs::write_atomic(
            &dir.join(".last-end"),
            format!(
                "{} {run} record={has} by={}\n",
                crate::state::now(),
                std::process::id()
            ),
        );
    }
    if !has {
        return;
    }
    for p in release(&place(env, &dir, &live), run) {
        warn(&p);
    }
}

/// `paddock gc`: the records of runs that ended without their end step.
pub fn gc_items(env: &HostEnv) -> Vec<crate::storage::Item> {
    load_all(&records(env))
        .into_iter()
        .filter(|r| !live(r.pid))
        .map(|r| crate::storage::Item {
            what: format!(
                "what run {} left in /private/tmp: {}",
                r.run,
                r.names().join(", ")
            ),
            tier: crate::storage::Tier::Auto,
            bytes: 0,
            reason: "the run ended without its end step".into(),
            warning: None,
            needs: vec![],
            action: crate::storage::Action::Tmp { run: r.run },
        })
        .collect()
}

/// `paddock gc --yes`: release the record of an ended run.
pub fn gc_remove(env: &HostEnv, run: &str) -> anyhow::Result<()> {
    let dir = records(env);
    if load_all(&dir).iter().any(|r| r.run == run && live(r.pid)) {
        anyhow::bail!("the run is live again; skipped");
    }
    let p = release(&place(env, &dir, &live), run);
    if p.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{}", p.join("; "))
    }
}

/// The lines of `paddock doctor` on macOS: what a Seatbelt run gets for
/// `/tmp`.
pub fn doctor_lines(env: &HostEnv, root: &Path) -> Vec<String> {
    use std::os::unix::fs::MetadataExt as _;
    let mut v = vec![
        "  ok       TMPDIR is a private folder of each run: programs that use $TMPDIR get their own /tmp".to_string(),
        format!("  ok       {} is denied: a run cannot read or change what the host and other runs keep there", root.display()),
    ];
    let home = PathBuf::from("/HOME");
    let vars = Vars {
        uid: env.uid,
        home: &home,
        run: "RUN",
    };
    for (name, text) in crate::profile::BUILTIN {
        let Ok(p) = toml::from_str::<crate::profile::Profile>(text) else {
            continue;
        };
        for w in p.macos.tmp.iter().filter_map(|w| resolve(w, &vars).ok()) {
            let path = root.join(&w.name);
            let state = match (std::fs::symlink_metadata(&path), w.kind) {
                (Err(_), _) => "free".to_string(),
                (Ok(m), WantKind::Shared)
                    if m.file_type().is_dir()
                        && m.uid() == env.uid
                        && m.mode() & 0o777 == 0o700 =>
                {
                    "a folder of mode 700: each run gets its own entries in it".to_string()
                }
                (Ok(m), WantKind::Shared) => format!(
                    "warn: not a folder of uid {} with mode 700 (mode {:o}): {name} runs go without it",
                    env.uid,
                    m.mode() & 0o777
                ),
                (Ok(_), _) => "in use: a run of a profile that names it goes without it".into(),
            };
            v.push(format!(
                "           {name} claims {} ({:?}): {state}",
                path.display(),
                w.kind
            ));
        }
    }
    let all = load_all(&records(env));
    let left = all.iter().filter(|r| !live(r.pid)).count();
    v.push(format!(
        "           {} runs hold names in {} now",
        all.len() - left,
        root.display()
    ));
    if left > 0 {
        v.push(format!(
            "  warn     {left} ended runs left names there: paddock gc --yes removes them"
        ));
    }
    v
}

/// The section of `paddock doctor` on macOS.
pub fn print_doctor(env: &HostEnv) {
    println!("\n/tmp in a Seatbelt run:");
    for l in doctor_lines(env, Path::new(ROOT)) {
        println!("{l}");
    }
}

/// `paddock tmp debug [RUN]`: what the end of a run sees (TMP1-F3). The
/// records, the last release, and for each process of the user what the
/// kernel tells about its sandbox. It stops and releases nothing, so it
/// skips the sweep of main.rs. It lists the runs, and that removes the run
/// folders of runs whose paddock process is gone, as every listing does.
pub fn debug(env: &HostEnv, run: Option<&str>) -> anyhow::Result<()> {
    let dir = records(env);
    let mut recs: Vec<Record> = load_all(&dir)
        .into_iter()
        .filter(|r| run.is_none_or(|x| r.run == x))
        .collect();
    println!("records in {}: {}", dir.display(), recs.len());
    for r in &recs {
        if !live(r.pid) {
            println!(
                "  run {}: its paddock process is gone. The next paddock command other than paddock tmp releases it",
                r.run
            );
        }
        println!(
            "  run {} pid {} (alive {}) home {} probe_read {} (exists {}) probe_write {} (exists {}) names {:?}",
            r.run,
            r.pid,
            live(r.pid),
            r.home.display(),
            r.probe_read.display(),
            r.probe_read.exists(),
            r.probe_write.display(),
            r.probe_write.exists(),
            r.names()
        );
    }
    match std::fs::read_to_string(dir.join(".last-end")) {
        Ok(t) => print!("last end step: {t}"),
        Err(e) => println!("last end step: none ({e})"),
    }
    match std::fs::read(dir.join(super::LAST_RELEASE))
        .ok()
        .and_then(|b| serde_json::from_slice::<super::LastRelease>(&b).ok())
    {
        Some(l) => {
            println!(
                "last release: time {} (now {}) by pid {} run {} stopped {:?} left {:?} probe_read exists {} probe_write exists {}",
                l.time,
                crate::state::now(),
                l.by,
                l.record.run,
                l.stopped,
                l.left,
                l.probe_read_exists,
                l.probe_write_exists
            );
            if run.is_none_or(|x| l.record.run == x) && !recs.iter().any(|r| r.run == l.record.run)
            {
                recs.push(l.record);
            }
        }
        None => println!("last release: none"),
    }
    let keep = live_run_folders(env);
    println!("live run folders (kept by a start): {keep:?}");
    for r in crate::state::Running::list(env) {
        println!(
            "  running {} pid {} (alive {}) host_pid {} (alive {})",
            r.id,
            r.pid,
            live(r.pid),
            r.host_pid,
            live(r.host_pid)
        );
    }
    let p = HostProcs;
    use super::Procs as _;
    for pid in p.list() {
        let mut line = super::macos::describe(pid);
        for r in &recs {
            line.push_str(&format!(
                " | run {}: {} of_run {} of_home {}",
                r.run,
                super::macos::probes(pid, r),
                p.of_run(pid, r),
                p.of_home(pid, &r.home, &keep)
            ));
        }
        println!("{line}");
    }
    Ok(())
}
