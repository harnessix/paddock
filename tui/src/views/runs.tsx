import React from 'react';
import { Box } from 'ink';
import { cells, col, cut, fit } from '../clean.js';
import { t } from '../i18n/index.js';
import { age, now, placeLabel, STATE, stateLabel, type RunRow } from '../model.js';
import { Button, List, Pane, T, inner, type ListRow } from '../ui.js';
import { useTheme, type Role } from '../theme.js';

// The backends that paddock knows (src/backend/mod.rs, by_name). The place
// column fits the longest of them, "sandbox seatbelt", at every width, so
// the backend is never cut (SPLIT1 rule). The text column after it takes
// what is left. The word "sandbox" is in the language of the interface, so
// the width is measured in cells of that word.
const BACKENDS = ['bwrap', 'docker', 'podman', 'srt', 'seatbelt', 'vm'];
export function placeWidth(): number {
  return Math.max(...BACKENDS.map((b) => cells(t('place.sandboxBackend', { backend: b })))) + 1;
}
/**
 * The state column: the longest state word of the language, with its
 * glyph. 16 cells in English; a longer word (German "wartet auf dich")
 * widens it up to 20 cells, and a still longer one is cut.
 */
export function stateWidth(): number {
  const words = (Object.keys(STATE) as (keyof typeof STATE)[]).map((k) => cells(stateLabel(k)) + 1);
  return Math.min(20, Math.max(16, ...words));
}
/** Under this width, a row has no room for the full state, the place and the agent: see runRow. */
export function compactWidth(): number {
  return 1 + stateWidth() + placeWidth() + 5;
}

/** The approvals mode of a run in plain words. */
export function approvalsLabel(a: string): string {
  if (a === 'manual') return t('view.runs.approvals.manual');
  if (a === 'auto') return t('view.runs.approvals.auto');
  if (a === 'command') return t('view.runs.approvals.command');
  return a;
}

export function runRow(r: RunRow, width: number, marked: boolean): ListRow {
  const s = STATE[r.state];
  const tnow = now();
  const ago = r.since ? age(tnow - r.since) : '';
  const extra = [r.approvals === 'manual' ? t('view.runs.manual') : '', r.frozen ? t('view.runs.frozen') : '', r.cards > 0 ? t('view.runs.cards', { n: r.cards }) : '', r.unreadMail > 0 ? `✉ ${r.unreadMail}` : '']
    .filter((x) => x.length > 0)
    .join(' ');
  const what = [r.ticket || r.projectName, extra, r.last].filter((x) => x.length > 0).join(' · ');
  const wide = width >= 100;
  const state = { text: col(stateLabel(r.state), stateWidth()), role: s.role, bold: r.state === 'waiting' };
  const place = { text: col(placeLabel(r), placeWidth()), role: 'muted' as Role };
  if (width < compactWidth()) {
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
    (props.title ?? t('view.runs.title', { live, ended: props.runs.length - live })) +
    (props.filter ? t('view.runs.filter', { filter: cut(props.filter, 20) }) : '') +
    (props.runs.length > box.height ? ` · ${props.selected + 1}/${props.runs.length}` : '');
  return (
    <Pane title={title} focused={props.focused} width={props.width} height={props.height} onWheel={props.onWheel}>
      <List
        rows={props.runs.map((r) => runRow(r, box.width, props.marked.has(r.id)))}
        selected={props.selected}
        width={box.width}
        height={box.height}
        focused={props.focused}
        empty={props.filter ? t('view.runs.noMatch') : t('view.runs.none')}
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
  /** The changes of the copy of the run, in words. Empty: the run has no copy. */
  copy?: string;
  onWheel: (d: 1 | -1) => void;
  onJumpBottom: () => void;
}): React.JSX.Element {
  const theme = useTheme();
  const r = props.run;
  const s = STATE[r.state];
  const tnow = now();
  const where = [placeLabel(r), r.network ? t('view.runs.network', { network: r.network }) : '', r.detached ? t('view.runs.detached') : '', r.frozen ? t('view.runs.frozen') : '']
    .filter((x) => x.length > 0)
    .join(' · ');
  const facts: [string, string, Role?][] = [
    [t('view.runs.fact.state'), stateLabel(r.state), s.role],
    [t('view.runs.fact.where'), where],
    [t('view.runs.fact.project'), r.project || '?'],
    ...(r.approvals ? [[t('view.runs.fact.approve'), approvalsLabel(r.approvals)] as [string, string]] : []),
    [t('view.runs.fact.ticket'), r.ticket || t('view.runs.noTicket')],
    [t('view.runs.fact.model'), r.model || t('view.runs.noModel')],
    [r.live ? t('view.runs.fact.started') : t('view.runs.fact.ended'), r.since ? t('view.runs.ago', { age: age(tnow - r.since) }) : '?'],
    [t('view.runs.fact.last'), r.last || t('view.runs.nothingYet')],
  ];
  if (r.cards > 0) facts.push([t('view.runs.fact.waits'), t('view.runs.waitingCards', { n: r.cards }), 'warning']);
  if (r.unreadMail > 0) facts.push([t('view.runs.fact.mail'), t('view.runs.unread', { n: r.unreadMail })]);
  if (props.copy) facts.push([t('view.runs.fact.copy'), t('view.runs.copy', { copy: props.copy }), props.copy === t('copy.noOpenChange') ? undefined : 'warning']);
  for (const w of r.warnings) facts.push([t('view.runs.fact.warning'), w, 'warning']);
  // The labels take 9 cells, or more when the words of the language are longer (at most a third of the pane).
  const labelW = Math.max(9, Math.min(Math.floor(props.width / 3), Math.max(...facts.map(([k]) => cells(k) + 1))));
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
      <Pane title={t('view.runs.runTitle', { name: `${r.harness}-${r.short}`, place: placeLabel(r) })} focused={props.focused} width={props.width} height={factRows}>
        {shownFacts.map(([k, v, role], i) => (
          <T key={i} role={role ?? 'normal'}>
            {fit(k, labelW)}
            {cut(v, Math.max(0, top.width - labelW))}
          </T>
        ))}
        {hiddenFacts > 0 ? <T role="muted">{cut(t('view.runs.moreFacts', { n: hiddenFacts }), top.width)}</T> : null}
      </Pane>
      <Pane
        title={`${t('view.runs.outputTitle', { n: lines.length })}${props.scroll > 0 ? t('view.runs.scrolledUp', { n: props.scroll }) : ''}`}
        focused={false}
        width={props.width}
        height={outHeight}
        onWheel={props.onWheel}
      >
        {props.tailError ? <T role="error">{cut(props.tailError, out.width)}</T> : null}
        {!props.tailError && lines.length === 0 ? <T role="muted">{t('view.runs.noOutput')}</T> : null}
        {shown.slice(0, pill ? Math.max(0, out.height - 1) : out.height).map((l, i) => (
          <T key={i}>{cut(l, out.width) || ' '}</T>
        ))}
        {pill ? (
          <Box justifyContent="flex-end" width={out.width}>
            <PillButton text={props.newLines > 0 ? t('view.runs.jumpNew', { n: props.newLines }) : t('view.runs.jump')} onClick={props.onJumpBottom} />
          </Box>
        ) : null}
      </Pane>
    </Box>
  );
}

function PillButton(props: { text: string; onClick: () => void }): React.JSX.Element {
  return <Button label={` ${props.text} `} focused={false} role="accent" onClick={props.onClick} />;
}
