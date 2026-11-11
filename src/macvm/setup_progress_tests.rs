use super::*;
use crate::eventstream::schema::Setup;

fn layout() -> (tempfile::TempDir, Layout) {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    std::fs::create_dir_all(&l.root).unwrap();
    (t, l)
}

/// Hold the setup lock as a build does.
fn hold(l: &Layout) -> std::fs::File {
    use std::os::fd::AsRawFd;
    let f = std::fs::File::create(&l.setup_lock).unwrap();
    assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
    f
}

#[test]
fn running_follows_the_setup_lock() {
    let (_t, l) = layout();
    assert!(!running(&l));
    let f = hold(&l);
    assert!(running(&l));
    drop(f);
    assert!(!running(&l), "a lock file without a holder is no build");
}

#[test]
fn pause_only_while_a_build_downloads() {
    let (_t, l) = layout();
    assert_eq!(request_pause(&l).unwrap(), Pause::NoBuild);
    assert!(!paused(&l));
    let _f = hold(&l);
    let mut r = Reporter::file_only(&l);
    r.step("install");
    assert_eq!(
        request_pause(&l).unwrap(),
        Pause::PastDownload("install".into())
    );
    assert!(!paused(&l));
    r.step("fetch");
    assert_eq!(request_pause(&l).unwrap(), Pause::Requested);
    assert!(paused(&l));
    assert!(resume(&l), "a running build continues by itself");
    assert!(!paused(&l));
    for p in [
        Pause::Requested,
        Pause::NoBuild,
        Pause::PastDownload("seal".into()),
    ] {
        assert!(!p.line().is_empty());
    }
}

#[test]
fn resume_without_a_build_asks_the_caller_to_start_one() {
    let (_t, l) = layout();
    std::fs::write(pause_file(&l), "").unwrap();
    assert!(!resume(&l));
    assert!(!paused(&l));
}

#[test]
fn the_file_holds_the_last_line() {
    let (_t, l) = layout();
    assert_eq!(read(&l), None);
    let mut r = Reporter::file_only(&l);
    r.step("fetch");
    r.download(Tick::Paused {
        bytes: 5,
        total: Some(10),
    });
    assert_eq!(
        read(&l),
        Some(Setup {
            step: "fetch".into(),
            bytes: Some(5),
            total: Some(10),
            paused: true,
            ..Default::default()
        })
    );
    r.failed("no space");
    let s = read(&l).unwrap();
    assert_eq!(
        (s.step.as_str(), s.failed.as_deref()),
        ("fetch", Some("no space"))
    );
    r.stopped("guided");
    let s = read(&l).unwrap();
    assert!(s.stopped && s.step == "guided" && s.failed.is_none());
}

#[test]
fn download_progress_is_rate_limited_but_the_end_is_not() {
    let (_t, l) = layout();
    let mut r = Reporter::file_only(&l);
    let p = |bytes, rate| Tick::Progress {
        bytes,
        total: Some(100),
        rate,
    };
    r.download(p(1, Some(1)));
    r.download(p(2, Some(1)));
    assert_eq!(read(&l).unwrap().bytes, Some(1), "the second came too soon");
    r.download(p(100, None));
    assert_eq!(read(&l).unwrap().bytes, Some(100));
    r.download(Tick::Resumed {
        bytes: 100,
        total: Some(100),
    });
    assert!(!read(&l).unwrap().paused);
}
