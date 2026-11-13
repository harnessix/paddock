//! Tests of the copy. Each test has its own temporary home, data folder
//! and project.

use super::apply::{self, OnConflict, Opts};
use super::changes::{self, Conflict, Kind, Status};
use super::run::Run;
use super::{start, Config};
use crate::env::HostEnv;
use std::path::{Path, PathBuf};

mod diff;
mod disk;
mod end;
mod flow;
mod pointer;
mod restore;

pub(super) struct World {
    pub t: tempfile::TempDir,
    pub env: HostEnv,
    pub cfg: Config,
    pub project: PathBuf,
}

pub(super) fn put(dir: &Path, rel: &str, text: &str) {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, text).unwrap();
}

pub(super) fn get(dir: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(rel)).ok()
}

/// A project with three files, outside git.
pub(super) fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let project = t.path().join("home/webshop");
    put(&project, "README.md", "the shop\n");
    put(&project, "src/cart.ts", "one\ntwo\nthree\n");
    put(&project, "src/price.ts", "price\n");
    World {
        t,
        env,
        cfg: Config::default(),
        project,
    }
}

impl World {
    /// Start a run on a copy. A question about the pointer fails the test.
    pub fn start(&self, id: &str) -> Run {
        start::create(
            &self.env,
            &self.cfg,
            &self.project,
            (id, "claude"),
            true,
            &mut |state, q| panic!("unexpected question ({state:?}): {q}"),
        )
        .unwrap()
        .run
    }

    pub fn apply_all(&self, run: &mut Run) -> apply::Applied {
        apply::apply(&self.env, run, &Opts::default()).unwrap()
    }

    pub fn apply(&self, run: &mut Run, paths: &[&str], on: OnConflict) -> apply::Applied {
        let o = Opts {
            paths: paths.iter().map(|s| s.to_string()).collect(),
            on_conflict: on,
            token: None,
        };
        apply::apply(&self.env, run, &o).unwrap()
    }
}

pub(super) fn status(run: &Run) -> Status {
    changes::status(run).unwrap()
}

/// The changes as `(path, kind, conflict)`.
pub(super) fn rows(st: &Status) -> Vec<(String, Kind, Option<Conflict>)> {
    st.changes
        .iter()
        .map(|c| (c.path.clone(), c.kind, c.conflict))
        .collect()
}

pub(super) fn paths(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
