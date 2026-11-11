//! The quiet steps of the golden: settings that stop background services
//! of the guest OS that phone home and have no use in a paddock guest
//! (the widgets, Siri and suggestions, the iCloud setup nags, News,
//! Stocks and Weather, the automatic macOS install, location services,
//! analytics and ads).
//!
//! Each one is a documented setting: a `defaults` key of the setting in
//! System Settings (named in `why`), `launchctl disable` of a user agent,
//! or `pluginkit -e ignore` of a widget extension. None of them touches a
//! folder that macOS guards with a privacy prompt.
//!
//! These steps are best effort. Since the firewall denies every connection
//! of the guest system without asking anyway (`owner.rs`), a quiet step
//! that does not take on a new macOS only means more lines in the
//! connection log, not a prompt. So a quiet step that its verify does not
//! prove is a warning of the setup, never a failed golden. The Mac check
//! `packaging/macos/checks/macvm-golden.sh quiet` measures each one.

use super::configure::Step;

/// The quiet steps, in the order the agent runs them, after
/// `configure::STEPS`.
pub const QUIET: &[Step] = &[
    Step {
        name: "quiet-widgets-hidden",
        as_root: false,
        argv: &[
            "/bin/sh",
            "-c",
            "/usr/bin/defaults write com.apple.WindowManager StandardHideWidgets -bool true && /usr/bin/defaults write com.apple.WindowManager StageManagerHideWidgets -bool true",
        ],
        why: "System Settings, Desktop & Dock, Show Widgets: no widgets on the desktop of the guest, which nobody sees",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "com.apple.WindowManager",
            "StandardHideWidgets",
        ],
        expect: "1",
    },
    Step {
        name: "quiet-widgets-ignored",
        as_root: false,
        argv: &[
            "/bin/sh",
            "-c",
            "for id in com.apple.weather.widget com.apple.stocks.widget com.apple.news.widget; do /usr/bin/pluginkit -e ignore -i \"$id\" 2>/dev/null; done; exit 0",
        ],
        why: "pluginkit -e ignore: the Weather, Stocks and News widget extensions do not run, so weather-edge, stocks-edge and news-edge get no connection",
        verify: &[
            "/bin/sh",
            "-c",
            "/usr/bin/pluginkit -m -i com.apple.weather.widget | /usr/bin/cut -c1",
        ],
        expect: "-",
    },
    Step {
        name: "quiet-siri-off",
        as_root: false,
        argv: &[
            "/bin/sh",
            "-c",
            "/usr/bin/defaults write com.apple.assistant.support 'Assistant Enabled' -bool false && /usr/bin/defaults write com.apple.assistant.support 'Siri Data Sharing Opt-In Status' -int 2 && /usr/bin/defaults write com.apple.Siri StatusMenuVisible -bool false && /usr/bin/defaults write com.apple.Siri UserHasDeclinedEnable -bool true",
        ],
        why: "System Settings, Apple Intelligence & Siri, Siri off, and Improve Siri off",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "com.apple.assistant.support",
            "Assistant Enabled",
        ],
        expect: "0",
    },
    Step {
        name: "quiet-suggestions-off",
        as_root: false,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "com.apple.lookup.shared",
            "LookupSuggestionsDisabled",
            "-bool",
            "true",
        ],
        why: "System Settings, Spotlight, Help Apple Improve Search off, and the suggestions of Look Up off",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "com.apple.lookup.shared",
            "LookupSuggestionsDisabled",
        ],
        expect: "1",
    },
    Step {
        name: "quiet-agents-off",
        as_root: false,
        argv: &[
            "/bin/sh",
            "-c",
            "u=$(/usr/bin/id -u); for l in com.apple.newsd com.apple.weatherd com.apple.assistantd com.apple.Siri.agent com.apple.parsec-fbf com.apple.suggestd; do /bin/launchctl disable \"gui/$u/$l\"; done",
        ],
        why: "launchctl disable of the News, Weather, Siri and suggestions agents of the user, from the next boot of the guest on",
        verify: &[
            "/bin/sh",
            "-c",
            "/bin/launchctl print-disabled \"gui/$(/usr/bin/id -u)\"",
        ],
        expect: "com.apple.newsd",
    },
    Step {
        name: "quiet-icloud-setup-seen",
        as_root: false,
        argv: &[
            "/bin/sh",
            "-c",
            "d=com.apple.SetupAssistant; /usr/bin/defaults write $d DidSeeCloudSetup -bool true && /usr/bin/defaults write $d DidSeeSiriSetup -bool true && /usr/bin/defaults write $d DidSeePrivacy -bool true && /usr/bin/defaults write $d LastSeenCloudProductVersion \"$(/usr/bin/sw_vers -productVersion)\" && /usr/bin/defaults write $d LastSeenBuddyBuildVersion \"$(/usr/bin/sw_vers -buildVersion)\" && /usr/bin/defaults write com.apple.systempreferences AttentionPrefBundleIDs -dict",
        ],
        why: "the Apple Account sign-in of the setup counts as seen, so no iCloud sign-in nag and no red badge on System Settings",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "com.apple.SetupAssistant",
            "DidSeeCloudSetup",
        ],
        expect: "1",
    },
    Step {
        name: "quiet-ads-off",
        as_root: false,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "com.apple.AdLib",
            "allowApplePersonalizedAdvertising",
            "-bool",
            "false",
        ],
        why: "System Settings, Privacy & Security, Apple Advertising, Personalized Ads off",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "com.apple.AdLib",
            "allowApplePersonalizedAdvertising",
        ],
        expect: "0",
    },
    Step {
        name: "quiet-no-macos-auto-install",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.SoftwareUpdate",
            "AutomaticallyInstallMacOSUpdates",
            "-bool",
            "false",
        ],
        why: "System Settings, General, Software Update, Automatic Updates, Install macOS updates off; paddock macvm update rebuilds the golden instead",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.SoftwareUpdate",
            "AutomaticallyInstallMacOSUpdates",
        ],
        expect: "0",
    },
    Step {
        name: "quiet-location-off",
        as_root: true,
        argv: &[
            "/usr/bin/defaults",
            "write",
            "/var/db/locationd/Library/Preferences/ByHost/com.apple.locationd",
            "LocationServicesEnabled",
            "-int",
            "0",
        ],
        why: "System Settings, Privacy & Security, Location Services off: the guest has no place to tell",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/var/db/locationd/Library/Preferences/ByHost/com.apple.locationd",
            "LocationServicesEnabled",
        ],
        expect: "0",
    },
    Step {
        name: "quiet-analytics-off",
        as_root: true,
        argv: &[
            "/bin/sh",
            "-c",
            "f='/Library/Application Support/CrashReporter/DiagnosticMessagesHistory.plist'; /usr/bin/defaults write \"$f\" AutoSubmit -bool false && /usr/bin/defaults write \"$f\" ThirdPartyDataSubmit -bool false && /usr/bin/defaults write /Library/Preferences/com.apple.SubmitDiagInfo AutoSubmit -bool false",
        ],
        why: "System Settings, Privacy & Security, Analytics & Improvements, Share Mac Analytics and Share with App Developers off",
        verify: &[
            "/usr/bin/defaults",
            "read",
            "/Library/Application Support/CrashReporter/DiagnosticMessagesHistory.plist",
            "AutoSubmit",
        ],
        expect: "0",
    },
];

/// Every step of the configure request: the steps of the golden, then the
/// quiet steps.
pub fn wire_all() -> Vec<super::proto::ConfigureStep> {
    super::configure::STEPS
        .iter()
        .chain(QUIET.iter())
        .map(Step::wire)
        .collect()
}

/// One line per quiet step, and the names of the quiet steps that the
/// results do not prove: warnings, never a failed golden.
pub fn report(results: &[super::proto::StepResult]) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut unproven = Vec::new();
    for s in QUIET {
        match results.iter().find(|r| r.name == s.name) {
            Some(r) => {
                let proven = s.proven(r);
                let line = super::text::configure_step_line(r, proven, s.expect);
                lines.push(super::text::configure_quiet_line(&line, proven));
                if !proven {
                    unproven.push(s.name.to_string());
                }
            }
            None => {
                let line = super::text::configure_step_missing(s.name);
                lines.push(super::text::configure_quiet_line(&line, false));
                unproven.push(s.name.to_string());
            }
        }
    }
    (lines, unproven)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macvm::configure::{is_system_command, STEPS};
    use crate::macvm::proto::StepResult;

    #[test]
    fn every_quiet_step_is_a_system_command_with_a_proof_and_a_setting() {
        let mut names = std::collections::HashSet::new();
        for s in QUIET {
            assert!(s.name.starts_with("quiet-"), "{}", s.name);
            assert!(names.insert(s.name), "{} twice", s.name);
            assert!(!STEPS.iter().any(|x| x.name == s.name), "{}", s.name);
            assert!(is_system_command(s.argv[0]), "{}", s.name);
            assert!(is_system_command(s.verify[0]), "{}", s.name);
            assert!(!s.expect.is_empty(), "{}", s.name);
            assert!(s.why.len() > 20, "{}", s.name);
            // No step reads or writes a folder that macOS guards with a
            // privacy prompt, and none runs a tool that needs one.
            let all = s.argv.join(" ") + &s.verify.join(" ");
            for guarded in [
                "Library/Mail",
                "Library/Messages",
                "Library/Safari",
                "Desktop",
                "Documents",
                "Downloads",
                "tmutil",
                "osascript",
                "tccutil",
            ] {
                assert!(!all.contains(guarded), "{} touches {guarded}", s.name);
            }
        }
        assert_eq!(QUIET.len(), 10);
        assert_eq!(wire_all().len(), STEPS.len() + QUIET.len());
        assert_eq!(wire_all().last().unwrap().name, "quiet-analytics-off");
    }

    #[test]
    fn an_unproven_quiet_step_is_a_warning_not_a_failure() {
        let mut results: Vec<StepResult> = QUIET
            .iter()
            .map(|s| StepResult {
                name: s.name.into(),
                ok: true,
                verify: s.expect.into(),
                code: Some(0),
                ..Default::default()
            })
            .collect();
        let (lines, unproven) = report(&results);
        assert!(unproven.is_empty());
        assert_eq!(lines.len(), QUIET.len());
        results[0].verify = "0".into();
        results.pop();
        let (lines, unproven) = report(&results);
        assert_eq!(unproven, [QUIET[0].name, QUIET[QUIET.len() - 1].name]);
        assert!(
            lines[0].contains("the golden goes on without it"),
            "{}",
            lines[0]
        );
        // The golden's own report knows nothing of the quiet steps.
        let (_, failed) = crate::macvm::configure::report(&results);
        assert!(failed.iter().all(|f| !f.starts_with("quiet-")));
    }
}
