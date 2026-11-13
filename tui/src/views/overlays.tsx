import React from 'react';
import { Box } from 'ink';
import { cells, cut, fit } from '../clean.js';
import { t as tr, type Key } from '../i18n/index.js';
import { searchActions, visibleActions, type Action } from '../actions.js';
import { Button, Clickable, List, Pane, T, inner } from '../ui.js';
import { useTheme } from '../theme.js';
import type { ApprovalsMode } from '../paddock.js';

export function helpLines(readonly: boolean, mouse: boolean): { key: string; text: string }[] {
  const lines = visibleActions(readonly).map((a) => ({ key: a.key, text: a.label }));
  lines.push({ key: '', text: '' });
  for (const t of [
    tr('view.help.move'),
    tr('view.help.rooms'),
    tr('view.help.ranchMove'),
    tr('view.help.ranchJump'),
    tr('view.help.ranchList'),
    tr('view.help.amber'),
    tr('view.help.files1'),
    tr('view.help.files2'),
    tr('view.help.files3'),
    tr('view.help.diff'),
    tr('view.help.fileCard'),
    tr('view.help.card'),
    tr('view.help.wide'),
    readonly ? '' : tr('view.help.newRun'),
    tr('view.help.redraw'),
    mouse ? tr('view.help.mouseOn') : tr('view.help.mouseOff'),
    readonly ? tr('view.help.readonly') : '',
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
  const title = more ? tr('view.help.titleScroll', { at: top + Math.min(box.height, lines.length), total: lines.length }) : tr('view.help.title');
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
    <Pane title={tr('view.palette.title')} focused width={props.width} height={props.height}>
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
        empty={tr('view.palette.empty')}
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
  if (a === 'manual') return tr('view.form.approvals.manual');
  if (a === 'auto') return tr('view.form.approvals.auto');
  return tr('view.form.approvals.default');
}

/** The fields of the New run form. Their names are in the catalog: `view.form.field.ID`. */
export const FORM_FIELDS = ['harness', 'project', 'promptFile'] as const;

/** The name of a field of the form in the language of the interface. */
export function fieldName(id: (typeof FORM_FIELDS)[number] | 'approvals'): string {
  return tr(`view.form.field.${id}` as Key);
}

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
  const fieldWidth = Math.max(8, Math.min(60, box.width - 4));
  const hints = [
    props.harnesses.length > 0 ? tr('view.form.installed', { list: props.harnesses.join(', ') }) : tr('view.form.listHarnesses'),
    tr('view.form.folderHint'),
    tr('view.form.fileHint'),
  ];
  // The names take 12 cells, or more when the words of the language are longer (at most a third of the box).
  const nameW = Math.max(12, Math.min(Math.floor(box.width / 3), Math.max(...[...FORM_FIELDS, 'approvals' as const].map((id) => cells(fieldName(id)) + 2))));
  return (
    <Pane title={tr('view.form.title')} focused width={props.width} height={props.height}>
      {FORM_FIELDS.map((id, i) => (
        <Box key={id} flexDirection="column">
          <Button label={`${fit(fieldName(id), nameW)}`} focused={f.field === i} onClick={() => props.onField(i)} />
          {/* The field shows as a line to write on, so an empty one is not an empty row. */}
          <Clickable onClick={() => props.onField(i)}>
            {() => (
              <T>
                {'  '}
                <T underline={!theme.plain} role={f.field === i ? 'normal' : 'muted'}>
                  {cut(f.values[i] ?? '', fieldWidth - 1)}
                </T>
                {f.field === i ? <T inverse> </T> : null}
                <T underline={!theme.plain} role="muted">
                  {' '.repeat(Math.max(0, fieldWidth - cells(cut(f.values[i] ?? '', fieldWidth - 1)) - (f.field === i ? 1 : 0)))}
                </T>
              </T>
            )}
          </Clickable>
          <T role="muted">{cut(`  ${hints[i] ?? ''}`, box.width)}</T>
        </Box>
      ))}
      <T>
        {fit(fieldName('approvals'), nameW)}
        <T role={f.approvals === 'manual' ? 'warning' : 'normal'}>{cut(formApprovalsLabel(f.approvals), Math.max(0, box.width - nameW))}</T>
      </T>
      <T role="muted">{cut(`  ${tr('view.form.switches')}`, box.width)}</T>
      <Box flexDirection="row" gap={2}>
        <Button label={` ${f.busy ? tr('view.form.starting') : tr('view.form.start')} `} focused={false} role="accent" onClick={props.onSubmit} />
        <Button label={` ${tr('view.form.cancel')} `} focused={false} onClick={props.onCancel} />
      </Box>
      {f.error ? <T role="error">{cut(f.error, box.width)}</T> : null}
    </Pane>
  );
}
