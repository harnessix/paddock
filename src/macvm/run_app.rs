//! The app of a session: the copy into the guest once per version, and the
//! launch with the workspace folders and the proxy variables. macOS only;
//! every decision it takes is a pure function of `appcopy.rs`, tested on
//! Linux.
//!
//! The order after the guest is ready:
//!
//! - A warm start finds the app running in the restored state. Nothing
//!   happens: a copy would have to replace a bundle that runs, which the
//!   agent refuses, and a launch would open a second window set.
//! - Any other start compares the host bundle with `stamp.app`. On a
//!   difference it sends `app.copy` and launches on the reply; the stamp
//!   gets the new version only after a copy that worked. On a failed copy
//!   the old copy in the guest stays and the app launches from it.

use super::appcopy::{self, HostBundle};
use super::configure;
use super::proto::Request;
use super::run::{Session, Why};
use super::run_vm::say;
use super::session::Start;
use super::text;
use std::time::Instant;

impl Session<'_> {
    /// The clone of the host bundle in the profile, which the `apps`
    /// share carries into the guest. None when this session runs no app,
    /// or when the clone is not readable.
    pub(super) fn app_bundle(&self) -> Option<HostBundle> {
        let path = self.plan.app_bundle.as_ref()?;
        match appcopy::read(path) {
            Ok(b) => Some(b),
            Err(e) => {
                let line = format!("{e:#}"); // not user text
                say(&line);
                self.events.record("error", &line, &[]);
                None
            }
        }
    }

    /// The guest is ready: copy the app when the guest has another
    /// version of it, else launch it.
    pub(super) fn start_app(&mut self) {
        if self.app_started || self.quitting {
            return;
        }
        self.app_started = true;
        let Some(bundle) = self.app_bundle() else {
            return;
        };
        if self.start == Start::Warm {
            // The restored state runs the app, unless the app quit before
            // the save (a quit from the Dock menu quits the app first). An
            // open of a running app only brings it to the front, so the
            // warm start always opens it.
            if appcopy::needs_copy(self.stamp.app.as_ref(), &bundle.stamp()) {
                self.events
                    .record("copy", &text::copy_while_running(&bundle.display_name), &[]);
            }
            self.launch_app(&bundle);
            return;
        }
        if !appcopy::needs_copy(self.stamp.app.as_ref(), &bundle.stamp()) {
            self.launch_app(&bundle);
            return;
        }
        self.copy_app(&bundle);
    }

    /// `app.copy` of the staged archive into the guest home; the launch
    /// follows its reply.
    pub(super) fn copy_app(&mut self, bundle: &HostBundle) {
        self.copy_tried = true;
        say(&text::copy_begin(&bundle.display_name, bundle.bytes));
        self.events.record(
            "copy",
            &text::copy_begin(&bundle.display_name, bundle.bytes),
            &[("bytes", bundle.bytes as f64)],
        );
        self.copy_begun = Some(Instant::now());
        let request = appcopy::request(bundle);
        if !self.send(request, Why::AppCopy) {
            self.copy_begun = None;
        }
    }

    /// The guest has no copy of the app, for example after a warm start
    /// of a state saved before the first copy finished. True when a copy
    /// starts now; the launch follows it.
    pub(super) fn copy_missing_app(&mut self) -> bool {
        if self.copy_tried || self.quitting {
            return false;
        }
        let Some(bundle) = self.app_bundle() else {
            return false;
        };
        let staged = self
            .plan
            .app_bundle
            .as_ref()
            .and_then(|clone| clone.parent())
            .map(|apps| apps.join(format!("{}.app.{}", bundle.name, appcopy::ARCHIVE_EXT)))
            .is_some_and(|archive| archive.is_file());
        if !staged {
            return false;
        }
        self.copy_app(&bundle);
        true
    }

    /// `app.launch` with the workspace folders of the profile and the
    /// proxy variables of the guest proxy.
    pub(super) fn launch_app(&mut self, bundle: &HostBundle) {
        let request = Request::AppLaunch {
            path: bundle.guest_target(),
            args: self.plan.app_args.clone(),
            env: configure::launch_env(),
        };
        // The Dock menu names the app, not the profile.
        let name = bundle.display_name.clone();
        self.shell.dock.update(|s| s.app = name);
        self.write_app(&bundle.display_name, &bundle.version);
        if self.send(request, Why::AppLaunch) {
            self.events.record(
                "launch",
                &format!("app.launch {}", bundle.guest_target().display()), // not user text
                &[("secs", self.begun.elapsed().as_secs_f64())],
            );
        }
    }

    /// The copy worked: the stamp carries the version from now on, so the
    /// next start of this version copies nothing.
    pub(super) fn app_copied(&mut self) {
        let secs = self
            .copy_begun
            .take()
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let Some(bundle) = self.app_bundle() else {
            return;
        };
        let mut stamp = bundle.stamp();
        stamp.copied = Some(today());
        self.stamp.app = Some(stamp);
        if let Err(e) = self.stamp.write(&self.plan.profile.stamp()) {
            let line = text::stamp_write_failed(&format!("{e:#}"));
            say(&line);
            self.events.record("error", &line, &[]);
        }
        say(&text::copy_done(&bundle.display_name, secs));
        self.events.record(
            "copy",
            &text::copy_done(&bundle.display_name, secs),
            &[("secs", secs)],
        );
        self.launch_app(&bundle);
    }

    /// The copy failed: the old copy in the guest stays, and the app runs
    /// from it. The stamp keeps the old version, so the next start tries
    /// the copy again.
    pub(super) fn app_copy_failed(&mut self, detail: &str) {
        self.copy_begun = None;
        let Some(bundle) = self.app_bundle() else {
            return;
        };
        let line = text::copy_failed(&bundle.display_name, detail);
        say(&line);
        self.events.record("copy", &line, &[]);
        self.launch_app(&bundle);
    }
}

/// The day of the copy, `2026-11-02`, for `paddock app status`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let t = crate::waiting::format_rfc3339(secs);
    t.split('T').next().unwrap_or(&t).to_string()
}
