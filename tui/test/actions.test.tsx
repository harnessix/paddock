// Actions go to paddock's commands: new run, stop with a confirm, the
// split view or its fallback, mail, the filter and the palette.
import { afterEach, expect, test, vi } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { paddock, PaddockError, UnsupportedError, type PaddockApi } from '../src/paddock.js';
import { actions, calls, fakeWorld, renderApp, rounds, sleep, until, writeJson } from './helpers.js';

afterEach(() => {
  cleanup();
  delete process.env.FAKE_PADDOCK_SPLIT;
});

async function typeText(r: { press: (s: string) => Promise<void> }, text: string) {
  for (const ch of text) await r.press(ch);
}

test('n starts a detached run with harness, project and prompt file', async () => {
  const dir = fakeWorld();
  const project = mkdtempSync(join(tmpdir(), 'tui-project-'));
  writeFileSync(join(project, 'task.md'), 'do it');
  const r = await renderApp({ columns: 80, rows: 24, cwd: project });
  await r.waitFor('sandbox bwrap');
  await r.press('n');
  await r.waitFor('installed: claude, codex');
  await typeText(r, 'claude');
  await r.press('\t');
  await r.press('\t');
  await typeText(r, 'missing.md');
  await r.press('\r');
  await r.waitFor('does not exist');
  for (let i = 0; i < 'missing.md'.length; i++) await r.press('\u007f');
  await typeText(r, 'task.md');
  await r.press('\r');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['run', 'claude', project, '--detach', '--prompt-file', join(project, 'task.md')]);
  await r.waitFor('ok run');
});

test('s asks first; y stops, another key keeps the run', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('s');
  await r.waitFor('Stop claude-71c3ac? Its agent ends.');
  await r.press('x');
  await r.waitFor('Not stopped');
  expect(actions(dir)).toEqual([]);
  await r.press('s');
  await r.waitFor('Stop claude-71c3ac?');
  await r.press('y');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['stop', 'claude-alpha-71c3ac9d']);
});

test('a failed stop shows the error of paddock, cleaned', async () => {
  fakeWorld((dir) =>
    writeJson(dir, 'ps.json', { sandboxes: [{ id: 'fail-me', short: 'fa11ed', harness: 'claude', backend: 'bwrap', started: 10 }], ended: [] }),
  );
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('fa11ed');
  await r.press('s');
  await r.press('y');
  const f = await r.waitFor('Could not stop');
  expect(f).toContain('error: no sandbox with that id');
});

test('v without paddock split opens the view and says so; with split it splits the marked runs', async () => {
  let dir = fakeWorld();
  let r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.waitFor('no paddock split');
  await r.press('v');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['view', 'claude-alpha-71c3ac9d']);
  await r.waitFor('paddock split is not in this paddock');
  r.unmount();

  process.env.FAKE_PADDOCK_SPLIT = '1';
  dir = fakeWorld();
  // The marks must wait for the answer of `paddock split --help`.
  let known = false;
  const api = {
    ...paddock,
    hasSplit: async () => {
      const v = await paddock.hasSplit();
      known = true;
      return v;
    },
  };
  r = await renderApp({ columns: 80, rows: 24, api });
  await r.waitFor('sandbox bwrap');
  await until(() => known);
  await r.press(' ');
  await r.press('j');
  await r.press(' ');
  await r.waitFor('2 marked for split');
  await r.press('v');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]?.args).toEqual(['split', 'claude-alpha-71c3ac9d', 'codex-alpha-0b0b0b11']);
});

test('w writes to the agent of a run through stdin', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('w');
  await r.waitFor('to claude-alpha-71c3ac9d');
  await typeText(r, 'please rebase');
  await r.press('\r');
  await until(() => actions(dir).length > 0);
  expect(actions(dir)[0]).toEqual({ args: ['mail', 'send', 'claude-alpha-71c3ac9d', '-'], stdin: 'please rebase' });
});

test('/ filters the runs and Esc clears the filter', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox podman');
  await r.press('/');
  await typeText(r, 'grok');
  let f = await r.waitFor('filter "grok"');
  expect(f).toContain('0f0f0f');
  expect(f).not.toContain('71c3ac');
  await r.press('\u001b');
  f = await r.waitFor('71c3ac');
  expect(f).not.toContain('filter "grok"');
});

test('the palette lists every action and runs the chosen one', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('\u0010'); // Ctrl+P
  await r.waitFor('Command palette');
  await typeText(r, 'board');
  await r.press('\r');
  await r.waitFor('◇ open');
});

test('read-only hides every action', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24, readonly: true });
  const f = await r.waitFor('sandbox bwrap');
  expect(f).toContain('read-only');
  expect(f).not.toContain('n new');
  for (const k of ['n', 's', 'y', 'w']) await r.press(k);
  await r.press(':');
  const p = await r.waitFor('Command palette');
  expect(p).not.toContain('new run');
  expect(p).not.toContain('stop the selected run');
  await rounds(dir, 3);
  expect(actions(dir)).toEqual([]);
});

test('q goes back and quits on the first screen', async () => {
  fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('b');
  await r.waitFor('◇ open');
  await r.press('q');
  await r.waitFor('sandbox podman');
});

test('a second s does not confirm the stop: only y or the button does', async () => {
  const dir = fakeWorld();
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('sandbox bwrap');
  await r.press('s');
  await r.waitFor('Stop claude-71c3ac?');
  await r.press('s');
  await r.waitFor('Not stopped');
  await rounds(dir, 3);
  expect(actions(dir)).toEqual([]);
});

test('a command that this paddock lacks: one line that says so, and no more calls', async () => {
  const dir = fakeWorld((d) => writeFileSync(join(d, 'unsupported-board'), ''));
  const r = await renderApp({ columns: 100, rows: 24 });
  const f = await r.waitFor('needs a newer paddock');
  expect(f).toContain('paddock board: needs a newer paddock: `paddock board --json`');
  expect(f).not.toContain('unrecognized subcommand');
  // Eight rounds of approvals (300 ms each) in the real program.
  await rounds(dir, 8);
  const boards = calls(dir).filter((c) => c.args.join(' ') === 'board --json');
  expect(boards.length).toBe(1);
  expect(r.frame()).toContain('needs a newer paddock');
  r.unmount();

  // On the fake clock, for longer than any backoff: after one failure,
  // board would be due again 3 s later, after two 6 s later. 30 s cover
  // both, so only the "off" flag keeps the count at one.
  let boardReads = 0;
  const board = async () => {
    boardReads += 1;
    throw new UnsupportedError('needs a newer paddock: `paddock board --json`');
  };
  await paced({ ps: async () => ({ sandboxes: [], ended: [] }), approvals: async () => ({ pending: [] }), board, mailBoxes: async () => [] } as Partial<PaddockApi>, 300, 30_000);
  expect(boardReads).toBe(1);
});

test('the output tail and the messages stop polling a command that this paddock lacks', async () => {
  const { poll } = await import('../src/data.js');
  vi.useFakeTimers({ toFake: [...FAKED] });
  try {
    let reads = 0;
    const stop = poll(async () => {
      reads += 1;
      throw new UnsupportedError('needs a newer paddock');
    }, 2000);
    await vi.advanceTimersByTimeAsync(120_000);
    stop();
    expect(reads).toBe(1);
    // Another error still polls again, slower each time.
    let failed = 0;
    const stop2 = poll(async () => {
      failed += 1;
      throw new PaddockError('error: the state is locked');
    }, 2000);
    await vi.advanceTimersByTimeAsync(120_000);
    stop2();
    expect(failed).toBeGreaterThan(1);
  } finally {
    vi.useRealTimers();
  }
});

function count(dir: string, cmd: string): number {
  return calls(dir).filter((c) => c.args.join(' ') === cmd).length;
}

// The pace of the reads is a matter of timers, so these tests drive
// useSnapshot with a fake clock and reads in memory: the counts are the
// same on a busy CI runner as on an idle one.
const FAKED = ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'] as const;

async function paced(api: Partial<PaddockApi>, intervalMs: number, ms: number): Promise<void> {
  const { useSnapshot } = await import('../src/data.js');
  const React = await import('react');
  const { render } = await import('ink-testing-library');
  const full = { ...paddock, ...api } as PaddockApi;
  function Probe(): React.JSX.Element {
    useSnapshot(full, intervalMs);
    return React.createElement(React.Fragment);
  }
  vi.useFakeTimers({ toFake: [...FAKED] });
  try {
    const inst = render(React.createElement(Probe));
    await vi.advanceTimersByTimeAsync(ms);
    inst.unmount();
  } finally {
    vi.useRealTimers();
  }
}

function counter<T>(v: () => T): { n: () => number; read: () => Promise<T> } {
  let n = 0;
  return {
    n: () => n,
    read: async () => {
      n += 1;
      return v();
    },
  };
}

test('board and mail are read less often than ps and approvals', async () => {
  const ps = counter(() => ({ sandboxes: [], ended: [] }));
  const approvals = counter(() => ({ pending: [] }));
  const board = counter(() => ({ projects: [] }));
  const mail = counter(() => []);
  await paced({ ps: ps.read, approvals: approvals.read, board: board.read, mailBoxes: mail.read } as Partial<PaddockApi>, 300, 3000);
  // 300 ms rounds: about 10 reads of approvals, board and mail every 1.5 s.
  expect(approvals.n()).toBeGreaterThanOrEqual(6);
  expect(board.n()).toBeLessThanOrEqual(3);
  expect(mail.n()).toBeLessThanOrEqual(3);
});

test('a read that fails waits longer each time, and an unchanged one slows down', async () => {
  // The error reaches the screen of the real program.
  const dir = fakeWorld((d) => writeFileSync(join(d, 'fail-approvals'), ''));
  const r = await renderApp({ columns: 80, rows: 24 });
  await r.waitFor('paddock approvals: error: the state is locked');
  expect(count(dir, 'approvals --json')).toBeGreaterThanOrEqual(1);
  r.unmount();

  // The pace, on the fake clock.
  const ps = counter(() => ({ sandboxes: [], ended: [] }));
  let approvalReads = 0;
  const approvals = async () => {
    approvalReads += 1;
    throw new PaddockError('error: the state is locked');
  };
  await paced({ ps: ps.read, approvals, board: async () => ({ projects: [] }), mailBoxes: async () => [] } as Partial<PaddockApi>, 300, 3500);
  // Without backoff: about 12 reads in 3.5 s. With it: 300, 600, 1200, 2400 ms.
  expect(approvalReads).toBeLessThanOrEqual(5);
  // ps answers the same every time: after 3 rounds at 300 ms it slows to
  // 600 ms, after 6 to 1200 ms.
  expect(ps.n()).toBeLessThanOrEqual(8);
  expect(ps.n()).toBeGreaterThanOrEqual(3);
});

test('locked approvals back off for a few seconds at most, so a new card shows soon', async () => {
  // The times of the reads, on the fake clock.
  const at: number[] = [];
  const approvals = async () => {
    at.push(Date.now());
    throw new PaddockError('error: the state is locked');
  };
  await paced({ ps: async () => ({ sandboxes: [], ended: [] }), approvals, board: async () => ({ projects: [] }), mailBoxes: async () => [] } as Partial<PaddockApi>, 2000, 60_000);
  // Uncapped: 4, 8, 16 and 32 s apart, 5 reads in a minute. Capped at
  // 5 s: about 12. No gap between two reads passes the cap and a tick.
  expect(at.length).toBeGreaterThanOrEqual(10);
  const gaps = at.slice(1).map((t, i) => t - (at[i] as number));
  expect(Math.max(...gaps)).toBeLessThanOrEqual(5000 + 1000);
});

test('a slow round does not start the next one at once', async () => {
  let running = 0;
  let most = 0;
  const starts: number[] = [];
  const ends: number[] = [];
  const slow = <T,>(v: T) => async (): Promise<T> => {
    running += 1;
    most = Math.max(most, running);
    starts.push(Date.now());
    await sleep(700);
    running -= 1;
    ends.push(Date.now());
    return v;
  };
  await paced(
    { ps: slow({ sandboxes: [], ended: [] }), approvals: slow({ pending: [] }), board: slow({ projects: [] }), mailBoxes: slow([]) } as Partial<PaddockApi>,
    100,
    1600,
  );
  // Rounds of 700 ms with ticks every 50 ms: never two at once, and a
  // read starts again only its delay (100 ms) after the round ended, not
  // back-to-back. The times are of the fake clock.
  expect(most).toBeLessThanOrEqual(4);
  const firstEnd = Math.min(...ends);
  const later = starts.filter((t) => t > firstEnd - 5);
  expect(later.length).toBeGreaterThan(0);
  expect(Math.min(...later) - firstEnd).toBeGreaterThanOrEqual(90);
});
