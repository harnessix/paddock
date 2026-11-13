// The rows the interface shows, put together from paddock's JSON. paddock
// decides every state. The interface only picks the most urgent word that
// paddock reported for a run.

import { oneLine } from './clean.js';
import type { Role } from './theme.js';
import { t, type Key } from './i18n/index.js';
import type { Board, BoardRun, MailBox, Pending, Ps } from './types.js';

export type RunState =
  | 'waiting'
  | 'blocked'
  | 'stuck'
  | 'running'
  | 'idle'
  | 'done'
  | 'failed'
  | 'ended';

interface StateInfo {
  glyph: string;
  /** The word of the state, in the language of the interface. */
  readonly word: string;
  role: Role;
  order: number;
}

function stateInfo(glyph: string, key: Key, role: Role, order: number): StateInfo {
  return {
    glyph,
    get word() {
      return t(key);
    },
    role,
    order,
  };
}

export const STATE: Record<RunState, StateInfo> = {
  waiting: stateInfo('◆', 'state.waiting', 'warning', 0),
  blocked: stateInfo('■', 'state.blocked', 'error', 1),
  stuck: stateInfo('▲', 'state.stuck', 'warning', 2),
  running: stateInfo('●', 'state.running', 'success', 3),
  idle: stateInfo('○', 'state.idle', 'muted', 4),
  done: stateInfo('✓', 'state.done', 'success', 5),
  failed: stateInfo('✖', 'state.failed', 'error', 6),
  ended: stateInfo('✓', 'state.ended', 'muted', 7),
};

export function stateLabel(s: RunState): string {
  return `${STATE[s].glyph} ${STATE[s].word}`;
}

export interface RunRow {
  id: string;
  short: string;
  harness: string;
  project: string;
  projectName: string;
  backend: string;
  network: string;
  /** auto, manual or command; empty when paddock does not say. */
  approvals: string;
  state: RunState;
  live: boolean;
  /** Unix seconds: start of a live run, end of an ended one. */
  since: number;
  /** Unix seconds: since when the run is in its state (0: not known). */
  stateSince: number;
  /** The last thing paddock knows of the run, one line. */
  last: string;
  model: string;
  ticket: string;
  cards: number;
  unreadMail: number;
  /** The unread messages that are a question, when paddock reports them. */
  unreadAsk?: number;
  frozen: boolean;
  detached: boolean;
  view: boolean;
  parent: string;
  warnings: string[];
}

function shortOf(id: string): string {
  const m = /([0-9a-f]{6})[0-9a-f]*$/.exec(id);
  return m ? (m[1] as string) : id.slice(-6);
}

function baseName(p: string): string {
  const parts = p.split('/').filter((x) => x.length > 0);
  return parts[parts.length - 1] ?? p;
}

const BOARD_STATE: Record<string, RunState> = {
  waiting: 'waiting',
  blocked: 'blocked',
  stuck: 'stuck',
  working: 'running',
  idle: 'idle',
  done: 'done',
  ended: 'ended',
};

interface BoardInfo {
  run: BoardRun;
  ticket: string;
}

function boardRuns(board: Board | undefined): Map<string, BoardInfo> {
  const map = new Map<string, BoardInfo>();
  for (const p of board?.projects ?? []) {
    for (const t of p.tickets ?? []) {
      for (const r of t.runs ?? []) map.set(r.id, { run: r, ticket: oneLine(t.id) });
    }
    for (const r of p.runs_without_ticket ?? []) map.set(r.id, { run: r, ticket: '' });
  }
  return map;
}

export function buildRuns(
  ps: Ps | undefined,
  board: Board | undefined,
  pending: Pending[],
  mail: MailBox[],
): RunRow[] {
  const byBoard = boardRuns(board);
  const cards = new Map<string, number>();
  // The oldest card of a run: it waits for you since then.
  const asked = new Map<string, number>();
  const nowS = now();
  for (const p of pending) {
    if (!p.sandbox) continue;
    cards.set(p.sandbox, (cards.get(p.sandbox) ?? 0) + 1);
    const at = p.age_s !== undefined ? nowS - p.age_s : (p.created ?? 0);
    if (at > 0) asked.set(p.sandbox, Math.min(asked.get(p.sandbox) ?? Infinity, at));
  }
  const unread = new Map<string, number>();
  const asks = new Map<string, number>();
  for (const m of mail) {
    unread.set(m.id, m.unread_up ?? 0);
    if (m.unread_ask !== undefined) asks.set(m.id, m.unread_ask);
  }
  const rows: RunRow[] = [];
  const seen = new Set<string>();
  for (const s of ps?.sandboxes ?? []) {
    const short = s.short ? oneLine(s.short) : shortOf(s.id);
    const b = byBoard.get(short);
    const n = cards.get(s.id) ?? 0;
    let state: RunState = 'running';
    if (n > 0) state = 'waiting';
    else if (b?.run.state && BOARD_STATE[b.run.state]) state = BOARD_STATE[b.run.state] as RunState;
    else if (s.stuck) state = 'stuck';
    else if (s.idle_since) state = 'idle';
    if (state === 'ended') state = 'running';
    seen.add(s.id);
    const project = oneLine(s.project ?? '');
    let stateSince = s.started ?? 0;
    if (state === 'waiting') stateSince = asked.get(s.id) ?? stateSince;
    else if (state === 'stuck') stateSince = s.stuck?.since ?? stateSince;
    else if (state === 'idle') stateSince = s.idle_since ?? stateSince;
    if (s.frozen_since !== undefined && state !== 'waiting') stateSince = s.frozen_since;
    rows.push({
      id: s.id,
      short,
      harness: oneLine(s.harness ?? '?'),
      project,
      projectName: baseName(project),
      backend: oneLine(s.backend ?? ''),
      network: oneLine(s.network ?? ''),
      approvals: oneLine(s.approvals || b?.run.approvals || ''),
      state,
      live: true,
      since: s.started ?? 0,
      stateSince,
      last: oneLine(b?.run.last || s.activity || ''),
      model: oneLine(b?.run.model || s.model || ''),
      ticket: b?.ticket ?? '',
      cards: n,
      unreadMail: unread.get(s.id) ?? 0,
      unreadAsk: asks.get(s.id),
      frozen: s.frozen_since !== undefined,
      detached: s.detached === true,
      view: s.view === true,
      parent: oneLine(s.parent ?? ''),
      warnings: (s.warnings ?? []).map(oneLine),
    });
  }
  for (const e of ps?.ended ?? []) {
    if (seen.has(e.id)) continue;
    const short = e.short ? oneLine(e.short) : shortOf(e.id);
    const b = byBoard.get(short);
    const state: RunState = e.status === 'failed' ? 'failed' : e.status === 'ok' ? 'done' : 'ended';
    const project = oneLine(e.project ?? '');
    rows.push({
      id: e.id,
      short,
      harness: oneLine(e.harness ?? '?'),
      project,
      projectName: baseName(project),
      backend: '',
      network: '',
      approvals: '',
      state,
      live: false,
      since: e.ended ?? 0,
      stateSince: e.ended ?? 0,
      last: oneLine(e.text || b?.run.last || (e.status === 'stopped' ? t('state.stopped') : '')),
      model: oneLine(b?.run.model ?? ''),
      ticket: b?.ticket ?? '',
      cards: 0,
      unreadMail: unread.get(e.id) ?? 0,
      unreadAsk: asks.get(e.id),
      frozen: false,
      detached: false,
      view: false,
      parent: '',
      warnings: [],
    });
  }
  rows.sort((a, b) => STATE[a.state].order - STATE[b.state].order || b.since - a.since || a.id.localeCompare(b.id));
  return rows;
}

/** "12 s", "4 min", "3 h", "2 d". */
export function age(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return t('age.s', { n: s });
  if (s < 3600) return t('age.min', { n: Math.floor(s / 60) });
  if (s < 86400) return t('age.h', { n: Math.floor(s / 3600) });
  return t('age.d', { n: Math.floor(s / 86400) });
}

export function now(): number {
  return Math.floor(Date.now() / 1000);
}

/** Where a run runs: always a sandbox, with its backend (SPLIT1 rule). */
export function placeLabel(r: RunRow): string {
  return r.backend ? t('place.sandboxBackend', { backend: r.backend }) : t('place.sandbox');
}

/** Unread mail of the runs that still run: an ended run's mailbox no longer counts. */
export function liveUnread(boxes: MailBox[]): number {
  return boxes.reduce((n, m) => n + (m.state === 'running' || m.state === 'starting' ? (m.unread_up ?? 0) : 0), 0);
}

export function matches(r: RunRow, filter: string): boolean {
  if (!filter) return true;
  const f = filter.toLowerCase();
  return [r.id, r.harness, r.projectName, r.ticket, r.last, STATE[r.state].word]
    .some((x) => x.toLowerCase().includes(f));
}
