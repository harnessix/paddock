import React from 'react';
import { Box } from 'ink';
import { cut, fit } from '../clean.js';
import { searchActions, visibleActions, type Action } from '../actions.js';
import { Button, List, Pane, T, inner } from '../ui.js';
import { useTheme } from '../theme.js';
import type { ApprovalsMode } from '../paddock.js';

export function helpLines(readonly: boolean, mouse: boolean): { key: string; text: string }[] {
  const lines = visibleActions(readonly).map((a) => ({ key: a.key, text: a.label }));
  lines.push({ key: '', text: '' });
  for (const t of [
    '↑↓ j k move · PgUp PgDn page · Home End first and last',
    'Approval card: 1-7 or y s n answer, ←→ choose, ↵ answer, Esc deny',
    'Wide grants (⚠: always, or every host) need the key again within 3 s',
    readonly ? '' : 'New run: Ctrl+A switches the approvals mode (default, manual, auto)',
    mouse ? 'Mouse: click selects, double-click opens, wheel scrolls; hold Shift to select text' : 'The mouse is off: M turns it on',
    readonly ? 'Read-only: the actions that change something are hidden' : '',
  ]) {
    if (t) lines.push({ key: '', text: t });
  }
  return lines;
}

export function HelpOverlay(props: { width: number; height: number; readonly: boolean; mouse: boolean; scroll: number; onClose: () => void; onWheel: (d: 1 | -1) => void }): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const lines = helpLines(props.readonly, props.mouse);
  const top = Math.max(0, Math.min(props.scroll, lines.length - box.height));
  const more = lines.length > box.height;
  const title = `Help · every key${more ? ` · ↑↓ scroll ${top + Math.min(box.height, lines.length)}/${lines.length}` : ''} · Esc closes`;
  return (
    <Pane title={title} focused width={props.width} height={props.height} onWheel={props.onWheel}>
      {lines.slice(top, top + box.height).map((l, i) =>
        l.key ? (
          <T key={i}>
            <T role="accent" bold>
              {fit(l.key, 10)}
            </T>
            {cut(l.text, Math.max(0, box.width - 10))}
          </T>
        ) : (
          <T key={i} role="muted">
            {cut(l.text, box.width) || ' '}
          </T>
        ),
      )}
    </Pane>
  );
}

export function PaletteOverlay(props: {
  width: number;
  height: number;
  readonly: boolean;
  query: string;
  selected: number;
  onRun: (a: Action) => void;
  onSelect: (i: number) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const found = searchActions(visibleActions(props.readonly), props.query);
  return (
    <Pane title="Command palette · type to search · ↵ runs · Esc closes" focused width={props.width} height={props.height}>
      <T>
        <T role="accent">{': '}</T>
        {cut(props.query, Math.max(0, box.width - 3))}
        <T inverse> </T>
      </T>
      <List
        rows={found.map((a) => ({ key: a.id, parts: [{ text: fit(a.key, 10), role: 'accent' }, { text: a.label }] }))}
        selected={Math.min(props.selected, Math.max(0, found.length - 1))}
        width={box.width}
        height={Math.max(0, box.height - 1)}
        focused
        empty="No action matches."
        onSelect={props.onSelect}
        onOpen={(i) => {
          const a = found[i];
          if (a) props.onRun(a);
        }}
      />
    </Pane>
  );
}

export interface FormState {
  field: number;
  values: [string, string, string];
  error: string;
  busy: boolean;
  /** Who approves the new run's steps; '' is the configured default. Ctrl+A switches it. */
  approvals?: ApprovalsMode;
}

/** The next approvals mode for Ctrl+A: default, manual, auto, default. */
export function nextApprovals(a: ApprovalsMode | undefined): ApprovalsMode {
  return !a ? 'manual' : a === 'manual' ? 'auto' : '';
}

export function formApprovalsLabel(a: ApprovalsMode | undefined): string {
  if (a === 'manual') return 'manual: you approve each step in the agent';
  if (a === 'auto') return 'auto: the agent approves its own steps in the sandbox';
  return 'your default ([run] approvals, else auto)';
}

export const FORM_FIELDS = ['Harness', 'Project', 'Prompt file'] as const;

export function NewRunForm(props: {
  width: number;
  height: number;
  form: FormState;
  harnesses: string[];
  onField: (i: number) => void;
  onSubmit: () => void;
  onCancel: () => void;
}): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const f = props.form;
  const hints = [
    props.harnesses.length > 0 ? `installed: ${props.harnesses.join(', ')}` : 'paddock list shows the harnesses',
    'a folder; empty is the current folder',
    'a file with the task, relative to the project or absolute',
  ];
  return (
    <Pane title="New run · in a sandbox, detached · Tab next field · Ctrl+A approvals · ↵ start · Esc cancel" focused width={props.width} height={props.height}>
      {FORM_FIELDS.map((name, i) => (
        <Box key={name} flexDirection="column">
          <Button label={`${fit(name, 12)}`} focused={f.field === i} onClick={() => props.onField(i)} />
          <T>
            {'  '}
            {cut(f.values[i] ?? '', Math.max(0, box.width - 4))}
            {f.field === i ? <T inverse> </T> : null}
          </T>
          <T role="muted">{cut(`  ${hints[i] ?? ''}`, box.width)}</T>
        </Box>
      ))}
      <T>
        {fit('Approvals', 12)}
        <T role={f.approvals === 'manual' ? 'warning' : 'normal'}>{cut(formApprovalsLabel(f.approvals), Math.max(0, box.width - 12))}</T>
      </T>
      <T role="muted">{cut('  Ctrl+A switches: default, manual, auto. The sandbox is the same.', box.width)}</T>
      <Box flexDirection="row" gap={2}>
        <Button label={f.busy ? ' Starting… ' : ' Start the run ↵ '} focused={false} role="accent" onClick={props.onSubmit} />
        <Button label=" Cancel Esc " focused={false} onClick={props.onCancel} />
      </Box>
      {f.error ? <T role="error">{cut(f.error, box.width)}</T> : null}
    </Pane>
  );
}
