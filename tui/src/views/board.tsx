import React from 'react';
import { col, cut, oneLine } from '../clean.js';
import { List, Pane, inner, type ListRow } from '../ui.js';
import { useTheme, type Role } from '../theme.js';
import type { Board } from '../types.js';

const BOARD: Record<string, { glyph: string; role: Role }> = {
  waiting: { glyph: '◆', role: 'warning' },
  blocked: { glyph: '■', role: 'error' },
  stuck: { glyph: '▲', role: 'warning' },
  working: { glyph: '●', role: 'success' },
  idle: { glyph: '○', role: 'muted' },
  open: { glyph: '◇', role: 'normal' },
  done: { glyph: '✓', role: 'success' },
  ended: { glyph: '✓', role: 'muted' },
};

export interface BoardLine {
  row: ListRow;
  /** The short id of the run that Enter opens, if any. */
  run?: string;
}

function stateParts(state: string | undefined): ListRow['parts'][number] {
  const s = oneLine(state ?? '?');
  const b = BOARD[s] ?? { glyph: '?', role: 'normal' as Role };
  return { text: col(`${b.glyph} ${s}`, 11), role: b.role, bold: s === 'waiting' };
}

export function boardLines(board: Board | undefined): BoardLine[] {
  const out: BoardLine[] = [];
  if (!board) return out;
  const slots = (board.compile_slots ?? []).map((s) => `${s.slot} ${oneLine(s.holder)}`);
  if ((board.approvals_waiting ?? 0) > 0) {
    out.push({ row: { key: 'approvals', parts: [{ text: `◆ ${board.approvals_waiting} approval${board.approvals_waiting === 1 ? '' : 's'} wait for you: press a`, role: 'warning', bold: true }] } });
  }
  if (slots.length > 0) out.push({ row: { key: 'slots', parts: [{ text: `compile slots: ${slots.join(', ')}`, role: 'muted' }] } });
  for (const p of board.projects) {
    const name = oneLine(p.name);
    const count = (p.tickets?.length ?? 0) + (p.runs_without_ticket?.length ?? 0);
    out.push({ row: { key: `p:${name}:${p.path ?? ''}`, parts: [{ text: `${name} (${count})  `, bold: true }, { text: oneLine(p.path ?? ''), role: 'muted' }] } });
    for (const t of p.tickets ?? []) {
      const runs = (t.runs ?? []).map((r) => oneLine(r.id));
      const last = oneLine(t.runs?.[0]?.last ?? (t.state === 'open' ? 'open' : ''));
      out.push({
        run: runs[0],
        row: {
          key: `t:${name}:${oneLine(t.id)}`,
          parts: [
            { text: '  ' },
            stateParts(t.state),
            { text: col(oneLine(t.id), 9), bold: true },
            { text: col(oneLine(t.title ?? ''), 23) },
            { text: col(runs.join(','), 16), role: 'muted' },
            { text: last },
          ],
        },
      });
    }
    for (const r of p.runs_without_ticket ?? []) {
      out.push({
        run: oneLine(r.id),
        row: {
          key: `r:${name}:${oneLine(r.id)}`,
          parts: [
            { text: '  ' },
            stateParts(r.state),
            { text: col(oneLine(r.id), 9), role: 'muted' },
            { text: col(`${oneLine(r.harness ?? '?')} (no ticket)`, 23) },
            { text: col('', 16) },
            { text: oneLine(r.last ?? '') },
          ],
        },
      });
    }
  }
  return out;
}

export function BoardPane(props: {
  lines: BoardLine[];
  selected: number;
  width: number;
  height: number;
  focused: boolean;
  loaded: boolean;
  onSelect: (i: number) => void;
  onOpen: (i: number) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  return (
    <Pane title={cut('Board · projects, tickets and runs', props.width)} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel}>
      <List
        rows={props.lines.map((l) => l.row)}
        selected={props.selected}
        width={box.width}
        height={box.height}
        focused={props.focused}
        empty={props.loaded ? 'The board is empty: no project has a run or a ticket.' : 'Reading paddock board…'}
        onSelect={props.onSelect}
        onOpen={props.onOpen}
      />
    </Pane>
  );
}
