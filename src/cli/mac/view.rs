//! `paddock mac view`: the macOS guest of an app session inline in the
//! terminal (src/macvm/view). The target flags (--app, --instance,
//! --desktop) are the global ones of `paddock mac`.

use super::PickArgs;
use anyhow::Result;
use clap::Args;
use paddock::macvm::text;
use paddock::macvm::view::source::{FakeSource, FrameSource, InputSink, Recorder, Target};
use paddock::macvm::view::term::Protocol;
use std::time::Duration;

#[derive(Args, Debug, Clone)]
pub(crate) struct ViewArgs {
    /// The most frames per second; the guest sends a frame only when the
    /// picture changes.
    #[arg(long, value_name = "N", default_value_t = 30,
          value_parser = clap::value_parser!(u32).range(1..=60))]
    fps: u32,
    /// The image protocol: auto asks the terminal.
    #[arg(long, value_name = "NAME", default_value = "auto",
          value_parser = ["auto", "kitty", "iterm2", "sixel"])]
    protocol: String,
    /// Print what the terminal supports and exit.
    #[arg(long)]
    probe: bool,
    /// Show a moving test picture instead of a guest, to try the terminal
    /// side. Prints the actions that the input became at the end.
    #[arg(long)]
    fake_source: bool,
}

/// The exit code when the terminal shows no images, so no view started:
/// a script can tell it from a view that ran and ended (0) or failed (1).
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const NO_IMAGES_EXIT: i32 = 3;

/// The message and the exit code when no image protocol works.
#[cfg_attr(not(unix), allow(dead_code))]
fn no_view(notes: &[String], t: &Target) -> (String, i32) {
    let windows = matches!(t, Target::App { .. });
    (
        text::view_no_images(notes, &t.label(), windows),
        NO_IMAGES_EXIT,
    )
}

/// The target of the view. --instance and --desktop need --app here: the
/// view of no app is always the whole guest desktop.
fn target(p: &PickArgs) -> Result<Target> {
    if p.app.is_none() && (p.instance.is_some() || p.desktop) {
        anyhow::bail!(text::view_target_needs_app());
    }
    Ok(match &p.app {
        Some(name) if p.desktop => Target::Desktop {
            app: Some(name.clone()),
            instance: p.instance,
        },
        Some(name) => Target::App {
            name: name.clone(),
            instance: p.instance,
        },
        None => Target::Desktop {
            app: None,
            instance: None,
        },
    })
}

fn again(p: &PickArgs, a: &ViewArgs) -> String {
    let mut s = "paddock mac view".to_string();
    if let Some(name) = &p.app {
        s.push_str(&format!(" --app {name}"));
    }
    if let Some(n) = p.instance {
        s.push_str(&format!(" --instance {n}"));
    }
    if p.desktop {
        s.push_str(" --desktop");
    }
    if a.fake_source {
        s.push_str(" --fake-source");
    }
    s
}

/// The frame source and the input sink of a guest. The guest agent's
/// frame stream (`frames.subscribe`) and its input commands come with
/// the computer-use tools; until the view reads them, the view says so.
fn guest_ends(_t: &Target) -> Result<(Box<dyn FrameSource>, Box<dyn InputSink + Send>)> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!(text::view_needs_macos());
    }
    anyhow::bail!(text::view_stream_missing())
}

#[cfg(unix)]
pub(crate) fn view(p: &PickArgs, a: &ViewArgs) -> Result<i32> {
    use paddock::macvm::view::run::{self, Options};
    let forced = Protocol::parse(&a.protocol).map_err(anyhow::Error::msg)?;
    if a.probe {
        let (lines, found) = run::probe_report(forced)?;
        for line in lines {
            println!("{line}");
        }
        return Ok(if found { 0 } else { NO_IMAGES_EXIT });
    }
    let t = target(p)?;
    let opts = Options {
        target: t.clone(),
        max_fps: a.fps,
        protocol: forced,
        again: again(p, a),
    };
    let fallback = |notes: &[String]| -> Result<i32> {
        let (msg, code) = no_view(notes, &t);
        eprintln!("{msg}");
        Ok(code)
    };
    if a.fake_source {
        let mut src = FakeSource::new(480, 300, Duration::from_millis(200), None);
        let rec = Recorder::default();
        let actions = rec.actions.clone();
        let code = run::run(&opts, &mut src, Box::new(rec), &fallback)?;
        let actions = actions.lock().unwrap();
        eprintln!("{}", text::view_fake_actions(actions.len()));
        for act in actions.iter().rev().take(20).rev() {
            eprintln!("  {act:?}");
        }
        return Ok(code);
    }
    let (mut src, sink) = guest_ends(&t)?;
    run::run(&opts, src.as_mut(), sink, &fallback)
}

#[cfg(not(unix))]
pub(crate) fn view(_p: &PickArgs, _a: &ViewArgs) -> Result<i32> {
    anyhow::bail!(text::view_needs_macos())
}

#[cfg(test)]
mod tests {
    use super::super::{MacArgs, MacCmd};
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(flatten)]
        a: MacArgs,
    }

    /// The view's arguments, or the error of clap or of the target.
    fn parse(args: &[&str]) -> Result<(PickArgs, ViewArgs), String> {
        let mut v = vec!["t"];
        v.extend_from_slice(args);
        let t = T::try_parse_from(v).map_err(|e| e.to_string())?;
        let MacCmd::View(a) = t.a.cmd else {
            panic!("not the view")
        };
        target(&t.a.pick).map_err(|e| e.to_string())?;
        Ok((t.a.pick, a))
    }

    #[test]
    fn view_targets_and_the_command_again() {
        let (p, a) = parse(&["view"]).unwrap();
        assert_eq!(target(&p).unwrap().label(), "the guest desktop");
        assert_eq!(again(&p, &a), "paddock mac view");
        let (p, a) = parse(&["view", "--app", "notes", "--instance", "2"]).unwrap();
        assert_eq!(
            target(&p).unwrap(),
            Target::App {
                name: "notes".into(),
                instance: Some(2)
            }
        );
        assert_eq!(again(&p, &a), "paddock mac view --app notes --instance 2");
        let (p, _) = parse(&["view", "--app", "notes", "--desktop"]).unwrap();
        assert_eq!(target(&p).unwrap().label(), "the desktop of notes");
        // The target flags also go before the command.
        let (p, _) = parse(&["--app", "notes", "view"]).unwrap();
        assert_eq!(p.app.as_deref(), Some("notes"));
        assert!(parse(&["view", "--desktop"]).is_err());
        assert!(parse(&["view", "--instance", "2"]).is_err());
        assert!(parse(&["view", "--fps", "0"]).is_err());
        assert!(parse(&["view", "--protocol", "png"]).is_err());
    }

    #[test]
    fn no_image_protocol_exits_non_zero_and_says_why() {
        let notes = vec!["This terminal (Apple_Terminal) answers no image protocol.".to_string()];
        let app = Target::App {
            name: "notes".into(),
            instance: None,
        };
        let (msg, code) = no_view(&notes, &app);
        assert_ne!(code, 0, "a script must tell that no view started");
        assert_eq!(code, NO_IMAGES_EXIT);
        assert!(msg.contains("Apple_Terminal"), "{msg}");
        assert!(msg.contains("did not start"), "{msg}");
        assert!(msg.contains("--protocol"), "{msg}");
        assert!(msg.contains("host desktop"), "{msg}");
        // The whole desktop has no window on the host: do not promise one.
        let desk = Target::Desktop {
            app: None,
            instance: None,
        };
        let (msg, code) = no_view(&notes, &desk);
        assert_eq!(code, NO_IMAGES_EXIT);
        assert!(!msg.contains("host desktop"), "{msg}");
    }

    #[test]
    fn guest_ends_say_what_is_missing() {
        let Err(e) = guest_ends(&Target::Desktop {
            app: None,
            instance: None,
        }) else {
            panic!("no guest ends in this build");
        };
        let e = e.to_string();
        if cfg!(target_os = "macos") {
            assert!(e.contains("frames.subscribe"), "{e}");
        } else {
            assert!(e.contains("--fake-source"), "{e}");
        }
    }
}
