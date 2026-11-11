use super::*;

#[test]
fn refused_setup_does_not_overwrite_an_existing_builds_progress() {
    use std::os::fd::AsRawFd;
    let w = world();
    std::fs::create_dir_all(&w.layout.root).unwrap();
    let progress = setup_progress::progress_file(&w.layout);
    std::fs::write(&progress, "existing progress").unwrap();
    let held = std::fs::File::create(&w.layout.setup_lock).unwrap();
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);
    let mut fake = Fake::new(&w.layout.golden);
    let mut out = Vec::new();
    let mut ask = |_: &str| Ok(true);
    let mut ui = Ui {
        out: &mut out,
        ask: &mut ask,
    };
    assert!(run(
        &w.env,
        &w.layout,
        &w.layout.golden,
        &mut fake,
        &roomy(),
        &mut ui
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(&progress).unwrap(),
        "existing progress"
    );
    let opts = Opts {
        ipsw: Some(w._t.path().join("missing")),
        ..roomy()
    };
    assert!(run(
        &w.env,
        &w.layout,
        &w.layout.golden,
        &mut fake,
        &opts,
        &mut ui
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(&progress).unwrap(),
        "existing progress"
    );
}

#[test]
fn leaving_fetch_removes_a_late_pause_request_for_download_and_ipsw() {
    for local in [false, true] {
        let w = world();
        let mut fake = Fake::new(&w.layout.golden);
        fake.pause_on_image = Some(setup_progress::pause_file(&w.layout));
        let mut opts = roomy();
        if local {
            let image = w._t.path().join("restore.ipsw");
            std::fs::write(&image, "ipsw").unwrap();
            opts.ipsw = Some(image);
        }
        let (code, _) = run_with(&w, &mut fake, &opts, true);
        assert_eq!(code, 0);
        assert!(!setup_progress::pause_file(&w.layout).exists());
    }
}
