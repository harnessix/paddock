//! The files and links the escape probe plants for the host, and the
//! checks that the host handles them safely after the run.

use crate::env::HostEnv;
use std::path::{Path, PathBuf};

/// Things the probe leaves behind for the host, as a sandbox could between
/// runs: a FIFO and a `/dev/zero` link at files the host reads after exit,
/// and links in the home where a mount point could go.
pub(super) struct Planted {
    project: PathBuf,
    home_src: PathBuf,
    home: PathBuf,
    watched: std::collections::BTreeMap<PathBuf, Option<String>>,
}

/// Names in the probe; keep them in step with probe.sh.
const PLANTED_FIFO: &str = ".envrc";
const PLANTED_ZERO: &str = ".vscode/tasks.json";
const PLANTED_DIR_LINK: &str = ".paddock-probe-dir-link";
const PLANTED_FILE_LINK: &str = ".paddock-probe-file-link";
const LINK_TARGET_DIR: &str = "paddock-escape-link-dir";
const LINK_TARGET_FILE: &str = "paddock-escape-link-file";

impl Planted {
    pub(super) fn before(spec: &crate::spec::SandboxSpec) -> Self {
        let home_src = spec.home_src.clone();
        let r = crate::safefs::Root::open(&home_src);
        for n in [PLANTED_DIR_LINK, PLANTED_FILE_LINK] {
            if let Ok(r) = &r {
                let _ = r.remove_tree(Path::new(n));
            }
        }
        if let Some(p) = home_src.parent() {
            let _ = std::fs::remove_dir_all(p.join(LINK_TARGET_DIR));
            let _ = std::fs::remove_file(p.join(LINK_TARGET_FILE));
        }
        Self {
            project: spec.project.clone(),
            home_src,
            home: spec.home.clone(),
            watched: crate::state::snapshot(&spec.watch),
        }
    }

    pub(super) fn check(&self, env: &HostEnv) -> Vec<(&'static str, Option<String>)> {
        let mut out = Vec::new();
        // The host compares watched files after exit. It must not hang on
        // the FIFO or read /dev/zero, and it must report both.
        let fifo = self.project.join(PLANTED_FIFO);
        let zero = self.project.join(PLANTED_ZERO);
        if kind_of(&fifo) == Some(crate::safefs::Kind::Other)
            && kind_of(&zero) == Some(crate::safefs::Kind::Symlink)
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let w = self.watched.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::state::changed(&w));
            });
            out.push((
                "host-reads-planted-files-safely",
                match rx.recv_timeout(std::time::Duration::from_secs(20)) {
                    Ok(c) if c.contains(&fifo) && c.contains(&zero) => None,
                    Ok(c) => Some(format!("not reported as changed: {c:?}")),
                    Err(_) => Some("the host read of a planted FIFO or /dev/zero link hung".into()),
                },
            ));
        } else {
            out.push((
                "host-reads-planted-files-safely",
                Some("the probe did not plant the FIFO and the link".into()),
            ));
        }
        let _ = std::fs::remove_file(&fifo);
        let _ = std::fs::remove_file(&zero);

        // The next container start makes mount points in the home. Links
        // the sandbox planted there must not lead it outside.
        let mut s = crate::spec::SandboxSpec::new(
            "probe".into(),
            "doctor",
            &self.project,
            self.home_src.clone(),
            &self.home,
            env.uid,
            env.gid,
            &env.user,
        );
        let links = [PLANTED_DIR_LINK, PLANTED_FILE_LINK]
            .iter()
            .all(|n| kind_of(&self.home_src.join(n)) == Some(crate::safefs::Kind::Symlink));
        // Only with the links in place: else this would make real folders.
        let refused = links
            && [
                crate::spec::Mount::tmpfs(self.home.join(PLANTED_DIR_LINK).join("x"), "doctor"),
                crate::spec::Mount::mask(self.home.join(PLANTED_FILE_LINK), false, "doctor"),
            ]
            .into_iter()
            .all(|m| {
                s.mounts = vec![m];
                crate::backend::docker::precreate_mount_points(&s).is_err()
            });
        let parent = self.home_src.parent().unwrap_or(Path::new("/nonexistent"));
        let escaped = [LINK_TARGET_DIR, LINK_TARGET_FILE]
            .iter()
            .map(|n| parent.join(n))
            .find(|p| std::fs::symlink_metadata(p).is_ok());
        out.push((
            "mount-points-never-follow-planted-links",
            match (links, refused, escaped) {
                (false, _, _) => Some("the probe did not plant the links".into()),
                (_, _, Some(p)) => Some(format!("created {}", p.display())),
                (_, false, _) => Some("a planted link was accepted as a mount point".into()),
                _ => None,
            },
        ));
        if let Ok(r) = crate::safefs::Root::open(&self.home_src) {
            for n in [PLANTED_DIR_LINK, PLANTED_FILE_LINK] {
                let _ = r.remove_tree(Path::new(n));
            }
        }
        let _ = std::fs::remove_dir_all(parent.join(LINK_TARGET_DIR));
        let _ = std::fs::remove_file(parent.join(LINK_TARGET_FILE));
        out
    }
}

fn kind_of(p: &Path) -> Option<crate::safefs::Kind> {
    use crate::safefs::Kind;
    use std::os::unix::fs::FileTypeExt;
    let t = std::fs::symlink_metadata(p).ok()?.file_type();
    Some(if t.is_symlink() {
        Kind::Symlink
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_file() {
        Kind::File
    } else if t.is_socket() {
        Kind::Socket
    } else {
        Kind::Other
    })
}
