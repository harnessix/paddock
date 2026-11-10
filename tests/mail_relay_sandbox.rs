//! A fake Codex TUI receives mail in a real bwrap sandbox over the relay.
mod common;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn a_sandboxed_tui_gets_mail_at_its_idle_prompt() {
    if !common::sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let tmp = common::tempdir();
    let root = tmp.path();
    for dir in [
        "home/proj",
        "home/.codex",
        "cfg/paddock/harness.d",
        "run",
        "tools/codex/bin",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::set_permissions(root.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let exe = root.join("tools/codex/bin/codex");
    let fixture = include_str!("fixtures/horse/codex-tui").replace(
        "if [ \"${1:-}\" = \"--version\" ]; then",
        "if [ \"${1:-}\" = \"--help\" ]; then echo --dangerously-bypass-approvals-and-sandbox; exit 0; fi\nif [ \"${1:-}\" = \"--version\" ]; then",
    );
    std::fs::write(&exe, fixture).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "backend = \"bwrap\"\n[network]\nmode = \"none\"\n[horse]\nenabled = true\n",
    )
    .unwrap();
    let profile = include_str!("../profiles/codex.toml")
        .replace("mise = \"codex\"", "")
        .replace(
            "commands = [\"codex\"]",
            &format!(
                "commands = [{}]",
                serde_json::to_string(exe.to_str().unwrap()).unwrap()
            ),
        );
    std::fs::write(root.join("cfg/paddock/harness.d/codex.toml"), profile).unwrap();
    let command = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("cfg"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("MISE_OFFLINE", "1")
            .current_dir(root.join("home/proj"));
        c
    };
    let (mut master, mut slave) = (0, 0);
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    let file = |fd| unsafe { std::fs::File::from_raw_fd(libc::dup(fd)) };
    let mut cmd = command();
    cmd.args([
        "run",
        "codex",
        "--horse",
        "--backend",
        "bwrap",
        "--network",
        "none",
        "--env",
        "clean",
        "-y",
    ])
    .stdin(Stdio::from(file(slave)))
    .stdout(Stdio::from(file(slave)))
    .stderr(Stdio::from(file(slave)));
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    unsafe {
        libc::close(slave);
    }
    let mut reader = file(master);
    let flags = unsafe { libc::fcntl(master, libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let terminal = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = terminal.clone();
    let reader_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = reader_stop.clone();
    let read = std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => captured.lock().unwrap().extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let mail = root.join("state/paddock/mail");
    let id = loop {
        let found = std::fs::read_dir(&mail)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .find_map(|e| {
                let info: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(e.path().join("run.json")).ok()?).ok()?;
                (info["delivery"] == "relay").then(|| info["id"].as_str().unwrap().to_owned())
            });
        if let Some(id) = found {
            break id;
        }
        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!(
                "no relay mailbox; terminal: {}",
                String::from_utf8_lossy(&terminal.lock().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut sender = command()
        .args(["mail", "send", &id, "sandbox relay message"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let send_deadline = Instant::now() + Duration::from_secs(20);
    while sender.try_wait().unwrap().is_none() {
        if Instant::now() >= send_deadline {
            let _ = sender.kill();
            let _ = child.kill();
            panic!(
                "mail send did not finish; terminal: {}",
                String::from_utf8_lossy(&terminal.lock().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = sender.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    loop {
        let up = std::fs::read_to_string(mail.join(&id).join("up.jsonl")).unwrap_or_default();
        if up.contains("typed at the idle prompt") {
            assert_eq!(up.matches("typed at the idle prompt").count(), 1);
            break;
        }
        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!(
                "no relay delivery: {up}; terminal: {}",
                String::from_utf8_lossy(&terminal.lock().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    loop {
        let log = std::fs::read_to_string(root.join("home/proj/tui.log")).unwrap_or_default();
        if log.contains("sandbox relay message") {
            break;
        }
        assert!(
            Instant::now() < deadline && child.try_wait().unwrap().is_none(),
            "message did not reach TUI: {log}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::write(root.join("home/proj/done"), "").unwrap();
    while child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(child.wait().unwrap().success());
    reader_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    read.join().unwrap();
    unsafe {
        libc::close(master);
    }
}
