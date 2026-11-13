import React from 'react';
import { cells, col, cut, oneLine } from '../clean.js';
import { t, type Key } from '../i18n/index.js';
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

/** The word of a state of the board: known states in the language of the interface, others as paddock wrote them. */
function stateWord(s: string): string {
  return BOARD[s] ? t(`view.board.state.${s}` as Key) : s;
}

/** The state column: 11 cells, or wider when the words of the language need it. */
function stateWidth(): number {
  return Math.min(16, Math.max(11, ...Object.keys(BOARD).map((s) => cells(`◆ ${stateWord(s)}`) + 2)));
}

function stateParts(state: string | undefined, width: number): ListRow['parts'][number] {
  const s = oneLine(state ?? '?');
  const b = BOARD[s] ?? { glyph: '?', role: 'normal' as Role };
  return { text: col(`${b.glyph} ${stateWord(s)}`, width), role: b.role, bold: s === 'waiting' };
}

export function boardLines(board: Board | undefined): BoardLine[] {
  const out: BoardLine[] = [];
  if (!board) return out;
  const sw = stateWidth();
  const slots = (board.compile_slots ?? []).map((s) => `${s.slot} ${oneLine(s.holder)}`);
  if ((board.approvals_waiting ?? 0) > 0) {
    out.push({ row: { key: 'approvals', parts: [{ text: `◆ ${t('view.board.approvalsWait', { n: board.approvals_waiting ?? 0 })}`, role: 'warning', bold: true }] } });
  }
  if (slots.length > 0) out.push({ row: { key: 'slots', parts: [{ text: t('view.board.compileSlots', { slots: slots.join(', ') }), role: 'muted' }] } });
  for (const p of board.projects) {
    const name = oneLine(p.name);
    const count = (p.tickets?.length ?? 0) + (p.runs_without_ticket?.length ?? 0);
    out.push({ row: { key: `p:${name}:${p.path ?? ''}`, parts: [{ text: `${name} (${count})  `, bold: true }, { text: oneLine(p.path ?? ''), role: 'muted' }] } });
    for (const tk of p.tickets ?? []) {
      const runs = (tk.runs ?? []).map((r) => oneLine(r.id));
      const last = oneLine(tk.runs?.[0]?.last ?? (tk.state === 'open' ? stateWord('open') : ''));
      out.push({
        run: runs[0],
        row: {
          key: `t:${name}:${oneLine(tk.id)}`,
          parts: [
            { text: '  ' },
            stateParts(tk.state, sw),
            { text: col(oneLine(tk.id), 9), bold: true },
            { text: col(oneLine(tk.title ?? ''), 23) },
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
            stateParts(r.state, sw),
            { text: col(oneLine(r.id), 9), role: 'muted' },
            { text: col(t('view.board.noTicket', { harness: oneLine(r.harness ?? '?') }), 23) },
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
    <Pane title={cut(t('view.board.title'), props.width)} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel}>
      <List
        rows={props.lines.map((l) => l.row)}
        selected={props.selected}
        width={box.width}
        height={box.height}
        focused={props.focused}
        empty={props.loaded ? t('view.board.empty') : t('view.board.reading')}
        onSelect={props.onSelect}
        onOpen={props.onOpen}
      />
    </Pane>
  );
}
