use super::paths::{bin_helpers, helpers_in, BinDir};
use super::*;
use std::path::Component;

/// Harnesses without a flag for no approvals: they ask none (pi, ori,
/// freebuff, mcode's terminal UI), take the mode from a variable (dsh),
/// or run on the host (openclaw).
const NO_APPROVAL_FLAG: &[&str] = &["pi", "ori", "freebuff", "mcode", "dsh", "openclaw"];

#[test]
fn builtin_profiles_parse_and_have_yolo_args() {
    for (name, text) in BUILTIN {
        let p: Profile = toml::from_str(text).unwrap();
        assert_eq!(&p.name, name);
        // Desktop apps keep their approval setting in their own settings.
        // Some harnesses ask for no approvals, or take the mode from a
        // variable that omarchy-agent sets.
        if p.app.is_none() && !NO_APPROVAL_FLAG.contains(name) {
            assert!(!p.launch.yolo_args.is_empty(), "{name} has no yolo_args");
        }
    }
}

#[test]
fn builtin_privacy_entries_are_well_formed() {
    for (name, text) in BUILTIN {
        let p: Profile = toml::from_str(text).unwrap();
        let t = p.privacy.training.as_ref();
        assert!(t.is_some(), "{name} has no privacy.training note");
        assert!(t.unwrap().url.starts_with("https://"), "{name}");
        for s in &p.privacy.settings {
            assert!(
                s.format == "json" || s.format == "toml",
                "{name}: {}",
                s.file
            );
            assert!(!s.path.is_empty(), "{name}: {}", s.file);
            let rel = Path::new(&s.file);
            assert!(
                rel.is_relative() && rel.components().all(|c| matches!(c, Component::Normal(_))),
                "{name}: {} must stay inside the sandbox home",
                s.file
            );
        }
    }
}

#[test]
fn one_shot_commands_never_carry_the_prompt_in_argv() {
    for (name, text) in BUILTIN {
        let p: Profile = toml::from_str(text).unwrap();
        let Some(o) = p.oneshot else { continue };
        let file_args = o.args.iter().filter(|a| a.contains(PROMPT_FILE)).count();
        match o.prompt {
            PromptVia::Stdin => assert_eq!(file_args, 0, "{name}"),
            PromptVia::File => assert_eq!(file_args, 1, "{name} must name the prompt file"),
        }
        let secret = "PROMPT-TEXT-THAT-MUST-NOT-LEAK";
        let argv = o.render(Path::new("/home/demo/.paddock-spawn/prompt.md"));
        assert!(argv.iter().all(|a| !a.contains(secret)), "{name}");
    }
}

#[test]
fn detach_args_name_the_prompt_file_once() {
    let mut with = vec![];
    for (name, text) in BUILTIN {
        let p: Profile = toml::from_str(text).unwrap();
        let Some(d) = p.detach else { continue };
        with.push(p.name.clone());
        let n: usize = d.args.iter().map(|a| a.matches(PROMPT_FILE).count()).sum();
        assert_eq!(n, 1, "{name} must name the prompt file once");
    }
    for h in ["claude", "codex", "grok", "agy"] {
        assert!(with.iter().any(|w| w == h), "{h} has no [detach]");
    }
}

#[test]
fn install_root_rules() {
    assert_eq!(
        install_root(Path::new(
            "/x/opencode/2.2.0/node_modules/opencode/bin/cli.js"
        )),
        PathBuf::from("/x/opencode/2.2.0/node_modules")
    );
    // Another repository's package: its node_modules, never the repository.
    assert_eq!(
        install_root(Path::new(
            "/home/demo/other/node_modules/pkg/node_modules/dep/cli.js"
        )),
        PathBuf::from("/home/demo/other/node_modules")
    );
    for single in ["/home/demo/.cargo/bin/tool", "/home/demo/go/bin/tool"] {
        assert_eq!(install_root(Path::new(single)), PathBuf::from(single));
    }
    assert_eq!(
        install_root(Path::new("/usr/bin/claude")),
        PathBuf::from("/usr/bin/claude")
    );
    assert_eq!(
        install_root(Path::new("/x/claude/2.1/claude")),
        PathBuf::from("/x/claude/2.1")
    );
    assert_eq!(
        install_root(Path::new("/x/node/26/bin/node")),
        PathBuf::from("/x/node/26")
    );
}

#[test]
fn a_user_profile_names_builtin_training_rules_and_cannot_bring_its_own() {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
    let dir = env.paddock_config().join("harness.d");
    std::fs::create_dir_all(&dir).unwrap();
    let base = "name = \"mine\"\n[locate]\ncommands = [\"sh\"]\n[privacy.training]\nsetting = \"s\"\nurl = \"https://example.com\"\n";
    std::fs::write(
        dir.join("mine.toml"),
        format!("{base}rules = [\"agy-1.4-data-use\"]\n"),
    )
    .unwrap();
    let f = HarnessFactory::new(&env).unwrap();
    let rules = f.profile("mine").unwrap().training_rules();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "agy-1.4-data-use");

    std::fs::write(
        dir.join("mine.toml"),
        format!("{base}rules = [\"no-such-rule\"]\n"),
    )
    .unwrap();
    assert!(HarnessFactory::new(&env).is_err());

    let own = "[[privacy.training.rule]]\nid = \"x\"\nurl = \"u\"\ntitle = \"T\"\n[[privacy.training.rule.state]]\nid = \"a\"\nrows = [\"T\"]\nsend = \"enter\"\nnext = [\"gone\"]\n";
    std::fs::write(dir.join("mine.toml"), format!("{base}{own}")).unwrap();
    let e = HarnessFactory::new(&env).err().unwrap();
    assert!(format!("{e:#}").contains("built-in"), "{e:#}");
}

#[test]
fn model_and_effort_args_name_their_placeholder_once() {
    for (name, text) in BUILTIN {
        let p: Profile = toml::from_str(text).unwrap();
        let Some(o) = p.oneshot else { continue };
        for (args, key) in [
            (&o.model_args, crate::spawn::model::MODEL),
            (&o.effort_args, crate::spawn::model::EFFORT),
        ] {
            let n: usize = args.iter().map(|a| a.matches(key).count()).sum();
            assert!(args.is_empty() || n == 1, "{name}: {key} once");
            assert!(args.iter().all(|a| !a.contains(PROMPT_FILE)), "{name}");
        }
    }
}

#[test]
fn a_link_to_a_multi_call_program_keeps_its_name() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    std::fs::write(root.join("busybox"), "").unwrap();
    std::fs::write(root.join("tool"), "").unwrap();
    std::os::unix::fs::symlink(root.join("busybox"), root.join("sh")).unwrap();
    std::os::unix::fs::symlink(root.join("tool"), root.join("t")).unwrap();
    assert_eq!(canonical(&root.join("sh")).unwrap(), root.join("sh"));
    assert_eq!(
        canonical(&root.join("busybox")).unwrap(),
        root.join("busybox")
    );
    assert_eq!(canonical(&root.join("t")).unwrap(), root.join("tool"));
}

fn program(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "\x7fELF").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn closure_of(exe: &Path) -> Vec<PathBuf> {
    closure_in(Path::new("/nonexistent"), exe)
}

/// The closure of a codex harness with `home` as the host home.
fn closure_in(home: &Path, exe: &Path) -> Vec<PathBuf> {
    let env = HostEnv::from_parts(home.to_path_buf(), "t".into(), 1, 1, Default::default());
    let listed = builtin("codex").unwrap().locate.helpers;
    harness_closure(&env, exe, &listed).unwrap().0
}

fn script(path: &Path, interpreter: &Path) {
    program(path);
    std::fs::write(path, format!("#!{}\n", interpreter.display())).unwrap();
}

#[test]
fn a_harness_in_a_home_bin_dir_gets_its_helpers_and_nothing_else() {
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().canonicalize().unwrap().join(".local/bin");
    for name in [
        "codex",
        "codex-code-mode-host",
        "codex-exec-server",
        "other",
    ] {
        program(&bin.join(name));
    }
    // Not a program: a helper must be executable.
    std::fs::write(bin.join("codex-notes"), "text").unwrap();
    assert_eq!(
        closure_of(&bin.join("codex")),
        vec![
            bin.join("codex"),
            bin.join("codex-code-mode-host"),
            bin.join("codex-exec-server"),
        ]
    );
}

#[test]
fn a_helper_that_is_a_symlink_resolves() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let bin = root.join(".local/bin");
    // A known tool install: the helper brings its install folder.
    let install = root.join(".local/share/mise/installs/codex/1.0");
    program(&bin.join("codex"));
    program(&install.join("bin/codex-exec-server"));
    std::os::unix::fs::symlink(
        install.join("bin/codex-exec-server"),
        bin.join("codex-exec-server"),
    )
    .unwrap();
    // A dangling link is no helper.
    std::os::unix::fs::symlink(root.join("gone"), bin.join("codex-gone")).unwrap();
    assert_eq!(
        bin_helpers(&bin.join("codex"), &[]),
        vec![(
            bin.join("codex-exec-server"),
            install.join("bin/codex-exec-server")
        )]
    );
    // The name the harness looks up, and the install it leads to.
    assert_eq!(
        closure_of(&bin.join("codex")),
        vec![bin.join("codex"), bin.join("codex-exec-server"), install]
    );
}

#[test]
fn a_harness_in_its_own_install_dir_keeps_the_dir_mount() {
    let t = tempfile::tempdir().unwrap();
    let install = t.path().canonicalize().unwrap().join("tools/codex/1.0");
    for name in ["codex", "codex-exec-server", "other"] {
        program(&install.join("bin").join(name));
    }
    let exe = install.join("bin/codex");
    assert!(bin_helpers(&exe, &["exec-server".into()]).is_empty());
    assert_eq!(closure_of(&exe), vec![install]);
}

#[test]
fn a_helper_link_into_a_home_folder_mounts_only_its_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    let bin = home.join(".local/bin");
    program(&bin.join("codex"));
    // A script the user keeps in ~/bin, linked as a codex helper.
    program(&home.join("bin/codex-review"));
    std::os::unix::fs::symlink(home.join("bin/codex-review"), bin.join("codex-review")).unwrap();
    // A link into a dotfiles folder outside the home.
    program(&root.join("dotfiles/x"));
    std::os::unix::fs::symlink(root.join("dotfiles/x"), bin.join("codex-x")).unwrap();
    let closure = closure_in(&home, &bin.join("codex"));
    assert!(!closure.contains(&home), "{closure:?}");
    assert!(!closure.contains(&root.join("dotfiles")), "{closure:?}");
    assert_eq!(
        closure,
        vec![
            root.join("dotfiles/x"),
            bin.join("codex"),
            bin.join("codex-review"),
            bin.join("codex-x"),
            home.join("bin/codex-review"),
        ]
    );
}

#[test]
fn a_helper_in_a_tool_install_never_mounts_the_home_or_above() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    // A home whose path has a tool install marker: the install folder of
    // the helper is the home itself.
    let home = root.join("mise/installs/me");
    let bin = home.join(".local/bin");
    program(&bin.join("codex"));
    program(&home.join("bin/codex-x"));
    std::os::unix::fs::symlink(home.join("bin/codex-x"), bin.join("codex-x")).unwrap();
    let closure = closure_in(&home, &bin.join("codex"));
    assert!(closure.iter().all(|m| !home.starts_with(m)), "{closure:?}");
    assert!(closure.contains(&home.join("bin/codex-x")), "{closure:?}");
}

#[test]
fn a_helper_script_header_does_not_take_a_later_helper_target() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let bin = root.join(".local/bin");
    program(&bin.join("codex"));
    // codex-b comes off the old shared queue first. Its header leads
    // through four interpreters and used up the budget.
    let interp = root.join("interp");
    program(&interp.join("i4"));
    script(&interp.join("i3"), &interp.join("i4"));
    script(&interp.join("i2"), &interp.join("i3"));
    script(&interp.join("i1"), &interp.join("i2"));
    script(&bin.join("codex-b"), &interp.join("i1"));
    let install = root.join(".local/share/mise/installs/codex/1.0");
    program(&install.join("bin/codex-a"));
    std::os::unix::fs::symlink(install.join("bin/codex-a"), bin.join("codex-a")).unwrap();
    let closure = closure_of(&bin.join("codex"));
    assert!(closure.contains(&install), "{closure:?}");
    // codex-b keeps its whole budget: itself and three interpreters. Each
    // interpreter of a helper is mounted as a file, not its folder.
    for step in ["i1", "i2", "i3"] {
        assert!(closure.contains(&interp.join(step)), "{closure:?}");
    }
    assert!(!closure.contains(&interp.join("i4")), "{closure:?}");
    assert!(!closure.contains(&interp), "{closure:?}");
}

#[test]
fn a_helper_interpreter_in_a_home_folder_mounts_only_its_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    let bin = home.join(".local/bin");
    program(&bin.join("codex"));
    // ~/.local/bin/codex-review links to ~/bin/codex-review, whose header
    // names ~/bin/mytool. install_root of ~/bin/mytool is the home.
    program(&home.join("bin/mytool"));
    script(&home.join("bin/codex-review"), &home.join("bin/mytool"));
    std::os::unix::fs::symlink(home.join("bin/codex-review"), bin.join("codex-review")).unwrap();
    // A helper whose header names a script in a dotfiles folder.
    program(&root.join("dotfiles/run"));
    script(&bin.join("codex-dots"), &root.join("dotfiles/run"));
    let closure = closure_in(&home, &bin.join("codex"));
    assert!(closure.iter().all(|m| !home.starts_with(m)), "{closure:?}");
    assert!(!closure.contains(&root.join("dotfiles")), "{closure:?}");
    assert!(!closure.contains(&home.join("bin")), "{closure:?}");
    assert_eq!(
        closure,
        vec![
            root.join("dotfiles/run"),
            bin.join("codex"),
            bin.join("codex-dots"),
            bin.join("codex-review"),
            home.join("bin/codex-review"),
            home.join("bin/mytool"),
        ]
    );
}

/// The closure of a codex harness with `home` as the host home and `path`
/// as the host PATH.
fn closure_with_path(home: &Path, path: &[PathBuf], exe: &Path) -> Vec<PathBuf> {
    let vars = [(
        "PATH".to_string(),
        std::env::join_paths(path).unwrap().into_string().unwrap(),
    )]
    .into();
    let env = HostEnv::from_parts(home.to_path_buf(), "t".into(), 1, 1, vars);
    let listed = builtin("codex").unwrap().locate.helpers;
    harness_closure(&env, exe, &listed).unwrap().0
}

fn env_script(path: &Path, name: &str) {
    program(path);
    std::fs::write(path, format!("#!/usr/bin/env {name}\n")).unwrap();
}

#[test]
fn a_harness_interpreter_in_a_home_folder_mounts_only_its_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    let bin = home.join(".local/bin");
    // ~/.local/bin/codex runs on the node that PATH finds in ~/bin.
    // install_root of ~/bin/node is the home.
    env_script(&bin.join("codex"), "node");
    program(&home.join("bin/node"));
    let closure = closure_with_path(&home, &[home.join("bin")], &bin.join("codex"));
    assert!(closure.iter().all(|m| !home.starts_with(m)), "{closure:?}");
    assert!(!closure.contains(&home.join("bin")), "{closure:?}");
    assert_eq!(closure, vec![bin.join("codex"), home.join("bin/node")]);
}

#[test]
fn every_interpreter_step_of_a_harness_gets_the_home_guard() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    let bin = home.join(".local/bin");
    // codex -> ~/bin/wrap -> ~/.cargo/run -> ~/.local/node: the folder of
    // each step holds the home, ~/.cargo with its credentials, or
    // ~/.local with ~/.local/bin.
    env_script(&bin.join("codex"), "wrap");
    env_script(&home.join("bin/wrap"), "run");
    env_script(&home.join(".cargo/run"), "node");
    program(&home.join(".local/node"));
    std::fs::write(home.join(".cargo/credentials.toml"), "token").unwrap();
    let path = [home.join("bin"), home.join(".cargo"), home.join(".local")];
    // The host home in another spelling: a link to the real home.
    let link = root.join("home-link");
    std::os::unix::fs::symlink(&home, &link).unwrap();
    for given in [&home, &link] {
        let closure = closure_with_path(given, &path, &bin.join("codex"));
        for wide in [
            &home,
            &home.join("bin"),
            &home.join(".cargo"),
            &home.join(".local"),
        ] {
            assert!(!closure.contains(wide), "{given:?}: {closure:?}");
        }
        assert_eq!(
            closure,
            vec![
                home.join(".cargo/run"),
                home.join(".local/bin/codex"),
                home.join(".local/node"),
                home.join("bin/wrap"),
            ],
            "{given:?}"
        );
    }
}

#[test]
fn a_harness_executable_in_a_home_bin_folder_mounts_only_its_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    // ~/bin/codex: install_root is the home.
    program(&home.join("bin/codex"));
    let closure = closure_in(&home, &home.join("bin/codex"));
    assert_eq!(closure, vec![home.join("bin/codex")]);
}

#[test]
fn a_harness_interpreter_in_its_own_install_keeps_the_install_mount() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let home = root.join("home");
    let bin = home.join(".local/bin");
    let nvm = home.join(".nvm/versions/node/v20");
    env_script(&bin.join("codex"), "node");
    program(&nvm.join("bin/node"));
    let closure = closure_with_path(&home, &[nvm.join("bin")], &bin.join("codex"));
    assert_eq!(closure, vec![bin.join("codex"), nvm]);
}

#[test]
fn harness_root_never_gives_a_folder_that_shows_the_home() {
    let env = HostEnv::from_parts("/home/demo".into(), "t".into(), 1, 1, Default::default());
    let root = |p: &str| super::closure::harness_root(&env, Path::new(p));
    for file in [
        "/home/demo/bin/node",
        "/home/demo/node",
        "/home/USER",
        "/home/demo/.cargo/tool",
        "/home/demo/.local/node",
        "/home/demo/go/tool",
        "/home/demo/.local/bin/codex",
        "/opt/bin/node",
        "/usr/lib/node",
        "/usr/local/bin/node",
    ] {
        assert_eq!(root(file), PathBuf::from(file), "{file}");
    }
    for (file, install) in [
        (
            "/home/demo/.nvm/versions/node/v20/bin/node",
            "/home/demo/.nvm/versions/node/v20",
        ),
        (
            "/home/demo/.npm/lib/node_modules/@openai/codex/bin/codex.js",
            "/home/demo/.npm/lib/node_modules",
        ),
        (
            "/home/demo/.local/bin/tools/node",
            "/home/demo/.local/bin/tools",
        ),
        ("/opt/node/bin/node", "/opt/node"),
        ("/usr/lib/jvm/java-21/bin/java", "/usr/lib/jvm/java-21"),
    ] {
        assert_eq!(root(file), PathBuf::from(install), "{file}");
    }
}

#[test]
fn a_helper_in_a_tool_install_never_mounts_a_home_bin_parent() {
    // A home inside a tool install: install_root of ~/.cargo/tool is
    // ~/.cargo, which holds ~/.cargo/bin and credentials.toml.
    let env = HostEnv::from_parts(
        "/srv/mise/installs/me".into(),
        "t".into(),
        1,
        1,
        Default::default(),
    );
    let file = Path::new("/srv/mise/installs/me/.cargo/tool");
    assert_eq!(super::closure::helper_root(&env, file), file);
}

#[test]
fn a_helper_in_a_shared_system_folder_mounts_only_its_file() {
    let env = HostEnv::from_parts("/home/demo".into(), "t".into(), 1, 1, Default::default());
    let root = |p: &str| super::closure::helper_root(&env, Path::new(p));
    // install_root gives /usr/lib, /opt or a multiarch folder here.
    for file in [
        "/usr/lib/codex-x",
        "/usr/lib/codex/codex-x",
        "/usr/lib/x86_64-linux-gnu/codex-x",
        "/opt/bin/codex-x",
        "/usr/local/lib/bin/codex-x",
    ] {
        assert_eq!(root(file), PathBuf::from(file), "{file}");
    }
    // A package folder in a system folder keeps its mount.
    assert_eq!(root("/opt/codex/bin/codex-x"), PathBuf::from("/opt/codex"));
    assert_eq!(
        root("/usr/lib/node_modules/@openai/codex/bin/codex-x"),
        PathBuf::from("/usr/lib/node_modules")
    );
}

#[test]
fn a_system_bin_dir_gives_only_the_listed_helpers() {
    let t = tempfile::tempdir().unwrap();
    // A fake /usr/bin.
    let usr_bin = t.path().canonicalize().unwrap().join("usr/bin");
    for name in [
        "claude",
        "claude-desktop",
        "codex",
        "codex-exec-server",
        "codex-other",
    ] {
        program(&usr_bin.join(name));
    }
    let claude = builtin("claude").unwrap().locate.helpers;
    assert!(helpers_in(&usr_bin, "claude", BinDir::System, &claude).is_empty());
    let codex = builtin("codex").unwrap().locate.helpers;
    assert_eq!(codex, vec!["code-mode-host", "exec-server"]);
    assert_eq!(
        helpers_in(&usr_bin, "codex", BinDir::System, &codex),
        vec![(
            usr_bin.join("codex-exec-server"),
            usr_bin.join("codex-exec-server")
        )]
    );
    // A home bin folder keeps every executable file named <exe>-*.
    std::fs::create_dir(usr_bin.join("claude-dir")).unwrap();
    assert_eq!(
        helpers_in(&usr_bin, "claude", BinDir::Home, &claude),
        vec![(
            usr_bin.join("claude-desktop"),
            usr_bin.join("claude-desktop")
        )]
    );
}
