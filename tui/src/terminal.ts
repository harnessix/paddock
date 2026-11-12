// The terminal modes the interface turns on, and their reset. The reset
// runs on every way out: a normal exit, an error, SIGTERM, SIGHUP, SIGINT.

import { writeSync } from 'node:fs';

/** Button events (1002), any motion for hover (1003), SGR coordinates (1006). */
export const MOUSE_ON = '\u001b[?1002h\u001b[?1003h\u001b[?1006h';
export const MOUSE_OFF = '\u001b[?1003l\u001b[?1002l\u001b[?1006l';
export const ALT_OFF = '\u001b[?1049l';
export const CURSOR_ON = '\u001b[?25h';

export interface TerminalState {
  fd: number;
  mouse: boolean;
  alt: boolean;
  restored: boolean;
}

export function newTerminalState(fd = 1): TerminalState {
  return { fd, mouse: false, alt: false, restored: false };
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

/** Every mode off, synchronously, so it also works in a signal handler. */
export function restore(t: TerminalState, stdin: NodeJS.ReadStream = process.stdin): void {
  if (t.restored) return;
  t.restored = true;
  let out = MOUSE_OFF + CURSOR_ON;
  if (t.alt) out += ALT_OFF;
  write(t.fd, out);
  t.mouse = false;
  try {
    if (stdin.isTTY) stdin.setRawMode(false);
  } catch {
    // stdin is closed.
  }
}

const SIGNALS: Record<string, number> = { SIGTERM: 15, SIGHUP: 1, SIGINT: 2, SIGQUIT: 3 };

/** Restore the terminal on signals and crashes, then end with the usual code. */
export function installExitHandlers(t: TerminalState, beforeExit: () => void): void {
  for (const [sig, n] of Object.entries(SIGNALS)) {
    process.on(sig as NodeJS.Signals, () => {
      try {
        beforeExit();
      } catch {
        // Restore anyway.
      }
      restore(t);
      process.exit(128 + n);
    });
  }
  const crash = (e: unknown) => {
    restore(t);
    const msg = e instanceof Error ? (e.stack ?? e.message) : String(e);
    write(2, `paddock tui: ${msg}\n`);
    process.exit(70);
  };
  process.on('uncaughtException', crash);
  process.on('unhandledRejection', crash);
  process.on('exit', () => restore(t));
}
