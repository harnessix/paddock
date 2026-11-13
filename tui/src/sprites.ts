// The characters of the rooms: which sprite a run gets, and the loader of
// the sprite files. A sprite file is sprites/character-COLOR-STATE.json,
// the toon art of the launch video (sprites/frames.json):
// {"w":24,"h":24,"fps":8,"frames":[[[r,g,b] or null per pixel, row by row]]}.
// The build puts the 35 files of the 7 colours and 5 states into the
// program (src/sprite-files.ts), and beside them the layers of the world
// (sprites/world, read by world/layers.ts with the same parser).

import type { RunRow } from './model.js';
import { SPRITE_FILES } from './sprite-files.js';

export type Rgb = readonly [number, number, number];
export type Pixel = Rgb | null;

export interface Sprite {
  w: number;
  h: number;
  /** Each frame: w*h pixels, row by row. */
  frames: Pixel[][];
  /** The frame rate of the file, when it gives one. */
  fps?: number;
}

export type ArtState = 'work' | 'wait' | 'idle' | 'sleep' | 'blocked';
export type ArtColor = 'orange' | 'blue' | 'purple' | 'green' | 'pink' | 'teal' | 'gray';

export const SPRITE_W = 24;
export const SPRITE_H = 24;

export const ART_COLORS: readonly ArtColor[] = ['orange', 'blue', 'purple', 'green', 'pink', 'teal', 'gray'];
export const ART_STATES: readonly ArtState[] = ['idle', 'work', 'wait', 'sleep', 'blocked'];

/** The frames of each state's loop, as in sprites/frames.json. All loops play at 8 fps. */
export const STATE_FRAMES: Record<ArtState, number> = { idle: 8, work: 8, wait: 8, sleep: 4, blocked: 4 };
export const SPRITE_FPS = 8;

const HARNESS_COLOR: Record<string, ArtColor> = {
  claude: 'orange',
  codex: 'blue',
  gemini: 'purple',
  copilot: 'green',
  grok: 'pink',
  opencode: 'teal',
};

/** The colour of an agent: gray for any agent not in the list. */
export function colorOf(harness: string): ArtColor {
  return HARNESS_COLOR[harness.toLowerCase()] ?? 'gray';
}

/** The sprite state of a run, from the state that paddock reported. */
export function artState(r: Pick<RunRow, 'state' | 'frozen'>): ArtState {
  if (r.frozen && r.state !== 'waiting') return 'sleep';
  switch (r.state) {
    case 'running':
      return 'work';
    case 'waiting':
      return 'wait';
    case 'idle':
      return 'idle';
    case 'done':
    case 'ended':
      return 'sleep';
    case 'failed':
    case 'blocked':
    case 'stuck':
      return 'blocked';
  }
}

function isByte(v: unknown): v is number {
  return typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= 255;
}

function pixel(v: unknown): Pixel | undefined {
  if (v === null) return null;
  if (Array.isArray(v) && v.length === 3 && v.every(isByte)) return [v[0], v[1], v[2]] as Rgb;
  return undefined;
}

/**
 * A sprite from the text of its file, or null when the shape is wrong. A
 * frame is a flat list of w*h pixels, or h rows of w pixels.
 */
export function parseSprite(text: string): Sprite | null {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return null;
  }
  if (!v || typeof v !== 'object') return null;
  const o = v as { w?: unknown; h?: unknown; frames?: unknown; fps?: unknown };
  const { w, h } = o;
  if (typeof w !== 'number' || typeof h !== 'number' || !Number.isInteger(w) || !Number.isInteger(h)) return null;
  if (w < 1 || h < 1 || w > 64 || h > 64) return null;
  if (!Array.isArray(o.frames) || o.frames.length === 0 || o.frames.length > 64) return null;
  const frames: Pixel[][] = [];
  for (const f of o.frames as unknown[]) {
    if (!Array.isArray(f)) return null;
    const flat: unknown[] = f.length === h && f.every((row) => Array.isArray(row) && row.length === w && !isByte(row[0])) ? f.flat() : f;
    if (flat.length !== w * h) return null;
    const out: Pixel[] = [];
    for (const p of flat) {
      const q = pixel(p);
      if (q === undefined) return null;
      out.push(q);
    }
    frames.push(out);
  }
  if (o.fps === undefined) return { w, h, frames };
  if (typeof o.fps !== 'number' || !(o.fps > 0)) return null;
  return { w, h, frames, fps: o.fps };
}

/** The name of a sprite file, without ".json". */
export function spriteName(color: ArtColor, state: ArtState): string {
  return `character-${color}-${state}`;
}

/** A sprite of the art set: 24 x 24, the frames of its state, 8 fps. */
function fits(s: Sprite, state: ArtState): boolean {
  return s.w === SPRITE_W && s.h === SPRITE_H && s.frames.length === STATE_FRAMES[state] && (s.fps ?? SPRITE_FPS) === SPRITE_FPS;
}

const INK: Rgb = [43, 33, 64];
const BODY: Rgb = [150, 150, 162];
const SHADE: Rgb = [112, 112, 128];

/**
 * The built-in character, for a sprite file that is missing or broken: a
 * still gray toon with the ink outline of the art set. It needs no file.
 */
function drawFallback(): Sprite {
  const W = SPRITE_W;
  const H = SPRITE_H;
  const inside = (x: number, y: number): boolean => {
    const head = (x - 11.5) ** 2 + (y - 9.5) ** 2 <= 30;
    const body = x >= 7 && x <= 16 && y >= 14 && y <= 21;
    return head || body;
  };
  const f: Pixel[] = [];
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      if (!inside(x, y)) {
        const edge = [-1, 0, 1].some((dy) => [-1, 0, 1].some((dx) => inside(x + dx, y + dy)));
        f.push(edge ? INK : null);
      } else if ((y === 9 || y === 10) && (x === 9 || x === 14)) {
        f.push(INK);
      } else {
        f.push(x >= 14 && y >= 14 ? SHADE : BODY);
      }
    }
  }
  return { w: W, h: H, frames: [f] };
}

export const FALLBACK_SPRITE: Sprite = drawFallback();

const cache = new Map<string, Sprite>();

/**
 * The sprite of an agent colour in a state: its own file, else the gray
 * file of the state (the art of an unknown agent), else the built-in
 * character. A file counts only with 24 x 24 pixels and the frame count
 * of its state. The bundled sprites are parsed once and kept.
 */
export function spriteFor(color: ArtColor, state: ArtState, files: Record<string, string> = SPRITE_FILES): Sprite {
  const key = spriteName(color, state);
  const bundled = files === SPRITE_FILES;
  const hit = bundled ? cache.get(key) : undefined;
  if (hit) return hit;
  const load = (name: string): Sprite | null => {
    const text = files[name];
    const s = text !== undefined ? parseSprite(text) : null;
    return s && fits(s, state) ? s : null;
  };
  const s = load(key) ?? (color === 'gray' ? null : load(spriteName('gray', state))) ?? FALLBACK_SPRITE;
  if (bundled) cache.set(key, s);
  return s;
}
