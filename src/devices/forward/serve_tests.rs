//! The ssh transport and the standard-input side of `serve-remote`, with
//! the two-home helpers of forward_tests.rs.

use super::serve::{self, serve_stdio};
use super::tests::{call, forward_config, sock_path, upper_config, Home};
use super::*;
use crate::broker::Answer;
use crate::devices::supervise::tests::script;
use crate::devices::DevicesCfg;
use crate::env::HostEnv;
use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// A fake `ssh` on the PATH: it records its arguments and bridges its
/// standard input and output to the socket `sock`.
fn fake_ssh(home: &Home, sock: &Path) -> PathBuf {
    let argv = home.dir.join("ssh-argv");
    let body = format!(
        r#"printf '%s\n' "$@" > {argv}
exec /usr/bin/python3 -I -c '
import os, socket, sys, threading
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
def pump():
    while True:
        b = sys.stdin.buffer.read1(65536)
        if not b:
            break
        s.sendall(b)
    s.shutdown(socket.SHUT_WR)
threading.Thread(target=pump, daemon=True).start()
while True:
    b = s.recv(65536)
    if not b:
        break
    sys.stdout.buffer.write(b)
    sys.stdout.buffer.flush()
# The pump thread may still block in read1 and hold the stdin lock: a
# normal shutdown would abort (Fatal Python error) and dump core.
sys.stdout.flush()
os._exit(0)
' {sock}"#,
        argv = argv.display(),
        sock = sock.display()
    );
    script(&home.dir.join("bin"), "ssh", &body);
    argv
}

#[test]
fn the_ssh_transport_runs_ssh_as_the_user_and_the_card_names_the_host() {
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("skipped: no /usr/bin/python3");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let serving = Home::new(
        t.path(),
        "serving",
        &upper_config("grant = [\"remote:any\"]\n", ""),
        Answer::Session,
    );
    let (_guard, _inst) = serving.serve(&sock);
    let forwarding = Home::new(
        t.path(),
        "forwarding",
        &forward_config("ssh://me@mac.local:2222", ""),
        Answer::Session,
    );
    let argv = fake_ssh(&forwarding, &sock);
    let f = forwarding.forward("mac-upper");
    let out = f.call(&call(), b"abc").unwrap();
    assert_eq!(out, b"status: exit 0\n\nABC");
    assert_eq!(
        std::fs::read_to_string(&argv).unwrap(),
        "-T\n-o\nBatchMode=yes\n--\nssh://me@mac.local:2222\npaddock\ndevice\nserve-remote\n"
    );
    let cards = forwarding.cards();
    assert_eq!(
        cards[0].summary,
        "allow the device `mac-upper` for this sandbox: it connects to me@mac.local:2222 with your ssh as you"
    );
    assert_eq!(
        cards[0].detail,
        "each request runs `ssh me@mac.local:2222 paddock device serve-remote` ON THE HOST as you, with your ssh configuration and keys, and the paddock on me@mac.local:2222 runs its device `upper` there, in its own device sandbox, with its own card for you"
    );
    let line: serde_json::Value =
        serde_json::from_str(forwarding.audit().lines().next().unwrap()).unwrap();
    assert_eq!(line["transport"], "ssh");
    assert_eq!(line["target"], "ssh://me@mac.local:2222");
    // The catalog text carries the host mark, before and after the
    // catalog of the other side is known.
    assert!(f
        .info()
        .request
        .starts_with("Connects to me@mac.local:2222 with your ssh as you. "));
    let t0 = Instant::now();
    while f.remote_entry().is_none() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f.remote_entry().is_some());
    assert!(f
        .info()
        .request
        .starts_with("Connects to me@mac.local:2222 with your ssh as you. Forwarded to"));
    // A host without ssh: EIO with the text in the catalog.
    let (_, _) = (
        std::fs::remove_file(forwarding.dir.join("bin/ssh")),
        std::fs::remove_file(&argv),
    );
    let mut vars = forwarding.env.vars.clone();
    vars.insert(
        "PATH".into(),
        forwarding.dir.join("bin").display().to_string(),
    );
    let env = HostEnv::from_parts(forwarding.env.home.clone(), "demo".into(), 1000, 1000, vars);
    let mut ctx = forwarding.ctx();
    ctx.env = env;
    let def = DevicesCfg::load(&forwarding.env).unwrap().user.remove(0);
    let f = Forward::new(def, ctx, GrantHit::NoKey);
    assert_eq!(f.call(&call(), b"abc").unwrap_err(), DevError::Failed);
    let d = f.details().unwrap();
    assert_eq!(d["last_error"], "ssh is not on the PATH of the host");
    assert!(d["how_to_fix"]
        .as_str()
        .unwrap()
        .contains("paddock device check mac-upper"));
}

#[test]
fn serve_remote_on_the_standard_input_answers_one_request_and_names_the_ssh_peer() {
    let t = tempfile::tempdir().unwrap();
    let serving = Home::new(
        t.path(),
        "serving",
        &upper_config("grant = [\"remote:10.0.0.7\"]\n", ""),
        Answer::Session,
    );
    let mut env = serving.env.clone();
    env.vars
        .insert("SSH_CONNECTION".into(), "10.0.0.7 51234 10.0.0.2 22".into());
    assert_eq!(serve::peer_of(&env), "10.0.0.7");
    assert_eq!(serve::peer_of(&serving.env), "local");
    // The request as the forwarding side sends it, on a socket pair that
    // stands for the pipes of ssh.
    let (mut client, server_in) = UnixStream::pair().unwrap();
    let (server_out, mut answer) = UnixStream::pair().unwrap();
    let req = Request::call("upper", 3).with_caller(&RemoteCaller::new(
        "box9",
        "linuxbox",
        &["root9".to_string()],
    ));
    let mut line = serde_json::to_vec(&req).unwrap();
    line.extend_from_slice(b"\nabc");
    client.write_all(&line).unwrap();
    let approver = serving.approver.clone();
    let worker =
        std::thread::spawn(move || serve_stdio(&env, "10.0.0.7", approver, server_in, server_out));
    let mut got = Vec::new();
    std::io::Read::read_to_end(&mut answer, &mut got).unwrap();
    let text = String::from_utf8_lossy(&got);
    assert!(
        text.starts_with("{\"v\":1,\"ok\":true,\"size\":19}\nstatus: exit 0\n\nABC"),
        "{text}"
    );
    drop(client);
    worker.join().unwrap().unwrap();
    let cards = serving.cards();
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].summary,
        "allow the device `upper` for a remote caller from 10.0.0.7 (sandbox box9 on linuxbox, started by root9)"
    );
    let line: serde_json::Value =
        serde_json::from_str(serving.audit().lines().next().unwrap()).unwrap();
    assert_eq!(line["sandbox"], "remote:10.0.0.7");
    assert_eq!(line["grant"], "remote 10.0.0.7");
    assert_eq!(line["remote_caller"], "box9");
    assert_eq!(line["remote_host"], "linuxbox");
    // The session answer of the peer holds for the next process: the
    // same request again shows no card.
    assert_eq!(session::load(&serving.env, "10.0.0.7", "upper"), Some(true));
    let (mut client, server_in) = UnixStream::pair().unwrap();
    let (server_out, mut answer) = UnixStream::pair().unwrap();
    client.write_all(&line_of(&req, b"abc")).unwrap();
    let approver = serving.approver.clone();
    let mut env = serving.env.clone();
    env.vars
        .insert("SSH_CONNECTION".into(), "10.0.0.7 5 10.0.0.2 22".into());
    let worker =
        std::thread::spawn(move || serve_stdio(&env, "10.0.0.7", approver, server_in, server_out));
    let mut got = Vec::new();
    std::io::Read::read_to_end(&mut answer, &mut got).unwrap();
    assert!(got.ends_with(b"ABC"), "{}", String::from_utf8_lossy(&got));
    drop(client);
    worker.join().unwrap().unwrap();
    assert_eq!(serving.cards().len(), 1, "no second card");
    // Another peer has no session: the grant does not cover it either.
    let (mut client, server_in) = UnixStream::pair().unwrap();
    let (server_out, mut answer) = UnixStream::pair().unwrap();
    client.write_all(&line_of(&req, b"abc")).unwrap();
    let approver = serving.approver.clone();
    let env = serving.env.clone();
    let worker =
        std::thread::spawn(move || serve_stdio(&env, "10.0.0.8", approver, server_in, server_out));
    let mut got = Vec::new();
    std::io::Read::read_to_end(&mut answer, &mut got).unwrap();
    let text = String::from_utf8_lossy(&got);
    assert!(text.contains("status: disabled"), "{text}");
    assert!(
        text.contains("The device upper is not granted to remote callers: the user on this machine grants it to remote callers with grant = [\"remote:10.0.0.8\"] or grant = [\"remote:any\"] in [devices.upper]."),
        "{text}"
    );
    drop(client);
    worker.join().unwrap().unwrap();
    // No secret file for a remote caller.
    let (mut client, server_in) = UnixStream::pair().unwrap();
    let (server_out, mut answer) = UnixStream::pair().unwrap();
    client
        .write_all(&line_of(&Request::secret("GH_TOKEN"), b""))
        .unwrap();
    let approver = serving.approver.clone();
    let env = serving.env.clone();
    let worker =
        std::thread::spawn(move || serve_stdio(&env, "10.0.0.7", approver, server_in, server_out));
    let mut got = Vec::new();
    std::io::Read::read_to_end(&mut answer, &mut got).unwrap();
    let reply: socket::Reply =
        serde_json::from_slice(got.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(reply.errno, Some(libc::EACCES));
    drop(client);
    worker.join().unwrap().unwrap();
}

fn line_of(req: &Request, body: &[u8]) -> Vec<u8> {
    let mut line = serde_json::to_vec(req).unwrap();
    line.push(b'\n');
    line.extend_from_slice(body);
    line
}
