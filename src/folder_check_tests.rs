use super::*;
use std::fs;

/// A context for a home at `home`, with paddock's folders in the XDG
/// defaults below it. No file is read.
fn ctx_at(home: &str) -> Ctx {
    let h = PathBuf::from(home);
    let data = h.join(".local/share/paddock");
    Ctx {
        homes: vec![h.clone()],
        paddock: vec![
            data.clone(),
            h.join(".local/state/paddock"),
            h.join(".config/paddock"),
            h.join(".cache/paddock"),
        ],
        project_roots: vec![
            (data.join("worktrees"), 2),
            (data.join("scratch"), 1),
            (h.join(".cache/paddock/doctor-runs"), 2),
        ],
        user_dirs: vec![h.join("Dokumente")],
        allow_wide: Vec::new(),
        config_file: h.join(".config/paddock/config.toml"),
    }
}

fn linux() -> Ctx {
    ctx_at("/home/demo")
}

fn mac() -> Ctx {
    ctx_at("/Users/demo")
}

fn refused(ctx: &Ctx, p: &str) -> bool {
    hard(ctx, Path::new(p)).is_some()
}

/// A context for a real temporary home, as `Ctx::paths` builds it.
fn real_env(home: &Path) -> HostEnv {
    HostEnv::from_parts(
        home.to_path_buf(),
        "demo".into(),
        4242,
        4242,
        Default::default(),
    )
}

fn small() -> Limits {
    Limits {
        repos: 5,
        files: 50,
        bytes: 10_000,
        time: Duration::from_secs(5),
    }
}

#[test]
fn the_root_the_home_and_above_are_refused_on_linux_and_macos() {
    for (ctx, home) in [(linux(), "/home/demo"), (mac(), "/Users/demo")] {
        for p in ["/", home, "/home", "/Users"] {
            assert!(refused(&ctx, p), "{p}");
        }
        let r = hard(&ctx, Path::new(home)).unwrap();
        assert!(r.contains("home directory"), "{r}");
    }
    // A home in an unusual place: its parents are refused too.
    let ctx = ctx_at("/data/users/demo");
    assert!(refused(&ctx, "/data/users"));
    assert!(refused(&ctx, "/data"));
    assert!(!refused(&ctx, "/data/users/demo/code/app"));
}

#[test]
fn system_folders_are_refused() {
    let ctx = linux();
    for p in [
        "/etc",
        "/usr",
        "/var",
        "/opt",
        "/System",
        "/Library",
        "/Applications",
        "/private",
        "/private/etc",
        "/private/var",
        "/usr/local",
        "/boot",
    ] {
        assert!(refused(&ctx, p), "{p}");
    }
    // Inside a system tree.
    for p in [
        "/etc/nginx",
        "/private/etc/ssh",
        "/System/Library",
        "/usr/share/doc",
    ] {
        let r = hard(&ctx, Path::new(p)).unwrap();
        assert!(r.contains("inside the system folder"), "{p}: {r}");
    }
}

#[test]
fn normal_projects_pass_the_hard_rules() {
    for ctx in [linux(), mac()] {
        let home = ctx.homes[0].display().to_string();
        for p in [
            format!("{home}/code/myapp"),
            format!("{home}/Documents/myapp"),
            format!("{home}/Library/Mobile Documents/com~apple~CloudDocs/app"),
            "/var/www/site".into(),
            "/opt/app".into(),
            "/tmp/scratch".into(),
            // The temporary folder of macOS.
            "/private/var/folders/x1/T/proj".into(),
            "/var/folders/x1/T/proj".into(),
            "/usr/local/src/app".into(),
            "/usr/src/linux".into(),
            "/srv/app".into(),
        ] {
            assert!(!refused(&ctx, &p), "{p}: {:?}", hard(&ctx, Path::new(&p)));
        }
    }
}

#[test]
fn paddock_folders_are_refused_but_worktrees_and_scratch_projects_pass() {
    let ctx = linux();
    for p in [
        "/home/demo/.local/share/paddock",
        "/home/demo/.local/share",
        "/home/demo/.local",
        "/home/demo/.local/state/paddock",
        "/home/demo/.config/paddock",
        "/home/demo/.cache/paddock",
        "/home/demo/.local/share/paddock/homes",
        "/home/demo/.local/share/paddock/homes/claude",
        "/home/demo/.local/share/paddock/worktrees",
        "/home/demo/.local/share/paddock/worktrees/repo",
        "/home/demo/.local/share/paddock/scratch",
        "/home/demo/.local/state/paddock/runs/x",
        "/home/demo/.cache/paddock/doctor-runs",
        "/home/demo/.cache/paddock/doctor-runs/12-ab",
    ] {
        assert!(refused(&ctx, p), "{p}");
    }
    for p in [
        "/home/demo/.local/share/paddock/worktrees/repo/claude-1a2b",
        "/home/demo/.local/share/paddock/worktrees/repo/claude-1a2b/src",
        "/home/demo/.local/share/paddock/scratch/codex",
        "/home/demo/.cache/paddock/doctor-runs/12-ab/project",
    ] {
        assert!(!refused(&ctx, p), "{p}: {:?}", hard(&ctx, Path::new(p)));
    }
    let r = hard(&ctx, Path::new("/home/demo/.local/share/paddock")).unwrap();
    assert!(r.contains("~/.local/share/paddock"), "{r}");
}

#[test]
fn personal_and_cloud_folders_need_a_confirmation() {
    for ctx in [linux(), mac()] {
        let h = ctx.homes[0].clone();
        for d in [
            "Documents",
            "Desktop",
            "Downloads",
            "Pictures",
            "Movies",
            "Music",
            "Dokumente",
            "Library/Mobile Documents/com~apple~CloudDocs",
            "Library/CloudStorage",
            "Library/CloudStorage/GoogleDrive-demo@example.com",
            "Library/CloudStorage/OneDrive-Personal",
            "Dropbox",
            "OneDrive - Contoso",
            "Google Drive",
        ] {
            let f = h.join(d);
            assert!(personal(&ctx, &f).is_some(), "{}", f.display());
            assert!(hard(&ctx, &f).is_none(), "{}", f.display());
        }
        for d in [
            "Documents/app",
            "Dropbox/app",
            "code",
            "Library/CloudStorage/x/app",
        ] {
            assert!(personal(&ctx, &h.join(d)).is_none(), "{d}");
        }
    }
}

#[test]
fn a_folder_with_keys_or_logins_needs_a_confirmation() {
    for marker in [
        ".ssh",
        ".gnupg",
        ".aws",
        ".config/gcloud",
        ".password-store",
        "Library/Keychains",
    ] {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(marker)).unwrap();
        let r = secrets(t.path()).unwrap_or_else(|| panic!("{marker}"));
        assert!(r.contains(marker), "{r}");
    }
    for file in ["login.keychain-db", "vault.kdbx"] {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(file), "x").unwrap();
        assert!(secrets(t.path()).unwrap().contains(file));
    }
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("src")).unwrap();
    fs::write(t.path().join(".npmrc"), "x").unwrap();
    assert!(secrets(t.path()).is_none());
}

fn repo(p: &Path) {
    fs::create_dir_all(p.join(".git")).unwrap();
}

#[test]
fn a_folder_of_projects_needs_a_confirmation_and_names_a_project() {
    let t = tempfile::tempdir().unwrap();
    let code = t.path().join("code");
    for i in 0..5 {
        repo(&code.join(format!("app{i}")));
    }
    let ctx = Ctx::default();
    let c = check_with(&ctx, &code, false, true, &LIMITS);
    assert_eq!(c.level, Level::Ok, "5 repositories are fine: {}", c.reason);
    repo(&code.join("app5"));
    let c = check_with(&ctx, &code, false, true, &LIMITS);
    assert_eq!(c.level, Level::Confirm);
    assert!(c.reason.contains("6 git repositories"), "{}", c.reason);
    assert_eq!(c.found.as_ref().unwrap().repos, 6);
    let line = c.error_line(&ctx).unwrap();
    assert!(line.contains(&code.display().to_string()), "{line}");
    assert!(
        line.contains("for example") && line.contains("app0"),
        "{line}"
    );
    assert!(line.contains(FLAG) && line.contains("allow_wide"), "{line}");
    assert!(!line.contains('\n'), "one line: {line}");
    // The flag confirms it.
    let c = check_with(&ctx, &code, true, true, &LIMITS);
    assert_eq!(c.level, Level::Confirmed);
    assert!(c.passes() && c.error_line(&ctx).is_none());
    assert_eq!(c.confirmed_by.as_deref(), Some(FLAG));
}

#[test]
fn a_repository_with_many_submodules_is_one_project() {
    let t = tempfile::tempdir().unwrap();
    let mono = t.path().join("mono");
    repo(&mono);
    for i in 0..8 {
        let m = mono.join(format!("mod{i}"));
        fs::create_dir_all(&m).unwrap();
        fs::write(m.join(".git"), "gitdir: ../.git/modules/x").unwrap();
    }
    let c = check_with(&Ctx::default(), &mono, false, true, &LIMITS);
    assert_eq!(c.level, Level::Ok, "{}", c.reason);
}

#[test]
fn many_files_or_bytes_need_a_confirmation_and_the_count_says_what_it_found() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("big");
    fs::create_dir_all(d.join("a/b")).unwrap();
    for i in 0..60 {
        fs::write(d.join("a/b").join(format!("f{i}")), "").unwrap();
    }
    let c = check_with(&Ctx::default(), &d, false, true, &small());
    assert_eq!(c.level, Level::Confirm);
    assert!(c.reason.contains("more than 50 files"), "{}", c.reason);
    assert!(c.reason.contains("counted in"), "{}", c.reason);

    let d = t.path().join("heavy");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("blob"), vec![0u8; 20_000]).unwrap();
    let c = check_with(&Ctx::default(), &d, false, true, &small());
    assert_eq!(c.level, Level::Confirm);
    assert!(c.reason.contains("more than 0.0 GB"), "{}", c.reason);
    assert!(c.found.unwrap().bytes >= 20_000);
}

#[test]
fn build_and_dependency_folders_do_not_count() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("app");
    for skip in ["node_modules/x", "target/debug", ".git/objects"] {
        fs::create_dir_all(d.join(skip)).unwrap();
        for i in 0..60 {
            fs::write(d.join(skip).join(format!("f{i}")), "").unwrap();
        }
    }
    fs::write(d.join("main.rs"), "fn main() {}").unwrap();
    let c = check_with(&Ctx::default(), &d, false, true, &small());
    assert_eq!(c.level, Level::Ok, "{}", c.reason);
    assert_eq!(c.found.unwrap().files, 1);
}

#[test]
fn the_count_stops_at_the_time_limit() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..2000 {
        fs::write(t.path().join(format!("f{i}")), "").unwrap();
    }
    let limits = Limits {
        time: Duration::ZERO,
        ..LIMITS
    };
    let f = count(t.path(), &limits);
    assert!(!f.complete);
    assert!(f.files < 2000, "{}", f.files);
    assert!(found_words(&f).contains("time limit"));
    let f = count(t.path(), &LIMITS);
    assert!(f.complete);
    assert_eq!(f.files, 2000);
}

#[test]
fn allow_wide_confirms_but_never_a_refused_folder() {
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("Documents");
    fs::create_dir_all(&docs).unwrap();
    let mut ctx = ctx_at(&t.path().display().to_string());
    let c = check_with(&ctx, &docs, false, true, &LIMITS);
    assert_eq!(c.level, Level::Confirm);
    let line = c.error_line(&ctx).unwrap();
    assert!(
        line.contains("~/Documents") && line.contains("Documents folder"),
        "{line}"
    );
    assert!(line.contains("allow_wide = [\"~/Documents\"]"), "{line}");
    ctx.allow_wide = vec![docs.clone(), t.path().to_path_buf()];
    let c = check_with(&ctx, &docs, false, true, &LIMITS);
    assert_eq!(c.level, Level::Confirmed);
    assert_eq!(c.confirmed_by.as_deref(), Some("[run] allow_wide"));
    // The home is in allow_wide too, and still refused.
    let c = check_with(&ctx, t.path(), true, true, &LIMITS);
    assert_eq!(c.level, Level::Refuse);
    let line = c.error_line(&ctx).unwrap();
    assert!(line.starts_with("the agent does not start in ~:"), "{line}");
    assert!(line.contains(FIX), "{line}");
}

#[test]
fn a_command_without_the_flag_names_only_allow_wide() {
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("Downloads");
    fs::create_dir_all(&docs).unwrap();
    let ctx = ctx_at(&t.path().display().to_string());
    let c = check_with(&ctx, &docs, false, false, &LIMITS);
    let line = c.error_line(&ctx).unwrap();
    assert!(
        !line.contains(FLAG) && line.contains("allow_wide"),
        "{line}"
    );
}

#[test]
fn the_host_context_reads_allow_wide_of_the_user_configuration() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let code = home.join("code");
    for i in 0..7 {
        repo(&code.join(format!("p{i}")));
    }
    let env = real_env(&home);
    let real = fs::canonicalize(&code).unwrap();
    let e = enforce(&env, &real, false, true).unwrap_err().to_string();
    assert!(e.contains("7 git repositories"), "{e}");
    assert!(e.contains("~/code"), "{e}");
    fs::create_dir_all(env.paddock_config()).unwrap();
    fs::write(
        env.paddock_config().join("config.toml"),
        "[run]\nallow_wide = [\"~/code\"]\n",
    )
    .unwrap();
    let c = enforce(&env, &real, false, true).unwrap();
    assert_eq!(c.level, Level::Confirmed);
    let lines = doctor_lines(&env).join("\n");
    assert!(lines.contains("allowed  ~/code"), "{lines}");
    // The home of the env, and a symlink to it, stay refused.
    let real_home = fs::canonicalize(&home).unwrap();
    assert!(enforce(&env, &real_home, true, true).is_err());
    assert!(enforce_hard(&env, &real_home).is_err());
    assert!(enforce_hard(&env, &real).is_ok());
}

#[test]
fn a_symlinked_home_is_refused_by_its_real_path() {
    let t = tempfile::tempdir().unwrap();
    let real = t.path().join("real");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, t.path().join("link")).unwrap();
    let env = real_env(&t.path().join("link"));
    let folder = fs::canonicalize(t.path().join("link")).unwrap();
    let e = enforce(&env, &folder, true, true).unwrap_err().to_string();
    assert!(e.contains("home directory"), "{e}");
}

#[test]
fn the_json_has_level_and_reason() {
    let t = tempfile::tempdir().unwrap();
    let ctx = ctx_at(&t.path().display().to_string());
    let c = check_with(&ctx, t.path(), false, true, &LIMITS);
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["level"], "refuse");
    assert!(v["reason"].as_str().unwrap().contains("home directory"));
    let p = t.path().join("app");
    fs::create_dir_all(&p).unwrap();
    let v = serde_json::to_value(check_with(&ctx, &p, false, true, &LIMITS)).unwrap();
    assert_eq!(v["level"], "ok");
    assert_eq!(v["reason"], "");
    assert_eq!(v["found"]["files"], 0);
}

#[test]
fn numbers_read_well() {
    assert_eq!(grouped(200_000), "200 000");
    assert_eq!(grouped(1_234_567), "1 234 567");
    assert_eq!(grouped(12), "12");
    assert_eq!(gigabytes(LIMITS.bytes), "50.0 GB");
}

#[test]
fn a_project_file_cannot_set_allow_wide() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("paddock.toml");
    fs::write(&f, "[run]\nallow_wide = [\"/\"]\n").unwrap();
    let e = crate::manifest::Manifest::read(&f).unwrap_err().to_string();
    assert!(
        e.contains("allow_wide") && e.contains("user configuration"),
        "{e}"
    );
}
