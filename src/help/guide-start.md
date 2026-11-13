## Start in 30 seconds

Install paddock first (see Install (`paddock help install`)), for example with `bin/install` from a clone of the latest release. Then:

```sh
paddock setup              # checks the packages, offers the shell commands, launchers and bar widget
cd ~/src/my-app
paddock claude             # Claude Code in its own room: you approve each step
```

Your computer stays yours. You use the agent as usual and approve its steps one by one. The agent lives in its own room and cannot touch your keys or your other projects. Its changes land on a copy first, and you decide what goes back.

`paddock` alone, in a terminal, opens the home screen. There you pick an agent, a folder and the options, then start.

### Who approves the agent's steps

paddock has two modes. The sandbox is the same in both.

- **manual** (the default of a new installation): you approve the steps yourself, in the agent's own interface, as you do without paddock. The agent also asks its usual first questions, for example whether you trust the project folder.
- **auto** (an opt-in, for trust that you build over time): the agent approves its own steps. It runs commands and edits files without asking you. The sandbox is the boundary: the agent sees only the project and what paddock gives it.

An installation that ran agents before this change keeps auto, the mode that it used until now. At its next start, paddock writes `[run] approvals = "auto"` into `~/.config/paddock/config.toml` once, and prints one line that tells you how to switch.

Choose the mode for one run:

```sh
paddock claude --manual    # you approve each step (--no-yolo is the old name)
paddock claude --auto      # the agent works on its own for this run
```

Choose a default in `~/.config/paddock/config.toml`:

```toml
[run]
approvals = "manual"       # every agent

[run.agents.codex]
approvals = "auto"         # one agent that you trust
```

A project can set `[run] approvals` in its `paddock.toml` too. The order is: the command line, the project, the agent, your default, then the default of the installation (`manual` for a new one). An agent can edit the project, so `approvals = "auto"` in a project file needs your one-time approval. The start line and `paddock explain` show the mode of each run and where it came from. `paddock ps` shows the mode.

Start paddock in a project folder. It refuses `/`, your home folder, the folders above it, the system folders and its own data folders. A wide folder needs `--yes-wide-folder`. Documents, Downloads and the cloud sync folders are wide. So is a folder with `.ssh` or other keys. So is a folder of more than 5 git repositories. So is a folder with more than 200 000 files or 50 GB. To allow such a folder for good, add it to `[run] allow_wide = ["~/code"]` in `~/.config/paddock/config.toml`. `paddock doctor` lists these folders.

> [!WARNING]
> By default, the sandbox inherits your host environment variables, except the ones that look like secrets (names with `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY` and similar). A harness still gets its own login variables, such as `ANTHROPIC_API_KEY` for Claude Code. Cloud credentials (AWS, Azure, Google Cloud, Vault and others) do not pass unless you name them in `[env] pass`. Run `paddock env hide NAME` to drop a variable, `paddock env show` to see the list, and `paddock claude --env clean` to pass only an allowlist. See Environment variables (`paddock help environment`).
