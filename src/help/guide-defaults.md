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
- When you install another agent, or a newer paddock brings a profile, the next harness start updates the script in the background (see Keeping the menu and the shims current (`paddock help app-launchers`)).
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

- The arguments stay the same. paddock adds no auto-approval flags and removes none. It adds only the telemetry switches of the profile (see Privacy defaults (`paddock help privacy`)).
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
