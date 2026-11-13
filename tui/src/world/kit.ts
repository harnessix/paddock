// What the pictures of the world share: the view, the things that can be
// selected, and the small parts that the ranch, a building and a room all
// draw (the character of a room, its shell, its frost, its door).

import type { ArtMode } from '../art.js';
import { cut } from '../clean.js';
import { t } from '../i18n/index.js';
import { STATE } from '../model.js';
import { artState, colorOf, spriteFor, type ArtColor, type Rgb } from '../sprites.js';
import { bitmap, mix, type Bitmap, type Canvas, type TextStyle } from './canvas.js';
import * as art from './layers.js';
import { arc, ease, FRAMES, type LoftTrip, type Motion } from './motion.js';
import type { NetMode, Room, Tool, World } from './model.js';
import * as px from './pixels.js';

export type Level = 'ranch' | 'building' | 'room' | 'loft';

export interface View {
  level: Level;
  /** The key of the house in view (building, room, loft). */
  house: string;
  /** The id of the room in view (room). */
  room: string;
  /** The id of the selected thing. */
  sel: string;
  /** The Plumbing view: glass walls, sockets, cables and labels. */
  plumbing: boolean;
}

export type HitKind = 'room' | 'loft' | 'yard' | 'bench' | 'island' | 'gate' | 'stable' | 'sign' | 'door' | 'window' | 'mail' | 'self' | 'copy' | 'tool' | 'borrow' | 'notes' | 'helper' | 'timer' | 'note';

/** A thing of the picture that can be selected: where it is, in cells. */
export interface Hit {
  id: string;
  kind: HitKind;
  x: number;
  y: number;
  w: number;
  h: number;
  /** The key of its house, the id of its room, the name of its tool or yard. */
  house?: string;
  room?: string;
  name?: string;
}

export interface SceneOptions {
  w: number;
  h: number;
  mode: ArtMode;
  plain?: boolean;
  /** A light terminal. */
  light?: boolean;
  tick: number;
  motion: Motion;
  view: View;
  /** Unix seconds, for the words of time. */
  t?: number;
}

export interface Drawn {
  canvas: Canvas;
  hits: Hit[];
  /** Whether a character loops in the picture: the clock runs while one does. */
  loops: boolean;
}

export const AGENT: Record<ArtColor, Rgb> = {
  orange: [255, 160, 64],
  blue: [104, 170, 251],
  purple: [170, 130, 240],
  green: [110, 200, 120],
  pink: [255, 128, 170],
  teal: [80, 200, 190],
  gray: [150, 150, 162],
};

export const DIM: TextStyle = { dim: true };
export const BOLD: TextStyle = { bold: true };

export interface Ctx {
  c: Canvas;
  world: World;
  o: SceneOptions;
  hits: Hit[];
  loops: boolean;
  t: number;
  /** The cells at the right end of the last line that a note of the scroll takes. The line of words ends before them. */
  note: number;
}

/** The note of a picture that scrolls ("↓ 2 more rows"), at the right end of the last line. */
export function scrollNote(ctx: Ctx, above: boolean, below: number, what: 'row' | 'floor'): void {
  if (!above && below <= 0) return;
  const { c, o } = ctx;
  const note = `${above ? '↑ ' : ''}${below > 0 ? t(what === 'row' ? 'world.scroll.rows' : 'world.scroll.floors', { n: below }) : ''}`;
  c.text(Math.max(0, o.w - 22), o.h - 1, cut(note, 22), DIM);
  ctx.note = 24;
}

export function hit(ctx: Ctx, h: Hit): void {
  ctx.hits.push(h);
}

export function selected(ctx: Ctx, id: string): boolean {
  return ctx.o.view.sel === id;
}

/** The word and glyph of the state of a room, in the plain words of the world. */
export function roomWord(r: Room): { glyph: string; word: string; amber: boolean } {
  if (r.asks.length > 0) return { glyph: '◆', word: t('world.room.asksYou'), amber: true };
  if (r.run.frozen) return { glyph: '❄', word: t('world.room.paused'), amber: false };
  const s = STATE[r.run.state];
  return { glyph: s.glyph, word: s.word, amber: false };
}

/** The face of a character without art: one glyph for its state. */
export function face(r: Room): string {
  if (r.run.frozen && r.asks.length === 0) return '❄';
  switch (r.run.state) {
    case 'waiting':
    case 'running':
      return '◕';
    case 'idle':
      return '◔';
    case 'done':
    case 'ended':
      return 'z';
    default:
      return '×';
  }
}

/**
 * The character of a room at a frame of the clock: at its full size, and
 * the small one of the ranch. A frozen character keeps the pose it had:
 * the sprite of its state, still. It does not sleep. In a room whose run
 * ended, the light is off.
 */
export function character(ctx: Ctx, r: Room): { frame: Bitmap; small: Bitmap; still: boolean } {
  const frozen = r.run.frozen && r.asks.length === 0;
  const state = frozen ? artState({ state: r.run.state, frozen: false }) : artState(r.run);
  const color = colorOf(r.run.harness);
  const still = frozen || !r.run.live;
  const night = !r.run.live;
  const n = spriteFor(color, state).frames.length;
  const small = art.layer(art.smallName(color, state));
  const sn = small.frames.length;
  if (!still && (n > 1 || sn > 1)) ctx.loops = true;
  const sf = still ? Math.min(2, sn - 1) : art.frameAt(small, ctx.o.tick);
  return {
    frame: art.character(color, state, still ? Math.min(2, n - 1) : ctx.o.tick % n, night),
    small: night ? art.tintFrame(small.name, sf, 'night') : (small.frames[sf] as Bitmap),
    still,
  };
}

/** How much frost a room has: 0 none, 1 full. It grows when the run freezes and melts when it thaws. */
export function frost(ctx: Ctx, r: Room): number {
  const { motion, tick } = ctx.o;
  if (r.run.frozen) return ease(motion.progress('freeze', r.id, tick));
  return 1 - ease(motion.progress('thaw', r.id, tick));
}

/** How far the character is on its way to the door: 0 at its place, 1 at the door. */
export function atDoor(ctx: Ctx, r: Room): number {
  const { motion, tick } = ctx.o;
  if (r.asks.length > 0) return ease(motion.progress('ask', r.id, tick));
  return 1 - ease(motion.progress('answer', r.id, tick));
}

/**
 * The shell of a room: glass in the Plumbing view, dark when the run
 * ended, lit, or ice while it is paused. For the few frames after the
 * Plumbing view goes on or off, the walls turn to glass or back.
 */
export function roomShell(ctx: Ctx, size: art.RoomSize, r: Room, w: number, h: number, f: number, sel: boolean): Bitmap {
  const tint = sel ? 'select' : undefined;
  const { motion, tick, view } = ctx.o;
  const glass = view.plumbing ? ease(motion.progress('plumb', 'world', tick)) : 1 - ease(motion.progress('unplumb', 'world', tick));
  if (glass >= 1) return art.shell(size, 'glass', w, h, tint);
  const wall = (): Bitmap => {
    if (!r.run.live) return art.shell(size, 'night', w, h, tint);
    if (f >= 1) return art.shell(size, 'ice', w, h, tint);
    const lit = art.shell(size, 'lit', w, h, tint);
    // The few frames in which the room freezes or thaws.
    return f > 0 ? art.blend(lit, art.shell(size, 'ice', w, h, tint), f) : lit;
  };
  return glass > 0 ? art.blend(wall(), art.shell(size, 'glass', w, h, tint), glass) : wall();
}

/**
 * The window of a room. For the few frames after its mode changed, the
 * old window turns into the new one.
 */
export function windowOf(ctx: Ctx, r: Room, small = false): Bitmap {
  const name = (n: NetMode): string => `window-${small ? 'small-' : ''}${winKey(n)}`;
  const now = art.pic(name(r.net));
  const { motion, tick } = ctx.o;
  const swap = motion.progress('net', r.id, tick);
  if (swap >= 1) return now;
  const was = motion.from('net', r.id) as NetMode | undefined;
  // The old mode is not known: the shutters of a window with no mode show for a moment.
  if (was === undefined) return swap < 0.5 ? art.pic(name('unknown')) : now;
  return art.blend(art.pic(name(was)), now, swap);
}

const mailboxes = new Map<number, Bitmap>();

/** The mailbox with its flag down (0), half up, or up (1). */
export function mailbox(up: number): Bitmap {
  if (up <= 0) return art.pic('mailbox-down');
  if (up >= 1) return art.pic('mailbox-up');
  let b = mailboxes.get(0.5);
  if (!b) {
    // Half up: the flag of the raised box, two pixels lower on its pole.
    const src = art.pic('mailbox-up');
    const out = src.px.slice();
    for (let y = 3; y >= 0; y--) for (const x of [4, 5]) out[y * src.w + x] = y >= 2 ? (src.px[(y - 2) * src.w + x] ?? null) : null;
    b = { w: src.w, h: src.h, px: out };
    mailboxes.set(0.5, b);
  }
  return b;
}

/** How far up the flag of a mailbox is: it goes up when unread mail comes. */
export function flagUp(ctx: Ctx, r: Room): number {
  return r.unread > 0 ? ctx.o.motion.progress('mail', r.id, ctx.o.tick) : 0;
}

/** How many pixels a mailbox is off the ground: it hops once for a question. */
export function hop(ctx: Ctx, r: Room): number {
  const { motion, tick } = ctx.o;
  if (!motion.running('hop', r.id, tick)) return 0;
  // The middle of each frame: up, at the top, down.
  return Math.round(arc(motion.progress('hop', r.id, tick) + 1 / (2 * FRAMES.hop)) * 2);
}

/** Where the character of a room is on its way to the loft and back. */
export function trip(ctx: Ctx, r: Room): LoftTrip {
  return ctx.o.motion.trip(r.id, ctx.o.tick);
}

/** A character that writes: the loop of its work, at its full size and small. */
export function writing(ctx: Ctx, r: Room): { frame: Bitmap; small: Bitmap } {
  const color = colorOf(r.run.harness);
  // With Reduce Motion it stands still while it writes.
  const tick = ctx.o.motion.reduce ? 0 : ctx.o.tick;
  const small = art.layer(art.smallName(color, 'work'));
  return { frame: art.character(color, 'work', tick % spriteFor(color, 'work').frames.length), small: small.frames[art.frameAt(small, tick)] as Bitmap };
}

/** Whether a call of a tool shows on its cable at this frame, and how: a spark at a place (0 to 1), or the bright cable of Reduce Motion. */
export function call(ctx: Ctx, r: Room, tool: string): { spark: number | null; bright: boolean; rise: number | null } {
  const { motion, tick } = ctx.o;
  const id = `${r.id}:${tool}`;
  return {
    // The middle of each frame, so the spark is on the cable in every one.
    spark: motion.running('spark', id, tick) ? motion.progress('spark', id, tick) + 1 / (2 * FRAMES.spark) : null,
    bright: motion.hint('spark', id, tick),
    rise: motion.running('rise', id, tick) ? motion.progress('rise', id, tick) : null,
  };
}

/**
 * A tag of the shared notes that was written, 1 x 2 pixels on a board of
 * one colour. While it flips it shows its edge: first low, then high.
 * With Reduce Motion a dot stands beside it for two seconds.
 */
export function tagFlip(ctx: Ctx, id: string, x: number, y: number, board: Rgb): boolean {
  const { c } = ctx;
  const { motion, tick } = ctx.o;
  if (motion.running('flip', id, tick)) {
    const second = motion.progress('flip', id, tick) >= 0.5;
    c.pixel(x, y, second ? px.WHITE : board);
    c.pixel(x, y + 1, second ? board : px.WHITE);
    return true;
  }
  if (!motion.hint('flip', id, tick)) return false;
  c.fill(x, y, 1, 2, px.WHITE);
  c.pixel(x + 1, y, px.SELECT);
  return true;
}

const FLAKE: Bitmap = bitmap(['kkkkkkk', 'kwgwgwk', 'kgwwwgk', 'kwwwwwk', 'kgwwwgk', 'kwgwgwk', 'kkkkkkk'], { k: px.INK, g: px.GLASS, w: px.WHITE });

/**
 * Reduce Motion: a paused room has a snowflake badge. The frost does not
 * grow there, so the badge says at once that the run is paused.
 */
export function flakeBadge(ctx: Ctx, r: Room, x: number, y: number): void {
  if (ctx.o.motion.reduce && r.run.frozen) ctx.c.blit(FLAKE, x, y);
}

/** Frost in the four corners of a rectangle of pixels, at a step of its growth. */
export function frostCorners(c: Canvas, x: number, y: number, w: number, h: number, step: number): void {
  if (step < 1) return;
  const s = art.LAYER_SIZES['frost-1'] as readonly [number, number];
  c.blit(art.frost(step, 'tl'), x, y);
  c.blit(art.frost(step, 'tr'), x + w - s[0], y);
  c.blit(art.frost(step, 'bl'), x, y + h - s[1]);
  c.blit(art.frost(step, 'br'), x + w - s[0], y + h - s[1]);
}

/** The door of a room: amber, and it blinks, while a request waits at it. */
export function doorOf(ctx: Ctx, r: Room): string {
  if (r.asks.length === 0) return 'door-closed';
  if (art.loops('door-asking')) ctx.loops = true;
  return 'door-asking';
}

/** The light of a room whose run ended is off: its things are dark. */
export const NIGHT_MAP = (p: Rgb): Rgb => mix(p, px.NIGHT, 0.5);

/** A picture with its lowest row at a row of pixels: it stands on the ground. */
export function stand(c: Canvas, b: Bitmap, x: number, ground: number): void {
  c.blit(b, x, ground - b.h + 1);
}

/** A frame of quarter blocks, for the world without art. */
export function textFrame(c: Canvas, x: number, y: number, w: number, h: number, style: TextStyle = {}): void {
  if (w < 2 || h < 2) return;
  c.text(x, y, '▛' + '▀'.repeat(w - 2) + '▜', style);
  for (let i = 1; i < h - 1; i++) {
    c.text(x, y + i, '▌', style);
    c.text(x + w - 1, y + i, '▐', style);
  }
  c.text(x, y + h - 1, '▙' + '▄'.repeat(w - 2) + '▟', style);
}

/** A light frame, for glass walls without art. */
export function glassFrame(c: Canvas, x: number, y: number, w: number, h: number, style: TextStyle = {}): void {
  if (w < 2 || h < 2) return;
  c.text(x, y, '┌' + '┄'.repeat(w - 2) + '┐', style);
  for (let i = 1; i < h - 1; i++) {
    c.text(x, y + i, '┆', style);
    c.text(x + w - 1, y + i, '┆', style);
  }
  c.text(x, y + h - 1, '└' + '┄'.repeat(w - 2) + '┘', style);
}

export function hostTools(r: Room): Tool[] {
  return r.tools.filter((t) => t.run === 'host');
}

export function winKey(n: NetMode): art.WindowKey {
  return n === 'ask' ? 'allowlist' : n;
}

/** The rooms of a house that the ranch shows: as many as fit, the ones that need you first. */
export function pickRooms(h: { rooms: Room[] }, fitN: number): { shown: Room[]; hidden: number } {
  const live = h.rooms.filter((r) => r.run.live);
  const ended = h.rooms.filter((r) => !r.run.live).slice(0, 2);
  const all = [...live, ...ended];
  if (all.length <= fitN) return { shown: all, hidden: h.rooms.length - all.length };
  const keep = new Set(
    [...all]
      .sort((a, b) => Number(b.asks.length > 0) - Number(a.asks.length > 0) || Number(b.run.live) - Number(a.run.live) || STATE[a.run.state].order - STATE[b.run.state].order)
      .slice(0, fitN),
  );
  const shown = all.filter((r) => keep.has(r));
  return { shown, hidden: h.rooms.length - shown.length };
}

/** The sign of a house: its name, under it. */
export const SIGN: TextStyle = { fg: px.INK, bg: px.CREAM, bold: true };
/** Words of the thing that is selected. */
export const PICKED: TextStyle = { fg: px.INK, bg: px.SELECT, bold: true };

/**
 * The side wall of a building, to the right of its front: the house is
 * seen from the front and a little from above. `eave` is the first row of
 * pixels of the front, `d` how far the wall goes back.
 */
export function sideWall(c: Canvas, x: number, eave: number, h: number, d: number, night = false): void {
  const shade: Rgb = night ? NIGHT_MAP(px.CREAM_SHADE) : px.CREAM_SHADE;
  for (let i = 0; i < d; i++) {
    c.fill(x + i, eave - 1 - i, 1, h, i === d - 1 ? px.INK : shade);
    c.pixel(x + i, eave - 2 - i + h, px.INK);
  }
}

/** The roof over a front of `fw` pixels: `rows` high, and it goes back by `d` as the side wall does. */
export function roof(c: Canvas, X: number, eave: number, fw: number, rows: number, d: number): void {
  const tile = art.pic('tile-roof');
  for (let j = 0; j < rows; j++) {
    // The edges go back as the side wall does, then straight up.
    const shift = Math.min(j, d);
    const x0 = X - 1 + shift;
    const y = eave - 1 - j;
    for (let i = 0; i < fw + 2; i++) {
      const edge = i === 0 || i === fw + 1 || (j === rows - 1 && rows >= 4);
      c.pixel(x0 + i, y, edge ? px.INK : j === 0 ? px.ROOF_DARK : (tile.px[((rows - 1 - j) % tile.h) * tile.w + (i % tile.w)] ?? px.ROOF));
    }
  }
}
