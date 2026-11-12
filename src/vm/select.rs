//! The backend choice on macOS: `--backend seatbelt | vm | auto`. The rule
//! is a pure function of a few facts, so a table test covers it on every
//! system. `facts` reads them from a built run.

use crate::build::Built;
use crate::env::HostEnv;
use crate::spec::{GuiMode, MountKind};
use anyhow::{bail, Result};

/// The name of the VM backend.
pub const NAME: &str = "vm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Seatbelt,
    Vm,
}

impl Choice {
    pub fn name(self) -> &'static str {
        match self {
            Choice::Seatbelt => crate::backend::seatbelt::NAME,
            Choice::Vm => NAME,
        }
    }
}

/// What the choice depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// `paddock app run`.
    pub app: bool,
    /// The harness of the run, for the refusal text.
    pub harness: String,
    /// Features of the run that Seatbelt cannot give, by name.
    pub linux_only: Vec<String>,
    /// Who asks for Seatbelt: `[macos] backend = "seatbelt"` of the profile.
    pub wants_seatbelt: Option<String>,
    /// `[macos] backend = "vm"` of the user configuration.
    pub user_vm: bool,
    /// The VM files check: None when it passes, else the error.
    pub assets_error: Option<String>,
}

/// The refusal of a terminal run that needs what Seatbelt cannot give.
/// `auto` never moves a terminal agent into the VM by itself: the VM has
/// its own Linux programs, not the agent of this Mac.
pub fn seatbelt_refusal(harness: &str, need: &str) -> String {
    format!(
        "{harness} on macOS runs with Seatbelt, which cannot give {need}. Turn it off and start again, or start with --backend vm when {harness} is installed in the VM (paddock vm setup)"
    )
}

/// The backend for `--backend auto` on macOS, and the reason. A terminal
/// run gets Seatbelt. The VM only when the user asks for it: `--backend
/// vm`, or `[macos] backend = "vm"` in the user configuration.
pub fn decide(f: &Facts) -> Result<(Choice, String)> {
    if f.app {
        return Ok((
            Choice::Seatbelt,
            "app mode runs desktop apps with Seatbelt".into(),
        ));
    }
    if let Some(who) = &f.wants_seatbelt {
        if !f.linux_only.is_empty() {
            bail!(seatbelt_refusal(&f.harness, &f.linux_only.join(", ")));
        }
        return Ok((Choice::Seatbelt, format!("{who} asks for Seatbelt")));
    }
    if f.user_vm {
        return match &f.assets_error {
            None => Ok((
                Choice::Vm,
                "[macos] backend = \"vm\" in the user configuration".into(),
            )),
            Some(e) => bail!(
                "[macos] backend = \"vm\" in the user configuration asks for the VM, and the VM files are not set up ({e}). Run `paddock vm setup`, or start with --backend seatbelt"
            ),
        };
    }
    if !f.linux_only.is_empty() {
        bail!(seatbelt_refusal(&f.harness, &f.linux_only.join(", ")));
    }
    Ok((Choice::Seatbelt, "the default for a terminal run".into()))
}

/// The backend that a run asks for. On macOS, `auto` becomes Seatbelt or
/// the VM by [`decide`], and paddock prints the choice and the reason.
pub fn wanted(env: &HostEnv, built: &Built) -> Result<String> {
    let rule = cfg!(target_os = "macos")
        && built.backend == "auto"
        && built.spec.network != crate::spec::Network::Allowlist;
    if !rule {
        return Ok(built.backend.clone());
    }
    let (choice, why) = decide(&facts(env, built)?)?;
    eprintln!("paddock: backend {}: {why}", choice.name());
    Ok(choice.name().to_string())
}

/// The `[macos] backend` of the user configuration.
pub fn user_backend(env: &HostEnv) -> Result<Option<String>> {
    let file = env.paddock_config().join("config.toml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(None);
    };
    let doc: toml::Value = crate::manifest::user_config_doc(&text)
        .map_err(|_| anyhow::anyhow!("{} is invalid", file.display()))?;
    match doc.get("macos").and_then(|m| m.get("backend")) {
        None => Ok(None),
        Some(toml::Value::String(s)) if s == "vm" || s == "seatbelt" || s == "auto" => {
            Ok(Some(s.clone()))
        }
        Some(_) => bail!("[macos] backend must be \"seatbelt\", \"vm\" or \"auto\""),
    }
}

/// The features of a built run that Seatbelt cannot give.
/// The devices are not in it: Seatbelt gets them over a socket, and the VM
/// does not get them yet (`devices::start` says so in a notice). Borrow is
/// not in it either: Seatbelt sees the borrow folder at its host path
/// (`PADDOCK_BORROW_DIR`).
pub fn linux_only(s: &crate::spec::SandboxSpec) -> Vec<String> {
    let mut out = Vec::new();
    if s.gui.mode == GuiMode::Window {
        out.push("window mode".into());
    }
    if let Some(m) = s.mounts.iter().find(|m| m.kind == MountKind::Overlay) {
        out.push(format!("an overlay at {}", m.dst.display()));
    }
    if s.keyring || s.private_bus {
        out.push("the private keyring and session bus".into());
    }
    out
}

/// The facts of a built run.
/// `[macos] needs_pty` of the profile does not move the run into the VM:
/// Seatbelt starts the harness and says in one line that it makes its own
/// ptys (`seatbelt::run::start_pty`).
pub fn facts(env: &HostEnv, built: &Built) -> Result<Facts> {
    let mac = &built.profile.macos;
    if let Some(b) = mac.backend.as_deref().filter(|b| *b != "seatbelt") {
        bail!(
            "[macos] backend = {b:?} in the {} profile: only \"seatbelt\" is known",
            built.profile.name
        );
    }
    Ok(Facts {
        app: built.spec.gui.mode == GuiMode::App,
        harness: built.spec.harness.clone(),
        linux_only: linux_only(&built.spec),
        wants_seatbelt: (mac.backend.as_deref() == Some("seatbelt"))
            .then(|| format!("the {} profile ([macos] backend)", built.profile.name)),
        user_vm: user_backend(env)?.as_deref() == Some(NAME),
        assets_error: crate::vm::assets::check(&crate::vm::assets::assets_dir(env))
            .err()
            .map(|e| format!("{e:#}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> Facts {
        Facts::default()
    }

    #[test]
    fn the_auto_rule() {
        let missing = Some("cannot read assets.toml".to_string());
        let lin = vec!["window mode".to_string()];
        let sb = Some("the xcode profile ([macos] backend)".to_string());
        // (facts, the choice or None for a refusal, a word of the reason)
        let table: Vec<(Facts, Option<Choice>, &str)> = vec![
            (f(), Some(Choice::Seatbelt), "default"),
            // A terminal agent gets Seatbelt, whatever the VM files.
            (
                Facts {
                    assets_error: missing.clone(),
                    ..f()
                },
                Some(Choice::Seatbelt),
                "default",
            ),
            (
                Facts {
                    app: true,
                    linux_only: lin.clone(),
                    user_vm: true,
                    ..f()
                },
                Some(Choice::Seatbelt),
                "app mode",
            ),
            // A need that Seatbelt cannot give is a refusal, never a
            // silent move into the VM, set up or not.
            (
                Facts {
                    linux_only: lin.clone(),
                    ..f()
                },
                None,
                "--backend vm",
            ),
            (
                Facts {
                    linux_only: lin.clone(),
                    assets_error: missing.clone(),
                    ..f()
                },
                None,
                "window mode",
            ),
            (
                Facts {
                    wants_seatbelt: sb.clone(),
                    ..f()
                },
                Some(Choice::Seatbelt),
                "xcode",
            ),
            (
                Facts {
                    wants_seatbelt: sb.clone(),
                    user_vm: true,
                    ..f()
                },
                Some(Choice::Seatbelt),
                "xcode",
            ),
            (
                Facts {
                    wants_seatbelt: sb.clone(),
                    linux_only: lin.clone(),
                    ..f()
                },
                None,
                "window mode",
            ),
            // The user asks for the VM.
            (
                Facts {
                    user_vm: true,
                    ..f()
                },
                Some(Choice::Vm),
                "user configuration",
            ),
            (
                Facts {
                    user_vm: true,
                    linux_only: lin.clone(),
                    ..f()
                },
                Some(Choice::Vm),
                "user configuration",
            ),
            (
                Facts {
                    user_vm: true,
                    assets_error: missing.clone(),
                    ..f()
                },
                None,
                "--backend seatbelt",
            ),
        ];
        for (i, (facts, want, word)) in table.into_iter().enumerate() {
            match (decide(&facts), want) {
                (Ok((c, why)), Some(w)) => {
                    assert_eq!(c, w, "row {i}: {why}");
                    assert!(why.contains(word), "row {i}: {why}");
                }
                (Err(e), None) => assert!(e.to_string().contains(word), "row {i}: {e}"),
                (got, want) => panic!("row {i}: got {got:?}, want {want:?}"),
            }
        }
    }

    #[test]
    fn the_refusal_names_the_need_and_the_command_that_works() {
        let facts = Facts {
            harness: "codex".into(),
            linux_only: vec!["an overlay at /x".into(), "window mode".into()],
            assets_error: Some("cannot read /x/assets.toml".into()),
            ..f()
        };
        let e = decide(&facts).unwrap_err().to_string();
        assert_eq!(
            e,
            "codex on macOS runs with Seatbelt, which cannot give an overlay at /x, window mode. Turn it off and start again, or start with --backend vm when codex is installed in the VM (paddock vm setup)"
        );
        assert_eq!(e.lines().count(), 1);
    }

    /// MACRUN1: `paddock run codex` in a git project has a borrow folder.
    /// Seatbelt gives it, so the run stays in Seatbelt.
    #[test]
    fn borrow_is_not_a_reason_for_the_vm() {
        let home = std::path::PathBuf::from("/Users/u");
        let project = home.join("src/app");
        let mut s = crate::spec::SandboxSpec::new(
            "id1".into(),
            "codex",
            &project,
            "/Users/u/.local/share/paddock/homes/codex/x".into(),
            &home,
            501,
            20,
            "u",
        );
        s.add_mount(crate::spec::Mount::same(&project, true, "project"));
        s.add_mount(crate::spec::Mount::bind(
            "/Users/u/.local/share/paddock/borrow/id1",
            crate::borrow::MOUNT,
            true,
            "borrow folder",
        ));
        assert!(linux_only(&s).is_empty(), "{:?}", linux_only(&s));
        let facts = Facts {
            linux_only: linux_only(&s),
            ..f()
        };
        assert_eq!(decide(&facts).unwrap().0, Choice::Seatbelt);
    }

    #[test]
    fn user_backend_reads_the_macos_table() {
        let t = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(t.path().into(), "u".into(), 501, 20, Default::default());
        assert_eq!(user_backend(&env).unwrap(), None);
        let cfg = env.paddock_config();
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("config.toml"), "[macos]\nbackend = \"vm\"\n").unwrap();
        assert_eq!(user_backend(&env).unwrap().as_deref(), Some("vm"));
        std::fs::write(cfg.join("config.toml"), "[macos]\nbackend = \"bwrap\"\n").unwrap();
        assert!(user_backend(&env).is_err());
    }
}
