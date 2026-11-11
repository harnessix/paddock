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
| Package caches (cargo, npm, uv, go) | uv and npm use writable caches per repository under `$XDG_CACHE_HOME/paddock/package-projects`. Other caches use worktree layers under `$XDG_CACHE_HOME/paddock/projects` and verified read-only layers under `$XDG_CACHE_HOME/paddock/layers`. |
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
