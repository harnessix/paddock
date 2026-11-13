// The main views in English, German, Japanese and Simplified Chinese, at
// 80x24 and 120x40. Each one is a snapshot file in __l10n__/, and no line
// is wider than the terminal: CJK text takes two cells per character, and
// a text that does not fit is cut with an ellipsis, never wrapped out of
// its box.
import { afterEach, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { cells } from '../src/clean.js';
import { setLang, type Lang } from '../src/i18n/index.js';
import { demoCopy } from './files-demo.js';
import { fakeWorld, renderApp, rounds, sleep, type Rendered } from './helpers.js';

// The ages on the screen must not change while a test runs. The fixtures
// give ages of 100 s and more, which show in minutes. The requests are
// 12 s to 30 s old: here they are 11 minutes old, so they show in minutes too.
function oldRequests(dir: string): void {
  const p = join(dir, 'approvals.json');
  const a = JSON.parse(readFileSync(p, 'utf8')) as { pending: Record<string, unknown>[] };
  for (const r of a.pending) {
    r.created = 660;
    r.age_s = 660;
    if (r.connect && typeof r.connect === 'object') (r.connect as Record<string, unknown>).asked_at = 660;
  }
  writeFileSync(p, JSON.stringify(a));
}
afterEach(() => {
  cleanup();
  setLang('en');
});

const LANGS: Lang[] = ['en', 'de', 'ja', 'zh-Hans'];
const SIZES = [
  [80, 24],
  [120, 40],
] as const;

/** The views, and the keys that open each one from the runs. */
const VIEWS: [name: string, keys: string[]][] = [
  ['runs', []],
  ['run', ['\r']],
  ['board', ['b']],
  ['ranch', ['b', 'L']],
  ['approvals', ['a']],
  ['mail', ['m']],
  ['help', ['?']],
  ['palette', [':']],
  ['new-run', ['n']],
  ['home', ['H']],
  // The files of a run with a copy (7c1e42), and the diff of its first file.
  ['files', ['f']],
  ['files-diff', ['f', '\r']],
];

/** Wait until two rounds of paddock calls leave the screen as it was. */
async function settle(r: Rendered, dir: string): Promise<string> {
  let prev = '';
  for (let i = 0; i < 40; i++) {
    await rounds(dir, 1, 'ps --json');
    const f = r.frame();
    if (f === prev) return f;
    prev = f;
  }
  return prev;
}

/** The frame without its styles (bold, dim, underline, inverse). */
function strip(s: string): string {
  return s.replace(/\u001b\[[\d;]*m/g, '');
}

function fits(frame: string, columns: number, rows: number): void {
  const lines = frame.split('\n');
  expect(lines.length).toBeLessThanOrEqual(rows);
  for (const l of lines) expect(cells(l), l).toBeLessThanOrEqual(columns);
  expect(frame).not.toContain('\u001b');
}

for (const l of LANGS) {
  for (const [columns, rows] of SIZES) {
    for (const [name, keys] of VIEWS) test(`${l} ${columns}x${rows} ${name}`, async () => {
      setLang(l);
      const copy = name.startsWith('files');
      const dir = fakeWorld((d) => {
        oldRequests(d);
        if (copy) demoCopy(d);
      });
      const r = await renderApp({ columns, rows });
      // The id of a run is the same in every language.
      await r.waitFor(copy ? '7c1e42' : '71c3ac');
      await settle(r, dir);
      for (const k of keys) {
        await r.press(k);
        await settle(r, dir);
      }
      // L says what it did in a toast, for 5 s: wait until it is gone.
      if (name === 'ranch') {
        await sleep(5500);
        await settle(r, dir);
      }
      const f = strip(r.frame());
      fits(f, columns, rows);
      await expect(f + '\n').toMatchFileSnapshot(`__l10n__/${l}/${name}-${columns}x${rows}.snap`);
    });
  }
}
