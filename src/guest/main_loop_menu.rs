//! The menu bar of the app in the agent's main loop: the `menu` request, the
//! `menu` event after a focus change or a menu notification when the tree
//! differs from the last one sent, the 20 KiB cap, and `menu.press`.

use super::Session;
use crate::guest::platform::{Fail, Platform};
use crate::macvm::menu::{self, MenuItem, CAP};
use crate::macvm::proto::{ErrorCode, Event, Reply};
use crate::macvm::text;

/// The capped tree and whether the cap cut it, or the failure.
type Tree = Result<(Vec<MenuItem>, bool), Fail>;

impl<P: Platform> Session<'_, P> {
    /// The menu bar of the app, capped. None without an app.
    fn menu_tree(&mut self) -> Option<Tree> {
        let pid = self.st.follow.root?;
        Some(self.p.menu_read(pid).map(|mut items| {
            let cut = menu::cap(&mut items, CAP);
            (items, cut)
        }))
    }

    /// `menu`: the tree as an event, then the reply. A tree that fails to
    /// read is the error of the reply, and the host keeps its last tree.
    pub(super) fn menu_request(&mut self, req: u64) -> Result<Reply, String> {
        let app = self.st.app.clone().unwrap_or_default();
        match self.menu_tree() {
            None => Ok(Reply::error(
                req,
                ErrorCode::NoSuchApp,
                text::app_not_running(&app),
            )),
            Some(Err((code, detail))) => Ok(Reply::error(req, code, detail)),
            Some(Ok((items, cut))) => {
                self.st.menu_dirty = false;
                self.st.menu_last = Some(items.clone());
                self.writer.send(&Event::Menu { app, items, cut })?;
                Ok(Reply::ok(req))
            }
        }
    }

    /// The poll: after a focus change or a menu notification, the tree
    /// goes out when it differs from the last one sent. A read that
    /// times out is one log line; the host keeps its tree.
    pub(super) fn menu_poll(&mut self) -> Result<(), String> {
        if self.p.take_menu_dirty() {
            self.st.menu_dirty = true;
        }
        if self.st.follow.root.is_none() {
            self.st.menu_last = None;
            self.st.menu_dirty = false;
            return Ok(());
        }
        if !self.st.menu_dirty || !self.st.follow.app_focused {
            return Ok(());
        }
        self.st.menu_dirty = false;
        match self.menu_tree() {
            Some(Ok((items, cut))) => {
                if self.st.menu_last.as_ref() == Some(&items) {
                    return Ok(());
                }
                self.st.menu_last = Some(items.clone());
                let app = self.st.app.clone().unwrap_or_default();
                self.writer.send(&Event::Menu { app, items, cut })
            }
            Some(Err((code, detail))) => {
                log::warn!("the menu bar could not be read ({code:?}): {detail}");
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// `menu.press`: the press, then the next poll reads the tree again
    /// because a press can change a checked or enabled state.
    pub(super) fn menu_press(&mut self, req: u64, path: &[usize]) -> Reply {
        let Some(pid) = self.st.follow.root else {
            let app = self.st.app.clone().unwrap_or_default();
            return Reply::error(req, ErrorCode::NoSuchApp, text::app_not_running(&app));
        };
        if path.is_empty() {
            return Reply::error(req, ErrorCode::BadRequest, "an empty menu path");
        }
        match self.p.menu_press(pid, path) {
            Ok(()) => {
                self.st.menu_dirty = true;
                self.next_poll = std::time::Instant::now();
                Reply::ok(req)
            }
            Err((code, detail)) => Reply::error(req, code, detail),
        }
    }
}
