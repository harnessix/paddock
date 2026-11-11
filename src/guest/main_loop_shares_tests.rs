//! `shares.open` through the dispatch of the agent: the count of the
//! platform comes back as `open_files`, a failed count as an error, and
//! the loop answers other requests meanwhile.

use super::tests::{start, Host};
use crate::guest::platform::fake::Fake;
use crate::macvm::proto::{ErrorCode, Outcome, ReplyData, Request};
use std::os::unix::net::UnixStream;

#[test]
fn shares_open_answers_the_count_or_the_error() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    // No count: the error of the platform.
    let req = host.send(Request::SharesOpen);
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::Io,
            ..
        }
    ));
    st.lock().unwrap().open_files = Some([("ws-1".to_string(), 2)].into());
    let req = host.send(Request::SharesOpen);
    assert_eq!(
        host.reply(req),
        Outcome::Ok {
            data: Some(ReplyData::OpenFiles {
                per_share: [("ws-1".to_string(), 2)].into()
            })
        }
    );
    let req = host.send(Request::AgentRestart);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(agent.join().unwrap(), 0);
}
