// The terminal modes the interface turns on, and their reset. The reset
// runs on every way out: a normal exit, an error, SIGTERM, SIGHUP, SIGINT.

import { writeSync } from 'node:fs';

/** Button events (1002), any motion for hover (1003), SGR coordinates (1006). */
export const MOUSE_ON = '\u001b[?1002h\u001b[?1003h\u001b[?1006h';
export const MOUSE_OFF = '\u001b[?1003l\u001b[?1002l\u001b[?1006l';
export const ALT_ON = '\u001b[?1049h';
export const ALT_OFF = '\u001b[?1049l';
/** Focus reports (1004): the terminal sends ESC [ I on focus in, ESC [ O on focus out. */
export const FOCUS_ON = '\u001b[?1004h';
export const FOCUS_OFF = '\u001b[?1004l';
export const PASTE_OFF = '\u001b[?2004l';
export const CURSOR_ON = '\u001b[?25h';
export const CURSOR_OFF = '\u001b[?25l';

export interface TerminalState {
  fd: number;
  mouse: boolean;
  alt: boolean;
  focus: boolean;
  restored: boolean;
}

export function newTerminalState(fd = 1): TerminalState {
  return { fd, mouse: false, alt: false, focus: false, restored: false };
}

function write(fd: number, s: string): void {
  try {
    writeSync(fd, s);
  } catch {
    // The terminal is gone: nothing to restore.
  }
}

export function setMouse(t: TerminalState, on: boolean): void {
  if (on === t.mouse) return;
  write(t.fd, on ? MOUSE_ON : MOUSE_OFF);
  t.mouse = on;
}

export function setFocusReports(t: TerminalState, on: boolean): void {
  if (on === t.focus) return;
  write(t.fd, on ? FOCUS_ON : FOCUS_OFF);
  t.focus = on;
}

/** Every mode off, synchronously, so it also works in a signal handler. */
export function restore(t: TerminalState, stdin: NodeJS.ReadStream = process.stdin): void {
  if (t.restored) return;
  t.restored = true;
  // Bracketed paste off too: Ink turns it on for the paste handler.
  let out = MOUSE_OFF + PASTE_OFF + CURSOR_ON;
  if (t.focus) out += FOCUS_OFF;
  if (t.alt) out += ALT_OFF;
  write(t.fd, out);
  t.mouse = false;
  t.focus = false;
  try {
    if (stdin.isTTY) stdin.setRawMode(false);
  } catch {
    // stdin is closed.
  }
}

const SIGNALS: Record<string, number> = { SIGTERM: 15, SIGHUP: 1, SIGINT: 2, SIGQUIT: 3 };

/** The terminal state and the cleanup that the handlers work on: the ones of the interface that runs now. */
let current: { t: TerminalState; beforeExit: () => void } | null = null;
/** A command has the terminal (holdSignals): its keys are not for this program. */
let held = false;

/**
 * Restore the terminal on signals and crashes, then end with the usual
 * code. The interface can start again after a command had the terminal:
 * each call replaces the state of the last one, and the handlers are
 * installed once.
 */
export function installExitHandlers(t: TerminalState, beforeExit: () => void): void {
  const first = current === null;
  current = { t, beforeExit };
  if (!first) return;
  for (const [sig, n] of Object.entries(SIGNALS)) {
    process.on(sig as NodeJS.Signals, () => {
      // Ctrl+C and Ctrl+\ in a command that has the terminal go to that command, as in a shell.
      if (held && (sig === 'SIGINT' || sig === 'SIGQUIT')) return;
      try {
        current?.beforeExit();
      } catch {
        // Restore anyway.
      }
      if (current) restore(current.t);
      process.exit(128 + n);
    });
  }
  const crash = (e: unknown) => {
    if (current) restore(current.t);
    const msg = e instanceof Error ? (e.stack ?? e.message) : String(e);
    write(2, `paddock tui: ${msg}\n`);
    process.exit(70);
  };
  process.on('uncaughtException', crash);
  process.on('unhandledRejection', crash);
  process.on('exit', () => {
    if (current) restore(current.t);
  });
}

/** While a command has the terminal, SIGINT and SIGQUIT do not end this program. */
export function holdSignals(on: boolean): void {
  held = on;
}
