# paddock-tui

The terminal interface of [paddock](https://github.com/harnessix/paddock).
It shows every run, the approvals that wait for you, the mail of the runs
and the board in one terminal window. You can use it with the keyboard
alone or with the mouse.

## Start it

Run `paddock tui`. paddock starts the first of these that it finds:

1. A `paddock-tui` binary in the folder of the paddock binary.
2. `paddock-tui` on your PATH.

If neither is available, paddock tells you how to install it. paddock never
downloads the interface itself. It does not use `npx`: npx downloads code
from npm, and it also runs a package or reads an `.npmrc` from the current
folder, which an agent can write.

The package is not on npm. Do not install a package with this name from
npm, because the project does not own the name there. Install it in one
of these two ways:

1. Put the `paddock-tui` binary of a paddock release in the folder of the
   paddock binary.
2. Build it from the paddock source tree with
   `cd tui && npm ci && npm run build`. Then put `tui/dist/cli.js` on your
   PATH as `paddock-tui`, for example with
   `ln -s "$PWD/dist/cli.js" ~/.local/bin/paddock-tui`.

## Options

| Option | What it does |
| --- | --- |
| `--plain` | No boxes, no colors, no mouse and no alternate screen. Use it with a screen reader or for logs. |
| `--readonly` | Hide every action that changes something. |
| `--no-mouse` | Start with the mouse off. `PADDOCK_TUI_MOUSE=0` does the same. |
| `--theme dark` or `--theme light` | The color theme. The default comes from `PADDOCK_TUI_THEME`, then `COLORFGBG`, then dark. |

## Keys

Press `?` for every key and `:` or `Ctrl+P` for the command palette.
The footer always shows the main keys of the current screen.

While the mouse is on, the terminal does not select text on a plain drag.
Hold Shift to select text, or press `M` to turn the mouse off.

Every key counts, also when several arrive at once (fast typing, a key that
repeats, a slow connection): the interface takes them one by one, in their
order. A paste goes into the open text field, with its line breaks as
spaces. With no text field open, a paste does nothing: its letters are never
commands. The question before a stop takes no answer from keys typed ahead.

A button shows a press at once (inverse video) and acts on the release. A
room, a row or a button under the pointer is marked. `Ctrl+L` draws the
whole screen again, for when another program wrote over it. `R` does the
same and reads paddock again.

## Rooms

The runs show as rooms: one card per run, with its agent as a small
character at a desk, the agent and project, the state word and the time in
the state. The character shows the state at a glance: it works, waits for
you, idles, sleeps (ended or frozen) or is blocked (failed or stuck). Each
agent has its color: orange claude, blue codex, purple gemini, green copilot,
pink grok, teal opencode, gray for any other.

The rooms need 80 columns and 24-bit or 256 colors. Under 80 columns, with
`--plain`, with `NO_COLOR` or in a terminal with only 16 colors, the runs show
as the list. `L` switches between the rooms and the list.
`PADDOCK_TUI_ART=off` turns the art off; `PADDOCK_TUI_ART=256` or
`truecolor` sets the colors when the terminal does not say.

The characters move at 8 frames a second while the terminal has the focus.
Each step writes only the cells that change. In the background, or when the
terminal does not show the card, nothing moves.

## Colors and small terminals

The colors are the 16 colors of the terminal, so its light or dark palette
decides the shades. No state shows by color alone: each has a glyph and a
word. With `NO_COLOR` the interface writes no color code at all; bold, dim,
inverse and underline carry the roles, and the runs show as the list.
`--plain` has no boxes, no colors and no mouse.

Under 80 columns the runs show as the list. Under 43 columns a row shows the
state in one word, the agent and where it runs. The header then has only the
tabs. A pane that is too low for its lines cuts them at the bottom.

## Drawing

The interface keeps the cells that the terminal shows. For a new frame it
writes only the cells that differ, each run of cells with one cursor move
and the shortest color codes. It clears the screen once at the start and
after a resize, and it never erases a line, so the screen does not flicker.
Frames that come faster than every 8 ms are joined, and a terminal that is
slow gets only the last frame. Where the terminal knows synchronized output
(mode 2026), a frame shows all at once.

While nothing changes, nothing happens: a read of paddock with the same
answer as before draws nothing, and a time such as "12 s" or "4 min" wakes
the interface only when its word changes. Measured with 20 rooms on a
200 x 60 terminal: no byte and about 0.3 % of one processor while the
terminal is in the background (the reads of paddock), about 3 % with the
animation, and about 10 ms from a key to its frame.
`PADDOCK_TUI_STATS=1 npx vitest run test/perf.test.tsx` prints the numbers
of the test with a fake clock.

### Sprites

The characters are the toon sprites of the launch video
(`sprites/frames.json`). A sprite is `sprites/character-COLOR-STATE.json`:
COLOR is `orange` (claude), `blue` (codex), `purple` (gemini), `green`
(copilot), `pink` (grok), `teal` (opencode) or `gray` (any other agent),
STATE is `idle`, `work` or `wait` (8 frames each), `sleep` or `blocked`
(4 frames each). The file holds
`{"w":24,"h":24,"fps":8,"frames":[[[r,g,b] or null per pixel, row by row]]}`;
`null` is a transparent pixel. Each terminal cell shows two pixels, so a
sprite takes 24 columns and 12 rows. All loops play at 8 frames a second.

The build puts these 35 files into the program, also into the single binary,
and nothing else from `sprites/`. It stops when a file is missing or has the
wrong frame count. At run time a color whose file does not load gets the gray
sprite of the state, and without that a built-in gray character. The cells
and their color codes are made once per sprite, when it first shows; an
animation step only writes the ready text of the changed cells.

## How it works

The interface reads and changes everything through paddock's commands.
It runs `paddock ps --json`, `paddock board --json`, `paddock approvals --json`,
`paddock mail ls --json` and `paddock view ID --tail --json`. Actions run
`paddock approvals answer`, `paddock run --detach`, `paddock stop`,
`paddock view`, `paddock split` (when paddock has it) and `paddock mail send`.
`PADDOCK_BIN` selects the paddock binary. `paddock tui` sets it.

## Develop

The tools come from `mise.toml` (Node.js 22 and bun).

```sh
cd tui
npm ci
npm run check                        # typecheck, lint and all tests
node scripts/compile.mjs linux-x64   # bin/paddock-tui-linux-x64
PADDOCK_TUI_UNDER_TEST=$PWD/bin/paddock-tui-linux-x64 npx vitest run test/pty.test.ts
```

The tests use a fake paddock (`test/fake-paddock.mjs`) that answers from
the JSON files in `test/fixtures` and records every call.
