//! Small deterministic in-memory queue with a temporary approval store.
use super::*;

pub(super) struct Fixture {
    _tmp: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        super::notice::BEFORE_OUTCOME_HOOK.with(|hook| *hook.borrow_mut() = None);
        crate::broker::OUTCOME_TIME.with(|clock| clock.set(None));
        crate::broker::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = None);
    }
}

pub(super) fn fixture() -> (Fixture, HostEnv, Asker) {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    // The slash also disables card mail for this synthetic profile.
    let asker = Asker::new(
        env.clone(),
        "fake/fixture",
        "Fixture",
        tmp.path().into(),
        123,
    );
    crate::broker::OUTCOME_TIME.with(|clock| clock.set(Some(120)));
    crate::broker::PID_LIVENESS_HOOK
        .with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(true))));
    (Fixture { _tmp: tmp }, env, asker)
}

pub(super) fn add(a: &Asker, id: &str, now: u64) -> (Triple, Arc<Question>) {
    let t = Triple {
        app: "app".into(),
        host: format!("{id}.example"),
        port: 443,
        identity: None,
    };
    let q = Arc::new(Question {
        id: id.into(),
        who: Who::default(),
        path: None,
        asked_at: AtomicU64::new(now),
        count: AtomicU64::new(1),
        taken: AtomicBool::new(false),
        generation: AtomicU64::new(0),
        published: AtomicBool::new(false),
        accepted_file: AtomicBool::new(false),
        retirement_ready: AtomicBool::new(false),
        selection: Mutex::new(None),
        result: Mutex::new(None),
    });
    let mut g = a.lock();
    g.map.insert(t.clone(), q.clone());
    g.queue.add(t.clone());
    a.pump(&mut g, now, true);
    (t, q)
}

pub(super) fn notice(a: &Asker) {
    let mut g = a.lock();
    g.queue.force_pause();
    g.notice = Some("notice-fixture".into());
    g.notice_root = g.notice.clone();
    g.notice_created = 100;
    a.pump(&mut g, 100, true);
}

pub(super) fn card(env: &HostEnv, id: &str) -> Pending {
    serde_json::from_slice(
        &std::fs::read(broker::approvals_dir(env).join(format!("{id}.json"))).unwrap(),
    )
    .unwrap()
}

pub(super) fn notice_card(a: &Asker, env: &HostEnv) -> Pending {
    let id = a.lock().notice.clone().unwrap();
    card(env, &id)
}
