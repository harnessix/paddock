## Use

```sh
paddock claude                 # Claude Code in the current project
paddock codex ~/src/api        # Codex in another project
paddock claude -- -p "fix the failing test"   # arguments after -- go to the harness
paddock claude --worktree      # work on a new branch in a new worktree, review the diff at exit
paddock worktree list          # the worktrees of --worktree runs and apps; merge or remove them
paddock shell claude           # a shell in the sandbox that claude gets
paddock explain claude         # every mount, variable and rule, and why it is there
paddock ps                     # running sandboxes
paddock files                  # files a sandbox saved outside the project
paddock doctor                 # dependencies, backends, located harnesses, training opt-outs
paddock doctor --escape        # try to escape, and report each attempt
paddock setup                  # first-run steps; a second run changes nothing
paddock update --check         # is there a newer release
paddock mcp claude             # MCP servers, and whether each runs in the sandbox
paddock env show               # which host variables pass, by name
```

A sandbox id looks like `claude-app-1a2b3c`. A command that takes a sandbox id also takes its last part, `1a2b3c`, or the start of that part with at least 4 characters, such as `1a2b`. paddock looks for the id among the running sandboxes, the recent ended runs, the sandbox homes of `paddock files` and the snapshots of `paddock undo`. When the start fits more than one sandbox, paddock lists their full ids. The hints that paddock prints show the short id. `--json` output keeps the full id in `id` and adds the short id in `short`.

A sandbox uses your existing login of the harness. See Logins and MCP (`paddock help agent-logins`) for a separate login.
