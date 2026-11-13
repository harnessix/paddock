// What changed in the world between two reads of paddock, as motions: a
// card that came (the character walks to the door), a run that froze
// (frost grows), a helper that started (its room builds) or ended (its
// envelope travels), a tool that started (a spark on its cable), a count
// of shared notes that changed (a tag flips). paddock has no event for
// these: the interface sees the change in the next read, and the motion
// starts then.
//
// Three motions wait for data that paddock does not report yet: the hop
// of the mailbox (`unread_ask` in `mail ls --json`), the climb to the
// loft (`loft_commits` in the board) and a spark for each call of a tool
// (device calls in the event stream, see `deviceCall`). Each starts by
// itself when its field arrives.

import type { Motion } from './motion.js';
import { findRoom, type House, type Room, type World } from './model.js';

function roomsOf(w: World): Map<string, Room> {
  const m = new Map<string, Room>();
  for (const h of w.houses) for (const r of h.rooms) m.set(r.id, r);
  return m;
}

/**
 * Start the motions of the changes from `prev` to `next` at a frame of
 * the clock. With no world before, nothing changed and nothing moves.
 */
export function observe(motion: Motion, prev: World | null, next: World, tick: number): void {
  if (!prev) return;
  const before = roomsOf(prev);
  for (const r of roomsOf(next).values()) {
    const old = before.get(r.id);
    if (!old) {
      if (r.parent && r.run.live) motion.start('spawn', r.id, tick);
      if (r.yard && r.run.live) motion.start('join', `${r.yard}:${r.id}`, tick);
      continue;
    }
    if (old.asks.length === 0 && r.asks.length > 0) motion.start('ask', r.id, tick);
    if (old.asks.length > 0 && r.asks.length === 0) motion.start('answer', r.id, tick);
    if (!old.run.frozen && r.run.frozen) motion.start('freeze', r.id, tick);
    if (old.run.frozen && !r.run.frozen) motion.start('thaw', r.id, tick);
    if (old.run.live && r.run.live && old.net !== r.net) motion.start('net', r.id, tick, old.net);
    // The flag goes up for the first unread message. Only a question makes the box hop.
    if (r.unread > old.unread) motion.start('mail', r.id, tick);
    if (r.unreadAsk !== null && r.unreadAsk > (old.unreadAsk ?? 0)) motion.start('hop', r.id, tick);
    // An instance of a tool that runs now and did not before: the room called the tool.
    for (const t of r.tools) {
      const was = old.tools.find((x) => x.name === t.name);
      if (was && !was.on && t.on) motion.called(r.id, t.name, t.ask, tick);
    }
    // An apply: paddock counts one more applied file.
    if ((r.copy?.applied ?? 0) > (old.copy?.applied ?? 0)) motion.start('apply', r.id, tick);
    if (r.yard && old.yard !== r.yard) motion.start('join', `${r.yard}:${r.id}`, tick);
    // A helper that ended: its result goes to the run that started it.
    if (old.run.live && !r.run.live && old.parent) motion.end(r.id, old.parent, tick);
  }
  // The shared notes: a count that changed is a note that was written.
  const count = (keys: number | null, leases: number | null): string | null => (keys === null ? null : `${keys}:${leases ?? 0}`);
  const gate = (w: World): string | null => (w.gate.keys === null ? null : `${count(w.gate.keys, w.gate.leases)}|${w.gate.cabinets.map((c) => `${c.name}:${c.keys}:${c.leases}`).join(',')}`);
  if (gate(prev) !== null && gate(next) !== null && gate(prev) !== gate(next)) motion.start('flip', 'gate', tick);
  const houses = new Map<string, House>(prev.houses.map((h) => [h.key, h]));
  for (const h of next.houses) {
    const old = houses.get(h.key);
    if (!old) continue;
    for (const y of h.yards) {
      const was = old.yards.find((x) => x.name === y.name);
      const a = was ? count(was.keys, was.leases) : null;
      const b = count(y.keys, y.leases);
      if (a !== null && b !== null && a !== b) motion.start('flip', `yard:${h.key}:${y.name}`, tick);
    }
    // The loft has one change more: a note was written.
    if (old.notes !== null && h.notes !== null && h.notes > old.notes) motion.wrote(h.key, writer(h), tick);
  }
  const seats = new Set(prev.bench.seats.map((s) => `${s.slot}:${s.holder}`));
  for (const s of next.bench.seats) if (!seats.has(`${s.slot}:${s.holder}`)) motion.start('slot', String(s.slot), tick);
}

/**
 * Who wrote a note in the loft of a house: its one run that works. With
 * two or more, paddock does not say which: no character climbs, and only
 * the notebook moves.
 */
export function writer(h: House): string | null {
  const at = h.rooms.filter((r) => r.run.live && !r.run.frozen);
  return at.length === 1 ? (at[0] as Room).id : null;
}

/** A call of a tool, as an event stream of paddock will report it. */
export interface DeviceCall {
  sandbox: string;
  device: string;
  /** `ask`: the call waits for you. Without it, the mode of the tool decides. */
  mode?: string;
}

/**
 * A room called a tool: the spark runs along the cable of the tool. For
 * the day when `paddock events --follow --json` has device calls. Until
 * then, `observe` starts the spark when an instance of a tool starts.
 */
export function deviceCall(motion: Motion, world: World, call: DeviceCall, tick: number): boolean {
  const room = findRoom(world, call.sandbox)?.room;
  const tool = room?.tools.find((t) => t.name === call.device);
  if (!room || !tool) return false;
  motion.called(room.id, tool.name, call.mode !== undefined ? call.mode === 'ask' : tool.ask, tick);
  return true;
}

/** A note was written in the loft of a project, as an event stream of paddock will report it. */
export function loftWrite(motion: Motion, world: World, project: string, sandbox: string | null, tick: number): boolean {
  const h = world.houses.find((x) => x.key === project || x.path === project);
  if (!h || !h.loft) return false;
  const room = sandbox !== null && h.rooms.some((r) => r.id === sandbox) ? sandbox : writer(h);
  motion.wrote(h.key, room, tick);
  return true;
}
