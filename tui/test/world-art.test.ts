// The art of the world: every layer that the world asks for is a file of
// sprites/world with the size of frames.json, the build puts it into the
// program, and the pictures show it. A layer whose file is missing shows
// as its placeholder, and one line says so, once.
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, test } from 'vitest';
import { WORLD_FILES } from '../src/sprite-files.js';
import { parseSprite } from '../src/sprites.js';
import { Canvas, rowText } from '../src/world/canvas.js';
import * as art from '../src/world/layers.js';
import { Motion } from '../src/world/motion.js';
import { INK, SELECT } from '../src/world/pixels.js';
import { drawWorld, type Level, type View } from '../src/world/scene.js';
import { CLAUDE, CODEX, demoWorld, GROK, T } from './world-demo.js';

const DIR = join(dirname(fileURLToPath(import.meta.url)), '..', 'sprites', 'world');
interface Meta {
  w: number;
  h: number;
  fps: number;
  frames: number;
  slice?: { left: number; top: number; right: number; bottom: number };
  fan_at?: [number, number];
}
const META = JSON.parse(readFileSync(join(DIR, 'frames.json'), 'utf8')) as Record<string, Meta>;
const ALPHA = '/src/alpha';

function view(level: Level, o: Partial<View> = {}): View {
  return { level, house: level === 'ranch' ? '' : ALPHA, room: '', sel: '', plumbing: false, ...o };
}

function picture(v: View, o: { w?: number; h?: number; light?: boolean; tick?: number } = {}): string {
  return drawWorld(demoWorld(), { w: o.w ?? 80, h: o.h ?? 21, mode: 'truecolor', light: o.light, tick: o.tick ?? 0, motion: new Motion(), view: v, t: T })
    .canvas.rows()
    .map(rowText)
    .join('\n');
}

/** Every view of the demo world, in both forms of a room. */
const VIEWS: View[] = [
  view('ranch'),
  view('ranch', { plumbing: true, sel: `yard:${ALPHA}:team` }),
  view('building'),
  view('building', { plumbing: true }),
  view('building', { house: '/src/website' }),
  view('room', { room: CLAUDE }),
  view('room', { room: CLAUDE, plumbing: true }),
  view('room', { room: CODEX, sel: 'door' }),
  view('room', { room: GROK }),
  view('room', { house: '/src/website', room: 'codex-web-0d0d0d55' }),
  view('loft'),
];

afterEach(() => art.setLayerFiles());

describe('the layers of the world', () => {
  test('every layer that the world asks for is a file with the size, the frames and the rate of frames.json', () => {
    const names = Object.keys(art.LAYER_SIZES);
    // The world asks for all the layers of the scene, and for no other.
    expect(names.sort()).toEqual(Object.keys(META).sort());
    expect(
      readdirSync(DIR)
        .filter((f) => f !== 'frames.json')
        .map((f) => f.replace(/\.json$/, ''))
        .sort(),
    ).toEqual(names);
    expect(names).toHaveLength(105);
    for (const name of names) {
      const meta = META[name] as Meta;
      const sprite = parseSprite(readFileSync(join(DIR, `${name}.json`), 'utf8'));
      expect(sprite, name).not.toBeNull();
      expect([sprite?.w, sprite?.h], name).toEqual([meta.w, meta.h]);
      expect(art.LAYER_SIZES[name], name).toEqual([meta.w, meta.h]);
      expect(sprite?.frames, name).toHaveLength(meta.frames);
      expect(sprite?.fps, name).toBe(meta.fps);
    }
  });

  test('the cuts of the room shells and the place of the fan are the ones of frames.json', () => {
    for (const size of art.ROOM_SIZES) for (const kind of art.ROOM_KINDS) expect(META[`room-${size}-${kind}`]?.slice, `${size} ${kind}`).toEqual(art.SLICE[size]);
    expect(META.island?.fan_at).toEqual(art.FAN_AT);
    expect(art.FROST_STEPS).toBe(Object.keys(META).filter((n) => n.startsWith('frost-')).length);
  });

  test('the build puts every layer into the program, and the loader of the sprites reads it: no placeholder, no line', () => {
    const lines: string[] = [];
    art.setLayerFiles(WORLD_FILES, (l) => lines.push(l));
    for (const name of Object.keys(art.LAYER_SIZES)) {
      expect(JSON.parse(WORLD_FILES[name] as string), name).toEqual(JSON.parse(readFileSync(join(DIR, `${name}.json`), 'utf8')));
      const l = art.layer(name);
      expect(l.placeholder, name).toBe(false);
      expect(l.frames, name).toHaveLength((META[name] as Meta).frames);
      expect(l.fps, name).toBe((META[name] as Meta).fps);
      for (const f of l.frames) expect(f.px, name).toHaveLength(l.w * l.h);
      // Read once, then kept.
      expect(art.layer(name)).toBe(l);
    }
    expect(lines).toEqual([]);
    expect(() => art.layer('no-such-layer')).toThrow('no layer');
  });

  test('the pictures of every view are made of the layers: nothing is missing, at 80, 120 and 200 columns', () => {
    const lines: string[] = [];
    art.setLayerFiles(WORLD_FILES, (l) => lines.push(l));
    for (const [w, h] of [
      [80, 21],
      [120, 30],
      [200, 57],
    ] as const) {
      for (const v of VIEWS) for (const light of [false, true]) expect(picture(v, { w, h, light }).split('\n')).toHaveLength(h);
    }
    expect(lines).toEqual([]);
  });
});

describe('what the world makes from a layer', () => {
  test('a loop shows each frame for a whole number of the 8 steps of a second', () => {
    const at = (fps: number, n: number): number[] => Array.from({ length: 9 }, (_, tick) => art.frameAt({ fps, frames: { length: n } }, tick));
    expect(at(8, 4)).toEqual([0, 1, 2, 3, 0, 1, 2, 3, 0]);
    expect(at(6, 4)).toEqual([0, 1, 2, 3, 0, 1, 2, 3, 0]);
    expect(at(3, 2)).toEqual([0, 0, 0, 1, 1, 1, 0, 0, 0]);
    expect(at(2, 2)).toEqual([0, 0, 0, 0, 1, 1, 1, 1, 0]);
    expect(at(8, 1)).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0]);
  });

  test('a room shell at another size keeps its corners and its floor', () => {
    const src = art.pic('room-40x34-lit');
    expect(art.shell('40x34', 'lit', 40, 34)).toEqual(src);
    const low = art.shell('40x34', 'lit', 40, 20);
    const wide = art.shell('40x34', 'lit', 70, 34);
    expect([low.w, low.h, wide.w, wide.h]).toEqual([40, 20, 70, 34]);
    const s = art.SLICE['40x34'];
    // The floor: the lowest rows of the layer, at the bottom of the low room.
    for (let y = 0; y < s.bottom; y++) expect(low.px.slice((20 - s.bottom + y) * 40, (21 - s.bottom + y) * 40)).toEqual(src.px.slice((34 - s.bottom + y) * 40, (35 - s.bottom + y) * 40));
    expect(low.px.slice(0, 40 * s.top)).toEqual(src.px.slice(0, 40 * s.top));
    // The wide room: the ink line all around, and the two side walls of the layer.
    for (let x = 0; x < 70; x++) expect(wide.px[x]).toEqual(INK);
    for (let y = 0; y < 34; y++) {
      expect([wide.px[y * 70], wide.px[y * 70 + 1]]).toEqual([src.px[y * 40], src.px[y * 40 + 1]]);
      expect([wide.px[y * 70 + 68], wide.px[y * 70 + 69]]).toEqual([src.px[y * 40 + 38], src.px[y * 40 + 39]]);
    }
    // Made once, then kept.
    expect(art.shell('40x34', 'lit', 70, 34)).toBe(wide);
    // The selection: the ink line in the accent, the rest as it was.
    const sel = art.shell('40x34', 'lit', 40, 34, 'select');
    expect(sel.px[0]).toEqual(SELECT);
    expect(sel.px[40 * 10 + 10]).toEqual(src.px[40 * 10 + 10]);
  });

  test('frost grows in five steps, and the other corners are mirrors of the layer', () => {
    const count = (b: { px: readonly unknown[] }): number => b.px.filter((p) => p !== null).length;
    const sizes = [1, 2, 3, 4, 5].map((n) => count(art.frost(n, 'tl')));
    for (let i = 1; i < sizes.length; i++) expect(sizes[i]).toBeGreaterThan(sizes[i - 1] as number);
    const tl = art.frost(5, 'tl');
    const br = art.frost(5, 'br');
    for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) expect(br.px[(7 - y) * 8 + (7 - x)]).toEqual(tl.px[y * 8 + x]);
    expect(art.frost(5, 'tr').px[7]).toEqual(tl.px[0]);
    expect(art.frost(9, 'tl')).toBe(tl);
  });

  test('the model of the project: the part of its mask takes the colour of the agent', () => {
    const plain = art.model(null);
    const changed = art.model([1, 2, 3]);
    const mask = art.pic('model-mask');
    expect(mask.px.some((p) => p !== null)).toBe(true);
    mask.px.forEach((m, i) => expect(changed.px[i]).toEqual(m ? [1, 2, 3] : plain.px[i]));
  });

  test('a tile repeats over its area', () => {
    const t = art.pic('tile-roof');
    const strip = art.tiles('tile-roof', 20, 2);
    expect([strip.w, strip.h]).toEqual([20, 2]);
    for (let x = 0; x < 20; x++) expect(strip.px[x]).toEqual(t.px[x % t.w]);
    // From a row of the tile on: the fence without the tips of its posts.
    const fence = art.pic('tile-fence');
    expect(art.tiles('tile-fence', 4, 2, 0, 1).px).toEqual(fence.px.slice(4, 12));
  });

  test('the cell of a pair of colours is made once: two frames give the same cells', () => {
    const cells = (): string => {
      const c = new Canvas(4, 2, { mode: 'truecolor' });
      c.blit(art.pic('socket-host'), 0, 0);
      return c.rows().map(rowText).join('\n');
    };
    expect(cells()).toBe(cells());
    expect(cells()).toContain('▀');
  });
});

describe('the pictures in 24-bit colour', () => {
  test('the ranch', () => {
    expect(picture(view('ranch', { sel: `room:${CODEX}` }))).toMatchSnapshot();
  });

  test('a room: the request at the door', () => {
    expect(picture(view('room', { room: CODEX, sel: 'door' }))).toMatchSnapshot();
  });

  test('a room with glass walls: the tools, their sockets and their appliances', () => {
    expect(picture(view('room', { room: CLAUDE, plumbing: true }))).toMatchSnapshot();
  });

  test('the loft', () => {
    expect(picture(view('loft', { sel: 'note:log' }))).toMatchSnapshot();
  });

  test('the ranch shows the small characters of the layers, in the colour of each agent and at their own pace', () => {
    const orange = art.pic('character-small-orange-work', 0);
    const main = orange.px.find((p) => p !== null && !(p[0] === INK[0] && p[1] === INK[1])) as readonly number[];
    expect(picture(view('ranch'))).toContain(`2;${main[0]};${main[1]};${main[2]}`);
    // A run that works moves at each step of the clock.
    expect(picture(view('ranch'), { tick: 0 })).not.toBe(picture(view('ranch'), { tick: 1 }));
    expect(picture(view('ranch'), { tick: 0 })).toBe(picture(view('ranch'), { tick: 24 }));
  });
});

describe('the fallback', () => {
  test('a missing file, a broken file and a file of a wrong size show as placeholders, and one line says so, once', () => {
    const lines: string[] = [];
    const files = { ...WORLD_FILES, card: '{', 'door-asking': JSON.stringify({ w: 1, h: 1, fps: 8, frames: [[[1, 2, 3]]] }) };
    delete (files as Record<string, string>)['desk'];
    art.setLayerFiles(files, (l) => lines.push(l));
    const v = view('room', { room: CODEX, sel: 'door' });
    const first = picture(v);
    for (let tick = 1; tick < 6; tick++) picture(v, { tick });
    expect(lines).toEqual([
      'paddock tui: the layer door-asking.json of the world is 1 x 1 px, not 2 x 16. Its placeholder shows.',
      'paddock tui: the layer desk.json of the world is missing. Its placeholder shows.',
      'paddock tui: the layer card.json of the world is not a sprite. Its placeholder shows.',
    ]);
    for (const name of ['card', 'door-asking', 'desk']) {
      const l = art.layer(name);
      expect(l.placeholder, name).toBe(true);
      expect([l.w, l.h], name).toEqual(art.LAYER_SIZES[name]);
    }
    expect(art.layer('model').placeholder).toBe(false);
    // The picture keeps its size, and it is not the one of the art.
    expect(first.split('\n')).toHaveLength(21);
    art.setLayerFiles();
    expect(first).not.toBe(picture(v));
  });

  test('no file at all: every view still draws, each layer as its placeholder with the size of the layer', () => {
    const lines: string[] = [];
    art.setLayerFiles({}, (l) => lines.push(l));
    for (const name of Object.keys(art.LAYER_SIZES)) {
      const l = art.layer(name);
      expect(l.placeholder, name).toBe(true);
      expect(l.frames.length, name).toBeGreaterThan(0);
      for (const f of l.frames) expect([f.w, f.h, f.px.length], name).toEqual([l.w, l.h, l.w * l.h]);
    }
    expect(lines).toHaveLength(105);
    expect(new Set(lines).size).toBe(105);
    for (const v of VIEWS) expect(picture(v, { w: 120, h: 30 }).split('\n')).toHaveLength(30);
    // Each layer was reported once, however often it was drawn.
    expect(lines).toHaveLength(105);
  });

  test('the ranch and a room without any file', () => {
    art.setLayerFiles({}, () => undefined);
    expect(picture(view('ranch', { sel: `room:${CODEX}` }))).toMatchSnapshot();
    expect(picture(view('room', { room: CODEX, sel: 'door' }))).toMatchSnapshot();
  });
});
