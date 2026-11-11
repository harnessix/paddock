## Trainer (experimental)

A trainer gives a live horse-mode run an independent review. On the host, use `paddock train RUN` or `paddock train RUN --deep`. The worker can request one with `paddock-host train [--deep]`. The trainer starts a separate session with network disabled, no approvals and read-only access to the target worktree. It gets a bounded brief, then can ask for more with its scoped, time-limited `paddock-host peek --grant trainer` grant. `--deep` increases the brief and peek limits; it does not copy the target session into the trainer's context. The target's encrypted session files remain ciphertext in the trainer mount.

A trainer sends structured advice to the worker's mailbox. Normal advice waits for a turn end; severe findings interrupt. The worker decides what to do. Trainers can run periodically during active work, on stall signals, on request and before final completion, subject to the run's frequency cap and budget. If the final review fails, the worker receives the failure and the final report waits for a successful retry. In user config:

```toml
[trainer]
enabled = true
interval = "45m"
no_commit = "90m"
min_interval = "5m"
budget = 8
```

`paddock train stats` shows recorded advice and follow-up facts for all runs; add a run ID to narrow it. Adoption starts as unknown. Record an explicit decision with `paddock train feedback RUN ADVICE adopted|not-adopted --note TEXT` on the host or `paddock-host train feedback ADVICE adopted|not-adopted [NOTE]` in the worker. Repeated negative feedback reduces the matching automatic trigger. Later check results and commits are observations, not proof that the advice caused them.
