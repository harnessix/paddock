#!/usr/bin/env node
// paddock-tui: the terminal interface of paddock. `paddock tui` starts it.

import React from 'react';
import { render } from 'ink';
import { App } from './app.js';
import { paddock } from './paddock.js';
import { detectTheme, makeTheme, ThemeContext } from './theme.js';
import { installExitHandlers, newTerminalState, restore, setMouse } from './terminal.js';
import { VERSION } from './version.js';

const HELP = `paddock tui: runs, approvals, mail and the board of paddock in the terminal.

Usage: paddock tui [--plain] [--readonly] [--no-mouse] [--theme dark|light]

  --plain        no boxes, no colours, no mouse, no alternate screen (screen
                 readers, logs)
  --readonly     hide every action: only look
  --no-mouse     start with the mouse off (M turns it on). PADDOCK_TUI_MOUSE=0
                 does the same
  --theme NAME   dark or light. Default: PADDOCK_TUI_THEME, then COLORFGBG,
                 then dark
  --version      print the version

Keys: ? shows every key, : or Ctrl+P opens the command palette.
The interface calls paddock (PADDOCK_BIN, else paddock on PATH) for every read
and every action.`;

interface Options {
  plain: boolean;
  readonly: boolean;
  mouse: boolean;
  theme?: string;
}

function parse(argv: string[]): Options | number {
  const o: Options = { plain: false, readonly: false, mouse: process.env.PADDOCK_TUI_MOUSE !== '0' };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--plain') o.plain = true;
    else if (a === '--readonly' || a === '--read-only') o.readonly = true;
    else if (a === '--no-mouse') o.mouse = false;
    else if (a === '--theme') o.theme = argv[++i];
    else if (a?.startsWith('--theme=')) o.theme = a.slice('--theme='.length);
    else if (a === '-h' || a === '--help') {
      process.stdout.write(HELP + '\n');
      return 0;
    } else if (a === '-V' || a === '--version') {
      process.stdout.write(`paddock-tui ${VERSION}\n`);
      return 0;
    } else {
      process.stderr.write(`paddock tui: unknown option ${JSON.stringify(a)}\n\n${HELP}\n`);
      return 2;
    }
  }
  if (o.theme !== undefined && o.theme !== 'dark' && o.theme !== 'light') {
    process.stderr.write('paddock tui: --theme takes dark or light\n');
    return 2;
  }
  if (o.plain) o.mouse = false;
  return o;
}

async function main(): Promise<number> {
  const o = parse(process.argv.slice(2));
  if (typeof o === 'number') return o;
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    process.stderr.write('paddock tui needs a terminal. paddock ps, paddock board and paddock approvals print the same state as text or JSON.\n');
    return 2;
  }
  const term = newTerminalState(1);
  const theme = makeTheme(detectTheme(o.theme, process.env), o.plain);
  const alt = !o.plain;
  term.alt = alt;
  let unmount: () => void = () => undefined;
  installExitHandlers(term, () => unmount());
  if (o.mouse) setMouse(term, true);
  const instance = render(
    <ThemeContext.Provider value={theme}>
      <App api={paddock} readonly={o.readonly} initialMouse={o.mouse} cwd={process.cwd()} onMouseChange={(on) => setMouse(term, on)} />
    </ThemeContext.Provider>,
    { alternateScreen: alt, exitOnCtrlC: false, patchConsole: true, maxFps: 30 },
  );
  unmount = () => instance.unmount();
  try {
    await instance.waitUntilExit();
  } finally {
    restore(term);
  }
  return 0;
}

main().then(
  (code) => process.exit(code),
  (e: unknown) => {
    process.stderr.write(`paddock tui: ${e instanceof Error ? e.message : String(e)}\n`);
    process.exit(70);
  },
);
