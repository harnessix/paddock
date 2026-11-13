// The home screen: what `paddock` alone opens. The paddock icon, the
// version and one line of state on top, then the things to do as big
// choices, each with its key. It also holds the pages behind the choices
// (views/start.tsx) and routes their keys, so app.tsx only asks it.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Box, Text, useApp, type Key } from 'ink';
import { halfBlock, rowText, type ArtMode } from '../art.js';
import { useArt } from '../art-layer.js';
import { cells, cut, fit, oneLine } from '../clean.js';
import { useHost } from '../host.js';
import { t as tr } from '../i18n/index.js';
import { folderRows, lastAgent, type Job, type Recent } from '../launch.js';
import type { RunRow } from '../model.js';
import type { PaddockApi } from '../paddock.js';
import type { Pixel, Rgb, Sprite } from '../sprites.js';
import { Clickable, Pane, T } from '../ui.js';
import { useTheme, type Role } from '../theme.js';
import type { Harness } from '../types.js';
import { VERSION } from '../version.js';
import { AppsView, checkKey, chosenFolder, FirstRunView, isDir, newStart, settings, SettingsView, startKey, StartView, verdict, type Check, type StartData, type StartState } from './start.js';

// ---- the icon ----

const TILE: Rgb = [15, 18, 20];
const RAIL: Rgb = [127, 163, 132];
const PICKET: Rgb = [159, 184, 160];
export const ICON_W = 19;
const ICON_H = 10;

/**
 * The paddock icon (docs/assets/paddock-icon.svg) in 19 x 10 pixels: four
 * pointed pickets on two rails, sage on a dark tile with round corners.
 */
function iconPixels(): Pixel[] {
  const px: Pixel[] = [];
  for (let y = 0; y < ICON_H; y++) {
    for (let x = 0; x < ICON_W; x++) {
      const corner = (x === 0 || x === ICON_W - 1) && (y === 0 || y === ICON_H - 1);
      // Pickets at 2-4, 6-8, 10-12 and 14-16. Each has its point one pixel up.
      const inPicket = x >= 2 && x <= 16 && (x - 2) % 4 < 3;
      const tip = y === 2 && x >= 2 && x <= 16 && (x - 2) % 4 === 1;
      const body = y >= 3 && y <= 7 && inPicket;
      const rail = (y === 4 || y === 6) && x >= 1 && x <= 17;
      px.push(corner ? null : tip || body ? PICKET : rail ? RAIL : TILE);
    }
  }
  return px;
}

const ICON: Sprite = { w: ICON_W, h: ICON_H, frames: [iconPixels()] };

/**
 * The rows of the icon. With art: half blocks in the colours of the icon.
 * Without (16 colours, NO_COLOR): the fence alone as blocks, which the
 * caller may colour, with an empty row above and below.
 */
export function iconRows(mode: ArtMode): string[] {
  const rows = halfBlock(ICON, 0);
  if (mode !== 'none') return rows.map((r) => rowText(r, mode));
  const fence = (c: Rgb | null): boolean => c !== null && c !== TILE;
  return rows.map((r) =>
    r
      .map((c) => {
        const top = fence(c.fg) && c.ch === '▀';
        const bottom = c.ch === '▄' ? fence(c.fg) : fence(c.bg);
        return top && bottom ? '█' : top ? '▀' : bottom ? '▄' : ' ';
      })
      .join(''),
  );
}

// ---- the words ----

/** One line about what runs now and what waits. */
export function stateLine(runs: RunRow[], requests: number, loaded: boolean, readonly = false): string {
  if (!loaded) return tr('view.home.reading');
  const live = runs.filter((r) => r.live);
  const working = live.filter((r) => r.state === 'running').length;
  const idle = live.filter((r) => r.state === 'idle').length;
  const blocked = live.filter((r) => r.state === 'blocked' || r.state === 'stuck').length;
  const parts: string[] = [];
  if (working > 0) parts.push(tr('view.home.working', { n: working }));
  if (idle > 0) parts.push(tr('view.home.idle', { n: idle }));
  if (blocked > 0) parts.push(tr('view.home.blocked', { n: blocked }));
  if (requests > 0) parts.push(tr('view.home.requests', { n: requests }));
  if (parts.length > 0) return parts.join(' · ');
  if (live.length > 0) return tr('view.home.inSandbox', { n: live.length });
  return readonly ? tr('view.home.noRuns') : tr('view.home.noRunsStart');
}

export type ChoiceId = 'agent' | 'apps' | 'requests' | 'runs' | 'world' | 'settings' | 'help';

export interface HomeChoice {
  id: ChoiceId;
  key: string;
  title: string;
  /** What the choice does or holds now, in short lines. */
  lines: string[];
  /** One of the two large choices on top. */
  big: boolean;
  role?: Role;
}

export interface HomeFacts {
  readonly: boolean;
  /** The names of the installed agents and apps, or null while paddock reads them. */
  agents: string[] | null;
  apps: string[] | null;
  requests: number;
  live: number;
  ended: number;
}

/** The choices of the home screen. Read-only hides those that start something. */
export function homeChoices(f: HomeFacts): HomeChoice[] {
  const names = (list: string[] | null, none: string): string => (list === null ? tr('view.home.reading') : list.length > 0 ? tr('view.home.installed', { list: list.join(', ') }) : none);
  const all: HomeChoice[] = [
    { id: 'agent', key: 'n', title: tr('view.home.agent.title'), big: true, lines: [tr('view.home.agent.line'), names(f.agents, tr('view.home.agent.none'))] },
    { id: 'apps', key: 'o', title: tr('view.home.apps.title'), big: true, lines: [tr('view.home.apps.line'), names(f.apps, tr('view.home.apps.none'))] },
    {
      id: 'requests',
      key: 'a',
      title: tr('view.home.requests.title'),
      big: false,
      role: f.requests > 0 ? 'warning' : 'muted',
      lines: f.requests > 0 ? [`◆ ${tr('view.home.requests.wait', { n: f.requests })}`, tr('view.home.requests.forYou')] : [tr('view.home.requests.none')],
    },
    { id: 'runs', key: 'r', title: tr('view.home.runs.title'), big: false, lines: [tr('view.home.runs.live', { n: f.live }), tr('view.home.runs.ended', { n: f.ended })] },
    { id: 'world', key: 'w', title: tr('view.home.world.title'), big: false, lines: [tr('view.home.world.line1'), tr('view.home.world.line2')] },
    { id: 'settings', key: 's', title: tr('view.home.settings.title'), big: false, lines: [tr('view.home.settings.line1'), tr('view.home.settings.line2')] },
    { id: 'help', key: '?', title: tr('view.home.help.title'), big: false, lines: [tr('view.home.help.line')] },
  ];
  return f.readonly ? all.filter((c) => c.id !== 'agent' && c.id !== 'apps' && c.id !== 'settings') : all;
}

/** Tiles need this many columns and rows. A smaller or plain screen shows the choices as a list. */
export const TILES_MIN_COLUMNS = 80;
export const TILES_MIN_ROWS = 18;
/** The home screen is at most this wide. A wider terminal has it in the middle. */
export const MAX_COLUMNS = 104;
const HEAD = 7;
/** The promise of the home screen. */
export function tagline(): string {
  return tr('view.home.tagline');
}
/** One short line under the promise. */
export function taglineUnder(): string {
  return tr('view.home.taglineUnder');
}
const BIG_H = 6;
const SMALL_H = 5;

export interface HomeLayout {
  tiles: boolean;
  /** The choices by row, as places in the list of choices. */
  rows: number[][];
}

export function homeLayout(choices: HomeChoice[], width: number, height: number, plain: boolean): HomeLayout {
  const all = choices.map((_, i) => i);
  if (plain || width < TILES_MIN_COLUMNS || height < TILES_MIN_ROWS) return { tiles: false, rows: all.map((i) => [i]) };
  const big = all.filter((i) => choices[i]?.big);
  const small = all.filter((i) => !choices[i]?.big);
  return { tiles: true, rows: [big, small].filter((r) => r.length > 0) };
}

/** The choice that an arrow key leads to: left and right in the order, up and down to the nearest choice of the next row. */
export function moveChoice(layout: HomeLayout, sel: number, dx: number, dy: number): number {
  const count = layout.rows.reduce((n, r) => n + r.length, 0);
  if (dx !== 0) return Math.max(0, Math.min(count - 1, sel + dx));
  const r = layout.rows.findIndex((row) => row.includes(sel));
  const row = layout.rows[r];
  const next = layout.rows[r + dy];
  if (!row || !next) return sel;
  const c = row.indexOf(sel);
  return next[Math.min(next.length - 1, Math.floor(((c + 0.5) / row.length) * next.length))] as number;
}

// ---- the menu ----

function Icon(): React.JSX.Element {
  const theme = useTheme();
  const art = useArt();
  const mode: ArtMode = art.enabled ? art.mode : 'none';
  const color = mode === 'none' && !theme.mono ? { color: theme.color.success } : {};
  return (
    <Box flexDirection="column" width={ICON_W} flexShrink={0}>
      {iconRows(mode).map((r, i) => (
        <Text key={i} {...color} wrap="truncate-end">
          {r}
        </Text>
      ))}
    </Box>
  );
}

function Tile(props: { choice: HomeChoice; width: number; height: number; selected: boolean; onOpen: () => void }): React.JSX.Element {
  const c = props.choice;
  const w = Math.max(0, props.width - 4);
  return (
    <Pane title={`${props.selected ? '› ' : ''}${c.title}`} focused={props.selected} width={props.width} height={props.height} onClick={props.onOpen}>
      {/* The words fill the tile, so its key always stands on the last line. */}
      {Array.from({ length: Math.max(0, props.height - 3) }, (_, i) => (
        <T key={i} role={i === 0 ? (c.role ?? 'normal') : 'muted'} bold={i === 0 && c.role === 'warning'}>
          {cut(c.lines[i] ?? '', w) || ' '}
        </T>
      ))}
      <T role="muted">
        {`${tr('view.home.key')} `}
        <T role="accent" bold>
          {c.key}
        </T>
      </T>
    </Pane>
  );
}

export function HomeMenu(props: {
  width: number;
  height: number;
  line: string;
  /** Requests wait: the line of state stands out. */
  urgent: boolean;
  choices: HomeChoice[];
  layout: HomeLayout;
  selected: number;
  onOpen: (i: number) => void;
}): React.JSX.Element {
  const theme = useTheme();
  const { height, choices, layout } = props;
  // A wide terminal: the screen stays as wide as it reads well, in the middle.
  const width = Math.min(props.width, MAX_COLUMNS);
  const left = Math.floor((props.width - width) / 2);
  // The icon needs its five rows and the words beside it. A low or plain screen has the words alone.
  const withIcon = !theme.plain && width >= ICON_W + 24 && height >= HEAD + choices.length;
  const textW = Math.max(0, width - (withIcon ? ICON_W + 5 : 2));
  const words = (
    <Box flexDirection="column" width={textW}>
      {withIcon ? <T> </T> : null}
      <T bold>{cut(`paddock ${VERSION}`, textW)}</T>
      <T role={props.urgent ? 'warning' : 'normal'} bold={props.urgent}>
        {cut(props.line, textW)}
      </T>
      <T bold>{cut(tagline(), textW)}</T>
      <T role="muted">{cut(taglineUnder(), textW)}</T>
    </Box>
  );
  const head = withIcon ? (
    <Box flexDirection="row" height={HEAD} paddingTop={1} paddingLeft={2} flexShrink={0}>
      <Icon />
      <Box width={3} flexShrink={0} />
      {words}
    </Box>
  ) : (
    <Box flexDirection="column" height={5} paddingLeft={theme.plain ? 0 : 2} flexShrink={0}>
      {words}
    </Box>
  );
  // A high terminal: a third of the free rows above, so the choices stand near the eye.
  const used = (withIcon ? HEAD : 5) + (layout.tiles ? layout.rows.reduce((n, row) => n + (choices[row[0] as number]?.big ? BIG_H : SMALL_H), 0) : choices.length);
  const top = Math.max(0, Math.floor((height - used) / 3));
  if (layout.tiles) {
    return (
      <Box flexDirection="column" width={props.width} height={height} overflow="hidden" paddingLeft={left} paddingTop={top}>
        {head}
        {layout.rows.map((row, r) => {
          const big = choices[row[0] as number]?.big === true;
          const base = Math.floor(width / row.length);
          return (
            <Box key={r} flexDirection="row" width={width} height={big ? BIG_H : SMALL_H} flexShrink={0}>
              {row.map((i, c) => (
                <Tile
                  key={i}
                  choice={choices[i] as HomeChoice}
                  width={base + (c < width - base * row.length ? 1 : 0)}
                  height={big ? BIG_H : SMALL_H}
                  selected={i === props.selected}
                  onOpen={() => props.onOpen(i)}
                />
              ))}
            </Box>
          );
        })}
      </Box>
    );
  }
  const titleW = Math.max(...choices.map((c) => cells(c.title))) + 2;
  return (
    <Box flexDirection="column" width={props.width} height={height} overflow="hidden" paddingLeft={left} paddingTop={top}>
      {head}
      {choices.map((c, i) => {
        const on = i === props.selected;
        return (
          <Clickable key={c.id} width={width} onClick={() => props.onOpen(i)}>
            {(hovered, pointer) => (
              <Text wrap="truncate-end" inverse={pointer.pressed} underline={hovered && !on}>
                <T role={on ? 'accent' : 'normal'} bold={on}>{`${on ? '›' : ' '} ${fit(c.key, 3)}`}</T>
                <T bold={on}>{fit(c.title, titleW)}</T>
                <T role={c.role ?? 'muted'} bold={c.role === 'warning'}>
                  {cut(c.lines.join(' '), Math.max(0, width - titleW - 6))}
                </T>
              </Text>
            )}
          </Clickable>
        );
      })}
    </Box>
  );
}

// ---- the controller ----

type Page = null | 'menu' | 'agent' | 'apps' | 'settings';

export type HomeHint = [string, { id: 'help' | 'quit' | 'back' | null; text: string }];

export interface HomeDeps {
  api: PaddockApi;
  readonly: boolean;
  cwd: string;
  runs: RunRow[];
  requests: number;
  loaded: boolean;
  say: (text: string, role?: 'success' | 'error' | 'warning' | 'muted') => void;
  /** Leave the home screen for a screen of the interface. */
  go: (where: 'requests' | 'runs' | 'world' | 'help') => void;
}

export interface HomeCtl {
  /** The interface started on the home screen: Esc on a first screen comes back to it. */
  enabled: boolean;
  /** A page of the home screen is open. */
  shown: boolean;
  /** A text field of the home screen is open. */
  textMode: boolean;
  open: () => void;
  /** Leave the home screen for the screen under it. */
  close: () => void;
  /** Take a key. False: the key is not for the home screen. */
  onKey: (input: string, key: Key) => boolean;
  view: (width: number, height: number) => React.JSX.Element;
  hints: HomeHint[];
}

/** The state of the home screen and its pages. */
export function useHome(deps: HomeDeps): HomeCtl {
  const { api, readonly, cwd, say } = deps;
  const host = useHost();
  const theme = useTheme();
  const { exit } = useApp();
  const [page, setPage] = useState<Page>((host.show ?? host.home) ? 'menu' : null);
  const [sel, setSel] = useState(0);
  const [list, setList] = useState<Harness[] | null>(null);
  const [recent, setRecent] = useState<Recent[]>([]);
  const [chosen, setStart] = useState<StartState>(newStart());
  const [check, setCheck] = useState<Check>({ key: '', state: 'done' });
  const [row, setRow] = useState({ apps: 0, settings: 0 });
  const size = useRef({ width: 80, height: 22 });
  const home = process.env.HOME;

  const load = useCallback(() => {
    api.harnesses().then(
      (v) => setList(v),
      (e: unknown) => {
        setList([]);
        say(`paddock list: ${e instanceof Error ? e.message : String(e)}`, 'error');
      },
    );
    api.recent().then(
      (v) => setRecent(v),
      () => setRecent([]),
    );
  }, [api, say]);

  // The agents and the folders: read when the home screen first shows.
  const asked = useRef(false);
  useEffect(() => {
    if (page === null || asked.current) return;
    asked.current = true;
    load();
  }, [page, load]);

  // What the last command in the terminal did, once.
  const told = useRef(false);
  useEffect(() => {
    if (told.current || !host.notice) return;
    told.current = true;
    say(host.notice.text, host.notice.role);
  }, [host.notice, say]);

  const agents = useMemo(() => (list ?? []).filter((h) => h.installed !== false && !h.app), [list]);
  const apps = useMemo(() => (list === null ? null : list.filter((h) => h.app).sort((a, b) => Number(a.installed === false) - Number(b.installed === false))), [list]);
  const folders = useMemo(() => folderRows(cwd, recent), [cwd, recent]);
  // Until the user picks an agent, it is the one that ran last in this folder: also when paddock answers after the page opened.
  const start: StartState = chosen.picked ? chosen : { ...chosen, agent: lastAgent(cwd, recent, agents.map((h) => h.name)) };
  const data: StartData = { agents, folders, check, cwd, home };

  const choices = homeChoices({
    readonly,
    agents: list === null ? null : agents.map((h) => oneLine(h.name)),
    apps: apps === null ? null : apps.filter((h) => h.installed !== false).map((h) => oneLine(h.name)),
    requests: deps.requests,
    live: deps.runs.filter((r) => r.live).length,
    ended: deps.runs.filter((r) => !r.live).length,
  });
  const at = Math.min(sel, choices.length - 1);

  // paddock checks the folder for the agent before the start, a moment after the last change.
  const agentName = agents[start.agent]?.name ?? '';
  const folder = chosenFolder(start, data);
  const key = page === 'agent' ? checkKey(agentName, folder) : '';
  useEffect(() => {
    if (!key) return;
    if (!isDir(folder)) return setCheck({ key, state: 'done', explain: {} });
    setCheck({ key, state: 'loading' });
    let alive = true;
    const t = setTimeout(() => {
      api.explain(agentName, folder).then(
        (explain) => alive && setCheck({ key, state: 'done', explain }),
        (e: unknown) => alive && setCheck({ key, state: 'done', explain: { error: e instanceof Error ? e.message : String(e) } }),
      );
    }, 150);
    return () => {
      alive = false;
      clearTimeout(t);
    };
    // The key holds the agent and the folder.
  }, [api, key]);

  /** Leave the interface and run a paddock command in the terminal. The home screen comes back after it. */
  const handOver = (job: Job): void => {
    if (!host.run) return say(tr('view.home.runInTerminal', { command: `paddock ${job.args.join(' ')}` }), 'warning');
    host.run(job);
    exit();
  };

  const tryStart = (s: StartState): void => {
    const v = verdict(s, data);
    if ('wait' in v) return setStart({ ...s, starting: true, error: '' });
    if ('error' in v) return setStart({ ...s, starting: false, error: v.error, field: v.field ?? s.field });
    setStart({ ...s, starting: false, error: '' });
    handOver(v.job);
  };
  // Start was pressed during the check: go on when the answer is there.
  useEffect(() => {
    if (page === 'agent' && start.starting && check.key === key && check.state === 'done') tryStart(start);
  }, [page, start.starting, check, key]);

  const openChoice = (i: number): void => {
    const c = choices[i];
    if (!c) return;
    setSel(i);
    switch (c.id) {
      case 'agent':
        setStart(newStart());
        return setPage('agent');
      case 'apps':
        return setPage('apps');
      case 'settings':
        return setPage('settings');
      case 'help':
        return deps.go('help');
      case 'requests':
      case 'runs':
      case 'world':
        setPage(null);
        return deps.go(c.id);
    }
  };

  const openApp = (i: number): void => {
    const a = (apps ?? [])[i];
    if (!a) return;
    if (a.installed === false) return say(tr('view.home.notInstalled', { name: oneLine(a.name) }), 'warning');
    handOver({ args: ['app', 'run', a.name], title: oneLine(a.name), pause: 'on-error' });
  };
  const openSetting = (i: number): void => {
    const s = settings()[i];
    if (s) handOver(s.job);
  };

  const firstRun = page === 'agent' && list !== null && agents.length === 0;
  const textMode = page === 'agent' && !firstRun && list !== null && start.field === 'folder' && start.folder >= folders.length;

  const onKey = (input: string, k: Key): boolean => {
    if (page === null) return false;
    // Help, the palette, the mouse and quit work as on every screen.
    if (!textMode && (input === '?' || input === ':' || input === 'M' || input === 'Q' || (k.ctrl && input === 'p'))) return false;
    if (!textMode && input === 'R') {
      load();
      return false;
    }
    if (page === 'menu') {
      const layout = homeLayout(choices, size.current.width, size.current.height, theme.plain);
      const dx = k.leftArrow || input === 'h' ? -1 : k.rightArrow || input === 'l' ? 1 : 0;
      const dy = k.upArrow || input === 'k' ? -1 : k.downArrow || input === 'j' ? 1 : 0;
      if (dx !== 0 || dy !== 0) setSel(moveChoice(layout, at, layout.tiles ? dx : dx + dy, layout.tiles ? dy : 0));
      else if (k.tab) setSel((at + (k.shift ? -1 : 1) + choices.length) % choices.length);
      else if (k.return || input === ' ') openChoice(at);
      else if (k.escape || input === 'q') {
        if (host.home) exit();
        else setPage(null);
      } else {
        const i = choices.findIndex((c) => c.key === input);
        if (i >= 0) openChoice(i);
      }
      return true;
    }
    if (page === 'agent') {
      if (list === null || firstRun) {
        if (k.escape || input === 'q') setPage('menu');
        return true;
      }
      const r = startKey(start, data, input, k);
      if (r.act === 'back') setPage('menu');
      else if (r.act === 'start') tryStart(r.state);
      else if (r.state !== start) setStart(r.state);
      return true;
    }
    const n = page === 'apps' ? (apps ?? []).length : settings().length;
    const cur = Math.min(row[page], Math.max(0, n - 1));
    if (k.escape || input === 'q') setPage('menu');
    else if (k.upArrow || input === 'k') setRow({ ...row, [page]: Math.max(0, cur - 1) });
    else if (k.downArrow || input === 'j') setRow({ ...row, [page]: Math.min(Math.max(0, n - 1), cur + 1) });
    else if (k.return) {
      if (page === 'apps') openApp(cur);
      else openSetting(cur);
    }
    return true;
  };

  const view = (width: number, height: number): React.JSX.Element => {
    size.current = { width, height };
    if (page === 'agent') {
      if (list === null) return <Pane title={tr('view.home.agent.title')} focused width={width} height={height}><T role="muted">{tr('view.home.readingAgents')}</T></Pane>;
      if (firstRun) return <FirstRunView known={list} width={width} height={height} onBack={() => setPage('menu')} />;
      return <StartView state={start} data={data} width={width} height={height} onChange={setStart} onStart={() => tryStart(start)} onBack={() => setPage('menu')} />;
    }
    if (page === 'apps') return <AppsView apps={apps} selected={row.apps} width={width} height={height} onSelect={(i) => setRow({ ...row, apps: i })} onOpen={openApp} />;
    if (page === 'settings') return <SettingsView selected={row.settings} width={width} height={height} onSelect={(i) => setRow({ ...row, settings: i })} onOpen={openSetting} />;
    return (
      <HomeMenu
        width={width}
        height={height}
        line={stateLine(deps.runs, deps.requests, deps.loaded, readonly)}
        urgent={deps.requests > 0}
        choices={choices}
        layout={homeLayout(choices, width, height, theme.plain)}
        selected={at}
        onOpen={openChoice}
      />
    );
  };

  const hints: HomeHint[] =
    page === 'agent' && !firstRun
      ? [['Tab', { id: null, text: tr('view.home.hint.next') }], ['←→', { id: null, text: tr('view.home.hint.change') }], ['↵', { id: null, text: tr('view.home.hint.start') }], ['Esc', { id: null, text: tr('view.home.hint.back') }]]
      : page === 'agent'
        ? [['R', { id: null, text: tr('view.home.hint.lookAgain') }], ['Esc', { id: null, text: tr('view.home.hint.back') }], ['?', { id: 'help', text: tr('view.home.hint.help') }]]
        : page === 'apps' || page === 'settings'
          ? [['↑↓', { id: null, text: tr('view.home.hint.choose') }], ['↵', { id: null, text: page === 'apps' ? tr('view.home.hint.open') : tr('view.home.hint.run') }], ['Esc', { id: null, text: tr('view.home.hint.back') }], ['?', { id: 'help', text: tr('view.home.hint.help') }]]
          : [['←→↑↓', { id: null, text: tr('view.home.hint.move') }], ['↵', { id: null, text: tr('view.home.hint.open') }], ['?', { id: 'help', text: tr('view.home.hint.help') }], ['Q', { id: 'quit', text: tr('view.home.hint.quit') }]];

  return {
    enabled: host.home,
    shown: page !== null,
    textMode,
    open: () => {
      setPage('menu');
    },
    close: () => setPage(null),
    onKey,
    view,
    hints,
  };
}
