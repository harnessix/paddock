use super::*;

#[test]
fn on_timeout_is_read_from_the_user_configuration() {
    let doc = |t: &str| toml::from_str::<toml::Value>(t).unwrap();
    assert_eq!(SpawnCfg::default().on_timeout, OnTimeout::Stop);
    let c = SpawnCfg::from_doc(&doc("[spawn]\non_timeout = \"ask\"")).unwrap();
    assert_eq!(c.on_timeout, OnTimeout::Ask);
    assert!(SpawnCfg::from_doc(&doc("[spawn]\non_timeout = \"wait\"")).is_err());
}

#[test]
fn ask_freezes_the_child_and_more_time_thaws_and_extends_it() {
    let s = setup();
    let seam = FakeSeam::new(Choice::More("30m".into()), Duration::from_secs(1));
    let sp = limit_spawner(&s, OnTimeout::Ask, seam.clone(), child_that_runs("3"));
    let done = run_with_limit(&sp, 1);
    assert_eq!(done.state, State::Exited, "{done:?}");
    assert_eq!(done.limits.timeout, 1 + 1800);
    assert!(done.frozen_s >= 1, "{done:?}");
    let sb = &done.sandbox;
    assert_eq!(
        seam.calls(),
        [format!("freeze {sb}"), "ask".into(), format!("thaw {sb}")]
    );
    let card = seam.cards.lock().unwrap()[0].clone();
    assert_eq!(card.capability, crate::broker::TIMEOUT_CAPABILITY);
    assert_eq!(card.choices, ["15m", "30m", "1h"]);
    assert_eq!(card.started_by, ["p-1"]);
    for want in [
        "Parent: p-1 (claude)",
        sb.as_str(),
        "(codex)",
        "implementer",
        "Time used: ",
    ] {
        assert!(
            card.detail.contains(want),
            "{want} missing in {}",
            card.detail
        );
    }
    assert!(!card.detail.contains("TASK-TEXT"));
    assert_eq!(audit_decisions(&s.env), ["more time: 30m"]);
}

/// The user froze the child before it reached the limit. More time moves
/// the limit and leaves that freeze in place.
#[test]
fn more_time_keeps_a_sandbox_the_user_froze() {
    let s = setup();
    let seam = FakeSeam::new(Choice::More("30m".into()), Duration::from_secs(1));
    *seam.fresh.lock().unwrap() = Some(vec![]);
    let sp = limit_spawner(&s, OnTimeout::Ask, seam.clone(), child_that_runs("3"));
    let done = run_with_limit(&sp, 1);
    assert_eq!(done.state, State::Exited, "{done:?}");
    assert_eq!(done.limits.timeout, 1 + 1800);
    let sb = &done.sandbox;
    assert_eq!(seam.calls(), [format!("freeze {sb}"), "ask".into()]);
    assert_eq!(audit_decisions(&s.env), ["more time: 30m, stays frozen"]);
}

#[test]
fn stop_or_no_answer_stops_the_child_and_thaws_it_to_end() {
    for (answer, decision) in [
        (Choice::Stop, "stop"),
        (Choice::NoAnswer, "no answer: stop"),
        (Choice::Unavailable, "approval unavailable: stop"),
        (Choice::Unknown, "approval outcome unknown: stop"),
        (Choice::More("5h".into()), "no answer: stop"),
    ] {
        let s = setup();
        let seam = FakeSeam::new(answer, Duration::ZERO);
        let sp = limit_spawner(&s, OnTimeout::Ask, seam.clone(), child_that_runs("30"));
        let done = run_with_limit(&sp, 1);
        assert_eq!(done.state, State::TimedOut, "{done:?}");
        assert_eq!(done.limits.timeout, 1);
        let sb = &done.sandbox;
        assert_eq!(
            seam.calls(),
            [format!("freeze {sb}"), "ask".into(), format!("thaw {sb}")]
        );
        assert_eq!(audit_decisions(&s.env), [decision]);
    }
}

#[test]
fn a_locked_child_mailbox_cannot_block_timeout_stop_and_thaw() {
    use std::sync::mpsc;
    struct MailSeam {
        env: HostEnv,
        entering: Mutex<Option<mpsc::Sender<()>>>,
        entered: Mutex<Option<mpsc::Receiver<()>>>,
        finished: Mutex<Option<mpsc::Sender<()>>>,
        thawed: Mutex<Vec<String>>,
    }
    impl TimeoutSeam for MailSeam {
        fn freeze(&self, sandbox: &str) -> Result<Vec<String>> {
            Ok(vec![sandbox.into()])
        }
        fn thaw(&self, sandbox: &str) -> Result<()> {
            self.thawed.lock().unwrap().push(sandbox.into());
            Ok(())
        }
        fn thaw_ids(&self, ids: &[String]) -> Result<()> {
            for id in ids {
                self.thaw(id)?;
            }
            Ok(())
        }
        fn ask(&self, card: &Pending, wait: Duration, cancelled: &dyn Fn() -> bool) -> Choice {
            let entering = self.entering.lock().unwrap().take().unwrap();
            let entered = self.entered.lock().unwrap().take().unwrap();
            let finished = self.finished.lock().unwrap().take().unwrap();
            crate::broker::CARD_MAIL_HOOK.with(|hook| {
                *hook.borrow_mut() = Some((
                    Box::new(move || entering.send(()).unwrap()),
                    Box::new(move || finished.send(()).unwrap()),
                ))
            });
            crate::broker::OWNER_PUBLISHED_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    entered.recv().unwrap();
                }))
            });
            let choice = crate::broker::ask_choice(&self.env, card, wait, cancelled);
            crate::broker::OWNER_PUBLISHED_HOOK.with(|hook| *hook.borrow_mut() = None);
            choice
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let env = env_in(tmp.path());
    crate::privfs::write(
        &crate::broker::approval_ui_file(&env),
        state::now().to_string(),
    )
    .unwrap();
    let sandbox = "codex-spawn-synthetic";
    crate::mail::create(
        &env,
        sandbox,
        &crate::mail::Starter {
            kind: "host".into(),
            caller: "test".into(),
            parent: None,
            created: 0,
        },
    )
    .unwrap();
    let mailbox = crate::mail::dir(&env, sandbox).unwrap();
    let held = crate::mail::lock(&mailbox).unwrap();
    let (entering_tx, entering_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let seam = Arc::new(MailSeam {
        env: env.clone(),
        entering: Mutex::new(Some(entering_tx)),
        entered: Mutex::new(Some(entering_rx)),
        finished: Mutex::new(Some(finished_tx)),
        thawed: Mutex::new(Vec::new()),
    });
    let mut config = cfg(&["codex"], 2);
    config.on_timeout = OnTimeout::Ask;
    let mut sp = Spawner::with_program(
        &env,
        config,
        ceiling("p-1", 0, tmp.path()),
        tmp.path(),
        vec![],
    );
    sp.seam = seam.clone();
    sp.approval_wait = Duration::ZERO;
    sp.children.lock().unwrap().insert(
        "test".into(),
        Entry {
            status: Status {
                id: "test".into(),
                sandbox: sandbox.into(),
                harness: "codex".into(),
                branch: "test".into(),
                state: State::Running,
                exit_code: None,
                started: 0,
                ended: None,
                inbox: None,
                message: String::new(),
                role: Role::default(),
                limits: Limits::default(),
                enforced_by: String::new(),
                from_head: None,
                frozen_s: 0,
                pick: model::Pick::default(),
            },
            pid: 0,
            stop: None,
            hitch: None,
        },
    );
    let sp = Arc::new(sp);
    let (done_tx, done_rx) = mpsc::channel();
    let other = sp.clone();
    let worker = std::thread::spawn(move || {
        done_tx
            .send(other.at_limit("test", Duration::ZERO))
            .unwrap()
    });
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    let stopped_and_thawed = sp.children.lock().unwrap()["test"].stop.is_some()
        && *seam.thawed.lock().unwrap() == vec![sandbox.to_string()];
    drop(held);
    worker.join().unwrap();
    finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(result.unwrap().0, Duration::ZERO);
    assert!(stopped_and_thawed);
}

/// The parent died while the child was frozen at its limit. SIGTERM does not
/// end a frozen sandbox. The child's paddock thaws it through the freezer
/// seam, and the sandbox then exits. No systemd freezer runs here.
#[test]
fn a_frozen_child_thaws_when_its_parent_dies() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env_in(tmp.path());
    let id = "sb-frozen";
    let run = env.paddock_run().join(id);
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("frozen.json"),
        r#"{"since":1,"reason":"a spawned child reached its time limit"}"#,
    )
    .unwrap();
    let ready = tmp.path().join("ready");
    let flag = tmp.path().join("thawed");
    let script = format!(
        "trap 'true' TERM
touch '{}'
while [ ! -f '{}' ]; do sleep 0.05; done
exit 0
",
        ready.display(),
        flag.display()
    );
    let mut child = std::process::Command::new("sh")
        .args(["-c", &script])
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !ready.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "the stand-in sandbox did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: the stand-in is this test's child. SIGTERM stays pending in a
    // real freezer. The trap stands in for that.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let seam = FakeSeam::new(Choice::Stop, Duration::ZERO);
    *seam.release.lock().unwrap() = Some(flag);
    seam.freeze(id).unwrap();
    let stop = std::sync::atomic::AtomicBool::new(true);
    let status = wait_outliving_parent(
        &mut child,
        &stop,
        &env,
        id,
        seam.as_ref(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(
        status.success(),
        "the frozen child did not exit after the thaw: {status}"
    );
    assert_eq!(seam.calls(), [format!("freeze {id}"), format!("thaw {id}")]);
}

#[test]
fn stop_mode_stops_at_the_limit_without_a_card() {
    let s = setup();
    let seam = FakeSeam::new(Choice::More("30m".into()), Duration::ZERO);
    let sp = limit_spawner(&s, OnTimeout::Stop, seam.clone(), child_that_runs("30"));
    let done = run_with_limit(&sp, 1);
    assert_eq!(done.state, State::TimedOut);
    assert!(
        done.message.contains("time limit of 1 seconds"),
        "{}",
        done.message
    );
    assert!(seam.calls().is_empty());
    assert!(audit_decisions(&s.env).is_empty());
}

#[test]
fn the_frozen_time_does_not_count_against_the_limit() {
    // Limit 1 s, 2 s more at the card, which waits 2 s. The child runs 4 s.
    // Without the frozen time, it used about 2 s of its 3 s at its end, so
    // there is one card only. With the frozen time counted, a second card
    // would come at once.
    let s = setup();
    let seam = FakeSeam::new(Choice::More("2s".into()), Duration::from_secs(2));
    let mut sp = Arc::try_unwrap(limit_spawner(
        &s,
        OnTimeout::Ask,
        seam.clone(),
        child_that_runs("4"),
    ))
    .ok()
    .unwrap();
    sp.more_time = vec![("2s".into(), 2)];
    let sp = Arc::new(sp);
    let done = run_with_limit(&sp, 1);
    assert_eq!(done.state, State::Exited, "{done:?}");
    assert_eq!(seam.calls().iter().filter(|c| *c == "ask").count(), 1);
    assert_eq!(done.limits.timeout, 3);
    assert!(done.frozen_s >= 2);
}

#[test]
fn ask_mode_leaves_out_the_scope_runtime_limit() {
    let s = setup();
    let seam = FakeSeam::new(Choice::Stop, Duration::ZERO);
    let sp = limit_spawner(&s, OnTimeout::Ask, seam, vec!["/usr/bin/paddock".into()]);
    let log = std::fs::File::create(s.project.join("log")).unwrap();
    let limits = Limits {
        timeout: 600,
        mem: Some(1 << 30),
        cpu_percent: None,
    };
    let cmd = sp
        .command(
            &s.project.join("job.json"),
            log,
            limits,
            Some(Path::new("/usr/bin/systemd-run")),
            "paddock-spawn-x",
        )
        .unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.iter().all(|a| !a.starts_with("RuntimeMaxSec")),
        "{args:?}"
    );
    assert!(args.iter().any(|a| a == "MemoryMax=1073741824"));
}
