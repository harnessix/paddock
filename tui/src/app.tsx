// The interface: one input handler routes every key and mouse report to
// the screen that is open. Views only draw.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { existsSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { Box, Text, useApp, useWindowSize, type Key } from 'ink';
import { useKeys, type Keys } from './keys.js';
import { cells, cleanText, cut, fit, oneLine } from './clean.js';
import { poll, useSnapshot } from './data.js';
import { msUntilChange, useRedrawAfter } from './clock.js';
import { editLine } from './edit.js';
import { buildRuns, liveUnread, matches, type RunRow } from './model.js';
import { isMouseInput, MouseProvider, parseMouse, type MouseDispatcher } from './mouse.js';
import type { PaddockApi } from './paddock.js';
import { choiceForKey, choicesOf, CONFIRM_MIN_MS, CONFIRM_MS, defaultChoice, SETTLE_MS, takesText, type Choice } from './approval.js';
import { ACTIONS, searchActions, visibleActions, type ActionId } from './actions.js';
import { Button, Clickable, T } from './ui.js';
import { useTheme } from './theme.js';
import type { MailMessage, Pending } from './types.js';
import { RunDetail, RunsPane } from './views/runs.js';
import { copyRow, gridCols, roomsGrid, RoomsPane, ROOMS_MIN_COLUMNS } from './views/rooms.js';
import { useArt } from './art-layer.js';
import { WorldPane, type Rect, type WorldHandle } from './views/world.js';
import { buildWorld } from './world/model.js';
import { useWorldExtra } from './world/data.js';
import { FRAMES, Motion } from './world/motion.js';
import { observe } from './world/observe.js';
import { moveSelection, type Hit, type Level, type View } from './world/scene.js';
import type { World } from './world/model.js';
import { inspect } from './world/inspect.js';
import { now } from './model.js';
import { ApprovalCard, ApprovalsView, cardHeight, type CardState } from './views/approvals.js';
import { BoardPane, boardLines } from './views/board.js';
import { MailListPane, MessagePane, mailBoxRow, messageRow } from './views/mail.js';
import { FORM_FIELDS, HelpOverlay, helpLines, NewRunForm, nextApprovals, PaletteOverlay, type FormState } from './views/overlays.js';
import { useHome } from './views/home.js';
import { useCopies, useFiles } from './files.js';
import { changesLine, editorCommand, tildePath } from './copy.js';
import { useHost } from './host.js';
import { t } from './i18n/index.js';

export type Tab = 'runs' | 'board' | 'approvals' | 'mail';
const TABS: { id: Tab; readonly label: string }[] = [
  { id: 'runs', get label() { return t('app.tab.runs'); } },
  { id: 'board', get label() { return t('app.tab.board'); } },
  { id: 'approvals', get label() { return t('app.tab.approvals'); } },
  { id: 'mail', get label() { return t('app.tab.mail'); } },
];

type Screen = { kind: 'root' } | { kind: 'run'; id: string } | { kind: 'files'; id: string } | { kind: 'mailbox'; run: string } | { kind: 'message'; run: string; seq: number };
type Overlay = null | { kind: 'help'; scroll: number } | { kind: 'palette'; query: string; sel: number } | { kind: 'form'; form: FormState };
type LineInput = null | { kind: 'filter' } | { kind: 'compose'; run: string; label: string; text: string };
type WidePane = 'approvals' | 'runs' | 'detail' | 'board' | 'mail';

export interface AppProps {
  api: PaddockApi;
  readonly?: boolean;
  initialMouse?: boolean;
  intervalMs?: number;
  cwd?: string;
  /** Turn the terminal's mouse reports on or off. */
  onMouseChange?: (on: boolean) => void;
  /** Draw every cell of the screen again (another program wrote over it). */
  onRedraw?: () => void;
  /** Override the terminal size (tests). */
  size?: { columns: number; rows: number };
  /** Reduced motion: the world shows each change at once, without a walk, frost that grows or a zoom. */
  reduceMotion?: boolean;
  /** Put a text on the clipboard of the terminal. */
  onCopy?: (text: string) => void;
}

interface Toast {
  text: string;
  role: 'success' | 'error' | 'warning' | 'muted';
  at: number;
}

const WIDE = 120;
const TAIL_LINES = 120;
/** The actions that leave the home screen open: they are its own, or they change no screen. */
const HOME_ACTIONS: ReadonlySet<ActionId> = new Set(['home', 'help', 'palette', 'mouse', 'layout', 'refresh', 'quit']);

function newCount(old: string[], next: string[]): number {
  if (old.length === 0) return 0;
  const last = old[old.length - 1];
  for (let j = next.length - 1; j >= 0; j--) if (next[j] === last) return next.length - 1 - j;
  return next.length;
}

export function App(props: AppProps): React.JSX.Element {
  const { api } = props;
  const readonly = props.readonly === true;
  const theme = useTheme();
  const { exit } = useApp();
  const win = useWindowSize();
  const columns = props.size?.columns ?? win.columns ?? 80;
  const rows = props.size?.rows ?? win.rows ?? 24;
  const [snap, refresh] = useSnapshot(api, props.intervalMs ?? 2000);
  const [tab, setTab] = useState<Tab>('runs');
  const host = useHost();
  // Back from the editor: the files of the run show again.
  const [stack, setStack] = useState<Screen[]>(host.resume ? [{ kind: 'files', id: host.resume.files }] : []);
  const [overlay, setOverlay] = useState<Overlay>(null);
  const [line, setLine] = useState<LineInput>(null);
  const [filter, setFilter] = useState('');
  const [sel, setSel] = useState({ runs: 0, board: 0, mail: 0, messages: 0 });
  const [cards, setCards] = useState(new Map<string, CardState>());
  const [toast, setToast] = useState<Toast | null>(null);
  const [confirmStop, setConfirmStop] = useState<RunRow | null>(null);
  const [marked, setMarked] = useState(new Set<string>());
  const [mouseOn, setMouseOn] = useState(props.initialMouse === true);
  const [tail, setTail] = useState<{ id: string; lines: string[]; error: string; scroll: number; newLines: number }>({ id: '', lines: [], error: '', scroll: 0, newLines: 0 });
  const [messages, setMessages] = useState<{ run: string; list: MailMessage[]; error: string }>({ run: '', list: [], error: '' });
  const [message, setMessage] = useState<{ key: string; m?: MailMessage; error: string; scroll: number }>({ key: '', error: '', scroll: 0 });
  const [widePane, setWidePane] = useState<WidePane>('runs');
  const [split, setSplit] = useState<boolean | null>(null);
  const [harnesses, setHarnesses] = useState<string[]>([]);
  const dispatcher = useRef<MouseDispatcher | null>(null);
  const keysRef = useRef<Keys | null>(null);

  const screen: Screen = stack[stack.length - 1] ?? { kind: 'root' };
  const art = useArt();
  const [roomsOn, setRoomsOn] = useState(true);
  // The rooms: wide enough, colours for the art, not plain. Else the list.
  const rooms = roomsOn && art.enabled && !theme.plain && columns >= ROOMS_MIN_COLUMNS;
  // The split layout of a wide screen: the list left, the detail right.
  const wide = columns >= WIDE && !rooms;
  // Cards answered here: hidden until paddock stops listing them (at most 30 s).
  const [answered, setAnswered] = useState(new Map<string, number>());
  const pending: Pending[] = useMemo(
    () => (snap.approvals?.pending ?? []).filter((p) => !(answered.has(p.id) && Date.now() - (answered.get(p.id) ?? 0) < 30_000)),
    [snap.approvals, answered],
  );
  // After 30 s, a card that paddock still lists shows again.
  useEffect(() => {
    if (answered.size === 0) return;
    const t = setTimeout(() => setAnswered((m) => new Map([...m].filter(([, at]) => Date.now() - at < 30_000))), Math.max(0, Math.min(...answered.values()) + 30_000 - Date.now()) + 10);
    return () => clearTimeout(t);
  }, [answered]);
  const allRuns = useMemo(() => buildRuns(snap.ps, snap.board, pending, snap.mail ?? []), [snap.ps, snap.board, pending, snap.mail]);
  // The world: the board as a ranch. On by default where the terminal has
  // art; L switches to the list, and to the world in plain text without art.
  const [worldOn, setWorldOn] = useState<boolean | null>(null);
  const worldShown = tab === 'board' && (worldOn ?? (art.enabled && !theme.plain && columns >= ROOMS_MIN_COLUMNS));
  const [wview, setWview] = useState<View>({ level: 'ranch', house: '', room: '', sel: '', plumbing: false });
  const [inspector, setInspector] = useState(false);
  const [worldCard, setWorldCard] = useState(false);
  const [zoomFrom, setZoomFrom] = useState<Rect | null>(null);
  const motion = useRef(new Motion(props.reduceMotion === true)).current;
  motion.reduce = props.reduceMotion === true;
  // Nothing can move without art or without a way to draw a step: then each change shows at once.
  motion.still = !art.animates;
  const worldHandle = useRef<WorldHandle>({ hits: [], sel: '' });
  const extra = useWorldExtra(api, worldShown, props.intervalMs ?? 2000);
  // The runs that work on a copy: the line of a room card, the glow of the model on the desk.
  const [copies, readCopies] = useCopies(api, (props.intervalMs ?? 2000) * 5);
  const copyLines = useMemo(() => new Map((copies ?? []).map((c) => [c.id, changesLine(c.counts)] as [string, string]).filter(([, l]) => l.length > 0)), [copies]);
  const world: World = useMemo(() => buildWorld(allRuns, snap.ps, snap.board, pending, { ...extra, copies }), [allRuns, snap.ps, snap.board, pending, extra, copies]);
  // An apply in the files of a run: its parts move into the house at the next look at the world.
  const applied = useRef(new Set<string>());
  // What changed since the last look at the world moves: a walk to the
  // door, frost, a helper's room. Only while the world shows.
  const seenWorld = useRef<World | null>(null);
  const pulsed = useRef(false);
  useEffect(() => {
    if (!worldShown || !snap.loaded) {
      seenWorld.current = null;
      return;
    }
    observe(motion, seenWorld.current, world, art.tick);
    for (const id of applied.current) motion.start('apply', id, art.tick);
    applied.current.clear();
    seenWorld.current = world;
    art.update();
  }, [world, worldShown, snap.loaded, motion, art]);
  const runs = useMemo(() => allRuns.filter((r) => matches(r, filter)), [allRuns, filter]);
  const bLines = useMemo(() => boardLines(snap.board), [snap.board]);
  const boxes = useMemo(() => (snap.mail ?? []).filter((m) => !filter || `${m.id} ${m.state ?? ''}`.toLowerCase().includes(filter.toLowerCase())), [snap.mail, filter]);
  // The selected run, kept by its id: when paddock sorts the runs anew (a
  // run starts to wait, another ends), the selection stays on its run and
  // does not jump to the run that took its place.
  const selRunId = useRef<string | null>(null);
  let runIndex = Math.min(sel.runs, Math.max(0, runs.length - 1));
  if (runs[runIndex]?.id !== selRunId.current) {
    const i = selRunId.current === null ? -1 : runs.findIndex((r) => r.id === selRunId.current);
    if (i >= 0) runIndex = i;
    else selRunId.current = runs[runIndex]?.id ?? null;
  }
  const selRun: RunRow | undefined = runs[runIndex];
  useEffect(() => {
    if (sel.runs !== runIndex) setSel((s) => ({ ...s, runs: runIndex }));
  }, [sel.runs, runIndex]);
  /** Move the selection by `d` runs. It counts from the last move, so several wheel steps in one read add up. */
  const runAt = useRef(0);
  runAt.current = runIndex;
  const moveRun = (d: number): void => {
    const at = Math.max(0, Math.min(runs.length - 1, runAt.current + d));
    runAt.current = at;
    selRunId.current = runs[at]?.id ?? null;
    setSel((s) => ({ ...s, runs: at }));
  };
  /** Select the run at a place in the list. */
  const selectRun = (i: number): void => {
    const at = Math.max(0, Math.min(runs.length - 1, i));
    selRunId.current = runs[at]?.id ?? null;
    setSel((s) => ({ ...s, runs: at }));
  };

  // The card in front, kept by its id, not by its place in the list: when
  // a card goes, the next one must not take its answer. A card that came
  // to the front by itself (a new card, or the one after an answered
  // card) takes no answer for SETTLE_MS, so a repeated key or a second
  // click cannot answer a card that the user did not read.
  const [approvalId, setApprovalId] = useState('');
  const frontIndex = useRef(0);
  const seen = useRef<{ id: string; at: number } | null>(null);
  const busyCards = useRef(new Set<string>());
  // Cards whose answer went through, at once and not after the next
  // render: the lock of a card ends before the render that hides it, and
  // a key in that gap must not send a second answer.
  const doneCards = useRef(new Map<string, number>());
  const isDone = useCallback((id: string): boolean => Date.now() - (doneCards.current.get(id) ?? -Infinity) < 30_000, []);
  let fi = pending.findIndex((p) => p.id === approvalId);
  if (fi < 0) fi = Math.min(frontIndex.current, Math.max(0, pending.length - 1));
  frontIndex.current = fi;
  const frontCard: Pending | undefined = pending[fi];
  if (snap.approvals) {
    const id = frontCard?.id ?? '';
    if (seen.current === null) seen.current = { id, at: 0 };
    else if (seen.current.id !== id) seen.current = { id, at: id ? Date.now() : 0 };
  }
  const pickCard = (i: number) => {
    const p = pending[Math.max(0, Math.min(pending.length - 1, i))];
    if (!p) return;
    frontIndex.current = pending.indexOf(p);
    seen.current = { id: p.id, at: 0 };
    setApprovalId(p.id);
  };
  /** Whether the card may take an answer now. */
  const settled = (p: Pending): boolean =>
    !busyCards.current.has(p.id) &&
    !isDone(p.id) && seen.current?.id === p.id && Date.now() - seen.current.at >= SETTLE_MS;
  const detailRun = screen.kind === 'run' || screen.kind === 'files' ? allRuns.find((r) => r.id === screen.id) : undefined;

  // The words of time on the screen ("12 s", "4 min") change at known
  // moments: draw again then, and not in between.
  useRedrawAfter(
    msUntilChange(Date.now(), {
      since: [...allRuns.flatMap((r) => [r.since, r.stateSince]), ...pending.map((p) => (p.age_s === undefined ? (p.created ?? 0) : 0))],
      until: pending.map((p) => p.expires ?? 0),
      // The bar of the time left moves in small steps.
      at: pending.some((p) => p.expires) ? [Date.now() + 5000] : [],
    }),
  );

  const say = useCallback((text: string, role: Toast['role'] = 'success') => setToast({ text: oneLine(text), role, at: Date.now() }), []);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), toast.role === 'error' ? 8000 : 5000);
    return () => clearTimeout(t);
  }, [toast]);

  useEffect(() => {
    if (readonly) return;
    let alive = true;
    void api.hasSplit().then((v) => alive && setSplit(v));
    void api
      .harnesses()
      .then((h) => alive && setHarnesses(h.filter((x) => x.installed !== false && !x.app).map((x) => oneLine(x.name))))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [api, readonly]);

  // The output tail: of the open run, or of the selected run in the wide layout.
  const tailId = screen.kind === 'run' ? screen.id : wide && tab === 'runs' && screen.kind === 'root' ? (selRun?.id ?? '') : '';
  useEffect(() => {
    if (!tailId) return;
    let alive = true;
    const load = () =>
      api.tail(tailId, TAIL_LINES).then(
        (v) => {
          if (alive)
            setTail((old) => {
            const lines = v.lines.map((l) => cleanText(l).replace(/\n/g, ' ').trimEnd());
            if (old.id !== tailId) return { id: tailId, lines, error: '', scroll: 0, newLines: 0 };
            // The same output as before: nothing to draw.
            if (old.error === '' && old.lines.length === lines.length && old.lines.every((l, i) => l === lines[i])) return old;
            const added = old.scroll > 0 ? newCount(old.lines, lines) : 0;
            return { ...old, lines, error: '', scroll: old.scroll > 0 ? Math.min(old.scroll + added, lines.length) : 0, newLines: old.newLines + added };
          });
          return v;
        },
        (e: unknown) => {
          if (alive) setTail((old) => ({ ...old, id: tailId, error: e instanceof Error ? e.message : String(e) }));
          throw e;
        },
      );
    const stop = poll(load, props.intervalMs ?? 2000);
    return () => {
      alive = false;
      stop();
    };
  }, [api, tailId, props.intervalMs]);

  const mailRun = screen.kind === 'mailbox' ? screen.run : '';
  useEffect(() => {
    if (!mailRun) return;
    let alive = true;
    const load = () =>
      api.mailMessages(mailRun).then(
        (list) => {
          if (alive) setMessages((old) => (old.run === mailRun && old.error === '' && JSON.stringify(old.list) === JSON.stringify(list) ? old : { run: mailRun, list, error: '' }));
          return list;
        },
        (e: unknown) => {
          if (alive) setMessages({ run: mailRun, list: [], error: e instanceof Error ? e.message : String(e) });
          throw e;
        },
      );
    const stop = poll(load, props.intervalMs ?? 2000);
    return () => {
      alive = false;
      stop();
    };
  }, [api, mailRun, props.intervalMs]);

  const msgKey = screen.kind === 'message' ? `${screen.run}#${screen.seq}` : '';
  useEffect(() => {
    if (screen.kind !== 'message') return;
    let alive = true;
    setMessage({ key: msgKey, error: '', scroll: 0 });
    api.mailShow(screen.run, screen.seq).then(
      (m) => alive && setMessage((o) => ({ ...o, key: msgKey, m })),
      (e: unknown) => alive && setMessage((o) => ({ ...o, key: msgKey, error: e instanceof Error ? e.message : String(e) })),
    );
    return () => {
      alive = false;
    };
  }, [api, msgKey]);

  // A card that paddock no longer lists was answered elsewhere or timed out.
  const known = useRef(new Set<string>());
  useEffect(() => {
    if (!snap.approvals) return;
    const ids = new Set((snap.approvals.pending ?? []).map((p) => p.id));
    setAnswered((m) => ([...m.keys()].some((k) => !ids.has(k)) ? new Map([...m].filter(([k]) => ids.has(k))) : m));
    for (const id of doneCards.current.keys()) if (!ids.has(id)) doneCards.current.delete(id);
    for (const id of known.current) if (!ids.has(id)) setCards((m) => (m.has(id) ? new Map([...m].filter(([k]) => k !== id)) : m));
    known.current = ids;
  }, [snap.approvals, pending]);

  const toggleMouse = useCallback(() => {
    setMouseOn((on) => {
      props.onMouseChange?.(!on);
      return !on;
    });
  }, [props]);

  const cardState = useCallback((p: Pending): CardState => cards.get(p.id) ?? { focus: defaultChoice(choicesOf(p))?.key ?? '1' }, [cards]);
  const setCard = (id: string, s: CardState) => setCards((m) => new Map(m).set(id, s));

  const answer = useCallback(
    async (p: Pending, c: Choice, text?: string) => {
      if (readonly || busyCards.current.has(p.id) || isDone(p.id)) return;
      // The card is locked while its answer runs: a second key or click does nothing.
      busyCards.current.add(p.id);
      try {
        await api.answer(p.id, c.args, text ?? c.text);
        doneCards.current.set(p.id, Date.now());
        setAnswered((m) => new Map(m).set(p.id, Date.now()));
        say(t('app.answered', { who: oneLine(p.sandbox ?? p.id), answer: text !== undefined ? t('app.yourText') : c.label }), c.deny ? 'warning' : 'success');
      } catch (e) {
        say(t('app.couldNotAnswer', { error: e instanceof Error ? e.message : String(e) }), 'error');
      } finally {
        busyCards.current.delete(p.id);
      }
      setCards((m) => new Map([...m].filter(([k]) => k !== p.id)));
      refresh();
    },
    [api, readonly, refresh, say, isDone],
  );

  /** A choice from a key, a click or Enter. Wide grants need it twice within 3 s. */
  const choose = useCallback(
    (p: Pending, c: Choice, viaEnter = false) => {
      if (readonly) return;
      if (!settled(p)) {
        if (!busyCards.current.has(p.id)) say(t('app.newCard'), 'warning');
        return;
      }
      const st = cardState(p);
      if (c.wide) {
        const since = st.armed && st.armed.key === c.key ? Date.now() - st.armed.at : -1;
        // A double-click or a key repeat is not a second confirm.
        if (since >= 0 && since < CONFIRM_MIN_MS) return;
        const armed = since >= 0 && since <= CONFIRM_MS;
        if (!armed) {
          setCard(p.id, { ...st, focus: c.key, armed: { key: c.key, at: Date.now() } });
          return;
        }
      }
      void viaEnter;
      setCard(p.id, { ...st, focus: c.key, armed: undefined });
      void answer(p, c);
    },
    // settled() reads refs and the pending list of this render.
    [readonly, cardState, answer, pending, say],
  );

  const go = (s: Screen) => setStack((st) => [...st, s]);
  const back = () => setStack((st) => st.slice(0, -1));
  const showTab = (t: Tab) => {
    setTab(t);
    setStack([]);
    setFilter('');
  };

  /** The files of a run: its copy, the backups and the pointer. */
  const openFiles = (id: string, first?: 'backups' | 'pointer' | 'apply') => {
    if (first) files.intend(first);
    if (home.shown) home.close();
    setStack((st) => {
      const top = st[st.length - 1];
      // From the run screen and back: one stop on the way, not two.
      if (top?.kind === 'files' && top.id === id) return st;
      return [...st, { kind: 'files', id }];
    });
  };

  const pauseRun = async (r: RunRow) => {
    try {
      const out = r.frozen ? await api.thaw(r.id) : await api.freeze(r.id);
      say(out || t(r.frozen ? 'app.resumed' : 'app.paused', { run: `${r.harness}-${r.short}` }));
    } catch (e) {
      say(t(r.frozen ? 'app.couldNotResume' : 'app.couldNotPause', { error: e instanceof Error ? e.message : String(e) }), 'error');
    }
    refresh();
  };

  // ---- the world ----
  const worldActive = worldShown && screen.kind === 'root';
  const worldHit = (id = worldHandle.current.sel): Hit | undefined => worldHandle.current.hits.find((h) => h.id === id);
  /** The run of the selected room, or of the room in view. */
  const worldRun = (): RunRow | undefined => {
    const id = worldHit()?.room ?? (wview.level === 'room' ? wview.room : '');
    return id ? allRuns.find((r) => r.id === id) : undefined;
  };
  /**
   * Move the camera. To a stop below: a zoom from the thing that was
   * opened. To the loft: a pan up. One stop up is the same in reverse: a
   * zoom out, or a pan down from the loft.
   */
  const camera = (next: Partial<View>, from?: Hit) => {
    const to = next.level ?? wview.level;
    const kind = to === 'loft' || wview.level === 'loft' ? 'pan' : 'zoom';
    const back = kind === 'pan' ? wview.level === 'loft' && to !== 'loft' : LEVELS.indexOf(to) < LEVELS.indexOf(wview.level);
    setZoomFrom(from ? { x: from.x, y: from.y, w: from.w, h: from.h } : null);
    motion.start(kind, 'world', art.tick, back ? (kind === 'pan' ? 'down' : 'out') : undefined);
    // The first room that you open: its door, its window and its ladder pulse once, after the zoom.
    if (next.level === 'room' && !pulsed.current) {
      pulsed.current = true;
      motion.start('pulse', 'world', art.tick + FRAMES.zoom);
    }
    setWorldCard(false);
    setWview((v) => ({ ...v, ...next }));
  };
  const worldOpen = (id = worldHandle.current.sel) => {
    const h = worldHit(id);
    if (!h) return;
    if (h.kind === 'room' && h.room && h.house !== undefined) {
      if (wview.level === 'ranch') return camera({ level: 'building', house: h.house, room: '', sel: h.id }, h);
      return camera({ level: 'room', house: h.house, room: h.room, sel: '' }, h);
    }
    if (h.kind === 'loft' && h.house !== undefined) return camera({ level: 'loft', house: h.house, sel: '' }, h);
    if (h.kind === 'sign' && h.house !== undefined) return camera({ level: 'building', house: h.house, room: '', sel: '' }, h);
    if (h.kind === 'door') {
      const i = pending.findIndex((p) => p.sandbox === h.room);
      if (i >= 0 && !readonly) {
        pickCard(i);
        keysRef.current?.clear();
        return setWorldCard(true);
      }
    }
    if (h.kind === 'mail' && h.room) {
      setSel((s) => ({ ...s, messages: Number.MAX_SAFE_INTEGER }));
      return go({ kind: 'mailbox', run: h.room });
    }
    // The model on the desk is the copy: its files open.
    if (h.kind === 'copy' && h.room) return openFiles(h.room);
    setWview((v) => ({ ...v, sel: h.id }));
    setInspector(true);
  };
  /** One stop up. False at the ranch: there is no stop above it. */
  const worldBack = (): boolean => {
    if (worldCard) {
      setWorldCard(false);
      return true;
    }
    if (inspector) {
      setInspector(false);
      return true;
    }
    if (wview.level === 'ranch') return false;
    if (wview.level === 'loft' && wview.room) camera({ level: 'room', sel: `loft:${wview.house}` });
    else if (wview.level === 'loft') camera({ level: 'building', sel: `loft:${wview.house}` });
    else if (wview.level === 'room') camera({ level: 'building', sel: `room:${wview.room}`, room: '' });
    else camera({ level: 'ranch', sel: worldHandle.current.sel.startsWith('room:') ? worldHandle.current.sel : `loft:${wview.house}`, room: '' });
    return true;
  };
  /** Jump to a stop of the camera: 1 the ranch, 2 the building, 3 the room. */
  const worldJump = (level: Level) => {
    const h = worldHit();
    const house = h?.house ?? wview.house;
    const room = h?.room ?? wview.room;
    if (level === 'ranch') return camera({ level, room: '', sel: room ? `room:${room}` : worldHandle.current.sel });
    if (level === 'building' && house) return camera({ level, house, room: '', sel: room ? `room:${room}` : '' });
    if (level === 'room' && house && room) return camera({ level, house, room, sel: '' });
    if (level === 'loft' && house && world.houses.find((x) => x.key === house)?.loft) return camera({ level, house, sel: '' });
    say(level === 'loft' ? t('app.noLoft') : t('app.selectRoom'), 'muted');
  };
  const inspectCommand = (): string => commandOf(world, worldHit());
  // A card that is open in the world belongs to the room in view.
  const worldCardOpen = worldActive && worldCard && wview.level === 'room' && frontCard !== undefined && frontCard.sandbox === wview.room;
  // After the answer the card in front is one of another run: it does not show in this room.
  useEffect(() => {
    if (worldCard && !worldCardOpen) setWorldCard(false);
  }, [worldCard, worldCardOpen]);

  const stopRun = async (r: RunRow) => {
    try {
      const out = await api.stop(r.id);
      say(out || t('app.stopped', { run: `${r.harness}-${r.short}` }));
    } catch (e) {
      say(t('app.couldNotStop', { error: e instanceof Error ? e.message : String(e) }), 'error');
    }
    refresh();
  };

  const openView = async (r: RunRow) => {
    try {
      await api.view(r.id);
      say(t('app.openedView', { run: `${r.harness}-${r.short}` }));
    } catch (e) {
      say(t('app.couldNotOpenView', { error: e instanceof Error ? e.message : String(e) }), 'error');
    }
  };

  const splitView = async () => {
    const ids = marked.size > 0 ? [...marked] : selRun ? [selRun.id] : [];
    if (ids.length === 0) return say(t('app.noRunSelected'), 'warning');
    if (split) {
      try {
        say((await api.split(ids)) || t('app.splitView', { n: ids.length }));
        setMarked(new Set());
      } catch (e) {
        say(t('app.couldNotSplit', { error: e instanceof Error ? e.message : String(e) }), 'error');
      }
      return;
    }
    const r = allRuns.find((x) => x.id === ids[0]);
    if (r) {
      await openView(r);
      say(t('app.noSplit', { run: `${r.harness}-${r.short}` }), 'warning');
    }
  };

  const submitForm = async (form: FormState) => {
    const [h, p, f] = form.values.map((v) => v.trim()) as [string, string, string];
    const project = resolve(props.cwd ?? process.cwd(), p || '.');
    const file = f ? (isAbsolute(f) ? f : resolve(project, f)) : '';
    let error = '';
    if (!h) error = t('app.form.typeHarness');
    else if (!existsSync(project)) error = t('app.form.noProject', { project });
    else if (!f) error = t('app.form.typePrompt');
    else if (!existsSync(file)) error = t('app.form.noPrompt', { file });
    if (error) return setOverlay({ kind: 'form', form: { ...form, error } });
    setOverlay({ kind: 'form', form: { ...form, error: '', busy: true } });
    try {
      const out = await api.run(h, project, file, form.approvals ?? '');
      setOverlay(null);
      say(out || (form.approvals ? t('app.startedMode', { agent: h, project, mode: form.approvals }) : t('app.started', { agent: h, project })));
    } catch (e) {
      setOverlay({ kind: 'form', form: { ...form, busy: false, error: e instanceof Error ? e.message : String(e) } });
    }
    refresh();
  };

  const runAction = (id: ActionId) => {
    const a = ACTIONS.find((x) => x.id === id);
    if (!a || (readonly && a.changes)) return;
    // An action of another screen, from the palette: that screen shows, not the home screen over it.
    if (home.shown && !HOME_ACTIONS.has(id)) home.close();
    setOverlay(null);
    const target = screen.kind === 'run' || screen.kind === 'files' ? detailRun : tab === 'runs' ? selRun : worldActive ? worldRun() : undefined;
    switch (id) {
      case 'open':
        return openSelected();
      case 'back':
        if (worldActive && worldBack()) return;
        return goBack();
      case 'home':
        return home.open();
      case 'new':
        return setOverlay({ kind: 'form', form: { field: 0, values: ['', '', ''], error: '', busy: false } });
      case 'stop':
        if (!target) return say(t('app.selectRun'), 'warning');
        if (!target.live) return say(t('app.runEnded'), 'warning');
        // The question gets its answer from a key pressed after it showed, not from keys typed ahead.
        keysRef.current?.clear();
        return setConfirmStop(target);
      case 'split':
        return void splitView();
      case 'mark':
        if (selRun) setMarked((m) => {
          const n = new Set(m);
          if (n.has(selRun.id)) n.delete(selRun.id);
          else n.add(selRun.id);
          return n;
        });
        return;
      case 'view':
        if (target) void openView(target);
        return;
      case 'write': {
        const run = screen.kind === 'mailbox' ? screen.run : target?.id;
        if (!run) return say(t('app.selectRun'), 'warning');
        return setLine({ kind: 'compose', run, label: run, text: '' });
      }
      case 'files':
      case 'applyAll':
      case 'backups':
      case 'pointer':
        if (!target) return say(t('app.selectRun'), 'warning');
        return openFiles(target.id, id === 'applyAll' ? 'apply' : id === 'files' ? undefined : id);
      case 'runs':
      case 'board':
      case 'approvals':
      case 'mail':
        return showTab(id);
      case 'nextTab':
        return nextTab(1);
      case 'filter':
        return setLine({ kind: 'filter' });
      case 'palette':
        return setOverlay({ kind: 'palette', query: '', sel: 0 });
      case 'help':
        return setOverlay({ kind: 'help', scroll: 0 });
      case 'mouse':
        return toggleMouse();
      case 'world':
        // The one way into the world from any screen: the ranch, also where the board is a list by default.
        setWorldOn(true);
        return showTab('board');
      case 'plumbing':
        if (!worldActive) return say(t('app.plumbingRanch'), 'muted');
        motion.start(wview.plumbing ? 'unplumb' : 'plumb', 'world', art.tick);
        setWview((v) => ({ ...v, plumbing: !v.plumbing }));
        return say(wview.plumbing ? t('app.simpleView') : t('app.plumbingOn'), 'muted');
      case 'inspector':
        if (!worldActive) return say(t('app.inspectorRanch'), 'muted');
        return setInspector((on) => !on);
      case 'pause':
        if (!target) return say(t('app.selectRoom'), 'warning');
        if (!target.live) return say(t('app.runEnded'), 'warning');
        return void pauseRun(target);
      case 'layout':
        if (tab === 'board' && screen.kind === 'root') {
          setWorldOn(!worldShown);
          return say(worldShown ? t('app.boardList') : t('app.boardRanch'), 'muted');
        }
        if (!art.enabled || theme.plain) return say(t('app.noArt'), 'muted');
        if (columns < ROOMS_MIN_COLUMNS) return say(t('app.roomsNeed', { n: ROOMS_MIN_COLUMNS }), 'muted');
        setRoomsOn((on) => !on);
        return say(rooms ? t('app.runsList') : t('app.runsRooms'), 'muted');
      case 'refresh':
        refresh();
        // Also draw the whole screen again: when another program wrote over it, this puts it right.
        props.onRedraw?.();
        return say(t('app.reading'), 'muted');
      case 'quit':
        return exit();
    }
  };

  // The home screen and its pages: what `paddock` alone opens, and H from any screen.
  const home = useHome({
    api,
    readonly,
    cwd: props.cwd ?? process.cwd(),
    runs: allRuns,
    requests: pending.length,
    loaded: snap.loaded,
    say,
    go: (where) => {
      if (where === 'help') return runAction('help');
      if (where === 'requests') return runAction('approvals');
      if (where === 'runs') return runAction('runs');
      // The world is the ranch, where this interface has it. Else the board.
      runAction(ACTIONS.find((a) => (a.id as string) === 'world')?.id ?? 'board');
    },
  });
  const homeShown = home.shown && overlay === null;

  const files = useFiles({
    api,
    run: screen.kind === 'files' ? screen.id : '',
    readonly,
    intervalMs: props.intervalMs ?? 2000,
    columns,
    rows,
    runs: allRuns,
    copies,
    say,
    clearKeys: () => keysRef.current?.clear(),
    onApplied: (id) => applied.current.add(id),
    onChanged: () => {
      readCopies();
      refresh();
    },
    onTab: (t) => {
      const id = screen.kind === 'files' ? screen.id : '';
      if (t === 'approvals') return showTab('approvals');
      // The run screen lies under this tab, or takes its place.
      setStack((st) => {
        const below = st.slice(0, -1);
        const run: Screen[] = below[below.length - 1]?.kind === 'run' ? below : [...below, { kind: 'run', id }];
        return t === 'mail' ? [...run, { kind: 'mailbox', run: id }] : run;
      });
    },
    onEdit: (file) => {
      if (!host.run || screen.kind !== 'files') return false;
      const cmd = editorCommand(process.env, file);
      host.run({ bin: cmd.bin, args: cmd.args, title: `${cmd.bin} ${tildePath(file)}`, pause: 'on-error', resume: { files: screen.id } });
      exit();
      return true;
    },
  });

  const widePanes = (): WidePane[] => {
    if (rooms) return pending.length > 0 && columns >= WIDE ? ['approvals', 'runs'] : ['runs'];
    return pending.length > 0 ? ['approvals', 'runs', 'detail', 'board', 'mail'] : ['runs', 'detail', 'board', 'mail'];
  };
  // With no card, the approvals strip is gone, and so is its focus: a new
  // card must not get the keys that the user meant for the runs.
  const focusPane: WidePane = widePanes().includes(widePane) ? widePane : 'runs';
  useEffect(() => {
    if (pending.length === 0 && widePane === 'approvals') setWidePane('runs');
  }, [pending.length, widePane]);

  const nextTab = (d: 1 | -1) => {
    if ((wide || (rooms && widePanes().length > 1)) && tab === 'runs' && screen.kind === 'root') {
      const panes = widePanes();
      const i = panes.indexOf(focusPane);
      setWidePane(panes[(i + d + panes.length) % panes.length] as WidePane);
      return;
    }
    const i = TABS.findIndex((t) => t.id === tab);
    showTab((TABS[(i + d + TABS.length) % TABS.length] as { id: Tab }).id);
  };

  const goBack = () => {
    if (stack.length > 0) return back();
    if (filter) return setFilter('');
    // Started on the home screen: a first screen goes back to it.
    if (home.enabled) return home.open();
    if (tab !== 'runs') return showTab('runs');
    exit();
  };

  // The list that the arrow keys move in.
  const activeList = (): 'runs' | 'board' | 'approvals' | 'mail' | 'messages' | 'tail' | 'message' | null => {
    if (screen.kind === 'run') return 'tail';
    if (screen.kind === 'files') return null;
    if (screen.kind === 'mailbox') return 'messages';
    if (screen.kind === 'message') return 'message';
    if (tab === 'runs' && rooms && focusPane === 'approvals') return 'approvals';
    if (tab === 'runs' && wide) {
      if (focusPane === 'detail') return 'tail';
      if (focusPane === 'board') return 'board';
      if (focusPane === 'mail') return 'mail';
      if (focusPane === 'approvals') return 'approvals';
    }
    return tab;
  };

  const listLength = (l: ReturnType<typeof activeList>): number => {
    switch (l) {
      case 'runs':
        return runs.length;
      case 'board':
        return bLines.length;
      case 'approvals':
        return pending.length;
      case 'mail':
        return boxes.length;
      case 'messages':
        return messages.list.length;
      default:
        return 0;
    }
  };

  const move = (l: ReturnType<typeof activeList>, d: number) => {
    if (l === 'tail') {
      setTail((t) => {
        const scroll = Math.max(0, Math.min(t.lines.length, t.scroll - d));
        return { ...t, scroll, newLines: scroll === 0 ? 0 : t.newLines };
      });
      return;
    }
    if (l === 'message') {
      setMessage((m) => ({ ...m, scroll: Math.max(0, m.scroll + d) }));
      return;
    }
    if (!l) return;
    if (l === 'approvals') return pickCard(fi + Math.max(-pending.length, Math.min(pending.length, d)));
    if (l === 'runs') return moveRun(d);
    const n = listLength(l);
    setSel((s) => ({ ...s, [l]: Math.max(0, Math.min(n - 1, s[l as keyof typeof s] + d)) }));
  };

  const openSelected = () => {
    const l = activeList();
    if (l === 'runs') {
      if (selRun) go({ kind: 'run', id: selRun.id });
      return;
    }
    if (l === 'board') {
      const bl = bLines[Math.min(sel.board, bLines.length - 1)];
      if (bl?.row.key === 'approvals') return showTab('approvals');
      const r = bl?.run ? allRuns.find((x) => x.short === bl.run) : undefined;
      if (r) go({ kind: 'run', id: r.id });
      else if (bl?.run) say(t('app.noRecord', { run: bl.run }), 'muted');
      return;
    }
    if (l === 'mail') {
      const b = boxes[Math.min(sel.mail, boxes.length - 1)];
      if (b) {
        setSel((s) => ({ ...s, messages: Number.MAX_SAFE_INTEGER }));
        go({ kind: 'mailbox', run: b.id });
      }
      return;
    }
    if (l === 'messages' && screen.kind === 'mailbox') {
      const m = messages.list[Math.min(sel.messages, messages.list.length - 1)];
      if (m) go({ kind: 'message', run: screen.run, seq: m.seq });
      return;
    }
    if (l === 'tail' && wide && screen.kind === 'root' && selRun) go({ kind: 'run', id: selRun.id });
  };

  // Keys of the approval card in front: true when the key was for it.
  const cardKeys = (p: Pending, input: string, key: Key): boolean => {
    const choices = choicesOf(p);
    const st = cardState(p);
    if (st.typing !== undefined) {
      if (key.escape) {
        setCard(p.id, { ...st, typing: undefined });
        return true;
      }
      if (key.return) {
        const text = st.typing.trim();
        if (text && settled(p)) {
          setCard(p.id, { ...st, typing: undefined });
          void answer(p, { key: 't', label: 'text', args: [], scope: '', wide: false, deny: false }, text);
        }
        return true;
      }
      const next = editLine(st.typing, input, key);
      if (next !== null) setCard(p.id, { ...st, typing: next });
      return true;
    }
    if (readonly) return false;
    const i = Math.max(0, choices.findIndex((c) => c.key === st.focus));
    if (key.leftArrow || key.rightArrow) {
      const c = choices[(i + (key.rightArrow ? 1 : -1) + choices.length) % choices.length];
      if (c) setCard(p.id, { ...st, focus: c.key, armed: st.armed?.key === c.key ? st.armed : undefined });
      return true;
    }
    if (key.return) {
      const c = choices[i];
      if (c) choose(p, c, true);
      return true;
    }
    if (key.escape) {
      const d = defaultChoice(choices);
      if (d) choose(p, d);
      return true;
    }
    if (input === 't' && takesText(p)) {
      setCard(p.id, { ...st, typing: '' });
      return true;
    }
    const c = choiceForKey(choices, input);
    if (c) {
      choose(p, c);
      return true;
    }
    return false;
  };

  /** What is not a key: a focus or a mouse report. */
  const report = (input: string): boolean => {
    // Focus reports of the terminal (DECSET 1004): the art rests in the background.
    if (input === '[I' || input === '[O') {
      art.setFocused(input === '[I');
      // In the background the pointer is elsewhere: nothing stays lit.
      if (input === '[O') dispatcher.current?.leave();
      return true;
    }
    if (isMouseInput(input)) {
      const e = parseMouse(input);
      if (e && mouseOn) dispatcher.current?.dispatch(e);
      return true;
    }
    return false;
  };

  const onKey = (input: string, key: Key): void => {
    if (key.ctrl && input === 'c') return exit();
    // Ctrl+L, as in a shell: draw the screen again.
    if (key.ctrl && input === 'l') return props.onRedraw?.();

    if (confirmStop) {
      const r = confirmStop;
      setConfirmStop(null);
      // Only y stops: a second s (a repeat of the key that asked) keeps the run.
      if (input === 'y') void stopRun(r);
      else say(t('app.notStopped'), 'muted');
      return;
    }

    if (line) {
      if (key.escape) {
        if (line.kind === 'filter') setFilter('');
        return setLine(null);
      }
      if (key.return) {
        if (line.kind === 'compose' && line.text.trim()) {
          const { run, text } = line;
          void api.sendMail(run, text).then(
            () => say(t('app.sent', { run })),
            (e: unknown) => say(t('app.couldNotSend', { error: e instanceof Error ? e.message : String(e) }), 'error'),
          );
        }
        return setLine(null);
      }
      if (line.kind === 'filter') {
        const next = editLine(filter, input, key);
        if (next !== null) {
          setFilter(next);
          // A new filter: the first run that matches.
          selRunId.current = null;
          setSel((s) => ({ ...s, runs: 0, mail: 0 }));
        }
      } else {
        const next = editLine(line.text, input, key);
        if (next !== null) setLine({ ...line, text: next });
      }
      return;
    }

    // The home screen takes every key but those of all screens (help, the palette, the mouse, quit).
    if (homeShown && home.onKey(input, key)) return;

    if (overlay?.kind === 'form') {
      const f = overlay.form;
      if (f.busy) return;
      if (key.escape) return setOverlay(null);
      if (key.ctrl && input === 'a') return setOverlay({ kind: 'form', form: { ...f, approvals: nextApprovals(f.approvals), error: '' } });
      if (key.tab || key.downArrow || key.upArrow) {
        const d = key.upArrow || (key.tab && key.shift) ? -1 : 1;
        return setOverlay({ kind: 'form', form: { ...f, field: (f.field + d + FORM_FIELDS.length) % FORM_FIELDS.length } });
      }
      if (key.return) {
        if (f.field < FORM_FIELDS.length - 1) return setOverlay({ kind: 'form', form: { ...f, field: f.field + 1 } });
        return void submitForm(f);
      }
      const v = editLine(f.values[f.field] ?? '', input, key);
      if (v !== null) {
        const values = [...f.values] as FormState['values'];
        values[f.field] = v;
        setOverlay({ kind: 'form', form: { ...f, values, error: '' } });
      }
      return;
    }

    if (overlay?.kind === 'palette') {
      const found = searchActions(visibleActions(readonly), overlay.query);
      if (key.escape) return setOverlay(null);
      if (key.upArrow || key.downArrow) {
        const d = key.upArrow ? -1 : 1;
        return setOverlay({ ...overlay, sel: Math.max(0, Math.min(found.length - 1, overlay.sel + d)) });
      }
      if (key.return) {
        const a = found[Math.min(overlay.sel, found.length - 1)];
        return a ? runAction(a.id) : undefined;
      }
      const q = editLine(overlay.query, input, key);
      if (q !== null) setOverlay({ kind: 'palette', query: q, sel: 0 });
      return;
    }

    if (overlay?.kind === 'help') {
      if (key.escape || input === 'q' || input === '?' || key.return) return setOverlay(null);
      const d = key.upArrow || input === 'k' ? -1 : key.downArrow || input === 'j' ? 1 : key.pageUp ? -10 : key.pageDown ? 10 : 0;
      if (d !== 0) setOverlay({ kind: 'help', scroll: Math.max(0, Math.min(helpLines(readonly, mouseOn).length - 1, overlay.scroll + d)) });
      return;
    }

    // The files of a run: the tree, the diff and the cards take their keys first.
    if (screen.kind === 'files' && !homeShown && files.onKey(input, key)) return;

    if (worldActive) {
      if (worldCardOpen && frontCard) {
        // In the world, Esc closes the card. It does not answer it. While you type an answer, Esc stops the typing.
        if (key.escape && cardState(frontCard).typing === undefined) return setWorldCard(false);
        // The answers stand in a column: up and down choose, as left and right do.
        if (cardKeys(frontCard, input, { ...key, leftArrow: key.leftArrow || key.upArrow, rightArrow: key.rightArrow || key.downArrow })) return;
      }
      const hnd = worldHandle.current;
      const step = (dx: number, dy: number): void => setWview((v) => ({ ...v, sel: moveSelection(hnd.hits, hnd.sel, dx, dy) }));
      if (key.leftArrow) return step(-1, 0);
      if (key.rightArrow) return step(1, 0);
      if (key.upArrow) return step(0, -1);
      if (key.downArrow) return step(0, 1);
      if (key.return) return worldOpen();
      if (key.escape || input === 'q') return worldBack() ? undefined : goBack();
      if (input === 'p') return runAction('plumbing');
      if (input === 'i') return runAction('inspector');
      if (input === 'l') return worldJump('loft');
      if (input === '1') return worldJump('ranch');
      if (input === '2') return worldJump('building');
      if (input === '3') return worldJump('room');
      if (input === 'c') {
        const cmd = inspectCommand();
        if (!cmd) return say(t('app.selectThing'), 'muted');
        if (props.onCopy) {
          props.onCopy(cmd);
          return say(t('app.copied', { command: cmd }));
        }
        return say(cmd, 'muted');
      }
      // The list keys have no list here.
      if (key.pageUp || key.pageDown || key.home || key.end || 'jkhgG'.includes(input)) return;
    }

    // The approval card in front takes its keys first.
    const l = activeList();
    const front = l === 'approvals' && screen.kind === 'root' ? frontCard : undefined;
    if (front && cardKeys(front, input, key)) return;

    if (rooms && l === 'runs' && screen.kind === 'root' && tab === 'runs') {
      // The grid: left and right one card, up and down one row of cards.
      const cols = gridCols(columns);
      if (key.leftArrow || input === 'h') return move(l, -1);
      if (key.rightArrow || input === 'l') return move(l, 1);
      if (key.upArrow || input === 'k') return move(l, -cols);
      if (key.downArrow || input === 'j') return move(l, cols);
      if (key.pageUp) return move(l, -cols * roomsRows());
      if (key.pageDown) return move(l, cols * roomsRows());
    }
    if (key.upArrow || input === 'k') return move(l, -1);
    if (key.downArrow || input === 'j') return move(l, 1);
    if (key.pageUp) return move(l, -Math.max(1, rows - 6));
    if (key.pageDown) return move(l, Math.max(1, rows - 6));
    if (key.home || input === 'g') return move(l, -Number.MAX_SAFE_INTEGER);
    if (key.end || input === 'G') return move(l, Number.MAX_SAFE_INTEGER);
    // On a run, Tab goes to its files.
    if (key.tab && screen.kind === 'run') return openFiles(screen.id);
    if (key.tab) return nextTab(key.shift ? -1 : 1);
    if (key.return) return openSelected();
    if (key.escape || input === 'q') return goBack();
    if (key.ctrl && input === 'p') return runAction('palette');
    if (input === 'm' && (screen.kind === 'run' || screen.kind === 'files') && detailRun) return go({ kind: 'mailbox', run: detailRun.id });
    if (input === 'a' && (screen.kind === 'run' || screen.kind === 'files')) return showTab('approvals');
    const byKey: Record<string, ActionId> = {
      n: 'new',
      f: 'files',
      W: 'world',
      P: 'plumbing',
      I: 'inspector',
      z: 'pause',
      s: 'stop',
      v: 'split',
      ' ': 'mark',
      o: 'view',
      w: 'write',
      r: 'runs',
      b: 'board',
      a: 'approvals',
      m: 'mail',
      '/': 'filter',
      ':': 'palette',
      '?': 'help',
      M: 'mouse',
      L: 'layout',
      R: 'refresh',
      Q: 'quit',
      H: 'home',
    };
    const id = byKey[input];
    if (id) runAction(id);
  };
  // A text field is open: what is typed is text, not commands.
  const typingCard = tab === 'approvals' || focusPane === 'approvals' || worldCardOpen ? frontCard : undefined;
  const textMode = (screen.kind === 'files' && files.textMode) || line !== null || overlay?.kind === 'form' || overlay?.kind === 'palette' || (typingCard !== undefined && cardState(typingCard).typing !== undefined);
  keysRef.current = useKeys({ onKey, textMode: textMode || (homeShown && home.textMode), report, onDrop: () => say(t('app.pasteNowhere'), 'muted') });

  // ---- drawing ----
  const bodyHeight = Math.max(3, rows - 3);
  const live = allRuns.filter((r) => r.live).length;
  const waiting = pending.length;
  const headLeft = ['paddock', readonly ? t('app.head.readonly') : '', t('app.head.running', { n: live }), waiting > 0 ? t('app.head.waiting', { n: waiting }) : '']
    .filter((x) => x !== '')
    .join(' · ');
  const unread = liveUnread(snap.mail ?? []);

  // A tab with a count keeps room for it also when there is none: the tabs
  // stay where they are when a card or a mail comes or goes.
  const tabLabel = (tb: { id: Tab; label: string }): string => {
    if (tb.id !== 'approvals' && tb.id !== 'mail') return tb.label;
    const n = tb.id === 'approvals' ? waiting : unread;
    return fit(`${tb.label}${n > 0 ? ` ${tb.id === 'approvals' ? '◆' : '✉'}${n}` : ''}`, Math.max(cells(tb.label) + 3, cells(tb.label) + 2 + String(n).length));
  };
  // Each tab has a space on both sides, and one cell lies between two tabs.
  const tabsWidth = TABS.reduce((n, tb) => n + cells(tabLabel(tb)) + 2, 0) + TABS.length - 1;

  const header = (
    <Box width={columns} height={1} flexDirection="row" justifyContent="space-between">
      {/* The tabs always fit. The name and the counts take what is left, or nothing. */}
      {columns - tabsWidth >= 'paddock'.length + 1 ? (
        <Text wrap="truncate-end">
          <T bold>{'paddock'}</T>
          <T>{cut(headLeft.slice('paddock'.length), Math.max(0, columns - tabsWidth - 'paddock'.length - 1))}</T>
        </Text>
      ) : (
        <Text> </Text>
      )}
      <Box flexDirection="row" gap={1} flexShrink={0}>
        {TABS.map((tb) => {
          const label = tabLabel(tb);
          return (
            <Button
              key={tb.id}
              label={` ${label} `}
              focused={tab === tb.id}
              role={tb.id === 'approvals' && waiting > 0 ? 'warning' : 'normal'}
              onClick={() => showTab(tb.id)}
            />
          );
        })}
      </Box>
    </Box>
  );

  const runsPane = (w: number, h: number, focused: boolean) => (
    <RunsPane
      runs={runs}
      selected={runIndex}
      marked={marked}
      width={w}
      height={h}
      focused={focused}
      filter={filter}
      title={snap.loaded ? undefined : t('app.runsReading')}
      onSelect={(i) => {
        selectRun(i);
        setWidePane('runs');
      }}
      onOpen={(i) => {
        const r = runs[i];
        if (r) go({ kind: 'run', id: r.id });
      }}
      onWheel={(d) => {
        moveRun(d * 3);
      }}
    />
  );

  // The rooms: a sticky approval strip on a wide screen, the grid under it.
  const front0 = frontCard;
  // The strip is as high as the highest card that waits, so the panes under
  // it do not move when another card comes to the front.
  const stripFor = (width: number): number => Math.min(Math.floor(bodyHeight / 3), Math.max(0, ...pending.map((p) => cardHeight(p, width, readonly, 2))));
  const roomsStrip = rooms && columns >= WIDE && front0 ? stripFor(columns - 4) : 0;
  const roomsHeight = bodyHeight - roomsStrip;
  function roomsRows(): number {
    return roomsGrid(columns, Math.max(0, roomsHeight - 1), art.enabled, copyRow(runs, copyLines)).rows;
  }
  const roomsPane = (focused: boolean) => (
    <RoomsPane
      runs={runs}
      selected={runIndex}
      marked={marked}
      width={columns}
      height={roomsHeight}
      focused={focused}
      filter={filter}
      art={art.enabled}
      changes={copyLines}
      onFiles={(i) => {
        const r = runs[i];
        if (r) openFiles(r.id);
      }}
      title={snap.loaded ? undefined : t('app.runsReading')}
      onSelect={(i) => {
        selectRun(i);
        setWidePane('runs');
      }}
      onOpen={(i) => {
        const r = runs[i];
        if (r) go({ kind: 'run', id: r.id });
      }}
      onWheel={(d) => {
        const cols = gridCols(columns);
        moveRun(d * cols);
      }}
    />
  );

  const detail = (r: RunRow, w: number, h: number, focused: boolean) => (
    <RunDetail
      run={r}
      tail={tail.id === r.id ? tail.lines : []}
      tailError={tail.id === r.id ? tail.error : ''}
      scroll={tail.id === r.id ? tail.scroll : 0}
      newLines={tail.id === r.id ? tail.newLines : 0}
      width={w}
      height={h}
      focused={focused}
      readonly={readonly}
      copy={copyLines.get(r.id) ?? (copies?.some((c) => c.id === r.id) ? t('copy.noOpenChange') : '')}
      onWheel={(d) => setTail((t) => ({ ...t, scroll: Math.max(0, Math.min(t.lines.length, t.scroll - d * 3)), newLines: t.scroll - d * 3 <= 0 ? 0 : t.newLines }))}
      onJumpBottom={() => setTail((t) => ({ ...t, scroll: 0, newLines: 0 }))}
    />
  );

  const boardPane = (w: number, h: number, focused: boolean) => (
    <BoardPane
      lines={bLines}
      selected={Math.min(sel.board, Math.max(0, bLines.length - 1))}
      width={w}
      height={h}
      focused={focused}
      loaded={snap.board !== undefined}
      onSelect={(i) => {
        setSel((s) => ({ ...s, board: i }));
        setWidePane('board');
      }}
      onOpen={(i) => {
        setSel((s) => ({ ...s, board: i }));
        const bl = bLines[i];
        const r = bl?.run ? allRuns.find((x) => x.short === bl.run) : undefined;
        if (r) go({ kind: 'run', id: r.id });
      }}
      onWheel={(d) => setSel((s) => ({ ...s, board: Math.max(0, Math.min(bLines.length - 1, s.board + d * 3)) }))}
    />
  );

  const mailPane = (w: number, h: number, focused: boolean) => (
    <MailListPane
      title={[t('app.tab.mail'), t('app.mailboxes', { n: boxes.length }), unread > 0 ? t('app.unread', { n: unread }) : ''].filter((x) => x !== '').join(' · ')}
      rows={boxes.map(mailBoxRow)}
      selected={Math.min(sel.mail, Math.max(0, boxes.length - 1))}
      width={w}
      height={h}
      focused={focused}
      empty={snap.mail ? t('app.noMailbox') : t('app.readingMail')}
      onSelect={(i) => {
        setSel((s) => ({ ...s, mail: i }));
        setWidePane('mail');
      }}
      onOpen={(i) => {
        const b = boxes[i];
        if (b) {
          setSel((s) => ({ ...s, messages: Number.MAX_SAFE_INTEGER }));
          go({ kind: 'mailbox', run: b.id });
        }
      }}
      onWheel={(d) => setSel((s) => ({ ...s, mail: Math.max(0, Math.min(boxes.length - 1, s.mail + d * 3)) }))}
    />
  );

  const approvalsView = (w: number, h: number) => (
    <ApprovalsView
      pending={pending}
      selected={fi}
      states={cards}
      defaultState={cardState}
      width={w}
      height={h}
      readonly={readonly}
      onSelect={(i) => pickCard(i)}
      onChoose={(p, c) => choose(p, c)}
      onWheel={(d) => pickCard(fi + d)}
    />
  );

  let body: React.JSX.Element;
  if (overlay?.kind === 'help') {
    body = (
      <HelpOverlay
        width={columns}
        height={bodyHeight}
        readonly={readonly}
        mouse={mouseOn}
        scroll={overlay.scroll}
        onClose={() => setOverlay(null)}
        onWheel={(d) => setOverlay({ kind: 'help', scroll: Math.max(0, Math.min(helpLines(readonly, mouseOn).length - 1, overlay.scroll + d * 3)) })}
      />
    );
  } else if (overlay?.kind === 'palette') {
    body = (
      <PaletteOverlay
        width={columns}
        height={bodyHeight}
        readonly={readonly}
        query={overlay.query}
        selected={overlay.sel}
        onSelect={(i) => setOverlay({ ...overlay, sel: i })}
        onRun={(a) => runAction(a.id)}
      />
    );
  } else if (overlay?.kind === 'form') {
    const f = overlay.form;
    body = (
      <NewRunForm
        width={columns}
        height={bodyHeight}
        form={f}
        harnesses={harnesses}
        onField={(i) => setOverlay({ kind: 'form', form: { ...f, field: i } })}
        onSubmit={() => void submitForm(f)}
        onCancel={() => setOverlay(null)}
      />
    );
  } else if (home.shown) {
    // The home screen has no tabs above it: it takes their row too.
    body = home.view(columns, bodyHeight + 1);
  } else if (screen.kind === 'run') {
    body = detailRun ? (
      detail(detailRun, columns, bodyHeight, true)
    ) : (
      <T role="muted">{t('app.runGone')}</T>
    );
  } else if (screen.kind === 'files') {
    body = files.view(columns, bodyHeight);
  } else if (screen.kind === 'mailbox') {
    body = (
      <MailListPane
        title={t('app.mailboxTitle', { run: oneLine(screen.run) })}
        rows={messages.list.map(messageRow)}
        selected={Math.min(sel.messages, Math.max(0, messages.list.length - 1))}
        width={columns}
        height={bodyHeight}
        focused
        empty={messages.error || (messages.run === screen.run ? t('app.noMessages') : t('app.readingMailbox'))}
        onSelect={(i) => setSel((s) => ({ ...s, messages: i }))}
        onOpen={(i) => {
          const m = messages.list[i];
          if (m) go({ kind: 'message', run: screen.run, seq: m.seq });
        }}
        onWheel={(d) => setSel((s) => ({ ...s, messages: Math.max(0, Math.min(messages.list.length - 1, s.messages + d * 3)) }))}
      />
    );
  } else if (screen.kind === 'message') {
    body = (
      <MessagePane
        run={screen.run}
        message={message.key === msgKey ? message.m : undefined}
        error={message.key === msgKey ? message.error : ''}
        scroll={message.scroll}
        width={columns}
        height={bodyHeight}
        onWheel={(d) => setMessage((m) => ({ ...m, scroll: Math.max(0, m.scroll + d * 3) }))}
      />
    );
  } else if (tab === 'board' && worldShown) {
    // The request card lies beside its door, in the picture. Its answers go through the same rules as in the Approvals tab.
    const card = worldCardOpen ? frontCard : undefined;
    const answerClick = (id: string): void => {
      const c = card ? choicesOf(card).find((x) => `choice:${x.key}` === id) : undefined;
      if (card && c) choose(card, c);
    };
    body = (
      <WorldPane
        world={world}
        view={wview}
        width={columns}
        height={bodyHeight}
        motion={motion}
        inspector={inspector}
        card={card ? { pending: card, state: cardState(card) } : null}
        readonly={readonly}
        zoomFrom={zoomFrom}
        t={now()}
        handle={worldHandle}
        onSelect={(id) => (id.startsWith('choice:') ? answerClick(id) : setWview((v) => ({ ...v, sel: id })))}
        onOpen={(id) => (id.startsWith('choice:') ? undefined : worldOpen(id))}
      />
    );
  } else if (tab === 'board') {
    body = boardPane(columns, bodyHeight, true);
  } else if (tab === 'approvals') {
    body = approvalsView(columns, bodyHeight);
  } else if (tab === 'mail') {
    body = mailPane(columns, bodyHeight, true);
  } else if (rooms) {
    body = (
      <Box flexDirection="column" width={columns} height={bodyHeight}>
        {roomsStrip > 0 && front0 ? (
          <Box height={roomsStrip} flexShrink={0} overflow="hidden">
          <ApprovalCard
            pending={front0}
            state={cardState(front0)}
            width={columns}
            focused={focusPane === 'approvals'}
            readonly={readonly}
            maxRequest={2}
            onChoose={(c) => choose(front0, c)}
            onFocusCard={() => {
              pickCard(fi);
              setWidePane('approvals');
            }}
          />
          </Box>
        ) : null}
        {roomsPane(focusPane === 'runs' || roomsStrip === 0)}
      </Box>
    );
  } else if (wide) {
    // Wide: a sticky approval strip, runs left, the selected run right, board and mail below.
    const front = frontCard;
    const stripHeight = front ? stripFor(columns - 4) : 0;
    const bottom = Math.max(5, Math.floor((bodyHeight - stripHeight) / 3));
    const mid = bodyHeight - stripHeight - bottom;
    const left = Math.min(56, Math.max(36, Math.floor(columns * 0.36)));
    body = (
      <Box flexDirection="column" width={columns} height={bodyHeight}>
        {front ? (
          <Box height={stripHeight} flexShrink={0} overflow="hidden">
          <ApprovalCard
            pending={front}
            state={cardState(front)}
            width={columns}
            focused={focusPane === 'approvals'}
            readonly={readonly}
            maxRequest={2}
            onChoose={(c) => choose(front, c)}
            onFocusCard={() => {
              pickCard(fi);
              setWidePane('approvals');
            }}
          />
          </Box>
        ) : null}
        <Box flexDirection="row" height={mid}>
          {runsPane(left, mid, focusPane === 'runs')}
          {selRun ? (
            detail(selRun, columns - left, mid, focusPane === 'detail')
          ) : (
            <Box width={columns - left} height={mid} paddingX={2}>
              <T role="muted">{t('app.selectRunDetail')}</T>
            </Box>
          )}
        </Box>
        <Box flexDirection="row" height={bottom}>
          {boardPane(Math.floor(columns / 2), bottom, focusPane === 'board')}
          {mailPane(columns - Math.floor(columns / 2), bottom, focusPane === 'mail')}
        </Box>
      </Box>
    );
  } else {
    body = runsPane(columns, bodyHeight, true);
  }

  // Row 23: a question, an input line, a toast, or the state of the reads.
  let status: React.JSX.Element;
  if (confirmStop) {
    status = (
      <Box flexDirection="row" gap={2}>
        <T role="warning" bold>
          {cut(t('app.confirmStop', { run: `${confirmStop.harness}-${confirmStop.short}` }), Math.max(0, columns - 30))}
        </T>
        <Button label={` y ${t('app.stopYes')} `} focused={false} role="error" onClick={() => { setConfirmStop(null); void stopRun(confirmStop); }} />
        <Button label={` n ${t('app.stopNo')} `} focused={false} onClick={() => setConfirmStop(null)} />
      </Box>
    );
  } else if (line?.kind === 'filter') {
    status = (
      <T>
        <T role="accent">{'/ '}</T>
        {cut(filter, Math.max(0, columns - 30))}
        <T inverse> </T>
        <T role="muted">{`  ${t('app.filterHint')}`}</T>
      </T>
    );
  } else if (line?.kind === 'compose') {
    status = (
      <T>
        <T role="accent">{cut(t('app.composeTo', { run: line.label }), 30)}</T>
        {cut(line.text, Math.max(0, columns - 52))}
        <T inverse> </T>
        <T role="muted">{`  ${t('app.composeHint')}`}</T>
      </T>
    );
  } else if (toast) {
    status = <T role={toast.role}>{cut(toast.text, columns)}</T>;
  } else if (snap.errors.length > 0) {
    status = <T role="error">{cut(`✖ ${snap.errors[0]}`, columns)}</T>;
  } else {
    status = (
      <T role="muted">
        {cut(
          [
            snap.loaded ? t('app.status.upToDate') : t('app.status.reading'),
            filter ? t('app.status.filter', { filter }) : '',
            marked.size > 0 ? t('app.status.marked', { n: marked.size }) : '',
            split === false && !homeShown ? t('app.status.noSplit') : '',
          ]
            .filter((x) => x !== '')
            .join(' · '),
          columns,
        )}
      </T>
    );
  }

  const hints: Hint[] = worldActive ? worldHints(wview.level, worldCardOpen, readonly) : screen.kind === 'files' ? [...files.hints, ['?', { id: 'help', text: t('app.hint.help') }]] : footerHints(screen, tab, readonly);
  const hintAction = (id: HintAction): (() => void) | undefined => {
    if (id === null) return undefined;
    if (id === 'deny')
      return () => {
        const d = frontCard ? defaultChoice(choicesOf(frontCard)) : undefined;
        if (frontCard && d) choose(frontCard, d);
      };
    return () => runAction(id);
  };
  const mouseNote = mouseOn ? t('app.mouse.on') : t('app.mouse.off');
  // Only whole hints: a hint that does not fit is left out, never cut in
  // the middle (a long language fills the line sooner). The help key stays.
  const allHints: Hint[] = homeShown ? home.hints : hints;
  const hintWidth = ([k, label]: Hint) => cells(k) + cells(label.text) + 3;
  // The cells of a row of hints: the two spaces after the last one do not count.
  const rowWidth = (list: Hint[]) => Math.max(0, list.reduce((n, h) => n + hintWidth(h), 0) - 2);
  const shownHints: Hint[] = (() => {
    if (rowWidth(allHints) <= columns) return allHints;
    const help = allHints.find(([k]) => k === '?');
    const out: Hint[] = [];
    for (const h of allHints) {
      if (h === help) continue;
      if (rowWidth([...out, h, ...(help ? [help] : [])]) > columns) break;
      out.push(h);
    }
    return help ? [...out, help] : out;
  })();
  const hintsWidth = shownHints.reduce((n, h) => n + hintWidth(h), 0);
  const footer = (
    <Box width={columns} height={1} flexDirection="row" justifyContent="space-between">
      <Box flexDirection="row" overflow="hidden" flexShrink={1}>
        {shownHints.map(([k, label], i) => (
          <Clickable key={i} onClick={label.run ?? hintAction(label.id)}>
            {(hovered, pointer) => (
              <Text underline={hovered && (label.id !== null || label.run !== undefined)} inverse={pointer.pressed && (label.id !== null || label.run !== undefined)}>
                <T role="accent" bold>
                  {k}
                </T>
                <T role="muted">{` ${label.text}  `}</T>
              </Text>
            )}
          </Clickable>
        ))}
      </Box>
      {/* The note shows only when the hints leave room for it and two cells of space. */}
      {hintsWidth + 2 + cells(mouseNote) <= columns ? <Box flexShrink={0}><T role="muted">{mouseNote}</T></Box> : null}
    </Box>
  );

  void theme;
  return (
    <MouseProvider enabled={mouseOn} dispatcherRef={dispatcher}>
      <Box flexDirection="column" width={columns} height={rows} overflow="hidden">
        {homeShown ? null : header}
        <Box height={bodyHeight + (homeShown ? 1 : 0)} flexDirection="column" overflow="hidden">
          {body}
        </Box>
        <Box height={1} width={columns} overflow="hidden">
          {status}
        </Box>
        {footer}
      </Box>
    </MouseProvider>
  );
}

/** The first command of the Inspector for a thing: what `c` copies. */
function commandOf(world: World, hit: Hit | undefined): string {
  return inspect(world, hit)?.commands[0] ?? '';
}

/** The stops of the camera, from the top down. The loft is beside them: a pan, not a zoom. */
const LEVELS: readonly Level[] = ['ranch', 'building', 'room', 'loft'];

function worldHints(level: Level, card: boolean, readonly: boolean): Hint[] {
  const h: Hint[] = [];
  if (card) {
    h.push(['1-7', { id: null, text: t('app.hint.answer') }], ['Esc', { id: null, text: t('app.hint.closeTheCard') }]);
    return h;
  }
  h.push(['←→↑↓', { id: null, text: t('app.hint.move') }]);
  h.push(['↵', { id: null, text: level === 'ranch' || level === 'building' ? t('app.hint.zoomIn') : t('app.hint.open') }]);
  h.push(['Esc', { id: 'back', text: level === 'ranch' ? t('app.hint.runs') : t('app.hint.out') }]);
  h.push(['P', { id: 'plumbing', text: t('app.hint.plumbing') }]);
  h.push(['I', { id: 'inspector', text: t('app.hint.inspector') }]);
  if (!readonly && level !== 'loft') h.push(['z', { id: 'pause', text: t('app.hint.pause') }]);
  h.push(['L', { id: 'layout', text: t('app.hint.list') }]);
  h.push(['?', { id: 'help', text: t('app.hint.help') }]);
  return h;
}

/** What a click on a footer hint does: an action, Deny on the card in front, or nothing (a key hint only). */
type HintAction = ActionId | 'deny' | null;
/** A key of the footer: what it shows, and what a click on it does. `run` is for a key that is no action of the list. */
type Hint = [string, { id: HintAction; text: string; run?: () => void }];

function footerHints(screen: Screen, tab: Tab, readonly: boolean): Hint[] {
  const h: Hint[] = [];
  const add = (k: string, id: HintAction, text: string, changes = false) => {
    if (!(readonly && changes)) h.push([k, { id, text }]);
  };
  if (screen.kind === 'run') {
    add('Esc', 'back', t('app.hint.back'));
    add('s', 'stop', t('app.hint.stop'), true);
    add('v', 'split', t('app.hint.split'));
    add('w', 'write', t('app.hint.mail'), true);
    add('a', 'approvals', t('app.hint.approvals'));
    add('f', 'files', t('app.hint.files'));
  } else if (screen.kind === 'mailbox') {
    add('↵', 'open', t('app.hint.read'));
    add('w', 'write', t('app.hint.write'), true);
    add('Esc', 'back', t('app.hint.back'));
  } else if (screen.kind === 'message') {
    add('↑↓', null, t('app.hint.scroll'));
    add('Esc', 'back', t('app.hint.back'));
  } else if (tab === 'approvals') {
    add('1-7', null, t('app.hint.answer'), true);
    add('←→', null, t('app.hint.choose'), true);
    add('Esc', 'deny', t('app.hint.deny'), true);
    add('r', 'runs', t('app.hint.runs'));
  } else {
    add('↵', 'open', t('app.hint.open'));
    add('n', 'new', t('app.hint.new'), true);
    add('s', 'stop', t('app.hint.stop'), true);
    add('v', 'split', t('app.hint.split'));
    add('a', 'approvals', t('app.hint.approvals'));
  }
  add('?', 'help', t('app.hint.help'));
  return h;
}
