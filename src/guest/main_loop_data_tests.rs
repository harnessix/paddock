//! `data.prepare` in its thread: the agent answers beside it, and the
//! shutdown of the quit stops it first.

use super::*;

fn prepare(host: &mut Host) -> u64 {
    host.send(Request::DataPrepare {
        password: "pw".into(),
        data_id: "d1".into(),
    })
}

#[test]
fn the_agent_answers_the_quit_while_the_home_is_copied() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    st.lock().unwrap().data_prepare_delay = Duration::from_secs(30);
    let _agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let t = Instant::now();
    let data = prepare(&mut host);
    // The progress of the prepare reaches the host.
    let ev = host.until(|e| matches!(e, Event::DataProgress { .. }));
    assert!(matches!(ev, Event::DataProgress { wait_secs, .. } if wait_secs >= 30));
    // No app runs: the quit is answered at once, as are the log and a
    // launch, which waits for the disk.
    let quit = host.send(Request::AppQuit {
        app: APP.into(),
        force: false,
    });
    assert!(matches!(
        host.reply(quit),
        Outcome::Error {
            code: ErrorCode::NoSuchApp,
            ..
        }
    ));
    let log = host.send(Request::LogFetch);
    assert_eq!(host.reply(log), Outcome::Ok { data: None });
    let launch = host.send(Request::AppLaunch {
        path: "/x.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(
        host.reply(launch),
        Outcome::Error {
            code: ErrorCode::Busy,
            ..
        }
    ));
    // The shutdown cancels the copy, then the guest shuts down: both
    // answers come within seconds, not after the 30 s of the copy.
    let down = host.send(Request::Shutdown {
        password: "pw".into(),
    });
    match host.reply(data) {
        Outcome::Error { code, text } => {
            assert_eq!(code, ErrorCode::Io);
            assert!(text.contains("quit"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(host.reply(down), Outcome::Ok { data: None });
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    let st = st.lock().unwrap();
    assert_eq!((st.data_prepare_cancels, st.shutdowns), (1, 1));
    assert_eq!(st.logs_sent.len(), 1);
}

#[test]
fn a_prepare_that_ends_answers_and_frees_the_agent() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    st.lock().unwrap().data_prepare_delay = Duration::from_millis(200);
    let _agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let first = prepare(&mut host);
    // A second prepare while the first runs takes over its reply.
    let second = prepare(&mut host);
    assert_eq!(
        host.reply(second),
        Outcome::Ok {
            data: Some(ReplyData::DataPrepare {
                verify: "verified".into()
            })
        }
    );
    assert!(!host
        .seen
        .iter()
        .any(|e| matches!(e, Event::Reply(r) if r.req == first)));
    assert_eq!(st.lock().unwrap().data_prepares, vec!["d1"]);
    // Free again: a launch runs, and a shutdown is answered at once.
    let launch = host.send(Request::AppLaunch {
        path: "/x.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(launch), Outcome::Ok { .. }));
}
