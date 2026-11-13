// The look of the file manager: the screens T1 to T5 of the design
// (.paddock/design/COPY1-tui-files.md) at 80 and at 120 columns, and with
// NO_COLOR. Each picture is a snapshot of its text, and a file of
// test/__screens__ with the colour codes of a terminal:
// `cat test/__screens__/files-t1-80.ans` shows it.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeAll, beforeEach, describe, expect, test, vi } from 'vitest';
import { cells } from '../src/clean.js';

let h: typeof import('./helpers.js');
let demo: typeof import('./files-demo.js');
let lib: typeof import('ink-testing-library');

const SCREENS = join(dirname(fileURLToPath(import.meta.url)), '__screens__');
const HOME = process.env.HOME;

beforeAll(async () => {
  // Colours are chosen when chalk loads: force them before the imports.
  process.env.FORCE_COLOR = '1';
  process.env.TZ = 'UTC';
  lib = await import('ink-testing-library');
  h = await import('./helpers.js');
  demo = await import('./files-demo.js');
});
beforeEach(() => {
  // The pictures show times: the clock of the test stands still.
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date(demo.NOW));
});
afterEach(() => {
  lib.cleanup();
  vi.useRealTimers();
  process.env.HOME = HOME;
});

const strip = (frame: string): string => frame.replace(/\u001b\[[\d;]*m/g, '');

/** The picture as a terminal gets it, as a file of test/__screens__. Returns its text. */
function save(name: string, frame: string): string {
  mkdirSync(SCREENS, { recursive: true });
  writeFileSync(join(SCREENS, `${name}.ans`), frame + '\n');
  return strip(frame);
}

/** A terminal of 80 x 24 and of 120 x 34. */
const SIZES = [
  [80, 24],
  [120, 34],
] as const;

type Rendered = import('./helpers.js').Rendered;

/** The files of the run, with the diff of src/cart.ts beside the tree. */
async function files(columns: number, rows: number, o: { mono?: boolean; plain?: boolean; readonly?: boolean } = {}): Promise<Rendered> {
  h.fakeWorld((dir) => void demo.demoCopy(dir));
  const r = await h.renderApp({ columns, rows, ...o });
  await r.waitFor('npm test');
  await r.press('f');
  await r.waitFor('Files · changes');
  await r.waitFor('README.md');
  // From the first file down to src/cart.ts.
  for (const name of ['format.ts · new', 'cart.ts · changed']) {
    while (!strip(r.frame()).includes(name)) {
      await r.press('j');
    }
  }
  await r.waitFor((f) => strip(f).includes('@@ line 38'));
  return r;
}

async function card(r: Rendered, keys: string[], title: string): Promise<string> {
  for (const k of keys) await r.press(k);
  return r.waitFor((f) => strip(f).includes(title));
}

/** The keys from the files of the run to each screen, and the words that say it shows. */
const SCREEN: Record<string, { keys: string[]; shows: string }> = {
  t1: { keys: [], shows: 'Files · changes' },
  't2-diff': { keys: ['\r'], shows: 'original ↔ copy' },
  // The folder src/: two rows above src/cart.ts.
  't3-apply': { keys: ['k', 'k', 'k', 'A'], shows: 'Apply to your folder' },
  't3-apply-deletes': { keys: ['G', 'A'], shows: 'leaves your folder' },
  't3-conflict': { keys: ['g', 'j', 'j', 'A'], shows: 'Both sides changed' },
  't3-discard': { keys: ['D'], shows: 'Discard from the copy' },
  't3-undo': { keys: ['u'], shows: 'Undo the last apply' },
  't3-restore': { keys: ['b', 'R'], shows: 'Restore a backup' },
  't4-backups': { keys: ['b'], shows: 'Backups · webshop' },
  't5-pointer': { keys: ['i'], shows: 'encrypted (LUKS)' },
};

describe.each(SIZES)('the file manager at %i x %i', (columns, rows) => {
  test.each(Object.keys(SCREEN))('%s: the picture', async (name) => {
    const s = SCREEN[name] as { keys: string[]; shows: string };
    const r = await files(columns, rows);
    const frame = await card(r, s.keys, s.shows);
    const text = save(`files-${name}-${columns}`, frame);
    expect(text).toMatchSnapshot();
    // No line is wider than the terminal, and the picture is as high as it.
    const lines = text.split('\n');
    expect(lines).toHaveLength(rows);
    for (const l of lines) expect(cells(l), l).toBeLessThanOrEqual(columns);
  });
});

describe('NO_COLOR', () => {
  test.each(Object.keys(SCREEN))('%s: no colour code, and each state is a glyph and a word', async (name) => {
    const s = SCREEN[name] as { keys: string[]; shows: string };
    const r = await files(80, 24, { mono: true });
    const frame = await card(r, s.keys, s.shows);
    const text = save(`files-${name}-80-nocolor`, frame);
    expect(text).toMatchSnapshot();
    // Bold, dim, inverse and underline only: no colour of the 16, of the 256 or of the 24 bits.
    for (const m of frame.matchAll(/\u001b\[([\d;]*)m/g)) {
      for (const c of (m[1] ?? '').split(';').filter((x) => x).map(Number)) expect([0, 1, 2, 4, 7, 22, 24, 27], `SGR ${c}`).toContain(c);
    }
    for (const l of text.split('\n')) expect(cells(l), l).toBeLessThanOrEqual(80);
  });

  test('the marks: bold for changed and deleted, bold and inverse for a conflict, dim for kept', async () => {
    const r = await files(80, 24, { mono: true });
    const f = r.frame();
    const row = (word: string): string => f.split('\n').find((l) => strip(l).includes(word)) ?? '';
    expect(row('price.ts')).toMatch(/\u001b\[1m[^\u001b]*~/);
    expect(row('logo.png')).toMatch(/\u001b\[1m[^\u001b]*-/);
    expect(row('README.md')).toMatch(/\u001b\[7m\u001b\[1m!/);
    expect(row('README.md')).toMatch(/\u001b\[7m\u001b\[1mconflict/);
    expect(row('old.ts')).toMatch(/\u001b\[2m[^\u001b]*old\.ts/);
    // A new file is normal text: its mark and its word say what it is.
    expect(strip(row('tax.ts'))).toMatch(/\+ tax\.ts +new/);
  });
});

describe('--plain', () => {
  test('no boxes and no inverse: the selected row starts with >', async () => {
    const r = await files(80, 24, { plain: true });
    const text = save('files-t1-80-plain', r.frame());
    expect(text).toMatchSnapshot();
    expect(text).not.toMatch(/[╭╮╰╯│]/);
    // The panes have no inverse: the selected row has its mark.
    for (const l of r.frame().split('\n').slice(3, -2)) expect(l).not.toMatch(/\u001b\[[\d;]*7m/);
    expect(text).toMatch(/^> {3}~ cart\.ts/m);
  });
});
