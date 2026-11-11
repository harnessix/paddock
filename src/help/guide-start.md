## Start in 30 seconds

Install paddock first (see Install (`paddock help install`)), for example with `bin/install` from a clone of the latest release. Then:

```sh
paddock setup              # checks the packages, offers the shell commands, launchers and bar widget
cd ~/src/my-app
paddock claude             # Claude Code with full auto-approval, in a sandbox
```

> [!WARNING]
> By default, the sandbox inherits your host environment variables, except the ones that look like secrets (names with `TOKEN`, `SECRET`, `PASSWORD`, `API_KEY` and similar). A harness still gets its own login variables, such as `ANTHROPIC_API_KEY` for Claude Code. Cloud credentials (AWS, Azure, Google Cloud, Vault and others) do not pass unless you name them in `[env] pass`. Run `paddock env hide NAME` to drop a variable, `paddock env show` to see the list, and `paddock claude --env clean` to pass only an allowlist. See Environment variables (`paddock help environment`).
