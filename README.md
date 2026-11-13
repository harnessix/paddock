# paddock

<p align="center">
  <img src="docs/assets/hero.png" alt="paddock: full auto-approval, nothing outside the fence" width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-192327" alt="License: MIT"></a>
</p>

paddock is a sandbox for coding agents such as Claude Code, Codex, Grok
Build, agy (Antigravity), Gemini CLI, opencode and Copilot CLI, on Linux
(Omarchy) and macOS. On a Mac, terminal agents run under Seatbelt, and
desktop apps run in paddock's own macOS guest VM. `paddock help macos`
lists what is not confirmed on a Mac yet.

## Your computer stays yours

You use your agents as usual, and you approve their steps one by one. Each
agent lives in its own room. It cannot touch your keys or your other
projects. Its changes land on a copy first, and you decide what goes back.

When you trust an agent with a task, you can let it work on its own:
`paddock claude --auto`. The room is the same in both modes.

## Watch a run

![A terminal recording of paddock running an agent](docs/assets/demo.gif)

## Why use paddock?

Coding agents can run commands and edit the project while the sandbox limits
access to the rest of your machine. Each run gets a private home, and the
agent can ask for host access when it needs it. Snapshots let you undo a run.
See `paddock help why` for more.

## Get started in 30 seconds

Install paddock below, then run:

```sh
paddock setup
cd ~/src/my-app
paddock claude             # manual: you approve each step in Claude Code
paddock claude --auto      # opt-in: the agent works on its own in its room
```

You can use `paddock codex` or another installed agent in place of `claude`.
Manual is the default of a new installation. To make auto your default, set
`[run] approvals = "auto"` in `~/.config/paddock/config.toml`.
By default, environment variables pass through except names that look like
secrets. See `paddock help environment` for the details and controls.

## Install on Linux

From a release clone:

```sh
git clone --branch v0.2.4 https://github.com/harnessix/paddock
cd paddock
bin/install
```

Or download the prebuilt binary and its checksum:

```sh
curl -fLO https://github.com/harnessix/paddock/releases/download/v0.2.4/paddock-0.2.4-x86_64-linux.tar.gz
curl -fLO https://github.com/harnessix/paddock/releases/download/v0.2.4/paddock-0.2.4-x86_64-linux.tar.gz.sha256
sha256sum -c paddock-0.2.4-x86_64-linux.tar.gz.sha256
tar -xzf paddock-0.2.4-x86_64-linux.tar.gz
install -m 755 paddock-0.2.4-x86_64-linux/paddock ~/.local/bin/paddock
install -m 755 paddock-0.2.4-x86_64-linux/paddock-tui ~/.local/bin/paddock-tui
paddock doctor
```

For 64-bit Arm, replace `x86_64` with `aarch64`. See
`paddock help install` for dependencies, updates and other options.

To check that a download came from the release workflow, see `paddock help install`.

## Install on macOS

On Apple silicon, install with Homebrew:

```sh
brew tap harnessix/paddock
brew install paddock tmux
paddock setup
paddock doctor
```

You can also build from a clone with `cargo install --path .`. See
`paddock help install` for the full setup and `paddock help macos` for
app mode, terminal runs and current Mac limitations.

## Everyday commands

| Command | Purpose |
| --- | --- |
| `paddock` | Open the home screen in a terminal: start an agent, open an app, answer requests. |
| `paddock codex` | Start an installed agent in the current project. |
| `paddock run claude PATH` | Start an agent in a chosen project. |
| `paddock ps` | List running sandboxes. |
| `paddock board` | Show every project, ticket and agent run, and what waits for you. |
| `paddock split` | Show an agent in its sandbox and a host shell in one window, each pane labeled sandbox or host. |
| `paddock tui` | Open the terminal interface: runs, approvals, mail and the board, with the keyboard or the mouse. |
| `paddock explain codex` | Show mounts, environment settings and their origins before a run. |
| `paddock run claude --copy` | Let the agent work on a copy. Your folder changes only when you apply. |
| `paddock changes` | List what a run changed in its copy. |
| `paddock apply 1a2b3c` | Write the changes of a copy to your folder, after a backup. |
| `paddock undo` | Take the last apply back, or restore files from a run's snapshot. |
| `paddock doctor` | Check the system and its dependencies. |
| `paddock gateway openclaw start` | Run the OpenClaw gateway in a sandbox of its own. `paddock openclaw tui` attaches to it. |

Run `paddock help` for all commands, or `paddock help COMMAND` for one.
Run `paddock help TOPIC` to read the full guide for a topic.

## Help topics

The topics are built into the executable and available offline. Run `paddock help TOPIC` to read one.

### Start

- `overview` — What is paddock?
- `why` — Why use a sandbox?
- `start` — How do I start a run?
- `demo` — What does a run look like?
- `install` — How do I install and update paddock?
- `remove` — How do I remove paddock?

### Everyday use

- `usage` — How do I run an agent?
- `sandbox` — What does a sandbox contain?
- `undo-runs` — How do I undo a run?
- `saved-files` — Where are saved files?
- `history` — Where is run history?
- `storage` — How do I manage disk space?
- `desktop` — How do desktop sandboxes work?
- `defaults` — What are the Omarchy defaults?
- `app-launchers` — How do I set up launchers?
- `settings` — How do I configure paddock?
- `split-guide` — How do I see several terminals in one window?
- `logs` — Where are the logs?

### Safety and privacy

- `network` — How is network access controlled?
- `agent-logins` — How are agent logins handled?
- `environment` — Which environment variables pass through?
- `privacy` — What are the privacy defaults?
- `host-requests` — How do agents request host access?
- `limits` — What are the sandbox limits?

### Agents working together

- `shared-state` — How do agents share state?
- `private-notes` — Where do private agent notes live?
- `spec-kit` — How do Spec Kit projects work?
- `scheduled-jobs` — How do scheduled jobs work?
- `agent-review` — How are runs reviewed?
- `horse-interface` — How does the experimental horse interface work?
- `timers` — How do task timers work?
- `usage-limits` — How do I inspect usage limits?
- `board-guide` — How do I see every project, ticket and run?
- `device-guide` — How do agents use devices?

### Platforms and reference

- `app-mode` — How do desktop app sessions work?
- `macos` — How does paddock work on a Mac?
- `backends` — Which sandbox backends are available?
- `harness` — How do I add an agent harness?
- `contents` — Where is the full guide index?
- `support` — What is supported?
- `development` — How do I develop paddock?
- `license` — What is the license?

## Support and contribution

See `paddock help support` for the support scope and
`paddock help logs` for bug reports. Development instructions are in
`paddock help development` and [CONTRIBUTING.md](CONTRIBUTING.md).
The project is distributed under the [MIT license](LICENSE).
