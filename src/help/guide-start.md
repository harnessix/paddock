## Start in 30 seconds

Install paddock first (see Install (`paddock help install`)), for example with `bin/install` from a clone of the latest release. Then:

```sh
paddock setup              # checks the packages, offers the shell commands, launchers and bar widget
cd ~/src/my-app
paddock claude             # Claude Code with full auto-approval, in a sandbox
```

### Who approves the agent's steps

paddock has two modes. The sandbox is the same in both.

- **auto** (the default, and the one we recommend): the agent approves its own steps. It runs commands and edits files without asking you. The sandbox is the boundary: the agent sees only the project and what paddock gives it.
- **manual**: you approve the steps yourself, in the agent's own interface, as you do without paddock. The agent also asks its usual first questions, for example whether you trust the project folder.

Choose the mode for one run:

```sh
paddock claude --manual    # you approve each step (--no-yolo is the old name)
paddock claude --auto      # auto for this run, when manual is your default
```

Choose a default in `~/.config/paddock/config.toml`:

```toml
[run]
approvals = "manual"       # every agent

[run.agents.codex]
approvals = "auto"         # one agent
```

A project can set `[run] approvals` in its `paddock.toml` too. The order is: the command line, the project, the agent, your default, then `auto`. An agent can edit the project, so `approvals = "auto"` in a project file needs your one-time approval. The start line, `paddock explain` and `paddock ps` show the mode of each run.

> [!WARNING]
> By default, the sandbox inherits your host environment variables, except the ones that look like secrets (names with `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY` and similar). A harness still gets its own login variables, such as `ANTHROPIC_API_KEY` for Claude Code. Cloud credentials (AWS, Azure, Google Cloud, Vault and others) do not pass unless you name them in `[env] pass`. Run `paddock env hide NAME` to drop a variable, `paddock env show` to see the list, and `paddock claude --env clean` to pass only an allowlist. See Environment variables (`paddock help environment`).
