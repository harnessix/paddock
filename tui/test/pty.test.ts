// The built program in a real pseudo-terminal, against the fake paddock:
// keys work, the screen fits, and every way out restores the terminal
// (main screen, mouse reports off, cursor on, cooked mode).
import { afterEach, expect, test } from 'vitest';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import pty from 'node-pty';
import xterm from '@xterm/headless';
import { actions, fakeWorld, HANG_MS, sleep, until } from './helpers.js';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const CLI = join(root, 'dist', 'cli.js');
/** The shell prints this after the terminal settings: all output is in. */
const DONE = 'STTY-DONE';

interface Session {
  p: pty.IPty;
  term: xterm.Terminal;
  raw: () => string;
  screen: () => string;
  /** What the program wrote to stderr: in a file, not on the screen. */
  stderr: () => string;
  exited: Promise<{ exitCode: number; signal?: number }>;
}

const open: Session[] = [];
afterEach(() => {
  for (const s of open.splice(0)) {
    try {
      s.p.kill('SIGKILL');
    } catch {
      // gone
    }
  }
});

function start(args: string[], env: Record<string, string> = {}): Session {
  if (!process.env.PADDOCK_TUI_UNDER_TEST && !existsSync(CLI)) execFileSync('npm', ['run', 'build'], { cwd: root, stdio: 'ignore' });
  const term = new xterm.Terminal({ cols: 80, rows: 24, allowProposedApi: true });
  // A shell runs the program, then prints the terminal settings it leaves.
  // PADDOCK_TUI_UNDER_TEST: a compiled binary (bun build --compile) instead of node and dist/cli.js.
  const bin = process.env.PADDOCK_TUI_UNDER_TEST;
  const cmd = bin ? `"${bin}"` : `"${process.execPath}" "${CLI}"`;
  const errFile = join(mkdtempSync(join(tmpdir(), 'paddock-tui-stderr-')), 'stderr');
  const p = pty.spawn('/bin/sh', ['-c', `${cmd} "$@" 2>"$TUI_STDERR"; echo "EXIT=$?"; stty -a; echo "${DONE}"`, 'sh', ...args], {
    name: 'xterm-256color',
    cols: 80,
    rows: 24,
    cwd: root,
    // CI as on a CI runner: the program draws in a terminal also then.
    env: { ...process.env, TERM: 'xterm-256color', CI: 'true', PADDOCK_TUI_MOUSE: '1', TUI_STDERR: errFile, ...env } as Record<string, string>,
  });
  let raw = '';
  p.onData((d) => {
    raw += d;
    term.write(d);
  });
  const exited = new Promise<{ exitCode: number; signal?: number }>((res) => p.onExit(res));
  const screen = () => {
    const b = term.buffer.active;
    const out: string[] = [];
    for (let i = 0; i < term.rows; i++) out.push(b.getLine(b.viewportY + i)?.translateToString(true) ?? '');
    return out.join('\n');
  };
  const stderr = () => {
    try {
      return readFileSync(errFile, 'utf8');
    } catch {
      return '';
    }
  };
  const s = { p, term, raw: () => raw, screen, stderr, exited };
  open.push(s);
  return s;
}

async function waitScreen(s: Session, text: string, ms = HANG_MS): Promise<string> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    await new Promise<void>((r) => s.term.write('', r));
    if (s.screen().includes(text)) return s.screen();
    await sleep(50);
  }
  throw new Error(
    `no "${text}" on the screen:\n${s.screen()}\n--- raw tail:\n${JSON.stringify(s.raw().slice(-500))}\n--- stderr of the program:\n${s.stderr().slice(-2000)}`,
  );
}

async function restored(s: Session): Promise<void> {
  // Wait for the end of the output and the exit of the shell, not for a time.
  await until(() => s.raw().includes(DONE));
  await s.exited;
  await new Promise<void>((r) => s.term.write('', r));
  const raw = s.raw();
  expect(s.term.buffer.active.type).toBe('normal');
  expect(s.term.modes.mouseTrackingMode).toBe('none');
  expect(raw.lastIndexOf('\u001b[?1006l')).toBeGreaterThan(raw.lastIndexOf('\u001b[?1006h'));
  expect(raw.lastIndexOf('\u001b[?25h')).toBeGreaterThan(raw.lastIndexOf('\u001b[?25l'));
  // The shell after the program: cooked mode with echo.
  const stty = raw.slice(raw.lastIndexOf('EXIT='), raw.lastIndexOf(DONE));
  expect(stty).toMatch(/(^|\s)icanon(\s|$)/m);
  expect(stty).toMatch(/(^|\s)echo(\s|$)/m);
}

test('keys work in the real program, and q leaves a clean terminal', async () => {
  const dir = fakeWorld();
  const s = start([]);
  const first = await waitScreen(s, 'sandbox bwrap');
  expect(first).toContain('◆ waits for you');
  expect(s.term.buffer.active.type).toBe('alternate');
  expect(s.term.modes.mouseTrackingMode).toBe('any');
  expect(first.split('\n').length).toBe(24);
  s.p.write('a');
  await waitScreen(s, '[1] Allow once');
  s.p.write('1');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'once']);
  // A mouse click through the terminal: the Board tab.
  const row = s.screen().split('\n')[0] as string;
  const x = row.indexOf('Board');
  s.p.write(`\u001b[<0;${x + 2};1M\u001b[<0;${x + 2};1m`);
  await waitScreen(s, '◇ open');
  s.p.write('q');
  await waitScreen(s, 'sandbox bwrap');
  s.p.write('q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('SIGTERM leaves a clean terminal', async () => {
  fakeWorld();
  const s = start([]);
  await waitScreen(s, 'sandbox bwrap');
  const child = execFileSync('pgrep', ['-P', String(s.p.pid)], { encoding: 'utf8' }).trim().split('\n')[0];
  expect(child).toMatch(/^\d+$/);
  process.kill(Number(child), 'SIGTERM');
  await waitScreen(s, 'EXIT=143');
  await restored(s);
});

test('Ctrl+C quits and leaves a clean terminal', async () => {
  fakeWorld();
  const s = start([]);
  await waitScreen(s, 'sandbox bwrap');
  s.p.write('\u0003');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('--plain uses no alternate screen and no mouse', async () => {
  fakeWorld();
  const s = start(['--plain']);
  const f = await waitScreen(s, 'sandbox bwrap');
  expect(f).not.toContain('╭');
  expect(s.term.buffer.active.type).toBe('normal');
  expect(s.term.modes.mouseTrackingMode).toBe('none');
  expect(s.raw()).not.toContain('\u001b[?1049h');
  expect(s.raw()).not.toContain('\u001b[?1002h');
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('PADDOCK_TUI_MOUSE=0 starts with the mouse off', async () => {
  fakeWorld();
  const s = start([], { PADDOCK_TUI_MOUSE: '0' });
  await waitScreen(s, 'M mouse on');
  expect(s.term.modes.mouseTrackingMode).toBe('none');
  s.p.write('M');
  await waitScreen(s, 'hold Shift to select text');
  expect(s.term.modes.mouseTrackingMode).toBe('any');
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});
