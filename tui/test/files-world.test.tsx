// The copy in the world: the model on the desk of a room glows while
// changes wait, Enter on it opens the files of the run, and after an
// apply the changed parts move from the desk into the house. All of it
// at the 8 frames a second of the art layer, and nothing with reduced
// motion. The pictures are files of test/__screens__ too.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, test } from 'vitest';
import { cleanup } from 'ink-testing-library';
import { ArtLayer } from '../src/art-layer.js';
import { editorCommand, parseDiff, sideRows, treeRows, unifiedRows, cutMiddle, human, type Counts } from '../src/copy.js';
import { rowText } from '../src/world/canvas.js';
import { AGENT } from '../src/world/kit.js';
import { FRAMES, Motion } from '../src/world/motion.js';
import { observe } from '../src/world/observe.js';
import { waiting, type World } from '../src/world/model.js';
import { AMBER_DARK, AMBER_LIGHT } from '../src/world/pixels.js';
import { drawWorld, type Drawn, type View } from '../src/world/scene.js';
import { fakeWorld, renderApp, type Rendered } from './helpers.js';
import { CART_DIFF, demoChanges, demoCopy } from './files-demo.js';
import { CLAUDE, CODEX, demoWorld, T } from './world-demo.js';

const HOME = process.env.HOME;
afterEach(() => {
  cleanup();
  process.env.HOME = HOME;
});

const SCREENS = join(dirname(fileURLToPath(import.meta.url)), '__screens__');
const ALPHA = '/src/alpha';
const room: View = { level: 'room', house: ALPHA, room: CLAUDE, sel: '', plumbing: false };
const counts = (o: Partial<Counts>): Counts => ({ new: 0, changed: 0, deleted: 0, conflicts: 0, kept: 0, ready: 0, applied: 0, behind: 0, ...o });
const WAITS = counts({ changed: 4, new: 2, deleted: 1, conflicts: 1, ready: 5 });

/** The world of the tests, with a copy for the run of claude. */
function world(c: Counts | null): World {
  return demoWorld((d) => {
    d.extra.copies = c ? [{ id: CLAUDE, harness: 'claude', project: ALPHA, state: 'live', counts: c }] : [];
  });
}

function draw(w: World, tick: number, motion = new Motion(), size: readonly [number, number] = [80, 21]): Drawn {
  return drawWorld(w, { w: size[0], h: size[1], mode: 'truecolor', tick, motion, view: room, t: T });
}

/** How many half cells of a picture have a colour. */
function count(d: Drawn, rgb: readonly number[]): number {
  const code = `2;${rgb[0]};${rgb[1]};${rgb[2]}`;
  let n = 0;
  for (const r of d.canvas.rows()) for (const s of r.st) n += s.split(code).length - 1;
  return n;
}

function save(name: string, d: Drawn): void {
  mkdirSync(SCREENS, { recursive: true });
  writeFileSync(join(SCREENS, `${name}.ans`), d.canvas.rows().map(rowText).join('\n') + '\n');
}

const strip = (s: string): string => s.replace(/\u001b\[[\d;]*m/g, '');
const see = (r: Rendered, words: string): Promise<string> => r.waitFor((f) => strip(f).includes(words));

describe('the model on the desk', () => {
  test('it glows amber while changes wait, and the words beside the room say how many', () => {
    const d = draw(world(WAITS), 8);
    expect(count(d, AMBER_LIGHT)).toBeGreaterThan(0);
    // A narrow panel cuts the words with an ellipsis. A wide one has them all.
    expect(d.canvas.lines().join('\n')).toMatch(/Working copy 7 changes · 1 con/);
    const wide = draw(world(WAITS), 8, new Motion(), [120, 31]);
    expect(wide.canvas.lines().join('\n')).toMatch(/Working copy +7 changes · 1 conflict/);
    // The clock runs for the glow.
    expect(d.loops).toBe(true);
    save('room-copy-waits-80', d);
    save('room-copy-waits-120', wide);
  });

  test('the glow breathes: dark at the start of its two seconds, light in the middle', () => {
    const w = world(WAITS);
    const dark = draw(w, 0);
    const light = draw(w, 8);
    expect(count(dark, AMBER_DARK)).toBeGreaterThan(count(light, AMBER_DARK));
    expect(count(light, AMBER_LIGHT)).toBeGreaterThan(count(dark, AMBER_LIGHT));
    // The same frame of the next breath is the same picture.
    expect(draw(w, 16).canvas.rows().map(rowText).join('\n')).toBe(dark.canvas.rows().map(rowText).join('\n'));
  });

  test('no glow without a change that waits: no copy, no open change, or all applied', () => {
    const base = draw(world(null), 8);
    for (const c of [null, counts({}), counts({ applied: 3 })]) {
      const d = draw(world(c), 8);
      expect(count(d, AMBER_LIGHT), JSON.stringify(c)).toBe(count(base, AMBER_LIGHT));
    }
    expect(waiting(world(counts({ applied: 3 })).houses[0]?.rooms[0] as never)).toBe(0);
  });

  test('reduced motion: the glow is there and stands still', () => {
    const w = world(WAITS);
    const m = new Motion(true);
    const a = draw(w, 0, m);
    expect(count(a, AMBER_LIGHT)).toBeGreaterThan(0);
    // The same light at every frame.
    for (const tick of [3, 5, 8, 12]) {
      const b = draw(w, tick, m);
      expect([count(b, AMBER_LIGHT), count(b, AMBER_DARK)]).toEqual([count(a, AMBER_LIGHT), count(a, AMBER_DARK)]);
    }
  });

  test('without art the room says it in words', () => {
    const d = drawWorld(world(WAITS), { w: 80, h: 21, mode: 'none', tick: 0, motion: new Motion(), view: room, t: T });
    const f = d.canvas.lines().join('\n');
    expect(f).toContain('copy 7~');
    expect(f).toMatch(/Working copy 7 changes/);
  });
});

describe('after an apply', () => {
  const tint = AGENT.orange;

  test('paddock counts one more applied file: the motion starts, and only then', () => {
    const m = new Motion();
    observe(m, world(WAITS), world(WAITS), 0);
    expect(m.running('apply', CLAUDE, 0)).toBe(false);
    observe(m, world(WAITS), world(counts({ applied: 5, changed: 1, conflicts: 1 })), 0);
    expect(m.running('apply', CLAUDE, 0)).toBe(true);
    expect(m.running('apply', CODEX, 0)).toBe(false);
    // One second at 8 frames a second.
    expect(FRAMES.apply).toBe(8);
    expect(m.running('apply', CLAUDE, 7)).toBe(true);
    expect(m.running('apply', CLAUDE, 8)).toBe(false);
  });

  test('the changed parts leave the desk, rise, and are in the house at the end', () => {
    const after = world(counts({ applied: 5 }));
    const m = new Motion();
    m.start('apply', CLAUDE, 0);
    const rest = count(draw(after, 40, m), tint);
    // On the way, parts in the colour of the agent are in the picture, and they move from frame to frame.
    const frames = [1, 2, 3, 4, 5, 6].map((t) => draw(after, t, m));
    for (const f of frames) expect(count(f, tint)).toBeGreaterThan(rest);
    const pictures = new Set(frames.map((f) => f.canvas.rows().map(rowText).join('\n')));
    expect(pictures.size).toBe(frames.length);
    // The parts rise: the highest row with the colour of the agent is higher later.
    const top = (d: Drawn): number => d.canvas.rows().findIndex((r) => r.st.some((s) => s.includes(`2;${tint[0]};${tint[1]};${tint[2]}`)));
    expect(top(frames[5] as Drawn)).toBeLessThan(top(frames[0] as Drawn));
    save('room-copy-apply-80', frames[3] as Drawn);
    // Over: the model has its plain colour again, and nothing of the motion is left.
    expect(m.running('apply', CLAUDE, 8)).toBe(false);
    expect(draw(after, 8, m).canvas.rows().map(rowText).join('\n')).toBe(draw(after, 8, new Motion()).canvas.rows().map(rowText).join('\n'));
  });

  test('reduced motion: nothing moves', () => {
    const after = world(counts({ applied: 5 }));
    const m = new Motion(true);
    m.start('apply', CLAUDE, 0);
    expect(m.running('apply', CLAUDE, 0)).toBe(false);
    expect(draw(after, 2, m).canvas.rows().map(rowText).join('\n')).toBe(draw(after, 2, new Motion()).canvas.rows().map(rowText).join('\n'));
  });
});

describe('from the desk to the files', () => {
  test('Enter on the model of a room opens the files of its run', async () => {
    fakeWorld((d) => void demoCopy(d));
    const r = await renderApp({ columns: 120, rows: 34, art: new ArtLayer({ mode: 'truecolor', sink: null, screen: () => ({ columns: 120, rows: 34 }) }) });
    await see(r, '◆ 7 changes · 1 conflict');
    // The ranch, the building, the room.
    await r.press('b');
    await r.waitFor((f) => strip(f).split('\n')[1]?.startsWith('Ranch') === true);
    await r.press('\r');
    await r.press('\r');
    await see(r, 'Working copy');
    expect(strip(r.frame())).toMatch(/Working copy +7 changes · 1 conflict/);
    for (let i = 0; i < 30 && !/Working copy/.test(strip(r.frame()).split('\n').find((l) => l.includes('\u001b[7m')) ?? ''); i++) {
      const line = r.frame().split('\n').find((l) => strip(l).includes('Working copy')) ?? '';
      if (line.includes('\u001b[7m')) break;
      await r.press('\u001b[B');
    }
    await r.press('\r');
    await see(r, 'Files · changes');
    expect(strip(r.frame())).toContain('claude · webshop');
    // Esc goes back to the room.
    await r.press('\u001b');
    await see(r, 'Working copy');
  });
});

describe('the model of the copy', () => {
  test('a diff of paddock: the lines with their numbers, unified and side by side', () => {
    const d = parseDiff(CART_DIFF);
    expect(d.hunks.map((h) => [h.oldStart, h.newStart, h.lines.length])).toEqual([
      [38, 38, 9],
      [61, 62, 8],
    ]);
    const u = unifiedRows(d);
    expect(u.filter((r) => r.sign === '-').map((r) => r.no)).toEqual(['42', '64']);
    expect(u.filter((r) => r.sign === '+').map((r) => r.no)).toEqual(['42', '43', '65']);
    expect(u[0]).toMatchObject({ kind: 'hunk', text: '@@ line 38' });
    const s = sideRows(d);
    // A changed line has ~ on both sides. A line with no partner stands alone.
    expect(s.filter((r) => r.left?.sign === '~').map((r) => [r.left?.no, r.right?.no])).toEqual([
      ['42', '42'],
      ['64', '65'],
    ]);
    expect(s.find((r) => r.left === null && r.right?.sign === '+')?.right?.no).toBe('43');
    // One side of it: the original has no added line.
    expect(unifiedRows(d, 'original').some((r) => r.sign === '+')).toBe(false);
    expect(unifiedRows(d, 'copy').some((r) => r.sign === '-')).toBe(false);
  });

  test('a new file, a line without a line end, what is no diff, and an escape code in the text', () => {
    const d = parseDiff('--- your folder/a\n+++ the copy/a\n@@ -0,0 +1,2 @@\n+one\n+two \u001b[31mred\u001b[0m\n\\ No newline at end of file\n');
    expect(d.hunks[0]?.lines.map((l) => [l.sign, l.new, l.text])).toEqual([
      ['+', 1, 'one'],
      ['+', 2, 'two red'],
    ]);
    expect(parseDiff('logo.png: deleted, no text to show (a binary file, a symlink, or above 1 MB).\n')).toEqual({ hunks: [], note: 'logo.png: deleted, no text to show (a binary file, a symlink, or above 1 MB).' });
    expect(parseDiff('')).toEqual({ hunks: [], note: '' });
  });

  test('the tree: the root first, folders before files, a chain of folders in one row, counts on the folders', () => {
    const doc = demoChanges('/p/webshop', '/c/tree');
    const rows = treeRows(doc, 'changes', '', new Set(), 'webshop');
    expect(rows.map((r) => `${'  '.repeat(r.depth)}${r.name}`)).toEqual(['webshop/', '  docs/', '    README.md', '  src/', '    lib/util/', '      format.ts', '    cart.ts', '    old.ts', '    price.ts', '    tax.ts', '  logo.png']);
    expect(rows[0]).toMatchObject({ kind: 'root', counts: { conflict: 1, changed: 2, new: 2, deleted: 1, kept: 1 } });
    expect(rows[0]?.items).toHaveLength(7);
    // A closed folder hides its rows and keeps its counts.
    const closed = treeRows(doc, 'changes', '', new Set(['src/']), 'webshop');
    expect(closed.map((r) => r.name)).toEqual(['webshop/', 'docs/', 'README.md', 'src/', 'logo.png']);
    expect(closed[3]).toMatchObject({ open: false, counts: { changed: 2, new: 2, kept: 1 } });
    expect(treeRows(doc, 'conflicts', '', new Set(), 'webshop').map((r) => r.name)).toEqual(['webshop/', 'docs/', 'README.md']);
    expect(treeRows(doc, 'ready', 'TAX', new Set(), 'webshop').map((r) => r.name)).toEqual(['webshop/', 'src/', 'tax.ts']);
    expect(treeRows({ ...doc, changes: [] }, 'changes', '', new Set(), 'webshop')).toEqual([]);
  });

  test('a long name is cut in the middle, and sizes are short', () => {
    expect(cutMiddle('a-very-long-component-name.test.tsx', 16)).toBe('a-very-l….test.tsx'.slice(0, 8) + '…' + 'est.tsx');
    expect(cutMiddle('short.ts', 16)).toBe('short.ts');
    expect([human(0), human(900), human(12_288), human(1_500_000), human(212 * 1024 * 1024)]).toEqual(['0 B', '900 B', '12 KB', '1.4 MB', '212 MB']);
  });

  test('the editor opens read-only: a known editor with its flag, any other program gives way to less', () => {
    expect(editorCommand({ EDITOR: 'nvim' }, '/c/a.ts')).toEqual({ bin: 'nvim', args: ['-R', '--', '/c/a.ts'] });
    expect(editorCommand({ EDITOR: '/usr/bin/vim -u NONE' }, '/c/a.ts')).toEqual({ bin: '/usr/bin/vim', args: ['-u', 'NONE', '-R', '--', '/c/a.ts'] });
    expect(editorCommand({ EDITOR: 'nano' }, '/c/a.ts')).toEqual({ bin: 'nano', args: ['-v', '--', '/c/a.ts'] });
    expect(editorCommand({ EDITOR: 'code --wait' }, '/c/a.ts')).toEqual({ bin: 'less', args: ['--', '/c/a.ts'] });
    expect(editorCommand({}, '/c/a.ts')).toEqual({ bin: 'less', args: ['--', '/c/a.ts'] });
  });
});
