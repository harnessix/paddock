## Turnout: scheduled host checks

Turnout requires Linux and a systemd user manager. It runs host-owned jobs from `~/.config/paddock/config.toml`. A project `paddock.toml` cannot define a job. The check is a host shell command: only exit 0 with `work` on stdout admits the wake. Other output, a failure or a check timeout ends it without mail. Turnout does not call a model.

For example, to test new commits of a local branch, add a job to the user configuration (replace the project path):

```toml
[turnout.host-checks]
harness = "none"
trigger = "git"
project = "/home/demo/src/paddock"
branch = "integration"
poll_interval = "60s"
check = "echo work"
check_timeout = "30s"
suites = ["horse", "horse_e2e", "mail_sandbox", "mail_relay_sandbox", "mail_hooks_sandbox", "detach", "omarchy_agent"]
suite_timeout = "15m"
max_runtime = "2h"
max_mem = "4G"
quiet_hours = "23:00-07:00"
max_load = 8.0
```

Run `paddock turnout ls` and `paddock turnout show host-checks` to inspect the configuration, then `paddock turnout install host-checks` to install its user timer. `paddock turnout remove host-checks` removes it. Git polling is at least 60 seconds apart. The job tests the selected commit in a detached worktree, one suite at a time with `CARGO_BUILD_JOBS=4`, and saves the output under paddock's state directory. Runtime and memory are limited by a systemd user service. A report goes to `paddock mail ls turnout-host-checks`; set `mailbox` to use another mailbox. `paddock turnout log host-checks` prints health records and `paddock turnout health` summarizes them.

For a calendar timer, set `trigger = "timer"` and `schedule` to a systemd calendar expression such as `daily`. `jitter` is a duration; it defaults to one minute. A calendar wake waits for quiet hours and load limits to clear; its recovery service resumes that pending wake when the user systemd manager starts again. A git wake retries at the next poll. `paddock turnout run NAME` starts a wake manually. `paddock turnout pause NAME` stops a job, and `paddock turnout pause` stops all jobs, including later timer wakes. `paddock turnout resume` clears the pause. These commands run on the host and require user systemd for timers and budgeted work. Agent jobs are not yet supported.
