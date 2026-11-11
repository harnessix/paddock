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

A host command can run after each run ends. Add `run_end = ["/path/to/command", "argument"]` to `[hooks]` in the user configuration. The array is the command and its arguments. paddock starts it without a shell and does not wait for it. `PADDOCK_RUN_ID` contains the full run ID. `PADDOCK_RUN_STATUS` contains the latest agent status, the detached run result, or `exit: CODE` when no status exists. If the exit code is unknown, it contains `ended: unknown`. For a killed run, paddock starts the hook when it detects the loss.

`paddock explain HARNESS` lists each hook entry of the sandbox copy with its origin: paddock, your file, or left out and why.
