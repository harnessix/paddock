// The layers of the world: the pixel art of the launch video scene, one
// file per layer in sprites/world, in the sprite form of the characters
// ({"w","h","fps","frames"}). The build puts them into the program
// (src/sprite-files.ts), and the loader of the character sprites reads
// them. A layer whose file is missing or has a wrong size shows as its
// placeholder (pixels.ts), and one line says so, once.
//
// Everything that a scene makes from a layer is made once and kept: the
// frames, the room shell at a size (9-slice), a strip of a tile, a mirror
// of a frost corner, a tint (night, the selection). A frame of the world
// only copies pixels.

import { WORLD_FILES } from '../sprite-files.js';
import { ART_COLORS, ART_STATES, parseSprite, SPRITE_FPS, spriteFor, type ArtColor, type ArtState, type Pixel, type Rgb } from '../sprites.js';
import { bitmap, mix, shrink, type Bitmap } from './canvas.js';
import * as px from './pixels.js';

export interface Layer {
  name: string;
  w: number;
  h: number;
  /** The frame rate of its loop. */
  fps: number;
  frames: Bitmap[];
  /** Drawn in the program: the file is missing or broken. */
  placeholder: boolean;
}

export type RoomSize = '12x12' | '33x28' | '40x34';
export type RoomKind = 'lit' | 'night' | 'ice' | 'glass';
export type WindowKey = 'none' | 'api' | 'allowlist' | 'egress' | 'host' | 'unknown';
export type Corner = 'tl' | 'tr' | 'bl' | 'br';

export const ROOM_SIZES: readonly RoomSize[] = ['12x12', '33x28', '40x34'];
export const ROOM_KINDS: readonly RoomKind[] = ['lit', 'night', 'ice', 'glass'];
const WINDOWS: readonly WindowKey[] = ['none', 'api', 'allowlist', 'egress', 'host', 'unknown'];

/** The cuts of the room shells: the corners keep their size, the rest stretches. As in sprites/world/frames.json. */
export const SLICE: Record<RoomSize, { left: number; top: number; right: number; bottom: number }> = {
  '12x12': { left: 2, top: 2, right: 2, bottom: 5 },
  '33x28': { left: 2, top: 2, right: 2, bottom: 8 },
  '40x34': { left: 2, top: 2, right: 2, bottom: 9 },
};

/** Where the fan turns on the island. */
export const FAN_AT: readonly [number, number] = [6, 1];

/** The most frost: the steps of its growth. */
export const FROST_STEPS = 5;

/** Every layer that the world asks for, with its size in pixels. */
export const LAYER_SIZES: Readonly<Record<string, readonly [number, number]>> = (() => {
  const out: Record<string, readonly [number, number]> = {};
  const add = (w: number, h: number, ...names: string[]): void => {
    for (const n of names) out[n] = [w, h];
  };
  for (const c of ART_COLORS) for (const s of ART_STATES) add(8, 8, `character-small-${c}-${s}`);
  for (const k of WINDOWS) {
    add(7, 8, `window-${k}`);
    add(3, 3, `window-small-${k}`);
  }
  add(2, 16, 'door-closed', 'door-asking');
  add(5, 7, 'card');
  add(6, 11, 'mailbox-down', 'mailbox-up');
  add(11, 7, 'desk');
  add(7, 5, 'model', 'model-mask');
  add(3, 5, 'hourglass');
  add(7, 6, 'crate-amber', 'crate-red', 'crate-none');
  add(3, 4, 'socket-open', 'socket-cover', 'socket-host');
  add(5, 5, 'appliance');
  add(11, 5, 'pegboard');
  add(5, 4, 'ladder');
  for (let i = 0; i < 6; i++) add(2, 6, `spine-${i}`);
  add(4, 2, 'shelf');
  add(16, 4, 'bench');
  add(4, 4, 'stool-free', 'stool-taken');
  add(16, 8, 'island');
  add(3, 3, 'fan');
  add(4, 2, 'water');
  add(5, 4, 'envelope');
  add(8, 4, 'trough');
  add(5, 4, 'saddle');
  add(12, 8, 'stall');
  for (let i = 1; i <= FROST_STEPS; i++) add(8, 8, `frost-${i}`);
  add(8, 4, 'tile-roof', 'tile-wall', 'tile-grass', 'tile-path');
  add(4, 5, 'tile-fence');
  add(6, 5, 'tile-gate');
  for (const s of ROOM_SIZES) {
    const [w, h] = s.split('x').map(Number) as [number, number];
    for (const k of ROOM_KINDS) add(w, h, `room-${s}-${k}`);
  }
  return out;
})();

// ------------------------------------------------------------ colours

const interned = new Map<number, Rgb>();

/** One object per colour: the pixels of all layers share them. */
function intern(p: Pixel): Pixel {
  if (!p) return null;
  const k = (p[0] << 16) | (p[1] << 8) | p[2];
  let c = interned.get(k);
  if (!c) {
    c = [p[0], p[1], p[2]];
    interned.set(k, c);
  }
  return c;
}

function same(a: Rgb, b: Rgb): boolean {
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

// -------------------------------------------------------- placeholders

/** A picture at a size: cut at the right and the bottom, or padded with transparent pixels. */
function fitTo(b: Bitmap, w: number, h: number): Bitmap {
  if (b.w === w && b.h === h) return b;
  const out: Pixel[] = [];
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out.push(x < b.w && y < b.h ? (b.px[y * b.w + x] ?? null) : null);
  return { w, h, px: out };
}

function flat(w: number, h: number, c: Pixel): Bitmap {
  return { w, h, px: new Array<Pixel>(w * h).fill(c) };
}

/** A picture from a function of its pixels. */
function paint(w: number, h: number, at: (x: number, y: number) => Pixel): Bitmap {
  const out: Pixel[] = [];
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out.push(at(x, y));
  return { w, h, px: out };
}

/** The shell of a room, drawn in the program: the wall, the floor, the outline. */
function shellPlaceholder(size: RoomSize, kind: RoomKind, w: number, h: number): Bitmap {
  const floorH = SLICE[size].bottom - 2;
  const wall = kind === 'night' ? px.NIGHT : kind === 'ice' ? px.ICE : px.CREAM;
  const floor = kind === 'night' ? px.NIGHT_FLOOR : kind === 'ice' ? mix(px.FLOOR, px.ICE, 0.55) : px.FLOOR;
  return paint(w, h, (x, y) => {
    const edge = x === 0 || y === 0 || x === w - 1 || y === h - 1;
    if (y >= h - 1 - floorH) return edge ? px.INK : floor;
    if (edge) return kind === 'glass' ? px.GLASS : px.INK;
    return kind === 'glass' ? null : wall;
  });
}

function placeholderFrames(name: string, w: number, h: number): { frames: Bitmap[]; fps: number } {
  const one = (b: Bitmap): { frames: Bitmap[]; fps: number } => ({ frames: [b], fps: SPRITE_FPS });
  const last = name.slice(name.lastIndexOf('-') + 1);
  if (name.startsWith('character-small-')) {
    const [color, state] = name.slice('character-small-'.length).split('-') as [ArtColor, ArtState];
    const s = spriteFor(color, state);
    // The character of the rooms at a third of its size.
    return { frames: s.frames.map((f) => shrink({ w: s.w, h: s.h, px: f }, 3, px.INK)), fps: SPRITE_FPS };
  }
  if (name.startsWith('window-small-')) return one(px.WINDOW_SMALL[last as WindowKey]);
  if (name.startsWith('window-')) return one(px.WINDOW[last as WindowKey]);
  if (name.startsWith('door-')) return one(paint(w, h, (x, y) => (x === 1 && y === h >> 1 ? px.INK : name === 'door-asking' ? px.AMBER : px.WOOD_LIGHT)));
  if (name.startsWith('mailbox-')) return one(last === 'up' ? px.MAILBOX.up : px.MAILBOX.down);
  if (name.startsWith('crate-')) return one(px.crate(last as 'amber' | 'red' | 'none'));
  if (name.startsWith('socket-')) return one(px.socket(last === 'cover', last === 'host'));
  if (name.startsWith('spine-')) return one(px.spine(Number(last)));
  if (name.startsWith('stool-')) return one(px.STOOL);
  if (name.startsWith('frost-')) {
    const n = [2, 3, 4, 6, 8][Number(last) - 1] ?? 8;
    return one(paint(w, h, (x, y) => (x + y < n ? px.FROST : null)));
  }
  if (name.startsWith('room-')) {
    const [, size, kind] = name.split('-') as [string, RoomSize, RoomKind];
    return one(shellPlaceholder(size, kind, w, h));
  }
  switch (name) {
    case 'card':
      return one(px.CARD);
    case 'desk':
      return one(px.DESK);
    case 'model':
      return one(px.model(null));
    case 'model-mask':
      return one(bitmap(px.MODEL_ROWS, { h: px.WHITE }));
    case 'hourglass':
      return one(px.HOURGLASS);
    case 'appliance':
      return one(px.APPLIANCE);
    case 'pegboard':
      return one(px.PEGBOARD);
    case 'ladder':
      return one(px.LADDER);
    case 'shelf':
      return one(paint(w, h, (_x, y) => (y === 0 ? px.WOOD_LIGHT : px.INK)));
    case 'bench':
      return one(px.BENCH);
    case 'island':
      return one(px.ISLAND);
    case 'fan':
      return { frames: px.FAN, fps: SPRITE_FPS };
    case 'water':
      return one(paint(w, h, (x, y) => (x === 1 && y === 0 ? px.WATER_LIGHT : px.WATER)));
    case 'envelope':
      return one(px.ENVELOPE);
    case 'trough':
      return one(px.TROUGH);
    case 'saddle':
      return one(px.SADDLE);
    case 'stall':
      return one(paint(w, h, (x, y) => (x === 0 || y === 0 || x === w - 1 || y === h - 1 ? px.INK : y < 4 ? px.WOOD : px.WOOD_LIGHT)));
    case 'tile-roof':
      return one(paint(w, h, (_x, y) => (y % 2 === 0 ? px.ROOF_LIGHT : px.ROOF)));
    case 'tile-wall':
      return one(flat(w, h, px.WALL));
    case 'tile-grass':
      return one(flat(w, h, px.GRASS));
    case 'tile-path':
      return one(flat(w, h, px.PATH));
    case 'tile-fence':
      return one(paint(w, h, (x, y) => (y === 0 ? (x === 0 ? px.INK : null) : y % 2 === 1 || x === 0 ? px.WHITE : null)));
    case 'tile-gate':
      return one(paint(w, h, (x, y) => (y === 0 ? (x === 0 || x === w - 1 ? px.INK : null) : y % 2 === 1 || x === 0 || x === w - 1 ? px.WOOD_LIGHT : null)));
    default:
      return one(flat(w, h, px.CREAM_SHADE));
  }
}

// --------------------------------------------------------------- loader

let files: Record<string, string> = WORLD_FILES;
let report: (line: string) => void = (line) => console.warn(line);
const layers = new Map<string, Layer>();
const made = new Map<string, Bitmap>();

/**
 * The files that the layers come from: the ones of the build, or others
 * (a test). Everything that was made from the old files is forgotten.
 */
export function setLayerFiles(f: Record<string, string> = WORLD_FILES, log?: (line: string) => void): void {
  files = f;
  report = log ?? ((line) => console.warn(line));
  layers.clear();
  made.clear();
}

/** A layer by its name: read once, then kept. */
export function layer(name: string): Layer {
  const kept = layers.get(name);
  if (kept) return kept;
  const want = LAYER_SIZES[name];
  if (!want) throw new Error(`the world has no layer ${name}`);
  const [w, h] = want;
  const text = files[name];
  const s = text === undefined ? null : parseSprite(text);
  let l: Layer;
  if (s && s.w === w && s.h === h) {
    l = { name, w, h, fps: s.fps ?? SPRITE_FPS, frames: s.frames.map((f) => ({ w, h, px: f.map(intern) })), placeholder: false };
  } else {
    // Once for each layer: the layer is kept, and so is its placeholder.
    report(`paddock tui: the layer ${name}.json of the world is ${text === undefined ? 'missing' : s ? `${s.w} x ${s.h} px, not ${w} x ${h}` : 'not a sprite'}. Its placeholder shows.`);
    const p = placeholderFrames(name, w, h);
    l = { name, w, h, fps: p.fps, frames: p.frames.map((f) => fitTo(f, w, h)), placeholder: true };
  }
  layers.set(name, l);
  return l;
}

function memo(key: string, make: () => Bitmap): Bitmap {
  let b = made.get(key);
  if (!b) {
    // A window that changes its size for a long time: the old sizes go.
    if (made.size > 2000) made.clear();
    b = make();
    made.set(key, b);
  }
  return b;
}

// ------------------------------------------------------------- pictures

/**
 * Which frame of a loop shows at a tick of the clock (8 a second). A
 * frame stays for a whole number of ticks, so a loop has an even step: at
 * 6 or 8 frames a second one tick, at 3 three ticks, at 2 four ticks.
 */
export function frameAt(l: { fps: number; frames: { length: number } }, tick: number): number {
  const n = l.frames.length;
  return n > 1 ? Math.floor(Math.max(0, tick) / Math.max(1, Math.round(SPRITE_FPS / l.fps))) % n : 0;
}

/** Whether a layer has a loop. */
export function loops(name: string): boolean {
  return layer(name).frames.length > 1;
}

/** A layer at a tick of the clock, or one of its frames. */
export function pic(name: string, tick = 0): Bitmap {
  const l = layer(name);
  return l.frames[frameAt(l, tick)] as Bitmap;
}

/** A part of a layer. */
export function part(name: string, tick: number, x: number, y: number, w: number, h: number): Bitmap {
  const l = layer(name);
  const f = frameAt(l, tick);
  return memo(`part:${name}#${f}:${x},${y},${w},${h}`, () => {
    const b = l.frames[f] as Bitmap;
    return paint(w, h, (xx, yy) => (x + xx < b.w && y + yy < b.h ? (b.px[(y + yy) * b.w + x + xx] ?? null) : null));
  });
}

/** A tile over an area: it repeats both ways. `oy` is the row of the tile that the area starts with. */
export function tiles(name: string, w: number, h: number, tick = 0, oy = 0, t?: Tint): Bitmap {
  const l = layer(name);
  const f = frameAt(l, tick);
  const key = `tiles:${name}#${f}:${w}x${h}+${oy}`;
  const b = memo(key, () => {
    const src = l.frames[f] as Bitmap;
    return paint(Math.max(0, w), Math.max(0, h), (x, y) => src.px[((y + oy) % src.h) * src.w + (x % src.w)] ?? null);
  });
  return t ? tinted(key, b, t) : b;
}

export type Tint = 'night' | 'select' | 'glow';

function tintMap(t: Tint): (c: Rgb) => Rgb {
  // Night: the light is off. Select: the ink line takes the accent. Glow: all of it does.
  if (t === 'night') return (c) => mix(c, px.NIGHT, 0.5);
  if (t === 'glow') return () => px.SELECT;
  return (c) => (same(c, px.INK) || same(c, px.GLASS) ? px.SELECT : c);
}

function tinted(key: string, b: Bitmap, t: Tint): Bitmap {
  return memo(`${t}:${key}`, () => {
    const map = tintMap(t);
    return { w: b.w, h: b.h, px: b.px.map((p) => (p ? intern(map(p)) : null)) };
  });
}

/** A layer in a tint, at a tick of the clock. */
export function tint(name: string, t: Tint, tick = 0): Bitmap {
  return tintFrame(name, frameAt(layer(name), tick), t);
}

/** One frame of a layer in a tint. */
export function tintFrame(name: string, f: number, t: Tint): Bitmap {
  return tinted(`${name}#${f}`, layer(name).frames[f] as Bitmap, t);
}

/** The shell of a room at a size in pixels: the corners keep their size, the edges and the middle stretch. */
export function shell(size: RoomSize, kind: RoomKind, w: number, h: number, t?: Tint): Bitmap {
  const name = `room-${size}-${kind}`;
  const key = `shell:${name}:${w}x${h}`;
  const b = memo(key, () => {
    const src = pic(name);
    const s = SLICE[size];
    // The middle is one colour in each row and column: it stretches. A wall lower than the two cuts keeps its floor.
    const sx = (x: number): number => (x >= w - s.right ? src.w - (w - x) : Math.min(x, src.w - s.right - 1));
    const sy = (y: number): number => (y >= h - s.bottom ? src.h - (h - y) : Math.min(y, src.h - s.bottom - 1));
    return paint(w, h, (x, y) => {
      const yy = sy(y);
      const xx = sx(x);
      return yy < 0 || xx < 0 ? null : (src.px[yy * src.w + xx] ?? null);
    });
  });
  return t ? tinted(key, b, t) : b;
}

/** Two pictures of one size, mixed: t = 0 gives a, t = 1 gives b. For the few frames of a change. */
export function blend(a: Bitmap, b: Bitmap, t: number): Bitmap {
  if (t <= 0 || a.w !== b.w || a.h !== b.h) return a;
  if (t >= 1) return b;
  return { w: a.w, h: a.h, px: a.px.map((p, i) => {
    const q = b.px[i] ?? null;
    return p && q ? mix(p, q, t) : (p ?? q);
  }) };
}

/** Frost in a corner of a room, at a step of its growth (1 to 5). The layer is the top left corner. */
export function frost(step: number, corner: Corner): Bitmap {
  const n = Math.max(1, Math.min(FROST_STEPS, step));
  const name = `frost-${n}`;
  if (corner === 'tl') return pic(name);
  return memo(`frost:${n}:${corner}`, () => {
    const b = pic(name);
    const fx = corner === 'tr' || corner === 'br';
    const fy = corner === 'bl' || corner === 'br';
    return paint(b.w, b.h, (x, y) => b.px[(fy ? b.h - 1 - y : y) * b.w + (fx ? b.w - 1 - x : x)] ?? null);
  });
}

/** The scale model of the project. The part that changed takes a colour: the one of the agent. */
export function model(changed: Rgb | null): Bitmap {
  if (!changed) return pic('model');
  return memo(`model:${changed.join(',')}`, () => {
    const b = pic('model');
    const m = pic('model-mask');
    const c = intern(changed);
    return { w: b.w, h: b.h, px: b.px.map((p, i) => (m.px[i] ? c : p)) };
  });
}

/** The character of a room at its full size (24 x 24): a frame of its loop. `night`: its light is off. */
export function character(color: ArtColor, state: ArtState, f: number, night = false): Bitmap {
  const key = `big:${color}:${state}:${f}`;
  const b = memo(key, () => {
    const s = spriteFor(color, state);
    return { w: s.w, h: s.h, px: (s.frames[f % s.frames.length] as Pixel[]).map(intern) };
  });
  return night ? tinted(key, b, 'night') : b;
}

/** The name of the small character of the ranch (8 x 8). */
export function smallName(color: ArtColor, state: ArtState): string {
  return `character-small-${color}-${state}`;
}
