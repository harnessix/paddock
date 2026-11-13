// The rooms layout of the runs: one card per run, with its character in
// its room, the agent and project, the state word and the time in the
// state. The words are the same as in the list: the art only adds to them.

import React, { useRef } from 'react';
import { Box, type DOMElement } from 'ink';
import { cut } from '../clean.js';
import { t as tr } from '../i18n/index.js';
import { age, now, placeLabel, STATE, stateLabel, type RunRow } from '../model.js';
import { useClickable } from '../mouse.js';
import { artState, colorOf, spriteFor, SPRITE_H, SPRITE_W } from '../sprites.js';
import { SpriteView } from '../art-layer.js';
import { Clickable, Pane, T, scrollTop } from '../ui.js';
import type { Role } from '../theme.js';

/** A card: the border, the 24 columns of the sprite, a space on each side. */
export const CARD_MIN_W = SPRITE_W + 4;
export const ART_ROWS = Math.ceil(SPRITE_H / 2);
export const TEXT_ROWS = 4;
/** Title row, art, text, bottom border. */
export const CARD_H = 1 + ART_ROWS + TEXT_ROWS + 1;
/** A card without the art, when the screen is too low for it. */
export const COMPACT_H = 1 + TEXT_ROWS + 1;
/** The rooms need this many columns; a narrower screen shows the list. */
export const ROOMS_MIN_COLUMNS = 80;

export interface Grid {
  /** Cards in a row. */
  cols: number;
  /** The width of each card in a row: they fill the width. */
  widths: number[];
  cardH: number;
  art: boolean;
  /** Rows of cards that fit. */
  rows: number;
}

/** Cards in a row at a width. */
export function gridCols(width: number): number {
  return Math.max(1, Math.floor(width / CARD_MIN_W));
}

/** The grid of the cards in an area of the screen. */
/** `more`: one more row of text in each card, the line of the copy. */
export function roomsGrid(width: number, height: number, art: boolean, more = 0): Grid {
  const cols = gridCols(width);
  const base = Math.floor(width / cols);
  const extra = width - base * cols;
  const widths = Array.from({ length: cols }, (_, i) => base + (i < extra ? 1 : 0));
  const withArt = art && height >= CARD_H + more;
  const cardH = (withArt ? CARD_H : COMPACT_H) + more;
  return { cols, widths, cardH, art: withArt, rows: Math.max(1, Math.floor(height / cardH)) };
}

/** One more row in every card, when a run of the grid has changes in its copy. */
export function copyRow(runs: RunRow[], changes: Map<string, string> | undefined): number {
  return changes && runs.some((r) => changes.has(r.id)) ? 1 : 0;
}

/** The lines of text of a card, each cut to the width. */
export function cardLines(r: RunRow, width: number, marked: boolean, t = now()): { text: string; role: Role; bold?: boolean }[] {
  const s = STATE[r.state];
  const inState = r.stateSince > 0 ? ` · ${age(t - r.stateSince)}` : '';
  const extra = [r.frozen ? tr('view.runs.frozen') : '', r.cards > 0 ? tr('view.runs.cards', { n: r.cards }) : '', r.unreadMail > 0 ? `✉ ${r.unreadMail}` : '']
    .filter((x) => x.length > 0)
    .join(' · ');
  return [
    { text: cut(`${marked ? '+ ' : ''}${stateLabel(r.state)}${inState}`, width), role: s.role, bold: r.state === 'waiting' },
    { text: cut([r.harness, r.projectName || '?', r.ticket].filter((x) => x.length > 0).join(' · '), width), role: 'normal' },
    { text: cut([placeLabel(r), extra].filter((x) => x.length > 0).join(' · '), width), role: 'muted' },
    { text: cut(r.last || ' ', width), role: 'muted' },
  ];
}

function RoomCard(props: {
  run: RunRow;
  width: number;
  height: number;
  art: boolean;
  selected: boolean;
  focused: boolean;
  marked: boolean;
  /** The line of the copy: "7 changes · 1 conflict". Undefined: no card of the grid has one. */
  changes?: string;
  onSelect: () => void;
  onOpen: () => void;
  onFiles: () => void;
}): React.JSX.Element {
  const r = props.run;
  const iw = Math.max(0, props.width - 4);
  const sprite = props.art ? spriteFor(colorOf(r.harness), artState(r)) : null;
  return (
    <Pane
      title={`${props.selected ? '› ' : ''}${r.harness}-${r.short}`}
      focused={props.selected && props.focused}
      width={props.width}
      height={props.height}
      onClick={props.onSelect}
      onDoubleClick={props.onOpen}
    >
      {props.art ? (
        <Box width={iw} height={ART_ROWS} justifyContent="center" flexShrink={0}>
          {sprite ? <SpriteView sprite={sprite} /> : null}
        </Box>
      ) : null}
      {cardLines(r, iw, props.marked).map((l, i) => (
        <T key={i} role={l.role} bold={l.bold || (i === 0 && props.selected)}>
          {l.text}
        </T>
      ))}
      {props.changes === undefined ? null : props.changes ? (
        // The changes of the copy wait for you: a click, or f, opens the files.
        <Clickable onClick={props.onFiles} width={iw}>
          {(hovered, pointer) => (
            <T role="warning" bold underline={hovered} inverse={pointer.pressed}>
              {cut(`◆ ${props.changes}`, iw)}
            </T>
          )}
        </Clickable>
      ) : (
        <T> </T>
      )}
    </Pane>
  );
}

export function RoomsPane(props: {
  runs: RunRow[];
  selected: number;
  marked: Set<string>;
  width: number;
  height: number;
  focused: boolean;
  filter: string;
  art: boolean;
  title?: string;
  /** The line of the copy of each run that has open changes. */
  changes?: Map<string, string>;
  onSelect: (i: number) => void;
  onOpen: (i: number) => void;
  onFiles?: (i: number) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const ref = useRef<DOMElement>(null);
  useClickable(ref, { onWheel: props.onWheel });
  const extra = copyRow(props.runs, props.changes);
  const top = useRef(0);
  const { runs, width } = props;
  const grid = roomsGrid(width, Math.max(0, props.height - 1), props.art, extra);
  const total = Math.ceil(runs.length / grid.cols);
  const selRow = Math.floor(props.selected / grid.cols);
  top.current = scrollTop(top.current, selRow, grid.rows, total);
  const live = runs.filter((r) => r.live).length;
  const title =
    (props.title ?? tr('view.runs.title', { live, ended: runs.length - live })) +
    (props.filter ? tr('view.runs.filter', { filter: cut(props.filter, 20) }) : '') +
    (runs.length > 0 ? ` · ${props.selected + 1}/${runs.length}` : '') +
    tr('view.rooms.keys');
  const rows: RunRow[][] = [];
  for (let i = top.current; i < Math.min(total, top.current + grid.rows); i++) rows.push(runs.slice(i * grid.cols, (i + 1) * grid.cols));
  return (
    <Box ref={ref} flexDirection="column" width={width} height={props.height} overflow="hidden">
      <T bold={props.focused} role={props.focused ? 'accent' : 'muted'}>
        {cut(title, width)}
      </T>
      {runs.length === 0 ? <T role="muted">{cut(props.filter ? tr('view.runs.noMatch') : tr('view.runs.none'), width)}</T> : null}
      {rows.map((row, ri) => (
        <Box key={top.current + ri} flexDirection="row" width={width} height={grid.cardH} flexShrink={0}>
          {row.map((r, ci) => {
            const index = (top.current + ri) * grid.cols + ci;
            return (
              <RoomCard
                key={r.id}
                run={r}
                width={grid.widths[ci] as number}
                height={grid.cardH}
                art={grid.art}
                selected={index === props.selected}
                focused={props.focused}
                marked={props.marked.has(r.id)}
                changes={extra ? (props.changes?.get(r.id) ?? '') : undefined}
                onFiles={() => props.onFiles?.(index)}
                onSelect={() => props.onSelect(index)}
                onOpen={() => props.onOpen(index)}
              />
            );
          })}
        </Box>
      ))}
    </Box>
  );
}
