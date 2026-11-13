// The look of the world, with a ranch as a real one looks: six projects,
// three runs that work, one that waits, one that is paused, a helper on
// its path, a shared yard and another computer. The ranch, a building and
// a room are drawn at 80, 120 and 200 columns. No word may lie on the art:
// a name stands on a sign or on the ground, and it ends with an ellipsis
// when it does not fit.
//
// Each picture is also a file of test/__screens__ (text with the colour
// codes of a terminal): `cat test/__screens__/ranch-200.ans` shows it.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';
import { cells } from '../src/clean.js';
import { rowText } from '../src/world/canvas.js';
import { Motion } from '../src/world/motion.js';
import { drawWorld, type Drawn, type Level, type View } from '../src/world/scene.js';
import type { World } from '../src/world/model.js';
import { T } from './world-demo.js';
import { CORE, R_CLAUDE, R_CODEX, R_COPILOT, R_GEMINI, R_GROK, R_OPENCODE, ranchWorld } from './world-ranch.js';

const SCREENS = join(dirname(fileURLToPath(import.meta.url)), '__screens__');
const WEBSITE = '/home/demo/src/website';
const DOCS = '/home/demo/src/docs';
const LONG = '/home/demo/src/infrastructure-scripts-and-dotfiles';

/** A terminal of 80 x 24, of 120 x 34 and of 200 x 60: the picture has three rows less. */
const SIZES = [
  [80, 21],
  [120, 31],
  [200, 57],
] as const;

function view(level: Level, o: Partial<View> = {}): View {
  return { level, house: level === 'ranch' ? '' : CORE, room: '', sel: '', plumbing: false, ...o };
}

function draw(v: View, w: number, h: number, world: World = ranchWorld()): Drawn {
  return drawWorld(world, { w, h, mode: 'truecolor', tick: 0, motion: new Motion(), view: v, t: T });
}

/** The picture as a terminal gets it, and the same as a file of test/__screens__. */
function save(name: string, d: Drawn): string {
  const text = d.canvas.rows().map(rowText).join('\n');
  mkdirSync(SCREENS, { recursive: true });
  writeFileSync(join(SCREENS, `${name}.ans`), text + '\n');
  return text;
}

/** The words of the picture, row by row. */
function words(d: Drawn): string {
  return d.canvas.lines().join('\n');
}

/** How many cells of the picture have a pixel of the art or a word: the rest is the empty background of the terminal. */
function filled(d: Drawn): number {
  const rows = d.canvas.rows().slice(1, -1);
  let n = 0;
  for (const r of rows) for (let x = 0; x < r.ch.length; x++) if (r.ch[x] !== ' ' || r.st[x] !== '') n++;
  return n / (rows.length * (rows[0]?.ch.length ?? 1));
}

const RANCH = view('ranch', { sel: `room:${R_CLAUDE}` });

describe('the ranch of the test is a ranch as a real one looks', () => {
  test('six projects, three runs that work, one that waits, one that is paused, a helper, a yard, another computer', () => {
    const w = ranchWorld();
    expect(w.houses.map((h) => h.name)).toEqual(['paddock-core', 'website', 'trainer', 'notes-app', 'docs', 'infrastructure-scripts-and-dotfiles']);
    const rooms = w.houses.flatMap((h) => h.rooms);
    expect(rooms.filter((r) => r.run.state === 'running' && !r.run.frozen && !r.parent).map((r) => r.id)).toEqual([R_CLAUDE, R_CODEX, R_COPILOT]);
    expect(rooms.filter((r) => r.asks.length > 0).map((r) => r.id)).toEqual([R_OPENCODE]);
    expect(rooms.filter((r) => r.run.frozen).map((r) => r.id)).toEqual([R_GROK]);
    expect(rooms.filter((r) => r.parent).map((r) => [r.id, r.parent])).toEqual([[R_GEMINI, R_CLAUDE]]);
    expect(w.houses.flatMap((h) => h.yards).map((y) => [y.name, y.members])).toEqual([['web', [R_CODEX, R_OPENCODE]]]);
    expect(w.islands.map((i) => i.name)).toEqual(['gpu-1']);
    // Two projects have no run: one with tickets, one with nothing.
    expect(w.houses.filter((h) => h.rooms.length === 0).map((h) => [h.name, h.tickets.length])).toEqual([
      ['docs', 2],
      ['infrastructure-scripts-and-dotfiles', 0],
    ]);
  });
});

describe('the ranch', () => {
  test.each(SIZES)('%i x %i: the picture', (w, h) => {
    expect(save(`ranch-${w}`, draw(RANCH, w, h))).toMatchSnapshot();
  });

  test.each(SIZES)('%i x %i: no word lies on the art', (w, h) => {
    for (const v of [RANCH, view('ranch', { sel: `yard:${WEBSITE}:web`, plumbing: true }), view('ranch', { sel: `sign:${DOCS}` }), view('ranch', { sel: 'gate' })]) {
      const d = draw(v, w, h);
      expect(d.canvas.clashes(), v.sel).toEqual([]);
      // There are words, and there is art: the test does not pass on an empty picture.
      expect(words(d)).toMatch(/ (paddock-core|notes-app)( |$)/m);
      expect(d.canvas.rows().some((r) => r.st.some((s) => s.includes('48;2;') || s.includes('38;2;')))).toBe(true);
    }
  });

  test.each(SIZES)('%i x %i: every project has a house with its name on a sign, and the picture has its size', (w, h) => {
    const d = draw(RANCH, w, h);
    const f = words(d);
    for (const name of ['paddock-core', 'website', 'trainer', 'notes-app', 'docs']) expect(f, name).toMatch(new RegExp(` ${name}( |$)`, 'm'));
    // A name that does not fit ends with an ellipsis. Nothing is cut without one.
    expect(f).toMatch(/ infrastru[\w-]*…( |$)/m);
    expect(f).not.toContain('infrastructure-scripts-and-dotfiles');
    const rows = d.canvas.rows();
    expect(rows).toHaveLength(h);
    for (const r of rows) expect(cells(r.ch.join(''))).toBe(w);
    // The tickets are in the Inspector and in the line at the bottom, not in the house.
    for (const t of ['ACL1', 'COORD1', 'more tickets', 'DOC-3']) expect(f, t).not.toContain(t);
    // The line at the bottom fits the width, with room before the note of the scroll.
    const last = (f.split('\n')[h - 1] ?? '').trimEnd();
    expect(cells(last)).toBeLessThanOrEqual(w);
    expect(last.startsWith('claude-a1b2c3 · ● running')).toBe(true);
  });

  test.each(SIZES)('%i x %i: each thing can be selected, inside the picture, and no two rooms share a cell', (w, h) => {
    const d = draw(RANCH, w, h);
    const ids = d.hits.map((x) => x.id);
    for (const id of [R_CLAUDE, R_GEMINI, R_CODEX, R_OPENCODE, R_GROK, R_COPILOT]) expect(ids, id).toContain(`room:${id}`);
    for (const id of [`sign:${CORE}`, `sign:${DOCS}`, `sign:${LONG}`, `loft:${CORE}`, `yard:${WEBSITE}:web`, 'island:gpu-1']) expect(ids, id).toContain(id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const x of d.hits) {
      expect(x.x, x.id).toBeGreaterThanOrEqual(0);
      expect(x.y, x.id).toBeGreaterThanOrEqual(1);
      expect(x.w, x.id).toBeGreaterThan(0);
      expect(x.h, x.id).toBeGreaterThan(0);
      expect(x.x + x.w, x.id).toBeLessThanOrEqual(w);
      expect(x.y + x.h, x.id).toBeLessThanOrEqual(h - 1);
    }
    const rooms = d.hits.filter((x) => x.kind === 'room');
    for (const a of rooms) for (const b of rooms) if (a !== b) expect(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, `${a.id} ${b.id}`).toBe(true);
  });

  test('120 and 200 columns: the ranch fills the screen, with the bench, the gate and the bay of the other computer', () => {
    for (const [w, h] of SIZES.slice(1)) {
      const d = draw(RANCH, w, h);
      const f = words(d);
      expect(filled(d), `${w}`).toBeGreaterThan(0.9);
      for (const t of ['Build bench', '1 claude-core-a1b2c3', 'Ranch gate', 'codex week 91%', 'gpu-1', 'a job runs', 'loft ▲']) expect(f, t).toContain(t);
      expect(f).not.toContain('more row');
      expect(d.hits.map((x) => x.id)).toEqual(expect.arrayContaining(['bench', 'gate']));
    }
    // At 200 columns a pixel of the art is twice as large, and a run has its whole name.
    expect(draw(RANCH, 200, 57).canvas.scale).toBe(2);
    expect(draw(RANCH, 120, 31).canvas.scale).toBe(1);
    expect(words(draw(RANCH, 200, 57))).toContain('opencode-0e0e0e ◆');
  });

  test('80 columns: a compact ranch with every house, and the rest one row down', () => {
    const d = draw(RANCH, 80, 21);
    const f = words(d);
    expect(d.canvas.scale).toBe(1);
    expect(filled(d)).toBeGreaterThan(0.9);
    for (const t of ['claude ✉2', 'gemini', 'codex', 'opencode ◆', 'grok', 'copilot', 'gpu-1']) expect(f, t).toContain(t);
    expect(f).toContain('↓ 1 more row');
    // The row of the bench and the gate shows when one of them is selected.
    const down = draw(view('ranch', { sel: 'gate' }), 80, 21);
    expect(words(down)).toContain('Ranch gate');
    expect(words(down)).toContain('↑');
    expect(down.canvas.clashes()).toEqual([]);
  });

  test('a project without a run is a closed house: dark walls, no white block, and its sign can be selected', () => {
    for (const [w, h] of SIZES) {
      const d = draw(view('ranch', { sel: `sign:${DOCS}` }), w, h);
      const sign = d.hits.find((x) => x.id === `sign:${DOCS}`);
      expect(sign, `${w}`).toBeDefined();
      if (!sign) continue;
      // The house stands over its sign: the cells there are the night shell, not the white wall of a lit house.
      const s = d.canvas.scale;
      const rows = d.canvas.rows();
      // A room is 12 pixels high: 6 rows of cells, or 12 where a pixel is twice as large.
      const over = rows.slice(Math.max(1, sign.y - 1 - 6 * s), sign.y - 1).flatMap((r) => r.st.slice(sign.x + 2 * s, sign.x + 10 * s));
      expect(over.some((st) => st.includes('76;70;123')), `${w}`).toBe(true);
      expect(over.some((st) => st.includes('255;255;249') || st.includes('255;255;255')), `${w}`).toBe(false);
      expect(words(d).split('\n')[h - 1]).toContain('Tickets of docs: DOC-3 open, DOC-4 open');
    }
    expect(words(draw(view('ranch', { sel: `sign:${LONG}` }), 120, 31)).split('\n')[30]).toContain('infrastructure-scripts-and-dotfiles: no ticket on its board.');
  });
});

describe('a building', () => {
  test.each(SIZES)('%i x %i: the rooms stand in the middle, the loft over them, the words under them', (w, h) => {
    const d = draw(view('building', { sel: `room:${R_CLAUDE}` }), w, h);
    save(`building-${w}`, d);
    expect(d.canvas.clashes()).toEqual([]);
    const f = words(d);
    expect(f).toContain('› claude-a1b2c3 ● running');
    expect(f).toContain('↳ gemini-0a1a2a ● running');
    const rooms = d.hits.filter((x) => x.kind === 'room');
    const loft = d.hits.find((x) => x.kind === 'loft');
    expect(rooms).toHaveLength(2);
    expect(loft).toBeDefined();
    if (!loft) return;
    // The loft is over the rooms, and as wide as the house.
    for (const r of rooms) expect(loft.y + loft.h).toBeLessThanOrEqual(r.y);
    // The middle of the building is the middle of the picture.
    const left = Math.min(loft.x, ...rooms.map((r) => r.x));
    const right = Math.max(...rooms.map((r) => r.x + r.w));
    expect(Math.abs((left + right) / 2 - w / 2)).toBeLessThanOrEqual(w >= 100 ? 6 : 4);
  });

  test.each(SIZES)('%i x %i: a project without a run is one closed room, and a small sign says that it has no notes', (w, h) => {
    const d = draw(view('building', { house: DOCS }), w, h);
    save(`building-empty-${w}`, d);
    expect(d.canvas.clashes()).toEqual([]);
    const f = words(d);
    expect(f).toContain(' docs   No notes yet');
    expect(f).toContain('No run in this project now.');
    expect(f).not.toContain('No loft: this project keeps no private notes.');
    // No white block: the room is the night shell.
    const st = d.canvas.rows().flatMap((r) => r.st);
    expect(st.some((x) => x.includes('76;70;123'))).toBe(true);
    expect(st.some((x) => x.includes('255;255;249'))).toBe(false);
  });

  test('200 columns: the building is twice as large', () => {
    expect(draw(view('building'), 200, 57).canvas.scale).toBe(2);
    expect(draw(view('building'), 120, 31).canvas.scale).toBe(1);
  });
});

describe('a room', () => {
  test.each(SIZES)('%i x %i: the room stands in the middle with its words beside it, and the loft over it', (w, h) => {
    const d = draw(view('room', { room: R_CLAUDE, sel: 'self' }), w, h);
    save(`room-${w}`, d);
    expect(d.canvas.clashes()).toEqual([]);
    const lines = d.canvas.lines();
    const at = lines.findIndex((l) => l.includes('Internet'));
    expect(at).toBeGreaterThan(0);
    const x = (lines[at] as string).indexOf('Internet');
    // The words begin right of the middle: the room is left of them, not in a corner.
    if (w >= 120) expect(x).toBeGreaterThan(w * 0.35);
    expect(x + 30).toBeLessThanOrEqual(w);
    for (const t of ['Messages', '2 unread', 'Loft', 'notes upstairs', 'screenshot']) expect(lines.join('\n'), t).toContain(t);
    for (const l of lines) expect(cells(l)).toBeLessThanOrEqual(w);
  });

  test('a room of a project without a loft: a small sign beside its roof, no bar over the room', () => {
    for (const [w, h] of SIZES) {
      const d = draw(view('room', { house: WEBSITE, room: R_OPENCODE, sel: 'door' }), w, h);
      save(`room-ask-${w}`, d);
      expect(d.canvas.clashes(), `${w}`).toEqual([]);
      const f = words(d);
      expect(f, `${w}`).toContain('No notes yet');
      expect(f).not.toContain('No loft');
      expect(f).toMatch(/Asks you +git push/);
    }
  });

  test('200 columns: the room is twice as large', () => {
    expect(draw(view('room', { room: R_CLAUDE }), 200, 57).canvas.scale).toBe(2);
    expect(draw(view('room', { room: R_CLAUDE }), 100, 27).canvas.scale).toBe(1);
  });
});

describe('the loft', () => {
  test.each(SIZES)('%i x %i: the attic stands where the room stood, and its words stand beside it', (w, h) => {
    const d = draw(view('loft', { sel: 'note:log' }), w, h);
    save(`loft-${w}`, d);
    expect(d.canvas.clashes()).toEqual([]);
    const f = words(d);
    // On a narrow screen the folder ends with an ellipsis.
    for (const t of ['Loft: private notes', '› Notes, newest first', w >= 120 ? 'Folder  /data/loft/core/tree/.paddock' : 'Folder  /data/loft/core/tr']) expect(f, t).toContain(t);
    expect(d.hits.map((x) => x.id)).toEqual(['note:log', 'note:folder']);
    // The same place as the words of the room: the camera went up, not sideways.
    const room = draw(view('room', { room: R_CLAUDE }), w, h);
    const col = (x: Drawn, t: string): number => (x.canvas.lines().find((l) => l.includes(t)) ?? '').indexOf(t);
    expect(col(d, 'Loft: private notes')).toBe(col(room, 'Internet'));
  });
});
