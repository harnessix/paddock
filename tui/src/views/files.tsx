// The file manager of a copy, as it is drawn: the tree with its marks, a
// pane of text for the diff, the action card, the backups and the two
// head rows. The state and the keys are in ../files.tsx.

import React, { useRef } from 'react';
import { Box, Text, type DOMElement } from 'ink';
import { cells, charCells, cut, fit, oneLine } from '../clean.js';
import { t } from '../i18n/index.js';
import { countsWord, cutMiddle, MARKS, type Card, type CardChoice, type MarkId, type TreeRow } from '../copy.js';
import { usePointer } from '../mouse.js';
import { accentColor, roleProps, useTheme, type Role } from '../theme.js';
import { Button, Clickable, Pane, T, inner, scrollTop } from '../ui.js';

/** A piece of a line: its text and how it looks. */
export interface Seg {
  text: string;
  role?: Role;
  bold?: boolean;
  inverse?: boolean;
}

export function Segs(props: { segs: Seg[]; inverse?: boolean; underline?: boolean; bold?: boolean }): React.JSX.Element {
  const theme = useTheme();
  return (
    <Text wrap="truncate-end" inverse={props.inverse} underline={props.underline}>
      {props.segs.map((s, i) => (
        <Text key={i} {...(theme.plain ? {} : roleProps(theme, s.role ?? 'normal'))} bold={s.bold || props.bold || (theme.mono && !theme.plain && (s.role === 'warning' || s.role === 'error'))} inverse={s.inverse && !props.inverse && !theme.plain}>
          {s.text}
        </Text>
      ))}
    </Text>
  );
}

/** The pane of a text: a diff, the files of a folder, a state in words. */
export function TextPane(props: { title: string; footer?: string; lines: Seg[][]; width: number; height: number; focused: boolean; onWheel?: (d: 1 | -1) => void; onClick?: () => void }): React.JSX.Element {
  return (
    <Pane title={props.title} footer={props.footer} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel} onClick={props.onClick}>
      {props.lines.map((l, i) => (
        <Segs key={i} segs={l.length > 0 ? l : [{ text: ' ' }]} />
      ))}
    </Pane>
  );
}

/** The inner size of a pane with a footer. */
export function paneInner(plain: boolean, width: number, height: number): { width: number; height: number } {
  const box = inner(plain, width, height);
  return plain ? { width: box.width, height: Math.max(0, box.height - 1) } : box;
}

// ---- the tree ----

export type RowAct = 'A' | 'K' | 'D';
const BUTTONS: RowAct[] = ['A', 'K', 'D'];
/** The cells of the word of a row, and of the three buttons that take its place. */
export const WORD_W = 9;

/**
 * The word of a row in `n` pieces of `w` cells: the three buttons take
 * their places. A wide character (CJK) never stands across two pieces: the
 * piece before it ends with a space. A word that does not fit ends with an
 * ellipsis in its last piece.
 */
export function wordPieces(word: string, n = 3, w = 3): string[] {
  const out: string[] = [''];
  let used = 0;
  const chars = Array.from(word);
  for (let i = 0; i < chars.length; i++) {
    const ch = chars[i] as string;
    const cw = charCells(ch);
    if (used + cw > w) {
      if (out.length === n) {
        // No room left: the last piece gets the rest, cut with an ellipsis.
        const last = out.pop() as string;
        out.push(fit(cut(last + chars.slice(i).join(''), w), w));
        return out;
      }
      out[out.length - 1] = fit(out[out.length - 1] as string, w);
      out.push('');
      used = 0;
    }
    out[out.length - 1] += ch;
    used += cw;
  }
  out[out.length - 1] = fit(out[out.length - 1] as string, w);
  while (out.length < n) out.push(' '.repeat(w));
  return out;
}

/** The left part of a row and its word, as text: what the design shows. */
export function rowText(r: TreeRow, width: number, selected: boolean, plain: boolean): { left: string; word: string; prefix: number } {
  const marker = selected ? (plain ? '>' : '▸') : ' ';
  const indent = ' '.repeat(Math.max(0, r.depth - 1) * 2);
  if (r.kind === 'file') {
    const m = MARKS[r.mark ?? 'changed'];
    const head = `${marker}${indent} ${m.glyph} `;
    const room = Math.max(1, width - cells(head) - 1 - WORD_W);
    return { left: fit(`${head}${cutMiddle(oneLine(r.name), room)}`, Math.max(0, width - WORD_W)), word: fit(r.writing ? t('fview.writing') : m.word, WORD_W), prefix: cells(head) };
  }
  const head = `${marker}${indent}${r.open ? '▾' : '▸'} `;
  const counts = countsWord(r.counts);
  // The counts stand at the right end. What does not fit in the cells of the word lies before them.
  const over = Math.max(0, cells(counts) - WORD_W);
  const room = Math.max(1, width - cells(head) - 1 - WORD_W - over);
  const left = fit(`${head}${cutMiddle(oneLine(r.name), room)}`, Math.max(0, width - WORD_W - over)) + counts.slice(0, over);
  const rest = counts.slice(over);
  return { left, word: ' '.repeat(Math.max(0, WORD_W - cells(rest))) + rest, prefix: cells(head) };
}

function TreeLine(props: {
  row: TreeRow;
  width: number;
  selected: boolean;
  focused: boolean;
  onSelect: () => void;
  onOpen: () => void;
  onToggle: () => void;
  onAct: (a: RowAct) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const { row: r, width, selected } = props;
  const dir = r.kind !== 'file';
  // The buttons belong to rows with a change: an applied file has none.
  const can = r.items.length > 0;
  const r0 = useRef<DOMElement>(null);
  const r1 = useRef<DOMElement>(null);
  const r2 = useRef<DOMElement>(null);
  const r3 = useRef<DOMElement>(null);
  const r4 = useRef<DOMElement>(null);
  const refs = [r2, r3, r4];
  const shown = useRef(false);
  const click = (a: RowAct) => (): void => (shown.current ? props.onAct(a) : props.onSelect());
  const p0 = usePointer(r0, { onClick: dir ? props.onToggle : props.onSelect, onDoubleClick: props.onOpen });
  const p1 = usePointer(r1, { onClick: props.onSelect, onDoubleClick: props.onOpen });
  const pb = [usePointer(r2, { onClick: click('A') }), usePointer(r3, { onClick: click('K') }), usePointer(r4, { onClick: click('D') })];
  const p = [p0, p1, ...pb];
  const hovered = p.some((x) => x.hovered);
  const buttons = can && (selected || hovered);
  shown.current = buttons;
  const rt = rowText(r, width, selected, theme.plain);
  const pieces = wordPieces(r.kind === 'file' ? (r.writing ? t('fview.writing') : MARKS[r.mark ?? 'changed'].word) : rt.word);
  const inverse = selected && props.focused && !theme.plain;
  const m: MarkId | undefined = r.kind === 'file' ? r.mark : undefined;
  const role: Role = m ? MARKS[m].role : 'normal';
  const quiet = m === 'kept' || m === 'applied';
  const loud = theme.mono && m === 'conflict' && !inverse;
  const accent = selected ? { color: accentColor(theme) } : {};
  const head = Array.from(rt.left).slice(0, rt.prefix).join('');
  const name = rt.left.slice(head.length);
  return (
    <Box flexDirection="row" width={width} height={1} flexShrink={0}>
      <Box ref={r0} flexShrink={0}>
        <Text inverse={inverse} underline={!selected && dir && p0.hovered} wrap="truncate-end">
          <Text {...accent} bold={selected}>
            {head.slice(0, 1)}
          </Text>
          <Segs
            segs={
              dir
                ? [{ text: head.slice(1), bold: true }]
                : [{ text: head.slice(1, -2) }, { text: head.slice(-2, -1), role, bold: selected, inverse: loud }, { text: ' ' }]
            }
          />
        </Text>
      </Box>
      <Box ref={r1} flexShrink={0}>
        <Segs segs={[{ text: name, role: quiet ? 'muted' : 'normal', bold: selected || dir }]} inverse={inverse} underline={hovered && !selected} />
      </Box>
      {BUTTONS.map((b, i) => {
        const ptr = pb[i];
        const piece = buttons ? `[${b}]` : (pieces[i] ?? '   ');
        return (
          <Box key={b} ref={refs[i]} flexShrink={0}>
            {buttons ? (
              <T role="accent" bold={ptr?.hovered || selected} inverse={inverse !== (ptr?.pressed === true)} underline={ptr?.hovered && !ptr.pressed}>
                {piece}
              </T>
            ) : (
              <Segs segs={[{ text: piece, role: dir ? 'muted' : role, inverse: loud && piece.trim().length > 0 }]} inverse={inverse} />
            )}
          </Box>
        );
      })}
    </Box>
  );
}

export function TreePane(props: {
  title: string;
  footer: string;
  rows: TreeRow[];
  selected: number;
  width: number;
  height: number;
  focused: boolean;
  /** The words in the middle of the pane when it has no row. */
  empty: string[];
  onSelect: (i: number) => void;
  onOpen: (i: number) => void;
  onToggle: (i: number) => void;
  onAct: (i: number, a: RowAct) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const top = useRef(0);
  const box = paneInner(theme.plain, props.width, props.height);
  top.current = scrollTop(top.current, props.selected, box.height, props.rows.length);
  const visible = props.rows.slice(top.current, top.current + box.height);
  const pad = Math.max(0, Math.floor((box.height - props.empty.length) / 2));
  return (
    <Pane title={props.title} footer={props.footer} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel}>
      {props.rows.length === 0
        ? [...Array<string>(pad).fill(''), ...props.empty].map((l, i) => (
            <T key={i} role="muted">
              {l || ' '}
            </T>
          ))
        : visible.map((r, i) => {
            const index = top.current + i;
            return (
              <TreeLine
                key={`${r.kind}:${r.key}`}
                row={r}
                width={box.width}
                selected={index === props.selected}
                focused={props.focused}
                onSelect={() => (index === props.selected && r.kind === 'file' ? props.onOpen(index) : props.onSelect(index))}
                onOpen={() => props.onOpen(index)}
                onToggle={() => props.onToggle(index)}
                onAct={(a) => props.onAct(index, a)}
              />
            );
          })}
    </Pane>
  );
}

// ---- the head: the room line and the tabs of the run ----

export type RunTab = 'run' | 'approvals' | 'mail' | 'files';
const RUN_TABS: RunTab[] = ['run', 'approvals', 'mail', 'files'];
/** The word of a tab of the run, in the language of the interface. */
function tabWord(tab: RunTab): string {
  return t(`fview.tab.${tab}`);
}

export function FilesHead(props: { line: string; pointer: string; pointerOk: boolean; width: number; onTab: (t: RunTab) => void; onPointer: () => void }): React.JSX.Element {
  const word = t('fview.pointer', { state: props.pointer });
  // The tabs: ' run ', ' approvals ', ' mail ' and '[files]'.
  const tabsW = RUN_TABS.reduce((n, tab) => n + cells(tabWord(tab)) + 2, 0);
  return (
    <Box flexDirection="column" width={props.width} height={2} flexShrink={0}>
      <T>{cut(` ${props.line}`, props.width)}</T>
      <Box flexDirection="row" width={props.width} height={1} justifyContent="space-between">
        <Box flexDirection="row" flexShrink={0}>
          {RUN_TABS.map((tab) =>
            tab === 'files' ? (
              <T key={tab} role="accent" bold>
                {`[${tabWord(tab)}]`}
              </T>
            ) : (
              <Button key={tab} label={` ${tabWord(tab)} `} focused={false} onClick={() => props.onTab(tab)} />
            ),
          )}
        </Box>
        {props.width >= tabsW - 1 + cells(word) + 2 ? (
          <Clickable onClick={props.onPointer}>
            {(hovered, pointer) => (
              <T role={props.pointerOk ? 'muted' : 'warning'} underline={hovered} inverse={pointer.pressed}>
                {`${word} `}
              </T>
            )}
          </Clickable>
        ) : null}
      </Box>
    </Box>
  );
}

// ---- the action card ----

export interface CardView {
  card: Card;
  /** The key of the focused choice. */
  focus: string;
  /** A ⚠ choice that waits for its second confirm. */
  armed?: { key: string; at: number };
  /** The first item that shows, when the list is longer than the card. */
  top: number;
  /** The command runs. */
  busy: boolean;
  /** Why the last try did not work. */
  error: string;
}

/** A card shows this many files; the rest is a count, and ↑↓ moves the list. */
export const CARD_ITEMS = 8;
const GUTTER = ' ▏ ';

const buttonLabel = (c: CardChoice): string => `[${c.key}] ${c.label}${c.warn ? ' ⚠' : ''}`;

function buttonRows(choices: CardChoice[], width: number): CardChoice[][] {
  const rows: CardChoice[][] = [[]];
  let used = 0;
  for (const c of choices) {
    const w = cells(buttonLabel(c)) + 2;
    if (used + w > width && (rows[rows.length - 1] as CardChoice[]).length > 0) {
      rows.push([]);
      used = 0;
    }
    (rows[rows.length - 1] as CardChoice[]).push(c);
    used += w;
  }
  return rows;
}

/** The lines of a card above its buttons. */
export function cardLines(v: CardView, width: number): Seg[][] {
  const { card } = v;
  const out: Seg[][] = [];
  out.push([
    { text: card.pill, role: card.pillRole ?? 'warning', bold: true, inverse: true },
    { text: cut(`   ${card.rest}`, Math.max(0, width - cells(card.pill))), role: 'muted' },
  ]);
  if (card.question) out.push([{ text: cut(card.question, width), bold: true }]);
  const shown = card.items.slice(v.top, v.top + CARD_ITEMS);
  const nameW = Math.min(Math.max(8, ...shown.map((i) => cells(i.text))), Math.max(8, width - cells(GUTTER) - 2 - 10 - 12));
  for (const it of shown) {
    const m = it.mark ? MARKS[it.mark] : undefined;
    const segs: Seg[] = [{ text: GUTTER, role: 'muted' }];
    if (m) segs.push({ text: `${m.glyph} `, role: m.role });
    segs.push({ text: `${fit(cutMiddle(it.text, nameW), nameW)}  ` });
    if (it.word) segs.push({ text: fit(it.word, 10), role: m?.role ?? 'muted' });
    segs.push({ text: cut(it.stat ?? '', Math.max(0, width - cells(GUTTER) - (m ? 2 : 0) - nameW - 2 - (it.word ? 10 : 0))), role: 'muted' });
    out.push(segs);
  }
  const more = card.items.length - v.top - shown.length;
  if (card.items.length > CARD_ITEMS) out.push([{ text: cut(`${GUTTER}${v.top > 0 ? `${t('fview.card.above', { n: v.top })} · ` : ''}${more > 0 ? `${t('fview.card.more', { n: more })} · ` : ''}${t('fview.card.moves')}`, width), role: 'muted' }]);
  for (const n of card.notes) out.push([{ text: cut(n.text, width), role: n.role ?? 'muted' }]);
  const focus = card.choices.find((c) => c.key === v.focus);
  // What the focused choice does. A card with one choice has nothing to compare.
  if (focus && card.choices.length > 1) out.push([{ text: cut(`${focus.label}: ${focus.note}`, width), role: 'muted' }]);
  if (v.error) out.push([{ text: cut(`✖ ${v.error}`, width), role: 'error', bold: true }]);
  return out;
}

export function cardHeight(v: CardView, width: number, plain: boolean): number {
  const w = inner(plain, width, 3).width;
  return cardLines(v, w).length + buttonRows(v.card.choices, w).length + 1 + (plain ? 1 : 2);
}

export function ActionCard(props: { view: CardView; width: number; onChoose: (c: CardChoice) => void }): React.JSX.Element {
  const theme = useTheme();
  const v = props.view;
  const w = inner(theme.plain, props.width, 3).width;
  const armed = v.armed ? v.card.choices.find((c) => c.key === v.armed?.key) : undefined;
  let hint: React.JSX.Element;
  if (v.busy) hint = <T role="muted">{cut(t('fview.card.busy'), w)}</T>;
  else if (armed)
    hint = (
      <T role="warning" bold>
        {cut(t('fview.card.again', { key: armed.key, label: armed.label }), w)}
      </T>
    );
  else {
    const keys = v.card.choices.filter((c) => c.key.length === 1).map((c) => c.key);
    const esc = v.card.kind === 'pointer' ? t('fview.card.escClose') : t('fview.card.escCancel');
    hint = <T role="muted">{cut(keys.length > 0 ? t('fview.card.keys', { keys: keys.join('/'), esc }) : esc, w)}</T>;
  }
  return (
    <Pane title={v.card.title} focused width={props.width} height={cardHeight(v, props.width, theme.plain)}>
      {cardLines(v, w).map((l, i) => (
        <Segs key={i} segs={l} />
      ))}
      {buttonRows(v.card.choices, w).map((row, i) => (
        <Box key={i} flexDirection="row" gap={2}>
          {row.map((c) => (
            <Button key={c.key} label={buttonLabel(c)} focused={c.key === v.focus} role={c.role} onClick={() => props.onChoose(c)} />
          ))}
        </Box>
      ))}
      {hint}
    </Pane>
  );
}

// ---- the backups ----

export interface BackupLine {
  /** A day ("today") or a backup. */
  kind: 'day' | 'backup';
  key: string;
  text: string;
  /** The words at the right end: "undone". */
  note?: string;
  /** `paddock restore` takes this one. */
  restorable?: boolean;
}

function BackupRow(props: { line: BackupLine; width: number; selected: boolean; onSelect: () => void; onRestore: () => void }): React.JSX.Element {
  const theme = useTheme();
  const l = props.line;
  const a = useRef<DOMElement>(null);
  const b = useRef<DOMElement>(null);
  const pa = usePointer(a, { onClick: props.onSelect, onDoubleClick: props.onRestore });
  const pb = usePointer(b, { onClick: props.onRestore });
  const inverse = props.selected && !theme.plain;
  const tail = l.restorable ? '[R]' : '   ';
  const note = l.note ? ` ${l.note} ` : '';
  const left = fit(`${props.selected ? (theme.plain ? '>' : '▸') : ' '} ${l.text}`, Math.max(0, props.width - 3 - cells(note)));
  return (
    <Box flexDirection="row" width={props.width} height={1} flexShrink={0}>
      <Box ref={a} flexShrink={0}>
        <Segs segs={[{ text: left, role: l.restorable ? 'normal' : 'muted', bold: props.selected }, { text: note, role: 'muted' }]} inverse={inverse} underline={pa.hovered && !props.selected} />
      </Box>
      <Box ref={b} flexShrink={0}>
        <T role={l.restorable ? 'accent' : 'muted'} bold={pb.hovered || props.selected} inverse={inverse !== pb.pressed} underline={pb.hovered && !pb.pressed}>
          {tail}
        </T>
      </Box>
    </Box>
  );
}

export function BackupsPane(props: {
  title: string;
  footer: string;
  lines: BackupLine[];
  /** The index of the selected line. */
  selected: number;
  empty: string;
  width: number;
  height: number;
  onSelect: (i: number) => void;
  onRestore: (i: number) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const top = useRef(0);
  const box = paneInner(theme.plain, props.width, props.height);
  // The day of the first backup stays in view with it.
  top.current = scrollTop(top.current, props.selected, box.height, props.lines.length);
  if (props.selected <= 1) top.current = 0;
  return (
    <Pane title={props.title} footer={props.footer} focused width={props.width} height={props.height} onWheel={props.onWheel}>
      {props.lines.length === 0 ? <T role="muted" wrap="wrap">{props.empty}</T> : null}
      {props.lines.slice(top.current, top.current + box.height).map((l, i) =>
        l.kind === 'day' ? (
          <T key={l.key} role="muted" bold>
            {cut(l.text, box.width)}
          </T>
        ) : (
          <BackupRow key={l.key} line={l} width={box.width} selected={top.current + i === props.selected} onSelect={() => props.onSelect(top.current + i)} onRestore={() => props.onRestore(top.current + i)} />
        ),
      )}
    </Pane>
  );
}
