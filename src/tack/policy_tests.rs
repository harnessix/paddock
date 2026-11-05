use super::*;
use crate::tack::config;
use crate::tack::ident::{identify, HashCache};
use crate::tack::procs::{self, Fake};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const UID: u32 = 1000;
const PID: u32 = 700;

struct World {
    _t: tempfile::TempDir,
    gh: PathBuf,
    git: PathBuf,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let mk = |n: &str, body: &str| {
        let p = t.path().join(n);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::canonicalize(p).unwrap()
    };
    let gh = mk("gh", "#!/fake gh\n");
    let git = mk("git", "#!/fake git\n");
    World { _t: t, gh, git }
}

fn sha(p: &Path) -> String {
    crate::tack::program::sha256_file(p).unwrap()
}

fn terminal() -> Fake {
    Fake::default()
        .add(1, 0, "/sbin/init", &[])
        .add(500, 1, "/usr/bin/foot", &["foot"])
        .add(600, 500, "/usr/bin/bash", &["bash"])
        .add(PID, 600, "/usr/bin/paddock", &["paddock"])
}

fn launch(program: &Path, args: &[&str]) -> Launch {
    Launch {
        program: program.display().to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: "/home/demo/p".into(),
        caller_pid: PID,
    }
}

fn no_signer(_: &Path) -> Option<String> {
    None
}

/// Decide with a configuration text, a process table and a pinned or
/// root-owned program.
fn run(cfg: &str, table: &Fake, req: &Launch, root_owned: bool) -> Decision {
    let cfg = config::parse(cfg).map_err(|e| format!("{e:#}"));
    let chain = procs::chain(table, PID, &procs::agent_names());
    let mut id = identify(Path::new(&req.program), UID, &HashCache::default());
    if let (Ok(i), true) = (&mut id, root_owned) {
        i.changeable = None;
    }
    decide(&Input {
        peer: Peer { uid: UID, pid: PID },
        req,
        cfg: cfg.as_ref().map_err(Clone::clone),
        chain: chain.as_ref(),
        ident: id.as_ref().map_err(Clone::clone),
        signer: &no_signer,
    })
}

fn allow_gh(w: &World, extra: &str) -> String {
    format!(
        "[tack.secrets.GH_TOKEN]\nhosts = [\"api.github.com\"]\n[[tack.secrets.GH_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{}\"\n{extra}",
        w.gh.display(),
        sha(&w.gh)
    )
}

#[test]
fn a_pinned_program_from_a_terminal_is_allowed_with_its_secrets_and_hosts() {
    let w = world();
    let d = run(
        &allow_gh(&w, ""),
        &terminal(),
        &launch(&w.gh, &["pr", "list"]),
        false,
    );
    assert_eq!(d.verdict, Verdict::Allow, "{}", d.reason);
    assert_eq!(d.secrets, ["GH_TOKEN"]);
    assert_eq!(d.hosts["GH_TOKEN"], ["api.github.com"]);
    let p = d.program.unwrap();
    assert_eq!(p.sha256, sha(&w.gh));
    assert_eq!(d.chain[0], format!("paddock[{PID}]"));
}

#[test]
fn a_program_on_no_allowlist_is_denied_without_its_hash() {
    let w = world();
    let d = run(&allow_gh(&w, ""), &terminal(), &launch(&w.git, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("not on the allowlist"), "{}", d.reason);
    assert!(d.program.is_none());
    // A missing file reads the same: no probing through tackd.
    let d2 = run(
        &allow_gh(&w, ""),
        &terminal(),
        &launch(Path::new("/no/such"), &[]),
        false,
    );
    assert!(d2.reason.contains("not on the allowlist"), "{}", d2.reason);
}

#[test]
fn a_changed_program_is_denied_with_its_new_hash() {
    let w = world();
    let cfg = allow_gh(&w, "");
    std::fs::write(&w.gh, "#!/fake evil\n").unwrap();
    let d = run(&cfg, &terminal(), &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("changed since its sha256") && d.reason.contains(&sha(&w.gh)),
        "{}",
        d.reason
    );
}

#[test]
fn an_unpinned_program_that_the_user_can_replace_is_denied() {
    let w = world();
    let cfg = format!(
        "[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"{}\"\n",
        w.gh.display()
    );
    let d = run(&cfg, &terminal(), &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("can be replaced") && d.reason.contains("sha256 = "),
        "{}",
        d.reason
    );
    // The same entry for a program that only root can change is fine.
    let d = run(&cfg, &terminal(), &launch(&w.gh, &[]), true);
    assert_eq!(d.verdict, Verdict::Allow, "{}", d.reason);
}

#[test]
fn a_link_to_an_allowlisted_program_matches_by_its_real_path() {
    let w = world();
    let link = w.gh.parent().unwrap().join("gh-link");
    std::os::unix::fs::symlink(&w.gh, &link).unwrap();
    let d = run(&allow_gh(&w, ""), &terminal(), &launch(&link, &[]), false);
    assert_eq!(d.verdict, Verdict::Allow, "{}", d.reason);
    assert!(listed(&config::parse(&allow_gh(&w, "")).unwrap(), &link));
}

#[test]
fn arguments_must_match_a_shape_and_are_never_repeated() {
    let w = world();
    let cfg = allow_gh(&w, "args = [\"pr list\", \"api *\"]\n");
    let ok = run(&cfg, &terminal(), &launch(&w.gh, &["api", "user"]), false);
    assert_eq!(ok.verdict, Verdict::Allow, "{}", ok.reason);
    let secret_arg = "ghp_SECRETVALUE123";
    for args in [
        &["auth", "token"][..],
        &["api", "--token", secret_arg][..],
        &[secret_arg][..],
    ] {
        let d = run(&cfg, &terminal(), &launch(&w.gh, args), false);
        assert_eq!(d.verdict, Verdict::Deny, "{args:?}");
        assert!(d.reason.contains("match no allowed shape"), "{}", d.reason);
        let all = serde_json::to_string(&d).unwrap();
        assert!(
            !all.contains(secret_arg) && !all.contains("token\""),
            "{all}"
        );
    }
}

#[test]
fn a_sandbox_or_agent_among_the_callers_is_refused_by_default() {
    let w = world();
    let sandboxed = terminal().contain(600);
    let d = run(&allow_gh(&w, ""), &sandboxed, &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("sandbox is among the callers"),
        "{}",
        d.reason
    );
    assert!(d.chain.iter().any(|c| c.contains("(sandbox)")));
    let agent = terminal().add(600, 500, "/home/demo/.local/bin/claude", &["claude"]);
    let d = run(&allow_gh(&w, ""), &agent, &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("coding agent claude"), "{}", d.reason);
}

#[test]
fn caller_rules_can_ask_or_allow_and_the_strictest_wins() {
    let w = world();
    let agent = terminal().add(600, 500, "/usr/bin/codex", &["codex"]);
    let ask = allow_gh(
        &w,
        "[[tack.secrets.GH_TOKEN.callers]]\nkind = \"agent\"\naction = \"ask\"\n",
    );
    let d = run(&ask, &agent, &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Ask, "{}", d.reason);
    assert_eq!(d.secrets, ["GH_TOKEN"]);
    let allow = allow_gh(
        &w,
        "[[tack.secrets.GH_TOKEN.callers]]\nkind = \"agent\"\naction = \"allow\"\n",
    );
    assert_eq!(
        run(&allow, &agent, &launch(&w.gh, &[]), false).verdict,
        Verdict::Allow
    );
    // A program rule that refuses beats the kind rule that allows.
    let both = format!("{allow}[[tack.secrets.GH_TOKEN.callers]]\nprogram = \"/usr/bin/foot\"\n");
    let d = run(&both, &agent, &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("/usr/bin/foot is among the callers"),
        "{}",
        d.reason
    );
    // Two kind rules for one kind: the strictest.
    let two = format!("{allow}[[tack.secrets.GH_TOKEN.callers]]\nkind = \"agent\"\n");
    assert_eq!(
        run(&two, &agent, &launch(&w.gh, &[]), false).verdict,
        Verdict::Deny
    );
}

#[test]
fn some_secrets_pass_and_others_are_named_with_their_reason() {
    let w = world();
    let cfg = format!(
        "{}[tack.secrets.NPM_TOKEN]\n[[tack.secrets.NPM_TOKEN.allow]]\nprogram = \"{}\"\nsha256 = \"{}\"\nargs = [\"publish\"]\n",
        allow_gh(&w, ""),
        w.gh.display(),
        sha(&w.gh)
    );
    let d = run(&cfg, &terminal(), &launch(&w.gh, &["pr"]), false);
    assert_eq!(d.verdict, Verdict::Allow);
    assert_eq!(d.secrets, ["GH_TOKEN"]);
    assert!(!d.hosts.contains_key("NPM_TOKEN"));
    assert!(d.reason.contains("not for NPM_TOKEN"), "{}", d.reason);
}

#[test]
fn the_kernel_pid_and_uid_win_over_the_request() {
    let w = world();
    let cfg = allow_gh(&w, "");
    let mut req = launch(&w.gh, &[]);
    req.caller_pid = 1;
    let d = run(&cfg, &terminal(), &req, false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("kernel reports"), "{}", d.reason);
    // The calling process belongs to another user.
    let mut t = terminal();
    t.0.get_mut(&PID).unwrap().uid = 1001;
    let d = run(&cfg, &t, &launch(&w.gh, &[]), false);
    assert!(d.reason.contains("changed its user"), "{}", d.reason);
    // The calling process ended.
    let t = Fake::default();
    let d = run(&cfg, &t, &launch(&w.gh, &[]), false);
    assert!(d.reason.contains("ended"), "{}", d.reason);
}

#[test]
fn root_gets_nothing_and_a_broken_config_denies() {
    let w = world();
    let req = launch(&w.gh, &[]);
    let chain = procs::chain(&terminal(), PID, &[]);
    let id = identify(&w.gh, 0, &HashCache::default()).unwrap();
    let cfg = config::parse(&allow_gh(&w, "")).unwrap();
    let d = decide(&Input {
        peer: Peer { uid: 0, pid: PID },
        req: &req,
        cfg: Ok(&cfg),
        chain: chain.as_ref(),
        ident: Ok(&id),
        signer: &no_signer,
    });
    assert!(d.reason.contains("root"), "{}", d.reason);
    let d = run("[tack.secrets.A]\nbackend = 3\n", &terminal(), &req, false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("configuration cannot be used"),
        "{}",
        d.reason
    );
}

#[test]
fn a_cut_chain_is_refused() {
    let w = world();
    let mut t = Fake::default();
    for i in 1..=200u32 {
        t = t.add(i, i - 1, "/x", &[]);
    }
    t = t.add(PID, 200, "/usr/bin/paddock", &[]);
    let d = run(&allow_gh(&w, ""), &t, &launch(&w.gh, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(
        d.reason.contains("could not be read to its end"),
        "{}",
        d.reason
    );
}

#[test]
fn a_link_that_now_points_to_a_shell_is_refused() {
    let w = world();
    let dir = w.gh.parent().unwrap();
    let bash = dir.join("bash");
    std::fs::write(&bash, "#!/fake bash\n").unwrap();
    std::fs::set_permissions(&bash, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tool = dir.join("tool");
    std::os::unix::fs::symlink(&w.git, &tool).unwrap();
    // Written while tool pointed at git; the link changes later.
    let cfg = format!(
        "[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"{}\"\nsha256 = \"{}\"\n",
        tool.display(),
        sha(&bash)
    );
    std::fs::remove_file(&tool).unwrap();
    std::os::unix::fs::symlink(&bash, &tool).unwrap();
    let d = run(&cfg, &terminal(), &launch(&tool, &[]), false);
    assert_eq!(d.verdict, Verdict::Deny);
    assert!(d.reason.contains("a shell"), "{}", d.reason);
}

#[test]
fn signer_entries_are_only_checked_on_macos() {
    let w = world();
    let cfg = format!(
        "[tack.secrets.A]\n[[tack.secrets.A.allow]]\nprogram = \"{}\"\nsigner = \"Developer ID Application: X (1)\"\n",
        w.gh.display()
    );
    let d = run(&cfg, &terminal(), &launch(&w.gh, &[]), false);
    if cfg!(target_os = "macos") {
        assert!(d.reason.contains("no valid signature"), "{}", d.reason);
    } else {
        // On Linux the signer pins nothing: the user-owned file needs a hash.
        assert!(d.reason.contains("can be replaced"), "{}", d.reason);
    }
}
