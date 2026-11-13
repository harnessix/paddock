// A building with art: the rooms of one project in the middle of the
// picture, at twice their size on a wide screen. The loft is the attic
// over the rooms, under the roof, with the ladder in the hall beside
// them. Words stand under the rooms and beside the building, never on it.

import { cells, cut } from '../clean.js';
import { t } from '../i18n/index.js';
import { age } from '../model.js';
import { atDoor, call, character, DIM, doorOf, flagUp, flakeBadge, frost, frostCorners, hit, hop, hostTools, NIGHT_MAP, PICKED, roof, roomShell, roomWord, scrollNote, selected, sideWall, SIGN, trip, windowOf, writing, type Ctx } from './kit.js';
import * as art from './layers.js';
import { lerp } from './motion.js';
import { mix } from './canvas.js';
import { netWord, type House, type Room } from './model.js';
import * as px from './pixels.js';

/** A room in the building: 33 x 28 pixels, the character at its full size. */
const ROOM_W = 33;
const ROOM_H = 28;
/** The hall with the ladder to the loft and the sockets of the shared tools. */
const HALL = 7;
const DEPTH = 4;
/** From this width on the building can be twice as large. */
const DOUBLE_W = 150;

/** The heights of the roof and of the attic, and whether the sign of the house shows: from the roomiest to the most compact. */
const DETAILS: readonly (readonly [number, number, number])[] = [
  [6, 12, 1],
  [4, 10, 1],
  [4, 8, 1],
  [2, 8, 1],
  [2, 6, 1],
  [2, 6, 0],
];

export interface BuildingPlan {
  scale: number;
  lw: number;
  perFloor: number;
  /** The floors that show, the first of them, and how many there are. */
  floors: number;
  first: number;
  total: number;
  hall: number;
  roof: number;
  attic: number;
  sign: boolean;
  /** The height of a room, and the rows of words under a floor, in pixels. */
  room: number;
  words: number;
  /** The left edge of the building and its first row of pixels. */
  x: number;
  y: number;
}

function layout(h: House, w: number, hgt: number, s: number, sel: string, plumbing: boolean, scroll: boolean): BuildingPlan | null {
  const lw = Math.floor(w / s);
  const row = Math.max(1, Math.ceil(2 / s));
  const top = row;
  const P = Math.floor(((hgt - 1) * 2) / s) - top;
  const hall = h.loft || h.hall.length > 0 ? HALL : 0;
  const count = Math.max(1, h.rooms.length);
  const perFloor = Math.min(count, Math.floor((lw - 2 - hall - DEPTH) / ROOM_W));
  if (perFloor < 1) return null;
  const total = Math.ceil(count / perFloor);
  // The words under a floor: the name and the state of each run, and with Plumbing what leaves its room.
  const words = (plumbing ? 2 : 1) * row;
  const fit = (roofH: number, attic: number, sign: number, floors: number, room: number): boolean => roofH + attic + floors * (room + words) + sign * row <= P;
  // A project without a loft says so on its sign: the sign always shows.
  const details = h.loft ? DETAILS : DETAILS.filter(([, , sg]) => sg === 1);
  let detail = details.find(([r, a, sg]) => fit(r, h.loft ? a : 0, sg, total, ROOM_H));
  let floors = total;
  let room = ROOM_H;
  if (!detail) {
    if (!scroll) return null;
    detail = details[details.length - 1] as readonly [number, number, number];
    const [r, a, sg] = detail;
    const attic = h.loft ? a : 0;
    floors = Math.max(1, Math.floor((P - r - attic - sg * row) / (ROOM_H + words)));
    // A screen too low for one room: the room is as high as the screen has rows.
    if (!fit(r, attic, sg, 1, ROOM_H)) room = Math.max(12, Math.floor((P - r - attic - words - sg * row) / 2) * 2);
  }
  const [roofH, a, sg] = detail;
  const attic = h.loft ? a : 0;
  const selIndex = Math.max(0, h.rooms.findIndex((r) => `room:${r.id}` === sel));
  const selFloor = Math.floor(selIndex / perFloor);
  const first = Math.max(0, Math.min(selFloor, total - floors, selFloor - floors + 1 > 0 ? selFloor - floors + 1 : 0));
  const high = roofH + attic + floors * (room + words) + sg * row;
  const wide = hall + perFloor * ROOM_W + DEPTH;
  return {
    scale: s,
    lw,
    perFloor,
    floors,
    first: selFloor < first ? selFloor : first,
    total,
    hall,
    roof: roofH,
    attic,
    sign: sg === 1,
    room,
    words,
    x: Math.max(1, Math.floor((lw - wide) / 2)),
    y: top + Math.max(0, Math.floor((P - high) / 2 / row) * row),
  };
}

/** The building for a screen: twice as large when all its rooms fit so. */
export function planBuilding(h: House, w: number, hgt: number, sel: string, plumbing: boolean): BuildingPlan {
  const big = w >= DOUBLE_W ? layout(h, w, hgt, 2, sel, plumbing, false) : null;
  return big ?? (layout(h, w, hgt, 1, sel, plumbing, true) as BuildingPlan);
}

function roomArt(ctx: Ctx, r: Room, X: number, Y: number, H: number, sel: boolean): void {
  const { c } = ctx;
  const w = ROOM_W;
  const f = frost(ctx, r);
  const door = atDoor(ctx, r);
  c.blit(roomShell(ctx, '33x28', r, w, H, f, sel), X, Y);
  // In a low window the room is low: what stands in it stays inside its walls.
  c.setClip({ x: X, y: Y + 1, w: w + 1, h: H - 2 });
  if (r.run.live) c.blit(windowOf(ctx, r), X + 2, Y + 2);
  // The door, on the left wall. Amber while a request waits.
  c.blit(art.pic(doorOf(ctx, r), ctx.o.tick), X, Y + H - 17, r.run.live ? undefined : NIGHT_MAP);
  const ch = character(ctx, r);
  // A character that writes a note left its room: it is on the ladder of the hall, or in the loft.
  if (trip(ctx, r).where === 'room') c.blit(ch.frame, X + lerp(7, 4, door), Y + H - 26);
  if (r.asks.length > 0 && door >= 1) c.blit(art.pic('card'), X + 3, Y + H - 16);
  // Unread messages: the flag of the mailbox goes up at the right wall.
  if (r.unread > 0) c.blit(art.part('mailbox-up', 0, 4, 0, 2, 5), X + w - 5, Y + H - lerp(12, 15, flagUp(ctx, r)) - hop(ctx, r));
  // A tool on your machine: an amber socket in the right wall, and its plug outside. It always shows.
  if (hostTools(r).length > 0) {
    // A call of the tool: its socket is lit and its plug is white for the time of the spark. With Reduce Motion the plug is bright.
    const calls = hostTools(r).map((t) => call(ctx, r, t.name));
    const spark = calls.some((k) => k.spark !== null);
    c.blit(spark ? art.tint('socket-host', 'glow') : art.pic('socket-host'), X + w - 3, Y + H - 16);
    c.fill(X + w, Y + H - 15, 1, 2, spark ? px.WHITE : calls.some((k) => k.bright) ? mix(px.AMBER, px.WHITE, 0.6) : px.AMBER);
  }
  c.setClip(null);
  frostCorners(c, X + 1, Y + 1, w - 2, H - 2, Math.round(f * 4));
  flakeBadge(ctx, r, X + w - 13, Y + 9);
}

export function drawBuildingArt(ctx: Ctx, h: House, plan: BuildingPlan): void {
  const { c, o } = ctx;
  const s = plan.scale;
  const col = (x: number): number => x * s;
  const rowOf = (y: number): number => Math.floor((y * s) / 2);
  const X0 = plan.x;
  const fw = plan.hall + plan.perFloor * ROOM_W;
  const eave = plan.y + plan.roof;
  const floorTop = (f: number): number => eave + plan.attic + f * (plan.room + plan.words);
  const right = col(X0 + fw + DEPTH + 2);
  const beside = Math.max(0, o.w - right - 1);
  const night = h.rooms.length === 0 || h.rooms.every((r) => !r.run.live);

  // The attic, under the roof: the loft of the project, with its shelf of notebooks.
  const loftId = `loft:${h.key}`;
  if (plan.attic > 0) {
    const sel = selected(ctx, loftId);
    c.blit(art.tiles('tile-wall', fw, plan.attic), X0, eave);
    c.outline(X0, eave, fw, plan.attic + 1, sel ? px.SELECT : px.INK);
    // How many notebooks there are, paddock does not report: the shelf is a picture, not a count.
    const slide = o.motion.progress('note', h.key, o.tick);
    const n = Math.max(0, Math.min(14, Math.floor((fw - plan.hall - 8) / 2)));
    c.setClip({ x: X0 + 1, y: eave + 1, w: fw - 2, h: plan.attic - 1 });
    for (let i = 0; i < n; i++) c.blit(art.pic(`spine-${i % 6}`), X0 + plan.hall + 4 + i * 2 + (i === 0 ? lerp(-6, 0, slide) : 0), eave + plan.attic - 6);
    // A character that writes a note: small, in the loft beside the shelf.
    h.rooms.filter((r) => trip(ctx, r).where === 'loft').forEach((r, i) => c.blit(writing(ctx, r).small, X0 + plan.hall + 6 + n * 2 + i * 9, eave + plan.attic - 8));
    c.setClip(null);
    hit(ctx, { id: loftId, kind: 'loft', x: col(X0), y: rowOf(eave), w: col(fw), h: Math.max(1, rowOf(eave + plan.attic) - rowOf(eave)), house: h.key });
    // Its name stands beside it.
    const pulse = o.motion.running('pulse', 'world', o.tick) && (o.tick & 2) === 0;
    const full = t('world.building.loftFull');
    const label = beside >= cells(full) + 1 ? full : t('world.building.loft');
    if (beside >= cells(label)) c.text(right, rowOf(eave + plan.attic / 2), label, sel ? PICKED : { bold: true, inverse: pulse });
  }


  const shown = Math.min(plan.floors, plan.total - plan.first);
  const depth = plan.roof <= 2 ? 1 : DEPTH;
  for (let f = 0; f < shown; f++) {
    const Y = floorTop(f);
    const rooms = h.rooms.slice((plan.first + f) * plan.perFloor, (plan.first + f + 1) * plan.perFloor);
    const used = plan.hall + Math.max(1, rooms.length) * ROOM_W;
    // The side wall: under the roof it goes up past the attic.
    const up = f === 0 ? plan.attic : 0;
    sideWall(c, X0 + (up > 0 ? fw : used), Y - up, plan.room + up, depth, night);
    // The hall: the ladder comes down from the loft, through every floor.
    if (plan.hall > 0) {
      c.fill(X0, Y, plan.hall, plan.room, px.CREAM_SHADE);
      c.fill(X0 + 1, Y + plan.room - 3, plan.hall - 1, 2, px.FLOOR_DARK);
      c.outline(X0, Y, plan.hall + 1, plan.room, px.INK);
      if (h.loft) {
        const lit = selected(ctx, loftId);
        c.setClip({ x: X0 + 1, y: f === 0 ? eave + 1 : Y + 1, w: plan.hall - 1, h: (f === 0 ? Y - eave : 0) + plan.room - 4 });
        for (let ly = Y + plan.room - 7; ly > (f === 0 ? eave - 4 : Y - 4); ly -= 4) c.blit(lit ? art.tint('ladder', 'glow') : art.pic('ladder'), X0 + 1, ly);
        c.setClip(null);
        // A character on its way to the loft: small, on the ladder, from the floor of its room up to the attic.
        for (const r of rooms) {
          const way = trip(ctx, r);
          if (way.where !== 'ladder') continue;
          c.setClip({ x: X0, y: eave + 1, w: plan.hall + 1, h: Y + plan.room - eave - 2 });
          c.blit(character(ctx, r).small, X0, lerp(Y + plan.room - 11, eave + plan.attic - 8, way.p));
          c.setClip(null);
        }
      }
    }
    if (rooms.length === 0) {
      // A project without a run: one closed room, its light off.
      c.blit(art.shell('33x28', 'night', ROOM_W, plan.room), X0 + plan.hall, Y);
      c.blit(art.pic('door-closed'), X0 + plan.hall, Y + plan.room - 17, NIGHT_MAP);
      c.blit(art.pic('window-none'), X0 + plan.hall + ROOM_W - 10, Y + 3, NIGHT_MAP);
      c.text(col(X0 + plan.hall), rowOf(Y + plan.room), cut(t('world.building.noRun'), o.w - col(X0 + plan.hall) - 1), DIM);
    }
    rooms.forEach((r, i) => {
      const X = X0 + plan.hall + i * ROOM_W;
      const id = `room:${r.id}`;
      const sel = selected(ctx, id);
      roomArt(ctx, r, X, Y, plan.room, sel);
      const capRow = rowOf(Y + plan.room);
      hit(ctx, { id, kind: 'room', x: col(X), y: rowOf(Y), w: col(ROOM_W), h: capRow - rowOf(Y) + 1, house: h.key, room: r.id });
      // The words under the room: its name and its state. A helper has the mark of its path. The time shows when it fits.
      const word = roomWord(r);
      const W = col(ROOM_W) - 1;
      const since = r.run.stateSince > 0 ? ` · ${age(ctx.t - r.run.stateSince)}` : '';
      const short = `${sel ? '› ' : ''}${r.parent ? '↳ ' : ''}${r.name} ${word.glyph} ${word.word}`;
      const cap = cut(cells(short + since) <= W ? short + since : short, W);
      c.text(col(X), capRow, cap, sel ? PICKED : { bold: word.amber, ...(word.amber ? { fg: px.amberText(c.light) } : {}), dim: !r.run.live });
      if (plan.words > Math.ceil(2 / s) && r.run.live) {
        // Plumbing: what leaves the room.
        const tools = r.tools.length;
        c.text(col(X), capRow + 1, cut(tools > 0 ? t('world.building.netTools', { net: netWord(r.net), n: tools }) : t('world.building.net', { net: netWord(r.net) }), W), DIM);
      }
    });
  }

  roof(c, X0, eave, fw, plan.roof, depth);

  // The tools of the hall: one socket each in the outer wall, amber for a tool on your machine. Its name stands beside it.
  if (plan.hall > 0) {
    const Y = floorTop(0);
    h.hall.slice(0, 3).forEach((t, i) => {
      const y = Y + 3 + i * 7;
      if (y + 5 >= Y + plan.room - 3) return;
      const id = `tool:${t.name}`;
      const sel = selected(ctx, id);
      const name = t.run === 'host' ? 'socket-host' : t.ask ? 'socket-cover' : 'socket-open';
      // A room calls the tool of the hall: its socket is lit for the time of the spark.
      const on = h.rooms.map((r) => call(ctx, r, t.name)).some((k) => k.spark !== null || k.bright || k.rise !== null);
      c.blit(on ? art.tint(name, 'glow') : sel ? art.tint(name, 'select') : art.pic(name), X0 - 1, y);
      const room = Math.max(0, col(X0 - 2) - 1);
      const label = o.view.plumbing || sel || t.run === 'host' ? cut(t.name, room) : '';
      const lx = col(X0 - 2) - cells(label);
      hit(ctx, { id, kind: 'tool', x: Math.max(0, lx), y: rowOf(y), w: col(X0 + plan.hall) - Math.max(0, lx), h: Math.max(1, rowOf(y + 4) - rowOf(y) + 1), house: h.key, name: t.name });
      if (label) c.text(lx, rowOf(y + 1), label, sel ? PICKED : t.run === 'host' ? { fg: px.amberText(c.light), bold: true } : {});
    });
  }

  // The sign of the house, under it.
  if (plan.sign) {
    // A project without a loft: its sign says so, in small words.
    const none = h.loft ? '' : `  ${t('world.noNotes')}`;
    const plate = ` ${cut(h.name, Math.max(1, col(fw) - 2 - cells(none)))} `;
    const sx = col(X0) + Math.max(0, Math.floor((col(fw) - cells(plate) - cells(none)) / 2));
    c.text(sx, rowOf(floorTop(shown)), plate, SIGN);
    if (none) c.text(sx + cells(plate), rowOf(floorTop(shown)), none, DIM);
  }
  scrollNote(ctx, plan.first > 0, plan.total - plan.first - shown, 'floor');
}
