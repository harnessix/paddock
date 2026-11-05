use super::*;
use crate::tack::client;
use crate::tack::procs::Fake;
use crate::tack::proto::Launch;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::FileTypeExt;
use std::sync::Mutex;

fn me() -> u32 {
    unsafe { libc::geteuid() }
}

/// Configurations by uid; records which uids were asked for.
#[derive(Default)]
struct FakeUsers {
    cfg: BTreeMap<u32, String>,
    asked: Arc<Mutex<Vec<u32>>>,
}

impl Users for FakeUsers {
    fn config(&self, uid: u32) -> Result<Tack, String> {
        self.asked.lock().unwrap().push(uid);
        match self.cfg.get(&uid) {
            Some(t) => config::parse(t).map_err(|e| format!("{e:#}")),
            None => Ok(Tack::default()),
        }
    }
}

struct Running {
    t: tempfile::TempDir,
    sock: PathBuf,
    server: Arc<Server>,
    gh: PathBuf,
}

fn table() -> Fake {
    let mut t = Fake::default().add(1, 0, "/sbin/init", &[]).add(
        std::process::id(),
        1,
        "/usr/bin/paddock",
        &["paddock"],
    );
    t.0.get_mut(&std::process::id()).unwrap().uid = me();
    t
}

fn start_with(users: impl FnOnce(&Path) -> FakeUsers, tweak: impl FnOnce(&mut Server)) -> Running {
    let t = tempfile::tempdir().unwrap();
    let gh = t.path().join("gh");
    std::fs::write(&gh, "#!/fake gh\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let users = users(&gh);
    let audit = Log::new(t.path().join("log/tackd.log"), me());
    let mut server = Server::new(Box::new(table()), Box::new(users), audit);
    tweak(&mut server);
    let server = Arc::new(server);
    let sock = t.path().join("run/tackd.sock");
    let l = bind(&sock, me()).unwrap();
    let s = server.clone();
    std::thread::spawn(move || s.serve(l));
    Running {
        t,
        sock,
        server,
        gh,
    }
}

fn gh_cfg(gh: &Path) -> String {
    format!(
        "[tack.secrets.GH_TOKEN]\nhosts = [\"api.github.com\"]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{}\"\n",
        gh.display(),
        crate::tack::program::sha256_file(gh).unwrap()
    )
}

fn start() -> Running {
    start_with(
        |gh| FakeUsers {
            cfg: [(me(), gh_cfg(gh))].into(),
            ..Default::default()
        },
        |_| {},
    )
}

fn launch(r: &Running, args: &[&str]) -> Request {
    Request::Launch(Launch {
        program: r.gh.display().to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: "/".into(),
        caller_pid: std::process::id(),
    })
}

fn decision(a: Answer) -> Decision {
    match a {
        Answer::Decision(d) => d,
        other => panic!("not a decision: {other:?}"),
    }
}

const T: Duration = Duration::from_secs(10);

#[test]
fn a_launch_is_decided_audited_and_counted() {
    let r = start();
    let d = decision(client::ask(&r.sock, launch(&r, &["pr", "list"]), T).unwrap());
    assert_eq!(d.verdict, Verdict::Allow, "{}", d.reason);
    assert_eq!(d.secrets, ["GH_TOKEN"]);
    let lines = match client::ask(&r.sock, Request::Audit { lines: 10 }, T).unwrap() {
        Answer::Audit { lines } => lines,
        a => panic!("{a:?}"),
    };
    assert_eq!(lines.len(), 1);
    let rec: Record = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!((rec.uid, rec.pid), (me(), std::process::id()));
    assert_eq!(rec.decision, "allow");
    assert_eq!(rec.secrets, ["GH_TOKEN"]);
    // The arguments are not on record.
    assert!(!lines[0].contains("\"list\""), "{}", lines[0]);
    let st = match client::ask(&r.sock, Request::Status {}, T).unwrap() {
        Answer::Status(s) => s,
        a => panic!("{a:?}"),
    };
    assert_eq!(st.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        (st.requests, st.allowed, st.denied, st.errors),
        (3, 1, 0, 0)
    );
    assert!(st.audit_error.is_none());
}

#[test]
fn the_socket_is_open_to_all_users_in_a_closed_folder() {
    let r = start();
    let m = std::fs::symlink_metadata(&r.sock).unwrap();
    assert!(m.file_type().is_socket());
    assert_eq!(m.mode() & 0o777, 0o666);
    let d = std::fs::metadata(r.sock.parent().unwrap()).unwrap();
    assert_eq!(d.mode() & 0o777, 0o755);
}

#[test]
fn only_the_peers_own_configuration_is_read() {
    // uid+1 has a configuration that would allow; it is never looked at.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let a2 = asked.clone();
    let r = start_with(
        move |gh| FakeUsers {
            cfg: [(me() + 1, gh_cfg(gh))].into(),
            asked: a2,
        },
        |_| {},
    );
    let d = decision(client::ask(&r.sock, launch(&r, &[]), T).unwrap());
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("not on the allowlist"), "{}", d.reason);
    assert_eq!(*asked.lock().unwrap(), [me()]);
}

#[test]
fn a_forged_caller_pid_is_denied_and_on_record() {
    let r = start();
    let req = Request::Launch(Launch {
        caller_pid: 1,
        ..match launch(&r, &[]) {
            Request::Launch(l) => l,
            _ => unreachable!(),
        }
    });
    let d = decision(client::ask(&r.sock, req, T).unwrap());
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("kernel reports"), "{}", d.reason);
    assert_eq!(r.server.audit.read(None, 10).len(), 1);
}

#[test]
fn an_audit_log_that_cannot_be_written_turns_allow_into_deny() {
    let r = start_with(
        |gh| FakeUsers {
            cfg: [(me(), gh_cfg(gh))].into(),
            ..Default::default()
        },
        |s| {
            let dir = s.audit.path().to_path_buf();
            // The log path is a folder: no record can be written.
            std::fs::create_dir_all(&dir).unwrap();
        },
    );
    let d = decision(client::ask(&r.sock, launch(&r, &[]), T).unwrap());
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("cannot write its audit log"),
        "{}",
        d.reason
    );
    assert!(d.secrets.is_empty() && d.hosts.is_empty());
    let st = r.server.status();
    assert!(st.audit_error.is_some());
}

fn raw(sock: &Path, bytes: &[u8]) -> Answer {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(T)).unwrap();
    s.write_all(bytes).unwrap();
    let body = proto::read_frame(&mut s).unwrap();
    proto::parse_answer(&body).unwrap()
}

#[test]
fn hostile_frames_get_an_error_and_tackd_keeps_serving() {
    let r = start();
    let mut garbage = 9u32.to_be_bytes().to_vec();
    garbage.extend_from_slice(b"not json!");
    let mut huge = u32::MAX.to_be_bytes().to_vec();
    huge.extend_from_slice(b"x");
    let mut wrong_version = Vec::new();
    proto::write_frame(
        &mut wrong_version,
        &serde_json::json!({"v": 7, "type": "status"}),
    )
    .unwrap();
    for (bytes, want) in [
        (garbage, "not JSON"),
        (huge, "limit"),
        (wrong_version, "version 7"),
    ] {
        match raw(&r.sock, &bytes) {
            Answer::Error { message, protocols } => {
                assert!(message.contains(want), "{message}");
                assert_eq!(protocols, [proto::VERSION]);
            }
            a => panic!("{a:?}"),
        }
    }
    // A client that hangs up at once does no harm either.
    drop(UnixStream::connect(&r.sock).unwrap());
    let d = decision(client::ask(&r.sock, launch(&r, &[]), T).unwrap());
    assert_eq!(d.verdict, Verdict::Allow);
    assert_eq!(r.server.status().errors, 4);
}

#[test]
fn a_slow_client_is_cut_off_and_does_not_block_others() {
    let r = start_with(
        |gh| FakeUsers {
            cfg: [(me(), gh_cfg(gh))].into(),
            ..Default::default()
        },
        |s| s.deadline = Duration::from_millis(300),
    );
    let mut slow = UnixStream::connect(&r.sock).unwrap();
    slow.set_read_timeout(Some(T)).unwrap();
    slow.write_all(&[0, 0]).unwrap();
    // Meanwhile another client is served.
    let d = decision(client::ask(&r.sock, launch(&r, &[]), T).unwrap());
    assert_eq!(d.verdict, Verdict::Allow);
    // One byte at a time does not stretch the deadline.
    let start = Instant::now();
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(80));
        let _ = slow.write_all(&[1]);
    }
    let body = proto::read_frame(&mut slow).unwrap();
    match proto::parse_answer(&body).unwrap() {
        Answer::Error { message, .. } => assert!(message.contains("did not arrive"), "{message}"),
        a => panic!("{a:?}"),
    }
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn over_the_connection_limit_a_client_hears_busy() {
    let r = start_with(
        |_| FakeUsers::default(),
        |s| {
            s.max_conns = 2;
            s.deadline = Duration::from_secs(3);
        },
    );
    let a = UnixStream::connect(&r.sock).unwrap();
    let b = UnixStream::connect(&r.sock).unwrap();
    // Give the server the time to take both.
    std::thread::sleep(Duration::from_millis(200));
    let e = client::ask(&r.sock, Request::Status {}, T)
        .unwrap_err()
        .to_string();
    assert!(e.contains("busy"), "{e}");
    drop((a, b));
    // The slots come back once those connections end.
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match client::ask(&r.sock, Request::Status {}, T) {
            Ok(_) => break,
            Err(e) if Instant::now() < end => {
                assert!(e.to_string().contains("busy"), "{e}");
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn one_user_cannot_take_every_slot() {
    let r = start_with(
        |_| FakeUsers::default(),
        |s| {
            s.max_per_uid = 2;
            s.deadline = Duration::from_secs(3);
        },
    );
    let a = UnixStream::connect(&r.sock).unwrap();
    let b = UnixStream::connect(&r.sock).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let e = client::ask(&r.sock, Request::Status {}, T)
        .unwrap_err()
        .to_string();
    assert!(e.contains("requests of your user"), "{e}");
    drop((a, b));
    let end = Instant::now() + Duration::from_secs(5);
    while client::ask(&r.sock, Request::Status {}, T).is_err() {
        assert!(
            Instant::now() < end,
            "the slots of the uid did not come back"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn many_clients_at_once_get_their_own_answers() {
    let r = Arc::new(start_with(
        |gh| FakeUsers {
            cfg: [(me(), gh_cfg(gh))].into(),
            ..Default::default()
        },
        |s| s.max_per_uid = MAX_CONNS,
    ));
    let hs: Vec<_> = (0..12)
        .map(|i| {
            let r = r.clone();
            std::thread::spawn(move || {
                for _ in 0..5 {
                    let args = if i % 2 == 0 { vec!["pr"] } else { vec![] };
                    let d = decision(client::ask(&r.sock, launch(&r, &args), T).unwrap());
                    assert_eq!(d.verdict, Verdict::Allow);
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(r.server.audit.read(None, 1000).len(), 60);
    assert_eq!(r.server.status().allowed, 60);
    let _ = &r.t;
}

#[test]
fn a_stale_socket_is_replaced_and_a_live_one_is_not() {
    let t = tempfile::tempdir().unwrap();
    let sock = t.path().join("run/tackd.sock");
    let l = bind(&sock, me()).unwrap();
    let e = bind(&sock, me()).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AddrInUse, "{e}");
    // tackd crashed: the socket file stays, nobody answers.
    drop(l);
    let _l = bind(&sock, me()).unwrap();
    // A plain file in its place is not removed.
    let other = t.path().join("run/other.sock");
    std::fs::write(&other, "x").unwrap();
    assert!(bind(&other, me())
        .unwrap_err()
        .to_string()
        .contains("not a socket"));
    // A folder of another owner is refused.
    assert!(bind(&t.path().join("x/s.sock"), me() + 1).is_err());
}

#[test]
fn the_client_names_a_missing_or_dead_tackd() {
    let t = tempfile::tempdir().unwrap();
    let sock = t.path().join("tackd.sock");
    let e = client::ask(&sock, Request::Status {}, T).unwrap_err();
    assert!(matches!(e, client::Error::NoSocket), "{e}");
    drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
    let e = client::ask(&sock, Request::Status {}, T).unwrap_err();
    assert!(matches!(e, client::Error::NotRunning), "{e}");
    assert!(e.to_string().contains("stopped or crashed"));
}

#[test]
fn a_config_file_is_read_only_when_the_user_owns_it() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("config.toml");
    assert_eq!(read_config(&f, me()).unwrap(), Tack::default());
    std::fs::write(&f, "[tack.secrets.A]\n").unwrap();
    assert_eq!(read_config(&f, me()).unwrap().secrets.len(), 1);
    let e = read_config(&f, me() + 1).unwrap_err();
    assert!(e.contains("not to you"), "{e}");
    // A link to a file that the user does not own is refused before a
    // byte is read, so no line of it shows in an error.
    let foreign = t.path().join("foreign");
    std::fs::write(&foreign, "SECRET-LINE = [\n").unwrap();
    let link = t.path().join("link.toml");
    std::os::unix::fs::symlink(&foreign, &link).unwrap();
    let e = read_config(&link, me() + 1).unwrap_err();
    assert!(!e.contains("SECRET-LINE"), "{e}");
    std::fs::write(&f, "[tack.secrets.A]\nbackend = 3\n").unwrap();
    assert!(read_config(&f, me()).unwrap_err().contains("is invalid"));
    std::fs::write(&f, vec![b'#'; (MAX_CONFIG + 1) as usize]).unwrap();
    assert!(read_config(&f, me()).unwrap_err().contains("larger"));
    assert!(read_config(t.path(), me()).is_err());
}

#[test]
fn home_folders_come_from_the_passwd_database() {
    assert_eq!(home_of(u32::MAX - 7), None);
    if let Some(h) = home_of(me()) {
        assert!(h.is_absolute());
        assert_eq!(config_file(&h), h.join(".config/paddock/config.toml"));
    }
}

#[test]
fn main_refuses_to_run_as_a_user() {
    if me() == 0 {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let e = main(&crate::tack::service::Paths::under(t.path()))
        .unwrap_err()
        .to_string();
    assert!(e.contains("runs as root"), "{e}");
}
