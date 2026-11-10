use super::*;
use crate::tack::proto::Program as ProgFile;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn decision_for(p: &Path) -> Decision {
    let m = std::fs::metadata(p).unwrap();
    Decision {
        verdict: crate::tack::proto::Verdict::Allow,
        reason: String::new(),
        secrets: vec![],
        hosts: Default::default(),
        program: Some(ProgFile {
            path: p.display().to_string(),
            real: p.display().to_string(),
            sha256: crate::tack::program::sha256_file(p).unwrap(),
            dev: m.dev(),
            ino: m.ino(),
            signer: None,
        }),
        chain: vec![],
    }
}

#[test]
fn the_started_file_must_be_the_checked_file() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("tool");
    std::fs::write(&p, "#!/bin/sh\nprintf \"%s\\n\" one\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = decision_for(&p);
    let cache = HashCache::default();
    let (_, script) = open_program(&d, &cache).unwrap();
    assert!(script);
    // Same file, new content.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&p, "#!/bin/sh\nprintf \"%s\\n\" two\n").unwrap();
    let e = open_program(&d, &cache).unwrap_err();
    assert!(e.contains("changed after tackd checked it"), "{e}");
    // Another file in its place.
    let other = t.path().join("other");
    std::fs::copy("/bin/true", &other).unwrap();
    std::fs::rename(&other, &p).unwrap();
    let e = open_program(&d, &cache).unwrap_err();
    assert!(e.contains("replaced"), "{e}");
    let (_, script) = open_program(&decision_for(&p), &cache).unwrap();
    assert!(!script, "a binary is started from its open file");
    // A link in the last step is not followed.
    let link = t.path().join("link");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    let mut d = decision_for(&p);
    d.program.as_mut().unwrap().real = link.display().to_string();
    assert!(open_program(&d, &cache).is_err());
}

#[test]
fn limits_never_go_above_what_tackd_may_set() {
    let mut own: libc::rlimit = unsafe { std::mem::zeroed() };
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut own) };
    let c = Caller {
        uid: 1,
        gid: 1,
        groups: vec![],
        umask: 0o22,
        limits: vec![
            caller::Limit {
                resource: libc::RLIMIT_NOFILE as i32,
                soft: None,
                hard: None,
            },
            caller::Limit {
                resource: libc::RLIMIT_CORE as i32,
                soft: Some(0),
                hard: Some(0),
            },
        ],
        cgroup: None,
        nice: None,
    };
    let l = limits(&c);
    assert_eq!(l.len(), 2);
    let nofile = l
        .iter()
        .find(|(r, _)| *r == libc::RLIMIT_NOFILE as i32)
        .unwrap()
        .1;
    assert_eq!(nofile.rlim_max, own.rlim_max);
    assert_eq!(nofile.rlim_cur, own.rlim_max);
    let core = l
        .iter()
        .find(|(r, _)| *r == libc::RLIMIT_CORE as i32)
        .unwrap()
        .1;
    assert_eq!((core.rlim_cur, core.rlim_max), (0, 0));
}

#[test]
fn a_signal_after_the_end_reaches_no_other_process() {
    // Child::next reaps it, not std.
    #[allow(clippy::zombie_processes)]
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 4"])
        .spawn()
        .unwrap();
    let c = Child::adopt(child.id());
    assert_eq!(c.next(), Event::Exited { code: 4 });
    let e = c.signal(libc::SIGTERM, true).unwrap_err();
    assert!(e.contains("ended"), "{e}");
}

#[test]
fn the_caller_env_for_the_store_tools_has_no_loader_variables() {
    let p = pairs(&[
        "A=1".into(),
        "LD_PRELOAD=x".into(),
        "=bad".into(),
        "B=x=y".into(),
    ]);
    assert_eq!(p, [("A".into(), "1".into()), ("B".into(), "x=y".into())]);
    // Not root, and the caller's own user: no change of user (tests).
    let me = unsafe { libc::geteuid() };
    if me != 0 {
        assert!(!must_drop(me));
        assert!(must_drop(me + 1));
    }
}
