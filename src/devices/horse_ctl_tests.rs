//! `self/ctl` of the horse tree through the FUSE protocol, with the kernel
//! side on a socket pair (horse_run_tests.rs): no `/dev/fuse`, no mount.
use super::horse_run_tests::{serve, Kernel, OPEN};
use super::*;
use crate::broker::{Answer, Approver, Pending};
use crate::horse::device::ctl::{grant, tests as ctl_tests};
use std::sync::atomic::{AtomicUsize, Ordering};

const WRITE: u32 = 16;

struct Fixed {
    answer: Option<Answer>,
    asked: AtomicUsize,
}

impl Approver for Fixed {
    fn ask(&self, _: &HostEnv, _: &Pending, _: std::time::Duration) -> Option<Answer> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer
    }
}

/// Open `path` for writing and write `data` in one request: the errno.
fn write(k: &mut Kernel, path: &str, data: &[u8]) -> i32 {
    let node = k.walk(path).unwrap();
    let mut open = Vec::new();
    open.extend((libc::O_WRONLY as u32).to_ne_bytes());
    open.extend(0u32.to_ne_bytes());
    let (e, out) = k.send(OPEN, node, &open);
    assert_eq!(e, 0, "open of {path}");
    let fh = u64::from_ne_bytes(out[..8].try_into().unwrap());
    let mut body = Vec::new();
    body.extend(fh.to_ne_bytes());
    body.extend(0u64.to_ne_bytes());
    body.extend((data.len() as u32).to_ne_bytes());
    body.extend([0u8; 20]);
    body.extend(data);
    match k.send(WRITE, node, &body) {
        (0, out) => {
            assert_eq!(
                u32::from_ne_bytes(out[..4].try_into().unwrap()),
                data.len() as u32
            );
            0
        }
        (e, _) => -e,
    }
}

fn rows(env: &HostEnv) -> Vec<String> {
    let c = rusqlite::Connection::open(ledger::path(env)).unwrap();
    let mut s = c
        .prepare("SELECT status FROM requests WHERE device = 'horse' ORDER BY id")
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn the_ctl_file_runs_granted_commands_and_answers_with_errnos() {
    use crate::horse as hs;
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().into(),
        "demo".into(),
        unsafe { libc::getuid() },
        unsafe { libc::getgid() },
        Default::default(),
    );
    let store = hs::Store::open(&tmp.path().join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let sid = ctl_tests::ingest(&store, &tmp.path().join("sessions"), "session-f", &h.id);
    let fixed = Arc::new(Fixed {
        answer: Some(Answer::Deny),
        asked: AtomicUsize::new(0),
    });
    let ctx = DeviceCtx {
        env: env.clone(),
        sandbox: "sbx-fuse".into(),
        harness: "dsh".into(),
        project: tmp.path().into(),
        started_by: vec![],
        approver: fixed.clone(),
        instances: crate::devices::sandbox::Instances::new(env.clone()),
    };
    let setup = hs::device::Access {
        store: store.clone(),
        horse: h.id.clone(),
        session: sid.clone(),
    };
    let fs = DeviceFs::new("sbx-fuse", vec![]).with_horse(Some(setup), ctx);
    let (mut k, _session) = serve(fs);
    let ctl = "horse/self/ctl";
    let fork = format!("fork {sid}\n");

    // No grant: EACCES and one ledger row.
    assert_eq!(write(&mut k, ctl, fork.as_bytes()), libc::EACCES);
    assert_eq!(rows(&env), ["ctl fork refused no grant"]);
    let answer = String::from_utf8(k.read(ctl).unwrap()).unwrap();
    assert!(answer.contains("no ctl grant"), "{answer}");

    grant::set(
        &store,
        &grant::Grant {
            sandbox: "sbx-fuse".into(),
            commands: vec!["fork".into(), "append".into()],
            horses: Vec::new(),
            budget: 3,
            tokens: 1_000_000,
            used: 0,
            spent_tokens: 0,
            granted_ms: 1,
        },
    )
    .unwrap();
    // A fork at the head: no card, the new id is the answer.
    assert_eq!(write(&mut k, ctl, fork.as_bytes()), 0);
    let id = String::from_utf8(k.read(ctl).unwrap()).unwrap();
    let forked = store.session(id.trim()).unwrap();
    assert_eq!(forked.horse, h.id);
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 0);
    // A cache-breaking fork asks; the denial is EPERM and writes nothing.
    let mid = format!("fork {sid} {}\n", ctl_tests::MID);
    assert_eq!(write(&mut k, ctl, mid.as_bytes()), libc::EPERM);
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 1);
    assert_eq!(store.horse(&h.id).unwrap().sessions.len(), 2);
    // A bad command is EINVAL; a command that is not granted is EACCES.
    assert_eq!(write(&mut k, ctl, b"rewrite everything\n"), libc::EINVAL);
    assert_eq!(
        write(&mut k, ctl, format!("view {sid} v all").as_bytes()),
        libc::EACCES
    );
    // The reads of self/ctl have their own read rows.
    let ctl_rows: Vec<String> = rows(&env)
        .into_iter()
        .filter(|r| r.starts_with("ctl"))
        .collect();
    assert_eq!(
        ctl_rows,
        [
            "ctl fork refused no grant".to_string(),
            format!("ctl fork {sid} passed"),
            format!("ctl fork {sid} denied"),
            "ctl invalid".to_string(),
            "ctl view refused not granted".to_string(),
        ]
    );
    assert!(k.walk("horse/self/views").is_ok());
}
