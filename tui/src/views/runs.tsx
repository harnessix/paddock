import React from 'react';
import { Box } from 'ink';
import { cells, col, cut, fit } from '../clean.js';
import { age, now, placeLabel, STATE, stateLabel, type RunRow } from '../model.js';
import { Button, List, Pane, T, inner, type ListRow } from '../ui.js';
import { useTheme, type Role } from '../theme.js';

// The backends that paddock knows (src/backend/mod.rs, by_name). The place
// column fits the longest of them, "sandbox seatbelt", at every width, so
// the backend is never cut (SPLIT1 rule). The text column after it takes
// what is left.
const BACKENDS = ['bwrap', 'docker', 'podman', 'srt', 'seatbelt', 'vm'];
export const PLACE_WIDTH = Math.max(...BACKENDS.map((b) => cells(`sandbox ${b}`))) + 1;
/** Under this width, a row has no room for the full state, the place and the agent: see runRow. */
export const COMPACT_WIDTH = 1 + 16 + PLACE_WIDTH + 5;

/** The approvals mode of a run in plain words. */
export function approvalsLabel(a: string): string {
  if (a === 'manual') return 'manual: you approve each step in the agent';
  if (a === 'auto') return 'auto: the agent approves its own steps';
  if (a === 'command') return "the command's own flags";
  return a;
}

export function runRow(r: RunRow, width: number, marked: boolean): ListRow {
  const s = STATE[r.state];
  const t = now();
  const ago = r.since ? age(t - r.since) : '';
  const extra = [r.approvals === 'manual' ? 'manual' : '', r.frozen ? 'frozen' : '', r.cards > 0 ? `${r.cards} card${r.cards > 1 ? 's' : ''}` : '', r.unreadMail > 0 ? `✉ ${r.unreadMail}` : '']
    .filter((x) => x.length > 0)
    .join(' ');
  const what = [r.ticket || r.projectName, extra, r.last].filter((x) => x.length > 0).join(' · ');
  const wide = width >= 100;
  const state = { text: col(stateLabel(r.state), 16), role: s.role, bold: r.state === 'waiting' };
  const place = { text: col(placeLabel(r), PLACE_WIDTH), role: 'muted' as Role };
  if (width < COMPACT_WIDTH) {
    // A very narrow screen: the state in one word, the agent, and where it runs, whole.
    return {
      key: r.id,
      parts: [...(marked ? [{ text: '+' }] : []), { ...state, text: col(`${s.glyph} ${s.word.split(' ')[0] ?? ''}`, 10) }, { text: col(r.harness, 8) }, { ...place, text: placeLabel(r) }, { text: ` ${r.short}`, role: 'muted' }],
    };
  }
  if (width < 60) {
    // A narrow pane (the left pane of the wide layout): where the run runs
    // comes right after its state, so it is never cut (SPLIT1 rule).
    return {
      key: r.id,
      parts: [{ text: marked ? '+' : ' ' }, state, place, { text: col(r.harness, 8) }, { text: col(r.short, 7), role: 'muted' }, { text: col(ago, 7), role: 'muted' }, { text: what }],
    };
  }
  return {
    key: r.id,
    parts: [
      { text: marked ? '+' : ' ' },
      state,
      { text: col(r.short, 7), role: 'muted' },
      { text: col(r.harness, 8) },
      place,
      ...(wide ? [{ text: col(r.projectName, 14) }] : []),
      { text: col(ago, 7), role: 'muted' },
      { text: what },
    ],
  };
}

export function RunsPane(props: {
  runs: RunRow[];
  selected: number;
  marked: Set<string>;
  width: number;
  height: number;
  focused: boolean;
  filter: string;
  title?: string;
  onSelect: (i: number) => void;
  onOpen: (i: number) => void;
  onWheel: (d: 1 | -1) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const live = props.runs.filter((r) => r.live).length;
  const title =
    (props.title ?? `Runs · ${live} live, ${props.runs.length - live} ended`) +
    (props.filter ? ` · filter "${cut(props.filter, 20)}"` : '') +
    (props.runs.length > box.height ? ` · ${props.selected + 1}/${props.runs.length}` : '');
  return (
    <Pane title={title} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel}>
      <List
        rows={props.runs.map((r) => runRow(r, box.width, props.marked.has(r.id)))}
        selected={props.selected}
        width={box.width}
        height={box.height}
        focused={props.focused}
        empty={props.filter ? 'No run matches the filter. Esc clears it.' : 'No runs. Press n to start one.'}
        onSelect={props.onSelect}
        onOpen={props.onOpen}
      />
    </Pane>
  );
}

/** The detail of one run: what paddock knows, then the end of its output. */
export function RunDetail(props: {
  run: RunRow;
  tail: string[];
  tailError: string;
  scroll: number;
  newLines: number;
  width: number;
  height: number;
  focused: boolean;
  readonly: boolean;
  onWheel: (d: 1 | -1) => void;
  onJumpBottom: () => void;
}): React.JSX.Element {
  const theme = useTheme();
  const r = props.run;
  const s = STATE[r.state];
  const t = now();
  const facts: [string, string, Role?][] = [
    ['state', stateLabel(r.state), s.role],
    ['where', `${placeLabel(r)}${r.network ? ` · network ${r.network}` : ''}${r.detached ? ' · detached' : ''}${r.frozen ? ' · frozen' : ''}`],
    ['project', r.project || '?'],
    ...(r.approvals ? [['approve', approvalsLabel(r.approvals)] as [string, string]] : []),
    ['ticket', r.ticket || '(none)'],
    ['model', r.model || '(paddock did not choose one)'],
    [r.live ? 'started' : 'ended', r.since ? `${age(t - r.since)} ago` : '?'],
    ['last', r.last || '(nothing yet)'],
  ];
  if (r.cards > 0) facts.push(['waits', `${r.cards} approval card${r.cards > 1 ? 's' : ''}: press a`, 'warning']);
  if (r.unreadMail > 0) facts.push(['mail', `${r.unreadMail} unread: press m`]);
  for (const w of r.warnings) facts.push(['warning', w, 'warning']);
  const factRows = Math.min(facts.length + 2, Math.max(4, props.height - 7));
  const top = inner(theme.plain, props.width, factRows);
  // Facts that do not fit: the last line says how many, in place of a silent cut.
  const hiddenFacts = facts.length > top.height ? facts.length - Math.max(0, top.height - 1) : 0;
  const shownFacts = hiddenFacts > 0 ? facts.slice(0, Math.max(0, top.height - 1)) : facts;
  const outHeight = props.height - factRows;
  const out = inner(theme.plain, props.width, outHeight);
  const lines = props.tail;
  const end = Math.max(0, lines.length - props.scroll);
  const start = Math.max(0, end - out.height);
  const shown = lines.slice(start, end);
  const pill = props.scroll > 0;
  return (
    <Box flexDirection="column" width={props.width} height={props.height}>
      <Pane title={`Run · ${r.harness}-${r.short} · ${placeLabel(r)}`} focused={props.focused} width={props.width} height={factRows}>
        {shownFacts.map(([k, v, role], i) => (
          <T key={i} role={role ?? 'normal'}>
            {fit(k, 9)}
            {cut(v, Math.max(0, top.width - 9))}
          </T>
        ))}
        {hiddenFacts > 0 ? <T role="muted">{cut(`… ${hiddenFacts} more: ↵ opens the run`, top.width)}</T> : null}
      </Pane>
      <Pane
        title={`Output · last ${lines.length} lines${props.scroll > 0 ? ` · ${props.scroll} up` : ''}`}
        focused={false}
        width={props.width}
        height={outHeight}
        onWheel={props.onWheel}
      >
        {props.tailError ? <T role="error">{cut(props.tailError, out.width)}</T> : null}
        {!props.tailError && lines.length === 0 ? <T role="muted">No output yet.</T> : null}
        {shown.slice(0, pill ? Math.max(0, out.height - 1) : out.height).map((l, i) => (
          <T key={i}>{cut(l, out.width) || ' '}</T>
        ))}
        {pill ? (
          <Box justifyContent="flex-end" width={out.width}>
            <PillButton text={`↓ ${props.newLines > 0 ? `${props.newLines} new · ` : ''}End jumps to the bottom`} onClick={props.onJumpBottom} />
          </Box>
        ) : null}
      </Pane>
    </Box>
  );
}

function PillButton(props: { text: string; onClick: () => void }): React.JSX.Element {
  return <Button label={` ${props.text} `} focused={false} role="accent" onClick={props.onClick} />;
}
