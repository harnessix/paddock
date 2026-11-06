use super::*;
use crate::tack::config::parse;
use std::path::Path;

fn err(text: &str) -> String {
    format!("{:#}", parse(text).unwrap_err())
}

#[test]
fn built_in_profiles_give_gh_and_git_credential_helpers_uid() {
    for p in [
        "/usr/bin/gh",
        "/opt/homebrew/bin/gh",
        "/usr/lib/git-core/git-credential-libsecret",
        "/usr/local/bin/git-credential-manager",
    ] {
        assert_eq!(default_level(Path::new(p)), Level::Uid, "{p}");
    }
    for p in [
        "/usr/bin/git",
        "/usr/bin/npm",
        "/usr/bin/ghq",
        "/usr/bin/git-credential-",
        "/usr/bin/aws",
    ] {
        assert_eq!(default_level(Path::new(p)), Level::Refs, "{p}");
    }
}

#[test]
fn the_configuration_reads_levels_defaults_and_write() {
    let t = parse(
        r#"
[tack.secrets.GH_TOKEN]
[[tack.secrets.GH_TOKEN.allow]]
program = "/usr/bin/gh"
[[tack.secrets.GH_TOKEN.allow]]
program = "/usr/bin/git-credential-oauth"
protect = "refs"
[tack.secrets.NPM_TOKEN]
[[tack.secrets.NPM_TOKEN.allow]]
program = "/usr/bin/npm"
[[tack.secrets.NPM_TOKEN.allow]]
program = "/usr/bin/pnpm"
protect = "uid"
write = true
"#,
    )
    .unwrap();
    let gh = &t.secrets["GH_TOKEN"].allow;
    assert_eq!(
        (gh[0].protect, gh[0].level(), gh[0].write),
        (None, Level::Uid, false)
    );
    assert_eq!(
        gh[1].level(),
        Level::Refs,
        "an entry's own level wins over its profile"
    );
    let npm = &t.secrets["NPM_TOKEN"].allow;
    assert_eq!(npm[0].level(), Level::Refs);
    assert_eq!((npm[1].level(), npm[1].write), (Level::Uid, true));
    assert_eq!(source(&gh[0]), "the default of the built-in profile for gh");
    assert_eq!(source(&gh[1]), "set in its entry");
    assert_eq!(source(&npm[0]), "the default");
    assert!(wants_uid(&t));
    // An entry without the new fields is written back without them.
    let text = toml::to_string(&npm[0]).unwrap();
    assert!(
        !text.contains("protect") && !text.contains("write"),
        "{text}"
    );
}

#[test]
fn write_needs_uid_and_one_program_has_one_level() {
    let e = err(
        "[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/npm\"\nwrite = true\n",
    );
    assert!(
        e.contains("write = true is only for protect = \"uid\""),
        "{e}"
    );
    let e = err("[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/gh\"\nprotect = \"refs\"\nwrite = true\n");
    assert!(e.contains("write = true is only for"), "{e}");
    let e = err("[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/gh\"\n[tack.secrets.B]\n[[tack.secrets.B.allow]]\nprogram = \"/usr/bin/gh\"\nprotect = \"refs\"\n");
    assert!(
        e.contains("/usr/bin/gh has protect = \"uid\" in [tack.secrets.A] and protect = \"refs\" in [tack.secrets.B]"),
        "{e}"
    );
    let e = err("[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/gh\"\nwrite = true\n[tack.secrets.B]\n[[tack.secrets.B.allow]]\nprogram = \"/usr/bin/gh\"\n");
    assert!(
        e.contains("write = true in [tack.secrets.A] and write = false in [tack.secrets.B]"),
        "{e}"
    );
    assert!(err("[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"/usr/bin/gh\"\nprotect = \"full\"\n").contains("unknown variant"));
}

#[test]
fn the_level_of_a_start_comes_from_the_allowed_secrets_entries() {
    let t = parse(
        r#"
[tack.secrets.A]
[[tack.secrets.A.allow]]
program = "/usr/bin/tool"
protect = "uid"
write = true
[tack.secrets.B]
[[tack.secrets.B.allow]]
program = "/usr/bin/other"
"#,
    )
    .unwrap();
    let p = Path::new("/usr/bin/tool");
    assert_eq!(
        effective(&t, &["A".into()], p, p),
        Effective {
            level: Level::Uid,
            write: true
        }
    );
    // A secret that was not allowed does not count.
    assert_eq!(
        effective(&t, &["B".into()], p, p),
        Effective {
            level: Level::Refs,
            write: false
        }
    );
}

fn helper() -> Helper {
    Helper {
        owner: 1000,
        name: "_tack_alice".into(),
        uid: 981,
        gid: 977,
        home: "/var/lib/paddock-tackd/home/_tack_alice".into(),
    }
}

fn ids() -> Ids {
    Ids {
        uid: 1000,
        gid: 1000,
        groups: vec![1000, 10, 998],
    }
}

fn env() -> Vec<String> {
    [
        "PATH=/home/alice/.local/share/paddock/tack/bin:/usr/bin:/bin",
        "HOME=/home/alice",
        "XDG_CONFIG_HOME=/home/alice/.config",
        "XDG_RUNTIME_DIR=/run/user/1000",
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus",
        "SSH_AUTH_SOCK=/run/user/1000/ssh",
        "GH_TOKEN=tack://GH_TOKEN",
        "TERM=xterm",
        "GIT_CONFIG_COUNT=1",
        "GIT_CONFIG_KEY_0=core.pager",
        "GIT_CONFIG_VALUE_0=less",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn input<'a>(
    level: Level,
    h: Option<&'a Helper>,
    ids: &'a Ids,
    env: &'a [String],
    nested: bool,
) -> PlanInput<'a> {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    static LISTED: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    PlanInput {
        level,
        write: false,
        caller: ids,
        helper: h,
        nested,
        cwd: Path::new("/home/alice/src/app"),
        env,
        wrap: Some(Path::new("/var/lib/paddock-tackd/wrap/abc")),
        names: NAMES.get_or_init(|| vec!["GH_TOKEN".into(), "NPM_TOKEN".into()]),
        listed: LISTED.get_or_init(|| vec!["/usr/bin/gh".into(), "/odd:path".into()]),
    }
}

fn val<'a>(env: &'a [String], k: &str) -> Option<&'a str> {
    env.iter().find_map(|e| e.strip_prefix(&format!("{k}=")))
}

#[test]
fn a_plan_at_uid_runs_as_the_helper_with_its_group_home_acl_and_wrapper() {
    let (h, ids, env) = (helper(), ids(), env());
    let p = plan(&input(Level::Uid, Some(&h), &ids, &env, false)).unwrap();
    assert_eq!((p.level, p.uid, p.gid), (Level::Uid, 981, 977));
    assert_eq!(p.groups, [977], "none of the user's groups");
    assert_eq!(p.helper.as_deref(), Some("_tack_alice"));
    assert_eq!(
        p.grant,
        Some(Grant {
            folder: "/home/alice/src/app".into(),
            owner: 1000,
            helper: 981,
            write: false
        })
    );
    let e = &p.env;
    assert_eq!(
        val(e, "HOME"),
        Some("/var/lib/paddock-tackd/home/_tack_alice")
    );
    for gone in [
        "XDG_CONFIG_HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "SSH_AUTH_SOCK",
    ] {
        assert_eq!(val(e, gone), None, "{gone}");
    }
    assert_eq!(val(e, "TERM"), Some("xterm"));
    assert_eq!(
        val(e, "GH_TOKEN"),
        Some("tack://GH_TOKEN"),
        "the value comes later"
    );
    assert_eq!(
        val(e, "PATH"),
        Some("/var/lib/paddock-tackd/wrap/abc:/home/alice/.local/share/paddock/tack/bin:/usr/bin:/bin")
    );
    assert_eq!(val(e, WRAP_DIR), Some("/var/lib/paddock-tackd/wrap/abc"));
    assert_eq!(
        val(e, WRAP_PATH),
        Some("/home/alice/.local/share/paddock/tack/bin:/usr/bin:/bin")
    );
    assert_eq!(val(e, WRAP_SECRETS), Some("GH_TOKEN,NPM_TOKEN"));
    assert_eq!(
        val(e, WRAP_LISTED),
        Some("/usr/bin/gh"),
        "a path with : is left out"
    );
    // git: after the caller's own command-line configuration.
    assert_eq!(val(e, "GIT_CONFIG_COUNT"), Some("2"));
    assert_eq!(val(e, "GIT_CONFIG_KEY_0"), Some("core.pager"));
    assert_eq!(val(e, "GIT_CONFIG_KEY_1"), Some("safe.directory"));
    assert_eq!(val(e, "GIT_CONFIG_VALUE_1"), Some("*"));
    assert!(describe(&p).starts_with(
        "protect = uid: runs as _tack_alice (uid 981); processes of your user cannot read"
    ));
}

#[test]
fn a_plan_at_refs_runs_as_the_caller_with_its_environment() {
    let (ids, env) = (ids(), env());
    let p = plan(&input(Level::Refs, Some(&helper()), &ids, &env, false)).unwrap();
    assert_eq!(
        (p.uid, p.gid, p.groups.clone()),
        (1000, 1000, vec![1000, 10, 998])
    );
    assert_eq!((p.grant.clone(), p.helper.clone()), (None, None));
    assert_eq!(p.env, env);
    assert_eq!(
        describe(&p),
        "protect = refs: runs as uid 1000; other processes of your user can still read this program's environment and memory while it runs"
    );
}

#[test]
fn a_plan_at_uid_without_a_good_helper_fails_with_the_fix() {
    let (ids, env) = (ids(), env());
    let e = plan(&input(Level::Uid, None, &ids, &env, false)).unwrap_err();
    assert!(
        e.contains("helper account does not exist yet: run sudo paddock tack setup"),
        "{e}"
    );
    let other = Helper {
        owner: 1001,
        ..helper()
    };
    assert!(plan(&input(Level::Uid, Some(&other), &ids, &env, false))
        .unwrap_err()
        .contains("belongs to uid 1001"));
    let root = Helper { uid: 0, ..helper() };
    assert!(plan(&input(Level::Uid, Some(&root), &ids, &env, false)).is_err());
    let same = Helper {
        uid: 1000,
        ..helper()
    };
    assert!(plan(&input(Level::Uid, Some(&same), &ids, &env, false)).is_err());
}

#[test]
fn a_nested_start_stays_at_the_helper_and_grants_nothing() {
    let h = helper();
    let helper_ids = Ids {
        uid: 981,
        gid: 977,
        groups: vec![977],
    };
    let first = plan(&input(Level::Uid, Some(&h), &ids(), &env(), false)).unwrap();
    // The helper's program asks for a refs program: it still runs as the
    // helper, never as the user.
    let p = plan(&input(Level::Refs, Some(&h), &helper_ids, &first.env, true)).unwrap();
    assert_eq!((p.level, p.uid, p.grant.clone()), (Level::Uid, 981, None));
    // The environment does not grow: one wrapper folder, one git entry.
    assert_eq!(p.env.iter().filter(|e| e.starts_with("PATH=")).count(), 1);
    assert_eq!(val(&p.env, "PATH"), val(&first.env, "PATH"));
    assert_eq!(val(&p.env, "GIT_CONFIG_COUNT"), Some("2"));
}

#[test]
fn unwrap_env_gives_back_the_path_from_before_the_wrapper() {
    let w: Vec<String> = [
        "PATH=/w:/usr/bin",
        "PADDOCK_TACK_PATH=/usr/bin",
        "PADDOCK_TACK_WRAP=/w",
        "A=1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(unwrap_env(&w), ["PATH=/usr/bin", "A=1"]);
    let plain = vec!["PATH=/x".to_string()];
    assert_eq!(unwrap_env(&plain), plain);
}

#[test]
fn git_safe_directory_leaves_a_bad_count_alone() {
    let mut e = vec!["GIT_CONFIG_COUNT=x".to_string()];
    git_safe_directory(&mut e);
    assert_eq!(e, ["GIT_CONFIG_COUNT=x"]);
    let mut e = Vec::new();
    git_safe_directory(&mut e);
    assert_eq!(
        e,
        [
            "GIT_CONFIG_COUNT=1",
            "GIT_CONFIG_KEY_0=safe.directory",
            "GIT_CONFIG_VALUE_0=*"
        ]
    );
}

#[test]
fn helper_names_and_comments() {
    assert_eq!(helper_name("alice", 1000), "_tack_alice");
    assert_eq!(helper_name("Alice.Smith", 1000), "_tack_u1000");
    assert_eq!(helper_name(&"a".repeat(27), 1001), "_tack_u1001");
    assert_eq!(helper_name(&"a".repeat(26), 1001).len(), 32);
    let c = comment("alice", 1000);
    assert!(our_comment(&c, 1000) && !our_comment(&c, 100) && !our_comment("someone", 1000));
}

#[test]
fn the_registry_is_refused_when_others_can_write_it() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    // SAFETY: geteuid cannot fail.
    let me = unsafe { libc::geteuid() };
    assert_eq!(Registry::load(t.path(), me).unwrap(), Registry::default());
    let mut r = Registry::default();
    r.put(helper());
    r.put(Helper {
        name: "_tack_bob".into(),
        owner: 1001,
        uid: 982,
        ..helper()
    });
    r.save(t.path()).unwrap();
    let back = Registry::load(t.path(), me).unwrap();
    assert_eq!(back.of_owner(1000).map(|h| h.uid), Some(981));
    assert_eq!(back.of_helper(982).map(|h| h.owner), Some(1001));
    assert!(back.of_helper(1000).is_none());
    let f = Registry::file(t.path());
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(Registry::load(t.path(), me)
        .unwrap_err()
        .contains("only uid"));
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Registry::load(t.path(), me + 1).is_err(), "another owner");
    std::fs::write(&f, "{\"version\": 9, \"helpers\": []}").unwrap();
    assert!(Registry::load(t.path(), me)
        .unwrap_err()
        .contains("version 9"));
    std::fs::write(&f, "{half").unwrap();
    assert!(Registry::load(t.path(), me)
        .unwrap_err()
        .contains("damaged"));
}
