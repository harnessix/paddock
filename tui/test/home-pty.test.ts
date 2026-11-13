// The home screen of the built program in a real pseudo-terminal, against
// the fake paddock: `paddock` alone opens it, Start hands the terminal to
// `paddock run`, the agent gets every key, and the home screen comes back.
import { afterEach, expect, test } from 'vitest';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import pty from 'node-pty';
import xterm from '@xterm/headless';
import { calls, fakeWorld, HANG_MS, sleep, until } from './helpers.js';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const CLI = join(root, 'dist', 'cli.js');
const DONE = 'STTY-DONE';

interface Session {
  p: pty.IPty;
  term: xterm.Terminal;
  raw: () => string;
  screen: () => string;
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

/** The program in a terminal of 80 x 24, started by a shell that prints the terminal settings it leaves. */
function start(args: string[], env: Record<string, string> = {}): Session {
  if (!process.env.PADDOCK_TUI_UNDER_TEST && !existsSync(CLI)) execFileSync('npm', ['run', 'build'], { cwd: root, stdio: 'ignore' });
  const term = new xterm.Terminal({ cols: 80, rows: 24, allowProposedApi: true });
  const bin = process.env.PADDOCK_TUI_UNDER_TEST;
  const cmd = bin ? `"${bin}"` : `"${process.execPath}" "${CLI}"`;
  const errFile = join(mkdtempSync(join(tmpdir(), 'paddock-tui-stderr-')), 'stderr');
  const p = pty.spawn('/bin/sh', ['-c', `${cmd} "$@" 2>"$TUI_STDERR"; echo "EXIT=$?"; stty -a; echo "${DONE}"`, 'sh', ...args], {
    name: 'xterm-256color',
    cols: 80,
    rows: 24,
    cwd: root,
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
  throw new Error(`no "${text}" on the screen:\n${s.screen()}\n--- raw tail:\n${JSON.stringify(s.raw().slice(-500))}\n--- stderr of the program:\n${s.stderr().slice(-2000)}`);
}

/** The terminal after the program: main screen, no mouse reports, cursor on, cooked mode with echo. */
async function restored(s: Session): Promise<void> {
  await until(() => s.raw().includes(DONE));
  await s.exited;
  await new Promise<void>((r) => s.term.write('', r));
  const raw = s.raw();
  expect(s.term.buffer.active.type).toBe('normal');
  expect(s.term.modes.mouseTrackingMode).toBe('none');
  expect(raw.lastIndexOf('\u001b[?25h')).toBeGreaterThan(raw.lastIndexOf('\u001b[?25l'));
  const stty = raw.slice(raw.lastIndexOf('EXIT='), raw.lastIndexOf(DONE));
  expect(stty).toMatch(/(^|\s)icanon(\s|$)/m);
  expect(stty).toMatch(/(^|\s)echo(\s|$)/m);
}

/** The screen as one line: a text that wraps at the right edge reads as it was written. */
const joined = (s: Session): string => s.screen().replace(/\n/g, '');

const runs = (dir: string): string[][] => calls(dir).filter((c) => c.args[0] === 'run').map((c) => c.args);

/** The process of the interface under the shell, and the paddock command that it runs in the terminal. */
function pids(s: Session): { tui: number; child: number } {
  const under = (pid: number): number[] => {
    try {
      return execFileSync('pgrep', ['-P', String(pid)], { encoding: 'utf8' }).trim().split('\n').filter(Boolean).map(Number);
    } catch {
      return [];
    }
  };
  const tui = under(s.p.pid)[0] ?? 0;
  return { tui, child: under(tui)[0] ?? 0 };
}

test('`paddock` alone: the home screen, a start that hands the terminal to the agent, and the home screen again', async () => {
  const dir = fakeWorld((d) => writeFileSync(join(d, 'run-reads'), ''));
  // PADDOCK_TUI_START=home is what `paddock` alone sets.
  const s = start([], { PADDOCK_TUI_START: 'home', COLORTERM: 'truecolor' });
  const first = await waitScreen(s, 'Installed: claude, codex');
  await waitScreen(s, '3 requests wait for you');
  expect(first).toContain('› New agent');
  expect(first.split('\n').length).toBe(24);
  expect(s.term.buffer.active.type).toBe('alternate');
  expect(s.term.modes.mouseTrackingMode).toBe('any');
  // The icon in the colours of the paddock icon: the pickets and the rails.
  expect(s.raw()).toContain('38;2;159;184;160');
  expect(s.raw()).toContain('2;127;163;132');
  s.p.write('n');
  await waitScreen(s, '✓ The agent sees only this folder.');
  // The character of the agent, in 24-bit colours.
  expect(s.screen()).toMatch(/[▀▄]{4}/);
  expect(s.screen()).toContain('[claude]');
  s.p.write('\r');
  // The agent has the terminal: the main screen, no mouse reports, and its own prompt.
  await waitScreen(s, 'fake> ');
  expect(s.term.buffer.active.type).toBe('normal');
  expect(s.term.modes.mouseTrackingMode).toBe('none');
  // A long path wraps at 80 columns.
  expect(joined(s)).toContain(`paddock run claude ${root}`);
  // The variable of the home screen does not reach the agent.
  expect(joined(s)).toContain(`FAKE run claude ${root} START=unset`);
  // Every key goes to the agent, in a terminal in cooked mode: the interface reads none of them.
  s.p.write('hello agent\r');
  await until(() => existsSync(join(dir, 'run-input.txt')));
  expect(readFileSync(join(dir, 'run-input.txt'), 'utf8')).toBe('hello agent\n');
  expect(runs(dir)).toEqual([['run', 'claude', root]]);
  // The run ended: the home screen is back, and says so.
  await waitScreen(s, 'claude in tui ended');
  await waitScreen(s, '› New agent');
  expect(s.term.buffer.active.type).toBe('alternate');
  expect(s.term.modes.mouseTrackingMode).toBe('any');
  // A second start from the home screen that came back.
  s.p.write('n');
  await waitScreen(s, '✓ The agent sees only this folder.');
  s.p.write('\r');
  await until(() => runs(dir).length === 2);
  s.p.write('again\r');
  await until(() => readFileSync(join(dir, 'run-input.txt'), 'utf8') === 'hello agent\nagain\n');
  await waitScreen(s, '› New agent');
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
  expect(s.stderr()).toBe('');
});

test('`paddock tui`: H opens the home screen, and a start from it comes back to it', async () => {
  const dir = fakeWorld();
  const s = start([]);
  await waitScreen(s, 'sandbox bwrap');
  s.p.write('H');
  await waitScreen(s, '› New agent');
  s.p.write('n');
  await waitScreen(s, '✓ The agent sees only this folder.');
  s.p.write('\r');
  await until(() => runs(dir).length === 1);
  await waitScreen(s, '› New agent');
  await waitScreen(s, 'claude in tui ended');
  // The runs are the first screen here: Esc closes the home screen, and q quits.
  s.p.write('\u001b');
  await waitScreen(s, 'sandbox bwrap');
  s.p.write('q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('a run that fails keeps its last lines on the screen until Enter', async () => {
  const dir = fakeWorld((d) => writeFileSync(join(d, 'run-exit'), '3'));
  const s = start(['--home']);
  await waitScreen(s, 'Installed: claude, codex');
  s.p.write('n');
  await waitScreen(s, '✓ The agent sees only this folder.');
  s.p.write('\r');
  await waitScreen(s, 'claude in tui ended with code 3. Press Enter to go back to paddock.');
  expect(joined(s)).toContain(`FAKE run claude ${root}`);
  expect(s.term.buffer.active.type).toBe('normal');
  // Nothing happens until Enter.
  await sleep(400);
  expect(s.term.buffer.active.type).toBe('normal');
  s.p.write('\r');
  await waitScreen(s, '› New agent');
  await waitScreen(s, 'claude in tui ended with code 3');
  expect(s.term.buffer.active.type).toBe('alternate');
  expect(runs(dir).length).toBe(1);
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('Ctrl+C in the agent ends the agent, not the interface', async () => {
  fakeWorld((d) => writeFileSync(join(d, 'run-reads'), ''));
  const s = start(['--home']);
  await waitScreen(s, 'Installed: claude, codex');
  s.p.write('n');
  await waitScreen(s, '✓ The agent sees only this folder.');
  s.p.write('\r');
  await waitScreen(s, 'fake> ');
  // The terminal sends SIGINT to every process in front: the agent and the interface that waits for it.
  const { tui, child } = pids(s);
  expect(tui).toBeGreaterThan(0);
  expect(child).toBeGreaterThan(0);
  process.kill(tui, 'SIGINT');
  process.kill(child, 'SIGINT');
  await waitScreen(s, 'ended with code 130. Press Enter to go back to paddock.');
  s.p.write('\r');
  await waitScreen(s, '› New agent');
  expect(s.term.buffer.active.type).toBe('alternate');
  // Now the interface has the terminal again, and SIGTERM ends it with a clean terminal.
  process.kill(tui, 'SIGTERM');
  await waitScreen(s, 'EXIT=143');
  await restored(s);
});

test('Settings runs a paddock command, and its output stays until Enter', async () => {
  const dir = fakeWorld();
  const s = start(['--home']);
  await waitScreen(s, 'Installed: claude, codex');
  s.p.write('s');
  await waitScreen(s, '› Set up paddock');
  s.p.write('\u001b[B');
  await waitScreen(s, '› Check this computer');
  s.p.write('\r');
  const f = await waitScreen(s, 'paddock doctor ended. Press Enter to go back to paddock.');
  expect(f).toContain('FAKE doctor');
  expect(s.term.buffer.active.type).toBe('normal');
  s.p.write('\r');
  await waitScreen(s, '› New agent');
  expect(calls(dir).filter((c) => c.args[0] === 'doctor').length).toBe(1);
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('NO_COLOR: the home screen writes no colour code, and the fence is blocks', async () => {
  fakeWorld();
  const s = start(['--home'], { NO_COLOR: '1' });
  const f = await waitScreen(s, 'Installed: claude, codex');
  expect(f).toContain('▄█▄ ▄█▄ ▄█▄ ▄█▄');
  expect(f).toContain('› New agent');
  s.p.write('n');
  await waitScreen(s, 'The agent sees only this folder.');
  const codes = [...s.raw().matchAll(/\u001b\[([\d;]*)m/g)].flatMap((m) => (m[1] ?? '').split(';').map(Number));
  expect(codes.filter((c) => (c >= 30 && c <= 37) || (c >= 40 && c <= 47) || (c >= 90 && c <= 107) || c === 38 || c === 48)).toEqual([]);
  s.p.write('\u001b');
  await waitScreen(s, '› New agent');
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('--plain: the home screen is a list on the main screen, and a start comes back to it', async () => {
  const dir = fakeWorld();
  const s = start(['--home', '--plain']);
  const f = await waitScreen(s, 'Installed: claude, codex');
  expect(f).not.toMatch(/[╭│▀▄█]/);
  expect(f).toMatch(/› n +New agent/);
  expect(s.term.buffer.active.type).toBe('normal');
  expect(s.raw()).not.toContain('\u001b[?1049h');
  s.p.write('n');
  await waitScreen(s, '✓ The agent sees only this folder.');
  s.p.write('\r');
  await until(() => runs(dir).length === 1);
  await waitScreen(s, 'claude in tui ended');
  await waitScreen(s, 'New agent');
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});
