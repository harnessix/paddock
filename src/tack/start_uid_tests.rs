//! tackd with the real starter at both levels, as far as a test without
//! root can go: a start at `uid` without a helper is refused with the fix,
//! and a start from a helper (the test user stands in for one) is decided
//! with the owner's configuration and runs with the helper's environment.

use super::*;
use crate::credentials::secret::Secret;
use crate::tack::audit::{Log, Record};
use crate::tack::config;
use crate::tack::daemon::{bind, Server, Users};
use crate::tack::procs::Fake;
use crate::tack::protect::Helper;
use crate::tack::proto::{Answer, Event, Request};
use crate::tack::start::{Linux, Values, Who};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

const OWNER: u32 = 4242;
const FAKE: &str = "fake-uid-value-77";

fn me() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { libc::geteuid() }
}

/// The configuration of `who`, for every uid.
struct For(u32, String);

impl Users for For {
    fn config(&self, uid: u32) -> Result<Tack, String> {
        if uid != self.0 {
            return Ok(Tack::default());
        }
        config::parse(&self.1).map_err(|e| format!("{e:#}"))
    }
}

/// Values for the store user that the test expects.
struct Expect(u32);

impl Values for Expect {
    fn fetch(
        &self,
        who: &Who,
        _: &Tack,
        names: &[String],
    ) -> Result<Vec<(String, Secret)>, String> {
        if who.caller.uid != self.0 {
            return Err(format!(
                "the store ran for uid {}, not {}",
                who.caller.uid, self.0
            ));
        }
        let path = who
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone());
        if path.as_deref().is_some_and(|p| p.contains("/wrap/")) {
            return Err("the store tools got the wrapper's PATH".into());
        }
        Ok(names
            .iter()
            .map(|n| (n.clone(), Secret::from(FAKE)))
            .collect())
    }
}

struct Run {
    t: tempfile::TempDir,
    sock: std::path::PathBuf,
    prog: std::path::PathBuf,
    _s: Arc<Server>,
}

fn serve(protect: &str, cfg_uid: u32, store_uid: u32, helper: Option<Helper>) -> Run {
    let t = tempfile::tempdir().unwrap();
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let prog = t.path().join("tool");
    std::fs::write(
        &prog,
        "#!/bin/sh\necho \"token=$GH_TOKEN home=$HOME wrap=${PADDOCK_TACK_WRAP:+yes} secrets=$PADDOCK_TACK_SECRETS git=$GIT_CONFIG_KEY_0\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = format!(
        "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{}\"\nprotect = \"{protect}\"\n",
        prog.display(),
        crate::tack::program::sha256_file(&prog).unwrap()
    );
    let mut table = Fake::default().add(1, 0, "/sbin/init", &[]).add(
        std::process::id(),
        1,
        "/usr/bin/paddock",
        &["paddock", "tack"],
    );
    table.0.get_mut(&std::process::id()).unwrap().uid = me();
    let audit = Log::new(t.path().join("log/tackd.log"), me());
    let mut server = Server::new(Box::new(table), Box::new(For(cfg_uid, cfg)), audit);
    let state = t.path().join("state");
    std::fs::create_dir(&state).unwrap();
    if let Some(h) = &helper {
        let mut r = Registry::default();
        r.put(h.clone());
        r.save(&state).unwrap();
        let owner = h.owner;
        let huid = h.uid;
        server.owner_of = Box::new(move |u| (u == huid).then_some(owner));
    }
    let mut l = Linux::new(Box::new(Expect(store_uid)));
    l.cache = server.cache.clone();
    l.cgroups = t.path().join("no-cgroups");
    l.protect.state = state;
    l.protect.owner = me();
    l.protect.check_accounts = false;
    server.starter = Some(Box::new(l));
    let server = Arc::new(server);
    let sock = t.path().join("run/tackd.sock");
    let lst = bind(&sock, me()).unwrap();
    let s = server.clone();
    std::thread::spawn(move || s.serve(lst));
    Run {
        t,
        sock,
        prog,
        _s: server,
    }
}

fn start(r: &Run) -> (Answer, String) {
    let (rd, wr) = std::os::unix::net::UnixStream::pair().unwrap();
    let null = std::fs::File::open("/dev/null").unwrap();
    let req = Start {
        program: r.prog.display().to_string(),
        argv0: "tool".into(),
        args: vec![],
        cwd: r.t.path().display().to_string(),
        caller_pid: std::process::id(),
        env: vec![
            "PATH=/usr/bin:/bin".into(),
            "GH_TOKEN=tack://GH_TOKEN".into(),
            "HOME=/home/demo".into(),
        ],
        stdio: [true, true, true],
    };
    let s = UnixStream::connect(&r.sock).unwrap();
    let mut body = Vec::new();
    crate::tack::proto::write_frame(
        &mut body,
        &crate::tack::proto::envelope(Request::Start(req)),
    )
    .unwrap();
    crate::tack::fdpass::send(
        &s,
        &body,
        &[null.as_raw_fd(), wr.as_raw_fd(), wr.as_raw_fd()],
    )
    .unwrap();
    drop(wr);
    let mut rr = &s;
    let a = crate::tack::proto::parse_answer(&crate::tack::proto::read_frame(&mut rr).unwrap())
        .unwrap();
    if matches!(a, Answer::Started { .. }) {
        loop {
            let b = crate::tack::proto::read_small_frame(&mut rr, 4096).unwrap();
            if matches!(
                crate::tack::proto::parse_after::<Event>(&b).unwrap(),
                Event::Exited { .. }
            ) {
                break;
            }
        }
    }
    let mut out = String::new();
    let _ = std::io::Read::read_to_string(&mut &rd, &mut out);
    (a, out)
}

fn audit(r: &Run) -> Vec<Record> {
    for _ in 0..300 {
        let v: Vec<Record> = Log::new(r.t.path().join("log/tackd.log"), me())
            .read(None, 100)
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if v.iter()
            .any(|x| x.decision == "ended" || x.decision == "failed")
        {
            return v;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("no ended or failed line");
}

#[test]
fn a_start_at_uid_without_a_helper_is_refused_with_the_fix() {
    if me() == 0 {
        eprintln!("SKIP: runs as root");
        return;
    }
    let r = serve("uid", me(), me(), None);
    let (a, out) = start(&r);
    let Answer::Failed { message } = &a else {
        panic!("{a:?}")
    };
    assert!(
        message.contains("helper account does not exist yet: run sudo paddock tack setup"),
        "{message}"
    );
    assert!(out.is_empty(), "nothing ran: {out}");
    let recs = audit(&r);
    assert_eq!(recs.last().unwrap().decision, "failed", "{recs:?}");
}

#[test]
fn a_start_from_a_helper_uses_the_owners_configuration_and_the_helpers_environment() {
    if me() == 0 {
        eprintln!("SKIP: runs as root");
        return;
    }
    let h = Helper {
        owner: OWNER,
        name: "_tack_test".into(),
        uid: me(),
        gid: 0,
        home: "/nonexistent/helper-home".into(),
    };
    // The entry says refs: from a helper it still runs at uid, as the helper.
    let r = serve(
        "refs",
        OWNER,
        OWNER,
        Some(Helper {
            gid: unsafe { libc::getegid() },
            ..h
        }),
    );
    let (a, out) = start(&r);
    let Answer::Started { protection, .. } = &a else {
        panic!("{a:?}")
    };
    assert!(
        protection.starts_with(&format!("protect = uid: runs as _tack_test (uid {})", me())),
        "{protection}"
    );
    assert_eq!(
        out.trim(),
        format!("token={FAKE} home=/nonexistent/helper-home wrap=yes secrets=GH_TOKEN git=safe.directory")
    );
    let recs = audit(&r);
    let started = recs.iter().find(|x| x.decision == "started").unwrap();
    assert_eq!(
        (started.uid, started.caller_uid),
        (OWNER, Some(me())),
        "{started:?}"
    );
    assert_eq!(
        (started.protect.as_deref(), started.run_uid),
        (Some("uid"), Some(me()))
    );
    assert!(
        started
            .reason
            .contains("started by a program at protect = uid"),
        "{started:?}"
    );
    assert!(!started.reason.contains(FAKE));
}
