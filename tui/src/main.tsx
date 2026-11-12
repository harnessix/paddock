// paddock-tui: the terminal interface of paddock. `paddock tui` starts it
// through cli.ts.

import React from 'react';
import { format } from 'node:util';
import { render } from 'ink';
import { App } from './app.js';
import { paddock } from './paddock.js';
import { detectTheme, makeTheme, noColor, ThemeContext } from './theme.js';
import { ALT_ON, CURSOR_OFF, installExitHandlers, newTerminalState, restore, setFocusReports, setMouse } from './terminal.js';
import { Painter } from './screen.js';
import { frameStream } from './frame-stream.js';
import { detectArt } from './art.js';
import { ArtContext, ArtLayer } from './art-layer.js';
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

The runs show as rooms, one card per run with its character, when the
terminal has 80 columns or more and 256 or 24-bit colours. L switches to the
list and back. PADDOCK_TUI_ART=off, NO_COLOR or --plain: no art, the list.
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
  const theme = makeTheme(detectTheme(o.theme, process.env), o.plain, noColor(process.env));
  const alt = !o.plain;
  term.alt = alt;
  let unmount: () => void = () => undefined;
  const out = process.stdout;
  const hostWrite = out.write.bind(out) as (s: string) => boolean;
  // Some terminals report 0 until their first resize.
  const size = () => ({ columns: out.columns || 80, rows: out.rows || 24 });
  // With the alternate screen, the painter stands between Ink and the
  // terminal: Ink hands over each frame as text, and the painter writes
  // only the cells that changed. The art layer draws through it too.
  let art: ArtLayer | null = null;
  const painter = alt ? new Painter({ write: hostWrite, size, overlays: () => art?.overlays() ?? [] }) : null;
  art = new ArtLayer({ mode: detectArt(process.env, out.isTTY === true, o.plain), sink: null, screen: size, painter });
  const layer = art;
  const stop = () => {
    painter?.stop();
    layer.stop();
  };
  installExitHandlers(term, () => {
    stop();
    unmount();
  });
  if (painter) {
    hostWrite(ALT_ON + CURSOR_OFF);
    out.on('drain', () => painter.drained());
    // Nothing else may write to the screen that the painter keeps.
    for (const k of ['log', 'info', 'warn', 'error', 'debug'] as const) {
      console[k] = (...a: unknown[]) => {
        if (!process.stderr.isTTY) process.stderr.write(format(...a) + '\n');
      };
    }
  }
  if (o.mouse) setMouse(term, true);
  // Focus reports: in the background the animation stops and nothing stays lit under the pointer.
  if (alt) setFocusReports(term, true);
  const tree = (
    <ThemeContext.Provider value={theme}>
      <ArtContext.Provider value={layer}>
        <App api={paddock} readonly={o.readonly} initialMouse={o.mouse} cwd={process.cwd()} onMouseChange={(on) => setMouse(term, on)} onRedraw={() => painter?.invalidate()} />
      </ArtContext.Provider>
    </ThemeContext.Provider>
  );
  // interactive: stdin and stdout are terminals (checked above). Ink's own
  // check also says no when CI is set, and then draws nothing until exit.
  const instance = painter
    ? // debug: Ink writes each frame whole and at once, as text. The painter does the rest.
      render(tree, { stdout: frameStream(out, painter, hostWrite), debug: true, exitOnCtrlC: false, patchConsole: false, interactive: true })
    : render(tree, { alternateScreen: false, exitOnCtrlC: false, patchConsole: true, maxFps: 30, interactive: true });
  unmount = () => instance.unmount();
  try {
    await instance.waitUntilExit();
  } finally {
    stop();
    restore(term);
  }
  return 0;
}

/** Run the interface and end the process with its exit code. */
export function run(): void {
  main().then(
    (code) => process.exit(code),
    (e: unknown) => {
      process.stderr.write(`paddock tui: ${e instanceof Error ? e.message : String(e)}\n`);
      process.exit(70);
    },
  );
}
