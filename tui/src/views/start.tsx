// The pages behind the home screen: New agent (pick an agent, a folder and
// three options, then Start), Open an app, Settings, and the first start
// with no agent installed. The state and its rules are plain functions, so
// a test checks them without a screen. The views only draw.

import React from 'react';
import { statSync } from 'node:fs';
import { basename } from 'node:path';
import { Box, type Key } from 'ink';
import { cells, cut, fit, oneLine } from '../clean.js';
import { t } from '../i18n/index.js';
import { editLine } from '../edit.js';
import { expand, INSTALL, NETWORKS, networkWord, runArgs, tilde, wrapWords, type Explain, type FolderRow, type Job } from '../launch.js';
import { useArt, SpriteView } from '../art-layer.js';
import { colorOf, spriteFor, SPRITE_H, SPRITE_W } from '../sprites.js';
import { Button, Clickable, List, Pane, T, inner } from '../ui.js';
import { useTheme, type Role } from '../theme.js';
import type { Harness } from '../types.js';

export type Field = 'agent' | 'folder' | 'wide' | 'network' | 'approvals' | 'branch';

export interface StartState {
  field: Field;
  /** The place of the agent in the installed agents. */
  agent: number;
  /** The user picked the agent. Until then it is the agent that ran last in the folder where paddock started. */
  picked: boolean;
  /** A row of the folders. One more than the last row: the typed path. */
  folder: number;
  /** The typed path. */
  path: string;
  /** The place of the level in NETWORKS. */
  network: number;
  /** null: as configured. */
  approvals: 'auto' | 'manual' | null;
  branch: boolean;
  /** Yes to a wide folder. A new agent or folder takes it back. */
  wide: boolean;
  error: string;
  /** Start was pressed while paddock still checks the folder: the start follows the answer. */
  starting: boolean;
}

export function newStart(agent = 0): StartState {
  return { field: 'agent', agent, picked: false, folder: 0, path: '', network: 0, approvals: null, branch: false, wide: false, error: '', starting: false };
}

/** What paddock says about the agent and the folder that are picked now. */
export interface Check {
  /** The agent and the folder of the answer. */
  key: string;
  state: 'loading' | 'done';
  explain?: Explain;
}

export interface StartData {
  /** The installed agents. */
  agents: Harness[];
  folders: FolderRow[];
  check: Check;
  cwd: string;
  home?: string;
}

export function checkKey(agent: string, folder: string): string {
  return agent && folder ? `${agent}\n${folder}` : '';
}

/** The folder that is picked: a row, or the typed path made absolute. '' when nothing is typed. */
export function chosenFolder(s: StartState, d: StartData): string {
  const row = d.folders[s.folder];
  return row ? row.path : expand(s.path, d.cwd, d.home);
}

/** The answer of paddock for what is picked now, or undefined while it is not there. */
function answer(s: StartState, d: StartData): Explain | undefined {
  const key = checkKey(d.agents[s.agent]?.name ?? '', chosenFolder(s, d));
  return key !== '' && d.check.key === key && d.check.state === 'done' ? d.check.explain : undefined;
}

/** Whether the folder guard wants a yes for this folder. */
export function needsYes(s: StartState, d: StartData): boolean {
  return answer(s, d)?.folder_check?.level === 'confirm';
}

/** The fields in the order of Tab. The yes to a wide folder is there only when paddock asks for it. */
export function fields(s: StartState, d: StartData): Field[] {
  return needsYes(s, d) ? ['agent', 'folder', 'wide', 'network', 'approvals', 'branch'] : ['agent', 'folder', 'network', 'approvals', 'branch'];
}

/** The approvals mode on the screen: what the user picked, else what paddock would use, else manual (the default). */
export function shownApprovals(s: StartState, d: StartData): 'auto' | 'manual' {
  if (s.approvals) return s.approvals;
  return answer(s, d)?.approvals === 'auto' ? 'auto' : 'manual';
}

/** The approvals chips: Manual first, the default. Auto is an opt-in, not a recommendation. */
export function approvalsChips(): string[] {
  return [t('view.start.chip.manual'), t('view.start.chip.auto')];
}

/** The line under the approvals chips. */
export function approvalsLine(mode: 'auto' | 'manual'): string {
  return mode === 'manual' ? t('view.start.line.manual') : t('view.start.line.auto');
}

export function isDir(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

export type Verdict = { job: Job } | { error: string; field?: Field } | { wait: true };

/** Whether Start can run now, and with what. The folder guard decides before the start, not after. */
export function verdict(s: StartState, d: StartData): Verdict {
  const agent = d.agents[s.agent];
  if (!agent) return { error: t('view.start.noAgent') };
  const folder = chosenFolder(s, d);
  if (!folder) return { error: t('view.start.typeFolder'), field: 'folder' };
  if (!isDir(folder)) return { error: t('view.start.noFolder', { folder }), field: 'folder' };
  const a = answer(s, d);
  if (!a) return { wait: true };
  const fc = a.folder_check;
  if (fc?.level === 'refuse') return { error: oneLine(fc.message || [fc.reason, fc.fix].filter(Boolean).join('. ') || t('view.start.refuse')), field: 'folder' };
  if (fc?.level === 'confirm' && !s.wide) return { error: t('view.start.sayYes'), field: 'wide' };
  const level = NETWORKS[s.network] ?? NETWORKS[0];
  const args = runArgs({ agent: agent.name, folder, network: level?.id ?? '', approvals: s.approvals, branch: s.branch, wide: s.wide && fc?.level === 'confirm' });
  return { job: { args, title: t('view.start.jobTitle', { agent: oneLine(agent.name), folder: oneLine(basename(folder)) || folder }), pause: 'on-error' } };
}

/** One key on the New agent page: the new state, and Start or Back when the key asks for it. */
export function startKey(s: StartState, d: StartData, input: string, key: Key): { state: StartState; act?: 'start' | 'back' } {
  if (key.escape) return s.starting ? { state: { ...s, starting: false } } : { state: s, act: 'back' };
  if (s.starting) return { state: s };
  if (key.return) return { state: s, act: 'start' };
  const order = fields(s, d);
  const at = Math.max(0, order.indexOf(s.field));
  if (key.tab) return { state: { ...s, field: order[(at + (key.shift ? -1 : 1) + order.length) % order.length] as Field, error: '' } };
  if (key.upArrow || key.downArrow) {
    const dir = key.upArrow ? -1 : 1;
    if (s.field === 'folder') {
      const n = s.folder + dir;
      if (n >= 0 && n <= d.folders.length) return { state: { ...s, folder: n, wide: false, error: '' } };
    }
    const to = order[at + dir];
    return { state: to ? { ...s, field: to, error: '' } : s };
  }
  if (s.field === 'folder') {
    if (s.folder < d.folders.length) return { state: s };
    const v = editLine(s.path, input, key);
    return { state: v === null || v === s.path ? s : { ...s, path: v, wide: false, error: '' } };
  }
  const step = key.leftArrow || input === 'h' ? -1 : key.rightArrow || input === 'l' || input === ' ' ? 1 : 0;
  if (step === 0) return { state: s };
  switch (s.field) {
    case 'agent':
      return { state: d.agents.length > 1 ? { ...s, agent: (s.agent + step + d.agents.length) % d.agents.length, picked: true, wide: false, error: '' } : s };
    case 'network':
      return { state: { ...s, network: (s.network + step + NETWORKS.length) % NETWORKS.length, error: '' } };
    case 'approvals':
      return { state: setApprovals(s, d, shownApprovals(s, d) === 'auto' ? 'manual' : 'auto') };
    case 'branch':
      return { state: { ...s, branch: !s.branch, error: '' } };
    case 'wide':
      return { state: { ...s, wide: !s.wide, error: '' } };
  }
}

/** Pick an approvals mode. The mode that paddock would use anyway needs no flag. */
export function setApprovals(s: StartState, d: StartData, mode: 'auto' | 'manual'): StartState {
  const configured = answer(s, d)?.approvals;
  return { ...s, approvals: configured === mode ? null : mode, error: '' };
}

export interface GuardLine {
  text: string;
  role: Role;
}

/** What the folder guard says before the start: the reason in at most two lines, then what to do in at most two. */
export function guardLines(s: StartState, d: StartData, width: number): GuardLine[] {
  const folder = chosenFolder(s, d);
  if (!folder) return [{ text: t('view.start.oneProject'), role: 'muted' }];
  if (!isDir(folder)) return [{ text: `✖ ${t('view.start.notFolder', { folder: tilde(folder, d.home) })}`, role: 'error' }];
  const a = answer(s, d);
  if (!a) return [{ text: t('view.start.checking'), role: 'muted' }];
  const fc = a.folder_check;
  const lines = (mark: string, role: Role, reason: string, fix: string): GuardLine[] => [
    ...wrapWords(`${mark} ${oneLine(reason)}`, width, 2).map((text) => ({ text, role })),
    ...wrapWords(oneLine(fix), width, 2).map((text) => ({ text, role: 'muted' as Role })),
  ];
  if (fc?.level === 'refuse') return lines('✖', 'error', fc.reason || fc.message || t('view.start.refuse'), fc.fix ?? '');
  if (fc?.level === 'confirm') return lines('⚠', 'warning', fc.reason || fc.message || t('view.start.wide'), fc.fix ?? '');
  if (fc?.level === 'confirmed') return lines('✓', 'warning', fc.confirmed_by ? t('view.start.allowedBy', { by: fc.confirmed_by, reason: fc.reason ?? '' }) : t('view.start.allowed', { reason: fc.reason ?? '' }), '');
  // explain failed for another reason: paddock run says the rest in the terminal.
  if (a.error) return lines('⚠', 'warning', a.error, '');
  if (fc?.level === 'ok') return [{ text: `✓ ${t('view.start.onlyFolder')}`, role: 'success' }];
  // A paddock without the folder guard.
  return [{ text: t('view.start.onlyFolder'), role: 'muted' }];
}

const LABELS = ['agent', 'folder', 'network', 'approvals', 'branch'] as const;

function label(id: (typeof LABELS)[number]): string {
  return t(`view.start.label.${id}`);
}

/** The column of the labels: 12 cells, or more when the words of the language are longer (at most 20). */
export function labelWidth(): number {
  return Math.max(12, Math.min(20, Math.max(...LABELS.map((id) => cells(label(id)) + 3))));
}

function Label(props: { text: string; focused: boolean; width: number; onClick: () => void }): React.JSX.Element {
  return (
    <Clickable onClick={props.onClick} width={props.width}>
      {(hovered) => (
        <T role={props.focused ? 'accent' : 'normal'} bold={props.focused} underline={hovered && !props.focused}>
          {fit(`${props.focused ? '› ' : '  '}${props.text}`, props.width)}
        </T>
      )}
    </Clickable>
  );
}

/** A row of choices. The picked one stands in brackets, so it shows without colours too. */
function Chips(props: { items: string[]; picked: number; focused: boolean; width: number; warn?: boolean; onPick: (i: number) => void }): React.JSX.Element {
  const { items, picked } = props;
  // The window of items that fits, with the picked one in it.
  let from = 0;
  let to = items.length;
  const size = (a: number, b: number): number => items.slice(a, b).reduce((n, t) => n + cells(t) + 3, 0);
  while (size(from, to) > props.width && to - from > 1) {
    if (to - 1 > picked) to--;
    else from++;
  }
  return (
    <Box flexDirection="row" height={1} overflow="hidden">
      {from > 0 ? <T role="muted">{'‹ '}</T> : null}
      {items.slice(from, to).map((text, j) => {
        const i = from + j;
        const on = i === picked;
        return (
          <Clickable key={i} onClick={() => props.onPick(i)}>
            {(hovered, pointer) => (
              <T role={on && props.warn ? 'warning' : on ? 'normal' : 'muted'} bold={on} inverse={(on && props.focused) || pointer.pressed} underline={hovered && !on}>
                {on ? `[${text}] ` : ` ${text}  `}
              </T>
            )}
          </Clickable>
        );
      })}
      {to < items.length ? <T role="muted">{'›'}</T> : null}
    </Box>
  );
}

function Check_(props: { on: boolean; text: string; focused: boolean; role?: Role; onClick: () => void }): React.JSX.Element {
  return (
    <Clickable onClick={props.onClick}>
      {(hovered, pointer) => (
        <T role={props.role ?? 'normal'} bold={props.on} inverse={props.focused || pointer.pressed} underline={hovered && !props.focused}>
          {`[${props.on ? 'x' : ' '}] ${props.text}`}
        </T>
      )}
    </Clickable>
  );
}

export function StartView(props: {
  state: StartState;
  data: StartData;
  width: number;
  height: number;
  onChange: (s: StartState) => void;
  onStart: () => void;
  onBack: () => void;
}): React.JSX.Element {
  const theme = useTheme();
  const art = useArt();
  const { state: s, data: d } = props;
  const box = inner(theme.plain, props.width, props.height);
  // The character of the picked agent stands left of the choices, where there is room for it.
  const withArt = art.enabled && !theme.plain && box.width >= SPRITE_W + 2 + 50 && box.height >= Math.ceil(SPRITE_H / 2) + 2;
  const w = box.width - (withArt ? SPRITE_W + 2 : 0);
  const LABEL = labelWidth();
  const cw = Math.max(8, w - LABEL);
  const agent = d.agents[s.agent];
  const typing = s.folder >= d.folders.length;
  const guard = guardLines(s, d, cw);
  const yes = needsYes(s, d);
  // Rows that are always there: agent 2, the typed folder 1, network 2, approvals 2, branch 2, buttons 2, the error 1.
  const room = Math.max(1, Math.min(4, box.height - 12 - guard.length - (yes ? 1 : 0)));
  const first = Math.max(0, Math.min(d.folders.length - room, typing ? d.folders.length - room : s.folder - Math.floor(room / 2)));
  const rows = d.folders.slice(first, first + room);
  const level = NETWORKS[s.network] ?? (NETWORKS[0] as (typeof NETWORKS)[number]);
  const a = answer(s, d);
  const configured = networkWord(a?.network);
  const mode = shownApprovals(s, d);
  const set = (patch: Partial<StartState>): void => props.onChange({ ...s, ...patch, error: '' });
  const indent = ' '.repeat(LABEL);
  return (
    <Pane title={t('view.start.title')} focused width={props.width} height={props.height}>
      <Box flexDirection="row" width={box.width}>
        {withArt && agent ? (
          <Box flexDirection="column" width={SPRITE_W + 2} flexShrink={0}>
            <SpriteView sprite={spriteFor(colorOf(agent.name), 'idle')} />
            <T bold>{fit(`${' '.repeat(Math.max(0, Math.floor((SPRITE_W - cells(oneLine(agent.name))) / 2)))}${oneLine(agent.name)}`, SPRITE_W)}</T>
          </Box>
        ) : null}
        <Box flexDirection="column" width={w} flexShrink={0}>
          <Box flexDirection="row">
            <Label text={label('agent')} width={LABEL} focused={s.field === 'agent'} onClick={() => set({ field: 'agent' })} />
            <Chips items={d.agents.map((x) => oneLine(x.name))} picked={s.agent} focused={s.field === 'agent'} width={cw} onPick={(i) => set({ field: 'agent', agent: i, picked: true, wide: false })} />
          </Box>
          <T role="muted">{cut(`${indent}${oneLine(agent?.description ?? '') || t('view.start.inSandbox')}`, w)}</T>
          {rows.map((r, j) => {
            const i = first + j;
            const on = i === s.folder;
            return (
              <Box key={r.path} flexDirection="row">
                {j === 0 ? <Label text={label('folder')} width={LABEL} focused={s.field === 'folder'} onClick={() => set({ field: 'folder' })} /> : <T>{indent}</T>}
                <Clickable onClick={() => set({ field: 'folder', folder: i, wide: false })}>
                  {(hovered, pointer) => (
                    <T bold={on} inverse={(on && s.field === 'folder') || pointer.pressed} underline={hovered && !on}>
                      {cut(`${on ? '●' : '○'} ${tilde(r.path, d.home)}`, Math.max(4, cw - cells(r.note) - 2))}
                    </T>
                  )}
                </Clickable>
                <T role="muted">{r.note ? `  ${r.note}` : ''}</T>
              </Box>
            );
          })}
          <Box flexDirection="row">
            {rows.length === 0 ? <Label text={label('folder')} width={LABEL} focused={s.field === 'folder'} onClick={() => set({ field: 'folder' })} /> : <T>{indent}</T>}
            <Clickable onClick={() => set({ field: 'folder', folder: d.folders.length, wide: false })}>
              {(hovered) => (
                <T bold={typing} underline={hovered && !typing}>
                  {`${typing ? '●' : '○'} ${t('view.start.otherFolder')} `}
                  <T underline={!theme.plain} role={typing ? 'normal' : 'muted'}>
                    {cut(s.path, Math.max(4, cw - cells(t('view.start.otherFolder')) - 7)) || (typing && s.field === 'folder' ? '' : cut(t('view.start.typePath'), Math.max(4, cw - cells(t('view.start.otherFolder')) - 4)))}
                  </T>
                  {typing && s.field === 'folder' ? <T inverse> </T> : null}
                </T>
              )}
            </Clickable>
          </Box>
          {guard.map((g, i) => (
            <T key={i} role={g.role} bold={g.role === 'error' || g.role === 'warning'}>
              {cut(`${indent}${g.text}`, w)}
            </T>
          ))}
          {yes ? (
            <Box flexDirection="row">
              <T>{indent}</T>
              <Check_ on={s.wide} text={t('view.start.yesWide')} focused={s.field === 'wide'} role="warning" onClick={() => set({ field: 'wide', wide: !s.wide })} />
            </Box>
          ) : null}
          <Box flexDirection="row">
            <Label text={label('network')} width={LABEL} focused={s.field === 'network'} onClick={() => set({ field: 'network' })} />
            <Chips items={NETWORKS.map((n) => n.label)} picked={s.network} focused={s.field === 'network'} width={cw} warn={level.wide} onPick={(i) => set({ field: 'network', network: i })} />
          </Box>
          <T role={level.wide ? 'warning' : 'muted'}>{cut(`${indent}${level.words}${level.id === '' && configured ? ` ${t('view.start.now', { network: configured.toLowerCase() })}` : ''}`, w)}</T>
          <Box flexDirection="row">
            <Label text={label('approvals')} width={LABEL} focused={s.field === 'approvals'} onClick={() => set({ field: 'approvals' })} />
            <Chips items={approvalsChips()} picked={mode === 'manual' ? 0 : 1} focused={s.field === 'approvals'} width={cw} onPick={(i) => props.onChange({ ...setApprovals(s, d, i === 0 ? 'manual' : 'auto'), field: 'approvals' })} />
          </Box>
          <T role="muted">{cut(`${indent}${approvalsLine(mode)}`, w)}</T>
          <Box flexDirection="row">
            <Label text={label('branch')} width={LABEL} focused={s.field === 'branch'} onClick={() => set({ field: 'branch' })} />
            <Check_ on={s.branch} text={t('view.start.newBranch')} focused={s.field === 'branch'} onClick={() => set({ field: 'branch', branch: !s.branch })} />
          </Box>
          <T role="muted">{cut(`${indent}${s.branch ? t('view.start.worktree') : t('view.start.inFolder')}`, w)}</T>
          <T> </T>
          <Box flexDirection="row" gap={2}>
            <T>{' '.repeat(LABEL - 2)}</T>
            <Button label={` ${s.starting ? t('view.start.checkingButton') : t('view.start.start')} `} focused={false} role="accent" onClick={props.onStart} />
            <Button label={` ${t('view.start.back')} `} focused={false} onClick={props.onBack} />
          </Box>
          {s.error ? <T role="error" bold>{cut(`${indent}${s.error}`, w)}</T> : null}
        </Box>
      </Box>
    </Pane>
  );
}

/** The first start: no agent is installed. Which ones paddock knows, and how to get one. */
export function firstRunLines(known: Harness[], width: number, height: number): { text: string; role: Role; bold?: boolean }[] {
  const out: { text: string; role: Role; bold?: boolean }[] = [
    { text: t('view.first.none'), role: 'normal' },
    { text: t('view.first.install'), role: 'normal' },
    { text: ' ', role: 'normal' },
  ];
  const agents = known.filter((h) => !h.app);
  const withCommand = agents.filter((h) => INSTALL[h.name] !== undefined);
  const others = agents.filter((h) => INSTALL[h.name] === undefined);
  const nameW = Math.max(8, ...withCommand.map((h) => cells(oneLine(h.name)))) + 2;
  for (const h of withCommand) {
    out.push({ text: `${fit(oneLine(h.name), nameW)}${oneLine(h.description ?? '')}`, role: 'normal', bold: true });
    out.push({ text: `${' '.repeat(nameW)}${INSTALL[h.name]}`, role: 'accent' });
  }
  if (others.length > 0) {
    if (withCommand.length > 0) out.push({ text: ' ', role: 'normal' });
    const left = Math.max(1, height - out.length - 2);
    const names = others.map((h) => oneLine(h.name)).join(', ');
    for (const text of wrapWords(t(withCommand.length > 0 ? 'view.first.also' : 'view.first.knows', { names }), width, left)) out.push({ text, role: 'muted' });
  }
  if (agents.length === 0) out.push({ text: t('view.first.noList'), role: 'warning' });
  out.push({ text: t('view.first.path'), role: 'muted' });
  out.push({ text: t('view.first.where'), role: 'muted' });
  return out;
}

export function FirstRunView(props: { known: Harness[]; width: number; height: number; onBack: () => void }): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  return (
    <Pane title={t('view.first.title')} focused width={props.width} height={props.height}>
      {firstRunLines(props.known, box.width, box.height).map((l, i) => (
        <T key={i} role={l.role} bold={l.bold}>
          {cut(l.text, box.width)}
        </T>
      ))}
    </Pane>
  );
}

export function AppsView(props: { apps: Harness[] | null; selected: number; width: number; height: number; onSelect: (i: number) => void; onOpen: (i: number) => void }): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const apps = props.apps ?? [];
  const nameW = Math.min(24, Math.max(8, ...apps.map((a) => cells(oneLine(a.name)))) + 2);
  return (
    <Pane title={t('view.apps.title')} focused width={props.width} height={props.height}>
      <T role="muted">{cut(t('view.apps.own'), box.width)}</T>
      <T role="muted">{cut(t('view.apps.folders'), box.width)}</T>
      <T> </T>
      <List
        rows={apps.map((a) => ({
          key: a.name,
          parts: [
            { text: fit(oneLine(a.name), nameW), bold: a.installed !== false },
            { text: oneLine(a.description ?? ''), role: a.installed === false ? 'muted' : 'normal' },
            { text: a.installed === false ? `  · ${t('view.apps.notInstalled')}` : '', role: 'muted' },
          ],
        }))}
        selected={Math.min(props.selected, Math.max(0, apps.length - 1))}
        width={box.width}
        height={Math.max(0, box.height - 3)}
        focused
        empty={props.apps === null ? t('view.apps.reading') : t('view.apps.none')}
        onSelect={props.onSelect}
        onOpen={props.onOpen}
      />
    </Pane>
  );
}

export interface Setting {
  title: string;
  words: string;
  job: Job;
}

/** What Settings offers: each row hands the terminal to one paddock command. */
export function settings(): Setting[] {
  return [
    { title: t('view.settings.setup'), words: t('view.settings.setupWords'), job: { args: ['setup'], title: 'paddock setup', pause: 'always' } },
    { title: t('view.settings.doctor'), words: t('view.settings.doctorWords'), job: { args: ['doctor'], title: 'paddock doctor', pause: 'always' } },
    { title: t('view.settings.help'), words: t('view.settings.helpWords'), job: { args: ['help'], title: 'paddock help', pause: 'always' } },
  ];
}

export function SettingsView(props: { selected: number; width: number; height: number; onSelect: (i: number) => void; onOpen: (i: number) => void }): React.JSX.Element {
  const theme = useTheme();
  const box = inner(theme.plain, props.width, props.height);
  const list = settings();
  const titleW = Math.max(...list.map((s) => cells(s.title))) + 2;
  return (
    <Pane title={t('view.settings.title')} focused width={props.width} height={props.height}>
      <List
        rows={list.map((s) => ({ key: s.job.args.join(' '), parts: [{ text: fit(s.title, titleW), bold: true }, { text: s.words, role: 'muted' }] }))}
        selected={props.selected}
        width={box.width}
        height={list.length}
        focused
        empty=""
        onSelect={props.onSelect}
        onOpen={props.onOpen}
      />
      <T> </T>
      {[
        t('view.settings.where'),
        t('view.settings.topic'),
        t('view.settings.here'),
      ].map((line, i) => (
        <T key={i} role="muted">
          {cut(line, box.width)}
        </T>
      ))}
    </Pane>
  );
}
