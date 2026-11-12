## horse (experimental)

In horse mode, paddock keeps the sessions of a run in its own store on the host, `~/.local/share/paddock/horse/` (mode 0700). A horse is a group of sessions with one DNA, a set of traits. paddock reads each session from the native files of the harness and keeps every row, also the rows that the harness later removes or rewrites. During the run, you can queue messages for a session or fork it. You can also ask whether a change keeps the prompt cache of the model.

Horse content in the host store is authenticated and encrypted. The master key is in the host paddock keyring; each horse has a wrapped data key. Back up the master key to a private, secure location and retain it alongside your store backup plan:

```sh
paddock horse key export /secure/horse-master.key
paddock horse key import /secure/horse-master.key  # restore a matching key into an empty keyring
```

A missing or wrong key stops access without deleting the store. Native harness transcripts remain in the harness's own format. An existing plaintext store is never migrated automatically:

```sh
paddock horse migrate --verify       # copy and verify; retain horse.plain
paddock horse migrate --drop-plain   # warning: permanently remove the verified plaintext copies
```

Keep the plaintext backup until you have checked your encrypted store and saved a key backup. Migration refuses active runs, unsafe source files, and records larger than 256 MiB; it leaves the source intact on refusal. A failed migration can be retried.

Horse mode is off by default. Turn it on for one run, or for every run in the user configuration:

```sh
paddock run codex --horse                     # a new horse
paddock run dsh --horse-id h-1a2b3c4d5e6f     # continue a horse, resume its newest dsh session
```

```toml
[horse]
enabled = true
# dsh_model = "deepseek-flash"
# dsh_provider = "deepseek-official"
# dsh_effort = "high"
```

Horse mode works with the harness versions below. Before the run starts, paddock asks the harness for its version. If the version is not in this list, or the harness is not supported, the run fails and says why. This is also true with `enabled = true`. If you run other harnesses or versions, turn horse mode on per run with `--horse`.

| Harness | Version |
|---|---|
| dsh | 0.2.1-rc.5 |
| Claude Code | 2.1.317 |
| Codex | 0.166.0, 0.167.0 |
| Grok Build | 1.0.70 |
| agy | 1.4.12 |

Codex rollout reading supports 0.166.0 and 0.167.0. The codec preserves complete records byte for byte and records the first metadata row's producer version separately from the version used to read it. Later rows may have a different producer. A fork, swap or delivery refuses unknown or unverified records and nested variants, and reports the line. This conservative check also refuses some valid record types until they are validated. A swap refuses a rollout that changed since ingest.

The interactive Codex screen pattern is still limited to 0.166.0. With 0.167.0, the interactive run continues and its session is ingested when it ends, but safe-point message delivery stays unavailable until the screen pattern is verified. The versioned rollout fixtures use upstream types.

`--horse-id` works for dsh only. A detached run (`--detach`) cannot use horse mode yet.

### How paddock holds a run

In custody, paddock delivers your queued messages at a safe point and reads the session into the store after every turn. A safe point is a moment with no running turn. paddock never puts a message into a running turn.

A dsh run starts as `dsh --profile sdk` instead of `dsh web`, so there is no browser page. You type your prompts in the terminal, and paddock prints the answers. A prompt that you type during a reply waits until the reply is complete. Ctrl-D ends the run after the running turn and the queued messages. If the input ends, for example with stdin from `/dev/null`, paddock still delivers the queued messages, and then ends dsh. paddock prints a `horse:` line when the run waits for them. Press Ctrl-D a second time to end the run at once.

A child of `paddock-host spawn` with a task has no terminal (a headless child). paddock gives the task as the first message after the DNA message. It delivers queued messages after each turn. If the child is idle and its inbox is empty, paddock ends it. The output of the custody is the output of the child. An empty task fails before paddock writes to the store. Each harness runs in its own machine mode:

- Claude Code: `claude -p` over stream-json, with a session id that paddock picks.
- Codex: `codex app-server` on stdio. An approval request of Codex gets the answer that the plain run of the profile gives, never a wider one.
- Grok Build: `grok agent stdio` over ACP. Tool permissions get the answer of the plain run of the profile.
- agy: `--input-format stream-json --output-format stream-json`. paddock stops a running turn with SIGINT.

An interactive run keeps its terminal user interface (TUI). paddock relays the terminal through a pty that it owns (the pty relay), and types each queued message into it at a safe point. The hooks or the `notify` program of the harness tell the host about the start and the end of each turn. paddock adds them only to the sandbox copy of the configuration of the harness, and the host copy does not change:

- Claude Code: the `Stop` and `UserPromptSubmit` hooks, and a session id that paddock picks.
- Codex: the `notify` program in `~/.codex/config.toml`. Your own `notify` still runs after it.
- Grok Build: the `UserPromptSubmit`, `Stop`, `StopCancelled` and `StopFailure` hooks, and the `idle_prompt` notification.
- agy: the `PreInvocation` and `Stop` hooks in `.gemini/config/hooks.json`.

paddock types a message only if all three of these are true:

- The last hook says that the turn ended, and no newer user row is in the session file.
- The screen shows the empty input row of the TUI, and no row shows that a turn runs.
- The screen did not change for 1.5 seconds.

paddock sends each message as one bracketed paste, then the Enter key. It removes control characters from a message, so a message cannot send keys of its own. While paddock types, your keys wait, and they go to the TUI right after the Enter key. The screen patterns are in the `[horse.pty]` table of each profile. None of them comes from a recorded screen, so `paddock doctor` lists them as unverified. A wrong pattern only holds messages back.

If the user row of a message is in the session file, paddock marks the message landed. If a message does not land within 60 seconds of an idle TUI, paddock types it once more. After a second miss, the message stays recorded but not landed, and `paddock horse status` shows it so. Each step is one line in the device ledger: `pty.deliver`, `pty.interrupt`, `pty.retry` or `pty.lost`. If the screen looks idle but no hook ever came, the line is `pty.no_stop`.

If paddock cannot hold an interactive run, the run goes on, and paddock reads the session files into the store when it ends. Your queued messages stay in the inbox, and the run says why in one line:

```text
horse: no safe-point delivery for codex 0.166.0 in the TUI (the run has no terminal for paddock's relay); entries stay queued
```

These are the causes:

- The run has no terminal.
- The profile has no `[horse.pty]` table for the version.
- Your command sets what paddock sets itself: `--resume`, `--continue` or `--session-id` for Claude Code, or `-c notify=...` for Codex.

paddock also has a channel path for Claude Code 2.1.318. A channel is an MCP server, `paddock-host channel`, that pushes messages into a running session. A horse run needs Claude Code 2.1.317 today, so paddock does not use the channel yet.

On macOS, the relay, the hooks and the host requests are the same. The `run.end` event of each run names its custody: `live`, `pty`, `channel` or `final-only`. It also has the counts, and `error` if the custody failed.

### Host sessions

A host session is a session that a harness keeps on the host, outside a sandbox. paddock can read host sessions into the store, so they get the same keep-every-row store, encryption and forks as the sessions of a run. paddock never writes a host session file. It copies the file into a private folder in the paddock state folder, reads the copy and removes the copy. The copy is a reflink (Linux) or an APFS clone (macOS) where the file system has them, so it takes no extra space on the disk.

The paddock state folder is `$XDG_STATE_HOME/paddock`. Without `XDG_STATE_HOME`, it is `~/.local/state/paddock`, on Linux and on macOS. The paths below name it `<state>`. `paddock horse adopt --status` prints the path of the adoption state.

Import chosen host sessions once:

```sh
paddock horse import claude --list                         # the sessions that paddock can read
paddock horse import claude 01934b20-1111-7777-8888-123456789abc
paddock horse import claude --path .claude/projects/-home-me-src-app   # every session of one project
paddock horse import codex --all --horse h-1a2b3c4d5e6f
```

The harnesses are `claude`, `codex`, `grok`, `agy` and `dsh`. An id names a session, never a folder: the file name of the session (`ID.jsonl`, the end of a Codex rollout name, `ID.db`) or its session folder (Grok, dsh). paddock checks that the copy holds that session. A host session is known by its harness, its native id and its home, so `--home` with a copy of a home gives sessions of their own. New sessions go into one new horse, or into the horse of `--horse`. If a session is in the store already, an import adds the new rows of the host file to it. The rows that the store has stay, also when the harness removed or rewrote them. paddock reads only the harness versions of the table above. A session of another version does not come in, and the import says why. The import exits 1 when a chosen session did not come in.

paddock reads only regular files below the transcript folder of the harness, for example `~/.claude/projects`. It does not follow a symlink.

### Adopt the host sessions of a harness

Adoption makes the host sessions of a harness join horse from then on. Nothing happens before you run it:

```sh
paddock horse adopt claude                                  # the whole transcript folder
paddock horse adopt codex --path .codex/sessions/2026       # one location
paddock horse adopt --status
paddock horse adopt --sync                                  # read new and changed sessions once
paddock horse adopt --watch --interval 30                   # the same, every 30 seconds
paddock horse adopt --off claude                            # end it; the sessions stay in the store
```

The first adopt of a harness makes one horse for its host sessions and reads every session of the locations into it. After that, each sweep reads the sessions that are new or changed since the last sweep. A session that paddock cannot read is read again when it changes. `--status` shows them. The adoption state is in `<state>/horse/adoption.json`. A second `paddock horse adopt HARNESS` adds its locations, and it puts the hook entries in again when the paddock program of the entries moved or went (for example after an upgrade). `--status` says so when the program is gone.

### Live host sessions

An adoption also adds paddock's hook entries to the host configuration of the harness, so each turn of a host session reaches the store when it ends. With `--no-hooks`, paddock changes no configuration, and new sessions come in at `--sync` and `--watch` only.

| Harness | File | Events | What the hook does |
|---|---|---|---|
| Claude Code | `~/.claude/settings.json` | `SessionStart`, `UserPromptSubmit`, `Stop` | reads the turn, gives the DNA to a new session, delivers queued messages |
| Codex | `~/.codex/hooks.json` | `SessionStart`, `Stop` | reads the turn, gives the DNA to a new session |
| Grok Build | `~/.grok/hooks/paddock-horse.json` (a file of paddock's own) | `Stop` | reads the turn |
| agy | `~/.gemini/config/hooks.json` (the entry `paddock-horse`) | `Stop` | reads the turn |

dsh has no hooks. Codex asks you to trust the new entries at its next start. Grok Build runs hooks only in a folder that you trust.

For a Claude Code host session, `paddock horse append` works as for a run. paddock delivers the queued messages when the turn of the session ends: the `Stop` hook gives them to Claude Code, and the session goes on with them at once. If you type a prompt first, the messages come with your prompt. A message leaves the queue only after the hook gave its text to Claude Code. If the hook dies before that, the next event gives the message again. A fork and the DNA work for every host session, because they are operations of the store.

A hook must be quick. The `Stop` and `SessionStart` hooks read only the session of the event (the `transcript_path` of the hook input), and the `UserPromptSubmit` hook reads nothing, so your prompt does not wait. A hook waits at most 5 seconds for another sweep, such as a large first adopt or `--sync`, and reads for at most 10 seconds. What it leaves, the next sweep reads. paddock finds the host session of a hook in a small index, not by reading every session of the store. The hook entries name `paddock` by its path on `PATH` when that leads to the running program, so an upgrade that moves the versioned file does not break them.

paddock changes the hook file by these rules:

- Before the first change, paddock copies the file to `<state>/horse/hooks-backup/`. It records the entries in the adoption state before it changes the file, so `--off` knows them even after a crash.
- Each entry of paddock runs `paddock horse hook ... --mark paddock-horse`. The marker and the exact command identify the entry.
- paddock adds its entries after your entries of the same event, so each of your entries keeps its position. Codex keys its trust of an entry by that position.
- paddock refuses a file that is a symlink, that is not a JSON object, or that already has entries with the marker. Then nothing changes, and the adoption does not start.
- paddock never writes your file again in its own layout. It inserts its entries as text, in the layout around them (the indent of the neighbours, or one line when they are on one line). Every other byte of the file stays: the order of your keys, your spaces and your line breaks. paddock checks that before it writes.
- `--off` cuts out exactly the text that paddock added, and the lists and objects that paddock made for it. If you moved paddock's entries, `--off` finds and cuts them, and every other byte stays. If you did not edit the file, it is the backup again, byte for byte. If paddock made the file and only its entries are in it, the file goes. If paddock cannot change the file, the adoption stays on, so you can run `--off` again.

A hook never fails the harness. Its errors go to `<state>/horse/hook.log`. A run in a sandbox does not get paddock's host hook entries.

### Runs that continue a host session

A run with `--profile default` starts from a private snapshot of your host data (see `paddock help history`). In horse mode, the run can continue a session of the host. At the end of the run, paddock reads the host session from the snapshot as it was before the run, into a host session in the store. If the store has the host session already, from `paddock horse import` or `paddock horse adopt`, paddock uses that session and does not change it. The session of the run is a fork of the host session at the last row that both have. Its lineage starts with the lineage of the host session. When the host session is in a horse of its own (an import or an adoption), the session of the run joins that horse at the end of the run, so the host session and its continuation are in one horse. The session keeps the run as its origin (`origin_run`): the rows after the fork node are the rows of that run. The DNA of the run's horse does not graft into the host horse.

None of this can stop a run. If paddock cannot make the snapshot copy for the lineage, cannot read the host session, or cannot record the lineage, it logs why, and the run ends as usual without the lineage.

The host session in the store gets none of the rows of the run, and the host file does not change. So the store and the host history agree: the host has the rows up to the fork node, and only the session of the run has the rows after it. The run prints one line for each host session that it continued:

```text
horse: s-1a2b3c4d5e6f continues the host session s-7e6d5c4b3a21 (claude 01934b20-...) after row 12; its 4 new row(s) are in the store only, the host history has none of them
```

`paddock horse list` marks a host session with `host session` and the file it came from.

### Queue a message

On the host:

```sh
paddock horse status                                   # live sessions, heads, inboxes, guard verdicts
paddock horse append s-1a2b3c4d5e6f --text "The tests pass on main now."
paddock horse append s-1a2b3c4d5e6f --role note --text "Use the staging database."
paddock horse append s-1a2b3c4d5e6f --text "Stop, read the new spec first." --interrupt
```

A message waits in the inbox of the session. paddock delivers it at the next safe point, as the newest message. The roles are `user`, `system`, `developer` and `note`. A harness that takes only user messages gets the other roles as a user message with a short role tag. `--interrupt` marks a message as urgent: in a TUI, paddock stops the running turn with the interrupt key of the harness, and delivers the message first. dsh has no cancel, so a dsh turn always runs to its end.

### The prefix guard

A model provider keeps the start of a prompt in a cache, so the next request is cheaper and faster. The prefix guard compares what the model sees before and after an operation. It warns if one of these loses the cached prefix: a fork, a message that is not at the end, a change of model, effort, tools, system prompt or cwd, or a long pause. The warning shows the size of the lost part and the cost. Where a message at the end can do the same, it shows that form.

```sh
paddock horse guard s-1a2b3c4d5e6f --model deepseek-v4-pro
paddock horse guard s-1a2b3c4d5e6f --fork-at 3f9a2c
```

For Claude Code, the guard reads the request shape from the transcript. It says if the cache is warm (under 5 minutes), maybe warm (under 1 hour) or cold. Codex keys its cache by the thread id, so a fork under a new id starts cold. For agy, the cache scope is unknown, and a fork with a new conversation id counts as a cold start.

### Forks and swaps

```sh
paddock horse list                                     # every session in the store
paddock horse log s-1a2b3c4d5e6f -n 20                 # the last 20 nodes of a session
paddock horse fork s-1a2b3c4d5e6f --at 3f9a2c          # a new session from that node
```

A swap continues a Codex or Grok Build session at another branch under the same id. The prompt up to the fork point stays cached. The old head stays in the store as a session of its own:

```sh
paddock horse swap s-1a2b3c4d5e6f 3f9a2c1b7d4e                  # a session that a headless run holds
paddock horse swap s-1a2b3c4d5e6f 3f9a2c1b7d4e --interrupt      # stop the running turn first
paddock horse swap s-1a2b3c4d5e6f s-7e6d5c4b3a21 --codex-home ~/old-sandbox-home/.codex
paddock horse swap s-2b3c4d5e6f7a 4c5d6e7f8a9b --grok-home ~/old-sandbox-home/.grok
```

If a headless run holds a Codex session, the swap waits until no turn runs, and Codex loads the thread again. An interactive Codex run cannot load its thread again, so paddock refuses the swap until the run ends. If no run holds the session, name the home folder of the harness. If the guard says that the swap loses cached tokens, add `--yes`.

A warm side branch is a fork of a held Codex session that keeps the cache key. Codex writes no file for it, so paddock stores its turns from the live channel and marks them `live` in `paddock horse log`. paddock cannot write such a session back as a native file. The parent session waits until the side branch closes:

```sh
paddock horse fork s-1a2b3c4d5e6f --warm      # prints the new session, for example s-9c8b7a6d5e4f
paddock horse append s-9c8b7a6d5e4f --text "Try the other approach."
paddock horse fork s-9c8b7a6d5e4f --close     # or the run ends
```

### Hitch mode

A run in horse mode can start a child that joins its horse as a new session (a hitched child):

```sh
paddock-host spawn claude --prompt-file task.md --hitch
```

The result of the child goes into the session of the parent at its next safe point, once: from `paddock-host done TEXT` of the child, or from its end, whichever comes first. A run does not end while one of its hitched children still owes its result, and it prints a `horse:` line that says so. A plain spawn without `--hitch` makes a child horse with the DNA of its parent. `--hitch` outside horse mode is an error. A hitched child sees only its own session in `/dev/paddock/horse`, never the session of its parent or of a sibling.

### DNA

The DNA of a horse is a TOML document with traits, policies and capabilities. The capabilities are recorded only and grant nothing. The first message of every new session tells the model the DNA of its horse. Every change is an event, and the old DNA stays in the store.

```sh
paddock horse dna show h-1a2b3c4d5e6f
paddock horse dna set h-1a2b3c4d5e6f dna.toml          # hot-load
paddock horse dna history h-1a2b3c4d5e6f
paddock horse dna revert h-1a2b3c4d5e6f HASH           # back to a DNA from history
paddock horse graft h-1a2b3c4d5e6f h-6f5e4d3c2b1a      # add the traits of the second horse
paddock horse move s-1a2b3c4d5e6f h-6f5e4d3c2b1a       # move an idle session to another horse
paddock horse explain h-1a2b3c4d5e6f                   # where each trait comes from
```

```toml
[traits]
style = "careful"
```

- Inherit: a child horse of a plain spawn starts with the DNA of its parent.
- Hot-load: after `dna set`, each running session of the horse gets the new DNA as one `system` message at its next safe point. Inside the sandbox, pipe the DNA to `paddock-host horse dna put` to do the same.
- Graft: the first horse keeps its id, its sessions and its own trait values, and a different value of the second horse becomes an alternative. A different policy, or a capability that the first horse does not have, refuses the graft, and nothing changes. A move grafts the DNA of the old horse into the new one. `paddock horse guard SESSION --move HORSE` shows that a move keeps the cached prefix.
- Adapt: at the end of each run, three rules set the traits `runs`, `harnesses` and `reliability`.
- Explain: `explain` shows the origin of each trait, the DNA events and the lineage of each session. `paddock explain` shows the same for a run in horse mode.
- Revert: `dna revert` sets an earlier DNA from `history` again. The history stays.

### Published DNA (registry)

You can publish a DNA to an index and use it in other horses. An index is a folder on the host, or a git checkout of one. A reference has the form `registry://org/name@version`, `@latest` or `@sha256:HASH`. You keep the signing key, the trusted keys and the index list on the host only, in `~/.local/state/paddock/registry/`. The folder is 0700 and each file is 0600.

```sh
paddock horse registry key new                          # make the signing key, print its public key
paddock horse registry trust add acme 3f9a...e1 --name release   # trust this key for the org acme
paddock horse registry index add team /srv/dna-index    # add an index after the local one
paddock horse registry publish h-1a2b3c4d5e6f registry://acme/review@1.0.0
paddock horse registry resolve registry://acme/review@latest     # manifest, verdict, hash; no import
paddock horse dna use h-6f5e4d3c2b1a registry://acme/review@1.0.0
paddock horse registry sync team                        # git pull --ff-only, for a git checkout only
```

The default local registry is encrypted with the horse store. Configured external indexes keep their shared transport format; protect those folders separately.

A publish signs with the key of the host. If there is no key, run `paddock horse registry key new` first, or publish with `--unsigned`. A published version cannot change. By default, the trust policy refuses an unsigned manifest and a manifest that an untrusted key signed. `resolve` prints the verdict. If the policy refuses the reference, it exits 1. `dna use` grafts the DNA into the horse, with the same checks as a graft.

A sandbox in horse mode can ask for published DNA, but each request goes to you first:

```sh
paddock-host horse registry fetch registry://acme/review@1.0.0 --use
paddock-host horse registry sync team
```

If you deny the request, nothing is written, and the command exits 1 with `fetch of <ref> was not approved`. If you approve it, the host resolves the reference with its own indexes and trust policy. With `--use`, the host grafts the DNA into the horse, and the session gets the new traits at its next safe point. The answer to the sandbox holds the reference, the hash, the manifest fields and the verdict. It never holds a path, a key, an index name or the trust list. `sync` is the only network path of the registry, and it runs only after your approval. The device ledger and the audit log get one line for each request.

### The horse device

A sandbox in horse mode sees its own horse at `/dev/paddock/horse/self/`. Files that carry session or DNA content contain ciphertext. Names and IDs remain visible; `edges` and `instances` expose relationship and run metadata, and `ctl` is a command channel. Use the broker to read content deliberately:

```sh
paddock-host peek ls                      # list node selectors and metadata
paddock-host peek cat 0                   # content of node 0
paddock-host peek ls | paddock-host peek cat
paddock-host peek grep 'error'            # basic regular expression
paddock-host peek range 0 3              # inclusive node range
paddock-host peek dna
paddock-host peek binding
paddock-host peek inbox                  # queued entries of this run
printf '[traits]\nstyle = "careful"\n' | paddock-host horse dna put
```

The output is marked as data. Reads use the run's grant, are counted and logged, and stop when the grant is revoked or exhausted. A trainer grant selects its target with `--grant ID`. A closed output pipe exits cleanly. If the mount is unavailable, the run reports why.

A sandbox can also change the context of its own horse, one command per write to `self/ctl`. It needs your grant first. Without a grant, a write fails with EACCES:

```sh
paddock horse grant 1a2b3c --allow fork,append,view --commands 20 --tokens 50000   # on the host
paddock horse grants                                                              # what each grant used
paddock horse grant 1a2b3c --revoke
```

```sh
echo "fork s-1a2b3c4d5e6f" > /dev/paddock/horse/self/ctl          # inside the sandbox
cat /dev/paddock/horse/self/ctl                                     # the answer: the new session
echo "view s-1a2b3c4d5e6f errors grep:error" > /dev/paddock/horse/self/ctl
paddock-host horse ctl append s-1a2b3c4d5e6f note Check the logs.   # the same through the broker
```

The commands are `fork SESSION [AT_NODE]`, `append SESSION ROLE TEXT`, `move SESSION HORSE`, `view SESSION NAME FILTER` and `disconnect SESSION`. A view is the encrypted read-only file `self/views/NAME`, built from messages that pass the filter: `all`, `role:ROLE`, `last:N` or `grep:TEXT`. `disconnect` ends the custody of a session at its next safe point. `move` can go only to a horse that you named with `--horse`.

The prefix guard judges each command before anything changes. An append at the end passes at once. A command that breaks the cached prefix shows an approval card with the lost tokens and the cost. Its tokens come from the token budget of the grant, so with `--tokens 0` every such command is refused. A command works only on the sessions of the sandbox's own horse, and a hitched child only on its own session. A denied command fails with EPERM and changes nothing. Each command writes one line to the device ledger.
