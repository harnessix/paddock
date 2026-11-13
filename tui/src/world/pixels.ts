// The palette of the world, and its placeholder pictures. The world draws
// with the layers of the launch video scene (layers.ts, sprites/world).
// A picture here shows only when the file of a layer is missing or
// broken: it is drawn in the program, in the style of the art (the ink
// outline, flat toon colours, no gradients). The colours are the ones of
// the layers, so a line that the program draws (a cable, a pipe) matches
// the art beside it.
//
// Amber has one meaning in the world: "this touches your real machine" or
// "this waits for you". No other picture may use it.

import { bitmap, type Bitmap } from './canvas.js';
import type { Rgb } from '../sprites.js';

export const INK: Rgb = [43, 33, 64];
export const AMBER: Rgb = [255, 191, 77];
/** The light and the shade of amber in the layers. */
export const AMBER_LIGHT: Rgb = [255, 209, 150];
export const AMBER_SHADE: Rgb = [211, 156, 63];
/** Amber for text on a light background. */
export const AMBER_DARK: Rgb = [206, 132, 22];
export const CREAM: Rgb = [255, 239, 209];
export const CREAM_SHADE: Rgb = [211, 197, 173];
export const FLOOR: Rgb = [250, 207, 154];
export const FLOOR_DARK: Rgb = [202, 170, 128];
export const GLASS: Rgb = [143, 180, 214];
export const SKY: Rgb = [195, 251, 255];
export const ICE: Rgb = [228, 255, 255];
export const FROST: Rgb = [244, 251, 255];
export const NIGHT: Rgb = [76, 70, 123];
export const NIGHT_FLOOR: Rgb = [92, 80, 129];
export const WOOD: Rgb = [172, 114, 61];
export const WOOD_LIGHT: Rgb = [222, 162, 98];
export const GRASS: Rgb = [162, 232, 149];
export const GRASS_DARK: Rgb = [130, 191, 123];
export const WATER: Rgb = [127, 206, 255];
export const WATER_LIGHT: Rgb = [236, 248, 255];
export const RED: Rgb = [255, 95, 103];
export const STEEL: Rgb = [141, 151, 175];
export const STEEL_DARK: Rgb = [113, 123, 146];
export const WHITE: Rgb = [255, 255, 255];
export const SAND: Rgb = [239, 219, 149];
export const GREEN: Rgb = [97, 219, 172];
export const ROOF: Rgb = [255, 145, 132];
export const ROOF_DARK: Rgb = [211, 119, 109];
export const ROOF_LIGHT: Rgb = [255, 181, 174];
/** The face of a house. */
export const WALL: Rgb = [255, 255, 249];
/** The path between a helper and the run that started it. */
export const PATH: Rgb = [250, 248, 238];
export const PAPER: Rgb = [255, 255, 242];
/** The colour of the selection: the accent of the interface in the art. */
export const SELECT: Rgb = [64, 224, 240];

/** The accent on a light terminal. */
export const SELECT_DARK: Rgb = [0, 132, 164];

/** Amber for text on the terminal's own background: the darker shade on a light terminal. */
export function amberText(light: boolean): Rgb {
  return light ? AMBER_DARK : AMBER;
}

/** The accent for lines on the terminal's own background. */
export function accentText(light: boolean): Rgb {
  return light ? SELECT_DARK : SELECT;
}

const P: Record<string, Rgb> = {
  k: INK,
  a: AMBER,
  A: AMBER_DARK,
  c: CREAM,
  C: CREAM_SHADE,
  f: FLOOR,
  F: FLOOR_DARK,
  g: GLASS,
  s: SKY,
  i: ICE,
  w: WHITE,
  n: NIGHT,
  d: WOOD,
  D: WOOD_LIGHT,
  r: RED,
  t: STEEL,
  T: STEEL_DARK,
  b: WATER,
  B: WATER_LIGHT,
  e: GREEN,
  E: GRASS_DARK,
  y: SAND,
  p: PAPER,
  o: ROOF,
  O: ROOF_DARK,
};

const bm = (rows: string[]): Bitmap => bitmap(rows, P);

/** The window of a room, by network mode: 7 x 8 pixels. */
export const WINDOW: Record<'none' | 'api' | 'allowlist' | 'egress' | 'host' | 'unknown', Bitmap> = {
  // Shutters closed.
  none: bm(['kkkkkkk', 'kdDkdDk', 'kdDkdDk', 'kdDkdDk', 'kdDkdDk', 'kdDkdDk', 'kdDkdDk', 'kkkkkkk']),
  // One pipe leaves through the pane.
  api: bm(['kkkkkkk', 'kssssCk', 'kssssCk', 'kttttTk', 'kttttTk', 'kssssCk', 'kssssCk', 'kkkkkkk']),
  // A grille.
  allowlist: bm(['kkkkkkk', 'kskskCk', 'kkkkkkk', 'kskskCk', 'kkkkkkk', 'kskskCk', 'kskskCk', 'kkkkkkk']),
  // Open.
  egress: bm(['kkkkkkk', 'ksssswk', 'kssswsk', 'ksswssk', 'ksssssk', 'ksssssk', 'ksssssk', 'kkkkkkk']),
  // An open door to your machine: amber.
  host: bm(['kkkkkkk', 'kaaaaAk', 'kaaaaAk', 'kaaaaAk', 'kaaaaAk', 'kaakaAk', 'kaaaaAk', 'kaaaaAk']),
  unknown: bm(['kkkkkkk', 'kCCCCCk', 'kCCCCCk', 'kCCCCCk', 'kCCCCCk', 'kCCCCCk', 'kCCCCCk', 'kkkkkkk']),
};

/** The small window of a room on the ranch: 3 x 3 pixels. */
export const WINDOW_SMALL: Record<keyof typeof WINDOW, Bitmap> = {
  none: bm(['dDd', 'dDd', 'dDd']),
  api: bm(['sss', 'ttt', 'sss']),
  allowlist: bm(['sks', 'kkk', 'sks']),
  egress: bm(['ssw', 'sws', 'sss']),
  host: bm(['aaa', 'aaa', 'aaa']),
  unknown: bm(['CCC', 'CCC', 'CCC']),
};

/** The card of a request: amber, with its mark. 5 x 7 pixels. */
export const CARD: Bitmap = bm(['kkkkk', 'kaaak', 'kakak', 'kakak', 'kaaak', 'kakak', 'kkkkk']);

/** The mailbox on its post, flag down and flag up: 6 x 11 pixels. */
export const MAILBOX: { down: Bitmap; up: Bitmap } = {
  down: bm(['......', '......', '......', 'kkkkk.', 'ktttkk', 'ktTtkr', 'kkkkkk', '..k...', '..k...', '..k...', '..k...']),
  up: bm(['.....r', '....kr', '....kr', 'kkkkk.', 'kpppk.', 'kpTpk.', 'kkkkk.', '..k...', '..k...', '..k...', '..k...']),
};

/** The desk: 11 x 7 pixels. The model of the project stands on it. */
export const DESK: Bitmap = bm(['kkkkkkkkkkk', 'kDDDDDDDDDk', 'kkkkkkkkkkk', '.kd.....dk.', '.kd.....dk.', '.kd.....dk.', '.kk.....kk.']);

/** The scale model of the project: 7 x 5 pixels. `h` is the part that changed: it takes the colour of the agent. */
export const MODEL_ROWS = ['..kkk..', '.kooOk.', 'kkkkkkk', 'kchchCk', 'kkkkkkk'];

export function model(changed: Rgb | null): Bitmap {
  return bitmap(MODEL_ROWS, { ...P, h: changed ?? CREAM });
}

/** The hourglass of a timer: 3 x 5 pixels. */
export const HOURGLASS: Bitmap = bm(['kkk', 'kyk', '.k.', 'kyk', 'kkk']);

/** A crate, the tag in amber (yours) or red (the original changed): 7 x 6 pixels. */
export function crate(tag: 'amber' | 'red' | 'none'): Bitmap {
  return bitmap(['kkkkkkk', 'kDdDdDk', 'kdDqqdk', 'kDdqqDk', 'kdDdDdk', 'kkkkkkk'], { ...P, q: tag === 'amber' ? AMBER : tag === 'red' ? RED : WOOD_LIGHT });
}

/** An appliance in its socket: 5 x 5 pixels. */
export const APPLIANCE: Bitmap = bm(['kkkkk', 'ktwtk', 'ktttk', 'kTTTk', 'kkkkk']);

/**
 * A socket in the wall, open or with a cover (the tool asks first): 3 x 4
 * pixels. Amber only for a tool that runs on your machine.
 */
export function socket(cover: boolean, amber: boolean): Bitmap {
  return bitmap(['kkk', 'kqk', 'kqk', 'kkk'], { ...P, q: amber ? AMBER : cover ? STEEL_DARK : WHITE });
}

/** The board with the tags of the shared notes: 11 x 5 pixels. */
export const PEGBOARD: Bitmap = bm(['kkkkkkkkkkk', 'kDpDpDpDpDk', 'kDpDpDpDpDk', 'kDDDDDDDDDk', 'kkkkkkkkkkk']);

/** A stool of the build bench, free and taken: 4 x 4 pixels. */
export const STOOL: Bitmap = bm(['kkkk', 'kDDk', '.kk.', 'k..k']);

/** The build bench: 16 x 4 pixels. */
export const BENCH: Bitmap = bm(['kkkkkkkkkkkkkkkk', 'kDDDDDDDDDDDDDDk', 'kkkkkkkkkkkkkkkk', '.kd..........dk.']);

/** The far island: 16 x 8 pixels on the water. The house on it is the other computer. */
export const ISLAND: Bitmap = bm([
  '....kkkkkkkk....',
  '....ktttttTk....',
  '....ktwwttTk....',
  '....ktwwttTk....',
  '..kkkkkkkkkkkk..',
  '.kyyeeeeeeeyyyk.',
  'kyyyyyyyyyyyyyyk',
  '.kkkkkkkkkkkkkk.',
]);

/** The fan of an island with a graphics card: 4 frames of 3 x 3 pixels. */
export const FAN: Bitmap[] = [bm(['w.w', '.k.', 'w.w']), bm(['.w.', 'wkw', '.w.']), bm(['w.w', '.k.', 'w.w']), bm(['.w.', 'wkw', '.w.'])];

/** The envelope that a helper sends when it ends: 5 x 4 pixels. */
export const ENVELOPE: Bitmap = bm(['kkkkk', 'kpkpk', 'kpppk', 'kkkkk']);

/** The feed trough at the gate (the use of your accounts): 8 x 4 pixels. */
export const TROUGH: Bitmap = bm(['k......k', 'kdyyyydk', 'kddddddk', '.kkkkkk.']);

/** A notebook on the shelf of the loft, by its place: 2 pixels wide, 6 high. */
export function spine(i: number): Bitmap {
  const colors: Rgb[] = [ROOF, WATER, GREEN, SAND, STEEL, WOOD_LIGHT];
  const c = colors[i % colors.length] as Rgb;
  return bitmap(['kk', 'qk', 'qk', 'wk', 'qk', 'kk'], { ...P, q: c });
}

/** The ladder to the loft, one piece: 5 x 4 pixels. It repeats upward. */
export const LADDER: Bitmap = bm(['d...d', 'ddddd', 'd...d', 'd...d']);

/** A saddle on the wall of the stable: one saved session. 5 x 4 pixels. */
export const SADDLE: Bitmap = bm(['.kkk.', 'kdDdk', 'kdddk', 'k.k.k']);
