// The rows the interface shows, put together from paddock's JSON. paddock
// decides every state. The interface only picks the most urgent word that
// paddock reported for a run.

import { oneLine } from './clean.js';
import type { Role } from './theme.js';
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

export const STATE: Record<RunState, { glyph: string; word: string; role: Role; order: number }> = {
  waiting: { glyph: '◆', word: 'waits for you', role: 'warning', order: 0 },
  blocked: { glyph: '■', word: 'blocked', role: 'error', order: 1 },
  stuck: { glyph: '▲', word: 'stuck', role: 'warning', order: 2 },
  running: { glyph: '●', word: 'running', role: 'success', order: 3 },
  idle: { glyph: '○', word: 'idle', role: 'muted', order: 4 },
  done: { glyph: '✓', word: 'done', role: 'success', order: 5 },
  failed: { glyph: '✖', word: 'failed', role: 'error', order: 6 },
  ended: { glyph: '✓', word: 'ended', role: 'muted', order: 7 },
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
  state: RunState;
  live: boolean;
  /** Unix seconds: start of a live run, end of an ended one. */
  since: number;
  /** The last thing paddock knows of the run, one line. */
  last: string;
  model: string;
  ticket: string;
  cards: number;
  unreadMail: number;
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
  for (const p of pending) {
    if (!p.sandbox) continue;
    cards.set(p.sandbox, (cards.get(p.sandbox) ?? 0) + 1);
  }
  const unread = new Map<string, number>();
  for (const m of mail) unread.set(m.id, m.unread_up ?? 0);
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
    rows.push({
      id: s.id,
      short,
      harness: oneLine(s.harness ?? '?'),
      project,
      projectName: baseName(project),
      backend: oneLine(s.backend ?? ''),
      network: oneLine(s.network ?? ''),
      state,
      live: true,
      since: s.started ?? 0,
      last: oneLine(b?.run.last || s.activity || ''),
      model: oneLine(b?.run.model || s.model || ''),
      ticket: b?.ticket ?? '',
      cards: n,
      unreadMail: unread.get(s.id) ?? 0,
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
      state,
      live: false,
      since: e.ended ?? 0,
      last: oneLine(e.text || b?.run.last || (e.status === 'stopped' ? 'stopped' : '')),
      model: oneLine(b?.run.model ?? ''),
      ticket: b?.ticket ?? '',
      cards: 0,
      unreadMail: unread.get(e.id) ?? 0,
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
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86400)} d`;
}

export function now(): number {
  return Math.floor(Date.now() / 1000);
}

/** Where a run runs: always a sandbox, with its backend (SPLIT1 rule). */
export function placeLabel(r: RunRow): string {
  return r.backend ? `sandbox ${r.backend}` : 'sandbox';
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
