//! The idle test, the keys of a delivery and the relay tap, over a
//! scripted screen and a recording writer in place of the pty.

use super::*;
use std::sync::mpsc;

/// The input row of the Codex TUI: the prompt, then a dim placeholder.
pub(crate) const IDLE: &[u8] =
    b"\x1b[2J\x1b[H\xe2\x80\xba \x1b[2mAsk Codex to do anything\x1b[0m\r\n\r\n  100% context left";
/// A turn runs: the status row, and the input row below it.
pub(crate) const BUSY: &[u8] = b"\x1b[2J\x1b[H\xe2\x80\xa2 Working (3s \xe2\x80\xa2 esc to interrupt)\r\n\r\n\xe2\x80\xba \x1b[2mAsk Codex to do anything\x1b[0m";
/// The user typed a draft into the input row.
const DRAFT: &[u8] = b"\x1b[2J\x1b[H\xe2\x80\xba fix the bug";

pub(crate) fn codex_idle() -> PtyIdle {
    let p = crate::profile::builtin("codex").unwrap();
    PtyIdle::of(&p, "codex", crate::horse::codex::VERSION).unwrap()
}

fn screen(b: &[u8]) -> Screen {
    Screen::from_bytes(24, 80, b)
}

#[test]
fn the_codex_profile_has_the_table_and_another_version_refuses() {
    let p = crate::profile::builtin("codex").unwrap();
    let idle = codex_idle();
    assert_eq!(idle.prompt, "›");
    assert!(idle.empty_input && idle.bracketed_paste);
    assert_eq!(idle.interrupt_key, b"\x1b");
    assert_eq!(idle.submit_key, b"\r");
    // The rollout codec and the screen pattern have separate compatibility gates.
    crate::horse::codex::check_version("0.167.0").unwrap();
    assert!(crate::horse::codex::check_version("0.168.0").is_err());
    let e = PtyIdle::of(&p, "codex", "0.167.0").unwrap_err();
    assert_eq!(
        format!("{e:#}"),
        "horse: no pty delivery pattern for codex 0.167.0"
    );
    // A profile without the table refuses too.
    let gemini = crate::profile::builtin("gemini").unwrap();
    let e = PtyIdle::of(&gemini, "gemini", "1.0.0").unwrap_err();
    assert_eq!(
        format!("{e:#}"),
        "horse: no pty delivery pattern for gemini 1.0.0"
    );
    let mut t = p.horse.unwrap().pty.unwrap();
    t.interrupt_key = "ctrl-q".into();
    assert!(PtyIdle::from_table(&t).is_err());
}

#[test]
fn idle_needs_the_prompt_with_no_input_no_busy_row_and_a_quiet_screen() {
    let p = codex_idle();
    let quiet = p.quiet;
    assert!(is_idle(&screen(IDLE), &p, quiet));
    // Each part alone fails.
    assert!(!is_idle(&screen(IDLE), &p, quiet / 2), "screen changed");
    assert!(!is_idle(&screen(BUSY), &p, quiet), "a busy row");
    assert!(!is_idle(&screen(DRAFT), &p, quiet), "the user typed");
    assert!(!is_idle(&screen(b"loading"), &p, quiet), "no input row");
    // The busy text is compared without case.
    let b = b"\x1b[H\xe2\x80\xba \r\nWorking (Esc to interrupt)";
    assert!(!is_idle(&screen(b), &p, quiet));
    // Without empty_input, any input row is enough.
    let mut any = p.clone();
    any.empty_input = false;
    assert!(is_idle(&screen(DRAFT), &any, quiet));
}

#[test]
fn an_entry_is_one_bracketed_paste_then_the_submit_key() {
    let p = codex_idle();
    assert_eq!(
        keys("two\nlines", &p),
        vec![b"\x1b[200~two\nlines\x1b[201~".to_vec(), b"\r".to_vec()]
    );
    // Without bracketed paste: line by line, Ctrl-J between the lines.
    let mut plain = p.clone();
    plain.bracketed_paste = false;
    assert_eq!(
        keys("a\r\nb", &plain),
        vec![b"a".to_vec(), b"\nb".to_vec(), b"\r".to_vec()]
    );
}

#[test]
fn an_entry_can_never_send_keys_of_its_own() {
    // An escape sequence would end the paste early and submit the rest.
    let evil = "hi\x1b[201~\r/quit\x07\u{9b}x";
    assert_eq!(terminal_text(evil), "hi[201~\n/quitx");
    let k = keys(evil, &codex_idle());
    assert_eq!(k[0], b"\x1b[200~hi[201~\n/quitx\x1b[201~");
    assert_eq!(k[0].iter().filter(|b| **b == 0x1b).count(), 2);
}

type Writes = Arc<Mutex<Vec<Vec<u8>>>>;

/// A writer that records each write and says when it got one.
fn recorder() -> (
    impl FnMut(&[u8]) -> io::Result<()> + Send + 'static,
    Writes,
    mpsc::Receiver<()>,
) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    let g = got.clone();
    let w = move |b: &[u8]| {
        g.lock().unwrap().push(b.to_vec());
        let _ = tx.send(());
        Ok(())
    };
    (w, got, rx)
}

#[test]
fn the_keys_of_the_user_wait_until_the_submit_key_went_out() {
    let tap = PtyTap::new();
    let (w, got, wrote) = recorder();
    tap.attach_with(24, 80, w);
    let mut p = codex_idle();
    p.submit_gap = Duration::from_millis(300);
    let t2 = tap.clone();
    let typing = std::thread::spawn(move || t2.deliver("hello", &p));
    // The paste is out; the submit key waits for its gap.
    wrote.recv().unwrap();
    let t3 = tap.clone();
    let user = std::thread::spawn(move || t3.write_user(-1, b"x"));
    typing.join().unwrap().unwrap();
    user.join().unwrap().unwrap();
    assert_eq!(
        *got.lock().unwrap(),
        vec![
            b"\x1b[200~hello\x1b[201~".to_vec(),
            b"\r".to_vec(),
            b"x".to_vec()
        ]
    );
    // Another tap that writes the user's keys goes through the same lock.
    while wrote.try_recv().is_ok() {}
    let t4 = tap.clone();
    let mut p = codex_idle();
    p.submit_gap = Duration::from_millis(300);
    let typing = std::thread::spawn(move || t4.deliver("again", &p));
    wrote.recv().unwrap();
    let t5 = tap.clone();
    let order = got.clone();
    let other = std::thread::spawn(move || {
        serialized(&t5, &mut || {
            order.lock().unwrap().push(b"y".to_vec());
            Ok(())
        })
    });
    typing.join().unwrap().unwrap();
    other.join().unwrap().unwrap();
    let all = got.lock().unwrap();
    assert_eq!(all[all.len() - 2..], [b"\r".to_vec(), b"y".to_vec()]);
}

#[test]
fn the_tap_follows_the_screen_and_fails_after_the_relay_ends() {
    let tap = PtyTap::new();
    assert!(tap.screen().is_none(), "no screen before the relay");
    assert!(tap.deliver("x", &codex_idle()).is_err());
    let (w, got, _rx) = recorder();
    tap.attach_with(24, 80, w);
    tap.output_bytes(IDLE);
    let (s, _) = tap.screen().unwrap();
    assert!(is_idle(&s, &codex_idle(), Duration::from_secs(9)));
    std::thread::sleep(Duration::from_millis(20));
    // Output that changes nothing on the screen keeps it quiet.
    tap.output_bytes(b"\x1b[?25l\x1b[?25h");
    assert!(tap.screen().unwrap().1 >= Duration::from_millis(20));
    tap.output_bytes(BUSY);
    let (s, unchanged) = tap.screen().unwrap();
    assert!(unchanged < Duration::from_millis(20));
    assert!(!is_idle(&s, &codex_idle(), Duration::from_secs(9)));
    tap.interrupt(&codex_idle()).unwrap();
    assert_eq!(got.lock().unwrap().last().unwrap(), b"\x1b");
    Tap::finish(&*tap);
    assert!(!tap.attached());
    assert!(tap.interrupt(&codex_idle()).is_err());
}

#[test]
fn mail_stays_unread_when_terminal_write_fails_and_a_hook_can_take_it() {
    let dir = tempfile::tempdir().unwrap();
    let (env, store) = super::mail_test_support::setup(dir.path());
    let horse = store.new_default_horse().unwrap();
    let session = store
        .new_session(
            &horse.id,
            crate::horse::Binding {
                harness: crate::horse::codex::HARNESS.into(),
                native_id: None,
                version: crate::horse::codex::VERSION.into(),
                container: crate::horse::codex::CONTAINER.into(),
            },
        )
        .unwrap();
    super::mail_test_support::link(&env, &session.id);
    let message = super::mail_test_support::send(&env, "mail that waits");
    let entry = store.inbox(&session.id).unwrap().pop().unwrap();
    let tap = PtyTap::with_mail(&env, super::mail_test_support::RUN);
    let profile = crate::profile::builtin("codex").unwrap();
    let idle = PtyIdle::of(&profile, "codex", crate::horse::codex::VERSION).unwrap();
    assert!(super::type_entry(&store, &tap, &idle, &entry)
        .unwrap()
        .is_none());
    assert_eq!(
        crate::mail::cursor(
            &env,
            super::mail_test_support::RUN,
            crate::mail::Dir::Down,
            crate::mail::AGENT
        ),
        0
    );
    assert_eq!(store.inbox(&session.id).unwrap().len(), 1);
    let taken = crate::mail::deliver::take(
        &env,
        super::mail_test_support::RUN,
        crate::mail::deliver::Via::Stop,
        crate::mail::deliver::MAX_HOOK_TEXT,
    )
    .unwrap();
    assert_eq!(taken.messages[0].seq, message.seq);
    assert_eq!(
        super::type_entry(&store, &tap, &idle, &entry)
            .unwrap()
            .unwrap()
            .0,
        ""
    );
    assert!(store.inbox(&session.id).unwrap().is_empty());
}

#[test]
fn a_terminal_that_stops_reading_has_a_write_deadline() {
    use std::os::fd::RawFd;
    let (mut master, mut slave): (RawFd, RawFd) = (0, 0);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let mut term = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(slave, &mut term) }, 0);
    unsafe {
        libc::cfmakeraw(&mut term);
    }
    assert_eq!(unsafe { libc::tcsetattr(slave, libc::TCSANOW, &term) }, 0);
    let flags = unsafe { libc::fcntl(master, libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let start = std::time::Instant::now();
    let err = super::write_until(
        master,
        &vec![b'x'; 1024 * 1024],
        start + Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
    unsafe {
        libc::close(master);
        libc::close(slave);
    }
}

#[test]
fn a_failed_mail_write_clears_the_partial_input() {
    let dir = tempfile::tempdir().unwrap();
    let (env, store) = super::mail_test_support::setup(dir.path());
    let horse = store.new_default_horse().unwrap();
    let session = store
        .new_session(
            &horse.id,
            crate::horse::Binding {
                harness: crate::horse::codex::HARNESS.into(),
                native_id: None,
                version: crate::horse::codex::VERSION.into(),
                container: crate::horse::codex::CONTAINER.into(),
            },
        )
        .unwrap();
    super::mail_test_support::link(&env, &session.id);
    let message = super::mail_test_support::send(&env, "partial input");
    let entry = store.inbox(&session.id).unwrap().pop().unwrap();
    let tap = PtyTap::with_mail(&env, super::mail_test_support::RUN);
    let writes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let saved = writes.clone();
    tap.attach_with(24, 80, move |b| {
        let mut writes = saved.lock().unwrap();
        writes.push(b.to_vec());
        if writes.len() == 1 {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "test timeout",
            ))
        } else {
            Ok(())
        }
    });
    let profile = crate::profile::builtin("codex").unwrap();
    let idle = PtyIdle::of(&profile, "codex", crate::horse::codex::VERSION).unwrap();
    assert!(super::type_entry(&store, &tap, &idle, &entry)
        .unwrap()
        .is_none());
    assert_eq!(writes.lock().unwrap().last().unwrap(), b"\x1b[201~\x15");
    assert_eq!(
        crate::mail::cursor(
            &env,
            super::mail_test_support::RUN,
            crate::mail::Dir::Down,
            crate::mail::AGENT
        ),
        0
    );
    assert_eq!(
        crate::mail::deliver::take(
            &env,
            super::mail_test_support::RUN,
            crate::mail::deliver::Via::Stop,
            10000
        )
        .unwrap()
        .messages[0]
            .seq,
        message.seq
    );
}
