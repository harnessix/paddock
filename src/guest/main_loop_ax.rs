//! The Accessibility mirror in the agent's main loop: the `ax.tree` request,
//! the `ax.tree` event after a change of the focused element or a value while
//! the host watches a window, the byte cap, and `ax.press` and `ax.set`. The
//! host names the window it watches with its request; the agent reads that
//! window again on each Accessibility change, at most once per
//! [`TREE_READ_GAP`], and sends the tree only when it differs from the last
//! one sent.

use super::Session;
use crate::guest::platform::{Fail, Platform};
use crate::macvm::axmirror::{self, Node, CAP};
use crate::macvm::proto::{ErrorCode, Event, Reply};
use crate::macvm::text;
use std::time::{Duration, Instant};

/// Two reads of the tree are at least this far apart, so a stream of
/// value changes costs one read per quarter second.
pub const TREE_READ_GAP: Duration = Duration::from_millis(250);

/// What the agent keeps between polls for the mirror.
#[derive(Default)]
pub struct AxTreeState {
    /// The window the host watches: the one of its last `ax.tree`
    /// request.
    pub window: Option<u32>,
    last: Option<Vec<Node>>,
    dirty: bool,
    read_at: Option<Instant>,
}

/// The capped tree and whether it was cut, or the failure.
type Tree = Result<(Vec<Node>, bool), Fail>;

impl<P: Platform> Session<'_, P> {
    /// The tree of the window, capped. None when the window is unknown.
    fn ax_tree_read(&mut self, window: u32) -> Option<Tree> {
        let w = self.st.snapshot.get(window)?.clone();
        Some(self.p.ax_tree(&w).map(|(mut nodes, cut)| {
            let capped = axmirror::cap(&mut nodes, CAP);
            (nodes, cut || capped)
        }))
    }

    /// `ax.tree`: the tree as an event, then the reply, and the window
    /// is the watched one from now on. A tree that fails to read is the
    /// error of the reply, and the host keeps its last tree.
    pub(super) fn ax_tree_request(&mut self, req: u64, window: u32) -> Result<Reply, String> {
        match self.ax_tree_read(window) {
            None => Ok(Reply::error(
                req,
                ErrorCode::NoSuchWindow,
                text::no_window(window),
            )),
            Some(Err((code, detail))) => Ok(Reply::error(req, code, detail)),
            Some(Ok((nodes, cut))) => {
                self.st.ax_tree.window = Some(window);
                self.st.ax_tree.dirty = false;
                self.st.ax_tree.read_at = Some(Instant::now());
                self.st.ax_tree.last = Some(nodes.clone());
                self.writer.send(&Event::AxTree { window, nodes, cut })?;
                Ok(Reply::ok(req))
            }
        }
    }

    /// The poll: after an Accessibility change of the app, the watched
    /// window's tree goes out when it differs from the last one sent. A
    /// window that is gone ends the watch (the host closes its mirror
    /// with the window). A read that fails is one log line.
    pub(super) fn ax_tree_poll(&mut self) -> Result<(), String> {
        if self.p.take_ax_tree_dirty() {
            self.st.ax_tree.dirty = true;
        }
        let Some(window) = self.st.ax_tree.window else {
            return Ok(());
        };
        if self.st.snapshot.get(window).is_none() {
            self.st.ax_tree = AxTreeState::default();
            return Ok(());
        }
        if !self.st.ax_tree.dirty || !self.st.follow.app_focused {
            return Ok(());
        }
        if self
            .st
            .ax_tree
            .read_at
            .is_some_and(|t| t.elapsed() < TREE_READ_GAP)
        {
            return Ok(());
        }
        self.st.ax_tree.dirty = false;
        match self.ax_tree_read(window) {
            Some(Ok((nodes, cut))) => {
                self.st.ax_tree.read_at = Some(Instant::now());
                if self.st.ax_tree.last.as_ref() == Some(&nodes) {
                    return Ok(());
                }
                self.st.ax_tree.last = Some(nodes.clone());
                self.writer.send(&Event::AxTree { window, nodes, cut })
            }
            Some(Err((code, detail))) => {
                log::warn!("the tree of window {window} could not be read ({code:?}): {detail}");
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// `ax.press`: the press, then the next poll reads the tree again
    /// because a press changes a state.
    pub(super) fn ax_tree_press(
        &mut self,
        req: u64,
        window: u32,
        path: &[usize],
        role: &str,
    ) -> Reply {
        let Some(w) = self.st.snapshot.get(window).cloned() else {
            return Reply::error(req, ErrorCode::NoSuchWindow, text::no_window(window));
        };
        let r = self.p.ax_tree_press(&w, path, role);
        self.ax_tree_after_action(window, r, req)
    }

    /// `ax.set`: the value, then the tree again.
    pub(super) fn ax_tree_set(
        &mut self,
        req: u64,
        window: u32,
        path: &[usize],
        role: &str,
        value: &str,
    ) -> Reply {
        let Some(w) = self.st.snapshot.get(window).cloned() else {
            return Reply::error(req, ErrorCode::NoSuchWindow, text::no_window(window));
        };
        let r = self.p.ax_tree_set(&w, path, role, value);
        self.ax_tree_after_action(window, r, req)
    }

    fn ax_tree_after_action(&mut self, window: u32, r: Result<(), Fail>, req: u64) -> Reply {
        match r {
            Ok(()) => {
                self.st.ax_tree.window = Some(window);
                self.st.ax_tree.dirty = true;
                self.st.ax_tree.read_at = None;
                self.next_poll = Instant::now();
                Reply::ok(req)
            }
            Err((code, detail)) => Reply::error(req, code, detail),
        }
    }
}
