# paddock

<p align="center">
  <img src="docs/assets/hero.png" alt="paddock: full auto-approval, nothing outside the fence" width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-192327" alt="License: MIT"></a>
</p>

paddock is a sandbox for coding agents such as Claude Code, Codex, Grok Build, agy (Antigravity), Gemini CLI, opencode and Copilot CLI, on Omarchy. A macOS port is in progress.

## Why

| Problem | What paddock does |
| :--- | :--- |
| An agent with full auto-approval can read your home and your other projects. | The sandbox contains only the project, the harness, a private home and package caches. See [What the sandbox contains](#what-the-sandbox-contains). |
| The agent sometimes needs the host: a push, the browser, a screenshot. | It asks with `paddock-host`, and you approve each request on the host. See [Host requests](#host-requests). |
| Agents leave design notes and instruction files in the repository, and a push publishes them. | The loft keeps these files in a private store. See [The loft](#the-loft). |
| An unattended run stops at a start screen, and nobody answers. | paddock answers known screens with a safe choice and reports stuck runs. See [Start screens and stuck runs](#start-screens-and-stuck-runs). |
| A context compaction makes the agent forget its role and its task. | After a compaction, the agent gets a short note. See [After a context compaction](#after-a-context-compaction). |
| You want several agents to work in parallel. | Agents spawn other agents, share state in pastures, and claim Spec Kit tasks with leases. See [Spawn other agents](#spawn-other-agents), [Pastures](#pastures) and [Spec Kit projects](#spec-kit-projects). |
| You do not know how much of your usage limits is left. | `paddock forage` and the Omarchy agents panel show the limits of each account. See [Forage](#forage-how-much-of-the-limits-is-left) and [Usage statistics](#usage-statistics-in-the-omarchy-agents-panel). |
| A run breaks project files. | paddock takes a snapshot before each start, and `paddock undo` restores it. See [Undo a run](#undo-a-run). |

The boundary is "which files exist", not "which commands can run". The agent has every tool and approves itself, and kernel namespaces keep the rest of the host out of the file system.

## Start in 30 seconds

Install paddock first (see [Install](#install)), for example with `bin/install` from a clone of the latest release. Then:

```sh
paddock setup              # checks the packages, offers the shell commands, launchers and bar widget
cd ~/src/my-app
paddock claude             # Claude Code with full auto-approval, in a sandbox
```

> [!WARNING]
> By default, the sandbox inherits your host environment variables, except the ones that look like secrets (names with `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY` and similar). A harness still gets its own login variables, such as `ANTHROPIC_API_KEY` for Claude Code. Cloud credentials (AWS, Azure, Google Cloud, Vault and others) do not pass unless you name them in `[env] pass`. Run `paddock env hide NAME` to drop a variable, `paddock env show` to see the list, and `paddock claude --env clean` to pass only an allowlist. See [Environment variables](#environment-variables).

## See it

Inside the paddock, the agent cannot see the SSH key on the machine, the `.env` in the project is empty for it, and it still runs commands and writes code without approval prompts.

<p align="center">
  <img src="docs/assets/demo.gif" alt="A terminal recording: paddock claude opens Claude Code's TUI with bypass permissions on. The agent runs a shell command and writes src/greet.rs without approval prompts, and reports that the SSH key does not exist and .env is empty. Then paddock explain lists the mounts" width="100%">
</p>

## Main sections

[Install](#install) · [Use](#use) · [Undo a run](#undo-a-run) · [Network](#network) · [Logins and MCP](#logins-and-mcp) · [Privacy defaults](#privacy-defaults) · [Host requests](#host-requests) · [Spawn other agents](#spawn-other-agents) · [Shared state](#shared-state-paddock-kv) · [The loft](#the-loft) · [Spec Kit projects](#spec-kit-projects) · [Forage](#forage-how-much-of-the-limits-is-left) · [App mode](#app-mode) · [Add a harness](#add-a-harness) · [Limits](#limits)

## Install

### Quick start

1. On Omarchy, add the bar widget. It installs only the widget, not the `paddock` command:

   ```sh
   omarchy plugin add https://github.com/harnessix/omarchy-paddock --enable
   ```

2. Install the `paddock` command from the latest release, v0.2.1: the prebuilt binary, or `bin/install` from a clone of that tag. Both are described below.

3. Run the setup. It checks the system packages, and offers the shell commands, the launchers, the bar widget and the default backend, one step at a time. Each step says what it changes and how to undo it. A second run shows the state and changes nothing unless you ask. The last step lists where you turn off the use of your data for training (see [Privacy defaults](#privacy-defaults)).

   ```sh
   paddock setup
   ```

   Without questions, for scripts and agents, name each step: `paddock setup --yes --shims --launchers --no-plugin --backend bwrap`. `--app vscode=~/src/api` saves a workspace folder for an app. With `--yes`, the setup only prints the command that installs missing packages and never runs `sudo`.

4. Turn on the loft right after the install: answer yes to its step in `paddock setup`, or run `paddock setup --yes --loft`. Both set `[loft] auto = true` in `~/.config/paddock/config.toml`. Agents leave design notes and instruction files in repositories, and a push publishes them. The loft keeps them in a private store instead. See [The loft](#the-loft).

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

All releases are on the [releases page](https://github.com/harnessix/paddock/releases).

On Arch Linux, the installer installs a package with makepkg. It also knows apt and dnf, and then copies the binary to `~/.local/bin`; that path is best effort and is not tested on every release (see [Support scope](#support-scope)). Use `bin/install --minimal` to skip the desktop packages.

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

On macOS the agent runs under Seatbelt (`sandbox-exec`). It writes the project and its own sandbox home, reads the system, and cannot read your real home. Its network goes through the proxy of paddock. [Terminal runs on macOS](#terminal-runs-on-macos) has the details.

These parts are Linux only. On macOS, the command refuses with one line, or `paddock setup` skips the step:

- `paddock launchers` (XDG desktop entries).
- `paddock shims install --scope omarchy-agent` and the Omarchy bar widget.
- `paddock update` replaces no binary. It says when a release is out. Then you run `brew upgrade paddock`, or you update the clone and run `cargo install --path .` again.
- The devices are not files in `/dev/paddock` on macOS: Seatbelt runs reach them with `paddock-host device` (see [On macOS](#devices-on-macos)).

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

## Remove

1. Remove the bar widget:

   ```sh
   omarchy plugin remove harnessix.paddock
   ```

2. If you turned them on, remove the shell commands and the launchers. Do this before you remove the command:

   ```sh
   paddock shims remove
   paddock launchers remove
   ```

   `paddock setup --yes --no-shims --no-launchers --no-plugin` does steps 1 and 2 in one command.

   If you forget this step, nothing breaks. The line in `~/.bashrc` runs only when its script exists, and each command falls back to the real program when paddock is gone. The launcher entries stay in the menu until you delete them.

3. Remove the command. If you installed the Arch package (`bin/install` does this on Arch Linux), run `sudo pacman -R omarchy-paddock`. Otherwise, delete `~/.local/bin/paddock`.

4. Optional: to free space and keep paddock, run `paddock gc` instead. See [Disk space](#disk-space).

5. Optional: delete the data that paddock keeps. These directories hold the sandbox homes, isolated logins, shared caches, worktrees and settings. Your own logins and files are not in them.

   ```sh
   rm -rf ~/.local/share/paddock ~/.cache/paddock ~/.local/state/paddock ~/.config/paddock
   ```

   If you set `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` or `XDG_CONFIG_HOME`, the directories are under those paths instead.

6. Optional: if you used the docker or podman backend, run `paddock gc --yes` before you remove the command. It removes the old paddock images and paddock's build cache. To remove the current images too, run `docker image rm $(docker image ls -q paddock-base)`, and the same command with `podman`.

paddock does not change any file outside these directories, except the files that you name in `paddock env hide` (in `~/.config/paddock/config.toml`), the git worktrees that `--worktree` creates, the files that `paddock shims install` and `paddock launchers install` change (also through `paddock setup`), the `.paddock` link, the empty mount points and the `.git/info/exclude` lines of [the loft](#the-loft), and the paddock binary that `paddock update` replaces. Step 2 undoes the shell commands and launchers. To remove a worktree, use `paddock worktree remove`.

## Use

```sh
paddock claude                 # Claude Code in the current project
paddock codex ~/src/api        # Codex in another project
paddock claude -- -p "fix the failing test"   # arguments after -- go to the harness
paddock claude --worktree      # work on a new branch in a new worktree, review the diff at exit
paddock worktree list          # the worktrees of --worktree runs and apps; merge or remove them
paddock shell claude           # a shell in the sandbox that claude gets
paddock explain claude         # every mount, variable and rule, and why it is there
paddock ps                     # running sandboxes
paddock files                  # files a sandbox saved outside the project
paddock doctor                 # dependencies, backends, located harnesses, training opt-outs
paddock doctor --escape        # try to escape, and report each attempt
paddock setup                  # first-run steps; a second run changes nothing
paddock update --check         # is there a newer release
paddock mcp claude             # MCP servers, and whether each runs in the sandbox
paddock env show               # which host variables pass, by name
```

A sandbox id looks like `claude-app-1a2b3c`. A command that takes a sandbox id also takes its last part, `1a2b3c`, or the start of that part with at least 4 characters, such as `1a2b`. paddock looks for the id among the running sandboxes, the recent ended runs, the sandbox homes of `paddock files` and the snapshots of `paddock undo`. When the start fits more than one sandbox, paddock lists their full ids. The hints that paddock prints show the short id. `--json` output keeps the full id in `id` and adds the short id in `short`.

A sandbox uses your existing login of the harness. See [Logins and MCP](#logins-and-mcp) for a separate login.

## What the sandbox contains

| Inside the sandbox | Source on the host |
| :--- | :--- |
| The project (git root), read-write | the project directory |
| `.git`, read-write, its own mount point | the same directory. A mount point cannot be renamed, so the read-only files below it stay in place. |
| `.git/hooks`, `.git/config`, `.git/config.worktree`, `.git/commondir`, `.git/objects/info/alternates`, `.git/modules`, read-only | the same files. Git on the host reads them, and they can name a program or another directory. The git directories of submodules that exist at the start stay writable below `.git/modules`, with the same read-only files. |
| `$HOME`, read-write | a private directory per harness, login and project, under `$XDG_DATA_HOME/paddock/homes` |
| Credential files of the harness | copied from your home directory, or from an isolated store |
| Harness settings and skills, read-only | the files that the harness profile lists in `import_ro` |
| The harness program, read-only | its install directory, found through PATH or mise |
| Package caches (cargo, npm, uv, go) | cache layers: a shared layer under `$XDG_CACHE_HOME/paddock/layers` and one layer per project under `$XDG_CACHE_HOME/paddock/projects` |
| mise tools from the registry, read-only | `$XDG_CACHE_HOME/paddock/shared/mise-tools` |
| `/usr`, `/etc`, read-only (bwrap backend) | the host system |

The sandbox never contains your real home directory, `~/.ssh`, `~/.gnupg`, cloud credentials, browser profiles, the container runtime socket, or the session bus. Files with credential-shaped names in the project (`.env`, `*.pem`, `*.key`) are hidden. The policy engine refuses any configuration that breaks these rules, and names the line that asked for it.

### Git on the host

Git takes commands from the repository it reads. `core.fsmonitor`, `core.hooksPath`, a hook and a `filter` driver all start a program. The agent writes the project, so paddock treats every file in `.git` as hostile.

- paddock finds the repository with the file system: it walks up and looks for a real `.git`. It never believes `core.worktree`, `commondir` or a `.git` file that names another directory. A layout that does not match is refused with a message that says what is wrong.
- Every git call that paddock makes on a project names the git directory directly, drops your global and the system configuration, and turns off the settings that can start a program.
- Work-tree state, such as the list of uncommitted changes, comes from a `git status` that runs in a sandbox of its own, with no network. A `filter` driver cannot be turned off with a flag, so it runs in there, where it can do no more than the agent already can. On a host without bubblewrap, paddock says the state is unknown instead of guessing, and `paddock gc` then asks for `--force` before it removes such a worktree.
- `git worktree add` and `git worktree remove` write a work tree. Before one of them runs, paddock reads the repository configuration as text. If it holds a key that starts a program, paddock stops and prints the file and the key.
- Your own `git` commands in the project are protected by the read-only binds in the table above, as long as paddock started the sandbox. A repository that the agent creates inside the project is new, so it has no such binds: paddock's own calls stay safe, but take care with your own.
- `.git/modules` is read-only, so the agent cannot plant the git directory of a new submodule there, whose config your `git status` would read. `git submodule add` and the first `git submodule update --init` of a submodule therefore fail in the sandbox; run them on the host.
- The first start in a repository creates `.git/commondir` with `.`, and empty `.git/config.worktree` and `.git/objects/info/alternates` files, if they are missing. Git treats all three as no-ops. They exist so that paddock can bind them read-only; otherwise the agent could create them and point git elsewhere. While a repository is bound this way, `git config --worktree` cannot write.
- `paddock doctor --escape` checks all of this on every backend: that `.git` cannot be renamed in the sandbox, that the pointer files are read-only, and that a planted setting or hook does not run during paddock's host git calls.

### mise toolchains

If the project has a `mise.toml` or `.tool-versions` file, paddock installs its tools before the harness starts. This happens in two steps.

1. paddock reads the tool list on the host. Tools from mise's registry that use the `core` or `aqua` backends, with a plain version, go to the shared installs. Examples are `node = "22"`, `python = "3.12"` and `jq = "1.7.1"`. A clean sandbox installs them. That sandbox has no project, no project config and no sandbox home, so hooks, `[env]` scripts and tasks cannot run while the shared installs are writable.
2. `mise install` runs in the project sandbox with the project config. It installs every other tool into the private sandbox home, for example `cargo:`, `npm:`, `ubi:` or plugin tools, and tools with options. The shared installs are read-only in this step, and also while the harness runs.

If the shared installs already hold every version, paddock skips the first step. This does not apply to `latest`. Every project uses the shared tools. A tool in the sandbox home belongs to one project and its harness. If you set `mise_install = false` in `[setup]` of `paddock.toml`, paddock skips both steps. The srt backend cannot mount the shared installs, so it installs every tool in the sandbox home.

## Undo a run

The project is writable in the sandbox, and host tools run some project files later, for example `.envrc`, `mise.toml` hooks and editor tasks. Before each start, paddock takes a snapshot of the project files, so one command undoes a bad run.

```sh
paddock undo                  # the last session of this project
paddock undo --list           # the snapshots: time, harness, size
paddock undo 1a2b3c --yes
```

`paddock undo` lists the files that the run added, changed and deleted, and asks before it restores. Changed and deleted files come back from the snapshot. Added files go to `~/.local/share/paddock/undo-trash/TIME/`, so you can undo an undo by hand. The command refuses while a sandbox of the project runs. A file that changed after the session ended (you or another agent edited it) gets a warning and stays as it is. When paddock did not record the end of the session (it was stopped, or the end walk took longer than 20 s), every changed file counts as such. `--force` restores it too. At exit, paddock names `paddock undo` when the run changed files.

The snapshot:

- lives in `~/.local/share/paddock/snapshots/PROJECT/SESSION/`. paddock keeps the last 10 for each project.
- leaves out `.git/`, which has its own history, and the folders that `.gitignore` ignores, such as `target` and `node_modules`. paddock asks its hardened host git for that list. Outside git, it leaves out `target`, `node_modules`, `.venv`, `dist` and `build`. `paddock undo` never touches these paths, also when a folder became ignored only during the session.
- shares the file data with the project on btrfs, xfs and bcachefs (`FICLONE`), so it takes almost no space. On other file systems, paddock copies the files only while the copy stays under `[snapshot] max`.
- has a time limit of 20 seconds. A slower snapshot is dropped with a notice, and the sandbox starts anyway.

A `--worktree` run has its own branch and gets no snapshot. Set the options in `~/.config/paddock/config.toml`:

```toml
[snapshot]
enabled = true     # the default
max = "500M"       # the largest full copy, without shared file data
```

## Files outside the project

The project is shared with the host, so the agent's changes there are already yours. A file that the agent saves anywhere else, for example to `~/Desktop` or `~/Downloads`, stays in the private sandbox home. You do not see it until you recover it.

`paddock files` lists these files for each sandbox, also after the sandbox exits. It also shows the commits and the changed paths in the project since the sandbox started.

```sh
paddock files                                 # every sandbox that has files
paddock files 1a2b3c                          # one sandbox
paddock recover 1a2b3c Desktop/report.pdf     # to ~/Desktop/report.pdf
paddock recover 1a2b3c --all --to ~/Inbox
paddock discard 1a2b3c Downloads/setup.zip
```

`paddock recover` copies the file to the host, then deletes the sandbox copy. Add `--keep` to keep it. The rules:

- A recovered file never replaces a file. If the name is taken, the file gets a new name, such as `report (1).pdf`.
- paddock never writes into a hidden path of your home, such as `~/.ssh` or `~/.config`, into its own directories, or into system directories. It refuses, even when you ask for it.
- paddock copies regular files only. It skips symlinks, devices and pipes, and it never follows a symlink in the sandbox home. A recovered file is never executable: paddock drops the execute bits, so one click on the host cannot run it.

The Omarchy panel lists the same files. Press Enter on a file to recover it. When a sandbox saves a new file, a desktop notification tells you. The notification does not take the keyboard.

paddock looks in the recovery folders only. The defaults are `Desktop`, `Documents`, `Downloads`, `Pictures`, `Videos` and `Music`, with all their subfolders. A folder of your host that has another name, set in `~/.config/user-dirs.dirs`, is the target. Change the folders in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change them, because the agent can edit it.

```toml
[recovery]
folders = ["Desktop", "Downloads", "out"]
ignore = ["*.desktop", "*.part", "*.tmp"]   # never listed
```

By default, paddock ignores `*.desktop` launchers, because they can run commands when you open them, and partial or lock files.

## Continue your host history in a sandbox (`--profile default`)

A normal sandbox has its own home, so the harness starts there without your conversations. With `--profile default`, the harness starts with its host data in the sandbox: conversations, sessions, settings, plugins and caches. You continue your real history.

```sh
paddock run claude --profile default
paddock run codex ~/src/app --profile default
```

The run works on a private snapshot. paddock copies the data roots of the harness into `~/.local/share/paddock/shadow/runs/ID/` and makes the sandbox home an overlay on that copy. Every change of the run goes into the overlay. When the sandbox ends, paddock prints how many files the run added, changed and deleted, with the first 20 paths, and removes the run folder. The host is never written.

The rules:

- The real home is never visible. Only the data roots of the harness come in. `~/.ssh`, your documents, other projects, the data of other harnesses and paddock's own folders are absent.
- paddock knows no vendor format. Whole files and folders come in as bytes.
- Logins belong to the credential sandbox. The credential files (`[state] auth`) are not part of the snapshot. They come in and sync back exactly as in every sandbox (`--auth`).
- `--profile default` is never the default mode. Only the command line turns it on. A project `paddock.toml` cannot turn it on or add data roots.
- With `--worktree`, the project of the run is the worktree. paddock does not tell the harness about the main project.
- bwrap runs it on Linux, Seatbelt on macOS. docker, podman, srt and the VM backend refuse.

The data roots are `[state] data` of the profile: `.claude` and `.claude.json` for claude, `.codex` for codex, `.grok` for grok. A harness without data roots refuses `--profile default`. Add roots in `~/.config/paddock/config.toml`:

```toml
[profiles.claude]
data = [".config/claude-extra"]

[storage]
shadow_copy_limit_mib = 2048   # the largest snapshot without reflinks
```

A data root is a path relative to the home. paddock refuses the home itself, a folder that many programs share (`.config`, `.local`, `.local/share`, `.local/state`, `.cache`, `.agents`), the key folders (`.ssh`, `.gnupg`, `.pki`, `.password-store`), its own folders, and paths with `..`.

The snapshot copies folders, regular files and the symlinks that stay inside their root. It skips symlinks that leave the root, sockets, FIFOs and devices, and names how many. On btrfs, xfs and bcachefs each file is a reflink (`FICLONE`), so the snapshot takes almost no space. On other file systems paddock refuses a snapshot above the size cap and prints its size. The run folder must be on the file system of the home.

paddock still binds the project, keeps the network mode, the secret masking, the host passthrough and the shared caches. Inside the data roots, the selective copies are off: no `seed` copies, no `json_import`, no history binds and no copy back. `import_ro` paths outside the roots, such as `~/.agents/skills`, stay. When the harness also runs on the host, paddock prints a notice: the snapshot is private, and nothing goes back. `paddock ps --json` marks such a sandbox with `"profile": "default"`.

On macOS, Seatbelt cannot mount an overlay. So the sandbox home is a writable copy of the data roots in `~/.local/share/paddock/shadow/runs/ID/home`, and the harness writes that copy. On APFS each file is a clone (`clonefileat`), so the copy takes almost no space and no time, and the start line says `(clones)`. On another file system, such as an external HFS+ disk, the start line says `plain copies` and the size cap applies. After its own writes, paddock records the size and the change time of each file. At the end it compares the copy with that record, prints the same summary and removes the run folder. A file that the run rewrote with the same size and the same change time is not in the summary. The other rules are the same as on Linux: nothing goes back to the host. The run uses the keychain of its sandbox login, not your login keychain. Claude Code keeps its login in the Keychain on macOS, so you log in once in the sandbox, and later runs with that login keep it.

## Disk space

Each harness gets its own home in each project, so the homes grow with the number of projects. The docker and podman backends also keep one image for each configuration.

`paddock du` lists what paddock stores and how much space it uses:

- Each sandbox home: harness, login, project, size, last use, and what you lose when you delete it.
- The paddock images of docker and podman, and whether each is current or in use.
- The worktrees that `--worktree` created, with their total size, last use, unmerged commits and merged state.
- The build caches of worktree runs.
- The session records of `paddock files`, and the keyring passwords of app homes.
- The project snapshots and the undo trash of `paddock undo`.
- The caches in the homes, by class (see [Caches](#caches)).

On btrfs, a size counts only the data that no other file shares. On other file systems, it counts the allocated blocks, as `du` does. The first line names the kind. `paddock du --json` prints the same data as JSON.

`paddock gc` removes what is no longer needed. Without `--yes`, it removes nothing and prints the list.

```sh
paddock gc                                  # what --yes would remove
paddock gc --yes                            # remove the automatic items
paddock gc --homes --older-than 30          # also list homes unused for 30 days
paddock gc --homes --yes                    # remove them
```

`--yes` alone removes these items:

- Paddock images that are not current and that no container uses. Images share layers, so `gc` counts only the part that belongs to one image alone.
- paddock's own records in the docker build cache. paddock marks its build steps, so the build cache of your other projects stays. A current image keeps its own layers, so this only makes the next image build slower.
- Keyring passwords whose home is gone.
- Session records that ended more than 30 days ago.
- Homes of harnesses whose profile no longer exists.
- Old shared caches that paddock no longer uses, such as `~/.cache/paddock/shared/mise-installs` and the old writable package caches in `~/.cache/paddock/shared`. paddock keeps them while a sandbox runs.
- The cache layers of projects that no longer exist, in `~/.cache/paddock/projects`.
- Project snapshots whose project is gone.
- Pure caches in the homes of sandboxes that do not run (see [Caches](#caches)).

paddock also does this after it builds a new image. To turn that off, set `PADDOCK_GC_AFTER_BUILD=0`.

The other items need a flag that names them:

- `--homes` adds homes whose project is gone, or that were not used for `--older-than` days (default 90). A home holds the transcripts and settings of a harness in one project. A terminal harness keeps its login in your home or in `~/.local/share/paddock/auth`, so the login stays.
- A desktop app keeps its login in its home. If you delete the home, you must log in again. These homes also need `--include-app-logins`.
- `--worktrees` adds paddock worktrees. A worktree with unmerged commits or uncommitted changes also needs `--force`. paddock then keeps its branch, so the commits stay in the repository.
- `--snapshots` adds project snapshots and undo trash folders older than `--older-than` days. The files that an undo moved to the trash are lost.
- `--scratch` adds scratch projects that were not used for `--older-than` days. An agent from `omarchy-agent` works in one when it starts in the home folder (see [Only the agents that Omarchy starts](#only-the-agents-that-omarchy-starts)). Its files are lost.

paddock never removes the home or worktree of a running sandbox. It deletes only inside its own directories. It never follows a symlink, so a link in a sandbox home cannot point it at your files. A sandbox can edit the `.paddock-origin` file in its home, which records the project. paddock uses that file only when it matches the folder name of the home, which contains a hash of the project path.

### Automatic cleanup

paddock cleans up by itself, so its folders do not fill the disk:

- When a `--worktree` run or a spawned worker ends without a commit and without uncommitted changes, paddock removes its worktree and its branch at once. There is nothing to review.
- Each `paddock run` starts a cleanup in the background, at most once an hour. The start does not wait for it. It removes the worktrees whose work is merged and that have no uncommitted changes, and their `paddock/` branches. If a local branch outside `paddock/` has a commit, the commit counts as merged. A commit with the same patch counts too, for example after a cherry-pick or a rebase.
- A worktree with unmerged work stays. When it is not used for `worktree_cache_days`, paddock deletes its build output: folders with a `CACHEDIR.TAG` file, such as cargo's `target/`, that git ignores and that hold no tracked file. paddock never deletes a tracked file, or an untracked file that git does not ignore.
- The same cleanup removes snapshots and undo trash folders older than `snapshot_days`.
- It never removes a sandbox home. A home holds your conversations with the harness (transcripts and history) and an app's login. Only `paddock gc --homes` removes homes, when you ask, and it names what is lost.
- It also does what `paddock gc --yes` does, except for images.

The cleanup never touches the home, worktree or snapshot of a running sandbox. `paddock doctor` shows when it last ran, what it removed, and its errors. `paddock gc-auto --now` runs it at once.

### Free space

Before a sandbox starts, paddock checks the free space on the disks of the project, the worktrees, the homes and the build cache:

- Below the hard floor, paddock first runs the cleanup, for at most two minutes. If the space is still below the floor, the sandbox does not start. The message gives the numbers and the commands that free more space.
- Below the soft floor, the sandbox starts with one warning line, and a desktop notification at most once an hour.
- While sandboxes run, paddock checks every five minutes. At the hard floor it sends a notification. It does not stop a running sandbox.

Each floor is the smaller of its two numbers, so it fits both small and large disks. The defaults are 15 GB or 5% for the hard floor, and 50 GB or 10% for the soft floor. `paddock doctor` shows the free space and the floors.

### Build cache of worktree runs

A `--worktree` run or a worker of a Rust project does not build into `target/` of its worktree. Its cargo target folder (`CARGO_TARGET_DIR`) is in `~/.cache/paddock/build/<repository>/`. At the start, paddock copies the newest target folder of the same repository with copy-on-write, so the worker does not compile the project from nothing. When the worker ends, its folder becomes the newest one. Each worker has its own copy. Two workers that build different commits do not wait for each other, and they do not see each other's output. A project never sees the build cache of another project.

On btrfs and xfs the copy takes almost no space. On ext4 and other file systems without copy-on-write, each worker starts with an empty folder, and paddock deletes it when the worker ends. On macOS the build cache is off. The cleanup keeps the build caches of all repositories under `build_cache_gib` and removes the least recently used first.

A script that reads `target/` directly does not find the build output. For such a project, set this in its `paddock.toml`:

```toml
[setup]
shared_build_cache = false
```

### Cleanup settings

Set these in `~/.config/paddock/config.toml`. The values here are the defaults. 0 turns a rule or a floor off.

```toml
[storage]
auto_gc = true               # the automatic cleanup; PADDOCK_AUTO_GC=0 also turns it off
worktree_cache_days = 2      # delete the build output of a kept worktree after this many days without use
snapshot_days = 30           # delete snapshots and undo trash folders older than this
hard_floor_gib = 15          # do not start a sandbox below the smaller of these two
hard_floor_percent = 5
soft_floor_gib = 50          # warn below the smaller of these two
soft_floor_percent = 10
build_cache = true           # the build cache of worktree runs
build_cache_gib = 40         # the size of all build caches together
```

If a value is wrong, paddock uses the default, and the run and `paddock doctor` print a warning.

### Caches

paddock sorts the caches in a sandbox home into three classes. `paddock du` shows the size of each class.

- Pure caches rebuild themselves: the mesa shader cache, the fontconfig cache, the `Cache`, `Code Cache` and `GPUCache` folders of Electron and Chromium apps under `~/.config/APP`, and `~/.codex/cache`. In the sandbox, each of them with a known path is an empty tmpfs, so it is gone at exit. The tmpfs uses memory while the sandbox runs. The Electron folders get their names from the app, so `paddock gc --yes` clears them instead. paddock never clears a login, settings or history folder.
- Package caches check each entry themselves: `~/.npm`, `~/.cache/pip` and `~/.cache/uv`. A project with a `package.json`, a `pyproject.toml` or a similar file gets one shared copy for all projects, in `~/.cache/paddock/shared`. A harness that installs through npm (Claude Code, codex, gemini, copilot) gets the shared `~/.npm` in every project.
- Runtimes, such as `.codex/packages` and `.copilot/pkg`, stay in the home. Deduplication shares their data.

The list of pure caches is in `src/storage/caches.rs`. A harness profile adds its own with `[state] scratch = ["PATH"]`, relative to the home. A path with `*` matches one folder name, and only `paddock gc` clears it. To turn off the tmpfs mounts and the gc step, set this in `~/.config/paddock/config.toml`:

```toml
[storage]
tmpfs_caches = false
```

### Deduplication

Harnesses download the same runtimes into the home of each project. For example, `.codex/packages` holds about 400 MB, and it is the same in each project. On btrfs, xfs and bcachefs, the kernel can let identical files share their data (`FIDEDUPERANGE`). The kernel compares the bytes first and refuses data that differs. A later write to one copy gets its own blocks, so the other copy does not change.

```sh
paddock gc --dedupe                         # count the files with the same data and their size
paddock gc --dedupe --yes                   # share the data and print the space that came back
```

After each sandbox ends, paddock also shares the data of the files in its home with the other homes. This runs in the background at the lowest priority, for at most two minutes, and the exit does not wait for it. To turn it off, set this in `~/.config/paddock/config.toml`:

```toml
[storage]
dedupe = false
```

paddock reads only files of at least 4 KiB in the sandbox homes. It groups them by size, then by SHA-256. It keeps the hashes in `~/.cache/paddock/dedupe/index`, so it reads each file once. It never follows a symlink, never leaves the file system of the homes, and skips a file that changes while it reads it. On ext4, tmpfs and other file systems that cannot share data, paddock skips this step, and `paddock du` says so.

## Network

| Mode | Behavior |
| :--- | :--- |
| `egress` (default) | Internet access that leaves the same way as host traffic. Host services and the LAN are not reachable. |
| `none` | No network. |
| `api` | Only HTTPS to the model API hosts of the harness profile and the domains of `--allow`. Meant for reviewers and agents that need only their model. |
| `allowlist` | Only the hosts of the list, the same list as `api`. On Linux it runs on the srt backend. On macOS it runs on Seatbelt with paddock's proxy (see [Network mode `allowlist` on macOS](#network-mode-allowlist-on-macos)). |
| `host` | The host network namespace. Host-local services are reachable. Needs approval in `paddock.toml`. |

Many users depend on a system proxy or a TUN device for a stable exit address, and an account can be flagged when that address changes. So paddock never gives a sandbox a bridged network.

On the bwrap backend, `egress` uses pasta. pasta makes each outbound connection as an ordinary socket of a host process, so the connection follows the host routes, the TUN device, and the host DNS resolver. If the host sets `HTTPS_PROXY` or a similar variable, the sandbox gets the same value, and a proxy on the host loopback is the one loopback port that the sandbox can reach. paddock turns off the other port forwarding of pasta, and makes every address of the host unreachable inside the sandbox, so services on the host stay hidden.

The LAN is blocked too. A sandbox cannot reach private and link-local networks: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, 100.64.0.0/10 (VPN peers such as Tailscale), and the IPv6 ranges fc00::/7 and fe80::/10. So your router, NAS, printers and other machines stay out of reach. A host proxy on the LAN stays reachable, because all traffic goes through it. To let a project reach the LAN, set this in `paddock.toml` (it needs your approval) or in `~/.config/paddock/config.toml`:

```toml
[network]
lan = true
```

On the docker and podman backends, the container has no network interface. Its only way out is a Unix socket to a proxy in the paddock process on the host. That proxy connects like a host program, through the host proxy variables or directly. It refuses connections to the host itself. Programs that ignore `HTTPS_PROXY` get no network, so no traffic leaves by another path.

A host proxy URL that starts with `https://` means TLS to the proxy itself. The paddock proxy then opens a TLS connection to that proxy and checks its certificate against the system trust store. A private CA that you installed system-wide works, and so do `SSL_CERT_FILE` and `SSL_CERT_DIR`. The request and the user name and password from the URL go only inside TLS. If the TLS connection fails, for example because of a certificate error, the request fails. paddock never falls back to plain TCP. A URL that starts with `http://` or `socks5://` sends the user name and password in cleartext, because those protocols do that, for host programs too. paddock allows this, but it logs a warning when the proxy is not on the host loopback. Use an `https://` proxy URL to protect them.

Who talks to the host proxy depends on the backend. On docker and podman, the paddock proxy described above does. On bwrap, the sandbox gets the host proxy variables, and the harness in the sandbox connects to the proxy itself, through pasta. The harness then handles `https://` proxy URLs and certificates the way it does on the host. On srt, the srt proxy on the host handles the traffic, and the paddock proxy is not used.

### Network mode `api`

In mode `api`, the sandbox has no network at all. Its only way out is the paddock proxy on the host, which a relay in the sandbox offers on `127.0.0.1:3131`. paddock sets `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and their lowercase forms to it, and `NO_PROXY=127.0.0.1,localhost`. A program must honor these variables to reach anything. A program that ignores them gets no network, so the rule holds either way. The credential gateway on `127.0.0.1:3130` works as before.

The proxy lets a connection through only when all of these hold:

- It is a `CONNECT` to a host on the list, on port 443. The list holds the `[network] endpoints` of the harness profile, the `[network] allow` entries, and each `--allow`. An entry `HOST:PORT` allows that port instead. An entry `*.example.com` allows the subdomains of `example.com`, not `example.com` itself.
- The host is a name. IP addresses are refused.
- The TLS ClientHello names the same host (SNI). The proxy checks this before any byte goes to the host.

Plain HTTP requests are refused. The proxy connects like a host program, through the host proxy variables or directly, and it still refuses the host itself and the LAN. The audit log (`paddock log`, capability `network`) records each refused host and the reason, at most once per host and minute, never content. `paddock explain HARNESS --network api` lists the allowed hosts and where each one comes from, and `paddock ps --json` shows the mode.

On docker and podman, the container gets the same proxy and the same rules. On srt, mode `api` uses the srt proxy with the same list (`allow_domains`). srt does not check the TLS server name.

### Network mode `allowlist` on macOS

On macOS, `--network allowlist` runs on the Seatbelt backend, and `--backend auto` picks it. The sandbox connects only to localhost, as in mode `api`. paddock's proxy lets through only the hosts of the list. The list is the same one that srt gets on Linux: the `[network] endpoints` of the harness profile, the `[network] allow` entries, and each `--allow`. The rules of mode `api` apply: `CONNECT` only, host names and not IP addresses, a matching TLS server name, and no plain HTTP. paddock ignores IP entries and names them in a warning at the start. Unlike mode `api`, the sandbox also reaches the host ports that `[network] host_ports` allows, as on srt.

As in mode `api`, a refused host goes into the audit log and the boundary log (`paddock boundary`), and the egress monitoring records the allowed hosts. The reason names the mode, for example `network mode allowlist: not on the list`. Terminal runs and app mode both work this way. The VM backend does not have mode `allowlist` yet.

`paddock doctor --escape --network egress --online` checks all of this: DNS and HTTPS work, and the host loopback, the other host addresses, and direct connections that bypass the proxy do not.

### Monitoring

paddock watches each bwrap or srt sandbox that runs in its scope (a systemd user instance), and the connections of the host proxy. It counts on this computer, keeps the numbers here and contacts no vendor service. It never reads content. You get a desktop notification and a line in the audit log (`paddock log`, capability `monitor`) once per sandbox and finding, so you never need to read logs.

#### Tunnel watch

A tunnel lets a machine outside reach into the sandbox, or lets the sandbox reach past its network rules. While a bwrap or srt sandbox runs in its scope (a systemd user instance), paddock looks at the command lines of its processes every 10 seconds. It reports `ssh` with `-R`, `-L` or `-D`, and `ngrok`, `cloudflared`, `frpc`, `frps`, `bore`, `chisel`, `lt` (localtunnel) and `tailscale funnel`.

- paddock tells you once per sandbox and tool: one desktop notification and one line in the audit log (`paddock log`, capability `monitor`), with the tool and its tunnel flags. It never records hosts or other arguments.
- `paddock ps --json` lists the reported tools of each sandbox in `tunnels`.
- paddock reads command lines only. It never reads what goes through a tunnel and contacts no vendor service.
- It does not stop the tunnel. `paddock stop <id>` or `paddock freeze <id>` does.
- It is a notice, not a barrier: a process can rename itself or rewrite its command line. The network rules of the sandbox still hold either way.
- docker and podman sandboxes, and sandboxes without a scope, are not watched.
- In `~/.config/paddock/config.toml`, `[monitor] tunnels = false` turns the watch off, and `[monitor] notify = false` keeps the audit line but shows no notification. A project `paddock.toml` cannot change either.

#### Traffic

Every 10 seconds paddock reads the byte counters of the sandbox's own network namespace (`/proc/PID/net/dev`, all interfaces but `lo`). It never sees what goes through them.

- `paddock ps --json` shows `traffic: {"sent": N, "received": N}` for each sandbox, in bytes since its start.
- paddock keeps the sent bytes per hour of each harness and project in `~/.local/state/paddock/monitor/`: numbers only, the last 30 days. The baseline is the median of the hours with traffic.
- paddock tells you once per sandbox and reason when the sent bytes of the current hour pass `upload_per_hour` (default 5G), or pass `factor` (default 10) times the baseline, after 5 hours with traffic. The audit line holds numbers only.
- The settings go in `[monitor]` of `~/.config/paddock/config.toml`, and a project `paddock.toml` cannot set them:

```toml
[monitor]
traffic = true            # false turns the count off
upload_per_hour = "5G"    # K, M or G (1024 based), or plain bytes
factor = 10
notify = true             # false keeps the audit line, without a notification
```

- docker and podman sandboxes, sandboxes with `--network host` and sandboxes without a scope are not counted.
- srt sends through a local proxy, so most of its traffic goes over `lo` and does not count.

#### Credential files

A key or a token that a sandbox plants in its home can keep access later. paddock takes a list of the credential files in the sandbox home when the sandbox starts, looks again every 3 minutes and once when the sandbox ends. It reads names, sizes and change times, never content.

- Credential files: everything in `~/.ssh/`, `~/.netrc`, `~/.git-credentials`, `~/.config/gh/hosts.yml`, `~/.docker/config.json`, `~/.aws/credentials`, `~/.kube/config`, keys such as `*.pem` and `id_ed25519`, and files whose name contains `token`, `credential`, `secret` or `apikey`.
- paddock skips caches and build folders (`.cache`, `.npm`, `.cargo/registry`, `node_modules`, `target` and similar), never follows a symlink, and stops after 50,000 entries.
- A new credential file, or a changed one, gets one line in the audit log with its path and size, and the check gets one desktop notification for all its files. Each path counts once per sandbox. `paddock ps --json` lists them in `credential_files`.
- The login files of the harness are not news: when the harness refreshes its token, paddock only counts it in the run folder.
- In `~/.config/paddock/config.toml`, `[monitor] credentials = false` turns the check off, and `[monitor] notify = false` keeps the audit lines without a notification.
- It works with every backend, as long as paddock kv runs.

#### Egress targets

The host proxy sees the target of each connection of docker and podman sandboxes, and of network mode `api` on every backend. paddock records the target host of each allowed connection per harness in `~/.local/state/paddock/monitor/egress-<harness>.json`: the host name, first seen, last seen and a count. It never records a path or content, and keeps the last 90 days. The boundary log has the refused connections.

- paddock tells you once per sandbox and reason when a harness connects to a host it never reached before (`new_host`), or to more than 10 new hosts within 10 minutes (`new_hosts`). The alert is an audit line (capability `monitor`), a notification and a panel mark.
- A new install does not alert on everything: the rules start after 20 sandbox runs of the harness, or 7 days after its first recorded run.
- In `~/.config/paddock/config.toml`, `[monitor] egress = false` turns the record off, and `[monitor] notify = false` keeps the audit line without a notification. A project `paddock.toml` cannot change either.
- bwrap and srt sandboxes in network mode `egress` do not use the host proxy, so paddock does not see their targets.

#### Findings in the panel, OK and mute

The bar panel marks a sandbox with findings: its row gets a line in the urgent color, such as `tunnel: ngrok`, `upload: 6.2 GB this hour`, `new credential file: ~/.ssh/id_ed25519` or `new host: example.com`, and `+N` for more. A frozen sandbox and warnings come first, timers after.

- The OK action on the row (or `paddock monitor ack ID`) clears the mark of that sandbox until a new finding. paddock writes the acknowledgement into the run folder. The audit log keeps the findings.
- `paddock monitor list` prints the findings of the running sandboxes, and `--json` prints them as JSON. `paddock ps --json` shows the marked ones in `monitor_marks`.
- `monitor.json` in the run folder flags upload alerts in `alerts` (`upload_per_hour`, `factor`) and keeps the bytes of the alert's hour in `alert_sent`.
- `monitor-egress.json` in the run folder keeps the egress findings: the new host, or the count of new hosts.
- A project that is noisy on purpose gets no notification and no mark when you mute it. The audit lines stay. A muted path covers its folders too.

```sh
paddock monitor mute ~/src/tunnel-demo     # adds it to [monitor] mute
paddock monitor unmute ~/src/tunnel-demo
```

`mute` and `unmute` change only `[monitor] mute = ["/path/to/project"]` in `~/.config/paddock/config.toml` and keep the rest of the file with its comments. You can edit the list by hand too. A project `paddock.toml` can never mute itself.

### Boundary log

A program in a sandbox rarely says "permission denied". A path is missing, a socket is gone, or a LAN address times out, and the program falls back to its defaults without a word. paddock knows its own rules, so it writes each denial to a boundary log, `boundary.jsonl` in the run folder of the sandbox. The log is on by default.

- Host requests: each `paddock-host` request that paddock refused or that the user denied, for example a `fetch` of a protected path, an `exec` in `deny` mode, a tool call that matches no shape, or a refused `borrow`. A note of the user stays out of the log.
- Network: each target that the host proxy refused, with host and port: a private network address, a host loopback port, an address of this host, or a host outside the list in network mode `api` (or `allowlist` on macOS).
- Environment: each variable that paddock withheld at the start (secret-looking names, cloud credentials, `paddock env hide`). The log holds the name, never the value.
- MCP: each MCP server that does not start in the sandbox, with the reason.

Each line has the time, the kind, the target, the rule and a count. The same denial within a minute raises the count of its line. The log never holds content. It goes away with the run folder when the sandbox ends.

```sh
paddock boundary 1a2b3c        # the denials, by kind and target, with counts
paddock boundary 1a2b3c --follow   # and each new denial as it happens
paddock-host boundary          # inside the sandbox: its own log, read-only
```

`paddock ps --json` shows `boundary_count` for each sandbox, and the bar panel shows the count on the row with the command that reads the log. File access that the kernel refuses inside the sandbox is not in the log: paddock does not trace the file system.

## Logins and MCP

By default, a sandbox uses your existing login (auth mode `shared`). paddock copies the credential files of the harness from your home directory into the sandbox at start. While the sandbox runs, paddock keeps its copy and your home in line every two seconds, both ways. When the harness refreshes a token in the sandbox, your home gets it at once, and every other sandbox on the same login gets it from there. When your home changes (a new login, or `claude` on the host), the sandbox gets it. This matters for OAuth logins whose refresh token works only once: with a copy that went back only at the end, a second sandbox started meanwhile got a token that no longer worked. When both sides changed, the login that expires later wins, by the field that the harness profile names in `[state] auth_expiry` (for Claude Code `claudeAiOauth.expiresAt`); without that field, the end of the run decides, and the host version stays. Only JSON login files move while the sandbox runs: a database file that the harness keeps open (kiro-cli) is copied back at the end, as before.

Some harnesses keep their login in the system keyring. Copilot CLI is one of them. Its sandbox gets a private keyring of its own (gnome-keyring, on a private session bus), encrypted with a password that paddock keeps on the host. The sandbox cannot reach your host keyring, and your host keyring does not see the sandbox's login. So you log in once inside the sandbox, and the login stays for later runs.

For a separate account, use an isolated login. The sandbox then starts logged out, and you log in inside it. Each name is a separate store.

```sh
paddock claude --auth isolated          # one separate login
paddock claude --auth isolated:work     # another one, named "work"
paddock auth import claude --auth isolated:work   # start it from a copy of your login
```

### Codex logins: the refresh stays outside the sandbox

A ChatGPT login holds an access token, which expires soon, and a refresh token, which trades for a new pair and is then spent. When two copies of one login each spend the same refresh token, one of them is logged out, and a service can take the reuse for a stolen token and end the login.

So for Codex, paddock keeps the refresh token out of the sandbox:

- The sandbox gets `~/.codex/auth.json` without its refresh token (the field is there, empty). Codex in the sandbox uses the access token as usual, and cannot refresh it. An agent in the sandbox can take at most an access token that expires soon.
- A keeper refreshes the login before the access token runs out (30 minutes before, by default; Codex itself would refresh 5 minutes before). It runs Codex's own refresh (`codex app-server`) in a sandbox of its own that looks like the agent's sandbox to OpenAI: the same machine ID, the host name `paddock`, and your routes and proxy. It sees nothing of your home but the login.
- The keeper writes the new login back where it lives (your `~/.codex/auth.json`, or the isolated store), and the new access token, without the refresh token, into every running sandbox that uses the login. Codex reads the file again before it would refresh, so it takes the new token.
- Several sandboxes of one login share one keeper. When the sandbox whose paddock runs it ends, another one takes over. If a refresh fails three times in a row, paddock shows a notification.
- The sandbox's copy is never written back at exit. A login that you make inside the sandbox, when it started logged out, is saved as before.
- The limits in the Omarchy agents panel keep working while a sandbox uses your Codex login. Omarchy's own Codex collector runs in a keeper sandbox, with your home read-only and the full login. It waits for the keeper, so the two never refresh at the same time. If it refreshes the login, the result goes back to `~/.codex/auth.json` and into the sandboxes, as after the keeper's own refresh.

In `~/.config/paddock/config.toml`:

```toml
[logins]
keep = true              # false: copy the whole login into the sandbox, as before
refresh_margin = 1800    # seconds before the access token expires; at least 360
check_interval = 60      # seconds between two looks
```

`paddock logins refresh codex [--auth isolated:NAME]` refreshes a login at once, in a keeper sandbox, and updates the running sandboxes that use it. It waits while another run with the login (the keeper, or the usage collector) finishes.

An API-key login is not touched. Claude Code and Grok Build logins are copied whole, as before.

### MCP servers

MCP servers come from the same configuration as on the host, and MCP OAuth tokens come with the login. A stdio server runs a command on your computer, so paddock must mount that command into the sandbox. paddock mounts only the smallest part that runs: the binary, the interpreter in its script header, or the `node_modules` folder of a package. It never mounts the whole repository or install folder around a package.

paddock trusts the configuration in your home folder, such as `~/.claude.json`. A server there runs without a question when its command is in a tool install folder: system folders, mise, uv, pipx, a `node_modules` folder, or a single file in `~/.local/bin`.

paddock does not trust a configuration file in the project, such as `.mcp.json`, `opencode.json` or `.gemini/settings.json`. Anyone who can change the project can change these files, and so can the agent in the sandbox. A server from a project file runs without a question only when its command is in the project, in a system folder, or in the shared tool installs in your home (mise, asdf, uv, pipx, `~/.local/bin`). A command anywhere else, for example in another repository's `node_modules`, stays off. The same applies to a script header that names an interpreter outside these folders.

A server that needs other host paths, or a server on a host loopback port, stays off until you allow it. `paddock mcp` shows each server, where it comes from, and the exact host paths that paddock mounts when you allow it:

```sh
paddock mcp claude     # each server, and whether it runs in the sandbox
```

paddock hides `.env` files, keys and other credential-shaped files in every folder that it mounts for a server, as it does in the project. A folder in the shared tool installs is the exception. To show one such file, add it to `[secrets] reveal`.

```toml
# ~/.config/paddock/config.toml or the project paddock.toml
[mcp]
allow = ["my-server"]   # mount the host paths that this server needs, read-only.
                        # In a project paddock.toml, you approve this once on the host.

[network]
host_ports = [8765]     # a local MCP server on the host loopback
```

Some harnesses can keep MCP tokens in the system keyring instead of a file. The sandbox cannot reach the keyring. For Codex, set `mcp_oauth_credentials_store = "file"` to share the tokens.

## Environment variables

By default, the sandbox gets the host environment, without the variables that look like secrets: their names contain `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `API_KEY`, `APIKEY`, `CREDENTIAL` or `PRIVATE_KEY`. Those stay on the host. Two kinds pass all the same, with their values, and the agent can read them: the login variables that the harness's profile names (`env_pass`, for example `ANTHROPIC_API_KEY` for Claude Code and `OPENAI_API_KEY` for Codex), so that the harness can sign in, and the variables that you name in `[env] pass`. A later paddock version keeps the login variables on the host too, behind a local gateway. The first time in a project, and each time the lists change, paddock prints how many variables it passed, the secrets that stayed on the host, and the login variables that passed. High-privilege cloud and infrastructure credentials are not passed either: AWS keys and session tokens, Azure client secrets, `GOOGLE_APPLICATION_CREDENTIALS`, Cloudflare, DigitalOcean and Hetzner tokens, `VAULT_TOKEN`, Terraform `TF_TOKEN_*`, forge tokens such as `GH_TOKEN` and `GITLAB_TOKEN`, and similar. They can create, delete or bill resources far beyond the project. The notice names the ones that it dropped, and `paddock env show` lists them. To pass one, name it in `[env] pass`. Variables that name a host socket or a control channel (`SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `HYPRLAND_INSTANCE_SIGNATURE`, `DOCKER_HOST`, terminal multiplexer sockets, the Claude app's `CLAUDE_CODE_MESSAGING_*`) are never passed, and the sandbox sets `HOME`, `PATH` and `TMPDIR` itself.

To pass only an allowlist (terminal, locale, the variables of the harness profile, `[env] pass`, and the host proxy), use `--env clean`, or set this in `~/.config/paddock/config.toml`:

```toml
[env]
inherit = false
pass = ["MY_TOOL_TOKEN"]
```

### Credentials

A variable in `[env] pass` reaches the sandbox at every start, without a question. For a credential that you want to control, register it in `~/.config/paddock/config.toml`. There are three ways to give it to a sandbox.

`via = "gateway"` keeps the value on the host. The sandbox gets a stand-in with the same start and length, and a base URL that points at a local gateway. For each request, paddock removes every credential that the sandbox sent, adds the real one, and sends the request over HTTPS to the host that you bound it to: from your computer, through your proxy settings, with your system's certificates. No TLS is decrypted and no certificate authority is made. Answers stream back as they come, so model APIs work.

```toml
[credentials.ANTHROPIC_API_KEY]
via = "gateway"
preset = "anthropic"        # api.anthropic.com, x-api-key, ANTHROPIC_BASE_URL

[credentials.OPENAI_API_KEY]
via = "gateway"
preset = "openai"           # api.openai.com, Authorization: Bearer, OPENAI_BASE_URL (/v1)

[credentials.GITHUB_TOKEN]
via = "gateway"
preset = "github"           # api.github.com, Authorization: Bearer, GITHUB_API_URL
paths = ["/repos/my-org/"]  # optional: only these path prefixes
```

Without a preset, name `hosts` (`HOST` or `HOST:PORT`), `base_url_env` (the variable that points the tool at the gateway), and how the value goes: `header` (default `Authorization`) with `prefix` (default `Bearer ` for Authorization), or `query` for an API that takes the key as a query parameter. `env` names the variable for the stand-in (default: the name), and `base_path` is added to the base URL. A prefix matches whole path segments (`/v1` is not `/v10`), and a bound route refuses paths with `..`, `//` or an encoded dot, slash or backslash. A request to another path gets 403, and nothing leaves your computer. After 20 refused requests, paddock freezes the sandbox and tells you. The gateway works in every network mode, also `none`: the sandbox can then reach the model API and nothing else. It needs the bwrap, docker or podman backend; srt cannot reach it. A tool must accept a base URL: the `gh` command does not, because it talks only HTTPS to its host.

git over HTTPS works through the gateway too, for `git fetch`, `git pull`, `git clone` of a private repository and submodules:

```toml
[credentials.github_git]
via = "gateway"
preset = "github-git"     # github.com, git fetch and clone only
from = "command"
command = ["gh", "auth", "token"]
```

paddock sets `url.http://127.0.0.1:3130/s/github_git/.insteadOf = https://github.com/` for git in the sandbox, in the environment (`GIT_CONFIG_COUNT`), never in a repository file. Your remotes keep their `https://github.com/` URLs. The gateway adds `Authorization: Basic` with the user `x-access-token` and the token, and lets only git smart HTTP through: `/OWNER/REPO/info/refs` and `/OWNER/REPO/git-upload-pack`. A `*` in `paths` stands for one path segment. The audit log names the repository path, never the token. `git push` still goes through `paddock-host git push`, with your own credentials and an approval card. To push through the gateway instead, add `"/*/*/git-receive-pack"` to `paths` together with the two paths above. For GitHub Enterprise, set `hosts = ["git.example.com"]`. ssh remotes do not change: they use the ssh agent channel.

`via = "deliver"` hands the value over. It enters the sandbox as a variable, the agent can read it, and it cannot be taken back. Use it only for a tool that can do nothing else, such as a database password:

```toml
[credentials.DB_PASSWORD]
via = "deliver"
from = "command"                    # or "env": a host variable (var = "NAME", default: the name)
command = ["secret-tool", "lookup", "service", "mydb"]
harnesses = ["claude"]              # optional: only these harnesses
```

With `as = "file"`, paddock sets no variable. The sandbox reads the value in the file `/dev/paddock/secret/NAME` (on srt: `$PADDOCK_DEVICES/secret/NAME`), and only that sandbox sees it. A read returns the value as it is, with no newline added. Each open counts as one use in the audit log and in `paddock credentials list`. `paddock credentials revoke` makes the file fail at once. `/dev/paddock/.catalog` lists the file by name. The child processes of the harness do not inherit the value, and it is not in their `/proc/PID/environ`. Give it to one command only, or to a tool that reads a token file:

```toml
[credentials.GH_TOKEN]
via = "deliver"
as = "file"                         # default: "env"
from = "command"
command = ["gh", "auth", "token"]
```

```sh
GH_TOKEN=$(cat /dev/paddock/secret/GH_TOKEN) gh pr list
```

The file needs `/dev/paddock` (FUSE on the host). Without it, the sandbox starts without the credential and paddock says so. On macOS the file is a FIFO at `$PADDOCK_DEVICES/secret/NAME` (see [On macOS](#devices-on-macos)):

```sh
GH_TOKEN=$(cat "$PADDOCK_DEVICES/secret/GH_TOKEN") gh pr list
```

`via = "agent"` with `keys` gives the sandbox your ssh keys through your ssh agent. The private keys stay in the ssh agent on your computer (`SSH_AUTH_SOCK` where paddock started). The sandbox gets its own agent socket, and `SSH_AUTH_SOCK` in the sandbox points at it. paddock answers on that socket: a list of keys shows only the keys that you name, and a signature passes only for them. Every other request (add or remove keys, lock, unlock, extensions) fails and never reaches your agent. A message over 256 KiB, or a malformed one, fails too.

```toml
[credentials.ssh]
via = "agent"
keys = ["~/.ssh/id_ed25519.pub"]   # the public keys; only these keys
confirm = "each"                   # each: a card for every signature; session: once on the start card
```

With `confirm = "each"`, the approval card asks for every signature and shows the key's fingerprint and comment, never the data to sign. When two credentials name the same key, the stricter one that is not revoked decides, and a revoke while the card is open wins over Allow. The audit log records each signature with the sandbox, the credential name, the key fingerprint and the answer. When your agent is not there, or does not hold a key, paddock says so at start, and the sandbox starts without the socket.

What it protects: the private key never enters the sandbox, so the agent cannot copy it, and the sandbox cannot add, remove or use other keys. What it does not protect: while the run is allowed, the sandbox can ask for signatures with the named keys, for example to push or to log in to a server that trusts them. Use `confirm = "each"` to see each one, and revoke the credential to end it.

`via = "agent"` with `kind = "docker"` gives docker in the sandbox the login of listed registries, one request at a time:

```toml
[credentials.registry]
via = "agent"
kind = "docker"
registries = ["ghcr.io", "registry.example.com"]
```

The sandbox gets `DOCKER_CONFIG=/run/paddock/docker`, whose `config.json` sends only these registries to the helper `docker-credential-paddock`. The host takes the login from your own docker configuration (`credHelpers`, then `credsStore`, then `auths`) and refuses any registry that is not listed. The helper refuses `store`, `erase` and `list`. What it protects: the sandbox never sees your docker configuration or the logins of other registries, each registry is asked once per run (a Yes or a No holds for the rest of the run), audited and can be revoked. What it does not protect: the login of a listed registry reaches the sandbox's docker client for that request.

`via = "agent"` with `kind = "aws"` gives the sandbox short-lived credentials of one profile of your `~/.aws/config`:

```toml
[credentials.aws-dev]
via = "agent"
kind = "aws"
profile = "dev"
duration = 900                    # seconds, 900 to 3600
```

The sandbox gets its own `AWS_CONFIG_FILE`, whose profile `dev` runs `paddock-host credential aws aws-dev` as its `credential_process`, and an empty shared credentials file. The host runs `aws sts get-session-token` for the profile and returns only the temporary keys with their expiry. A profile that gives temporary credentials already (single sign-on, an assumed role) passes them on as they are, and `duration` does not apply to them. What it protects: the long-lived keys of the profile never leave your computer, and each set expires. What it does not protect: until it expires, a set works like any AWS credential of that profile.

For every way:

- When a sandbox starts, the approval card asks twice: first whether to give the credentials, then to confirm. The harness starts only after the second answer. With "No" at either step, or no answer, the sandbox starts without them.
- paddock reads the value only after both answers. A command runs on the host. paddock never writes the value to disk.
- A registered credential never passes in any other way: not with the inherited environment, not as a harness login variable, not through `[env] pass`. To keep a harness login key such as `ANTHROPIC_API_KEY` on the host, register it with its preset.
- A sandbox that another one started gets no credentials. A project `paddock.toml` cannot register one.
- `paddock credentials revoke ID [NAME]` stops a sandbox from using a gateway credential, an agent channel (ssh, docker or AWS) or a secret file at once. The sandbox keeps running. The bar panel has a button for it. A value delivered as a variable cannot be revoked: stop the sandbox.
- The paddock process locks delivered and gateway values in RAM, so they never go to swap. It overwrites them with zeros after a revoke and when the sandbox ends.
- The bar panel marks a sandbox that holds a delivered value, and names the gateway credentials that it uses.
- `paddock credentials list` shows the registered credentials, whether their source is there, how often each was delivered, how many requests went through the gateway, how many ssh signatures were allowed, and how often each secret file was opened. The audit log (`paddock log`) records each question, answer, request (method, host, path without its query) and revocation by name, never a value.

## Privacy defaults

paddock turns off vendor telemetry in every sandbox. It uses the switches that each vendor documents. It sets them only inside the sandbox: as environment variables, or in the sandbox's own copy of a settings file. Your host configuration does not change. `paddock explain` shows each switch and the profile that sets it.

| Program | What paddock turns off |
|---|---|
| Claude Code | telemetry (`DISABLE_TELEMETRY`), error reports (`DISABLE_ERROR_REPORTING`), the session survey (`CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`) |
| Codex | analytics and the metrics exporter (`[analytics] enabled = false` in `~/.codex/config.toml`) |
| Copilot CLI | the telemetry that the CLI sends (`--disable-client-telemetry`) |
| Gemini CLI | usage statistics (`privacy.usageStatisticsEnabled` in `~/.gemini/settings.json`) and telemetry (`GEMINI_TELEMETRY_ENABLED`) |
| Grok Build | telemetry, usage metrics, crash reports, Mixpanel and trace uploads (`GROK_TELEMETRY_*`, `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING`) |
| opencode | automatic sharing of sessions from a project `opencode.json` (`share` stays `manual`). opencode documents no telemetry switch. |
| VS Code | usage data, error telemetry and crash reports (`telemetry.telemetryLevel: off`), and experiments |

dsh sends the session log with each request to the DeepSeek API (the plugin `session-log-deepseek`, on by default). paddock turns it off in every dsh run: it adds the row `enabled: false` for that plugin to the sandbox copy of `~/.dsh/cordis.patch.yml`, and `paddock run dsh` prints one line about it. The host file does not change. This switch does not follow `telemetry = "vendor"`. To keep the upload on, set this in `~/.config/paddock/config.toml`:

```toml
[profiles.dsh]
upload_session_logs = true
```

`DISABLE_TELEMETRY` also stops the feature flags of Claude Code. Some features need them, for example Remote Control. The feedback commands of the harnesses still work. They send data only when you use them.

Some settings belong to your account, and no program can change them. The use of your data for model training is one of them. The Antigravity telemetry switch is another. The Claude and ChatGPT desktop apps document no telemetry setting at all. `paddock doctor` prints, for each installed program, where you turn off training:

| Vendor | Where |
|---|---|
| Anthropic (Claude Code, Claude app) | [claude.ai privacy settings](https://claude.ai/settings/data-privacy-controls), "Help improve Claude" |
| OpenAI (Codex, ChatGPT app) | [ChatGPT data controls](https://chatgpt.com/#settings/DataControls), "Improve the model for everyone" |
| GitHub (Copilot CLI, Copilot in VS Code) | [Copilot settings](https://github.com/settings/copilot), "Allow GitHub to use my data for AI model training" |
| Google (Gemini CLI) | the [privacy notice](https://developers.google.com/gemini-code-assist/resources/privacy-notices) of your sign-in method |
| Google (Antigravity) | `/settings` in agy, "Enable Telemetry" ([docs](https://antigravity.google/docs/settings)) |
| xAI (Grok Build) | `/privacy` in grok, coding-data sharing ([docs](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md#related-settings)) |
| opencode | the data settings of the model provider that you use |

To keep the vendor defaults, set this in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change it, in either direction.

```toml
[privacy]
telemetry = "vendor"   # default: "off"
```

#### The training question in a harness's setup

Some harnesses ask at a new login whether the vendor can use your data for training, with the box already checked. Antigravity (`agy`) asks it on its "Terms of Service & Data Use" screen. The answer goes to your account, so no file can preset it. By default, paddock unchecks that box and presses [Done] for you. It does this only while the harness sets up, before your task starts:

- paddock gives the harness a terminal of its own and reads the visible screen. It acts only when the whole dialog matches a rule of the built-in profile, row for row. It sends one key, reads the screen again, and presses [Done] only when the box shows unchecked.
- paddock answers only this question. It never answers terms, login, folder trust, billing or tool approvals.
- A screen that differs from the rule, a resize of your terminal, or a key that you press on the dialog stops paddock. The screen is then yours. Keys on the login and theme screens before the dialog go to the harness as usual.
- While it waits for the dialog, paddock gives the harness the terminal width of the rule (49 columns for agy). The harness gets the full width back when the dialog is done. A terminal that is narrower gets no help.
- The audit log (`paddock log`) gets one line with the rule, the keys and the outcome, never the screen text. paddock prints the outcome when the run ends.

Choose the behavior in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change it.

```toml
[privacy]
training = "manual"   # default: "deny"
```

| Value | What paddock does |
|---|---|
| `deny` | It unchecks the training box in the harness's setup. |
| `manual` | It types nothing. A run without a terminal stops while the harness's setup can still ask, so that you answer it once in a terminal. |
| `vendor` | It does not touch the question. Use this to opt in. |

paddock skips this when the harness has finished its onboarding (`[state] done_flags`). The rules come only with paddock's built-in profiles. A profile in `harness.d` can name one with `rules = ["agy-1.4-data-use"]` in `[privacy.training]`.

### The machine identity

A sandbox does not show the identity of your machine to the programs in it. The host name is always `paddock`. What else a program can read depends on the login:

| | Shared login (default) | Isolated login (`--auth isolated:NAME`) |
| :--- | :--- | :--- |
| `/etc/machine-id` | your machine's | an ID of this login, the same at every start |
| Boot ID (`/proc/sys/kernel/random/boot_id`) | your machine's | derived from the login's ID: new after a reboot, different for each login |

A shared login is your own login from the host, so it keeps your machine's values: a login that you made on the host keeps working. An isolated login is another machine: two isolated logins cannot be linked to each other or to your machine. paddock makes the ID at the first start and keeps it in the folder of the login (`.paddock-machine-id` in `~/.local/share/paddock/auth/HARNESS/NAME/`). It never makes a new one at each start, because a login that shows a new machine at every start looks like a login copied to many machines. When you delete the folder of the login, the ID goes with it.

Hardware identity:

- The MAC addresses of your network interfaces are hidden. In network modes `none` and `egress`, the sandbox has network interfaces of its own. With `--gpu`, paddock mounts `/sys/devices` for the graphics drivers, and hides the network folders and the board and firmware names (`/sys/devices/virtual/dmi`) in it. Network mode `host` shares your network stack, so the sandbox sees your interfaces and their MAC addresses there.
- The CPU model in `/proc/cpuinfo` stays visible: programs read it to choose CPU features.

Each backend has limits:

- docker and podman show the machine ID as above, never the one of the image. They show your machine's boot ID, because container runtimes refuse mounts inside `/proc`. They also hide the board and firmware names.
- srt cannot put another file at a host path. With an isolated login, a program in srt reads no machine ID at all. srt mounts `/proc` itself, so the program reads your machine's boot ID. srt also hides the MAC addresses and the board and firmware names.

## Desktop projects

The sandbox has the libraries for desktop apps: the bwrap backend sees the host `/usr`, and the docker backend installs them in its image. Access to the display is the risk. A client of the host compositor can capture the screen and read the clipboard, and on wlroots compositors it can type into other windows. paddock has four display modes:

| Mode | What the agent gets |
| :--- | :--- |
| `off` | No display. The default for projects that are not desktop projects. |
| `headless` | A private compositor inside the sandbox, with Xwayland. The agent runs the app and inspects it with `paddock-screenshot`. Nothing on your desktop is reachable. paddock turns this mode on when it finds a desktop project (Tauri, Electron, GTK, Qt, egui, iced, Flutter and others). |
| `window` | A nested compositor that runs on your desktop as one window. You watch the app, and the agent sees only that window. |
| `host` | Your real compositor socket. Use it only when you must. Needs approval. |

```sh
paddock claude --gui window
```

Add `--gpu` to bind the GPU render nodes. Without it, apps render in software.

## Host requests

Sometimes the agent needs one thing from your computer: a screenshot of the real screen, one command on the host, or one file outside the project. paddock does not open the sandbox for that. The agent asks with `paddock-host`, you decide on the host, and paddock does the work on the host.

| Capability | What the host does | Default |
| :--- | :--- | :--- |
| `screenshot` | Captures the screen, one output or the focused window | ask |
| `exec` | Runs one command in the project folder, without a shell, isolated (see below) | ask, and never allow |
| `tool` | Runs a named host tool of `[host.tools]` with allowed arguments only (see [Named host tools](#named-host-tools)) | the mode of each tool |
| `fetch` | Copies one host file or folder into the project, at most 64 MiB | ask |
| `borrow` | Copies a host folder to `/run/paddock/borrow/` for changes (see [Borrow a host folder](#borrow-a-host-folder)) | ask |
| `return` | Writes the changes of a borrowed copy back to its host folder | ask, and once per return |
| `clipboard-read`, `clipboard-write` | Reads or sets the host clipboard | ask |
| `open-url` | Opens an http or https link in your browser, for example a harness login page | ask |
| `notify` | Shows a desktop notification | allow |
| `git-push` | Pushes commits to a remote of the project, with your credentials | ask |
| `forge` | Runs a fixed set of `gh`, `tea` and `glab` commands on the project's repository, with your login | reads allow, writes ask |
| `spawn` | Starts another harness in its own sandbox with one task (see [Spawn other agents](#spawn-other-agents)) | ask, and off until `[spawn] harnesses` lists one |

A request travels through the gateway device `/dev/paddock/gw`, which every sandbox with `/dev/paddock` has. `paddock-host` writes the request there and reads the result. paddock normalizes each request and names it by a hash of its capability and arguments. The same request sent again while it waits joins the one that waits: you see one card, and the work runs once. A read waits at most 100 seconds (a `timeout` in the request sets 1 to 600); when that passes first, the read fails with `ETIME` ("still waiting"), and `paddock-host` sends the same request again on its own. An agent can also write to the device itself. A sandbox without `/dev/paddock` uses the host socket as before.

The paddock process that runs a sandbox decides and asks you, but it does not run an approved command itself. For `exec`, it signs a grant for exactly that request: a hash of the capability and the arguments, the sandbox, the project, a lifetime of 30 seconds and a random number. It signs with HMAC-SHA256 and a key in `$XDG_RUNTIME_DIR/paddock/gateway.key` (mode 0600; macOS: see below). That folder is in memory, so each login gets a new key. A separate process, `paddock gatewayd`, checks the signature, the time, that the grant was not used before, and that it covers this request, and only then runs the command. paddock starts the executor when a request needs it, and the executor stops after two minutes without requests. The command of an approved `exec` runs isolated, in a transient systemd user unit (`systemd-run --user`), or with bwrap where there is no systemd user instance. It sees the system read-only, the project read-write, an empty home folder and its own `/tmp`. It can use the network, but it cannot gain privileges or create namespaces, and it cannot reach your systemd user manager or session bus: the runtime folder (`/run/user`) is hidden from it. Of your environment, only `PATH`, the language, the time zone, `TERM`, `USER` and `LOGNAME` pass: tokens in your environment stay out. `[exec]` in `~/.config/paddock/config.toml` widens this, and a project `paddock.toml` cannot set it:

```toml
[exec]
home = "hidden"        # hidden (default), read-only, or full
network = true         # false: no network at all
paths = ["~/src/lib"]  # more folders, read-write
ro_paths = ["/srv/data"]
env = ["GOPATH"]       # more variables to pass, by name
```

`paddock gateway install` lets systemd (launchd on macOS) start it instead, on the first request (socket activation). `paddock gateway remove` turns that off. Every capability except `fetch` and `spawn` now runs in the executor, each with its own limits:

| Capability | In the executor |
| :--- | :--- |
| `exec` | isolated as below |
| `git-push`, `forge` | home read-only (your login and keys stay usable), only the scratch or an empty folder writable, network on, only the variables of git, SSH or gh |
| `screenshot`, `clipboard-read`, `clipboard-write` | no home, no network, of the runtime folder only the Wayland socket and the compositor's sockets; `wl-copy` keeps a copied text in a unit of its own until you copy something else |
| `open-url` | through the desktop portal (OpenURI), which starts your browser outside paddock; `xdg-open` with only the variables of a browser where there is no portal |
| `notify` | a message on the session bus |

On macOS, the executor runs an approved command with `sandbox-exec` and a Seatbelt profile. paddock makes the profile from the same `[exec]` configuration. The command sees the system folders read-only, including Homebrew, Nix and the developer tools. It sees the project read-write, an empty home folder and its own `TMPDIR`. It can connect to the network, and it can listen on localhost only. Seatbelt denies the rest of your files. `home = "read-only"` and `home = "full"` show your real home, and `network = false` allows no IP connection. `git-push` and `forge` see your home read-only and can reach the login keychain through `securityd`, so `gh` and git credential helpers keep working. The folder of the `gh` that paddock found on your PATH is read-only in the view too. The command starts in its working folder, and `PWD` names that folder as paddock got it, so `pwd` prints `/tmp/x`, not `/private/tmp/x`. The system keychains stay closed. The signing key is in the temporary folder of your user (`getconf DARWIN_USER_TEMP_DIR`), which macOS empties at each restart. The executor listens on `~/.local/state/paddock/gatewayd.sock`. `paddock gateway install` writes `~/Library/LaunchAgents/com.harnessix.paddock.gatewayd.plist` and loads it. Then launchd owns the socket and starts the executor on the first request, also after a new login. `paddock gateway remove` unloads the agent and deletes the file. `screenshot`, `clipboard-read` and `clipboard-write` are not available on macOS yet.

`fetch` stays in the paddock process: it opens the file before it asks you, so a path that changes during the wait changes nothing, and the executor would add no limit to reading that one file. `spawn` starts a sandbox, which only paddock does. `fetch` never copies paddock's own folders, with the key, into a sandbox.

A request that asks waits for your answer for up to two minutes. You can answer in three places, and the first answer wins:

- An approval card appears at the top of the screen as soon as a request arrives, without opening the panel. It shows the question, exactly what will run and in which folder, the reason that the agent gave, and the time left. Click No, Yes once, Yes for this session or No for this session. The field "Note for the agent" goes with either No: the agent gets the note in the answer (`the user denied this request: "..."`), so it knows why and does not ask again. The card never takes the keyboard when it appears, so typing in another window cannot answer it. To answer with the keyboard, bind a key to `omarchy-shell harnessix.paddock.approvals keys`: the card then starts on No.
- The bar widget turns red and shows the number of waiting requests. The panel lists them and the most recent answers, including the ones that the agent approver gave.
- Without the Omarchy plugin, a desktop notification offers the same three answers.
- In a terminal, `paddock approvals` lists the requests, and `paddock approvals answer <id> once|session|deny|deny-session` answers one. `deny --text TEXT` adds a note for the agent, and `--text -` reads it from standard input.
- "No for this session" (`deny-session`) denies the request and every same request of that sandbox until the sandbox ends, at once and without a new card, with the same note. "The same request" is what "Allow for this session" covers.
- A note is one line of at most 300 characters. paddock removes control characters. The note goes only to the agent that asked. The audit log records its length, never the text.

Some rules hold whatever your configuration says. `exec` is never allowed without asking, and "Allow for this session" covers only the identical command line. `fetch` refuses the protected paths (`~/.ssh`, `~/.gnupg`, cloud credentials, harness logins and the others), even if you approve. It also refuses a folder that holds one. paddock opens the path before it asks you, and the card shows where the path really leads. The copy reads what paddock opened, so the agent cannot swap the path while you decide. `fetch` copies regular files and folders only, and skips symlinks, pipes and devices. Results that are files go to `.paddock/inbox/` in the project, which git ignores.

Set the mode of each capability in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change these modes, because the agent can edit it.

```toml
[host]
screenshot = "allow"   # deny | ask | allow
clipboard-read = "deny"
wait = 60              # seconds to wait for an answer
```

### Borrow a host folder

A task sometimes needs to change a folder outside the project, for example a sibling library or another repository. `fetch` gives a copy with no way back, and `[mounts]` needs a restart. `borrow` and `return` fill the gap:

```sh
paddock-host borrow /srv/shared-lib --reason "fix the parser bug there"
# prints /run/paddock/borrow/shared-lib. The agent changes that copy.
paddock-host return shared-lib --reason "the parser fix, tests pass"
```

Every sandbox gets an empty folder of its own at `/run/paddock/borrow/` (on the host `$XDG_DATA_HOME/paddock/borrow/<sandbox id>/`). A Seatbelt run on macOS sees that host folder at its own path, in `PADDOCK_BORROW_DIR`. `borrow` opens the path first. Its last part must not be a symlink, and the card shows where the path really leads. paddock refuses the protected paths of `fetch` and every folder that holds one. It copies the folder with reflinks where the file system has them. Without reflinks the limit is `[borrow] max` (default `"1G"`). paddock records the state of the original in a manifest that the sandbox cannot see: path, size and time of each file, and a hash of each file up to 8 MiB. The original does not change.

`return` compares the copy with the manifest. The card lists the added, changed and deleted files, at most 30 names per list with "+N more". paddock refuses the return when the original changed after the borrow, so it never overwrites a change of yours or of another agent. It checks again after your answer, just before it writes. A return never writes `.git/config`, the hooks or the other git files that paddock binds read-only in a project, so a folder that holds one cannot become a file. New files get no execute bit. Symlinks that point outside the folder stop the return, also when they get there through other symlinks of the copy. A folder that host programs read or run, for example under `~/.config`, gets a warning on the card. After a return the copy is gone, and a new `borrow` starts again. When the sandbox ends, paddock deletes the copies that were not returned. `paddock du` shows the borrow folders, and `paddock gc` removes the ones of sandboxes that ended without that step. The audit log records the path, the size and the counts of each borrow and return, never the content. A spawned child cannot borrow from its parent yet.

```toml
[borrow]
max = "1G"   # without reflinks: the most a borrow copies, and the most changed bytes a return takes
```

### Named host tools

`exec` asks for every new command line. For a command that you run often, define a named tool in `~/.config/paddock/config.toml`. The command is fixed, and the sandbox can only add arguments that match a shape that you wrote:

```toml
[host.tools.omarchy-restart-shell]
command = ["omarchy", "restart", "shell"]   # fixed: no shape, so no arguments
mode = "allow"

[host.tools.compose]
command = ["docker", "compose"]
args = ["up -d", "down", "ps", "logs *"]    # the allowed argument shapes
cwd = "project"                              # project (default) or home
mode = "ask"                                 # allow | ask (default) | deny
```

In the sandbox, `paddock-host tool compose up -d` runs it, and `paddock-host tool --list` shows each tool with its mode and shapes.

- paddock compares the arguments with each shape word by word. `*` matches exactly one word that does not start with `-`. `**`, only as the last word, matches the rest of the words (none or more), none of them starting with `-`. A tool without `args` takes no arguments.
- Arguments that match no shape are refused at once. Nobody gets a card for them.
- `allow` runs the tool without a card. `ask` shows a card with the tool name and the exact arguments, and "Allow for this session" covers only the same tool with the same arguments. `deny` refuses. `[host] tool = "deny"` turns off every tool.
- The tool runs in the executor, isolated like `exec`, with the same `[exec]` limits and the same clean environment. The sandbox environment never reaches it. The executor reads `[host.tools]` again and checks the arguments once more before it runs the command. The program is found on the host `PATH`. With `cwd = "home"`, the tool runs in your home folder, which it sees read-only unless `[exec] home = "full"`.
- Only the user configuration defines tools. paddock ignores `[host.tools]` in a project `paddock.toml` and logs a warning.
- The audit log records the tool name, the arguments, the decision and the exit code.

`paddock tools suggest` reads the audit log and prints a ready `[host.tools.NAME]` block for each `exec` command line that you allowed at least 3 times and whose program is a name on PATH or an absolute path (a tool cannot run `./build.sh`). It never changes your configuration. For this, the audit log has to keep command lines, and it does so only after you set `remember_allowed = true` in `[exec]` of your configuration. Then it keeps the command line of an allowed `exec` when every word is plain: no spaces, quotes or `=`, no flag that names a token, password, secret, key or login, and no long run of mixed letters and digits like a key. Such a filter cannot find every private word, so the setting is off by default.

> [!WARNING]
> A `docker` or `podman` tool runs containers on the host as you. A container can mount any host folder, so on most systems this is the same as root. The shapes limit the words, not what a compose file in the project does: the agent writes that file. Use the docker backend or a VM for container work instead. Treat a container tool as the host itself.

### Push and pull requests

The sandbox has no SSH key, no git credential helper and no `gh` login. paddock also drops `GH_TOKEN`, `GITHUB_TOKEN`, `GITLAB_TOKEN` and similar variables from the inherited environment. The agent still runs plain `git push` and `gh pr create`. The host does the work with your own credentials, and the token never enters the sandbox.

For `git push`, the sandbox git configuration sends the push to paddock on the host. The approval card shows the remote URL, each local and remote branch, the commits (at most 20) and whether it is a force push. The host pushes from a scratch repository that has no configuration and no hooks of its own. A file that the agent writes in `.git` cannot run a program on the host.

The push itself runs in the gateway executor, from the plan that the card showed: the scratch repository, the URL and the refs are part of the signed grant, so the executor pushes exactly what you approved. It runs isolated, like `exec`, but with your credentials readable: the home folder is read-only (your git and SSH configuration, keys and credential helpers still work), only the scratch repository and a remote on this machine are writable, and of your environment only the variables that git and SSH need pass (`SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, the session bus for a keyring helper, and proxies).

- Force pushes, branch deletions and pushes to the default branch ask every time. "Allow for this session" is not offered for them.
- "Allow for this session" covers one remote and one branch.
- paddock reads the remotes when the sandbox starts. Inside the sandbox, `.git/config` is read-only. If the agent names another remote, for example with `git -c`, the card marks it as new and asks every time. A new remote must be a network URL (`https://`, `ssh://`, `git://` or `host:path`): paddock refuses a path or `file://` URL on this machine, because git runs the hooks of the receiving repository there. The host always pushes to the URL that a remote had at the start.
- Hooks never run on the host, not even `pre-push`.
- Right before the push, paddock checks the object store of the repository again: `objects/info/alternates` must be unchanged, and `objects`, `objects/info` and `objects/pack` must still be plain directories. The scratch repository reads objects through them, so a symlink or a new alternates line would push objects of another repository of yours.

For `gh`, only these commands exist: `pr create`, `pr view`, `pr list`, `pr checks`, `pr diff`, `pr comment`, `issue view`, `issue list`, `issue comment`, `run view`, `run watch`, `run list` and `repo view`. The commands that only read run without a question. `pr create` and the comment commands ask. `--repo` must name the repository of one of the project's remotes, and paddock adds it when the agent leaves it out. paddock refuses `gh api`, `gh auth`, `gh secret` and every other command. It also refuses the flags that read a host file or open a program on the host, such as `--web` and `--editor`, and a `--jq` expression that reads the environment (`$ENV` or `env`). Each tool gets only its own token variables from the host environment, for example `GH_TOKEN` for `gh`. The sandbox reads `--body-file` itself and sends the text. `gh` must be installed and logged in on the host.

`tea` (Gitea and Forgejo) and `glab` (GitLab) follow the same rules. The sandbox gets a `tea` or `glab` command only when the tool is on the host. A remote on any host counts, for example `codeberg.org` or your own Forgejo or GitLab server.

- `tea`: `pulls list`, `pulls INDEX`, `issues list`, `issues INDEX` and `releases list` read. `pulls create` and `comment INDEX TEXT` ask. paddock refuses `--login`, `--remote` and `--web`. The host picks the tea login for the host of the repository from `~/.config/tea/config.yml`: the default login for that host, or else the first one. Without `--repo`, paddock uses the first remote whose host has a tea login.
- `glab`: `mr list`, `mr view`, `mr diff`, `issue list`, `issue view`, `ci view`, `ci list`, `ci status` and `repo view` read. `mr create`, `mr note` and `issue note` ask. paddock refuses `--web`, `--hostname`, `--push` and the flags that read a file, such as `--description-file`. `--repo` takes nested groups, for example `team/apps/web`.
- `tea` and `glab` cannot read the body text on their standard input. Their body text goes into the arguments of the host process.
- `tea` and `glab` take only the flags that paddock lists for them, and refuse any other flag. Their flags come from their documentation and were not checked against the real programs, and an unknown flag could read a host file into a comment.

To make reads ask too, or to turn either capability off, set it in `[host]`, for example `forge = "deny"`. `forge = "allow"` also allows the write commands without a question. `git-push = "allow"` allows ordinary pushes, but force pushes, pushes to the default branch and pushes to a new remote still ask.

`paddock log` shows every request with its decision and result. The log is `~/.local/state/paddock/host-audit.jsonl`. It does not keep text that can be private. For `clipboard-write`, it keeps only the length of the text. For `exec`, it keeps the program and the number of arguments, because arguments can hold tokens. With `[exec] remember_allowed = true`, an allowed command line of plain words stays whole, for `paddock tools suggest` (see [Named host tools](#named-host-tools)); it is off by default. For `forge`, it keeps the command without the body text. For `notify`, it keeps the lengths of the title and the body. For `open-url`, it keeps the scheme and the host. paddock makes its folders readable only by you (mode 0700) and its files too (mode 0600), and tightens older ones that other users can read. Request text never goes into the arguments of a host process, which other users can see: notifications go over the desktop bus, and `gh` reads the body on its standard input (`tea` and `glab` cannot, see above). To teach agents the client, run `paddock skill install`: it copies a short skill into the skill folders of your harnesses. Inside a sandbox, `paddock-host --help` is the full reference.

### Spawn other agents

An agent in a sandbox can start an agent of another harness. For example, Claude plans the work and Codex writes one part of it. The agent asks with `paddock-host spawn`. paddock then starts a second sandbox on the host, next to the first one. It does not put a sandbox inside a sandbox.

```sh
paddock-host spawn codex --worktree --prompt-file task.md   # prints an id
paddock-host spawn gemini --role reviewer --from-head --prompt-file review.md
paddock-host spawn codex --prompt-file task.md --timeout 30m --mem 4G --cpus 2
paddock-host spawn codex --task next --model gpt-5.5 --effort high
paddock-host status <id>
paddock-host wait <id> --timeout 600
paddock-host kill <id>
```

The child runs the harness once, without a terminal (`claude -p`, `codex exec`, `opencode run`, `copilot -p`, `agy --print` or `grok --prompt-file`), and stops when the task is done. It works in a new worktree on the branch `paddock/spawn-<harness>-<id>`, from the commit that the parent has checked out. The worktree uses the same repository, so the child's commits are in the parent's repository at once. `wait` and `status` give the branch name and the exit status. The parent reads the branch with git, for example `git diff main...paddock/spawn-codex-<id>`, and merges what it wants. If the child made no commit and left no change, paddock removes its worktree and its branch when it ends. paddock also writes a short summary to `.paddock/inbox/spawn-<id>/`: `result.json` with the state, the exit code, the branch, the commit count and a diff summary, and `output.txt` with the end of the child's final output. When the child ends, paddock also writes `result:<id>` into the `tree` scope of [paddock kv](#shared-state-paddock-kv), with `status`, `summary` and `ref` (the branch). So the parent can run `paddock-host kv watch --prefix result: --scope tree` instead of polling several children.

With `--from-head`, the child starts from the parent's files as they are now. paddock first makes a commit of them, with uncommitted and new files but without ignored ones. It uses a private index file, so the parent's files, index and branches do not change. The host git call is the hardened one that paddock uses for worktrees: it refuses a repository configuration that can start a program.

A role is a fixed set of limits. The agent names a role. It cannot combine its own limits.

| Role | Files | Environment | Push and `gh` writes |
| :--- | :--- | :--- | :--- |
| `implementer` (the default) | its own writable worktree | clean | as the parent allows |
| `reviewer` | read-only | clean | refused |

"Clean" means that only the allowlist and the harness's own variables (for example `OPENAI_API_KEY` for codex) pass. A reviewer gets network mode `api` when the parent has `egress` or `host`, so it reaches only its model API. Otherwise it keeps the network mode of the parent.

`--model NAME` and `--effort low|medium|high|xhigh` choose the model and the reasoning effort of the child. paddock passes them with the harness's own flags, which `[oneshot] model_args` and `effort_args` of the harness profile name: codex gets `-c model="NAME"` and `-c model_reasoning_effort="LEVEL"`, claude gets `--model NAME` and `--effort LEVEL`. A harness whose profile has no such entry refuses the flag. Without a flag, a `--task` child takes the hint `[model:NAME]` or `[effort:LEVEL]` of its task line in `tasks.md`, then `[spawn] effort` of your configuration, then the harness's own default. A hint or `[spawn] effort` that the harness cannot take is left out, and the card says so. paddock has no default model: without one, the child runs the default that you set up for that harness. `[spawn] models` lists the models that a spawn may ask for, by flag or by hint; paddock refuses any other one. A model name has only letters, digits and `.-_:/@`. The approval card, `paddock-host status` and `paddock ps --json` (`model`, `effort`) show what the child got.

`--timeout`, `--mem` and `--cpus` can only lower the limits in your configuration. If the host has `systemd-run --user`, paddock starts the child's host process in a systemd scope with `MemoryMax`, `CPUQuota` and `RuntimeMaxSec`, so the limits cover the child sandbox and everything in it. paddock itself stops the child at the timeout, so the child can still save its login. The scope's runtime limit is a backstop one minute later. Without `systemd-run`, paddock enforces the timeout and refuses `--mem` and `--cpus`. The audit log says why a child stopped.

With `on_timeout = "ask"`, paddock does not stop a child at its time limit. It freezes the child (see `paddock freeze`, and `[suspend] leases` holds for its leases) and asks you on an approval card: more time (15m, 30m or 1h) or Stop. The card names the parent, the child, the harness, the role and the time used. In a terminal: `paddock approvals answer ID --text 30m`, or `deny` to stop. The time on the card does not count against the child. More time thaws a child that this limit froze, and moves its limit. A sandbox the user froze stays frozen, and more time only moves the limit. Stop, or no answer within the approval wait, stops the child as at any time limit. The audit log records each decision. In this mode the scope gets no `RuntimeMaxSec`: systemd counts frozen time too, so the backstop would kill a child that waits on the card. paddock's own timer stops the child, and the child ends with its parent's paddock as always. If the parent's paddock ends while the child is frozen, the child's paddock thaws the sandbox so that stop can land.

Spawn is off until you turn it on. Only `~/.config/paddock/config.toml` can turn it on. A project `paddock.toml` cannot.

```toml
[host]
spawn = "ask"            # deny | ask | allow; ask is the default

[spawn]
harnesses = ["codex"]    # the harnesses that a sandbox may start; empty by default
max_running = 2          # children of one sandbox that run at the same time
max_depth = 1            # 1: a child cannot start children of its own
timeout = 1800           # seconds that one child may run in total
# max_mem = "4G"         # the most memory of one child; no limit by default
# max_cpus = 2           # the most CPUs of one child; no limit by default
# on_timeout = "ask"    # stop (default) | ask: freeze the child at its time limit and ask
# effort = "high"        # the effort of a child that asks for none: low | medium | high | xhigh
# models = ["gpt-5.5"]   # the models that a spawn may ask for; empty: any
```

These rules always hold:

- A child never gets more than its parent. It gets the same backend, network mode (or `api` for a reviewer), LAN setting and approved mounts, and the same project. Its environment is clean. It gets no display. Its host capability modes are at most those of the parent. Its `/dev/paddock` has at most the devices of the parent. In paddock kv it writes only its own scope and the `tree` scope. paddock uses the settings that the parent had when it started, so a change to `paddock.toml` during the run does not reach the child.
- No credentials move between sandboxes. The child uses its own login of its harness, in its own sandbox home. The parent never sees it. The parent gets only the results.
- The first spawn of each harness in a session asks you. The card shows the harness, the role, the model and the effort, the login it uses, the limits and the task. "Allow for this session" covers only that harness in that role.
- Requests from a child, for example `git-push` or `exec`, show "Started by sandbox X" on the card. An "Allow for this session" or "No for this session" answer for the parent does not apply to the child, and an answer for the child does not apply to the parent. Otherwise a sandbox could avoid a question by giving the work to a child.
- paddock counts the running children and the depth on the host. A child stops when its parent ends, when you run `paddock stop` on the parent, at its time limit, or with `paddock-host kill`. No child keeps using your quota after the parent is gone.
- The task never goes into the arguments of a process, which other users can see. The harness reads it on its standard input or from a private file. paddock reads the prompt file from the project without following symlinks, refuses pipes and devices, and allows at most 256 KiB.

`paddock ps` and the bar panel show children under their parent, and the approval card names the sandbox that started a child. `paddock log` records the parent, the child, the harness, the role, the branch, the limits, the exit status, the reason for the end and the durations. It never records the task or the output. Gemini CLI has no one-shot entry yet, because paddock did not check its flags on a real install.

### Watch a sandbox without a window

A sandbox that starts without a terminal, such as `paddock run claude -- -p < ticket` in a script or a spawned child, has nothing on your screen. When `tmux` is on your PATH, paddock shows its output in a tmux session. Click the row in the bar panel, or run:

```sh
paddock view 1a2b3c           # a new terminal with a read-only live view
paddock view 1a2b3c --write   # the same with keys
```

A terminal that no window shows, such as the pty of a background process, gets this view too, and the panel says a background process started the run.

- paddock copies the output of the harness byte for byte to the caller and to a run log, and the session shows that log. The script that started the sandbox gets the same output, exit code and input as without tmux.
- The session uses paddock's own tmux server on `$XDG_RUNTIME_DIR/paddock/view.sock`, never your tmux server or your tmux configuration. No sandbox sees the socket or the logs. `paddock doctor --escape` checks this (`view-socket-hidden`).
- In a run like this, the keys of `--write` only scroll the view (tmux copy mode) or close it. They do not reach the harness, which reads its input from the caller.
- The session stays after the sandbox ends, with the exit code on the last line. `paddock stop <id>` ends it. `paddock gc --yes` removes sessions and logs of sandboxes that ended longer ago than `[view] keep_ended` (default `2h`). The start of a new view session removes those too.
- `paddock ps --json` adds `activity` on each sandbox. It is the last line of the view log with terminal control sequences removed, at most 120 characters, or null when that log has no such line. For a detached run it is the last line of the pane.
- The same JSON adds `ended`. That list holds up to 8 runs that ended in the last 10 minutes, newest first. Each entry has `id`, `short`, `harness`, `project`, `ended`, `status` (`ok`, `failed`, `idle`, `stopped` or `unknown`) and `text` (the `done` text, at most 200 characters). A run with no `done` has status `unknown`. A run that `paddock stop` ended has status `stopped`. A run that ended because its pane stayed quiet has status `idle`.
- Without tmux, `paddock view` opens `tail -f` of the run log. It uses `xdg-terminal-exec`, or `$TERMINAL -e` without it.
- One-shot modes such as `claude -p` print nothing until the end, so the view shows little until then.
- `[view] tmux = false` in `~/.config/paddock/config.toml` turns the tmux session off.

#### Detached runs: watch and take over

`--detach` starts the harness in the background with its normal interactive interface, on the terminal of a tmux session. The task is its first prompt:

```sh
paddock run codex ~/src/app --detach --prompt-file task.md --worktree
paddock run claude --detach --prompt "Fix the failing test in src/parser.rs"
```

paddock starts the run as if you typed `paddock run codex ~/src/app` in that session, and returns at once. It prints the sandbox id and `paddock view <id> --write`. The other flags (`--worktree`, `--pasture`, `--env`, `--auth`, ...) work as without `--detach`.

- `paddock view <id> --write` attaches to the terminal of the agent: your keys go to the agent, and you take over. Without `--write`, the view is read-only. In the bar panel, the take-over button shows only for detached runs.
- The agent calls `paddock-host done [--status ok|failed] [TEXT]` when the task ends. paddock then ends the run as `paddock stop` does. When you are attached with `--write`, paddock tells you once and leaves the run to you: it ends when you close the view. The status and the first 4 KiB of the text go into the run record.
- `paddock wait <id> [--timeout 30m]` blocks until the run ends and prints the `done` status and text. It exits with 0 for ok, 1 for failed, idle or no `done`, and 124 when the time runs out. Scripts that start detached workers use it.
- A detached run that has produced output and then stays quiet becomes idle after `[detach] idle_after` (default `10m`). `paddock ps --json` adds `idle_since` (a Unix time) on that run. The bar row says `waiting for input`. paddock sends one desktop notification, `HARNESS in PROJECT waits for input: view it or stop it`. New output clears the idle state and the next quiet period notifies once more.
- After `[detach] idle_timeout` more quiet time (default `30m`), paddock ends the run the same way `paddock-host done` does. The run record status is `idle`. `paddock wait` prints `idle: ended after N minutes without output` and exits 1. N is the quiet time. A quiet time that is not a whole number of minutes is written in seconds. `idle_timeout = "0"` never ends a run for being idle. A client attached with `paddock view ID --write` keeps the run. paddock tells that client once and ends the run when the client leaves.
- `[view] keep_ended` (default `2h`) is how long the view session of an ended run stays. The run record of a detached run stays, so `paddock wait` still reads it.
- A task timer (`paddock timer`) works as for any other run.
- The harness profile says how the interface takes a first prompt, in a `[detach]` table. claude, codex and grok have one. Other harnesses refuse `--detach`. `--detach` needs tmux.

#### Start screens and stuck runs

A harness can stop at a screen that waits for a choice before it starts the task. An example is the Codex screen `Hooks need review`, which comes when one of your hooks changed. In a detached run nobody is there to answer it.

- The harness profile names such screens in `[[screens.rule]]`, each with a safe answer. For the Codex hooks review, paddock chooses `3. Continue without trusting (hooks won't run)`. The changed hooks do not run. paddock never chooses an answer that trusts, approves, accepts terms or logs in. A rule with such an answer does not load.
- paddock answers in detached runs and on terminals that no window shows. In a run that you watch, paddock does not answer, so you see the screen. A rule with `always = true` is the exception. When you are attached with `paddock view ID --write`, or when you press a key, paddock answers no more screens in that run.
- paddock tells you about each answer. It writes one line in the view log and one line in the audit log. For a detached run, `paddock wait` and `paddock ps` show a note such as `answered Codex hooks review: continued without trusting 1 hook`.
- When the screen of a detached run does not change for `[detach] stuck_after` (default `3m`), paddock puts the last screen into the run record (`stuck` in `paddock ps --json`) and sends one desktop notification. For a spawn child it writes `stuck:<RUN-ID>` into the tree scope of kv, and for a pasture member into the pasture scope, so a parent can `paddock-host kv watch` for it. `stuck_after = "0"` turns this off.
- The idle end keeps working. Its done text also shows the last lines of the screen. A known message on the screen, such as `Individual quota reached` or a rate limit, gets clear words in the done text.
- `paddock explain HARNESS` lists the screen rules and the known messages of the harness.

```toml
# ~/.config/paddock/config.toml
[detach]
idle_after = "10m"
idle_timeout = "30m"
stuck_after = "3m"
```

You can let an agent on the host answer requests for you (`paddock approver`). paddock trusts that setting only from a root-owned file (`sudo paddock approver enable --system`) or from a file signed with a password that you type when a sandbox starts, so that a process on your computer cannot turn it on for itself. The agent approver can only answer the capabilities that you list, the rules above still hold, and if it does not answer clearly, the request comes to you.

## Shared state: paddock kv

Agents need to keep state between runs and hand work to each other. paddock kv is a small key-value store for that. It lives on the host, in `~/.local/state/paddock/kv/`, with one SQLite file for each pasture, each run, each spawn tree and the `user` scope. Each sandbox reaches it through its own socket. The socket tells paddock which sandbox asks, so an agent cannot claim to be another sandbox. The paddock process of a sandbox opens only the files of the scopes that the sandbox can use.

Inside the sandbox, the agent uses `paddock-host kv`:

```sh
paddock-host kv put task:review '{"title":"review src/auth.rs","status":"todo"}' --scope pasture
paddock-host kv claim review --ttl 600 --scope pasture
paddock-host kv watch --prefix result: --scope pasture --timeout 300
```

### Pastures

Two agents share the `pasture` scope only when you put them in one pasture. Define the pasture in `~/.config/paddock/config.toml`, and start each member with `--pasture`:

```toml
[pastures.review]
members = ["claude", "codex"]
```

```sh
paddock claude --pasture review
paddock codex --pasture review
```

A pasture belongs to one project: `review` in another project is another pasture. A project `paddock.toml` cannot define pastures. A sandbox that you start without `--pasture` is a pasture of its own: nobody else reads what it writes there, and its data goes with its run. So two agents that you start in the same folder do not share state unless you say so.

The members of a pasture also share a folder, `/pasture` in each sandbox (`$PADDOCK_PASTURE` names it; the srt backend keeps it at its host path). It lives in `~/.local/share/paddock/pastures/`, outside the project, so a member can hand a file to another without a commit. `$PADDOCK_PASTURE_NAME` holds the name.

- `paddock pasture list` shows the pastures of the current folder, their running members and their keys.
- `paddock pasture stop NAME` stops every running member.
- `paddock pasture remove NAME --yes` deletes the data of a pasture: its paddock kv file and its folder. It refuses while a member runs.
- `paddock ps` lists the members of one pasture together and names the pasture.

Each member keeps its own network. A member can share a local port with the others:

```sh
# member A: a dev server on its own 127.0.0.1:3000
paddock-host pasture share 3000
# member B: the same port reaches the server of A
curl http://127.0.0.1:3000/
# member A
paddock-host pasture unshare 3000
```

- A helper in each member relays the streams through a Unix socket in that member's own folder. The others see the folder read-only, so they can connect and they cannot replace the socket. paddock starts the helper with the sandbox. It works with every backend and needs no change to the host network.
- The share is the lease `lease:port:3000` in the `pasture` scope. A second member that shares 3000 gets exit code 9 and the name of the owner. The helper of the owner renews the lease, so the port frees itself within 30 seconds after the owner ends.
- Only the ports that a member names leave its sandbox. The control ports of a harness stay inside.
- When another member already uses the port itself, it skips the port. `paddock-host pasture members` shows why, with each member, its harness and its ports.
- Limits: TCP only, ports 1024 to 65535, at most 16 shared ports per pasture.
- `paddock pasture list` on the host shows the shared ports and their owners.
- The shared folder `/pasture` stays writable by every member. Port sockets are not in that folder. A member cannot replace another member's socket. Members still share the files in `/pasture`. Put only agents that you would let edit each other's files into one pasture.

A member can also ask another member a question. The other member answers with one run of its own harness, in its own sandbox, with its own login. This is the bridge, and it is off until you turn it on for a pasture:

```toml
[pastures.review]
members = ["claude", "codex"]
bridge = true
```

```sh
# in the claude member
paddock-host pasture ask codex --prompt-file question.md --timeout 300
```

- The answer is the final text of the one-shot run (`[oneshot]` of the profile). The asker never sees the login of the other member.
- The question goes through the kv socket of the asker, a folder outside every sandbox, and the kv socket of the other member. Its helper runs the harness with the prompt on stdin or in a private file, never in the arguments of a process.
- Before the answer goes back, paddock compares it with the login secrets of every member: the token values in their login files (`state.auth`) and their delivered credentials. Spaces and case do not matter, and a match needs at least 16 characters, so short words never match. On a match, the asker gets a short refusal instead of the answer, and the audit log names the member and the file or credential, never the value. The same check runs on an error from the helper.
- Name the member by its harness, or by its sandbox id when two members run the same harness. `paddock-host pasture members` shows whether the bridge is on.
- Limits: one question at a time per member, questions and answers up to 256 KiB (the end of a longer answer), a helper error of at most 4 KiB (the end of a longer error), and a timeout of 300 seconds by default, at most `[spawn] timeout`. A member that is busy or frozen answers with exit code 9, one that ended with exit code 1.
- The audit log `kv-audit.jsonl` records who asked whom, the sizes, the time and the result. It never records the question or the answer.
- Only the user configuration turns the bridge on. A project `paddock.toml` cannot.
- The guard finds a login only as plain text. An answer that encodes it, for example in base64 or with other characters between its parts, passes. The guard stops an accident or a simple trick, not a member that works hard to leak. Put only members that you trust with each other's answers into one pasture with the bridge on.
- With the srt backend, a member can read the bridge folder outside the sandboxes. It holds the pending questions and hashes of 16-character pieces of the delivered credentials, never their values.

Members of one pasture often use one account, so they eat from the same limits. Give each member a ration:

```toml
[pastures.review]
members = ["claude", "codex"]

[pastures.review.rations]
claude = { quota = "weekly=1%", budget = "2h" }
codex  = { quota = "weekly=2%", hard = true }
```

- When a member starts, paddock sets a timer named `ration` in its own scope, as `paddock timer set` does. You own it, so the member asks you on the approval card for more.
- `quota` is a share of a limit window of the account, counted from the start. `budget` is a time. A ration needs one of them, or both. `hard = true` freezes the member when the ration is used up. Without it, the member gets a notice.
- When paddock has no usage numbers for the account, the ration counts only the time, and paddock says so at the start.
- `paddock pasture list` and `paddock-host pasture members` show each ration and how much of it is used.
- A member that used up its ration answers no bridge questions. The asker gets a refusal and exit code 3.
- Each key of `rations` names a member of `members`. A wrong key, quota or duration stops the start with an error that names the pasture.

A pasture can also have a workbench: one terminal that every member and you can use, and that belongs to no member. A command in the terminal of a member runs in that member's sandbox and can read its login. The workbench is a sandbox of its own: a shell with no login, no harness, no MCP servers, an empty home, the clean environment and no network. It sees the project, `/pasture` and the shared ports, and it can share ports too. It runs one tmux session. Turn it on for a pasture:

```toml
[pastures.review]
members = ["claude", "codex"]
workbench = true
```

```sh
paddock pasture workbench review --start     # start it
paddock pasture workbench review --attach    # attach your terminal
paddock pasture workbench review --stop      # stop it
paddock-host pasture workbench               # in a member: join it
```

- Every member and you can type into the workbench, and each one sees what the others run there. A member can read the output of your commands, and it can run commands that you then see. Type no secret into it.
- The first member that runs `paddock-host pasture workbench` starts it. In a terminal, the command attaches to it. Without a terminal, or with `--print`, it prints the tmux command. A member needs `tmux` in its sandbox to attach. When it has none, the output says so.
- The workbench stops with `--stop`, when you set `workbench = false`, and when the last member ends. When you start it before any member, it waits for them.
- The tmux socket lives in a folder of the pasture on the host, never in a shared `/tmp`. Each member sees this folder read-only at `/run/paddock/workbench` from its start, so a member that started earlier can join later. The workbench itself can write it. `$PADDOCK_WORKBENCH` names the folder (the srt backend keeps it at its host path).
- The workbench uses the backend of your configuration and the project folder of the pasture, not the worktree of a member. It gets no extra mounts, ports or host access from the project `paddock.toml`. Its home is emptied at each start.
- The audit log records each start, stop and join, and who did it. It never records what the terminal shows.
- `paddock pasture list` and `paddock-host pasture members` show whether the workbench runs.

The bar panel shows the shared data too. A "Shared data" row opens a list of the pastures, your `user` scope and the data of older versions, with their members and the number of keys and locks. A pasture opens to its keys, grouped by the part before the first `:`, with locks first: who wrote each key, when, and its size. A key opens to its value, shown as plain text. From there you can copy the value, delete the key or free a lock; each delete asks twice. The panel reads a value only when you open its key, and reads paddock kv only while it is open. Bind a key to `omarchy-shell harnessix.paddock.kv open` to open the shared data directly.

In a spawn tree, the parent writes the tasks into the `tree` scope, each child claims one with a lease and writes its result, and the parent waits for `result:`:

```sh
# parent
paddock-host kv put task:parser '{"title":"fix the parser","status":"todo"}' --scope tree
paddock-host spawn codex --prompt-file task.md
paddock-host kv watch --prefix result: --scope tree --timeout 600
# each child
paddock-host kv claim parser --ttl 600 --scope tree    # exit code 9: a sibling has it
paddock-host kv put result:parser '{"status":"done","ref":"paddock/spawn-codex-ID"}' --scope tree
```

Each key belongs to a scope:

| Scope | Who reads and writes it | Default |
| :--- | :--- | :--- |
| `self` | This sandbox only | on |
| `pasture` | The members of the pasture of this sandbox, or this sandbox only without `--pasture`. Each entry records the sandbox that wrote it. `project` is another name for it. | on |
| `parent` | Read-only: the `self` scope of the sandbox that started this one with `spawn` | only in a spawned sandbox |
| `tree` | The sandbox that you started and every sandbox that it started with `spawn`, and theirs. Each entry records its writer. | on as soon as a sandbox can use `spawn` |
| `user` | Every sandbox of every project | asks you at the first use |

An agent with a prompt injection can write a false tip into the `pasture` scope, and the other members read it. So paddock records the writer of every entry, and the client marks what another sandbox wrote as untrusted.

A spawned sandbox never gets more than its parent here either:

- It has no `pasture` scope. It shares through `tree`, and its parent decides what the pasture keeps.
- For `user`, it inherits "Allow for this session" from the sandboxes above it. Without that, it is refused, and it never asks you itself: one prompt injection must not turn into many questions. Use the scope in the root sandbox first, or set `[kv] user = "allow"`.
- The `tree` scope belongs to the root of the tree: the sandbox that you started. When the root ends, its children stop and the tree is read-only. The data stays for you to review (`paddock kv list --tree ROOT-ID`) until `paddock gc` removes it with the root's `self` data.

A write can depend on the current version of a key (`--if-version`). A key can expire (`--ttl`). `mget` and `mset` read and write several keys at once. `history` shows the last versions of a key. `watch` waits for a change of one key or of a prefix. A lease (`claim NAME --ttl SECONDS`) makes sure that only one agent does a task. paddock sets the owner of a lease from the socket. Only the owner releases it, or with `--force` a sandbox above the owner in its spawn tree (its parent, or the parent of that one), or you on the host. A sibling cannot. A lease expires at the end of its time. Keep values small: put diffs, files and logs in a git branch or a file, and store the reference. `paddock-host kv --help` in the sandbox lists the key names that agents share: `task:`, `lease:`, `result:`, `heartbeat:` and `note:`.

On the host, `paddock kv list` shows the pastures of the current folder and their keys. `paddock kv list --pasture NAME`, `paddock kv get KEY --pasture NAME`, `paddock kv delete KEY --pasture NAME` and `paddock kv release NAME --pasture NAME` read one of them. The pasture of a sandbox without one is `solo-<sandbox id>`. `--user`, `--sandbox ID` and `--tree ROOT-ID` choose another scope.

### Host scripts as principals

A host script or a CI job on the same machine can hand tasks to agents and wait for their results. It acts as a named principal from the user configuration:

```toml
[kv.principals.ci]
pastures = ["review"]       # pastures of the current project that it may use
write = true                # false: read and watch only
```

- `paddock kv put KEY [VALUE] --as ci --pasture review` writes a key (without VALUE, it reads standard input). `--ttl`, `--version` and `--project` work as for `paddock-host kv`.
- `paddock kv watch KEY --as ci --pasture review` waits for a change. `--prefix`, `--version` and `--timeout` work as inside a sandbox. Exit code 75 means "still waiting".
- `paddock kv claim NAME --ttl 60 --as ci --pasture review`, `paddock kv release NAME --as ci --pasture review` and `paddock kv delete KEY --as ci --pasture review` work on the pasture as that principal.
- Each entry records the writer `principal:ci`. Agents see that name, and read the entry as untrusted data, as every entry of a pasture.
- The first write of a principal to a pasture asks you on an approval card ("let `ci` write to the pasture `review`"). "Allow for this session" covers that principal and pasture until your login session ends. paddock keeps the grant in the run folder.
- paddock refuses a principal that is not in the configuration, and a pasture that is not in its list.
- A principal claims and releases only its own leases. To free the lease of a sandbox, use `paddock kv release NAME --pasture NAME` without `--as`.
- A principal has the same quotas as a sandbox in the pasture. The audit log records the principal, the pasture, the key, the operation and the size, never the value.

Older paddock versions kept one `project` scope for all sandboxes of a project, in `store.db`. Sandboxes no longer read it, because nobody chose to share it. `paddock kv list --legacy` shows it, and `paddock gc` removes it when nothing changed it for `keep_days`.

The limits and the `user` scope are in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change them.

```toml
[kv]
user = "ask"           # ask | allow | deny
max_value = 65536      # bytes per value
self_bytes = 4194304   # bytes per scope; also project_bytes (pasture and tree) and user_bytes
max_keys = 4096        # keys per scope
keep_days = 30         # paddock gc removes the files of a run, and the tree of a root, this many days after it ended
enabled = true
```

`paddock gc` removes the file of each run (its `self` scope and its own pasture), and the `tree` scope of spawn trees whose root ended, more than `keep_days` ago, and keys whose time to live passed. A named pasture keeps its data until you delete it. The audit log `~/.local/state/paddock/kv-audit.jsonl` records each write with its key name and size. It never records a value.

## The loft

Agents leave internal process files in repositories: instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.cursor/rules/` and others), design and plan files (`DESIGN.md`, `PLAN.md`, `NOTES.md`, `TODO.md`), Spec Kit's `.specify/` and `specs/`, and new files under `docs/`. A push publishes them. The loft is a private layer above the project for these files. Each repository gets one loft in `~/.local/share/paddock/loft/<repository key>/`. The loft is a git repository of its own, so you can see the history of the agents' notes, specs and instruction files, and undo them, apart from the project's history.

| Command | What it does |
| :--- | :--- |
| `paddock loft init` | makes the loft of the current repository and turns it on. `.paddock` in the project becomes a link to the loft's notes |
| `paddock loft status` | the state, the private files that still sit in the project, and the changes since the last commit |
| `paddock loft log`, `paddock loft diff` | the history of the loft, and the changes since its last commit |
| `paddock loft commit` | commits the changes now |
| `paddock loft adopt` | moves the private files of the repository into the loft, after a question. It says how to drop them from git history |
| `paddock loft remove` | takes the `.paddock` link away and turns the loft off. The files and the history stay |

With the loft on, every sandbox of the repository (also each `--worktree` sandbox and each spawned worker) sees the private files of the loft in place, for example `CLAUDE.md` at the project root. `.git/info/exclude` keeps them out of git. paddock commits the loft at the end of each run, one commit per run, with the sandbox id and the harness in the message. A private file that is still in the project stays there until you run `paddock loft adopt`.

Every sandbox also gets the notes folder `.paddock/` for the agents' own design notes and plans, with or without `paddock loft init`. It is in the loft, the same for the whole spawn tree, and outside git. The host request inbox stays at `.paddock/inbox/`. Outside a sandbox, `.paddock` in the project is a link to the loft's notes after `paddock loft init`, so the agents and tools that you run on the host see the same notes.

```toml
[loft]
store = "external"     # default. "repo": the private paths stay in the repository
auto = true            # a loft for every project at its first paddock run. Default: false
paths = ["notes/"]     # added to the built-in list. A folder ends with /
public = ["AGENTS.md"] # kept in git on purpose, with no notice
commit = "run"         # default. "manual": only `paddock loft commit` commits
```

A project `paddock.toml` cannot set `[loft]`. At the start of a run, paddock names the private files that git tracks or can add, unless you marked them public. The push approval has one line for each kind of private file in the push (instruction files, design files, Spec Kit files, your own paths), and one line with the new files under `docs/` that the push adds, at most five names and then a count. These lines never block a push. You decide in the approval. `paddock explain` lists each private path, where it lives, and whether git tracks it.

## Spec Kit projects

A project with a `.specify/` folder ([GitHub Spec Kit](https://github.com/github/spec-kit)) is a Spec Kit project. paddock sees the folder and organizes spawn, kv, the hooks and the watch around the active feature (`feature_directory` in `.specify/feature.json`, or else the last folder in `specs/` with a `tasks.md`). You set nothing. To turn it off, set `enabled = false` in `[speckit]` of `~/.config/paddock/config.toml`. A project `paddock.toml` cannot turn it on or off.

paddock is the only writer of `tasks.md`. Workers do not edit it. paddock keeps one record per task in the `tree` scope of paddock kv (`spec/<feature>/task/<id>`): the state (todo, claimed, running, review, done, failed), the sandbox that holds it, its branch, its time limit, the result and the review. It writes the checkboxes of `tasks.md` from these records, and a short note such as the branch at the end of a task line. Nothing else in the file changes. When you edit `tasks.md` yourself, your edit wins: paddock reads the file again before each write, adds new tasks, and keeps the records of removed tasks.

| Command | What it does |
| :--- | :--- |
| `paddock-host spawn codex --task T012` | starts a worker for one task. paddock claims the task with a lease, so two workers never take one task, and the task of a dead worker is free again |
| `paddock-host spawn codex --task next` | the same for the next ready task. Run it once for each task of a ready `[P]` group. They run side by side, up to `[spawn] max_running` |
| `paddock-host spec status` | the feature, each task with state, owner and branch, what is ready, and what waits for review |
| `paddock-host spec review T012 pass` | marks a task done after you merged its branch. `changes "TEXT"` sends it back, and TEXT goes to the next worker |
| `paddock spec status` | the same status on the host, for the current folder |
| `paddock spec adopt` | moves `.specify/` and `specs/` of a repository into the external store (see below) |

`changes` also reopens a failed task and sends the note to the next worker.

A task is ready when all tasks of the earlier phases are done, all tasks before it in its phase are done (a run of `[P]` tasks can run together), and all tasks that its text names after "depends on" are done. The prompt of a worker has the task line, its acceptance text (the goal lines of the phase and the acceptance scenarios of its user story), and the paths of the constitution, `spec.md`, `plan.md` and the contracts. The worker ends with `paddock-host done "SUMMARY"`, and the task waits for your review. Watch the tasks with `paddock-host kv watch --scope tree --prefix spec/<feature>/task/`. `paddock ps` and `paddock wait` show the task of a worker. The session note (Claude Code) and the note after a context compaction name the feature, the task of the run, its acceptance text and the spec paths.

Task state, branches, results and review notes stay with the project after a run ends, so a later run continues the same tasks. With the loft or the external spec store, records live at `.paddock/spec/<feature>/tasks.json` inside that store. With the repo store, they live in the repository’s common git directory at `paddock/spec/<feature>/tasks.json` (usually `.git/paddock/spec/<feature>/tasks.json`), shared by linked worktrees.

### The external spec store

Spec Kit keeps `.specify/` and `specs/` in the repository. The external store is [the loft](#the-loft): with the loft on, the Spec Kit folders go there too, and `[speckit] store` is the short form for the Spec Kit folders alone. With the default store, paddock reminds you at the start of a run that a push publishes committed specs and plans, and adds a note to push approval requests that contain Spec Kit files. Set `store = "repo"` explicitly to keep the files there and silence these notices, or turn on the loft and move them there.

To keep them out of the repository, set:

```toml
[speckit]
store = "external"   # default: "repo", where Spec Kit puts them
```

Then run `paddock spec adopt` once in the repository. It asks, then moves both folders to the loft, `~/.local/share/paddock/loft/<repository key>/tree/`. A store of an earlier paddock moves there by itself. Every sandbox of the repository sees them at `.specify/` and `specs/`, also each `--worktree` sandbox and each worker, with the same content. paddock adds both paths to `.git/info/exclude`, which git does not track, so `git status` does not show them. If the files were committed, adopt tells you how to remove them from the index, and how to remove them from the history. It does not change the history itself.

## Timers

An agent loses its sense of time with its context. After a compaction, or when another agent takes over, it no longer knows how long it has worked on a bug or when the task must end. A paddock timer lives on the host, in paddock kv, so it outlasts every context, and the agent that it times cannot change it.

Inside the sandbox:

```sh
paddock-host timer start fix-login --budget 2h          # this agent owns it
paddock-host timer status                                # after a compaction, before one more try
paddock-host timer extend fix-login --by 30m --reason "the tests need a slow fixture"
paddock-host timer start review --quota weekly=1%       # at most 1% of the weekly limit
paddock-host timer extend review --by 1% --reason "two more files to check"
```

On the host:

```sh
paddock timer set 1a2b3c review --budget 1h30m --hard   # you own it; --hard freezes the sandbox at the end
paddock timer set 1a2b3c tests --quota weekly=1% --hard # a quota budget on the sandbox's account
paddock timer list                                      # the timers of every running sandbox
paddock timer extend 1a2b3c review --by 30m
paddock timer stop 1a2b3c review
```

- Whoever sets a deadline owns it. Only the owner extends or stops it. When an agent asks for more time on a timer that it does not own, you answer on the approval card, which shows its reason.
- A soft deadline gives one notification and marks the sandbox in the bar panel; the agent keeps running. A hard deadline freezes the sandbox (`paddock thaw ID` lets it continue).
- The time that counts leaves out the time when the sandbox was frozen.
- A quota budget (`--quota WINDOW=PERCENT`) limits a task to a share of a limit window of the sandbox's account, counted from the start of the timer: `weekly=1%` means 1 percentage point of the weekly window. Small budgets keep tasks small: work that needs more is better split into several tasks. The window is the name that `paddock forage` shows, or its first word. A timer can have a time budget, a quota budget, or both, and is over when either is used up. `extend --by 1%` adds quota.
- A quota budget counts what the account used, from the forage numbers. paddock cannot tell which sandbox used it, so it splits each rise among the quota timers on the same window that ran meanwhile: four timers at the same time count a quarter each. A timer whose sandbox ended keeps its part of the steps that it ran in. Your own use of the login outside paddock counts too, split the same way. When the account reports whole percents, a budget is over only when the count goes above it, so `weekly=1%` is not over at the first 1% step. The numbers are up to 15 minutes old, so a task can go a little over before paddock sees it. paddock needs usage numbers for the account to start a quota budget.
- Timers in the `pasture` and `tree` scopes are visible to the other agents there, so the agent that hands out work sees how long each part has run.
- `paddock ps --json` and the bar panel show the timer of each sandbox that is closest to its end.
- The keys `timer:*` change only through the timer commands, never with `paddock-host kv put`.

## Forage: how much of the limits is left

An agent that hands out work needs to know which account has room. `paddock forage` and, inside the sandbox, `paddock-host forage` show for each limit window of each account:

- the share used, and when the window resets;
- the rate now, from the last two checks, and the usual rate, from earlier windows;
- when the window runs out at the rate now, and OVERGRAZING when that is before the reset.

```sh
paddock forage                       # every account with numbers
paddock forage codex --refresh       # ask for fresh numbers first
paddock-host forage --refresh        # inside: your harness's account and those you may start
```

The numbers come from the Omarchy agents panel's records. paddock asks for fresh ones with the panel's own command, at most once in 5 minutes per account; a login that a sandbox holds is checked from paddock's side, as for the panel. While a sandbox has a running timer, paddock also asks every 15 minutes, so that the rates are known when someone asks. These checks reach the services as the panel does. paddock keeps the numbers, and nothing else, in `~/.local/state/paddock/forage/`. The bar panel shows a line for each account that runs out before it resets.

The rates are simple: the slope between two checks, and the median of earlier windows. The fewer the checks, the rougher the estimate.

### In the agent's context

An agent that forgot its timer does not ask for it. So for Claude Code, paddock adds three hooks (with `--settings`; Claude Code merges them with your own hooks) that run `paddock-host status`:

- at the start of a session, on resume and after a compaction: every timer and every limit, in a few lines;
- on each prompt: only what needs attention: a timer with less than a fifth of its time left or past its end, and a limit that runs out before it resets or is 90% used;
- after each tool: the same, and the same lines at most once in 10 minutes.

The hooks print nothing when nothing needs attention, and never stop the agent. Other harnesses get the same advice from the paddock-host skill. To turn the hooks off, set this in `~/.config/paddock/config.toml`:

```toml
[status]
hooks = false
```

### After a context compaction

A long run replaces its conversation with a summary, and the agent can forget its role, its task file, or that a detached run ends with `paddock-host done`. paddock adds a hook to a file in the sandbox home (never the host file). After a compaction, the hook runs `paddock-host compacted` and the agent gets a short note: the sandbox, the harness, the role, the parent, the pasture and the task file, with the request to read the task file again.

These harnesses get the note:

| Harness | Hook file in the sandbox home | When the note goes in |
|---|---|---|
| Claude Code | `~/.claude/settings.json` | `SessionStart` with the source `compact` |
| Qwen Code | `~/.qwen/settings.json` | `SessionStart` with the source `compact` |
| Codex | `~/.codex/hooks.json` | `SessionStart` with the source `compact` |
| Grok Build | `~/.grok/hooks/paddock-compaction.json` | `PostCompact` records the compaction, and the next `PostToolUse` adds the note |
| agy | `~/.gemini/config/hooks.json` | `PreInvocation` finds a transcript with a `CHECKPOINT` step and fewer lines than before |

Codex runs a hook only when `config.toml` has a trusted hash for it, and a user approves a hook with `/hooks`. No user is in a sandbox to approve, so paddock writes the hash of its own hook into the sandbox copy of `config.toml`. It trusts a hook of your own only when its program is in `[hooks] programs` (see below). A trust in the host copy of `config.toml` does not count in the sandbox.

Grok Build 1.0.70 runs the hooks of `~/.grok/hooks` only in a trusted folder. When the sandbox copy has paddock's hook or an entry of `[hooks] programs`, paddock trusts the project folder in the sandbox copy of `~/.grok/trusted_folders.toml`. The trust also covers the hooks, MCP servers and instructions of the project itself. It would also turn on your other hooks, so the sandbox copy has none: paddock leaves out the entries of `~/.grok/hooks` that run no listed program, takes the `hooks` table out of the copy of `~/.grok/config.toml`, and turns off the hooks of `~/.claude` and `~/.cursor` there.

agy has no compaction event, and a `CHECKPOINT` step in its transcript is not a sure sign: agy also writes one for the title of a plain run. paddock counts a compaction only when agy wrote the transcript again with fewer lines and a `CHECKPOINT` step. It can miss a compaction, but it does not give the note without one.

paddock also records the compaction. A spawn child writes `compacted:SPAWN-ID` into the `tree` scope of paddock kv, with the time and the count so far, so a parent can watch with `paddock-host kv watch --prefix compacted: --scope tree`. A pasture member writes `compacted:RUN-ID` into `pasture`. The audit log gets one line, and `paddock ps --json` shows `compactions` for each run.

Other harnesses have no hook that runs after a compaction and can add text to the context. They get no note.

```toml
[compaction]
enabled = true        # false: no hook
note = "Run the tests before you commit."   # text added to the note
notify = false        # true: a desktop notification for each compaction
```

### Your own hooks

Your own hook entries come into the sandbox copy of each hook file, after paddock's entry. This works for Claude Code, Codex, Grok Build, agy, Qwen Code and Gemini CLI. paddock checks every command of each entry, also each part of a line such as `[ ! -f PATH ] || PATH`. Each program and path must be in the sandbox: on the sandbox PATH, or at a path that the sandbox can see. Shell builtins count. An entry with a program or path that is not in the sandbox is left out of the sandbox copy, and `paddock run` prints one line that names the program. The host files never change.

To give a hook program to the sandbox, list it in the user configuration:

```toml
[hooks]
programs = ["~/.local/bin/my-hook"]   # bound read-only at /run/paddock/bin/my-hook and at its own path
ignore = ["other-hook"]               # left out without a notice
```

- A listed program must be a regular file, not a symlink. It must belong to you or to root, it must not be writable by group or others, and it must not be inside the project. paddock binds the file only, never its folder.
- The program runs with the sandbox home as `HOME`, so it keeps its state there (for example `~/.local/state/my-hook`), never on the host.
- Codex trusts the entries that run a listed program, and no other entry of yours. Codex does not run an entry that it does not trust, so paddock leaves your other entries out of the Codex copy. The start line names them. Without this, Codex would stop at its "Hooks need review" screen at each start.
- Grok Build gets the same rule: the folder trust that paddock writes would turn on every entry, so paddock leaves out the entries that run no listed program.
- An `ignore` entry is a program name or a path (`~/` is your home). A hook entry that runs it is left out for every harness, with no notice line. Use it for a hook that does a job paddock does already, such as a note after a compaction. `ignore` wins over `programs`.
- Only the user configuration can set `[hooks]`. A project `paddock.toml` with `[hooks]` is an error.

`paddock explain HARNESS` lists each hook entry of the sandbox copy with its origin: paddock, your file, or left out and why.

## Devices: /dev/paddock

A device is a host service that the sandbox uses as a file. Each sandbox gets its own small file system at `/dev/paddock`. The paddock process on the host serves it, so paddock knows which sandbox calls. No root and no credentials enter the sandbox.

One open is one request. The program opens the device for reading and writing, writes the request, and reads the response until the end of the file. The first read sends the request and waits for the answer. Closing the file early cancels the request. Each device has a time limit. After that time, the read fails with `ETIMEDOUT`. `/dev/paddock/.catalog` is read-only and describes each device: its request format, its limits and whether it is on.

```sh
exec 3<>/dev/paddock/echo; printf 'hello' >&3; cat <&3
```

All devices are off by default. Turn a device on in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot turn on a device, because the agent can edit that file. `paddock-host device NAME` in the sandbox sends one request and turns errors into exit codes. See "Wait without sleep" below.

### The built-in devices

| Device | What it does |
| :--- | :--- |
| `echo` | A test device. It sends the request back. |
| `wait` | The read returns at a time. The request is an RFC 3339 time, such as `2026-10-30T15:00:00Z`, or a duration, such as `15m`. At most seven days ahead. |
| `ask` | The agent writes a question. The question appears on the approval card. The read returns your answer. |

```toml
[devices]
wait = true
ask = true
mount = true   # false: no /dev/paddock at all
```

### Your own devices

A device is a program of yours that a sandbox reaches as a file. You declare it once in `~/.config/paddock/config.toml` and say who sees it. A project `paddock.toml` cannot define or turn on a device, because the agent can edit that file.

```toml
[devices.mac]
shape = "exec"                  # exec | serve. kind = "command" of 0.2.1 means shape = "exec"
run = "sandbox"                 # sandbox | host | remote
command = ["~/.config/paddock/devices/mac/build.sh"]
enabled = true                  # off: the file exists and answers "disabled"
grant = ["pasture:workers", "project:~/src/app", "harness:claude"]
mode = "ask"                    # ask (default) | allow: no first-request card
scope = "run"                   # run (default) | shared
start = "first-request"         # first-request (default) | with-sandbox
stream = true                   # the response comes as the program writes it
max_request = "4M"              # 1M by default, 64M at most
max_response = "16M"
timeout = 900                   # seconds per request, 30 by default
concurrency = 1                 # requests at the same time, 1 to 16
cost = 0                        # see "Paid devices"

[devices.mac.sandbox]           # the device sandbox: see below
network = "none"
connect = ["2222:mac.local:22"]
ro = ["~/.ssh/known_hosts_mac"]
credentials = ["mac-ssh"]
caller = ["project:ro"]
```

| Key | Values | What it means |
| :--- | :--- | :--- |
| `shape` | `exec`, `serve` | `exec`: the command runs once per request, the request on its standard input, the response on its standard output. `serve`: the command runs once and serves HTTP/1.1 on the Unix socket `$PADDOCK_DEVICE_SOCKET`. `kind = "http"` is the loopback proxy, as before. |
| `run` | `sandbox`, `host`, `remote` | Where the program runs. `sandbox` (default): a paddock sandbox of its own, with the profile of `[devices.NAME.sandbox]`. `host`: on the host as you, with your home, your keys and your network. `remote`: a device of another paddock (`target`, `device`). |
| `command` | a list | The program and its fixed arguments. The request never adds an argument. A program path that starts with `~/` is your home. |
| `enabled` | `true`, `false` | Off by default. An off device is still a file that answers "disabled". |
| `grant` | `pasture:NAME`, `project:PATH`, `harness:NAME`, `all` | Who sees the device. Without the key, every sandbox sees it. |
| `mode` | `ask`, `allow` | `ask`: the first request of a sandbox puts a card on the host. `allow`: no card, and the audit line names the grant. Refused for `run = "host"`. |
| `scope` | `run`, `shared` | One instance per sandbox, or one per user for all sandboxes. See "Shared devices" below. |
| `start` | `first-request`, `with-sandbox` | When the instance starts. |
| `stream` | `true`, `false` | With `true`, a read returns bytes as the program writes them. See "Streaming" below. |
| `max_request`, `max_response` | a size, 1 byte to 64M | `max_body` of 0.2.1 sets both. |
| `timeout` | 1 to 3600 seconds | Per request. |
| `concurrency` | 1 to 16 | Requests of one instance at the same time. |
| `idle` | 1 to 86400 seconds | A `shared` instance stops after this long without a request and with no run that uses it. 300 by default. |
| `ready` | 1 to 300 seconds | How long the start waits for the health check of a `serve` program. |

Who sees a device, in this order. First `paddock run HARNESS --device NAME`, or `paddock-host spawn ... --device NAME` from a sandbox that has it. Then `[pastures.NAME] devices = ["mac"]` for the members of a pasture. Then `grant` of the table. Last, every sandbox when the table has no `grant` key. A project `paddock.toml` can ask with `[devices] use = ["mac"]`. That widens access, so it needs the one-time approval like other widening fields. It never turns a device on. `.catalog` shows how the sandbox got each device as `grant`.

Every key has a parse error that names the table, the key, the allowed values and the fix. An unknown key is an error, so a typo cannot widen or narrow a device.

`kind = "http"` stays the loopback proxy of 0.2.1. The target must be a loopback address (`127.0.0.0/8` or `::1`). The request is the first line `METHOD PATH`, then header lines, an empty line and the body. paddock checks the method and the path prefix, and refuses paths with `..`, `//` or an encoded dot or slash. It drops `Authorization`, `Cookie` and hop-by-hop headers, and sets `Host`, `Content-Length` and `Connection` itself. A `Paddock-Status` header (`disabled`, `refused`, `unreachable` or `no answer`) means that paddock answered itself.

```toml
[devices.ollama]
kind = "http"
enabled = true
target = "127.0.0.1:11434"   # one loopback address and port
methods = ["GET", "POST"]
paths = ["/api/"]            # path prefixes
max_body = "1M"
timeout = 30                 # seconds, default 30
```

#### The device sandbox

A device program with `run = "sandbox"` runs in a paddock sandbox of its own, without a harness. The baseline is the view of an approved `exec`: the system read-only, an empty home, its own `/tmp`, and no network. Your runtime folders are hidden. The empty home sits at your home path so that `~` in the program's own files resolves. The profile `[devices.NAME.sandbox]` widens it. Nothing else of the host is there. The paddock process of the run starts one instance per sandbox, at the first request or with the sandbox, and ends it with the sandbox.

| Key | Values | What it means |
| :--- | :--- | :--- |
| `network` | `none` (default), `egress`, `api` | As for an agent sandbox. `api` takes `allow = [...]`. |
| `connect` | `["2222:mac.local:22"]` | `127.0.0.1:2222` inside the device sandbox reaches `mac.local:22` from the host. See below. |
| `ro`, `rw` | host paths | Bound at the same path inside, read-only or read-write. `/` and the home are refused. A single file under a protected folder such as `~/.ssh/known_hosts_mac` passes as `ro`; a folder there does not. |
| `home` | `hidden` (default), `read-only`, `full` | Your home in the device sandbox. |
| `env` | a table | Fixed variables for the program. |
| `env_pass` | names | Host variables passed by name, on top of `HOME`, `PATH`, `TMPDIR`, the locale and `TERM=dumb`. |
| `credentials` | `[credentials.NAME]` names | An `agent` credential of kind `ssh` gives a filtering ssh agent at `SSH_AUTH_SOCK` that signs with the named keys only; the private key never enters the sandbox. A `deliver` credential with `as = "file"` gives the value as a FIFO at `/run/paddock/device/secret/NAME`, named by `PADDOCK_SECRET_NAME`. `as = "env"`, `gateway` and `intercept` are refused with the fix. No card: you named the credential on the host. |
| `caller` | `["project:ro"]` | The caller's project, read-only, at `/run/paddock/caller/project`; `PADDOCK_CALLER_MOUNT` names it. See below. |
| `mem`, `cpus` | `"512M"`, `1` | `MemoryMax` and `CPUQuota` of the systemd scope of the instance. |
| `tmp` | `"4G"` | The size of the `/tmp` of the device sandbox. 1G by default. |

`connect` reads as the docker `-p` shape with the sandbox side first: `LOCAL_PORT:HOST:PORT`. The direction is the reverse of docker's. The device sandbox has no route to anything. The supervisor inside listens on `127.0.0.1:LOCAL_PORT` and copies each connection to a socket of the broker. The broker connects to `HOST:PORT` from the host, with the host's resolver, routes, VPN and proxy settings. The host must be another machine in the configuration (a loopback or LAN literal is refused). A name that resolves to a LAN address is still connected, and the audit line says `lan`. Every connection gets one audit line with the device, the caller, the host, the port, the bytes in and out and the duration, never content. At most 32 connections per entry. `paddock device stats NAME` sums them.

`caller = ["project:ro"]` binds the caller's project into the device sandbox, read-only, from the start of the instance. A build device reads what is on disk at the moment of the request. If the agent writes files during the request, the device sees a mixed tree. That is the agent's to manage, as with any build that runs beside an editor. A device that produces files answers with bytes, or writes into a folder that you mount with `rw`. Symbolic links in the project resolve inside the device sandbox, where nothing else of the host exists. `project:rw` is refused. Only `scope = "run"` instances can have `caller`.

The lifecycle. An instance is a child of the paddock process of the run, started by bubblewrap with `--die-with-parent`. When the host has a user manager, it runs in a systemd scope `paddock-dev-<sandbox>-<name>`. It ends with the run, with `paddock stop`, with `paddock device stop NAME --sandbox ID`, and when its paddock process dies, even by `kill -9`. `paddock freeze` of the agent sandbox freezes its device instances too. When the supervisor of an instance dies, the next request starts it again after 1 and then 2 seconds. The third death within a minute makes the device `failed`, and `.catalog` says so with `how_to_fix` naming the log command and `paddock device restart`. Each instance has a state file in `~/.local/state/paddock/devices/instances/NAME/ID.json` (written whole, then renamed). Its log is `~/.local/state/paddock/devices/log/NAME/ID.log`: the supervisor's lines and the program's standard error, capped at 10 MiB with one rotation, never a request or a response. Every start removes the state files and folders of instances whose process is gone. `exec` requests run in a fresh empty folder under the `/tmp` of the device sandbox.

`stop` and `restart` reach the paddock process of the run through its kv socket, so the owner of the instance does the work. `paddock ps --json` lists the instances of a sandbox under `devices`. A `kind = "command"` table of 0.2.1 runs this way too: one device sandbox per run instead of one unit per request. The view is the same. The `[exec]` keys of the user configuration are the base of its profile. Each request gets an empty working folder, and there is no network unless the table says so. The envelope, the card and the audit lines are the same. The visible change is speed.

On macOS, a device sandbox runs under Seatbelt (`sandbox-exec`) with a profile built from the same table. The system is read-only, the home and `TMPDIR` are private and empty, `ro` and `rw` sit at their own paths. Your real home is denied unless `home` says else, and there is no IP network. Seatbelt cannot move a path, so the program sees host paths. `caller = ["project:ro"]` gives the caller's project at its own path, which `PADDOCK_CALLER_MOUNT` names. A secret file is a FIFO in the instance folder, named by `PADDOCK_SECRET_NAME` as on Linux. With `connect`, the supervisor listens on `127.0.0.1:LOCAL_PORT` inside the sandbox and the profile opens that port only; the broker connects from the Mac as on Linux. The ssh agent of a credential is a Unix socket that the sandbox can connect to and nothing more. The instance is a plain child of the paddock process of the run, in a process group of its own. The supervisor ends itself within 2 seconds when that process dies, even by `kill -9`. There is no systemd on macOS: `mem` and `cpus` are accepted and ignored, and `paddock doctor` says so for each table that sets them. `paddock freeze` does not reach a device instance there, and `paddock device ls` keeps showing it as running. `paddock ps` and `paddock device check|logs|stop|restart|stats` work as on Linux, and the state file of an instance says `"backend": "seatbelt"`. The catalog entry of such a device has `"platform": "macos"`: the program builds on the tools of the Mac.

#### A device on the host

`run = "host"` runs the program on the host as you, outside every sandbox: with your home, your keys, your environment and your network. Use it for a tool that needs your session, or that must see your files as you see them. A GUI tool, a daemon of your desktop, or a program that reads your keychain are such tools. For everything else, a device sandbox is the better choice: the program gets only what its profile names.

```toml
[devices.notes]
shape = "exec"
run = "host"
command = ["~/bin/notes-search"]
cwd = "~/notes"                # the working folder. Default: your home
env = { EDITOR = "vi" }        # fixed values on top of your environment
```

The program gets the whole environment that paddock got, so `env_pass` is refused for a host device: it is a key of the sandbox profile. A `[devices.notes.sandbox]` table is refused too. `cwd` must exist when a request comes. If it does not, the request answers `status: error` with `message: cwd ~/notes does not exist`, and the next request works once you made the folder. A command that is not on your PATH answers `message: notes-search is not on the PATH of the host`.

Behind the file is the same supervisor as in a device sandbox, `paddock device-serve`. It starts on the host at the first request of a run and ends with the run. One supervisor per run and device: an `exec` command runs per request in `cwd`, a `serve` program runs once on the socket `dev-NAME.sock` of the run. The envelope, the exit mapping, `timeout`, the kill of the process group, `concurrency` and the log are the ones of the contract below. The log is `~/.local/state/paddock/devices/log/NAME/SANDBOX.log`.

A program that runs on the host is a program that can do what your own shell can do. paddock marks it everywhere, so you cannot miss it:

| Where | The mark |
| :--- | :--- |
| the card | "allow the device `notes` for this sandbox: it runs ON THE HOST as you", with the command and `cwd`. `mode = "allow"` is a parse error for a host device: the card shows every session, and "allow for this session" stays available |
| `.catalog` | `"run": "host"`, and the `request` text begins with "Runs on the host as you, with your home, your keys and your network." |
| `paddock device ls` | `HOST` in the run column, in capitals |
| `paddock doctor` | the heading "devices that run on the host as you", one line per device with its command and folder |
| `paddock ps` | `device notes (host, running 3m, pid 1234)` under the sandbox while the supervisor runs. `--json` has it under `devices` with `run: host` |
| the audit log | `"run": "host"` on every line of the device |

`paddock device check notes` prints the warning line before it starts anything. Then it starts the supervisor on the host, waits for its health, prints the start time and stops it. A child sandbox gets a host device only when its parent has it, as any device, and its card says which sandbox started it.

#### Shared devices: one instance per user

`scope = "shared"` runs one instance of a device for your whole user, for every sandbox that has the grant. Use it for a program that is expensive to start or that holds a warm state. A model server that takes a minute to load, a cache, or a daemon with an index in memory are such programs. With the default `scope = "run"` each sandbox gets its own instance, which is right for a build script and wrong for a model server.

```toml
[devices.model]
shape = "serve"
enabled = true
scope = "shared"
idle = 600                     # stop after this long without a request. 300 by default
command = ["~/bin/model-server"]
grant = ["pasture:workers"]

[devices.model.sandbox]
ro = ["~/models"]
```

The first broker that needs the device starts the instance. Every broker connects to the one socket `$XDG_RUNTIME_DIR/paddock/devices/NAME.sock`. The instance ends when its last user ended and `idle` seconds passed without a request. A broker that crashes never takes it down. Each run holds one open connection as its lease, and the kernel closes that connection when the run ends, however it ends.

Who starts and owns the instance depends on what your session has, and `paddock device show NAME` prints which one it is:

| Your session | Who owns the instance |
| :--- | :--- |
| `paddock device install NAME` was run | systemd owns the socket and starts the instance on the first connection (a `.socket` and a `.service` unit in `~/.config/systemd/user`). On macOS a LaunchAgent with a `Sockets` entry does the same |
| a systemd user manager, without `install` | the first broker runs `systemd-run --user --unit paddock-device-NAME`, so the instance outlives every run |
| neither | the first broker starts the instance as a child of its own paddock process. It ends when that run ends, and paddock logs a notice that names the run and `paddock device install NAME`. The next request of another run starts a new one |

```sh
paddock device install model     # hand the socket to systemd or launchd
paddock device uninstall model   # remove the units again
paddock device stop model        # end the instance now. The next request starts a new one
paddock device restart model     # end it; with a user manager, start one that reads the table as it is now
paddock device logs model        # the log of the shared instance
paddock device check model       # start it if needed, probe it, and report the user count
```

The instance reads your user configuration itself, never a project file, and it serves the table that it read at its start. Every request carries your run's view of that table. So an edit of `~/.config/paddock/config.toml` under a running instance cannot be served half old and half new. The request fails with `EIO`, the catalog says "the device was reconfigured", and `paddock device restart NAME` clears it. Where a user manager owns the instance, `restart` also starts the new one at once. Where the first run owns it, `restart` ends the old one and the next request starts the new one. An instance started by that short-lived command would end with it. `paddock device ls` shows `shared, 3 users`. `paddock ps` shows the instance once, at the top, with its user count and its unit. `paddock doctor` gives each shared device one line with who starts it, whether an instance runs and how many runs use it.

A `paddock freeze` of one sandbox does not reach a shared instance, because it serves other runs. `paddock device stop NAME` ends it for everyone, and the open requests of other runs get a `503`.

One instance serves many sandboxes, so a shared device sees many callers. Each request carries `PADDOCK_CALLER` for the sandbox that sent it, and a program that keeps per-caller state must key that state by it. `caller = ["project:ro"]` is a parse error for a shared device, because a mount of one running sandbox cannot serve the others. `run = "host"` with `scope = "shared"` is a parse error too: a host program shows its card per sandbox. Only your user reaches the socket, which lives in your runtime folder with mode 0700.

#### Remote devices

`run = "remote"` forwards each request to a paddock on another machine, which runs the device there. It runs in its own device sandbox, with its own table, its own card and its own audit log. Use it for a tool that lives on another machine, for example Linux tools for an agent in a Seatbelt sandbox. For a build on a Mac from a Linux agent, the worked example below needs no paddock on the Mac.

On the forwarding side, the table names the other paddock and the device there. No command, no shape, no sandbox table: those belong to the table on the other side.

```toml
[devices.mac-build]
run = "remote"
target = "ssh://mac"           # or "unix:/path/to/devices.sock"
device = "build"               # the device name on the other side
timeout = 600                  # the whole round trip
stream = true                  # ask for the output as it comes
```

On the serving side, the table of the device gets a grant for remote callers. Nothing else grants to a remote caller: not `all`, not a table without a `grant` key.

```toml
[devices.build]
shape = "exec"
command = ["~/bin/build.sh"]
enabled = true
grant = ["remote:linuxbox"]    # or remote:any, or the address, for example remote:192.168.1.5
```

The transport is `ssh HOST paddock device serve-remote`, started on the forwarding host as you, with your ssh configuration and keys, one ssh session per request. ssh runs with `BatchMode=yes`: a login that needs a prompt fails at once with ssh's own text. Use a key that an agent holds, or a `ControlMaster` in your ssh configuration. `paddock` must be on the PATH of a non-interactive shell on the serving machine. The other transport is a Unix socket. `paddock device serve-remote --socket PATH` on the serving side serves many connections on it, in a folder it makes 0700. `target = "unix:PATH"` connects to it. Who can reach the folder can call: that is the whole authentication of a socket. Keep it in a folder of your own, or forward it over ssh yourself.

The identity of a remote caller is its transport, never what it says about itself. For ssh it is the client address that sshd puts into `SSH_CONNECTION`. For a socket it is `local`. `remote:HOST` matches the address, or a name that resolves to it. `remote:any` matches every remote paddock. Each request carries the caller's sandbox id, the name of the forwarding machine and the chain of sandboxes above the caller, as text. The card on the serving side shows it: "allow the device `build` for a remote caller from 192.168.1.5 (sandbox 3f2a on linuxbox)", and says that the text is the other paddock's word. The audit line there has `"sandbox": "remote:192.168.1.5"` and the text in `remote_caller`, `remote_host` and `remote_chain`. The devices of the serving side see the caller as `remote:192.168.1.5`. A remote caller gets no built-in device and no secret file.

"Allow for this session" on the serving side holds for 8 hours per peer and device, in `~/.local/state/paddock/devices/remote-sessions.json`. The ssh transport starts a new `serve-remote` process per request, so the answer must outlive a process. `paddock device serve-remote --forget` forgets every answer. `mode = "allow"` on the serving table skips the card there, as for any device in a device sandbox.

The ssh transport runs a program on the forwarding host as you, so the forward is marked like a host device. The card says "allow the device `mac-build` for this sandbox: it connects to mac with your ssh as you". It has the ssh command in its text, and it shows every session. `mode = "allow"` is a parse error for an ssh target. The `request` text in `.catalog` begins with "Connects to mac with your ssh as you." A socket target is a connection, nothing more: its card is the plain one and takes `mode = "allow"`. `.catalog` has `"run": "remote"`, the `target` and the `device` on the other side. After the first allowed request it also has the other side's own `request` and `response` text, and a `remote` field with its state, shape and platform. A request that does not reach the other side fails with `EIO`, and `last_error` in `.catalog` says why, with ssh's own words when it has them.

The local `timeout` is the whole round trip, and the other side has its own. A cancel (the agent closes the file) closes the transport. The other side cancels as it does for a closed file: the program there is killed. The errors of the other side arrive unchanged: `EINVAL`, `EACCES` when the user there declined, `ETIME` when a device there still waits, `ETIMEDOUT` when its deadline passed. With `stream = true` the forward asks for the output as it comes and each piece reaches the agent as it arrives.

`paddock device check mac-build` reaches the other side as you, reads its catalog and prints the entry of the device there. That is its state, shape, run mode and platform, and its `request` text, or the reason it is off. `paddock device add mac-build --run remote --target ssh://mac --device build` writes the table. `paddock doctor` lists the remote devices with their transport under "devices of another paddock". The VM backend of macOS is the same shape with vsock as the transport, when the devices reach the VM.

#### Streaming

With `stream = true` a read returns bytes as the program writes them, and the end of file comes when the program ends. A build log appears line by line, and a long answer starts at once. A failure after the first byte fails the next read with `EIO`: the agent gets what came, never a silent truncation. Without `stream`, the response comes whole, and a failure is one errno before any byte. Streaming is per device, so an agent reads `stream` in the catalog and knows what to expect.

A stream is read once, in order: the file is not seekable. `max_response` holds for the whole stream, with `EMSGSIZE` at the byte that passes it. paddock keeps about 1 MiB of a stream in memory, whatever its size. A program that writes faster than the agent reads waits. Closing the file ends the program within 300 ms.

`paddock-host device NAME` prints a stream as it comes. It exits with 7 after a late failure, or with 75 when the device says still waiting after some bytes. On macOS the socket carries the same stream in chunks, and an older `paddock-host` gets the response whole.

#### The worked example: build and test on a Mac

Workers in the pasture `workers` build and test their worktree on a Mac at `mac.local`, with the account `me`. The device `mac` runs a script in a device sandbox. The sandbox reaches one host and port, holds the ssh key in a filtering agent, and reads the caller's worktree read-only. The cargo output streams back. One command writes it all:

```sh
paddock device add --example mac --host mac.local --user me
```

It writes these tables into `~/.config/paddock/config.toml`, and `~/.config/paddock/devices/mac/build.sh`. It never writes `[pastures]`: it prints the line to add. The `env` value gets your real home path instead of `~`, because a device sandbox on macOS has a private home.

```toml
[credentials.mac-ssh]
via = "agent"
kind = "ssh"
keys = ["~/.ssh/id_mac.pub"]        # the agent signs with this key only; the key file never enters a sandbox

[devices.mac]
shape = "exec"
run = "sandbox"
command = ["~/.config/paddock/devices/mac/build.sh"]
enabled = true
grant = ["pasture:workers"]
mode = "ask"
stream = true
max_request = "4K"
max_response = "16M"
timeout = 1200
concurrency = 1                     # a second request waits for the first

[devices.mac.sandbox]
network = "none"
connect = ["2222:mac.local:22"]     # 127.0.0.1:2222 inside reaches mac.local:22 from the host
ro = ["~/.ssh/known_hosts_mac", "~/.config/paddock/devices/mac"]
credentials = ["mac-ssh"]
caller = ["project:ro"]
env = { SSH = "ssh -p 2222 -o HostKeyAlias=mac.local -o UserKnownHostsFile=~/.ssh/known_hosts_mac -o StrictHostKeyChecking=yes -o BatchMode=yes", MAC = "me@127.0.0.1" }
mem = "512M"

[pastures.workers]
members = ["codex", "claude"]
devices = ["mac"]
```

`build.sh` reads one line, `test` or `check`, and rejects anything else with exit 65. It copies the worktree with `rsync` to `paddock/<caller>` on the Mac, without `.git`, then runs `cargo test` or `cargo check` there. `HostKeyAlias` makes ssh check the Mac's host key under its name although the connection goes to `127.0.0.1:2222`. `MAC_DIR` in `env` moves the copies to another folder on the Mac.

```sh
#!/bin/sh
set -eu
read -r what || true
case "$what" in
  test|check) ;;
  *) echo "request must be test or check" >&2; exit 65 ;;
esac
dest="${MAC_DIR:-paddock}/$PADDOCK_CALLER"
$SSH "$MAC" "mkdir -p '$dest'"
rsync -a --delete --exclude .git -e "$SSH" "$PADDOCK_CALLER_MOUNT/" "$MAC:$dest/"
exec $SSH "$MAC" "PATH=\"\$HOME/.cargo/bin:\$PATH\"; export PATH; cd '$dest' && cargo $what 2>&1"
```

The Mac side, as the command prints it:

1. On this machine, a key for the Mac in your ssh agent: `ssh-keygen -t ed25519 -f ~/.ssh/id_mac` and `ssh-add ~/.ssh/id_mac`. paddock must start where `SSH_AUTH_SOCK` names that agent.
2. On the Mac, Remote Login on for the account, and the public key in its `authorized_keys`: `ssh-copy-id -i ~/.ssh/id_mac.pub me@mac.local`.
3. On this machine, the host key of the Mac under its name: `ssh-keyscan -t ed25519 mac.local >> ~/.ssh/known_hosts_mac`. The command runs `ssh-keyscan` itself and shows the line, so you can compare it with the Mac's own key.
4. On the Mac, rust. rustup puts cargo in `~/.cargo/bin`, which the script puts on the PATH itself.
5. The line `devices = ["mac"]` in `[pastures.workers]`.
6. `paddock device check mac --send check` from a folder with a `Cargo.toml`: the start time, a health line, and the cargo output.

What happens on `printf test | paddock-host device mac -` in a worker:

1. The worker's broker sees the device from the pasture grant. The first request shows the card: "allow the device `mac` for this sandbox", with the command, the connect target, the credential and the caller mount. You allow it for the session.
2. The broker starts the instance: bubblewrap, no network, the `connect` socket, the filtering ssh agent of `mac-ssh`, the known hosts file, the worktree at `/run/paddock/caller/project`. It waits for the supervisor's health.
3. The supervisor runs `build.sh` with the request on its standard input. `rsync` and `ssh` connect to `127.0.0.1:2222`, the supervisor copies the bytes to the `connect` socket, and the broker connects to `mac.local:22` on the host. The ssh client signs through the agent. The host key is checked under the alias.
4. The output streams back: each read of the worker returns the next bytes of the cargo output. A non-zero exit fails the last read with `EIO`, and `paddock-host device` exits with 7 after it printed the output. A wrong request exits with 8.
5. The audit line: device `mac`, run `sandbox`, the caller, the decision, the exit, the bytes, the duration. One line per ssh connection with the bytes. The device log has the standard error of `build.sh`. No log holds a request or a response.

A second request while one runs waits (`concurrency = 1`). Closing the worker's terminal during a build ends `build.sh`, which is the ssh process, so the session on the Mac ends within 2 seconds. Why `connect` and not `network = "egress"` with the LAN: `egress` would let the device reach every machine of the LAN and every VPN peer. `connect` lets it reach one host and one port, and the broker makes the connection, so the device sandbox has no route at all.

#### The commands

```sh
paddock device ls                          # name, shape, run, scope, state, grants, instances
paddock device show mac                    # the table, the resolved profile, what the sandbox reaches
paddock device add mac --grant pasture:workers --connect 2222:mac.local:22 --exec ./build.sh
paddock device add --example mac --host mac.local --user me   # the worked example above
paddock device example                     # writes upper.py, a device in 20 lines of Python
paddock device check mac                   # start an instance without a run, print the start time, stop it
paddock device check mac --send test       # also send one request from the host and print the response
paddock device logs mac --sandbox ID [-f]  # the log of one instance
paddock device stop mac --sandbox ID       # the next request starts it again
paddock device restart mac --sandbox ID    # clears `failed`
paddock device stop mac --all              # every instance of the device
paddock device stats mac --since 7d        # requests, errors, cancels, p50, p95, connect relays
paddock device install model               # a shared device: hand its socket to systemd or launchd
paddock doctor --escape --device mac       # the escape probe, run as the program of the device sandbox
paddock ps                                 # shows `device mac (sandbox, running 3m, pid N)` under the sandbox
paddock claude --device mac                # the grant of one run
```

`add` writes the table into `~/.config/paddock/config.toml`, refuses a name that exists, and prints what it wrote. Put `--exec` or `--serve` last: the rest of the line is the command. `paddock devices` stays as an alias of `paddock device`. `paddock doctor` prints a line for each table with an old spelling, each host device and each remote device. It also names each device without a grant whose sandbox reaches something, and each `project:` grant whose folder does not exist.

`paddock doctor --escape --device NAME` runs a probe as the program of the named device, in its device sandbox. The probe tries what a device program must not do. It tries to see your home and your runtime folder. It tries to read the private key of a credential, and to see more of `~/.ssh` than the table binds. It tries to reach the host's loopback and the LAN, to write the caller's project, and to find `paddock-host`. The connect ports of the table must answer inside. One line per check, `ok` or `FAIL`, and exit 1 when one failed.

#### Write a device

A device program is a program of yours in any language. It never talks to the agent. paddock (the broker) sits between them. It knows the caller from the mount, asks you once per sandbox, and keeps the limits, the budget and the audit log. It sends the request to your program over one small contract. The contract has two shapes. One script can do both.

`shape = "exec"`: paddock runs the command once per request.

| Channel | Content |
| :--- | :--- |
| arguments | the `command` list of the table, nothing else, ever |
| standard input | the request bytes |
| standard output | the response bytes |
| standard error | the device log. The agent sees at most the first five lines |
| exit 0 | the response is the standard output |
| exit 65 | the request was rejected: the agent gets `EINVAL` |
| exit 75 | still waiting: the agent gets `ETIME` and sends the same request again |
| exit 77 | the program declined: the agent gets `EACCES` |
| other exit | `status: exit N` in the response, the read succeeds |
| signal, timeout | `status: signal N` or `status: timeout` |

Without `stream`, the response of an `exec` device is the envelope of 0.2.1: a `status:` line, an optional `message:` line, up to five `stderr:` lines, an empty line, then the standard output. With `stream = true` the response is the raw standard output, and a non-zero exit fails the last read with `EIO`.

The command runs in a fresh empty working folder, with these variables. paddock sets them, the caller cannot.

| Variable | Value |
| :--- | :--- |
| `PADDOCK_DEVICE` | the device name |
| `PADDOCK_DEVICE_PROTOCOL` | `1` |
| `PADDOCK_CALLER` | the sandbox id of the caller |
| `PADDOCK_CALLER_HARNESS` | its harness |
| `PADDOCK_CALLER_PROJECT` | the host path of its project |
| `PADDOCK_CALLER_MOUNT` | where the caller's project is mounted in the device sandbox, with `caller = ["project:ro"]`, else unset |
| `PADDOCK_CALLER_CHAIN` | the sandboxes above the caller, root first, comma separated |
| `PADDOCK_REQUEST` | a request id, unique per instance |
| `PADDOCK_DEADLINE` | seconds left before paddock fails the request |
| `PADDOCK_MAX_RESPONSE` | bytes |
| `HOME`, `PATH`, `TMPDIR`, the locale, `TERM=dumb` | the same set that `[exec]` gives |

`shape = "serve"`: paddock starts the command once per instance. The program listens on the Unix socket in `$PADDOCK_DEVICE_SOCKET` and answers HTTP/1.1. One request of an agent is one `POST`:

```
POST /v1/call HTTP/1.1
Host: paddock
Paddock-Protocol: 1
Paddock-Device: mac
Paddock-Caller: codex-app-1a2b3c
Paddock-Caller-Harness: codex
Paddock-Caller-Project: /home/demo/src/app/.worktrees/wt-1a2b3c
Paddock-Caller-Mount: /run/paddock/caller/project
Paddock-Request: 17
Paddock-Deadline: 580
Paddock-Max-Response: 1048576
Content-Length: 42

<42 bytes>
```

A request of a `scope = "shared"` device carries one header more, `Paddock-Table`, which is the calling run's view of the device table. The shared instance checks it and answers `503` when it differs (see "Shared devices"). A device program ignores it: it never needs to read it.

| Response | What the agent gets |
| :--- | :--- |
| `200` with a body | the response bytes. `Content-Length` or `Transfer-Encoding: chunked`. Chunked is how a program streams. |
| `204` | an empty response |
| `202` | still waiting: `ETIME`, the same request comes again |
| `400` | rejected: `EINVAL` |
| `403` | declined: `EACCES` |
| `413` | the request is too large for the program: `EFBIG` |
| `429` | busy: `EBUSY` |
| `5xx`, a closed connection, a response that is not HTTP | `EIO`. The audit line keeps the status. |
| the header `Paddock-Cost: 0.002` | the cost of this request, in the unit of your budgets. It counts before `cost_header`. |

paddock closes the connection to cancel. A program that works for a long time checks for the close. paddock reads at most `max_response` bytes, fails the read with `EMSGSIZE` beyond them and closes.

Two more paths, both optional. `GET /v1/health` answers `200` when the program is ready. paddock waits for it after the start (`ready`, 10 seconds by default) and before the next request after a failure. A `serve` program must answer it, else the start fails and `.catalog` names the log. `GET /v1/catalog` answers a JSON object with `request`, `response`, `examples` (a list of short strings) and `cost`. paddock merges `request`, `response` and `examples` into the catalog entry at the start of the instance. The table wins for the name, the limits, the cost and the budget.

The protocol is numbered. A program that needs a newer one answers `GET /v1/health` with `Paddock-Protocol-Min: 2`, and paddock refuses the instance with "the device program needs protocol 2; this paddock speaks 1". A new header or a new status code is additive and stays in protocol 1.

When the program exits, every open request fails with `EIO` and paddock restarts it after 1, 2 and then 4 seconds. After three exits within a minute the device is `failed`: `.catalog` says so with `how_to_fix`, and every request fails with `EIO` until the sandbox restarts. A request beyond `concurrency` waits up to `timeout`, then fails with `EBUSY`.

Rules for a device author. Do not print request bytes to standard error: standard error is the device log, which stays on disk. Read the whole request before you answer. Close the connection after each response. `paddock device example` writes this script, which does both shapes in 20 lines of Python:

```python
#!/usr/bin/env python3
# A device that upper-cases its request. exec: stdin to stdout.
# serve: an HTTP server on $PADDOCK_DEVICE_SOCKET.
import os, sys, socketserver
from http.server import BaseHTTPRequestHandler
sock = os.environ.get("PADDOCK_DEVICE_SOCKET")
if not sock:
    sys.stdout.buffer.write(sys.stdin.buffer.read().upper()); sys.exit(0)
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass  # a Unix socket has no client address
    def do_GET(self):
        self.send_response(200); self.end_headers()
    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(n).upper()
        self.send_response(200); self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)
class S(socketserver.UnixStreamServer): pass
S(sock, H).serve_forever()
```

`paddock device add upper --exec ./upper.py` or `--serve ./upper.py`. No SDK and no library of paddock's. The `log_message` line matters: without it, Python's HTTP handler tries to log the client address of the Unix socket and raises.

Behind the socket is `paddock device-serve`, the supervisor. It is the paddock binary itself. For `exec` it forks the command per request. A thread feeds the request on standard input, so a program that does not read cannot block. It caps the output at `max_response`, kills the process group at `timeout` and when the agent closes the file, and removes the working folder. For `serve` it starts the program once and proxies each request byte for byte. Both sides enforce the limits. The supervisor stops at `max_response` and at `timeout`, and the broker does the same, so a bug in one cannot flood or hang the other. The log of an instance has one line per request: the id, the caller, the status, the bytes, the duration. It also has the program's standard error, with the request id as a prefix. It never has a request or a response. It is capped at 10 MiB and rotated once.

#### Paid devices: budgets and the ledger

Some services cost money per request, for example a paid search API behind an `http` device. paddock never spends money without a budget.

```toml
[devices.search]
kind = "http"
enabled = true
target = "127.0.0.1:8090"
cost = 0.002            # per request, in a currency unit you pick
cost_header = "X-Cost"  # http only: the service reports the cost of each response
budget = 1.0            # per run
```

- A device with `cost` or `cost_header` is paid. paddock does not convert currencies. With `cost_header`, a request costs the larger of `cost` and the header value. paddock ignores a header that is not a number.
- A request that paddock could not send (the service did not accept the connection) costs nothing. A request that the service got costs at least `cost`, also when the service does not answer in `timeout` seconds or closes the connection early (`Paddock-Status: no answer`).
- A paid device needs a budget before its first request. Without `budget` in its table, the first-use card asks for one: 0.10, 1 or 5, or deny. No budget, no request.
- The budget is per run: a sandbox and the sandboxes it started. A spawned child spends the budget of the root sandbox, never a budget of its own.
- A request that would pass the budget fails with `EDQUOT` (`paddock-host device` says "budget used" and exits with 11). You get one notification with the command to raise it: `paddock device budget ID DEVICE --add 1.0`. The next request can pass.
- The ledger `~/.local/state/paddock/devices/ledger.db` records the time, the sandbox, the project, the device, the cost and the status of each request. Never a body. `paddock device ledger [--json] [--project PATH] [--since 7d]` sums it by sandbox and device.
- `.catalog` shows the cost of each device and the budget left for this sandbox. The bar panel row shows the spent amount, for example "spent 0.42 of 1.00 on search".

#### Migration from 0.2.1

A configuration of 0.2.1 parses without change. `paddock doctor` prints one line per table that uses an old spelling, with the new one, and never refuses it.

| 0.2.1 | Now | What you see |
| :--- | :--- | :--- |
| `kind = "http"` | the same | nothing changes |
| `kind = "command"` | `shape = "exec"`, `run = "sandbox"`; `network = true` is `network = "egress"` of the profile; the `[exec]` keys stay the base | nothing changes; `doctor` shows the new spelling |
| `max_body` | sets `max_request` and `max_response` | nothing changes |
| `isolation` (never released) | `run = "host"` | a parse error that names the new key |
| `paddock devices budget`, `ledger` | `paddock device budget`, `ledger`; `devices` stays | nothing changes |
| `.catalog` version 1 | version 2: `shape`, `run`, `stream`, `grant`, `instance` per device | nothing for a reader that ignores unknown fields |

```toml
[devices.jq]
kind = "command"
enabled = true
command = ["jq", "-c", "."]  # fixed arguments
max_body = "256K"
timeout = 30
network = false              # default false
```

#### The limits

- Each device is the file `/dev/paddock/NAME`, and `.catalog` shows its format, its limits and whether it is on.
- A device is off until its table says `enabled = true`. An off device answers with a `disabled` status, and one that no grant covers says which grant is missing.
- The first request of each sandbox puts a card on the host: "allow the device `ollama` for this sandbox". "Allow for this session" lets the later requests of this sandbox through without a card.
- A request over `max_request` fails with `EFBIG`, a response over `max_response` with `EMSGSIZE`. A declined device fails with `EACCES`.
- The audit log records the device, where it runs, the grant, the method and path (`http`) or the byte counts (`exec`), the status and the duration. It never records a body.
- The mount needs FUSE: `/dev/fuse` and `fusermount3` (package `fuse3`). `paddock doctor` checks both. The bwrap backend binds the mount at the start. docker and podman bind an empty folder and paddock mounts the file system on it after the container starts. The container waits up to five seconds for it before the harness starts. This needs a shared mount for the paddock run folder, which is the systemd default. srt cannot place a mount at `/dev/paddock`: there, the devices are at the path in `$PADDOCK_DEVICES`. When FUSE is missing, the sandbox starts without devices, and `paddock doctor` says why.
- Programs must read the response with `read`. `cat` from coreutils, `dd`, shells, Python and Rust do that. busybox `cat` uses `sendfile` and sees an empty file: use `dd` there. Requests and responses never go into logs or process arguments.
- The device sandbox and the agent sandbox share the kernel, as every bubblewrap sandbox does. The VM backend is the answer on macOS.
- A device with `rw` on a folder you also edit has no lock. The card names the folder.

<a id="devices-on-macos"></a>**On macOS**, there is no `/dev/paddock`: Seatbelt cannot show a mount, and FUSE on macOS needs a kernel extension, which paddock does not ask for. A Seatbelt run (terminal or app) gets the same devices over a Unix socket of its own, which `PADDOCK_DEVICES_SOCKET` names. `paddock-host device NAME`, `device list` and the exit codes are the same as on Linux. A shell redirection such as `exec 3<>/dev/paddock/echo` does not work: use `paddock-host device`. The secret files of `as = "file"` credentials are FIFOs in the folder that `$PADDOCK_DEVICES` names: `cat "$PADDOCK_DEVICES/secret/NAME"` reads the value, and each open counts as one use. The value is never on disk. After `paddock credentials revoke`, the FIFO is gone within 50 ms and a read fails with "No such file or directory". `paddock-host device secret/NAME` prints the value too. The question of `ask` comes as a plain notification on macOS: answer it with `paddock approvals answer ID --text -`. A device with `run = "sandbox"` runs under Seatbelt too, with host paths in place of the mounts of bubblewrap (see "The device sandbox"). A device with `scope = "shared"` has a LaunchAgent in place of the systemd units. `paddock device install NAME` writes `~/Library/LaunchAgents/com.harnessix.paddock.device.NAME.plist` with a `Sockets` entry and loads it. launchd owns the socket and keeps it when the instance exits on idle, and `paddock device uninstall NAME` unloads the agent and deletes the file. Without `install`, macOS has no `systemd-run`, so the first run owns the instance and paddock logs the notice (see "Shared devices"). The socket of a shared device is at `~/.local/state/paddock/devices/NAME.sock` there, not in a runtime folder, because launchd makes it at login and that folder exists then. The VM backend does not get the devices yet. `paddock doctor` shows a line for each. A Unix socket path on macOS has at most 103 bytes, and the default `TMPDIR` under `/var/folders` is long. So paddock keeps the sockets of a run in `/tmp/paddock-<uid>/<hash of the run folder>/<id>`, mode 0700, with a hash in place of an id that is too long. macOS starts each process with a limit of 256 open files, so paddock raises its own limit to the hard limit, at most 10240. The worked example works from a Mac too: the device then reaches `mac.local:22` of the same Mac, and the audit line says `lan`.

### Wait without sleep

Agents often wait: for CI, for a child agent, for your answer. Many agents wait with `sleep 60` and then check again. Each check costs a model turn, and a result that arrives between two checks waits for the next one.

paddock gives agents blocking reads instead. The read waits until the event happens and returns at once. While it waits, the agent uses no CPU and no tokens.

| Wait for | Command in the sandbox |
| :--- | :--- |
| A key in paddock kv | `paddock-host kv watch KEY --timeout 100`, or `--prefix PREFIX` |
| A child agent | `paddock-host wait ID --timeout 100` |
| A time | `paddock-host device wait 2026-10-30T15:00:00Z --timeout 100` |
| Your answer | `paddock-host device ask "Deploy to staging?" --choice yes --choice no` |

A tool call of a harness has a time limit, often two minutes. So each read has a longest wait, `--timeout`. When that time passes before the event, the command exits with 75, "still waiting", and the agent runs the same read again. Exit code 75 means the same for every one of these commands. A plain read of a waiting device fails with `ETIME` ("Timer expired") instead. The `paddock-host` skill tells agents to loop on the read and never to use `sleep`.

A prefix watch also wakes when a key under the prefix expires. So `watch --prefix lease:` returns when a lease runs out and the task is free.

A question from `ask` shows on the approval card in the bar, with the choices as buttons. Click the text field to type another answer, and press Enter. Escape gives the keyboard back. "Dismiss" ends the question, and the agent gets `EACCES` (exit code 3). Without the bar, a desktop notification shows the question and its choices. On the command line, use `paddock approvals` and `paddock approvals answer ID --text -`, then type the answer and press Ctrl-D: read from standard input, it never shows in the process list or your shell history. Your answer goes to the agent only. The audit log records the sizes of the question and the answer, never the text. A question stays open while the agent reads it again; paddock removes it two minutes after the last read, or when the sandbox ends. At most 8 questions of all sandboxes wait at once, host requests come first on the card, and after you dismiss a question, that sandbox cannot ask again for 30 seconds. Control characters and invisible format characters are removed from questions before they are shown.


### Freeze a sandbox

A frozen sandbox uses no CPU and keeps its memory, the harness too. Thawed, it continues where it stopped, without a new start and without losing the agent's context.

```sh
paddock freeze <id> --reason "wait for CI"   # --tree: also the sandboxes it started
paddock thaw <id>
paddock ps                                    # shows "frozen 12m"
```

With a systemd user instance, every bwrap or srt sandbox runs in a scope of its own, `paddock-sb-<id>`, and `paddock freeze` freezes that scope with the cgroup freezer. paddock never freezes a scope that is not its own, such as the one of your terminal. docker and podman sandboxes are paused with `pause` instead. A sandbox that started without a systemd user instance cannot be frozen. The bar panel has a key for it on each row, and shows how long a sandbox has been frozen.

macOS has no cgroups: a Seatbelt run starts in a process group of its own, and `paddock freeze` sends SIGSTOP to that group and to every process under the sandbox process, again and again until it finds no new one, so a process that forks during the freeze cannot get away. `paddock thaw` sends SIGCONT the same way. A process that left both the group and the tree (with `setsid` after its parent ended) is not frozen, and a thaw also continues a process that the agent had stopped itself. The device instances of the run are not frozen with it: `paddock device ls` keeps showing them as running. When the run has your terminal, its group gets the terminal while it runs, and paddock takes it back at the end.

- A session of the macOS guest (`paddock app run` on macOS) is frozen with the pause of its VM: the id is `macvm-<app>`. See [The marks and freeze](#the-marks-and-freeze).
- `paddock stop` thaws a frozen sandbox first, so that it can end.
- Each freeze and thaw goes to the audit log, with the reason.
- The other members of its pasture see the state in paddock kv: `sandbox:<id>` in the `pasture` scope is `{"state": "suspended", ...}` while it is frozen and `running` after.
- The clock keeps running while a sandbox is frozen: a harness timeout or a model's prompt cache can run out.
- Leases in paddock kv follow `[suspend] leases` in the user configuration. With `keep` (the default), the leases of a frozen sandbox stop counting down and run again after the thaw with the time they had left; another agent that claims one gets a conflict with `owner_state: "suspended"`. If the sandbox ends while frozen, its leases count down again. This holds also when its paddock process was killed or the computer restarted: the next claim of such a lease finds no running holder, and the leases of that holder count down again. With `release`, paddock frees them when it freezes the sandbox, with one audit line per lease. The rule holds for every freeze: `paddock freeze`, a hard timer, and the gateway.

## App mode

Desktop agent apps run in a sandbox too: ChatGPT with Codex, Claude Desktop, VS Code, and flatpak apps. They get the same file, git, secrets, environment and network rules as a terminal harness, and their windows are normal windows on your desktop. Flatpak apps have some limits, described at the end of this section.

<p align="center">
  <img src="docs/assets/app-demo.gif" alt="A recording: a host terminal shows an SSH key and a .env with a token, then runs paddock app run vscode; in VS Code's own terminal, the SSH key does not exist, .env is 0 bytes, and an edit to the project works" width="100%">
</p>

```sh
paddock app list                          # the apps paddock knows, and their launcher entries
paddock app run claude-desktop ~/src/api  # run an app with one or more workspace folders
paddock app install chatgpt-desktop ~/src/web   # add a launcher entry that runs it in paddock
paddock app uninstall chatgpt-desktop
```

The app sees only the folders that you give it, from the command line or from `[apps.NAME] roots` in `~/.config/paddock/config.toml`. Its windows connect to your compositor through a restricted socket: the app cannot capture the screen, read the clipboard in the background, type into other windows or list them. Flatpak apps use flatpak's own Wayland socket instead. Add `--gpu` for hardware rendering.

The app gets a session bus of its own, inside the sandbox. It cannot reach your session bus, your keyring or the system services. Three services run on its bus:

- A private keyring (gnome-keyring). The app stores its login and other secrets there, encrypted with a password that paddock keeps on the host. Claude, ChatGPT and VS Code use it. You log in once inside the sandbox, and the login stays for later runs.
- Links. When the app opens a web link, paddock asks you, the same way as for `paddock-host open`, and then opens the link in your browser. The app's request for a link through the desktop portal works the same way.
- Notifications. They appear on your desktop, with the app's name.

The app gets no file chooser portal. It uses its own file dialog, which shows only the files in the sandbox. If an app needs the real desktop portals, give it the filtered host bus in `~/.config/paddock/config.toml`:

```toml
[apps.NAME]
bus = "host-filtered"
```

On that bus the app reaches only the desktop portals, notifications and the tray. It gets no keyring, so it keeps its login in a plain file in the sandbox home. Flatpak apps always use flatpak's own filtered host bus.

### One worktree per app

Two agents in one checkout switch branches and overwrite each other's changes. With `--worktree`, the app gets its own git worktree of the first workspace folder, on a new branch `paddock/<app>-<date>-<id>`. paddock creates it in `~/.local/share/paddock/worktrees`, the same way as `paddock <harness> --worktree`. Your checkout keeps its branch and its changes.

```sh
paddock app run vscode --worktree ~/src/api
```

To use a worktree on every start, set `worktree = true` in `[apps.NAME]` of `~/.config/paddock/config.toml`. When you start an app from the menu without saved folders, the folder dialog asks whether to open a git repository in a new worktree. "New worktree" is the default button.

When the app exits, paddock prints the commits and the uncommitted changes in the worktree, and the commands to review, merge or remove it. If no terminal shows that text, for example after a start from the menu, a notification gives the branch and the number of commits and changes, with a Merge and a Remove button where the notification server supports buttons. If the app made no commit and left no change, paddock removes the worktree and the branch at once. A worktree with work stays until you remove it, or until its work is merged (see [Automatic cleanup](#automatic-cleanup)).

Flatpak apps use worktrees too. paddock gives the app the worktree folder and the repository's `.git` folder read-write, so git in the app can commit there. The `.git` file of the worktree, the `config`, `config.worktree`, `commondir`, `gitdir` and `hooks` of the repository and of the worktree stay read-only.

Later, the commands and the "Worktrees" row of the bar panel do the same:

```sh
paddock worktree list                       # size, last use, branch, merged state, uncommitted changes, in use
paddock worktree merge paddock/vscode-20261102-1a2b
paddock worktree remove ~/.local/share/paddock/worktrees/api/wt-api-1a2b3c [--force]
```

- `list` shows each worktree that paddock made, for apps and for `--worktree` runs. `--json` gives data.
- `merge` merges the branch into the current branch of the repository. It uses paddock's hardened host git, so no hook and no program from the repository configuration runs. It refuses when the repository has uncommitted changes. On a conflict it aborts the merge, and the repository stays as it was. Uncommitted changes in the worktree are not part of the merge.
- `remove` removes the worktree and deletes its branch. It refuses while a sandbox works in it. Without `--force`, it refuses when the branch has commits that no other branch has, or when the worktree has uncommitted changes.
- To keep a worktree, do nothing.

The panel row lists the worktrees with Merge, Remove (press twice) and Open folder buttons.

A flatpak app becomes an app profile with `[app] flatpak = "ID"`. paddock drops every permission that the app asks for, also those from `flatpak override`, and adds back only the workspace folders, the Wayland socket, the portals and, with `--gpu`, the GPU. Many flatpak apps ask for your whole home directory and the host bus, so their own sandbox is not enough. The network is `egress` or `none`, and the host passthrough (`paddock-host`) is not available.

The workspace folders of a flatpak app get the same protection as for other apps. paddock refuses the home directory and protected paths such as `~/.ssh`. In each folder, `.git` is a mount point of its own, its `config`, `config.worktree`, `commondir`, `gitdir`, `objects/info/alternates` and `hooks` are read-only, and the folders in `secrets.hide` are hidden. The host environment passes through the same filter. flatpak can hide a folder, but not a single file. So when the secrets detector finds a file such as `.env` or `id_ed25519` in a folder, paddock does not start the app, and names the files. Move them out, or put them in a folder and add that folder to `[secrets] hide`. To start the app anyway, with these files visible to it:

```toml
[apps.NAME]
unprotected_workspaces = true
```

This works only in `~/.config/paddock/config.toml`, never in a project's `paddock.toml`. paddock then prints a warning with the files on every start, `paddock app list` shows the setting, and the bar panel marks the running app.

Windows of app sandboxes get a colored border, so you can tell a protected app at a glance. "Protected windows" below describes the border. App sandboxes appear in the bar panel with the other sandboxes. Enter on one focuses its window.

### Protected windows

On Omarchy (Hyprland), each window that shows a sandbox gets a border, always: an app sandbox window, the display of window mode, and the terminal window of a terminal run. A terminal window counts when its terminal emulator holds the terminal of the run. Some terminal emulators draw several windows from one process, for example Ghostty, `foot --server` and kitty with a single instance. paddock cannot tell which of those windows holds the run, so none of them gets a border. An app window counts only when its process runs in the sandbox. A run in tmux, a detached run and a spawned child have no window of their own and get no border. On macOS the rule for terminal windows is different: see [App mode on macOS](#app-mode-on-macos).

- Sage green: the sandbox reaches the network only through paddock's proxy, or not at all (network modes `none`, `api` and `egress`).
- Amber: the sandbox has a wider network (`host`, and `allowlist` on srt).

paddock sets the border on each window at run time, and your Hyprland configuration does not change. After a run ends, its terminal window gets its own border back. On a light Omarchy theme, both colors take a deeper tone.

When a protected window gets focus, a small badge shows on its top edge for a few seconds, for example `paddock · dev · egress · 2h`: the pasture or the harness, the network mode, and the run time. The badge stays while the pointer is on it. A click opens that sandbox in the bar panel. The badge never takes the keyboard, and only the badge itself takes the pointer.

The bar widget shows a shield with the number of protected windows. The "Protected windows" row of the panel lists them, and a click on one focuses it. Bind a key to `omarchy-shell harnessix.paddock windows` to open the list directly.

```sh
paddock windows                  # the protected windows
paddock windows --json           # with level, badge text and geometry
paddock windows --focus 0x55d0   # focus one protected window
```

Known limits:

- Desktop apps log in again inside the sandbox, because your keyring is out of reach. The login then stays in the private keyring of the sandbox. An app with `bus = "host-filtered"` has no keyring. Claude, for example, then warns that no system keyring is available and stores its secrets without encryption, in the sandbox home, which only your user can read.
- The app cannot open files with your desktop apps or pick files outside the sandbox. Its file dialog shows only the sandbox.
- A new window that gets focus can read the clipboard once. This is how Wayland clipboards work, and Flatpak has the same gap.
- Tray icons may not appear.
- Apps cannot update themselves inside the sandbox. Update them on the host, with your package manager or the app's own installer outside paddock.
- To add a folder, restart the app. A running sandbox cannot get new mounts.
- Custom links (`claude://`, `codex://`, `vscode://`) reach the app in its sandbox after `paddock app install`, which makes paddock their handler. `paddock app uninstall` gives them back to the old handler. A login page that the app opens through paddock and that returns to `http://127.0.0.1:PORT` reaches the app: paddock opens that one port on 127.0.0.1 of your machine for ten minutes, while the app listens on it. If an app opens its login page in another way, set `[apps.NAME] login_ports = true` in `~/.config/paddock/config.toml`. Then all the ports the app listens on are open on 127.0.0.1 of your machine while it listens, for any local program.

### App mode on macOS in a guest VM

<a id="app-mode-in-the-macos-guest"></a>On macOS, `paddock app run NAME` runs a desktop app in a macOS guest of paddock's own, in Apple's Virtualization framework. Each window of the app in the guest is a window on your desktop, with its own Dock icon `ChatGPT (paddock)`. This is the default route for every app profile once the golden image is sealed. Without the golden image, `paddock app run` stops with one line that names `paddock macvm setup`. `--backend seatbelt` keeps the older route of [App mode on macOS under Seatbelt](#app-mode-on-macos) for the apps that ran there. `paddock app run` never downloads anything.

The guest is a real macOS. So an app that uses Apple's App Sandbox, such as ChatGPT, runs unchanged under Apple's own sandbox in the guest. The app sees only the workspace folders that you name, never your home. All its traffic goes through paddock's proxy, and paddock asks you about each new host in network mode `ask`.

#### The first run

The example app is ChatGPT, with ChatGPT.app in `/Applications`.

1. Run `paddock app run chatgpt-desktop`. Without the golden image, paddock prints the size and the time of the setup, and the command to run: `paddock macvm setup`.
2. Run `paddock macvm setup`. It shows the URL, the macOS version, the sizes and the time, and asks `Download and install? [y/N]`. Then it prints one progress line per 5 percent, each with the time left.
3. Watch the setup window. On a Mac with macOS 27 or later and a guest of macOS 27 or later, the setup needs no clicks and no typing. Before the first boot, paddock writes the guest agent and a first-boot service into the guest disk, as your user and without administrator rights. The provisioning options of macOS 27 give the guest the account `paddock`, so the guest skips Setup Assistant. The first-boot service installs the agent and removes itself. After the agent reports, paddock shuts the guest down, turns on Accessibility for `paddock-guest` in the permission database of the stopped guest, makes `~/Downloads` of the guest a link to the downloads share, and boots the guest again. So the guest boots twice. The panel shows `paddock sets up the guest, there is nothing to do in this window`. Click Stop to do the steps yourself.
   On an older Mac or with an older guest, follow the guided window, step by step. The agent and the Accessibility permission still need no step of yours when paddock can write the guest disk: you make the account only. Choose the language and the region. Keep the keyboard English (US) or ABC, which is the default, because the Type buttons of the window send the keys of a US keyboard. Skip every sign-in, analytics and Siri step, because paddock needs no Apple ID. Create the account `paddock` with the password that the window shows. The guest has no clipboard before its agent runs, so do not paste: click into the Password field of the guest, then click Type the password into the guest. Do the same for Verify. Then open Terminal in the guest, click into it, click Type the command into the guest and press Return. The command is `sudo /bin/sh "/Volumes/My Shared Files/paddock-seed/install.sh"`. When sudo asks for the password, click Type the password into the guest again and press Return. The command installs the guest agent from a read-only folder of your Mac and starts it. paddock needs no administrator rights on your Mac for it, and a second run of the command is safe. Turn on `paddock-guest` in System Settings, Privacy & Security, Accessibility of the guest.
4. Wait for the line `golden ready`. paddock configures and seals the image by itself in about 2 minutes.
5. Run `paddock app run chatgpt-desktop` again. The start lines name the backend, the memory, the display, the shares, the downloads folder, the network mode and the time of the start. The first start of a profile is a cold boot of about 40 s. The copy of the app into the guest adds about 20 s.
6. Log in to ChatGPT. The hosts of a ChatGPT login are in the profile, so a plain login asks nothing. If the app opens its sign-in page in a browser, the page opens in the browser of your Mac, and the login comes back to the app in the guest. See [Links and logins](#links-and-logins-in-the-guest).
7. When you are done, quit the app. The quit saves the state of the guest, and the next start is warm, in about 6 s.

The password has 20 lowercase letters without `a`, `m`, `q`, `w`, `y` and `z`. These letters are on the same key on US, UK, German, French and Chinese (Pinyin or ABC) keyboards, so the password types the same with any of these guest keyboards, also by hand. The command contains `/`, `.`, `"` and spaces, which are on other keys on a French keyboard, so Type the command needs English (US) or ABC in the guest. The guided window opens inside the visible part of your screen, never under the Dock or the menu bar, and you can resize it. The guest view keeps its aspect ratio, and the panel scrolls when its text is longer than the window.

If paddock cannot write the agent into the guest disk, the disk stays as it was, and the window shows the install command to type in Terminal of the guest. The terminal says why. If the agent does not start within 5 minutes of the boot, the window shows the same command, and the terminal names the first-boot log. If paddock cannot set the Accessibility permission in the stopped guest, the window asks you to turn on `paddock-guest` in System Settings, Privacy & Security, Accessibility of the guest. `paddock macvm status` names the step that failed until the setup goes on. paddock never reads the guest screen or clicks in the guest. `PADDOCK_MACVM_GUIDED=1 paddock macvm setup` leaves every step to you from the start. The guest reads the provisioning options only on its first boot: a golden that booted once without them continues with Setup Assistant in the window.

The setup needs about 32 GB free for the golden image, plus the size of the restore image (about 15 GB) before the download. If the volume has less, the setup stops before the download or the install, and it names the space that it needs and the space that is free.

Each configure step runs in the guest with a limit of 30 seconds, and the terminal shows one line per step with its exit code and its time. If a step fails, the line says which step, its output, and what its check printed. A step that waits for a permission prompt in the guest fails at its limit and does not stop the setup. No step asks for a permission in the guest. A command in the running guest that replaces `~/Downloads` waits on a Files and Folders prompt, so paddock makes that link in the stopped guest, with the owner of the account `paddock`. The `downloads-link` step only reads the link. If the link fails, the terminal says why, `paddock macvm status` names it, and the `downloads-link` step fails. A Downloads folder with files in it stays as it is: move the files out in the guest, then run `paddock macvm setup` again. The golden has no black desktop picture: on macOS 26 and later the picture is in a private store of the wallpaper agent that no public API writes, and the guest desktop never shows. Time Machine stays as macOS sets it, because `tmutil disable` needs Full Disk Access and a golden has no backup disk.

At the end of a session paddock shuts the guest down with `sudo shutdown -h now` in the guest, with the guest password on stdin and a limit of 15 seconds. No Automation permission is needed. If the agent refuses or the shutdown fails, the VM stops at once, and the event log of the session says why.

A session reads the guest password from the keyring of `paddock macvm setup` (`~/.local/state/paddock/keyring/macvm/guest-password`) and never makes a new one. The shutdown at the quit and the first preparation of the data disk need it. If the keyring has no password, `paddock app run` prints one warning, the data disk is not prepared, and the quit stops the VM without a shutdown, so the stamp says that the session did not end clean. `paddock macvm setup` asks for the password of the guest account and keeps it in the keyring.

At the first start of a profile, the guest agent moves the home of the guest account onto the data disk (`data.prepare`), before the app starts. The agent copies the home part by part. `du` measures the home first, and the copy gets 60 seconds plus 1 second per 50 MB. macOS guards some folders of the home with its privacy prompts: `Desktop`, `Documents`, `Downloads`, and in `Library` the data of other apps (`Containers`, `Group Containers`) and the data vaults such as `Mail` and `Safari`. Each of these folders gets 20 seconds. If it does not end in that time, it stays as an empty folder, and the event log names it. A file that the agent cannot read is skipped and counted, and the copy goes on. A symbolic link stays a link. The agent makes the `Downloads` link to the host share again on the data disk. The host waits up to 120 seconds for the answer, and longer while the agent reports progress (`copying the home: 3 of 40 parts` in the event log). The agent answers the quit while it copies. A quit during the copy stops the copy, shuts the guest down cleanly, saves no state, and the next cold start prepares the disk again. If a step fails or the agent does not answer, the event log and the `data disk` row of `paddock app status` name the step, the app starts with the home on the system disk, the quit saves no state, and the next start tries again. If that quit finds too little space for a saved state, it also prints the `not enough space for the saved state` line. A warm start never moves the home under the running app: it skips the preparation, and the quit saves no state, so the next start boots cold and prepares the disk. After a session that did not end clean, the agent verifies the volume, and the event log has the result.

After the home moves, the login keychain of the guest is locked, because macOS keeps the keychain of the old path open. An app then asks for the password of the "login" keychain. So before the app starts, at every start, warm starts included, the agent unlocks the login keychain with the guest password through the Security framework (`SecKeychainUnlock`). The password never goes on a command line. The agent also makes the keychain in the moved home the first entry of the search list and the default keychain. The event log gets a `keychain` line with the result. If the unlock fails, the session prints one line, and you type the guest password into the prompt of the app.

The guest screen can lock during a session, although the golden image turns off the screen saver, the sleep and the screen lock. The cause is not known yet. The agent reports the lock state in each `health` and in a `screen.lock` event the moment that the state changes. On a lock, the event log gets a `screen` line with the time, the seconds since the last input in the guest and the uptime of the guest. Then the session types the guest password and Return through the keyboard of the VM, and waits 10 seconds for a report that says unlocked. It types only when the newest report says locked and is at most 5 seconds old, and it checks that state again before each key. After 3 tries for one lock, the session prints one line and stops. Then you click into the app window and type the password yourself. The typing uses the US key layout, the same as the Type button of `paddock macvm setup`, so a password with a character that has no US key is not typed. The `health` of the session record has the field `locked`. paddock has not confirmed on a Mac yet: the lock report, the unlock by typing, and the keychain unlock after the home moves and after a warm start. `packaging/macos/checks/macvm-session.sh` checks them (`lock`, `keychain`).

If you set another password in Setup Assistant, or typed it wrong twice in the same way, the configure step does not start over. The terminal and the window say that the guest refused the password. Type the password that you set into the field of the window and click Use this password. paddock keeps it in its keyring in place of its own and runs the configure step again. After 5 refused passwords the step fails, and `paddock macvm setup` asks again.

You click on the host only in the setup. paddock asks for no host permission, also not Accessibility or Screen Recording.

#### The commands

```sh
paddock macvm setup                 # the golden image, once: download, install, first boot, trust, configure
paddock macvm status                # golden version, profiles, sizes, slots, running sessions
paddock macvm update | rollback | remove [--profiles]
paddock macvm bench CASE --json     # one case of the bench, for packaging/macos/bench.sh
paddock app run chatgpt-desktop ~/src/web   # the guest route on macOS
paddock app install chatgpt-desktop         # the Dock launcher in ~/Applications
paddock app status NAME [--json]            # one screen about the session
paddock app stop | files | rules NAME
paddock app log NAME [--events | --shim | --guest] [--follow]
paddock app reset NAME [--data | --all]     # the reset tiers below
paddock app backup NAME [--out FILE] [--stop]
paddock app restore NAME FILE|TIME
paddock app backups NAME
paddock app install --drop                  # Paddock Apps.app, the drag target
```

`paddock app files NAME` opens the downloads folder of the profile in the Finder. `paddock app rules NAME` shows the firewall rules of the app. A bundle that you drop on `Paddock Apps.app` becomes an app profile with its own launcher `<Name> (paddock).app`, and it starts.

#### The marks and freeze

Every window of a session carries the paddock marks, as every app sandbox does: the frame, the badge and an entry in the shield menu of the menu bar. The frame is sage, because every connection of the guest goes through paddock's proxy, in each network mode. Network mode `host` is refused with one line, because the guest has no host network. The badge has the word `vm`, for example `paddock · chatgpt-desktop · ask · vm · 2h`. While the input source of the guest differs from the one that the host shows, the badge names it: `paddock · chatgpt-desktop · ask · vm · Pinyin - Simplified · 2h`. A note of the session, such as `waking`, comes at the end. The menu entry names the app, for example `ChatGPT (paddock) · web · ask · vm · 2h`.

`paddock freeze macvm-chatgpt-desktop` pauses the VM: the guest uses no CPU and keeps its memory, and the badge says `frozen: paddock thaw resumes it`. The windows keep the last picture. `paddock thaw macvm-chatgpt-desktop` resumes the VM, and the guest goes on where it stopped. A second freeze and a thaw of a running session do nothing. The audit log gets each freeze and thaw, and the members of a pasture see the state in paddock kv, as for every sandbox. A quit of a frozen session thaws it first: `paddock app stop`, Cmd-Q and the Dock menu. Then the app can save its work, and paddock can save the state. The paused time does not count as a silence of the guest.

#### When something fails

Your data is on the data disk of the profile, which holds the guest home with the app, its login and its documents. No failure below removes it.

- If the app crashes in the guest, its windows close and the session stays. The Dock menu offers `Open ChatGPT again`. After three crashes in a minute, paddock stops opening it and prints the path of the crash report.
- If the guest hangs, the badge says `guest not answering` after 10 s. After 30 s the Dock menu offers `Restart guest`. After 120 s paddock restarts the guest by itself, with the same shares, and the windows come back.
- If the session process crashes, the VM stops with it. The next `paddock app run` or `paddock app list` cleans up, and the next start is a cold boot with the line `the last session ended without a save`.
- If the terminal closes or paddock is updated during a session, the session goes on. It depends on no other paddock process after the start.
- If the Mac sleeps, the VM sleeps with it. At the wake the badge says `waking` until the guest answers, at most 10 s. The app reconnects as any laptop app does, and a firewall question from before the sleep is asked again.
- If the disk gets full, the badge warns at 2 GiB free. At 1 GiB free the guest shows an alert, and the next quit skips the save. A start that needs more space than the disk has stops before it spends any, with the sizes.
- If macOS on the Mac is updated, the saved state of the guest no longer fits. The next start says so and boots cold, in about 40 s.
- If a start finds a profile that a crash left half written, paddock rebuilds the part that is missing and keeps your data.

#### Reset and backups

`paddock app reset NAME` has three tiers. Each one prints what it removes and what it keeps, and asks first. A reset refuses while the session runs.

| Command | Removes | Keeps | Backup first |
| --- | --- | --- | --- |
| `paddock app reset NAME` | the saved state, the app copy of the profile, the drops and the logs. The next start is a cold boot and copies the app again | the system disk and the data disk, `downloads/`, `backups/`, the firewall rules | no |
| `paddock app reset NAME --data` | the system disk and the data disk, the saved state, the stamp, the app copy, the drops and the logs. The next start is a fresh profile from the current golden image | `downloads/`, `backups/`, the firewall rules | yes, `backups/pre-reset-<time>/` |
| `paddock app reset NAME --all` | the whole profile and its firewall rules, moved to `macvm/trash/` for 7 days | nothing | the trash is the backup |

The system disk and the data disk of a profile are a pair. The system disk holds the account of the guest, the path of its home on the data disk and the daemon that mounts that disk. A data disk that the guest formatted itself (a profile made before this version) is also encrypted with keys that its own system disk holds. So the pair stays together for the life of the profile. A new macOS in the golden image reaches an existing profile only through `paddock app reset NAME --data`.

`paddock app backup NAME` makes a backup of the pair in `backups/<time>/` of the profile, in seconds, because the copy shares the blocks of the disk. `--out FILE` writes it elsewhere. A backup refuses while the session runs, because the guest has writes that are not on the disk yet. `--stop` quits the session first. `paddock app restore NAME FILE|TIME` puts a backup back, and the pair of before goes to `backups/pre-restore-<time>/`. `paddock app backups NAME` lists the backups with their sizes. paddock also makes a backup by itself before `--data`, `--all` and a repair. It keeps the newest 3 automatic backups for 30 days at most, and `paddock gc` removes the rest. A backup holds the login keychain of the guest, so its files have mode 0600 in a folder of mode 0700.

#### Status and logs

`paddock app status NAME` shows one screen about a profile. It has the session with its pid, its age and its kind of start, the golden image and the guest agent. It has the app with its version and the last health of the guest. It has the sizes on disk, the network mode with the allowed and denied connections, the shares and the logs folder. `--json` gives the same as JSON. A stopped profile shows how the last session ended and how the next one starts. `paddock macvm status` lists every profile in one line each, with the golden image and the two VM slots.

The logs of a profile are in `logs/` of its folder:

| Log | What | Size |
| --- | --- | --- |
| `shim.log` | the log of the session process | 5 files of 1 MiB |
| `events.jsonl` | one JSON line per change of the session, for example the start, a lost health, a wake, a freeze, the quit and each error | 2 MiB |
| `console.log` | the serial console of the guest: the start line of the guest agent and the `sw_vers` lines at each boot, because macOS writes nothing there on a normal boot | 5 files of 1 MiB |
| `agent.log` | the log of the guest agent, fetched at the quit | 5 files of 1 MiB |

`paddock app log NAME` prints the connection log, `--events` the event log, `--shim` the session log and `--guest` the agent log. `--follow` follows. No log holds the session token, the guest password, clipboard content, file content or the saved state. `paddock doctor` on macOS has a `macvm` block, and `paddock doctor --report` adds the logs of each profile, with the secrets removed.

#### Keyboard and input methods

The session windows send key codes to the guest, not characters, and the guest turns them into text with its own input source. So paddock keeps the input source of the guest equal to the one of the host. At the start the guest gets the enabled input sources of the host. While a session window is key, each change on the host goes to the guest. A layout that the guest lacks, such as one of a third-party input method, shows in the badge.

An input method that composes text, such as Pinyin, Japanese, Korean or Vietnamese Telex, must compose in the guest. While a session window is key, the host gets the plain layout of the input method, for example `ABC` for Pinyin. The guest gets the input method itself. The candidate window of the guest shows next to the text cursor. When the window loses the focus, the host gets its input method back. The Globe key switches the input source of the guest while a session window is key.

#### Screens, Spaces and full screen

The guest has one display, as large as all your screens together, at the scale of the main screen. With two screens of two scales, windows on the other screen are soft, and the start lines say so. The display keeps its size until the next start. If a screen goes away during a session, the windows on it move to the main screen, and the badge says `display changed, restart for full size`.

A session window moves to another Space as any window does. But all windows of the guest are on its one display. If you put two windows that overlap on two Spaces, the one of the other Space can show through. paddock raises the windows of the active Space in the guest, to keep this rare. When an app window goes to full screen in the guest, its session window goes to full screen on your Mac and shows the whole guest display. When the guest window leaves full screen, the session window comes back to the guest window's frame. A window that the guest puts on a window server level outside the levels of app windows (below the desktop, or at the levels of a drag image and the screen saver) is still refused, and the badge says `full screen is not available in the guest`.

#### VoiceOver

In the first version, VoiceOver read only the title of a session window and `ChatGPT (paddock)`. paddock now mirrors the Accessibility tree of the key window of the guest, four levels deep, with the roles, the titles and the values. A press or a new value goes to the guest. Keyboard navigation with Tab, the arrow keys and Full Keyboard Access works, because the guest handles it. Reduce Motion and Increase Contrast of the Mac go to the guest. The Dynamic Type of the Mac has no counterpart in the guest.

#### Clipboard and drag and drop

While a session window is key, the clipboard goes both ways, up to 16 MiB per copy. The types are plain text, RTF, HTML, PNG, TIFF and file URLs. Items that password managers mark as concealed or transient never cross. A file URL from the guest must point into a share, or it does not cross. A file from the Mac crosses as a drop. You can drag files, text and images into a session window, and paddock puts them into the guest. A drag out of a session window to the Mac is not possible. Use the downloads folder (`paddock app files NAME`) or the clipboard instead.

#### Limits

What a session costs, and where paddock tells you before it spends the time or the disk:

| What | Value | Where paddock says it |
| --- | --- | --- |
| Sessions at once | 2 (Apple's limit on macOS VMs) | the third `paddock app run` suspends the least recently used session, or stops and names the two with `[macvm] auto_suspend = false` |
| RAM per session | 4 GiB, `vm_memory = "8G"` in `[apps.NAME]` for editors, at most half of the Mac's memory | the start lines, `paddock macvm status` |
| CPUs per session | 4 | the start lines |
| Golden image on disk | 20 to 25 GB after the seal, plus the 15 GB IPSW until it is removed | `paddock macvm setup` before the download, `paddock macvm status` |
| Profile on disk | 1 to 5 GB of system writes, a data disk up to 20 GB, a saved state of the RAM size | `paddock app list`, `paddock macvm status` |
| First golden build | 10 to 20 minutes of download, 10 to 20 minutes of install, 3 minutes of first boots and configure | `paddock macvm setup` before it starts |
| First start of a profile | 25 to 40 s cold boot, plus 20 to 40 s for the copy of the app | the start lines |
| Later start | 3 to 6 s restore | the start lines |
| Quit | 2 to 8 s for the save | the quit lines |
| Input to screen | one display frame, 16 ms at 60 Hz, plus the guest's own time | this README |

These are not there, and paddock refuses each one with one line where you can ask for it:

- A Mac without Apple silicon.
- A host before macOS 14, because the save of the state and the keyboard of the guest need macOS 14.
- More than two sessions, and guest memory above half of the Mac's memory.
- The network mode `host`.
- A drag out of the guest.
- Apple ID and App Store apps.
- `paddock-host` inside a desktop app.

Two screens of two scales work, with soft windows on the other screen.

#### Performance

The targets of the route are in `packaging/macos/bench/targets.json`, and `packaging/macos/bench.sh` measures them on a Mac with `paddock macvm bench CASE --json`. The baseline is the last committed run in `packaging/macos/bench/baseline.json`. A Linux test checks that the baseline meets the targets, so a change of a target and of the baseline is one commit. The baseline below holds the targets themselves until the first measured run.

| Measure | Target | Baseline |
| --- | --- | --- |
| `warm` | p50 6 s, p95 10 s | p50 6 s, p95 10 s |
| `cold` | p95 45 s | p95 45 s |
| `first` | p95 120 s | p95 120 s |
| `quit` | p95 10 s | p95 10 s |
| `update` | p95 5 s | p95 5 s |
| `input` | p95 40 ms | p95 40 ms |
| `move` | p95 8 ms | p95 8 ms |
| `frames` | p50 17 ms, p95 50 ms | p50 17 ms, p95 50 ms |
| `idle.cpu` | mean 3 % | mean 3 % |
| `idle.ram` | p95 4 GiB | p95 4 GiB |
| `idle.load` | mean 1 | mean 1 |
| `video` | mean 60 % | mean 60 % |
| `disk` | p95 6 GiB | p95 6 GiB |

`warm`, `cold` and `first` are the times from `paddock app run` to the first window, `first` with the copy of a 500 MB app. `quit` is Cmd-Q to the end of the session process at 4 GiB, and `update` is the self-update of the guest agent. `input` is the time from a key on the Mac to the change of a text field in the guest. `move` is the time from a window move in the guest to the host window. `frames` is the gap between two frames of a guest animation (17 ms is 60 fps). `idle.cpu` and `video` are the CPU of the session in percent of one core, at idle and with a 1080p video. `idle.ram` is its memory at idle. `disk` is the profile after a start and a quit, without the app.

#### Security

What the guest can reach on the Mac, and nothing else:

| Path | What | Guard |
| --- | --- | --- |
| vsock 1030 | the control line of its own session | a token after the hello, one connection, JSON lines of a limited length |
| vsock 1031 | paddock's proxy of its own session | the token, the network mode of the session, the firewall rules |
| vsock 1032 | the clipboard and the drops | the token, 16 MiB per transfer, 64 transfers per minute |
| vsock 1033 | one forwarded login callback | the token and a one-time ticket that the host made for a connection to a callback port of a link that it opened |
| the `ws-*` shares | the workspace folders that you name | never your home, `/` or a protected folder |
| the `drop` and `downloads` shares | two folders of the profile | inside the paddock data folder |
| the `apps` and `agent` shares | the app copy and the guest agent | read-only, and the agent checks the hash before it replaces itself |
| the VM slots | two lock files in `/private/tmp` | the text of a holder is shown and never trusted |
| the network device | ARP, DHCP, DNS refused, ping of the gateway | every other frame is dropped and logged |
| the windows | pixels out, keys and the mouse in | only for the window that has the focus |
| the clipboard | both ways | only while a session window is key |

The guest cannot reach your home, your keychain, your screen or the clipboard while you work elsewhere. It cannot reach any socket of paddock on the Mac, the network of the Mac or other profiles. It cannot reach the golden image, which is read-only and only cloned, or the Linux VM of `paddock vm`. `paddock-host` is not in the guest, so no host request comes from a desktop app. The 21 items of the checklist each have a test or a Mac script (`packaging/macos/checks/security.sh`).

#### Not confirmed on a Mac yet

These checks have not run on a Mac. The scripts under `packaging/macos/checks/` check each one on a Mac.

- paddock has not confirmed the build, clippy and tests of the bench on macOS (`macvm-bench.sh build`) on a Mac yet.
- paddock has not confirmed the gates of `paddock macvm bench`, which refuse a session that started without `--bench` (`macvm-bench.sh gates`) on a Mac yet.
- paddock has not confirmed the numbers of the bench: the baseline above holds the targets until `packaging/macos/bench.sh --baseline` writes a measured run (`macvm-bench.sh bench`) on a Mac yet.
- paddock has not confirmed the JSON report of a bench run (`macvm-bench.sh json`) on a Mac yet.
- paddock has not confirmed the MISS line of a target that a bench run misses (`macvm-bench.sh env`) on a Mac yet.
- paddock has not confirmed that a key reaches the guest app on a Mac yet.
- paddock has not confirmed the offline seed: the files on the guest disk as your user, their root owner, `fsck_apfs -n` and the agent start by the first-boot service (`macvm-offline.sh seed`) on a Mac yet.
- paddock has not confirmed the offline trust: the Accessibility row in the permission database of the stopped guest and the report of the agent after the next boot (`macvm-offline.sh trust`) on a Mac yet.
- paddock has not confirmed that the Accessibility permission stays after a self-update of the agent (`macvm-offline.sh update`) on a Mac yet.
- paddock has not confirmed that every configure step ends with `ok` within its limit on a macOS 27 guest (`macvm-first-run.sh` unattended) on a Mac yet.
- paddock has not confirmed the offline `~/Downloads` link: a link to the downloads share with the owner 501:20 in the stopped guest (`macvm-offline.sh downloads`) on a Mac yet.
- paddock has not confirmed that the end of a session shuts the guest down with `sudo shutdown` and no permission prompt (`macvm-offline.sh shutdown`) on a Mac yet.
- paddock has not confirmed that the unattended setup needs no click of the person before `golden ready` (`macvm-first-run.sh` clicks) on a Mac yet.
- paddock has not confirmed the seed of the guided step: the shared seed folder, the install command in Terminal of the guest and the agent start without a new login (`macvm-golden.sh` seed, `macvm-first-run.sh` clicks) on a Mac yet.
- paddock has not confirmed that the Type buttons of the guided window reach the account fields of Setup Assistant and Terminal of the guest (`macvm-first-run.sh` typing) on a Mac yet.
- paddock has not confirmed that the guided window fits a 13-inch screen with the Dock visible and that a resize keeps the aspect of the guest view (`macvm-first-run.sh` window) on a Mac yet.
- paddock has not confirmed that a refused password shows the question of the panel and that the configure step passes with the typed one (`macvm-first-run.sh` password) on a Mac yet.
- paddock has not confirmed the window list, the titles and the moves through Accessibility in the guest on a Mac yet.
- paddock has not confirmed the guest layout of the host key codes and Pinyin in the guest on a Mac yet.
- paddock has not confirmed the marks on two windows of one session (`macvm-marks.sh marks`) on a Mac yet.
- paddock has not confirmed that `paddock freeze` stops the guest and `paddock thaw` lets it go on (`macvm-marks.sh freeze`) on a Mac yet.
- paddock has not confirmed that a window on a screen that goes away comes to the main screen (`macvm-marks.sh screen`) on a Mac yet.
- paddock has not confirmed that a quit of a frozen session saves it (`macvm-marks.sh quit-frozen`) on a Mac yet.
- paddock has not confirmed that the guest window follows a host drag within 50 ms at the 95th percentile, with no frame more than 2 points apart after the move (`macvm-session.sh drag`) on a Mac yet.
- paddock has not confirmed that the clip of a session window has the corner radius that the agent measured, within 1 point, for TextEdit and a toolbar window (`macvm-session.sh radius`) on a Mac yet.
- paddock has not confirmed that a guest window in full screen puts its session window in full screen with the whole guest display, and back (`macvm-session.sh fullscreen`) on a Mac yet.
- paddock has not confirmed that a guest link opens through the link helper and that a login callback on the host loopback reaches a guest server, and only that port (`macvm-session.sh login`) on a Mac yet.

#### Links and logins in the guest

<a id="links-and-logins-in-the-guest"></a>A link that an app in the guest opens goes to the browser of your Mac. The default browser of the guest account is `Paddock Links`, a small helper of the guest agent. The setup of the golden image writes this choice into the guest disk while the guest is stopped, so macOS in the guest never asks to change the default browser. The helper hands each link to the agent, and the agent sends it to the session on the Mac.

The session uses the rules of the host requests of a terminal run. Only http and https links pass. The mode of `open-url` in `[host]` of `~/.config/paddock/config.toml` decides: `ask` (the default) shows the approval card with the full link, `allow` opens it, and `deny` keeps it closed. The card offers "allow for this session" and "deny for this session". The event log of the profile names each link by its scheme, its host and its length, never by its path or its query.

Many apps log in with OAuth: they listen on a port of the guest loopback and wait for the browser to come back to `http://localhost:PORT/...`. When the session opens a link whose callback address (`redirect_uri` and the like, also in a nested link) is a loopback port, it listens on `127.0.0.1:PORT` and `[::1]:PORT` of your Mac for 10 minutes. The session forwards each connection on that port to the same port of the guest loopback, through vsock 1033, and only while a program in the guest listens there. The port closes 10 seconds after the first callback that the guest answered, so the page that the app redirects to still loads. No other port of the guest is reachable from the Mac, and nothing listens on an address of your Mac that is not a loopback address. When a program of your Mac holds the port, the event log says so, and the login cannot finish in the guest.

The proxy settings of the guest send `localhost`, `127.0.0.1` and `::1` around the proxy, and nothing else: the bypass list of the system proxy and `NO_PROXY` of the app hold exactly these three. A golden image of an earlier paddock has no helper as its default browser and an empty bypass list. Run `paddock macvm update --force` to build a new golden image of the same macOS; until then, links open in Safari of the guest, as before.

#### Details of the route

`paddock app run NAME --backend macvm` boots a macOS guest of paddock's own in the Virtualization framework, one profile per app under `~/.local/share/paddock/macvm/profiles.noindex/NAME/default/`. On macOS `paddock app run NAME` picks this route for every app profile once the golden image is sealed, and otherwise stops with the line that names `paddock macvm setup`; `--backend seatbelt` keeps the older Seatbelt route for the apps that ran there. It needs the sealed golden image of `paddock macvm setup` and refuses with that line without it. The profile holds a clone of the golden (`system.img`, APFS clone, no space), the data disk `data.img` (the guest home, 20 GiB sparse, mode 0600) and the saved machine state `state.vzs` of the last quit. The data disk survives every failure and every repair: only `paddock app reset NAME --data` and `--all` remove it. paddock formats a new data disk on your Mac before the first boot, as a plain user (`hdiutil attach -nomount`, `diskutil partitionDisk ... GPT APFS Data`, `hdiutil detach`), so the guest never shows the dialog `The disk you attached was not readable by this computer`. The guest agent then moves the home onto the empty volume. If the format fails, one warning names the reason, the disk stays blank, and the agent formats it in the guest as before. If a disk dialog shows anyway (an older profile with a blank disk), the agent presses Ignore through Accessibility (never Eject or Initialize), and `events.jsonl` gets a `dialog` line. If the agent cannot press it, the dialog shows on your desktop as a window of the app `system`, and the session prints a line that names it. A volume that paddock formats on your Mac is not encrypted by the guest, so its data at rest has the same protection as the system disk next to it: the mode 0600 of `data.img`, and FileVault when it is on for your Mac. The system disk stays with it for the life of the profile, because the data disk mounts only under the system disk that formatted it; a repair keeps both, and a profile whose system disk is gone is refused with the line that names `paddock app restore` and `--data`, with nothing changed. `paddock app backup NAME` clones the pair (system and data disk, shared blocks, seconds) into `backups/<time>/`, `paddock app restore NAME FOLDER|TIME` puts one back (the pair of before goes to `backups/pre-restore-<time>/`), `paddock app backups NAME` lists them, and a backup holds the login keychain, so it has mode 0600 in a 0700 folder. `paddock app reset NAME` removes the saved state, the app clone, the drops and the logs; `--data` removes the pair after a `pre-reset` backup; `--all` moves the whole profile to `macvm/trash/` for 7 days. `paddock gc` removes the trash after 7 days and the automatic backups beyond the newest 3 or older than 30 days; manual backups stay. During a session the shim watches the guest: no `health` for 10 s sets the badge `guest not answering` and pings, 30 s offers `Restart guest` in the Dock menu, 120 s restarts the guest alone with the same shares (the data disk replays its journal). Three unanswered requests in a row count as an agent crash. An app that exits three times in a minute is not opened again, and the line names the crash report. At a wake of the Mac the badge says `waking` until the first pong, the idle tunnels close and the open firewall questions are asked again. With 2 GiB free the badge warns; with 1 GiB the guest shows an alert and the next quit skips the save. paddock has not confirmed on a Mac yet: the repair, the backup and the restore of a pair, the reset tiers, the guest restart, the crash counter, the wake and the disk watch; `packaging/macos/checks/macvm-recovery.sh` checks them. A start is `warm` (the saved state resumes), `cold` (a boot), `repair` (a half profile after a crash: paddock rebuilds the system part and keeps your data) or `fresh`. Before a clone, the volume needs 8 GiB free. `paddock app stop NAME`, SIGTERM and Ctrl-C ask the app to quit (when the app does not run in the guest, the quit goes on at once), save the state when `[macvm] save` is on (the default) and the volume has the RAM size plus 1 GiB free, then stop the VM. A second signal stops the VM at once without a save. The end of the terminal does not end the session: the shim ignores SIGHUP, and `paddock app list` shows it. Two macOS VMs run per Mac (a limit of the framework): a third start is refused with the holders named, before any clone. A second start of the same profile is refused with the pid of the first. paddock has not confirmed on a Mac yet: the boot of a profile from the golden, the warm start, the two-session limit, the format of a new data disk as a plain user, the copy of a fresh home with its time limit and the guarded folders, the press of Ignore on a disk dialog, the quit of a session whose app does not run, the agent log after each quit, the input source fallback, and that no paddock process leaves zombie processes; `packaging/macos/checks/macvm-session.sh` checks them (`format`, `datadisk`, `dialog`, `noapp`, `input`, `zombies`). The logs of a session are in `profiles.noindex/NAME/default/logs/`: `shim.log`, `events.jsonl` (one JSON line per state change, 2 MiB cap) and `console.log` (the serial console of the guest). The network of the guest: the guest has one network device, and its host end is a stub inside the session helper that answers only ARP, one DHCP lease, DNS with `REFUSED` and `ping` of the gateway `10.0.2.2`. Every other frame is dropped and counted, one line per minute per target in `logs/boundary.jsonl` of the profile, so an app that ignores the proxy shows up there with the host it tried. The only way out is the proxy of the guest agent on `127.0.0.1:3128`, which is the system proxy of the guest and the `HTTPS_PROXY` of the app. Each connection goes over vsock to paddock's proxy inside the session helper, with the guest process in front (pid, bundle id, team id, path, and the app above a helper), and the network mode of the run decides as for every other backend. The identity comes from the guest and can be forged by a root process there, so paddock uses it for rules and the log, never as the only reason to allow a host. paddock has not confirmed on a Mac yet: the network service name of the guest, the proxy through the guest agent, the identity of a connecting process, and the tunnel close at wake; `packaging/macos/checks/macvm-network.sh` checks them.

Suspend and instances. Apple runs at most two macOS VMs per Mac, so paddock holds two slots. If both slots are in use and you start a third session, paddock saves the least recently used session first: the one whose windows were not active for the longest time. The save takes about the RAM size in disk space and a few seconds, and it frees the slot within 15 s. paddock never suspends a session with an open dialog of its app (a save dialog, for example), a session that copies its app into the guest, or a session whose guest prepares its data disk: it takes the next one, or it refuses the start with one line that names each session and the reason. The suspended session keeps its icon in the Dock. A click on the icon, `Resume` in its Dock menu, or `paddock app run NAME` resumes it from the saved state in about 4 s, and suspends another session if it needs the slot. `paddock app list` and `paddock app status NAME --json` show the state `suspended`, and `events.jsonl` of the profile has a `suspend` line and a `resume` line, each with its reason. `paddock app stop NAME` ends a suspended session at once: its state is on disk already. To turn automatic suspend off, set `auto_suspend = false` in `[macvm]` of `~/.config/paddock/config.toml`. Then a third start is refused, as before.

`paddock app run NAME --instance N` runs instance N of an app: another profile with its own system and data disk, login keychain and firewall rules, cloned from the golden like the first. `--new` picks the lowest free number. Instance 1 is the folder `profiles.noindex/NAME/default/`, and instance N is `profiles.noindex/NAME/instance-N/`. The Dock names the instance (`ChatGPT 2 (paddock)`). `paddock app stop`, `status`, `files`, `reset`, `backup`, `restore` and `backups` take `--instance N` too. The firewall rules of instance N are in `~/.config/paddock/firewall/NAME--N.toml`, and `paddock app rules NAME--N` edits them.

`paddock du` lists the macvm folder after the Linux sections: the golden image, the restore images, the trash, and each profile instance with its disks, saved state, app copies and backups, and whether it runs, is suspended or is stopped. `paddock du --json` has the same data under `macvm`. `paddock gc --yes` removes the app archives of stopped profiles (the next copy of a new version makes them again) and the saved states of profiles that were not used for `--older-than` days (their next start is a cold boot). A profile is a user asset, like a home: `paddock gc` lists a profile that was not used for `--older-than` days, but removes it only with `--homes --include-app-logins`, because the app's login in it will be lost. paddock never removes anything of a running or suspended instance. When a start finds less than 8 GiB free on the volume, it runs the automatic tier of `paddock gc` on the macvm folder first. paddock has not confirmed on a Mac yet: the suspend of the least recently used session, the warm resume from the Dock, two instances at once, and the macvm items of `paddock du` and `paddock gc`; `packaging/macos/checks/macvm-session.sh` checks them (`suspend`, `instances`, `gcdata`).

The guest sees the workspace folders that you name, as `ws-1`, `ws-2` and so on under `/Volumes/My Shared Files` (virtiofs), read-write, or read-only with `--ro PATH`. It never gets `/`, your home folder or a parent of it, `/Users`, `/Volumes`, `/private`, `/tmp`, `/var`, `/etc`, `/Library`, `/System`, `/Applications`, `~/.ssh`, `~/Library` or the keyring folders: the start refuses with one line per folder. paddock checks the folder as you named it and its real path, and shares the real path. So a symlink in your home that points into `/etc` or `/tmp` is refused too. A folder inside your home is never a system folder, even when the home itself is under `/private/var`. The secrets detector runs over each folder, and a `.env`, key or token file stops the start with the file named, unless `[apps.NAME] unprotected_workspaces = true` in `~/.config/paddock/config.toml`; virtiofs shares a whole folder, so paddock cannot hide a single file inside one. A file, text or image dropped on a session window is copied (an APFS clone) into `drop/<time>-<n>/` of the profile, never the original, at most 2 GiB per drop, and opened in the guest app ("dropped as open"); the staging folders go at quit unless the app still has a file open. A drag out of a session window to the host is not possible: use the downloads folder or the clipboard. The clipboard moves both ways only while a session window is key on the host, with plain text, RTF, HTML, PNG, TIFF and file URLs (into a share), up to 16 MiB per transfer; an HTML item without plain text gets the text of the HTML as a second type, a TIFF image without PNG gets a PNG as a second type, and an image over 16 MiB crosses as its PNG when that fits, or not at all (the badge says `image too large for the clipboard (max 16 MiB)`); an item that a password manager marks concealed or transient never crosses, and a file URL from the guest crosses only when it points into a share. The guest's `Downloads` folder is `profile/downloads/` on the host (`paddock app files NAME` opens it), and the agent remakes the link at each boot. The `guest-files` port of the agent takes 16 MiB per transfer, 64 transfers per minute, and closes a transfer idle for 10 s. paddock has not confirmed the shares, the drops and the clipboard on a Mac yet; `packaging/macos/checks/macvm-files.sh` checks them.
- The windows. Every window of the app in the guest is a borderless window on your desktop that shows exactly that window: a view of the full guest display, placed so that only that window's rectangle shows. There is no pixel copy and no Screen Recording permission on your Mac. The window follows the guest's moves, resizes, titles, order, minimize and focus, and your clicks, keys and Cmd-Tab reach the guest window. A drag of the title bar or an edge moves or resizes the session window on your Mac at once, and the guest window follows (one move to the guest at a time, then the newest). The content moves with the frame, and the guest's own reports move only the content until the guest window is where you put it. A drag that ends where the guest cannot show the window (the gap between two screens, beyond the size of its level) moves the window back to the nearest allowed place. A click on the title bar without a move, a double click (the guest's zoom) and a click on a button of the title bar or the toolbar go to the guest. The guest agent reads the title bar height and its buttons from Accessibility (28 points when it cannot), and it measures the corner radius of each window once on the window's image, so the clip of the session window has the same corners. The image needs Screen Recording in the guest: `paddock macvm setup` sets that permission for the agent with the Accessibility and Full Disk Access permissions. A golden image from before this version has no such permission until `paddock macvm setup` runs again, and its windows get the radius of the guest's macOS version. Menus, tooltips, popovers and sheets of the app are windows of their own, attached to their parent, and never take the keyboard. The guest display is as large as the union of your screens in points, at the scale of your main screen, and that size is fixed at the start of the session (`PADDOCK_MACVM_DISPLAY` overrides it). With two screens of two scales, the start lines say so, and a window on the other screen is drawn soft. A window that the guest puts in the gap between two screens goes onto the nearest screen, and the guest follows. Your keyboard layout follows you into the guest: while a session window is key, the guest holds the host's input source. An input method that composes (Pinyin, Wubi, Zhuyin, Cangjie, Kotoeri, Hangul, Telex and the third party ones) composes in the guest: your Mac shows the plain layout (ABC) while the window is key, the badge shows the guest's source (`input: Pinyin - Simplified`), and the candidates show as a window of their own next to the caret. When the window resigns key or the session ends, your Mac gets its input source back; after a crash of the session helper, the next `paddock app list` puts it back. A lone press of the Globe key on a session window switches the guest's input source, not the host's. Reduce Motion and Increase Contrast of your Mac are set in the guest. A keyboard layout that the guest does not have is named in the badge (`keyboard layout X not in the guest`). When the guest cannot select your input source (for example Pinyin, when its input method is not on in the guest), the guest uses the ABC layout (or US) instead, and `agent.log` has one info line that names both. VoiceOver and other Accessibility clients read the controls of the guest window through the host window: the first query of a session window's children starts a mirror of the guest window's Accessibility tree (roles, titles, values, frames, four levels deep, read by the guest agent and kept current while the window is key), a press on a mirrored button presses it in the guest, and a value set on a mirrored field sets it in the guest. A tree above 64 KiB or 1500 elements loses whole subtrees. A press on an element from an earlier tree is dropped with a line in the event log, never sent to another control. Keyboard navigation inside the guest window (Tab, the arrow keys) is the guest's own. Limits of this version: a menu, popover or tooltip larger than a quarter of your main screen, or a floating window larger than your main screen, is refused the same way; two overlapping windows of one session on two Spaces show through each other, because the guest has one display; a full screen guest window fills one screen of your Mac, so with two screens the guest display is larger than the full screen window and its lower part does not show; and a window without a measured radius has the radius of the guest's macOS, so a wrong guess shows a black corner, not your desktop. The host menu bar shows the app's own menus from the guest (`File`, `Edit`, `View` and the rest, with their shortcuts and their enabled and checked states, two levels deep), and a click on an item presses it in the guest; the paddock app menu (`About`, `Firewall rules…`, `Hide`, `Quit`) stays first, and the app's own first menu (`Settings…`) is not shown, its shortcut Cmd-, still reaches the guest. A menu bar above 20 KiB loses whole submenus. When the guest agent is slow, restarts or is away, the host keeps the last menu bar, a click then is dropped with a line in the event log, and after the app exits the paddock menu with its `Window` menu is back. The Dock menu has the windows, `Open <App> again` after the app exited, `Restart guest agent`, `Restart guest` after 30 s without a health report, and `Quit`. Cmd-Q on a session window quits the session the same way as `paddock app stop`. paddock has not confirmed on a Mac yet: the windows themselves, the latency of a key press and of a drag, the context menus, the second screen, and the rounded corners (`packaging/macos/checks/macvm-windows.sh` checks them), nor the keyboard layouts and input methods, the Globe key, the restore after a crash, two screens of two scales and Spaces (`packaging/macos/checks/macvm-input.sh` checks them), nor the menu bar of the app, a press through it, the switch between two sessions and the menu after an agent restart (`packaging/macos/checks/macvm-menu.sh` checks them), nor the Accessibility mirror of the guest window (`packaging/macos/checks/macvm-input.sh voiceover` checks it).
- Upgrades. A new paddock never needs a new golden: the session shares the guest agent of its own version read-only (`macvm/agent/<version>/`, made from `paddock-guest` next to `paddock-vm`), and the agent in the guest replaces itself with it at the next start, also for a downgrade, in under 5 s. When the new agent fails three times at once, the guest falls back to its previous agent, the session logs `agent rollback` and talks to it. A new macOS for the guest is `paddock macvm update` (or `--ipsw PATH`; `--force` rebuilds the same version): it builds into `golden.new`, with the first boots again, and swaps the folders when the build is sealed. The old golden stays as `golden.prev` until a profile completes a session on the new one, then `paddock gc` removes it; `paddock macvm rollback` puts it back before that. An existing profile keeps its own system disk and its macOS version: the home on the data disk belongs to the account on the system disk, and a data disk that the guest formatted mounts only under the system disk that formatted it (the keys live there), so paddock never replaces the system disk under a data disk. A new profile whose data disk was never used counts as a pair too. New profiles clone the new golden, and `paddock app reset NAME --data` moves an existing profile to it after a backup of the pair; the login and the files of the guest go with that. A macOS update of your Mac costs one cold boot per profile: the saved state is discarded, the data stays. A swap that a crash interrupted is finished at the next command, with one line. paddock has not confirmed the agent update, the golden update and the rollback on a Mac yet (`packaging/macos/checks/macvm-upgrade.sh`).
- The app, the Dock face and the launchers. The app profile names the host bundle (`/Applications/ChatGPT.app`, `/Applications/Visual Studio Code.app`, `/Applications/Claude.app`), and `[apps.NAME] bundle = "PATH"` in `~/.config/paddock/config.toml` names another one. The route never signs or edits the bundle, so an App Sandbox app and a hardened app run unchanged, under Apple's own sandbox inside the guest. At each start paddock compares `CFBundleShortVersionString` and `CFBundleVersion` with the stamp of the profile and, when they differ, clones the bundle into the profile (an APFS clone, no space) and the guest copies it into its home with `ditto`, about 20 s for a 500 MB app, once per version. The copy is written as `<Name>.app.partial` and renamed, so a killed session never leaves half an app, and the old copy stays until the new one is whole: a failed copy is one line and the app runs from the copy that is already there. The guest needs the size of the bundle plus 500 MiB free on its data disk, and refuses the copy with that line otherwise. An App Store app is refused with one line, because the guest has no Apple ID: install the vendor's download into `/Applications` instead. The quarantine attribute is removed from the copy only when the host bundle carried your approval, so Gatekeeper shows no dialog in the guest for an app you have opened on your Mac. A session is one process with the app's own name and icon in the Dock, in Cmd-Tab and in Mission Control: paddock makes `macvm/shims/NAME.app` with a copy of `paddock-vm` inside, the app's icon, `CFBundleName` `ChatGPT (paddock)`, and an ad hoc signature with the virtualization entitlement, and builds it again when the paddock version in it differs. `paddock app install NAME` puts the same bundle in `~/Applications/<Name> (paddock).app`, so the app starts from the Dock, Launchpad and Spotlight; `paddock app install --drop` puts `~/Applications/Paddock Apps.app` there, and an app bundle dropped on it becomes a profile (`[apps.<slug>] bundle = "PATH"`, the slug from the app's name) with its own launcher, and starts. `paddock app uninstall NAME` and `paddock app uninstall --drop` remove them. `[apps.NAME] vm_memory = "6G"` gives the guest more memory, at most half of the Mac's. paddock has not confirmed on a Mac yet: the copy into the guest, the login that survives a quit, a chat with no direct connection, the Dock name and icon, the half copy after a kill, the relaunch from the Dock menu, the absence of a Gatekeeper dialog, the drag target, and VS Code with a workspace folder; `packaging/macos/checks/macvm-chatgpt.sh` checks them, and it is the end-to-end check of this route.
- The firewall of the guest (network mode `ask`). An app profile in the guest runs in network mode `ask` unless `--network` says otherwise: `egress`, `api`, `allowlist` and `none` work as on every route, `host` is refused, and `ask` is refused on every other route with one line. In mode `ask`, only the app asks: the processes of its bundle and their children, as the guest proxy names them. The guest system never asks. A connection of a daemon, a widget or an Apple service that no rule allows is denied at once with the rule word `system`, written to the connection log with its host and its process, and counted per host. A seed or a rule of your own can still allow such a host. When the app connects to a host that no rule covers, the connection is held, and paddock asks you: a panel next to the app's window (drawn by the window marker), the same question in `paddock approvals`, and `paddock approvals answer ID --allow-once | --allow-session | --allow-always | --deny | --deny-always` from a terminal, whichever comes first. A session shows one panel at a time. The other questions wait behind it, and the panel says how many (`and 3 more waiting`). At most three questions come up per minute. When more than 20 questions are open, asking pauses for that session: one line goes to the session log, `paddock app status NAME` says `asking paused` and lists the open questions, and one notice replaces the panel. The notice has two buttons. `Deny all` denies every open question once, and `Ask one by one` goes on. A panel never closes by itself while its question is open. The panel uses the system material and the system colors in Light and Dark mode, with `Deny` (Esc), `Allow once` and `Always allow` (Return), and the other answers under `More…`. `paddock approvals` keeps the `Why?` line with the process in the guest that opened the connection. `--allow-always` and `--deny-always` write a rule into `~/.config/paddock/firewall/NAME.toml` (mode 0600, written whole and renamed into place); `--allow-session` makes a rule of this session only. A second connection to the same app, host and port while one asks waits on the same question. After 60 s with no answer, that connection is denied once and the question stays. An answer that comes later still counts: the next connection to that host gets it; a question from before a sleep of the Mac is denied once and asked again, so the panel you did not see is fresh. An approver hook (`paddock approver`) with the capability `connect` answers before you. The rules decide by the most specific match: the exact app, host and port first, then wildcards from the right (the port, then the host, then the app); at the same specificity your own rule beats a seed of the profile, a session rule beats an always rule, and deny beats allow. The host of a rule is a name, `*.name` (every name under it, not the name itself), an IP address or `*`; a rule that allows every host needs `--yes-all-hosts`. `paddock app rules NAME` lists the rules with their ids, `add HOST [--port P] [--app BUNDLE] [--deny]` and `remove ID` edit the file, also while the session runs. The profile seeds `allow` rules for the hosts of a plain login (`seed_hosts` of `[app.macos]` in the profile, `by = "profile"`), so the first run of ChatGPT asks nothing; a seed never covers a host outside its list, and your own deny rule wins over it. In modes `api` and `allowlist`, the host list of the mode decides before any rule, because the identity of the guest process can be forged by a root process in the guest. Every decision goes into `logs/connections.jsonl` of the profile (the time, the app, the team, the host, the port, the verdict, the rule id or `ask`, `once`, `timeout`, `stale`, `mode`, `egress`, and at the close the bytes up and down and the seconds; never the content), merged per minute and capped at 5000 lines like the boundary log, a new file per session with the last five kept. The golden also turns off the guest services that phone home and have no use in a paddock guest: desktop widgets, Siri and suggestions, the iCloud setup nag, the News, Weather and Siri agents, automatic macOS installs, location services, analytics and personalized ads. Each one is a documented setting. Each is best effort: a setting that does not take on a new macOS is a warning of the setup, not a failed golden. paddock has not confirmed on a Mac yet: the panel next to the window and how it looks, the answer from a terminal, the rule file after `--allow-always`, the 60 s timeout, the panel after a restart of the marker, a first ChatGPT login with no question, the guest system denied without a question, the queue, the rate limit and the pause, and the effect of each quiet setting. `packaging/macos/checks/macvm-firewall.sh` and `packaging/macos/checks/macvm-quiet.sh` check them.
- Performance. The targets of the route are in `packaging/macos/bench/targets.json`: a warm start to the first window in 6 s (p50) and 10 s (p95), a cold start in 45 s, a first start with the copy of a 500 MB app in 120 s, a quit with save in 10 s, the agent self-update in 5 s, 40 ms from a key on the host to the value change of a guest text field, 8 ms from a guest window event to the host frame, 60 fps for a guest animation with no frame older than 50 ms, 3 percent of one core and the configured 4 GiB at idle (the guest keeps its memory: the memory balloon is off), 60 percent of one core with a 1080p video, and 6 GiB of disk per profile without the app copy. `packaging/macos/bench.sh` measures every target through `paddock macvm bench CASE` (warm, cold, first, quit, update, input, move, frames, idle, video, disk), writes `.paddock/bench/<date>.json`, prints the table and exits non-zero with the case named when a target is missed. The bench runs in a development build, or in a release build with `PADDOCK_MACVM_BENCH=1`, and only in a session started with `paddock app run NAME --bench`: that flag lets the guest agent honor the bench ops (`bench.animate`, `bench.text`, `bench.hang`, `bench.spawn_child`, `bench.pasteboard`, `bench.env`, `bench.dock_menu`), which every other session refuses. The committed reference run is `packaging/macos/bench/baseline.json`, and a Linux test checks that it meets every target, so a target change and a baseline change are one commit. paddock has not run the bench on a Mac yet: the baseline is a placeholder at the targets, marked `measured: false`, until `packaging/macos/bench.sh --baseline` writes a real run; `packaging/macos/checks/macvm-bench.sh` checks the bench.
- Status, logs and the support bundle. `paddock app status NAME` is one screen about a profile of the guest: the session (pid, age, `warm start in 4.1 s`), the golden, the agent and the protocol of its stamp, the app with its version and the day of its copy, how many windows it has, the guest's last health (age, load, free RAM, free data disk), the sizes of the profile and the free space, the network mode with the counts of allowed and denied connections, the shares and the logs folder; `--json` gives the same as a struct. A stopped profile shows how the last session ended (`quit with save`, `quit without save`, `crashed`) and what the next start does (`warm`, `cold: the guest image changed`). `paddock macvm status` lists every profile with that state and the two VM slots of the Mac with their holders. The logs of a profile are in `profiles.noindex/NAME/default/logs/`: `shim.log` (the session helper, 5 files of 1 MiB), `events.jsonl` (one JSON line per state change, 2 MiB cap), `console.log` (the serial console of the guest, 5 files of 1 MiB), `agent.log` (the log of the agent in the guest, fetched at each quit, the previous one kept as `agent.log.1`) and `boundary.jsonl` (the dropped frames of the guest network). `paddock app log NAME` prints the connection log of the session, one decision per line with the app, the host, the rule and the bytes (the boundary log of the profile when a session never ran), `--events`, `--shim`, `--guest` and `--console` the others, `--follow` keeps printing until the session ends. No log holds the session token, the guest password or clipboard text: every message is logged by its kind without its payload, and a test writes the clipboard messages through each log and greps for a marker. `paddock doctor` has a `macvm` block: the golden and its seal, the host and the entitlement of `paddock-vm`, each profile with its size and last end, the slots, the permissions (none needed) and the free space. `paddock doctor --report` adds a `MACVM` section with that block, the manifest, each stamp, the last 200 lines of each log of each profile, the firewall rules files and the counts of the connection logs by verdict, through the redaction of the report plus two passes of its own: a 32-digit hex run on a line with `token` becomes `<token>`, a line with a password is removed; `--no-hosts` turns every host name of the rules files and the connection logs into `host-N`. paddock has not confirmed on a Mac yet: the status of a running session, the four logs after a quit, `--follow`, the report and the doctor block; `packaging/macos/checks/macvm-status.sh` checks them.
- The security checklist. The 21 items of the route (no share of a system folder, a share only for a folder you named, the guest password never in a session request, the token never on disk or in a log, the modes of the profile files, a forged guest identity that cannot pass the host list of the mode, no forward of a host socket but the proxy, every unanswered frame dropped and logged, the clipboard only while a window is key, a crash that leaves no VM and discards the state, no host permission, the sealed golden, one control connection per session, the limits of the files port, the bounded header line, the sticky slots folder, the hash of an agent update, the redaction of the support bundle, the rules file, the window size per level, and the input source restore) are each one test in `src/macvm/security_tests.rs`, named `s01` to `s21` after the item numbers, and a test counts them. Four of them need a Mac: the `uchg` flag of the golden, `kill -9` of the session helper, the TCC database and the slots folder; `packaging/macos/checks/security.sh` prints one line per item, and the `macvm` block of `paddock doctor` says the state of the modes, the seal, the permissions and the slots folder. paddock has not confirmed those four on a Mac yet.
- The devices that you turn on in `[devices]` come over a socket, as for terminal runs (see [On macOS](#devices-on-macos)). paddock puts `paddock-host` at the end of the app's PATH, so a terminal in the app can run `paddock-host device list`.

### App mode on macOS under Seatbelt

<a id="app-mode-on-macos"></a>`paddock app run vscode ~/src/api --backend seatbelt` runs the app bundle under Seatbelt (`sandbox-exec`) instead of the guest VM of [App mode on macOS in a guest VM](#app-mode-on-macos-in-a-guest-vm). This is the older route, and it gets no new work. paddock knows `vscode` (`/Applications/Visual Studio Code.app`) and `claude-desktop` (`/Applications/Claude.app`) there.

What the sandbox gives the app:

- The workspace folders and the sandbox home are writable. The app bundle and the system folders are read-only. Everything else is denied, also your real home.
- `HOME` and `CFFIXED_USER_HOME` point at the sandbox home, so the app's Application Support and preferences live there.
- The app connects only to localhost. paddock's proxy listens there and passes its traffic on, with the same rules as network mode `egress` (or `api`, or `allowlist`). Electron apps get `--proxy-server` and the proxy variables.
- With `--network host`, the app gets the network of your Mac: it connects anywhere, listens on every address and resolves names itself. It keeps the host proxy variables, as on Linux. paddock prints a warning at the start.
- `[apps.NAME] login_ports = true` needs nothing on macOS, because Seatbelt has no network namespace. The app listens on the localhost of your Mac, and your browser reaches it there. paddock prints one notice line at the start and opens nothing. With network mode `none`, the profile then allows the app to listen on localhost, and nothing more.
- Electron apps get `--no-sandbox`, because Chromium's own sandbox cannot start inside Seatbelt. They get `--use-mock-keychain`, so they never read or write your Keychain, and a data folder of their own (`--user-data-dir`) in the sandbox home.
- Hidden files such as `.env` stay denied inside the workspace folders.

Every window of an app sandbox carries a mark, so that you can tell it from your other windows:

- A thin frame around the window. Sage means that traffic goes only through paddock's proxy (network `egress`, `api` or `allowlist`) or that there is no network. Amber means a wider network.
- A badge such as `paddock · vscode · egress · 2h` on the top right corner. It shows for a few seconds when the window gets focus, and while the pointer is near the corner. Click it for the sandbox details.
- A shield in the menu bar with the number of protected windows. Its menu lists each window. Choose one to bring it to the front.

paddock starts the marks with the first app sandbox, and they end with the last one. The marks need no permission. If you want the menu to raise the exact window instead of the app, open System Settings, then Privacy & Security, then Accessibility, and turn on the app that runs paddock (your terminal app). To turn the marks off, set `PADDOCK_MARK=0`.

Terminal runs get the same mark when the terminal app is in front at the start. When `paddock codex` starts in a terminal window, paddock finds the terminal app: the nearest parent process that owns a window. If that app is the front app, its front window is the window where you typed the command, and that window gets the frame, the badge and an entry in the menu. The menu entry names the terminal app, for example `codex · api · egress · 2h (Terminal)`. The other windows of the terminal app get no mark. When the run ends, the mark goes, and the window stays. If you close the window while the run continues, the mark goes too. This works for Terminal, iTerm2, Ghostty, kitty and WezTerm, and needs no permission. `paddock windows` lists the marked windows with their window numbers.

These terminal runs get no mark, and with `-v` paddock prints one log line that says why:

- A run that starts while another app is in front, for example from a script or a Shortcut.
- A run in tmux or `screen`, a run with `--detach`, a child of `paddock-host spawn` and a relayed run.
- A terminal window that is not a normal window: the hotkey window of iTerm2, the quick terminal of Ghostty and the panel of kitty.
- The window of `paddock view`.

The limits:

- Denied, not absent. The names of files in your real home stay visible (the app can `stat` them), but the app cannot read or write them.
- Apps that use Apple's App Sandbox (ChatGPT, for example) crash inside Seatbelt. paddock refuses them: Apple already confines them. Start them as usual.
- One instance per data folder. paddock refuses a start when the app already runs with the same data folder, because the running copy would take over the start.
- The app reaches the macOS services that a desktop app needs (the window server, the pasteboard, Launch Services). It can ask macOS to open a link or another app, outside the sandbox. It can also connect to services on your Mac's localhost.
- Not there yet on macOS, and refused when you ask for them: `[apps.NAME] bus`, `paddock freeze` of an app under Seatbelt (it works for terminal runs and for the guest route), and backends other than Seatbelt and `macvm`. Links (`claude://`) do not reach the sandbox yet.
- An app that Seatbelt cannot run, such as an App Sandbox app, runs in the guest route: see [App mode on macOS in a guest VM](#app-mode-on-macos-in-a-guest-vm).

### Terminal runs on macOS

On macOS, `paddock run codex ~/src/api` runs the agent under Seatbelt (`sandbox-exec`). The backend is `seatbelt`, and `--backend auto` picks it for most runs on macOS (see [the VM backend](#the-vm-backend-on-macos)). `docker` and `podman` stay available.

What the sandbox gives the agent:

- The project, or its worktree, is writable. Hidden files such as `.env` stay denied.
- The sandbox home is writable. `HOME` and `CFFIXED_USER_HOME` point at it. It gets the read-only imports of the profile and the login of `[state] auth`, as on Linux.
- The system folders, `/opt` (Homebrew) and `/nix` are read-only. Everything else is denied, also your real home and the shared `/tmp`.
- Network modes `egress`, `api` and `allowlist` connect only to a relay on a free localhost port, which passes the traffic to paddock's proxy. The proxy variables point at the relay. In modes `api` and `allowlist`, the proxy lets through only the hosts of the list. In modes `egress` and `allowlist`, the sandbox can also reach the host ports that `[network] host_ports` allows. Network mode `none` has no IP network.
- Network mode `host` gives the network of your Mac: the agent connects anywhere, listens on every address, and resolves names through mDNSResponder. It keeps the host proxy variables, as on Linux. `paddock run` prints the mode and a warning at the start. Its servers are open to the LAN.
- `paddock-host` works: kv, spawn, `done`, timers and the other host requests. The sockets stay at their host paths, and `PADDOCK_HOST_SOCKET` and `PADDOCK_KV_SOCKET` name them.
- The devices that you turn on in `[devices]` work over a socket: `paddock-host device NAME` (see [On macOS](#devices-on-macos)). Secret files are at `$PADDOCK_DEVICES/secret/NAME`. A device with `run = "sandbox"` runs in a Seatbelt device sandbox of its own, started by the paddock process of the run (see [The device sandbox](#the-device-sandbox)).
- `paddock-host borrow` and `return` work. Seatbelt cannot mount the borrow folder at `/run/paddock/borrow`, so the agent changes the copies in the host folder `$XDG_DATA_HOME/paddock/borrow/<sandbox id>/`, which `PADDOCK_BORROW_DIR` names and `borrow` prints. The agent cannot remove or replace that folder itself. paddock copies without reflinks on macOS, so `[borrow] max` limits every borrow.
- `--env clean` passes only the allowlist, as on Linux. `paddock run --detach` uses paddock's tmux server, as on Linux.
- The agent reaches a short list of macOS services: user names, the system log, TLS certificate checks and file change events. It does not reach the Keychain, the pasteboard, Launch Services or the preferences of your real home.

The limits:

- Denied, not absent. The names of files in your real home stay visible, but the agent cannot read or write them. The user names of your Mac and its hardware UUID stay readable.
- Shared caches (`~/.npm` and the others) are not shared: the sandbox home keeps its own copy. paddock names them at the start.
- The profile allows only the relay and the allowed host ports. So the agent cannot connect to its own servers on localhost, such as a dev server, unless you open a port range (see below). On macOS 15, Seatbelt also counts the other addresses of your Mac as localhost. The same ports then open on those addresses too, and no other ports.
- The agent cannot open a terminal by its path (`/dev/ttys*` or `/dev/ptmx`). Seatbelt cannot tell a new terminal from your other terminals by path, and an open terminal gives its keystrokes away. No process in the sandbox can push input into a terminal (`TIOCSTI`).
- Some features are not there yet on macOS. If you ask for one, paddock refuses the start, or the request, with one line:
  - display modes other than `off`
  - the private keyring, login ports and links into the sandbox
  - credentials that go through the gateway
- `paddock doctor --escape` is for the Linux backends. The Seatbelt checks run with `cargo test` on a Mac.

New terminals (ptys) come from paddock. Some agents open a pty for their shell tool, for example Codex's unified exec. paddock opens each pty on the host and sends both ends into the sandbox over a Unix socket. A small library asks for them in place of `openpty`, `forkpty` and `posix_openpt`. paddock loads it into the agent with `DYLD_INSERT_LIBRARIES`. This works only when the agent's program takes the library:

- A program that you build yourself, or a script for a Node or Python from Homebrew or nodejs.org, takes it.
- A program with the hardened runtime ignores it, unless it has the entitlements `com.apple.security.cs.allow-dyld-environment-variables` and `com.apple.security.cs.disable-library-validation`. The released Codex builds (npm, Homebrew, GitHub) have the hardened runtime without them.
- macOS drops the library for its own programs (`/bin`, `/usr/bin`), and for a script that starts with `#!/usr/bin/env`.

If the agent ignores the library, paddock prints one line at the start, for example `paddock: no pty for codex: ... has the hardened runtime without ...`. Its requests for a new pty then stay refused, as before. A program that the agent starts from `/bin/bash` or `/bin/zsh` does not get the library either. paddock gives a run at most 64 new ptys in 10 seconds.

To let the agent reach its own servers, open a range of localhost ports in `~/.config/paddock/config.toml`:

```toml
[macos]
localhost_ports = "3000-3999"
```

In network mode `egress`, the agent can then listen on these ports and connect to them. macOS has no network namespace, so the range also reaches the services of your Mac on these ports. Choose a range that your own services do not use. `paddock run` prints the range at the start. The range has at most 1024 ports. No range is open by default, and the project `paddock.toml` cannot set one.

### The VM backend on macOS

`paddock run codex --backend vm` runs the agent with Linux paddock and bwrap in a light Linux VM. Use it when an agent needs a Linux sandbox on a Mac, for example new ptys for a harness that ignores paddock's pty library. paddock kv, the devices of `/dev/paddock` and borrow do not reach the VM yet: a run with devices starts with a notice. When they do, a device of a VM run runs on the Mac, started by the paddock process of the run: a `run = "sandbox"` device in a Seatbelt device sandbox, with the host path of the project as the caller's project, and `"platform": "macos"` in the catalog so that an agent in the guest knows the program builds on macOS tools. Nothing runs in the guest for a device. A device that needs Linux tools is a `remote` device to a Linux machine (see "Remote devices"). The VM needs Apple silicon and its files. `paddock vm setup` downloads them once, after it shows the URL, size and sha256 and asks you. `paddock run` never downloads anything. paddock looks for the files in its data folder first, then in the share folder of the Homebrew install. `PADDOCK_VM_SHARE` replaces the Homebrew folders with its own list of folders, separated by `:`, and an empty value turns them off.

The first run starts one VM for your user. Later runs use the same VM. It stops after 10 minutes without a run. The VM has no network device, and the project folder is the only host folder in it, at the same path. Your home is never in it. Host requests, network modes `egress` and `api`, and login pages in your browser go to paddock on the Mac, as on Linux. The logins of the agent stay in the VM, in a home per sandbox. The VM holds one project at a time. A run in another project stops with a message while the runs of the first project go on.

```sh
paddock vm status    # the running VM: pid, uptime, runs and project
paddock vm stop      # stop it, with its runs
```

The macOS guest for desktop apps is a VM of its own, apart from this Linux VM: see [App mode on macOS in a guest VM](#app-mode-on-macos-in-a-guest-vm).

`--backend auto` chooses as follows:

1. `paddock app run` uses Seatbelt.
2. A run that needs a Linux-only feature, or new ptys that Seatbelt cannot give its harness, uses the VM. If the VM files are missing, paddock stops and names `paddock vm setup`.
3. A harness profile with `[macos] backend = "seatbelt"` uses Seatbelt, for tools that need macOS itself, such as Xcode or codesign.
4. Every other run uses Seatbelt, unless your configuration makes the VM the default:

```toml
[macos]
backend = "vm"
```

`paddock run` prints the backend and the reason at the start. The VM has no display: display modes `window` and `headless` stay Linux only, and network modes `host` and `allowlist` are not available in the VM.

## Sandbox by default

An agent reads files with the same commands that you use. One wrong `grep -r` or `find ~` outside a sandbox sends your SSH keys, other projects and personal files into the agent's context, and to the vendor. `paddock shims install` makes the sandbox the default, so that you do not have to remember `paddock`:

```sh
paddock shims install    # claude, codex, copilot and the others now start in a sandbox
paddock shims list       # each command, and whether it is on
paddock shims remove     # takes everything away again
```

After `install`, typing `claude` in a new shell runs `paddock run claude` in the current project. Arguments pass through: `claude --resume` runs `paddock run claude -- --resume`. paddock writes one script, `~/.local/share/paddock/shims.sh`, with a shell function for each command of each installed harness. It adds one marked line to `~/.bashrc` and `~/.zshrc`, where they exist, that loads the script.

- To run a command without the sandbox once, prefix it: `PADDOCK_BYPASS=1 claude`.
- In your home directory, the command stops with an error, because paddock does not use the home directory as a project. Go to a project first.
- Shell functions win over PATH, so version managers such as mise do not bypass them. Scripts and programs that start `claude` themselves are not covered: only interactive bash and zsh shells. For the agents that Omarchy starts, see the next section.
- If paddock or the script is gone, each function runs the real program, and the line in the rc file does nothing.
- When you install another agent, or a newer paddock brings a profile, the next harness start updates the script in the background (see [Keeping the menu and the shims current](#keeping-the-menu-and-the-shims-current)).
- `paddock doctor` lists the commands that start in a sandbox, the rc file that loads them, and how to turn them off. Each run also prints a `paddock:` line with the sandbox it started, so you always see when a shim takes over.

### Only the agents that Omarchy starts

Omarchy starts the default agent with `omarchy-agent`: from the key Super+Shift+Ctrl+A, the drop-down console, the agents panel, `omarchy agent prompt`, crash reports and the alias `a`. These starts run unattended: `omarchy-agent` adds each agent's flag for no approvals, such as `codex --approve-for-me`. This scope puts only these starts in a sandbox:

```sh
paddock shims install --scope omarchy-agent   # agents from omarchy-agent start in a sandbox
paddock shims list                            # shows the scope and whether it is on
paddock shims remove                          # removes both scopes
```

- Commands that you type and scripts do not change. Omarchy's files do not change either.
- paddock writes one marked block into `~/.config/mise/config.toml`: a mise wrapper for `omarchy-agent`. It keeps the rest of the file, with its comments and order, and runs `mise reshim`. `paddock shims remove` takes the block out again, byte for byte. If the file already has its own `[wrappers.omarchy-agent]`, paddock does not replace it and stops with a message.
- The wrapper runs the real `omarchy-agent` with a private folder first on `PATH`, for that start only. Its shim for `omarchy-launch-tui` puts `paddock wrap --` in front of the agent command. The agent window keeps its app id, `org.omarchy.agent`. With `--inline`, the agent starts the same way in the current terminal.
- If paddock is gone, the wrapper runs `omarchy-agent` without a sandbox, so the key still works.
- The same block has a second wrapper, for the agents panel's `omarchy-agent-usage-update`. It runs Omarchy's command, unchanged, in a box: the system read-only, an empty home and its own `/tmp`. In that home, each harness folder (`~/.claude`, `~/.codex`, `~/.grok`, `~/.gemini` and the others that the profiles name) is one tree of the host folder and the same folder of every sandbox home of that harness. A collector can write into these trees, for example Codex's database in `~/.codex`. The writes go to a temporary layer in the box and are gone when the box ends. They never reach your home or a sandbox home. With many sandbox homes, paddock merges the folders in two steps, so the panel counts every sandbox home. The box also sees the panel's record and cache folders (read-write), its settings in `~/.config/omarchy/agents` and the tool folders on `PATH` (read-only). It does not see `~/.ssh`, your documents or other projects. If the box cannot start, the command does not run on the host: it prints one line that names the problem and `paddock shims remove`, and the panel keeps its last records.
- `PADDOCK_BYPASS=1 omarchy-agent` starts once without a sandbox.

`paddock wrap -- COMMAND` runs the command as it is, in a sandbox:

- The arguments stay the same. paddock adds no auto-approval flags and removes none. It adds only the telemetry switches of the profile (see [Privacy defaults](#privacy-defaults)).
- The project is the current folder. The home folder is no project. If `omarchy-agent` starts in the home folder (when `~/Work` does not exist), the agent works in a scratch project of its own, `~/.local/share/paddock/scratch/<agent>`. The `paddock:` line names it, `paddock files` lists it, and `paddock gc --scratch` removes old ones.
- An `env` prefix keeps its variables. For Claude Code, Codex and Grok Build, Omarchy's account switch sets `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `GROK_HOME`. paddock uses that folder as the login of the sandbox: it copies the login in at the start, and copies a refreshed token back at the end, the same as for your main login. The pin `OMARCHY_AGENT_<AGENT>_HOME` stays.
- The shell form `bash -c '…' amp PROMPT` runs whole in the sandbox, with `$0` set to the agent program. An absolute path, such as a user install in `~/.local/bin`, runs that program.
- The exit code passes through.
- A command without a paddock profile runs without a sandbox. paddock shows a notification that names it, so a start never fails only because paddock does not know an agent yet.
- If paddock knows the agent but cannot start the sandbox, it says why and does not start the agent on the host.

`paddock doctor` shows whether the wrapper and the shim folder are in place, whether `mise reshim` made the dispatch link, whether `omarchy-agent` still starts agents the way paddock expects, and whether your default agent has a paddock profile.

Codex starts its own sandbox for each command. That sandbox cannot start inside paddock. With `--approve-for-me`, Codex runs the command again without it, inside paddock's sandbox.

paddock has a profile for every agent that `omarchy-agent` can start. `paddock doctor` lists the entries of each profile that come from documentation and not from a test.

| Agent | In a sandbox | Notes |
| :--- | :--- | :--- |
| claude, codex, copilot, opencode, agy, grok | yes | Tested. claude, codex and grok follow Omarchy's account switch. |
| hermes, pi | yes | Tested: they start in the sandbox. Their code is read-only there, so `hermes update` runs on the host. |
| crush, cursor-agent, omp, ori, kimi, qwen, mcode, kiro-cli, freebuff, kilo, devin, amp, commandcode | yes | From the vendors' documentation. They were not installed when the profiles were written. |
| dsh | yes | `dsh web` serves its page on port 3080. paddock forwards that one port from 127.0.0.1 of your machine into the sandbox. |
| muse | no | Muse runs each shell command in its own bubblewrap sandbox, and refuses the command when that sandbox cannot start. paddock blocks nested sandboxes, so muse starts on the host with its own sandbox, and paddock shows a notice. paddock never turns muse's sandbox off. |
| openclaw | no | Its agent runs in the OpenClaw gateway, a service on your machine. The terminal UI only talks to it, so a sandbox around the UI protects nothing. Starts from `omarchy-agent` run as before, with a notice. `paddock openclaw` runs OpenClaw's embedded runtime in a sandbox, with its own state folder in the sandbox home. |

#### Usage statistics in the Omarchy agents panel

The agents panel reads the session history of each agent from fixed places in your home folder. A sandbox keeps its history in its own home. The panel's command runs in the usage box (see above), which shows the host folders and the sandbox homes as one tree, so the panel counts both. paddock copies no session back to your home: a transcript that a sandbox wrote and that your host harness loads later, for example with `claude --resume`, can carry instructions out of the sandbox.

- Claude Code keeps one folder per project in `~/.claude/projects`. paddock binds the host folder of the current project into the sandbox, read-write. The sandbox sees only this one folder, no other project's. `claude --resume` on the host also finds these sessions.
- Codex (`~/.codex/sessions`) and Grok Build (`~/.grok/sessions`) keep their history in the sandbox home. The usage box shows it to the panel. A folder in a sandbox home that is a symlink is not part of the tree.
- With an account folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME`), Claude Code's project folder is in that folder, the same as on the host.
- The panel's collectors count the sessions the same way as sessions on the host. `paddock gc` deletes a sandbox home with its sessions, and the panel stops counting them.

The panel also shows each agent's limits. Its collectors check limits online with the agent's login. Inside the usage box, every agent uses the same rule:

- paddock adds one marked block at the end of `~/.config/hypr/hyprland.lua`, where Omarchy asks for your own configuration. The block loads a file of paddock, which puts one folder first on the `PATH` of the programs that Hyprland starts, ahead of Omarchy's `bin` folder. The folder has one command in it: `omarchy-agent-usage-update`, which the panel runs for each refresh. Hyprland puts Omarchy's `bin` folder ahead of mise's wrappers, and the panel runs the command without a shell, so the panel reaches the usage box through this folder, not through the mise wrapper. Before paddock writes the file, `luac -p` (if installed) checks that it still compiles. It takes effect at your next login.
- For each login file of `[state] auth` in a profile and each collector login file, the usage box mounts the newest copy read-only. A collector cannot change a login file. The copies are the host file and the same file in every non-isolated sandbox home of that profile. When the file names a token expiry, the newest copy is the one with the latest expiry. Otherwise it is the one with the latest modification time.
- A login that the credential keeper manages (Codex) comes from the keeper's copy, and paddock holds the keeper's refresh lock while the usage box runs.
- An isolated login (`--auth isolated`) belongs to its sandbox only and never appears in the usage box.
- Some agents renew their login only while they run (agy). After an hour without a run, every copy of the login has lapsed, and the panel shows no limits. A profile can name a short command in `[state] refresh` that renews the login and does no model work. For agy it is `agy models`. When the panel asks for limits, and the newest copy of a login has lapsed and holds a refresh token, paddock runs that command once in a sandbox of the agent: with your shared login, network `api`, a scratch project and a time limit of 30 seconds. The renewed login goes back to your home, as after any run of the agent. Then the usage box picks the newest copy again. paddock tries this at most once per agent in 10 minutes. It never does this for an isolated login, a login of an account folder, or a login that the credential keeper keeps (Codex).
- Inside the box, there is no session bus and no host keyring (`DBUS_SESSION_BUS_ADDRESS` is unset and the runtime folders are hidden), so collectors read the mounted login files instead.
- Network traffic in the usage box goes through paddock's proxy. The proxy allows only the `[network] endpoints` of the profiles and the vendor hosts that the collectors call. If a collector calls a host outside that list, the proxy refuses the connection and prints the host name to standard error.
- Login files are read-only in the usage box. paddock never writes a login back to the host home from the box. If a collector tries to write into a read-only folder, the write fails and paddock prints one line about it.
- paddock checks only when the panel asks. It adds no timer of its own.
- `paddock shims remove` removes the block, byte for byte, and the folder. `paddock doctor` shows whether the panel's checks go through paddock.

Inside the sandbox, the installs of the agents are read-only. mise updates them on the host before each start (`auto_update`). Where a vendor documents a switch, paddock turns the agent's own self-update off in the sandbox. Other self-updates fail with "Read-only file system".

## Launchers

`paddock launchers install` adds a launcher entry for every harness and app that paddock finds on this machine, with the paddock icon, in a "Paddock" folder:

- In the Omarchy menu (Super+Space), a Paddock submenu. paddock writes it as a marked block in `~/.config/omarchy/extensions/omarchy-menu.jsonc` and leaves the rest of the file as it is.
- In menus that follow the XDG menu standard (GNOME, KDE, Xfce), a Paddock folder.
- In any app launcher, search for "paddock".

A terminal harness entry opens a terminal with `paddock pick <harness>`: pick a recent project or type a path, and the harness starts there. An app entry runs `paddock app run`. If you did not save workspace folders for the app (`[apps.NAME] roots`), a dialog asks for one: pick a recent project or any other folder. The dialog needs zenity. Without it, a desktop notification says how to name the folders.

Logins work from these entries too. When a harness opens its login page, the approval card asks "Open the login page of claude in your browser?" and shows the link. After you allow it, the page opens in your browser on the host, and paddock forwards the one localhost port that the login returns to, for ten minutes and only while the sandbox listens on it. Login callbacks are on by default for claude, codex, copilot, gemini, opencode and agy (`[launch] login_callback` in the profile). They work on the bwrap, docker and podman backends; on srt, paddock prints a warning instead.

```sh
paddock launchers list      # what install would create
paddock launchers install
paddock launchers remove    # removes everything that install wrote, and nothing else
```

### Keeping the menu and the shims current

The menu entries, the Omarchy menu block and the shell shims list the harnesses that were installed when you turned them on. paddock keeps them current:

- On a harness start, paddock compares a cheap signal with the one of the last update: its version and the modification times of the folders on `PATH`. Installing an agent with mise or into `~/.local/bin` changes a folder on `PATH`. When the signal changed, paddock runs `paddock refresh --background` in the background, and the start does not wait.
- `paddock update` runs `paddock refresh` with the new version.
- `paddock refresh` adds the entries and shims of new harnesses and removes those of harnesses that are gone. The background run only adds: with a smaller `PATH`, it could miss a harness, and a missing shim would start that harness without the sandbox.
- Only the parts that you turned on change. `~/.bashrc` and `~/.zshrc` do not change.
- `paddock doctor` names the harnesses that the menu or the shims miss.

## Project settings

`paddock init` writes a commented `paddock.toml` into the project. The project directory is writable from inside the sandbox, so an agent can edit this file. Any setting that widens access (extra mounts, host network, host display, GPU, passed variables, setup commands, image changes) needs a one-time approval on the host. paddock asks on the terminal, or you run `paddock approve`.

paddock finds the folders in `mounts.rw`, `mounts.ro` and `secrets.hide` again at each start. The agent can create symlinks in the project and in other folders that it can write. So paddock refuses such an entry if its path goes through a symlink in one of those folders, and the start stops with a message. Name the real folder instead. The same rule applies to the workspace folders of an app.

## Backends

| Backend | Use it when |
| :--- | :--- |
| bwrap (default) | You want every host tool, and start in milliseconds. |
| podman, docker | The project needs system packages that the host does not have. The base image follows the host distribution. |
| seatbelt (macOS) | You run an agent on a Mac. `--backend auto` picks it for most runs. |
| vm (macOS) | You need a Linux-only feature on a Mac. See [The VM backend on macOS](#the-vm-backend-on-macos). |

Prefer rootless podman over docker. paddock runs it with `--userns=keep-id`, so files in the project keep your user as owner. The default seccomp profile of podman allows nested user namespaces, a common start of kernel exploits, so paddock derives a profile that blocks only that.

## Add a harness

A harness is a TOML profile. Put a file in `$XDG_CONFIG_HOME/paddock/harness.d/`. A file with the name of a built-in profile replaces it. The built-in profiles are in [profiles/](profiles/).

```toml
name = "myagent"
description = "My agent"

[locate]
commands = ["myagent"]

[launch]
yolo_args = ["--yes-to-everything"]

[state]
auth = [".myagent/token.json"]
import_ro = [".myagent/config.toml"]
env_pass = ["MYAGENT_API_KEY"]
scratch = [".myagent/cache"]      # pure caches: a tmpfs in the sandbox
data = [".myagent"]               # what --profile default copies into its snapshot

# Telemetry switches, set only inside the sandbox. Cite the vendor's documentation.
[privacy]
env = { MYAGENT_TELEMETRY = "0" }

[[privacy.settings]]
file = ".myagent/settings.json"   # in the sandbox home; json or toml (format = "toml")
path = ["telemetry", "enabled"]
value = false

[privacy.training]
setting = "Settings, Data, Use my data for training"
url = "https://example.com/settings/data"

# The one-shot mode for `paddock-host spawn`. Optional.
[oneshot]
args = ["run", "--yes-to-everything", "--task-file", "{prompt_file}"]
prompt = "file"          # or "stdin"
probe_args = ["run", "--help"]

# The first prompt of the interactive interface, for `paddock run --detach`. Optional.
[detach]
args = ["Read the file {prompt_file} and do the task that it describes."]
```

More entries, for harnesses that need them:

```toml
[launch]
# paddock wrap runs the harness on the host, with this notice.
wrap_on_host = "it keeps its own sandbox, which cannot start inside paddock"

[state]
# An entry that ends with / is a folder: paddock copies every file in it.
auth = [".myagent/credentials/"]
# JSON keys that only go from false to true, such as a finished onboarding. paddock carries a true value between the login store and the sandbox, in both directions, and never writes false over true.
done_flags = { ".myagent/onboarding.json" = ["onboardingComplete"] }

# The folder that an account variable replaces, for paddock wrap.
[state.account]
env = "MYAGENT_HOME"
dir = ".myagent"

# Values written into the sandbox copy of a settings file, for example to
# turn off a self-update that cannot work on the read-only install.
[[state.settings]]
file = ".myagent/settings.json"
path = ["auto_update"]
value = false

[network]
# A loopback port of the sandbox that the host browser reaches on 127.0.0.1.
publish = [3080]
```

Before each start, paddock makes sure that the help text of the harness still mentions every flag in `yolo_args`. If a flag is renamed, paddock stops with an error and does not start a harness that waits for approvals. A spawn makes the same check for the flags in `[oneshot] args`, with the help text of `probe_args`. The task never goes into `args`: with `prompt = "stdin"` the harness reads it on its standard input, and with `prompt = "file"` paddock replaces `{prompt_file}` with the path of a private file in the sandbox home.

## Limits

The project directory is writable, and you use its files on the host later. Some host tools run project files without being asked: direnv runs `.envrc`, mise runs hooks in `mise.toml`, editors run tasks. paddock reports changes to these files when the sandbox exits. Review them before you use them on the host.

The agent can read its own API token and the variables that it inherits. With network access, it can send them elsewhere. Use `--env clean` and an isolated login to limit what it holds.

pasta hides the host addresses that exist when the sandbox starts. If the host gets a new address while a sandbox runs, for example from a VPN that starts later, services on it are reachable from that sandbox until the next start. paddock watches for new addresses, shows a warning in the bar panel, and prints it at exit.

Shared package caches connect the sandboxes of different projects. The shared mise installs are read-only in every project sandbox, and no project file runs while paddock writes them (see mise toolchains). The package caches (cargo, npm, pnpm, bun, yarn, uv, pip, go) are in layers. On the bwrap backend each cache is an overlay. The lower layer under `$XDG_CACHE_HOME/paddock/layers` is shared by all projects, is read-only in every sandbox, and only paddock writes it on the host. The upper layer under `$XDG_CACHE_HOME/paddock/projects` belongs to one project and takes all writes. A file that one project changes in its cache never reaches another project.

After a bwrap sandbox ends, paddock checks the new downloads in the project's upper layer in the background, for at most two minutes. It copies a package into the shared layer only when its content matches a source that no project can change:

- cargo: a `.crate` file from crates.io, against the `cksum` in the crates.io sparse index, fetched on the host.
- npm: a file of `_cacache/content-v2`, against the sha512 or sha256 hash in its own path.
- go: a module zip and its `go.mod`, against the `h1:` hashes of the checksum database `sum.golang.org`, fetched on the host.

A file that fails the check, or that paddock cannot check (no network, another registry), stays in the project. paddock never replaces a file that is already in the shared layer. The audit log records each result with the cache, the package and the version, never the content. A small record in the paddock state keeps each checked package version, so paddock asks the index only once for it. pip, uv, pnpm, bun and yarn have no shared layer: their packages stay in each project. Unpacked sources and build caches always stay in each project.

The docker, podman and srt backends have no overlay. They bind the project's own folder, read-write, so each project downloads its packages again. `[storage] layered_caches = false` does the same on the bwrap backend. No setting goes back to one writable cache for all projects.

Some profile entries come from documentation, not from a test on a real install. `paddock doctor` lists them for each installed harness.

On rootful docker, an escape from the container is an escape to root. Use the bwrap backend or rootless podman when you can.

## Logs and bug reports

paddock prints warnings and errors to the terminal. Other messages stay hidden.

- `-v` shows more: `-v` adds info, `-vv` debug, `-vvv` trace. `-q` shows errors only.
- `PADDOCK_LOG` sets the level too. `PADDOCK_LOG=debug` applies to everything. `PADDOCK_LOG=broker=debug,proxy=trace` applies to single parts.
- The parts that run for the whole session also write to the systemd journal, under the name `paddock`. These are the host request broker, the proxy, the login callback forwarder and the window border. Read the warnings with `journalctl -t paddock -p warning`. Without systemd, paddock skips the journal.
- The bar plugin is silent. It writes a warning to the shell log when it cannot read paddock's output. To see more, start the shell with `PADDOCK_DEBUG=true`. Read the shell log with `journalctl -t omarchy-shell`.

To report a bug, run `paddock doctor --report`. It prints the version, the `paddock doctor` output, the running sandboxes and recent paddock warnings from the journal, and on a Mac the `MACVM` section of the macOS guest (its logs, stamps and rules files). `--output FILE` writes it to a file. The report replaces your user name, host name, home path, variable values and tokens with placeholders; `--no-hosts` also replaces every host name of the guest's rules files and connection logs with `host-N`. Read it before you share it.

## Support scope

paddock is built and tested on Omarchy: Arch Linux with Hyprland. That is the supported platform. Every release passes `bin/test` and `paddock doctor --escape` there.

- Other distributions and compositors are maintained by the community. The bwrap backend, the network isolation and the terminal harnesses work wherever bubblewrap, passt and user namespaces work, but nobody tests each release on them. The apt and dnf paths in `bin/install` are best effort.
- Window focus and the border on protected app windows need Hyprland. The bar widget and the approval card need Omarchy.
- Pull requests for other platforms are welcome when they meet the bar in [CONTRIBUTING.md](CONTRIBUTING.md). Issues about platforms outside Omarchy can be closed without a fix.
- Every issue needs the output of `paddock doctor --report`. The issue form asks for it.

## Development

```sh
bin/test
```

To contribute, read [CONTRIBUTING.md](CONTRIBUTING.md). `bin/test` runs the unit tests, the integration tests, the leak guard, and `paddock doctor --escape` on each backend that the host can run. One test makes sure that tracked files contain no facts about the development machine: the home directory, the user name, the host name, the git server, or the git email.


## License

MIT
