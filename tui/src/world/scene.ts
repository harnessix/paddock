// The pictures of the world: the ranch, a building, a room and the loft,
// each drawn on a canvas from the world model, the view and a frame of
// the clock. The picture has two forms. With art the things are pixels,
// laid out for the screen: the ranch is an island that fills it
// (ranch.ts), a building and a room stand in its middle (building.ts,
// and the room here), and on a wide screen each pixel is twice as large.
// Without art (NO_COLOR, --plain) the same things are characters and
// words, in a layout of their own. The words of the world are plain
// ("Tools", "Asks you"); the words of paddock are in the Inspector.
//
// The one rule of the design: the wall of a room is the sandbox. What
// leaves the room is a line through the wall. Amber means "this touches
// your real machine" or "this waits for you", and nothing else is amber.
//
// With art, words never lie on a thing of the picture: they stand on a
// sign, on the ground under the thing, or beside it (Canvas.clashes).

import { cells, cut, fit } from '../clean.js';
import { t } from '../i18n/index.js';
import { age, now, STATE, type RunRow } from '../model.js';
import { colorOf, type Rgb } from '../sprites.js';
import { Canvas, mix, type TextStyle } from './canvas.js';
import { AGENT, atDoor, BOLD, call, character, DIM, doorOf, face, flagUp, flakeBadge, frost, frostCorners, glassFrame, hit, hop, hostTools, mailbox, NIGHT_MAP, pickRooms, roof, roomShell, roomWord, scrollNote, selected, tagFlip, textFrame, trip, windowOf, writing, type Ctx, type Drawn, type Hit, type HitKind, type SceneOptions, type View } from './kit.js';
import * as art from './layers.js';
import { ease, lerp } from './motion.js';
import { netWord, waiting, type House, type NetMode, type Room, type World, type Yard } from './model.js';
import { changesLine } from '../copy.js';
import * as px from './pixels.js';
import { drawBuildingArt, planBuilding } from './building.js';
import { drawRanchArt, islandBusy, planRanch, type RanchPlan } from './ranch.js';

export { roomWord, type Drawn, type Hit, type HitKind, type Level, type SceneOptions, type View } from './kit.js';


// The sizes of the layout, in cells.
/** A room on the ranch: 12 x 12 pixels. */
export const RANCH_ROOM_W = 12;
export const RANCH_ROOM_H = 6;
/** A block of the ranch: roof, rooms, captions, ground. */
export const BAND_H = 9;
const SIGN_W = 17;
/** A room in the building: 34 x 28 pixels, the character at its full size. */
export const BUILDING_ROOM_W = 33;
export const BUILDING_ROOM_H = 14;
const HALL_W = 8;
/** The one room of the room view: 40 x 34 pixels. */
export const ROOM_W = 40;
export const ROOM_H = 17;
const ROOM_X = 5;
/** Where the words beside the room begin. */
export const ROOM_PANEL_X = ROOM_X + ROOM_W + 4;
/** The smallest scene that shows the world. A smaller one says so. */
export const WORLD_MIN_W = 60;
export const WORLD_MIN_H = 11;


// ------------------------------------------------- the ranch in words
//
// Without art (NO_COLOR, 16 colours, --plain) the ranch is drawn with
// characters: blocks side by side, a new row when the row is full. With
// art it is the island of ranch.ts.

interface Block {
  w: number;
  draw: (x: number, y: number) => void;
  /** The ids of the things in the block, for the scroll. */
  ids: string[];
}

/** The rooms of a house that the ranch shows: all that fit, the ones that need you first. */
function ranchRooms(h: House, maxW: number): { shown: Room[]; hidden: number } {
  const sign = h.tickets.length > 0 ? SIGN_W : 0;
  const room = maxW - 4 - sign;
  return pickRooms(h, Math.max(1, Math.floor((room + 2) / (RANCH_ROOM_W + 2))));
}

const TICKET: Record<string, string> = { waiting: '◆', blocked: '■', stuck: '▲', working: '●', idle: '○', open: '◇', done: '✓', ended: '✓' };

function drawRanchRoom(ctx: Ctx, h: House, r: Room, x: number, y: number): void {
  const { c } = ctx;
  const id = `room:${r.id}`;
  const sel = selected(ctx, id);
  const w = RANCH_ROOM_W;
  const hh = RANCH_ROOM_H;
  const word = roomWord(r);
  hit(ctx, { id, kind: 'room', x, y, w, h: hh + 1, house: h.key, room: r.id });
  textFrame(c, x, y, w, hh, sel ? BOLD : {});
  if (r.run.live) c.text(x + 1, y + 1, cut(`[${netShort(r.net)}]`, w - 2), r.net === 'host' ? BOLD : {});
  c.text(x + 2, y + 2, face(r) + (r.asks.length > 0 ? '!' : ''), r.asks.length > 0 ? BOLD : {});
  c.text(x + 1, y + 3, '▟█▙');
  if (r.unread > 0) c.text(x + 6, y + 2, cut(`⚑${r.unread}`, 4));
  if (hostTools(r).length > 0) c.text(x + 6, y + 3, '⏚!', BOLD);
  c.text(x + 1, y + 4, cut(word.word, w - 2), word.amber ? BOLD : DIM);
  if (r.asks.length > 0) c.text(x, y + 3, '▐', BOLD);
  // The caption: the agent, and what needs you.
  const marks = (r.asks.length > 0 ? ' ◆' : '') + (r.unread > 0 ? ` ✉${r.unread}` : '');
  const name = cut(r.run.live ? r.run.harness : `${STATE[r.run.state].glyph} ${r.run.harness}`, w - cells(marks));
  c.text(x, y + hh, fit('', w), {});
  c.text(x, y + hh, name, { bold: sel || r.asks.length > 0, inverse: sel, dim: !r.run.live && !sel });
  if (r.asks.length > 0) c.text(x + cells(name), y + hh, ' ◆', BOLD);
  if (r.unread > 0) c.text(x + cells(name) + (r.asks.length > 0 ? 2 : 0), y + hh, ` ✉${r.unread}`, {});
}

/** The network mode in one short word, for a small room without art. */
const netShort = (m: NetMode): string => t(`world.net.short.${m}`);

function houseBlock(ctx: Ctx, h: House, maxW: number): Block {
  const { shown, hidden } = ranchRooms(h, maxW);
  const sign = h.tickets.length > 0 ? SIGN_W : 0;
  // The place of each room: a helper stands a path away from the run that started it.
  const xs: number[] = [];
  let at = 2;
  shown.forEach((_, i) => {
    if (i > 0) at += 2;
    xs.push(at);
    at += RANCH_ROOM_W;
  });
  const roomsW = Math.max(at - 2, cells(h.name) + (h.loft ? cells(t('world.ranch.loft')) + 2 : 0) + 2);
  const more = hidden > 0 ? t('world.more', { n: hidden }) : '';
  const w = Math.min(maxW, 2 + roomsW + (sign > 0 ? 1 + sign : 0) + (more ? cells(more) + 1 : 0) + 2);
  const ids = [...shown.map((r) => `room:${r.id}`), ...(h.loft ? [`loft:${h.key}`] : []), ...h.yards.map((y) => `yard:${h.key}:${y.name}`), ...(sign ? [`sign:${h.key}`] : [])];
  const draw = (x: number, y: number): void => {
    const { c } = ctx;
    const loftId = `loft:${h.key}`;
    const loftLabel = h.loft ? t('world.ranch.loft') : '';
    c.text(x, y, '▄'.repeat(w));
    for (let i = 1; i < BAND_H - 1; i++) {
      c.text(x, y + i, '█');
      c.text(x + w - 1, y + i, '█');
    }
    c.text(x, y + BAND_H - 1, '▀'.repeat(w));
    c.text(x + 2, y, ` ${cut(h.name, w - 6 - cells(loftLabel) - 1)} `, BOLD);
    if (h.loft) {
      const lx = x + w - 2 - cells(loftLabel);
      hit(ctx, { id: loftId, kind: 'loft', x: lx, y, w: cells(loftLabel), h: 1, house: h.key });
      c.text(lx - 1, y, ` ${loftLabel} `, { inverse: selected(ctx, loftId) });
    }
    shown.forEach((r, i) => {
      const rx = x + (xs[i] as number);
      drawRanchRoom(ctx, h, r, rx, y + 1);
      // The path from a helper to the run that started it.
      if (i > 0 && r.parent && shown[i - 1] && (shown[i - 1]?.id === r.parent || shown[i - 1]?.parent === r.parent)) c.text(rx - 2, y + 4, '··');
    });
    // The envelope of a helper that ended: it travels the path to the run that started it.
    for (const r of shown) {
      for (const e of ctx.o.motion.ended(r.id, ctx.o.tick)) {
        const i = shown.indexOf(r);
        c.text(lerp(x + (xs[i] as number) + RANCH_ROOM_W + 6, x + (xs[i] as number) + RANCH_ROOM_W, e), y + 4, '✉');
      }
    }
    let sx = x + 2 + (at - 2) + 1;
    if (more) {
      c.text(sx, y + 3, more, DIM);
      sx += cells(more) + 1;
    }
    if (sign > 0) {
      // The signpost: the tickets of the project, each with its state.
      const id = `sign:${h.key}`;
      const sel = selected(ctx, id);
      const room = Math.max(0, Math.min(sign, x + w - 2 - sx));
      hit(ctx, { id, kind: 'sign', x: sx, y: y + 1, w: room, h: RANCH_ROOM_H, house: h.key });
      const lines = h.tickets.length > RANCH_ROOM_H ? RANCH_ROOM_H - 1 : h.tickets.length;
      h.tickets.slice(0, lines).forEach((t, i) => {
        c.text(sx, y + 1 + i, cut(`${TICKET[t.state] ?? '?'} ${t.state} ${t.id}`, room), { bold: t.state === 'waiting' || (sel && i === 0), inverse: sel && i === 0 });
      });
      if (h.tickets.length > lines) c.text(sx, y + 1 + lines, cut(t('world.sign.more', { n: h.tickets.length - lines }), room), DIM);
    }
    // The yards: a fence under the rooms that share one.
    for (const yd of h.yards) {
      const members = shown.map((r, i) => (yd.members.includes(r.id) ? i : -1)).filter((i) => i >= 0);
      if (members.length === 0) continue;
      const x0 = x + (xs[members[0] as number] as number);
      const full = x + (xs[members[members.length - 1] as number] as number) + RANCH_ROOM_W;
      // A room that joined: the fence extends to it.
      const joined = Math.min(...yd.members.map((m) => ctx.o.motion.progress('join', `${yd.name}:${m}`, ctx.o.tick)));
      const x1 = lerp(x0 + RANCH_ROOM_W, full, ease(joined));
      const id = `yard:${h.key}:${yd.name}`;
      const sel = selected(ctx, id);
      const fy = y + BAND_H - 1;
      hit(ctx, { id, kind: 'yard', x: x0, y: fy, w: x1 - x0, h: 1, house: h.key, name: yd.name });
      c.text(x0, fy, '┄'.repeat(Math.max(0, x1 - x0)), sel ? BOLD : {});
      c.text(x0 + 2, fy, cut(t('world.yard', { name: yd.name }), Math.max(0, x1 - x0 - 2)), { inverse: sel, dim: !sel });
    }
  };
  return { w, draw, ids };
}

function benchBlock(ctx: Ctx): Block {
  const seats = ctx.world.bench.seats;
  const w = 26;
  const draw = (x: number, y: number): void => {
    const { c } = ctx;
    const sel = selected(ctx, 'bench');
    hit(ctx, { id: 'bench', kind: 'bench', x, y, w, h: BAND_H });
    c.text(x, y, cut(t('world.bench.title'), w), { bold: true, inverse: sel });
    if (seats.length === 0) c.text(x, y + 1, cut(t('world.bench.none'), w), DIM);
    seats.slice(0, 4).forEach((s, i) => c.text(x, y + 1 + i, cut(`▟▙ ${s.slot} ${s.holder}`, w), {}));
    if (seats.length > 4) c.text(x, y + 5, cut(t('world.more', { n: seats.length - 4 }), w), DIM);
    c.text(x, y + BAND_H - 2, cut(t('world.bench.word', { seats: seats.map(() => '▟▙').join(' ') }), w));
  };
  return { w, draw, ids: ['bench'] };
}

function gateBlock(ctx: Ctx): Block {
  const g = ctx.world.gate;
  const w = 28;
  const draw = (x: number, y: number): void => {
    const { c } = ctx;
    const sel = selected(ctx, 'gate');
    hit(ctx, { id: 'gate', kind: 'gate', x, y, w, h: BAND_H });
    c.text(x, y, cut(t('world.gate.title'), w), { bold: true, inverse: sel });
    let row = y + 1;
    if (g.keys !== null) c.text(x, row++, cut(g.leases ? t('world.gate.notesClaimed', { keys: g.keys, claimed: g.leases }) : t('world.gate.notes', { keys: g.keys }), w));
    for (const cab of g.cabinets.slice(0, 2)) c.text(x, row++, cut(`▣ ${cab.name} ${cab.keys}`, w), DIM);
    if (g.cabinets.length > 2) c.text(x, row++, cut(t('world.gate.cabinets', { n: g.cabinets.length - 2 }), w), DIM);
    // The feed trough: how much of each account is used.
    for (const f of (g.forage ?? []).slice(0, Math.max(0, y + BAND_H - 2 - row))) {
      const used = Math.round(f.used ?? 0);
      c.text(x, row++, cut(`${f.agent ?? '?'} ${f.label ?? ''} ${used}%`, w), f.overgrazing ? BOLD : DIM);
    }
  };
  return { w, draw, ids: ['gate'] };
}

function islandBlock(ctx: Ctx, name: string): Block {
  const island = ctx.world.islands.find((i) => i.name === name);
  const w = 20;
  const id = `island:${name}`;
  const draw = (x: number, y: number): void => {
    const { c } = ctx;
    if (!island) return;
    const sel = selected(ctx, id);
    hit(ctx, { id, kind: 'island', x, y, w, h: BAND_H, name });
    c.text(x, y, cut(t('world.island.title'), w), DIM);
    c.text(x, y + 1, cut(island.name, w), { bold: true, inverse: sel });
    const busy = islandBusy(ctx.world, name);
    c.text(x, y + 2, '≈≈≈ ▄▄▄▄▄▄ ≈≈≈');
    c.text(x, y + 3, `≈≈≈ █ ${busy ? '◉' : '○'}  █ ≈≈≈`);
    c.text(x, y + 4, '≈≈≈ ▀▀▀▀▀▀ ≈≈≈');
    c.text(x, y + BAND_H - 2, cut(island.tools.join(', '), w), DIM);
    if (busy) c.text(x, y + BAND_H - 1, cut(t('world.island.busy'), w), {});
  };
  return { w, draw, ids: [id] };
}

function stableBlock(ctx: Ctx): Block {
  const stalls = ctx.world.stable?.stalls ?? [];
  const w = 22;
  const draw = (x: number, y: number): void => {
    const { c } = ctx;
    const sel = selected(ctx, 'stable');
    hit(ctx, { id: 'stable', kind: 'stable', x, y, w, h: BAND_H });
    c.text(x, y, cut(t('world.stable.title'), w), { bold: true, inverse: sel });
    stalls.slice(0, 3).forEach((s, i) => {
      const row = y + 1 + i * 2;
      c.text(x, row, cut(`${s.horse} · ${s.sessions.length}`, w));
      // One saddle per saved session, the newest nearest the door.
      c.text(x, row + 1, cut(s.sessions.map(() => '∩').join(' '), w), DIM);
    });
    if (stalls.length > 3) c.text(x, y + 7, cut(t('world.stable.stalls', { n: stalls.length - 3 }), w), DIM);
  };
  return { w, draw, ids: ['stable'] };
}

function drawRanch(ctx: Ctx): void {
  const { c, world, o } = ctx;
  const blocks: Block[] = world.houses.map((h) => houseBlock(ctx, h, o.w - 2));
  blocks.push(benchBlock(ctx));
  if (world.gate.keys !== null || world.gate.forage !== null) blocks.push(gateBlock(ctx));
  for (const i of world.islands) blocks.push(islandBlock(ctx, i.name));
  if (world.stable) blocks.push(stableBlock(ctx));
  // Flow: blocks side by side, a new band when the row is full.
  const bands: { blocks: { b: Block; x: number }[] }[] = [];
  let x = 1;
  let band: { blocks: { b: Block; x: number }[] } = { blocks: [] };
  for (const b of blocks) {
    if (band.blocks.length > 0 && x + b.w > o.w - 1) {
      bands.push(band);
      band = { blocks: [] };
      x = 1;
    }
    band.blocks.push({ b, x });
    x += b.w + 3;
  }
  if (band.blocks.length > 0) bands.push(band);
  // The bands that fit, with the band of the selection among them.
  const room = o.h - 2;
  const fitBands = Math.max(1, Math.floor((room + 1) / (BAND_H + 1)));
  const selBand = Math.max(0, bands.findIndex((bd) => bd.blocks.some((e) => e.b.ids.includes(o.view.sel))));
  const first = Math.max(0, Math.min(selBand - fitBands + 1 > 0 ? selBand - fitBands + 1 : 0, bands.length - fitBands));
  const top = selBand < first ? selBand : first;
  bands.slice(top, top + fitBands).forEach((bd, i) => {
    for (const e of bd.blocks) e.b.draw(e.x, 1 + i * (BAND_H + 1));
  });
  if (world.houses.length === 0) c.text(2, 2, cut(t('world.ranch.empty'), Math.max(0, o.w - 2)), DIM);
  const below = bands.length - (top + fitBands);
  scrollNote(ctx, top > 0, below, 'row');
}

// ----------------------------------------------- a building in words

function drawBuildingRoom(ctx: Ctx, h: House, r: Room, x: number, y: number): void {
  const { c } = ctx;
  const id = `room:${r.id}`;
  const sel = selected(ctx, id);
  const w = BUILDING_ROOM_W;
  const hh = BUILDING_ROOM_H;
  const word = roomWord(r);
  const plumbing = ctx.o.view.plumbing;
  hit(ctx, { id, kind: 'room', x, y, w, h: hh + 1, house: h.key, room: r.id });
  (plumbing ? glassFrame : textFrame)(c, x, y, w, hh, sel ? BOLD : {});
  if (r.run.live) c.text(x + 2, y + 1, cut(t('world.room.netSign', { net: netWord(r.net) }), w - 4));
  c.text(x + 5, y + 4, face(r) + (r.asks.length > 0 ? ' !' : ''), r.asks.length > 0 ? BOLD : {});
  c.text(x + 4, y + 5, '▟█▙');
  c.text(x + 4, y + 6, '▀ ▀');
  if (r.run.live) c.text(x + 14, y + 5, cut(r.unread > 0 ? t('world.room.messagesUnread', { n: r.unread }) : t('world.room.messages'), w - 16), r.unread > 0 ? {} : DIM);
  if (r.asks.length > 0) c.text(x, y + 8, cut(t('world.room.asksSign', { summary: r.asks[0]?.summary ?? '' }), w - 1), BOLD);
  if (r.run.frozen) c.text(x + 2, y + 9, cut(r.frozenReason ? t('world.room.pausedReason', { reason: r.frozenReason }) : t('world.room.pausedSign'), w - 4));
  const host = hostTools(r);
  if (host.length > 0) c.text(x + 2, y + 10, cut(t('world.room.hostTools', { tools: host.map((x) => x.name).join(', ') }), w - 3), BOLD);
  if (plumbing && r.tools.length > host.length) c.text(x + 2, y + 11, cut(t('world.room.tools', { tools: r.tools.filter((x) => x.run !== 'host').map((x) => x.name).join(', ') }), w - 4), DIM);
  if (!r.run.live) c.text(x + 2, y + 9, cut(t('world.room.dark'), w - 4), DIM);
  const since = r.run.stateSince > 0 ? ` · ${age(ctx.t - r.run.stateSince)}` : '';
  // A helper has the mark of its path. The time shows when it fits.
  const short = `${sel ? '› ' : ''}${r.parent ? '↳ ' : ''}${r.name} ${word.glyph} ${word.word}`;
  const cap = cut(cells(short + since) <= w ? short + since : short, w);
  c.text(x, y + hh, cap, { bold: sel || word.amber, inverse: sel, dim: !r.run.live && !sel });
}

function drawLoftFloor(ctx: Ctx, h: House, x: number, y: number, w: number): void {
  const { c } = ctx;
  const id = `loft:${h.key}`;
  const sel = selected(ctx, id);
  c.text(x, y, '▄'.repeat(w));
  c.text(x + 2, y, ` ${cut(h.name, w - 6)} `, BOLD);
  for (let i = 1; i < 4; i++) {
    c.text(x, y + i, '█');
    c.text(x + w - 1, y + i, '█');
  }
  if (!h.loft) {
    c.text(x + 2, y + 2, cut(t('world.loft.none'), w - 4), DIM);
    return;
  }
  hit(ctx, { id, kind: 'loft', x: x + 1, y: y + 1, w: w - 2, h: 3, house: h.key });
  const pulse = ctx.o.motion.running('pulse', 'world', ctx.o.tick) && (ctx.o.tick & 2) === 0;
  const lx = x + HALL_W + 1;
  c.text(lx, y + 1, cut(`${sel ? '› ' : ''}${t('world.building.loftFull')}`, Math.max(0, w - HALL_W - 3)), { bold: true, inverse: sel || pulse });
  c.text(lx, y + 2, cut(t('world.loft.shelf'), w - HALL_W - 3), DIM);
}

function drawBuilding(ctx: Ctx): void {
  const { c, world, o } = ctx;
  const h = world.houses.find((x) => x.key === o.view.house);
  if (!h) {
    c.text(2, 2, cut(t('world.building.gone'), Math.max(0, o.w - 2)), DIM);
    return;
  }
  const W = o.w - 2;
  const perFloor = Math.max(1, Math.floor((W - HALL_W - 1 + 2) / (BUILDING_ROOM_W + 2)));
  drawLoftFloor(ctx, h, 1, 1, W);
  const top = 5;
  const floorH = BUILDING_ROOM_H + 1;
  const floors = Math.max(1, Math.floor((o.h - 1 - top) / floorH));
  const selIndex = Math.max(0, h.rooms.findIndex((r) => `room:${r.id}` === o.view.sel));
  const selFloor = Math.floor(selIndex / perFloor);
  const total = Math.max(1, Math.ceil(h.rooms.length / perFloor));
  const first = Math.max(0, Math.min(selFloor - floors + 1 > 0 ? selFloor - floors + 1 : 0, total - floors));
  const firstFloor = selFloor < first ? selFloor : first;
  const shownFloors = Math.min(floors, total - firstFloor);
  const bottom = top + shownFloors * floorH;
  // The hall: the ladder to the loft, and the tools that every room shares.
  for (let i = top; i < bottom - 1; i++) c.text(1, i, h.loft ? '█ ╫ ' : '█', {});
  h.hall.slice(0, 3).forEach((t, i) => {
    const y = top + 2 + i * 4;
    if (y + 3 >= bottom) return;
    const id = `tool:${t.name}`;
    const sel = selected(ctx, id);
    hit(ctx, { id, kind: 'tool', x: 2, y, w: HALL_W - 2, h: 3, house: h.key, name: t.name });
    c.text(5, y, t.run === 'host' ? '⏚!' : '⏚', { bold: t.run === 'host' || sel });
    if (o.view.plumbing || sel || t.run === 'host') c.text(2, y + 2, cut(t.name, HALL_W - 2), { inverse: sel, bold: t.run === 'host' });
  });
  if (h.rooms.length === 0) c.text(HALL_W + 3, top + 2, cut(t('world.building.noRun'), Math.max(0, o.w - HALL_W - 3)), DIM);
  for (let fl = 0; fl < shownFloors; fl++) {
    const y = top + fl * floorH;
    h.rooms.slice((firstFloor + fl) * perFloor, (firstFloor + fl + 1) * perFloor).forEach((r, i) => {
      const x = 1 + HALL_W + 1 + i * (BUILDING_ROOM_W + 2);
      drawBuildingRoom(ctx, h, r, x, y);
      // The path of a helper, at the floor.
      if (i > 0 && r.parent) c.text(x - 2, y + BUILDING_ROOM_H - 2, '··');
    });
  }
  scrollNote(ctx, firstFloor > 0, total - firstFloor - shownFloors, 'floor');
}

// ----------------------------------------------------------------- room

/** One line of the panel beside the room: a plain word, what it says, and the line that joins it to the room. */
export interface PanelLine {
  id: string;
  kind: HitKind;
  label: string;
  value: string;
  amber?: boolean;
  dim?: boolean;
  /** What joins the line to the wall: nothing, a pipe, a cable to your machine, a cable to an island, a cable that stays inside. */
  link?: 'pipe' | 'host' | 'remote' | 'inside' | 'open';
  name?: string;
  /** A header: not a thing to select. */
  header?: boolean;
  /** A tool whose calls wait for you: its socket has a cover. */
  asks?: boolean;
}

function left(tm: { left?: number; over?: boolean; stopped?: boolean; name?: string }): string {
  if (tm.over) return t('world.timer.over', { name: tm.name ?? t('world.timer.name') });
  return tm.left !== undefined ? t('world.timer.left', { name: tm.name ?? t('world.timer.name'), left: age(tm.left) }) : (tm.name ?? t('world.timer.name'));
}

/** The lines of the panel of a room. Simple: what you need at a glance, and everything amber. Plumbing: every line that leaves the room. */
export function panelLines(world: World, h: House, r: Room, plumbing: boolean, at = now()): PanelLine[] {
  const out: PanelLine[] = [];
  const live = r.run.live;
  if (live) {
    const link = r.net === 'none' || r.net === 'unknown' ? undefined : r.net === 'host' ? 'host' : r.net === 'egress' ? 'open' : 'pipe';
    out.push({ id: 'window', kind: 'window', label: t('world.inspect.internet'), value: netWord(r.net), amber: r.net === 'host', link });
  }
  const ask = r.asks[0];
  if (ask) {
    const exp = ask.expires && ask.expires > at ? ` · ${age(ask.expires - at)}` : '';
    out.push({ id: 'door', kind: 'door', label: t('world.inspect.asks'), value: `${ask.summary || ask.capability || t('world.panel.request')}${r.asks.length > 1 ? ` (+${r.asks.length - 1})` : ''}${exp}`, amber: true });
  } else if (live) out.push({ id: 'door', kind: 'door', label: t('world.inspect.asks'), value: t('world.panel.nothing'), dim: true });
  out.push({ id: 'mail', kind: 'mail', label: t('world.inspect.messages'), value: r.unread > 0 ? t('world.panel.unread', { n: r.unread }) : t('world.panel.noneUnread'), dim: r.unread === 0 });
  const word = roomWord(r);
  const since = r.run.stateSince > 0 ? ` · ${age(at - r.run.stateSince)}` : '';
  out.push({ id: 'self', kind: 'self', label: r.run.frozen ? t('world.inspect.paused') : t('world.inspect.agent'), value: r.run.frozen ? `${r.frozenReason || t('world.panel.memory')}${since}` : `${word.word}${since}` });
  if (live || r.changed !== null || r.copy) {
    // The copy of the run, when paddock has one: its changes wait for you, so they are amber.
    if (r.copy) out.push({ id: 'copy', kind: 'copy', label: t('world.inspect.copy'), value: changesLine(r.copy) || (r.copy.applied > 0 ? t('world.panel.appliedNoOpen', { n: r.copy.applied }) : t('copy.noOpenChange')), amber: waiting(r) > 0, dim: waiting(r) === 0 });
    else out.push({ id: 'copy', kind: 'copy', label: t('world.inspect.copy'), value: r.changed === null ? t('world.panel.changesUnknown') : r.changed === 0 ? t('world.panel.noChange') : t('world.panel.changed', { n: r.changed }), dim: !r.changed });
  }
  if (r.parent) out.push({ id: 'helper', kind: 'helper', label: t('world.inspect.helper'), value: `${t('world.panel.helperOf', { run: world.houses.flatMap((x) => x.rooms).find((x) => x.id === r.parent)?.name ?? r.parent })}${r.specTask ? ` · ${r.specTask}` : ''}` });
  else if (r.children.length > 0) out.push({ id: 'helper', kind: 'helper', label: t('world.panel.helpers'), value: t('world.panel.onPath', { n: r.children.length }) });
  const yard: Yard | undefined = h.yards.find((y) => y.name === r.yard);
  if (yard) out.push({ id: 'yard', kind: 'yard', label: t('world.inspect.yard'), value: `${yard.name} · ${t('world.panel.rooms', { n: yard.members.length })}${yard.ports.length > 0 ? ` · ${t('world.panel.ports', { n: yard.ports.length })}` : ''}`, name: yard.name });
  for (const tm of r.timers.slice(0, 2)) out.push({ id: `timer:${tm.name ?? ''}`, kind: 'timer', label: t('world.inspect.timer'), value: left(tm), amber: false, name: tm.name ?? '' });
  if (h.loft) out.push({ id: `loft:${h.key}`, kind: 'loft', label: t('world.panel.loft'), value: t('world.panel.notesUp') });
  // Tools. A tool on your machine is amber and always shows; the rest shows with Plumbing.
  const tools = r.tools.filter((x) => plumbing || x.run === 'host');
  if (tools.length > 0) out.push({ id: '', kind: 'tool', label: t('world.panel.tools'), value: plumbing ? '' : t('world.panel.onHost'), header: true, amber: !plumbing });
  for (const tl of tools) {
    const where = tl.run === 'host' ? t('world.panel.yourMachine') : tl.run === 'remote' ? tl.peer : tl.run === 'sandbox' ? t('world.panel.inRoom') : tl.run === 'broker' ? 'paddock' : t('world.inspect.notKnown');
    out.push({
      id: `tool:${tl.name}`,
      kind: 'tool',
      label: tl.name,
      value: `${where}${tl.ask ? ` · ${t('world.panel.ask')}` : ''}${tl.on ? ` · ${t('world.panel.on')}` : ''}`,
      amber: tl.run === 'host',
      asks: tl.ask,
      link: tl.run === 'host' ? 'host' : tl.run === 'remote' ? 'remote' : tl.run === 'sandbox' ? 'inside' : 'pipe',
      name: tl.name,
    });
  }
  if (plumbing && live) {
    out.push({ id: 'notes', kind: 'notes', label: t('world.inspect.kv'), value: t('world.panel.notes') });
    out.push({ id: 'borrow', kind: 'borrow', label: t('world.inspect.borrow'), value: t('world.panel.borrow'), dim: true });
  }
  return out;
}

/** The line through the wall, without art: a pipe, a cable to your machine, a cable to an island, a cable that stays inside, an open window. */
export const LINK: Record<NonNullable<PanelLine['link']>, string> = { pipe: '╞═▶', host: '══▶', remote: '≈≈▶', inside: '──●', open: '┄┄▶' };

/** Where the room stands in a picture with art, and where the words beside it begin. */
export interface RoomPlan {
  /** The size of a pixel of the art. */
  scale: number;
  /** The left edge of the room and its first row, in pixels of the art. The mailbox stands 6 pixels to its left. */
  x: number;
  y: number;
  /** The height of the room, of the attic over it and of the roof, in pixels. */
  h: number;
  attic: number;
  roof: number;
  /** The column of the words beside the room, and the row of the first of them. */
  panelX: number;
  row: number;
  /** The rows from one line of words to the next. */
  step: number;
}

/** From this width on the room can be twice as large. */
const ROOM_DOUBLE_W = 150;
const ROOM_PX_H = ROOM_H * 2;

function roomLayout(w: number, h: number, s: number, loft: boolean): RoomPlan | null {
  const row = Math.max(1, Math.ceil(2 / s));
  const top = row;
  const P = Math.floor(((h - 1) * 2) / s) - top;
  // The roof and the attic over the room, from the roomiest to the most compact. Then the room itself gets lower.
  const details: [number, number][] = loft
    ? [
        [4, 10],
        [2, 8],
        [2, 6],
      ]
    : [
        [4, 0],
        [2, 0],
      ];
  let pick = details.find(([r, a]) => r + a + ROOM_PX_H <= P);
  let high = ROOM_PX_H;
  if (!pick) {
    if (s > 1) return null;
    pick = details[details.length - 1] as [number, number];
    high = Math.floor((P - pick[0] - pick[1]) / 2) * 2;
    // A very low screen: the room alone, as high as the screen has rows.
    if (high < 28) {
      pick = [0, 0];
      high = Math.max(16, Math.min(ROOM_PX_H, Math.floor(P / 2) * 2));
    }
  }
  const [roof, attic] = pick;
  // The mailbox, the room, the lines through its wall, the words.
  const art = ROOM_X + 1 + ROOM_W + 3;
  const words = Math.max(0, Math.min(56, w - art * s));
  const left = Math.max(0, Math.floor((w - art * s - words) / 2 / s));
  const total = roof + attic + high;
  const y = top + Math.max(0, Math.floor((P - total) / 2 / row) * row) + roof + attic;
  return { scale: s, x: left + ROOM_X + 1, y, h: high, attic, roof, panelX: (left + art) * s, row: Math.floor(((y + 4) * s) / 2), step: s };
}

/** The room for a screen: twice as large when the screen has the rows and the columns for it. */
export function planRoom(w: number, h: number, loft: boolean): RoomPlan {
  const big = w >= ROOM_DOUBLE_W ? roomLayout(w, h, 2, loft) : null;
  return big ?? (roomLayout(w, h, 1, loft) as RoomPlan);
}

/** The column where the words beside the room begin: the card and the Inspector stand there. */
export function roomPanelX(world: World, v: View, o: { w: number; h: number; mode: SceneOptions['mode'] }): number {
  if (o.mode === 'none' || o.w < WORLD_MIN_W || o.h < WORLD_MIN_H) return ROOM_PANEL_X;
  const h = world.houses.find((x) => x.rooms.some((r) => r.id === v.room));
  return planRoom(o.w, o.h, (h?.loft ?? '') !== '').panelX;
}

function drawRoom(ctx: Ctx, plan: RoomPlan | null): void {
  const { c, world, o } = ctx;
  const found = world.houses.flatMap((h) => h.rooms.map((r) => ({ h, r }))).find((e) => e.r.id === o.view.room);
  if (!found) {
    c.text(2, 2, cut(t('world.room.gone'), Math.max(0, o.w - 2)), DIM);
    return;
  }
  const { h, r } = found;
  const plumbing = o.view.plumbing;
  const x = ROOM_X;
  const y = 1;
  const w = ROOM_W;
  const hh = Math.min(ROOM_H, Math.max(8, o.h - 3));
  const f = frost(ctx, r);
  const door = atDoor(ctx, r);
  const pulse = o.motion.running('pulse', 'world', o.tick) && (o.tick & 2) === 0;
  const lines = panelLines(world, h, r, plumbing, ctx.t);
  // The panel: the Internet line stands at the window, the rest follows.
  const panelX = plan ? plan.panelX : ROOM_PANEL_X;
  const panelW = Math.max(0, Math.min(o.w - panelX, 64));
  // The words stand in two columns. A wide panel has room for the longest label.
  const labelW = panelW >= 44 ? 16 : 13;
  const rowOf = new Map<PanelLine, number>();
  const step = plan ? plan.step : 1;
  let row = plan ? plan.row : y + 2;
  const maxRow = o.h - 2;
  let cutAt = lines.length;
  lines.forEach((l, i) => {
    if (row > maxRow && cutAt === lines.length) cutAt = i;
    rowOf.set(l, row);
    row += step;
  });
  if (cutAt < lines.length) cutAt = Math.max(0, cutAt - 1);

  if (plan) {
    const s = plan.scale;
    const X = plan.x;
    const Y = plan.y;
    const H = plan.h;
    /** The row of pixels of a row of words. */
    const py = (l: PanelLine): number => Math.floor(((rowOf.get(l) as number) * 2) / s);
    const tall = H >= art.LAYER_SIZES['room-40x34-lit']![1];
    const self = selected(ctx, 'self');
    const loftLit = selected(ctx, `loft:${h.key}`) || pulse;
    // The attic of the house over the room: the loft, with its shelf of notebooks. The roof lies on it.
    if (plan.attic > 0) {
      const A = Y - plan.attic;
      c.blit(art.tiles('tile-wall', w, plan.attic), X, A);
      c.outline(X, A, w, plan.attic + 1, loftLit ? px.SELECT : px.INK);
      c.setClip({ x: X + 1, y: A + 1, w: w - 2, h: plan.attic - 1 });
      // A new notebook slides onto the shelf from the left.
      const slide = o.motion.progress('note', h.key, o.tick);
      for (let i = 0; i < 10; i++) c.blit(art.pic(`spine-${i % 6}`), X + 17 + i * 2 + (i === 0 ? lerp(-6, 0, slide) : 0), Y - 6);
      // A character that writes a note is upstairs, small as the attic is.
      for (const w of h.rooms) if (trip(ctx, w).where === 'loft') c.blit(writing(ctx, w).small, X + 28 + (w.id === r.id ? 0 : 4), Y - 8);
      c.setClip(null);
    }
    if (plan.roof > 0) roof(c, X, Y - plan.attic, w, plan.roof, 0);
    // With Plumbing the walls turn to glass, and back.
    c.blit(roomShell(ctx, '40x34', r, w, H, f, self), X, Y);
    // The ladder to the loft goes up through the ceiling, into the attic.
    if (h.loft) {
      c.setClip({ x: X, y: Y - plan.attic + 1, w, h: plan.attic + 8 });
      for (let ly = Y + 4; ly > Y - plan.attic - 4; ly -= 4) c.blit(loftLit ? art.tint('ladder', 'glow') : art.pic('ladder'), X + 9, ly);
    }
    // In a low window the room is low: what stands in it stays inside its walls.
    c.setClip({ x: X + 1, y: Y + 1, w: w - 2, h: H - 2 });
    if (r.run.live) {
      // The window changes its mode: the old window turns into the new one.
      c.blit(windowOf(ctx, r), X + w - 9, Y + 2);
      if (selected(ctx, 'window') || pulse) c.outline(X + w - 9, Y + 2, 7, 8, px.SELECT);
    }
    if (plumbing) {
      c.blit(art.pic('pegboard'), X + 16, Y + 1);
      // A shared note was written: its tag flips. The notes of the yard hang left, yours right.
      if (r.yard) tagFlip(ctx, `yard:${h.key}:${r.yard}`, X + 18, Y + 2, px.WOOD_LIGHT);
      tagFlip(ctx, 'gate', X + 20, Y + 2, px.WOOD_LIGHT);
      if (selected(ctx, 'notes')) c.outline(X + 16, Y + 1, 11, 5, px.SELECT);
    }
    // The door, in the left wall. Amber while a request waits.
    c.setClip({ x: X, y: Y + 1, w, h: H - 2 });
    c.blit(art.pic(doorOf(ctx, r), o.tick), X, Y + H - 17, r.run.live ? undefined : NIGHT_MAP);
    if (selected(ctx, 'door') || pulse) c.fill(X + 2, Y + H - 17, 1, 16, px.SELECT);
    c.setClip({ x: X + 1, y: Y + 1, w: w - 2, h: H - 2 });
    // The desk with the model of the project. The part that changed has the colour of the agent.
    const waits = waiting(r);
    const moving = o.motion.running('apply', r.id, o.tick);
    const move = o.motion.progress('apply', r.id, o.tick);
    if (r.run.live || r.changed !== null || r.copy) {
      const dark = r.run.live ? undefined : NIGHT_MAP;
      c.blit(art.pic('desk'), X + 27, Y + H - 11, dark);
      // While its parts leave for the house, the model keeps their colour for the first half of the way.
      c.blit(art.model(r.changed || waits > 0 || (moving && move < 0.5) ? AGENT[colorOf(r.run.harness)] : null), X + 31, Y + H - 16, dark);
      if (waits > 0) {
        // Changes wait for you: the model glows amber, and the glow breathes once in two seconds.
        const still = o.motion.reduce;
        const breath = still ? 1 : (1 - Math.cos(((o.tick % 16) / 16) * 2 * Math.PI)) / 2;
        const glow = mix(px.AMBER_DARK, px.AMBER_LIGHT, breath);
        // A frame with open corners, one pixel off the model.
        c.fill(X + 31, Y + H - 17, 7, 1, glow);
        c.fill(X + 31, Y + H - 11, 7, 1, glow);
        c.fill(X + 30, Y + H - 16, 1, 5, glow);
        c.fill(X + 38, Y + H - 16, 1, 5, glow);
        if (!still) ctx.loops = true;
      }
      const timer = r.timers[0];
      // The hourglass says where the timer is: stopped, running, over. It does not need the clock.
      if (timer) c.blit(art.layer('hourglass').frames[timer.over ? 2 : timer.stopped ? 0 : 1] ?? art.pic('hourglass'), X + 27, Y + H - 16);
      if (selected(ctx, 'copy')) c.outline(X + 30, Y + H - 17, 9, 7, px.SELECT);
    }
    const ch = character(ctx, r);
    const way = trip(ctx, r);
    // It climbs the ladder to the loft: up through the ceiling, and down again when it wrote its note.
    if (way.where === 'ladder') c.blit(ch.frame, X + lerp(3, -1, way.p * 3), Y + H - 28 - lerp(0, H - 8, way.p));
    else if (way.where === 'room') c.blit(ch.frame, X + lerp(3, 1, door), Y + H - 28);
    // The card of the request: on the wall at the door, where the character holds it up.
    if (r.asks.length > 0 && door >= 1) c.blit(art.pic('card'), X + 2, Y + H - 29);
    // A tool that runs in the room: an appliance on the wall, and its cable down to its socket.
    const inside = tall && plumbing ? lines.slice(0, cutAt).filter((l) => l.link === 'inside') : [];
    inside.slice(0, 2).forEach((_, i) => c.blit(art.pic('appliance'), X + w - 7 - i * 6, Y + 11));
    c.setClip(null);
    if (moving) {
      // After an apply: the changed parts rise from the model and go into the house, one after the other.
      const tint = AGENT[colorOf(r.run.harness)];
      const toX = X + Math.floor(w / 2);
      const toY = plan.roof > 0 || plan.attic > 0 ? Y - plan.attic - Math.floor(plan.roof / 2) : Y + 1;
      for (let k = 0; k < 4; k++) {
        const t = move * 1.45 - k * 0.15;
        if (t <= 0 || t >= 1) continue;
        const e = ease(t);
        const x = lerp(X + 33 + k, toX - 2 + k, e);
        const y = lerp(Y + H - 14, toY, e) - Math.round(Math.sin(Math.PI * t) * 3);
        c.fill(x, y, 2, 2, tint);
        c.pixel(x, y + 2, px.INK);
        c.pixel(x + 1, y + 2, px.INK);
      }
      // They arrive: the top of the house takes their colour for a moment.
      if (move > 0.6) c.fill(toX - 3, toY, 7, 1, tint);
    }
    // The mailbox stands outside, at the door.
    if (r.run.live || r.unread > 0) {
      // The flag goes up for unread mail. The box hops once for a question.
      c.blit(mailbox(flagUp(ctx, r)), X - 6, Y + H - 12 - hop(ctx, r));
      if (selected(ctx, 'mail')) c.outline(X - 6, Y + H - 12, 6, 11, px.SELECT);
    }
    frostCorners(c, X + 1, Y + 1, w - 2, H - 2, Math.ceil(f * art.FROST_STEPS));
    flakeBadge(ctx, r, X + 2, Y + 9);
    // What leaves the room: a line through the right wall, at the row of its words.
    const ex = Math.floor(panelX / s) - 1;
    const through = (sx: number, ly: number, link: PanelLine['link'], bright = false): void => {
      // Reduce Motion: the cable of a call is bright for a second.
      const lit = (p: Rgb): Rgb => (bright ? mix(p, px.WHITE, 0.6) : p);
      if (link === 'host') c.fill(sx, ly, ex - sx, 1, lit(px.AMBER));
      else if (link === 'open') c.fill(sx, ly, ex - sx, 2, px.SKY);
      else if (link === 'pipe') c.fill(sx, ly, ex - sx, 1, lit(px.STEEL));
      else if (link === 'remote') for (let i = sx; i < ex; i++) if ((i - sx) % 3 !== 2) c.pixel(i, ly, lit(px.WATER));
    };
    const shown = lines.slice(0, cutAt).filter((l) => l.link);
    const inWall = (ly: number): boolean => ly > Y + 1 && ly + 2 < Y + H - 1;
    for (const l of shown) {
      const ly = py(l);
      // The Internet leaves at the window. A tool leaves at its socket.
      const on = l.kind === 'tool' && l.name ? call(ctx, r, l.name) : null;
      if (l.kind === 'window') through(X + w - 2, ly, l.link);
      else through(X + w, ly, l.link, on?.bright);
      const cable = l.link === 'inside' && inside.includes(l) && inWall(ly);
      if (cable) c.fill(X + w - 2, Math.min(Y + 16, ly), 1, Math.abs(ly - Y - 16), on?.bright ? mix(px.STEEL_DARK, px.WHITE, 0.6) : px.STEEL_DARK);
      // A call of a tool in the room: a spark runs along its cable, from the socket to the appliance.
      if (on && on.spark !== null && !l.asks && cable) c.fill(X + w - 2, lerp(ly, Y + 16, on.spark), 1, 2, px.WHITE);
    }
    for (const l of shown) {
      const ly = py(l);
      if (l.kind !== 'tool' || !inWall(ly)) continue;
      const on = l.name ? call(ctx, r, l.name) : null;
      const socket = l.amber ? 'socket-host' : l.asks ? 'socket-cover' : 'socket-open';
      // The socket of a tool in the room is lit for the time of its spark. With Reduce Motion, so is the cover of a tool that asks.
      const lit = on !== null && ((on.bright && (l.asks === true || l.link === 'inside')) || (on.spark !== null && l.link === 'inside'));
      c.blit(lit ? art.tint(socket, 'glow') : art.pic(socket), X + w - 3, ly - 1);
      if (on && on.spark !== null && !l.asks && l.link !== 'inside') {
        // A call: a spark leaves through the socket and runs out along the cable. A short cable has a short spark.
        const long = Math.max(1, Math.min(3, Math.floor((ex - X - w) / 3)));
        c.fill(lerp(X + w - 2, Math.max(X + w - 2, ex - long), on.spark), ly, long, 1, px.WHITE);
      }
      if (!on || !l.asks) continue;
      // The socket of a tool that asks has a cover: the spark comes from inside the room and stops there. Then a card rises.
      c.setClip({ x: X + 1, y: Y + 1, w: w - 2, h: H - 2 });
      if (on.spark !== null) c.fill(lerp(X + w - 14, X + w - 6, on.spark), ly, 3, 1, px.WHITE);
      if (on.rise !== null) c.blit(art.pic('card'), X + w - 9, ly - 6 - lerp(0, 3, on.rise));
      c.setClip(null);
    }
    // A project without a loft: a small sign beside the roof says so.
    if (!h.loft && plan.roof > 0 && panelW >= 14) c.text(panelX, Math.floor(((Y - plan.roof) * s) / 2), cut(t('world.noNotes'), panelW), DIM);
  } else {
    (plumbing ? glassFrame : textFrame)(c, x, y, w, hh, selected(ctx, 'self') ? BOLD : {});
    if (h.loft) c.text(x + 3, y, t('world.room.loftTag'), { inverse: selected(ctx, `loft:${h.key}`) || pulse });
    if (r.run.live) c.text(x + w - 2 - cells(`[${netWord(r.net)}]`), y + 2, `[${netWord(r.net)}]`, { inverse: selected(ctx, 'window') || pulse, bold: r.net === 'host' });
    const cx = x + lerp(8, 3, door);
    c.text(cx + 1, y + hh - 5, face(r) + (r.asks.length > 0 ? ' !' : ''), r.asks.length > 0 ? BOLD : {});
    c.text(cx, y + hh - 4, '▟█▙');
    c.text(cx, y + hh - 3, '▀ ▀');
    if (r.asks.length > 0) c.text(x, y + hh - 4, '▐!', { bold: true, inverse: selected(ctx, 'door') || pulse });
    else c.text(x, y + hh - 4, '▐', { inverse: selected(ctx, 'door') || pulse });
    c.text(1, y + hh - 3, cut(r.unread > 0 ? `⚑${r.unread}` : '⚐', 4), { inverse: selected(ctx, 'mail'), dim: r.unread === 0 });
    if (r.run.live || r.changed !== null || r.copy) {
      const open = r.copy ? waiting(r) : (r.changed ?? 0);
      c.text(x + w - 13, y + hh - 4, cut(`▄▄▄ ${t('world.room.copy')}${open ? ` ${open}~` : ''}`, 12), { inverse: selected(ctx, 'copy'), bold: waiting(r) > 0 });
      c.text(x + w - 13, y + hh - 3, '▀▀▀');
    }
    if (r.timers.length > 0) c.text(x + w - 16, y + hh - 4, '⧖');
    if (r.run.frozen) c.text(x + 2, y + 2, cut(`❄ ${t('world.room.paused')}`, Math.max(0, w - 4)), {});
    if (!r.run.live) c.text(x + 2, y + 2, cut(t('world.room.dark'), Math.max(0, w - 4)), DIM);
    if (plumbing) c.text(x + 14, y + 1, cut(`▦ ${t('world.room.notes')}`, Math.max(0, w - 15)), { inverse: selected(ctx, 'notes') });
  }
  // The panel of words.
  lines.slice(0, cutAt).forEach((l) => {
    const ly = rowOf.get(l) as number;
    const sel = !l.header && selected(ctx, l.id);
    if (!l.header) hit(ctx, { id: l.id, kind: l.kind, x: panelX, y: ly, w: panelW, h: 1, house: h.key, room: r.id, name: l.name });
    const amber: TextStyle = c.art ? { fg: px.amberText(c.light), bold: true } : { bold: true };
    if (!c.art && l.link) c.text(x + w, ly, cut(LINK[l.link], panelX - x - w - 1), l.amber ? BOLD : {});
    const lab = l.header ? l.label : fit(l.label, labelW - 1);
    c.text(panelX, ly, cut(lab, panelW), { bold: true, ...(l.amber ? amber : {}), inverse: sel, dim: l.dim && !sel });
    c.text(panelX + cells(lab) + 1, ly, cut(l.value, Math.max(0, panelW - cells(lab) - 1)), { ...(l.amber ? amber : {}), dim: (l.dim || l.header) && !l.amber });
  });
  if (cutAt < lines.length) c.text(panelX, maxRow, cut(t('world.panel.more', { n: lines.length - cutAt }), panelW), DIM);
}

// ----------------------------------------------------------------- loft

function drawLoft(ctx: Ctx, plan: RoomPlan | null): void {
  const { c, world, o } = ctx;
  const h = world.houses.find((x) => x.key === o.view.house);
  if (!h || !h.loft) {
    c.text(2, 2, cut(t('world.loft.gone'), Math.max(0, o.w - 2)), DIM);
    return;
  }
  const rows: [string, string, TextStyle][] = [
    ['note:log', t('world.loft.log'), { bold: true }],
    ['', t('world.inspect.notesAbout'), {}],
    ['', t('world.loft.noList'), {}],
    ['', t('world.loft.inspector'), {}],
    ['note:folder', t('world.loft.folder', { path: h.loft }), { dim: true }],
  ];
  if (plan) {
    // The attic stands where the room stood, one floor up: the roof, the shelves of notebooks, the ladder through the floor.
    const s = plan.scale;
    const X = plan.x;
    const top = plan.y - plan.attic;
    const H = plan.attic + plan.h;
    c.blit(art.shell('40x34', 'lit', ROOM_W, H), X, top);
    if (plan.roof > 0) roof(c, X, top, ROOM_W, plan.roof, 0);
    c.setClip({ x: X + 1, y: top + 1, w: ROOM_W - 2, h: H - 2 });
    // A new notebook slides onto the top shelf from the left.
    const slide = o.motion.progress('note', h.key, o.tick);
    for (let shelf = 0; shelf < 2 && top + 12 + shelf * 10 < top + H - 12; shelf++) {
      const y = top + 4 + shelf * 10;
      c.blit(art.tiles('shelf', 24, 2), X + 13, y + 6);
      for (let i = 0; i < 11 - shelf * 4; i++) c.blit(art.pic(`spine-${(i + shelf * 2) % 6}`), X + 14 + i * 2 + (shelf === 0 && i === 0 ? lerp(-8, 0, slide) : 0), y);
    }
    for (let i = 0; i < 3; i++) c.blit(art.pic('ladder'), X + 5, top + H - 4 - i * 4);
    // A character that writes a note: it comes up the ladder through the floor, writes at the shelf, and goes down again.
    h.rooms.forEach((w, i) => {
      const way = trip(ctx, w);
      if (way.where === 'room') return;
      const frame = way.where === 'loft' ? writing(ctx, w).frame : character(ctx, w).frame;
      c.blit(frame, X + (way.where === 'loft' ? 12 + (i % 2) * 6 : lerp(-5, 4, way.p)), top + H - 28 + (way.where === 'loft' ? 0 : lerp(22, 0, way.p)));
    });
    c.setClip(null);
    // The words stand beside the attic.
    const px0 = plan.panelX;
    const W = Math.max(0, Math.min(o.w - px0, 64));
    let row = Math.floor(((top + 2) * s) / 2);
    c.text(px0, row, cut(t('world.inspect.loft'), W), BOLD);
    row += 2;
    for (const [id, text, style] of rows) {
      if (row > o.h - 2) break;
      const sel = id !== '' && selected(ctx, id);
      if (id) hit(ctx, { id, kind: 'note', x: px0, y: row, w: W, h: 1, house: h.key });
      c.text(px0, row, cut(`${sel ? '› ' : ''}${text}`, W), { ...style, inverse: sel });
      row += plan.step;
    }
    return;
  }
  const x = ROOM_X;
  const y = 1;
  const w = Math.min(o.w - x - 2, 70);
  const hh = Math.min(ROOM_H, Math.max(8, o.h - 3));
  textFrame(c, x, y, w, hh);
  for (let i = 3; i < hh - 1; i++) c.text(x + 3, y + i, '╫');
  c.text(x + 11, y + 2, '▌▌▌▌▌▌▌▌▌▌▌▌');
  rows.forEach(([id, text, style], i) => {
    const ry = y + 7 + i;
    if (ry >= y + hh - 2) return;
    const sel = id !== '' && selected(ctx, id);
    if (id) hit(ctx, { id, kind: 'note', x: x + 11, y: ry, w: w - 13, h: 1, house: h.key });
    c.text(x + 11, ry, cut(`${sel ? '› ' : ''}${text}`, w - 13), { ...style, inverse: sel });
  });
  c.text(x + 11, y + 1, cut(t('world.inspect.loft'), Math.max(0, w - 13)), BOLD);
}

// ---------------------------------------------------------------- frame

/** The breadcrumb: "Ranch › project › room › Loft". */
export function breadcrumb(world: World, v: View): string {
  const parts = [t('world.crumb.ranch')];
  const h = world.houses.find((x) => x.key === v.house);
  if (v.level !== 'ranch') parts.push(h?.name ?? '?');
  if (v.level === 'room') parts.push(h?.rooms.find((r) => r.id === v.room)?.name ?? '?');
  if (v.level === 'loft') parts.push(t('world.crumb.loft'));
  return parts.join(' › ');
}

/** One line on what is selected, in plain words. */
export function infoLine(world: World, v: View, at = now()): string {
  const [kind, ...rest] = v.sel.split(':');
  const name = rest.join(':');
  if (kind === 'room') {
    const r = world.houses.flatMap((h) => h.rooms).find((x) => x.id === name);
    if (!r) return '';
    const w = roomWord(r);
    const run: RunRow = r.run;
    const since = run.stateSince > 0 ? ` ${age(at - run.stateSince)}` : '';
    const host = hostTools(r);
    return [
      r.name,
      `${w.glyph} ${w.word}${since}`,
      r.asks[0] ? `"${r.asks[0].summary ?? ''}"` : '',
      run.live ? t('world.building.net', { net: netWord(r.net) }) : '',
      host.length > 0 ? t('world.info.host', { tools: host.map((x) => x.name).join(', ') }) : '',
      r.yard ? t('world.yard', { name: r.yard }) : '',
      r.parent ? t('world.info.helper') : '',
      run.ticket,
      run.last,
    ]
      .filter((x) => x.length > 0)
      .join(' · ');
  }
  if (v.level === 'room') {
    const e = world.houses.flatMap((h) => h.rooms.map((r) => ({ h, r }))).find((x) => x.r.id === v.room);
    const l = e ? panelLines(world, e.h, e.r, true, at).find((x) => x.id === v.sel && !x.header) : undefined;
    return l ? t('world.info.line', { label: l.label, value: l.value }) : '';
  }
  if (kind === 'loft') return t('world.info.loft');
  if (kind === 'sign') {
    const h = world.houses.find((x) => x.key === name);
    if (!h) return '';
    return h.tickets.length > 0 ? t('world.info.tickets', { name: h.name, tickets: h.tickets.map((x) => `${x.id} ${x.state}`).join(', ') }) : t('world.info.noTicket', { name: h.name });
  }
  if (kind === 'yard') {
    const [key, ...yn] = rest;
    const yd = world.houses.find((x) => x.key === key)?.yards.find((x) => x.name === yn.join(':')) ?? world.houses.flatMap((x) => x.yards).find((x) => x.name === name);
    return yd ? `${yd.keys !== null ? t('world.info.yardNotes', { name: yd.name, n: yd.members.length, notes: yd.keys }) : t('world.info.yard', { name: yd.name, n: yd.members.length })}${yd.ports.length > 0 ? t('world.info.ports', { ports: yd.ports.map((p) => p.port).join(', ') }) : ''}` : t('world.inspect.yard');
  }
  if (kind === 'bench') return world.bench.seats.length > 0 ? t('world.info.bench', { seats: world.bench.seats.map((s) => `${s.slot} ${s.holder}`).join(', ') }) : t('world.info.benchNone');
  if (kind === 'island') {
    const i = world.islands.find((x) => x.name === name);
    return i ? t('world.info.island', { name: i.name, tools: i.tools.join(', ') }) : '';
  }
  if (kind === 'gate') return t('world.info.gate');
  if (kind === 'stable') return t('world.info.stable');
  return '';
}

/** The picture of the world for a view, at a frame of the clock. */
export function drawWorld(world: World, o: SceneOptions): Drawn {
  const small = o.w < WORLD_MIN_W || o.h < WORLD_MIN_H;
  // With art the ranch has its own layout: it can be twice as large on a wide screen.
  const plan: RanchPlan | null = o.mode !== 'none' && !small && o.view.level === 'ranch' ? planRanch(world, o.w, o.h, o.view.sel) : null;
  const house = world.houses.find((x) => x.key === o.view.house);
  const building = o.mode !== 'none' && !small && o.view.level === 'building' && house ? planBuilding(house, o.w, o.h, o.view.sel, o.view.plumbing) : null;
  const inRoom = o.view.level === 'room' ? world.houses.find((x) => x.rooms.some((r) => r.id === o.view.room)) : undefined;
  const roomPlan = o.mode !== 'none' && !small && inRoom ? planRoom(o.w, o.h, inRoom.loft !== '') : null;
  // The loft is the same cutaway as the room, one floor up.
  const loftPlan = o.mode !== 'none' && !small && o.view.level === 'loft' && house?.loft ? planRoom(o.w, o.h, true) : null;
  const c = new Canvas(o.w, o.h, { mode: o.mode, plain: o.plain, light: o.light, scale: plan?.scale ?? building?.scale ?? roomPlan?.scale ?? loftPlan?.scale ?? 1 });
  const ctx: Ctx = { c, world, o, hits: [], loops: false, t: o.t ?? now(), note: 0 };
  if (small) {
    c.text(0, 0, cut(t('world.small', { columns: WORLD_MIN_W, rows: WORLD_MIN_H }), o.w), DIM);
    return { canvas: c, hits: [], loops: false };
  }
  switch (o.view.level) {
    case 'ranch':
      if (plan) drawRanchArt(ctx, plan);
      else drawRanch(ctx);
      break;
    case 'building':
      if (building && house) drawBuildingArt(ctx, house, building);
      else drawBuilding(ctx);
      break;
    case 'room':
      drawRoom(ctx, roomPlan);
      break;
    case 'loft':
      drawLoft(ctx, loftPlan);
      break;
  }
  // The top line: where you are, and what waits for you (amber, always).
  const crumb = breadcrumb(world, o.view);
  const asks = world.waiting > 0 ? t('world.top.asks', { n: world.waiting }) : '';
  const mode = o.view.plumbing ? t('world.top.plumbing') : '';
  c.clearText(0, 0, o.w, 1);
  c.text(0, 0, cut(crumb, Math.max(0, o.w - cells(asks) - cells(mode) - 4)), { bold: true });
  if (mode) c.text(o.w - cells(mode) - (asks ? cells(asks) + 2 : 0), 0, mode, DIM);
  if (asks) c.text(o.w - cells(asks), 0, asks, c.art ? { fg: px.amberText(c.light), bold: true } : BOLD);
  const info = infoLine(world, o.view, ctx.t);
  // The line of words fits the width: it ends with an ellipsis, two cells before the note of the scroll.
  if (info) c.text(0, o.h - 1, cut(info, Math.max(0, o.w - ctx.note)), {});
  return { canvas: c, hits: ctx.hits, loops: ctx.loops };
}

/** The thing to select after a move in a direction: the nearest one that lies that way. */
export function moveSelection(hits: Hit[], sel: string, dx: number, dy: number): string {
  const cur = hits.find((h) => h.id === sel);
  if (!cur) return hits[0]?.id ?? sel;
  const cx = cur.x + cur.w / 2;
  const cy = cur.y + cur.h / 2;
  let best: Hit | null = null;
  let bestScore = Infinity;
  for (const h of hits) {
    if (h.id === cur.id) continue;
    const ox = h.x + h.w / 2 - cx;
    // A row of cells is about twice as high as a column is wide.
    const oy = (h.y + h.h / 2 - cy) * 2;
    const along = dx !== 0 ? ox * dx : oy * dy;
    const across = dx !== 0 ? Math.abs(oy) : Math.abs(ox);
    if (along <= 0.5) continue;
    const score = along + across * 2.5;
    if (score < bestScore) {
      bestScore = score;
      best = h;
    }
  }
  return best ? best.id : sel;
}
