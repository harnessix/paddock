// The ranch with art: one green island that fills the picture. A project
// is a house with a roof, a run is a room of it, a helper is a hut at the
// end of a path, a shared yard is a fence in front of its rooms. The
// build bench and the gate stand at the front edge, and another computer
// is an island in a bay of the shore.
//
// The picture is laid out for the screen it gets. On a wide screen every
// pixel of the art is twice as large, so the ranch fills 200 columns as
// it fills 100. On a low screen the roofs are low and the yards are a
// rail under the rooms. A ranch that still does not fit scrolls by rows.
//
// Words never lie on the art. The name of a house is on a sign under it,
// the name of a run is on the grass under its room, and what does not fit
// ends with an ellipsis. The tickets of a project are in the Inspector.

import { cells, cut } from '../clean.js';
import { t } from '../i18n/index.js';
import { STATE } from '../model.js';
import type { Pixel, Rgb } from '../sprites.js';
import { bitmap, mix, type Bitmap, type TextStyle } from './canvas.js';
import { call, character, doorOf, flagUp, frost, frostCorners, hit, hop, hostTools, NIGHT_MAP, PICKED, pickRooms, roof, roomShell, scrollNote, selected, sideWall, SIGN, stand, tagFlip, windowOf, type Ctx } from './kit.js';
import * as art from './layers.js';
import { ease, FAN_LOOP, lerp } from './motion.js';
import type { House, Island, Room, World } from './model.js';
import * as px from './pixels.js';

/** A room on the ranch, in pixels of the art. */
const ROOM = 12;
/** How far the side wall of a house goes back. */
const DEPTH = 3;
/** The path between a helper and the run that started it. */
const PATH = 5;
/** The grass at the left and the right edge. */
const EDGE = 3;
/** The bay of another computer. */
const BAY_W = 20;
/** The widest screen that shows the art at its own size. From here on it can be twice as large. */
const DOUBLE_W = 160;

/** The heights of a roof and of a yard, from the roomiest to the most compact. */
const DETAILS: readonly (readonly [number, number])[] = [
  [6, 8],
  [6, 6],
  [4, 6],
  [4, 4],
  [2, 4],
  [2, 2],
  [2, 0],
];

interface Seg {
  /** Where the building begins in its lot. */
  x: number;
  rooms: Room[];
  /** A helper's hut: a path leads to the building before it. */
  path: boolean;
}

interface Lot {
  w: number;
  ids: string[];
  house: House;
  segs: Seg[];
  hidden: number;
}

type ItemKind = 'bench' | 'gate' | 'stable';

interface Item {
  kind: ItemKind;
  /** The width of its picture and of the whole item, in pixels of the art. */
  artW: number;
  w: number;
  lines: { text: string; bold?: boolean; dim?: boolean }[];
}

interface Unit {
  /** A row of houses, or the front edge with the bench and the gate. */
  strip: boolean;
  lots: { lot: Lot; x: number }[];
  island: { island: Island; x: number } | null;
  items: { item: Item; x: number }[];
  ids: string[];
  /** Its first row of pixels, and its height. */
  y: number;
  h: number;
}

export interface RanchPlan {
  /** The size of a pixel of the art. */
  scale: number;
  /** The width of the picture in pixels of the art. */
  lw: number;
  roof: number;
  yard: number;
  /** How far the side wall of a house goes back. */
  depth: number;
  /** The rows of the ranch that show. */
  units: Unit[];
  above: number;
  below: number;
  /** The first row of pixels of the island, and the one after its last. */
  top: number;
  bottom: number;
}

function houseLot(h: House, fitN: number, depth: number): Lot {
  const { shown, hidden } = pickRooms(h, fitN);
  const segs: Seg[] = [];
  shown.forEach((r, i) => {
    const prev = shown[i - 1];
    const helper = prev !== undefined && r.parent !== '' && (prev.id === r.parent || prev.parent === r.parent);
    const last = segs[segs.length - 1];
    if (helper || !last || last.path) segs.push({ x: 0, rooms: [r], path: helper });
    else last.rooms.push(r);
  });
  // A project without a run: one closed house.
  if (segs.length === 0) segs.push({ x: 0, rooms: [], path: false });
  let x = 0;
  segs.forEach((s, i) => {
    if (i > 0) x += s.path ? PATH : 2;
    s.x = x;
    x += ROOM * Math.max(1, s.rooms.length) + depth;
  });
  return {
    w: x,
    house: h,
    segs,
    hidden,
    ids: [...shown.map((r) => `room:${r.id}`), ...(h.loft ? [`loft:${h.key}`] : []), ...h.yards.map((y) => `yard:${h.key}:${y.name}`), `sign:${h.key}`],
  };
}

function benchItem(world: World, s: number): Item {
  const seats = world.bench.seats;
  const artW = 17 + 5 * Math.max(1, Math.min(3, seats.length));
  // Who sits at the bench: two holders by name, more as a count.
  const who: Item['lines'] =
    seats.length === 0
      ? [{ text: t('world.bench.none'), dim: true }]
      : seats.length <= 2
        ? seats.map((seat) => ({ text: `${seat.slot} ${seat.holder}` }))
        : [{ text: `${seats[0]?.slot ?? 1} ${seats[0]?.holder ?? ''}` }, { text: t('world.more', { n: seats.length - 1 }), dim: true }];
  return item('bench', artW, [{ text: t('world.bench.title'), bold: true }, ...who], s);
}

function gateItem(world: World, s: number): Item {
  const g = world.gate;
  const lines: Item['lines'] = [{ text: t('world.gate.title'), bold: true }];
  // The account that is used most: the feed trough at a glance. The rest is in the Inspector.
  const worst = [...(g.forage ?? [])].sort((a, b) => Number(b.overgrazing ?? false) - Number(a.overgrazing ?? false) || (b.used ?? 0) - (a.used ?? 0))[0];
  if (worst) lines.push({ text: `${worst.agent ?? '?'} ${worst.label ?? ''} ${Math.round(worst.used ?? 0)}%`, bold: worst.overgrazing === true, dim: worst.overgrazing !== true });
  else if (g.keys !== null) lines.push({ text: t('world.gate.keys', { n: g.keys }), dim: true });
  return item('gate', 24, lines, s);
}

function stableItem(world: World, s: number): Item {
  const stalls = world.stable?.stalls ?? [];
  const n = stalls.reduce((a, b) => a + b.sessions.length, 0);
  return item('stable', 13 + 6 * Math.min(3, n), [{ text: t('world.stable.title'), bold: true }, { text: t('world.stable.count', { n: stalls.length, saved: n }), dim: true }], s);
}

function item(kind: ItemKind, artW: number, lines: Item['lines'], s: number): Item {
  const words = Math.max(...lines.map((l) => cells(l.text)));
  return { kind, artW, lines, w: artW + 2 + Math.ceil(words / s) };
}

/**
 * The ranch at a size of its pixels, or null: it does not fit so. `tight`:
 * low roofs, thin side walls and less grass between the houses. `scroll`:
 * a ranch that is too high shows the rows that fit.
 */
function layout(world: World, w: number, h: number, s: number, sel: string, tight: boolean, scroll: boolean): RanchPlan | null {
  const lw = Math.floor(w / s);
  const depth = tight ? 1 : DEPTH;
  const GAP = tight ? 2 : 4;
  // The rows of pixels between the top line and the line of words at the bottom.
  const top = Math.ceil(2 / s);
  const bottom = Math.floor(((h - 1) * 2) / s);
  const P = bottom - top;
  const avail = lw - 2 * EDGE;
  const fitN = Math.max(1, Math.floor((avail - depth) / ROOM));
  const lots = world.houses.map((house) => {
    let n = fitN;
    let lot = houseLot(house, n, depth);
    while (lot.w > avail && n > 1) lot = houseLot(house, --n, depth);
    return lot;
  });
  if (lots.some((l) => l.w > avail)) return null;

  // The houses flow into rows.
  const units: Unit[] = [];
  const unit = (strip: boolean): Unit => ({ strip, lots: [], island: null, items: [], ids: [], y: 0, h: 0 });
  const used = (u: Unit): number => u.lots.reduce((a, e) => a + e.lot.w, 0) + GAP * Math.max(0, u.lots.length - 1);
  let cur = unit(false);
  for (const lot of lots) {
    if (cur.lots.length > 0 && used(cur) + GAP + lot.w > avail) {
      units.push(cur);
      cur = unit(false);
    }
    cur.lots.push({ lot, x: 0 });
    cur.ids.push(...lot.ids);
  }
  if (cur.lots.length > 0) units.push(cur);
  // Another computer: a bay at the right end of the first row that has room for it.
  for (const island of world.islands) {
    let u = units.find((x) => !x.island && used(x) + GAP <= avail - BAY_W);
    if (!u) {
      u = unit(false);
      units.push(u);
    }
    u.island = { island, x: lw - BAY_W + 1 };
    u.ids.push(`island:${island.name}`);
  }
  // Each row: its houses spread over its width.
  for (const u of units) {
    const room = (u.island ? lw - BAY_W - 2 : lw) - 2 * EDGE;
    const total = u.lots.reduce((a, e) => a + e.lot.w, 0);
    const n = u.lots.length;
    const gap = n > 1 ? Math.min(14, Math.floor((room - total) / (n - 1))) : 0;
    let x = EDGE + Math.max(0, Math.floor((room - total - gap * (n - 1)) / 2));
    for (const e of u.lots) {
      e.x = x;
      x += e.lot.w + gap;
    }
  }
  // The front edge: the bench, the gate, the stable.
  let items = [benchItem(world, s)];
  if (world.gate.keys !== null || world.gate.forage !== null) items.push(gateItem(world, s));
  if (world.stable) items.push(stableItem(world, s));
  const wide = (list: Item[]): number => list.reduce((a, it) => a + it.w, 0) + GAP * (list.length - 1);
  // On a narrow screen each thing has its name only: the rest is in the line at the bottom and in the Inspector.
  if (wide(items) > avail) items = items.map((it) => item(it.kind, it.artW, it.lines.slice(0, 1), s));
  const strips: Unit[] = [];
  let strip = unit(true);
  let sx = 0;
  for (const it of items) {
    if (strip.items.length > 0 && sx + GAP + it.w > avail) {
      strips.push(strip);
      strip = unit(true);
      sx = 0;
    }
    if (strip.items.length > 0) sx += GAP;
    strip.items.push({ item: it, x: sx });
    strip.ids.push(it.kind);
    sx += it.w;
  }
  strips.push(strip);
  for (const u of strips) {
    const total = u.items.reduce((a, e) => a + e.item.w, 0);
    const n = u.items.length;
    const gap = Math.min(24, Math.floor((avail - total) / (n + 1)));
    let x = EDGE + Math.max(0, Math.floor((avail - total - gap * (n - 1)) / 2));
    for (const e of u.items) {
      e.x = x;
      x += e.item.w + gap;
    }
    // A stall is higher than a bench.
    u.h = u.items.some((e) => e.item.kind === 'stable') ? 8 : 6;
  }
  units.push(...strips);

  // The words under a house: the names of its runs, and its sign.
  const words = Math.ceil(4 / s);
  // Between two rows: one row of cells, at least.
  const between = Math.max(2, Math.ceil(2 / s));
  const need = (roof: number, yard: number, list: Unit[]): number => list.reduce((a, u) => a + (u.strip ? u.h : roof + ROOM + yard + words), 0) + between * (list.length - 1);
  const details = tight ? DETAILS.filter(([roof]) => roof <= 2) : DETAILS;
  let detail = details.find(([roof, yard]) => need(roof, yard, units) <= P);
  let shown = units;
  let above = 0;
  if (!detail) {
    if (!scroll) return null;
    // The ranch scrolls: the rows that fit, with the row of the selection among them.
    detail = details[details.length - 1] as readonly [number, number];
    const [roof, yard] = detail;
    const at = Math.max(0, units.findIndex((u) => u.ids.includes(sel)));
    let first = at;
    let last = at;
    // The rows before the selection stay while they fit; then the rows after it.
    while (first > 0 && need(roof, yard, units.slice(first - 1, last + 1)) <= P) first--;
    while (last < units.length - 1 && need(roof, yard, units.slice(first, last + 2)) <= P) last++;
    shown = units.slice(first, last + 1);
    above = first;
  }
  const [roof, yard] = detail;
  for (const u of shown) if (!u.strip) u.h = roof + ROOM + yard + words;
  // What is left over goes between the rows and around them, in whole rows of cells.
  const row = Math.max(1, Math.ceil(2 / s));
  const spare = P - need(roof, yard, shown);
  const extra = Math.min(8, Math.floor(spare / (shown.length + 1) / row) * row);
  const total = need(roof, yard, shown) + extra * (shown.length - 1);
  let y = top + Math.floor((P - total) / 2 / row) * row;
  for (const u of shown) {
    u.y = y;
    y += u.h + between + extra;
  }
  return { scale: s, lw, roof, yard, depth, units: shown, above, below: units.length - above - shown.length, top, bottom };
}

/** The ranch for a screen: twice as large when all of it fits so, at its own size when not. */
export function planRanch(world: World, w: number, h: number, sel: string): RanchPlan {
  const big = w >= DOUBLE_W ? (layout(world, w, h, 2, sel, false, false) ?? layout(world, w, h, 2, sel, true, false)) : null;
  return big ?? layout(world, w, h, 1, sel, false, false) ?? (layout(world, w, h, 1, sel, true, true) as RanchPlan);
}

// ---------------------------------------------------------------- words

const ON_GRASS: TextStyle = { fg: px.INK, bg: px.GRASS };
const ON_WATER: TextStyle = { fg: px.INK, bg: px.WATER };
/** A run that waits for you: its name on amber. */
const ASKS: TextStyle = { fg: px.INK, bg: px.AMBER, bold: true };

/** The name of a run under its room, and the marks of what needs you. */
function caption(r: Room, width: number): { name: string; marks: string } {
  const marks = (r.asks.length > 0 ? ' ◆' : '') + (r.unread > 0 ? ` ✉${r.unread}` : '');
  const state = r.run.live ? (r.run.frozen ? '❄ ' : '') : `${STATE[r.run.state].glyph} `;
  const room = Math.max(1, width - cells(marks));
  const long = state + r.name;
  return { name: cells(long) <= room ? long : cut(state + r.run.harness, room), marks };
}

// ------------------------------------------------------------------ art

function roomArt(ctx: Ctx, r: Room, X: number, Y: number, sel: boolean): void {
  const { c } = ctx;
  const f = frost(ctx, r);
  const built = r.parent ? ease(ctx.o.motion.progress('spawn', r.id, ctx.o.tick)) : 1;
  // A helper's room builds from the ground up.
  const tall = Math.max(art.SLICE['12x12'].bottom + 2, Math.round(ROOM - (ROOM - 2) * (1 - built)));
  c.blit(roomShell(ctx, '12x12', r, ROOM, tall, f, sel), X, Y + ROOM - tall);
  if (built < 1) return;
  if (r.run.live) c.blit(windowOf(ctx, r, true), X + 8, Y + 1);
  // The door: amber while a request waits at it.
  c.blit(art.part(doorOf(ctx, r), ctx.o.tick, 1, 0, 1, 5), X, Y + 6, r.run.live ? undefined : NIGHT_MAP);
  c.blit(character(ctx, r).small, X + 1, Y + 3);
  // Unread messages: a flag on its post.
  if (r.unread > 0) {
    // The flag goes up its post, and the post hops once for a question.
    const up = Y + lerp(8, 5, flagUp(ctx, r)) - hop(ctx, r);
    c.fill(X + 10, Y + 6 - hop(ctx, r), 1, 5, px.WOOD);
    c.fill(X + 9, up, 2, 1, px.RED);
    c.pixel(X + 9, up + 1, px.RED);
  }
  frostCorners(c, X + 1, Y + 1, ROOM - 2, ROOM - 2, Math.round(f * 2));
}

/** A closed house: the light is off, the shutters and the door are shut. */
function closedArt(ctx: Ctx, X: number, Y: number): void {
  const { c } = ctx;
  c.blit(art.shell('12x12', 'night', ROOM, ROOM), X, Y);
  c.blit(art.pic('window-small-none'), X + 8, Y + 1, NIGHT_MAP);
  c.blit(art.part('door-closed', 0, 1, 0, 1, 5), X, Y + 6, NIGHT_MAP);
}

const grounds = new Map<string, Bitmap>();

function ground(key: string, w: number, h: number, at: (x: number, y: number, n: number) => Pixel): Bitmap {
  const k = `${key}:${w}x${h}`;
  let b = grounds.get(k);
  if (!b) {
    if (grounds.size > 64) grounds.clear();
    const out: Pixel[] = [];
    // A number for each place, always the same: the grass does not move.
    for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out.push(at(x, y, (Math.imul(x + 11, 73856093) ^ Math.imul(y + 7, 19349663)) >>> 0));
    b = { w, h, px: out };
    grounds.set(k, b);
  }
  return b;
}

const GRASS_LIGHT: Rgb = [191, 238, 183];
const PETAL: Rgb = [255, 181, 174];

/** The grass of the island: calm, with a tuft or a flower here and there. */
function grass(w: number, h: number): Bitmap {
  return ground('grass', w, h, (_x, _y, n) => {
    const k = n % 211;
    return k < 5 ? GRASS_LIGHT : k < 8 ? px.GRASS_DARK : k === 8 ? px.WHITE : k === 9 ? PETAL : px.GRASS;
  });
}

/** The water of a bay: a few waves, which move while a job runs on the island. */
function water(w: number, h: number, phase: number): Bitmap {
  return ground(`water${phase}`, w, h, (x, y) => {
    const n = (Math.imul((x + phase) >> 1, 73856093) ^ Math.imul(y + 3, 19349663)) >>> 0;
    return n % 23 === 0 ? px.WATER_LIGHT : px.WATER;
  });
}

function slab(ctx: Ctx, plan: RanchPlan): void {
  const { c } = ctx;
  const { lw, top, bottom } = plan;
  c.setKind('ground');
  c.blit(grass(lw - 2, bottom - top), 1, top);
  if (bottom - top > 8) c.fill(1, bottom - 1, lw - 2, 1, px.GRASS_DARK);
  // The bay of another computer: water from the shore of the island to the edge of the picture.
  plan.units.forEach((u, i) => {
    if (!u.island) return;
    const x = u.island.x - 2;
    const next = plan.units[i + 1];
    const y0 = i === 0 ? top : u.y - 1;
    const y1 = next ? next.y - 1 : bottom;
    const busy = islandBusy(ctx.world, u.island.island.name);
    c.blit(water(lw - x, y1 - y0, busy && !ctx.o.motion.reduce ? (ctx.o.tick >> 2) & 1 : 0), x, y0);
    // The shore.
    c.fill(x - 1, y0, 1, y1 - y0, px.SAND);
    if (next) c.fill(x - 1, y1, lw - x, 1, px.SAND);
  });
  // The corners of the island are round.
  for (const [x, w] of [
    [1, 2],
    [lw - 3, 2],
  ] as const) {
    c.fill(x, top, w, 1, null);
    c.fill(x, bottom - 1, w, 1, null);
  }
  for (const x of [1, lw - 2]) {
    c.fill(x, top + 1, 1, 1, null);
    c.fill(x, bottom - 2, 1, 1, null);
  }
  c.setKind('art');
}

/**
 * The fan at a frame of the clock. Its blade has two ends, so its
 * pictures are half a turn: one turn of 2 s shows them twice.
 */
export function fanAt(tick: number): Bitmap {
  const frames = art.layer('fan').frames;
  const stay = Math.max(1, Math.round(FAN_LOOP / (2 * frames.length)));
  return frames[Math.floor((Math.max(0, tick) % FAN_LOOP) / stay) % frames.length] as Bitmap;
}

export function islandBusy(world: World, name: string): boolean {
  // A job runs there: a tool of this island has an instance in a room.
  return world.houses.some((h) => h.rooms.some((r) => r.tools.some((t) => t.run === 'remote' && t.peer === name && t.on)));
}

/** The first cell of a pixel of the art, and the row of cells of a row of pixels. */
function cell(plan: RanchPlan, x: number): number {
  return x * plan.scale;
}

function rowOf(plan: RanchPlan, y: number): number {
  return Math.floor((y * plan.scale) / 2);
}

function drawLot(ctx: Ctx, plan: RanchPlan, lot: Lot, X: number, T: number): void {
  const { c } = ctx;
  const s = plan.scale;
  const h = lot.house;
  const eave = T + plan.roof;
  const base = eave + ROOM;
  // The words stand right under the house; its yard lies in front of them.
  const capRow = rowOf(plan, base);
  const nameRow = capRow + 1;
  const front = base + Math.ceil(4 / s);
  const d = plan.roof <= 2 ? 1 : plan.depth;
  // Where each room stands, and the words under it.
  const at = new Map<string, number>();
  const caps = new Map<string, { name: string; marks: string }>();
  for (const seg of lot.segs) seg.rooms.forEach((r, i) => {
    at.set(r.id, X + seg.x + i * ROOM);
    caps.set(r.id, caption(r, ROOM * s - 1));
  });
  const capEnd = (r: Room): number => {
    const cp = caps.get(r.id);
    return (at.get(r.id) as number) + Math.ceil((cells(cp?.name ?? '') + cells(cp?.marks ?? '') + 1) / s);
  };

  lot.segs.forEach((seg, k) => {
    const sx = X + seg.x;
    const fw = ROOM * Math.max(1, seg.rooms.length);
    const night = seg.rooms.every((r) => !r.run.live);
    if (seg.rooms.length === 0) closedArt(ctx, sx, eave);
    seg.rooms.forEach((r, i) => roomArt(ctx, r, sx + i * ROOM, eave, selected(ctx, `room:${r.id}`)));
    const helper = seg.rooms[0];
    const built = seg.path && helper ? ctx.o.motion.progress('spawn', helper.id, ctx.o.tick) : 1;
    if (built >= 1) {
      sideWall(c, sx + fw, eave, ROOM, d, night);
      roof(c, sx, eave, fw, plan.roof, d);
      // The loft: a window in the roof.
      if (h.loft !== '' && k === 0 && plan.roof >= 4) {
        c.fill(sx + fw - 5, eave - 4, 4, 3, px.INK);
        c.fill(sx + fw - 4, eave - 3, 2, 2, night ? px.NIGHT : px.SKY);
      }
    }
    // A tool on your machine: an amber cable through the roof. It always shows.
    seg.rooms.forEach((r, i) => {
      if (built < 1 || hostTools(r).length === 0) return;
      const calls = hostTools(r).map((t) => call(ctx, r, t.name));
      c.fill(sx + i * ROOM + 5, T, 1, plan.roof + 1, calls.some((k) => k.bright) ? mix(px.AMBER, px.WHITE, 0.6) : px.AMBER);
      // A call of the tool: a spark runs up the cable, out of the room.
      for (const k of calls) if (k.spark !== null) c.pixel(sx + i * ROOM + 5, lerp(T + plan.roof, T, k.spark), px.WHITE);
    });
    // Reduce Motion: a helper's room and its path fade in.
    const shown = seg.path && helper ? ctx.o.motion.fade('spawn', helper.id, ctx.o.tick) : 1;
    // The path from a helper to the run that started it, from door to door.
    const prev = lot.segs[k - 1];
    if (seg.path && prev) {
      const from = X + prev.x + ROOM * prev.rooms.length + d;
      c.blit(art.tiles('tile-path', sx - from, 2), from, base - 2);
      if (built < 1) c.pixel(lerp(from, sx - 1, built), base - 2, px.WHITE);
    }
    // The envelope of a helper that ended: it travels the path to the run that started it.
    for (const r of seg.rooms) {
      const rx = at.get(r.id) as number;
      for (const e of ctx.o.motion.ended(r.id, ctx.o.tick)) c.blit(art.pic('envelope'), lerp(rx + ROOM + PATH + 6, rx + ROOM, e), eave + 6);
    }
    if (shown < 1 && prev) {
      const from = X + prev.x + ROOM * prev.rooms.length + d;
      c.recolor(from, T, sx + fw + d - from, plan.roof + ROOM, (p) => mix(px.GRASS, p, shown));
    }
  });

  // The yards: a fence in front of the rooms that share one.
  const rooms = lot.segs.flatMap((seg) => seg.rooms);
  for (const yd of h.yards) {
    const members = rooms.filter((r) => yd.members.includes(r.id));
    const first = members[0];
    const last = members[members.length - 1];
    if (!first || !last) continue;
    const x0 = at.get(first.id) as number;
    const full = (at.get(last.id) as number) + ROOM;
    // A room that joined: the fence extends to it.
    const joined = Math.min(...yd.members.map((m) => ctx.o.motion.progress('join', `${yd.name}:${m}`, ctx.o.tick)));
    const x1 = lerp(x0 + ROOM, full, ease(joined));
    const id = `yard:${h.key}:${yd.name}`;
    const sel = selected(ctx, id);
    const glow = sel ? 'glow' : undefined;
    const yh = plan.yard;
    if (yh === 0) {
      // No room for a yard: a rail under the rooms, beside the names of their runs.
      const y = base;
      const rail = Math.max(1, Math.ceil(2 / s));
      let hx = x1;
      for (const r of members) {
        const from = Math.min(capEnd(r), (at.get(r.id) as number) + ROOM);
        const to = Math.min(x1, (at.get(r.id) as number) + ROOM);
        if (to > from) c.blit(art.tiles('tile-fence', to - from, rail, 0, 1, glow), from, y);
        hx = from;
      }
      hit(ctx, { id, kind: 'yard', x: cell(plan, hx), y: capRow, w: Math.max(1, cell(plan, x1 - hx)), h: 1, house: h.key, name: yd.name });
    } else {
      const fh = Math.min(yh, 5);
      const fy = front + (yh >= 8 ? 3 : yh > fh ? 1 : 0);
      // The crate of the shared folder stands in the yard, behind the fence.
      if (yh >= 8) c.blit(art.pic('crate-none'), x1 - 9, fy + fh - 7);
      c.blit(art.tiles('tile-fence', x1 - x0, fh, 0, 5 - fh, glow), x0, fy);
      // Reduce Motion: the new piece of the fence fades in.
      const faded = Math.min(...yd.members.map((m) => ctx.o.motion.fade('join', `${yd.name}:${m}`, ctx.o.tick)));
      if (faded < 1) c.recolor(x0 + ROOM, fy, x1 - x0 - ROOM, fh, (p) => mix(px.GRASS, p, faded));
      // A note of the yard was written: its tag on the fence flips.
      tagFlip(ctx, id, x0 + 2, fy + 1, px.WOOD);
      // A small gate for each shared port.
      yd.ports.slice(0, 3).forEach((_, i) => {
        const gx = x0 + 3 + i * 7;
        if (gx + 6 < x1 && fh >= 4) c.blit(art.part('tile-gate', 0, 0, 5 - fh, 6, fh), gx, fy);
      });
      hit(ctx, { id, kind: 'yard', x: cell(plan, x0), y: rowOf(plan, fy), w: cell(plan, x1 - x0), h: rowOf(plan, fy + fh - 1) - rowOf(plan, fy) + 1, house: h.key, name: yd.name });
    }
  }

  // The words: the name of each run under its room.
  for (const seg of lot.segs) {
    for (const r of seg.rooms) {
      const id = `room:${r.id}`;
      const sel = selected(ctx, id);
      const x = cell(plan, at.get(r.id) as number);
      hit(ctx, { id, kind: 'room', x, y: rowOf(plan, eave), w: ROOM * s, h: capRow - rowOf(plan, eave) + 1, house: h.key, room: r.id });
      const cp = caps.get(r.id) as { name: string; marks: string };
      const asks = r.asks.length > 0;
      const n = c.text(x, capRow, cp.name, sel ? PICKED : asks ? ASKS : { ...ON_GRASS, dim: !r.run.live });
      if (asks) c.text(x + n, capRow, ' ◆', sel ? PICKED : ASKS);
      if (r.unread > 0) c.text(x + n + (asks ? 2 : 0), capRow, ` ✉${r.unread}`, ON_GRASS);
    }
  }
  // The sign of the house: its name. The ladder to its loft is beside it.
  const W = lot.w * s;
  const x = cell(plan, X);
  const loft = h.loft ? (W >= 24 ? t('world.ranch.loft') : '▲') : '';
  const more = lot.hidden > 0 ? ` ${t('world.more', { n: lot.hidden })}` : '';
  const signId = `sign:${h.key}`;
  const plate = ` ${cut(h.name, Math.max(1, W - 2 - (loft ? cells(loft) + 1 : 0) - cells(more)))} `;
  let nx = x + c.text(x, nameRow, plate, selected(ctx, signId) ? PICKED : SIGN);
  hit(ctx, { id: signId, kind: 'sign', x, y: nameRow, w: cells(plate), h: 1, house: h.key });
  if (more) nx += c.text(nx, nameRow, more, { ...ON_GRASS, dim: true });
  const lx = x + W - cells(loft);
  // The names of the yards show with Plumbing, and for a yard that is selected.
  for (const yd of h.yards) {
    const sel = selected(ctx, `yard:${h.key}:${yd.name}`);
    const word = ` ${t('world.yard', { name: yd.name })}`;
    if ((ctx.o.view.plumbing || sel) && nx + cells(word) < lx) nx += c.text(nx, nameRow, word, sel ? PICKED : ON_GRASS);
  }
  if (loft) {
    const id = `loft:${h.key}`;
    hit(ctx, { id, kind: 'loft', x: lx, y: nameRow, w: cells(loft), h: 1, house: h.key });
    c.text(lx, nameRow, loft, selected(ctx, id) ? PICKED : ON_GRASS);
  }
}

function drawIsland(ctx: Ctx, plan: RanchPlan, island: Island, X: number, T: number): void {
  const { c } = ctx;
  const s = plan.scale;
  const base = T + plan.roof + ROOM;
  const capRow = rowOf(plan, base);
  const id = `island:${island.name}`;
  const busy = islandBusy(ctx.world, island.name);
  // The fan turns while a job runs: once in 2 s. Reduce Motion: a lit window in its place.
  const still = ctx.o.motion.reduce;
  if (busy && !still) ctx.loops = true;
  const ix = X + 1;
  stand(c, art.pic('island'), ix, base - 1);
  const fx = ix + art.FAN_AT[0];
  const fy = base - art.layer('island').h + art.FAN_AT[1];
  if (busy && still) c.fill(fx, fy, 3, 3, px.PAPER);
  else if (busy) c.blit(fanAt(ctx.o.tick), fx, fy);
  const x = cell(plan, X);
  const W = (plan.lw - X - 1) * s;
  hit(ctx, { id, kind: 'island', x, y: rowOf(plan, base - 8), w: W, h: capRow + 2 - rowOf(plan, base - 8), name: island.name });
  c.text(x, capRow, cut(island.name, W), selected(ctx, id) ? PICKED : { ...ON_WATER, bold: true });
  c.text(x, capRow + 1, cut(busy ? t('world.island.busy') : t('world.island.title'), W), { ...ON_WATER, dim: !busy });
}

function drawItem(ctx: Ctx, plan: RanchPlan, it: Item, X: number, T: number, h: number): void {
  const { c } = ctx;
  const ground = T + h - 1;
  const sel = selected(ctx, it.kind);
  if (it.kind === 'bench') {
    const seats = ctx.world.bench.seats;
    stand(c, art.pic('bench'), X, ground);
    // A free stool while no one builds. The stools of the holders stand beside the bench.
    if (seats.length === 0) stand(c, art.pic('stool-free'), X + 17, ground);
    seats.slice(0, 3).forEach((seat, i) => {
      const taken = ctx.o.motion.progress('slot', String(seat.slot), ctx.o.tick) >= 1;
      stand(c, art.pic(taken ? 'stool-taken' : 'stool-free'), X + 17 + i * 5, ground);
    });
  } else if (it.kind === 'gate') {
    // The feed trough, and the gate in its piece of fence.
    stand(c, art.pic('trough'), X, ground);
    stand(c, art.tiles('tile-fence', 4, 5), X + 10, ground);
    stand(c, art.pic('tile-gate'), X + 14, ground);
    // A note of yours was written: its tag on the gate flips.
    tagFlip(ctx, 'gate', X + 16, ground - 3, px.WOOD);
    stand(c, art.tiles('tile-fence', 4, 5), X + 20, ground);
  } else {
    const stalls = ctx.world.stable?.stalls ?? [];
    stand(c, art.pic('stall'), X, ground);
    // One saddle per saved session, the newest nearest the door.
    stalls
      .flatMap((st) => st.sessions)
      .slice(0, 3)
      .forEach((_, i) => stand(c, art.pic('saddle'), X + 13 + i * 6, ground));
  }
  // The words stand beside the picture, on the grass.
  const x = cell(plan, X + it.artW + 2);
  const row = rowOf(plan, T + h) - it.lines.length;
  const W = Math.max(0, cell(plan, plan.lw - EDGE) - x);
  it.lines.forEach((l, i) => c.text(x, row + i, cut(l.text, W), i === 0 && sel ? PICKED : { ...ON_GRASS, bold: l.bold, dim: l.dim }));
  hit(ctx, { id: it.kind, kind: it.kind, x: cell(plan, X), y: rowOf(plan, T), w: Math.min(cell(plan, it.w), ctx.o.w - cell(plan, X)), h: rowOf(plan, T + h) - rowOf(plan, T) });
}

const LEAF: Rgb = [104, 178, 112];
const TREE: Bitmap = bitmap(['..kkk..', '.kgGgk.', 'kgGGGGk', 'kGGgGGk', 'kGGGGGk', '.kGGGk.', '..kdk..', '..kdk..'], { k: px.INK, G: LEAF, g: px.GRASS_DARK, d: px.WOOD });
const BUSH: Bitmap = bitmap(['.kkkk.', 'kgGGGk', 'kGGgGk', '.kkkk.'], { k: px.INK, G: LEAF, g: px.GRASS_DARK });

/** A tree or a bush where a row of houses leaves room: between two houses, and at its ends. */
function plants(ctx: Ctx, plan: RanchPlan, u: Unit, n: number): void {
  const ground = u.y + plan.roof + ROOM - 1;
  const ends = [EDGE - 1, ...u.lots.flatMap((e) => [e.x - 1, e.x + e.lot.w]), u.island ? u.island.x - 3 : plan.lw - EDGE];
  for (let i = 0; i + 1 < ends.length; i += 2) {
    const from = ends[i] as number;
    const room = (ends[i + 1] as number) - from;
    // The first and the last place are at the edge of the island: a plant needs more room there.
    const pic = (i / 2 + n) % 2 === 0 ? TREE : BUSH;
    if (room >= pic.w + 4) stand(ctx.c, pic, from + Math.floor((room - pic.w + 1) / 2), ground);
  }
}

export function drawRanchArt(ctx: Ctx, plan: RanchPlan): void {
  const { c, world, o } = ctx;
  slab(ctx, plan);
  plan.units.forEach((u, n) => {
    if (!u.strip) plants(ctx, plan, u, n);
  });
  for (const u of plan.units) {
    for (const e of u.lots) drawLot(ctx, plan, e.lot, e.x, u.y);
    if (u.island) drawIsland(ctx, plan, u.island.island, u.island.x, u.y);
    for (const e of u.items) drawItem(ctx, plan, e.item, e.x, u.y, u.h);
  }
  if (world.houses.length === 0) c.text(cell(plan, EDGE), rowOf(plan, plan.top) + 1, cut(t('world.ranch.empty'), o.w - 2 * cell(plan, EDGE)), ON_GRASS);
  scrollNote(ctx, plan.above > 0, plan.below, 'row');
}
