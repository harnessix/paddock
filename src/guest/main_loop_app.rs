//! The app memory of the agent's main loop: `PID PATH` in the state
//! file, so an agent that launchd restarted follows the same app again.
//! Also `app.copy`, which runs in a thread of its own.

use super::{Msg, Session, State};
use crate::guest::apps;
use crate::guest::configure;
use crate::guest::platform::Platform;
use crate::macvm::proto::{ErrorCode, Reply};
use crate::macvm::text;
use std::path::Path;

/// The app memory of the agent: `PID PATH` in the state file, so an
/// agent that launchd restarted follows the same app again.
pub(super) fn load_app(p: &mut impl Platform, st: &mut State) {
    let Some(path) = p.state_file() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some((pid, app)) = text.trim().split_once(' ') else {
        return;
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return;
    };
    if p.procs().contains_key(&pid) {
        log::info!("following the app {app} (pid {pid}) of the last agent again");
        st.follow.root = Some(pid);
        st.app = Some(app.to_string());
    } else {
        let _ = std::fs::remove_file(&path);
    }
}

pub(super) fn save_app(p: &mut impl Platform, st: &State) {
    let Some(path) = p.state_file() else {
        return;
    };
    match (st.follow.root, &st.app) {
        (Some(pid), Some(app)) => {
            let line = format!("{pid} {app}\n"); // not user text
            if let Err(e) = std::fs::write(&path, line) {
                log::warn!("cannot write {}: {e}", path.display());
            }
        }
        _ => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

impl<P: Platform> Session<'_, P> {
    /// `app.copy`: the checks here, then the copy in a thread of its own, so
    /// the health timer and the window poll keep running through a copy that
    /// takes half a minute.
    pub(super) fn app_copy(
        &mut self,
        req: u64,
        from: &Path,
        to: &Path,
        unquarantine: bool,
        busy: bool,
    ) -> Option<Reply> {
        let name = to
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if busy {
            // The old copy is replaced only after the app quit.
            return Some(Reply::error(
                req,
                ErrorCode::Busy,
                text::copy_while_running(&name),
            ));
        }
        if self.st.copy_running {
            return Some(Reply::error(
                req,
                ErrorCode::Busy,
                text::copy_already_running(&name),
            ));
        }
        let copy = match apps::plan_copy(from, to, unquarantine) {
            Ok(c) => c,
            Err((code, text)) => return Some(Reply::error(req, code, text)),
        };
        let free = self.p.health().data_disk_free;
        let (tx, waker) = (self.tx.clone(), self.waker.clone());
        self.st.copy_running = true;
        std::thread::Builder::new()
            .name("app-copy".into())
            .spawn(move || {
                let run = |plan: &configure::Plan| configure::spawn(plan, None);
                let reply = match apps::run_copy(&copy, &name, free, &run) {
                    Ok(()) => Reply::ok(req),
                    Err((code, text)) => Reply::error(req, code, text),
                };
                let _ = tx.send(Msg::Done(reply));
                waker();
            })
            .expect("a thread");
        None
    }
}
