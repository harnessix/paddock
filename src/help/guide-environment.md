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

The file needs `/dev/paddock` (FUSE on the host). Without it, the sandbox starts without the credential and paddock says so. On macOS the file is a FIFO at `$PADDOCK_DEVICES/secret/NAME` (see On macOS (`paddock help device-guide`)):

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

### tack: secrets that only allowlisted programs receive

tack keeps a secret such as `GH_TOKEN` out of your shell startup files and out of the environment of your processes. A process sees a reference, `tack://GH_TOKEN`, in place of the value. The reference holds only the name. It holds no part of the value and no hash of it. tack works on the host, inside and outside a sandbox.

This version stores the values, writes the references, and checks the configuration. With the service tackd installed on Linux, it also starts the allowlisted programs with the real values: `gh` gets the value of `GH_TOKEN` when you type `gh`, and every other process sees the reference (see tackd (`paddock help environment`) and starting programs (`paddock help environment`)). tackd keeps an audit log of each decision and each start. Each allowlist entry has a level of protection (see protection levels (`paddock help environment`)). At `protect = "uid"`, the program runs as a helper account of your user, and your other processes cannot read its environment or memory. At `protect = "refs"`, the program runs as you, and the only promise is that shells, startup files and other programs see references: other processes of your user can still read the program's environment while it runs. Without tackd, and on macOS for now, every process, `gh` too, sees the reference and not the value.

The values stay in the stores that you already have:

- `backend = "keyring"` (the default) is the system keyring. On Linux, paddock uses `secret-tool` (package libsecret) and a Secret Service such as gnome-keyring or KeePassXC. On macOS, it uses the login keychain through `security`. The items have the service `paddock-tack` and the name of the secret.
- `backend = "command"` is a password manager command that prints the value, as `from = "command"` of `[credentials]` does. paddock only reads it.

Only the user configuration, `~/.config/paddock/config.toml`, can have the `[tack]` table. paddock refuses a project `paddock.toml` that names it, because an agent can edit the project.

```toml
[tack.secrets.GH_TOKEN]
hosts = ["api.github.com", "github.com"]   # optional

[[tack.secrets.GH_TOKEN.allow]]
program = "/usr/bin/gh"
sha256 = "..."      # optional: sha256sum /usr/bin/gh
# signer = "..."    # optional, macOS: the signing identity
# args = ["pr *", "api **"]   # optional: the allowed argument shapes
# protect = "uid"   # or "refs"; gh and the git credential helpers default to "uid"
# write = true      # protect = "uid" only: the program may write the working folder

[[tack.secrets.GH_TOKEN.callers]]
kind = "sandbox"    # or "agent", or program = "/path"
action = "refuse"   # or "ask"; for a kind also "allow"

[tack.secrets.OPENAI_API_KEY]
backend = "command"
command = ["op", "read", "op://Private/OpenAI/key"]
```

An allowlist entry names a program by its absolute path. paddock refuses shells, interpreters (`python`, `node` and others), `env`, `printenv` and `sudo`, also through a symbolic link: they run any command that they get, so they would give the value to that command. `args` lists the argument shapes that the program may get, word by word, as for host tools: `*` is one word, `**` as the last word is the rest, and neither matches a word that starts with `-`. Without `args`, any arguments are fine. tackd checks `args` and `callers`. It records `hosts` with each decision; the version that starts programs with the value enforces them.

To move a value out of your files, name the variable:

```sh
paddock tack import GH_TOKEN                      # print what would change
paddock tack import GH_TOKEN --file ~/src/app/.env --yes
```

- The command reads `~/.profile`, the bash and zsh startup files, the fish configuration, `~/.config/environment.d/*.conf`, and each file that you name with `--file`.
- It changes only plain lines, such as `export GH_TOKEN=value`, `GH_TOKEN='value'` or `set -gx GH_TOKEN value`. It names a line where the shell computes the value, such as `$(pass show gh)`, and does not change that line.
- Without `--yes`, it prints the plan and changes nothing.
- With `--yes`, it first writes the value into the store and reads it back. It changes a file only after the store holds the same value. Each changed file gets a backup under `~/.local/state/paddock/tack/backups/`, with mode 0600. The command prints the `cp` command that undoes the change. The backup holds the old value, so delete it when the new file works.
- If the files hold two different values, or the store holds a different value, the command stops for that variable. `--replace` overwrites the value in the keyring.
- A file that changes while the command runs is not written. A crash leaves each file whole, the old one or the new one. If you run the command again, it finishes the work.
- No output and no log line holds a value.

`paddock tack list` shows the secrets, their store, their reference and their programs. `paddock explain` shows what the sandbox sees for each secret. `paddock doctor` has a block for tack. The block shows if the store is reachable or locked, the plain values that are still in your files or in the current environment, and the allowlisted programs that are missing or whose content changed since you wrote their `sha256`.

#### tackd: the service that decides

tackd is a system service that runs as root. It answers one question: may this program, started by these processes, receive which secrets? On Linux it then starts the program with them, as you. It reads no value to decide. Install it once, and again after each paddock update:

```sh
sudo paddock tack setup     # install, or update to this paddock, and start;
                            # also the stand-ins of your user (below)
paddock tack status         # installed, running, version, socket
paddock tack check gh pr list   # what tackd would decide here; starts nothing
paddock tack audit          # your decisions and starts, newest last
sudo paddock tack remove    # stop it and delete every file it made
```

How tackd decides:

- The kernel tells tackd which user and which process asks. tackd reads only the `[tack]` table in `~/.config/paddock/config.toml` of that user, and only if that user owns the file. It never decides for one user with the configuration of another, and it gives nothing to root.
- The program counts by its real path, after every symbolic link. tackd computes its SHA-256 and compares it with `sha256`. On macOS it can also check the `signer`. If an entry has neither, the program must be a file that only root can change, in folders that only root can change. A program under your home or under Homebrew needs a `sha256`, because any process of yours could replace it. The message gives the hash to write.
- tackd walks the parent processes up to the first one. If a paddock sandbox (another pid namespace or root folder, bwrap, or Seatbelt) or a coding agent that paddock knows is among them, tackd refuses, unless a `callers` rule for that kind says `ask` or `allow`. A rule with a caller `program` applies when that program is among the parents. The strictest rule wins. The sandbox mark comes from the kernel, so a process cannot remove it. The agent mark uses program names: a process that detaches from its parents leaves that chain.
- `ask` comes back as its own answer. This version cannot ask you yet, so a start that needs asking is refused.
- A program on no allowlist gets the same answer whether it exists or not, and tackd does not read it.

The audit log has one line per decision: the time, the user and process, the program with its real path and hash, the parent processes, the names of the secrets, the decision and the reason. A start adds a line when the program starts (`started`, with its pid, its level of protection and the uid that it runs as, its cgroup, the ACL that tackd added, and the names of the variables that tackd dropped; `paddock tack audit --json` has the level and the uid as the fields `protect` and `run_uid`), a line when it ends (`ended`, with its exit code or signal), or a line when it could not start (`failed`). It never holds a value or an argument. It is root's file, `/var/log/paddock/tackd.log`, mode 0600. It rotates at 4 MiB and keeps four old files. `paddock tack audit` shows your own lines through tackd. If tackd cannot write the log, it allows nothing, and `paddock tack status` says so.

The service:

- On Linux it is the systemd unit `/etc/systemd/system/paddock-tackd.service`. It runs `/usr/local/libexec/paddock/tackd`, a copy of paddock that only root can change: root never runs a file that your user could replace. The unit keeps six capabilities: to read files, to read the process table of all users, to become a user and that user's groups, to send signals to the programs it started, and to move a program into your cgroup. `NoNewPrivileges` holds for tackd and for the programs that it starts. A started program inherits the namespaces and system call filters of the unit, so the unit has none: the program sees the file system and the network as it would from your shell. The unit file explains each option. A restart or an update of tackd does not stop the programs that it started.
- On macOS it is the LaunchDaemon `/Library/LaunchDaemons/io.paddock.tackd.plist`, which runs `/Library/PrivilegedHelperTools/io.paddock.tackd`. Its error output goes to `/var/log/paddock/tackd.err`.
- The socket is `/run/paddock-tackd/tackd.sock` (macOS: `/var/run/paddock-tackd/tackd.sock`). Every user can connect to it. tackd takes the user from the kernel and not from the request.
- `setup` refuses a service file that paddock did not write, and folders that a user other than root can write. It checks that the new copy runs and reports this version before it replaces the old one. If the check fails, the old tackd stays. After `paddock update`, `paddock tack status` and `paddock doctor` show `CHECK` until you run `sudo paddock tack setup` again.
- `remove` deletes the service file, the binary, the socket and the audit log (`--keep-audit` keeps the log), and the folders when they are empty. On Linux it also removes every ACL that tackd added, gives the files that a helper account wrote to their user, ends the programs that still run as a helper account, deletes the helper accounts and their groups, and deletes `/var/lib/paddock-tackd`.
- A client has 5 seconds to send its request. tackd serves 32 connections at once, and at most 6 of one user, so one user cannot block the others. A slow, too large or malformed request gets an error, and the other clients are still served. After a crash, the service manager starts tackd again within seconds.

Without tackd, tack works in rootless mode. Values stay out of shells, startup files and agent environments. A program that gets a value is not protected from other processes of your user. `paddock doctor` and `paddock tack status` say which mode you have.

#### Starting allowlisted programs with their secrets

A program does not ask tackd for its secrets: something must send its start there. paddock puts a stand-in first on PATH, one small script for each allowlisted program name:

```sh
paddock tack setup          # as you: the stand-ins and the PATH lines
sudo paddock tack setup     # tackd, and the same for the user who runs sudo
paddock tack run gh pr list # start one program through tackd explicitly
paddock tack remove         # as you: delete the stand-ins and the PATH lines
```

- The stand-ins are in `~/.local/share/paddock/tack/bin`. Setup puts that folder first on PATH with one marked line in `~/.profile`, `~/.bash_profile`, `~/.bash_login` and `~/.zprofile` (those that exist, and the one that your login shell needs), in `~/.config/fish/conf.d/paddock-tack.fish` when you use fish, and in `~/.config/environment.d/60-paddock-tack.conf` for the systemd user session. A paddock sandbox gets stand-ins of its own, first on its PATH. Log in again after setup. Run setup again when you add a program to an allowlist: it writes the new stand-ins and deletes the old ones.
- Typing `gh` runs the stand-in. It finds the next `gh` on PATH and asks tackd to start it. tackd decides as above, takes the values from your store (it runs `secret-tool` or your password manager command as you), and starts the program as you: your user and the groups of your shell, your umask, resource limits, nice value and cgroup, your working folder, and your environment with the real values in place of the references. It drops variables that load code into a program, such as `LD_PRELOAD`, `LD_LIBRARY_PATH`, `NODE_OPTIONS` and `PYTHONPATH`. It starts the file that it checked: it opens it, checks the hash again, and runs that open file. A script (`#!`) runs by its path.
- The program uses your terminal or your pipes directly: the stand-in hands its standard input, output and error to tackd. Ctrl-C, Ctrl-Z and a new window size reach the program. When the program stops, the stand-in stops too, and `fg` continues both. The program's exit code is the stand-in's, and a program that a signal ends, ends the stand-in with the same signal. If the stand-in is killed, tackd kills the program.
- Exit codes of the stand-in itself: 77 when tackd refuses (it prints the reason once), 69 when tackd is installed but does not answer or cannot start the program (a locked keyring, a program that changed), 70 when tackd went away while the program ran.
- Without tackd, in a sandbox, and on macOS for now, the stand-in runs the program itself with the references, and says so once in each shell session. A session whose first process is gone, such as a terminal that was closed while a program still runs, stays one session: the note does not come again. When paddock is gone, the stand-in script runs the next program of that name itself.
- Not covered: a program that something starts by its absolute path (`/usr/bin/gh`), a process with its own PATH, and a folder that comes before the stand-ins on PATH (a version manager that puts its folder first at each prompt). These get the references only. `paddock doctor` shows whether each allowlisted name reaches its stand-in in your shell; `paddock tack run PROGRAM` starts a program through tackd in every case.
- A program that tackd starts has no controlling terminal: it reads and writes yours through the descriptors, but cannot open `/dev/tty`, and a program that you send to the background with `bg` can still read the terminal. It cannot gain privileges through `sudo` or another setuid program. At `protect = "refs"`, the programs that it starts (its pager, its editor, `git`) get the values in their environment, as they would from a shell. At `protect = "uid"`, they get the references (see below).
- Why a PATH stand-in: every shell finds programs through PATH, so it works the same in bash, zsh, fish and the others, needs no root to set up, and touches no other process. The other ways reach further and break more: a `binfmt_misc` handler changes how every program of every user (root too) starts; an `LD_PRELOAD` hook misses static programs (`gh` is one) and any process that clears its environment, and loads into every process; a FUSE view of the program folders needs a mount for each user and blocks every start when it hangs.

#### Protection levels: uid and refs

Each allowlist entry has a level, `protect = "uid"` or `protect = "refs"`. `paddock tack status`, `paddock doctor` and `paddock explain` show the level of each entry and where it comes from. `paddock tack check` shows the level of one start.

- `protect = "uid"`: tackd runs the program as a helper account of your user. On Linux, a process cannot read the environment or the memory of a process of another uid, so your shell, your editor and your other programs cannot read the value from the program while it runs. This is the default of the built-in profiles for `gh` and the git credential helpers (`git-credential-*`): they talk to the network and write no project files.
- `protect = "refs"`: tackd runs the program as you. This is the default for every other program. The only promise is that shells, startup files and other programs see references, never values. Other processes of your user can still read the program's environment and memory while it runs (`/proc/PID/environ`, a debugger).

`sudo paddock tack setup` makes one helper account for each user whose configuration has an entry at `protect = "uid"`: `_tack_<user>` (or `_tack_u<uid>` for a long name), a system account with its own group, no password, no login shell, and a home of its own in `/var/lib/paddock-tackd/homes`. Run it again after you add such an entry: until then, the start fails with a message that says so. The list of helper accounts is `/var/lib/paddock-tackd/helpers.json`.

What a program at `protect = "uid"` can reach:

- Your working folder, through a POSIX ACL that tackd adds when the program starts. tackd adds it as you, so it changes only files that you own. The helper account can read the folder and every file and folder in it on the same file system. With `write = true` in the entry, it can also write there. The folders above get "enter" (`x`) for the helper account when others cannot enter them already. A folder that you do not own and that others cannot enter stops the start, with a message.
- Files that the helper account creates in the folder get your user in their ACL (from a default ACL), so you can read them and, as far as the program's file mode allows, write them. `sudo paddock tack remove` gives them to you.
- Its own home folder, the network, and the files that every user can read. It gets your standard streams, your terminal, your limits and your cgroup, as at `refs`.

The program file itself must be one that every user can run, such as a program in `/usr/bin` or `/usr/local/bin`. A program under your home folder is not, when others cannot enter your home: the start fails with a message that says so.

What it cannot reach:

- Your home folder and its settings (`~/.config/gh`, `~/.gitconfig`, `~/.ssh`), your keyring, your ssh agent, your session bus and your other folders. `HOME` is the helper's home. tackd removes `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, `SSH_AUTH_SOCK` and similar variables from its environment. The value of the secret is what the program uses to sign in.
- A working folder without `write = true`: `gh pr checkout`, `gh repo clone` and other commands that write files fail there. Set `write = true` for such a program.

More details:

- tackd adds the ACL to every file once. A start in the same folder later finds the ACL on the folder and does not walk it again. New files inherit the ACL. A file that you move into the folder keeps its own ACL. One grant changes at most 200,000 files and takes at most 100 seconds. The ACLs of a folder are the sum of the grants: after a program with `write = true` ran there, every program at `protect = "uid"` can write there.
- git refuses a repository that another uid owns. tackd adds `safe.directory = *` to the git configuration of the program's command line (`GIT_CONFIG_COUNT`). That covers only what the ACLs let the helper account read.
- A file system without POSIX ACLs (some network and FUSE file systems) cannot give the folder to the helper account. `paddock doctor` tries ACLs in your paddock state folder and says if they work.
- tackd records each grant in `/var/lib/paddock-tackd/acl.jsonl` before it starts the grant. `sudo paddock tack remove` uses this file to remove the ACLs, also after a crash.

The programs that a program at `protect = "uid"` starts get references, not values. tackd puts a folder first on the program's PATH, with one link for each program name on your PATH. When the program runs `git`, the link runs paddock, which puts the references back in the environment and then runs the real `git`. When the name is on your allowlist, such as `gh` that git runs as its credential helper, paddock asks tackd to start it, as a stand-in does. tackd decides with your configuration and runs that program as the same helper account, at `uid`. The limits:

- A program that starts another program by its absolute path (`/usr/bin/git`), or that gives a child its own copy of the environment from before, does not pass the link. That child gets the values.
- A child runs as the same helper account. It could read the program's memory if it tried. The link keeps the values out of the environment of children such as a pager or a hook that prints it, not out of a child that means harm.
