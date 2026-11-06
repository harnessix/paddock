//! The device sandbox profile under `sandbox-exec`, on a Mac: a shell
//! script runs with the rendered profile and the rendered environment of
//! a device table, without the supervisor. Temporary folders only: the
//! "real home" is a temporary folder too, and the sockets are listeners
//! of this test. The supervisor, the credentials and the host commands
//! run in tests/seatbelt_devices.rs.

use super::*;
use crate::devices::sandbox::creds;
use crate::devices::DevicesCfg;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

struct World {
    _t: tempfile::TempDir,
    env: HostEnv,
    ctx: InstanceCtx,
    def: DeviceDef,
    plan: CredPlan,
    sockets: Sockets,
    /// The stand-in for the real home: nothing in it is allowed.
    home: PathBuf,
    /// The caller's project.
    project: PathBuf,
    /// The folder of the device scripts (`ro`).
    dev: PathBuf,
    /// A folder the table names in `rw`.
    out: PathBuf,
}

/// `sandbox` is the `[devices.d.sandbox]` table; `{dev}`, `{out}` and
/// `{home}` are replaced.
fn world(sandbox: &str) -> World {
    let t = tempfile::tempdir().unwrap();
    let root = real(t.path());
    let home = root.join("home");
    let project = root.join("project");
    let dev = root.join("dev");
    let out = root.join("out");
    let cfg = root.join("cfg");
    for d in [
        &home,
        &project,
        &dev,
        &out,
        &cfg.join("paddock"),
        &home.join(".ssh"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(home.join("secret.txt"), "secret").unwrap();
    std::fs::write(home.join(".ssh/id_test"), "PRIVATE KEY").unwrap();
    std::fs::write(home.join(".ssh/id_test.pub"), "ssh-ed25519 AAAA test").unwrap();
    std::fs::write(project.join("hello.txt"), "hello from the caller\n").unwrap();
    std::fs::write(dev.join("note.txt"), "note-7f3a\n").unwrap();
    let sandbox = sandbox
        .replace("{dev}", &dev.display().to_string())
        .replace("{out}", &out.display().to_string())
        .replace("{home}", &home.display().to_string());
    let config = format!(
        "[credentials.k]\nvia = \"agent\"\nkeys = [\"{home}/.ssh/id_test.pub\"]\n\n[credentials.tok]\nvia = \"deliver\"\nas = \"file\"\nvar = \"TEST_TOKEN\"\n\n[devices.d]\nshape = \"exec\"\ncommand = [\"{dev}/d.sh\"]\n[devices.d.sandbox]\n{sandbox}\n",
        home = home.display(),
        dev = dev.display()
    );
    std::fs::write(cfg.join("paddock/config.toml"), &config).unwrap();
    // The real ids: the sockets go into /tmp/paddock-<uid>.
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut vars = BTreeMap::new();
    vars.insert("XDG_CONFIG_HOME".to_string(), cfg.display().to_string());
    vars.insert(
        "XDG_RUNTIME_DIR".to_string(),
        root.join("run").display().to_string(),
    );
    let env = HostEnv::from_parts(home.clone(), "u".into(), uid, gid, vars);
    let def = DevicesCfg::parse(&config).unwrap().user.remove(0);
    let caller = "fake-abc123";
    let ctx = InstanceCtx {
        caller: caller.into(),
        project: project.clone(),
        harness: "fake".into(),
        chain: Vec::new(),
        dir: crate::devices::sandbox::state::instance_dir(&env, &def.name, caller),
    };
    for d in ["home", "tmp", "secret"] {
        std::fs::create_dir_all(ctx.dir.join(d)).unwrap();
    }
    std::fs::write(ctx.dir.join("secret/tok"), "tok-value").unwrap();
    std::fs::write(ctx.dir.join("private.txt"), "instance").unwrap();
    let plan = creds::plan(&env, &def.name, &def.profile().credentials).unwrap();
    let sockets = Sockets::of(&env, &def, &ctx).unwrap();
    World {
        _t: t,
        env,
        ctx,
        def,
        plan,
        sockets,
        home,
        project,
        dev,
        out,
    }
}

impl Drop for World {
    fn drop(&mut self) {
        remove_sockets(
            &self.env,
            &sspec::sandbox_id(&self.def.name, &self.ctx.caller),
        );
    }
}

/// Run `script` with the profile and the environment of the device
/// sandbox. The exit code and the output.
fn sandboxed(w: &World, script: &str) -> (i32, String) {
    let paddock = std::env::current_exe().unwrap();
    let spec = sspec::spec(&w.env, &w.def, &w.ctx, &w.plan, &paddock).unwrap();
    let pol = policy(&w.env, &w.def, &w.ctx, &w.plan, &w.sockets, &paddock).unwrap();
    let profile = w.ctx.dir.join("seatbelt.sb");
    std::fs::write(&profile, crate::seatbelt::render(&pol).unwrap()).unwrap();
    let out = Command::new(SANDBOX_EXEC)
        .arg("-f")
        .arg(&profile)
        .args(["/bin/sh", "-c", script])
        .env_clear()
        .envs(run_env(&w.env, &spec, &w.ctx, &w.plan, &w.sockets))
        .current_dir(w.ctx.dir.join("home"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn ok(w: &World, script: &str) -> bool {
    sandboxed(w, script).0 == 0
}

const PLAIN: &str = "ro = [\"{dev}\"]";

#[test]
fn the_program_sees_the_folders_of_the_table_and_nothing_else() {
    let w = world(
        "ro = [\"{dev}\"]\nrw = [\"{out}\"]\ncaller = [\"project:ro\"]\ncredentials = [\"tok\"]",
    );
    let (dev, out, project, home) = (&w.dev, &w.out, &w.project, &w.home);
    // ro: read, no write.
    assert!(ok(
        &w,
        &format!("test \"$(cat {}/note.txt)\" = note-7f3a", dev.display())
    ));
    assert!(!ok(&w, &format!("touch {}/w", dev.display())));
    // rw: read and write.
    assert!(ok(
        &w,
        &format!(
            "echo y > {}/y.txt && test -f {}/y.txt",
            out.display(),
            out.display()
        )
    ));
    // caller project:ro: read, no write, and the error is a permission one.
    assert!(ok(
        &w,
        &format!("grep -q hello {}/hello.txt", project.display())
    ));
    let (code, text) = sandboxed(&w, &format!("touch {}/written 2>&1", project.display()));
    assert_ne!(code, 0);
    assert!(text.contains("Operation not permitted"), "{text}");
    assert!(!project.join("written").exists());
    // The real home: denied, the name visible.
    assert!(!ok(&w, &format!("cat {}/secret.txt", home.display())));
    assert!(!ok(&w, &format!("ls {}", home.display())));
    assert!(ok(&w, &format!("test -e {}/secret.txt", home.display())));
    // The private home and TMPDIR: writable, HOME is the private one.
    assert!(ok(
        &w,
        "test \"$HOME\" = \"$(pwd)\" && echo x > \"$HOME/x\" && test -f \"$HOME/x\""
    ));
    assert!(ok(
        &w,
        "mkdir \"$TMPDIR/req-1\" && echo x > \"$TMPDIR/req-1/f\""
    ));
    assert!(ok(
        &w,
        &format!("test \"$TMPDIR\" = {}/tmp", w.ctx.dir.display())
    ));
    // The instance folder itself: not readable, not writable.
    assert!(!ok(&w, &format!("cat {}/private.txt", w.ctx.dir.display())));
    assert!(!ok(&w, &format!("ls {}", w.ctx.dir.display())));
    assert!(!ok(&w, &format!("touch {}/new", w.ctx.dir.display())));
    // The secret folder: read, no write, no replace.
    assert!(ok(
        &w,
        "test \"$(cat \"$PADDOCK_SECRET_TOK\")\" = tok-value"
    ));
    assert!(!ok(&w, "rm \"$PADDOCK_SECRET_TOK\""));
    assert!(w.ctx.dir.join("secret/tok").exists());
    // The shared /tmp and the environment: no proxy, dumb terminal.
    assert!(!ok(&w, "touch /tmp/paddock-device-test-escape"));
    let (_, env) = sandboxed(&w, "env");
    assert!(env.contains("PADDOCK_DEVICE=d\n"), "{env}");
    assert!(env.contains("TERM=dumb\n"), "{env}");
    assert!(!env.contains("PROXY"), "{env}");
    assert!(!env.contains("XDG_RUNTIME_DIR"), "{env}");
}

#[test]
fn no_network_without_connect_and_the_port_of_connect_with_it() {
    let server = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in server.incoming() {});
    let nc = format!("/usr/bin/nc -z -G 2 127.0.0.1 {port}");
    let w = world(PLAIN);
    assert!(!ok(&w, &nc), "a direct connection worked without connect");
    // connect: the port of the entry is open on localhost (the supervisor
    // listens there), nothing else.
    let w = world(&format!(
        "ro = [\"{{dev}}\"]\nconnect = [\"{port}:mac.example:22\"]"
    ));
    assert!(
        ok(&w, &nc),
        "127.0.0.1:{port} is not reachable with connect"
    );
    let other = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let other_port = other.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in other.incoming() {});
    assert!(!ok(
        &w,
        &format!("/usr/bin/nc -z -G 2 127.0.0.1 {other_port}")
    ));
    assert!(!ok(&w, "/usr/bin/nc -z -G 2 1.1.1.1 443"));
    // The supervisor can bind a localhost port inside.
    assert!(ok(
        &w,
        "/usr/bin/python3 -c 'import socket; s=socket.socket(); s.bind((\"127.0.0.1\", 0)); s.listen(1); print(\"BOUND\")'"
    ));
}

/// A listener at `path` that accepts in a thread until the test ends.
fn listen(path: &Path) -> UnixListener {
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path).unwrap();
    let a = l.try_clone().unwrap();
    std::thread::spawn(move || for _ in a.incoming() {});
    l
}

fn unix_connect(path: &Path) -> String {
    format!(
        "/usr/bin/python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect(\"{}\"); print(\"CONNECTED\")'",
        path.display()
    )
}

#[test]
fn the_agent_and_connect_sockets_are_reachable_and_the_key_file_is_not() {
    let w = world("ro = [\"{dev}\"]\nconnect = [\"2222:mac.example:22\"]\ncredentials = [\"k\"]");
    std::fs::create_dir_all(w.sockets.connect()).unwrap();
    let _agent = listen(&w.sockets.agent());
    let _relay = listen(&w.sockets.connect().join("0.sock"));
    let (_, env) = sandboxed(&w, "env");
    assert!(
        env.contains(&format!("SSH_AUTH_SOCK={}\n", w.sockets.agent().display())),
        "{env}"
    );
    let (code, text) = sandboxed(&w, &unix_connect(&w.sockets.agent()));
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("CONNECTED"), "{text}");
    assert!(ok(&w, &unix_connect(&w.sockets.connect().join("0.sock"))));
    // No replace, no remove of the agent socket; nothing else in the
    // host folder is reachable.
    assert!(!ok(&w, &format!("rm {}", w.sockets.agent().display())));
    assert!(w.sockets.agent().exists());
    let _other = listen(&w.sockets.host.join("other.sock"));
    assert!(!ok(&w, &unix_connect(&w.sockets.host.join("other.sock"))));
    // The private key of the credential stays in the real home: denied.
    assert!(!ok(&w, &format!("cat {}/.ssh/id_test", w.home.display())));
    assert!(!ok(
        &w,
        &format!("cat {}/.ssh/id_test.pub", w.home.display())
    ));
    // Without the credential, the agent socket is not reachable.
    let w2 = world(PLAIN);
    let _agent2 = listen(&w2.sockets.agent());
    let (_, env) = sandboxed(&w2, "env");
    assert!(!env.contains("SSH_AUTH_SOCK"), "{env}");
    assert!(!ok(&w2, &unix_connect(&w2.sockets.agent())));
}

#[test]
fn the_supervisor_can_create_its_socket_and_the_program_cannot_touch_the_host_side() {
    let w = world(PLAIN);
    let sock = w.sockets.supervisor.display().to_string();
    assert!(ok(&w, &format!(
        "/usr/bin/python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.bind(\"{sock}\"); print(\"LISTENING\")'"
    )));
    assert!(w.sockets.supervisor.exists());
    assert!(!ok(&w, &format!("touch {}/x", w.sockets.host.display())));
    assert!(!ok(&w, &format!("ls {}", w.sockets.host.display())));
}

#[test]
fn sandbox_exec_accepts_the_profile_of_every_table_shape() {
    for sandbox in [
        PLAIN,
        "ro = [\"{dev}\"]\nhome = \"read-only\"",
        "ro = [\"{dev}\"]\nhome = \"full\"",
        "ro = [\"{dev}\"]\nconnect = [\"2222:mac.example:22\", \"2223:other.example:22\"]\ncredentials = [\"k\", \"tok\"]",
        "ro = [\"{dev}\", \"{dev}/note.txt\"]\nrw = [\"{out}\"]\ncaller = [\"project:ro\"]",
    ] {
        let w = world(sandbox);
        let (code, text) = sandboxed(&w, "echo PROFILE_OK");
        assert_eq!(code, 0, "{sandbox}:\n{text}");
        assert!(text.contains("PROFILE_OK"), "{sandbox}:\n{text}");
    }
    let w = world("ro = [\"{dev}\"]\nhome = \"read-only\"");
    assert!(ok(&w, &format!("cat {}/secret.txt", w.home.display())));
    assert!(!ok(&w, &format!("touch {}/w", w.home.display())));
    let w = world("ro = [\"{dev}\"]\nhome = \"full\"");
    assert!(ok(&w, &format!("touch {}/w", w.home.display())));
}
