//! `shares.open` of the agent's main loop: the count of the open files
//! per share runs in a thread (lsof can take seconds), and its reply
//! comes back through the channel of the loop.

use super::{Msg, Session};
use crate::guest::platform::Platform;
use crate::macvm::proto::{Reply, ReplyData};

impl<P: Platform> Session<'_, P> {
    pub(super) fn shares_open(&mut self, req: u64) -> Result<(), String> {
        let job = self.p.open_files();
        let (tx, waker) = (self.tx.clone(), self.waker.clone());
        std::thread::Builder::new()
            .name("shares-open".into()) // not user text
            .spawn(move || {
                let reply = match job() {
                    Ok(per_share) => Reply::data(req, ReplyData::OpenFiles { per_share }),
                    Err((code, text)) => Reply::error(req, code, text),
                };
                let _ = tx.send(Msg::Done(reply));
                waker();
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
