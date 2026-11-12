//! Tests of the terminal strings in the input (legacy Alt keys that
//! start like a reply, CAN and SUB, a string longer than the limit) and
//! of the retry after a failed subscribe.

use super::super::stream::{RETRY_MAX_MS, RETRY_MS};
use super::*;
use std::sync::mpsc::Receiver;

fn alt_shift() -> Mods {
    Mods { shift: true, ..ALT }
}

#[test]
fn alt_with_a_string_introducer_and_a_key_soon_after_reach_the_guest() {
    // Alt-] and then an arrow within the Escape wait: one read.
    assert_eq!(
        parse(b"\x1b]\x1b[A"),
        vec![k("]", ALT), k("up", Mods::NONE)]
    );
    // Alt-Shift-P and then Alt-x.
    assert_eq!(parse(b"\x1bP\x1bx"), vec![k("p", alt_shift()), k("x", ALT)]);
    // Alt-_, a letter, then Return: Return is in no reply.
    assert_eq!(
        parse(b"\x1b_a\r"),
        vec![
            k("_", ALT),
            Event::Text("a".into()),
            k("return", Mods::NONE)
        ]
    );
    // Alt-Shift-X, then Backspace (DEL) and Tab.
    assert_eq!(
        parse(b"\x1bX\x7f\t"),
        vec![
            k("x", alt_shift()),
            k("delete", Mods::NONE),
            k("tab", Mods::NONE)
        ]
    );
    // Alt-^, a letter, then Escape alone at the end of the input.
    assert_eq!(
        parse(b"\x1b^b\x1b"),
        vec![
            k("^", ALT),
            Event::Text("b".into()),
            k("escape", Mods::NONE)
        ]
    );
    // Split across reads: the ESC after the introducer waits for its
    // next byte, which says that it is not ST.
    let mut p = Parser::new(false);
    assert_eq!(p.feed(b"\x1b]a\x1b"), vec![]);
    assert!(p.pending());
    assert_eq!(
        p.feed(b"[B"),
        vec![k("]", ALT), Event::Text("a".into()), k("down", Mods::NONE)]
    );
    assert!(!p.pending());
}

#[test]
fn replies_with_st_or_bel_are_still_dropped_whole() {
    assert_eq!(
        parse(b"\x1b]11;rgb:0000/0000/0000\x1b\\\x1b_Gi=31;OK\x07z"),
        vec![Event::Text("z".into())]
    );
    // ST split between two reads.
    let mut p = Parser::new(false);
    assert_eq!(p.feed(b"\x1bP>|kitty(0.36)\x1b"), vec![]);
    assert_eq!(p.feed(b"\\q"), vec![Event::Text("q".into())]);
}

#[test]
fn can_and_sub_cancel_a_string() {
    assert_eq!(parse(b"\x1b]52;c;AAAA\x18b"), vec![Event::Text("b".into())]);
    assert_eq!(parse(b"\x1b_Gi=31;O\x1ac"), vec![Event::Text("c".into())]);
    // Split: the string waits, then SUB ends it.
    let mut p = Parser::new(false);
    assert_eq!(p.feed(b"\x1bPabc"), vec![]);
    assert_eq!(p.feed(b"def\x1ag"), vec![Event::Text("g".into())]);
}

#[test]
fn a_string_longer_than_the_limit_is_dropped_up_to_its_end() {
    let body = vec![b'A'; 70_000];
    // One read: nothing of the string becomes keys.
    let mut one = b"\x1b_G".to_vec();
    one.extend_from_slice(&body);
    one.extend_from_slice(b"\x1b\\x");
    assert_eq!(parse(&one), vec![Event::Text("x".into())]);
    // Many reads, and the end after the limit.
    let mut p = Parser::new(false);
    assert_eq!(p.feed(b"\x1b]1337;"), vec![]);
    for chunk in body.chunks(4096) {
        assert_eq!(p.feed(chunk), vec![]);
    }
    assert!(p.pending(), "the rest of the string is still dropped");
    assert_eq!(p.feed(b"AAAA\x1b"), vec![]);
    assert_eq!(p.feed(b"\\y"), vec![Event::Text("y".into())]);
    assert!(!p.pending());
    // CAN and BEL end the rest too.
    let mut p = Parser::new(false);
    p.feed(b"\x1bP");
    for chunk in body.chunks(4096) {
        p.feed(chunk);
    }
    assert_eq!(p.feed(b"BB\x18z"), vec![Event::Text("z".into())]);
    let mut p = Parser::new(false);
    p.feed(b"\x1bP");
    p.feed(&body);
    assert_eq!(p.feed(b"BB\x07w"), vec![Event::Text("w".into())]);
    // A key typed after a broken long string still reaches the guest:
    // an escape sequence, or anything after a quiet input.
    let mut p = Parser::new(false);
    p.feed(b"\x1b_");
    p.feed(&body);
    assert_eq!(p.feed(b"BB\x1b[A"), vec![k("up", Mods::NONE)]);
    let mut p = Parser::new(false);
    p.feed(b"\x1b_");
    p.feed(&body);
    assert_eq!(p.idle(), vec![]);
    assert!(!p.pending());
    assert_eq!(p.feed(b"v"), vec![Event::Text("v".into())]);
}

/// A source whose subscribe fails while `fail` is true.
struct Flaky {
    fail: bool,
    tries: Vec<Max>,
    keep: Vec<std::sync::mpsc::Sender<Item>>,
}

impl FrameSource for Flaky {
    fn subscribe(&mut self, _: &Target, _: u32, max: Max) -> Result<Receiver<Item>, String> {
        self.tries.push(max);
        if self.fail {
            return Err("the guest is busy".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.keep.push(tx);
        Ok(rx)
    }

    fn stop(&mut self) {}
}

#[test]
fn a_failed_subscribe_backs_off_and_keeps_the_old_stream() {
    let mut src = Flaky {
        fail: false,
        tries: vec![],
        keep: vec![],
    };
    let t = Target::Desktop {
        app: None,
        instance: None,
    };
    let small = Max {
        width: 80,
        height: 40,
    };
    let big = Max {
        width: 160,
        height: 80,
    };
    let mut stream = Stream::start(&mut src, &t, 30, small).unwrap();
    src.keep[0].send(Item::End("old".into())).unwrap();
    src.fail = true;
    // The view asks every 50 ms here. The new room settles, then fails.
    let mut errors = Vec::new();
    let mut now = 0;
    while now <= 120_000 {
        if let Err(e) = stream.room(&mut src, big, now) {
            errors.push((now, e));
        }
        now += 50;
    }
    // The tries: after the settle, then 1 s, 2 s, 4 s ... up to 30 s apart.
    let times: Vec<u64> = errors.iter().map(|(t, _)| *t).collect();
    assert_eq!(times[0], SETTLE_MS);
    let gaps: Vec<u64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    let mut want = Vec::new();
    let mut w = RETRY_MS;
    for _ in 0..gaps.len() {
        want.push(w);
        w = (w * 2).min(RETRY_MAX_MS);
    }
    assert_eq!(gaps, want, "{times:?}");
    assert!(times.len() < 12, "{} tries in two minutes", times.len());
    assert_eq!(src.tries.len(), 1 + times.len());
    assert!(errors.iter().all(|(_, e)| e == "the guest is busy"));
    // The old stream stays.
    assert_eq!(stream.max(), small);
    assert!(matches!(stream.frames().try_recv(), Ok(Item::End(w)) if w == "old"));
    // The guest recovers: the next try subscribes.
    src.fail = false;
    let next = times.last().unwrap() + RETRY_MAX_MS;
    assert!(!stream.room(&mut src, big, next - 100).unwrap());
    assert!(stream.room(&mut src, big, next).unwrap());
    assert_eq!(stream.max(), big);
    // A later failure starts again at the first wait.
    src.fail = true;
    let other = Max {
        width: 40,
        height: 20,
    };
    let base = next + 1000;
    assert!(!stream.room(&mut src, other, base).unwrap());
    assert!(stream.room(&mut src, other, base + SETTLE_MS).is_err());
    assert!(!stream
        .room(&mut src, other, base + SETTLE_MS + RETRY_MS - 1)
        .unwrap());
    assert!(stream
        .room(&mut src, other, base + SETTLE_MS + RETRY_MS)
        .is_err());
    // Back to the room of the stream: no try, and no retry left.
    let tries = src.tries.len();
    assert!(!stream.room(&mut src, big, base + 60_000).unwrap());
    assert_eq!(src.tries.len(), tries);
}
