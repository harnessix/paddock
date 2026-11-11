## Host requests

Sometimes the agent needs one thing from your computer: a screenshot of the real screen, one command on the host, or one file outside the project. paddock does not open the sandbox for that. The agent asks with `paddock-host`, you decide on the host, and paddock does the work on the host.

| Capability | What the host does | Default |
| :--- | :--- | :--- |
| `screenshot` | Captures the screen, one output or the focused window | ask |
| `exec` | Runs one command in the project folder, without a shell, isolated (see below) | ask, and never allow |
| `tool` | Runs a named host tool of `[host.tools]` with allowed arguments only (see Named host tools (`paddock help host-requests`)) | the mode of each tool |
| `fetch` | Copies one host file or folder into the project, at most 64 MiB | ask |
| `borrow` | Copies a host folder to `/run/paddock/borrow/` for changes (see Borrow a host folder (`paddock help host-requests`)) | ask |
| `return` | Writes the changes of a borrowed copy back to its host folder | ask, and once per return |
| `clipboard-read`, `clipboard-write` | Reads or sets the host clipboard | ask |
| `open-url` | Opens an http or https link in your browser, for example a harness login page | ask |
| `notify` | Shows a desktop notification | allow |
| `git-push` | Pushes commits to a remote of the project, with your credentials | ask |
| `forge` | Runs a fixed set of `gh`, `tea` and `glab` commands on the project's repository, with your login | reads allow, writes ask |
| `spawn` | Starts another harness in its own sandbox with one task (see Spawn other agents (`paddock help host-requests`)) | ask, and off until `[spawn] harnesses` lists one |

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

`paddock log` shows every request with its decision and result. The log is `~/.local/state/paddock/host-audit.jsonl`. It does not keep text that can be private. For `clipboard-write`, it keeps only the length of the text. For `exec`, it keeps the program and the number of arguments, because arguments can hold tokens. With `[exec] remember_allowed = true`, an allowed command line of plain words stays whole, for `paddock tools suggest` (see Named host tools (`paddock help host-requests`)); it is off by default. For `forge`, it keeps the command without the body text. For `notify`, it keeps the lengths of the title and the body. For `open-url`, it keeps the scheme and the host. paddock makes its folders readable only by you (mode 0700) and its files too (mode 0600), and tightens older ones that other users can read. Request text never goes into the arguments of a host process, which other users can see: notifications go over the desktop bus, and `gh` reads the body on its standard input (`tea` and `glab` cannot, see above). To teach agents the client, run `paddock skill install`: it copies a short skill into the skill folders of your harnesses. Inside a sandbox, `paddock-host --help` is the full reference.

### Exclave

Exclave runs a command in a sandbox on a peer, which is another machine. The agent stays on the local host. The person configures the transport on the local host in `~/.config/paddock/config.toml`:

```toml
[exclave]
prefer = ["build", "test"]

[exclave.peers.worker]
transport = ["ssh", "worker", "paddock", "exclave", "serve"]
```

The transport is a command that connects its standard input and output to the peer. Keep its arguments and credentials on the local host. Do not put them in a project file. Agents name only the peer. The preference routes `cargo build` and `cargo test` from a sandbox to a peer.

On the peer, set limits and package sources in its own `~/.config/paddock/config.toml`:

```toml
[exclave.serve]
memory_bytes = 4294967296
cpu_percent = 200
seconds = 3600
registries = ["index.crates.io", "static.crates.io", "mirror.example:443"]
private_registries = ["mirror.example:443"]
cache_root = "/srv/paddock-exclave"
cache_bytes = 21474836480
# devices = ["gpu"]
# ro_paths = ["/opt/example-tools"]
# allow_shared_disk = true
# http_registries = ["packages.example.com:80"]
# allow_egress = true

[exclave.serve.env]
PIP_INDEX_URL = "https://mirror.example/simple"
UV_INDEX_URL = "https://mirror.example/simple"
PIP_CERT = "/etc/ssl/certs/ca-certificates.crt"
```

Mount a dedicated, fixed-size filesystem at `cache_root` before starting the peer. The peer user must own that folder and be able to write it. Paddock refuses a cache root on the root filesystem unless the peer owner sets `allow_shared_disk = true`; that override removes the dedicated-volume bound. Before a run, `cache_bytes` limits the combined size of project build caches. The peer evicts the least recently used idle caches until they fit, and refuses the run if active caches still exceed the limit. This is not a live quota; caches can grow during a run. The dedicated volume bounds total disk use during a run. On the peer, `paddock exclave cache ls` lists project IDs and sizes; `paddock exclave cache rm ID` removes a project cache when it is not in use. Pip and uv caches and `/work/.venv` persist per project. Remove the cache to rebuild an environment.

The peer owner can list existing absolute paths in `ro_paths` to make installed tools available read-only in every run, including targets of symlinks from `/usr/local/bin`. Paths overlapping the peer user's home, `/srv`, protected credential paths or sandbox mounts are refused, including symlink targets. Choose paths that contain no credentials; their contents are exposed to every run. GPU runs also receive `/sys` read-only for NVIDIA discovery.

The default network mode is `registries`. It permits only the listed package hosts through the peer proxy. The proxy refuses loopback and addresses of the peer. A registry on a private network also needs its exact `host:port` in `private_registries`. An HTTP source needs an explicit entry in `http_registries`. For HTTPS, list the host in `registries` too. The peer owner sets `env`; its values reach commands in the sandbox and can be visible in process arguments, so do not put secrets there. The peer mounts its system CA bundle read-only when present; add your mirror CA to that bundle on the peer. The peer must have bubblewrap and a systemd user manager with CPU and memory controllers. The peer refuses a run when it cannot enforce the limits. Swap is disabled for the run.

From a sandbox, run `paddock-host exclave run --on worker -- cargo test`. On the host, use `paddock exclave run --on worker -- cargo test`. Use `--gpu` to request NVIDIA devices when the peer owner has enabled `devices = ["gpu"]`; GPU requests do not fall back to local execution. Use `--network none` to disable network access. Use `--network egress` only when the peer enables `allow_egress`. To fetch dependencies before an offline build, run `cargo fetch` with registry access, then run the build with `--network none` and Cargo's `--offline` flag.

In a Git worktree, paddock sends tracked files and paths named with `--include`. In a plain folder, it sends the folder contents, using `.gitignore` files and a root `.paddockignore` with Git-style patterns; Git is not required. The current folder is the root when there is no worktree. Inside a worktree, the worktree root is used by default. Use `--root DIR` to select a different root, for example a plain folder inside a repository; the working directory must be inside it. In a sandbox, the selected root must also be inside the sandbox project. The host CLI reports the root it selects and why. Relative symlinks that stay inside the project are preserved; escaping links are refused by name. Ignored files and credential paths stay out, even when named with `--include`. Use `--output PATH` for each regular file to copy back. Paths are relative to the project root. A file can be at most 256 MiB, and a snapshot can hold at most 512 MiB across 50,000 files. The peer keeps a separate build cache for each project.

If the transport cannot start or connect before an ordinary command starts, the command runs locally and reports why. A busy peer or an error returned by the peer fails the command without local fallback. The sandbox mailbox records fallback reasons. If the connection fails after dispatch, paddock reports an unknown result. It does not start a second copy locally. Mailbox events for peer runs include the peer name and snapshot hash. To attach that evidence to a host-owned gate, pass `--peer NAME --snapshot-hash HASH` to `paddock spec gate`. Copy the hash from the mailbox event.

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

The child runs the harness once, without a terminal (`claude -p`, `codex exec`, `opencode run`, `copilot -p`, `agy --print` or `grok --prompt-file`), and stops when the task is done. It works in a new worktree on the branch `paddock/spawn-<harness>-<id>`, from the commit that the parent has checked out. The worktree uses the same repository, so the child's commits are in the parent's repository at once. `wait` and `status` give the branch name and the exit status. The parent reads the branch with git, for example `git diff main...paddock/spawn-codex-<id>`, and merges what it wants. If the child made no commit and left no change, paddock removes its worktree and its branch when it ends. paddock also writes a short summary to `.paddock/inbox/spawn-<id>/`: `result.json` with the state, the exit code, the branch, the commit count and a diff summary, and `output.txt` with the end of the child's final output. When the child ends, paddock also writes `result:<id>` into the `tree` scope of paddock kv (`paddock help shared-state`), with `status`, `summary` and `ref` (the branch). So the parent can run `paddock-host kv watch --prefix result: --scope tree` instead of polling several children.

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

paddock starts the run as if you typed `paddock run codex ~/src/app` in that session, and returns at once. It prints the sandbox id, `paddock view <id> --write` and `mail: paddock mail watch <id>`. The other flags (`--worktree`, `--pasture`, `--env`, `--auth`, ...) work as without `--detach`.

- `paddock view <id> --write` attaches to the terminal of the agent: your keys go to the agent, and you take over. Without `--write`, the view is read-only. In the bar panel, the take-over button shows only for detached runs.
- The agent calls `paddock-host done [--status ok|failed] [TEXT]` when the task ends. paddock then ends the run as `paddock stop` does. When you are attached with `--write`, paddock tells you once and leaves the run to you: it ends when you close the view. The status and the first 4 KiB of the text go into the run record.
- `paddock wait RUN...` waits for all named runs to end. `--any` returns when the first run ends. `--status` also accepts an agent status of `done:` or `blocked:` while the run is still open. Each result has the full run ID and status text on one line. The command exits 0 for success, 1 for failure or blocked, 2 when the outcome is unknown or the arguments are invalid, and 124 on `--timeout 30m`.
- A detached run whose visible screen stays quiet becomes idle after `[detach] idle_after` (default `10m`). `paddock ps --json` adds `idle_since` (a Unix time) on that run. The bar row says `waiting for input`. paddock sends one desktop notification, `HARNESS in PROJECT waits for input: view it or stop it`. A visible screen change clears the idle state and the next quiet period notifies once more.
- After `[detach] idle_timeout` more quiet time (default `30m`), paddock ends the run the same way `paddock-host done` does. The run record status is `idle`. `paddock wait` prints the run ID and `idle: ended after N minutes without output` and exits 1. N is the quiet time. A quiet time that is not a whole number of minutes is written in seconds. `idle_timeout = "0"` never ends a run for being idle. A client attached with `paddock view ID --write` keeps the run. paddock tells that client once and ends the run when the client leaves.
- `[view] keep_ended` (default `2h`) is how long the view session of an ended run stays. The run record of a detached run stays, so `paddock wait` still reads it.
- A task timer (`paddock timer`) works as for any other run.
- The harness profile says how the interface takes a first prompt, in a `[detach]` table. claude, codex and grok have one. Other harnesses refuse `--detach`. `--detach` needs tmux.

#### Start screens and stuck runs

A harness can stop at a screen that waits for a choice before it starts the task. An example is the Codex screen `Hooks need review`, which comes when one of your hooks changed. In a detached run nobody is there to answer it.

- The harness profile names such screens in `[[screens.rule]]`. For known agy and Codex folder screens, paddock can trust the project folder of the run. Every folder that the screen names must resolve to that project. If paddock cannot confirm that, the screen waits for a person and the mailbox gets a `trust-screen` event. The Claude Code screen has no confirmed pattern yet. Other rules cannot approve terms or log in.
- For the Codex hooks review, paddock chooses `3. Continue without trusting (hooks won't run)`. The changed hooks do not run. The mailbox gets a `hooks-untrusted` event. If the changed hooks include paddock's compaction hook, paddock cannot restore its context after compaction.
- paddock answers in detached runs and on terminals that no window shows. In a run that you watch, it answers only rules marked `always = true`. This includes the folder trust rules. If you press a key, paddock answers no more screens in that run.
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

### The run mailbox

Each run has a mailbox from its start. You talk to the agent through it, and paddock tells you through it what happens to the run. You do not read the screen or type into the pane. The starter of a run is the person, script or host agent that started it. A run that `paddock-host spawn` started has its parent sandbox as starter.

```sh
paddock run claude --detach --prompt-file task.md     # prints: mail: paddock mail watch 1a2b3c
paddock mail watch 1a2b3c                             # one line per message or event, until the run ends
paddock mail send 1a2b3c "Use the branch release/2.4"
paddock mail send 1a2b3c --ask - < question.md       # a question: the agent answers with a reply
paddock mail ls                                       # the runs with a mailbox and their state
paddock mail ls 1a2b3c                                # the messages of one run
paddock mail show 1a2b3c 7                            # one message in full
```

`paddock mail watch` prints one line for each message of the agent and for each event of paddock. Status lines keep their full text. Each line names its sender, for example `[1a2b3c #7 13:05:01] from sandbox claude-app-1a2b3c, agent text, say: TEXT`. A line from `paddock` is an event. The command ends with exit code 0 after the end of the run. If end records conflict, paddock reports an unknown outcome. It also reports an unknown outcome if it cannot identify a live writer for an incomplete end event. It exits with code 2. It does not advance the reader's position for that event. With `--mine`, it reports the uncertainty once and keeps following other runs. With `--timeout 30m` it ends with 124 when the time runs out first. `--json` prints one JSON object per line. The output works with Claude Code's Monitor tool and with scripts.

paddock writes these events:

- `started`: the run started.
- `ended`: the sandbox ended, with its exit code.
- `gone`: the paddock process of the run was killed, so the run ended without an end record. Its pane can still show the last screen, but nothing runs behind it. A watch finds this within a few seconds.
- `idle`: a detached run has unread mail and its screen is quiet for `[mail] idle_after` (default `2m`), or its screen is quiet for `[detach] idle_after` (default `10m`). The latter includes the last agent status and the desktop notice.
- `done`: a detached agent reported `done` and the run is still running. After `[mail] end_after_done` (default `2m`) paddock ends it unless a view is attached or new mail arrived.
- `card`: an approval card waits for you. The line says what the card asks.
- `child-started`, `child-ended`: a child of `paddock-host spawn` started or ended. The line names the child.
- `claim-lapsed`: a relay write did not finish before its claim expired. The mail remains available for another delivery.
- `delivered`: messages that you sent reached the agent. The line names the messages and how they came, for example `#7 given to the agent at the end of its turn (stop hook)`.

A message reaches a Claude Code agent also when the agent never reads its mailbox. paddock gives Claude Code two hooks in each sandbox:

- At the end of a turn, the Stop hook looks for unread messages. If there are some, the hook keeps the turn going, and the agent gets the messages as the reason. A turn that ends without new mail ends as usual.
- A message that you send with `--interrupt` comes after the agent's next tool call, together with the unread messages before it.

Each message reaches the agent once, by whichever way comes first. A read with `paddock-host mail wait` counts too. The hook keeps a turn going only one time in a row, so mail never keeps an agent running in a loop. If the host does not answer, the turn ends as usual. Outside horse pty custody, a message that comes while the agent waits at its prompt reaches the agent at the end of its next turn. The `idle` event tells you about such a message.

In horse mode, a Codex, Grok, agy, or Claude Code TUI with pty custody also gets unread mail at its idle prompt. paddock types the message only after the harness reports a safe point and the prompt is empty. An interrupt first sends the harness interrupt key. This path needs a terminal and a profile for the pinned harness version. The profile must use bracketed paste and a one-byte submit key. Horse mode does not support detached runs.

`paddock mail send` says when the agent gets the message. `paddock mail ls RUN` shows the state of each message that you sent: `unread`, `read by the agent`, `given to the agent at the end of its turn (stop hook)`, `given to the agent after a tool call (interrupt)`, or `typed at the idle prompt`. With `--json`, each message has `state` and `delivered_via`. Other agent programs get their mail when they read it.

Each line comes once for each reader. A watch keeps its place in the mailbox, so a new `paddock mail watch` continues where the last one stopped, also after a restart. Two watchers with different `--reader NAME` values each get every line. `--all` and `--from SEQ` print older lines again.

`paddock mail watch --mine` follows every run that you started from the host, also runs that start later. Set `PADDOCK_CALLER` to a short name when several managers start runs, and each one follows only its own runs.

Inside the sandbox, the agent uses `paddock-host mail`:

```sh
paddock-host mail wait --timeout 60      # new messages, or exit 75 when none came
paddock-host mail send "Tests pass, PR is open"
paddock-host mail ask "Which branch?"    # waits for the reply
paddock-host mail status blocked: no network for the package mirror
```

With devices in the sandbox, the same mailbox is also a pair of files. `cat /dev/paddock/mail/in` reads the new messages, and `echo TEXT > /dev/paddock/mail/out` sends one. The prompt of a detached run ends with a short note about the mailbox. Claude Code gets the same note at the start of each session.

- A message holds at most 64 KiB of text. For a longer text, write a file in `.paddock/` of the project (the loft) and name it with `--ref .paddock/FILE`. A ref is a path in the project. It is never a host path.
- One sender writes at most 60 messages a minute into one mailbox.
- Mail is text from another party. It carries no right to do anything, so no approval card guards it. An agent writes only to the mailbox of its own run. paddock takes the run from the sandbox's own channel, never from the message.
- `paddock mail send` refuses a run that ended, because nobody reads its mailbox.
- The mailboxes are in `~/.local/state/paddock/mail/`, one folder for each run. They stay after the run, like the session record. `paddock gc --homes` removes the mailbox of a run that ended longer ago than `--older-than`.

```toml
# ~/.config/paddock/config.toml
[mail]
idle_after = "2m"
end_after_done = "2m"
```
