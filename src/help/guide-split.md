## Split: several terminals in one window

`paddock split` shows several terminals in one window: an agent in a sandbox, a shell on the host, a view of a detached run, the board. Each pane has a header row that paddock writes. The header says whether the pane runs in a sandbox or on the host.

```sh
paddock split                          # default_harness above, a host shell below
paddock split run claude + shell       # Claude Code above, a host shell below
paddock split run codex + board + shell
paddock split view 1a2b3c + shell      # a detached run, read-only, and a shell
paddock split add shell                # add a pane to the running split
paddock split ls                       # list the panes
```

A lone `+` separates two panes. Flags of `paddock split`, such as `--detached`, go before the panes.

### Pane kinds

| Pane | What runs in it | Side |
| --- | --- | --- |
| `run HARNESS [ARGS]` | A new sandbox run, as `paddock HARNESS [ARGS]`. | sandbox |
| `view ID [--write]` | An existing run. It is read-only unless you add `--write`. | sandbox |
| `shell` | Your `$SHELL` on the host, in the current folder. | host |
| `board` | `paddock board --watch`. | host |
| `mac [APP]` | `paddock mac view`, the macOS guest or one app session. macOS only. | sandbox |

Without panes, `paddock split` starts `run` of `default_harness` from `paddock.toml` above and `shell` below. If `paddock.toml` sets no `default_harness`, name the panes.

### Headers

The header of a sandbox pane reads `sandbox · claude · claude-app-1a2b3c` in the paddock accent color (sage), with a status badge: `[running]`, `[waits for you]` or `[ended]`. The badge comes from the same state as `paddock board`. A run shows `[starting]` until its sandbox runs. The header of a host pane reads `host · shell · ~/src/app` in the warning color (amber). The words `sandbox` and `host` are always in the header, so the header also reads on a monochrome terminal and in a screenshot. A long folder or id loses its middle, never the side word. The active pane has a reversed header.

paddock sets the headers. A program in a pane cannot change its header: paddock ignores OSC titles and window renames from the pane. A pane that you open with tmux keys, for example Ctrl-B %, has no paddock label. Its header reads `host · shell` in amber, because that pane runs a shell on the host. If paddock cannot label a sandbox pane, the pane starts nothing and says so: a sandbox never shows as a host shell.

A program can print text that looks like a header inside its own pane. The status bar names the side of the active pane, for example `paddock split · sandbox`, and no program in a pane can write there. A view of a detached run shows no second status bar inside its pane.

### Layout

The panes sit from top to bottom. With more than three panes, they form a grid. When the window is at least 140 columns wide, a `board` pane sits on the left and the other panes sit on the right. The split starts at the size of your terminal, and paddock lays it out again when a terminal attaches or changes its size: in 80 columns, the board is one pane among the others.

### Keys

The keys are the tmux keys, with the prefix Ctrl-B. The status bar shows the most important keys and nothing else: no host name and no clock.

- Ctrl-B o moves to the next pane. A mouse click also moves to a pane.
- Ctrl-B z zooms the pane to the full window, and back.
- Ctrl-B d leaves the split. The sandboxes keep running. `paddock split` attaches again.
- Ctrl-B x closes a pane.

### Where the split runs

The split is a session on paddock's own tmux server, the same server as `paddock view`. paddock never uses your tmux server or your tmux configuration. The socket is in the runtime folder, which every sandbox hides.

Outside tmux, `paddock split` uses the current terminal. Inside tmux, yours or a split, it opens a new terminal window, so two Ctrl-B prefixes do not collide. `--detached` starts the split without a terminal. When a split runs, `paddock split` without panes attaches to it, and `paddock split add` adds a pane. Inside the split, the new pane goes below the current pane.

### Without tmux

`paddock split` needs tmux. Without it, paddock prints the install command. `paddock split --windows PANES` opens each pane in its own terminal window instead, as `paddock view` does. A window has no header row: it prints its label first, for example `paddock: sandbox · claude · starting`, in the color of its side. A program that clears the screen also clears that line.
