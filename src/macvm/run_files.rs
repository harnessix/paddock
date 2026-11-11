//! The files and the clipboard of a session on the host, macOS only: the
//! host pasteboard poll with the key rule of `clipboard::Sync`, the
//! `clipboard.set` requests with their spill files under
//! `drop/.clipboard/`, the `clipboard` events and the `guest-files`
//! transfers of the guest, the drops of `vz_mac_drop::DropVmView` staged by
//! `drop::stage`, and the sweep of the staging folders at quit. `run.rs`
//! calls `files_tick` once per run loop pass and the other methods from its
//! event handlers.
//!
//! "Key" is read as the shim being the active app: a session window is key
//! exactly when the shim is active, and the viewport windows are windows of
//! this process.

use super::clipboard::{self, Action, Item, Payload, Sync, FILE_URL};
use super::drop::{self, StageError, CLIPBOARD_DIR};
use super::files_port::{Limiter, TRANSFER_MAX};
use super::pasteboard;
use super::proto::{FileKind, PasteItem, Request, SHARE_ROOT};
use super::run::{Session, Why};
use super::run_vm::say;
use super::shares::{self, Share};
use super::text;
use super::vz_mac_drop::DropEvent;
use crate::vm::helper::AppPlan;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use std::collections::{BTreeMap, VecDeque};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The state of the files and the clipboard of one session.
pub(super) struct Files {
    pub sync: Sync,
    shares: Vec<Share>,
    /// The host pasteboard change count last seen.
    host_count: u64,
    /// The shim is the active app: a session window is key.
    active: bool,
    /// The sender for the `DropVmView` of each window (`windows.rs`
    /// clones it).
    pub drop_tx: Sender<DropEvent>,
    drop_rx: Receiver<DropEvent>,
    /// The admission of `guest-files` transfers.
    pub limiter: Arc<Mutex<Limiter>>,
    /// The clipboard transfers of the guest that arrived, by name, until
    /// their `clipboard` event names them.
    received: BTreeMap<String, PathBuf>,
    /// A `clipboard` event whose transfer has not arrived yet.
    pending: Option<(u64, String)>,
    /// The spill files of `clipboard.set`, oldest first, removed on the
    /// reply.
    spills: VecDeque<PathBuf>,
    spill_count: u64,
    /// The paths in `drop/` that the app had open when the quit began.
    open_at_quit: Vec<PathBuf>,
    /// The live grants of the session (`run_grants.rs`).
    pub grants: super::grants_live::Live,
    pub grant_pending: Option<super::run_grants::Pending>,
    pub fresh_handles: Option<(Sender<Result<String, String>>, std::time::Instant)>,
    /// When the shim last asked the guest for the open file handles.
    pub handles_asked: Option<std::time::Instant>,
    /// The agent does not count open files: no more asking.
    pub handles_off: bool,
}

impl Files {
    pub fn new(plan: &AppPlan) -> Self {
        let shares: Vec<Share> = plan
            .shares
            .iter()
            .map(|s| Share {
                name: s.name.clone(),
                host: s.path.clone(),
                read_only: s.read_only,
            })
            .collect();
        let (drop_tx, drop_rx) = mpsc::channel();
        let (saved, skipped) = super::grants::load_for_start(&plan.profile);
        if let Some(line) = skipped {
            say(&line);
        }
        let clip = saved.clipboard;
        let mut sync = Sync::new(shares.clone());
        sync.set_directions(clip.to_app, clip.from_app);
        Files {
            grants: super::grants_live::Live::new(&plan.app, shares.clone(), clip, &plan.network),
            grant_pending: None,
            fresh_handles: None,
            handles_asked: None,
            handles_off: false,
            sync,
            shares,
            host_count: pasteboard::count(),
            active: false,
            drop_tx,
            drop_rx,
            limiter: Arc::new(Mutex::new(Limiter::new())),
            received: BTreeMap::new(),
            pending: None,
            spills: VecDeque::new(),
            spill_count: 0,
            open_at_quit: Vec::new(),
        }
    }

    /// The shares after a grant change: the clipboard translates paths
    /// with them.
    pub fn set_shares(&mut self, shares: Vec<Share>) {
        self.sync.set_shares(shares.clone());
        self.shares = shares;
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Session<'_> {
    /// Once per run loop pass: the key state, the host pasteboard, the
    /// drops that the windows reported.
    pub(super) fn files_tick(&mut self) {
        let active = MainThreadMarker::new()
            .map(|mtm| NSApplication::sharedApplication(mtm).isActive())
            .unwrap_or(false);
        if active != self.files.active {
            self.files.active = active;
            self.send(Request::ClipboardWatch { on: active }, Why::Other);
            let actions = self.files.sync.on_key(active);
            self.act_clipboard(actions);
        }
        let count = pasteboard::count();
        if count != self.files.host_count {
            self.files.host_count = count;
            // The content is read only when it can go: while key, the
            // machine sends it; while not key, it keeps it for the key.
            let items = clipboard::convert(pasteboard::read(), &mut pasteboard::tiff_to_png);
            let items = self.stage_host_file_urls(items);
            let actions = self.files.sync.on_host_change(count, items);
            self.act_clipboard(actions);
        }
        while let Ok(ev) = self.files.drop_rx.try_recv() {
            self.host_drop(ev);
        }
    }

    /// A host file URL outside every share is a drop into `drop/` first so
    /// the machine can translate it to a guest path.
    fn stage_host_file_urls(&mut self, items: Vec<Item>) -> Vec<Item> {
        items
            .into_iter()
            .map(|mut item| {
                for (t, d) in &mut item.types {
                    if t != FILE_URL {
                        continue;
                    }
                    let Some(path) = std::str::from_utf8(d)
                        .ok()
                        .and_then(|u| clipboard::url_path(u.trim()))
                    else {
                        continue;
                    };
                    if shares::to_guest(&self.files.shares, &path).is_some() {
                        continue;
                    }
                    match drop::stage(&self.plan.profile, &[drop::Item::File(path)], now_secs()) {
                        Ok(staged) => {
                            let host = staged
                                .dir
                                .join(staged.guest_paths[0].file_name().unwrap_or_default());
                            *d = clipboard::path_url(&host).into_bytes();
                        }
                        Err(e) => log::warn!("macvm: a copied file does not cross: {e}"),
                    }
                }
                item
            })
            .collect()
    }

    pub(super) fn act_clipboard(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::SendToGuest(p) => self.send_clipboard(p),
                Action::SetHost(p) => match pasteboard::write(&p.items) {
                    Some(count) => {
                        self.files.host_count = count;
                        self.events.record(
                            "clipboard",
                            "the guest clipboard is on the host",
                            &[("bytes", p.size() as f64)],
                        );
                    }
                    None => log::warn!("macvm: the host pasteboard refused the write"),
                },
                Action::Nothing => {}
                Action::Refuse(badge) => {
                    say(&badge);
                    self.events.record("clipboard", &badge, &[]);
                }
            }
        }
    }

    /// `clipboard.set`: inline up to 64 KiB, else a JSON file under
    /// `drop/.clipboard/` that the guest reads from the share.
    fn send_clipboard(&mut self, p: Payload) {
        let count = self.files.host_count;
        let bytes = p.size();
        if p.inline() {
            self.send(
                Request::ClipboardSet {
                    count,
                    items: clipboard::to_wire(&p.items),
                    transfer: None,
                },
                Why::Other,
            );
        } else {
            let dir = self.plan.profile.drop_dir().join(CLIPBOARD_DIR);
            self.files.spill_count += 1;
            let name = format!("host-{}.json", self.files.spill_count); // not user text
            let path = dir.join(&name);
            let written = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .and_then(|()| {
                    let json = serde_json::to_vec(&clipboard::to_wire(&p.items))?;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .mode(0o600)
                        .open(&path)
                        .and_then(|mut f| std::io::Write::write_all(&mut f, &json))
                });
            if let Err(e) = written {
                say(&text::io_failed(
                    "cannot write the clipboard file",
                    &e.to_string(),
                ));
                return;
            }
            self.files.spills.push_back(path);
            let guest = Path::new(SHARE_ROOT)
                .join("drop")
                .join(CLIPBOARD_DIR)
                .join(name);
            self.send(
                Request::ClipboardSet {
                    count,
                    items: Vec::new(),
                    transfer: Some(guest),
                },
                Why::ClipboardSpill,
            );
        }
        self.events.record(
            "clipboard",
            "the host clipboard is in the guest",
            &[("bytes", bytes as f64)],
        );
    }

    /// The reply to a `clipboard.set` with a spill file: the file goes.
    pub(super) fn spill_done(&mut self) {
        if let Some(p) = self.files.spills.pop_front() {
            let _ = std::fs::remove_file(p);
        }
    }

    /// A `clipboard` event of the guest: inline, or by the name of its
    /// transfer, which may arrive before or after the event.
    pub(super) fn guest_clipboard(
        &mut self,
        count: u64,
        items: Vec<PasteItem>,
        transfer: Option<String>,
    ) {
        match transfer {
            None => {
                let actions = self
                    .files
                    .sync
                    .on_guest_change(count, clipboard::from_wire(&items));
                self.act_clipboard(actions);
            }
            Some(name) => match self.files.received.remove(&name) {
                Some(path) => self.guest_clipboard_file(count, &path),
                None => self.files.pending = Some((count, name)),
            },
        }
    }

    fn guest_clipboard_file(&mut self, count: u64, path: &Path) {
        let items = std::fs::metadata(path)
            .ok()
            .filter(|m| m.len() <= TRANSFER_MAX * 3 / 2 + 1024)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|b| serde_json::from_slice::<Vec<PasteItem>>(&b).ok());
        let _ = std::fs::remove_file(path);
        match items {
            Some(wire) => {
                let actions = self
                    .files
                    .sync
                    .on_guest_change(count, clipboard::from_wire(&wire));
                self.act_clipboard(actions);
            }
            None => log::warn!(
                "macvm: the clipboard transfer {} is not readable",
                path.display()
            ),
        }
    }

    /// A `guest-files` transfer landed.
    pub(super) fn guest_file(&mut self, kind: FileKind, name: String, path: PathBuf) {
        match kind {
            FileKind::Clipboard => match self.files.pending.take() {
                Some((count, pending)) if pending == name => {
                    self.guest_clipboard_file(count, &path)
                }
                other => {
                    self.files.pending = other;
                    self.files.received.insert(name, path);
                }
            },
            FileKind::Log => {
                self.log_wanted = false;
                let shown = match super::logs::place_agent_log(&self.plan.profile, &path) {
                    Ok(to) => to,
                    Err(e) => {
                        log::warn!("macvm: cannot place the agent log: {e}");
                        path
                    }
                };
                self.events.record(
                    "files",
                    &format!("agent log {name} received at {}", shown.display()), // not user text
                    &[],
                );
            }
            FileKind::Crash | FileKind::Drop => {
                self.events.record(
                    "files",
                    &format!("{kind:?} {name} received at {}", path.display()), // not user text
                    &[],
                );
            }
        }
    }

    /// A drop on a session window: stage it, then `drop` to the guest.
    pub(super) fn host_drop(&mut self, ev: DropEvent) {
        match drop::stage(&self.plan.profile, &ev.items, now_secs()) {
            Ok(staged) => {
                self.events.record(
                    "drop",
                    &text::drop_staged(staged.guest_paths.len(), &staged.dir),
                    &[("bytes", staged.bytes as f64)],
                );
                let sent = self.send(
                    Request::Drop {
                        window: ev.window,
                        point: ev.point,
                        paths: staged.guest_paths,
                    },
                    Why::Drop,
                );
                if !sent {
                    say(&text::drop_failed());
                }
            }
            Err(e) => {
                say(&e.badge());
                let line = match &e {
                    StageError::Io(err) => text::io_failed(&e.badge(), &err.to_string()),
                    _ => e.badge(),
                };
                self.events.record("drop", &line, &[]);
            }
        }
    }

    /// At the start of the quit: the files of `drop/` that are open in
    /// this process, which serves the shares to the guest.
    pub(super) fn note_open_drops(&mut self) {
        let dir = self.plan.profile.drop_dir();
        let out = std::process::Command::new("/usr/sbin/lsof")
            .args(["-Fn", "-p", &std::process::id().to_string(), "+D"])
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .output();
        self.files.open_at_quit = match out {
            Ok(o) => drop::open_paths(&String::from_utf8_lossy(&o.stdout)),
            Err(e) => {
                log::debug!("macvm: lsof: {e}");
                Vec::new()
            }
        };
    }

    /// At the exit: the staging folders go, except one with an open file.
    pub(super) fn sweep_drops(&mut self) {
        let open = std::mem::take(&mut self.files.open_at_quit);
        let removed = drop::sweep(&self.plan.profile, &open);
        if !removed.is_empty() {
            self.events.record(
                "drop",
                "the staging folders of the drops are removed",
                &[("count", removed.len() as f64)],
            );
        }
    }
}
