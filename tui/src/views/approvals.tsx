import React from 'react';
import { Box, Text } from 'ink';
import { cells, cut, fit, oneLine } from '../clean.js';
import { age, now } from '../model.js';
import { choicesOf, GUTTER, requestLines, takesText, type Choice } from '../approval.js';
import { Button, Clickable, Pane, T, inner } from '../ui.js';
import { useTheme } from '../theme.js';
import type { Pending } from '../types.js';

export interface CardState {
  /** The key of the focused choice. */
  focus: string;
  /** A wide choice that waits for its second confirm. */
  armed?: { key: string; at: number };
  /** The typed answer of a question, while typing. */
  typing?: string;
}

function chainText(p: Pending): string {
  const chain = (p.started_by ?? []).map(oneLine).filter((x) => x.length > 0);
  return chain.length > 0 ? `started by ${chain.join(' → ')}` : '';
}

export function cardTitle(p: Pending): string {
  return `Approval · ${oneLine(p.sandbox || p.harness || '?')} · sandbox`;
}

type LineKind = 'pill' | 'question' | 'code' | 'text';

interface BodyLine {
  text: string;
  role?: 'muted' | 'warning';
  kind: LineKind;
  /** The pill line: the words after the pill. */
  rest?: string;
}

export const PILL = '◆ Approval needed';

/** The question of a card, in words: who asks for what. */
export function questionOf(p: Pending): string {
  const who = oneLine(p.harness || p.sandbox || 'The agent');
  const cap = oneLine(p.capability ?? '');
  switch (cap) {
    case 'exec':
      return `${who} wants to run a command:`;
    case 'connect':
      return `${who} wants to connect to a host:`;
    case 'ask':
      return `${who} asks you:`;
    case '':
      return `${who} asks for this:`;
    default:
      return `${who} asks for ${cap}:`;
  }
}

/** Seconds left and the whole time of a card with a time limit, else null. */
export function timeLeft(p: Pending, t = now()): { left: number; total: number } | null {
  if (!p.expires) return null;
  const left = Math.max(0, p.expires - t);
  const asked = p.age_s !== undefined ? t - p.age_s : (p.created ?? 0);
  const total = asked > 0 ? p.expires - asked : left;
  return { left, total: Math.max(total, left, 1) };
}

/** The bar of the time left: full when the card is new, empty when it times out. */
export function timeBar(p: Pending, width: number, t = now()): string {
  const tl = timeLeft(p, t);
  if (!tl || width < 8) return '';
  const label = ` ${age(tl.left)} left`;
  const bar = Math.max(1, width - label.length);
  const full = Math.round((bar * tl.left) / tl.total);
  return '━'.repeat(full) + '─'.repeat(bar - full) + label;
}

/** The lines of a card body, before the buttons: the pill, the question, the request in a code box, the reason and the scope. */
function bodyLines(p: Pending, focus: Choice | undefined, width: number, maxRequest: number): BodyLine[] {
  const out: BodyLine[] = [];
  const asked = p.age_s !== undefined ? p.age_s : p.created ? now() - p.created : 0;
  const tl = timeLeft(p);
  out.push({ kind: 'pill', text: PILL, rest: cut(`  ${tl ? `times out in ${age(tl.left)}` : 'no time limit'} · asked ${age(asked)} ago`, Math.max(0, width - cells(PILL))) });
  out.push({ kind: 'question', text: cut(questionOf(p), width) });
  const req = requestLines(p);
  // The sandbox writes the request and its line breaks: each line gets the
  // gutter, so a line such as "[1] Deny" cannot pass for a button.
  const head = `${oneLine(p.capability ?? '?')}: `;
  req.slice(0, maxRequest).forEach((l, i) => out.push({ kind: 'code', text: fit(cut(GUTTER + (i === 0 ? head : '') + l, width), width) }));
  if (req.length > maxRequest) out.push({ kind: 'text', text: `  … ${req.length - maxRequest} more lines: Enter on the card shows all`, role: 'muted' });
  if (p.reason) out.push({ kind: 'text', text: cut(`reason given by the agent: ${oneLine(p.reason)}`, width), role: 'muted' });
  const chain = chainText(p);
  if (chain) out.push({ kind: 'text', text: cut(chain, width), role: 'muted' });
  if (focus) out.push({ kind: 'text', text: cut(`remember: ${focus.scope}`, width), role: 'muted' });
  return out;
}

/** Buttons in rows that fit the width. */
function buttonRows(choices: Choice[], width: number): Choice[][] {
  const rows: Choice[][] = [[]];
  let used = 0;
  for (const c of choices) {
    const w = cells(`[${c.key}] ${c.label}${c.wide ? ' ⚠' : ''}`) + 2;
    if (used + w > width && (rows[rows.length - 1] as Choice[]).length > 0) {
      rows.push([]);
      used = 0;
    }
    (rows[rows.length - 1] as Choice[]).push(c);
    used += w;
  }
  return rows;
}

export function cardHeight(p: Pending, width: number, readonly: boolean, maxRequest = 3): number {
  const choices = choicesOf(p);
  const body = bodyLines(p, choices[0], width, maxRequest).length;
  const buttons = readonly ? 1 : buttonRows(choices, width).length + 1 + (takesText(p) ? 1 : 0);
  const bar = timeLeft(p) ? 1 : 0;
  return body + buttons + bar + 2;
}

export function ApprovalCard(props: {
  pending: Pending;
  state: CardState;
  width: number;
  focused: boolean;
  readonly: boolean;
  maxRequest?: number;
  onChoose: (c: Choice) => void;
  onFocusCard: () => void;
}): React.JSX.Element {
  const theme = useTheme();
  const p = props.pending;
  const choices = choicesOf(p);
  const focus = choices.find((c) => c.key === props.state.focus);
  const w = inner(theme.plain, props.width, 3).width;
  const body = bodyLines(p, focus, w, props.maxRequest ?? 3);
  const height = cardHeight(p, w, props.readonly, props.maxRequest ?? 3);
  const bar = timeBar(p, w);
  const armed = props.state.armed ? choices.find((c) => c.key === props.state.armed?.key) : undefined;
  let hint: React.JSX.Element;
  if (props.readonly) {
    hint = <T role="muted">{cut(`read-only: paddock approvals answer ${oneLine(p.id)}`, w)}</T>;
  } else if (!props.focused) {
    hint = <T role="warning">{cut('Tab or a click puts the focus here; a click on a button answers', w)}</T>;
  } else if (armed) {
    hint = <T role="warning" bold>{cut(`Press ${armed.key} again within 3 s, or hold Enter, to confirm: ${armed.scope}`, w)}</T>;
  } else if (props.state.typing !== undefined) {
    hint = <T role="muted">{cut('↵ send the answer · Esc stop typing', w)}</T>;
  } else {
    hint = (
      <T role="muted">
        {cut(`←→ choose · ↵ answer · ${choices.map((c) => c.key).join('/')} keys${takesText(p) ? ' · t type an answer' : ''} · Esc ${choices.find((c) => c.deny)?.label.toLowerCase() ?? 'deny'}`, w)}
      </T>
    );
  }
  return (
    <Pane title={cardTitle(p)} focused={props.focused} width={props.width} height={height} onClick={props.onFocusCard}>
      {body.map((l, i) => (
        <CardLine key={i} line={l} />
      ))}
      {props.readonly
        ? null
        : buttonRows(choices, w).map((row, i) => (
            <Box key={i} flexDirection="row" gap={2}>
              {row.map((c) => (
                <Button
                  key={c.key}
                  label={`[${c.key}] ${c.label}${c.wide ? ' ⚠' : ''}`}
                  focused={props.focused && c.key === props.state.focus}
                  role={c.deny ? 'error' : c.wide ? 'warning' : 'success'}
                  onClick={() => props.onChoose(c)}
                />
              ))}
            </Box>
          ))}
      {props.state.typing !== undefined ? (
        <T>
          {'answer: '}
          {cut(props.state.typing, Math.max(0, w - 10))}
          <T inverse> </T>
        </T>
      ) : null}
      {hint}
      {bar ? <T role={(timeLeft(p)?.left ?? 0) * 4 < (timeLeft(p)?.total ?? 1) ? 'warning' : 'accent'}>{bar}</T> : null}
    </Pane>
  );
}

function CardLine(props: { line: BodyLine }): React.JSX.Element {
  const theme = useTheme();
  const l = props.line;
  if (l.kind === 'pill') {
    return (
      <T>
        <T role="warning" inverse={!theme.plain} bold>
          {l.text}
        </T>
        <T role="muted">{l.rest ?? ''}</T>
      </T>
    );
  }
  if (l.kind === 'question') return <T bold>{l.text}</T>;
  if (l.kind === 'code') {
    // The dark box of the command: the same dark in both themes.
    if (theme.plain || theme.mono) return <Text>{l.text}</Text>;
    return (
      <Text color="whiteBright" backgroundColor={theme.name === 'light' ? 'black' : 'blackBright'} wrap="truncate-end">
        {l.text}
      </Text>
    );
  }
  return <T role={l.role}>{l.text}</T>;
}

export function ApprovalsView(props: {
  pending: Pending[];
  selected: number;
  states: Map<string, CardState>;
  defaultState: (p: Pending) => CardState;
  width: number;
  height: number;
  readonly: boolean;
  onSelect: (i: number) => void;
  onChoose: (p: Pending, c: Choice) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const { pending, width, height } = props;
  if (pending.length === 0) {
    return (
      <Pane title="Approvals · nothing waits" focused width={width} height={height} onWheel={props.onWheel}>
        <T role="muted">No request waits for an answer. New cards show here and in the header.</T>
      </Pane>
    );
  }
  const sel = Math.min(props.selected, pending.length - 1);
  const p = pending[sel] as Pending;
  const others = pending.length - 1;
  // The selected card in full, the others as one line each, above and below.
  const theme = useTheme();
  const cw = inner(theme.plain, width, 3).width;
  const listRows = Math.min(others, Math.max(0, height - cardHeight(p, cw, props.readonly, 1)));
  let maxRequest = 1;
  for (let m = 8; m >= 1; m--) {
    if (cardHeight(p, cw, props.readonly, m) <= height - listRows) {
      maxRequest = m;
      break;
    }
  }
  const before = pending.slice(0, sel).slice(-Math.ceil(listRows / 2));
  const after = pending.slice(sel + 1).slice(0, listRows - before.length);
  const line = (q: Pending, i: number) => (
    <Clickable key={q.id} width={width} onClick={() => props.onSelect(i)}>
      {(hovered, pointer) => (
        <T role="warning" underline={hovered} inverse={pointer.pressed}>
          {/* In line with the text of the card over it. */}
          {cut(`${theme.plain ? '' : '  '}◆ waits  ${fit(oneLine(q.sandbox ?? '?'), 22)} ${oneLine(q.capability ?? '')}: ${oneLine(q.summary || q.detail || '')}`, width)}
        </T>
      )}
    </Clickable>
  );
  return (
    <Box flexDirection="column" width={width} height={height} overflow="hidden">
      {before.map((q) => line(q, pending.indexOf(q)))}
      <ApprovalCard
        pending={p}
        state={props.states.get(p.id) ?? props.defaultState(p)}
        width={width}
        focused
        readonly={props.readonly}
        maxRequest={maxRequest}
        onChoose={(c) => props.onChoose(p, c)}
        onFocusCard={() => props.onSelect(sel)}
      />
      {after.map((q) => line(q, pending.indexOf(q)))}
    </Box>
  );
}
