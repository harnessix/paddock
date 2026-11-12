## Network

| Mode | Behavior |
| :--- | :--- |
| `egress` (default) | Internet access that leaves the same way as host traffic. Host services and the LAN are not reachable. |
| `none` | No network. |
| `api` | Only HTTPS to the model API hosts of the harness profile and the domains of `--allow`. Meant for reviewers and agents that need only their model. |
| `allowlist` | Only the hosts of the list, the same list as `api`. On Linux it runs on the srt backend. On macOS it runs on Seatbelt with paddock's proxy (see Network mode `allowlist` on macOS (`paddock help network`)). |
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
