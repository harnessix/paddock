//! Read everything that gauge measures for one feature of one project: the
//! history sources, the task records, the gate results, merges in git, and
//! the workers' run times and blocked times from the registry and the
//! mailbox.

use super::history::{self, Snapshot, Source};
use super::{appetite, evidence, Effort, Input};
use crate::env::HostEnv;
use crate::speckit::{flow, records, Layout};
use anyhow::{Context as _, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The files of a feature that paddock keeps beside its task records.
pub struct Files {
    pub dir: PathBuf,
}

impl Files {
    pub fn of(layout: &Layout, feature: &str) -> Self {
        let p = records::ProjectRecords::path_for(layout, feature);
        Files {
            dir: p.parent().map(Path::to_path_buf).unwrap_or_default(),
        }
    }
    pub fn history(&self) -> PathBuf {
        self.dir.join("history.jsonl")
    }
    pub fn gates(&self) -> PathBuf {
        self.dir.join("gates.jsonl")
    }
    pub fn appetite(&self) -> PathBuf {
        self.dir.join("appetite.json")
    }
}

/// The feature that gauge reads: the one named, or the active one.
pub fn feature(layout: &Layout, named: Option<&str>) -> Result<String> {
    match named {
        Some(f) if crate::speckit::valid_feature(f) => {
            anyhow::ensure!(
                layout.exists(&Layout::tasks_rel(f)),
                "feature {f} has no tasks.md (features: {})",
                layout.features().join(", ")
            );
            Ok(f.to_string())
        }
        Some(f) => anyhow::bail!("'{f}' is not a feature folder name"),
        None => layout
            .feature()
            .context("this Spec Kit project has no feature with a tasks.md yet"),
    }
}

/// The git versions of the feature's `tasks.md`: from the loft for the
/// external store, else from the repository when git tracks the file.
fn git_snapshots(layout: &Layout, feature: &str) -> (Vec<Snapshot>, Option<Source>) {
    let rel = Layout::tasks_rel(feature);
    let (versions, source) = if layout.external {
        // The store is the loft's tree: its git folder is beside it.
        let loft = crate::loft::Loft {
            root: layout
                .root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        };
        if !loft.initialized() {
            return (Vec::new(), None);
        }
        let git_dir = loft.git_dir();
        let v = history::git_versions(
            &|| crate::hostgit::hardened(&git_dir, None),
            &rel.display().to_string(),
        );
        (v, Source::LoftGit)
    } else {
        let Ok(Some(git)) = crate::hostgit::Layout::discover(&layout.root) else {
            return (Vec::new(), None);
        };
        let Ok(rel) = layout
            .root
            .join(&rel)
            .strip_prefix(&git.work_tree)
            .map(Path::to_path_buf)
        else {
            return (Vec::new(), None);
        };
        let v = history::git_versions(&|| git.command(), &rel.display().to_string());
        (v, Source::RepoGit)
    };
    if versions.is_empty() {
        return (Vec::new(), None);
    }
    let snaps = versions
        .into_iter()
        .map(|(at, text)| Snapshot {
            at,
            source,
            tasks: history::from_text(&text),
        })
        .collect();
    (snaps, Some(source))
}

/// Run a git check that answers with its exit code.
fn git_ok(git: &crate::hostgit::Layout, args: &[&str]) -> bool {
    let mut c = git.command();
    c.args(args);
    matches!(
        crate::broker::run_capture(&mut c, Duration::from_secs(30)),
        Ok((0, _, _))
    )
}

/// Per done task: is its branch, or a commit that its result names, in the
/// history of HEAD of the project or of its main work tree?
fn merges(
    project: &Path,
    rows: &[(String, Option<String>, String)],
) -> (bool, BTreeMap<String, bool>) {
    let mut out = BTreeMap::new();
    let Ok(Some(git)) = crate::hostgit::Layout::discover(project) else {
        return (false, out);
    };
    let mut heads = vec![git.clone()];
    if git.linked {
        if let Ok(Some(main)) = crate::hostgit::Layout::discover(&crate::speckit::repo_of(project))
        {
            heads.push(main);
        }
    }
    for (id, branch, text) in rows {
        let mut commits = Vec::new();
        if let Some(b) = branch.as_deref().filter(|b| !b.starts_with('-')) {
            let r = format!("refs/heads/{b}");
            if git_ok(
                &git,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{r}^{{commit}}"),
                ],
            ) {
                commits.push(r);
            }
        }
        commits.extend(
            evidence::commits_in(text)
                .into_iter()
                .filter(|c| git_ok(&git, &["cat-file", "-e", &format!("{c}^{{commit}}")])),
        );
        if commits.is_empty() {
            continue;
        }
        let merged = commits.iter().any(|c| {
            heads
                .iter()
                .any(|h| git_ok(h, &["merge-base", "--is-ancestor", c, "HEAD"]))
        });
        out.insert(id.clone(), merged);
    }
    (true, out)
}

/// The run time and the blocked time of a worker, from the registry and
/// its mailbox.
fn effort(env: &HostEnv, sandbox: &str, now: u64) -> Option<Effort> {
    let reg =
        crate::kv::store::Store::open(&crate::kv::shard::registry(&crate::kv::kv_root(env))).ok();
    let times = reg.and_then(|r| r.times(sandbox).ok().flatten());
    let up = crate::mail::read(env, sandbox, crate::mail::Dir::Up);
    let started = times.map(|t| t.0).or_else(|| {
        up.iter()
            .find(|m| m.event.as_deref() == Some("started"))
            .map(|m| m.ts)
    })?;
    let ended = times
        .and_then(|t| t.1)
        .or_else(|| up.iter().find(|m| m.is_end()).map(|m| m.ts))
        .unwrap_or(now);
    let mut blocked = 0;
    let mut since: Option<u64> = None;
    for m in up
        .iter()
        .filter(|m| m.kind == crate::mail::Kind::Status || m.is_end())
    {
        if let Some(s) = since.take() {
            blocked += m.ts.saturating_sub(s);
        }
        if m.kind == crate::mail::Kind::Status && m.text.starts_with("blocked") {
            since = Some(m.ts);
        }
    }
    if let Some(s) = since {
        blocked += ended.saturating_sub(s);
    }
    Some(Effort {
        agent_secs: ended.saturating_sub(started),
        blocked_secs: blocked,
    })
}

/// The oldest time of the feature's files: the start when no history
/// knows it.
fn file_start(layout: &Layout, feature: &str) -> Option<u64> {
    ["spec.md", "plan.md", "tasks.md"]
        .iter()
        .filter_map(|n| {
            let m = std::fs::metadata(layout.root.join("specs").join(feature).join(n)).ok()?;
            let t = m.modified().ok()?;
            Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
        })
        .min()
}

/// The tasks now: the file with its notes, and the state of every task
/// that paddock keeps a record of.
fn current_tasks(
    layout: &Layout,
    feature: &str,
    stored: &BTreeMap<String, crate::speckit::board::Record>,
) -> Vec<history::SnapTask> {
    let text = layout.read(&Layout::tasks_rel(feature)).unwrap_or_default();
    let mut tasks = history::from_text(&text);
    for t in tasks.iter_mut() {
        match stored.get(&t.id) {
            Some(r) => {
                t.checked = r.state == crate::speckit::board::State::Done;
                t.state = Some(r.state.name().to_string());
                if r.branch.is_some() {
                    t.branch = r.branch.clone();
                }
            }
            None if t.state.is_none() => {
                t.state = Some(if t.checked { "done" } else { "todo" }.to_string());
            }
            None => {}
        }
    }
    tasks
}

/// Read the input of one feature at `now`. `live`: now is the real time,
/// so the current file is the last snapshot, and its states go into the
/// state log. Without it, the history up to `now` is all.
pub fn collect(
    env: &HostEnv,
    project: &Path,
    named: Option<&str>,
    now: u64,
    live: bool,
) -> Result<(Input, Layout)> {
    let layout = Layout::resolve(env, project).with_context(|| {
        format!(
            "{} is no Spec Kit project (no .specify/ folder), or [speckit] enabled = false",
            project.display()
        )
    })?;
    let feature = feature(&layout, named)?;
    let repo = crate::speckit::repo_of(project);
    let root = flow::newest_tree(env, &repo, &feature, now);
    let spot = flow::Spot::find(
        env,
        project,
        root.as_deref().unwrap_or("no-run"),
        Some(&feature),
    )?
    .context("no Spec Kit project")?;
    let view = spot.sync(env, false, now)?;
    let files = Files::of(&layout, &feature);
    // Only the records that paddock stored: a task it never ran has none.
    let path = records::ProjectRecords::path_for(&layout, &feature);
    let stored: BTreeMap<String, crate::speckit::board::Record> = if path.is_file() {
        let ids = records::load(&path).unwrap_or_default();
        view.records
            .iter()
            .filter(|(id, _)| ids.contains_key(*id))
            .map(|(id, r)| (id.clone(), r.clone()))
            .collect()
    } else {
        BTreeMap::new()
    };
    let mut sources = Vec::new();
    let (mut snaps, git_source) = git_snapshots(&layout, &feature);
    sources.extend(git_source);
    let logged = history::read_log(&files.history());
    if !logged.is_empty() {
        sources.push(Source::StateLog);
    }
    snaps.extend(logged);
    if live {
        let current = current_tasks(&layout, &feature, &stored);
        history::log_states(&files.history(), current.clone(), now);
        snaps.push(Snapshot {
            at: now,
            source: Source::Current,
            tasks: current,
        });
    }
    snaps.retain(|s| s.at <= now);
    let snapshots = history::merge(snaps);
    let done_now: Vec<String> = snapshots
        .last()
        .map(|s| {
            s.tasks
                .iter()
                .filter(|t| t.checked)
                .map(|t| t.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let rows: Vec<(String, Option<String>, String)> = view
        .records
        .values()
        .filter(|r| done_now.contains(&r.id) && !r.removed)
        .map(|r| {
            let branch = r.branch.clone().or_else(|| {
                snapshots
                    .iter()
                    .rev()
                    .find_map(|s| s.task(&r.id).and_then(|t| t.branch.clone()))
            });
            let mut text = r.result.clone().unwrap_or_default();
            if let Some(p) = &r.report {
                text.push(' ');
                text.push_str(&p.text);
            }
            (r.id.clone(), branch, text)
        })
        .collect();
    let (git, merged) = merges(project, &rows);
    let effort = view
        .records
        .values()
        .filter_map(|r| Some((r.id.clone(), effort(env, r.sandbox.as_deref()?, now)?)))
        .collect();
    let input = Input {
        feature,
        now,
        start_hint: sources
            .is_empty()
            .then(|| file_start(&layout, &view.feature))
            .flatten(),
        snapshots,
        sources,
        records: stored,
        gates: evidence::read_gates(&files.gates()),
        merged,
        git,
        effort,
        appetite: appetite::load(&files.appetite()),
    };
    Ok((input, layout))
}
