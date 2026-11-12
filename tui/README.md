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
