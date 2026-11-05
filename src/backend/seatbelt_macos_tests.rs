use super::*;
use std::process::Command;

fn run_in(profile: &str, cmd: &[&str]) -> bool {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("p.sb");
    std::fs::write(&f, profile).unwrap();
    Command::new(SANDBOX_EXEC)
        .arg("-f")
        .arg(&f)
        .args(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn ls_reads_the_project_but_not_the_real_home() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    // A folder in the real home, as most projects are.
    let base = tempfile::tempdir_in(&home).unwrap();
    let project = base.path().join("project");
    let secret = base.path().join("secret.txt");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("file.txt"), "x").unwrap();
    std::fs::write(project.join(".env"), "TOKEN=1").unwrap();
    std::fs::write(&secret, "secret").unwrap();
    let mut p = Policy::new(Net::None);
    p.add(real(&project), Access::ReadWrite, "project");
    p.add_file(real(&project.join(".env")), Access::Deny, "secrets");
    let sbpl = crate::seatbelt::render(&p).unwrap();
    let s = |p: &Path| p.display().to_string();
    assert!(run_in(&sbpl, &["/bin/ls", &s(&project)]));
    assert!(run_in(&sbpl, &["/bin/cat", &s(&project.join("file.txt"))]));
    assert!(!run_in(&sbpl, &["/bin/cat", &s(&secret)]));
    assert!(!run_in(&sbpl, &["/bin/cat", &s(&project.join(".env"))]));
    assert!(!run_in(&sbpl, &["/bin/ls", &s(&home)]));
    assert!(run_in(&sbpl, &["/usr/bin/touch", &s(&project.join("new"))]));
    assert!(!run_in(
        &sbpl,
        &["/usr/bin/touch", &s(&base.path().join("new"))]
    ));
}

#[test]
fn localhost_only_refuses_an_outside_address() {
    let l = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
    let port = l.local_addr().unwrap().port().to_string();
    std::thread::spawn(move || for _ in l.incoming() {});
    let nc = |sbpl: Option<&str>, host: &str| {
        let cmd = ["/usr/bin/nc", "-z", "-G", "2", host, &port];
        match sbpl {
            Some(p) => run_in(p, &cmd),
            None => Command::new(cmd[0])
                .args(&cmd[1..])
                .status()
                .unwrap()
                .success(),
        }
    };
    let local = crate::seatbelt::render(&Policy::new(Net::Localhost)).unwrap();
    let none = crate::seatbelt::render(&Policy::new(Net::None)).unwrap();
    assert!(nc(Some(&local), "127.0.0.1"));
    assert!(!nc(Some(&none), "127.0.0.1"));
    // An address of this Mac that is not loopback: the same listener
    // answers it outside the sandbox, so only the profile refuses it.
    let out = Command::new("/usr/sbin/ipconfig")
        .args(["getifaddr", "en0"])
        .output()
        .unwrap();
    let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ip.is_empty() {
        eprintln!("no address on en0: the outside check is skipped");
        return;
    }
    assert!(nc(None, &ip));
    if nc(Some(&local), &ip) {
        // Some macOS versions (15) count the Mac's own addresses as
        // localhost. The port list must still hold there.
        eprintln!("Seatbelt counts {ip} as localhost on this macOS");
        let other = crate::seatbelt::render(&Policy::terminal(Net::Localhost, vec![9])).unwrap();
        assert!(!nc(Some(&other), &ip));
        assert!(!nc(Some(&other), "127.0.0.1"));
    }
}

#[test]
fn bundle_detection_on_this_mac() {
    // Only checks what is installed; it never starts an app. Apps change
    // their entitlements between versions, so codesign's own XML is the
    // reference, not a fixed answer per app.
    for b in [
        "/Applications/ChatGPT.app",
        "/Applications/Claude.app",
        "/Applications/Visual Studio Code.app",
    ] {
        let b = Path::new(b);
        if !b.is_dir() {
            continue;
        }
        let exe = bundle_exe(b).unwrap();
        let xml = std::process::Command::new("/usr/bin/codesign")
            .args(["-d", "--entitlements", "-", "--xml"])
            .arg(&exe)
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap()
            .stdout;
        let flat: String = String::from_utf8_lossy(&xml)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let want = flat.contains("<key>com.apple.security.app-sandbox</key><true/>");
        assert_eq!(
            app_sandboxed(&entitlements(&exe).unwrap()),
            want,
            "{}",
            b.display()
        );
    }
}

// tackd on macOS: the kernel facts that tack reads. The maintainer runs
// these on a Mac.

#[test]
fn tack_reads_this_process_from_the_kernel() {
    let me = std::process::id();
    let p = crate::tack::procs::macos::get(me).expect("proc_pidinfo for this process");
    assert_eq!(p.pid, me);
    assert_eq!(p.uid, unsafe { libc::getuid() });
    assert_eq!(p.ppid, std::os::unix::process::parent_id());
    let exe = p.exe.expect("proc_pidpath");
    assert_eq!(
        std::fs::canonicalize(&exe).unwrap(),
        std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
    );
    assert!(!p.contained, "a test run outside Seatbelt is not sandboxed");
    let c = crate::tack::procs::chain(&crate::tack::procs::Kernel::new(), me, &[]).unwrap();
    assert!(c.links.len() >= 2, "{:?}", c.summary());
    assert_eq!(c.links.last().unwrap().proc.pid, 1, "{:?}", c.summary());
}

#[test]
fn tack_marks_a_process_under_seatbelt_as_contained() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("p.sb");
    std::fs::write(&f, "(version 1)\n(allow default)\n").unwrap();
    let mut child = Command::new(SANDBOX_EXEC)
        .arg("-f")
        .arg(&f)
        .args(["/bin/sleep", "5"])
        .spawn()
        .unwrap();
    // sandbox-exec applies the profile, then execs sleep in the same pid.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let p = crate::tack::procs::macos::get(child.id()).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        p.contained,
        "sandbox_check did not report the Seatbelt profile: {p:?}"
    );
    assert_eq!(p.exe.as_deref(), Some(std::path::Path::new("/bin/sleep")));
}

#[test]
fn tack_peer_credentials_name_this_process() {
    let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
    let c = crate::tack::peer::of(&a).unwrap();
    assert_eq!(c.uid, unsafe { libc::geteuid() });
    assert_eq!(c.pid, std::process::id());
}

#[test]
fn tack_reads_the_signer_of_a_system_program() {
    // Apple's own programs are signed by "Software Signing".
    assert_eq!(
        crate::tack::ident::signer(std::path::Path::new("/bin/ls")).as_deref(),
        Some("Software Signing")
    );
    // An unsigned file has none.
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("tool");
    std::fs::write(&f, "#!/bin/sh\n").unwrap();
    assert_eq!(crate::tack::ident::signer(&f), None);
}

#[test]
fn tack_service_paths_and_plist_are_the_macos_ones() {
    let p = crate::tack::service::Paths::system();
    assert_eq!(
        p.binary,
        std::path::Path::new("/Library/PrivilegedHelperTools/io.paddock.tackd")
    );
    assert_eq!(
        p.unit,
        std::path::Path::new("/Library/LaunchDaemons/io.paddock.tackd.plist")
    );
    assert_eq!(
        p.socket,
        std::path::Path::new("/var/run/paddock-tackd/tackd.sock")
    );
    // plutil checks that the plist is valid.
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("tackd.plist");
    std::fs::write(&f, crate::tack::service::service_text(&p)).unwrap();
    let ok = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(&f)
        .status()
        .unwrap();
    assert!(ok.success());
}

#[test]
fn tack_descriptors_pass_over_a_socket_on_macos() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let (mut r, w) = std::os::unix::net::UnixStream::pair().unwrap();
    crate::tack::fdpass::send(&a, b"hello", &[w.as_raw_fd()]).unwrap();
    drop(w);
    let mut buf = [0u8; 5];
    let (n, fds) = crate::tack::fdpass::recv(&b, &mut buf, 3).unwrap();
    assert_eq!(&buf[..n], b"hello");
    assert_eq!(fds.len(), 1);
    let flags = unsafe { libc::fcntl(fds[0].as_raw_fd(), libc::F_GETFD) };
    assert_ne!(
        flags & libc::FD_CLOEXEC,
        0,
        "a received descriptor is close-on-exec"
    );
    let mut got = std::os::unix::net::UnixStream::from(fds.into_iter().next().unwrap());
    got.write_all(b"x").unwrap();
    drop(got);
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    assert_eq!(s, "x");
}

#[test]
fn tack_the_stand_in_runs_the_program_with_references_on_macos() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("PATH".to_string(), bin.display().to_string());
    let env = crate::env::HostEnv::from_parts(t.path().into(), "u".into(), 501, 20, vars);
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        crate::tack::config::file(&env),
        format!(
            "[tack.secrets.GH_TOKEN]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\n",
            gh.display()
        ),
    )
    .unwrap();
    let paths = crate::tack::service::Paths::under(&t.path().join("root"));
    let r = crate::tack::standin::route(&env, true, None, &["gh".into()], &paths);
    match r {
        crate::tack::standin::Route::Direct {
            note: Some((kind, msg)),
            ..
        } => {
            assert_eq!(kind, "platform");
            assert!(
                msg.contains("Linux only") && msg.contains("GH_TOKEN"),
                "{msg}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn tack_a_start_request_gets_a_clear_answer_from_a_macos_tackd() {
    use crate::tack::daemon::{bind, Server, Users};
    struct NoCfg;
    impl Users for NoCfg {
        fn config(&self, _: u32) -> Result<crate::tack::config::Tack, String> {
            Ok(Default::default())
        }
    }
    let me = unsafe { libc::geteuid() };
    let t = tempfile::tempdir().unwrap();
    let mut table = crate::tack::procs::Fake::default()
        .add(1, 0, "/sbin/launchd", &[])
        .add(
            std::process::id(),
            1,
            "/usr/local/bin/paddock",
            &["paddock"],
        );
    table.0.get_mut(&std::process::id()).unwrap().uid = me;
    let audit = crate::tack::audit::Log::new(t.path().join("log/tackd.log"), me);
    let server = std::sync::Arc::new(Server::new(Box::new(table), Box::new(NoCfg), audit));
    assert!(
        server.starter.is_none(),
        "tackd starts no program on macOS yet"
    );
    let sock = t.path().join("run/tackd.sock");
    let l = bind(&sock, me).unwrap();
    let s = server.clone();
    std::thread::spawn(move || s.serve(l));
    let req = crate::tack::proto::Start {
        program: "/bin/ls".into(),
        argv0: "ls".into(),
        args: vec![],
        cwd: "/".into(),
        caller_pid: std::process::id(),
        env: vec![],
        stdio: [false; 3],
    };
    let c = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    let o = crate::tack::standin::session(&c, &req, &[], None, &mut |_| {}).unwrap();
    // /bin/ls is on no allowlist: a denial, and nothing starts.
    assert!(
        matches!(o, crate::tack::standin::Outcome::Denied(_)),
        "{o:?}"
    );
    assert!(crate::tack::standin::from_terminal(libc::SIGINT, 0));
    assert!(!crate::tack::standin::from_terminal(libc::SIGTERM, 0));
}
