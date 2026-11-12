//! Bounded best-effort approval mailbox notifications.

use super::*;

// Mail is a best-effort historical notice, never evidence that a card still
// waits. Keep at most one latest follow-up per outstanding mailbox and
// never retry a busy mailbox.
// A small fixed pool isolates one stalled mailbox I/O operation from the
// others; a full queue or a pool of stalled filesystems can still lose notices.
const CARD_MAIL_QUEUE: usize = 64;
const CARD_MAIL_WORKERS: usize = 4;
const CARD_MAIL_MAX_CARD_BYTES: usize = 1024 * 1024;

struct CardMailJob {
    env: HostEnv,
    key: CardMailKey,
    mailbox: PathBuf,
    #[cfg(test)]
    hooks: Option<CardMailHooks>,
}

pub(super) struct CardMailKey {
    id: String,
    sandbox: String,
    text: String,
}

pub(super) fn card_mail_key(card: &Pending, json: &str) -> Option<CardMailKey> {
    if !crate::view::valid_id(&card.sandbox) {
        return None;
    }
    if json.len() > CARD_MAIL_MAX_CARD_BYTES {
        log::warn!("approval {} is too large for a mail notification", card.id);
        return None;
    }
    Some(CardMailKey {
        id: card.id.clone(),
        sandbox: card.sandbox.clone(),
        text: crate::mail::events::card_opened(card),
    })
}

#[derive(Default)]
struct CardMailState {
    ready: std::collections::VecDeque<CardMailJob>,
    // One latest follow-up per queued or running mailbox.
    outstanding: HashMap<PathBuf, Option<CardMailJob>>,
}

struct CardMailQueue {
    state: Mutex<CardMailState>,
    ready: std::sync::Condvar,
    limit: usize,
}

impl CardMailQueue {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(CardMailState::default()),
            ready: std::sync::Condvar::new(),
            limit,
        }
    }

    // The bound counts running mailboxes too, reserving room for their
    // follow-ups. A producer never waits for mailbox I/O.
    fn enqueue(&self, job: CardMailJob) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(follow_up) = state.outstanding.get_mut(&job.mailbox) {
            *follow_up = Some(job);
            return true;
        }
        if state.outstanding.len() >= self.limit {
            log::warn!("approval {} mail notification queue is full", job.key.id);
            return false;
        }
        state.outstanding.insert(job.mailbox.clone(), None);
        state.ready.push_back(job);
        self.ready.notify_one();
        true
    }

    fn next(&self) -> CardMailJob {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(job) = state.ready.pop_front() {
                return job;
            }
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    // Requeue at the tail after exactly one write. In particular four
    // continuously dirty mailboxes cannot bypass an already queued fifth.
    fn finish(&self, mailbox: &Path) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Some(job)) = state.outstanding.get_mut(mailbox).map(Option::take) {
            state.ready.push_back(job);
            self.ready.notify_one();
        } else {
            state.outstanding.remove(mailbox);
        }
    }
}

// One worker turn. Always finish the mailbox, including on an I/O error.
// The injected operation lets tests exercise the same scheduling path with
// fake mailboxes instead of user state.
fn work_card_mail_once(
    queue: &CardMailQueue,
    deliver: impl FnOnce(&mut CardMailJob) -> io::Result<bool>,
) -> (String, io::Result<bool>) {
    let mut job = queue.next();
    let result = deliver(&mut job);
    queue.finish(&job.mailbox);
    (job.key.id, result)
}

fn card_mail_sender() -> Option<&'static Arc<CardMailQueue>> {
    static SENDER: OnceLock<Option<Arc<CardMailQueue>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let queue = Arc::new(CardMailQueue::new(CARD_MAIL_QUEUE));
            let mut started = 0;
            for index in 0..CARD_MAIL_WORKERS {
                let queue = queue.clone();
                match std::thread::Builder::new()
                    .name(format!("approval-mail-{index}"))
                    .spawn(move || loop {
                        #[cfg(test)]
                        let mut after = None;
                        let (id, result) = work_card_mail_once(&queue, |job| {
                            #[cfg(test)]
                            {
                                after = job.hooks.take().map(|(before, after)| {
                                    before();
                                    after
                                });
                            }
                            try_card_mail_key(&job.env, &job.key)
                        });
                        match result {
                            Ok(true) => {}
                            Ok(false) => {
                                log::warn!("approval {id} mail notification skipped: mailbox busy")
                            }
                            Err(e) => log::warn!("approval {id} mail notification failed: {e}"),
                        }
                        #[cfg(test)]
                        if let Some(after) = after {
                            after();
                        }
                    }) {
                    Ok(_) => started += 1,
                    Err(e) => log::warn!("cannot start approval mail worker: {e}"),
                }
            }
            (started != 0).then_some(queue)
        })
        .as_ref()
}

pub(super) fn notify_card(env: &HostEnv, key: Option<CardMailKey>) {
    let Some(key) = key else { return };
    let Some(queue) = card_mail_sender() else {
        return;
    };
    let Ok(mailbox) = crate::mail::dir(env, &key.sandbox) else {
        return;
    };
    let mut mail_env = env.clone();
    mail_env.vars.clear();
    mail_env.path.clear();
    let job = CardMailJob {
        env: mail_env,
        key,
        mailbox,
        #[cfg(test)]
        hooks: CARD_MAIL_HOOK.with(|hook| hook.borrow_mut().take()),
    };
    queue.enqueue(job);
}

// This uses only the immutable publication snapshot. It deliberately takes
// no approval-store lock while scanning, writing or syncing the mailbox.
fn try_card_mail_key(env: &HostEnv, key: &CardMailKey) -> io::Result<bool> {
    crate::mail::events::try_card(env, &key.sandbox, key.text.clone())
        .map_err(|e| io::Error::other(e.to_string()))
}

#[cfg(test)]
pub(in super::super) fn try_card_mail(env: &HostEnv, card: &Pending) -> io::Result<bool> {
    let json = serde_json::to_string(card).map_err(io::Error::other)?;
    match card_mail_key(card, &json) {
        Some(key) => try_card_mail_key(env, &key),
        None => Ok(true),
    }
}

#[cfg(test)]
mod card_mail_queue_tests {
    use super::*;

    fn job(env: &HostEnv, sandbox: &str, id: &str) -> CardMailJob {
        CardMailJob {
            env: env.clone(),
            key: CardMailKey {
                id: id.into(),
                sandbox: sandbox.into(),
                text: "opened".into(),
            },
            mailbox: crate::mail::dir(env, sandbox).unwrap(),
            hooks: None,
        }
    }

    #[test]
    fn four_dirty_workers_requeue_behind_an_already_waiting_mailbox() {
        let tmp = tempfile::tempdir().unwrap();
        let env = crate::privfs::tests::env_in(tmp.path());
        let queue = Arc::new(CardMailQueue::new(5));
        for name in ["a", "b", "c", "d"] {
            assert!(queue.enqueue(job(&env, name, "r-first")));
        }
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let queue = queue.clone();
                let entered = entered_tx.clone();
                let release = release_rx.clone();
                std::thread::spawn(move || {
                    work_card_mail_once(&queue, |job| {
                        entered.send(job.key.sandbox.clone()).unwrap();
                        release.lock().unwrap().recv().unwrap();
                        Err(io::Error::other("synthetic mail failure"))
                    })
                })
            })
            .collect();
        let running: Vec<_> = (0..4).map(|_| entered_rx.recv().unwrap()).collect();
        assert!(queue.enqueue(job(&env, "healthy", "r-healthy")));
        for (i, name) in running.iter().enumerate() {
            for revision in 0..3 {
                assert!(queue.enqueue(job(&env, name, &format!("r-{i}-{revision}"))));
            }
        }
        for _ in 0..4 {
            release_tx.send(()).unwrap();
        }
        for worker in workers {
            assert!(worker.join().unwrap().1.is_err());
        }
        let (first, result) = work_card_mail_once(&queue, |job| {
            assert_eq!(job.key.sandbox, "healthy");
            Ok(true)
        });
        assert_eq!(first, "r-healthy");
        assert!(result.unwrap());
        let mut follow_ups = Vec::new();
        for _ in 0..4 {
            let (id, result) = work_card_mail_once(&queue, |job| {
                follow_ups.push(job.key.sandbox.clone());
                Ok(false)
            });
            assert!(id.ends_with("-2"));
            assert!(!result.unwrap());
        }
        follow_ups.sort();
        assert_eq!(follow_ups, ["a", "b", "c", "d"]);
        assert!(queue.state.lock().unwrap().outstanding.is_empty());
    }

    #[test]
    fn full_queue_keeps_dirty_follow_up_but_rejects_new_mailbox() {
        let tmp = tempfile::tempdir().unwrap();
        let env = crate::privfs::tests::env_in(tmp.path());
        let queue = CardMailQueue::new(2);
        assert!(queue.enqueue(job(&env, "first", "r-first")));
        let first = queue.next();
        assert!(queue.enqueue(job(&env, "queued", "r-queued")));
        assert!(queue.enqueue(job(&env, "first", "r-later")));
        assert!(!queue.enqueue(job(&env, "overflow", "r-overflow")));
        queue.finish(&first.mailbox);
        let queued = queue.next();
        assert_eq!(queued.key.id, "r-queued");
        queue.finish(&queued.mailbox);
        let later = queue.next();
        assert_eq!(later.key.id, "r-later");
        queue.finish(&later.mailbox);
        assert!(queue.enqueue(job(&env, "overflow", "r-now")));
    }
}
