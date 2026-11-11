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

For Codex and Gemini, paddock can choose which configured servers start. On Linux it leaves `xcodebuildmcp` off by default. Set a per-server policy in the user configuration or project manifest:

```toml
[mcp.servers.xcode]
start = "always"  # keep this server eligible even on Linux

[mcp.servers.search]
start = "never"   # do not start this server
```

`auto` uses the platform default. `always` overrides paddock's platform filter; it does not turn on a server that the harness itself has disabled. `never` turns the server off. A project choice other than `never` needs approval, because it can override a user or platform restriction. `lazy` is reserved for harnesses that can defer starting the server process; paddock currently refuses it for these harnesses. Claude Code is not filtered yet; an explicit `never` or `lazy` policy for a configured Claude server fails clearly. For other harnesses, a matching explicit `never` or `lazy` choice is refused rather than ignored. Platform filtering is currently limited to Codex and Gemini.

For Codex, platform filtering inspects the user and root project configuration; it does not inspect nested `--cd` layers, selected profile files, plugin configuration, or alternate account configuration. `paddock explain` lists these limits. `never` is applied to discovered server names with a command-line override; an undiscovered name is not yet enforced.

`paddock explain` shows the startup decision and boundary requirements for each discovered server. A name defined in more than one source follows the harness profile's source precedence. Different names with the same configuration stay separate so their tool names remain available; `paddock explain` identifies them. Claude Code continues to use its own MCP configuration.

Some harnesses can keep MCP tokens in the system keyring instead of a file. The sandbox cannot reach the keyring. For Codex, set `mcp_oauth_credentials_store = "file"` to share the tokens.
