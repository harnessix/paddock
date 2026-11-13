// The copy of a run, as paddock reports it (src/copy/json.rs), and what
// the file manager makes of it: the tree, the diff, the cards. Nothing
// here reads a file or decides a state: paddock does both.

import { cells, charCells, cleanText, cut, oneLine } from './clean.js';
import type { Role } from './theme.js';
import { t, type Key } from './i18n/index.js';

// ---- the JSON of paddock ----

export interface Entry {
  size: number;
  mtime: number | null;
  mode: string;
  link: string | null;
}

export type Kind = 'new' | 'changed' | 'deleted';

export interface Change {
  path: string;
  kind: Kind;
  state: 'conflict' | 'kept' | 'open';
  conflict: unknown;
  conflict_text: string | null;
  kept: boolean;
  writing: boolean;
  copy: Entry | null;
  original: Entry | null;
  text: boolean | null;
  added: number | null;
  removed: number | null;
  merge: 'clean' | 'conflicts' | null;
}

export interface Counts {
  new: number;
  changed: number;
  deleted: number;
  conflicts: number;
  kept: number;
  ready: number;
  applied: number;
  behind: number;
}

export interface CopyRun {
  id: string;
  short?: string;
  harness?: string;
  project?: string;
  copy?: string;
  state?: 'live' | 'ended' | 'removed';
  started?: number;
  ended?: number | null;
  in_place?: boolean;
  counts?: Counts | null;
}

export interface PointerPart {
  state: string | null;
  text?: string;
  action?: string | null;
  ok?: boolean;
  store?: string | null;
}

/** `paddock changes RUN --json` (paddock.changes/v1). */
export interface ChangesDoc {
  run: CopyRun;
  pointer?: PointerPart;
  token: string;
  counts: Counts;
  changes: Change[];
  applied: string[];
  behind: { path: string; kind: Kind }[];
  special: string[];
  auto_apply?: boolean;
}

/** `paddock changes --json` (paddock.copies/v1). */
export interface CopiesDoc {
  runs: CopyRun[];
}

export interface Backup {
  id: string;
  short?: string;
  kind?: string;
  words?: string;
  created?: number;
  run?: string | null;
  harness?: string | null;
  files?: number;
  bytes?: number;
  shared?: boolean;
  undone?: number | null;
  undoable?: boolean;
}

/** `paddock backups --json` (paddock.backups/v1). */
export interface BackupsDoc {
  project?: string;
  pointer?: PointerPart;
  backups: Backup[];
  snapshots: Backup[];
}

/** `paddock pointer status --json` (paddock.pointer/v1). */
export interface PointerDoc {
  project?: string;
  state: string | null;
  text?: string;
  action?: string | null;
  ok?: boolean;
  pointer?: { format?: number; store?: string; created?: number } | null;
  store?: { id?: string; dir?: string; original?: string; runs?: number; copies?: number; backups?: number; size?: { bytes?: number; kind?: string } } | null;
  encryption?: { state?: string; kind?: string } | null;
}

/** `paddock apply --json` (paddock.apply/v1). */
export interface ApplyResult {
  written: string[];
  deleted: string[];
  merged: string[];
  both: [string, string][];
  kept_original: string[];
  conflicts: string[];
  kept: string[];
  unsafe_links: string[];
  backup: string | null;
}

/** `paddock keep --json` and `paddock discard --json`. */
export interface PathsResult {
  paths: string[];
  kept?: boolean;
  copy_removed?: boolean;
}

/** `paddock restore --json` and `paddock undo --json`. */
export interface RestoreResult {
  of?: string;
  restored: string[];
  removed: string[];
  skipped: string[];
  backup?: string | null;
  newer?: string[];
}

export type OnConflict = 'stop' | 'original' | 'copy' | 'both' | 'merge';

const ZERO: Counts = { new: 0, changed: 0, deleted: 0, conflicts: 0, kept: 0, ready: 0, applied: 0, behind: 0 };

const list = <T>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);

/** A changes document with every list in place, whatever paddock left out. */
export function normalChanges(v: Partial<ChangesDoc>): ChangesDoc {
  return {
    ...v,
    run: v.run ?? { id: '' },
    token: typeof v.token === 'string' ? v.token : '',
    counts: { ...ZERO, ...(v.counts ?? {}) },
    changes: list<Change>(v.changes).filter((c) => typeof c?.path === 'string' && c.path.length > 0),
    applied: list<string>(v.applied).filter((p) => typeof p === 'string'),
    behind: list<{ path: string; kind: Kind }>(v.behind),
    special: list<string>(v.special),
  };
}

export function normalApply(v: Partial<ApplyResult>): ApplyResult {
  return {
    written: list(v.written),
    deleted: list(v.deleted),
    merged: list(v.merged),
    both: list(v.both),
    kept_original: list(v.kept_original),
    conflicts: list(v.conflicts),
    kept: list(v.kept),
    unsafe_links: list(v.unsafe_links),
    backup: v.backup ?? null,
  };
}

export function normalRestore(v: Partial<RestoreResult>): RestoreResult {
  return { ...v, restored: list(v.restored), removed: list(v.removed), skipped: list(v.skipped) };
}

// ---- words ----

export type MarkId = 'new' | 'changed' | 'deleted' | 'conflict' | 'kept' | 'applied';

/** The marks of the design: a glyph, a word and a role. No state is a colour alone. */
const mark = (id: MarkId, glyph: string, role: Role): { glyph: string; word: string; role: Role } => ({
  glyph,
  // The word is in the language of the interface, read at each use.
  get word() {
    return t(`copy.mark.${id}`);
  },
  role,
});
export const MARKS: Record<MarkId, { glyph: string; word: string; role: Role }> = {
  new: mark('new', '+', 'success'),
  changed: mark('changed', '~', 'warning'),
  deleted: mark('deleted', '-', 'error'),
  conflict: mark('conflict', '!', 'error'),
  kept: mark('kept', '=', 'muted'),
  applied: mark('applied', '✓', 'muted'),
};
const MARK_ORDER: MarkId[] = ['conflict', 'changed', 'new', 'deleted', 'kept', 'applied'];

export function markOf(c: Change): MarkId {
  if (c.state === 'conflict' || (c.conflict !== null && c.conflict !== undefined)) return 'conflict';
  if (c.kept || c.state === 'kept') return 'kept';
  return c.kind === 'new' || c.kind === 'deleted' ? c.kind : 'changed';
}

export const files = (n: number): string => t('copy.files', { n });

/** "4 changed, 2 new, 1 deleted", or "no open change". */
export function countsLine(c: Counts): string {
  const parts = [c.changed ? t('copy.count.changed', { n: c.changed }) : '', c.new ? t('copy.count.new', { n: c.new }) : '', c.deleted ? t('copy.count.deleted', { n: c.deleted }) : ''].filter((x) => x);
  return parts.length > 0 ? parts.join(t('copy.listSep')) : t('copy.noOpenChange');
}

/** The line of a room card: "7 changes · 1 conflict". Empty without an open change. */
export function changesLine(c: Counts | null | undefined): string {
  if (!c) return '';
  const n = c.new + c.changed + c.deleted;
  if (n === 0) return '';
  return c.conflicts > 0 ? `${t('copy.changes', { n })} · ${t('copy.conflicts', { n: c.conflicts })}` : t('copy.changes', { n });
}

/** "12 KB", "1.4 MB". */
export function human(bytes: number): string {
  const b = Math.max(0, bytes);
  if (b < 1024) return `${b} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = b / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

const two = (n: number): string => String(n).padStart(2, '0');
const MONTHS: Key[] = ['copy.month.1', 'copy.month.2', 'copy.month.3', 'copy.month.4', 'copy.month.5', 'copy.month.6', 'copy.month.7', 'copy.month.8', 'copy.month.9', 'copy.month.10', 'copy.month.11', 'copy.month.12'];

/** "14:02", in the time of the user. */
export function clock(secs: number): string {
  const d = new Date(secs * 1000);
  return `${two(d.getHours())}:${two(d.getMinutes())}`;
}

/** "today", "yesterday" or "12 Nov". */
export function dayWord(secs: number, nowSecs: number): string {
  const day = (s: number): number => {
    const d = new Date(s * 1000);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  };
  const days = Math.round((day(nowSecs) - day(secs)) / 86_400_000);
  if (days === 0) return t('copy.today');
  if (days === 1) return t('copy.yesterday');
  const d = new Date(secs * 1000);
  const month = t(MONTHS[d.getMonth()] as Key);
  return d.getFullYear() === new Date(nowSecs * 1000).getFullYear() ? t('copy.date', { day: d.getDate(), month }) : t('copy.dateYear', { day: d.getDate(), month, year: d.getFullYear() });
}

export function baseName(p: string): string {
  const parts = p.split('/').filter((x) => x.length > 0);
  return parts[parts.length - 1] ?? p;
}

/** The home folder as `~`. */
export function tildePath(p: string, home = process.env.HOME ?? ''): string {
  const c = oneLine(p);
  return home && (c === home || c.startsWith(`${home}/`)) ? `~${c.slice(home.length)}` : c;
}

/** Cut in the middle, so the start of a name and its ending both stay. */
export function cutMiddle(text: string, width: number): string {
  if (width <= 0) return '';
  if (cells(text) <= width) return text;
  if (width <= 2) return cut(text, width);
  const chars = Array.from(text);
  const tail = Math.floor((width - 1) / 2);
  const head = width - 1 - tail;
  let a = '';
  let used = 0;
  for (const ch of chars) {
    if (used + charCells(ch) > head) break;
    a += ch;
    used += charCells(ch);
  }
  let b = '';
  used = 0;
  for (let i = chars.length - 1; i >= 0; i--) {
    const ch = chars[i] as string;
    if (used + charCells(ch) > tail) break;
    b = ch + b;
    used += charCells(ch);
  }
  return `${a}…${b}`;
}

/** What one side of a change is: "12 KB · 14:02", "a link to x", "no file". */
export function entryWords(e: Entry | null, nowSecs: number): string {
  if (!e) return t('copy.noFile');
  if (e.link) return t('copy.linkTo', { target: oneLine(e.link) });
  const day = e.mtime ? dayWord(e.mtime, nowSecs) : '';
  return `${human(e.size)}${e.mtime ? ` · ${day === t('copy.today') ? '' : `${day} `}${clock(e.mtime)}` : ''}`;
}

/** The line counts of a change: "+2 -1", "40 lines", or the size of a file that is no text. */
export function statWords(c: Change): string {
  if (c.text === true) {
    const a = c.added ?? 0;
    const r = c.removed ?? 0;
    if (c.kind === 'new' && r === 0) return t('copy.lines', { n: a });
    if (c.kind === 'deleted' && a === 0) return t('copy.lines', { n: r });
    return `+${a} -${r}`;
  }
  const e = c.copy ?? c.original;
  if (!e) return '';
  return e.link ? t('copy.link') : human(e.size);
}

// ---- the tree ----

/** What the tree shows: every change, the changes that an apply of all writes, or the conflicts. */
export type Filter = 'changes' | 'ready' | 'conflicts';
export const FILTERS: Filter[] = ['changes', 'ready', 'conflicts'];
export const FILTER_WORD: Record<Filter, string> = {
  get changes() {
    return t('copy.filter.changes');
  },
  get ready() {
    return t('copy.filter.ready');
  },
  get conflicts() {
    return t('copy.filter.conflicts');
  },
};

export interface TreeRow {
  /** '' for the root, "src/" for a folder, "src/a.ts" for a file. */
  key: string;
  kind: 'root' | 'dir' | 'file';
  name: string;
  depth: number;
  open: boolean;
  mark?: MarkId;
  writing?: boolean;
  change?: Change;
  /** The open changes under the row (one for a file with a change). */
  items: Change[];
  /** The count of each mark under a folder. */
  counts: Partial<Record<MarkId, number>>;
}

interface Node {
  name: string;
  path: string;
  dirs: Map<string, Node>;
  files: { name: string; path: string; change?: Change }[];
}

/** "~2 +1 !1": the counts of a folder. */
export function countsWord(counts: Partial<Record<MarkId, number>>): string {
  return MARK_ORDER.filter((m) => (counts[m] ?? 0) > 0)
    .map((m) => `${MARKS[m].glyph}${counts[m]}`)
    .join(' ');
}

/**
 * The rows of the tree. Only folders that hold a listed file show. A chain
 * of folders with nothing else in them is one row ("src/lib/util/"). The
 * first row is the project root: it stands for all.
 */
export function treeRows(doc: ChangesDoc, filter: Filter, name: string, closed: ReadonlySet<string>, rootName: string): TreeRow[] {
  const q = name.trim().toLowerCase();
  const match = (p: string): boolean => !q || p.toLowerCase().includes(q);
  const root: Node = { name: rootName, path: '', dirs: new Map(), files: [] };
  const add = (path: string, change?: Change): void => {
    const parts = path.split('/').filter((x) => x.length > 0);
    const file = parts.pop();
    if (file === undefined) return;
    let n = root;
    for (const p of parts) {
      let d = n.dirs.get(p);
      if (!d) {
        d = { name: p, path: `${n.path}${p}/`, dirs: new Map(), files: [] };
        n.dirs.set(p, d);
      }
      n = d;
    }
    n.files.push({ name: file, path, change });
  };
  for (const c of doc.changes) {
    const m = markOf(c);
    if (filter === 'conflicts' && m !== 'conflict') continue;
    if (filter === 'ready' && m !== 'new' && m !== 'changed' && m !== 'deleted') continue;
    if (match(c.path)) add(c.path, c);
  }
  // An applied file keeps its mark until the next change.
  if (filter === 'changes') for (const p of doc.applied) if (match(p) && !doc.changes.some((c) => c.path === p)) add(p);
  if (root.dirs.size === 0 && root.files.length === 0) return [];

  const out: TreeRow[] = [];
  const walk = (n: Node, depth: number, kind: 'root' | 'dir', label: string): { items: Change[]; counts: Partial<Record<MarkId, number>> } => {
    // A folder with one folder in it and no file: one row for both.
    while (kind === 'dir' && n.files.length === 0 && n.dirs.size === 1) {
      const only = [...n.dirs.values()][0] as Node;
      label += `${only.name}/`;
      n = only;
    }
    const row: TreeRow = { key: n.path, kind, name: label, depth, open: !closed.has(n.path), items: [], counts: {} };
    out.push(row);
    const at = out.length;
    for (const d of [...n.dirs.values()].sort((a, b) => a.name.localeCompare(b.name))) {
      const sub = walk(d, depth + 1, 'dir', `${d.name}/`);
      row.items.push(...sub.items);
      for (const [k, v] of Object.entries(sub.counts)) row.counts[k as MarkId] = (row.counts[k as MarkId] ?? 0) + (v ?? 0);
    }
    for (const f of n.files.sort((a, b) => a.name.localeCompare(b.name))) {
      const mark: MarkId = f.change ? markOf(f.change) : 'applied';
      row.counts[mark] = (row.counts[mark] ?? 0) + 1;
      if (f.change) row.items.push(f.change);
      out.push({ key: f.path, kind: 'file', name: f.name, depth: depth + 1, open: false, mark, writing: f.change?.writing === true, change: f.change, items: f.change ? [f.change] : [], counts: {} });
    }
    // A closed folder keeps its counts and hides its rows.
    if (!row.open) out.length = at;
    return { items: row.items, counts: row.counts };
  };
  walk(root, 0, 'root', `${rootName}/`);
  return out;
}

/** The folder row that holds a row, or -1. */
export function parentRow(rows: TreeRow[], index: number): number {
  const r = rows[index];
  if (!r) return -1;
  for (let i = index - 1; i >= 0; i--) {
    const p = rows[i] as TreeRow;
    if (p.kind !== 'file' && p.depth < r.depth) return i;
  }
  return -1;
}

// ---- the diff ----

export interface DiffLine {
  sign: ' ' | '+' | '-';
  old: number | null;
  new: number | null;
  text: string;
}

export interface Hunk {
  oldStart: number;
  newStart: number;
  lines: DiffLine[];
}

export interface Diff {
  hunks: Hunk[];
  /** What paddock says in place of a diff: a binary file, a link, a file above 1 MB. */
  note: string;
}

/** The unified text of `paddock changes RUN --diff PATH`, as lines with their numbers. */
export function parseDiff(text: string): Diff {
  const hunks: Hunk[] = [];
  const notes: string[] = [];
  let h: Hunk | null = null;
  let o = 0;
  let n = 0;
  // The lines that the block still has, by its header: an empty line after it is no line of the file.
  let oldLeft = 0;
  let newLeft = 0;
  for (const raw of text.split('\n')) {
    const m = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(raw);
    if (m) {
      o = Number(m[1]);
      n = Number(m[3]);
      oldLeft = m[2] === undefined ? 1 : Number(m[2]);
      newLeft = m[4] === undefined ? 1 : Number(m[4]);
      // A block without lines on one side starts after the line that its header names.
      if (oldLeft === 0) o += 1;
      if (newLeft === 0) n += 1;
      h = { oldStart: o, newStart: n, lines: [] };
      hunks.push(h);
      continue;
    }
    if (!h) {
      if (!raw.startsWith('--- ') && !raw.startsWith('+++ ') && raw.trim()) notes.push(oneLine(raw));
      continue;
    }
    if (raw.startsWith('\\') || (oldLeft <= 0 && newLeft <= 0)) continue;
    const sign = raw[0] ?? ' ';
    // The agent writes the file: its text reaches the screen only cleaned.
    const body = cleanText(raw.slice(1)).replace(/\n/g, ' ');
    if (sign === '+' && newLeft > 0) {
      h.lines.push({ sign, old: null, new: n++, text: body });
      newLeft--;
    } else if (sign === '-' && oldLeft > 0) {
      h.lines.push({ sign, old: o++, new: null, text: body });
      oldLeft--;
    } else if (sign === ' ') {
      h.lines.push({ sign, old: o++, new: n++, text: body });
      oldLeft--;
      newLeft--;
    }
  }
  return { hunks, note: notes.join(' ') };
}

/** Which text of a file shows: the diff, or one side of it, or the state of the merge. */
export type Side = 'diff' | 'original' | 'copy' | 'merge';

export interface ViewRow {
  kind: 'hunk' | 'line';
  /** The index of the change (the block) that the row belongs to. */
  hunk: number;
  sign: ' ' | '+' | '-' | '~';
  no: string;
  text: string;
}

/** The rows of the unified view, or of one side of it. */
export function unifiedRows(d: Diff, side: Side = 'diff'): ViewRow[] {
  const out: ViewRow[] = [];
  d.hunks.forEach((h, i) => {
    out.push({ kind: 'hunk', hunk: i, sign: ' ', no: '', text: t('copy.diff.line', { n: side === 'original' ? h.oldStart : h.newStart }) });
    for (const l of h.lines) {
      if (side === 'original' && l.sign === '+') continue;
      if (side === 'copy' && l.sign === '-') continue;
      const no = side === 'original' ? l.old : l.sign === '-' ? l.old : l.new;
      out.push({ kind: 'line', hunk: i, sign: l.sign, no: String(no ?? ''), text: l.text });
    }
  });
  return out;
}

export interface SideRow {
  kind: 'hunk' | 'line';
  hunk: number;
  /** The head of a block: where it starts in the original, and in the copy. */
  text?: string;
  other?: string;
  left: { sign: ' ' | '-' | '~'; no: string; text: string } | null;
  right: { sign: ' ' | '+' | '~'; no: string; text: string } | null;
}

/** The rows of the side by side view: the original at the left, the copy at the right. A changed line has `~` on both sides. */
export function sideRows(d: Diff): SideRow[] {
  const out: SideRow[] = [];
  d.hunks.forEach((h, i) => {
    out.push({ kind: 'hunk', hunk: i, text: t('copy.diff.line', { n: h.oldStart }), right: null, left: null, other: t('copy.diff.line', { n: h.newStart }) });
    let dels: DiffLine[] = [];
    let adds: DiffLine[] = [];
    const flush = (): void => {
      const n = Math.max(dels.length, adds.length);
      for (let k = 0; k < n; k++) {
        const a = dels[k];
        const b = adds[k];
        out.push({
          kind: 'line',
          hunk: i,
          left: a ? { sign: b ? '~' : '-', no: String(a.old ?? ''), text: a.text } : null,
          right: b ? { sign: a ? '~' : '+', no: String(b.new ?? ''), text: b.text } : null,
        });
      }
      dels = [];
      adds = [];
    };
    for (const l of h.lines) {
      if (l.sign === '-') dels.push(l);
      else if (l.sign === '+') adds.push(l);
      else {
        flush();
        out.push({ kind: 'line', hunk: i, left: { sign: ' ', no: String(l.old ?? ''), text: l.text }, right: { sign: ' ', no: String(l.new ?? ''), text: l.text } });
      }
    }
    flush();
  });
  return out;
}

/** The widest line number of a diff, in cells. */
export function numberWidth(d: Diff): number {
  let max = 1;
  for (const h of d.hunks) for (const l of h.lines) max = Math.max(max, String(l.old ?? l.new ?? 0).length, String(l.new ?? 0).length);
  return max;
}

/** A line from column `from` on, cut to `width` cells with an ellipsis at each cut end. */
export function lineWindow(text: string, from: number, width: number): string {
  if (from <= 0) return cut(text, width);
  let skipped = 0;
  let rest = '';
  for (const ch of text) {
    if (skipped < from) skipped += charCells(ch);
    else rest += ch;
  }
  return rest ? cut(`…${rest}`, width) : '';
}

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|ico|tiff?|avif)$/i;
export const TEXT_MAX = 1_000_000;

/** What to show for a change that has no text: the two sides in words. */
export function noTextLines(c: Change, nowSecs: number): string[] {
  const big = [c.copy, c.original].some((e) => e && !e.link && e.size > TEXT_MAX);
  // The two side words in one column, as wide as the longer word of the language.
  const names = [t('copy.side.original'), t('copy.side.copy')];
  const w = Math.max(...names.map((x) => cells(x))) + 2;
  const pad = (x: string): string => x + ' '.repeat(Math.max(0, w - cells(x)));
  const sides = [`${pad(names[0] as string)}${entryWords(c.original, nowSecs)}`, `${pad(names[1] as string)}${entryWords(c.copy, nowSecs)}`];
  if (big) return [t('copy.noText.big'), '', ...sides];
  if (c.copy?.link || c.original?.link) return [t('copy.noText.link'), '', ...sides];
  if (IMAGE.test(c.path)) return [t('copy.noText.image'), '', ...sides];
  return [t('copy.noText.binary'), '', ...sides];
}

// ---- the cards ----

export type Act =
  | { kind: 'cancel' }
  | { kind: 'apply'; paths: string[]; on: OnConflict }
  | { kind: 'keep'; paths: string[]; off: boolean }
  | { kind: 'discard'; paths: string[] }
  | { kind: 'undo' }
  | { kind: 'restore'; backup: string }
  | { kind: 'pointer'; verb: 'new' | 'create' | 'link' | 'read' };

export interface CardChoice {
  key: string;
  label: string;
  /** A choice that cannot be taken back at once: it needs the same key again within 3 s. */
  warn: boolean;
  role: Role;
  /** What the choice does, shown while it has the focus. */
  note: string;
  act: Act;
}

export interface CardItem {
  mark?: MarkId;
  text: string;
  word?: string;
  stat?: string;
}

export interface Card {
  kind: 'apply' | 'conflict' | 'discard' | 'undo' | 'restore' | 'pointer';
  title: string;
  pill: string;
  /** The role of the pill: a warning, when nothing else is said. */
  pillRole?: Role;
  /** The words after the pill. */
  rest: string;
  question: string;
  items: CardItem[];
  notes: { text: string; role?: Role }[];
  choices: CardChoice[];
}

const cancel = (): CardChoice => ({ key: 'n', label: t('copy.cancel'), warn: false, role: 'normal', note: t('copy.cancel.note'), act: { kind: 'cancel' } });

/** Where the files of a card are: "3 files in src/", "1 file". */
function scopeWords(changes: Change[], scope: string): string {
  return scope ? t('copy.scopeIn', { files: files(changes.length), scope: oneLine(scope) }) : files(changes.length);
}

const itemOf = (c: Change): CardItem => ({ mark: markOf(c), text: oneLine(c.path), word: MARKS[markOf(c)].word, stat: statWords(c) });

export interface CardContext {
  /** "claude · webshop". */
  who: string;
  /** The project folder, with `~`. */
  folder: string;
}

/** The Apply card: the changes that go to the folder. Deletes take the keep choice away and need the second confirm. */
export function applyCard(ctx: CardContext, changes: Change[], scope: string): Card {
  const deletes = changes.filter((c) => c.kind === 'deleted').length;
  const paths = changes.map((c) => c.path);
  const notes: Card['notes'] = [];
  if (deletes > 0) notes.push({ text: t('copy.apply.leave', { n: deletes }), role: 'warning' });
  notes.push({ text: t('copy.apply.backupFirst'), role: 'muted' });
  const yes: CardChoice = {
    key: 'y',
    label: deletes > 0 ? t('copy.apply.yes') : t('copy.apply.yesFiles', { files: files(changes.length) }),
    warn: deletes > 0,
    role: deletes > 0 ? 'warning' : 'success',
    note: t('copy.apply.yesNote', { files: files(changes.length), folder: ctx.folder }),
    act: { kind: 'apply', paths, on: 'stop' },
  };
  const keep: CardChoice = { key: 'k', label: t('copy.apply.keep'), warn: false, role: 'normal', note: t('copy.apply.keepNote'), act: { kind: 'keep', paths, off: false } };
  return {
    kind: 'apply',
    title: t('copy.apply.title', { who: ctx.who }),
    pill: t('copy.apply.pill'),
    rest: scopeWords(changes, scope),
    question: t('copy.apply.question', { folder: ctx.folder }),
    items: changes.map(itemOf),
    notes,
    choices: deletes > 0 ? [yes, cancel()] : [yes, keep, cancel()],
  };
}

/** The Conflict card of one file. Without a clean merge, the merge choice is left out. */
export function conflictCard(ctx: CardContext, c: Change, index: number, total: number, nowSecs: number): Card {
  const p = oneLine(c.path);
  const choices: CardChoice[] = [
    { key: 'o', label: t('copy.conflict.original'), warn: false, role: 'normal', note: t('copy.conflict.originalNote'), act: { kind: 'apply', paths: [c.path], on: 'original' } },
    { key: 'c', label: t('copy.conflict.copy'), warn: true, role: 'warning', note: t('copy.conflict.copyNote'), act: { kind: 'apply', paths: [c.path], on: 'copy' } },
    { key: 'b', label: t('copy.conflict.both'), warn: false, role: 'normal', note: t('copy.conflict.bothNote', { name: baseName(p) }), act: { kind: 'apply', paths: [c.path], on: 'both' } },
  ];
  if (c.merge === 'clean') choices.push({ key: 'm', label: t('copy.conflict.merge'), warn: false, role: 'success', note: t('copy.conflict.mergeNote'), act: { kind: 'apply', paths: [c.path], on: 'merge' } });
  choices.push(cancel());
  const notes: Card['notes'] = [];
  if (c.merge === 'conflicts') notes.push({ text: t('copy.conflict.noMerge'), role: 'muted' });
  notes.push({ text: t('copy.conflict.never'), role: 'muted' });
  return {
    kind: 'conflict',
    title: total > 1 ? t('copy.conflict.titleOf', { who: ctx.who, index: index + 1, total }) : t('copy.conflict.title', { who: ctx.who }),
    pill: t('copy.conflict.pill'),
    rest: oneLine(c.conflict_text ?? t('copy.conflict.rest')),
    question: t('copy.conflict.question', { path: p, folder: ctx.folder }),
    items: [
      { text: t('copy.conflict.originalNow'), stat: entryWords(c.original, nowSecs) },
      { text: t('copy.side.copy'), stat: `${entryWords(c.copy, nowSecs)}${c.text === true ? ` · ${statWords(c)}` : ''}` },
    ],
    notes,
    choices,
  };
}

export function discardCard(ctx: CardContext, changes: Change[], scope: string): Card {
  return {
    kind: 'discard',
    title: t('copy.discard.title', { who: ctx.who }),
    pill: t('copy.discard.pill'),
    rest: scopeWords(changes, scope),
    question: t('copy.discard.question'),
    items: changes.map(itemOf),
    notes: [{ text: t('copy.discard.noUndo'), role: 'warning' }],
    choices: [
      { key: 'y', label: t('copy.discard.yes', { files: files(changes.length) }), warn: true, role: 'error', note: t('copy.discard.yesNote'), act: { kind: 'discard', paths: changes.map((c) => c.path) } },
      cancel(),
    ],
  };
}

const backupWords = (b: Backup): string => `${oneLine(b.words ?? b.kind ?? t('copy.backup'))} · ${files(b.files ?? 0)}${b.harness ? ` · ${oneLine(b.harness)}` : ''}`;

export function undoCard(ctx: CardContext, b: Backup, nowSecs: number): Card {
  const when = b.created ? `${dayWord(b.created, nowSecs)} ${clock(b.created)}` : '';
  return {
    kind: 'undo',
    title: t('copy.undo.title', { who: ctx.who }),
    pill: t('copy.undo.pill'),
    rest: when,
    question: t('copy.undo.question', { folder: ctx.folder }),
    items: [{ text: backupWords(b), stat: when }],
    notes: [{ text: t('copy.undo.note'), role: 'muted' }],
    choices: [{ key: 'y', label: t('copy.undo.yes'), warn: false, role: 'success', note: t('copy.undo.yesNote'), act: { kind: 'undo' } }, cancel()],
  };
}

export function restoreCard(ctx: CardContext, b: Backup, nowSecs: number): Card {
  const when = b.created ? `${dayWord(b.created, nowSecs)} ${clock(b.created)}` : '';
  return {
    kind: 'restore',
    title: t('copy.restore.title', { who: ctx.who }),
    pill: t('copy.restore.pill'),
    rest: when,
    question: t('copy.restore.question', { folder: ctx.folder }),
    items: [{ text: backupWords(b), stat: b.shared ? t('copy.restore.shared') : human(b.bytes ?? 0) }],
    notes: [
      { text: t('copy.restore.note'), role: 'warning' },
      { text: t('copy.restore.before'), role: 'muted' },
    ],
    choices: [{ key: 'y', label: t('copy.restore.yes'), warn: true, role: 'warning', note: t('copy.restore.yesNote'), act: { kind: 'restore', backup: b.id } }, cancel()],
  };
}

/** The mark of a pointer state: ● when the pointer leads to a store that paddock can use, ▲ else. */
export function pointerMark(ok: boolean | undefined): string {
  return ok ? '●' : '▲';
}

/** The word of the disk: "encrypted (LUKS)". */
export function diskWord(e: PointerDoc['encryption']): string {
  if (e?.state === 'encrypted') return e.kind ? t('copy.disk.encryptedKind', { kind: oneLine(e.kind) }) : t('copy.disk.encrypted');
  if (e?.state === 'not_encrypted') return t('copy.disk.notEncrypted');
  return t('copy.disk.unknown');
}

/** The choices of the Pointer card for a state. Close is always the last one. */
export function pointerChoices(state: string | null): CardChoice[] {
  const out: CardChoice[] = [];
  if (state === 'copy' || state === 'no store') out.push({ key: 'n', label: state === 'copy' ? t('copy.pointer.newHere') : t('copy.pointer.new'), warn: true, role: 'warning', note: t('copy.pointer.newNote'), act: { kind: 'pointer', verb: 'new' } });
  if (state === 'restored') out.push({ key: 'l', label: t('copy.pointer.link'), warn: true, role: 'warning', note: t('copy.pointer.linkNote'), act: { kind: 'pointer', verb: 'link' } });
  if (state === 'no pointer') out.push({ key: 'c', label: t('copy.pointer.create'), warn: false, role: 'success', note: t('copy.pointer.createNote'), act: { kind: 'pointer', verb: 'create' } });
  if (state === 'disk away' || state === null) out.push({ key: 'r', label: t('copy.pointer.read'), warn: false, role: 'normal', note: t('copy.pointer.readNote'), act: { kind: 'pointer', verb: 'read' } });
  return out;
}

/** The lines of the Pointer card: a label and its words. */
export function pointerLines(p: PointerDoc): { label: string; text: string; role?: Role }[] {
  const out: { label: string; text: string; role?: Role }[] = [];
  out.push({ label: `${pointerMark(p.ok)} ${oneLine(p.state ?? t('copy.pointer.notKnown'))}`, text: oneLine(p.text ?? ''), role: p.ok ? 'success' : 'warning' });
  if (p.store) {
    const s = p.store;
    out.push({ label: t('copy.pointer.store'), text: `${tildePath(s.dir ?? '')}  ·  ${t('copy.pointer.copies', { n: s.copies ?? 0 })}  ·  ${t('copy.pointer.backups', { n: s.backups ?? 0 })}` });
    out.push({ label: t('copy.pointer.size'), text: t('copy.pointer.ownSize', { size: human(s.size?.bytes ?? 0) }) });
    if (!p.ok && s.original) out.push({ label: t('copy.pointer.belongs'), text: tildePath(s.original) });
  }
  out.push({ label: t('copy.pointer.disk'), text: diskWord(p.encryption) });
  return out;
}

/** The last backup that an undo takes back, or the reason why the interface does not offer one. */
export function undoTarget(doc: BackupsDoc | undefined): { backup?: Backup; why: string } {
  if (!doc) return { why: t('copy.undoTarget.noAnswer') };
  const b = doc.backups.find((x) => x.undoable === true);
  if (!b) return { why: t('copy.undoTarget.none') };
  // A newer snapshot of a run in the folder itself: `paddock undo` would restore that one.
  const snap = doc.snapshots[0];
  if (snap && (snap.created ?? 0) > (b.created ?? 0)) return { why: t('copy.undoTarget.newer') };
  return { backup: b, why: '' };
}

/** What an apply did, in words for the toast. */
/** The role of a toast. */
export type Tone = 'success' | 'error' | 'warning' | 'muted';

export function applyWords(r: ApplyResult): { text: string; role: Tone } {
  const n = r.written.length + r.deleted.length + r.merged.length;
  const parts: string[] = [];
  if (n > 0) parts.push(r.backup ? t('copy.applied.undo', { files: files(n) }) : t('copy.applied', { files: files(n) }));
  for (const [p, aside] of r.both) parts.push(t('copy.applied.both', { path: oneLine(p), aside: oneLine(aside) }));
  if (r.kept_original.length > 0) parts.push(t('copy.applied.kept', { paths: r.kept_original.map(oneLine).join(', ') }));
  if (r.unsafe_links.length > 0) parts.push(t('copy.applied.unsafe', { paths: r.unsafe_links.map(oneLine).join(', ') }));
  if (r.conflicts.length > 0) parts.push(t('copy.applied.conflicts', { n: r.conflicts.length }));
  if (parts.length === 0) parts.push(t('copy.applied.nothing'));
  return { text: parts.join(' '), role: r.conflicts.length + r.unsafe_links.length > 0 ? 'warning' : n > 0 || r.both.length > 0 ? 'success' : 'muted' };
}

export function restoreWords(verb: 'Restored' | 'Undone', r: RestoreResult): { text: string; role: Tone } {
  const n = r.restored.length + r.removed.length;
  const done = verb === 'Restored' ? 'copy.restored' : 'copy.undone';
  const head = n === 0 ? t('copy.restored.same') : r.removed.length > 0 ? t(`${done}.removed`, { files: files(r.restored.length), removed: r.removed.length }) : t(done, { files: files(r.restored.length) });
  const skipped = r.skipped.length > 0 ? ` ${t('copy.restored.skipped', { n: r.skipped.length })}` : '';
  return { text: `${head}${skipped}${verb === 'Restored' && n > 0 ? ` ${t('copy.restored.undo')}` : ''}`, role: r.skipped.length > 0 ? 'warning' : n > 0 ? 'success' : 'muted' };
}

/**
 * The command that shows a file read-only, from `$EDITOR`. An editor that
 * paddock knows gets its read-only flag. Any other program could write
 * the file, so `less` shows it then.
 */
export function editorCommand(env: NodeJS.ProcessEnv, file: string): { bin: string; args: string[] } {
  const words = (env.EDITOR ?? '').split(/\s+/).filter((w) => w.length > 0);
  const name = baseName(words[0] ?? '');
  if (['vi', 'vim', 'nvim'].includes(name)) return { bin: words[0] as string, args: [...words.slice(1), '-R', '--', file] };
  if (name === 'nano') return { bin: words[0] as string, args: [...words.slice(1), '-v', '--', file] };
  if (name === 'view' || name === 'less' || name === 'more') return { bin: words[0] as string, args: [...words.slice(1), '--', file] };
  return { bin: 'less', args: ['--', file] };
}
