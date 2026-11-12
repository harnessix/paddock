// The interface: one input handler routes every key and mouse report to
// the screen that is open. Views only draw.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { existsSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { Box, Text, useApp, useInput, useWindowSize, type Key } from 'ink';
import { cleanText, cut, oneLine } from './clean.js';
import { poll, useSnapshot } from './data.js';
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
import { ApprovalCard, ApprovalsView, cardHeight, type CardState } from './views/approvals.js';
import { BoardPane, boardLines } from './views/board.js';
import { MailListPane, MessagePane, mailBoxRow, messageRow } from './views/mail.js';
import { FORM_FIELDS, HelpOverlay, helpLines, NewRunForm, nextApprovals, PaletteOverlay, type FormState } from './views/overlays.js';

export type Tab = 'runs' | 'board' | 'approvals' | 'mail';
const TABS: { id: Tab; label: string }[] = [
  { id: 'runs', label: 'Runs' },
  { id: 'board', label: 'Board' },
  { id: 'approvals', label: 'Approvals' },
  { id: 'mail', label: 'Mail' },
];

type Screen = { kind: 'root' } | { kind: 'run'; id: string } | { kind: 'mailbox'; run: string } | { kind: 'message'; run: string; seq: number };
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
  /** Override the terminal size (tests). */
  size?: { columns: number; rows: number };
}

interface Toast {
  text: string;
  role: 'success' | 'error' | 'warning' | 'muted';
  at: number;
}

const WIDE = 120;
const TAIL_LINES = 120;

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
  const [stack, setStack] = useState<Screen[]>([]);
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

  const screen: Screen = stack[stack.length - 1] ?? { kind: 'root' };
  const wide = columns >= WIDE;
  // Cards answered here: hidden until paddock stops listing them (at most 30 s).
  const [answered, setAnswered] = useState(new Map<string, number>());
  const pending: Pending[] = useMemo(
    () => (snap.approvals?.pending ?? []).filter((p) => !(answered.has(p.id) && Date.now() - (answered.get(p.id) ?? 0) < 30_000)),
    [snap.approvals, answered],
  );
  const allRuns = useMemo(() => buildRuns(snap.ps, snap.board, pending, snap.mail ?? []), [snap.ps, snap.board, pending, snap.mail]);
  const runs = useMemo(() => allRuns.filter((r) => matches(r, filter)), [allRuns, filter]);
  const bLines = useMemo(() => boardLines(snap.board), [snap.board]);
  const boxes = useMemo(() => (snap.mail ?? []).filter((m) => !filter || `${m.id} ${m.state ?? ''}`.toLowerCase().includes(filter.toLowerCase())), [snap.mail, filter]);
  const selRun: RunRow | undefined = runs[Math.min(sel.runs, Math.max(0, runs.length - 1))];

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
  const detailRun = screen.kind === 'run' ? allRuns.find((r) => r.id === screen.id) : undefined;

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
          if (alive) setMessages({ run: mailRun, list, error: '' });
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
        say(`Answered ${oneLine(p.sandbox ?? p.id)}: ${text !== undefined ? 'your text' : c.label}`, c.deny ? 'warning' : 'success');
      } catch (e) {
        say(`Could not answer: ${e instanceof Error ? e.message : String(e)}`, 'error');
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
        if (!busyCards.current.has(p.id)) say('A new card came to the front: read it, then answer', 'warning');
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

  const stopRun = async (r: RunRow) => {
    try {
      const out = await api.stop(r.id);
      say(out || `Stopped ${r.harness}-${r.short}`);
    } catch (e) {
      say(`Could not stop: ${e instanceof Error ? e.message : String(e)}`, 'error');
    }
    refresh();
  };

  const openView = async (r: RunRow) => {
    try {
      await api.view(r.id);
      say(`Opened the view of ${r.harness}-${r.short} in a new window`);
    } catch (e) {
      say(`Could not open the view: ${e instanceof Error ? e.message : String(e)}`, 'error');
    }
  };

  const splitView = async () => {
    const ids = marked.size > 0 ? [...marked] : selRun ? [selRun.id] : [];
    if (ids.length === 0) return say('No run is selected', 'warning');
    if (split) {
      try {
        say((await api.split(ids)) || `Split view of ${ids.length} run${ids.length > 1 ? 's' : ''}`);
        setMarked(new Set());
      } catch (e) {
        say(`Could not split: ${e instanceof Error ? e.message : String(e)}`, 'error');
      }
      return;
    }
    const r = allRuns.find((x) => x.id === ids[0]);
    if (r) {
      await openView(r);
      say(`paddock split is not in this paddock: opened the view of ${r.harness}-${r.short} in a new window`, 'warning');
    }
  };

  const submitForm = async (form: FormState) => {
    const [h, p, f] = form.values.map((v) => v.trim()) as [string, string, string];
    const project = resolve(props.cwd ?? process.cwd(), p || '.');
    const file = f ? (isAbsolute(f) ? f : resolve(project, f)) : '';
    let error = '';
    if (!h) error = 'Type the harness, for example claude.';
    else if (!existsSync(project)) error = `The project ${project} does not exist.`;
    else if (!f) error = 'Type the prompt file: the task for the agent.';
    else if (!existsSync(file)) error = `The prompt file ${file} does not exist.`;
    if (error) return setOverlay({ kind: 'form', form: { ...form, error } });
    setOverlay({ kind: 'form', form: { ...form, error: '', busy: true } });
    try {
      const out = await api.run(h, project, file, form.approvals ?? '');
      setOverlay(null);
      say(out || `Started ${h} in ${project}${form.approvals ? ` (${form.approvals} approvals)` : ''}`);
    } catch (e) {
      setOverlay({ kind: 'form', form: { ...form, busy: false, error: e instanceof Error ? e.message : String(e) } });
    }
    refresh();
  };

  const runAction = (id: ActionId) => {
    const a = ACTIONS.find((x) => x.id === id);
    if (!a || (readonly && a.changes)) return;
    setOverlay(null);
    const target = screen.kind === 'run' ? detailRun : tab === 'runs' ? selRun : undefined;
    switch (id) {
      case 'open':
        return openSelected();
      case 'back':
        return goBack();
      case 'new':
        return setOverlay({ kind: 'form', form: { field: 0, values: ['', '', ''], error: '', busy: false } });
      case 'stop':
        if (!target) return say('Select a run first', 'warning');
        if (!target.live) return say('The run has ended', 'warning');
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
        if (!run) return say('Select a run first', 'warning');
        return setLine({ kind: 'compose', run, label: run, text: '' });
      }
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
      case 'refresh':
        refresh();
        return say('Reading paddock again', 'muted');
      case 'quit':
        return exit();
    }
  };

  const widePanes = (): WidePane[] => (pending.length > 0 ? ['approvals', 'runs', 'detail', 'board', 'mail'] : ['runs', 'detail', 'board', 'mail']);
  // With no card, the approvals strip is gone, and so is its focus: a new
  // card must not get the keys that the user meant for the runs.
  const focusPane: WidePane = widePanes().includes(widePane) ? widePane : 'runs';
  useEffect(() => {
    if (pending.length === 0 && widePane === 'approvals') setWidePane('runs');
  }, [pending.length, widePane]);

  const nextTab = (d: 1 | -1) => {
    if (wide && tab === 'runs' && screen.kind === 'root') {
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
    if (tab !== 'runs') return showTab('runs');
    exit();
  };

  // The list that the arrow keys move in.
  const activeList = (): 'runs' | 'board' | 'approvals' | 'mail' | 'messages' | 'tail' | 'message' | null => {
    if (screen.kind === 'run') return 'tail';
    if (screen.kind === 'mailbox') return 'messages';
    if (screen.kind === 'message') return 'message';
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
      else if (bl?.run) say(`Run ${bl.run} is not running and has no record: paddock board shows its last state`, 'muted');
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

  useInput((input, key) => {
    if (isMouseInput(input)) {
      const e = parseMouse(input);
      if (e && mouseOn) dispatcher.current?.dispatch(e);
      return;
    }
    if (key.ctrl && input === 'c') return exit();

    if (confirmStop) {
      const r = confirmStop;
      setConfirmStop(null);
      // Only y stops: a second s (a repeat of the key that asked) keeps the run.
      if (input === 'y') void stopRun(r);
      else say('Not stopped', 'muted');
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
            () => say(`Sent to ${run}`),
            (e: unknown) => say(`Could not send: ${e instanceof Error ? e.message : String(e)}`, 'error'),
          );
        }
        return setLine(null);
      }
      if (line.kind === 'filter') {
        const next = editLine(filter, input, key);
        if (next !== null) {
          setFilter(next);
          setSel((s) => ({ ...s, runs: 0, mail: 0 }));
        }
      } else {
        const next = editLine(line.text, input, key);
        if (next !== null) setLine({ ...line, text: next });
      }
      return;
    }

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

    // The approval card in front takes its keys first.
    const l = activeList();
    const front = l === 'approvals' && screen.kind === 'root' ? frontCard : undefined;
    if (front && cardKeys(front, input, key)) return;

    if (key.upArrow || input === 'k') return move(l, -1);
    if (key.downArrow || input === 'j') return move(l, 1);
    if (key.pageUp) return move(l, -Math.max(1, rows - 6));
    if (key.pageDown) return move(l, Math.max(1, rows - 6));
    if (key.home || input === 'g') return move(l, -Number.MAX_SAFE_INTEGER);
    if (key.end || input === 'G') return move(l, Number.MAX_SAFE_INTEGER);
    if (key.tab) return nextTab(key.shift ? -1 : 1);
    if (key.return) return openSelected();
    if (key.escape || input === 'q') return goBack();
    if (key.ctrl && input === 'p') return runAction('palette');
    if (input === 'm' && screen.kind === 'run' && detailRun) return go({ kind: 'mailbox', run: detailRun.id });
    if (input === 'a' && screen.kind === 'run') return showTab('approvals');
    const byKey: Record<string, ActionId> = {
      n: 'new',
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
      R: 'refresh',
      Q: 'quit',
    };
    const id = byKey[input];
    if (id) runAction(id);
  });

  // ---- drawing ----
  const bodyHeight = Math.max(3, rows - 3);
  const live = allRuns.filter((r) => r.live).length;
  const waiting = pending.length;
  const headLeft = `paddock${readonly ? ' · read-only' : ''} · ${live} running${waiting > 0 ? ` · ${waiting} wait${waiting === 1 ? 's' : ''} for you` : ''}`;
  const unread = liveUnread(snap.mail ?? []);

  const header = (
    <Box width={columns} height={1} flexDirection="row" justifyContent="space-between">
      <Text wrap="truncate-end">
        <T bold>{'paddock'}</T>
        <T>{cut(headLeft.slice('paddock'.length), Math.max(0, columns - 44))}</T>
      </Text>
      <Box flexDirection="row" gap={1} flexShrink={0}>
        {TABS.map((t) => {
          const label = t.id === 'approvals' && waiting > 0 ? `${t.label} ◆${waiting}` : t.id === 'mail' && unread > 0 ? `${t.label} ✉${unread}` : t.label;
          return (
            <Button
              key={t.id}
              label={` ${label} `}
              focused={tab === t.id}
              role={t.id === 'approvals' && waiting > 0 ? 'warning' : 'normal'}
              onClick={() => showTab(t.id)}
            />
          );
        })}
      </Box>
    </Box>
  );

  const runsPane = (w: number, h: number, focused: boolean) => (
    <RunsPane
      runs={runs}
      selected={Math.min(sel.runs, Math.max(0, runs.length - 1))}
      marked={marked}
      width={w}
      height={h}
      focused={focused}
      filter={filter}
      title={snap.loaded ? undefined : 'Runs · reading paddock…'}
      onSelect={(i) => {
        setSel((s) => ({ ...s, runs: i }));
        setWidePane('runs');
      }}
      onOpen={(i) => {
        const r = runs[i];
        if (r) go({ kind: 'run', id: r.id });
      }}
      onWheel={(d) => {
        setSel((s) => ({ ...s, runs: Math.max(0, Math.min(runs.length - 1, s.runs + d * 3)) }));
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
      title={`Mail · ${boxes.length} mailbox${boxes.length === 1 ? '' : 'es'}${unread > 0 ? ` · ${unread} unread` : ''}`}
      rows={boxes.map(mailBoxRow)}
      selected={Math.min(sel.mail, Math.max(0, boxes.length - 1))}
      width={w}
      height={h}
      focused={focused}
      empty={snap.mail ? 'No run has a mailbox yet.' : 'Reading paddock mail…'}
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
  } else if (screen.kind === 'run') {
    body = detailRun ? (
      detail(detailRun, columns, bodyHeight, true)
    ) : (
      <T role="muted">The run is gone from paddock ps and paddock board. Esc goes back.</T>
    );
  } else if (screen.kind === 'mailbox') {
    body = (
      <MailListPane
        title={`Mail · ${oneLine(screen.run)} · ↵ read · w write`}
        rows={messages.list.map(messageRow)}
        selected={Math.min(sel.messages, Math.max(0, messages.list.length - 1))}
        width={columns}
        height={bodyHeight}
        focused
        empty={messages.error || (messages.run === screen.run ? 'No messages yet.' : 'Reading the mailbox…')}
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
  } else if (tab === 'board') {
    body = boardPane(columns, bodyHeight, true);
  } else if (tab === 'approvals') {
    body = approvalsView(columns, bodyHeight);
  } else if (tab === 'mail') {
    body = mailPane(columns, bodyHeight, true);
  } else if (wide) {
    // Wide: a sticky approval strip, runs left, the selected run right, board and mail below.
    const front = frontCard;
    const stripHeight = front ? Math.min(Math.floor(bodyHeight / 3), cardHeight(front, columns - 4, readonly, 2)) : 0;
    const bottom = Math.max(5, Math.floor((bodyHeight - stripHeight) / 3));
    const mid = bodyHeight - stripHeight - bottom;
    const left = Math.min(56, Math.max(36, Math.floor(columns * 0.36)));
    body = (
      <Box flexDirection="column" width={columns} height={bodyHeight}>
        {front ? (
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
        ) : null}
        <Box flexDirection="row" height={mid}>
          {runsPane(left, mid, focusPane === 'runs')}
          {selRun ? (
            detail(selRun, columns - left, mid, focusPane === 'detail')
          ) : (
            <Box width={columns - left} height={mid} paddingX={2}>
              <T role="muted">Select a run to see its detail and output.</T>
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
          {cut(`Stop ${confirmStop.harness}-${confirmStop.short}? Its agent ends.`, Math.max(0, columns - 30))}
        </T>
        <Button label=" y Stop " focused={false} role="error" onClick={() => { setConfirmStop(null); void stopRun(confirmStop); }} />
        <Button label=" n Keep " focused={false} onClick={() => setConfirmStop(null)} />
      </Box>
    );
  } else if (line?.kind === 'filter') {
    status = (
      <T>
        <T role="accent">{'/ '}</T>
        {cut(filter, Math.max(0, columns - 30))}
        <T inverse> </T>
        <T role="muted">{'  ↵ keep · Esc clear'}</T>
      </T>
    );
  } else if (line?.kind === 'compose') {
    status = (
      <T>
        <T role="accent">{cut(`to ${line.label}: `, 30)}</T>
        {cut(line.text, Math.max(0, columns - 52))}
        <T inverse> </T>
        <T role="muted">{'  ↵ send · Esc cancel'}</T>
      </T>
    );
  } else if (toast) {
    status = <T role={toast.role}>{cut(toast.text, columns)}</T>;
  } else if (snap.errors.length > 0) {
    status = <T role="error">{cut(`✖ ${snap.errors[0]}`, columns)}</T>;
  } else {
    const ago = snap.loaded ? Math.max(0, Math.floor((Date.now() - snap.at) / 1000)) : -1;
    status = (
      <T role="muted">
        {cut(
          `${snap.loaded ? `read ${ago} s ago` : 'reading paddock…'}${filter ? ` · filter "${filter}"` : ''}${marked.size > 0 ? ` · ${marked.size} marked for split` : ''}${split === false ? ' · v opens a view window (no paddock split)' : ''}`,
          columns,
        )}
      </T>
    );
  }

  const hints = footerHints(screen, tab, readonly);
  const hintAction = (id: HintAction): (() => void) | undefined => {
    if (id === null) return undefined;
    if (id === 'deny')
      return () => {
        const d = frontCard ? defaultChoice(choicesOf(frontCard)) : undefined;
        if (frontCard && d) choose(frontCard, d);
      };
    return () => runAction(id);
  };
  const mouseNote = mouseOn ? 'hold Shift to select text' : 'M mouse on';
  const footer = (
    <Box width={columns} height={1} flexDirection="row" justifyContent="space-between">
      <Box flexDirection="row" overflow="hidden" flexShrink={1}>
        {hints.map(([k, label], i) => (
          <Clickable key={i} onClick={hintAction(label.id)}>
            {(hovered) => (
              <Text underline={hovered && label.id !== null}>
                <T role="accent" bold>
                  {k}
                </T>
                <T role="muted">{` ${label.text}  `}</T>
              </Text>
            )}
          </Clickable>
        ))}
      </Box>
      {columns >= 60 ? <Box flexShrink={0}><T role="muted">{mouseNote}</T></Box> : null}
    </Box>
  );

  void theme;
  return (
    <MouseProvider enabled={mouseOn} dispatcherRef={dispatcher}>
      <Box flexDirection="column" width={columns} height={rows} overflow="hidden">
        {header}
        <Box height={bodyHeight} flexDirection="column" overflow="hidden">
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

/** What a click on a footer hint does: an action, Deny on the card in front, or nothing (a key hint only). */
type HintAction = ActionId | 'deny' | null;

function footerHints(screen: Screen, tab: Tab, readonly: boolean): [string, { id: HintAction; text: string }][] {
  const h: [string, { id: HintAction; text: string }][] = [];
  const add = (k: string, id: HintAction, text: string, changes = false) => {
    if (!(readonly && changes)) h.push([k, { id, text }]);
  };
  if (screen.kind === 'run') {
    add('Esc', 'back', 'back');
    add('s', 'stop', 'stop', true);
    add('v', 'split', 'split');
    add('w', 'write', 'mail', true);
    add('a', 'approvals', 'approvals');
  } else if (screen.kind === 'mailbox') {
    add('↵', 'open', 'read');
    add('w', 'write', 'write', true);
    add('Esc', 'back', 'back');
  } else if (screen.kind === 'message') {
    add('↑↓', null, 'scroll');
    add('Esc', 'back', 'back');
  } else if (tab === 'approvals') {
    add('1-7', null, 'answer', true);
    add('←→', null, 'choose', true);
    add('Esc', 'deny', 'deny', true);
    add('r', 'runs', 'runs');
  } else {
    add('↵', 'open', 'open');
    add('n', 'new', 'new', true);
    add('s', 'stop', 'stop', true);
    add('v', 'split', 'split');
    add('a', 'approvals', 'approvals');
  }
  add('?', 'help', 'help');
  return h;
}
