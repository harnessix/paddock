## Install

### Quick start

1. On Omarchy, add the bar widget. It installs only the widget, not the `paddock` command:

   ```sh
   omarchy plugin add https://github.com/harnessix/omarchy-paddock --enable
   ```

2. Install the `paddock` command from the latest release, v0.2.1: the prebuilt binary, or `bin/install` from a clone of that tag. Both are described below.

3. Run the setup. It checks the system packages, and offers the shell commands, the launchers, the bar widget and the default backend, one step at a time. Each step says what it changes and how to undo it. A second run shows the state and changes nothing unless you ask. The last step lists where you turn off the use of your data for training (see Privacy defaults (`paddock help privacy`)).

   ```sh
   paddock setup
   ```

   Without questions, for scripts and agents, name each step: `paddock setup --yes --shims --launchers --no-plugin --backend bwrap`. `--app vscode=~/src/api` saves a workspace folder for an app. With `--yes`, the setup only prints the command that installs missing packages and never runs `sudo`.

4. Turn on the loft right after the install: answer yes to its step in `paddock setup`, or run `paddock setup --yes --loft`. Both set `[loft] auto = true` in `~/.config/paddock/config.toml`. Agents leave design notes and instruction files in repositories, and a push publishes them. The loft keeps them in a private store instead. See The loft (`paddock help private-notes`).

### From a clone or a release

paddock is one Rust command for Linux and macOS. The Omarchy bar widget lives in its own repository, [harnessix/omarchy-paddock](https://github.com/harnessix/omarchy-paddock). The commands below install the latest release, v0.2.1. The `main` branch can hold changes that are not released yet.

Clone the release tag, and run the installer from its root. It installs the system packages, builds paddock, and runs `paddock doctor`.

```sh
git clone --branch v0.2.1 https://github.com/harnessix/paddock
cd paddock
bin/install
```

Without a Rust toolchain, use the prebuilt binary of the release. It is one file. Download it with its checksum, check the checksum, copy the binary to `~/.local/bin`, and run `paddock doctor`, which prints the command that installs the system packages below:

```sh
curl -fLO https://github.com/harnessix/paddock/releases/download/v0.2.1/paddock-0.2.1-x86_64-linux.tar.gz
curl -fLO https://github.com/harnessix/paddock/releases/download/v0.2.1/paddock-0.2.1-x86_64-linux.tar.gz.sha256
sha256sum -c paddock-0.2.1-x86_64-linux.tar.gz.sha256
tar -xzf paddock-0.2.1-x86_64-linux.tar.gz
install -m 755 paddock-0.2.1-x86_64-linux/paddock ~/.local/bin/paddock
paddock doctor
```

On a 64-bit Arm machine (aarch64), use `aarch64` in place of `x86_64` in these names.

The checksum shows that the download is complete. To make sure that the file also came from the release workflow of this repository, verify its build provenance attestation (a signed statement from GitHub: which workflow run built the file, from which commit). You need the [GitHub CLI](https://cli.github.com/), signed in with `gh auth login`:

```sh
gh attestation verify paddock-0.2.1-x86_64-linux.tar.gz --repo harnessix/paddock
```

If the command prints no `Verification succeeded!`, do not install the file. Each file of a release has an attestation: the binaries, the source tarball, `SHA256SUMS`, the Homebrew formula and the VM guest files.

`paddock --version` of a release prints only the version, for example `paddock 0.2.1`. A build from a git checkout that is not a clean checkout of a release tag prints `-dev`, the commit and a dirty mark, for example `paddock 0.2.1-dev (commit 1a2b3c4, dirty)`. `paddock doctor --report` names the build kind and the full commit. Add it to a bug report.

All releases are on the [releases page](https://github.com/harnessix/paddock/releases).

On Arch Linux, the installer installs a package with makepkg. It also knows apt and dnf, and then copies the binary to `~/.local/bin`; that path is best effort and is not tested on every release (see Support scope (`paddock help support`)). Use `bin/install --minimal` to skip the desktop packages.

| Package | Needed for |
| :--- | :--- |
| git | project and worktree detection (required) |
| bubblewrap | the default backend (required) |
| passt | outbound network on the bwrap backend (required) |
| iproute2 | hiding host addresses on the bwrap backend (required) |
| weston | displays for desktop projects, and `paddock-screenshot` |
| xwayland | X11 apps on the private display |
| dbus | a private session bus for desktop apps |
| docker or podman | the container backend |
| mise | per-project toolchains from `mise.toml` |
| gnome-keyring | a private keyring for app sandboxes and for harnesses that need one, such as Copilot CLI |

`paddock doctor` prints the same list with the state of each package and the install command for your distribution.

To add the bar widget on Omarchy, point `omarchy plugin add` at the widget repository:

```sh
omarchy plugin add https://github.com/harnessix/omarchy-paddock --enable
```

`omarchy plugin add` always installs the widget from the default branch, because it takes no tag. `paddock update` updates the widget together with the command.

### macOS

paddock runs on Apple silicon Macs. Install it with Homebrew, or build it from a clone.

With Homebrew, add the paddock tap, then install paddock and tmux:

```sh
brew tap harnessix/paddock
brew install paddock tmux
```

The formula builds paddock from the source of the release, signs `paddock-vm` for the VM backend, and on Apple silicon installs the guest files of the VM. `brew upgrade paddock` installs a newer release. Then continue with step 4.

To build from a clone:

1. Install the command line tools. They give `git`, and `cc` builds the pty library of paddock:

   ```sh
   xcode-select --install
   ```

2. Install Rust with rustup. Then, in the root of a clone of this repository, build and install paddock:

   ```sh
   cargo install --path .
   ```

3. Install tmux for detached runs and `paddock view`: `brew install tmux`.

4. Run the setup and the checks, and start an agent in a project:

   ```sh
   paddock setup
   paddock doctor
   paddock codex ~/src/api
   ```

The shell step of `paddock setup` adds one marked line to `~/.zshrc`, and creates the file when there is none. Then `codex` in a new shell starts in a sandbox. `paddock shims remove` takes the line out, and deletes `~/.zshrc` when nothing else is in it.

On macOS the agent runs under Seatbelt (`sandbox-exec`). It writes the project and its own sandbox home, reads the system, and cannot read your real home. Its network goes through the proxy of paddock. Terminal runs on macOS (`paddock help macos`) has the details.

These parts are Linux only. On macOS, the command refuses with one line, or `paddock setup` skips the step:

- `paddock launchers` (XDG desktop entries).
- `paddock shims install --scope omarchy-agent` and the Omarchy bar widget.
- `paddock update` replaces no binary. It says when a release is out. Then you run `brew upgrade paddock`, or you update the clone and run `cargo install --path .` again.
- The devices are not files in `/dev/paddock` on macOS: Seatbelt runs reach them with `paddock-host device` (see On macOS (`paddock help device-guide`)).

`paddock setup` names the missing programs with the command that installs them, and installs nothing itself. Notifications use `osascript`. `paddock view` opens a Terminal window when `$TERMINAL` is not set.

The widget lists running sandboxes with their backend, network, display and login. Press Enter on a sandbox to go to the terminal that runs it (Hyprland). The row actions open the project folder, copy its path, show the display window of a sandbox in `window` mode, and stop the sandbox after a second press. The New button starts a sandbox in two steps: pick a recent project, then a harness that is installed on this machine, and paddock opens a terminal that runs it. The Checks button runs `paddock doctor`. Every action has a key, and the panel footer lists them.

### Update

```sh
paddock update --check    # is there a newer release, and what an update would do
paddock update            # asks first; --yes updates without asking
```

`paddock update` asks the GitHub API for the latest release. It downloads the tarball and its `.sha256` into a private folder, checks the checksum, and checks that the new binary prints the expected version. Then it writes the new binary next to the old one and renames it into place, so a cut download never leaves half a binary. It talks only to `api.github.com`, `github.com` and GitHub's download hosts over HTTPS, follows no redirect to another host, and uses your `HTTPS_PROXY`. If the bar widget is installed, it also runs `omarchy plugin update harnessix.paddock --yes`.

If a package owns the binary (the Arch package that `bin/install` builds), `paddock update` does not overwrite it. In your clone, check out the new release tag and run `bin/install` again, for example `git fetch --tags && git checkout vX.Y.Z && bin/install`, with the tag that `paddock update --check` names. `paddock update` runs only on the host, never in a sandbox.

`paddock doctor` says when a newer release exists. It checks at most once a day and waits at most two seconds; `PADDOCK_NO_UPDATE_CHECK=1` turns the check off. It also says when a desktop app was updated on the host while its sandbox runs, so that you restart it.

### Let your agent set it up

paddock ships two agent skills. Install them with the [skills](https://github.com/vercel-labs/skills) command, which supports Claude Code, Codex, Copilot, Gemini CLI, opencode and other agents:

```sh
npx skills add harnessix/paddock
```

- `paddock` is for an agent on the host. It covers install, `paddock doctor`, shell commands, launchers, configuration and removal. It tells the agent to leave every change that weakens the sandbox to you: approvals, `[host]` modes set to `allow`, host network, and agent approvers.
- `paddock-host` is for an agent inside a sandbox. It explains how to ask the host for a screenshot, a file or a command, and how `git push` and `gh` work there. `paddock skill install` installs this one too, with its reference files.

Then ask your agent, for example: "Install paddock and make the sandbox the default for my agents."
