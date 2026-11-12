//! The password guard from the launch reply on. The main loop dispatches
//! every queued call before its next poll, so these tests call `dispatch`
//! back to back with no poll in between: the order is fixed, not timed.

use super::*;
use crate::guest::platform::fake::Fake;
use crate::macvm::proto::{read_line, Outcome};

fn launch(req: u64) -> Call {
    Call {
        req,
        request: Request::AppLaunch {
            path: "/Applications/TextEdit.app".into(),
            args: vec![],
            env: Default::default(),
        },
    }
}

/// The replies that the dispatched calls wrote. `dispatch` writes a
/// direct reply before it returns, so every such reply is in the socket
/// already: read until the socket has nothing more, never wait.
fn replies(s: &mut UnixStream) -> Vec<(u64, Outcome)> {
    s.set_nonblocking(true).unwrap();
    let mut out = Vec::new();
    loop {
        match read_line::<Event>(s) {
            Ok(Some(Event::Reply(r))) => out.push((r.req, r.outcome)),
            Ok(Some(_)) => {}
            Ok(None) => return out,
            Err(e) => match e.downcast_ref::<std::io::Error>() {
                Some(io) if io.kind() == std::io::ErrorKind::WouldBlock => return out,
                _ => panic!("{e:#}"),
            },
        }
    }
}

fn code(o: &Outcome) -> Option<ErrorCode> {
    match o {
        Outcome::Error { code, .. } => Some(*code),
        _ => None,
    }
}

#[test]
fn a_call_queued_behind_a_launch_sees_the_app() {
    let (a, mut b) = UnixStream::pair().unwrap();
    let (mut fake, fst) = Fake::new();
    let mut st = State::default();
    let mut s = Session::new(&mut fake, &mut st, a);
    let calls = [
        launch(1),
        Call {
            req: 2,
            request: Request::DataPrepare {
                password: "pw".into(),
                data_id: "d-1".into(),
            },
        },
        Call {
            req: 3,
            request: Request::Shutdown {
                password: "pw".into(),
            },
        },
        launch(4),
    ];
    for call in calls {
        assert!(s.dispatch(call).unwrap().is_none());
    }
    let got: Vec<(u64, Option<ErrorCode>)> =
        replies(&mut b).iter().map(|(r, o)| (*r, code(o))).collect();
    drop(s);
    assert_eq!(
        got,
        [
            (1, None),
            (2, Some(ErrorCode::NotInSession)),
            (3, Some(ErrorCode::NotInSession)),
            (4, Some(ErrorCode::Busy)),
        ],
        "the guard refuses every call behind the launch"
    );
    let fst = fst.lock().unwrap();
    assert!(fst.data_prepares.is_empty());
    assert_eq!(fst.shutdowns, 0);
    assert_eq!(fst.launched.len(), 1);
}
