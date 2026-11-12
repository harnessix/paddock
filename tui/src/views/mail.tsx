import React from 'react';
import { charCells, cleanText, col, cut, oneLine } from '../clean.js';
import { List, Pane, T, inner, type ListRow } from '../ui.js';
import { useTheme } from '../theme.js';
import type { MailBox, MailMessage } from '../types.js';

function clock(ts: number | undefined): string {
  if (!ts) return '     ';
  const d = new Date(ts * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

export function mailBoxRow(m: MailBox): ListRow {
  const unread = m.unread_up ?? 0;
  return {
    key: m.id,
    parts: [
      { text: col(unread > 0 ? `✉ ${unread} new` : '  read', 11), role: unread > 0 ? 'warning' : 'muted', bold: unread > 0 },
      { text: col(oneLine(m.state ?? '?'), 12), role: 'muted' },
      { text: col(oneLine(m.short ?? ''), 9) },
      { text: col(`from ${oneLine(m.starter ?? '?')}`, 17), role: 'muted' },
      { text: oneLine(m.id) },
    ],
  };
}

export function messageRow(m: MailMessage): ListRow {
  const kind = m.event ? `event ${oneLine(m.event)}` : oneLine(m.kind ?? '');
  return {
    key: `${m.dir ?? ''}:${m.seq}`,
    parts: [
      { text: col(`#${m.seq}`, 7), role: 'muted' },
      { text: col(clock(m.ts), 6), role: 'muted' },
      { text: col(m.dir === 'down' ? '→ agent' : '← agent', 8) },
      { text: col(kind, 15), role: m.kind === 'ask' ? 'warning' : m.kind === 'event' ? 'muted' : 'normal' },
      { text: oneLine(m.text ?? '') },
    ],
  };
}

export function MailListPane(props: {
  title: string;
  rows: ListRow[];
  selected: number;
  width: number;
  height: number;
  focused: boolean;
  empty: string;
  onSelect: (i: number) => void;
  onOpen: (i: number) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  return (
    <Pane title={props.title} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel}>
      <List rows={props.rows} selected={props.selected} width={box.width} height={box.height} focused={props.focused} empty={props.empty} onSelect={props.onSelect} onOpen={props.onOpen} />
    </Pane>
  );
}

/** Wrap text to a width, hard, by characters. */
export function wrapLines(text: string, width: number): string[] {
  const out: string[] = [];
  for (const raw of cleanText(text).split('\n')) {
    if (raw.length === 0) out.push('');
    let line = '';
    let used = 0;
    for (const ch of raw) {
      const w = charCells(ch);
      if (used + w > Math.max(1, width) && line) {
        out.push(line);
        line = '';
        used = 0;
      }
      line += ch;
      used += w;
    }
    if (line) out.push(line);
  }
  return out;
}

export function MessagePane(props: {
  run: string;
  message: MailMessage | undefined;
  error: string;
  scroll: number;
  width: number;
  height: number;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const m = props.message;
  const head = m
    ? [
        `#${m.seq} ${m.dir === 'down' ? 'to the agent' : 'from the agent'} · ${clock(m.ts)} · ${oneLine(m.from ?? '')} · ${m.event ? `event ${oneLine(m.event)}` : oneLine(m.kind ?? '')}`,
        ...(m.reply_to ? [`reply to #${m.reply_to}`] : []),
        ...((m.refs ?? []).length > 0 ? [`about: ${(m.refs ?? []).map(oneLine).join(', ')}`] : []),
        '',
      ]
    : [];
  const body = m ? wrapLines(m.text ?? '', box.width) : [];
  const all = [...head.map((h) => cut(h, box.width)), ...body];
  const start = Math.min(props.scroll, Math.max(0, all.length - box.height));
  return (
    <Pane title={`Message · ${oneLine(props.run)}`} focused width={props.width} height={props.height} onWheel={props.onWheel}>
      {props.error ? <T role="error">{cut(props.error, box.width)}</T> : null}
      {!m && !props.error ? <T role="muted">Reading the message…</T> : null}
      {all.slice(start, start + box.height).map((l, i) => (
        <T key={i} role={i + start < head.length ? 'muted' : 'normal'}>
          {l || ' '}
        </T>
      ))}
    </Pane>
  );
}
