//! One test per function of text/panel.rs, named as the function.

use super::super::panel as t;
use super::super::PanelStep;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn panel_title() {
    has(&t::panel_title(), &["paddock", "macOS guest"]);
}

#[test]
fn panel_step() {
    let clicks = [
        (
            PanelStep::Language,
            &[
                "Choose your language and region.",
                "English (US) or ABC",
                "Type buttons",
            ][..],
        ),
        (
            PanelStep::Skips,
            &["Skip", "sign-in", "Siri", "no Apple ID"],
        ),
        (
            PanelStep::Account,
            &[
                "Full name: paddock",
                "Password",
                &t::panel_type_password()[..13],
                "Verify",
            ],
        ),
        (
            PanelStep::Seed,
            &[
                "open Terminal",
                "Type the command",
                "Type the password",
                "press Return",
                "\nsudo /bin/sh \"/Volumes/My Shared Files/paddock-seed/install.sh\"",
            ],
        ),
        (
            PanelStep::Accessibility,
            &[
                "System Settings > Privacy & Security > Accessibility",
                "paddock-guest",
                "click Open System Settings",
            ],
        ),
        (PanelStep::Done, &["Done", "2 minutes"]),
    ];
    for (n, (step, names)) in clicks.iter().enumerate() {
        let open = t::panel_step(*step, false);
        assert!(open.starts_with(&format!("{}. ", n + 1)), "{open}");
        has(&open, names);
        let done = t::panel_step(*step, true);
        assert_eq!(done, format!("✓ {open}"));
    }
    // The first-run check reads step 1 by this sentence.
    assert!(t::panel_step(PanelStep::Language, false)
        .starts_with("1. Choose your language and region."));
    assert_eq!(PanelStep::ALL.len(), 6);
}

#[test]
fn panel_help() {
    for step in PanelStep::ALL {
        let s = t::panel_help(step);
        assert!(s.len() > 40, "{s}");
        assert_ne!(s, t::panel_step(step, false));
    }
    has(
        &t::panel_help(PanelStep::Language),
        &["US keyboard", "English (US) or ABC"],
    );
    has(
        &t::panel_help(PanelStep::Account),
        &["keyring", "German", "French", "Chinese"],
    );
    has(
        &t::panel_help(PanelStep::Seed),
        &[
            "read-only folder",
            "no administrator rights",
            "run it again",
        ],
    );
    has(&t::panel_help(PanelStep::Done), &["golden ready"]);
}

#[test]
fn panel_help_entry() {
    assert_eq!(t::panel_help_entry(3, "why"), "3. why");
}

#[test]
fn panel_stop() {
    has(&t::panel_stop(), &["Stop", "next time"]);
}

#[test]
fn panel_note() {
    has(
        &t::panel_note(),
        &[&t::panel_stop(), "account name is paddock", "Help"],
    );
}

#[test]
fn panel_password_label() {
    has(
        &t::panel_password_label(),
        &["account paddock", "20 lowercase letters"],
    );
}

#[test]
fn panel_password_set_label() {
    has(
        &t::panel_password_set_label(),
        &["account paddock", "you set", "hidden"],
    );
}

#[test]
fn panel_copy() {
    assert_eq!(t::panel_copy(), "Copy");
}

#[test]
fn panel_copied() {
    has(&t::panel_copied(), &["clipboard", "Type the password"]);
}

#[test]
fn panel_type_password() {
    assert_eq!(t::panel_type_password(), "Type the password into the guest");
}

#[test]
fn panel_type_command() {
    assert_eq!(t::panel_type_command(), "Type the command into the guest");
}

#[test]
fn panel_type_hint() {
    has(
        &t::panel_type_hint(),
        &["click into the field", "US keyboard", "English (US) or ABC"],
    );
}

#[test]
fn panel_typing() {
    has(
        &t::panel_typing(20),
        &["Typing 20 characters", "until it ends"],
    );
}

#[test]
fn panel_typed() {
    has(
        &t::panel_typed(20),
        &["Typed 20 characters", "ABC or English (US)", "type again"],
    );
}

#[test]
fn panel_cannot_type() {
    has(
        &t::panel_cannot_type('é'),
        &["'é'", "typed nothing", "by hand"],
    );
}

#[test]
fn panel_typing_busy() {
    has(&t::panel_typing_busy(), &["still typing", "click again"]);
}

#[test]
fn panel_wrong_password() {
    has(
        &t::panel_wrong_password(),
        &[
            "refused the password",
            "account paddock",
            &t::panel_use_password(),
            "keyring",
            "configure step again",
        ],
    );
}

#[test]
fn panel_password_field() {
    assert_eq!(
        t::panel_password_field(),
        "The password you set for paddock"
    );
}

#[test]
fn panel_use_password() {
    assert_eq!(t::panel_use_password(), "Use this password");
}

#[test]
fn keyring_password_invalid() {
    has(
        &t::keyring_password_invalid(128),
        &["password", "1 to 128 characters", "line break"],
    );
}

#[test]
fn configure_wrong_password() {
    has(
        &t::configure_wrong_password(),
        &["refused the password", "paddock", "sudo", "paddock window"],
    );
}

#[test]
fn password_replaced() {
    has(
        &t::password_replaced(),
        &["keyring", "configure step again"],
    );
}

#[test]
fn password_refused() {
    has(
        &t::password_refused(5),
        &["refused the password", "account paddock", "5 times"],
    );
    assert!(t::password_refused(1).ends_with("paddock once"));
}
