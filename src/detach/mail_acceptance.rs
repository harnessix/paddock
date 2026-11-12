use super::tests::{ctl, env_in, quiet_session, record, until};
use super::*;
use crate::mail::{self, Dir, Kind, Message, Starter, FROM_AGENT};
use crate::view::guard::TmuxGuard;
use crate::view::Tmux;
use std::process::{Command, Stdio};

#[test]
fn a_view_keeps_a_done_run_open_until_it_closes() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path());
    let Some(tmux) = Tmux::find(&env) else { return };
    let _server = TmuxGuard::of(&tmux);
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        env.paddock_config().join("config.toml"),
        "[mail]\nend_after_done = \"2s\"\n",
    )
    .unwrap();
    let id = "mail-done-view";
    mail::create(&env, id, &Starter::host()).unwrap();
    quiet_session(&tmux, id, "printf ready; exec sleep 60");
    record(&env, id);
    let mut view = Command::new(&tmux.exe)
        .arg("-S")
        .arg(&tmux.sock)
        .args(["-C", "attach", "-t", id])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    until("view attached", Duration::from_secs(10), || {
        tmux.clients(id) > 0
    });
    let (c, stops) = ctl(&env, id);
    c.watch(Vec::new());
    mail::append(
        &env,
        id,
        Dir::Up,
        Message::new(FROM_AGENT, Kind::Status, "done: complete"),
    )
    .unwrap();
    until("done event", Duration::from_secs(10), || {
        mail::read(&env, id, Dir::Up)
            .iter()
            .any(|m| m.event.as_deref() == Some("done"))
    });
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(stops.load(Ordering::SeqCst), 0);
    let _ = view.kill();
    let _ = view.wait();
    until("done stop", Duration::from_secs(10), || {
        stops.load(Ordering::SeqCst) == 1
    });
}
