//! The screen lock and the keychain in the main loop: `screen.lock` the
//! moment the state changes, the `locked` field of `health`, the
//! `keychain.unlock` request, and the bench ops `bench.lock` and
//! `bench.keychain`.

use super::*;

fn lock_events(host: &Host) -> Vec<bool> {
    host.seen
        .iter()
        .filter_map(|e| match e {
            Event::ScreenLock { locked, .. } => Some(*locked),
            _ => None,
        })
        .collect()
}

fn bench_handshake(host: &mut Host) {
    assert!(matches!(host.next(), Event::Hello(_)));
    let req = host.send(Request::Token {
        token: TOKEN.into(),
        v: VERSION,
        bench: true,
        now_ms: None,
    });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
}

#[test]
fn a_lock_and_an_unlock_are_sent_the_moment_they_happen() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    // An unlocked screen at the start sends no event.
    host.until(|e| matches!(e, Event::Health(h) if !h.locked));
    st.lock().unwrap().locked = true;
    let t = Instant::now();
    let Event::ScreenLock {
        locked,
        uptime,
        idle_secs,
    } = host.until(|e| matches!(e, Event::ScreenLock { .. }))
    else {
        unreachable!()
    };
    assert!(locked);
    // At the next window poll, well before the next health.
    assert!(t.elapsed() < HEALTH_INTERVAL, "{:?}", t.elapsed());
    assert_eq!(uptime, 0);
    assert_eq!(idle_secs, Some(7));
    host.until(|e| matches!(e, Event::Health(h) if h.locked));
    st.lock().unwrap().locked = false;
    host.until(|e| matches!(e, Event::ScreenLock { locked: false, .. }));
    host.until(|e| matches!(e, Event::Health(h) if !h.locked));
    let req = host.send(Request::AgentRestart);
    host.reply(req);
    assert_eq!(agent.join().unwrap(), 0);
    // One event per change, none for the polls between.
    assert_eq!(lock_events(&host), vec![true, false]);
}

#[test]
fn a_new_connection_hears_of_a_locked_screen_at_once() {
    let (a1, b1) = UnixStream::pair().unwrap();
    let (a2, b2) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    st.lock().unwrap().locked = true;
    let agent = start(fake, vec![a1, a2]);
    let mut host = Host::new(b1);
    host.handshake();
    host.until(|e| matches!(e, Event::ScreenLock { locked: true, .. }));
    drop(host);
    let mut host = Host::new(b2);
    host.handshake();
    host.until(|e| matches!(e, Event::ScreenLock { locked: true, .. }));
    let req = host.send(Request::AgentRestart);
    host.reply(req);
    assert_eq!(agent.join().unwrap(), 0);
}

#[test]
fn keychain_unlock_gets_the_password_and_answers_with_text() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let req = host.send(Request::KeychainUnlock {
        password: "pw-of-the-guest".into(),
    });
    assert_eq!(
        host.reply(req),
        Outcome::Ok {
            data: Some(ReplyData::Text {
                text: "unlocked".into()
            })
        }
    );
    // A failure is an error reply with the code of the platform.
    st.lock().unwrap().keychain_result =
        Some((ErrorCode::Io, "the keychain is still locked".into()));
    let req = host.send(Request::KeychainUnlock {
        password: "pw-of-the-guest".into(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error { code: ErrorCode::Io, text } if text.contains("still locked")
    ));
    let req = host.send(Request::AgentRestart);
    host.reply(req);
    assert_eq!(agent.join().unwrap(), 0);
    assert_eq!(
        st.lock().unwrap().keychain_unlocks,
        vec!["pw-of-the-guest".to_string(); 2]
    );
}

#[test]
fn bench_lock_and_bench_keychain_need_a_bench_session() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    for r in [Request::BenchLock, Request::BenchKeychain] {
        let req = host.send(r);
        assert!(matches!(
            host.reply(req),
            Outcome::Error {
                code: ErrorCode::Unsupported,
                ..
            }
        ));
    }
    let req = host.send(Request::AgentRestart);
    host.reply(req);
    assert_eq!(agent.join().unwrap(), 0);
    assert_eq!(st.lock().unwrap().locks, 0);
    assert!(!st.lock().unwrap().locked);
}

#[test]
fn bench_lock_locks_and_the_event_follows() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    bench_handshake(&mut host);
    let req = host.send(Request::BenchLock);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    host.until(|e| matches!(e, Event::ScreenLock { locked: true, .. }));
    let req = host.send(Request::BenchKeychain);
    assert_eq!(
        host.reply(req),
        Outcome::Ok {
            data: Some(ReplyData::Text {
                text: "status unlocked".into()
            })
        }
    );
    let req = host.send(Request::AgentRestart);
    host.reply(req);
    assert_eq!(agent.join().unwrap(), 0);
    assert_eq!(st.lock().unwrap().locks, 1);
}

#[test]
fn lock_changed_sends_on_a_change_and_on_a_first_lock() {
    use super::screen::lock_changed;
    assert!(!lock_changed(None, false));
    assert!(lock_changed(None, true));
    assert!(lock_changed(Some(false), true));
    assert!(lock_changed(Some(true), false));
    assert!(!lock_changed(Some(true), true));
    assert!(!lock_changed(Some(false), false));
}
