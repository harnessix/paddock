// The file manager of a copy: the `files` tab of a run. It holds what is
// selected, which card is open and what paddock said last, and it turns
// each key into a move or into one paddock command. The interface asks
// first for every command that writes to the folder or deletes: a card,
// and for a ⚠ choice the same key again.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Box, type Key } from 'ink';
import { cells, cut, fit, oneLine } from './clean.js';
import { t as tr } from './i18n/index.js';
import { CONFIRM_MIN_MS, CONFIRM_MS } from './approval.js';
import {
  applyCard,
  applyWords,
  baseName,
  changesLine,
  clock,
  conflictCard,
  countsLine,
  countsWord,
  dayWord,
  discardCard,
  files as filesWord,
  FILTER_WORD,
  FILTERS,
  human,
  markOf,
  MARKS,
  noTextLines,
  numberWidth,
  parentRow,
  parseDiff,
  pointerChoices,
  pointerLines,
  pointerMark,
  restoreCard,
  restoreWords,
  sideRows,
  statWords,
  tildePath,
  treeRows,
  undoCard,
  undoTarget,
  unifiedRows,
  lineWindow as windowOf,
  type Act,
  type Backup,
  type BackupsDoc,
  type Card,
  type CardChoice,
  type CardContext,
  type Change,
  type ChangesDoc,
  type CopyRun,
  type Diff,
  type Filter,
  type PointerDoc,
  type Side,
  type TreeRow,
} from './copy.js';
import { poll } from './data.js';
import { editLine } from './edit.js';
import { BLANK } from './keys.js';
import { wrapWords } from './launch.js';
import { age, now, STATE, type RunRow } from './model.js';
import { UnsupportedError, type PaddockApi } from './paddock.js';
import { SpriteView, useArt } from './art-layer.js';
import { artState, colorOf, spriteFor } from './sprites.js';
import { useTheme, type Role } from './theme.js';
import { Pane, T } from './ui.js';
import { ActionCard, BackupsPane, cardHeight, CARD_ITEMS, FilesHead, paneInner, TextPane, TreePane, type BackupLine, type CardView, type RowAct, type RunTab, type Seg } from './views/files.js';
import { ART_ROWS } from './views/rooms.js';

/** The tree is this wide beside the diff. */
export const TREE_W = 32;
/** From this width, the tree and the diff show side by side. Below it, one pane at a time. */
export const TWO_PANES = 80;
/** From this width, the diff shows the original and the copy side by side. */
export const SIDE_BY_SIDE = 120;
const readonlyNote = (): string => tr('files.readonly');

export type FilesHint = [string, { id: null; text: string; run: () => void }];

export interface FilesOptions {
  api: PaddockApi;
  /** The run whose files show, or '': the tab is closed. */
  run: string;
  readonly: boolean;
  intervalMs: number;
  columns: number;
  rows: number;
  runs: RunRow[];
  /** The runs that work on a copy, or undefined: paddock did not say yet. */
  copies: CopyRun[] | undefined;
  say: (text: string, role?: 'success' | 'error' | 'warning' | 'muted') => void;
  /** Drop the keys that wait: a card gets its answer from a key pressed after it showed. */
  clearKeys: () => void;
  /** An apply wrote to the folder of this run. */
  onApplied: (run: string) => void;
  /** Something changed that other screens show: read paddock again. */
  onChanged: () => void;
  /** Another tab of the run. */
  onTab: (t: RunTab) => void;
  /** Open a file in the editor of the user, read-only. False: this program cannot hand the terminal over. */
  onEdit?: (file: string) => boolean;
}

export interface Files {
  /** What to show first when the tab opens next: the backups, the pointer, or the Apply card of all changes. */
  intend: (what: 'backups' | 'pointer' | 'apply') => void;
  /** True when the key was for the file manager. */
  onKey: (input: string, key: Key) => boolean;
  /** The name filter is open: what is typed is text. */
  textMode: boolean;
  hints: FilesHint[];
  view: (width: number, height: number) => React.JSX.Element;
}

type Layer = null | 'backups' | 'pointer';

interface State {
  filter: Filter;
  name: string;
  typing: boolean;
  closed: Set<string>;
  /** The key of the selected row, or null: the first file. */
  sel: string | null;
  focus: 'tree' | 'diff';
  /** The diff has the whole width (T2). */
  full: boolean;
  mode: 'unified' | 'side' | null;
  side: Side;
  scroll: number;
  /** The change that n and N stand on. */
  hunk: number;
  hscroll: number;
  layer: Layer;
  /** The id of the selected backup. */
  backup: string;
}

const START: State = { filter: 'changes', name: '', typing: false, closed: new Set(), sel: null, focus: 'tree', full: false, mode: null, side: 'diff', scroll: 0, hunk: 0, hscroll: 0, layer: null, backup: '' };

/** An apply in steps: each conflict gets its own card first, then the rest gets the Apply card. */
interface Flow {
  paths: string[];
  scope: string;
  /** A kept file that was picked alone is applied too. */
  kept: boolean;
  decided: string[];
  conflicts: number;
}

interface CardSlot {
  card: Card;
  focus: string;
  armed?: { key: string; at: number };
  top: number;
  busy: boolean;
  error: string;
}

const errorText = (e: unknown): string => oneLine(e instanceof Error ? e.message : String(e));

/** The paths of the list that are not as the user saw them: what stopped an apply. */
export function changedSince(before: ChangesDoc, after: ChangesDoc): string[] {
  const key = (c: Change): string => JSON.stringify([c.kind, c.state, c.copy, c.original]);
  const old = new Map(before.changes.map((c) => [c.path, key(c)]));
  const out: string[] = [];
  for (const c of after.changes) {
    if (old.get(c.path) !== key(c)) out.push(c.path);
    old.delete(c.path);
  }
  return [...out, ...old.keys()];
}

/** The lines of the backups, newest first, under the word of their day. */
export function backupLines(doc: BackupsDoc, t: number): BackupLine[] {
  const all = [...doc.backups.map((b) => ({ b, restorable: true })), ...doc.snapshots.map((b) => ({ b, restorable: false }))].sort((x, y) => (y.b.created ?? 0) - (x.b.created ?? 0));
  const out: BackupLine[] = [];
  let day = '';
  for (const { b, restorable } of all) {
    const d = dayWord(b.created ?? 0, t);
    if (d !== day) out.push({ kind: 'day', key: `day:${d}`, text: d });
    day = d;
    const size = b.shared ? tr('copy.restore.shared') : human(b.bytes ?? 0);
    out.push({
      kind: 'backup',
      key: b.id,
      text: `${clock(b.created ?? 0)}  ${fit(oneLine(b.words ?? b.kind ?? ''), 16)} ${fit(filesWord(b.files ?? 0), 10)} ${fit(size, 7)} ${oneLine(b.harness ?? '')}`,
      note: typeof b.undone === 'number' ? tr('files.backup.undone') : undefined,
      restorable,
    });
  }
  return out;
}

export function useFiles(o: FilesOptions): Files {
  const { api, run, readonly, say } = o;
  const theme = useTheme();
  const art = useArt();
  const [st, setSt] = useState<State>(START);
  const patch = useCallback((p: Partial<State>) => setSt((s) => ({ ...s, ...p })), []);
  const [data, setData] = useState<{ run: string; doc?: ChangesDoc; error: string; unsupported: boolean }>({ run: '', error: '', unsupported: false });
  const [backups, setBackups] = useState<{ project: string; doc?: BackupsDoc; error: string }>({ project: '', error: '' });
  const [pointer, setPointer] = useState<{ project: string; doc?: PointerDoc; error: string }>({ project: '', error: '' });
  const [slot, setSlot] = useState<CardSlot | null>(null);
  const flow = useRef<Flow | null>(null);
  // The card on the screen now: an action that ends later closes only its own card.
  const front = useRef<Card | null>(null);
  front.current = slot?.card ?? null;
  const intent = useRef<'backups' | 'pointer' | 'apply' | null>(null);
  const diffs = useRef(new Map<string, { diff?: Diff; error?: string }>());
  const [, setDiffRound] = useState(0);

  const doc = data.run === run ? data.doc : undefined;
  const row = o.runs.find((r) => r.id === run);
  const project = doc?.run.project ?? row?.project ?? '';
  const harness = oneLine(doc?.run.harness ?? row?.harness ?? tr('files.agent'));
  const projectName = project ? baseName(oneLine(project)) : '?';
  const ctx: CardContext = { who: `${harness} · ${projectName}`, folder: tildePath(project) };

  // A new run: the tab starts as new.
  useEffect(() => {
    setSt({ ...START, closed: new Set(), layer: intent.current === 'backups' || intent.current === 'pointer' ? intent.current : null });
    if (intent.current !== 'apply') intent.current = null;
    setSlot(null);
    flow.current = null;
    diffs.current.clear();
  }, [run]);

  // The changes of the run, read again at the pace of the interface. A read that gives the same answer draws nothing.
  const load = useCallback(async (): Promise<ChangesDoc | undefined> => {
    if (!run) return undefined;
    try {
      const v = await api.changes(run);
      setData((old) => (old.run === run && old.error === '' && JSON.stringify(old.doc) === JSON.stringify(v) ? old : { run, doc: v, error: '', unsupported: false }));
      return v;
    } catch (e) {
      setData((old) => ({ run, doc: old.run === run ? old.doc : undefined, error: errorText(e), unsupported: e instanceof UnsupportedError }));
      throw e;
    }
  }, [api, run]);
  useEffect(() => {
    if (!run) return;
    return poll(load, o.intervalMs);
  }, [run, load, o.intervalMs]);

  const loadBackups = useCallback(async (): Promise<BackupsDoc | undefined> => {
    if (!project) return undefined;
    try {
      const v = await api.backups(project);
      setBackups({ project, doc: v, error: '' });
      return v;
    } catch (e) {
      setBackups((old) => ({ project, doc: old.project === project ? old.doc : undefined, error: errorText(e) }));
      return undefined;
    }
  }, [api, project]);
  const loadPointer = useCallback(async (): Promise<void> => {
    if (!project) return;
    try {
      const v = await api.pointer(project);
      setPointer({ project, doc: v, error: '' });
    } catch (e) {
      setPointer({ project, error: errorText(e) });
    }
  }, [api, project]);
  // The backups say what an undo takes back. They are read when the tab knows its project, and after each action.
  useEffect(() => {
    if (run && project) void loadBackups();
  }, [run, project, loadBackups]);
  useEffect(() => {
    if (run && project && st.layer === 'pointer') void loadPointer();
  }, [run, project, st.layer, loadPointer]);

  // ---- the tree ----
  const rows: TreeRow[] = useMemo(() => (doc ? treeRows(doc, st.filter, st.name, st.closed, projectName) : []), [doc, st.filter, st.name, st.closed, projectName]);
  let selIndex = st.sel === null ? -1 : rows.findIndex((r) => r.key === st.sel);
  if (selIndex < 0) selIndex = Math.max(0, rows.findIndex((r) => r.kind === 'file'));
  // A row that went away: the selection stays near its place.
  const lastIndex = useRef(0);
  if (st.sel !== null && rows.findIndex((r) => r.key === st.sel) < 0) selIndex = Math.min(lastIndex.current, Math.max(0, rows.length - 1));
  lastIndex.current = selIndex;
  const selRow: TreeRow | undefined = rows[selIndex];
  const selChange = selRow?.kind === 'file' ? selRow.change : undefined;
  const select = (i: number): void => {
    const r = rows[Math.max(0, Math.min(rows.length - 1, i))];
    if (r && r.key !== selRow?.key) patch({ sel: r.key, scroll: 0, hunk: 0, hscroll: 0, side: 'diff' });
    else if (r) patch({ sel: r.key });
  };
  const toggle = (i: number, open?: boolean): void => {
    const r = rows[i];
    if (!r || r.kind === 'file') return;
    const closed = new Set(st.closed);
    if (open ?? !r.open) closed.delete(r.key);
    else closed.add(r.key);
    patch({ closed, sel: r.key });
  };

  // ---- the diff of the selected file ----
  const diffKey = selChange ? `${run}\n${selChange.path}\n${JSON.stringify([selChange.kind, selChange.copy, selChange.original])}` : '';
  const wantDiff = selChange !== undefined && selChange.text !== false;
  useEffect(() => {
    if (!wantDiff || !selChange || diffs.current.has(diffKey)) return;
    let alive = true;
    const path = selChange.path;
    // A selection that moves on at once reads nothing.
    const timer = setTimeout(() => {
      api.diff(run, path).then(
        (text) => {
          diffs.current.set(diffKey, { diff: parseDiff(text) });
          if (alive) setDiffRound((n) => n + 1);
        },
        (e: unknown) => {
          diffs.current.set(diffKey, { error: errorText(e) });
          if (alive) setDiffRound((n) => n + 1);
        },
      );
    }, 60);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
    // The key names the file and its state: a new state reads again.
  }, [api, run, diffKey, wantDiff]);
  const loaded = diffKey ? diffs.current.get(diffKey) : undefined;
  const sideOk = o.columns >= SIDE_BY_SIDE;
  const mode: 'unified' | 'side' = sideOk && (st.mode ?? 'side') === 'side' ? 'side' : 'unified';

  // ---- the cards ----
  const openCard = useCallback(
    (card: Card, error = ''): void => {
      o.clearKeys();
      // The focus starts on the choice that changes nothing.
      setSlot({ card, focus: card.choices[card.choices.length - 1]?.key ?? '', top: 0, busy: false, error });
    },
    [o],
  );
  const closeCard = (): void => {
    flow.current = null;
    setSlot(null);
  };
  /** The next card of an apply: a conflict that waits for its choice, then the changes that are ready. */
  const nextCard = (d: ChangesDoc, error = ''): void => {
    const f = flow.current;
    if (!f) return setSlot(null);
    const scope = d.changes.filter((c) => f.paths.includes(c.path));
    const conflict = scope.find((c) => markOf(c) === 'conflict' && !f.decided.includes(c.path));
    if (conflict) return openCard(conflictCard(ctx, conflict, f.decided.length, Math.max(f.conflicts, f.decided.length + 1), now()), error);
    const ready = scope.filter((c) => markOf(c) !== 'conflict' && (f.kept || !c.kept));
    if (ready.length > 0) return openCard(applyCard(ctx, ready, f.scope), error);
    closeCard();
    if (error) say(error, 'error');
    else if (f.decided.length === 0) say(tr('files.say.nothingHere'), 'muted');
  };
  const scopeOf = (r: TreeRow): string => (r.kind === 'dir' ? r.key : '');
  const startApply = (items: Change[], scope: string): void => {
    if (!doc) return;
    flow.current = { paths: items.map((c) => c.path), scope, kept: items.length === 1, decided: [], conflicts: items.filter((c) => markOf(c) === 'conflict').length };
    nextCard(doc);
  };

  const after = async (): Promise<ChangesDoc | undefined> => {
    o.onChanged();
    void loadBackups();
    try {
      return await load();
    } catch {
      return undefined;
    }
  };

  const perform = async (act: Act): Promise<void> => {
    if (act.kind === 'cancel') return closeCard();
    const mine = front.current;
    /** Close the card of this action. A card that opened meanwhile stays. */
    const finish = (): void => (front.current === mine ? closeCard() : undefined);
    if (!doc && act.kind !== 'pointer' && act.kind !== 'restore' && act.kind !== 'undo') return;
    setSlot((s) => (s ? { ...s, busy: true, armed: undefined, error: '' } : s));
    try {
      if (act.kind === 'apply') {
        const done = await api.apply(run, act.paths, act.on, doc?.token ?? '');
        const w = applyWords(done);
        say(w.text, w.role);
        if (done.written.length + done.deleted.length + done.merged.length > 0) o.onApplied(run);
        const f = flow.current;
        // The Apply card is done when paddock answered. After a conflict, the next card needs the list of now.
        if (!f || act.on === 'stop') {
          finish();
          void after();
          return;
        }
        const next = await after();
        if (!next) return finish();
        f.decided.push(...act.paths);
        return nextCard(next);
      }
      if (act.kind === 'keep') {
        const done = await api.keep(run, act.paths, act.off);
        say(act.off ? tr('files.say.unkept', { n: done.paths.length }) : tr('files.say.kept', { n: done.paths.length }), 'success');
      } else if (act.kind === 'discard') {
        const done = await api.discardChanges(run, act.paths);
        say(tr('files.say.discarded', { files: filesWord(done.paths.length) }), 'warning');
      } else if (act.kind === 'undo') {
        const w = restoreWords('Undone', await api.undoApply(project));
        say(w.text, w.role);
      } else if (act.kind === 'restore') {
        const w = restoreWords('Restored', await api.restore(act.backup, project));
        say(w.text, w.role);
      } else if (act.kind === 'pointer') {
        if (act.verb !== 'read') say((await api.pointerDo(act.verb, project)) || tr('files.say.pointerDone', { verb: act.verb }), 'success');
        await loadPointer();
        setSlot(null);
        return;
      }
      // The card closes when paddock answered. The lists follow.
      finish();
      void after();
    } catch (e) {
      const text = errorText(e);
      // The list is not the one of the card: read it again, and say which file changed.
      const before = doc;
      const next = await after();
      if (act.kind === 'apply' && before && next && /not the ones that you saw/.test(text)) {
        const which = changedSince(before, next).map(oneLine);
        const names = which.length > 3 ? tr('files.stopped.more', { names: which.slice(0, 3).join(', '), n: which.length - 3 }) : which.length > 0 ? which.join(', ') : tr('files.stopped.aFile');
        return nextCard(next, tr('files.stopped', { names }));
      }
      setSlot((s) => (s ? { ...s, busy: false, armed: undefined, error: text } : s));
      if (!slot) say(text, 'error');
    }
  };

  /** A choice of the card in front. A ⚠ choice needs the same key again within 3 s. A click first puts the focus on its button. */
  const choose = (c: CardChoice, click = false): void => {
    const s = slot ?? (st.layer === 'pointer' ? pointerSlot : null);
    if (!s || s.busy) return;
    const set = (p: Partial<CardSlot>): void => (slot ? setSlot({ ...s, ...p }) : setPointerState({ focus: p.focus ?? s.focus, armed: 'armed' in p ? p.armed : s.armed }));
    if (click && s.focus !== c.key) return set({ focus: c.key, armed: undefined });
    if (c.warn) {
      const since = s.armed && s.armed.key === c.key ? Date.now() - s.armed.at : -1;
      // A double-click or a key repeat is not a second confirm.
      if (since >= 0 && since < CONFIRM_MIN_MS) return;
      if (!(since >= 0 && since <= CONFIRM_MS)) return set({ focus: c.key, armed: { key: c.key, at: Date.now() } });
    }
    set({ focus: c.key, armed: undefined });
    if (c.act.kind === 'cancel' && !slot) return patch({ layer: null });
    void perform(c.act);
  };

  // The Pointer card is made of what paddock says now. Only its focus is kept.
  const [pointerState, setPointerState] = useState<{ focus: string; armed?: { key: string; at: number } }>({ focus: 'Esc' });
  const pointerDoc = pointer.project === project ? pointer.doc : undefined;
  const pointerCard: Card = useMemo(() => {
    const close: CardChoice = { key: 'Esc', label: tr('files.close'), warn: false, role: 'normal', note: tr('files.close.note'), act: { kind: 'cancel' } };
    if (!pointerDoc) {
      return { kind: 'pointer', title: tr('files.pointer.title', { folder: ctx.folder }), pill: tr('files.pointer.pill'), rest: pointer.error ? '' : tr('files.pointer.reading'), question: '', items: [], notes: pointer.error ? [{ text: pointer.error, role: 'error' }] : [], choices: [close] };
    }
    const lines = pointerLines(pointerDoc);
    const head = lines[0] as { label: string; text: string };
    return {
      kind: 'pointer',
      title: tr('files.pointer.title', { folder: ctx.folder }),
      pill: head.label,
      pillRole: pointerDoc.ok ? 'success' : 'warning',
      rest: head.text,
      question: '',
      items: lines.slice(1).map((l) => ({ text: l.label, stat: l.text })),
      notes: [pointerDoc.action ? { text: tr('files.pointer.asks', { action: oneLine(pointerDoc.action) }), role: 'warning' as Role } : { text: tr('files.pointer.only'), role: 'muted' as Role }],
      choices: [...(readonly ? [] : pointerChoices(pointerDoc.state)), close],
    };
    // The folder names the card.
  }, [pointerDoc, pointer.error, ctx.folder, readonly]);
  const pointerSlot: CardSlot = { card: pointerCard, focus: pointerCard.choices.some((c) => c.key === pointerState.focus) ? pointerState.focus : 'Esc', armed: pointerState.armed, top: 0, busy: false, error: '' };
  const cardSlot: CardSlot | null = slot ?? (st.layer === 'pointer' ? pointerSlot : null);

  // The Apply card of all changes, asked for before the tab knew them.
  useEffect(() => {
    if (!doc || intent.current !== 'apply') return;
    intent.current = null;
    if (readonly) return say(readonlyNote(), 'muted');
    startApply(doc.changes, '');
    // Once, when the changes of the run are there.
  }, [doc]);

  // ---- the actions of a row ----
  const rowAct = (a: RowAct, r: TreeRow | undefined = selRow): void => {
    if (readonly) return say(readonlyNote(), 'muted');
    if (!doc || !r || r.items.length === 0) return say(r && r.kind === 'file' ? tr('files.say.inFolder') : tr('files.say.noChange'), 'muted');
    if (a === 'A') return startApply(r.items, scopeOf(r));
    if (a === 'D') return openCard(discardCard(ctx, r.items, scopeOf(r)));
    // Keep, and keep again: the mark goes away.
    const open = r.items.filter((c) => !c.kept && markOf(c) !== 'conflict');
    const kept = r.items.filter((c) => c.kept);
    if (open.length === 0 && kept.length === 0) return say(tr('files.say.conflictKeep'), 'muted');
    void perform({ kind: 'keep', paths: (open.length > 0 ? open : kept).map((c) => c.path), off: open.length === 0 });
  };
  const undo = (): void => {
    if (readonly) return say(readonlyNote(), 'muted');
    const offer = (doc: BackupsDoc | undefined): void => {
      const t = undoTarget(doc);
      if (!t.backup) return say(t.why, 'muted');
      openCard(undoCard(ctx, t.backup, now()));
    };
    const known = backups.project === project ? backups.doc : undefined;
    // The backups say what an undo takes back: without them, read them first.
    if (known) offer(known);
    else void loadBackups().then(offer);
  };

  // ---- the backups ----
  const bDoc = backups.project === project ? backups.doc : undefined;
  const bLines = useMemo(() => (bDoc ? backupLines(bDoc, now()) : []), [bDoc]);
  const bRows = bLines.filter((l) => l.kind === 'backup');
  let bSel = bRows.findIndex((l) => l.key === st.backup);
  if (bSel < 0) bSel = 0;
  const bRow = bRows[bSel];
  const moveBackup = (d: number): void => patch({ backup: bRows[Math.max(0, Math.min(bRows.length - 1, bSel + d))]?.key ?? '' });
  const restore = (key = bRow?.key): void => {
    if (readonly) return say(readonlyNote(), 'muted');
    const b: Backup | undefined = bDoc?.backups.find((x) => x.id === key);
    if (b) return openCard(restoreCard(ctx, b, now()));
    if (bDoc?.snapshots.some((x) => x.id === key)) return say(tr('files.say.snapshot'), 'muted');
    say(tr('files.say.noBackup'), 'muted');
  };

  // ---- the diff rows ----
  const diffRows = (): { hunk: number }[] => {
    if (!loaded?.diff) return [];
    return mode === 'side' && st.side === 'diff' ? sideRows(loaded.diff) : unifiedRows(loaded.diff, st.side);
  };
  const scrollBy = (d: number): void => {
    const list = diffRows();
    const scroll = Math.max(0, Math.min(Math.max(0, list.length - 1), st.scroll + d));
    // The change at the top of the view is the one that n and N count from.
    patch({ scroll, hunk: scroll === st.scroll ? st.hunk : (list[scroll]?.hunk ?? st.hunk) });
  };
  const jumpChange = (d: 1 | -1): void => {
    const list = diffRows();
    const count = list.length > 0 ? (list[list.length - 1]?.hunk ?? 0) + 1 : 0;
    if (count === 0) return say(tr('files.say.noJump'), 'muted');
    const next = st.hunk + d;
    if (next < 0 || next >= count) return say(d > 0 ? tr('files.say.last') : tr('files.say.first'), 'muted');
    patch({ hunk: next, scroll: Math.max(0, list.findIndex((r) => r.hunk === next)) });
  };
  const edit = (): void => {
    if (!selChange || !doc) return say(tr('files.say.selectFile'), 'muted');
    const inCopy = selChange.copy !== null;
    const root = inCopy ? doc.run.copy : doc.run.project;
    if (!root) return say(tr('files.say.noRoot'), 'muted');
    const file = `${root.replace(/\/$/, '')}/${selChange.path}`;
    if (!o.onEdit?.(file)) say(tr('files.say.openTerminal', { file: tildePath(file) }), 'muted');
  };

  // ---- keys ----
  const cardKey = (s: CardSlot, input: string, key: Key): boolean => {
    if (s.busy) return true;
    const choices = s.card.choices;
    const i = Math.max(0, choices.findIndex((c) => c.key === s.focus));
    const set = (p: Partial<CardSlot>): void => (slot ? setSlot({ ...s, ...p }) : setPointerState({ focus: p.focus ?? s.focus, armed: p.armed }));
    if (key.leftArrow || key.rightArrow || key.tab) {
      const c = choices[(i + (key.leftArrow || (key.tab && key.shift) ? -1 : 1) + choices.length) % choices.length];
      if (c) set({ focus: c.key, armed: s.armed?.key === c.key ? s.armed : undefined });
      return true;
    }
    if (key.upArrow || key.downArrow) {
      if (slot) setSlot({ ...s, top: Math.max(0, Math.min(Math.max(0, s.card.items.length - CARD_ITEMS), s.top + (key.upArrow ? -1 : 1))) });
      return true;
    }
    if (key.return) {
      const c = choices[i];
      if (c) choose(c);
      return true;
    }
    if (key.escape || (s.card.kind === 'pointer' && (input === 'q' || input === 'i'))) {
      if (slot) closeCard();
      else patch({ layer: null });
      return true;
    }
    const c = choices.find((x) => x.key === input);
    if (c) choose(c);
    // A card takes every key until it closes.
    return true;
  };

  const onKey = (input: string, key: Key): boolean => {
    if (!run) return false;
    if (key.ctrl) return false;
    if (cardSlot) return cardKey(cardSlot, input, key);
    if (st.typing) {
      if (key.escape) patch({ typing: false, name: '', sel: null });
      else if (key.return) patch({ typing: false });
      else {
        const next = editLine(st.name, input, key);
        if (next !== null) patch({ name: next, sel: null });
      }
      return true;
    }
    if (input === 'i') {
      patch({ layer: 'pointer' });
      setPointerState({ focus: 'Esc' });
      return true;
    }
    if (st.layer === 'backups') {
      if (key.escape || input === 'q' || input === 'b') patch({ layer: null });
      else if (key.upArrow || input === 'k') moveBackup(-1);
      else if (key.downArrow || input === 'j') moveBackup(1);
      else if (key.pageUp) moveBackup(-10);
      else if (key.pageDown) moveBackup(10);
      else if (key.home || input === 'g') moveBackup(-bRows.length);
      else if (key.end || input === 'G') moveBackup(bRows.length);
      else if (input === 'R' || key.return) restore();
      else if (input === 'u') undo();
      else return false;
      return true;
    }
    const inDiff = st.full || st.focus === 'diff';
    const page = Math.max(1, o.rows - 9);
    if (key.escape || input === 'q') {
      if (st.full) patch({ full: false });
      else if (st.focus === 'diff') patch({ focus: 'tree' });
      else if (st.name) patch({ name: '', sel: null });
      else return false;
      return true;
    }
    if (key.upArrow || input === 'k') return inDiff ? (scrollBy(-1), true) : (select(selIndex - 1), true);
    if (key.downArrow || input === 'j') return inDiff ? (scrollBy(1), true) : (select(selIndex + 1), true);
    if (key.pageUp) return inDiff ? (scrollBy(-page), true) : (select(selIndex - page), true);
    if (key.pageDown) return inDiff ? (scrollBy(page), true) : (select(selIndex + page), true);
    if (key.home || input === 'g') return inDiff ? (patch({ scroll: 0 }), true) : (select(0), true);
    if (key.end || input === 'G') return inDiff ? (scrollBy(Number.MAX_SAFE_INTEGER), true) : (select(rows.length - 1), true);
    if (key.leftArrow || key.rightArrow) {
      if (inDiff) patch({ hscroll: Math.max(0, st.hscroll + (key.rightArrow ? 8 : -8)) });
      else if (selRow && selRow.kind !== 'file') toggle(selIndex, key.rightArrow);
      else if (key.leftArrow) select(parentRow(rows, selIndex));
      else if (selChange) patch({ focus: 'diff' });
      return true;
    }
    if (key.tab) {
      if (!st.full) patch({ focus: st.focus === 'tree' ? 'diff' : 'tree' });
      return true;
    }
    if (key.return) {
      if (st.full) return true;
      if (selRow && selRow.kind !== 'file') toggle(selIndex);
      else if (selRow) patch({ full: true });
      return true;
    }
    switch (input) {
      case 'A':
      case 'K':
      case 'D':
        rowAct(input);
        return true;
      case 'u':
        undo();
        return true;
      case 'c':
        patch({ filter: FILTERS[(FILTERS.indexOf(st.filter) + 1) % FILTERS.length] as Filter, sel: null, full: false });
        return true;
      case '/':
        patch({ typing: true, full: false, focus: 'tree' });
        return true;
      case 'n':
        jumpChange(1);
        return true;
      case 'N':
        jumpChange(-1);
        return true;
      case 't':
        if (!sideOk) say(tr('files.say.sideNeeds', { n: SIDE_BY_SIDE }), 'muted');
        else patch({ mode: mode === 'side' ? 'unified' : 'side', scroll: 0, hunk: 0, side: 'diff' });
        return true;
      case 'e':
        edit();
        return true;
      case 'b':
        patch({ layer: 'backups' });
        void loadBackups();
        return true;
      case '1':
      case '2':
      case '3': {
        if (!selChange || markOf(selChange) !== 'conflict') return true;
        const side = (['original', 'copy', 'merge'] as const)[Number(input) - 1] as Side;
        patch({ side: st.side === side ? 'diff' : side, scroll: 0, hunk: 0 });
        return true;
      }
      // On a run, f opens this tab: here it changes nothing.
      case 'f':
        return true;
      default:
        return false;
    }
  };

  // ---- the footer ----
  const press = (k: string, key: Partial<Key> = {}) => (): void => void onKey(k, { ...BLANK, ...key });
  const hint = (k: string, text: string, input = k, key: Partial<Key> = {}): FilesHint => [k, { id: null, text, run: press(input, key) }];
  const hints: FilesHint[] = [];
  if (cardSlot) {
    hints.push(hint('←→', tr('files.hint.choose'), '', { rightArrow: true }), hint('↵', tr('files.hint.answer'), '\r', { return: true }), hint('Esc', cardSlot.card.kind === 'pointer' ? tr('files.hint.close') : tr('files.hint.cancel'), '', { escape: true }));
  } else if (st.layer === 'backups') {
    if (!readonly) hints.push(hint('R', tr('files.hint.restore')));
    hints.push(hint('u', tr('files.hint.undo')), hint('i', tr('files.hint.pointer')), hint('Esc', tr('files.hint.back'), '', { escape: true }));
  } else if (st.full) {
    hints.push(hint('n', tr('files.hint.next')), hint('N', tr('files.hint.previous')), hint('t', mode === 'side' ? tr('files.hint.unified') : tr('files.hint.side')));
    if (!readonly) hints.push(hint('A', tr('files.hint.apply')), hint('K', tr('files.hint.keep')), hint('D', tr('files.hint.discard')));
    hints.push(hint('Esc', tr('files.hint.back'), '', { escape: true }));
  } else {
    if (!readonly) hints.push(hint('A', tr('files.hint.apply')), hint('K', tr('files.hint.keep')), hint('D', tr('files.hint.discard')));
    hints.push(hint('↵', tr('files.hint.diff'), '\r', { return: true }), hint('c', tr('files.hint.filter')), hint('b', tr('files.hint.backups')), hint('i', tr('files.hint.pointer')));
  }

  // ---- drawing ----
  const diffPane = (width: number, height: number, focused: boolean, full: boolean): React.JSX.Element => {
    const box = paneInner(theme.plain, width, height);
    const w = box.width;
    const plainLines = (texts: string[], role: Role = 'muted'): Seg[][] => texts.flatMap((t) => (cells(t) > w ? wrapWords(t, w, 6) : [t])).map((t) => [{ text: t, role }]);
    const wheel = (d: 1 | -1): void => scrollBy(d * 3);
    const click = (): void => (st.full || cardSlot ? undefined : patch({ focus: 'diff' }));
    if (!selRow) return <TextPane title={tr('files.diff')} lines={[]} width={width} height={height} focused={focused} />;
    if (selRow.kind !== 'file') {
      // A folder: its files with their marks and line counts. A, K and D act on all of them.
      const items = selRow.items;
      const nameW = Math.max(8, Math.min(w - 22, Math.max(...items.map((c) => cells(oneLine(c.path))), 8)));
      const lines: Seg[][] = items.slice(0, Math.max(0, box.height)).map((c) => {
        const m = MARKS[markOf(c)];
        return [
          { text: ` ${m.glyph} `, role: m.role },
          { text: `${fit(oneLine(c.path), nameW)} ` },
          { text: fit(c.writing ? tr('files.writing') : m.word, 9), role: m.role },
          { text: cut(statWords(c), Math.max(0, w - nameW - 13)), role: 'muted' },
        ];
      });
      if (items.length > box.height && box.height > 0) lines[box.height - 1] = [{ text: cut(` ${tr('files.andMore', { n: items.length - box.height + 1 })}`, w), role: 'muted' }];
      const applied = selRow.counts.applied ?? 0;
      if (items.length === 0) lines.push(...plainLines([applied > 0 ? tr('files.allApplied') : tr('files.noChangeHere')]));
      return (
        <TextPane
          title={`${oneLine(selRow.name)} · ${items.length > 0 ? filesWord(items.length) : tr('copy.noOpenChange')}${countsWord(selRow.counts) ? ` · ${countsWord(selRow.counts)}` : ''}`}
          footer={items.length > 0 ? (readonly ? tr('files.readonlyShort') : tr('files.actOn', { n: items.length })) : ''}
          lines={lines}
          width={width}
          height={height}
          focused={focused}
          onClick={click}
        />
      );
    }
    const path = oneLine(selRow.key);
    if (!selChange) {
      return <TextPane title={tr('files.appliedTitle', { path })} footer="" lines={plainLines([tr('files.appliedLine'), '', tr('files.undoLast')])} width={width} height={height} focused={focused} onClick={click} />;
    }
    const m = MARKS[markOf(selChange)];
    const conflict = markOf(selChange) === 'conflict';
    const head = `${path} · ${selChange.writing ? tr('files.writing') : m.word}${statWords(selChange) ? ` · ${statWords(selChange)}` : ''}`;
    const sideName: Record<Side, string> = { diff: tr('files.side.diff'), original: tr('copy.conflict.originalNow'), copy: tr('copy.side.copy'), merge: tr('files.side.merge') };
    const title = `${head}${full || st.side !== 'diff' ? ` · ${sideName[st.side]}` : ''}`;
    // The whole width names the three sides. Beside the tree, their keys stand for them.
    const views = conflict ? ` · ${full ? tr('files.views.full', { original: sideName.original, copy: sideName.copy, merge: sideName.merge }) : tr('files.views.short')}` : '';
    if (selChange.text === false) {
      return <TextPane title={title} footer={conflict ? oneLine(selChange.conflict_text ?? tr('copy.mark.conflict')) : ''} lines={plainLines(noTextLines(selChange, now()), 'normal')} width={width} height={height} focused={focused} onClick={click} />;
    }
    if (st.side === 'merge') {
      const words =
        selChange.merge === 'clean'
          ? [tr('files.merge.clean'), '', tr('files.merge.cleanHint')]
          : selChange.merge === 'conflicts'
            ? [tr('files.merge.marks'), '', tr('files.merge.hint')]
            : [tr('files.merge.none'), '', tr('files.merge.hint')];
      return <TextPane title={title} footer={views.slice(3)} lines={plainLines(words, 'normal')} width={width} height={height} focused={focused} onClick={click} />;
    }
    if (!loaded) return <TextPane title={title} footer="" lines={plainLines([tr('files.readingDiff')])} width={width} height={height} focused={focused} onClick={click} />;
    if (loaded.error || !loaded.diff) return <TextPane title={title} footer="" lines={plainLines([loaded.error ?? tr('files.noDiff')], 'error')} width={width} height={height} focused={focused} onClick={click} />;
    const d = loaded.diff;
    if (d.hunks.length === 0) {
      return <TextPane title={title} footer="" lines={plainLines([d.note || tr('files.sameText')], 'normal')} width={width} height={height} focused={focused} onClick={click} />;
    }
    const nw = numberWidth(d);
    const signRole = (s: string): Role => (s === '+' ? 'success' : s === '-' ? 'error' : s === '~' ? 'warning' : 'normal');
    let lines: Seg[][];
    let total: number;
    const cur = Math.max(0, Math.min(d.hunks.length - 1, st.hunk));
    if (mode === 'side' && st.side === 'diff') {
      const list = sideRows(d);
      total = list.length;
      const top = Math.max(0, Math.min(st.scroll, Math.max(0, total - box.height)));
      const hw = Math.floor((w - 1) / 2);
      const half = (c: { sign: string; no: string; text: string } | null, width: number): Seg[] =>
        c ? [{ text: `${c.sign}${c.no.padStart(nw)} `, role: signRole(c.sign) }, { text: fit(windowOf(c.text, st.hscroll, Math.max(0, width - nw - 2)), Math.max(0, width - nw - 2)), role: c.sign === ' ' ? 'normal' : signRole(c.sign) }] : [{ text: ' '.repeat(width) }];
      lines = list.slice(top, top + box.height).map((r) => (r.kind === 'hunk' ? [{ text: fit(` ${r.text ?? ''}`, hw), role: 'accent' as Role, bold: r.hunk === cur }, { text: '│', role: 'muted' as Role }, { text: ` ${r.other ?? ''}`, role: 'accent' as Role, bold: r.hunk === cur }] : [...half(r.left, hw), { text: '│', role: 'muted' as Role }, ...half(r.right, w - hw - 1)]));
    } else {
      const list = unifiedRows(d, st.side);
      total = list.length;
      const top = Math.max(0, Math.min(st.scroll, Math.max(0, total - box.height)));
      lines = list.slice(top, top + box.height).map((r) =>
        r.kind === 'hunk'
          ? [{ text: ` ${r.text}`, role: 'accent' as Role, bold: r.hunk === cur }]
          : [
              { text: `${r.sign}${r.no.padStart(nw)} `, role: signRole(r.sign), bold: r.sign !== ' ' },
              { text: windowOf(r.text, st.hscroll, Math.max(0, w - nw - 2)), role: signRole(r.sign) },
            ],
      );
    }
    const footer = [
      tr('files.footer.change', { cur: cur + 1, total: d.hunks.length }),
      mode === 'side' && st.side === 'diff' ? tr('files.footer.sides') : tr('files.footer.unified'),
      ...(st.hscroll > 0 ? [tr('files.footer.fromColumn', { col: st.hscroll + 1 })] : []),
    ].join(' · ') + views;
    return <TextPane title={title} footer={footer} lines={lines} width={width} height={height} focused={focused} onWheel={wheel} onClick={click} />;
  };

  const treePane = (width: number, height: number, focused: boolean): React.JSX.Element => {
    const box = paneInner(theme.plain, width, height);
    const filesOnly = rows.filter((r) => r.kind === 'file');
    const at = selRow?.kind === 'file' ? filesOnly.indexOf(selRow) + 1 : 0;
    const conflicts = doc?.counts.conflicts ?? 0;
    const footer = st.typing
      ? `/ ${cut(st.name, Math.max(0, box.width - 6))}▏`
      : rows.length === 0
        ? ''
        : `${at > 0 ? tr('files.at', { at, total: filesOnly.length }) : filesWord(selRow?.items.length ?? 0)}${conflicts > 0 ? ` · ${tr('copy.conflicts', { n: conflicts })}` : ''}`;
    let empty = tr('files.empty');
    if (st.name) empty = tr('files.noMatch', { name: st.name });
    else if (st.filter === 'conflicts') empty = tr('files.noConflicts');
    else if (st.filter === 'ready') empty = tr('files.noReady');
    else if ((doc?.counts.applied ?? 0) > 0) empty = tr('files.allApplied');
    return (
      <TreePane
        title={`${tr('files.title', { filter: FILTER_WORD[st.filter] })}${st.name && !st.typing ? ` · "${cut(st.name, 10)}"` : ''}`}
        footer={footer}
        rows={rows}
        selected={selIndex}
        width={width}
        height={height}
        focused={focused}
        empty={wrapWords(empty, box.width, 6)}
        onSelect={(i) => {
          // A card has the focus until it closes: the panes under it take no click.
          if (cardSlot) return;
          select(i);
          patch({ focus: 'tree' });
        }}
        onOpen={(i) => {
          if (cardSlot) return;
          const r = rows[i];
          if (r && r.kind !== 'file') return toggle(i);
          if (r) patch({ sel: r.key, full: true });
        }}
        onToggle={(i) => (cardSlot ? undefined : toggle(i))}
        onAct={(i, a) => {
          if (cardSlot) return;
          select(i);
          rowAct(a, rows[i]);
        }}
        onWheel={(d) => (cardSlot ? undefined : select(selIndex + d * 3))}
      />
    );
  };

  const view = (width: number, height: number): React.JSX.Element => {
    const state = row ? `${STATE[row.state].word}${row.stateSince > 0 ? ` ${age(now() - row.stateSince)}` : ''}` : (doc?.run.state ?? STATE.ended.word);
    const copyWords = doc ? tr('files.head.copy', { counts: countsLine(doc.counts) }) : data.error ? tr('files.head.noCopy') : tr('files.head.reading');
    const p = pointerDoc ?? doc?.pointer;
    const head = (
      <FilesHead
        line={`${harness} · ${projectName} · ${doc?.run.state === 'ended' && !row?.live ? 'ended' : state} · ${copyWords}`}
        pointer={p ? `${pointerMark(p.ok)} ${oneLine(p.state ?? tr('copy.pointer.notKnown'))}` : '…'}
        pointerOk={p?.ok !== false}
        width={width}
        onTab={o.onTab}
        onPointer={() => {
          patch({ layer: 'pointer' });
          setPointerState({ focus: 'Esc' });
        }}
      />
    );
    const h = Math.max(0, height - 2);
    const cardH = cardSlot ? Math.min(h, cardHeight(cardSlot as CardView, width, theme.plain)) : 0;
    const rest = h - cardH;
    const message = (title: string, texts: string[], role: Role = 'muted'): React.JSX.Element => (
      <Pane title={title} focused width={width} height={rest}>
        {texts.map((t, i) => (
          <T key={i} role={i === 0 ? role : 'muted'} wrap="wrap">
            {t || ' '}
          </T>
        ))}
      </Pane>
    );
    let body: React.JSX.Element | null;
    if (rest < 4) body = null;
    else if (st.layer === 'backups') {
      const own = (bDoc?.backups ?? []).filter((b) => !b.shared).reduce((n, b) => n + (b.bytes ?? 0), 0);
      body = (
        <BackupsPane
          title={tr('files.backups.title', { project: projectName })}
          footer={bRows.length > 0 ? tr('files.backups.footer', { at: bSel + 1, total: bRows.length, size: human(own) }) : ''}
          lines={bLines}
          selected={bRow ? bLines.indexOf(bRow) : 0}
          empty={backups.error || (bDoc ? tr('files.backups.none') : tr('files.backups.reading'))}
          width={width}
          height={rest}
          onSelect={(i) => (cardSlot ? undefined : patch({ backup: bLines[i]?.key ?? '' }))}
          onRestore={(i) => {
            if (cardSlot) return;
            patch({ backup: bLines[i]?.key ?? '' });
            restore(bLines[i]?.key);
          }}
          onWheel={(d) => (cardSlot ? undefined : moveBackup(d * 3))}
        />
      );
    } else if (!doc) {
      if (data.unsupported) body = message(tr('files.files'), [tr('files.msg.unsupported'), data.error]);
      else if (data.error && o.copies !== undefined && !o.copies.some((c) => c.id === run)) body = message(tr('files.files'), [tr('files.msg.noCopy'), '', tr('files.msg.noCopyHint')], 'normal');
      else if (data.error) body = message(tr('files.files'), [data.error, '', tr('files.msg.noAnswer')], 'error');
      else body = message(tr('files.files'), [tr('files.msg.reading')]);
    } else if (doc.run.state === 'removed') {
      body = message(tr('files.files'), [tr('files.msg.gone')], 'normal');
    } else if (st.full) {
      body = diffPane(width, rest, true, true);
    } else if (width < TWO_PANES) {
      // A narrow screen: one pane at a time. Tab changes it.
      body = st.focus === 'diff' ? diffPane(width, rest, true, false) : treePane(width, rest, true);
    } else {
      // From 100 columns and 30 rows, the room of the run stands above the tree, with its character.
      const roomH = ART_ROWS + 3;
      const room = row && art.enabled && !theme.plain && o.columns >= 100 && o.rows >= 30 && rest - roomH >= 8 && !cardSlot;
      const sprite = room && row ? spriteFor(colorOf(row.harness), artState(row)) : null;
      body = (
        <Box flexDirection="row" width={width} height={rest}>
          {/* Without boxes, a column of space parts the tree from the diff. */}
          <Box flexDirection="column" width={TREE_W} height={rest} flexShrink={0} paddingRight={theme.plain ? 2 : 0}>
            {room && row ? (
              <Pane title={`${row.harness}-${row.short}`} focused={false} width={TREE_W} height={roomH}>
                <Box width={TREE_W - 4} height={ART_ROWS} justifyContent="center" flexShrink={0}>
                  {sprite ? <SpriteView sprite={sprite} /> : null}
                </Box>
                <T role={STATE[row.state].role}>{cut(`${state}${changesLine(doc.counts) ? ` · ${changesLine(doc.counts)}` : ''}`, TREE_W - 4)}</T>
              </Pane>
            ) : null}
            {treePane(TREE_W - (theme.plain ? 2 : 0), rest - (room ? roomH : 0), st.focus === 'tree' && !cardSlot)}
          </Box>
          {diffPane(width - TREE_W, rest, st.focus === 'diff' && !cardSlot, false)}
        </Box>
      );
    }
    return (
      <Box flexDirection="column" width={width} height={height} overflow="hidden">
        {head}
        {cardSlot ? (
          <Box height={cardH} flexShrink={0} overflow="hidden">
            <ActionCard view={cardSlot as CardView} width={width} onChoose={(c) => choose(c, true)} />
          </Box>
        ) : null}
        {body}
      </Box>
    );
  };

  return {
    intend: (what) => {
      intent.current = what;
    },
    onKey,
    textMode: run !== '' && st.typing && !cardSlot,
    hints,
    view,
  };
}

/** The runs that work on a copy, with the counts of their changes: for the room cards and the desks of the world. */
export function useCopies(api: PaddockApi, intervalMs: number): [CopyRun[] | undefined, () => void] {
  const [copies, setCopies] = useState<CopyRun[] | undefined>(undefined);
  const last = useRef('');
  const read = useCallback(
    () =>
      api.copies().then((v) => {
        const text = JSON.stringify(v.runs);
        if (text !== last.current) {
          last.current = text;
          setCopies(v.runs);
        }
        return v.runs;
      }),
    [api],
  );
  useEffect(() => poll(read, intervalMs), [read, intervalMs]);
  return [copies, () => void read().catch(() => undefined)];
}
