use super::*;

#[test]
fn site_answer_settles_five_waiting_hosts() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(10));
    a.set_wake_fence_for_test(true); // Models a platform wake fence.
    let fw = firewall(t.path(), a);
    let waiters: Vec<_> = [
        "accounts.google.com",
        "www.google.com",
        "google.com",
        "mail.google.com",
        "docs.google.com",
    ]
    .into_iter()
    .map(|h| {
        let fw = fw.clone();
        std::thread::spawn(move || ask(&fw, &host(h), None, crate::state::now()))
    })
    .collect();
    let q = wait_for_question(&env);
    for _ in 0..1000 {
        if fw.asker().unwrap().waiting() == 5 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fw.asker().unwrap().waiting(), 5);
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowSite).unwrap();
    for w in waiters {
        assert!(w.join().unwrap().unwrap().answer.allows());
    }
    assert_eq!(fw.asker().unwrap().waiting(), 0);
    assert!(questions(&env).is_empty());
    assert_eq!(fw.rules().file.len(), 2);
}
