//! The escape probe of a terminal run, for Seatbelt. `src/probe.sh` is for
//! bwrap: it checks mounts, namespaces and /proc, which macOS does not
//! have. These checks run the rendered profile, the rendered command and
//! the rendered environment with `sandbox-exec`. Temporary folders only:
//! the "real home" is a temporary folder too.

use super::*;
use crate::spec::Mount;
use std::process::{Command, Stdio};

struct World {
    _t: tempfile::TempDir,
    env: HostEnv,
    spec: SandboxSpec,
    /// The stand-in for the real home: nothing in it is allowed.
    home: PathBuf,
}

fn world(network: Network) -> World {
    let t = tempfile::tempdir().unwrap();
    let root = real(t.path());
    let home = root.join("home");
    let project = root.join("project");
    let home_src = root.join("sandbox-home");
    for d in [&home, &project, &home_src] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(home.join("secret.txt"), "secret").unwrap();
    std::fs::write(project.join("file.txt"), "x").unwrap();
    std::fs::write(project.join(".env"), "TOKEN=1").unwrap();
    // The real ids: the proxy socket goes into /tmp/paddock-<uid>.
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let env = HostEnv::from_parts(home.clone(), "u".into(), uid, gid, BTreeMap::new());
    let mut spec = SandboxSpec::new(
        "sbtest".into(),
        "fake",
        &project,
        home_src,
        &home,
        uid,
        gid,
        "u",
    );
    spec.add_mount(Mount::same(&project, true, "project"));
    spec.add_mount(Mount::mask(project.join(".env"), false, "secrets"));
    spec.network = network;
    std::fs::create_dir_all(env.paddock_run().join(&spec.id)).unwrap();
    World {
        _t: t,
        env,
        spec,
        home,
    }
}

/// Run `script` in the sandbox. Its exit code says if it succeeded.
fn sandboxed(w: &World, script: &str) -> bool {
    let mut spec = w.spec.clone();
    spec.command = vec!["/bin/sh".into(), "-c".into(), script.into()];
    SeatbeltRun.prepare(&w.env, &spec, false).unwrap();
    let l = SeatbeltRun.render(&w.env, &spec, false).unwrap();
    for (path, text, _) in &l.files {
        std::fs::write(path, text).unwrap();
    }
    Command::new(&l.program)
        .args(&l.args)
        .env_clear()
        .envs(&l.env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn it_reads_and_writes_the_project_in_its_work_folder() {
    let w = world(Network::None);
    assert!(sandboxed(&w, "test \"$(cat file.txt)\" = x"));
    assert!(sandboxed(&w, "echo y > new.txt && test -f new.txt"));
    assert!(w.spec.project.join("new.txt").is_file());
    assert!(!sandboxed(&w, "cat .env"));
}

#[test]
fn it_cannot_read_or_write_the_real_home_or_shared_temp() {
    let w = world(Network::None);
    let secret = w.home.join("secret.txt").display().to_string();
    assert!(!sandboxed(&w, &format!("cat '{secret}'")));
    assert!(!sandboxed(&w, &format!("ls '{}'", w.home.display())));
    assert!(!sandboxed(
        &w,
        &format!("touch '{}'", w.home.join("planted").display())
    ));
    assert!(!w.home.join("planted").exists());
    // The host /tmp holds the sockets of other runs.
    assert!(!sandboxed(&w, "touch /private/tmp/paddock-probe-$$"));
    // Its own home and temporary folder work.
    assert!(sandboxed(
        &w,
        "echo x > \"$HOME/h\" && echo x > \"$TMPDIR/t\""
    ));
    assert!(sandboxed(
        &w,
        "test \"$HOME\" = \"$CFFIXED_USER_HOME\" && test \"$HOME\" != /"
    ));
}

#[test]
fn it_reaches_the_network_only_through_the_proxy() {
    let other = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let other_port = other.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in other.incoming() {});
    let mut w = world(Network::Egress);
    start_proxy(&w.env, &mut w.spec).unwrap();
    let relay = w
        .spec
        .host_ports
        .iter()
        .find(|(_, o)| o == RELAY_ORIGIN)
        .unwrap()
        .0;
    let nc = |port: u16| format!("/usr/bin/nc -z -G 2 127.0.0.1 {port}");
    assert!(sandboxed(&w, &nc(relay)));
    assert!(!sandboxed(&w, &nc(other_port)));
    assert!(sandboxed(
        &w,
        &format!("test \"$HTTPS_PROXY\" = http://127.0.0.1:{relay}")
    ));
    // A host loopback port through the proxy: the proxy refuses it (403),
    // which shows that the proxy answers.
    assert!(sandboxed(
        &w,
        &format!(
            "test \"$(/usr/bin/curl -s --noproxy '' -o /dev/null -w '%{{http_code}}' -x \"$HTTP_PROXY\" http://127.0.0.1:{other_port}/)\" = 403"
        )
    ));
    // Without network, the relay is out of reach too.
    let none = world(Network::None);
    assert!(!sandboxed(&none, &nc(relay)));
}

#[test]
fn network_host_reaches_host_ports_and_listens() {
    let other = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let other_port = other.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in other.incoming() {});
    let nc = format!("/usr/bin/nc -z -G 2 127.0.0.1 {other_port}");
    let mut w = world(Network::Host);
    w.spec.set_env(
        "HTTPS_PROXY",
        "http://proxy.example:3128",
        "network: host proxy",
    );
    assert!(sandboxed(&w, &nc));
    // Name resolution, with no traffic outside this Mac.
    assert!(sandboxed(
        &w,
        &format!("/usr/bin/nc -z -G 2 localhost {other_port}")
    ));
    // The host proxy variables pass, as on Linux.
    assert!(sandboxed(
        &w,
        "test \"$HTTPS_PROXY\" = http://proxy.example:3128"
    ));
    // Its own server on every address, reached from the host.
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut spec = w.spec.clone();
    spec.command = vec!["/usr/bin/nc".into(), "-l".into(), port.to_string()];
    SeatbeltRun.prepare(&w.env, &spec, false).unwrap();
    let l = SeatbeltRun.render(&w.env, &spec, false).unwrap();
    for (path, text, _) in &l.files {
        std::fs::write(path, text).unwrap();
    }
    let mut child = Command::new(&l.program)
        .args(&l.args)
        .env_clear()
        .envs(&l.env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut listened = false;
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            listened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(listened);
    // Network mode egress does not reach the same port without the proxy.
    assert!(!sandboxed(&world(Network::Egress), &nc));
}

#[test]
fn allowlist_reaches_only_the_listed_hosts_through_the_proxy() {
    let other = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let other_port = other.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in other.incoming() {});
    let mut w = world(Network::Allowlist);
    w.spec.allow_domains = vec!["api.example.com".into()];
    start_proxy(&w.env, &mut w.spec).unwrap();
    let relay = w
        .spec
        .host_ports
        .iter()
        .find(|(_, o)| o == RELAY_ORIGIN)
        .unwrap()
        .0;
    let nc = |port: u16| format!("/usr/bin/nc -z -G 2 127.0.0.1 {port}");
    assert!(sandboxed(&w, &nc(relay)));
    assert!(!sandboxed(&w, &nc(other_port)));
    // A host outside the list: the proxy answers the CONNECT with 403.
    // curl sends the name to the proxy and does not resolve it.
    assert!(sandboxed(
        &w,
        "test \"$(/usr/bin/curl -s -o /dev/null -w '%{http_connect}' -x \"$HTTPS_PROXY\" https://other.example.com/)\" = 403"
    ));
    let log = crate::boundary::read(&w.env, &w.spec.id);
    assert!(
        log.iter().any(|e| e.target == "other.example.com:443"
            && e.rule == "network mode allowlist: not on the list"),
        "{log:?}"
    );
}

#[test]
fn it_cannot_open_another_terminal() {
    let w = world(Network::None);
    // Opening for reading only: nothing reaches the terminal.
    if Path::new("/dev/ttys000").exists() {
        assert!(!sandboxed(&w, "exec 3</dev/ttys000"));
    }
    assert!(sandboxed(&w, "echo x > /dev/null"));
}

/// `security` on the host with the stand-in home, so the real search list
/// of the user never changes.
fn host_security(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new("/usr/bin/security")
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn the_keychain_of_the_login_opens_and_the_users_keychain_stays_closed() {
    let mut w = world(Network::None);
    // A stand-in for the user's login keychain: a temporary keychain file
    // in the stand-in home, unlocked, with an item.
    // Named `user`: the security service treats a keychain named `login`
    // as the login keychain and refuses our test password for it.
    let user = w.home.join("Library/Keychains/user.keychain-db");
    std::fs::create_dir_all(user.parent().unwrap()).unwrap();
    let u = user.display().to_string();
    for args in [
        vec!["create-keychain", "-p", "pw", &u],
        vec!["unlock-keychain", "-p", "pw", &u],
        vec![
            "add-generic-password",
            "-a",
            "a",
            "-s",
            "paddock-user-item",
            "-w",
            "user-secret",
            &u,
        ],
    ] {
        assert!(host_security(&w.home, &args).status.success(), "{args:?}");
    }
    let mode = crate::state::AuthMode::Isolated("t".into());
    super::super::keychain::start(&w.env, &mut w.spec, &mode).unwrap();
    let kc = w.spec.env[super::super::keychain::VAR].value.clone();
    use std::os::unix::fs::PermissionsExt;
    let kc_mode = std::fs::metadata(&kc).unwrap().permissions().mode() & 0o777;
    assert_eq!(kc_mode, 0o600, "the keychain file is private");
    let sec = "/usr/bin/security";
    // The search list of the sandbox names its own keychain.
    assert!(sandboxed(
        &w,
        &format!("{sec} list-keychains -d user | grep -qF '{kc}'")
    ));
    assert!(sandboxed(
        &w,
        &format!("{sec} default-keychain -d user | grep -qF '{kc}'")
    ));
    assert!(sandboxed(&w, &format!("{sec} show-keychain-info")));
    // An item that the sandbox writes, as Claude Code does, reads back in a
    // new sandbox.
    assert!(sandboxed(
        &w,
        &format!("{sec} add-generic-password -U -a a -s paddock-test -w s3cret")
    ));
    assert!(sandboxed(
        &w,
        &format!("test \"$({sec} find-generic-password -a a -s paddock-test -w)\" = s3cret")
    ));
    // The stand-in user keychain stays closed: by path, by search and as
    // a file.
    assert!(!sandboxed(
        &w,
        &format!("{sec} find-generic-password -s paddock-user-item -w '{u}'")
    ));
    assert!(!sandboxed(
        &w,
        &format!("{sec} find-generic-password -s paddock-user-item -w")
    ));
    assert!(!sandboxed(&w, &format!("cat '{u}' >/dev/null")));
    assert!(!sandboxed(
        &w,
        &format!(
            "{sec} list-keychains -s '{u}' && {sec} find-generic-password -s paddock-user-item -w"
        )
    ));
    // The system keychains stay closed too.
    assert!(!sandboxed(
        &w,
        "cat /Library/Keychains/System.keychain >/dev/null"
    ));
    // The test is sound: the host reads both items.
    let out = host_security(
        &w.home,
        &["find-generic-password", "-s", "paddock-user-item", "-w", &u],
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "user-secret");
    let out = host_security(
        &w.home,
        &["find-generic-password", "-s", "paddock-test", "-w", &kc],
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "s3cret");
    // A second start keeps the keychain and its item.
    super::super::keychain::start(&w.env, &mut w.spec, &mode).unwrap();
    assert!(sandboxed(
        &w,
        &format!("{sec} find-generic-password -a a -s paddock-test -w >/dev/null")
    ));
    for k in [&u, &kc] {
        host_security(&w.home, &["delete-keychain", k]);
    }
}

/// A probe for ptys: it makes a pty with openpty, opens its terminal side
/// by name and sends a byte through it. Then it opens the terminal in
/// argv[1], which it did not make. Exit 0 when all of it works and the
/// open of argv[1] fails with EPERM.
const PTY_PROBE: &str = r#"
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>
#include <util.h>
int main(int argc, char **argv) {
    int m, s;
    char name[128], c;
    if (argc != 2) return 2;
    if (openpty(&m, &s, name, NULL, NULL) != 0) return 10;
    if (strncmp(name, "/dev/ttys", 9) != 0) return 11;
    int again = open(name, O_RDWR | O_NOCTTY);
    if (again < 0) return 12;
    if (write(m, "x\n", 2) != 2) return 13;
    if (read(again, &c, 1) != 1 || c != 'x') return 14;
    if (open(argv[1], O_RDWR | O_NOCTTY) >= 0) return 20;
    return errno == EPERM ? 0 : 21;
}
"#;

/// The probe, built with `cc` and signed with the hardened runtime, as
/// released agy, Codex and Grok builds are. None without `cc` or
/// `codesign`.
fn hardened_probe(dir: &Path) -> Option<PathBuf> {
    let codesign = Path::new("/usr/bin/codesign");
    let cc = Command::new("cc").arg("--version").output();
    if !codesign.is_file() || !cc.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipped: no cc or codesign");
        return None;
    }
    let src = dir.join("ptyprobe.c");
    let bin = dir.join("ptyprobe");
    std::fs::write(&src, PTY_PROBE).unwrap();
    let ok = |c: &mut Command| c.status().unwrap().success();
    assert!(ok(Command::new("cc").arg("-o").arg(&bin).arg(&src)));
    assert!(ok(Command::new(codesign)
        .args(["-f", "-s", "-", "-o", "runtime"])
        .arg(&bin)));
    Some(bin)
}

/// The exit code of `cmd` in the sandbox, with the policy of the run
/// changed by `edit`, and the profile text.
fn sandboxed_code(w: &World, cmd: Vec<String>, edit: fn(&mut Policy)) -> (i32, String) {
    let mut spec = w.spec.clone();
    spec.command = cmd;
    SeatbeltRun.prepare(&w.env, &spec, false).unwrap();
    let l = SeatbeltRun.render(&w.env, &spec, false).unwrap();
    let (mut pol, _) = policy(&w.env, &spec, &private_tmp(&spec)).unwrap();
    edit(&mut pol);
    let text = crate::seatbelt::render(&pol).unwrap();
    std::fs::write(&l.files[0].0, &text).unwrap();
    let code = Command::new(&l.program)
        .args(&l.args)
        .env_clear()
        .envs(&l.env)
        .stdin(Stdio::null())
        .status()
        .unwrap()
        .code()
        .unwrap_or(-1);
    (code, text)
}

#[test]
fn a_hardened_harness_makes_its_own_ptys_and_cannot_open_others() {
    let w = world(Network::None);
    let Some(probe) = hardened_probe(&w.spec.project) else {
        return;
    };
    // A pty of the host, outside the sandbox: the probe must not open it.
    let (mut m, mut s) = (0, 0);
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: the out pointers are valid, the name buffer is 128 bytes.
    let rc = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            name.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0);
    // SAFETY: openpty wrote a terminated name.
    let other = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let cmd = vec![probe.display().to_string(), other.clone()];
    // The host itself opens the other pty: the probe's EPERM comes from
    // the sandbox.
    assert!(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&other)
        .is_ok());
    let (code, text) = sandboxed_code(&w, cmd.clone(), |_| {});
    assert!(
        text.contains("(allow pseudo-tty)"),
        "pty_refusal found no reason for the hardened probe"
    );
    assert_eq!(code, 0, "probe exit code {code} (see PTY_PROBE)");
    // The test is sound: without the rules, openpty fails.
    let (code, _) = sandboxed_code(&w, cmd, |p| p.own_ptys = false);
    assert_eq!(code, 10);
    // SAFETY: the test owns both descriptors.
    unsafe {
        libc::close(m);
        libc::close(s);
    }
}

/// `paddock freeze` of a Seatbelt run: SIGSTOP to its process group and to
/// its process tree, also a process that left the tree (its parent ended).
#[test]
fn freeze_stops_the_whole_run_and_thaw_continues_it() {
    use std::os::unix::process::CommandExt;
    let w = world(Network::None);
    let mut spec = w.spec.clone();
    // a: a child; b: an orphan whose subshell ended; c: a grandchild.
    let script =
        "sleep 60 & echo $! > a; (sleep 60 & echo $! > b); (sleep 60 & echo $! > c; wait) & wait";
    spec.command = vec!["/bin/sh".into(), "-c".into(), script.into()];
    SeatbeltRun.prepare(&w.env, &spec, false).unwrap();
    let l = SeatbeltRun.render(&w.env, &spec, false).unwrap();
    for (path, text, _) in &l.files {
        std::fs::write(path, text).unwrap();
    }
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args)
        .env_clear()
        .envs(&l.env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: own_group only makes async-signal-safe calls.
    unsafe { cmd.pre_exec(|| crate::freeze::own_group()) };
    let mut child = cmd.spawn().unwrap();
    let pid = child.id();
    let read = |f: &str| -> Option<u32> {
        std::fs::read_to_string(w.spec.project.join(f))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let start = std::time::Instant::now();
    let mut pids = Vec::new();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        if let (Some(a), Some(b), Some(c)) = (read("a"), read("b"), read("c")) {
            pids = vec![pid, a, b, c];
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(pids.len(), 4, "the script did not start its processes");
    // SAFETY: getpgid only reads.
    assert_eq!(unsafe { libc::getpgid(pid as i32) }, pid as i32);
    let r: crate::state::Running = serde_json::from_value(serde_json::json!({
        "id": "sbtest", "harness": "fake", "project": w.spec.project, "backend": NAME,
        "network": "none", "gui": "off", "pid": pid, "started": 0,
        "host_pid": std::process::id(),
    }))
    .unwrap();
    let all = |stopped: bool| {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            let now = pids.iter().all(|&p| {
                crate::platform::bsd_info(p).map(|i| i.pbi_status == libc::SSTOP) == Some(stopped)
            });
            if now {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    };
    crate::freeze::signal_run(&r, true).unwrap();
    assert!(all(true), "not every process stopped: {pids:?}");
    crate::freeze::signal_run(&r, false).unwrap();
    assert!(all(false), "not every process runs again: {pids:?}");
    // SAFETY: the test's own process group.
    unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
    let _ = child.wait();
}

#[test]
fn it_changes_its_borrowed_copies_but_not_the_borrow_folder_itself() {
    let mut w = world(Network::None);
    let dir = real(w.home.parent().unwrap()).join("data/borrow/sbtest");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib/a.rs"), "old").unwrap();
    w.spec.add_mount(Mount::bind(
        &dir,
        crate::borrow::MOUNT,
        true,
        "borrow folder",
    ));
    assert!(sandboxed(
        &w,
        &format!("test \"$PADDOCK_BORROW_DIR\" = '{}'", dir.display())
    ));
    // The copies: read, change, add, remove.
    assert!(sandboxed(
        &w,
        "cd \"$PADDOCK_BORROW_DIR\" && test \"$(cat lib/a.rs)\" = old && echo new > lib/a.rs && mkdir lib/d && echo x > lib/d/b && mkdir gone && rmdir gone"
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("lib/a.rs")).unwrap(),
        "new\n"
    );
    assert!(dir.join("lib/d/b").is_file());
    // The folder itself cannot be removed, moved or replaced by a link.
    std::fs::remove_dir_all(dir.join("lib")).unwrap();
    assert!(!sandboxed(&w, "rmdir \"$PADDOCK_BORROW_DIR\""));
    assert!(!sandboxed(
        &w,
        "mv \"$PADDOCK_BORROW_DIR\" \"$PADDOCK_BORROW_DIR.x\""
    ));
    assert!(!sandboxed(&w, "chmod 777 \"$PADDOCK_BORROW_DIR\""));
    assert!(std::fs::symlink_metadata(&dir).unwrap().is_dir());
    // There is no /run/paddock on macOS.
    assert!(!sandboxed(&w, "test -e /run/paddock/borrow"));
}

/// The paddock binary that `cargo test` builds next to the test binary.
fn paddock_bin() -> Option<PathBuf> {
    let p = std::env::current_exe()
        .ok()?
        .parent()?
        .parent()?
        .join("paddock");
    p.is_file().then_some(p)
}

#[test]
fn the_devices_come_over_the_socket_and_secret_files_are_fifos() {
    use crate::credentials::secret::{Secret, SecretFiles};
    let Some(bin) = paddock_bin() else {
        eprintln!("skipped: no paddock binary next to the tests (run cargo build first)");
        return;
    };
    let mut w = world(Network::None);
    let run = w.env.paddock_run().join(&w.spec.id);
    let wrapper = run.join("paddock-host");
    std::fs::write(&wrapper, crate::backend::seatbelt::host_wrapper(&bin)).unwrap();
    std::fs::set_permissions(
        &wrapper,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    w.spec.add_tool("paddock", bin.clone(), "client");
    w.spec.add_tool("paddock-host", wrapper, "client");
    crate::privfs::write(
        &w.env.paddock_config().join("config.toml"),
        "[devices]\necho = true\n",
    )
    .unwrap();
    let revoked = w._t.path().join("revoked");
    let secrets = SecretFiles::new(Some(revoked.clone()), None);
    let devices = crate::devices::start(
        &w.env,
        &mut w.spec,
        NAME,
        false,
        &crate::devices::GrantCtx::none(),
        &[],
        None,
        secrets.clone(),
        None,
    )
    .expect("the devices of a Seatbelt run");
    assert!(w.spec.notices.is_empty(), "{:?}", w.spec.notices);
    secrets.insert("T", Secret::from("tok-1"));
    let fifo = crate::devices::socket_dir(&w.env, &w.spec.id).join("secret/T");
    let t = std::time::Instant::now();
    while !fifo.exists() {
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "no FIFO");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(sandboxed(
        &w,
        "test \"$(paddock-host device echo hello)\" = hello"
    ));
    assert!(sandboxed(
        &w,
        "paddock-host device list | grep -q '\"echo\"'"
    ));
    assert!(sandboxed(
        &w,
        "test \"$(cat \"$PADDOCK_DEVICES/secret/T\")\" = tok-1"
    ));
    std::fs::write(&revoked, "T\n").unwrap();
    let t = std::time::Instant::now();
    while fifo.exists() {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(1),
            "the FIFO stays"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!sandboxed(&w, "cat \"$PADDOCK_DEVICES/secret/T\""));
    // There is no /dev/paddock on macOS: a plain open of a device fails.
    assert!(!sandboxed(&w, "exec 3<>/dev/paddock/echo"));
    drop(devices);
}
