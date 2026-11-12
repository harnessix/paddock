## Timers

An agent loses its sense of time with its context. After a compaction, or when another agent takes over, it no longer knows how long it has worked on a bug or when the task must end. A paddock timer lives on the host, in paddock kv, so it outlasts every context, and the agent that it times cannot change it.

Inside the sandbox:

```sh
paddock-host timer start fix-login --budget 2h          # this agent owns it
paddock-host timer status                                # after a compaction, before one more try
paddock-host timer extend fix-login --by 30m --reason "the tests need a slow fixture"
paddock-host timer start review --quota weekly=1%       # at most 1% of the weekly limit
paddock-host timer extend review --by 1% --reason "two more files to check"
```

On the host:

```sh
paddock timer set 1a2b3c review --budget 1h30m --hard   # you own it; --hard freezes the sandbox at the end
paddock timer set 1a2b3c tests --quota weekly=1% --hard # a quota budget on the sandbox's account
paddock timer list                                      # the timers of every running sandbox
paddock timer extend 1a2b3c review --by 30m
paddock timer stop 1a2b3c review
```

- Whoever sets a deadline owns it. Only the owner extends or stops it. When an agent asks for more time on a timer that it does not own, you answer on the approval card, which shows its reason.
- A soft deadline gives one notification and marks the sandbox in the bar panel; the agent keeps running. A hard deadline freezes the sandbox (`paddock thaw ID` lets it continue).
- The time that counts leaves out the time when the sandbox was frozen.
- A quota budget (`--quota WINDOW=PERCENT`) limits a task to a share of a limit window of the sandbox's account, counted from the start of the timer: `weekly=1%` means 1 percentage point of the weekly window. Small budgets keep tasks small: work that needs more is better split into several tasks. The window is the name that `paddock forage` shows, or its first word. A timer can have a time budget, a quota budget, or both, and is over when either is used up. `extend --by 1%` adds quota.
- A quota budget counts what the account used, from the forage numbers. paddock cannot tell which sandbox used it, so it splits each rise among the quota timers on the same window that ran meanwhile: four timers at the same time count a quarter each. A timer whose sandbox ended keeps its part of the steps that it ran in. Your own use of the login outside paddock counts too, split the same way. When the account reports whole percents, a budget is over only when the count goes above it, so `weekly=1%` is not over at the first 1% step. The numbers are up to 15 minutes old, so a task can go a little over before paddock sees it. paddock needs usage numbers for the account to start a quota budget.
- Timers in the `pasture` and `tree` scopes are visible to the other agents there, so the agent that hands out work sees how long each part has run.
- `paddock ps --json` and the bar panel show the timer of each sandbox that is closest to its end.
- The keys `timer:*` change only through the timer commands, never with `paddock-host kv put`.
