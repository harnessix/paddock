// The world, put together from paddock's JSON: the ranch holds all
// projects, a house is one project, a room is one run, the loft is the
// attic of a house. Nothing here is decided by the interface: each fact
// comes from a field of a paddock command, and a fact that paddock does
// not report (a gap) is left out, never guessed.

import { oneLine } from '../clean.js';
import { t } from '../i18n/index.js';
import type { RunRow } from '../model.js';
import type { CopyRun, Counts } from '../copy.js';
import type { Board, DeviceDef, DeviceList, FilesList, Forage, HorseSession, KvOverview, Pending, Ps, PsSandbox, TimerStatus } from '../types.js';

/** The modes of `network` in `paddock ps --json`. `ask` is an app that asks for each connection. */
export type NetMode = 'none' | 'api' | 'allowlist' | 'egress' | 'host' | 'ask' | 'unknown';

const NET: readonly NetMode[] = ['none', 'api', 'allowlist', 'egress', 'host', 'ask'];

export function netMode(raw: string): NetMode {
  return (NET as readonly string[]).includes(raw) ? (raw as NetMode) : 'unknown';
}

/** The network mode in one sentence, in the language of the interface. */
export function netSentence(m: NetMode): string {
  return t(`world.net.sentence.${m}`);
}

/** The network mode in a few words, in the language of the interface. */
export function netWord(m: NetMode): string {
  return t(`world.net.word.${m}`);
}

/** Where a tool runs: inside the room, on your machine, or on another computer. */
export type ToolRun = 'sandbox' | 'host' | 'remote' | 'broker' | 'unknown';

export interface Tool {
  name: string;
  run: ToolRun;
  /** The other computer of a remote tool. */
  peer: string;
  shared: boolean;
  /** Each call waits for you. */
  ask: boolean;
  /** An instance of the tool runs for this room now (ps). */
  on: boolean;
  def: DeviceDef;
}

export interface Room {
  id: string;
  short: string;
  /** "claude-71c3ac". */
  name: string;
  run: RunRow;
  /** The requests of this run that wait for you. */
  asks: Pending[];
  net: NetMode;
  netRaw: string;
  /** Messages of the run that you did not read. */
  unread: number;
  /** Unread messages that are a question, or null: paddock did not say. */
  unreadAsk: number | null;
  frozenReason: string;
  /** The run that started this one (a helper), or ''. */
  parent: string;
  /** The helpers that this run started and that still show. */
  children: string[];
  /** The name of the shared yard, or ''. */
  yard: string;
  specTask: string;
  /** Files that the run changed in its project, or null: paddock did not say. */
  changed: number | null;
  changedFiles: string[];
  /** The counts of the copy of the run, or null: the run has no copy, or paddock did not say. */
  copy?: Counts | null;
  tools: Tool[];
  timers: TimerStatus[];
  /** The row of `paddock ps --json`, when the run is live. */
  ps?: PsSandbox;
}

export interface Ticket {
  id: string;
  title: string;
  state: string;
  runs: string[];
}

export interface House {
  /** The path of the project: the key of the house. */
  key: string;
  name: string;
  path: string;
  rooms: Room[];
  tickets: Ticket[];
  /** The folder of the loft's notes, or '': the project has no loft. */
  loft: string;
  /** How many changes the notes of the loft have, or null: paddock did not say. */
  notes: number | null;
  /** The tools that stand in the hall: one instance for every room. */
  hall: Tool[];
  yards: Yard[];
}

export interface Yard {
  name: string;
  project: string;
  /** The ids of the rooms that the fence joins. */
  members: string[];
  /** Notes and claims on the fence, or null: `kv overview` was not read. */
  keys: number | null;
  leases: number | null;
  /** Shared ports: a small gate each. */
  ports: { port: number; sandbox: string }[];
}

export interface Bench {
  /** The stools that are taken. paddock does not report how many exist. */
  seats: { slot: number; holder: string }[];
}

export interface Island {
  /** The host of the other computer. */
  name: string;
  target: string;
  /** The tools that run there. */
  tools: string[];
}

export interface Gate {
  /** Notes of the user scope, or null: not read. */
  keys: number | null;
  leases: number | null;
  /** Cabinets with a lock: the named namespaces. */
  cabinets: { name: string; keys: number; leases: number }[];
  /** The feed trough: the use of your accounts, or null. */
  forage: Forage['windows'] | null;
}

export interface Stable {
  /** One stall per horse, each with its saved sessions, the newest first. */
  stalls: { horse: string; sessions: HorseSession[] }[];
}

export interface World {
  houses: House[];
  bench: Bench;
  islands: Island[];
  gate: Gate;
  /** Hidden until horse mode has a saved session. */
  stable: Stable | null;
  /** Requests that wait for you, in all rooms and apps. */
  waiting: number;
}

/** What paddock reported besides ps, board, approvals and mail. Each part can be missing. */
export interface WorldExtra {
  devices?: DeviceList;
  kv?: KvOverview;
  files?: FilesList;
  forage?: Forage;
  horses?: HorseSession[];
  /** The runs that work on a copy, with the counts of their changes. */
  copies?: CopyRun[];
}

function baseName(p: string): string {
  const parts = p.split('/').filter((x) => x.length > 0);
  return parts[parts.length - 1] ?? p;
}

/** The host of `ssh://HOST` or `unix:/PATH`. */
export function peerName(target: string): string {
  const tt = oneLine(target);
  const m = /^[a-z]+:\/\/(?:[^@/]*@)?([^/:]+)/i.exec(tt);
  if (m) return m[1] as string;
  return baseName(tt.replace(/^[a-z]+:/i, '')) || tt || t('world.model.otherComputer');
}

function toolRun(d: DeviceDef): ToolRun {
  const r = d.run?.run;
  return r === 'sandbox' || r === 'host' || r === 'remote' || r === 'broker' ? r : 'unknown';
}

/**
 * Whether a room can reach a tool, by the grant of the tool. paddock has
 * no command that lists the tools of one run, so this compares the grant
 * with what ps says of the run. No grant: every sandbox.
 */
export function granted(d: DeviceDef, r: { harness: string; project: string; yard: string }): boolean {
  if (!d.grant || d.grant.length === 0) return true;
  return d.grant.some((g) => {
    if (g.grant === 'all') return true;
    if (g.grant === 'harness') return g.to === r.harness;
    if (g.grant === 'pasture') return r.yard !== '' && g.to === r.yard;
    if (g.grant === 'project') return g.to === r.project || g.to === baseName(r.project);
    return false;
  });
}

function toolOf(d: DeviceDef, on: boolean): Tool {
  const run = toolRun(d);
  return {
    name: oneLine(d.name),
    run,
    peer: run === 'remote' ? peerName(d.run?.target ?? '') : '',
    shared: d.scope === 'shared',
    ask: d.mode === 'ask',
    on,
    def: d,
  };
}

/** Tools in the order of what leaves the room: your machine first, then other computers, then the room's own. */
function toolOrder(t: Tool): number {
  return t.run === 'host' ? 0 : t.run === 'remote' ? 1 : t.run === 'broker' ? 2 : 3;
}

export function buildWorld(runs: RunRow[], ps: Ps | undefined, board: Board | undefined, pending: Pending[], extra: WorldExtra = {}): World {
  const psRows = new Map<string, PsSandbox>();
  for (const s of ps?.sandboxes ?? []) psRows.set(s.id, s);
  const devices = (extra.devices?.devices ?? []).filter((d) => d.enabled !== false);
  const sharedOn = new Set((ps?.shared_devices ?? []).map((i) => oneLine(i.device ?? '')));
  const files = new Map((extra.files?.sandboxes ?? []).map((f) => [f.id, f]));

  const houses = new Map<string, House>();
  const house = (path: string, name?: string): House => {
    const key = path || name || '?';
    let h = houses.get(key);
    if (!h) {
      h = { key, name: oneLine(name ?? '') || baseName(path) || '?', path, rooms: [], tickets: [], loft: '', notes: null, hall: [], yards: [] };
      houses.set(key, h);
    }
    return h;
  };
  // The board gives the order of the projects, their tickets and their lofts.
  for (const p of board?.projects ?? []) {
    const h = house(oneLine(p.path ?? ''), p.name);
    h.loft = oneLine(p.loft ?? '');
    h.notes = typeof p.loft_commits === 'number' ? p.loft_commits : null;
    h.tickets = (p.tickets ?? []).map((t) => ({ id: oneLine(t.id), title: oneLine(t.title ?? ''), state: oneLine(t.state ?? ''), runs: (t.runs ?? []).map((r) => oneLine(r.id)) }));
  }

  const copyOf = new Map((extra.copies ?? []).map((c) => [c.id, c.counts ?? null]));
  const rooms = new Map<string, Room>();
  for (const r of runs) {
    const s = psRows.get(r.id);
    const yard = oneLine(s?.pasture ?? '');
    const on = new Set((s?.devices ?? []).map((i) => oneLine(i.device ?? '')));
    const tools = r.live
      ? devices
          .filter((d) => granted(d, { harness: r.harness, project: r.project, yard }))
          .map((d) => toolOf(d, on.has(oneLine(d.name)) || (d.scope === 'shared' && sharedOn.has(oneLine(d.name)))))
          .sort((a, b) => toolOrder(a) - toolOrder(b) || a.name.localeCompare(b.name))
      : [];
    const f = files.get(r.id);
    const changedFiles = (f?.project_changes?.changed ?? []).map(oneLine);
    const room: Room = {
      id: r.id,
      short: r.short,
      name: `${r.harness}-${r.short}`,
      run: r,
      asks: pending.filter((p) => p.sandbox === r.id),
      net: r.live ? netMode(r.network) : 'unknown',
      netRaw: r.network,
      unread: r.unreadMail,
      unreadAsk: r.unreadAsk ?? null,
      frozenReason: oneLine(s?.frozen_reason ?? ''),
      parent: r.parent,
      children: [],
      yard,
      specTask: oneLine(s?.spec_task ?? ''),
      changed: f?.project_changes ? changedFiles.length : null,
      changedFiles,
      copy: copyOf.get(r.id) ?? null,
      tools,
      timers: s?.timers ?? [],
      ps: s,
    };
    rooms.set(r.id, room);
    house(r.project).rooms.push(room);
  }
  for (const r of rooms.values()) if (r.parent) rooms.get(r.parent)?.children.push(r.id);

  const kvYards = new Map<string, KvOverview['pastures'][number]>();
  for (const p of extra.kv?.pastures ?? []) if (!p.solo) kvYards.set(`${oneLine(p.project ?? '')}\n${oneLine(p.name)}`, p);

  for (const h of houses.values()) {
    // Live rooms first, a helper right after the run that started it; ended rooms last.
    const live = h.rooms.filter((r) => r.run.live).sort((a, b) => a.run.since - b.run.since || a.id.localeCompare(b.id));
    const ended = h.rooms.filter((r) => !r.run.live).sort((a, b) => b.run.since - a.run.since || a.id.localeCompare(b.id));
    const ordered: Room[] = [];
    const add = (r: Room): void => {
      if (ordered.includes(r)) return;
      ordered.push(r);
      for (const c of live) if (c.parent === r.id) add(c);
    };
    for (const r of live) if (!r.parent || !rooms.has(r.parent) || rooms.get(r.parent)?.run.project !== r.run.project) add(r);
    for (const r of live) add(r);
    h.rooms = [...ordered, ...ended];
    // A yard: the live rooms of the house that ps puts in the same named pasture.
    const names = [...new Set(live.map((r) => r.yard).filter((y) => y.length > 0))];
    h.yards = names.map((name) => {
      const kv = kvYards.get(`${h.path}\n${name}`);
      return {
        name,
        project: h.path,
        members: live.filter((r) => r.yard === name).map((r) => r.id),
        keys: kv ? (kv.keys ?? 0) : null,
        leases: kv ? (kv.leases ?? 0) : null,
        ports: (kv?.ports ?? []).map((p) => ({ port: p.port ?? 0, sandbox: oneLine(p.sandbox ?? '') })),
      };
    });
    // The hall: shared tools that a room of the house can reach.
    const hall = new Map<string, Tool>();
    for (const r of live) for (const t of r.tools) if (t.shared && !hall.has(t.name)) hall.set(t.name, t);
    h.hall = [...hall.values()];
  }

  const islands = new Map<string, Island>();
  for (const d of devices) {
    if (toolRun(d) !== 'remote') continue;
    const name = peerName(d.run?.target ?? '');
    const i = islands.get(name) ?? { name, target: oneLine(d.run?.target ?? ''), tools: [] };
    i.tools.push(oneLine(d.name));
    islands.set(name, i);
  }

  const stalls = new Map<string, HorseSession[]>();
  for (const s of extra.horses ?? []) {
    const k = oneLine(s.horse ?? '?');
    stalls.set(k, [...(stalls.get(k) ?? []), s]);
  }
  for (const list of stalls.values()) list.sort((a, b) => (b.created_ms ?? 0) - (a.created_ms ?? 0));

  return {
    houses: [...houses.values()],
    bench: { seats: (board?.compile_slots ?? []).map((s) => ({ slot: s.slot, holder: oneLine(s.holder) })).sort((a, b) => a.slot - b.slot) },
    islands: [...islands.values()].sort((a, b) => a.name.localeCompare(b.name)),
    gate: {
      keys: extra.kv ? (extra.kv.user?.keys ?? 0) : null,
      leases: extra.kv ? (extra.kv.user?.leases ?? 0) : null,
      cabinets: (extra.kv?.namespaces ?? []).map((n) => ({ name: oneLine(n.name), keys: n.keys ?? 0, leases: n.leases ?? 0 })),
      forage: extra.forage ? extra.forage.windows : null,
    },
    stable: stalls.size > 0 ? { stalls: [...stalls].map(([horse, sessions]) => ({ horse, sessions })) } : null,
    waiting: pending.length,
  };
}

export function findRoom(w: World, id: string): { house: House; room: Room } | null {
  for (const h of w.houses) {
    const r = h.rooms.find((x) => x.id === id);
    if (r) return { house: h, room: r };
  }
  return null;
}

/** Whether a run is frozen itself or under a frozen tree: paddock marks each frozen run. */
export function isFrozen(r: Room): boolean {
  return r.run.frozen;
}

/** The changes of a copy that wait for the user: not applied, not discarded. */
export function waiting(r: Room): number {
  return r.copy ? r.copy.new + r.copy.changed + r.copy.deleted : 0;
}
