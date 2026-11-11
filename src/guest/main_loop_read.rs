//! The socket thread of the main loop: every line to the agent thread,
//! except `ping`, which it answers at once.

use super::super::platform::Waker;
use super::{monotonic_ms, Msg, Writer};
use crate::macvm::proto::{Call, Event, Reply, Request};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;

/// The socket thread: every line to the agent thread, except `ping`,
/// answered here at once.
pub(super) fn read_loop(mut stream: UnixStream, tx: Sender<Msg>, writer: Writer, waker: Waker) {
    loop {
        match crate::macvm::proto::read_line::<Call>(&mut stream) {
            Ok(Some(call)) => {
                if let Request::Ping { .. } = call.request {
                    log::debug!("<- {}", call.kind());
                    let sent = writer
                        .send(&Event::Pong { t: monotonic_ms() })
                        .and_then(|_| writer.send(&Event::Reply(Reply::ok(call.req))));
                    if sent.is_err() {
                        let _ = tx.send(Msg::Closed(Some("cannot answer a ping".into())));
                        waker();
                        return;
                    }
                    continue;
                }
                if tx.send(Msg::Call(call)).is_err() {
                    return;
                }
                waker();
            }
            Ok(None) => {
                let _ = tx.send(Msg::Closed(None));
                waker();
                return;
            }
            Err(e) => {
                let _ = tx.send(Msg::Closed(Some(format!("{e:#}"))));
                waker();
                return;
            }
        }
    }
}
