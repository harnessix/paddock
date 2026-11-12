import React from 'react';
import { copyFileSync, mkdtempSync, readdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { render } from 'ink-testing-library';
import { App } from '../src/app.js';
import { paddock } from '../src/paddock.js';
import { makeTheme, ThemeContext } from '../src/theme.js';
import { ArtContext, ArtLayer } from '../src/art-layer.js';

const here = dirname(fileURLToPath(import.meta.url));
export const FAKE = join(here, 'fake-paddock.mjs');
export const FIXTURES = join(here, 'fixtures');

export interface Call {
  args: string[];
  stdin: string;
}

/** A folder with the fixture JSON, and the fake paddock pointed at it. */
export function fakeWorld(edit?: (dir: string) => void): string {
  const dir = mkdtempSync(join(tmpdir(), 'paddock-tui-'));
  for (const f of readdirSync(FIXTURES)) copyFileSync(join(FIXTURES, f), join(dir, f));
  edit?.(dir);
  relativeTimes(dir);
  process.env.PADDOCK_BIN = FAKE;
  process.env.FAKE_PADDOCK_DIR = dir;
  return dir;
}

// Fixture times below 10^6 mean "this many seconds ago".
function relativeTimes(dir: string): void {
  const now = Math.floor(Date.now() / 1000);
  const keys = new Set(['started', 'ended', 'idle_since', 'ts', 'last', 'created', 'frozen_since']);
  const walk = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === 'object') {
      const o: Record<string, unknown> = {};
      for (const [k, x] of Object.entries(v)) o[k] = keys.has(k) && typeof x === 'number' && x < 1e6 ? now - x : walk(x);
      return o;
    }
    return v;
  };
  for (const f of readdirSync(dir)) {
    if (!f.endsWith('.json')) continue;
    writeFileSync(join(dir, f), JSON.stringify(walk(JSON.parse(readFileSync(join(dir, f), 'utf8')))));
  }
}

export function writeJson(dir: string, name: string, v: unknown): void {
  writeFileSync(join(dir, name), JSON.stringify(v));
}

export function calls(dir: string): Call[] {
  const p = join(dir, 'calls.jsonl');
  if (!existsSync(p)) return [];
  return readFileSync(p, 'utf8')
    .split('\n')
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Call);
}

/** The calls that change something (not the reads of the refresh). */
export function actions(dir: string): Call[] {
  const reads = new Set(['ps --json', 'board --json', 'approvals --json', 'mail ls --json', 'list --json', 'split --help']);
  return calls(dir).filter((c) => {
    const a = c.args.join(' ');
    if (reads.has(a)) return false;
    if (c.args[0] === 'view' && c.args.includes('--tail')) return false;
    if (c.args[0] === 'mail' && (c.args[1] === 'ls' || c.args[1] === 'show')) return false;
    return true;
  });
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * The one bound of every wait: it only catches a hang. A busy CI runner
 * can need seconds for one fake paddock call (each is a new node), so a
 * wait ends on the event it waits for, never on a short clock.
 */
export const HANG_MS = 60_000;

export interface Rendered {
  frame: () => string;
  lines: () => string[];
  press: (s: string) => Promise<void>;
  waitFor: (pred: string | ((f: string) => boolean), ms?: number) => Promise<string>;
  unmount: () => void;
}

export async function renderApp(opts: {
  columns?: number;
  rows?: number;
  readonly?: boolean;
  mouse?: boolean;
  theme?: 'dark' | 'light';
  plain?: boolean;
  /** No colour codes (NO_COLOR). */
  mono?: boolean;
  cwd?: string;
  api?: typeof paddock;
  /** The art layer: none by default, so the runs show as the list. */
  art?: ArtLayer;
}): Promise<Rendered> {
  const columns = opts.columns ?? 80;
  const rows = opts.rows ?? 24;
  const art = opts.art ?? new ArtLayer({ mode: 'none', sink: null, screen: () => ({ columns, rows }) });
  const inst = render(
    <ThemeContext.Provider value={makeTheme(opts.theme ?? 'dark', opts.plain ?? false, opts.mono ?? false)}>
      <ArtContext.Provider value={art}>
        <App api={opts.api ?? paddock} readonly={opts.readonly} initialMouse={opts.mouse ?? true} intervalMs={300} cwd={opts.cwd} size={{ columns, rows }} />
      </ArtContext.Provider>
    </ThemeContext.Provider>,
  );
  Object.defineProperty(inst.stdout, 'columns', { value: columns, configurable: true });
  Object.defineProperty(inst.stdout, 'rows', { value: rows, configurable: true });
  inst.stdout.emit('resize');
  const frame = () => inst.lastFrame() ?? '';
  const r: Rendered = {
    frame,
    lines: () => frame().split('\n'),
    press: async (s: string) => {
      inst.stdin.write(s);
      await sleep(30);
    },
    waitFor: async (pred, ms = HANG_MS) => {
      const test = typeof pred === 'string' ? (f: string) => f.includes(pred) : pred;
      if (frame().includes('ERROR ')) throw new Error(frame());
      const end = Date.now() + ms;
      while (Date.now() < end) {
        if (test(frame())) return frame();
        await sleep(25);
      }
      throw new Error(`timed out waiting for ${String(pred)}; last frame:\n${frame()}`);
    },
    unmount: () => inst.unmount(),
  };
  return r;
}

/** The SGR report of a left click at 0-based column x and row y. */
export function click(x: number, y: number): string {
  return `\u001b[<0;${x + 1};${y + 1}M\u001b[<0;${x + 1};${y + 1}m`;
}

/** Where a text first stands in the frame: 0-based column and row. */
export function find(frame: string, text: string): { x: number; y: number } {
  const lines = frame.split('\n');
  for (let y = 0; y < lines.length; y++) {
    const x = Array.from(lines[y] as string).join('').indexOf(text);
    if (x >= 0) return { x: Array.from((lines[y] as string).slice(0, x)).length, y };
  }
  throw new Error(`"${text}" is not on the screen:\n${frame}`);
}

/**
 * Wait until the fake paddock got `n` more calls of `cmd` than now: the
 * app went through that many more rounds. A test that checks that
 * nothing happened waits for rounds, not for a time.
 */
export async function rounds(dir: string, n: number, cmd = 'approvals --json'): Promise<void> {
  const count = () => calls(dir).filter((c) => c.args.join(' ') === cmd).length;
  const start = count();
  await until(() => count() >= start + n);
}

export async function until(pred: () => boolean, ms = HANG_MS): Promise<void> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (pred()) return;
    await sleep(25);
  }
  throw new Error(`timed out after ${ms} ms waiting for ${pred.toString()}`);
}
