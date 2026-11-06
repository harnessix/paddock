# Devices in /dev/paddock

The user can turn on host devices. Each device is a file in `/dev/paddock` (on the srt backend: in `$PADDOCK_DEVICES`). `paddock-host device list` prints the catalog `/dev/paddock/.catalog`: each device, its request format and whether it is on. A device that shows as `off` does not work. Only the user can turn it on, on the host.

On macOS there is no `/dev/paddock`. When `PADDOCK_DEVICES_SOCKET` is set, the devices come over that socket: use `paddock-host device` for every request. A shell redirection to `/dev/paddock/NAME` fails there. Secret files are FIFOs: `cat "$PADDOCK_DEVICES/secret/NAME"` reads one, or `paddock-host device secret/NAME`. After a revoke the file is gone.

## Send a request

`paddock-host device NAME [REQUEST]` sends one request, prints the response and exits with a clear code. `-` as REQUEST reads it from standard input.

Without `paddock-host` (Linux only), one open is one request: open the device read-write, write the request, then read the response.

```sh
exec 3<>/dev/paddock/echo; printf 'hello' >&3; cat <&3; exec 3<&-
```

The user defines devices in the host configuration. The catalog (version 2) shows the format of each, and these fields per device:

- `shape`: `builtin`, `http` (a loopback service: the request is `METHOD PATH`, header lines, an empty line, then the body: `paddock-host device ollama "GET /api/tags"`), `exec` (the request is the standard input of a fixed command) or `serve` (a service of the user).
- `run`: where the program runs: `builtin`, `sandbox` (a sandbox of its own), `host` (on the host as the user), `remote` or `broker`. A device with `run: host` runs its program on the host as the user, outside every sandbox: it can do what the user's own shell can do, with the user's home, keys and network. Its `request` text begins with "Runs on the host as you". Use it only for what the user set it up for, and say in your reply that you used a device that runs on the host.
- `stream`: with `true`, the device answers as it goes: `paddock-host device` prints each piece when it comes, and a plain read returns bytes as the program writes them, in order, to the end of file. A failure after the first byte makes `paddock-host device` exit with 7 after what it printed (75 when the device says still waiting), and fails the next plain read with `EIO`. What was printed is what the device wrote, never more. A stream is read once, in order. With `false`, the response comes whole, and a failure is one exit code before any output.
- `grant`: how this sandbox got the device: `--device`, `pasture NAME`, `project PATH`, `harness NAME` or `all (no grant key)`.
- `state`: `on`, `off` (`how_to_enable` names what the user turns on or grants) or `unavailable` (`how_to_fix` names why).
- `instance`: the program of a `sandbox` device runs in a device sandbox of its own, one per agent sandbox. `stopped`: it starts at your first request. `running`: it answers. `failed`: its sandbox died three times within a minute, and every request fails with `EIO` until the user runs `paddock device restart NAME --sandbox ID` on the host; `how_to_fix` has the exact commands. Tell the user; do not retry in a loop.

The first request of a sandbox asks the user, unless the device says `mode = "allow"`. Exit code 3 of `paddock-host device` means that the user declined: do not send it again. Exit code 8 means that the device rejected the request: read its `request` text in the catalog and send what it names.

A paid device has a budget. EDQUOT from a plain read means that the budget is used up: tell the user, who raises it on the host.

## Build or test elsewhere

The user can give you a device that builds and tests your project on another machine, for example a Mac. The catalog names it; the worked example of paddock is the device `mac`, with `stream: true` and `caller: project:ro`. Its request is one line, `test` or `check`:

```sh
printf test | paddock-host device mac -
```

- The device copies your project as it is on disk when the request starts, and runs `cargo test` or `cargo check` there. The output comes line by line as the build runs. Do not edit files during the request.
- Exit code 0: the build passed, and the output is the cargo output. Exit code 7 after some output: the build failed (cargo exited non-zero), and the output you have is the whole output. Read it as you would read a local failure.
- Exit code 8: the request was not `test` or `check`.
- A second request while one runs waits for the first (`concurrency: 1` in the catalog). Send one at a time.
- The copy on the other machine is keyed by your sandbox id, and the machine keeps nothing of your keys. You cannot reach that machine in any other way: there is no ssh and no network in your sandbox.
- `instance: failed` in the catalog: the device cannot start. Tell the user the `how_to_fix` commands; do not retry.

## Wait and ask

- `wait`: `paddock-host device wait 2026-10-30T15:00:00Z --timeout 100` returns at that time. Give an RFC 3339 time, not a duration, so that every read waits for the same moment. `paddock-host device wait 30m` names the time to use in its message.
- `ask`: `paddock-host device ask "QUESTION" --choice yes --choice no --timeout 100` shows the user a question and prints the answer. Exit code 3: the user dismissed it. Ask only when you need a decision. A plain question in the conversation is often better.

Exit code 75: the timeout passed first. Send the same command again. For a question, paddock finds the open question, so the user sees one card. A plain read of a waiting device fails with `ETIME` ("Timer expired") instead.

`/dev/paddock/gw` is the gateway that `paddock-host` uses. Do not use it directly. Credential files in `/dev/paddock/secret/` are described in `SKILL.md`, under "Secrets".

`paddock-host device --help` gives the exit codes.
