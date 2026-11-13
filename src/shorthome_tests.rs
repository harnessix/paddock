use super::*;
use std::collections::BTreeMap;

const CODEX_SOCK: &str = ".codex/app-server-control/app-server-control.sock";

fn home_of(user: &str) -> PathBuf {
    PathBuf::from(format!("/Users/{user}"))
}

/// The real home of a project, as `state::sandbox_home` builds it.
fn sandbox_home(host: &Path, harness: &str, login: &str, project: &str) -> PathBuf {
    host.join(".local/share/paddock/homes")
        .join(harness)
        .join(login)
        .join(crate::state::slug(Path::new(project)))
}

#[test]
fn the_alias_of_a_home_fits_the_codex_socket_also_for_a_long_user_name() {
    // A normal name, and a long one (macOS allows 255 characters, but the
    // short name of an account rarely has more than 20).
    for user in ["demo", "maxwellgrant", "averyveryverylongname"] {
        let host = home_of(user);
        let real = sandbox_home(&host, "codex", "isolated-check", "/Users/x/src/pstest");
        let alias = candidate(&host, HOMES, &real, 0);
        assert_eq!(
            alias.as_os_str().len(),
            alias_len(host.as_os_str().len(), HOMES)
        );
        assert_eq!(alias.as_os_str().len(), host.as_os_str().len() + 16);
        let sock = alias.join(CODEX_SOCK);
        assert!(
            crate::sockpath::fits(&sock, crate::sockpath::MACOS_MAX),
            "{} has {} bytes",
            sock.display(),
            sock.as_os_str().len()
        );
        // The real home does not fit: this is the bug.
        assert!(!crate::sockpath::fits(
            &real.join(CODEX_SOCK),
            crate::sockpath::MACOS_MAX
        ));
    }
    // The goal: 40 bytes or less for a name of up to 17 letters.
    let host = home_of("seventeenletters1");
    assert!(alias_len(host.as_os_str().len(), HOMES) <= 40);
    assert!(alias_len(host.as_os_str().len(), TMPS) <= 40);
}

#[test]
fn one_project_keeps_one_alias_and_two_projects_get_two() {
    let host = home_of("u");
    let a = sandbox_home(&host, "codex", "shared", "/Users/u/src/a");
    let b = sandbox_home(&host, "codex", "shared", "/Users/u/src/b");
    let a_claude = sandbox_home(&host, "claude", "shared", "/Users/u/src/a");
    assert_eq!(
        candidate(&host, HOMES, &a, 0),
        candidate(&host, HOMES, &a, 0)
    );
    assert_ne!(
        candidate(&host, HOMES, &a, 0),
        candidate(&host, HOMES, &b, 0)
    );
    assert_ne!(
        candidate(&host, HOMES, &a, 0),
        candidate(&host, HOMES, &a_claude, 0)
    );
    assert_ne!(
        candidate(&host, HOMES, &a, 0),
        candidate(&host, HOMES, &a, 1)
    );
    assert_eq!(
        candidate(&host, HOMES, &a, 0).parent(),
        Some(host.join(".pdk/h").as_path())
    );

    // On disk: the same link at each start, a distinct one per project.
    let t = tempfile::tempdir().unwrap();
    let host = t.path().join("home");
    let a = sandbox_home(&host, "codex", "shared", "/p/a");
    let b = sandbox_home(&host, "codex", "shared", "/p/b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let la = ensure(&host, HOMES, &a).unwrap();
    assert_eq!(ensure(&host, HOMES, &a).unwrap(), la);
    let lb = ensure(&host, HOMES, &b).unwrap();
    assert_ne!(la, lb);
    assert_eq!(std::fs::read_link(&la).unwrap(), a);
    assert_eq!(std::fs::read_link(&lb).unwrap(), b);
}

/// A home of an earlier version has no alias. Its next start gives it
/// one; its data stays at its path and shows through the alias.
#[test]
fn an_old_home_gets_its_alias_at_the_next_start_and_keeps_its_data() {
    let t = tempfile::tempdir().unwrap();
    let host = t.path().join("home");
    let real = sandbox_home(&host, "codex", "shared", "/p/old");
    let sessions = real.join(".codex/sessions/2026/11/rollout.jsonl");
    std::fs::create_dir_all(sessions.parent().unwrap()).unwrap();
    std::fs::write(&sessions, "a session").unwrap();
    let tmp = real.join(".paddock-tmp/codex-old-abc123");
    std::fs::create_dir_all(&tmp).unwrap();
    assert!(!host.join(ROOT).exists());

    let s = Short::ensure(&host, &real, &tmp).unwrap();
    assert_eq!(
        std::fs::read_to_string(s.home.1.join(".codex/sessions/2026/11/rollout.jsonl")).unwrap(),
        "a session"
    );
    assert_eq!(std::fs::read_to_string(&sessions).unwrap(), "a session");
    assert!(std::fs::symlink_metadata(&real).unwrap().is_dir());
    assert_eq!(
        std::fs::canonicalize(&s.tmp.1).unwrap(),
        std::fs::canonicalize(&tmp).unwrap()
    );
    // The alias folders are private.
    use std::os::unix::fs::PermissionsExt;
    for d in [host.join(ROOT).join(HOMES), host.join(ROOT).join(TMPS)] {
        let mode = std::fs::metadata(&d).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{}", d.display());
    }
    // The second start finds the same aliases.
    assert_eq!(Short::ensure(&host, &real, &tmp).unwrap(), s);
}

/// Something else at the name of an alias is a user asset or another
/// home: it stays, and the alias takes the next name. A stale link goes.
#[test]
fn a_taken_name_stays_and_a_stale_link_is_replaced() {
    let t = tempfile::tempdir().unwrap();
    let host = t.path().join("home");
    let mine = t.path().join("mine");
    let other = t.path().join("other");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::create_dir_all(dir(&host, HOMES)).unwrap();

    // A folder with data at the first name.
    let first = candidate(&host, HOMES, &mine, 0);
    std::fs::create_dir_all(&first).unwrap();
    std::fs::write(first.join("keep"), "x").unwrap();
    // A link to another existing folder at the second name.
    let second = candidate(&host, HOMES, &mine, 1);
    std::os::unix::fs::symlink(&other, &second).unwrap();
    let got = ensure(&host, HOMES, &mine).unwrap();
    assert_eq!(got, candidate(&host, HOMES, &mine, 2));
    assert_eq!(std::fs::read_to_string(first.join("keep")).unwrap(), "x");
    assert_eq!(std::fs::read_link(&second).unwrap(), other);

    // A stale link at the first name of another target is replaced.
    let gone = t.path().join("gone");
    let third = t.path().join("third");
    std::fs::create_dir_all(&third).unwrap();
    let name = candidate(&host, HOMES, &third, 0);
    std::os::unix::fs::symlink(&gone, &name).unwrap();
    assert_eq!(ensure(&host, HOMES, &third).unwrap(), name);
    assert_eq!(std::fs::read_link(&name).unwrap(), third);

    assert!(ensure(&host, HOMES, Path::new("relative")).is_err());
}

#[test]
fn the_environment_names_the_aliases() {
    let s = Short {
        home: ("/long/place".into(), "/Users/u/.pdk/h/aaaaaaaa".into()),
        tmp: (
            "/long/place/.paddock-tmp/id".into(),
            "/Users/u/.pdk/t/bbbbbbbb".into(),
        ),
    };
    let mut vars: BTreeMap<String, String> = [
        ("HOME", "/long/place"),
        ("CFFIXED_USER_HOME", "/long/place"),
        ("TMPDIR", "/long/place/.paddock-tmp/id"),
        ("xcrun_db", "/long/place/.paddock-tmp/id/xcrun_db"),
        ("PATH", "/long/place/.local/bin:/usr/bin:/long/places/x"),
        ("OTHER", "/long/placework"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    s.env(&mut vars);
    assert_eq!(vars["HOME"], "/Users/u/.pdk/h/aaaaaaaa");
    assert_eq!(vars["CFFIXED_USER_HOME"], "/Users/u/.pdk/h/aaaaaaaa");
    assert_eq!(vars["TMPDIR"], "/Users/u/.pdk/t/bbbbbbbb");
    assert_eq!(vars["xcrun_db"], "/Users/u/.pdk/t/bbbbbbbb/xcrun_db");
    assert_eq!(
        vars["PATH"],
        "/Users/u/.pdk/h/aaaaaaaa/.local/bin:/usr/bin:/long/places/x"
    );
    // A prefix of the name only is not the home.
    assert_eq!(vars["OTHER"], "/long/placework");
}

#[test]
fn the_policy_allows_sockets_through_the_aliases_and_no_change_of_the_links() {
    let s = Short {
        home: ("/long/place".into(), "/Users/u/.pdk/h/aaaaaaaa".into()),
        tmp: ("/long/place/t".into(), "/Users/u/.pdk/t/bbbbbbbb".into()),
    };
    let mut p = Policy::terminal(crate::seatbelt::Net::None, vec![]);
    allow(&mut p, &s);
    let sb = crate::seatbelt::render(&p).unwrap();
    for a in ["/Users/u/.pdk/h/aaaaaaaa", "/Users/u/.pdk/t/bbbbbbbb"] {
        assert!(sb.contains(&format!(
            "(allow network-bind (local unix-socket (subpath \"{a}\")))"
        )));
        assert!(sb.contains(&format!(
            "(allow network-outbound (remote unix-socket (subpath \"{a}\")))"
        )));
        assert!(sb.contains(&format!("(deny file-write* (literal \"{a}\"))")));
        assert!(!sb.contains(&format!("file-write* (subpath \"{a}\")")));
    }
}

#[test]
fn doctor_names_the_longest_socket_path_below_home() {
    let l = doctor_lines(Path::new("/Users/alice"));
    assert!(l[0].starts_with("  ok"), "{l:?}");
    assert!(
        l[0].contains("$HOME of a run is 28 bytes (/Users/alice/.pdk/h/<8 hex>)"),
        "{l:?}"
    );
    assert!(
        l[0].contains("up to 102 bytes, so 73 bytes below $HOME"),
        "{l:?}"
    );
    let long = format!("/Users/{}", "n".repeat(40));
    let l = doctor_lines(Path::new(&long));
    assert!(l[0].starts_with("  warn"), "{l:?}");
    assert!(l.iter().any(|x| x.contains("codex needs")), "{l:?}");
}
