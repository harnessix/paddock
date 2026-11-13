use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::symlink;

fn setup() -> (tempfile::TempDir, HostEnv, Profile, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let host_home = t.path().join("host");
    let sandbox_home = t.path().join("sandbox");
    std::fs::create_dir_all(host_home.join(".tool")).unwrap();
    std::fs::create_dir_all(&sandbox_home).unwrap();
    let env = HostEnv::from_parts(host_home.clone(), "demo".into(), 1, 1, BTreeMap::new());
    let p: Profile = toml::from_str(
        "name = \"t\"\n[state]\nauth = [\".tool/token\"]\n\
             auth_expiry = { \".tool/token\" = \"claudeAiOauth.expiresAt\" }\n",
    )
    .unwrap();
    (t, env, p, host_home, sandbox_home)
}

const FLAGS: &str = ".tool/cache/onboarding.json";

fn flags_profile() -> Profile {
    toml::from_str(&format!(
        "name = \"t\"\n[state]\nauth = [\".tool/token\"]\n\
         done_flags = {{ \"{FLAGS}\" = [\"done\", \"consumerDone\"] }}\n"
    ))
    .unwrap()
}

fn put(path: &Path, v: serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn get(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn done_flags_go_into_a_missing_sandbox_file() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    put(
        &host.join(FLAGS),
        serde_json::json!({"done": true, "consumerDone": true, "other": false}),
    );
    AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert_eq!(
        get(&sb.join(FLAGS)),
        serde_json::json!({"consumerDone": true, "done": true})
    );
}

#[test]
fn done_flags_turn_true_in_the_sandbox_and_keep_other_keys() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    put(
        &host.join(FLAGS),
        serde_json::json!({"done": true, "consumerDone": true, "other": true}),
    );
    put(
        &sb.join(FLAGS),
        serde_json::json!({"done": false, "consumerDone": false, "other": false, "x": 1}),
    );
    let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert_eq!(
        get(&sb.join(FLAGS)),
        serde_json::json!({"done": true, "consumerDone": true, "other": false, "x": 1})
    );
    // The sandbox writes false again: the source stays true.
    put(
        &sb.join(FLAGS),
        serde_json::json!({"done": false, "consumerDone": false}),
    );
    a.copy_out().unwrap();
    assert_eq!(
        get(&host.join(FLAGS)),
        serde_json::json!({"done": true, "consumerDone": true, "other": true})
    );
}

#[test]
fn a_done_flag_set_in_the_sandbox_reaches_the_source() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    // Source missing.
    let mut a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert!(!sb.join(FLAGS).exists());
    put(
        &sb.join(FLAGS),
        serde_json::json!({"done": true, "consumerDone": false}),
    );
    // The live sync carries it.
    a.sync_now();
    assert_eq!(get(&host.join(FLAGS)), serde_json::json!({"done": true}));
    // Source false, with another key: only the listed true key moves.
    put(
        &host.join(FLAGS),
        serde_json::json!({"done": false, "consumerDone": false, "enterprise": false}),
    );
    put(
        &sb.join(FLAGS),
        serde_json::json!({"done": true, "consumerDone": true, "enterprise": true}),
    );
    a.copy_out().unwrap();
    assert_eq!(
        get(&host.join(FLAGS)),
        serde_json::json!({"done": true, "consumerDone": true, "enterprise": false})
    );
}

#[test]
fn a_missing_store_file_does_not_gain_unlisted_keys() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    put(
        &sb.join(FLAGS),
        serde_json::json!({
            "done": true,
            "consumerDone": false,
            "token": "secret",
            "notAFlag": true
        }),
    );
    a.copy_out().unwrap();
    let got = get(&host.join(FLAGS));
    assert!(
        got.get("token").is_none(),
        "unlisted key reached the host: {got}"
    );
    assert!(
        got.get("notAFlag").is_none(),
        "unlisted key reached the host: {got}"
    );
    assert_eq!(got["done"], serde_json::json!(true));
}

#[test]
fn done_flags_that_are_true_nowhere_write_nothing() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert!(!sb.join(FLAGS).exists());
    put(
        &sb.join(FLAGS),
        serde_json::json!({"done": false, "consumerDone": false, "other": true}),
    );
    a.copy_out().unwrap();
    assert!(!host.join(FLAGS).exists());
    assert!(!host.join(".tool/cache").exists());
    // Source false too: nothing changes on either side.
    put(&host.join(FLAGS), serde_json::json!({"done": false}));
    let before = std::fs::read(sb.join(FLAGS)).unwrap();
    let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    a.copy_out().unwrap();
    assert_eq!(std::fs::read(sb.join(FLAGS)).unwrap(), before);
    assert_eq!(get(&host.join(FLAGS)), serde_json::json!({"done": false}));
}

#[test]
fn invalid_or_large_done_flag_files_are_skipped() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    std::fs::create_dir_all(host.join(".tool/cache")).unwrap();
    std::fs::write(host.join(FLAGS), "{not json").unwrap();
    put(&sb.join(FLAGS), serde_json::json!({"done": true}));
    let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    a.copy_out().unwrap();
    assert_eq!(std::fs::read(host.join(FLAGS)).unwrap(), b"{not json");
    // A sandbox file over 64 KiB is not read.
    let big = format!("{{\"done\": true, \"pad\": \"{}\"}}", "x".repeat(70 * 1024));
    std::fs::write(sb.join(FLAGS), &big).unwrap();
    std::fs::write(host.join(FLAGS), "{\"done\": false}").unwrap();
    a.copy_out().unwrap();
    assert_eq!(get(&host.join(FLAGS)), serde_json::json!({"done": false}));
    // A large source is not read either.
    std::fs::write(host.join(FLAGS), &big).unwrap();
    put(&sb.join(FLAGS), serde_json::json!({"done": false}));
    AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert_eq!(get(&sb.join(FLAGS)), serde_json::json!({"done": false}));
    // Not an object.
    std::fs::write(host.join(FLAGS), "[true]").unwrap();
    AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert_eq!(get(&sb.join(FLAGS)), serde_json::json!({"done": false}));
}

#[test]
fn isolated_auth_keeps_done_flags_in_the_isolated_store() {
    let (_t, env, _, host, sb) = setup();
    let p = flags_profile();
    let mode = AuthMode::Isolated("work".into());
    let store = auth_root(&env, "t", &mode).join(FLAGS);
    // The host home is not the source.
    put(&host.join(FLAGS), serde_json::json!({"done": true}));
    AuthSync::copy_in(&env, &p, &mode, &sb).unwrap();
    assert!(!sb.join(FLAGS).exists());
    put(
        &store,
        serde_json::json!({"done": true, "consumerDone": false}),
    );
    let a2 = AuthSync::copy_in(&env, &p, &mode, &sb).unwrap();
    assert_eq!(get(&sb.join(FLAGS)), serde_json::json!({"done": true}));
    put(
        &sb.join(FLAGS),
        serde_json::json!({"done": true, "consumerDone": true}),
    );
    a2.copy_out().unwrap();
    assert_eq!(
        get(&store),
        serde_json::json!({"done": true, "consumerDone": true})
    );
    assert_eq!(get(&host.join(FLAGS)), serde_json::json!({"done": true}));
}

#[test]
fn the_account_mode_uses_the_account_folder_as_the_store() {
    let (t, env, _, host_home, sb) = setup();
    let p: Profile = toml::from_str(
        "name = \"t\"\n[state]\nauth = [\".tool/token\", \".tool.json\", \".other\"]\n\
         seed = [\".tool/settings\"]\n\
         [state.account]\nenv = \"TOOL_HOME\"\ndir = \".tool\"\nalso = [\".tool.json\"]\n",
    )
    .unwrap();
    let acct = t.path().join("accounts/work");
    std::fs::create_dir_all(&acct).unwrap();
    std::fs::write(host_home.join(".tool/token"), "main").unwrap();
    std::fs::write(host_home.join(".other"), "other").unwrap();
    std::fs::write(acct.join("token"), "work").unwrap();
    std::fs::write(acct.join(".tool.json"), "{}").unwrap();
    std::fs::write(acct.join("settings"), "work settings").unwrap();
    let mode = AuthMode::account(&acct);
    assert_eq!(
        host_file(&env, &p, &mode, ".tool/settings"),
        acct.join("settings")
    );
    assert_eq!(host_file(&env, &p, &mode, ".x"), host_home.join(".x"));
    let sync = AuthSync::copy_in(&env, &p, &mode, &sb).unwrap();
    seed(&env, &p, &mode, &sb).unwrap();
    // Inside, the files keep their usual places.
    assert_eq!(
        std::fs::read_to_string(sb.join(".tool/token")).unwrap(),
        "work"
    );
    assert_eq!(
        std::fs::read_to_string(sb.join(".tool.json")).unwrap(),
        "{}"
    );
    assert_eq!(std::fs::read_to_string(sb.join(".other")).unwrap(), "other");
    assert_eq!(
        std::fs::read_to_string(sb.join(".tool/settings")).unwrap(),
        "work settings"
    );
    // A refreshed token goes back to the account folder, not to the main login.
    std::fs::write(sb.join(".tool/token"), "work2").unwrap();
    let r = sync.copy_out().unwrap();
    assert_eq!(r.saved, vec![acct.join("token")]);
    assert_eq!(
        std::fs::read_to_string(acct.join("token")).unwrap(),
        "work2"
    );
    assert_eq!(
        std::fs::read_to_string(host_home.join(".tool/token")).unwrap(),
        "main"
    );
    // A symlink in the account folder is not followed.
    let secret = t.path().join("id_key");
    std::fs::write(&secret, "PRIVATE KEY").unwrap();
    std::fs::remove_file(acct.join("token")).unwrap();
    symlink(&secret, acct.join("token")).unwrap();
    assert!(AuthSync::copy_in(&env, &p, &mode, &sb).is_err());
    // The mode names its own sandbox home and reads back from records.
    let back: AuthMode = mode.to_string().parse().unwrap();
    assert!(matches!(back, AuthMode::Account { ref key, .. } if key == &slug(&acct)));
    assert!(auth_import(&env, &p, &mode).is_err());
}

#[test]
fn a_folder_entry_syncs_every_credential_file_in_it() {
    let (t, env, _, host_home, sb) = setup();
    let p: Profile = toml::from_str("name = \"t\"\n[state]\nauth = [\".tool/creds/\"]\n").unwrap();
    let creds = host_home.join(".tool/creds");
    std::fs::create_dir_all(&creds).unwrap();
    std::fs::write(creds.join("a.json"), "a1").unwrap();
    let secret = t.path().join("id_key");
    std::fs::write(&secret, "PRIVATE KEY").unwrap();
    symlink(&secret, creds.join("link.json")).unwrap();
    let sync = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert_eq!(
        std::fs::read_to_string(sb.join(".tool/creds/a.json")).unwrap(),
        "a1"
    );
    // A link in the store is not followed into the sandbox.
    assert!(!sb.join(".tool/creds/link.json").exists());
    // A refresh and a new login inside both come back.
    std::fs::write(sb.join(".tool/creds/a.json"), "a2").unwrap();
    std::fs::write(sb.join(".tool/creds/b.json"), "b1").unwrap();
    let r = sync.copy_out().unwrap();
    assert_eq!(r.saved.len(), 2, "{:?}", r.saved);
    assert_eq!(std::fs::read_to_string(creds.join("a.json")).unwrap(), "a2");
    assert_eq!(std::fs::read_to_string(creds.join("b.json")).unwrap(), "b1");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "PRIVATE KEY");
}

/// A login file as an OAuth harness writes it: a refresh token and when
/// the access token expires (milliseconds, like Claude Code).
fn login(rt: u32, expires_ms: u64) -> String {
    serde_json::json!({"claudeAiOauth": {"refreshToken": format!("rt{rt}"), "expiresAt": expires_ms}})
        .to_string()
}

fn rt_of(path: &Path) -> String {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["claudeAiOauth"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn login_expiry_reads_only_the_named_field() {
    let path = "claudeAiOauth.expiresAt";
    assert_eq!(
        login_expiry(login(1, 1_800_000_000_000).as_bytes(), path),
        Some(1_800_000_000)
    );
    assert_eq!(
        login_expiry(br#"{"expiry_date":1800000000}"#, "expiry_date"),
        Some(1_800_000_000)
    );
    assert_eq!(
        login_expiry(br#"{"expiry":"2027-01-15T00:00:00Z"}"#, "expiry"),
        crate::forage::parse_time("2027-01-15T00:00:00Z")
    );
    assert_eq!(login_expiry(br#"{"token":"x"}"#, path), None);
    assert_eq!(login_expiry(b"not json", path), None);
}

/// Another login in the same file (an MCP server) expires much later
/// than the account login: it must not decide which side is newer.
#[test]
fn only_the_account_login_decides_which_side_is_newer() {
    let (_t, env, p, host_home, sb) = setup();
    let host = host_home.join(".tool/token");
    std::fs::write(&host, login(1, 1_000)).unwrap();
    let mut a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    let old_with_mcp = serde_json::json!({
        "claudeAiOauth": {"refreshToken": "rt1", "expiresAt": 1_000},
        "mcpOAuth": {"srv": {"expiresAt": 9_000_000_000_000u64}},
    });
    std::fs::write(sb.join(".tool/token"), old_with_mcp.to_string()).unwrap();
    std::fs::write(&host, login(2, 5_000)).unwrap();
    a.sync_now();
    assert_eq!(rt_of(&host), "rt2");
    assert_eq!(rt_of(&sb.join(".tool/token")), "rt2");
}

/// A database login (kiro-cli's SQLite file) is open in the harness: it
/// never moves while the sandbox runs.
#[test]
fn a_login_that_is_not_json_waits_for_the_end() {
    let (_t, env, p, host_home, sb) = setup();
    let host = host_home.join(".tool/token");
    std::fs::write(&host, b"SQLite format 3\0one").unwrap();
    let mut a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    std::fs::write(sb.join(".tool/token"), b"SQLite format 3\0two").unwrap();
    assert!(a.sync_now().is_empty());
    assert_eq!(std::fs::read(&host).unwrap(), b"SQLite format 3\0one");
    // The end of the run still copies it back.
    assert_eq!(a.copy_out().unwrap().saved, vec![host.clone()]);
}

#[test]
fn a_running_sandbox_and_the_host_stay_in_line() {
    let (_t, env, p, host_home, sb) = setup();
    let host = host_home.join(".tool/token");
    std::fs::write(&host, login(1, 1_000)).unwrap();
    let mut a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    // Nothing changed: nothing moves.
    assert!(a.sync_now().is_empty());
    // The harness refreshes inside: the host gets it at once.
    std::fs::write(sb.join(".tool/token"), login(2, 2_000)).unwrap();
    assert_eq!(a.sync_now(), vec![SyncStep::ToStore(host.clone())]);
    assert_eq!(rt_of(&host), "rt2");
    // Someone else (the host, another sandbox) refreshes: the sandbox gets it.
    std::fs::write(&host, login(3, 3_000)).unwrap();
    assert_eq!(a.sync_now(), vec![SyncStep::ToSandbox(host.clone())]);
    assert_eq!(rt_of(&sb.join(".tool/token")), "rt3");
    // Both changed: the one that expires later wins, either way.
    std::fs::write(sb.join(".tool/token"), login(5, 5_000)).unwrap();
    std::fs::write(&host, login(4, 4_000)).unwrap();
    a.sync_now();
    assert_eq!(
        (rt_of(&host), rt_of(&sb.join(".tool/token"))),
        ("rt5".into(), "rt5".into())
    );
    std::fs::write(sb.join(".tool/token"), login(6, 6_000)).unwrap();
    std::fs::write(&host, login(7, 7_000)).unwrap();
    a.sync_now();
    assert_eq!(
        (rt_of(&host), rt_of(&sb.join(".tool/token"))),
        ("rt7".into(), "rt7".into())
    );
    // Both changed and neither says when it expires: nothing moves.
    std::fs::write(sb.join(".tool/token"), "inside").unwrap();
    std::fs::write(&host, "host").unwrap();
    assert!(a.sync_now().is_empty());
    // A side that is gone is never removed on the other side.
    std::fs::remove_file(&host).unwrap();
    assert!(a.sync_now().is_empty());
    assert!(sb.join(".tool/token").exists());
}

/// The failure that this sync fixes: an OAuth server whose refresh
/// tokens work once, three sandboxes on one login that each refresh
/// once, and a fourth that starts later. Each harness refreshes with the
/// token in its file, as Claude Code does after it reads the file again.
#[test]
fn refresh_tokens_that_work_once_survive_several_sandboxes() {
    let (t, env, p, host_home, _) = setup();
    let host = host_home.join(".tool/token");
    std::fs::write(&host, login(1, 1_000)).unwrap();
    let mut valid = 1u32; // the one refresh token that the server accepts
    let refresh = |file: &Path, valid: &mut u32| -> bool {
        let rt: u32 = rt_of(file)[2..].parse().unwrap();
        if rt != *valid {
            return false; // the server refuses a used token: "log in again"
        }
        *valid += 1;
        std::fs::write(file, login(*valid, 1_000 * u64::from(*valid))).unwrap();
        true
    };
    let mut boxes: Vec<(PathBuf, AuthSync)> = Vec::new();
    for i in 0..3 {
        let sb = t.path().join(format!("sb{i}"));
        std::fs::create_dir_all(&sb).unwrap();
        let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
        boxes.push((sb, a));
    }
    for i in 0..3 {
        // Each sandbox refreshes in turn; the sync runs between them.
        for (_, a) in boxes.iter_mut() {
            a.sync_now();
        }
        for (_, a) in boxes.iter_mut() {
            a.sync_now();
        }
        let file = boxes[i].0.join(".tool/token");
        assert!(refresh(&file, &mut valid), "sandbox {i} got a used token");
    }
    for (_, a) in boxes.iter_mut() {
        a.sync_now();
    }
    // A sandbox that starts now copies a token that still works.
    let late = t.path().join("late");
    std::fs::create_dir_all(&late).unwrap();
    let mut l = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &late).unwrap();
    assert!(
        refresh(&late.join(".tool/token"), &mut valid),
        "the late sandbox got a used token"
    );
    l.sync_now();
    // At the end nobody reports a conflict or overwrites the newest login.
    for (_, a) in boxes.into_iter() {
        let r = a.copy_out().unwrap();
        assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    }
    // The host holds the one token that the server accepts now.
    assert_eq!(rt_of(&host), format!("rt{valid}"));
}

#[test]
fn copy_out_lets_a_newer_copy_win() {
    let (_t, env, p, host_home, sb) = setup();
    let host = host_home.join(".tool/token");
    std::fs::write(&host, login(1, 1_000)).unwrap();
    let a = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    std::fs::write(&host, login(2, 2_000)).unwrap();
    std::fs::write(sb.join(".tool/token"), login(3, 3_000)).unwrap();
    let r = a.copy_out().unwrap();
    assert_eq!(r.saved, vec![host.clone()]);
    assert_eq!(rt_of(&host), "rt3");
    // An older copy still loses.
    let b = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    std::fs::write(&host, login(5, 5_000)).unwrap();
    std::fs::write(sb.join(".tool/token"), login(4, 4_000)).unwrap();
    let r = b.copy_out().unwrap();
    assert_eq!(r.conflicts, vec![host.clone()]);
    assert_eq!(rt_of(&host), "rt5");
}

#[test]
fn copy_out_refuses_a_symlinked_credential() {
    let (t, env, p, host_home, sb) = setup();
    std::fs::write(host_home.join(".tool/token"), "old").unwrap();
    let secret = t.path().join("id_key");
    std::fs::write(&secret, "PRIVATE KEY").unwrap();
    let sync = AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    // The sandbox swaps its token for a link to a host file.
    std::fs::remove_file(sb.join(".tool/token")).unwrap();
    symlink(&secret, sb.join(".tool/token")).unwrap();
    let r = sync.copy_out().unwrap();
    assert!(r.saved.is_empty());
    assert_eq!(
        std::fs::read_to_string(host_home.join(".tool/token")).unwrap(),
        "old"
    );
}

#[test]
fn watched_files_report_planted_links_and_fifos_without_hanging() {
    let t = tempfile::tempdir().unwrap();
    let (a, b, c) = (
        t.path().join(".envrc"),
        t.path().join("hook"),
        t.path().join("x"),
    );
    std::fs::write(&a, "echo hi").unwrap();
    std::fs::write(t.path().join("same"), "echo hi").unwrap();
    let before = snapshot(&[a.clone(), b.clone(), c.clone()]);
    // A link to a file with the same content, a FIFO, and /dev/zero.
    std::fs::remove_file(&a).unwrap();
    std::os::unix::fs::symlink(t.path().join("same"), &a).unwrap();
    let cs = std::ffi::CString::new(b.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cs.as_ptr(), 0o600) }, 0);
    std::os::unix::fs::symlink("/dev/zero", &c).unwrap();
    let mut got = changed(&before);
    got.sort();
    let mut want = vec![a, b, c];
    want.sort();
    assert_eq!(got, want);
    assert!(file_hash(Path::new("/dev/zero")).is_none());
}

#[test]
fn copy_in_never_writes_through_a_planted_link() {
    let (t, env, p, host_home, sb) = setup();
    std::fs::write(host_home.join(".tool/token"), "tok").unwrap();
    let victim = t.path().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    // A previous run left `.tool` as a link to a host folder.
    symlink(&victim, sb.join(".tool")).unwrap();
    assert!(AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).is_err());
    assert!(!victim.join("token").exists());
    // A file link is replaced, not written through.
    std::fs::remove_file(sb.join(".tool")).unwrap();
    std::fs::create_dir_all(sb.join(".tool")).unwrap();
    std::fs::write(victim.join("f"), "keep").unwrap();
    symlink(victim.join("f"), sb.join(".tool/token")).unwrap();
    AuthSync::copy_in(&env, &p, &AuthMode::Shared, &sb).unwrap();
    assert_eq!(std::fs::read_to_string(victim.join("f")).unwrap(), "keep");
    assert_eq!(
        std::fs::read_to_string(sb.join(".tool/token")).unwrap(),
        "tok"
    );
}

#[test]
fn gitconfig_replaces_a_link() {
    let (t, env, _p, _h, sb) = setup();
    let victim = t.path().join("bashrc");
    std::fs::write(&victim, "keep").unwrap();
    symlink(&victim, sb.join(".gitconfig")).unwrap();
    write_gitconfig(&env, &sb, None).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert!(std::fs::read_to_string(sb.join(".gitconfig"))
        .unwrap()
        .contains("[init]"));
}
