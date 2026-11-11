//! The fake dsh executables of the end-to-end tests (tests/fixtures/horse)
//! under the dsh custodian and the driver, without a sandbox. The test
//! stands in for the sandbox: a `paddock-host` stub on `PATH` prints the
//! store session, and a plain folder takes the place of the horse device
//! (`/dev/paddock/horse/self`). A write of `dna.toml` in that folder
//! hot-loads, as a write on the device does.
//!
//! The run gets no user input, as `paddock run` with stdin from
//! `/dev/null`: the end of the input comes first.

use super::{run, Setup};
use crate::horse::live::harness::Control;
use crate::horse::live::rpc;
use crate::horse::run::{self as hrun, RunHorse};
use crate::horse::{dna, dsh, Store};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A fixture that runs too long is a hang. Each run takes well under a
/// second.
const HANG: Duration = Duration::from_secs(20);

struct Rig {
    _t: tempfile::TempDir,
    st: Store,
    rh: RunHorse,
    project: PathBuf,
    device: PathBuf,
    child: Child,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start the fixture `name` as `dsh --profile sdk`, with a store, a run
/// and a main session as `paddock run --horse` makes them.
fn rig(name: &str) -> (Rig, Setup) {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let rh = hrun::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    hrun::record_event(&st, &rh, "run.begin", "sbx-fixture", serde_json::json!({})).unwrap();
    let main = format!("session-{}", crate::horse::uuid_v4().unwrap());
    st.lock().unwrap().bind_native(&rh.session, &main).unwrap();

    let (project, device, bin) = (
        t.path().join("project"),
        t.path().join("device"),
        t.path().join("bin"),
    );
    let binding = device.join("sessions").join(&rh.session);
    for d in [&project, &binding, &bin] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(
        binding.join("binding"),
        serde_json::json!({ "harness": "dsh", "native_id": main }).to_string(),
    )
    .unwrap();
    let host = bin.join("paddock-host");
    std::fs::write(&host, format!("#!/bin/sh\ncase \"$1 $2 $3\" in\n'horse sessions ') echo '{}' ;;\n'peek binding ') cat '{}' ;;\n'horse dna put') cat > '{}' ;;\n'peek dna ') cat '{}' ;;\nesac\n", rh.session, binding.join("binding").display(), device.join("dna.toml").display(), device.join("dna.toml").display())).unwrap();
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/horse")
        .join(name);
    let script = std::fs::read_to_string(&src)
        .unwrap()
        .replace("/dev/paddock/horse/self", device.to_str().unwrap());
    let dsh = bin.join("dsh");
    std::fs::write(&dsh, script).unwrap();
    std::fs::set_permissions(&dsh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let child = Command::new(&dsh)
        .args(["--profile", "sdk"])
        .current_dir(&project)
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let set = Setup {
        store: st.clone(),
        horse: rh.horse.clone(),
        main,
        resume: false,
        sessions_root: Some(t.path().join("sessions")),
        cwd: project.display().to_string(),
        provider: crate::horse::live::PROVIDER.into(),
        model: crate::horse::live::MODEL.into(),
        effort: None,
        run: "sbx-fixture".into(),
        tick: Duration::from_millis(20),
    };
    (
        Rig {
            _t: t,
            st,
            rh,
            project,
            device,
            child,
        },
        set,
    )
}

/// Hold the custody of the fixture with no user input. `side` runs
/// beside the custody (the device, the host). Panics when the custody
/// does not end: the fixture is then killed, so the test does not hang.
fn hold(r: &mut Rig, set: Setup, side: impl FnOnce(&Rig) + Send) -> super::Report {
    let stdin = r.child.stdin.take().unwrap();
    let stdout = r.child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    rpc::read_runtime(stdout, tx);
    let (ctl, control) = std::sync::mpsc::channel();
    // stdin is /dev/null: the end of the input comes before anything else.
    ctl.send(Control::Close).unwrap();
    drop(ctl);
    let join = std::thread::spawn(move || run(set, stdin, rx, &control, std::io::sink()));
    std::thread::scope(|s| {
        let rig: &Rig = r;
        s.spawn(move || side(rig));
    });
    let end = Instant::now() + HANG;
    while !join.is_finished() {
        if Instant::now() > end {
            let _ = r.child.kill();
            let _ = join.join();
            panic!(
                "the custody did not end within {} s; the fake got:\n{}",
                HANG.as_secs(),
                std::fs::read_to_string(r.project.join("prompts.txt")).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let report = join.join().unwrap().unwrap();
    assert!(r.child.wait().unwrap().success(), "the fake dsh failed");
    report
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + HANG;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The prompt line that the fake got with `needle`.
fn prompt_with(r: &Rig, needle: &str) -> String {
    let prompts = std::fs::read_to_string(r.project.join("prompts.txt")).unwrap();
    prompts
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no entry with {needle}: {prompts}"))
        .to_string()
}

#[test]
fn a_dna_edit_of_the_fixture_hot_loads_although_the_input_ended_first() {
    let (mut r, set) = rig("dsh-dna");
    let report = hold(&mut r, set, |r| {
        // The device: a write of dna.toml hot-loads.
        let file = r.device.join("dna.toml");
        wait_for("the DNA edit of the fake", || {
            std::fs::read_to_string(&file).is_ok_and(|t| t.contains("careful-e2e"))
        });
        let text = std::fs::read_to_string(&file).unwrap();
        dna::hot_load(&r.st, &r.rh.horse, &text).unwrap().unwrap();
    });
    let entry = prompt_with(&r, "careful-e2e");
    assert!(entry.contains("session/prompt"), "{entry}");
    assert!(entry.contains("role=\\\"system\\\""), "{entry}");
    assert!(entry.contains("hot-load"), "{entry}");
    // The start entry and the hot-load, each with a delivery record.
    assert_eq!(report.delivered, 2, "{report:?}");
    assert!(r.st.inbox(&r.rh.session).unwrap().is_empty());
}

#[test]
fn a_graft_reaches_the_idle_fixture_although_the_input_ended_first() {
    let (mut r, set) = rig("dsh-graft");
    let report = hold(&mut r, set, |r| {
        // The host: graft when the fake says that its session is idle.
        wait_for("the idle fake", || r.project.join("sessions.txt").exists());
        let donor = r.st.new_default_horse().unwrap();
        dna::hot_load(&r.st, &donor.id, "[traits]\nlang = \"graft-e2e\"\n").unwrap();
        dna::graft::graft_horses(&r.st, &r.rh.horse, &donor.id, "combine")
            .unwrap()
            .unwrap();
    });
    let entry = prompt_with(&r, "graft-e2e");
    assert!(entry.contains("role=\\\"system\\\""), "{entry}");
    assert!(entry.contains("graft from"), "{entry}");
    assert_eq!(report.delivered, 2, "{report:?}");
    assert!(r.st.inbox(&r.rh.session).unwrap().is_empty());
}
