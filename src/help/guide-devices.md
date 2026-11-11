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

For a systemd scope, paddock sets `memory.high` to 2 GiB by default. This is a soft boundary: the kernel reclaims and throttles above it, and it is not a hard memory limit. Set `[memory] high = "4G"` in `~/.config/paddock/config.toml` to change it, or `high = "max"` to disable it. Spawned child scopes use the same setting alongside any `--mem` hard limit. A run without a systemd scope does not get this boundary. `paddock explain` shows the planned setting; `paddock ps --mem` shows the effective cgroup value when it is available.

When a detached run becomes idle, paddock asks the kernel to reclaim up to 1 GiB of its inactive file cache from its own verified scope. It does not freeze the run. The request normally uses `swappiness=0`; on kernels that reject that option it retries without the override, which can also reclaim anonymous memory. The run note says which request was used. Reclaim is best effort, so the kernel may reclaim less or none.

The memory savings from these changes have not yet been measured in a run started with this version. Compare runs with similar workloads using `paddock ps --mem`; file cache and swap can vary over time.

macOS has no cgroups: a Seatbelt run starts in a process group of its own, and `paddock freeze` sends SIGSTOP to that group and to every process under the sandbox process, again and again until it finds no new one, so a process that forks during the freeze cannot get away. `paddock thaw` sends SIGCONT the same way. A process that left both the group and the tree (with `setsid` after its parent ended) is not frozen, and a thaw also continues a process that the agent had stopped itself. The device instances of the run are not frozen with it: `paddock device ls` keeps showing them as running. When the run has your terminal, its group gets the terminal while it runs, and paddock takes it back at the end.

- A session of the macOS guest (`paddock app run` on macOS) is frozen with the pause of its VM: the id is `macvm-<app>`. See The marks and freeze (`paddock help macos`).
- `paddock stop` thaws a frozen sandbox first, so that it can end.
- Each freeze and thaw goes to the audit log, with the reason.
- The other members of its pasture see the state in paddock kv: `sandbox:<id>` in the `pasture` scope is `{"state": "suspended", ...}` while it is frozen and `running` after.
- The clock keeps running while a sandbox is frozen: a harness timeout or a model's prompt cache can run out.
- Leases in paddock kv follow `[suspend] leases` in the user configuration. With `keep` (the default), the leases of a frozen sandbox stop counting down and run again after the thaw with the time they had left; another agent that claims one gets a conflict with `owner_state: "suspended"`. If the sandbox ends while frozen, its leases count down again. This holds also when its paddock process was killed or the computer restarted: the next claim of such a lease finds no running holder, and the leases of that holder count down again. With `release`, paddock frees them when it freezes the sandbox, with one audit line per lease. The rule holds for every freeze: `paddock freeze`, a hard timer, and the gateway.
