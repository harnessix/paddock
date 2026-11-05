use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

fn env_in(home: &Path, shell: &str) -> HostEnv {
    let mut vars = BTreeMap::new();
    vars.insert("SHELL".to_string(), shell.to_string());
    HostEnv::from_parts(home.to_path_buf(), "u".into(), 1000, 1000, vars)
}

fn cfg(programs: &[&str]) -> Tack {
    let mut text = String::from("[tack.secrets.A]\n");
    for p in programs {
        text.push_str(&format!("[[tack.secrets.A.allow]]\nprogram = \"{p}\"\n"));
    }
    super::super::config::parse(&text).unwrap()
}

fn exe(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn names_are_the_allowlisted_file_names() {
    let c = cfg(&["/usr/bin/gh", "/opt/x/bin/gh", "/usr/bin/glab"]);
    assert_eq!(names(&c), ["gh", "glab"]);
    assert!(!valid_name("paddock") && !valid_name(".x") && !valid_name("a b"));
    assert!(valid_name("git-lfs") && valid_name("node22"));
}

#[test]
fn a_stand_in_runs_paddock_and_falls_back_to_the_next_program() {
    let t = tempfile::tempdir().unwrap();
    let own = t.path().join("shims");
    let real = t.path().join("real");
    exe(&real.join("gh"), "#!/bin/sh\necho real-gh \"$@\"\n");
    // A fake paddock that shows what it got.
    let paddock = t.path().join("paddock");
    exe(&paddock, "#!/bin/sh\necho paddock \"$@\"\n");
    write_shims(&own, &["gh".into()], &paddock.display().to_string()).unwrap();
    let shim = own.join("gh");
    assert!(is_shim(&shim));
    assert!(!is_shim(&real.join("gh")));
    let path = format!("{}:{}", own.display(), real.display());
    let run = || {
        std::process::Command::new(&shim)
            .args(["pr", "it's"])
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    let out = run();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "paddock tack run --shim -- gh pr it's\n"
    );
    // paddock is gone: the real gh runs, with the same arguments.
    std::fs::remove_file(&paddock).unwrap();
    let out = run();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "real-gh pr it's\n");
    std::fs::remove_file(real.join("gh")).unwrap();
    let out = run();
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("gh: not found"));
}

#[test]
fn stale_stand_ins_go_and_other_files_stay() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("bin");
    write_shims(&d, &["gh".into(), "glab".into()], "/x/paddock").unwrap();
    std::fs::write(d.join("mine"), "not a stand-in").unwrap();
    let out = write_shims(&d, &["gh".into()], "/x/paddock").unwrap();
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out[0].contains("glab") && out[0].contains("no longer allowlisted"));
    assert!(d.join("gh").exists() && !d.join("glab").exists() && d.join("mine").exists());
    // Unchanged: nothing written.
    assert!(write_shims(&d, &["gh".into()], "/x/paddock")
        .unwrap()
        .is_empty());
}

#[test]
fn resolve_skips_the_stand_in_folder_and_stand_in_files() {
    let t = tempfile::tempdir().unwrap();
    let own = t.path().join("own");
    let other = t.path().join("other");
    let real = t.path().join("real");
    write_shims(&own, &["gh".into()], "paddock").unwrap();
    write_shims(&other, &["gh".into()], "paddock").unwrap();
    exe(&real.join("gh"), "#!/bin/sh\n");
    let path = vec![own.clone(), PathBuf::from("rel"), other, real.clone()];
    assert_eq!(resolve("gh", &path, &[own]), Some(real.join("gh")));
    assert_eq!(resolve("nope", &path, &[]), None);
    assert_eq!(
        resolve(&real.join("gh").display().to_string(), &[], &[]),
        Some(real.join("gh"))
    );
}

#[test]
fn the_path_line_puts_the_folder_first_once() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("with space");
    std::fs::create_dir_all(&d).unwrap();
    let line = sh_line(&d);
    let script = format!("PATH=/usr/bin\n{line}\n{line}\necho \"$PATH\"\n");
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("{}:/usr/bin", d.display())
    );
    // A folder that is gone changes nothing.
    std::fs::remove_dir(&d).unwrap();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/usr/bin");
}

#[test]
fn install_and_remove_touch_only_their_lines() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    std::fs::write(home.join(".bash_profile"), "source ~/.bashrc\n").unwrap();
    std::fs::create_dir_all(home.join(".config/fish")).unwrap();
    let env = env_in(home, "/usr/bin/zsh");
    let c = cfg(&["/usr/bin/gh"]);
    let out = install(&env, &c, Path::new("/usr/bin/paddock")).unwrap();
    assert!(out.iter().any(|l| l.contains(".bash_profile")), "{out:?}");
    assert!(out.iter().any(|l| l.contains(".zprofile")), "{out:?}");
    // bash reads .bash_profile, so no .profile is made.
    assert!(!home.join(".profile").exists());
    let bp = std::fs::read_to_string(home.join(".bash_profile")).unwrap();
    assert!(
        bp.starts_with("source ~/.bashrc\n") && bp.contains(LINE_MARK),
        "{bp}"
    );
    assert!(home.join(".config/environment.d").join(ENVD).exists());
    assert!(home.join(".config/fish/conf.d/paddock-tack.fish").exists());
    assert!(is_shim(&dir(&env).join("gh")));
    // Again: nothing changes.
    let again = install(&env, &c, Path::new("/usr/bin/paddock")).unwrap();
    assert!(again.is_empty(), "{again:?}");
    let gone = remove(&env).unwrap();
    assert!(!gone.is_empty());
    assert_eq!(
        std::fs::read_to_string(home.join(".bash_profile")).unwrap(),
        "source ~/.bashrc\n"
    );
    // .zprofile only held the line: it is gone again.
    assert!(!home.join(".zprofile").exists());
    assert!(!home.join(".config/environment.d").join(ENVD).exists());
    assert!(!dir(&env).exists());
}

#[test]
fn doctor_names_what_is_not_routed() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    let other = home.join("mise/shims");
    exe(&other.join("gh"), "#!/bin/sh\n");
    let base = env_in(home, "/bin/sh");
    let d = dir(&base);
    let mut vars = BTreeMap::new();
    vars.insert(
        "PATH".to_string(),
        format!("{}:{}", other.display(), d.display()),
    );
    let env = HostEnv::from_parts(home.to_path_buf(), "u".into(), 1000, 1000, vars);
    let c = cfg(&["/usr/bin/gh"]);
    let l = doctor_lines(&env, &c).join("\n");
    assert!(l.contains("no stand-in for gh"), "{l}");
    install(&env, &c, Path::new("/gone/paddock")).unwrap();
    let l = doctor_lines(&env, &c).join("\n");
    assert!(l.contains("runs /gone/paddock, which is gone"), "{l}");
    assert!(l.contains("not first"), "{l}");
    assert!(
        l.contains("gh is found at") && l.contains("paddock tack run gh"),
        "{l}"
    );
    assert!(l.contains("absolute path"), "{l}");
    assert!(doctor_lines(&env, &Tack::default()).is_empty());
}
