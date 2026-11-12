## Privacy defaults

paddock turns off vendor telemetry in every sandbox. It uses the switches that each vendor documents. It sets them only inside the sandbox: as environment variables, or in the sandbox's own copy of a settings file. Your host configuration does not change. `paddock explain` shows each switch and the profile that sets it.

| Program | What paddock turns off |
|---|---|
| Claude Code | telemetry (`DISABLE_TELEMETRY`), error reports (`DISABLE_ERROR_REPORTING`), the session survey (`CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`) |
| Codex | analytics and the metrics exporter (`[analytics] enabled = false` in `~/.codex/config.toml`) |
| Copilot CLI | the telemetry that the CLI sends (`--disable-client-telemetry`) |
| Gemini CLI | usage statistics (`privacy.usageStatisticsEnabled` in `~/.gemini/settings.json`) and telemetry (`GEMINI_TELEMETRY_ENABLED`) |
| Grok Build | telemetry, usage metrics, crash reports, Mixpanel and trace uploads (`GROK_TELEMETRY_*`, `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING`) |
| opencode | automatic sharing of sessions from a project `opencode.json` (`share` stays `manual`). opencode documents no telemetry switch. |
| VS Code | usage data, error telemetry and crash reports (`telemetry.telemetryLevel: off`), and experiments |

dsh sends the session log with each request to the DeepSeek API (the plugin `session-log-deepseek`, on by default). paddock turns it off in every dsh run: it adds the row `enabled: false` for that plugin to the sandbox copy of `~/.dsh/cordis.patch.yml`, and `paddock run dsh` prints one line about it. The host file does not change. This switch does not follow `telemetry = "vendor"`. To keep the upload on, set this in `~/.config/paddock/config.toml`:

```toml
[profiles.dsh]
upload_session_logs = true
```

`DISABLE_TELEMETRY` also stops the feature flags of Claude Code. Some features need them, for example Remote Control. The feedback commands of the harnesses still work. They send data only when you use them.

Some settings belong to your account, and no program can change them. The use of your data for model training is one of them. The Antigravity telemetry switch is another. The Claude and ChatGPT desktop apps document no telemetry setting at all. `paddock doctor` prints, for each installed program, where you turn off training:

| Vendor | Where |
|---|---|
| Anthropic (Claude Code, Claude app) | [claude.ai privacy settings](https://claude.ai/settings/data-privacy-controls), "Help improve Claude" |
| OpenAI (Codex, ChatGPT app) | [ChatGPT data controls](https://chatgpt.com/#settings/DataControls), "Improve the model for everyone" |
| GitHub (Copilot CLI, Copilot in VS Code) | [Copilot settings](https://github.com/settings/copilot), "Allow GitHub to use my data for AI model training" |
| Google (Gemini CLI) | the [privacy notice](https://developers.google.com/gemini-code-assist/resources/privacy-notices) of your sign-in method |
| Google (Antigravity) | `/settings` in agy, "Enable Telemetry" ([docs](https://antigravity.google/docs/settings)) |
| xAI (Grok Build) | `/privacy` in grok, coding-data sharing ([docs](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md#related-settings)) |
| opencode | the data settings of the model provider that you use |

To keep the vendor defaults, set this in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change it, in either direction.

```toml
[privacy]
telemetry = "vendor"   # default: "off"
```

#### The training question in a harness's setup

Some harnesses ask at a new login whether the vendor can use your data for training, with the box already checked. Antigravity (`agy`) asks it on its "Terms of Service & Data Use" screen. The answer goes to your account, so no file can preset it. By default, paddock unchecks that box and presses [Done] for you. It does this only while the harness sets up, before your task starts:

- paddock gives the harness a terminal of its own and reads the visible screen. It acts only when the whole dialog matches a rule of the built-in profile, row for row. It sends one key, reads the screen again, and presses [Done] only when the box shows unchecked.
- paddock answers only this question. It never answers terms, login, folder trust, billing or tool approvals.
- A screen that differs from the rule, a resize of your terminal, or a key that you press on the dialog stops paddock. The screen is then yours. Keys on the login and theme screens before the dialog go to the harness as usual.
- While it waits for the dialog, paddock gives the harness the terminal width of the rule (49 columns for agy). The harness gets the full width back when the dialog is done. A terminal that is narrower gets no help.
- The audit log (`paddock log`) gets one line with the rule, the keys and the outcome, never the screen text. paddock prints the outcome when the run ends.

Choose the behavior in `~/.config/paddock/config.toml`. A project `paddock.toml` cannot change it.

```toml
[privacy]
training = "manual"   # default: "deny"
```

| Value | What paddock does |
|---|---|
| `deny` | It unchecks the training box in the harness's setup. |
| `manual` | It types nothing. A run without a terminal stops while the harness's setup can still ask, so that you answer it once in a terminal. |
| `vendor` | It does not touch the question. Use this to opt in. |

paddock skips this when the harness has finished its onboarding (`[state] done_flags`). The rules come only with paddock's built-in profiles. A profile in `harness.d` can name one with `rules = ["agy-1.4-data-use"]` in `[privacy.training]`.

### The machine identity

A sandbox does not show the identity of your machine to the programs in it. The host name is always `paddock`. What else a program can read depends on the login:

| | Shared login (default) | Isolated login (`--auth isolated:NAME`) |
| :--- | :--- | :--- |
| `/etc/machine-id` | your machine's | an ID of this login, the same at every start |
| Boot ID (`/proc/sys/kernel/random/boot_id`) | your machine's | derived from the login's ID: new after a reboot, different for each login |

A shared login is your own login from the host, so it keeps your machine's values: a login that you made on the host keeps working. An isolated login is another machine: two isolated logins cannot be linked to each other or to your machine. paddock makes the ID at the first start and keeps it in the folder of the login (`.paddock-machine-id` in `~/.local/share/paddock/auth/HARNESS/NAME/`). It never makes a new one at each start, because a login that shows a new machine at every start looks like a login copied to many machines. When you delete the folder of the login, the ID goes with it.

Hardware identity:

- The MAC addresses of your network interfaces are hidden. In network modes `none` and `egress`, the sandbox has network interfaces of its own. With `--gpu`, paddock mounts `/sys/devices` for the graphics drivers, and hides the network folders and the board and firmware names (`/sys/devices/virtual/dmi`) in it. Network mode `host` shares your network stack, so the sandbox sees your interfaces and their MAC addresses there.
- The CPU model in `/proc/cpuinfo` stays visible: programs read it to choose CPU features.

Each backend has limits:

- docker and podman show the machine ID as above, never the one of the image. They show your machine's boot ID, because container runtimes refuse mounts inside `/proc`. They also hide the board and firmware names.
- srt cannot put another file at a host path. With an isolated login, a program in srt reads no machine ID at all. srt mounts `/proc` itself, so the program reads your machine's boot ID. srt also hides the MAC addresses and the board and firmware names.
