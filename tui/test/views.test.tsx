// Every view renders inside 80x24, 100x30, 120x34 and 160x48 with the
// fixture JSON, and says each state with a glyph and a word.
import { afterEach, describe, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { ACTIONS } from '../src/actions.js';
import { cells } from '../src/clean.js';
import { fakeWorld, find, renderApp, writeJson, type Rendered } from './helpers.js';

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

describe('narrow terminals', () => {
  const stripped = (f: string): string[] => f.replace(/\u001b\[[\d;]*m/g, '').split('\n');

  test('40 columns: each run shows its state, its agent and the whole place', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 40, rows: 12 });
    const lines = stripped(await r.waitFor('sandbox seatbelt'));
    for (const l of lines) expect(cells(l), l).toBeLessThanOrEqual(40);
    expect(lines.length).toBeLessThanOrEqual(12);
    const rows = lines.filter((l) => /sandbox/.test(l) && l.startsWith('│'));
    expect(rows.length).toBe(7);
    expect(rows[0]).toMatch(/◆ waits +claude +sandbox bwrap/);
    expect(rows[2]).toMatch(/▲ stuck +evil +sandbox seatbelt/);
    for (const l of rows) expect(l, l).not.toMatch(/sandbox [a-z]*…/);
    // The header has room for the tabs only: no cut name, no "…".
    expect(lines[0]).toMatch(/^ *Runs +Board +Approvals ◆3 +Mail ✉2 *$/);
  });

  test.each([40, 60, 80, 100])('%i columns: the key hints and the mouse note never run into each other', async (columns) => {
    fakeWorld();
    const r = await renderApp({ columns, rows: 20 });
    await r.waitFor('sandbox');
    for (const keys of ['', '\r', 'a']) {
      if (keys) await r.press(keys);
      await r.waitFor((f) => (keys === 'a' ? f.includes('Approval ·') : keys === '\r' ? f.includes('Run ·') : true));
      const foot = stripped(r.frame()).pop() ?? '';
      expect(cells(foot), foot).toBeLessThanOrEqual(columns);
      if (foot.includes('hold Shift')) expect(foot, foot).toMatch(/ {2,}hold Shift to select text$/);
      if (keys === '\r') await r.press('\u001b');
    }
  });

  test('a card that is higher than the screen is cut at the bottom: its lines never lie over each other', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 40, rows: 12 });
    await r.waitFor('sandbox');
    await r.press('a');
    const lines = stripped(await r.waitFor('Approval ·'));
    expect(lines.length).toBeLessThanOrEqual(12);
    const at = (s: string): number => lines.findIndex((l) => l.includes(s));
    // Each line of the card on its own row, in its order, each whole.
    expect(at('◆ Approval needed')).toBe(2);
    expect(lines[3]).toMatch(/^│ codex wants to run a command: +│$/);
    expect(at('│ exec: git push')).toBe(4);
    expect(at('[1] Allow once')).toBeGreaterThan(at('remember:'));
  });

  test('the wide layout says how many facts of the run do not fit', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 120, rows: 34 });
    const f = stripped(await r.waitFor('Output · last')).join('\n');
    expect(f).toMatch(/… \d more: ↵ opens the run/);
  });
});

describe('the layout stays where it is as cards and mail come and go', () => {
  const plain = (f: string): string => f.replace(/\u001b\[[\d;]*m/g, '');

  test('the tabs keep their place when the counts in them come and go', async () => {
    const dir = fakeWorld();
    const r = await renderApp({ columns: 100, rows: 30 });
    const before = plain(await r.waitFor((f) => f.includes('Approvals ◆3') && f.includes('Mail ✉2')));
    const places = (f: string): number[] => ['Runs ', 'Board ', 'Approvals', 'Mail'].map((t) => find(f.split('\n')[0] ?? '', ` ${t}`).x);
    writeJson(dir, 'approvals.json', { v: 1, pending: [] });
    writeJson(dir, 'mail.json', []);
    const after = plain(await r.waitFor((f) => !f.includes('◆3') && !f.includes('✉2')));
    expect(places(after)).toEqual(places(before));
  });

  test('the panes under the approval strip do not move when another card comes to the front', async () => {
    fakeWorld();
    const r = await renderApp({ columns: 120, rows: 40 });
    const first = plain(await r.waitFor((f) => f.includes('Approval · codex-alpha') && f.includes('Output · last')));
    const top = find(first, '╭─ Runs · ').y;
    // Shift+Tab: the focus goes to the strip. Down: the next card, with other lines.
    await r.press('\u001b[Z');
    await r.waitFor('←→ choose');
    for (const card of ['Approval · app-mac-123456', 'Approval · claude-alpha-71c3ac9d']) {
      await r.press('\u001b[B');
      const f = plain(await r.waitFor(card));
      expect(find(f, '╭─ Runs · ').y, card).toBe(top);
      const lines = f.split('\n');
      expect(lines.length).toBeLessThanOrEqual(40);
      for (const l of lines) expect(cells(l), l).toBeLessThanOrEqual(120);
    }
  });
});
