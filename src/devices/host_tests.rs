//! The marks of a host device that need no supervisor: the catalog text
//! and `run` field, the card, the gate that never takes `mode = "allow"`,
//! the source choice and the command line of the supervisor. The
//! supervisor itself, as the paddock binary on the host, is
//! tests/devices_host.rs.

use super::*;
use crate::broker::{Answer, Approver, Pending};
use crate::devices::config::Mode;
use crate::devices::grant::GrantHit;
use crate::devices::user::tests::ctx;
use crate::devices::user::Gate;
use crate::devices::{catalog, Call, DevicesCfg};
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;

fn host_def(extra: &str) -> DeviceDef {
    DevicesCfg::parse(&format!(
        "[devices.notes]\nshape = \"exec\"\nrun = \"host\"\nenabled = true\ncommand = [\"notes-search\", \"-q\"]\n{extra}"
    ))
    .unwrap()
    .user
    .remove(0)
}

/// Records every card and answers with one answer.
struct Recording(Answer, Mutex<Vec<Pending>>);

impl Approver for Recording {
    fn ask(&self, _: &HostEnv, p: &Pending, _: Duration) -> Option<Answer> {
        self.1.lock().unwrap().push(p.clone());
        Some(self.0)
    }
}

#[test]
fn the_catalog_text_and_the_run_field_mark_a_host_device() {
    let exec = host_def("cwd = \"/srv/notes\"\n");
    let i = info(&exec);
    assert!(i.request.starts_with(HOST_MARK), "{}", i.request);
    assert!(
        i.request
            .contains("`notes-search -q`, which runs in /srv/notes"),
        "{}",
        i.request
    );
    assert_eq!(i.shape, "exec");
    let serve = DevicesCfg::parse(
        "[devices.gui]\nshape = \"serve\"\nrun = \"host\"\nenabled = true\ncommand = [\"gui-tool\"]\n",
    )
    .unwrap()
    .user
    .remove(0);
    let i = info(&serve);
    assert!(i.request.starts_with(HOST_MARK), "{}", i.request);
    assert!(
        i.request.contains("the service `gui-tool`"),
        "{}",
        i.request
    );
    assert_eq!(i.shape, "serve");
    // Through the catalog: `run` is host, the instance is stopped until
    // the first request, and the text starts with the mark.
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(dir.path(), Answer::Session);
    let dev = exec.build(&ctx, Ok(GrantHit::NoKey));
    let v: serde_json::Value =
        serde_json::from_slice(&catalog(std::slice::from_ref(&dev))).unwrap();
    let e = &v["devices"][0];
    assert_eq!(e["name"], "notes");
    assert_eq!(e["run"], "host");
    assert_eq!(e["state"], "on");
    assert_eq!(e["instance"], "stopped");
    assert_eq!(e["instance_name"], "host");
    assert!(e["request"].as_str().unwrap().starts_with(HOST_MARK), "{e}");
    assert!(
        e["how_to_fix"].is_null(),
        "a host device is served, not not-yet: {e}"
    );
    assert_eq!(
        super::super::user::info(&exec).request,
        info(&exec).request,
        "the shared info picks the host text"
    );
}

#[test]
fn the_card_says_on_the_host_and_the_gate_never_takes_allow() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ctx, _) = ctx(dir.path(), Answer::Session);
    let rec = Arc::new(Recording(Answer::Session, Mutex::new(Vec::new())));
    ctx.approver = rec.clone();
    let call = Call::for_test("box1", Duration::from_secs(30));
    // A session answer of another device exists.
    let other =
        DevicesCfg::parse("[devices.jq]\nshape = \"exec\"\nenabled = true\ncommand = [\"jq\"]\n")
            .unwrap()
            .user
            .remove(0);
    let g = Gate::new(ctx.clone(), &other, GrantHit::NoKey);
    assert_eq!(g.check(&call).unwrap(), "allowed for the session");
    assert_eq!(g.check(&call).unwrap(), "allowed for the session");
    assert_eq!(rec.1.lock().unwrap().len(), 1, "one card for jq");
    // The host device still shows its own card, with the mark.
    let d = host_def("cwd = \"/srv/notes\"\n");
    let g = Gate::new(ctx.clone(), &d, GrantHit::NoKey);
    assert_eq!(g.check(&call).unwrap(), "allowed for the session");
    let cards = rec.1.lock().unwrap();
    assert_eq!(cards.len(), 2);
    let card = &cards[1];
    assert_eq!(
        card.summary,
        "allow the device `notes` for this sandbox: it runs ON THE HOST as you"
    );
    assert!(card.summary.contains("ON THE HOST"));
    assert_eq!(
        card.detail,
        "the command `notes-search -q` ON THE HOST as you, with your home, your keys and your network, in /srv/notes, the request on its standard input"
    );
    assert!(
        card.session_allowed,
        "allow for this session stays available"
    );
    drop(cards);
    // A table cannot say `mode = "allow"` for a host device (the parser
    // refuses it). A def that says so anyway still gets the card.
    let mut forced = host_def("");
    forced.mode = Mode::Allow;
    let g = Gate::new(ctx.clone(), &forced, GrantHit::NoKey);
    assert_eq!(g.check(&call).unwrap(), "allowed for the session");
    assert_eq!(rec.1.lock().unwrap().len(), 3, "the card appeared");
    assert_eq!(
        rec.1.lock().unwrap()[2].detail,
        "the command `notes-search -q` ON THE HOST as you, with your home, your keys and your network, in your home, the request on its standard input"
    );
    // The same def in a sandbox with `mode = "allow"` shows no card.
    let mut allowed = other.clone();
    allowed.mode = Mode::Allow;
    let g = Gate::new(ctx.clone(), &allowed, GrantHit::NoKey);
    assert_eq!(
        g.check(&call).unwrap(),
        "allowed by grant all (no grant key)"
    );
    assert_eq!(rec.1.lock().unwrap().len(), 3, "no card");
    // A denial is a denial.
    let deny = Arc::new(Recording(Answer::Deny, Mutex::new(Vec::new())));
    ctx.approver = deny.clone();
    let g = Gate::new(ctx, &d, GrantHit::NoKey);
    assert_eq!(g.check(&call).unwrap_err(), super::super::DevError::Denied);
}

#[test]
fn the_source_is_chosen_by_run_and_the_supervisor_runs_on_the_host_in_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(dir.path(), Answer::Session);
    let d = host_def("cwd = \"~/notes\"\nenv = { EDITOR = \"vi\" }\ntimeout = 7\nstream = false\n");
    let src = super::super::outproc::source_for(&d, &ctx).expect("a host source");
    assert_eq!(src.instance(), "host");
    assert_eq!(
        src.log_path().unwrap(),
        dir.path().join("state/paddock/devices/log/notes/box1.log")
    );
    let other =
        DevicesCfg::parse("[devices.jq]\nshape = \"exec\"\nenabled = true\ncommand = [\"jq\"]\n")
            .unwrap()
            .user
            .remove(0);
    assert_eq!(
        super::super::outproc::source_for(&other, &ctx)
            .expect("a sandbox source")
            .instance(),
        "box1",
        "a sandbox table gets the device sandbox of the caller"
    );
    assert!(
        super::super::outproc::source_for(
            &DevicesCfg::parse(
                "[devices.far]\nrun = \"remote\"\ntarget = \"ssh://mac\"\ndevice = \"build\"\n"
            )
            .unwrap()
            .user
            .remove(0),
            &ctx
        )
        .is_none(),
        "a remote table has no instance source: forward.rs opens a transport per request"
    );
    let src = HostSource::with_binary(d.clone(), &ctx.env, "box1", PathBuf::from("/opt/paddock"));
    assert_eq!(src.cwd(), dir.path().join("home/notes"), "~ expands");
    let sock = src.socket_path().unwrap();
    assert!(sock.ends_with("box1/dev-notes.sock"), "{}", sock.display());
    let cmd = src.command(&sock, Path::new("/log"));
    assert_eq!(cmd.get_program(), "/opt/paddock");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| String::from_utf8_lossy(a.as_bytes()).into_owned())
        .collect();
    let joined = args.join(" ");
    assert!(joined.starts_with("device-serve --socket "), "{joined}");
    for want in [
        "--shape exec",
        "--timeout 7",
        "--concurrency 1",
        "--log /log",
        "--host",
        &format!("--cwd {}", dir.path().join("home/notes").display()),
        "-- notes-search -q",
    ] {
        assert!(joined.contains(want), "{want} missing in {joined}");
    }
    assert!(!joined.contains("--stream"), "{joined}");
    // The whole environment of paddock, and the fixed values of `env`.
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert!(
        envs.iter()
            .any(|(k, v)| k == "EDITOR" && v.as_deref() == Some("vi")),
        "{envs:?}"
    );
    assert!(
        envs.iter().any(|(k, _)| k == "XDG_RUNTIME_DIR"),
        "the variables of the host: {envs:?}"
    );
    assert_eq!(src.pid(), None, "nothing started");
    // The default folder is the home, and a serve program has no input
    // line on its card.
    let plain = host_def("");
    assert_eq!(
        HostSource::with_binary(plain.clone(), &ctx.env, "box1", PathBuf::from("p")).cwd(),
        dir.path().join("home")
    );
    assert_eq!(
        warning(&plain),
        "notes runs ON THE HOST as you, with your home, your keys and your network: `notes-search -q` in your home"
    );
    let serve = DevicesCfg::parse(
        "[devices.gui]\nshape = \"serve\"\nrun = \"host\"\ncommand = [\"gui-tool\"]\ncwd = \"/opt\"\n",
    )
    .unwrap()
    .user
    .remove(0);
    assert_eq!(
        describe(&serve),
        "the service `gui-tool` ON THE HOST as you, with your home, your keys and your network, in /opt"
    );
}
