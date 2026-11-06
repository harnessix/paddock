//! The sweep of stale device mounts: which `fuse.paddock` mounts of
//! `/proc/self/mountinfo` a start unmounts.

use super::{probe, stale_mounts, Mount};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// One mountinfo line of a `fuse.paddock` mount at `mp`.
fn fuse_line(mp: &str) -> String {
    format!(
        "412 50 0:61 / {mp} rw,nosuid,nodev,noexec,noatime - fuse.paddock paddock rw,user_id=1000,group_id=1000,default_permissions"
    )
}

/// The mountinfo of a host with the device mounts of `ids` under `run`,
/// the srt mount of `srt`, and some mounts that are not paddock's.
fn mountinfo(run: &Path, ids: &[&str], srt: &str) -> String {
    let r = run.display();
    let mut lines = vec![
        "25 1 0:22 / / rw,relatime shared:1 - ext4 /dev/root rw".to_string(),
        format!("300 25 0:40 / {r} rw,nosuid,nodev,relatime shared:5 - tmpfs tmpfs rw,size=4096k"),
        // Another FUSE file system in the run folder: not paddock's.
        format!("301 300 0:42 / {r}/other-1/dev rw - fuse.sshfs sshfs rw"),
        // A paddock mount outside the run folder.
        fuse_line("/tmp/elsewhere/agent-p-9/dev"),
        // A paddock mount that is not `<id>/dev`.
        fuse_line(&format!("{r}/agent-p-8/other")),
    ];
    for id in ids {
        lines.push(fuse_line(&format!("{r}/{id}/dev")));
    }
    if !srt.is_empty() {
        lines.push(fuse_line(&format!("{r}/devices/{srt}")));
    }
    lines.join("\n") + "\n"
}

/// A probe that records what it was asked and answers from `answers`.
struct Probe {
    asked: RefCell<Vec<PathBuf>>,
    answers: Box<dyn Fn(&Path) -> Mount>,
}

impl Probe {
    fn new(answers: impl Fn(&Path) -> Mount + 'static) -> Self {
        Self {
            asked: RefCell::new(Vec::new()),
            answers: Box::new(answers),
        }
    }

    fn ask(&self, mp: &Path) -> Mount {
        self.asked.borrow_mut().push(mp.to_path_buf());
        (self.answers)(mp)
    }
}

/// The failure of two runs that start at once: the first run mounted its
/// devices but has not registered itself yet when the second run sweeps.
/// Its file system answers, so it stays.
#[test]
fn a_live_mount_of_a_sandbox_that_is_not_registered_yet_stays() {
    let run = Path::new("/run/user/1000/paddock");
    let info = mountinfo(run, &["agent-p-1", "agent-p-2"], "agent-p-3");
    let p = Probe::new(|_| Mount::Answers);
    let gone = stale_mounts(&info, run, &["agent-p-1".to_string()], |mp| p.ask(mp));
    assert!(gone.is_empty(), "{gone:?}");
    // The registered sandbox owns its mount and is not asked; the two
    // others are, once each. The mounts that are not paddock's device
    // mounts are never asked.
    assert_eq!(
        *p.asked.borrow(),
        [run.join("agent-p-2/dev"), run.join("devices/agent-p-3"),]
    );
}

/// A mount whose process is gone refuses the probe and is unmounted;
/// `devices/<id>` of srt is one too.
#[test]
fn a_mount_whose_process_is_gone_is_swept() {
    let run = Path::new("/run/user/1000/paddock");
    let info = mountinfo(run, &["agent-p-1", "agent-p-2"], "agent-p-3");
    let p = Probe::new(|_| Mount::Gone);
    let gone = stale_mounts(&info, run, &[], |mp| p.ask(mp));
    assert_eq!(
        gone,
        [
            run.join("agent-p-1/dev"),
            run.join("agent-p-2/dev"),
            run.join("devices/agent-p-3"),
        ]
    );
}

/// A mount that does not answer in time is nobody's to unmount: a slow
/// file system of a live run is not stale.
#[test]
fn a_silent_mount_is_left_alone() {
    let run = Path::new("/run/user/1000/paddock");
    let info = mountinfo(run, &["agent-p-1", "agent-p-2"], "");
    let p = Probe::new(|mp: &Path| {
        if mp.ends_with("agent-p-1/dev") {
            Mount::Silent
        } else {
            Mount::Gone
        }
    });
    let gone = stale_mounts(&info, run, &[], |mp| p.ask(mp));
    assert_eq!(gone, [run.join("agent-p-2/dev")]);
}

/// A registered sandbox is never asked, whatever its mount would answer.
#[test]
fn a_registered_sandbox_owns_its_mount() {
    let run = Path::new("/run/user/1000/paddock");
    let info = mountinfo(run, &["agent-p-1"], "agent-p-3");
    let p = Probe::new(|_| Mount::Gone);
    let running = ["agent-p-1".to_string(), "agent-p-3".to_string()];
    let gone = stale_mounts(&info, run, &running, |mp| p.ask(mp));
    assert!(gone.is_empty(), "{gone:?}");
    assert!(p.asked.borrow().is_empty(), "{:?}", p.asked.borrow());
}

/// The real probe: a plain folder answers, and a path that is not there
/// says nothing about a process, so it is not `Gone`.
#[test]
fn the_probe_tells_a_folder_that_answers_from_one_that_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(probe(dir.path()), Mount::Answers);
    assert_eq!(probe(&dir.path().join("missing")), Mount::Silent);
}
