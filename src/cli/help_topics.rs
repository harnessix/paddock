use super::Cli;
use clap::CommandFactory;
use std::io::{self, Write};

struct Topic {
    name: &'static str,
    description: &'static str,
    text: &'static str,
}
struct Group {
    name: &'static str,
    topics: &'static [Topic],
}
const GROUPS: &[Group] = &[
    Group {
        name: "Start",
        topics: &[
            Topic {
                name: "overview",
                description: "What is paddock?",
                text: include_str!("../help/guide-overview.md"),
            },
            Topic {
                name: "why",
                description: "Why use a sandbox?",
                text: include_str!("../help/guide-why.md"),
            },
            Topic {
                name: "start",
                description: "How do I start a run?",
                text: include_str!("../help/guide-start.md"),
            },
            Topic {
                name: "demo",
                description: "What does a run look like?",
                text: include_str!("../help/guide-demo.md"),
            },
            Topic {
                name: "install",
                description: "How do I install and update paddock?",
                text: include_str!("../help/guide-install.md"),
            },
            Topic {
                name: "remove",
                description: "How do I remove paddock?",
                text: include_str!("../help/guide-remove.md"),
            },
        ],
    },
    Group {
        name: "Everyday use",
        topics: &[
            Topic {
                name: "usage",
                description: "How do I run an agent?",
                text: include_str!("../help/guide-usage.md"),
            },
            Topic {
                name: "sandbox",
                description: "What does a sandbox contain?",
                text: include_str!("../help/guide-sandbox.md"),
            },
            Topic {
                name: "undo-runs",
                description: "How do I undo a run?",
                text: include_str!("../help/guide-undo.md"),
            },
            Topic {
                name: "saved-files",
                description: "Where are saved files?",
                text: include_str!("../help/guide-files.md"),
            },
            Topic {
                name: "history",
                description: "Where is run history?",
                text: include_str!("../help/guide-history.md"),
            },
            Topic {
                name: "storage",
                description: "How do I manage disk space?",
                text: include_str!("../help/guide-storage.md"),
            },
            Topic {
                name: "desktop",
                description: "How do desktop sandboxes work?",
                text: include_str!("../help/guide-desktop.md"),
            },
            Topic {
                name: "defaults",
                description: "What are the Omarchy defaults?",
                text: include_str!("../help/guide-defaults.md"),
            },
            Topic {
                name: "app-launchers",
                description: "How do I set up launchers?",
                text: include_str!("../help/guide-launchers.md"),
            },
            Topic {
                name: "settings",
                description: "How do I configure paddock?",
                text: include_str!("../help/guide-settings.md"),
            },
            Topic {
                name: "logs",
                description: "Where are the logs?",
                text: include_str!("../help/guide-logs.md"),
            },
        ],
    },
    Group {
        name: "Safety and privacy",
        topics: &[
            Topic {
                name: "network",
                description: "How is network access controlled?",
                text: include_str!("../help/guide-network.md"),
            },
            Topic {
                name: "agent-logins",
                description: "How are agent logins handled?",
                text: include_str!("../help/guide-logins.md"),
            },
            Topic {
                name: "environment",
                description: "Which environment variables pass through?",
                text: include_str!("../help/guide-environment.md"),
            },
            Topic {
                name: "privacy",
                description: "What are the privacy defaults?",
                text: include_str!("../help/guide-privacy.md"),
            },
            Topic {
                name: "host-requests",
                description: "How do agents request host access?",
                text: include_str!("../help/guide-host.md"),
            },
            Topic {
                name: "limits",
                description: "What are the sandbox limits?",
                text: include_str!("../help/guide-limits.md"),
            },
        ],
    },
    Group {
        name: "Agents working together",
        topics: &[
            Topic {
                name: "shared-state",
                description: "How do agents share state?",
                text: include_str!("../help/guide-shared-state.md"),
            },
            Topic {
                name: "private-notes",
                description: "Where do private agent notes live?",
                text: include_str!("../help/guide-loft.md"),
            },
            Topic {
                name: "spec-kit",
                description: "How do Spec Kit projects work?",
                text: include_str!("../help/guide-spec-kit.md"),
            },
            Topic {
                name: "scheduled-jobs",
                description: "How do scheduled jobs work?",
                text: include_str!("../help/guide-turnout.md"),
            },
            Topic {
                name: "agent-review",
                description: "How are runs reviewed?",
                text: include_str!("../help/guide-trainer.md"),
            },
            Topic {
                name: "horse-interface",
                description: "How does the experimental horse interface work?",
                text: include_str!("../help/guide-horse.md"),
            },
            Topic {
                name: "timers",
                description: "How do task timers work?",
                text: include_str!("../help/guide-timers.md"),
            },
            Topic {
                name: "usage-limits",
                description: "How do I inspect usage limits?",
                text: include_str!("../help/guide-forage.md"),
            },
            Topic {
                name: "device-guide",
                description: "How do agents use devices?",
                text: include_str!("../help/guide-devices.md"),
            },
        ],
    },
    Group {
        name: "Platforms and reference",
        topics: &[
            Topic {
                name: "app-mode",
                description: "How do desktop app sessions work?",
                text: include_str!("../help/app-mode.md"),
            },
            Topic {
                name: "macos",
                description: "How does paddock work on a Mac?",
                text: include_str!("../help/macos.md"),
            },
            Topic {
                name: "backends",
                description: "Which sandbox backends are available?",
                text: include_str!("../help/guide-backends.md"),
            },
            Topic {
                name: "harness",
                description: "How do I add an agent harness?",
                text: include_str!("../help/guide-harness.md"),
            },
            Topic {
                name: "contents",
                description: "Where is the full guide index?",
                text: include_str!("../help/guide-contents.md"),
            },
            Topic {
                name: "support",
                description: "What is supported?",
                text: include_str!("../help/guide-support.md"),
            },
            Topic {
                name: "development",
                description: "How do I develop paddock?",
                text: include_str!("../help/guide-development.md"),
            },
            Topic {
                name: "license",
                description: "What is the license?",
                text: include_str!("../help/guide-license.md"),
            },
        ],
    },
];

fn listing() -> String {
    let mut out = String::new();
    for group in GROUPS {
        out.push_str(&format!("\n{}:\n", group.name));
        for topic in group.topics {
            out.push_str(&format!("  {:<16} {}\n", topic.name, topic.description));
        }
    }
    out
}

fn write_output(text: &str) -> i32 {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Ok(()) => 0,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            eprintln!("paddock: {e}");
            2
        }
    }
}

/// Handle help before clap so all help output handles a closed pipe quietly.
pub(crate) fn handle(args: &[std::ffi::OsString]) -> Option<i32> {
    if args.get(1).is_some_and(|a| a == "help") {
        if args.len() == 2 {
            let help = Cli::command().render_help().to_string();
            return Some(write_output(&format!(
                "{help}\n\nHelp topics:{}",
                listing()
            )));
        }
        let name = args[2].to_str()?;
        if let Some(topic) = GROUPS
            .iter()
            .flat_map(|g| g.topics)
            .find(|t| t.name == name)
        {
            if args.len() != 3 {
                eprintln!("paddock: a help topic takes no arguments");
                return Some(2);
            }
            return Some(write_output(&super::help_render::render(topic.text)));
        }
        if Cli::command().find_subcommand(name).is_none() {
            eprintln!(
                "paddock: unknown help topic or command: {name}\n\nHelp topics:{}",
                listing()
            );
            return Some(2);
        }
    }
    if args.get(1).is_some_and(|a| a == "help")
        || args.iter().skip(1).any(|a| a == "--help" || a == "-h")
    {
        if let Err(e) = Cli::command().try_get_matches_from(args) {
            if e.kind() == clap::error::ErrorKind::DisplayHelp {
                return Some(write_output(&e.to_string()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Cli, GROUPS};
    use clap::CommandFactory;
    #[test]
    fn topics_do_not_shadow_commands() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let command = Cli::command();
                for topic in GROUPS.iter().flat_map(|g| g.topics) {
                    assert!(
                        command.find_subcommand(topic.name).is_none(),
                        "{}",
                        topic.name
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
