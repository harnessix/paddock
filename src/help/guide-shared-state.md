## Shared state: paddock kv

Agents need to keep state between runs and hand work to each other. paddock kv is a small key-value store for that. It lives on the host, in `~/.local/state/paddock/kv/`, with one SQLite file for each pasture, each run, each spawn tree and the `user` scope. Each sandbox reaches it through its own socket. The socket tells paddock which sandbox asks, so an agent cannot claim to be another sandbox. The paddock process of a sandbox opens only the files of the scopes that the sandbox can use.

Inside the sandbox, the agent uses `paddock-host kv`:

```sh
paddock-host kv put task:review '{"title":"review src/auth.rs","status":"todo"}' --scope pasture
paddock-host kv claim review --ttl 600 --scope pasture
paddock-host kv watch --prefix result: --scope pasture --timeout 300
```

### Pastures

Two agents share the `pasture` scope only when you put them in one pasture. Define the pasture in `~/.config/paddock/config.toml`, and start each member with `--pasture`:

```toml
[pastures.review]
members = ["claude", "codex"]
```

```sh
paddock claude --pasture review
paddock codex --pasture review
```

A pasture belongs to one project: `review` in another project is another pasture. A project `paddock.toml` cannot define pastures. A sandbox that you start without `--pasture` is a pasture of its own: nobody else reads what it writes there, and its data goes with its run. So two agents that you start in the same folder do not share state unless you say so.

The members of a pasture also share a folder, `/pasture` in each sandbox (`$PADDOCK_PASTURE` names it; the srt backend keeps it at its host path). It lives in `~/.local/share/paddock/pastures/`, outside the project, so a member can hand a file to another without a commit. `$PADDOCK_PASTURE_NAME` holds the name.

- `paddock pasture list` shows the pastures of the current folder, their running members and their keys.
- `paddock pasture stop NAME` stops every running member.
- `paddock pasture remove NAME --yes` deletes the data of a pasture: its paddock kv file and its folder. It refuses while a member runs.
- `paddock ps` lists the members of one pasture together and names the pasture.

Each member keeps its own network. A member can share a local port with the others:

```sh
# member A: a dev server on its own 127.0.0.1:3000
paddock-host pasture share 3000
# member B: the same port reaches the server of A
curl http://127.0.0.1:3000/
# member A
paddock-host pasture unshare 3000
```

- A helper in each member relays the streams through a Unix socket in that member's own folder. The others see the folder read-only, so they can connect and they cannot replace the socket. paddock starts the helper with the sandbox. It works with every backend and needs no change to the host network.
- The share is the lease `lease:port:3000` in the `pasture` scope. A second member that shares 3000 gets exit code 9 and the name of the owner. The helper of the owner renews the lease, so the port frees itself within 30 seconds after the owner ends.
- Only the ports that a member names leave its sandbox. The control ports of a harness stay inside.
- When another member already uses the port itself, it skips the port. `paddock-host pasture members` shows why, with each member, its harness and its ports.
- Limits: TCP only, ports 1024 to 65535, at most 16 shared ports per pasture.
- `paddock pasture list` on the host shows the shared ports and their owners.
- The shared folder `/pasture` stays writable by every member. Port sockets are not in that folder. A member cannot replace another member's socket. Members still share the files in `/pasture`. Put only agents that you would let edit each other's files into one pasture.

A member can also ask another member a question. The other member answers with one run of its own harness, in its own sandbox, with its own login. This is the bridge, and it is off until you turn it on for a pasture:

```toml
[pastures.review]
members = ["claude", "codex"]
bridge = true
```

```sh
# in the claude member
paddock-host pasture ask codex --prompt-file question.md --timeout 300
```

- The answer is the final text of the one-shot run (`[oneshot]` of the profile). The asker never sees the login of the other member.
- The question goes through the kv socket of the asker, a folder outside every sandbox, and the kv socket of the other member. Its helper runs the harness with the prompt on stdin or in a private file, never in the arguments of a process.
- Before the answer goes back, paddock compares it with the login secrets of every member: the token values in their login files (`state.auth`) and their delivered credentials. Spaces and case do not matter, and a match needs at least 16 characters, so short words never match. On a match, the asker gets a short refusal instead of the answer, and the audit log names the member and the file or credential, never the value. The same check runs on an error from the helper.
- Name the member by its harness, or by its sandbox id when two members run the same harness. `paddock-host pasture members` shows whether the bridge is on.
- Limits: one question at a time per member, questions and answers up to 256 KiB (the end of a longer answer), a helper error of at most 4 KiB (the end of a longer error), and a timeout of 300 seconds by default, at most `[spawn] timeout`. A member that is busy or frozen answers with exit code 9, one that ended with exit code 1.
- The audit log `kv-audit.jsonl` records who asked whom, the sizes, the time and the result. It never records the question or the answer.
- Only the user configuration turns the bridge on. A project `paddock.toml` cannot.
- The guard finds a login only as plain text. An answer that encodes it, for example in base64 or with other characters between its parts, passes. The guard stops an accident or a simple trick, not a member that works hard to leak. Put only members that you trust with each other's answers into one pasture with the bridge on.
- With the srt backend, a member can read the bridge folder outside the sandboxes. It holds the pending questions and hashes of 16-character pieces of the delivered credentials, never their values.

Members of one pasture often use one account, so they eat from the same limits. Give each member a ration:

```toml
[pastures.review]
members = ["claude", "codex"]

[pastures.review.rations]
claude = { quota = "weekly=1%", budget = "2h" }
codex  = { quota = "weekly=2%", hard = true }
```

- When a member starts, paddock sets a timer named `ration` in its own scope, as `paddock timer set` does. You own it, so the member asks you on the approval card for more.
- `quota` is a share of a limit window of the account, counted from the start. `budget` is a time. A ration needs one of them, or both. `hard = true` freezes the member when the ration is used up. Without it, the member gets a notice.
- When paddock has no usage numbers for the account, the ration counts only the time, and paddock says so at the start.
- `paddock pasture list` and `paddock-host pasture members` show each ration and how much of it is used.
- A member that used up its ration answers no bridge questions. The asker gets a refusal and exit code 3.
- Each key of `rations` names a member of `members`. A wrong key, quota or duration stops the start with an error that names the pasture.

A pasture can also have a workbench: one terminal that every member and you can use, and that belongs to no member. A command in the terminal of a member runs in that member's sandbox and can read its login. The workbench is a sandbox of its own: a shell with no login, no harness, no MCP servers, an empty home, the clean environment and no network. It sees the project, `/pasture` and the shared ports, and it can share ports too. It runs one tmux session. Turn it on for a pasture:

```toml
[pastures.review]
members = ["claude", "codex"]
workbench = true
```

```sh
paddock pasture workbench review --start     # start it
paddock pasture workbench review --attach    # attach your terminal
paddock pasture workbench review --stop      # stop it
paddock-host pasture workbench               # in a member: join it
```

- Every member and you can type into the workbench, and each one sees what the others run there. A member can read the output of your commands, and it can run commands that you then see. Type no secret into it.
- The first member that runs `paddock-host pasture workbench` starts it. In a terminal, the command attaches to it. Without a terminal, or with `--print`, it prints the tmux command. A member needs `tmux` in its sandbox to attach. When it has none, the output says so.
- The workbench stops with `--stop`, when you set `workbench = false`, and when the last member ends. When you start it before any member, it waits for them.
- The tmux socket lives in a folder of the pasture on the host, never in a shared `/tmp`. Each member sees this folder read-only at `/run/paddock/workbench` from its start, so a member that started earlier can join later. The workbench itself can write it. `$PADDOCK_WORKBENCH` names the folder (the srt backend keeps it at its host path).
- The workbench uses the backend of your configuration and the project folder of the pasture, not the worktree of a member. It gets no extra mounts, ports or host access from the project `paddock.toml`. Its home is emptied at each start.
- The audit log records each start, stop and join, and who did it. It never records what the terminal shows.
- `paddock pasture list` and `paddock-host pasture members` show whether the workbench runs.

The bar panel shows the shared data too. A "Shared data" row opens a list of the pastures, your `user` scope and the data of older versions, with their members and the number of keys and locks. A pasture opens to its keys, grouped by the part before the first `:`, with locks first: who wrote each key, when, and its size. A key opens to its value, shown as plain text. From there you can copy the value, delete the key or free a lock; each delete asks twice. The panel reads a value only when you open its key, and reads paddock kv only while it is open. Bind a key to `omarchy-shell harnessix.paddock.kv open` to open the shared data directly.

In a spawn tree, the parent writes the tasks into the `tree` scope, each child claims one with a lease and writes its result, and the parent waits for `result:`:

```sh
# parent
paddock-host kv put task:parser '{"title":"fix the parser","status":"todo"}' --scope tree
paddock-host spawn codex --prompt-file task.md
paddock-host kv watch --prefix result: --scope tree --timeout 600
# each child
paddock-host kv claim parser --ttl 600 --scope tree    # exit code 9: a sibling has it
paddock-host kv put result:parser '{"status":"done","ref":"paddock/spawn-codex-ID"}' --scope tree
```

Each key belongs to a scope:

| Scope | Who reads and writes it | Default |
| :--- | :--- | :--- |
| `self` | This sandbox only | on |
| `pasture` | The members of the pasture of this sandbox, or this sandbox only without `--pasture`. Each entry records the sandbox that wrote it. `project` is another name for it. | on |
| `parent` | Read-only: the `self` scope of the sandbox that started this one with `spawn` | only in a spawned sandbox |
| `tree` | The sandbox that you started and every sandbox that it started with `spawn`, and theirs. Each entry records its writer. | on as soon as a sandbox can use `spawn` |
| `user` | Every sandbox of every project | asks you at the first use |

An agent with a prompt injection can write a false tip into the `pasture` scope, and the other members read it. So paddock records the writer of every entry, and the client marks what another sandbox wrote as untrusted.

A spawned sandbox never gets more than its parent here either:

- It has no `pasture` scope. It shares through `tree`, and its parent decides what the pasture keeps.
- For `user`, it inherits "Allow for this session" from the sandboxes above it. Without that, it is refused, and it never asks you itself: one prompt injection must not turn into many questions. Use the scope in the root sandbox first, or set `[kv] user = "allow"`.
- The `tree` scope belongs to the root of the tree: the sandbox that you started. When the root ends, its children stop and the tree is read-only. The data stays for you to review (`paddock kv list --tree ROOT-ID`) until `paddock gc` removes it with the root's `self` data.

A write can depend on the current version of a key (`--if-version`). A key can expire (`--ttl`). `mget` and `mset` read and write several keys at once. `history` shows the last versions of a key. `watch` waits for a change of one key or of a prefix. A lease (`claim NAME --ttl SECONDS`) makes sure that only one agent does a task. paddock sets the owner of a lease from the socket. Only the owner releases it, or with `--force` a sandbox above the owner in its spawn tree (its parent, or the parent of that one), or you on the host. A sibling cannot. A lease expires at the end of its time. Keep values small: put diffs, files and logs in a git branch or a file, and store the reference. `paddock-host kv --help` in the sandbox lists the key names that agents share: `task:`, `lease:`, `result:`, `heartbeat:` and `note:`.

On the host, `paddock kv list` shows the pastures of the current folder and their keys. `paddock kv list --pasture NAME`, `paddock kv get KEY --pasture NAME`, `paddock kv delete KEY --pasture NAME` and `paddock kv release NAME --pasture NAME` read one of them. The pasture of a sandbox without one is `solo-<sandbox id>`. `--user`, `--sandbox ID` and `--tree ROOT-ID` choose another scope.

### Host scripts as principals

A host script or a CI job on the same machine can hand tasks to agents and wait for their results. It acts as a named principal from the user configuration:

```toml
[kv.principals.ci]
pastures = ["review"]       # pastures of the current project that it may use
write = true                # false: read and watch only
```

- `paddock kv put KEY [VALUE] --as ci --pasture review` writes a key (without VALUE, it reads standard input). `--ttl`, `--version` and `--project` work as for `paddock-host kv`.
- `paddock kv watch KEY --as ci --pasture review` waits for a change. `--prefix`, `--version` and `--timeout` work as inside a sandbox. Exit code 75 means "still waiting".
- `paddock kv claim NAME --ttl 60 --as ci --pasture review`, `paddock kv release NAME --as ci --pasture review` and `paddock kv delete KEY --as ci --pasture review` work on the pasture as that principal.
- Each entry records the writer `principal:ci`. Agents see that name, and read the entry as untrusted data, as every entry of a pasture.
- The first write of a principal to a pasture asks you on an approval card ("let `ci` write to the pasture `review`"). "Allow for this session" covers that principal and pasture until your login session ends. paddock keeps the grant in the run folder.
- paddock refuses a principal that is not in the configuration, and a pasture that is not in its list.
- A principal claims and releases only its own leases. To free the lease of a sandbox, use `paddock kv release NAME --pasture NAME` without `--as`.
- A principal has the same quotas as a sandbox in the pasture. The audit log records the principal, the pasture, the key, the operation and the size, never the value.

Older paddock versions kept one `project` scope for all sandboxes of a project, in `store.db`. Sandboxes no longer read it, because nobody chose to share it. `paddock kv list --legacy` shows it, and `paddock gc` removes it when nothing changed it for `keep_days`.

The limits and the `user` scope are in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change them.

```toml
[kv]
user = "ask"           # ask | allow | deny
max_value = 65536      # bytes per value
self_bytes = 4194304   # bytes per scope; also project_bytes (pasture and tree) and user_bytes
max_keys = 4096        # keys per scope
keep_days = 30         # paddock gc removes the files of a run, and the tree of a root, this many days after it ended
enabled = true
```

`paddock gc` removes the file of each run (its `self` scope and its own pasture), and the `tree` scope of spawn trees whose root ended, more than `keep_days` ago, and keys whose time to live passed. A named pasture keeps its data until you delete it. The audit log `~/.local/state/paddock/kv-audit.jsonl` records each write with its key name and size. It never records a value.
