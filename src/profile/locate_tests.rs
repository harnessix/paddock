//! The locate order with a thin PATH: PATH, the profile's folders, the
//! common folders. Each test has a temporary home with fake programs.

use super::*;
use crate::profile::HarnessFactory;
use std::collections::BTreeMap;

struct Home {
    _t: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

fn home() -> Home {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    Home { _t: t, home, root }
}

/// A host environment with `path` as PATH and the given extra variables.
fn env_with(h: &Home, path: &[PathBuf], extra: &[(&str, &str)]) -> HostEnv {
    let mut vars = BTreeMap::new();
    let path = std::env::join_paths(path).unwrap();
    vars.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    HostEnv::from_parts(h.home.clone(), "demo".into(), 1000, 1000, vars)
}

/// A thin PATH, as an app started from Finder, a service or `env -i` has.
/// It holds an empty folder, not `/usr/bin`, so that the host's own npm
/// and mise do not hide the fake ones.
fn thin(h: &Home) -> HostEnv {
    let empty = h.root.join("thin-path");
    std::fs::create_dir_all(&empty).unwrap();
    env_with(h, &[empty], &[])
}

/// A thin PATH and no folders of the system: the npm and node of the test
/// machine (a CI runner has them in /usr/local/bin) stay out.
fn isolated(h: &Home) -> HostEnv {
    let empty = h.root.join("thin-path");
    std::fs::create_dir_all(&empty).unwrap();
    env_with(h, &[empty], &[("PADDOCK_TEST_SYSTEM_BIN", "")])
}

fn program(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "\x7fELF").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn script(path: &Path, body: &str) {
    program(path);
    std::fs::write(path, body).unwrap();
}

/// A user profile in the temporary home, for a command that no host has.
fn user_profile(env: &HostEnv, text: &str) {
    let dir = env.paddock_config().join("harness.d");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fake.toml"), text).unwrap();
}

const FAKE: &str = "name = \"fake\"\n[locate]\ncommands = [\"paddock-fake-agent\"]\n";

#[test]
fn a_profile_folder_finds_grok_with_a_thin_path() {
    let h = home();
    let grok = h.home.join(".grok/bin/grok");
    program(&grok);
    std::fs::write(h.home.join(".grok/auth.json"), "{}").unwrap();
    let env = thin(&h);
    let f = HarnessFactory::new(&env).unwrap();
    let harness = f.create("grok").unwrap();
    assert_eq!(harness.exe, grok);
    let found = f.found("grok").unwrap();
    assert_eq!(found.by, FoundBy::Profile);
    assert_eq!(found.dir, h.home.join(".grok/bin"));
    // The folder of the program, read-only. Never ~/.grok with its login.
    assert_eq!(harness.install_mounts, vec![h.home.join(".grok/bin")]);
}

#[test]
fn path_comes_before_the_profile_folders() {
    let h = home();
    program(&h.home.join(".grok/bin/grok"));
    let on_path = h.root.join("bin/grok");
    program(&on_path);
    let env = env_with(&h, &[h.root.join("bin"), PathBuf::from("/usr/bin")], &[]);
    let f = HarnessFactory::new(&env).unwrap();
    assert_eq!(f.create("grok").unwrap().exe, on_path);
    assert_eq!(f.found("grok").unwrap().by, FoundBy::Path);
}

#[test]
fn the_profile_folders_come_before_the_common_folders() {
    let h = home();
    let env = thin(&h);
    let mine = h.root.join("tools/paddock-fake-agent");
    program(&mine);
    program(&h.home.join(".local/bin/paddock-fake-agent"));
    let dirs = format!("dirs = [\"{}\"]\n", mine.parent().unwrap().display());
    user_profile(&env, &format!("{FAKE}{dirs}"));
    let f = HarnessFactory::new(&env).unwrap();
    assert_eq!(f.create("fake").unwrap().exe, mine);
    assert_eq!(f.found("fake").unwrap().by, FoundBy::Profile);
}

#[test]
fn each_home_common_folder_finds_a_program() {
    for rel in [".local/bin", ".bun/bin", ".npm-global/bin", ".cargo/bin"] {
        let h = home();
        let env = thin(&h);
        user_profile(&env, FAKE);
        let exe = h.home.join(rel).join("paddock-fake-agent");
        program(&exe);
        let f = HarnessFactory::new(&env).unwrap();
        let harness = f.create("fake").unwrap();
        assert_eq!(harness.exe, exe, "{rel}");
        let found = f.found("fake").unwrap();
        assert_eq!(found.by, FoundBy::Common, "{rel}");
        assert_eq!(found.dir, h.home.join(rel), "{rel}");
        // A shared home bin folder: the file only, as for a PATH program.
        assert_eq!(harness.install_mounts, vec![exe], "{rel}");
    }
}

#[test]
fn the_fixed_common_folders_are_in_order() {
    let h = home();
    let dirs = fixed_dirs(&thin(&h));
    let want: Vec<PathBuf> = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        h.home.join(".local/bin"),
        h.home.join(".bun/bin"),
        h.home.join(".npm-global/bin"),
        h.home.join(".cargo/bin"),
    ]
    .into();
    assert_eq!(dirs, want);
}

#[test]
fn the_npm_global_prefix_is_a_common_folder() {
    let h = home();
    let env = isolated(&h);
    user_profile(&env, FAKE);
    let prefix = h.root.join("npm-prefix");
    // A fake npm in a common folder. It needs its own folder on PATH, as
    // the real one needs node there.
    script(
        &h.home.join(".local/bin/npm"),
        &format!(
            "#!/bin/sh\ncase \":$PATH:\" in *\":{}:\"*) ;; *) exit 3;; esac\n[ \"$1 $2\" = \"prefix -g\" ] && echo {}\n",
            h.home.join(".local/bin").display(),
            prefix.display()
        ),
    );
    let exe = prefix.join("bin/paddock-fake-agent");
    program(&exe);
    assert_eq!(npm_bin(&env, &fixed_dirs(&env)), Some(prefix.join("bin")));
    let f = HarnessFactory::new(&env).unwrap();
    assert_eq!(f.create("fake").unwrap().exe, exe);
    let found = f.found("fake").unwrap();
    assert_eq!((found.by, found.dir), (FoundBy::Common, prefix.join("bin")));
}

#[test]
fn a_mise_shim_resolves_through_mise_outside_path() {
    let h = home();
    let env = thin(&h);
    user_profile(&env, FAKE);
    let real = h.root.join("mise-installs/fake/1.0/paddock-fake-agent");
    program(&real);
    let shims = h.home.join(".local/share/mise/shims");
    program(&shims.join("paddock-fake-agent"));
    script(
        &h.home.join(".local/bin/mise"),
        &format!(
            "#!/bin/sh\n[ \"$1 $2\" = \"which paddock-fake-agent\" ] && echo {}\n",
            real.display()
        ),
    );
    let f = HarnessFactory::new(&env).unwrap();
    assert_eq!(f.create("fake").unwrap().exe, real);
    let found = f.found("fake").unwrap();
    assert_eq!((found.by, found.dir), (FoundBy::Common, shims));
}

#[test]
fn a_program_on_no_folder_is_not_found() {
    let h = home();
    let env = thin(&h);
    user_profile(&env, FAKE);
    std::fs::create_dir_all(h.home.join(".local/bin")).unwrap();
    let f = HarnessFactory::new(&env).unwrap();
    let err = f.create("fake").unwrap_err().to_string();
    assert!(err.contains(".local/bin"), "{err}");
    assert!(f.found("fake").is_none());
}

#[test]
fn an_npm_agent_outside_path_finds_its_interpreter_in_a_common_folder() {
    let h = home();
    let env = isolated(&h);
    user_profile(&env, FAKE);
    let pkg = h.home.join(".npm-global/lib/node_modules/fake");
    let js = pkg.join("cli.js");
    script(&js, "#!/usr/bin/env node\n");
    let link = h.home.join(".npm-global/bin/paddock-fake-agent");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&js, &link).unwrap();
    let node = h.home.join(".local/bin/node");
    program(&node);
    let f = HarnessFactory::new(&env).unwrap();
    let harness = f.create("fake").unwrap();
    assert_eq!(harness.exe, js);
    assert!(
        harness.install_mounts.contains(&node),
        "{:?}",
        harness.install_mounts
    );
    assert!(harness
        .install_mounts
        .contains(&h.home.join(".npm-global/lib/node_modules")));
    assert!(harness.path_dirs.contains(&h.home.join(".local/bin")));
    // Never the whole home or ~/.npm-global.
    for m in &harness.install_mounts {
        assert!(*m != h.home && *m != h.home.join(".npm-global"), "{m:?}");
    }
}

#[test]
fn the_builtin_profiles_name_the_folders_of_their_installers() {
    let dirs = |name: &str| crate::profile::builtin(name).unwrap().locate.dirs;
    assert_eq!(dirs("grok"), ["~/.grok/bin"]);
    assert!(dirs("opencode").contains(&"~/.opencode/bin".to_string()));
    assert!(dirs("kimi").contains(&"~/.kimi-code/bin".to_string()));
    assert!(dirs("claude").contains(&"~/.claude/local".to_string()));
    assert!(dirs("omp").contains(&"~/.bun/bin".to_string()));
    for npm in ["codex", "gemini", "copilot", "qwen", "kilo", "claude"] {
        assert!(
            dirs(npm).contains(&"~/.npm-global/bin".to_string()),
            "{npm}"
        );
        assert!(dirs(npm).contains(&"~/.bun/bin".to_string()), "{npm}");
    }
}

#[test]
fn the_path_hint_fits_the_shell() {
    let h = home();
    let dir = h.home.join(".grok/bin");
    let hint = |shell: &str| path_hint(&env_with(&h, &[], &[("SHELL", shell)]), &dir);
    assert_eq!(
        hint("/bin/zsh"),
        "add to ~/.zshrc: export PATH=\"$HOME/.grok/bin:$PATH\""
    );
    assert_eq!(
        hint("/usr/bin/bash"),
        "add to ~/.bashrc: export PATH=\"$HOME/.grok/bin:$PATH\""
    );
    assert_eq!(
        hint("/opt/homebrew/bin/fish"),
        "run once: fish_add_path \"$HOME/.grok/bin\""
    );
    assert!(hint("").starts_with("add to the startup file of your shell"));
    let brew = path_hint(&thin(&h), Path::new("/opt/homebrew/bin"));
    assert!(
        brew.ends_with("export PATH=\"/opt/homebrew/bin:$PATH\""),
        "{brew}"
    );
}

#[test]
fn the_doctor_names_only_programs_outside_path() {
    let h = home();
    let env = env_with(&h, &[], &[("SHELL", "/bin/zsh")]);
    let dir = h.home.join(".grok/bin");
    let at = |by| {
        Some(Found {
            by,
            dir: dir.clone(),
        })
    };
    assert_eq!(
        crate::doctor::outside_path_line(&env, at(FoundBy::Path)),
        None
    );
    assert_eq!(crate::doctor::outside_path_line(&env, None), None);
    for by in [FoundBy::Profile, FoundBy::Common] {
        let line = crate::doctor::outside_path_line(&env, at(by)).unwrap();
        assert!(line.contains(&dir.display().to_string()), "{line}");
        assert!(line.contains("~/.zshrc"), "{line}");
    }
}

#[test]
fn shims_cover_a_harness_outside_path() {
    let h = home();
    let env = thin(&h);
    program(&h.home.join(".grok/bin/grok"));
    let shims = crate::shims::detect(&env).unwrap();
    assert!(
        shims.contains(&("grok".to_string(), "grok".to_string())),
        "{shims:?}"
    );
}
