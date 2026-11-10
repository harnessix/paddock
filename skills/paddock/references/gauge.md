# How far a feature is: paddock gauge

Use this when the user asks how far a Spec Kit feature is, when it will be done, or whether the agents make progress. Do not ask the agents for a percentage. Run the gauge:

```sh
paddock gauge                   # the active feature of the current folder
paddock gauge 001-albums --json # one feature, as JSON
paddock gauge --strict          # count only tasks with evidence as finished
paddock gauge --at 2026-11-08T09:00:00Z   # what it said at that time
```

## Read it in this order

1. `forecast`. "Not converging" means that new tasks arrive at least as fast as tasks finish. No date exists then, and more workers do not help. Tell the user, and offer the three choices: cut scope, shape the tasks again, or stop. An "early forecast" is a wide range: say so, and do not give one date.
2. `done`. Report the number with evidence, not the checked boxes. "Checked without evidence" means that nobody but the worker saw the result. "Not done, tests only skipped" means that a test did not run: the task needs a real run, for example a host run under bwrap.
3. `stuck`. Most tasks were done at the start of the window and the open count did not go down. This is the "90 % done" trap: look at the tasks that keep coming, not at the done share.
4. `almost done`. Tasks that wait for a review or for the host. A long time here means that the next step waits for a person or for the coordinator.
5. `P85 trend`. A range that narrows is healthy. "slipping" or "widening" is an alarm: report it.
6. `paddock cannot know`. Tell the user what the numbers leave out, for example that a plain folder had no history before the first gauge.

Give P50 and P85 as a range, not one date. P85 means that 85 of 100 simulated futures end by that date.

## Gate results

A gate result is a test run that you saw on the host. Record it:

```sh
paddock spec gate T012 real-sandbox pass
paddock spec gate T012 real-sandbox skip --note "no bwrap"
paddock spec gate T012 tests fail
```

`skip` makes a done task "not done". `fail` after a task was done counts as a fail after done. A later `pass` of the same gate replaces both. Workers cannot record gate results.

## Appetite

The appetite is the time budget of a feature, from its start. Ask the user for a value before you set it.

```sh
paddock spec appetite 001-albums 3d
paddock spec appetite 001-albums off
```

paddock never stops the work because of it. At 50 %, 80 % and 100 %, and at every 25 % after that, it writes one `appetite` event into the mailbox of the run that works on the feature. `paddock mail watch` shows it. Bring the three choices to the user: cut scope, extend once with a reason, or stop.
