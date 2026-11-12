// Every view renders inside 80x24, 100x30, 120x34 and 160x48 with the
// fixture JSON, and says each state with a glyph and a word.
import { afterEach, describe, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { ACTIONS } from '../src/actions.js';
import { cells } from '../src/clean.js';
import { fakeWorld, renderApp, writeJson, type Rendered } from './helpers.js';

afterEach(() => cleanup());

const SIZES = [
  [80, 24],
  [100, 30],
  [120, 34],
  [160, 48],
] as const;

function fits(r: Rendered, columns: number, rows: number): void {
  const lines = r.lines();
  expect(lines.length).toBeLessThanOrEqual(rows);
  for (const l of lines) expect(cells(l), l).toBeLessThanOrEqual(columns);
  expect(r.frame()).not.toContain('\u001b');
}

describe.each(SIZES)('%ix%i', (columns, rows) => {
  test('runs: every state is a glyph and a word, waiting runs first, sandbox and backend named', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    const f = await r.waitFor('✖ failed');
    for (const w of ['◆ waits for you', '▲ stuck', '○ idle', '✓ done', '✖ failed', '✓ ended', 'sandbox bwrap', 'sandbox podman', 'sandbox seatbelt ']) {
      expect(f).toContain(w);
    }
    const rowsOfRuns = r.lines().filter((l) => /[◆▲○✓✖●■] (waits|stuck|idle|done|failed|ended|running|blocked)/.test(l));
    expect(rowsOfRuns[0]).toContain('◆ waits for you');
    expect(f).toMatch(/paddock · 4 running · 3 wait for you/);
    expect(f).toContain('Approvals ◆3');
    // Every run row names its sandbox and backend in full (SPLIT1 rule),
    // also in the narrow runs pane of the wide layout.
    // "sandbox seatbelt" is the longest place, the macOS default.
    expect(f).not.toMatch(/sand…|sandbox b…|sandbox p…|sandbox s…|sandbox se[a-z]*…/);
    const placed = rowsOfRuns.filter((l) => /sandbox (bwrap|podman|seatbelt) /.test(l));
    expect(placed.length, f).toBeGreaterThanOrEqual(4);
    expect(rowsOfRuns.filter((l) => l.includes('sandbox seatbelt ')).length, f).toBe(1);
    fits(r, columns, rows);
  });

  test('a run in manual approvals mode says so: a "manual" tag on its row, or its detail in the wide layout', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    await r.waitFor('✖ failed');
    const lines = r.lines();
    // No other run has the tag.
    for (const id of ['71c3ac', '0f0f0f', '0e0e0e', '0c0c0c', '0d0d0d', '0a0a0a']) {
      expect(lines.filter((l) => l.includes(id) && /\bmanual\b/.test(l))).toEqual([]);
    }
    if (columns < 120) {
      // One pane: the row of the run has the tag.
      expect(lines.filter((l) => /\bmanual\b/.test(l) && l.includes('0b0b0b')).length, r.frame()).toBe(1);
    } else {
      // The narrow runs pane cuts the text; the detail of the selected
      // run names the mode. The codex run is the second row.
      expect(r.frame()).not.toMatch(/approve +manual/);
      await r.press('j');
      const f = await r.waitFor('Run · codex-0b0b0b');
      expect(f).toMatch(/approve +manual: you approve each step/);
    }
    fits(r, columns, rows);
  });

  test('a working run says running', async () => {
    fakeWorld((dir) => {
      // Without the approval card, the board's "working" shows.
      writeJson(dir, 'approvals.json', { pending: [] });
    });
    const r = await renderApp({ columns, rows });
    const f = await r.waitFor('● running');
    expect(f).toContain('71c3ac');
    fits(r, columns, rows);
  });

  test('board', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    await r.waitFor('sandbox bwrap');
    await r.press('b');
    const f = await r.waitFor('◇ open');
    for (const w of ['◆ waiting', '● working', '▲ stuck', '○ idle', 'T-101', 'compile slots: 1 claude-alpha-71c3ac', 'alpha (4)']) {
      expect(f).toContain(w);
    }
    fits(r, columns, rows);
  });

  test('approvals', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    await r.waitFor('sandbox bwrap');
    await r.press('a');
    const f = await r.waitFor('[3] Deny');
    expect(f).toContain('Approval · codex-alpha-0b0b0b11 · sandbox');
    expect(f).toContain('exec: git push origin main --force-with-lease');
    expect(f).toContain('asked 12 s ago');
    expect(f).toContain('remember: this request only');
    expect(f).toContain('◆ waits');
    fits(r, columns, rows);
  });

  test('mail and board columns never run together', async () => {
    fakeWorld((dir) => {
      writeJson(dir, 'mail.json', [
        { id: 'claude-alpha-40d3579a', short: '40d3579a', state: 'running', starter: 'sandbox', up: 1, unread_up: 1, unread_down: 0, last: 1000 },
      ]);
      writeJson(dir, 'board.json', {
        version: 1,
        projects: [{ name: 'alpha', path: '/src/alpha', tickets: [{ id: 'TICKET12', title: 'a long title that fills', state: 'open', runs: [] }], runs_without_ticket: [] }],
      });
    });
    const r = await renderApp({ columns, rows });
    await r.waitFor('sandbox bwrap');
    await r.press('m');
    let f = await r.waitFor('40d3579a');
    expect(f).toContain('40d3579a from sandbox');
    expect(f).toContain('running ');
    await r.press('b');
    f = await r.waitFor('TICKET12');
    expect(f).toMatch(/TICKET12 a long title/);
    expect(f).toMatch(/◇ open\s+TICKET12/);
    fits(r, columns, rows);
  });

  test('the header counts only the unread mail of runs that still run', async () => {
    fakeWorld((dir) => {
      writeJson(dir, 'mail.json', [
        { id: 'claude-alpha-71c3ac9d', short: '71c3ac', state: 'running', starter: 'user', up: 3, unread_up: 2, unread_down: 0, last: 1000 },
        { id: 'codex-new-1a1a1a1a', short: '1a1a1a', state: 'starting', starter: 'user', up: 1, unread_up: 1, unread_down: 0, last: 1000 },
        { id: 'codex-beta-0d0d0d55', short: '0d0d0d', state: 'ended (1)', starter: 'user', up: 9000, unread_up: 8334, unread_down: 0, last: 900 },
      ]);
    });
    const r = await renderApp({ columns, rows });
    const f = await r.waitFor('Mail ✉3');
    expect(f).not.toContain('8337');
    fits(r, columns, rows);
  });

  test('mail, a mailbox and a message', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    await r.waitFor('sandbox bwrap');
    await r.press('m');
    let f = await r.waitFor('✉ 2 new');
    expect(f).toContain('ended (1)');
    fits(r, columns, rows);
    await r.press('\r');
    f = await r.waitFor('Which ledger format?');
    expect(f).toContain('#3');
    fits(r, columns, rows);
    await r.press('\r');
    f = await r.waitFor('Option two: sqlite');
    expect(f).toContain('about: notes/ledger.md');
    expect(f).not.toContain('http://evil');
    fits(r, columns, rows);
  });

  test('run detail with the output tail', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    await r.waitFor('sandbox bwrap');
    await r.press('\r');
    const f = await r.waitFor('test result: ok. 14 passed');
    expect(f).toContain('Run · claude-71c3ac · sandbox bwrap');
    expect(f).toContain('◆ waits for you');
    expect(f).toContain('project  /src/alpha');
    fits(r, columns, rows);
    await r.press('\u001b');
    await r.waitFor((x) => !x.includes('Output ·') || columns >= 120);
  });

  test('help, palette and the new run form', async () => {
    fakeWorld();
    const r = await renderApp({ columns, rows });
    await r.waitFor('sandbox bwrap');
    await r.press('?');
    let f = await r.waitFor('Help · every key');
    fits(r, columns, rows);
    // Every action is in the help, also when it has to scroll.
    const seen = new Set<string>();
    for (let i = 0; i < 30; i++) {
      for (const l of r.lines()) seen.add(l);
      await r.press('j');
    }
    const all = [...seen].join('\n');
    for (const a of ACTIONS) expect(all).toContain(a.label);
    await r.press('\u001b');
    await r.press(':');
    f = await r.waitFor('Command palette');
    expect(f).toContain('stop the selected run');
    fits(r, columns, rows);
    await r.press('\u001b');
    await r.press('n');
    f = await r.waitFor('New run');
    expect(f).toContain('Prompt file');
    expect(f).toContain('Approvals');
    expect(f).toContain('Ctrl+A');
    fits(r, columns, rows);
  });

  test('light theme and plain mode keep the words', async () => {
    fakeWorld();
    const light = await renderApp({ columns, rows, theme: 'light' });
    await light.waitFor('✖ failed');
    fits(light, columns, rows);
    light.unmount();
    const plain = await renderApp({ columns, rows, plain: true, mouse: false });
    const f = await plain.waitFor('✖ failed');
    expect(f).not.toContain('╭');
    expect(f).toContain('◆ waits for you');
    fits(plain, columns, rows);
  });
});

test('the place column keeps "sandbox seatbelt" whole at every width from 80 to 160 columns', async () => {
  const cut: number[] = [];
  for (let columns = 80; columns <= 160; columns++) {
    fakeWorld();
    const r = await renderApp({ columns, rows: 30 });
    const f = await r.waitFor('✖ failed');
    if (!f.includes('sandbox seatbelt ') || /sandbox [a-z]*…/.test(f)) cut.push(columns);
    cleanup();
  }
  expect(cut).toEqual([]);
}, 120_000);
