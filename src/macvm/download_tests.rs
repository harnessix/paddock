use super::*;
use std::process::{Command, Stdio};

/// A stand-in for curl: append `chunks` lines of 100 bytes to `dest`,
/// one per `pause_ms`, then exit with `code`.
fn writer(dest: &Path, chunks: u32, pause_ms: u32, code: i32) -> Result<Child> {
    let script = format!(
        "i=0; while [ $i -lt {chunks} ]; do printf '%099d\\n' 0 >> '{}'; i=$((i+1)); sleep {}; done; echo bad >&2; exit {code}",
        dest.display(),
        pause_ms as f64 / 1000.0
    );
    Ok(Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?)
}

fn fast<'a>(pause: PathBuf, report: &'a mut dyn FnMut(Tick)) -> Watch<'a> {
    Watch {
        pause,
        every: Duration::from_millis(30),
        report,
    }
}

#[test]
fn progress_lines_and_events_to_the_end() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("x.ipsw.partial");
    let mut ticks = Vec::new();
    let mut lines = Vec::new();
    let mut report = |t| ticks.push(t);
    let mut w = fast(t.path().join("pause"), &mut report);
    run(
        &mut || writer(&dest, 10, 20, 0),
        &dest,
        Some(1000),
        &mut w,
        &mut |l| lines.push(l),
    )
    .unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), 1000);
    assert_eq!(lines.last().unwrap(), "download 100%, about 0 s left");
    assert!(lines.len() >= 2, "{lines:?}");
    assert_eq!(
        ticks.last().unwrap(),
        &Tick::Progress {
            bytes: 1000,
            total: Some(1000),
            rate: None
        }
    );
    assert!(ticks.iter().all(|t| matches!(
        t,
        Tick::Progress {
            total: Some(1000),
            ..
        }
    )));
}

#[test]
fn a_failed_child_is_the_error_with_its_stderr() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("x.partial");
    let mut report = |_| {};
    let mut w = fast(PathBuf::new(), &mut report);
    let e = run(
        &mut || writer(&dest, 1, 1, 22),
        &dest,
        None,
        &mut w,
        &mut |_| {},
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("bad"), "{e}");
}

#[test]
fn a_pause_stops_the_child_and_a_resume_continues_the_file() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("x.partial");
    let pause = t.path().join("setup.pause");
    let spawns = std::sync::atomic::AtomicU32::new(0);
    let ticks = std::sync::Mutex::new(Vec::new());
    let mut report = |t: Tick| {
        // The first progress line pauses; the paused line resumes after a
        // moment, as `--resume` would.
        match &t {
            Tick::Progress { rate: Some(_), .. }
                if !pause.exists() && spawns.load(std::sync::atomic::Ordering::SeqCst) == 1 =>
            {
                std::fs::write(&pause, "").unwrap();
            }
            Tick::Paused { .. } => {
                let p = pause.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    std::fs::remove_file(p).unwrap();
                });
            }
            _ => {}
        }
        ticks.lock().unwrap().push(t);
    };
    let mut w = fast(pause.clone(), &mut report);
    let mut spawn = || {
        let n = spawns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // The first child is slow and is killed by the pause; the second
        // writes the rest of the file quickly.
        if n == 0 {
            writer(&dest, 1000, 20, 0)
        } else {
            let have = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            let left = (2000u64.saturating_sub(have) / 100) as u32;
            writer(&dest, left, 1, 0)
        }
    };
    run(&mut spawn, &dest, Some(2000), &mut w, &mut |_| {}).unwrap();
    assert_eq!(spawns.load(std::sync::atomic::Ordering::SeqCst), 2);
    let ticks = ticks.into_inner().unwrap();
    let paused = ticks
        .iter()
        .filter(|t| matches!(t, Tick::Paused { .. }))
        .count();
    let resumed = ticks
        .iter()
        .filter(|t| matches!(t, Tick::Resumed { .. }))
        .count();
    assert_eq!((paused, resumed), (1, 1), "{ticks:?}");
    let at_pause = ticks.iter().find_map(|t| match t {
        Tick::Paused { bytes, .. } => Some(*bytes),
        _ => None,
    });
    let at_resume = ticks.iter().find_map(|t| match t {
        Tick::Resumed { bytes, .. } => Some(*bytes),
        _ => None,
    });
    assert_eq!(at_pause, at_resume, "nothing downloads while paused");
    assert!(at_pause.unwrap() < 2000);
    assert!(!pause.exists());
}

#[test]
fn a_pause_before_the_start_waits_first() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("x.partial");
    let pause = t.path().join("setup.pause");
    std::fs::write(&pause, "").unwrap();
    let p = pause.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        std::fs::remove_file(p).unwrap();
    });
    let mut ticks = Vec::new();
    let mut report = |t| ticks.push(t);
    let mut w = fast(pause, &mut report);
    run(
        &mut || writer(&dest, 2, 1, 0),
        &dest,
        Some(200),
        &mut w,
        &mut |_| {},
    )
    .unwrap();
    assert!(
        matches!(ticks[0], Tick::Paused { bytes: 0, .. }),
        "{ticks:?}"
    );
    assert!(
        matches!(ticks[1], Tick::Resumed { bytes: 0, .. }),
        "{ticks:?}"
    );
}

#[test]
fn a_complete_partial_file_needs_no_child() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("x.partial");
    std::fs::write(&dest, vec![0u8; 300]).unwrap();
    let mut report = |_| {};
    let mut w = fast(PathBuf::new(), &mut report);
    let mut lines = Vec::new();
    run(
        &mut || panic!("no child for a complete file"),
        &dest,
        Some(300),
        &mut w,
        &mut |l| lines.push(l),
    )
    .unwrap();
    assert_eq!(lines, ["download 100%, about 0 s left"]);
}
