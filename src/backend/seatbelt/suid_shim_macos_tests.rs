//! paddock's copy of `ps` in a real Seatbelt run: the rendered profile, the
//! rendered command and the rendered environment with `sandbox-exec`.
//! Temporary folders only: the state folder of paddock is in a temporary
//! home.

use super::*;
use crate::backend::seatbelt::run::SeatbeltRun;
use crate::backend::Backend as _;
use crate::spec::{Mount, Network, SandboxSpec};
use std::collections::BTreeMap;

/// Run `script` in a Seatbelt run: its exit code and its output.
fn sandboxed(script: &str) -> (bool, String, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = crate::backend::seatbelt::real(t.path());
    let home = root.join("home");
    let project = root.join("project");
    let home_src = root.join("sandbox-home");
    for d in [&home, &project, &home_src] {
        fs::create_dir_all(d).unwrap();
    }
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let env = HostEnv::from_parts(home.clone(), "u".into(), uid, gid, BTreeMap::new());
    let mut spec = SandboxSpec::new(
        "shimtest".into(),
        "fake",
        &project,
        home_src,
        &home,
        uid,
        gid,
        "u",
    );
    spec.add_mount(Mount::same(&project, true, "project"));
    spec.network = Network::None;
    spec.command = vec!["/bin/sh".into(), "-c".into(), script.into()];
    fs::create_dir_all(env.paddock_run().join(&spec.id)).unwrap();
    SeatbeltRun.prepare(&env, &spec, false).unwrap();
    let l = SeatbeltRun.render(&env, &spec, false).unwrap();
    for (path, text, _) in &l.files {
        fs::write(path, text).unwrap();
    }
    let out = Command::new(&l.program)
        .args(&l.args)
        .env_clear()
        .envs(&l.env)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        super::root(&env),
    )
}

use std::fs;

#[test]
fn ps_reads_a_start_time_in_the_sandbox() {
    let (ok, out, shims) = sandboxed("command -v ps; ps -o lstart= -p $$");
    assert!(ok, "ps failed in the sandbox: {out}");
    let mut lines = out.lines();
    let found = lines.next().unwrap_or_default();
    assert!(
        Path::new(found).starts_with(&shims),
        "ps is {found}, not paddock's copy in {}",
        shims.display()
    );
    let start = lines.next().unwrap_or_default().trim();
    assert!(!start.is_empty(), "no start time: {out}");
}

#[test]
fn ps_reads_the_start_time_of_a_child() {
    let (ok, out, _) = sandboxed("sleep 5 & ps -o pid=,lstart= -p $!; kill $!");
    assert!(ok, "ps failed in the sandbox: {out}");
    assert!(out.split_whitespace().count() > 2, "{out}");
}

#[test]
fn the_setuid_ps_still_cannot_start() {
    // The profile allows the copy, not setuid programs in general.
    let (ok, out, _) = sandboxed("/bin/ps -o lstart= -p $$");
    assert!(!ok, "/bin/ps ran in the sandbox: {out}");
}
