## Spec Kit projects

A project with a `.specify/` folder ([GitHub Spec Kit](https://github.com/github/spec-kit)) is a Spec Kit project. paddock sees the folder and organizes spawn, kv, the hooks and the watch around the active feature (`feature_directory` in `.specify/feature.json`, or else the last folder in `specs/` with a `tasks.md`). You set nothing. To turn it off, set `enabled = false` in `[speckit]` of `~/.config/paddock/config.toml`. A project `paddock.toml` cannot turn it on or off.

paddock is the only writer of `tasks.md`. Workers do not edit it. paddock keeps one record per task in the `tree` scope of paddock kv (`spec/<feature>/task/<id>`): the state (todo, claimed, running, review, done, failed), the sandbox that holds it, its branch, its time limit, the result and the review. It writes the checkboxes of `tasks.md` from these records, and a short note such as the branch at the end of a task line. Nothing else in the file changes. When you edit `tasks.md` yourself, your edit wins: paddock reads the file again before each write, adds new tasks, and keeps the records of removed tasks.

| Command | What it does |
| :--- | :--- |
| `paddock-host spawn codex --task T012` | starts a worker for one task. paddock claims the task with a lease, so two workers never take one task, and the task of a dead worker is free again |
| `paddock-host spawn codex --task next` | the same for the next ready task. Run it once for each task of a ready `[P]` group. They run side by side, up to `[spawn] max_running` |
| `paddock-host spec status` | the feature, each task with state, owner and branch, what is ready, and what waits for review |
| `paddock-host spec review T012 pass` | marks a task done after you merged its branch. `changes "TEXT"` sends it back, and TEXT goes to the next worker |
| `paddock spec status` | the same status on the host, for the current folder |
| `paddock gauge` | how far the feature is: done with evidence, new and reopened tasks, open ages, and a forecast of the finish date (see How far a feature is (`paddock help spec-kit`)). `paddock-host gauge` gives a coordinator the same |
| `paddock spec appetite 001-albums 3d` | the time budget of a feature. paddock warns in the mailbox and never stops the work |
| `paddock spec gate T012 tests pass` | records a gate result on the host: `pass`, `fail` or `skip` |
| `paddock spec adopt` | moves `.specify/` and `specs/` of a repository into the external store (see below) |

`changes` also reopens a failed task and sends the note to the next worker.

A task is ready when all tasks of the earlier phases are done, all tasks before it in its phase are done (a run of `[P]` tasks can run together), and all tasks that its text names after "depends on" are done. The prompt of a worker has the task line, its acceptance text (the goal lines of the phase and the acceptance scenarios of its user story), and the paths of the constitution, `spec.md`, `plan.md` and the contracts. The worker ends with `paddock-host done "SUMMARY"`, and the task waits for your review. Watch the tasks with `paddock-host kv watch --scope tree --prefix spec/<feature>/task/`. `paddock ps` and `paddock wait` show the task of a worker. The session note (Claude Code) and the note after a context compaction name the feature, the task of the run, its acceptance text and the spec paths.

Task state, branches, results and review notes stay with the project after a run ends, so a later run continues the same tasks. With the loft or the external spec store, records live at `.paddock/spec/<feature>/tasks.json` inside that store. With the repo store, they live in the repository’s common git directory at `paddock/spec/<feature>/tasks.json` (usually `.git/paddock/spec/<feature>/tasks.json`), shared by linked worktrees.

### The external spec store

Spec Kit keeps `.specify/` and `specs/` in the repository. The external store is the loft (`paddock help private-notes`): with the loft on, the Spec Kit folders go there too, and `[speckit] store` is the short form for the Spec Kit folders alone. With the default store, paddock reminds you at the start of a run that a push publishes committed specs and plans, and adds a note to push approval requests that contain Spec Kit files. Set `store = "repo"` explicitly to keep the files there and silence these notices, or turn on the loft and move them there.

To keep them out of the repository, set:

```toml
[speckit]
store = "external"   # default: "repo", where Spec Kit puts them
```

Then run `paddock spec adopt` once in the repository. It asks, then moves both folders to the loft, `~/.local/share/paddock/loft/<repository key>/tree/`. A store of an earlier paddock moves there by itself. Every sandbox of the repository sees them at `.specify/` and `specs/`, also each `--worktree` sandbox and each worker, with the same content. paddock adds both paths to `.git/info/exclude`, which git does not track, so `git status` does not show them. If the files were committed, adopt tells you how to remove them from the index, and how to remove them from the history. It does not change the history itself.

### How far a feature is: paddock gauge

`paddock gauge` tells you how far a Spec Kit feature is and when it can end. It does not ask an agent for a percentage. It reads the history of `tasks.md`, the task records, the gate results and the mailboxes of the workers.

```sh
paddock gauge                 # the active feature of the current folder
paddock gauge 001-albums      # one feature
paddock gauge --json          # the same numbers as JSON (api_version paddock.gauge/v1)
paddock gauge --strict        # count only tasks with evidence as finished
paddock gauge --at 2026-11-08T09:00:00Z   # what the gauge said at that time
```

Inside a sandbox, a coordinator runs `paddock-host gauge`. The host computes it, because the sandbox cannot see the history of the loft.

The output has these parts:

- done: the done tasks, split into tasks with evidence, tasks that are only checked, and tasks that are not done although their box is checked.
- discovered: the tasks that appeared after the start, and how many a day.
- reopened, and fails after done: tasks that went back from done, and gate failures on done tasks.
- open: each open task with its age.
- almost done: the time that tasks sat in review, or with a worker that reported `blocked`.
- forecast: the P50 and P85 of the finish date and of the final task count. P85 means that 85 of 100 simulated futures end by that date.
- P85 trend: the P85 date at earlier times. A range that narrows is healthy. A date that slips, or a range that widens, is an alarm.
- appetite: the share of the time budget that the feature used, when it has one.
- a burn-up chart: `#` is done work, and the `-` line on top is the scope.
- paddock cannot know: what this project does not show, for example a history before the first snapshot.

#### Done with evidence

A checked box is a claim. A task counts as done with evidence only when a party other than its worker saw the result:

1. Its work is merged. Its branch, or a commit that its result names, is in the history of `HEAD` of the project or of its main work tree.
2. Or its review passed (`paddock-host spec review T012 pass`).
3. Or a gate passed for it.

A gate result speaks against a task. If the newest result of a gate is `skip`, the task is not done: a test that did not run proves nothing. If a gate failed after the task was done, the task is not done either. A worker report that says that its tests skipped, and names no passing run, counts as a skipped gate. A report that names both is "partly skipped". The task keeps its evidence, and gauge lists it, because the skipped part still waits for a real run.

The forecast counts a checked task without evidence as finished, and says how many such tasks there are. `--strict` counts it as open.

Record a gate result on the host:

```sh
paddock spec gate T012 real-sandbox skip --note "no bwrap in the sandbox"
paddock spec gate T012 real-sandbox pass
```

The results go to `gates.jsonl` next to the task records. Only the host writes them, never a worker.

#### The forecast

The forecast simulates 10,000 futures. Each simulated day draws how many tasks finish and how many new tasks arrive from a random day of the last 7 days (`--window`). New tasks are discovered and reopened tasks, less removed ones. One history and one seed (`--seed`, default 1) always give the same forecast.

- If new tasks arrive at least as fast as tasks finish, gauge says "not converging" and shows no date. More workers do not help then. Cut scope, or shape the tasks again.
- Before 5 tasks finished, or with less than three steps of history, gauge shows a wide range that it labels as an early forecast.
- A history shorter than the window is cut into at least six steps of at least one hour each.
- If more tasks appear at once than the list had before, gauge takes this as a rewrite of `tasks.md` and measures from there.
- gauge prints a warning when most tasks were done at the start of the window and the open count did not go down since. This is the "90 % done" trap.

#### Where the history comes from

1. The history of the loft, when the Spec Kit files live there. paddock commits the loft at the end of each run.
2. The git history of the repository, when git tracks `tasks.md`.
3. paddock's own state log, `history.jsonl` next to the task records. paddock adds a line each time a task changes state, and each time gauge runs.

A project without git still works. The first gauge has no history: it shows the current state, and it says what it cannot know. From then on, the state log is the history.

#### Appetite

The appetite is the time that you want to spend on a feature, counted from its start.

```sh
paddock spec appetite 001-albums 3d      # also 36h or 1d12h
paddock spec appetite 001-albums         # print it
paddock spec appetite 001-albums off     # remove it
```

paddock never stops the work because of the appetite. It sends a warning at 50 %, 80 % and 100 % of the appetite, and at every 25 % after that. Each warning comes once, as an `appetite` event in the mailbox of the run that works on the feature: to its starter, and to the coordinator while the run lives. The warning has the forecast and three choices: cut scope, extend the appetite once with a reason, or stop the feature. paddock checks the appetite when a worker ends and each time gauge runs.
