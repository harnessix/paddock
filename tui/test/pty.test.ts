// The built program in a real pseudo-terminal, against the fake paddock:
// keys work, the screen fits, and every way out restores the terminal
// (main screen, mouse reports off, cursor on, cooked mode).
import { afterEach, expect, test } from 'vitest';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import pty from 'node-pty';
import xterm from '@xterm/headless';
import { actions, fakeWorld, HANG_MS, rounds, sleep, until } from './helpers.js';

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
  await waitScreen(s, 'codex ◆');
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

test('rooms in a 24-bit terminal: the art animates in place, and rests while the terminal has no focus', async () => {
  fakeWorld();
  const s = start([], { COLORTERM: 'truecolor', TERM: 'xterm-256color' });
  const first = await waitScreen(s, '1/7 · ←→↑↓ move');
  expect(first).toContain('◆ waits for you');
  expect(first).toContain('▀');
  expect(s.raw()).toMatch(/\u001b\[38;2;\d+;\d+;\d+/);
  // The program asked for focus reports.
  expect(s.raw()).toContain('\u001b[?1004h');
  // Animation steps: the cursor is saved, cells are drawn, the cursor goes back.
  const steps = () => (s.raw().match(/\u001b7\u001b\[\d+;\d+H/g) ?? []).length;
  await until(() => steps() >= 3);
  // The screen still has the words after the steps: they draw only art cells.
  expect(s.screen()).toContain('◆ waits for you');
  s.p.write('\u001b[O');
  await sleep(300);
  const n = steps();
  await sleep(800);
  expect(steps()).toBe(n);
  s.p.write('\u001b[I');
  await until(() => steps() > n);
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
  // Focus reports off again, and no art after the program left the alternate screen.
  const raw = s.raw();
  expect(raw.lastIndexOf('\u001b[?1004l')).toBeGreaterThan(raw.lastIndexOf('\u001b[?1004h'));
  expect(raw.slice(raw.lastIndexOf('\u001b[?1049l'))).not.toMatch(/\u001b7\u001b\[\d+;\d+H/);
});

/** A world in which every time is hours old: no word of time changes during a test. */
function oldWorld(): string {
  return fakeWorld((dir) => {
    const edit = (name: string, f: (v: Record<string, unknown>) => void): void => {
      const walk = (v: unknown): void => {
        if (Array.isArray(v)) v.forEach(walk);
        else if (v && typeof v === 'object') {
          f(v as Record<string, unknown>);
          Object.values(v).forEach(walk);
        }
      };
      const p = join(dir, name);
      const v = JSON.parse(readFileSync(p, 'utf8')) as unknown;
      walk(v);
      writeFileSync(p, JSON.stringify(v));
    };
    edit('ps.json', (o) => {
      for (const k of ['started', 'ended', 'idle_since']) if (typeof o[k] === 'number') o[k] = 4000;
    });
    edit('approvals.json', (o) => {
      if (typeof o.age_s === 'number') o.age_s = 4000;
    });
  });
}

test('the real program draws by cell: one clear, no erased line, nothing while nothing changes, a few cells for a key', async () => {
  const dir = oldWorld();
  const s = start([], { COLORTERM: 'truecolor', TERM: 'xterm-256color' });
  await waitScreen(s, '1/7 · ←→↑↓ move');
  await waitScreen(s, 'no paddock split');
  // In the background: the animation rests, and paddock is still read.
  s.p.write('\u001b[O');
  await rounds(dir, 2);
  const quiet = s.raw().length;
  await rounds(dir, 4);
  // Four more reads of paddock with the same answers: not one byte for the terminal.
  expect(s.raw().length).toBe(quiet);
  // A key: the selection moves from one card to the next. Only their borders and titles are written.
  s.p.write('\u001b[C');
  await waitScreen(s, '2/7 · ←→↑↓ move');
  const key = s.raw().slice(quiet);
  expect(key.length).toBeLessThan(2500);
  expect(key).not.toContain('▀');
  expect(key).toContain('\u001b[?2026h');
  // The frame of xterm is the frame of the program: the cards and their words are whole.
  const screen = s.screen();
  expect(screen).toContain('╭─ › codex-0b0b0b');
  expect(screen).toContain('╭─ claude-71c3ac');
  expect(screen).toContain('◆ waits for you · 1 h');
  expect(screen.split('\n').length).toBe(24);
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
  const raw = s.raw();
  // The screen is cleared once, when the program starts. No line is ever erased: no flicker.
  expect(raw.match(/\u001b\[2J/g) ?? []).toHaveLength(1);
  expect(raw.slice(0, raw.lastIndexOf('\u001b[?1049l'))).not.toMatch(/\u001b\[[012]?K|\u001b\[[01]?J/);
  // Bracketed paste is off again.
  expect(raw.lastIndexOf('\u001b[?2004l')).toBeGreaterThan(raw.lastIndexOf('\u001b[?2004h'));
});

test('a new size of the terminal: the frame is drawn anew in it', async () => {
  oldWorld();
  const s = start([], { COLORTERM: 'truecolor', TERM: 'xterm-256color' });
  await waitScreen(s, '1/7 · ←→↑↓ move');
  s.term.resize(120, 34);
  s.p.resize(120, 34);
  await waitScreen(s, 'Approval · codex-alpha-0b0b0b11');
  const wide = await waitScreen(s, '╭─ grok-0f0f0f');
  expect(wide.split('\n')[0]).toMatch(/^paddock · 4 running.{80,}Mail ✉2$/);
  // Four rooms in a row, each with its character.
  expect(wide).toMatch(/╭─ › claude-71c3ac ─+╮╭─ codex-0b0b0b ─+╮╭─ evil-0e0e0e ─+╮╭─ grok-0f0f0f ─+╮/);
  // Under 80 columns: the list, and no cell of the rooms is left over.
  s.term.resize(70, 20);
  s.p.resize(70, 20);
  const narrow = await waitScreen(s, '╭─ Runs · 4 live, 3 ended ─');
  await until(() => !s.screen().includes('▀'));
  expect(narrow.split('\n').length).toBe(20);
  // Another program writes over the screen. Ctrl+L draws every cell again.
  const clears = (): number => (s.raw().match(/\u001b\[2J/g) ?? []).length;
  const n = clears();
  s.term.write('\u001b[2;1H' + 'X'.repeat(70));
  await until(() => s.screen().includes('XXXXXXXX'));
  s.p.write('\u000c');
  await until(() => clears() === n + 1);
  await until(() => !s.screen().includes('XXXXXXXX'));
  expect(s.screen()).toContain('╭─ Runs · 4 live, 3 ended ─');
  s.p.write('Q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
});

test('the ranch in the real program: the painter draws it, Enter zooms in, Esc out, and q leaves a clean terminal', async () => {
  const dir = fakeWorld();
  const s = start([]);
  await waitScreen(s, 'sandbox bwrap');
  s.p.write('b');
  const ranch = await waitScreen(s, 'codex ◆');
  const lines = ranch.split('\n');
  expect(lines.length).toBe(24);
  expect(lines[1]).toMatch(/^Ranch +◆ 3 ask you/);
  // The art: half blocks in the 256 colours of this terminal.
  expect(ranch).toContain('▀');
  expect(s.raw()).toMatch(/\u001b\[[\d;]*38;5;\d+/);
  expect(ranch).toContain('codex ◆');
  // The world reads its JSON while it shows.
  await until(() => ['device', 'kv', 'files', 'forage', 'horse'].every((c) => actions(dir).length === 0 && readFileSync(join(dir, 'calls.jsonl'), 'utf8').includes(`"${c}"`)));
  s.p.write('\r');
  await waitScreen(s, 'Ranch › alpha');
  await waitScreen(s, '› codex-0b0b0b ◆ asks you');
  s.p.write('\r');
  await waitScreen(s, 'Ranch › alpha › codex-0b0b0b');
  // The zoom opens the picture in four frames: wait for its last words, not for a time.
  await waitScreen(s, 'none unread');
  const room = await waitScreen(s, 'git push');
  expect(room).toMatch(/Internet +model only/);
  expect(room).toMatch(/Asks you +git push/);
  // The door: the card of the request, beside the room.
  s.p.write('\r');
  await waitScreen(s, '[1] Allow once');
  s.p.write('1');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['approvals', 'answer', 'r-1-net', 'once']);
  // The answer takes the card away. Then q goes out, one stop at a time.
  await waitScreen(s, 'Asks you     nothing now');
  expect(s.screen()).not.toContain('[1] Allow once');
  s.p.write('q');
  await waitScreen(s, 'No notes yet');
  s.p.write('q');
  await waitScreen(s, 'codex ◆');
  s.p.write('q');
  await waitScreen(s, 'sandbox bwrap');
  s.p.write('q');
  await waitScreen(s, 'EXIT=0');
  await restored(s);
  expect(s.stderr()).toBe('');
});
