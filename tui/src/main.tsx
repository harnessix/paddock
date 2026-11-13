// paddock-tui: the terminal interface of paddock. `paddock tui` starts it
// through cli.ts.

import React from 'react';
import { format } from 'node:util';
import { render } from 'ink';
import { App } from './app.js';
import { spawnSync } from 'node:child_process';
import { paddock, paddockBin } from './paddock.js';
import { HostContext, type Host, type Notice } from './host.js';
import type { Job } from './launch.js';
import { detectTheme, makeTheme, noColor, ThemeContext } from './theme.js';
import { ALT_ON, CURSOR_OFF, holdSignals, installExitHandlers, newTerminalState, restore, setFocusReports, setMouse } from './terminal.js';
import { Painter } from './screen.js';
import { frameStream } from './frame-stream.js';
import { detectArt } from './art.js';
import { ArtContext, ArtLayer } from './art-layer.js';
import { VERSION } from './version.js';
import { wantsReducedMotion } from './motion-pref.js';
import { t } from './i18n/index.js';

/** The text of --help, in the language of the interface. */
function help(): string {
  return t('main.help');
}

export { reducedMotion } from './motion-pref.js';

interface Options {
  plain: boolean;
  readonly: boolean;
  mouse: boolean;
  /** --reduce-motion: the world shows each change without motion. */
  reduceMotion: boolean;
  theme?: string;
  /** Start on the home screen (--home, or PADDOCK_TUI_START=home from `paddock` alone). */
  home: boolean;
}

/** PADDOCK_TUI_START=home: `paddock` alone started the interface. */
const startsHome = process.env.PADDOCK_TUI_START === 'home';
// The agents that the interface starts must not get the variable.
delete process.env.PADDOCK_TUI_START;

function parse(argv: string[]): Options | number {
  const o: Options = { plain: false, readonly: false, mouse: process.env.PADDOCK_TUI_MOUSE !== '0', reduceMotion: false, home: startsHome };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--plain') o.plain = true;
    else if (a === '--readonly' || a === '--read-only') o.readonly = true;
    else if (a === '--no-mouse') o.mouse = false;
    else if (a === '--reduce-motion' || a === '--reduced-motion') o.reduceMotion = true;
    else if (a === '--home') o.home = true;
    else if (a === '--theme') o.theme = argv[++i];
    else if (a?.startsWith('--theme=')) o.theme = a.slice('--theme='.length);
    else if (a === '-h' || a === '--help') {
      process.stdout.write(help() + '\n');
      return 0;
    } else if (a === '-V' || a === '--version') {
      process.stdout.write(`paddock-tui ${VERSION}\n`);
      return 0;
    } else {
      process.stderr.write(`paddock tui: ${t('main.unknownOption', { option: JSON.stringify(a) })}\n\n${help()}\n`);
      return 2;
    }
  }
  if (o.theme !== undefined && o.theme !== 'dark' && o.theme !== 'light') {
    process.stderr.write(`paddock tui: ${t('main.themeValue')}\n`);
    return 2;
  }
  if (o.plain) o.mouse = false;
  return o;
}

/** The number of a signal that ended a command, for its exit code. */
const SIGNAL_CODES: Record<string, number> = { SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGKILL: 9, SIGTERM: 15 };

/**
 * A paddock command gets the terminal: the interface is gone, and the
 * command reads the keys and writes the screen until it ends. This
 * program waits and reads nothing meanwhile. Returns what to say on the
 * home screen.
 */
async function handOver(job: Job, alt: boolean): Promise<Notice> {
  const stdin = process.stdin;
  // Stop reading the terminal: every key is for the command.
  stdin.pause();
  process.stdout.write(`${job.bin ?? 'paddock'} ${job.args.join(' ')}\n`);
  holdSignals(true);
  const r = spawnSync(job.bin ?? paddockBin(), job.args, { stdio: 'inherit' });
  // A Ctrl+C that the command got is in this program's queue too: let it pass first.
  await new Promise((done) => setTimeout(done, 50));
  holdSignals(false);
  const code = r.error ? 127 : (r.status ?? 128 + (SIGNAL_CODES[r.signal ?? ''] ?? 0));
  const notice: Notice = r.error
    ? { text: t('main.couldNotStart', { bin: job.bin ?? 'paddock', error: r.error.message }), role: 'error' }
    : code === 0
      ? { text: t('main.jobEnded', { title: job.title }), role: 'muted' }
      : { text: t('main.jobEndedCode', { title: job.title, code }), role: 'warning' };
  // The alternate screen hides what the command wrote. So wait for Enter
  // when that is the result, or when the command failed.
  if (alt && (job.pause === 'always' || code !== 0)) {
    process.stdout.write(`\n${t('main.pressEnter', { notice: notice.text })}`);
    await new Promise<void>((done) => {
      stdin.once('data', () => {
        stdin.pause();
        done();
      });
      // Ink let go of the terminal input when it left: without this, the program would end here.
      stdin.ref();
      stdin.resume();
    });
  }
  return notice;
}

async function main(notice?: Notice, resume?: Job['resume']): Promise<number> {
  const o = parse(process.argv.slice(2));
  if (typeof o === 'number') return o;
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    process.stderr.write(`${t('main.needsTerminal')}\n`);
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
  // Without art nothing moves: then the setting of the desktop is not read.
  const reduceMotion = layer.enabled ? wantsReducedMotion(o.reduceMotion, process.env) : true;
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
  // What the interface asks of this program: the home screen, and the terminal for a paddock command.
  let job: Job | null = null;
  const host: Host = { home: o.home, show: resume ? false : o.home || notice !== undefined, notice, resume, run: (j) => (job = j) };
  if (o.mouse) setMouse(term, true);
  // Focus reports: in the background the animation stops and nothing stays lit under the pointer.
  if (alt) setFocusReports(term, true);
  const tree = (
    <ThemeContext.Provider value={theme}>
      <ArtContext.Provider value={layer}>
        <App
          api={paddock}
          readonly={o.readonly}
          initialMouse={o.mouse}
          cwd={process.cwd()}
          onMouseChange={(on) => setMouse(term, on)}
          onRedraw={() => painter?.invalidate()}
          reduceMotion={reduceMotion}
          // OSC 52: the terminal puts the text on the clipboard. Only with the alternate screen, where nothing else writes.
          onCopy={painter ? (text) => void hostWrite(`\u001b]52;c;${Buffer.from(text, 'utf8').toString('base64')}\u0007`) : undefined}
        />
      </ArtContext.Provider>
    </ThemeContext.Provider>
  );
  // interactive: stdin and stdout are terminals (checked above). Ink's own
  // check also says no when CI is set, and then draws nothing until exit.
  const instance = painter
    ? // debug: Ink writes each frame whole and at once, as text. The painter does the rest.
      render(<HostContext.Provider value={host}>{tree}</HostContext.Provider>, { stdout: frameStream(out, painter, hostWrite), debug: true, exitOnCtrlC: false, patchConsole: false, interactive: true })
    : render(<HostContext.Provider value={host}>{tree}</HostContext.Provider>, { alternateScreen: false, exitOnCtrlC: false, patchConsole: true, maxFps: 30, interactive: true });
  unmount = () => instance.unmount();
  try {
    await instance.waitUntilExit();
  } finally {
    stop();
    restore(term);
  }
  // The interface left for a command: run it, then start again on the home screen.
  const next = job as Job | null;
  if (next === null) return 0;
  return main(await handOver(next, alt), next.resume);
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
