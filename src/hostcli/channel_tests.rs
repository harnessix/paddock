use super::*;
use std::collections::VecDeque;
use std::time::Instant;

fn entry(n: u32, role: &str) -> Entry {
    Entry {
        id: format!("0000{n}-abc"),
        session: "s-1".into(),
        role: role.into(),
        text: format!("text {n}"),
        queued_ms: n as u64,
        interrupt: false,
    }
}

/// A broker that answers the scripted entry lists in order, then empty
/// lists, and records the `since` of every request.
fn stub(script: Vec<Vec<Entry>>) -> (Call, Arc<Mutex<Vec<u64>>>) {
    let script = Mutex::new(VecDeque::from(script));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let call: Call = Arc::new(move |req: &Request| {
        assert_eq!(req.cap, "horse");
        assert_eq!(req.args["op"], CHANNEL_WAIT);
        assert_eq!(req.args["timeout_ms"], 25_000);
        s2.lock().unwrap().push(req.args["since"].as_u64().unwrap());
        let next = script.lock().unwrap().pop_front();
        let entries = next.unwrap_or_else(|| {
            std::thread::sleep(Duration::from_millis(5));
            Vec::new()
        });
        Ok(Response::done(json!({"entries": entries})))
    });
    (call, seen)
}

fn lines(out: &Mutex<Vec<u8>>) -> Vec<Value> {
    String::from_utf8(out.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.318"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

#[test]
fn two_released_entries_become_two_notifications_in_order() {
    for _ in 0..20 {
        let (call, seen) = stub(vec![vec![entry(1, "user"), entry(2, "note")]]);
        let (reader, mut writer) = std::io::pipe().unwrap();
        let out = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink: Arc<Mutex<dyn Write + Send>> = out.clone();
        let server = std::thread::spawn(move || {
            serve(
                std::io::BufReader::new(reader),
                sink,
                call,
                Config::default(),
            )
        });
        writeln!(writer, "{INIT}\n{INITIALIZED}").unwrap();
        wait_for("two notifications", || lines(&out).len() >= 3);
        let got = lines(&out);
        assert_eq!(got[0]["id"], 1);
        assert_eq!(got[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            got[0]["result"]["capabilities"]["experimental"]["claude/channel"],
            json!({})
        );
        for (n, (line, role)) in got[1..].iter().zip(["user", "note"]).enumerate() {
            let e = entry(n as u32 + 1, role);
            assert_eq!(line["method"], NOTIFICATION);
            assert!(line.get("id").is_none());
            assert_eq!(line["params"]["content"], e.wire_text());
            assert_eq!(
                line["params"]["meta"],
                json!({"session": "s-1", "entry_id": e.id, "role": role})
            );
        }
        assert_eq!(
            got[2]["params"]["content"],
            "<paddock-append role=\"note\">\ntext 2\n</paddock-append>"
        );
        // The next wait asks after the two entries.
        wait_for("the next wait", || seen.lock().unwrap().len() >= 2);
        assert_eq!(&seen.lock().unwrap()[..2], &[0, 2]);
        drop(writer);
        assert_eq!(server.join().unwrap(), 0);
        assert_eq!(lines(&out).len(), 3, "nothing after the end of input");
    }
}

#[test]
fn nothing_is_asked_before_the_handshake_ends() {
    let (call, seen) = stub(vec![vec![entry(1, "user")]]);
    let out = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink: Arc<Mutex<dyn Write + Send>> = out.clone();
    let input = format!("{INIT}\n");
    let code = serve(std::io::Cursor::new(input), sink, call, Config::default());
    assert_eq!(code, 0);
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(lines(&out).len(), 1);
}

#[test]
fn the_server_answers_the_mcp_requests_it_needs() {
    let (r, init) = answer(
        r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"2026-07-28"}}"#,
    );
    let r = r.unwrap();
    assert!(!init);
    assert_eq!(r["id"], "a");
    // Never a modern revision: it has no path for channel notifications.
    assert_eq!(r["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(r["result"]["serverInfo"]["name"], "paddock");
    let (r, _) = answer(r#"{"jsonrpc":"2.0","id":2,"method":"server/discover"}"#);
    assert_eq!(r.unwrap()["error"]["code"], -32601);
    let (r, _) = answer(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    assert_eq!(r.unwrap()["result"], json!({"tools": []}));
    let (r, _) = answer(r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#);
    assert_eq!(r.unwrap()["result"], json!({}));
    assert_eq!(answer(INITIALIZED), (None, true));
    assert_eq!(
        answer(r#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#),
        (None, false)
    );
    assert_eq!(
        answer(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#),
        (None, false)
    );
    assert_eq!(answer(""), (None, false));
    let (r, _) = answer("not json");
    assert_eq!(r.unwrap()["error"]["code"], -32700);
}

/// Accepts the handshake reply, then fails like a closed pipe.
struct Closing(usize);
impl Write for Closing {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.0 == 0 {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.0 -= 1;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_closed_stdout_ends_the_server_with_status_0() {
    let (call, seen) = stub(vec![vec![entry(1, "user"), entry(2, "user")]]);
    let (reader, mut writer) = std::io::pipe().unwrap();
    let sink: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(Closing(1)));
    writeln!(writer, "{INIT}\n{INITIALIZED}").unwrap();
    // stdin stays open: the failed push alone ends the server.
    let code = serve(
        std::io::BufReader::new(reader),
        sink,
        call,
        Config::default(),
    );
    assert_eq!(code, 0);
    assert_eq!(*seen.lock().unwrap(), vec![0]);
    drop(writer);
}

#[test]
fn the_server_retries_the_host_and_then_gives_up() {
    let calls = Arc::new(Mutex::new(0));
    let c2 = calls.clone();
    let call: Call = Arc::new(move |_: &Request| {
        let mut n = c2.lock().unwrap();
        *n += 1;
        match *n {
            1 => Err("cannot reach the host".to_string()),
            2 => Ok(Response::new(Status::Failed, "this sandbox has no horse")),
            3 => Ok(Response::done(json!({"entries": "garbage"}))),
            _ => Err("cannot reach the host".to_string()),
        }
    });
    let (reader, mut writer) = std::io::pipe().unwrap();
    writeln!(writer, "{INIT}\n{INITIALIZED}").unwrap();
    let out: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(Vec::<u8>::new()));
    let cfg = Config {
        backoff: Duration::from_millis(1),
        attempts: 5,
        ..Config::default()
    };
    assert_eq!(serve(std::io::BufReader::new(reader), out, call, cfg), 1);
    assert_eq!(*calls.lock().unwrap(), 5);
    drop(writer);
}

#[test]
fn a_success_resets_the_failure_count() {
    let calls = Arc::new(Mutex::new(0));
    let c2 = calls.clone();
    // Fail, succeed, fail, fail: with 2 attempts it gives up at call 4.
    let call: Call = Arc::new(move |_: &Request| {
        let mut n = c2.lock().unwrap();
        *n += 1;
        if *n == 2 {
            Ok(Response::done(json!({"entries": []})))
        } else {
            Err("down".to_string())
        }
    });
    let (r, mut w) = std::io::pipe().unwrap();
    writeln!(w, "{INIT}\n{INITIALIZED}").unwrap();
    let input = std::io::BufReader::new(r);
    let out: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(Vec::<u8>::new()));
    let cfg = Config {
        backoff: Duration::from_millis(1),
        attempts: 2,
        ..Config::default()
    };
    assert_eq!(serve(input, out, call, cfg), 1);
    assert_eq!(*calls.lock().unwrap(), 4);
    drop(w);
}
