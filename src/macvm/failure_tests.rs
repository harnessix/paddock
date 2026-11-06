use super::*;
use std::path::Path;

fn ctx() -> Ctx {
    Ctx {
        app: "ChatGPT".into(),
    }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn respond_matches_table_for_every_row() {
    use Step::*;
    let c = ctx();
    let report = PathBuf::from("/p/logs/ChatGPT-1.ips");
    let rows: Vec<(Failure, Vec<Step>)> = vec![
        (
            Failure::GuestHang(Stage::Badge),
            vec![
                Badge(text::guest_not_answering()),
                Send(Request::Ping { t: 0 }),
                Event {
                    kind: "health",
                    text: text::guest_not_answering(),
                },
            ],
        ),
        (
            Failure::AgentCrash,
            vec![Event {
                kind: "agent",
                text: text::agent_connection_closed(),
            }],
        ),
        (
            Failure::AppCrash {
                code: 1,
                report: Some(report.clone()),
                crashes: 1,
            },
            vec![
                Line(text::app_exited("ChatGPT", 1)),
                Event {
                    kind: "app",
                    text: text::app_exited("ChatGPT", 1),
                },
                CloseWindows,
                DockEntry(Entry::OpenAgain),
                Relaunch(true),
            ],
        ),
        (
            Failure::ShimCrash { pid: 812 },
            vec![
                Event {
                    kind: "crashed",
                    text: text::session_ended_without_save_event(812),
                },
                Line(text::ended_without_save()),
            ],
        ),
        (
            Failure::HostRestart,
            vec![Event {
                kind: "host",
                text: text::host_restarted(),
            }],
        ),
        (
            Failure::HostWake { slept: secs(30) },
            vec![
                Badge(text::waking()),
                Send(Request::Ping { t: 0 }),
                CloseTunnels,
                DenyStale,
                Event {
                    kind: "wake",
                    text: text::wake_line(30),
                },
            ],
        ),
        (
            Failure::PartialProfile,
            vec![
                Backup("pre-repair".into()),
                Discard(What::Partials),
                Discard(What::Stamp),
                Discard(What::State),
                Line(text::profile_repaired()),
            ],
        ),
        (
            Failure::DiskFull(Level::Low1G),
            vec![
                Badge(text::disk_warn(1 << 30)),
                Send(Request::DiskLow),
                Line(text::disk_low_line(1 << 30)),
                Event {
                    kind: "disk",
                    text: text::disk_low_line(1 << 30),
                },
                SkipSave,
            ],
        ),
        (
            Failure::StateNotRestored {
                error: "code 12".into(),
            },
            vec![
                Line(text::restore_failed("code 12")),
                Event {
                    kind: "restore",
                    text: text::restore_failed("code 12"),
                },
                Discard(What::State),
                ColdBoot,
            ],
        ),
        (
            Failure::SlotsTaken {
                line: "two macOS VMs already run".into(),
            },
            vec![Line("two macOS VMs already run".into()), Exit(2)],
        ),
        (
            Failure::RequestTimeout {
                kind: "ping".into(),
                strikes: 1,
            },
            vec![Event {
                kind: "timeout",
                text: text::request_timeout("ping", Duration::ZERO),
            }],
        ),
        (
            Failure::HelloRefused {
                line: "refused".into(),
            },
            vec![
                Line("refused".into()),
                Event {
                    kind: "error",
                    text: "refused".into(),
                },
                Exit(1),
            ],
        ),
        (
            Failure::AppCopyFailed {
                app: "ChatGPT".into(),
                reason: "full".into(),
            },
            vec![
                Line(text::copy_failed("ChatGPT", "full")),
                Event {
                    kind: "copy",
                    text: text::copy_failed("ChatGPT", "full"),
                },
            ],
        ),
        (Failure::MarkerDied, vec![RestartMarker]),
        (
            Failure::ProxyGone { pid: 7 },
            vec![
                Event {
                    kind: "crashed",
                    text: text::session_ended_without_save_event(7),
                },
                Line(text::ended_without_save()),
            ],
        ),
        (
            Failure::GoldenMissing,
            vec![Line(text::golden_unsealed()), Exit(2)],
        ),
        (
            Failure::SecondStart { pid: 812 },
            vec![Line(text::profile_running("ChatGPT", 812)), Exit(2)],
        ),
    ];
    assert_eq!(rows.len(), ROWS);
    for (f, want) in &rows {
        assert_eq!(&respond(f, &c), want, "{f:?}");
    }
    // The rows that touch the disks name only the saved state and the
    // partials: the data disk and the system set never.
    for (f, steps) in &rows {
        for s in steps {
            if let Discard(w) = s {
                assert!(
                    matches!(w, What::State | What::Partials | What::Stamp),
                    "{f:?} discards {w:?}"
                );
            }
        }
    }
    // The other stages of row 1 and the two levels of row 8.
    assert_eq!(
        respond(&Failure::GuestHang(Stage::Dock), &c),
        vec![
            DockEntry(Entry::RestartGuest),
            Event {
                kind: "health",
                text: text::guest_not_answering()
            }
        ]
    );
    let restart = respond(&Failure::GuestHang(Stage::Restart), &c);
    assert_eq!(
        restart[3..],
        [
            CloseWindows,
            StopVm { force: true },
            Discard(What::State),
            ColdBoot
        ]
    );
    assert_eq!(
        respond(&Failure::DiskFull(Level::Warn2G), &c),
        vec![Badge(text::disk_warn(2 << 30))]
    );
    // Every badge fits the title bar.
    for (f, steps) in &rows {
        for s in steps {
            if let Badge(b) = s {
                assert!(b.chars().count() <= text::BADGE_MAX, "{f:?}: {b}");
            }
        }
    }
}

#[test]
fn health_gap_timeline() {
    let t0 = Instant::now();
    let mut w = Watch::new("ChatGPT");
    assert!(w.alive(t0).is_empty());
    assert!(w.tick(t0 + secs(9)).is_empty());
    let at10 = w.tick(t0 + secs(10));
    assert!(at10.contains(&Step::Badge(text::guest_not_answering())));
    assert!(at10.contains(&Step::Send(Request::Ping { t: 0 })));
    assert_eq!(w.stage(), Some(Stage::Badge));
    assert!(w.tick(t0 + secs(11)).is_empty(), "once per stage");
    let at30 = w.tick(t0 + secs(30));
    assert_eq!(at30[0], Step::DockEntry(Entry::RestartGuest));
    assert_eq!(w.stage(), Some(Stage::Dock));
    let at120 = w.tick(t0 + secs(120));
    assert!(at120.contains(&Step::StopVm { force: true }));
    assert!(at120.contains(&Step::Discard(What::State)));
    assert_eq!(at120.last(), Some(&Step::ColdBoot));
    assert_eq!(w.stage(), None, "the cold boot starts the clock again");
    assert!(w.tick(t0 + secs(200)).is_empty(), "no health yet: no clock");
    // A pong at 25 s resets everything: 30 s gives nothing.
    let mut w = Watch::new("ChatGPT");
    w.alive(t0);
    w.tick(t0 + secs(10));
    let pong = w.alive(t0 + secs(25));
    assert_eq!(pong[0], Step::ClearBadge);
    assert_eq!(w.stage(), None);
    assert!(w.tick(t0 + secs(30)).is_empty());
    assert!(w.tick(t0 + secs(34)).is_empty());
    assert!(!w.tick(t0 + secs(35)).is_empty(), "10 s after the pong");
    // A tick that skips a stage still fires each stage once, in order.
    let mut w = Watch::new("ChatGPT");
    w.alive(t0);
    let all = w.tick(t0 + secs(130));
    let dock = all
        .iter()
        .position(|s| *s == Step::DockEntry(Entry::RestartGuest))
        .unwrap();
    let badge = all
        .iter()
        .position(|s| *s == Step::Badge(text::guest_not_answering()))
        .unwrap();
    assert!(badge < dock && all.last() == Some(&Step::ColdBoot));
}

#[test]
fn three_timeouts_is_agent_crash() {
    let mut w = Watch::new("ChatGPT");
    let t0 = Instant::now();
    w.alive(t0);
    assert_eq!(w.timeout("ping").len(), 1);
    assert_eq!(w.timeout("window.move 3").len(), 1);
    assert_eq!(w.strikes(), 2);
    let third = w.timeout("ping");
    assert!(third.contains(&Step::DropAgent));
    assert!(third.contains(&Step::Line(text::agent_timeouts(3))));
    assert_eq!(
        third.last(),
        Some(&Step::Event {
            kind: "agent",
            text: text::agent_connection_closed()
        })
    );
    assert_eq!(w.strikes(), 0);
    // A reply in between resets the count.
    w.timeout("ping");
    w.timeout("ping");
    w.alive(t0 + secs(1));
    assert_eq!(w.strikes(), 0);
    assert_eq!(w.timeout("ping").len(), 1);
}

#[test]
fn wake_sequence() {
    let mut w = Watch::new("ChatGPT");
    let t0 = Instant::now();
    w.alive(t0);
    let woke = w.woke(secs(30), t0 + secs(40));
    assert_eq!(woke[0], Step::Badge(text::waking()));
    assert_eq!(woke[1], Step::Send(Request::Ping { t: 0 }));
    assert!(woke.contains(&Step::CloseTunnels));
    assert!(woke.contains(&Step::DenyStale));
    assert!(w.waking());
    assert!(w.tick(t0 + secs(41)).is_empty());
    assert_eq!(
        w.tick(t0 + secs(42)),
        vec![Step::Send(Request::Ping { t: 0 })]
    );
    assert_eq!(
        w.tick(t0 + secs(44)),
        vec![Step::Send(Request::Ping { t: 0 })]
    );
    let pong = w.alive(t0 + secs(45));
    assert_eq!(pong[0], Step::ClearBadge);
    assert!(!w.waking());
    assert!(w.tick(t0 + secs(47)).is_empty());
    // The silence of the sleep itself is not a health gap: the clock
    // starts at the wake.
    let mut w = Watch::new("ChatGPT");
    w.alive(t0);
    w.woke(secs(300), t0 + secs(300));
    let tick = w.tick(t0 + secs(302));
    assert_eq!(tick, vec![Step::Send(Request::Ping { t: 0 })]);
    assert_eq!(w.stage(), None);
}

#[test]
fn crash_counter_stops_relaunch() {
    let mut w = Watch::new("ChatGPT");
    let t0 = Instant::now();
    let report = Some(PathBuf::from("/p/logs/ChatGPT-3.ips"));
    let one = w.app_exited(1, None, t0);
    assert!(one.contains(&Step::Relaunch(true)));
    assert!(one.contains(&Step::DockEntry(Entry::OpenAgain)));
    let two = w.app_exited(1, None, t0 + secs(20));
    assert!(two.contains(&Step::Relaunch(true)));
    let three = w.app_exited(1, report.clone(), t0 + secs(40));
    assert!(three.contains(&Step::Relaunch(false)));
    let line = text::relaunch_stopped("ChatGPT", report.as_deref());
    assert!(three.contains(&Step::Line(line.clone())));
    assert!(line.contains("/p/logs/ChatGPT-3.ips"));
    assert!(!w.relaunch_allowed(t0 + secs(41)));
    assert!(w.relaunch_allowed(t0 + secs(61)), "the window moved on");
    // A clean exit does not count.
    let mut w = Watch::new("ChatGPT");
    for i in 0..5 {
        let v = w.app_exited(0, None, t0 + secs(i));
        assert!(v.contains(&Step::Relaunch(true)));
    }
    // Three crashes spread over more than a minute do not stop it.
    let mut w = Watch::new("ChatGPT");
    w.app_exited(1, None, t0);
    w.app_exited(1, None, t0 + secs(30));
    let v = w.app_exited(1, None, t0 + secs(61));
    assert!(v.contains(&Step::Relaunch(true)));
}

#[test]
fn guest_restart_keeps_args() {
    let args = BootArgs {
        profile: Path::new("/p/profiles/ChatGPT/default").into(),
        shares: vec!["ws-0=/Users/me/src:rw".into(), "agent=/a:ro".into()],
        token: "tok".into(),
        display: "2560x1600@2".into(),
        cpus: 4,
        memory_mib: 4096,
    };
    assert_eq!(restart(&args), args);
    // The restart row of the table is a cold boot, not a fresh clone.
    let steps = respond(&Failure::GuestHang(Stage::Restart), &ctx());
    assert!(steps.contains(&Step::ColdBoot));
    assert!(!steps
        .iter()
        .any(|s| matches!(s, Step::Discard(w) if w.system_set() || *w == What::Data)));
}

#[test]
fn a_thaw_starts_the_health_clock_again() {
    let t0 = Instant::now();
    let mut w = Watch::new("ChatGPT");
    w.alive(t0);
    w.timeout("window.move 3");
    // A freeze of 5 minutes, then the thaw: the pause is not a silence.
    let thaw = t0 + secs(300);
    w.thawed(thaw);
    assert_eq!(w.strikes(), 0);
    assert!(w.tick(thaw + secs(9)).is_empty());
    assert!(!w.tick(thaw + secs(10)).is_empty(), "10 s after the thaw");
}
