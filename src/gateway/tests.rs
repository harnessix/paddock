use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

fn req(cap: &str, args: serde_json::Value, reason: &str) -> Request {
    let args: BTreeMap<String, serde_json::Value> = serde_json::from_value(args).unwrap();
    Request {
        v: 1,
        cap: cap.into(),
        args,
        reason: reason.into(),
    }
}

fn done(msg: &str) -> Response {
    Response {
        status: Status::Done,
        message: msg.into(),
        result: serde_json::Value::Null,
    }
}

#[test]
fn the_normalized_form_ignores_order_and_the_reason() {
    let a = req(
        "exec",
        serde_json::json!({"argv": ["ls", "-l"], "opts": {"b": 1, "a": 2}}),
        "look",
    );
    let b = req(
        "exec",
        serde_json::json!({"opts": {"a": 2, "b": 1}, "argv": ["ls", "-l"]}),
        "other words",
    );
    assert_eq!(canonical(&a), canonical(&b));
    assert_eq!(request_hash("s1", &a), request_hash("s1", &b));
    // Argument order inside a list is part of the action.
    let c = req("exec", serde_json::json!({"argv": ["-l", "ls"]}), "");
    assert_ne!(canonical(&a), canonical(&c));
    // The same request of another sandbox is another request.
    assert_ne!(request_hash("s1", &a), request_hash("s2", &a));
    assert_eq!(
        String::from_utf8(canonical(&c)).unwrap(),
        r#"{"args":{"argv":["-l","ls"]},"cap":"exec","v":1}"#
    );
}

#[test]
fn the_same_request_attaches_and_its_result_goes_to_one_reader() {
    let l = Ledger::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let start = |l: &Arc<Ledger>| {
        let (runs, gate) = (runs.clone(), gate.clone());
        l.submit(
            "s1",
            req("exec", serde_json::json!({"argv": ["ls"]}), ""),
            move |_| {
                runs.fetch_add(1, Ordering::SeqCst);
                let (m, c) = &*gate;
                let mut open = m.lock().unwrap();
                while !*open {
                    open = c.wait(open).unwrap();
                }
                done("listed")
            },
        )
        .unwrap()
    };
    let h = start(&l);
    // The user has not answered: the read reports still waiting.
    assert!(matches!(
        l.wait(&h, Duration::from_millis(50), || false),
        Wait::StillWaiting
    ));
    // The agent sends the same request again: no second run.
    assert_eq!(start(&l), h);
    {
        let (m, c) = &*gate;
        *m.lock().unwrap() = true;
        c.notify_all();
    }
    match l.wait(&h, Duration::from_secs(5), || false) {
        Wait::Done(r) => assert_eq!(r.message, "listed"),
        other => panic!("{other:?}"),
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // Collected once: a second read finds nothing.
    match l.wait(&h, Duration::from_millis(10), || false) {
        Wait::Done(r) => assert_eq!(r.status, Status::Invalid),
        other => panic!("{other:?}"),
    }
    // Sent again after that, it is a new request.
    let again = start(&l);
    assert!(matches!(
        l.wait(&again, Duration::from_secs(5), || false),
        Wait::Done(_)
    ));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn a_reader_that_goes_away_leaves_the_request_waiting() {
    let l = Ledger::new();
    let h = l
        .submit("s1", req("notify", serde_json::json!({}), ""), |_| {
            std::thread::sleep(Duration::from_millis(200));
            done("shown")
        })
        .unwrap();
    assert!(matches!(
        l.wait(&h, Duration::from_secs(5), || true),
        Wait::Cancelled
    ));
    assert_eq!(l.open(), 1);
    assert!(matches!(
        l.wait(&h, Duration::from_secs(5), || false),
        Wait::Done(_)
    ));
}

#[test]
fn a_sandbox_cannot_open_too_many_requests() {
    let l = Ledger::new();
    for i in 0..MAX_OPEN {
        l.submit("s1", req("exec", serde_json::json!({"n": i}), ""), |_| {
            std::thread::sleep(Duration::from_secs(2));
            done("")
        })
        .unwrap();
    }
    let r = l
        .submit("s1", req("exec", serde_json::json!({"n": 99}), ""), |_| {
            done("")
        })
        .unwrap_err();
    assert_eq!(r.status, Status::Invalid);
    // A request that already waits still attaches.
    assert!(l
        .submit(
            "s1",
            req("exec", serde_json::json!({"n": 0}), ""),
            |_| done("")
        )
        .is_ok());
}

#[test]
fn hmac_matches_rfc_4231() {
    // Test case 1.
    assert_eq!(
        crate::manifest::hex(&hmac(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // Test case 2.
    assert_eq!(
        crate::manifest::hex(&hmac(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // Test case 6: a key longer than the block.
    assert_eq!(
        crate::manifest::hex(&hmac(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn the_executor_runs_only_what_was_signed_once_and_in_time() {
    let key = [7u8; 32];
    let r = req("exec", serde_json::json!({"argv": ["ls"]}), "why");
    let env: std::collections::BTreeMap<String, String> =
        [("PATH".to_string(), "/usr/bin".to_string())].into();
    let job = sign(
        &key,
        "s1",
        std::path::Path::new("/p"),
        &r,
        env,
        serde_json::json!({"scratch": "/s"}),
    )
    .unwrap();
    let now = crate::state::now();
    let seen = Seen::default();
    assert_eq!(verify(&key, &job, &seen, now), Ok(()));
    // The same grant twice.
    assert_eq!(verify(&key, &job, &seen, now), Err(Refused::Replayed));
    // Another key.
    assert_eq!(
        verify(&[8u8; 32], &job, &Seen::default(), now),
        Err(Refused::Signature)
    );
    // A changed argument, a changed environment, a changed project.
    let mut j = job.clone();
    j.request
        .args
        .insert("argv".into(), serde_json::json!(["rm", "-rf", "/"]));
    assert_eq!(
        verify(&key, &j, &Seen::default(), now),
        Err(Refused::Signature)
    );
    let mut j = job.clone();
    j.env.insert("LD_PRELOAD".into(), "/x.so".into());
    assert_eq!(
        verify(&key, &j, &Seen::default(), now),
        Err(Refused::Signature)
    );
    let mut j = job.clone();
    j.grant.project = "/".into();
    assert_eq!(
        verify(&key, &j, &Seen::default(), now),
        Err(Refused::Signature)
    );
    // The reason is not part of what runs.
    let mut j = job.clone();
    j.request.reason = "other words".into();
    assert_eq!(verify(&key, &j, &Seen::default(), now), Ok(()));
    // Too late, or a grant that claims to be valid far ahead.
    assert_eq!(
        verify(&key, &job, &Seen::default(), now + GRANT_LIFETIME + 1),
        Err(Refused::Expired)
    );
    // A grant signed for one request does not cover another.
    let other = req("exec", serde_json::json!({"argv": ["id"]}), "");
    let mut j = sign(
        &key,
        "s1",
        std::path::Path::new("/p"),
        &other,
        Default::default(),
        serde_json::Value::Null,
    )
    .unwrap();
    j.grant.hash = job.grant.hash.clone();
    j.mac = crate::manifest::hex(&hmac(
        &key,
        &signed_bytes(&j.grant, &j.request, &j.env, &j.plan),
    ));
    assert_eq!(
        verify(&key, &j, &Seen::default(), now),
        Err(Refused::Mismatch)
    );
}

#[test]
fn the_key_is_made_once_and_kept_private() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let env = crate::env::HostEnv::from_parts(
        t.path().join("home"),
        "demo".into(),
        1,
        1,
        Default::default(),
    );
    // On macOS, key_file is the real temporary folder of the user: the
    // test makes its key in the test folder instead.
    let file = env.paddock_run().join("gateway.key");
    let a = key_at(&file).unwrap();
    assert_eq!(a.len(), 32);
    assert_eq!(key_at(&file).unwrap(), a);
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    if cfg!(target_os = "macos") {
        let t = crate::backend::seatbelt::keychain::user_temp().unwrap();
        assert_eq!(key_file(&env), t.join("paddock/gateway.key"));
    } else {
        assert_eq!(key_file(&env), file);
    }
}
