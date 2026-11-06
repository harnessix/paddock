//! The guest messages of the shim `paddock-vm app`, macOS only: the hello
//! with its version check, the token and the agent update, the data disk on
//! the first boot, the health, the app exit, the replies and their
//! timeouts. The screen lock and the keychain are in `run_screen.rs`.
//! `run.rs` holds the session.

use super::failure::Step;
use super::proto::{self, ErrorCode, Outcome, Request};
use super::quit::Input;
use super::run::{Session, Why};
use super::run_vm::say;
use super::session::Profile;
use super::text;
use super::version::{self, paddock_version, Direction, Followup};
use std::path::PathBuf;
use std::time::Instant;

impl Session<'_> {
    pub(super) fn guest_message(&mut self, ev: proto::Event) {
        log::debug!("macvm: <- {}", ev.kind());
        match ev {
            proto::Event::Hello(h) => self.hello(h),
            proto::Event::Health(h) => {
                let steps = self.watch.alive(Instant::now());
                self.execute(steps);
                self.last_health = Some(Instant::now());
                if let Some(v) = h.data_verify.as_deref().filter(|_| !self.data_verify_seen) {
                    // The agent verified the volume after an unclean end.
                    self.data_verify_seen = true;
                    self.events.record("data", &text::data_verified(v), &[]);
                }
                self.screen_report(h.locked, h.uptime, None);
                self.write_health(&h);
            }
            proto::Event::Pong { .. } => {
                let steps = self.watch.alive(Instant::now());
                self.execute(steps);
                self.last_health = Some(Instant::now());
            }
            proto::Event::AppExited {
                code, crash_report, ..
            } => {
                self.events.record(
                    "app",
                    &text::app_exited(&self.plan.app, code),
                    &[("code", code as f64)],
                );
                if self.quitting {
                    self.feed(Input::AppExited);
                } else {
                    // The crash row: the Dock entry, or after three
                    // crashes in a minute the line with the report path.
                    let report = crash_report.map(|p| {
                        self.plan
                            .profile
                            .logs()
                            .join(p.file_name().unwrap_or_default())
                    });
                    let steps = self.watch.app_exited(code, report, Instant::now());
                    self.execute(
                        steps
                            .into_iter()
                            .filter(|s| !matches!(s, Step::Event { kind: "app", .. }))
                            .filter(|s| !matches!(s, Step::Line(l) if l.starts_with(&self.plan.app) && l.contains("exited with code")))
                            .collect(),
                    );
                    self.menu_app_exited();
                    self.ax_app_exited();
                }
            }
            proto::Event::Windows(ev) => self.on_windows(ev),
            proto::Event::Focus { window, .. } => self.on_focus(window),
            proto::Event::AxValue {
                window,
                text: value,
            } => self.on_ax_value(window, value),
            proto::Event::Reply(r) => self.reply(r),
            proto::Event::Disk { data_free, warn } => {
                if warn {
                    say(&text::disk_warn(data_free));
                }
                self.events
                    .record("disk", "data disk space", &[("free", data_free as f64)]);
            }
            proto::Event::DataUnmountable { text: detail } => {
                let backup = newest_backup(&self.plan.profile);
                let line = text::data_unmountable(&self.plan.app, backup.as_deref());
                say(&line);
                self.events.record("error", &detail, &[]);
                self.events.record("error", &line, &[]);
                self.end_start_without_save();
            }
            proto::Event::DataProgress {
                text: line,
                wait_secs,
            } => {
                let wait = std::time::Duration::from_secs(wait_secs);
                let actions = self.data.progress(&line, wait);
                self.data_actions(actions);
            }
            proto::Event::Dialog {
                owner,
                text: detail,
                dismissed,
            } => {
                // A disk dialog in the guest: the agent pressed Ignore, or
                // it waits on the user, who sees it as a `system` window.
                if dismissed {
                    let line = text::guest_dialog_dismissed(&owner, &detail);
                    log::info!("macvm: {line}");
                    self.events.record("dialog", &line, &[]);
                } else {
                    let line = text::guest_dialog_open(&owner, &detail);
                    say(&line);
                    self.events.record("dialog", &line, &[]);
                }
            }
            proto::Event::Clipboard {
                count,
                items,
                transfer,
            } => self.guest_clipboard(count, items, transfer),
            proto::Event::Menu { items, cut, .. } => self.on_menu(items, cut),
            proto::Event::AxTree { window, nodes, cut } => self.on_ax_tree(window, nodes, cut),
            proto::Event::ScreenLock {
                locked,
                uptime,
                idle_secs,
            } => self.screen_report(locked, uptime, idle_secs),
            proto::Event::LinkOpen { url } => self.link_open(url),
        }
    }

    /// The hello of the agent: the version check, the token, the update of
    /// the agent, then the data disk on the first boot.
    fn hello(&mut self, h: proto::Hello) {
        let since = self.begun.elapsed();
        say(&text::agent_dialed(&h.agent, since));
        self.events.record(
            "hello",
            &format!("agent {}, {} start", h.agent, self.start.word()), // not user text
            &[("secs", since.as_secs_f64())],
        );
        if self.hello_deadline.take().is_some() {
            // The first hello of this start: the record says how it went.
            self.write_start(since.as_secs_f64());
        }
        self.last_health = Some(Instant::now());
        self.stamp.agent = h.agent.clone();
        self.hello_windows(&h.macos);
        let host = paddock_version();
        if let Some((_before, at)) = self.update_sent.take() {
            match version::after_update(host, &h.agent) {
                Followup::Updated => self.events.record(
                    "agent",
                    &text::event_agent_updated(&h.agent, at.elapsed().as_secs_f64()),
                    &[("secs", at.elapsed().as_secs_f64())],
                ),
                Followup::RolledBack => {
                    say(&text::agent_rolled_back(host, &h.agent));
                    self.events
                        .record("agent", &text::event_agent_rollback(&h.agent), &[]);
                    self.update_refused = true;
                }
            }
        }
        let host_sha = self.agent_dir.as_ref().and_then(|d| {
            std::fs::read_to_string(d.join(super::guest_files::BINARY_SHA256))
                .ok()
                .map(|s| s.trim().to_string())
        });
        let action = match proto::check_hello_sha(&h, proto::VERSION, host, host_sha.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                say(&e.to_string());
                self.events.record("error", &e.to_string(), &[]);
                self.end(false, 1);
                return;
            }
        };
        self.send(
            Request::Token {
                token: self.token.clone(),
                v: proto::VERSION,
                bench: self.plan.bench,
            },
            Why::Token,
        );
        if action == proto::HelloAction::Update {
            let request = if self.update_refused {
                None
            } else {
                self.update_request()
            };
            match request {
                Some(req) => {
                    say(&text::agent_updating(&h.agent, host));
                    let down = version::direction(&h.agent, host) == Direction::Downgrade;
                    self.events.record(
                        "agent",
                        &text::event_agent_update(&h.agent, host, down),
                        &[],
                    );
                    self.update_sent = Some((h.agent.clone(), Instant::now()));
                    self.send(req, Why::AgentUpdate);
                    return;
                }
                None if h.v.talks(proto::VERSION) => {}
                None => {
                    let line = text::agent_cannot_update(
                        &h.agent,
                        &h.v.to_string(),
                        host,
                        &proto::VERSION.to_string(),
                    );
                    say(&line);
                    self.events.record("error", &line, &[]);
                    self.end(false, 1);
                    return;
                }
            }
        }
        // The guest gets the host's input sources, and the host asks for
        // the menu bar again after a restart.
        self.input_hello();
        self.menu_hello();
        self.ax_hello();
        let hello = super::data_start::Hello {
            prepared: self.stamp.data_prepared,
            warm: self.start == super::session::Start::Warm,
            password: self.password.as_deref(),
            data_id: &self.stamp.data_id,
        };
        let actions = self.data.hello(&hello);
        self.data_actions(actions);
    }

    /// Run the actions of the data disk machine (`data_start.rs`).
    pub(super) fn data_actions(&mut self, actions: Vec<super::data_start::Action>) {
        use super::data_start::Action;
        for a in actions {
            match a {
                Action::Send { password, data_id } => {
                    if !self.send(Request::DataPrepare { password, data_id }, Why::DataPrepare) {
                        let steps = self.data.timeout(std::time::Duration::ZERO);
                        self.data_actions(steps);
                    }
                }
                Action::StartApp => {
                    // The login keychain first: the agent answers the
                    // requests in order, so the app finds it unlocked.
                    self.keychain_request();
                    self.start_app();
                }
                Action::Event { kind, text: line } => self.events.record(kind, &line, &[]),
                Action::Line(line) => say(&line),
                Action::Stamp { prepared, error } => {
                    self.stamp.data_prepared = prepared;
                    self.stamp.data_error = error;
                    if let Err(e) = self.stamp.write(&self.plan.profile.stamp()) {
                        say(&text::stamp_write_failed(&format!("{e:#}")));
                    }
                }
                Action::Extend(wait) => {
                    let at = Instant::now() + wait;
                    let reqs: Vec<u64> = self
                        .why
                        .iter()
                        .filter(|(_, w)| **w == Why::DataPrepare)
                        .map(|(r, _)| *r)
                        .collect();
                    if let Some(agent) = self.agent.as_mut() {
                        for r in reqs {
                            agent.extend(r, at);
                        }
                    }
                }
                Action::EndLocked { detail } => {
                    // The data disk belongs to another system disk, so it
                    // cannot mount: the line names the restore, and the
                    // start ends without a save. Nothing of the disks
                    // changes.
                    let backup = super::backup::newest(&self.plan.profile).map(|b| b.dir);
                    let line = text::data_locked(&self.plan.profile.dir, backup.as_deref());
                    say(&line);
                    self.events.record("error", &detail, &[]);
                    self.events.record("error", &line, &[]);
                    self.end_start_without_save();
                }
            }
        }
    }

    /// The `agent.update` request from the agent share: the path in the
    /// guest and the hash next to the binary on the host.
    pub(super) fn update_request(&self) -> Option<Request> {
        let dir = self.agent_dir.as_ref()?;
        let binary = dir.join(super::guest_files::BINARY);
        let sha = std::fs::read_to_string(dir.join(super::guest_files::BINARY_SHA256))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() == 64);
        let Some(sha256) = sha else {
            say(&text::agent_no_update_file(&binary));
            return None;
        };
        Some(Request::AgentUpdate {
            path: PathBuf::from(proto::SHARE_ROOT)
                .join("agent")
                .join(super::guest_files::BINARY),
            sha256,
            version: paddock_version().into(),
        })
    }

    /// A reply of the agent: the pending request and what it was for.
    fn reply(&mut self, r: proto::Reply) {
        let pending = self.agent.as_mut().and_then(|a| a.take(r.req));
        let why = self.why.remove(&r.req).unwrap_or(Why::Other);
        let kind = pending.map(|p| p.kind).unwrap_or_else(|| "?".into());
        match (why, r.outcome) {
            (Why::DataPrepare, outcome) => {
                let actions = self.data.answer(&outcome);
                self.data_actions(actions);
            }
            (Why::Keychain, outcome) => self.keychain_reply(outcome, false),
            (Why::Other, outcome) if kind == "bench.keychain" => self.keychain_reply(outcome, true),
            (Why::AppCopy, Outcome::Ok { .. }) => self.app_copied(),
            (Why::AppCopy, Outcome::Error { code, text: detail }) => {
                self.app_copy_failed(&text::io_failed(code.describe(), &detail));
            }
            (Why::AppLaunch, Outcome::Error { code, text: detail }) => {
                let line = text::launch_refused(&self.plan.app, code.describe(), &detail);
                say(&line);
                self.events.record("error", &line, &[]);
            }
            (Why::AgentUpdate, Outcome::Ok { .. }) => {
                // The agent exits and launchd restarts it: a new hello comes.
                self.hello_deadline = Some(Instant::now() + proto::timeout(&Request::AgentRestart));
                self.events.record("agent", "the agent updates itself", &[]);
            }
            (Why::AgentUpdate, Outcome::Error { code, text: detail }) => {
                say(&text::agent_update_failed());
                let error = text::io_failed(code.describe(), &detail);
                self.events
                    .record("error", &text::request_failed(&kind, &error), &[]);
            }
            (Why::Drop, Outcome::Ok { .. }) => {
                say(&text::dropped_as_open());
                self.events.record("drop", &text::dropped_as_open(), &[]);
            }
            (Why::Drop, Outcome::Error { code, text: detail }) => {
                say(&text::drop_failed());
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                self.events.record("drop", &line, &[]);
            }
            (Why::ClipboardSpill, _) => self.spill_done(),
            (Why::InputSet, Outcome::Ok { data }) => {
                let missing = match data {
                    Some(proto::ReplyData::InputSet { missing }) => missing,
                    _ => Vec::new(),
                };
                self.input_reply(&missing);
            }
            (Why::InputSet, Outcome::Error { code, text: detail }) => {
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                log::warn!("macvm: {line}");
                self.events.record("error", &line, &[]);
                self.input_failed();
            }
            (Why::Menu, Outcome::Ok { .. }) => self.menu_answered(),
            (Why::Menu, Outcome::Error { code, text: detail }) => {
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                log::warn!("macvm: {line}");
                self.events.record("error", &line, &[]);
                self.menu_timeout();
            }
            (Why::MenuPress, Outcome::Ok { .. }) => self.menu_request(),
            (Why::AxTree, Outcome::Ok { .. }) => self.ax_answered(),
            (Why::AxTree, Outcome::Error { code, text: detail }) => {
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                log::warn!("macvm: {line}");
                self.events.record("error", &line, &[]);
                self.ax_timeout();
            }
            (Why::AxAction, Outcome::Ok { .. }) => self.ax_action_done(),
            (Why::AxAction, Outcome::Error { code, text: detail }) => {
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                log::warn!("macvm: {line}");
                self.events.record("ax", &line, &[]);
                self.ax_action_done();
            }
            (Why::AppQuit, outcome) => {
                if let Outcome::Error { code, text: detail } = &outcome {
                    let line =
                        text::request_failed(&kind, &text::io_failed(code.describe(), detail));
                    log::info!("macvm: {line}");
                }
                if let Some(input) = super::quit::quit_answer(&outcome) {
                    self.feed(input);
                }
            }
            (Why::LogFetch, Outcome::Error { code, text: detail }) => {
                self.log_wanted = false;
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                log::warn!("macvm: {line}");
                self.events.record("files", &line, &[]);
            }
            (Why::Shutdown, Outcome::Error { code, text: detail }) => {
                let detail = text::io_failed(code.describe(), &detail);
                log::warn!("macvm: {}", text::request_failed(&kind, &detail));
                self.feed(super::quit::Input::ShutdownRefused(detail));
            }
            (Why::Lead(id), outcome) => {
                if let Outcome::Error { code, text: detail } = &outcome {
                    let line =
                        text::request_failed(&kind, &text::io_failed(code.describe(), detail));
                    log::debug!("macvm: {line}");
                }
                self.lead_answered(id);
            }
            (_, Outcome::Ok { .. }) => {}
            (_, Outcome::Error { code, text: detail }) => {
                let line = text::request_failed(&kind, &text::io_failed(code.describe(), &detail));
                log::warn!("macvm: {line}");
                if code != ErrorCode::NoSuchApp {
                    self.events.record("error", &line, &[]);
                }
            }
        }
    }
}

impl Session<'_> {
    /// The requests that got no reply in their time: the ones whose
    /// silence changes the host state.
    pub(super) fn request_timeouts(&mut self, timed_out: &[Why]) {
        if timed_out.contains(&Why::DataPrepare) {
            let wait = proto::timeout(&Request::DataPrepare {
                password: String::new(),
                data_id: String::new(),
            });
            let actions = self.data.timeout(wait);
            self.data_actions(actions);
        }
        if timed_out.contains(&Why::LogFetch) {
            self.log_wanted = false;
            let wait = proto::timeout(&Request::LogFetch).as_secs();
            self.events
                .record("files", &text::log_fetch_missing(wait), &[]);
        }
        if timed_out.contains(&Why::InputSet) {
            self.input_failed();
        }
        if timed_out.contains(&Why::Menu) {
            self.menu_timeout();
        }
        if timed_out.contains(&Why::AxTree) {
            self.ax_timeout();
        }
        for why in timed_out {
            if let Why::Lead(id) = why {
                self.lead_answered(*id);
            }
        }
    }
}

/// The newest backup of the pair, as its folder name.
fn newest_backup(profile: &Profile) -> Option<String> {
    super::backup::newest(profile).map(|b| b.name)
}
